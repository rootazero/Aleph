# Web 搜索面 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 `SearchOptions` 里五个零写入点的字段接到模型够得到的工具面上，让接不上的维度出声而不是静默消失，并消灭"哪些后端存在"的三份清单。

**Architecture:** 四层，自下而上：① `Recency` 枚举成为四词表的唯一所有者；② `SearchCapabilities` 让每个 provider 自陈支持哪些维度，由源码级守卫钉住"声明"与"真的调了映射器"一致；③ registry 拿它当**稳定排序键**（不是闸），并修掉"空结果终止链"；④ 工具面暴露参数、保真结果、加上下文经济。横向再做两件熵减：收敛遗留 Tavily 直连路径、把三份 provider 清单收敛成一个 protocol 常量。

**Tech Stack:** Rust (tokio + serde + schemars + async-trait)、Leptos/WASM (Panel)、Python (QA mock)、bash (QA 驱动)。

**Spec:** `docs/superpowers/specs/2026-08-30-web-search-face-design.md`（提交于 `d0f623311`）

## REQUIRED PREREQUISITE — worktree 隔离

用户协议：**严禁在 main 上做实现**。开工前用 `superpowers:using-git-worktrees` 建 worktree，分支名 `web-search-face`，从当前 main（`d0f623311` 或更新）切。

⚠️ **本仓 worktree 三条已知坑**（都栽过）：
1. 根 `.cargo/config.toml` 把 target 目录钉成绝对路径，**多个 worktree 共享它** ⇒ 上一棵树构建的二进制会被这棵树跑。跑真机 QA 前 `ls -l` 看一眼二进制 mtime。
2. `skills/` 与 `plugins/` 是 submodule，新 worktree 里**是空的**，而 `include_dir!` 是编译期宏 ⇒ 需要 `git submodule update --init --recursive`，否则编译失败。
3. 同会话内 `git worktree remove` 会损坏 Shell —— **只合并不删除**。

## Global Constraints

- **红线 R1**：`src/` 不直接调平台 API。本计划不涉及。
- **红线 R10**：不往 `src/harness/` 加任何东西。本计划不涉及 harness。
- **R10 零消费者优先 CUT**：任何本计划新增的抽象，如果落地后指不出消费者，撤回而不是留着。
- **MSRV 1.95**，工具链由 `rust-toolchain.toml` 钉住（当前 1.96.0），**不要** `cargo +version`。
- **提交信息**：英文，`<scope>: <description>`，正文解释"为什么"，**不加署名行**（仓库惯例，`git log` 可核）。
- **源码级守卫必须先 `.replace('\r', "")`**：本仓 Windows 检出是 CRLF，`\n` 锚定的分隔符在那里永不匹配（判据 D.10.9）。
- **源码级守卫必须剥 `#[cfg(test)]`**：用 `crate::utils::source_scan::production_prefix`，否则守卫会把自己的断言字符串当成命中（判据 D.10.24）。
- **每条新守卫写完必须手动变异一次证明它会红**，并确认**红的是预期的那一条**（判据 附录 C.5）。
- **最小验证集**（判据 §10，六条，不是一条）：
  ```
  cargo test -p alephcore --lib --no-run
  cargo test -p alephcore --bins
  cargo test -p alephcore --features test-helpers --test '*' --no-run
  cargo test -p aleph-panel --lib --no-run
  cargo check -p aleph-desktop-{macos,windows,linux}
  cargo clippy --workspace --all-targets     # 先 just _stage-shell-placeholders
  ```
  改了 `interfaces/webchat/` 还要 `just wasm`（唯一编译出厂形态的命令）。
- **绝不用 `cargo fmt -p <crate> -- <file>`**（它格式化整个 crate）也**不用 `rustfmt <file>`**（它顺着 `mod` 递归进子模块）。单文件格式化只有两条：`rustfmt --config skip_children=true <file>`，或格式化后比对 `git diff --name-only` 的前后差集把多余的 `git checkout --` 回去。

---

### Task 1: `Recency` 枚举成为四词表的唯一所有者

今天 `date_range: Option<String>`，七个映射器各写一份 `day|week|month|year` 的 match 并以 `_ => return None` **静默丢弃**未知值。字段全仓零写入点（spec E1），所以改名 + 改类型**没有存量调用者可破坏**。

**Files:**
- Modify: `src/search/options.rs`（struct 字段 · `Default` · 七个映射器 · 模块文档表格）
- Modify: `src/search/providers/{brave,bing,google,searxng,tavily,duckduckgo,firecrawl}.rs`（仅当映射器签名变化影响调用点；本任务保持返回类型不变，预期零改动——**跑一遍确认**）
- Test: `src/search/options.rs` 的 `#[cfg(test)] mod tests`

**Interfaces:**
- Produces: `pub enum Recency { Day, Week, Month, Year }`（`src/search/options.rs`，经 `src/search/mod.rs` re-export 为 `crate::search::Recency`）；`SearchOptions.recency: Option<Recency>`（原 `date_range`）。
- Consumes: 无。

- [ ] **Step 1: 写失败测试**

在 `src/search/options.rs` 的 tests 模块加：

```rust
#[test]
fn recency_maps_to_every_provider_vocabulary() {
    use Recency::{Day, Month, Week, Year};
    let cases = [
        (Day, "pd", "Day", "d1", "day", 1u32, "d", "qdr:d"),
        (Week, "pw", "Week", "w1", "week", 7, "w", "qdr:w"),
        (Month, "pm", "Month", "m1", "month", 30, "m", "qdr:m"),
        (Year, "py", "Year", "y1", "year", 365, "y", "qdr:y"),
    ];
    for (r, brave, bing, google, searxng, tavily, ddg, firecrawl) in cases {
        let o = SearchOptions {
            recency: Some(r),
            ..Default::default()
        };
        assert_eq!(o.brave_freshness(), Some(brave), "{r:?}");
        assert_eq!(o.bing_freshness(), Some(bing), "{r:?}");
        assert_eq!(o.google_date_restrict(), Some(google), "{r:?}");
        assert_eq!(o.searxng_time_range(), Some(searxng), "{r:?}");
        assert_eq!(o.tavily_days(), Some(tavily), "{r:?}");
        assert_eq!(o.ddg_df(), Some(ddg), "{r:?}");
        assert_eq!(o.firecrawl_tbs(), Some(firecrawl), "{r:?}");
    }
}

/// The whole point of the enum: a value outside the four-word table is
/// rejected at the edge instead of being dropped by seven mappers, each of
/// which used to answer `None` and let the caller believe it had constrained
/// the search.
#[test]
fn an_unknown_recency_string_is_rejected_not_dropped() {
    let err = serde_json::from_value::<Recency>(serde_json::json!("7d")).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("day"), "the error must list the legal values: {msg}");
}

#[test]
fn no_recency_means_no_provider_parameter() {
    let o = SearchOptions::default();
    assert_eq!(o.brave_freshness(), None);
    assert_eq!(o.tavily_days(), None);
    assert_eq!(o.firecrawl_tbs(), None);
}
```

⚠️ 上面 `bing`/`firecrawl` 那两列的期望值是**从现有代码抄的**，不是记忆：写测试前先 `sed -n '144,160p;254,268p' src/search/options.rs` 把 `bing_freshness` 与 `firecrawl_tbs` 的真实返回值抄下来，如与本表不符**以代码为准并改本表**。

- [ ] **Step 2: 跑测试确认它失败**

Run: `cargo test -p alephcore --lib search::options -- --nocapture`
Expected: 编译失败 —— `Recency` 未定义、`SearchOptions` 无 `recency` 字段。

- [ ] **Step 3: 实现**

在 `src/search/options.rs` 顶部（`SearchOptions` 定义之前）加：

```rust
/// The freshness vocabulary, owned in one place.
///
/// Every provider has its own spelling for the same four buckets (`pd` /
/// `Day` / `d1` / `qdr:d` / 1 day / ...). Before this enum the four words
/// lived seven times over — once inside each mapper — and every mapper
/// answered `None` for anything it did not recognise, so a caller passing
/// `"7d"` got an unconstrained search while believing it had constrained one.
/// With a closed enum the rejection happens at the tool boundary and names
/// the legal values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Recency {
    Day,
    Week,
    Month,
    Year,
}
```

改字段：

```rust
    /// How fresh a result has to be. `None` = no constraint.
    /// Forwarded to Brave/Bing/Google/SearXNG/Tavily/DDG/Firecrawl, each in
    /// its own vocabulary — see the mapping table above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recency: Option<Recency>,
```

七个映射器逐个从 `self.date_range.as_deref()?` 改成 `self.recency?`，match 臂改成枚举变体，**删掉 `_ => return None` 那一臂**（枚举穷尽，编译器现在替你保证覆盖）。例：

```rust
    /// Brave `freshness` (`pd`/`pw`/`pm`/`py`).
    #[must_use]
    pub fn brave_freshness(&self) -> Option<&'static str> {
        Some(match self.recency? {
            Recency::Day => "pd",
            Recency::Week => "pw",
            Recency::Month => "pm",
            Recency::Year => "py",
        })
    }
```

同步改 `Default::default()` 里的 `date_range: None` → `recency: None`，以及模块文档表格里的行名 `date_range` → `recency`。

在 `src/search/mod.rs` 的 re-export 行加上 `Recency`：

```rust
pub use options::{Recency, SearchOptions};
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p alephcore --lib search::options`
Expected: PASS。再跑 `cargo test -p alephcore --lib --no-run` 确认没有别处引用旧字段名。

- [ ] **Step 5: 变异一次证明穷尽性真的生效**

临时把 `Recency` 加一个变体 `Hour`，`cargo check -p alephcore` **必须**在七个映射器上各报一次 non-exhaustive match。确认后撤销。这一步证明的是"以后加桶不会静默漏一个 provider"——即这次改造买到的东西。

- [ ] **Step 6: 提交**

```bash
git add src/search/options.rs src/search/mod.rs
git commit -m "search: give the freshness vocabulary one owner

date_range was an Option<String> parsed seven times over, once inside each
provider mapper, and every one of them ended in \`_ => return None\`. A caller
passing \"7d\" therefore got a search with no date constraint at all while
believing it had asked for one — a no-op that reported success.

Recency is a closed enum, so the rejection happens where the value enters and
names the four legal buckets, and adding a fifth bucket becomes a compile
error in all seven mappers instead of a silent None.

The field is renamed date_range -> recency in the same commit: the tool-face
parameter this is about to feed is called \`recency\`, and one thing with two
names is the shape half this repo's criteria list is about. No caller breaks —
the field had zero writers anywhere in the tree."
```

---

### Task 2: `SearchCapabilities` + 源码级守卫

能力位是**排序键**（spec §3），不是闸。默认全 `false`——一个忘了声明的新 provider 不会假装支持。守卫钉的是"声明的位"与"源码里真的调了映射器"一致，因为**有 `fn` 不等于有能力**（`ChannelCapabilities` 上按"有没有 `async fn edit`"判断，四个通道全判反）。

**Files:**
- Modify: `src/search/provider.rs`（`SearchCapabilities` + trait 默认方法）
- Modify: `src/search/mod.rs`（re-export）
- Modify: `src/search/providers/{tavily,brave,bing,google,searxng,duckduckgo,firecrawl,exa,jina}.rs`（各加 `capabilities()`）
- Create: `src/search/providers/capability_census.rs`（守卫）
- Modify: `src/search/providers/mod.rs`（挂 `#[cfg(test)] mod capability_census;`）

**Interfaces:**
- Consumes: Task 1 的 `Recency`（守卫要认 `self.recency`）。
- Produces: `pub struct SearchCapabilities { pub domain_filter: bool, pub recency: bool, pub full_content: bool }`；`SearchProvider::capabilities(&self) -> SearchCapabilities`（默认全 false）。

- [ ] **Step 1: 写失败测试（守卫本身就是测试）**

创建 `src/search/providers/capability_census.rs`：

```rust
//! Source-level census: a provider's declared `SearchCapabilities` must match
//! the parameters its request builder actually sends.
//!
//! # Why this cannot be a runtime test
//!
//! A capability bit is a *promise about the wire*. At runtime the only way to
//! check it is to send a request and inspect it, which means an HTTP mock per
//! provider — and seven of the nine providers hardcode their endpoint, so
//! there is nowhere to point the mock. The source is the only place where all
//! nine can be asked the same question.
//!
//! # Why it is not a hand-written table
//!
//! A list of "who supports what" written here would be a second statement of
//! a fact the code already owns, and the two would drift (D.0.37: a guard
//! that enumerates its own inputs is structurally blind to whatever it did
//! not enumerate). Both sides are derived from source instead:
//!
//! * the accessor names are derived from `options.rs` — any fn whose body
//!   reads `self.recency` is a recency accessor, by construction;
//!ate provider's declaration is parsed out of its own `capabilities()`.

use crate::utils::source_scan::production_prefix;
use std::collections::{BTreeMap, BTreeSet};

const OPTIONS_SRC: &str = include_str!("../options.rs");

/// Which `SearchOptions` member a dimension is expressed through.
const DIMENSIONS: &[(&str, &[&str])] = &[
    ("recency", &["self.recency"]),
    ("full_content", &["self.include_full_content"]),
    ("domain_filter", &["self.include_domains", "self.exclude_domains"]),
];

/// Every provider source file, keyed by provider name.
fn provider_sources() -> BTreeMap<&'static str, &'static str> {
    // include_str! needs literals, so this list is explicit — but the
    // self-assertion below pins its length against the directory listing.
    BTreeMap::from([
        ("bing", include_str!("bing.rs")),
        ("brave", include_str!("brave.rs")),
        ("duckduckgo", include_str!("duckduckgo.rs")),
        ("exa", include_str!("exa.rs")),
        ("firecrawl", include_str!("firecrawl.rs")),
        ("google", include_str!("google.rs")),
        ("jina", include_str!("jina.rs")),
        ("searxng", include_str!("searxng.rs")),
        ("tavily", include_str!("tavily.rs")),
    ])
}

/// Accessor fn names in `options.rs` whose body reads one of `members`.
fn accessors_reading(members: &[&str]) -> BTreeSet<String> {
    let src = production_prefix(&OPTIONS_SRC.replace('\r', ""));
    let mut current: Option<String> = None;
    let mut found = BTreeSet::new();
    for line in src.lines() {
        let t = line.trim_start();
        if let Some(rest) = t
            .strip_prefix("pub fn ")
            .or_else(|| t.strip_prefix("pub const fn "))
        {
            current = rest.split('(').next().map(str::to_string);
        }
        if members.iter().any(|m| line.contains(m)) {
            if let Some(name) = &current {
                found.insert(name.clone());
            }
        }
    }
    found
}

/// The literal value of one field inside this file's `capabilities()` body.
fn declared_bit(src: &str, field: &str) -> bool {
    let src = production_prefix(&src.replace('\r', ""));
    let Some(start) = src.find("fn capabilities(") else {
        return false; // no override => trait default => all false
    };
    let body = &src[start..];
    let end = body.find("\n    }").map_or(body.len(), |i| i);
    body[..end].contains(&format!("{field}: true"))
}

#[test]
fn the_census_sees_every_provider_file() {
    let files: BTreeSet<String> = std::fs::read_dir(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/search/providers"),
    )
    .expect("providers dir")
    .filter_map(|e| e.ok())
    .filter_map(|e| e.file_name().into_string().ok())
    .filter(|n| n.ends_with(".rs"))
    .filter(|n| !matches!(n.as_str(), "mod.rs" | "base.rs" | "capability_census.rs"))
    .map(|n| n.trim_end_matches(".rs").to_string())
    .collect();
    let known: BTreeSet<String> = provider_sources().keys().map(|k| (*k).to_string()).collect();
    assert_eq!(
        files, known,
        "a provider file appeared or vanished without the census being told"
    );
}

#[test]
fn every_declared_capability_is_backed_by_a_parameter_that_is_actually_sent() {
    let mut checked = 0usize;
    for (dim, members) in DIMENSIONS {
        let accessors = accessors_reading(members);
        for (name, src) in provider_sources() {
            let prod = production_prefix(&src.replace('\r', ""));
            let uses = accessors.iter().any(|a| prod.contains(&format!("{a}(")))
                || members
                    .iter()
                    .any(|m| prod.contains(&m.replace("self.", "options.")));
            let declared = declared_bit(src, dim);
            assert_eq!(
                declared, uses,
                "provider `{name}` declares {dim}={declared} but its request builder \
                 {} the parameter. A capability bit is a promise about the wire: \
                 declaring one you do not send makes the registry route requests to \
                 you that you will silently drop; not declaring one you do send hides \
                 you from requests you could have answered.",
                if uses { "does send" } else { "never sends" }
            );
            checked += 1;
        }
    }
    assert_eq!(
        checked,
        DIMENSIONS.len() * 9,
        "the census must compare every dimension against every provider"
    );
}
```

在 `src/search/providers/mod.rs` 末尾加：

```rust
#[cfg(test)]
mod capability_census;
```

- [ ] **Step 2: 跑测试确认它失败**

Run: `cargo test -p alephcore --lib search::providers::capability_census`
Expected: FAIL —— `SearchCapabilities` / `capabilities()` 还不存在，编译失败。

- [ ] **Step 3: 实现**

`src/search/provider.rs`：

```rust
/// What a provider can express on the wire.
///
/// A bit here is a promise: the registry uses it as a **sorting key** (spec
/// §3) — a provider that claims `domain_filter` gets the requests that ask
/// for one. Claiming a parameter you do not send therefore does not fail
/// loudly, it silently widens somebody's search. `capability_census.rs`
/// compares each bit against the request builder that would have to send it.
///
/// The default is all-`false` on purpose: a new provider that forgets to
/// declare anything is invisible to dimension-aware routing, which is the
/// safe direction. The reverse default would make it claim everything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SearchCapabilities {
    /// Accepts an include/exclude domain list.
    pub domain_filter: bool,
    /// Accepts a freshness constraint (`SearchOptions::recency`).
    pub recency: bool,
    /// Can return page bodies, not just snippets.
    pub full_content: bool,
}
```

trait 加默认方法：

```rust
    /// What this provider can express. Default: nothing — see
    /// [`SearchCapabilities`] for why the default is not "everything".
    fn capabilities(&self) -> SearchCapabilities {
        SearchCapabilities::default()
    }
```

`src/search/mod.rs` re-export：`pub use provider::{SearchCapabilities, SearchProvider};`

九个 provider 各加一个 `capabilities()`。**起始值从代码读，不从记忆写**——先跑一遍守卫，让它告诉你每个 provider 该是什么，再填。预期（spec §3 的表）：

```rust
// tavily.rs
    fn capabilities(&self) -> SearchCapabilities {
        SearchCapabilities {
            domain_filter: false, // Task 3 flips this
            recency: true,        // tavily_days -> `days`
            full_content: true,   // include_raw_content
        }
    }
```

```rust
// brave.rs / bing.rs / google.rs / searxng.rs / duckduckgo.rs
    fn capabilities(&self) -> SearchCapabilities {
        SearchCapabilities {
            domain_filter: false,
            recency: true,
            full_content: false,
        }
    }
```

```rust
// firecrawl.rs
    fn capabilities(&self) -> SearchCapabilities {
        SearchCapabilities {
            domain_filter: false,
            recency: true,
            full_content: true,
        }
    }
```

`exa.rs` / `jina.rs` **不覆写**（trait 默认全 false 就是它们今天的真相）。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p alephcore --lib search::providers::capability_census -- --nocapture`
Expected: 两条都 PASS。

- [ ] **Step 5: 变异守卫，确认它会红且红在对的地方**

把 `exa.rs` 加一个 `capabilities()` 声明 `recency: true`（它不调任何 recency 映射器）。跑守卫，**必须**红且失败信息点名 `exa` 与 `recency`。撤销。
再反向变异一次：把 `tavily.rs` 的 `recency` 改成 `false`，**必须**红且点名 tavily。撤销。
⚠️ 两个方向都要试——只试一个方向的守卫只证明了它认得一半。

- [ ] **Step 6: 提交**

```bash
git add src/search/provider.rs src/search/mod.rs src/search/providers/
git commit -m "search: let each provider declare what it can express, and pin the claim to the wire

SearchCapabilities is a sorting key, not a gate (design doc section 3): the
registry prefers a provider that can carry the dimension a caller asked for,
and says so when nobody can. That makes a wrong bit dangerous in a quiet way
— a provider claiming domain_filter it does not send widens somebody's search
without failing.

So the bit is pinned to the request builder in source. Both sides of the
comparison are derived rather than listed: the accessor names come from
whichever fn in options.rs reads the member, and the declaration is parsed
out of the provider's own capabilities(). A hand-written table of who
supports what would be a second statement of a fact the code already owns.

The starting values are read out of today's code, not from memory: seven
providers already translate recency into a native parameter, two return page
bodies, and nobody accepts a domain list yet."
```

---

### Task 3: 域名过滤（字段 + Tavily/Exa 原生参数）

**Files:**
- Modify: `src/search/options.rs`（两个字段 + `Default`）
- Modify: `src/search/providers/tavily.rs`（`TavilyRequest` 两字段）
- Modify: `src/search/providers/exa.rs`（请求体两字段）
- Test: 各自文件的 tests 模块

**Interfaces:**
- Consumes: Task 2 的 `SearchCapabilities`。
- Produces: `SearchOptions.include_domains: Vec<String>` / `.exclude_domains: Vec<String>`（空 = 不约束）。

- [ ] **Step 1: 写失败测试**

`src/search/options.rs` tests：

```rust
#[test]
fn domain_lists_default_to_empty_which_means_no_constraint() {
    let o = SearchOptions::default();
    assert!(o.include_domains.is_empty());
    assert!(o.exclude_domains.is_empty());
}
```

`src/search/providers/tavily.rs` tests（请求体是 `Serialize`，直接断言序列化结果——不需要 HTTP）：

```rust
#[test]
fn domain_lists_reach_the_tavily_request_body() {
    let o = SearchOptions {
        include_domains: vec!["github.com".into()],
        exclude_domains: vec!["pinterest.com".into()],
        ..Default::default()
    };
    let body = TavilyProvider::build_request("k", "q", &o);
    let v = serde_json::to_value(&body).unwrap();
    assert_eq!(v["include_domains"], serde_json::json!(["github.com"]));
    assert_eq!(v["exclude_domains"], serde_json::json!(["pinterest.com"]));
}

/// An empty list must not appear on the wire at all: Tavily treats an empty
/// include list as "no results anywhere", not as "no constraint".
#[test]
fn empty_domain_lists_are_omitted_entirely() {
    let body = TavilyProvider::build_request("k", "q", &SearchOptions::default());
    let v = serde_json::to_value(&body).unwrap();
    assert!(v.get("include_domains").is_none(), "{v}");
    assert!(v.get("exclude_domains").is_none(), "{v}");
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p alephcore --lib search::providers::tavily`
Expected: FAIL —— `build_request` 不存在、`include_domains` 字段不存在。

- [ ] **Step 3: 实现**

`options.rs` 加两个字段（并加进 `Default`）：

```rust
    /// Restrict results to these domains. Empty = no constraint.
    ///
    /// Not every backend has a native parameter for this; the ones that do
    /// declare `SearchCapabilities::domain_filter` and the registry prefers
    /// them. A backend that cannot express it says so in the result notes
    /// rather than silently returning the whole web.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include_domains: Vec<String>,

    /// Drop results from these domains. Empty = no constraint.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude_domains: Vec<String>,
```

`tavily.rs`：把请求体构造抽成一个**纯函数**（这样测试不需要 HTTP，也不需要 API key）：

```rust
impl TavilyProvider {
    /// Build the request body. Split out of `search` so the wire shape can be
    /// asserted without an HTTP round trip — the parameter names are a
    /// contract with Tavily and "it looked right" is how the fill_form / 
    /// wait_for key mismatches in the browser layer shipped.
    fn build_request(api_key: &str, query: &str, options: &SearchOptions) -> TavilyRequest {
        TavilyRequest {
            api_key: api_key.to_string(),
            query: query.to_string(),
            search_depth: if options.include_full_content { "advanced".into() } else { "basic".into() },
            include_answer: false,
            max_results: options.validated_max_results(),
            include_raw_content: options.include_full_content.then_some(true),
            days: options.tavily_days(),
            include_domains: options.include_domains.clone(),
            exclude_domains: options.exclude_domains.clone(),
        }
    }
}
```

`TavilyRequest` 加两字段：

```rust
    #[serde(skip_serializing_if = "Vec::is_empty")]
    include_domains: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    exclude_domains: Vec<String>,
```

`search()` 改为调 `Self::build_request(&self.api_key, query, options)`。

`exa.rs` 同形，Exa 的字段名是 `includeDomains` / `excludeDomains`（camelCase）——**动手前先读 `exa.rs` 现有请求体是怎么 rename 的**（它可能已经有 `#[serde(rename_all = "camelCase")]`），照它的既有约定写。

两个 provider 的 `capabilities()` 把 `domain_filter` 翻成 `true`。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p alephcore --lib search::providers`
Expected: PASS，且 **Task 2 的守卫仍然绿**（它现在应该认可 tavily/exa 的 `domain_filter: true`）。

- [ ] **Step 5: 提交**

```bash
git add src/search/options.rs src/search/providers/tavily.rs src/search/providers/exa.rs
git commit -m "search: accept an include/exclude domain list where the backend has one

Two providers have a native parameter for it; the other seven do not, and
this commit deliberately does not fake one by folding site: operators into
the query. That would be a second answer to 'how do I restrict domains',
it would rewrite the query the caller can see, and multiple domains would
need hand-built OR groups. The capability bit plus the registry's ordering
covers the same ground without any of that.

The request body is built by a pure function so the wire shape is asserted
without an HTTP round trip: a parameter name is a contract with the vendor,
and 'this key reads sensible' is exactly how the browser layer shipped a
verb that argued with the server's schema on every call."
```

---

### Task 4: registry —— 能力感知的稳定排序

**Files:**
- Modify: `src/search/registry.rs`
- Test: 同文件 tests 模块

**Interfaces:**
- Consumes: Task 2 的 `SearchCapabilities`。
- Produces: `SearchRegistry` 内部私有 `fn ordered_candidates(&self, options: &SearchOptions) -> Vec<String>`（供本文件测试用，不 pub）。

- [ ] **Step 1: 写失败测试**

```rust
#[tokio::test]
async fn a_provider_that_can_carry_the_requested_dimension_goes_first() {
    let mut reg = SearchRegistry::new("plain");
    reg.add_provider("plain".into(), Arc::new(MockProvider::new("plain", false, 1)));
    reg.add_provider(
        "rich".into(),
        Arc::new(MockProvider::new("rich", false, 1).with_domain_filter()),
    );
    reg.set_fallback_providers(vec!["rich".into()]);

    let opts = SearchOptions {
        include_domains: vec!["github.com".into()],
        ..Default::default()
    };
    assert_eq!(reg.ordered_candidates(&opts), vec!["rich", "plain"]);

    // No dimension requested => configuration order is untouched.
    assert_eq!(
        reg.ordered_candidates(&SearchOptions::default()),
        vec!["plain", "rich"]
    );
}

/// Stable within a group: two providers that both satisfy (or both fail to
/// satisfy) the request keep configuration order, so the same query lands on
/// the same backend every time. Non-determinism here would make a cached or
/// rate-limited backend impossible to reason about.
#[tokio::test]
async fn ordering_is_stable_within_a_capability_group() {
    let mut reg = SearchRegistry::new("a");
    for n in ["a", "b", "c"] {
        reg.add_provider(n.into(), Arc::new(MockProvider::new(n, false, 1)));
    }
    reg.set_fallback_providers(vec!["b".into(), "c".into()]);
    for _ in 0..20 {
        assert_eq!(reg.ordered_candidates(&SearchOptions::default()), vec!["a", "b", "c"]);
    }
}
```

`MockProvider` 需要一个 `with_domain_filter()` builder 与 `capabilities()` 覆写——加在同文件既有的 mock 上。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p alephcore --lib search::registry`
Expected: FAIL —— `ordered_candidates` 不存在。

- [ ] **Step 3: 实现**

```rust
impl SearchRegistry {
    /// Which dimensions this request actually asks for.
    fn requested(options: &SearchOptions) -> SearchCapabilities {
        SearchCapabilities {
            domain_filter: !options.include_domains.is_empty()
                || !options.exclude_domains.is_empty(),
            recency: options.recency.is_some(),
            full_content: options.include_full_content,
        }
    }

    /// Default first, then fallbacks in configuration order, then stably
    /// reordered so providers that can carry every requested dimension come
    /// first.
    ///
    /// Stable on purpose: within a group the configured order survives, so the
    /// same query reaches the same backend on every call. An unstable sort
    /// would trade that for nothing anyone asked for.
    fn ordered_candidates(&self, options: &SearchOptions) -> Vec<String> {
        let want = Self::requested(options);
        let mut names: Vec<String> = std::iter::once(self.default_provider.clone())
            .chain(self.fallback_providers.iter().cloned())
            .filter(|n| self.providers.contains_key(n))
            .collect();
        names.dedup();
        names.sort_by_key(|n| {
            let have = self.providers[n].capabilities();
            let satisfies = (!want.domain_filter || have.domain_filter)
                && (!want.recency || have.recency)
                && (!want.full_content || have.full_content);
            usize::from(!satisfies)
        });
        names
    }
}
```

⚠️ `sort_by_key` 在 Rust 里是**稳定**排序——这正是上面第二条测试钉住的性质，别换成 `sort_unstable_by_key`。

改 `search()` 用 `ordered_candidates` 迭代，取代现有的"default 一段 + fallback 一段"两段重复代码（熵减：两段各写一遍 `is_available` 检查与错误记录）。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p alephcore --lib search::registry`
Expected: 全部 PASS，含既有的 7 条 fallback 测试。

- [ ] **Step 5: 提交**

```bash
git add src/search/registry.rs
git commit -m "search: prefer a backend that can carry the dimensions the caller asked for

Ordering, not gating: a request naming domains still runs when nobody can
express them, it just runs on the same backend it would have used anyway and
says so downstream. Refusing outright would turn an optional narrowing
parameter into a way to get zero results out of a search that would otherwise
have answered.

The sort is stable so configuration order survives inside a capability group
and the same query keeps landing on the same backend.

The two hand-unrolled loops (default, then fallbacks) collapse into one pass
over the ordered list; the availability check and the error bookkeeping were
written twice."
```

---

### Task 5: registry —— 空结果继续问下一个后端

**Files:**
- Modify: `src/search/registry.rs`
- Test: 同文件

- [ ] **Step 1: 写失败测试**

```rust
/// A backend answering "zero results" is answering, but it is not an answer
/// worth ending the chain on: the whole point of a fallback list is that the
/// backends disagree about what exists. Before this, a default provider that
/// returned an empty list stopped eight others and the SERP fallback from
/// ever being asked.
#[tokio::test]
async fn an_empty_result_set_does_not_end_the_chain() {
    let mut reg = SearchRegistry::new("empty");
    reg.add_provider("empty".into(), Arc::new(MockProvider::new("empty", false, 0)));
    reg.add_provider("full".into(), Arc::new(MockProvider::new("full", false, 3)));
    reg.set_fallback_providers(vec!["full".into()]);

    let out = reg.search("q", &SearchOptions::default()).await.unwrap();
    assert_eq!(out.len(), 3, "the chain must continue past a zero-result answer");
}

/// All empty is still a legitimate answer — an empty Ok, never an Err.
/// Folding "nobody found anything" into an error would make the model retry
/// a question that was answered.
#[tokio::test]
async fn all_backends_empty_returns_an_empty_ok() {
    let mut reg = SearchRegistry::new("a");
    reg.add_provider("a".into(), Arc::new(MockProvider::new("a", false, 0)));
    reg.add_provider("b".into(), Arc::new(MockProvider::new("b", false, 0)));
    reg.set_fallback_providers(vec!["b".into()]);
    let out = reg.search("q", &SearchOptions::default()).await.unwrap();
    assert!(out.is_empty());
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p alephcore --lib search::registry::tests::an_empty_result_set`
Expected: FAIL —— 返回 0 条（链在第一个 provider 就停了）。

- [ ] **Step 3: 实现**

在 `search()` 的成功臂里区分空与非空：

```rust
                match provider.search(query, options).await {
                    Ok(results) if !results.is_empty() => return Ok(results),
                    Ok(_) => {
                        // Answering "nothing" is not a reason to stop asking:
                        // a fallback list exists precisely because backends
                        // disagree about what exists. Recorded so the caller
                        // can tell "nobody found it" from "nobody was asked".
                        empty.push(name.clone());
                    }
                    Err(e) => { /* unchanged */ }
                }
```

链走完后：若 `errors.is_empty() && !empty.is_empty()` ⇒ `return Ok(Vec::new())`（全空是合法答案）；否则走既有的 `Err` 路径。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p alephcore --lib search::registry`

- [ ] **Step 5: 提交**

```bash
git add src/search/registry.rs
git commit -m "search: a zero-result answer no longer ends the failover chain

The default provider returning Ok(vec![]) counted as success, so eight other
backends and the WebFetch SERP fallback were never asked. A fallback list
exists because backends disagree about what exists; 'I found nothing' is the
one answer where asking the next one is most likely to help.

All-empty still returns an empty Ok, never an Err — folding 'nobody found
anything' into a failure would tell the model to retry a question that was
answered."
```

---

### Task 6: registry —— 显式 provider 覆写与结构化失败报告

**Files:**
- Modify: `src/search/registry.rs`
- Test: 同文件

**Interfaces:**
- Produces: `SearchOptions.provider: Option<String>`（新字段）；`SearchRegistry::search` 在点名不可用时返回 `AlephError::invalid_config`。

- [ ] **Step 1: 写失败测试**

```rust
/// Naming a provider is an instruction, not a preference. Falling back would
/// hand the caller results from a backend it did not choose while reporting
/// success — a confident wrong answer, which is the expensive kind.
#[tokio::test]
async fn an_unknown_named_provider_fails_loudly_instead_of_falling_back() {
    let mut reg = SearchRegistry::new("a");
    reg.add_provider("a".into(), Arc::new(MockProvider::new("a", false, 3)));
    let opts = SearchOptions { provider: Some("nope".into()), ..Default::default() };
    let err = reg.search("q", &opts).await.unwrap_err().to_string();
    assert!(err.contains("nope"), "{err}");
    assert!(err.contains('a'), "the error must list what IS configured: {err}");
}

#[tokio::test]
async fn a_named_provider_is_the_only_one_consulted() {
    let mut reg = SearchRegistry::new("a");
    reg.add_provider("a".into(), Arc::new(MockProvider::new("a", true, 0)));  // fails
    reg.add_provider("b".into(), Arc::new(MockProvider::new("b", false, 3)));
    reg.set_fallback_providers(vec!["b".into()]);
    let opts = SearchOptions { provider: Some("a".into()), ..Default::default() };
    assert!(reg.search("q", &opts).await.is_err(), "must not silently use b");
}

/// The classifier already computed a failure kind for every provider; before
/// this it fed one log line and nothing else. The message a model and an
/// operator both read is the right consumer.
#[tokio::test]
async fn the_failure_report_names_each_provider_and_its_failure_kind() {
    let mut reg = SearchRegistry::new("a");
    reg.add_provider("a".into(), Arc::new(MockProvider::new("a", true, 0)));
    let err = reg.search("q", &SearchOptions::default()).await.unwrap_err().to_string();
    assert!(err.contains("a ["), "expected `name [kind]` framing, got: {err}");
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p alephcore --lib search::registry`
Expected: FAIL —— `SearchOptions` 无 `provider` 字段。

- [ ] **Step 3: 实现**

`options.rs` 加字段：

```rust
    /// Consult exactly this backend. `None` = the configured chain.
    ///
    /// Naming one is an instruction: if it is unknown or unavailable the
    /// search fails and says so, rather than quietly answering from a backend
    /// the caller did not pick.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
```

`registry.rs` 在 `search()` 开头：

```rust
        if let Some(name) = &options.provider {
            let Some(p) = self.providers.get(name).filter(|p| p.is_available()) else {
                let mut known: Vec<&str> = self.providers.keys().map(String::as_str).collect();
                known.sort_unstable();
                return Err(AlephError::invalid_config(format!(
                    "search provider '{name}' is not configured or not available; \
                     configured: {}",
                    known.join(", ")
                )));
            };
            return p.search(query, options).await;
        }
```

失败报告改成逐行 `name [kind] message` 的结构化文本（既有 `errors: Vec<String>` 已是这个形状，把它整理成多行并加一句抬头）。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p alephcore --lib search::registry`

- [ ] **Step 5: 提交**

```bash
git add src/search/options.rs src/search/registry.rs
git commit -m "search: honour an explicit provider, and give the error classifier a real consumer

Naming a backend now means it, including when it fails: falling through would
answer from a backend the caller did not choose while reporting success.

classify_search_error has computed a kind for every failure since it was
written and fed exactly one log line. The chain-exhausted message is the
place where both a model and an operator can act on it, so that is where it
goes. It does not drive routing — every provider is tried once regardless,
and claiming otherwise would be inventing a use for it."
```

---

### Task 7: `search/notes.rs` —— 省略与降级的单一源

**Files:**
- Create: `src/search/notes.rs`
- Modify: `src/search/mod.rs`, `src/search/registry.rs`
- Test: `src/search/notes.rs`

**Interfaces:**
- Produces: `pub fn degraded(dimension: &str, provider: &str) -> String`、`pub fn answered_after_failures(provider: &str, failed: usize) -> String`、`pub fn all_empty(n: usize) -> String`、`pub fn snippets_clamped(n: usize, max: usize) -> String`；`SearchRegistry::search` 返回类型改为 `Result<SearchAnswer>`，`pub struct SearchAnswer { pub results: Vec<SearchResult>, pub provider: String, pub notes: Vec<String> }`。

- [ ] **Step 1: 写失败测试**

```rust
// src/search/notes.rs
#[cfg(test)]
mod tests {
    use super::*;

    /// Every omission has to read as a different thing. When they collapse
    /// into one phrasing ("some results were withheld") readers learn to skip
    /// the line, which costs more than the line saves.
    #[test]
    fn the_four_notes_do_not_read_alike() {
        let all = [
            degraded("domains", "exa"),
            answered_after_failures("brave", 2),
            all_empty(3),
            snippets_clamped(4, 600),
        ];
        for (i, a) in all.iter().enumerate() {
            for b in all.iter().skip(i + 1) {
                assert_ne!(a, b);
            }
        }
    }

    /// A note must name the lever the caller can pull, or it is just an
    /// apology.
    #[test]
    fn the_degraded_note_names_the_dimension_and_the_backend() {
        let n = degraded("domains", "exa");
        assert!(n.contains("domains"), "{n}");
        assert!(n.contains("exa"), "{n}");
    }
}
```

registry 测试加：

```rust
#[tokio::test]
async fn a_backend_that_cannot_express_the_dimension_says_so() {
    let mut reg = SearchRegistry::new("plain");
    reg.add_provider("plain".into(), Arc::new(MockProvider::new("plain", false, 2)));
    let opts = SearchOptions { include_domains: vec!["github.com".into()], ..Default::default() };
    let answer = reg.search("q", &opts).await.unwrap();
    assert_eq!(answer.results.len(), 2, "the search still runs");
    assert!(
        answer.notes.iter().any(|n| n.contains("domains") && n.contains("plain")),
        "silently dropping the dimension is the failure this note exists to prevent: {:?}",
        answer.notes
    );
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p alephcore --lib search::`
Expected: FAIL —— `notes` 模块与 `SearchAnswer` 不存在。

- [ ] **Step 3: 实现**

创建 `src/search/notes.rs`，模块文档写清"为什么是单一源"（`file_search::notes` 的先例：两个面各写各的会写成近乎相同但不相同）。四个函数各返回一句点名杠杆的话，例：

```rust
/// A dimension the answering backend cannot express.
///
/// Names both halves on purpose: without the dimension the reader cannot
/// tell which of their parameters was dropped, and without the backend they
/// cannot tell whether configuring another one would help.
#[must_use]
pub fn degraded(dimension: &str, provider: &str) -> String {
    format!(
        "`{dimension}` was not applied: the answering backend `{provider}` has no \
         native parameter for it, so these results are unfiltered on that axis"
    )
}
```

`SearchAnswer` 放在 `registry.rs`（它是 registry 的返回形状），`search()` 返回它。`SearchOptions` 的三个维度逐个与胜出 provider 的 `capabilities()` 比对，不满足的加 note。

⚠️ 所有既有 `registry.search(...)` 调用点（`src/builtin_tools/search.rs`、测试）都要跟着改——`cargo check` 会点名。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p alephcore --lib search::`

- [ ] **Step 5: 提交**

```bash
git add src/search/notes.rs src/search/mod.rs src/search/registry.rs src/builtin_tools/search.rs
git commit -m "search: one owner for the sentences that say what a result set is missing

Four omissions, four phrasings, one module. Written twice they come out
nearly-but-not-quite the same, and near-duplicates are how a reader learns to
skip the line.

The registry now returns SearchAnswer { results, provider, notes } so a
degraded dimension travels with the results instead of being dropped between
the layer that knows about it and the layer that renders it."
```

---

### Task 8: 工具面 —— 参数、保真、上下文经济

**Files:**
- Modify: `src/builtin_tools/search.rs`
- Test: 同文件

**Interfaces:**
- Consumes: Task 1 `Recency`、Task 7 `SearchAnswer`。
- Produces: `SearchArgs { query, limit, recency, domains, exclude_domains, full_content, provider }`；`SearchOutput { results, query, provider_used, notes }`；`SearchResult { title, url, snippet, relevance_score, published_date, full_content }`。

- [ ] **Step 1: 写失败测试**

```rust
#[test]
fn every_argument_reaches_search_options() {
    let args: SearchArgs = serde_json::from_value(serde_json::json!({
        "query": "q",
        "limit": 7,
        "recency": "week",
        "domains": ["github.com"],
        "exclude_domains": ["pinterest.com"],
        "full_content": true,
        "provider": "tavily"
    })).unwrap();
    let o = args.to_options(&SearchOptions::default());
    assert_eq!(o.max_results, 7);
    assert_eq!(o.recency, Some(crate::search::Recency::Week));
    assert_eq!(o.include_domains, vec!["github.com".to_string()]);
    assert_eq!(o.exclude_domains, vec!["pinterest.com".to_string()]);
    assert!(o.include_full_content);
    assert_eq!(o.provider.as_deref(), Some("tavily"));
}

/// The operator's [search] defaults apply to whatever the model omitted —
/// omitting a parameter must not silently mean "the hardcoded default".
#[test]
fn omitted_arguments_defer_to_the_operator_defaults() {
    let args: SearchArgs = serde_json::from_value(serde_json::json!({"query": "q"})).unwrap();
    let base = SearchOptions { max_results: 11, timeout_seconds: 42, ..Default::default() };
    let o = args.to_options(&base);
    assert_eq!(o.max_results, 11);
    assert_eq!(o.timeout_seconds, 42);
    assert_eq!(o.recency, None);
    assert!(o.include_domains.is_empty());
}

/// A snippet is content, not a locator (grep clamps a line to 240 because a
/// grep line points at a file you can then read; a snippet is the answer).
/// It still needs a bound, and exceeding it has to be said out loud.
#[test]
fn long_snippets_are_clamped_and_the_clamp_is_announced() {
    let long = "x".repeat(SNIPPET_MAX_CHARS + 500);
    let (results, notes) = render_results(
        vec![crate::search::SearchResult::new("t", "u", long)],
        "tavily",
    );
    assert!(results[0].snippet.chars().count() <= SNIPPET_MAX_CHARS + 1);
    assert!(notes.iter().any(|n| n.contains("clamp")), "{notes:?}");
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p alephcore --lib builtin_tools::search`
Expected: FAIL —— `to_options` / `render_results` / `SNIPPET_MAX_CHARS` 不存在。

- [ ] **Step 3: 实现**

`SearchArgs` 加五个字段，各带一句**能让模型用对**的描述（描述字节全付，见 Task 12）。例：

```rust
    /// How fresh results have to be. Omit for no constraint.
    ///
    /// Backends that have no freshness parameter are ranked behind those that
    /// do; if none can express it the search still runs and says so.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recency: Option<Recency>,
```

`to_options(&self, base: &SearchOptions) -> SearchOptions` 从 operator 默认起步，逐字段覆盖（**只覆盖模型真的给了的**）。

`SNIPPET_MAX_CHARS: usize = 600` 与 `FULL_CONTENT_MAX_CHARS: usize = 20_000`，各带一段解释量纲的注释（grep 行 240 是定位器 / snippet 是内容）。截断走 `crate::utils::text_format::truncate_chars`（UTF-8 安全）。

`SearchOutput` 加 `provider_used` 与 `notes`；`SearchResult` 加 `relevance_score` / `published_date` / `full_content`（三者都 `skip_serializing_if = "Option::is_none"`，没有就不占字节）。

⚠️ `published_date` 需要 `crate::search::SearchResult` 也有这个字段——**本任务同批加**，并在 tavily/exa 的响应解析里填（两家 API 都返回它；brave/bing/google/ddg/searxng/jina/firecrawl 不填就是 `None`，表示"这个后端没说"，**不许发明**）。

加预算覆写：

```rust
    /// A search can carry N page bodies when `full_content` is set, which is
    /// strictly more than the one page `web_fetch` bounds at 10k. Below that,
    /// above the global default.
    fn max_result_tokens(&self) -> Option<usize> {
        Some(8_000)
    }
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p alephcore --lib builtin_tools::search`

- [ ] **Step 5: 提交**

```bash
git add src/builtin_tools/search.rs src/search/result.rs src/search/providers/tavily.rs src/search/providers/exa.rs
git commit -m "search tool: expose the knobs that already existed, and stop discarding four fields

SearchOptions has had seven fields; two of them had a writer. The tool face
accepted {query, limit} while fifteen shipped per-provider decoders sat
downstream — seven of them translating a freshness value no caller could set.

The result mapping dropped relevance_score, full_content and provider on the
floor, and no layer carried a publication date at all, so a question about
recent work came back with no way to tell new from old. published_date is
filled where the vendor sends one and left absent where it does not: absent
means 'this backend did not say', never 'this result has no date'.

Snippets are clamped at 600 chars and the clamp is announced. The number is
not grep's 240 on purpose — a grep line is a locator for a file you then
read, a snippet is the answer itself."
```

---

### Task 9: 收敛遗留 Tavily 直连路径

无 `[search]` 块但有 `TAVILY_API_KEY` 时，boot 合成一个单后端 registry，`SearchTool` 只剩一条路径。**不这样做的话，Task 8 的五个新参数在那条路径上全是报成功的 no-op。**

**Files:**
- Modify: `src/search/registry.rs`（`from_env_only`）
- Modify: `src/executor/builtin_registry/definitions.rs:1020-1030`、`src/executor/builtin_registry/builder/constructor/mod.rs:45-55`
- Modify: `src/builtin_tools/search.rs`（删 legacy 分支）
- Modify: `src/tools/traits.rs:377`（文档示例）
- Test: `src/search/registry.rs`

- [ ] **Step 1: 写失败测试**

```rust
/// Zero-config still works: a machine with TAVILY_API_KEY and no [search]
/// block gets a one-backend registry, not a second code path that ignores
/// every option the tool face now accepts.
#[test]
fn an_env_only_install_still_gets_a_registry() {
    let reg = SearchRegistry::from_env_only("tvly-test").expect("registry");
    assert_eq!(reg.default_options().max_results, 5);
    assert!(reg.get_provider("tavily").is_some());
}

#[test]
fn no_key_and_no_config_yields_no_registry_rather_than_a_second_path() {
    assert!(SearchRegistry::from_env_only("").is_none());
}
```

`src/builtin_tools/search.rs` 加：

```rust
/// The tool must exist even with nothing configured, and say so when called —
/// a missing tool reads to the model as "this harness cannot search", which
/// is a different and wrong statement.
#[tokio::test]
async fn a_registry_with_no_providers_fails_with_an_actionable_message() {
    let tool = SearchTool::with_registry(Arc::new(SearchRegistry::new("none")));
    let err = tool.call_impl(SearchArgs { query: "q".into(), ..Default::default() })
        .await.unwrap_err().to_string();
    assert!(err.contains("no search backend"), "{err}");
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p alephcore --lib search::registry::tests::an_env_only`
Expected: FAIL —— `from_env_only` 不存在。

- [ ] **Step 3: 实现**

```rust
    /// Build a one-backend registry from a bare API key.
    ///
    /// This replaces a second implementation of "how do I search" that lived
    /// in SearchTool and read TAVILY_API_KEY directly. That path predated
    /// SearchOptions and ignored all of it, so every parameter the tool face
    /// accepts would have been a silent no-op on any install without a
    /// [search] block.
    #[must_use]
    pub fn from_env_only(api_key: &str) -> Option<Self> { /* ... */ }
```

删除 `src/builtin_tools/search.rs` 里：`TavilyResponse`、`TavilyResult`、`call_impl` 的 legacy 分支、`SearchTool::{new, with_api_key}`、`client` 与 `fallback_timeout` 字段、`DEFAULT_MAX_RESULTS`、`LEGACY_FALLBACK_TIMEOUT_SECS`、`impl Default for SearchTool`、`impl Clone for SearchTool` 里的对应字段。

两个构造点改成：先试 `registry`，否则 `from_env_only(cfg.tavily_api_key)`，否则空 registry。

`src/tools/traits.rs:377` 的 `SearchTool::new()` 示例改成 `SearchTool::with_registry(...)`。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p alephcore --lib && cargo test -p alephcore --bins`
⚠️ **必须跑 `--bins`**：构造点在 `src/bin/` 与 `executor/` 下，`--lib` 带不到。

- [ ] **Step 5: 提交**

```bash
git add -A
git commit -m "search: one implementation of 'how do I search', not two

SearchTool carried a direct Tavily path that read TAVILY_API_KEY itself and
predated SearchOptions entirely. It is reachable in production — it is the
zero-config install — so it is not dead code, which is exactly the problem:
every parameter the tool face now accepts would have been accepted, reported
as applied, and dropped on any machine without a [search] block.

Boot synthesises a one-backend registry from the bare key instead, and about
120 lines of second answer go away with it. A registry with no providers
still registers the tool and fails with a message naming what to configure:
a missing tool tells the model this harness cannot search, which is a
different claim and a false one."
```

---

### Task 10: protocol 单一源 + factory census

**Files:**
- Create: `shared/protocol/src/search/mod.rs`, `shared/protocol/src/search/providers.rs`
- Modify: `shared/protocol/src/lib.rs`
- Modify: `src/search/factory.rs`（census 测试）
- Modify: `src/config/types/search.rs:88`（删枚举、指向常量）

**Interfaces:**
- Produces: `aleph_protocol::search::{SearchProviderPreset, CONFIGURABLE_SEARCH_PROVIDERS}`。

- [ ] **Step 1: 写失败测试**

`src/search/factory.rs` tests：

```rust
/// Set equality, both directions. A one-way containment assertion reads as
/// passing when both sides are missing the same entry — which is how two
/// channel adapters sat unconfigurable for four months.
#[test]
fn the_factory_builds_exactly_the_providers_the_protocol_advertises() {
    use std::collections::BTreeSet;
    let buildable: BTreeSet<&str> = ProviderFactoryRegistry::with_defaults()
        .known_provider_types()
        .into_iter()
        .collect();
    let advertised: BTreeSet<&str> = aleph_protocol::search::CONFIGURABLE_SEARCH_PROVIDERS
        .iter()
        .map(|p| p.name)
        .collect();
    assert_eq!(
        buildable, advertised,
        "left = the factory can build it, right = the UI offers it. \
         An entry on only one side is either a provider nobody can configure \
         or a card that saves a config the server will refuse."
    );
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p alephcore --lib search::factory`
Expected: FAIL —— `aleph_protocol::search` 不存在。

- [ ] **Step 3: 实现**

`shared/protocol/src/search/providers.rs`：

```rust
//! Which search backends exist, in one place.
//!
//! There were three statements of this: the factory registry, a doc comment
//! in config/types/search.rs listing nine names, and the Panel's PRESETS
//! table listing eight — `jina` had a provider, a factory and a doc entry but
//! no card, so the only way to configure it was to hand-edit config.toml.
//!
//! Presentation (icon colour, description, i18n) stays in the Panel; this
//! constant owns identity and config shape, and a census asserts the two
//! agree as sets.

pub struct SearchProviderPreset {
    pub name: &'static str,
    pub display_name: &'static str,
    pub needs_api_key: bool,
    pub needs_base_url: bool,
    pub needs_engine_id: bool,
    pub default_base_url: Option<&'static str>,
    pub api_key_placeholder: Option<&'static str>,
}

pub const CONFIGURABLE_SEARCH_PROVIDERS: &[SearchProviderPreset] = &[ /* 9 条 */ ];
```

九条的取值**从 Panel 现有 `PRESETS` 抄那八条**（它们是今天上线的真值），第九条 `jina` 从 `src/search/providers/jina.rs` 的构造要求推（读它的 `new()` 看要不要 key / base_url）。

`src/config/types/search.rs:88` 的注释改成：

```rust
    /// Provider type — see `aleph_protocol::search::CONFIGURABLE_SEARCH_PROVIDERS`
    /// for the authoritative list. Enumerating the names here made a third
    /// copy of a fact that had already drifted once.
    pub provider_type: String,
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p alephcore --lib search::factory && cargo check -p aleph-protocol`

- [ ] **Step 5: 提交**

```bash
git add shared/protocol/src/search src/search/factory.rs src/config/types/search.rs shared/protocol/src/lib.rs
git commit -m "protocol: one list of which search backends exist

There were three, and one had already drifted: jina has a provider, a factory
and a line in the config doc comment, and no Panel card — so the only way to
configure it was to hand-edit config.toml.

The census asserts set equality in both directions. A one-way containment
check reads as passing when both sides are missing the same entry, which is
how two channel adapters stayed unconfigurable for four months."
```

---

### Task 11: Panel 从常量派生 + jina 卡片

**Files:**
- Modify: `interfaces/webchat/src/platform/wide/views/settings/search.rs`
- Test: 同文件

- [ ] **Step 1: 写失败测试**

```rust
/// The Panel may style a backend however it likes, but it may not decide
/// which backends exist.
#[test]
fn every_advertised_provider_has_a_card() {
    use std::collections::BTreeSet;
    let carded: BTreeSet<&str> = PRESENTATION.iter().map(|p| p.name).collect();
    let advertised: BTreeSet<&str> = aleph_protocol::search::CONFIGURABLE_SEARCH_PROVIDERS
        .iter().map(|p| p.name).collect();
    assert_eq!(carded, advertised);
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p aleph-panel --lib settings::search`
Expected: FAIL —— jina 无卡片。

- [ ] **Step 3: 实现**

把 `PRESETS` 拆成两半：identity/config shape 来自 protocol 常量；presentation（`description`、`icon_color`、`is_self_hosted`）留一张按 `name` 键控的 `PRESENTATION` 表，加上 jina 一行。渲染时 zip 两者。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p aleph-panel --lib && just wasm`
⚠️ **`just wasm` 是唯一编译出厂形态的命令**——`--lib` 测试构建里 `cfg(test)` 为真，两者不是同一份产物。

- [ ] **Step 5: 提交**

```bash
git add interfaces/webchat/src/platform/wide/views/settings/search.rs
git commit -m "panel: derive the search provider cards from the protocol list

jina gets a card for the first time. The Panel keeps the styling — icon
colour, description, whether it is self-hosted — and stops deciding which
backends exist."
```

---

### Task 12: 描述字节 —— 实测后抬棘轮

**Files:**
- Modify: `src/builtin_tools/search.rs`（`DESCRIPTION`）
- Modify: `src/executor/builtin_registry/definitions.rs`（两个棘轮常量 + 账本注释）

- [ ] **Step 1: 写 DESCRIPTION**

把 `search` 的一句话扩成能让模型**用对**新参数的几句（用户裁定：字节全付）。必须点到：多查询要分多次调用（本轮不做 `queries[]`）、`recency` 的四个值、`domains` 不是硬保证（不支持时会说）、`full_content` 很贵、`provider` 是指令。

- [ ] **Step 2: 跑棘轮，读实测值**

Run: `cargo test -p alephcore --lib catalog_description_bytes_ratchet -- --nocapture`
Expected: FAIL，且输出里带**实测新值与逐工具分解**。把那个分解原样抄进账本注释。
同样跑 `registry_schema_bytes_ratchet`。

⚠️ **不要手算**。判据 附录 C.1：只报标量的棘轮每次抬高都在强迫下一个人推断；账本要写分解。且 `(measured)` 只覆盖端点不覆盖因果——写"`search` +N B"时那个 N 要来自打印的分解，不是来自你对 diff 的阅读。

- [ ] **Step 3: 抬棘轮**

用实测值更新两个常量，注释写：日期 · 实测值 · 逐项分解 · 为什么这笔字节值得（五个参数各自买到什么）。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p alephcore --lib ratchet`

- [ ] **Step 5: 加守卫 G3**

```rust
/// The prose and the schema must not disagree about what this tool accepts.
/// A DESCRIPTION naming a parameter the schema does not carry teaches the
/// model to send something that will be rejected; the reverse hides a
/// parameter nobody will use.
#[test]
fn every_parameter_named_in_the_search_description_exists_in_its_schema() {
    let schema = schemars::schema_for!(SearchArgs);
    let props: std::collections::BTreeSet<String> =
        serde_json::to_value(&schema).unwrap()["properties"]
            .as_object().unwrap().keys().cloned().collect();
    let mut named = 0usize;
    for word in SearchTool::DESCRIPTION.split('`').skip(1).step_by(2) {
        if props.contains(word) { named += 1; continue; }
        // Not every backticked word is a parameter (values like `week` are
        // quoted too); only flag words that look like parameters and are not.
        assert!(
            !word.chars().all(|c| c.is_ascii_lowercase() || c == '_')
                || word.contains(' ')
                || KNOWN_NON_PARAMETER_WORDS.contains(&word),
            "`{word}` reads as a parameter name but SearchArgs has no such field"
        );
    }
    assert!(named >= 5, "the description must actually name the parameters");
}
```

⚠️ `KNOWN_NON_PARAMETER_WORDS` 是本守卫**自己的**豁免清单——写下它的同一笔里写下会让它缩的力（doc 里写明：加进来之前先问这个词能不能改成不像参数名）。

- [ ] **Step 6: 提交**

```bash
git add src/builtin_tools/search.rs src/executor/builtin_registry/definitions.rs
git commit -m "search tool: describe the new parameters, and pay for the bytes

Measured, not computed: <抄实测分解>. The knobs are worthless if the model
cannot tell when to reach for them, so the description names the four
recency buckets, says that domains is a preference the answer will report on
rather than a guarantee, and says full_content is expensive.

A guard asserts the prose and the schema name the same parameters — a
description naming a field the schema lacks teaches the model to send
something that gets rejected."
```

---

### Task 13: 真机 QA 装置

**Files:**
- Create: `qa/web_search/run.sh`, `qa/web_search/mock_searxng.py`, `qa/web_search/README.md`

**Interfaces:**
- Consumes: 全部前序任务。

- [ ] **Step 1: 读先例**

先读 `qa/file_search/run.sh` 与 `qa/busy_input/mock_anthropic.py`——本装置照它们的形状写（`qa/lib/build.sh::qa_build` 的 `PIPESTATUS` 用法、`qa/lib/scratch_home.sh::qa_redirect_home` 的 HOME 隔离、内容寻址的驱动而非按回合号索引）。

⚠️ **必须用 `qa_build`**：`cmd | tail` 的退出码是 `tail` 的，恒为 0，于是构建失败会被读成成功，装置接着跑共享 target 目录里躺着的旧二进制。

- [ ] **Step 2: 写 mock SearXNG**

`mock_searxng.py`：HTTP server，`/search` 返回 SearXNG JSON 形状，**记录每个请求的完整 query string 到 `requests.log`**。支持 `?__empty=1` 让它返回零条（`empty` 阶段用）。

⚠️ 靶子只能是 SearXNG：九个 provider 里只有它和 firecrawl 有 `base_url`，而它还不要 API key。这条约束写进 `README.md`。

- [ ] **Step 3: 四个阶段**

| 阶段 | 断言 |
|---|---|
| `reach` | 一次真回合，模型调 `search{query, recency:"week"}` ⇒ `requests.log` 里有 `time_range=week`。**先锚后否**：先断言这次调用真的返回了结果 |
| `order` | 配两个后端（searxng + 一个声明 `domain_filter` 的），带 `domains` 的请求打到支持的那个 |
| `degrade` | 只配 searxng ⇒ 输出 `notes` 里有 `domains` 与 `searxng`，**且先断言结果非空** |
| `empty` | 第一个后端 `__empty=1` ⇒ 第二个后端拿到请求（`requests.log` 有第二条） |

- [ ] **Step 4: 跑装置**

Run: `qa/web_search/run.sh`
Expected: 四阶段全 PASS。

- [ ] **Step 5: 变异证明装置真的会红**

把 Task 1 的 `searxng_time_range` 改成恒 `None`，`reach` **必须**红。撤销。
把 Task 5 的空结果分支改回 `return Ok(results)`，`empty` **必须**红。撤销。
⚠️ 判据 D.0.139：一条真机断言"跑到了"是待证命题，证法就是变异掉修复再跑。

- [ ] **Step 6: 提交**

```bash
git add qa/web_search/
git commit -m "qa: prove the search parameters reach the wire

In-process tests can assert that a mapper returns 'week'; they cannot assert
that a turn the model drives ends with time_range=week in a request. Those
are two objects on two paths, and this repo has shipped four rounds of a
feature whose only failure was the second one.

SearXNG is the only target available — seven of the nine providers hardcode
their endpoint and firecrawl needs a key — so the fixture proves the wiring,
not the nine backends. That limit is written into the fixture's own README.

Each phase anchors before it negates: it asserts the search actually returned
something before asserting what the notes say about it. A 'not in output'
assertion is satisfied just as well by a turn that never ran."
```

---

### Task 14: 文档

**Files:**
- Modify: `docs/reference/FEATURE_LOCATOR.md`（§3.6 新增 web 搜索轮）
- Modify: `CLAUDE.md`（仅当本轮产出新判据：写**触发器**，全文进 FEATURE_LOCATOR 附录 D）

- [ ] **Step 1: 写 FEATURE_LOCATOR 轮记录**

按本仓格式：口语关键词 / 代码锚点 / 职责 / 状态 + 本轮的 gap 分析、实测数字、刻意不做清单（附重访条件）。

**必须记进去的实测数字**（不是估算）：描述棘轮前后值与分解、schema 棘轮前后值。

- [ ] **Step 2: 判据落位**

本轮候选新判据（若实施中确认成立）：
- 「一个字段有生产者有消费者，而写它的那根线从未存在——15 个译码器等一个零写入点的字段」
- 「同一张四词表被七个映射器各写一份，且都以静默丢弃收尾」
两条都属**已有判据的新实例**（§0 断线 / 列举法），所以按写入纪律：**CLAUDE.md 只写触发器句 + 锚点 + `→ 附录 D.N.M` 指针，全文写 FEATURE_LOCATOR 附录 D**。

⚠️ 判据清单已超预算（MEMORY.md 有告警），**能挂到已有条目上就不新开一条**。

- [ ] **Step 3: 提交**

```bash
git add docs/reference/FEATURE_LOCATOR.md CLAUDE.md
git commit -m "docs: record the web search face round"
```

---

### Task 15: 收尾验证与合并

- [ ] **Step 1: 跑完整最小验证集**（六条，见 Global Constraints）
- [ ] **Step 2: 跑 `just wasm`**（改过 Panel）
- [ ] **Step 3: 跑 `qa/web_search/run.sh` 与 `qa/file_search/run.sh`**（后者确认没被本轮的 `notes` 改动波及）
- [ ] **Step 4: 报告**——把每条命令的真实结果贴出来；失败就说失败，不含糊
- [ ] **Step 5: 合并回 main**（用 `superpowers:finishing-a-development-branch`）。⚠️ **同会话内不要 `git worktree remove`**

---

## Self-Review

**Spec 覆盖**：spec §2.1→Task 8 · §2.2→Task 1 · §2.3→Task 3 · §2.4→Task 8 · §3→Task 2 · §4.1→Task 4 · §4.2/§4.4→Task 6 · §4.3→Task 5 · §5→Task 7+8 · §6.1→Task 9 · §6.2→Task 10+11 · §7 守卫→Task 2(G1)/10(G2)/12(G3) · §7 棘轮→Task 12 · §7 QA→Task 13 · §8 刻意不做→Task 14 记录。无遗漏。

**类型一致性**：`Recency`（T1 产 / T8 消）· `SearchCapabilities`（T2 产 / T3 T4 消）· `SearchAnswer{results,provider,notes}`（T7 产 / T8 消）· `SearchOptions.provider`（T6 产 / T8 消）· `CONFIGURABLE_SEARCH_PROVIDERS`（T10 产 / T11 消）—— 名字与字段全程一致。

**已知的计划外风险**：Task 3 的 Exa 字段名（`includeDomains`）与 Task 10 的 jina 配置要求（要不要 key）是**从代码/vendor 文档现读**的两处，计划里写了"先读再写"而不是替实施者猜。
