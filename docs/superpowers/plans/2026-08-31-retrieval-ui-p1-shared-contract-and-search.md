# P1：共享 picker 契约层 + search 设置面统一 — 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 `settings/search.rs`（2115 行）拆成目录，并把它的「9 个预设常驻网格 + 自定义列表 + 添加按钮」三件套换成 `providers` / `generation_providers` 已有的「隐藏式 picker + 单一已配置卡片列表」范式；同时把 `PresetPicker` 那条「空查询返回全量」的口头契约变成四个页面共用的可执行断言。

**Architecture:** 不新增泛型组件。复用 `PresetPicker` + `PickerRow` + `ProviderRowCard` + `ProviderBadges`，search 页新增一个 `picker.rs` 提供 `listed / offerable / chosen_target` 三个**纯函数**（i18n 通过闭包参数注入，所以它们无需 locale 即可单测）。`preset_picker.rs` 升格为目录并新增 `contract.rs`，装三条被四个页面共用的断言。

**Tech Stack:** Rust + Leptos 0.7 (CSR/WASM)、`aleph_protocol::providers::{Searchable, filter_rows}`、`leptos_i18n`（键缺失 = 编译错误）、`locales/{en,zh}.json`。

**Spec:** `docs/superpowers/specs/2026-08-31-retrieval-provider-ui-unification-design.md`（本计划实现其 §3、§4；§5 embedding / §6 rerank / §7 工具 / §8 QA 由 P2–P4 承接）

## Global Constraints

- **提交格式**：英文，`<scope>: <description>`，scope 用 `panel`。全局关闭 attribution。
- **单分支开发**：全部直接在 `main`。
- **Panel 的验证命令是 `cargo test -p aleph-panel --lib`**，不是 `cargo check` —— `check` 不编译 `#[cfg(test)]`，这个 crate 曾整程测试二进制编译不过而无人知晓。
- **`just wasm` 是唯一编译 Panel 出厂形态的命令**；在 Windows 上 `just` 必须**经 Bash 工具**调用（PowerShell 的 PATH 缺 cygpath）。
- **新增 i18n 键必须同时写进 `interfaces/webchat/locales/en.json` 与 `zh.json`** —— `t!` 在编译期解析，缺键是 build error，不是静默回退。
- **不硬编码中文字符串字面量**：`i18n_census.rs` 有一条只能向下的棘轮在数它们。
- **不改版本号**，不碰 `VERSION` 文件。
- 本计划**不动任何后端代码**。`search_config.{get,update,delete,test}` 的 wire 契约一个字节不变。

---

## File Structure

**新建**

| 文件 | 职责 |
|---|---|
| `interfaces/webchat/src/components/preset_picker/mod.rs` | 原 `preset_picker.rs` 原样搬入 |
| `interfaces/webchat/src/components/preset_picker/contract.rs` | 三条共享契约断言（`#[cfg(test)]`） |
| `.../settings/search/mod.rs` | `SearchView` + 左右分栏路由 |
| `.../settings/search/presentation.rs` | `SearchPresentation` / `PRESENTATION` / `UNSTYLED` / `SearchPreset` / `join` / `presets` / `find_preset` / `find_backend` |
| `.../settings/search/picker.rs` | `is_configured` / `listed` / `offerable` / `chosen_target` / `Subtitle` / `SearchPicker` |
| `.../settings/search/list.rs` | `ConfiguredList` —— 合并后的**单一**已配置卡片列表 |
| `.../settings/search/detail_panel.rs` | `ProviderDetailPanel` |
| `.../settings/search/add_custom.rs` | `AddCustomSearchProviderPanel` |
| `.../settings/search/global_settings.rs` | `GlobalSettings` |
| `.../settings/search/fetch_section.rs` | `FetchProvidersSection`（原样搬走，一行不改） |

**删除**

- `interfaces/webchat/src/components/preset_picker.rs`（变成 `preset_picker/mod.rs`）
- `interfaces/webchat/src/platform/wide/views/settings/search.rs`（变成 `search/` 目录）
- `PresetGrid` 与 `CustomSearchProvidersList` 两个组件（被 `ConfiguredList` + `SearchPicker` 取代，**删除不注释**）

**修改**

- `.../settings/mod.rs` —— `mod search;` 的路径不变（Rust 2018 目录模块），预期零改动，Task 2 需确认
- `interfaces/webchat/src/components/mod.rs` —— 同上
- `.../generation_providers/picker.rs` —— 测试改调共享断言
- `.../providers/picker.rs` —— 同上
- `interfaces/webchat/locales/{en,zh}.json` —— 新增 2 个键

---

## 一个已验证的前提（决定了本计划比 generation 少一层）

`search::ProviderDetailPanel` 的表单同步 Effect **已经处理「选中了一个未配置的预设」**：`find_backend` 落空时回落到 `find_preset(sel_name).base_url`（`search.rs:568-576`）。

所以 search **不需要 `__preset__` 前缀**。generation 需要它，是因为它的「已配置编辑器」和「未配置设置表单」是两个不同组件；search 是同一个组件的两个状态。`chosen_target` 因此只需要区分「一个 backend 名字」和「自定义端点」两种。

---

### Task 1: `preset_picker` 升格为目录 + 三条共享契约断言 + 两个样板页回填

**Files:**
- Create: `interfaces/webchat/src/components/preset_picker/contract.rs`
- Modify: `interfaces/webchat/src/components/preset_picker.rs` → 移动为 `interfaces/webchat/src/components/preset_picker/mod.rs`
- Modify: `interfaces/webchat/src/platform/wide/views/settings/generation_providers/picker.rs`（测试段）

**Interfaces:**
- Consumes: `crate::components::preset_picker::PickerRow`（已存在）
- Produces: `crate::components::preset_picker::contract::{empty_query_offers_everything, configured_rows_stay_offered_and_marked, deleted_row_returns_to_the_picker}` —— Task 3、以及 P2 / P3 的 picker 任务都会调用它们

- [ ] **Step 1: 移动文件，建立目录模块**

```bash
cd D:/Workspace/Aleph
mkdir -p interfaces/webchat/src/components/preset_picker
git mv interfaces/webchat/src/components/preset_picker.rs \
       interfaces/webchat/src/components/preset_picker/mod.rs
```

- [ ] **Step 2: 在 `preset_picker/mod.rs` 顶部（`use` 之后、`const LIST` 之前）挂上契约模块**

```rust
/// 四个 catalogue 页面共用的 picker 契约断言。
///
/// 这三条以前是**每页各写一遍**的：本模块的文档说「an empty query must return
/// every offerable row」，而担保物是每个页面的作者记得抄一份同名测试。四页
/// 各写一份，就是四个答案，且没人比较过它们。判据 E.0 §9：一个动词有几张脸，
/// 判据就要在每张脸上用**同一个推导**。
#[cfg(test)]
pub mod contract;
```

- [ ] **Step 3: 写 `contract.rs`**

```rust
//! Picker 分区函数必须满足的三条契约，四个页面共用同一份推导。
//!
//! 每条都对应一次真实可能的静默失效：
//!
//! * 空查询若过滤，目录页就**再也无法告诉你 Aleph 支持哪些供应商** —— 你得先
//!   知道厂商名字才能发现我们支持它。
//! * 已配置的行若从 offer 里消失，搜索就找不到它，且删除后无法再配回来。
//! * 已配置的行若 offer 了却不带标记，读起来像「还没配」，操作者会把已有的
//!   凭据覆盖掉。

use crate::components::preset_picker::PickerRow;

fn ids(rows: &[PickerRow]) -> Vec<String> {
    rows.iter().map(|r| r.id.clone()).collect()
}

/// 空查询必须返回**全部**可 offer 的行，且顺序 == 目录自身的顺序。
///
/// `expected_ids` 写全而不是只写个数：个数相等而顺序不同，意味着裸 Enter 选中
/// 的是另一行。
pub fn empty_query_offers_everything(
    offer: impl Fn(&str) -> Vec<PickerRow>,
    expected_ids: &[&str],
) {
    let rows = offer("");
    assert_eq!(
        ids(&rows),
        expected_ids,
        "an empty query must offer every row, in the catalogue's own order — \
         a catalogue that only appears after you type cannot tell you what exists"
    );
}

/// 已配置的行仍被 offer，且 `configured` 为真。
pub fn configured_rows_stay_offered_and_marked(
    offer: impl Fn(&str) -> Vec<PickerRow>,
    configured_id: &str,
) {
    let rows = offer("");
    let row = rows
        .iter()
        .find(|r| r.id == configured_id)
        .unwrap_or_else(|| {
            panic!(
                "configured row {configured_id} vanished from the picker — \
                 search can no longer find it and deleting it would be one-way"
            )
        });
    assert!(
        row.configured,
        "row {configured_id} is offered but unmarked, which reads as 'not set up yet'"
    );
}

/// 删除后该行回到 picker，且 `configured` 变假。
///
/// `after_delete` 是**删除后**的 offer 闭包（调用方用空的已配置列表构造它）。
pub fn deleted_row_returns_to_the_picker(
    after_delete: impl Fn(&str) -> Vec<PickerRow>,
    id: &str,
) {
    let rows = after_delete("");
    let row = rows
        .iter()
        .find(|r| r.id == id)
        .unwrap_or_else(|| panic!("deleted row {id} is unreachable — it can never be set up again"));
    assert!(
        !row.configured,
        "row {id} was deleted but still marked configured"
    );
}
```

- [ ] **Step 4: 回填 generation 的 picker 测试**

在 `generation_providers/picker.rs` 的 `mod tests` 里，把三个手写测试改成调共享断言。替换 `an_empty_query_offers_every_preset_in_the_category`、`a_configured_preset_is_listed_and_still_offered`、`deleting_a_provider_returns_its_row_to_the_panels_picker_only` 三个函数体（**保留函数名**，它们记录着各自的理由）：

```rust
    #[test]
    fn an_empty_query_offers_every_preset_in_the_category() {
        // 把目录放在按钮后面的全部理由：打开它仍然是在浏览。
        crate::components::preset_picker::contract::empty_query_offers_everything(
            |q| offerable(&catalog(), &[], GenerationType::Image, q),
            &["openai-dalle", "stability-ai"],
        );
    }

    #[test]
    fn a_configured_preset_is_listed_and_still_offered() {
        let providers = vec![configured("stability-ai")];
        assert_eq!(
            listed(&catalog(), &providers, GenerationType::Image).len(),
            1
        );
        crate::components::preset_picker::contract::configured_rows_stay_offered_and_marked(
            |q| offerable(&catalog(), &providers, GenerationType::Image, q),
            "stability-ai",
        );
    }

    #[test]
    fn deleting_a_provider_returns_its_row_to_the_panels_picker_only() {
        let before = vec![configured("stability-ai")];
        assert_eq!(listed(&catalog(), &before, GenerationType::Image).len(), 1);
        // 删除清空了配置列表；预设必须仍可 offer。
        assert!(listed(&catalog(), &[], GenerationType::Image).is_empty());
        crate::components::preset_picker::contract::deleted_row_returns_to_the_picker(
            |q| offerable(&catalog(), &[], GenerationType::Image, q),
            "stability-ai",
        );
    }
```

- [ ] **Step 5: 跑测试，确认全绿**

```bash
cd D:/Workspace/Aleph && cargo test -p aleph-panel --lib
```
Expected: PASS（这一步只是搬家 + 换调用，行为不变）

- [ ] **Step 6: 证伪这三条断言（E.0 §3 —— 没被证伪过的守卫不算守卫）**

临时把 `generation_providers/picker.rs` 的 `offerable` 首行改成：

```rust
    if query.is_empty() {
        return Vec::new();   // TEMPORARY — 证伪用
    }
```

```bash
cargo test -p aleph-panel --lib an_empty_query_offers_every_preset
```
Expected: **FAIL**，且报错信息里出现 `an empty query must offer every row`。

再把该行改成 `configured: false,`（`PickerRow` 构造处），跑：

```bash
cargo test -p aleph-panel --lib a_configured_preset_is_listed_and_still_offered
```
Expected: **FAIL**，报错含 `offered but unmarked`。

两次都确认后**撤销这两处临时改动**（`git checkout -- interfaces/webchat/src/platform/wide/views/settings/generation_providers/picker.rs` 会连 Step 4 一起撤掉，所以用手工改回，或先 `git stash` 证伪改动）。

- [ ] **Step 7: 复跑，确认回到全绿**

```bash
cargo test -p aleph-panel --lib
```
Expected: PASS

- [ ] **Step 8: Commit**

```bash
git add interfaces/webchat/src/components/preset_picker \
        interfaces/webchat/src/platform/wide/views/settings/generation_providers/picker.rs
git commit -m "panel: make the picker's empty-query contract executable and shared

The rule that an empty query must offer every row was stated in the
PresetPicker docs and enforced by each page remembering to write its own
copy of the test. Four pages is four answers nobody compares. Moves the
three assertions into preset_picker::contract so every catalogue page runs
the same derivation, and retrofits the generation page onto it."
```

---

### Task 2: 纯搬家 —— `search.rs` 拆成 `search/` 目录，零行为变化

**Files:**
- Create: `interfaces/webchat/src/platform/wide/views/settings/search/{mod,presentation,list,detail_panel,add_custom,global_settings,fetch_section}.rs`
- Delete: `interfaces/webchat/src/platform/wide/views/settings/search.rs`

**Interfaces:**
- Consumes: 无（纯移动）
- Produces: `presentation::{SearchPreset, presets, find_preset, find_backend, join}`（`pub(super)`）—— Task 3 的 `picker.rs` 依赖它们

> **为什么单独一笔**：拆分与行为改造合成一个 diff 就没人 review 得动，且「零行为变化」这个性质会被淹没。这一笔的验收标准是 **`git diff --stat` 只显示移动，测试数目不变**。

- [ ] **Step 1: 按行号区间切分**

源文件 `interfaces/webchat/src/platform/wide/views/settings/search.rs` 的既有边界（`#[component]` 属性行）：

| 行区间 | 内容 | 落点 |
|---|---|---|
| 1–18 | 模块文档 + `use` | 拆到各文件（各自只留用得上的） |
| 19–165 | `SearchPresentation` / `PRESENTATION` / `UNSTYLED` / `SearchPreset` / `join` / `presets` / `find_preset` / `find_backend` | `presentation.rs` |
| 166–290 | `SearchView` | `mod.rs` |
| 291–377 | `PresetGrid` | `list.rs`（Task 4 删除） |
| 378–463 | `CustomSearchProvidersList` | `list.rs`（Task 4 删除） |
| 464–520 | `GlobalSettings` | `global_settings.rs` |
| 521–1215 | `ProviderDetailPanel` | `detail_panel.rs` |
| 1216–1380 | `AddCustomSearchProviderPanel` | `add_custom.rs` |
| 1381–2115 | `FetchProvidersSection`（含 `FetchBackendEntry` 相关） | `fetch_section.rs` |

- [ ] **Step 2: 写 `search/mod.rs` 的模块头与再导出**

```rust
//! Search Settings View.
//!
//! ## Layout
//! - this module — top-level `SearchView` (left list + right pane router)
//! - [`presentation`] — the identity⋈styling join: which backends exist comes
//!   from `aleph_protocol::search::CONFIGURABLE_SEARCH_PROVIDERS`, how they
//!   look comes from this crate
//! - [`picker`] — which rows the panel lists vs. which the disclosure offers
//! - [`list`] — the single configured-backend card list
//! - [`detail_panel`] — `ProviderDetailPanel`, one component covering both the
//!   configured and the not-yet-configured state of a backend
//! - [`add_custom`] — `AddCustomSearchProviderPanel`
//! - [`global_settings`] — enable/max_results/timeout/PII
//! - [`fetch_section`] — `FetchProvidersSection` (crawl4ai + shared Firecrawl)

mod add_custom;
mod detail_panel;
mod fetch_section;
mod global_settings;
mod list;
mod presentation;

use add_custom::AddCustomSearchProviderPanel;
use detail_panel::ProviderDetailPanel;
use fetch_section::FetchProvidersSection;
use global_settings::GlobalSettings;
```

（`mod picker;` 在 Task 3 加入。）

- [ ] **Step 3: 每个被搬出的私有 item 加 `pub(super)`**

`presentation.rs` 里的 `SearchPreset`、`presets`、`find_preset`、`find_backend`、`join`、`SearchPresentation`、`PRESENTATION`、`UNSTYLED` 全部从私有改为 `pub(super)`；其余文件里被 `mod.rs` 引用的组件同理（`PresetGrid`、`CustomSearchProvidersList`、`GlobalSettings`、`ProviderDetailPanel`、`AddCustomSearchProviderPanel`、`FetchProvidersSection`）。

- [ ] **Step 4: 删除原文件**

```bash
cd D:/Workspace/Aleph && git rm interfaces/webchat/src/platform/wide/views/settings/search.rs
```

- [ ] **Step 5: 编译 + 测试**

```bash
cargo test -p aleph-panel --lib
```
Expected: PASS，且测试**条数与 Task 1 结束时相同**。

> **先看警告再看错误**：出现 `unused import` / `unused variable` 说明某半边没有调用者 —— 这个 crate 的语义合并冲突是常态形状，正解通常是 CUT 而不是补一个调用。

- [ ] **Step 6: 出厂形态编译**

```bash
just wasm
```
Expected: 成功。（Windows 上此命令必须经 Bash 工具跑。）

- [ ] **Step 7: 确认这一笔真的只是移动**

```bash
git add -A interfaces/webchat/src/platform/wide/views/settings/
git diff --cached --stat
```
Expected: 只有 `search.rs` 删除 + 7 个新文件新增，净增行数接近 0（模块头与 `use` 拆分带来的少量增长可接受，**不应有逻辑行变化**）。

- [ ] **Step 8: Commit**

```bash
git commit -m "panel: split settings/search.rs into a directory, no behaviour change

2115 lines in one file, against a 500-line guideline, and the next commit
has to rework its first third. Splits along the boundaries the section
banners already drew: presentation, list, detail_panel, add_custom,
global_settings, fetch_section. FetchProvidersSection moves verbatim."
```

---

### Task 3: `search/picker.rs` —— 三个纯函数，先测后写

**Files:**
- Create: `interfaces/webchat/src/platform/wide/views/settings/search/picker.rs`
- Modify: `interfaces/webchat/src/platform/wide/views/settings/search/mod.rs`（加 `mod picker;`）
- Modify: `interfaces/webchat/src/platform/wide/views/settings/search/presentation.rs`（给 `SearchPreset` 实现 `Searchable`）

**Interfaces:**
- Consumes: `presentation::{SearchPreset, presets, find_preset}`；`contract::*`（Task 1）；`aleph_protocol::providers::{Searchable, filter_rows}`
- Produces:
  - `pub(super) const CUSTOM_ROW_ID: &str = "__custom__";`
  - `pub(super) enum Subtitle { NeedsApiKey, SelfHosted, NoKeyRequired, CustomEndpoint }`
  - `pub(super) fn is_configured(backends: &[SearchBackendEntry], name: &str) -> bool`
  - `pub(super) fn listed(cfg: &SearchConfig) -> Vec<SearchBackendEntry>`
  - `pub(super) fn offerable(cfg: &SearchConfig, query: &str, copy: impl Fn(Subtitle) -> String) -> Vec<PickerRow>`
  - `pub(super) fn chosen_target(id: &str) -> Chosen`（`enum Chosen { Backend(String), CustomForm }`）
  - `#[component] pub(super) fn SearchPicker(...)`

- [ ] **Step 1: 给 `SearchPreset` 实现 `Searchable`（`presentation.rs` 末尾）**

```rust
/// 预设网格与 chat 目录走**同一个**匹配器。
///
/// `search_aliases` 保持默认空：搜索后端的 id 没有厂商别名，宣称一个空别名集
/// 是诚实的答案，而不是给它占个位。
impl aleph_protocol::providers::Searchable for SearchPreset {
    fn search_id(&self) -> &str {
        self.name
    }
    fn search_display_name(&self) -> &str {
        self.display_name
    }
}
```

- [ ] **Step 2: 先写失败的测试（`picker.rs` 全文的 `mod tests` 部分）**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用的副本解析器：把 `Subtitle` 映射成稳定的哨兵串，这样断言不依赖
    /// locale，也不会因为文案改字而变红。
    fn copy(s: Subtitle) -> String {
        match s {
            Subtitle::NeedsApiKey => "needs-key".to_string(),
            Subtitle::SelfHosted => "self-hosted".to_string(),
            Subtitle::NoKeyRequired => "no-key".to_string(),
            Subtitle::CustomEndpoint => "custom".to_string(),
        }
    }

    fn backend(name: &str) -> SearchBackendEntry {
        SearchBackendEntry {
            name: name.to_string(),
            api_key: None,
            base_url: None,
            engine_id: None,
            engines: None,
            has_api_key: true,
            verified: true,
        }
    }

    fn cfg(default_provider: &str, backends: Vec<SearchBackendEntry>) -> SearchConfig {
        SearchConfig {
            enabled: true,
            default_provider: default_provider.to_string(),
            max_results: 5,
            timeout_seconds: 10,
            pii_enabled: false,
            pii_scrub_email: true,
            pii_scrub_phone: true,
            pii_scrub_ssn: true,
            pii_scrub_credit_card: true,
            backends,
        }
    }

    fn ids(rows: &[PickerRow]) -> Vec<String> {
        rows.iter().map(|r| r.id.clone()).collect()
    }

    /// 目录的九条 + 末尾的自定义端点行。顺序 == 协议表的顺序。
    const EVERY_ROW: &[&str] = &[
        "tavily",
        "brave",
        "google",
        "bing",
        "searxng",
        "exa",
        "firecrawl",
        "duckduckgo",
        "jina",
        CUSTOM_ROW_ID,
    ];

    #[test]
    fn an_empty_query_offers_every_backend_plus_the_custom_row() {
        let c = cfg("", vec![]);
        crate::components::preset_picker::contract::empty_query_offers_everything(
            |q| offerable(&c, q, copy),
            EVERY_ROW,
        );
    }

    #[test]
    fn a_configured_backend_is_listed_and_still_offered() {
        let c = cfg("tavily", vec![backend("tavily")]);
        assert_eq!(
            listed(&c).iter().map(|b| b.name.clone()).collect::<Vec<_>>(),
            ["tavily"]
        );
        crate::components::preset_picker::contract::configured_rows_stay_offered_and_marked(
            |q| offerable(&c, q, copy),
            "tavily",
        );
    }

    #[test]
    fn deleting_a_backend_returns_its_row_to_the_picker() {
        let after = cfg("", vec![]);
        crate::components::preset_picker::contract::deleted_row_returns_to_the_picker(
            |q| offerable(&after, q, copy),
            "tavily",
        );
    }

    #[test]
    fn a_query_narrows_the_offered_presets() {
        let c = cfg("", vec![]);
        let rows = offerable(&c, "brav", copy);
        assert_eq!(ids(&rows), ["brave", CUSTOM_ROW_ID]);
    }

    /// 自定义端点行**永远**在 offer 里，且永远在最后。
    ///
    /// 它是一个动作，不是目录里的一行。把它藏在查询后面，意味着一个搜了我们
    /// 没有的厂商名的操作者会拿到一个空列表和零条出路 —— 判据 E.0 §14：被闸住
    /// 的人接下来会干什么。
    #[test]
    fn the_custom_row_survives_any_query_and_stays_last() {
        let c = cfg("", vec![]);
        for q in ["", "brav", "zzzz-no-such-vendor"] {
            let rows = offerable(&c, q, copy);
            assert_eq!(
                rows.last().map(|r| r.id.as_str()),
                Some(CUSTOM_ROW_ID),
                "query {q:?} left the operator with no way to add a custom endpoint"
            );
        }
    }

    #[test]
    fn the_custom_row_is_never_marked_configured() {
        let c = cfg("tavily", vec![backend("tavily")]);
        let rows = offerable(&c, "", copy);
        let custom = rows.iter().find(|r| r.id == CUSTOM_ROW_ID).unwrap();
        assert!(!custom.configured, "the custom row is an action, not a backend");
    }

    /// 一个不在预设表里的 backend（操作者自己加的）也要出现在已配置列表里 ——
    /// 这是合并两个列表的全部意义。
    #[test]
    fn a_custom_backend_is_listed_next_to_the_presets() {
        let c = cfg("tavily", vec![backend("tavily"), backend("my-searx")]);
        assert_eq!(
            listed(&c).iter().map(|b| b.name.clone()).collect::<Vec<_>>(),
            ["tavily", "my-searx"]
        );
    }

    #[test]
    fn subtitles_come_from_the_protocols_requirement_flags() {
        let c = cfg("", vec![]);
        let rows = offerable(&c, "", copy);
        let sub = |id: &str| {
            rows.iter().find(|r| r.id == id).unwrap().subtitle.clone()
        };
        assert_eq!(sub("tavily"), "needs-key", "tavily needs an API key");
        assert_eq!(sub("searxng"), "self-hosted", "searxng is run by the operator");
        assert_eq!(sub("duckduckgo"), "no-key", "duckduckgo needs no credential");
    }

    #[test]
    fn choosing_the_custom_row_opens_the_add_form() {
        assert_eq!(chosen_target(CUSTOM_ROW_ID), Chosen::CustomForm);
    }

    /// search 不需要 `__preset__` 前缀：它的详情面板同一个组件覆盖「已配置」和
    /// 「未配置」两种状态（`detail_panel.rs` 的表单同步 Effect 在 `find_backend`
    /// 落空时回落到预设的 base_url）。
    #[test]
    fn choosing_any_backend_row_selects_it_by_name() {
        assert_eq!(chosen_target("tavily"), Chosen::Backend("tavily".into()));
        assert_eq!(chosen_target("my-searx"), Chosen::Backend("my-searx".into()));
    }
}
```

- [ ] **Step 3: 跑测试，确认因「找不到 `picker` 模块」而失败**

```bash
cd D:/Workspace/Aleph && cargo test -p aleph-panel --lib picker
```
Expected: FAIL，编译错误 `cannot find module` / `unresolved import`（此时 `picker.rs` 只有测试段）

- [ ] **Step 4: 写实现（`picker.rs` 的 `mod tests` 之上）**

```rust
//! search 目录在左栏与「添加供应商」披露之间怎么分。
//!
//! 交互复用 [`crate::components::preset_picker`]，与 chat / generation 两页
//! 相同。本模块只拥有**分区**：哪些行左栏列、哪些行 picker offer、选中一行
//! 选中的是什么。三件事写在三处就会漂移，而一行既不被列出也不被 offer 就是
//! 不可达的，且没有任何代码会因此失败。
//!
//! # 与 generation 的两处不同
//!
//! * **没有 `__preset__` 前缀。** generation 的已配置编辑器和未配置设置表单是
//!   两个组件，需要一个前缀路由；search 是同一个组件的两个状态。
//! * **自定义端点是 offer 里的一行，不是列表下面的一个按钮。** 它永远被 offer
//!   且永远在最后 —— 见 `the_custom_row_survives_any_query_and_stays_last`。
//!
//! # i18n 不进这里
//!
//! [`offerable`] 收一个 `copy: impl Fn(Subtitle) -> String`，而不是自己调
//! `t_string!`。分区规则因此可以在没有 locale 的情况下单测，副本改字也不会
//! 让分区测试变红。

use leptos::prelude::*;

use super::presentation::{find_preset, presets, SearchPreset};
use crate::api::{SearchBackendEntry, SearchConfig};
use crate::components::preset_picker::{PickerRow, PresetPicker};
use crate::components::provider_badge::BadgeState;
use crate::i18n::{t_string, use_i18n};

/// picker 里那一行「自定义端点」的 id。
///
/// 不是任何 backend 的名字：它选中的是一个表单，不是一个供应商。用 `__` 前缀
/// 与协议里的 backend 名字划开，和 generation 的 `__preset__` 同一个约定。
pub(super) const CUSTOM_ROW_ID: &str = "__custom__";

/// 一行副标题需要哪一句副本。页面用 i18n 解析它；本模块保持纯粹。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Subtitle {
    NeedsApiKey,
    SelfHosted,
    NoKeyRequired,
    CustomEndpoint,
}

/// 选中一行之后页面要做什么。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Chosen {
    /// 选中这个 backend（已配置 → 编辑；未配置 → 同一个面板的设置态）。
    Backend(String),
    /// 打开自定义端点表单。
    CustomForm,
}

/// 操作者是否已经为这个名字配了一个 backend。
pub(super) fn is_configured(backends: &[SearchBackendEntry], name: &str) -> bool {
    backends.iter().any(|b| b.name == name)
}

/// 左栏列出的行：**所有**已配置的 backend，预设的和自定义的一视同仁。
///
/// 这是合并两个列表的实质：以前预设走 `PresetGrid`（无论配没配都列 9 个），
/// 自定义走 `CustomSearchProvidersList`，两个列表的卡片长得还不一样。左栏现在
/// 只回答一个问题 —— 你配了哪些。
pub(super) fn listed(cfg: &SearchConfig) -> Vec<SearchBackendEntry> {
    cfg.backends.clone()
}

/// 一个预设的副标题该用哪句副本。
fn subtitle_kind(preset: &SearchPreset) -> Subtitle {
    if preset.is_self_hosted {
        Subtitle::SelfHosted
    } else if preset.needs_api_key {
        Subtitle::NeedsApiKey
    } else {
        Subtitle::NoKeyRequired
    }
}

/// picker 为一个查询 offer 的行，最佳匹配在前，自定义端点行恒在最后。
///
/// 空查询返回**全部**预设，按协议表自身的顺序 —— [`PresetPicker`] 声明的契约，
/// 也是「打开披露仍然是在浏览」的理由。
pub(super) fn offerable(
    cfg: &SearchConfig,
    query: &str,
    copy: impl Fn(Subtitle) -> String,
) -> Vec<PickerRow> {
    let all: Vec<SearchPreset> = presets().collect();
    let mut rows: Vec<PickerRow> = aleph_protocol::providers::filter_rows(&all, query)
        .into_iter()
        .map(|preset| {
            let backend = cfg.backends.iter().find(|b| b.name == preset.name);
            PickerRow {
                configured: backend.is_some(),
                badge: BadgeState {
                    is_default: !cfg.default_provider.is_empty()
                        && cfg.default_provider == preset.name,
                    verified: backend.is_some_and(|b| b.verified),
                },
                id: preset.name.to_string(),
                name: preset.display_name.to_string(),
                subtitle: copy(subtitle_kind(&preset)),
                icon_color: preset.icon_color.to_string(),
                icon_glyph: None,
            }
        })
        .collect();

    rows.push(PickerRow {
        id: CUSTOM_ROW_ID.to_string(),
        name: copy(Subtitle::CustomEndpoint),
        subtitle: String::new(),
        icon_color: "#6B7280".to_string(),
        icon_glyph: Some("＋".to_string()),
        configured: false,
        badge: BadgeState {
            is_default: false,
            verified: false,
        },
    });
    rows
}

/// 选中 `id` 之后页面要做什么。
pub(super) fn chosen_target(id: &str) -> Chosen {
    if id == CUSTOM_ROW_ID {
        Chosen::CustomForm
    } else {
        Chosen::Backend(id.to_string())
    }
}

/// search 目录的披露。
#[component]
pub(super) fn SearchPicker(
    config: RwSignal<SearchConfig>,
    selected: RwSignal<Option<String>>,
    show_add_form: RwSignal<bool>,
    open: RwSignal<bool>,
) -> impl IntoView {
    let i18n = use_i18n();
    let copy = move |s: Subtitle| -> String {
        match s {
            Subtitle::NeedsApiKey => t_string!(i18n, settings.search.row_needs_key).to_string(),
            Subtitle::SelfHosted => t_string!(i18n, settings.search.self_hosted).to_string(),
            Subtitle::NoKeyRequired => t_string!(i18n, settings.search.no_api_key).to_string(),
            Subtitle::CustomEndpoint => {
                t_string!(i18n, settings.search.add_custom_provider).to_string()
            }
        }
    };
    let offer = move |query: &str| offerable(&config.get(), query, copy);
    let on_choose = move |id: String| match chosen_target(&id) {
        Chosen::Backend(name) => {
            show_add_form.set(false);
            selected.set(Some(name));
        }
        Chosen::CustomForm => {
            selected.set(None);
            show_add_form.set(true);
        }
    };

    view! { <PresetPicker offer=offer on_choose=on_choose open=open /> }
}
```

- [ ] **Step 5: 新增两个 i18n 键**

`interfaces/webchat/locales/en.json` 的 `settings.search` 对象里加：

```json
    "row_needs_key": "API key required",
    "cannot_delete_default": "This is the default provider. Set a different default first."
```

`interfaces/webchat/locales/zh.json` 的 `settings.search` 对象里加：

```json
    "row_needs_key": "需要 API 密钥",
    "cannot_delete_default": "它是默认供应商，请先改默认。"
```

（`cannot_delete_default` 在 Task 5 用；两个键一起加，避免 Task 5 再动一次 locale 文件。）

- [ ] **Step 6: 在 `search/mod.rs` 挂上模块**

在 `mod list;` 之后加一行 `mod picker;`（保持字母序：`add_custom, detail_panel, fetch_section, global_settings, list, picker, presentation`）。

- [ ] **Step 7: 跑测试，确认全绿**

```bash
cargo test -p aleph-panel --lib picker
```
Expected: PASS，10 个测试全过

- [ ] **Step 8: 证伪自定义行那条断言**

把 `offerable` 里 `rows.push(...)` 那一段临时挪到 `filter_rows` **之前**（即自定义行变成第一行），跑：

```bash
cargo test -p aleph-panel --lib the_custom_row_survives_any_query_and_stays_last
```
Expected: **FAIL**。确认后改回。

- [ ] **Step 9: Commit**

```bash
git add interfaces/webchat/src/platform/wide/views/settings/search/ \
        interfaces/webchat/locales/en.json interfaces/webchat/locales/zh.json
git commit -m "panel: add the search catalogue partition and its picker

listed/offerable/chosen_target as pure functions, i18n injected as a closure
so the partition can be unit-tested without a locale. The custom-endpoint row
is offered unconditionally and always last: it is an action, not a catalogue
row, and hiding it behind a query leaves an operator who searched for a vendor
we do not carry with an empty list and no way forward."
```

---

### Task 4: 左栏换成单一已配置列表 + 接上 picker

**Files:**
- Modify: `interfaces/webchat/src/platform/wide/views/settings/search/list.rs`（重写）
- Modify: `interfaces/webchat/src/platform/wide/views/settings/search/mod.rs`

**Interfaces:**
- Consumes: `picker::{SearchPicker, is_configured, listed}`；`presentation::find_preset`
- Produces: `list::ConfiguredList`

- [ ] **Step 1: 写 `list.rs` 的测试（先测）**

在 `list.rs` 底部：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// 一个不在预设表里的 backend 也要拿到一个能画的图标色 —— 缺色的行会画成
    /// 透明块。判据 E.0 §17：一份展示用的东西，提交前必须能指出渲染它的那一行。
    #[test]
    fn a_custom_backend_gets_the_neutral_icon_colour() {
        assert_eq!(icon_color_for("my-searx"), NEUTRAL_ICON_COLOR);
    }

    #[test]
    fn a_preset_backend_keeps_its_brand_colour() {
        assert_eq!(icon_color_for("brave"), "#FB542B");
    }

    /// 预设行显示厂商名，自定义行显示它自己的 id —— 自定义 backend 没有
    /// display_name，拿 id 是唯一的真话。
    #[test]
    fn display_name_falls_back_to_the_backend_id() {
        assert_eq!(display_name_for("brave"), "Brave");
        assert_eq!(display_name_for("my-searx"), "my-searx");
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

```bash
cargo test -p aleph-panel --lib list::tests
```
Expected: FAIL，`cannot find function icon_color_for`

- [ ] **Step 3: 重写 `list.rs`**

删除 `PresetGrid` 与 `CustomSearchProvidersList` 两个组件（**删除，不注释掉**），换成：

```rust
//! 左栏那一个列表：已配置的 backend，预设的和自定义的一视同仁。
//!
//! 以前这里是两个列表。预设走 `PresetGrid`（无论配没配都画 9 张卡），自定义走
//! `CustomSearchProvidersList`（灰色图标、另一句副标题），两者的卡片还长得不
//! 一样。于是「我配了哪些搜索供应商」这个问题在左栏有两个答案，取决于你配的
//! 那个恰好在不在协议表里 —— 而那件事跟操作者无关。

use leptos::prelude::*;

use super::presentation::find_preset;
use crate::api::SearchConfig;
use crate::components::provider_badge::{BadgeState, ProviderBadges};
use crate::components::provider_row_card::{ProviderRowCard, RowDot};
use crate::i18n::{t, use_i18n};

/// 不在预设表里的 backend 用的图标色。
const NEUTRAL_ICON_COLOR: &str = "#6B7280";

/// 一个 backend 名字的图标色：预设有品牌色，其余中性。
fn icon_color_for(name: &str) -> &'static str {
    find_preset(name).map_or(NEUTRAL_ICON_COLOR, |p| p.icon_color)
}

/// 一个 backend 名字的显示名：预设有厂商名，其余就是它自己的 id。
fn display_name_for(name: &str) -> String {
    find_preset(name).map_or_else(|| name.to_string(), |p| p.display_name.to_string())
}

/// 已配置的 backend 卡片列表。
#[component]
pub(super) fn ConfiguredList(
    config: RwSignal<SearchConfig>,
    selected: RwSignal<Option<String>>,
    show_add_form: RwSignal<bool>,
) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        {move || {
            let cfg = config.get();
            let rows = super::picker::listed(&cfg);
            if rows.is_empty() {
                // 空态不画标题：一个空的「搜索供应商」小标题读起来像加载失败。
                return view! { <div></div> }.into_any();
            }
            let default_provider = cfg.default_provider.clone();
            view! {
                <div>
                    <h2 class="text-sm font-medium text-text-secondary uppercase tracking-wider mb-3">
                        {t!(i18n, settings.search.providers_section)}
                    </h2>
                    <div class="grid grid-cols-1 gap-2">
                        {rows.into_iter().map(|backend| {
                            let name = backend.name.clone();
                            let name_sel = name.clone();
                            let name_click = name.clone();
                            let is_default = !default_provider.is_empty()
                                && default_provider == name;
                            let verified = backend.verified;
                            view! {
                                <ProviderRowCard
                                    name=display_name_for(&name)
                                    icon_color=icon_color_for(&name).to_string()
                                    subtitle=name.clone()
                                    is_selected=move || {
                                        selected.get().as_deref() == Some(name_sel.as_str())
                                    }
                                    is_configured=move || true
                                    dot=move || if verified { RowDot::Verified } else { RowDot::None }
                                    badge=move || view! {
                                        <ProviderBadges state=BadgeState { is_default, verified } />
                                    }.into_any()
                                    on_click=move || {
                                        show_add_form.set(false);
                                        selected.set(Some(name_click.clone()));
                                    }
                                />
                            }
                        }).collect_view()}
                    </div>
                </div>
            }.into_any()
        }}
    }
}
```

- [ ] **Step 4: 改 `mod.rs` —— 三件套换成 picker**

删掉 `SearchView` 里这三段（原 `search.rs:257-283` 对应的位置）：`<PresetGrid .../>`、`<CustomSearchProvidersList .../>`、以及「Add Custom Provider button」那个 `<div class="pt-2">…</div>`。换成：

```rust
                    // 已配置的 backend —— 一个列表，预设与自定义一视同仁。
                    <ConfiguredList config=config selected=selected show_add_form=show_add_form />

                    // 目录：九个预设 + 自定义端点，收在一个按钮后面。
                    <SearchPicker
                        config=config
                        selected=selected
                        show_add_form=show_add_form
                        open=picker_open
                    />
```

并在 `SearchView` 的信号区（`let show_add_form = RwSignal::new(false);` 之后）加：

```rust
    let picker_open = RwSignal::new(false);
    // 首次加载后**播种一次**：一个什么都没配的实例不该只看到一个收起的按钮。
    // 用播种而不是派生谓词 —— 会重算的信号会在操作者配第一个供应商的过程中，
    // 每次他关掉披露就又弹开。
    let seeded = RwSignal::new(false);
    Effect::new(move |_| {
        if loading.get() || seeded.get_untracked() {
            return;
        }
        seeded.set(true);
        if config.get_untracked().backends.is_empty() {
            picker_open.set(true);
        }
    });
```

`use` 段相应改为 `use list::ConfiguredList;` 与 `use picker::SearchPicker;`。

- [ ] **Step 5: 跑测试**

```bash
cargo test -p aleph-panel --lib
```
Expected: PASS。**先看警告**：`PresetGrid` / `CustomSearchProvidersList` 若还有 `unused` 警告说明没删干净。

- [ ] **Step 6: 出厂形态编译**

```bash
just wasm
```
Expected: 成功

- [ ] **Step 7: Commit**

```bash
git add interfaces/webchat/src/platform/wide/views/settings/search/
git commit -m "panel: one configured list plus a picker on the search page

Replaces the always-visible 9-preset grid, the separate custom-provider list
and the add-custom button with the shape the providers and generation pages
already use. The left panel now answers one question — which backends you have
set up — instead of two answers that differed by whether the backend happened
to be in the protocol table."
```

---

### Task 5: 删除按钮的禁用态 + 原因

**Files:**
- Modify: `interfaces/webchat/src/platform/wide/views/settings/search/detail_panel.rs`

**Interfaces:**
- Consumes: `settings.search.cannot_delete_default`（Task 3 Step 5 已加入两个 locale）
- Produces: 无

> 后端 `search_config.delete` 已经拒绝删除 `default_provider`（`src/gateway/handlers/search_config/delete.rs:48-53`），但今天这条拒绝只有**点下去之后**才看得到。判据 E.0 §14：被闸住的人接下来会干什么 —— 答不上就不是 fail-closed 是 fail-dead。

- [ ] **Step 1: 写测试**

在 `detail_panel.rs` 底部：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_provider_cannot_be_deleted() {
        assert!(!deletable("tavily", "tavily"));
    }

    #[test]
    fn a_non_default_provider_can_be_deleted() {
        assert!(deletable("brave", "tavily"));
    }

    /// 没有默认供应商时不该把所有删除按钮都锁死 —— 空的 default_provider 说的是
    /// 「还没选默认」，不是「每一个都是默认」。判据 E.0 §8：空字符串只有资格说
    /// 「我不知道」。
    #[test]
    fn an_empty_default_does_not_lock_every_row() {
        assert!(deletable("tavily", ""));
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

```bash
cargo test -p aleph-panel --lib detail_panel::tests
```
Expected: FAIL，`cannot find function deletable`

- [ ] **Step 3: 实现**

在 `detail_panel.rs` 的 `ProviderDetailPanel` 之上加：

```rust
/// 这个 backend 能不能删。
///
/// 与 `search_config.delete` 的服务端规则**逐字对应**：默认供应商不许删，要先
/// 改默认。写在这里是为了把那条拒绝画成禁用态而不是一次失败的往返 —— 服务端
/// 那条检查仍然是权威，这里只是提前把它说出来。
fn deletable(name: &str, default_provider: &str) -> bool {
    default_provider.is_empty() || name != default_provider
}
```

然后在现有删除按钮**外面**加一层分支。现有形状是 `confirming` 双态（搬家后位于 `detail_panel.rs`）：

```rust
                                                            {move || if confirming.get() {
                                                                view! {
                                                                    <ConfirmButton confirming=confirming on_confirm=on_confirm_delete width_class="flex-1" />
                                                                }.into_any()
                                                            } else {
                                                                view! {
                                                                    <button
                                                                        on:click=move |_| confirming.set(true)
                                                                        prop:disabled=move || deleting.get()
                                                                        class="flex-1 px-4 py-2.5 bg-danger-subtle border border-danger/20 text-danger text-sm font-medium rounded-lg hover:bg-danger-subtle/80 disabled:opacity-50"
                                                                    >
                                                                        {move || if deleting.get() { t_string!(i18n, settings.search.deleting).to_string() } else { t_string!(i18n, common.delete).to_string() }}
                                                                    </button>
                                                                }.into_any()
                                                            }}
```

改成（**只在最外层加 `deletable` 分支，双态那一段原样保留**）：

```rust
                                                            {move || {
                                                                let cfg = config.get();
                                                                let name = selected.get().unwrap_or_default();
                                                                if !deletable(&name, &cfg.default_provider) {
                                                                    // 服务端那条拒绝提前说出来：一个点下去才失败的按钮
                                                                    // 不告诉操作者接下来该干什么。
                                                                    return view! {
                                                                        <div class="flex-1 flex flex-col gap-1">
                                                                            <button
                                                                                prop:disabled=true
                                                                                class="w-full px-4 py-2.5 border border-border text-text-tertiary text-sm font-medium rounded-lg cursor-not-allowed opacity-60"
                                                                            >
                                                                                {t!(i18n, common.delete)}
                                                                            </button>
                                                                            <span class="text-xs text-text-tertiary">
                                                                                {t!(i18n, settings.search.cannot_delete_default)}
                                                                            </span>
                                                                        </div>
                                                                    }.into_any();
                                                                }
                                                                if confirming.get() {
                                                                    view! {
                                                                        <ConfirmButton confirming=confirming on_confirm=on_confirm_delete width_class="flex-1" />
                                                                    }.into_any()
                                                                } else {
                                                                    view! {
                                                                        <button
                                                                            on:click=move |_| confirming.set(true)
                                                                            prop:disabled=move || deleting.get()
                                                                            class="flex-1 px-4 py-2.5 bg-danger-subtle border border-danger/20 text-danger text-sm font-medium rounded-lg hover:bg-danger-subtle/80 disabled:opacity-50"
                                                                        >
                                                                            {move || if deleting.get() { t_string!(i18n, settings.search.deleting).to_string() } else { t_string!(i18n, common.delete).to_string() }}
                                                                        </button>
                                                                    }.into_any()
                                                                }
                                                            }}
```

`common.delete` 已存在（en = `"Delete"`），无需新增。

- [ ] **Step 4: 跑测试**

```bash
cargo test -p aleph-panel --lib detail_panel::tests
```
Expected: PASS

- [ ] **Step 5: 全量 + 出厂编译**

```bash
cargo test -p aleph-panel --lib && just wasm
```
Expected: 两者都成功

- [ ] **Step 6: Commit**

```bash
git add interfaces/webchat/src/platform/wide/views/settings/search/detail_panel.rs
git commit -m "panel: show the default-provider delete refusal before the click

search_config.delete has always refused to remove the default provider, but
the refusal only arrived after a round trip. Draws it as a disabled button
with the reason, so the operator learns what to do instead."
```

---

### Task 6: 收尾验证

**Files:** 无改动（纯验证）

- [ ] **Step 1: Panel 全量单测**

```bash
cd D:/Workspace/Aleph && cargo test -p aleph-panel --lib
```
Expected: PASS

- [ ] **Step 2: 出厂形态**

```bash
just wasm
```
Expected: 成功

- [ ] **Step 3: Clippy（需先 stage 占位文件）**

```bash
just _stage-shell-placeholders && cargo clippy --workspace --all-targets
```
Expected: 无新增 warning

- [ ] **Step 4: 确认没有新的硬编码中文**

```bash
cargo test -p aleph-panel --lib hardcoded_chinese
```
Expected: PASS（棘轮只能向下；本计划所有中文都进了 locale）

- [ ] **Step 5: 人工过一遍 search 设置页**

```bash
just dev
```
逐条确认：
1. 全新实例（无 backends）→ picker 默认展开
2. 空查询 → 9 个预设 + 末尾「添加自定义提供商」
3. 输入 `brav` → 只剩 Brave + 自定义行
4. 输入 `zzzz` → 只剩自定义行（不是空列表）
5. 配好 tavily 后 → 左栏出现一张卡，picker 里 tavily 带「Configured」标记
6. tavily 设为默认后 → 详情面板的删除按钮为禁用态并给出原因
7. 改默认为 brave 后 → tavily 的删除按钮恢复可用，删除后它回到 picker

---

## Self-Review

**Spec 覆盖（本计划负责 §3 + §4）**

| Spec 条目 | 落点 |
|---|---|
| §3.1 `contract.rs` 三条断言 | Task 1 Step 3 |
| §3.1 写完各证伪一次 | Task 1 Step 6、Task 3 Step 8 |
| §3.2 search 的 subtitle 由 `needs_*` 推导 | Task 3 Step 4 `subtitle_kind` + 测试 `subtitles_come_from_the_protocols_requirement_flags` |
| §3.2 未配置行不假装有值 | 自定义行 `subtitle: String::new()`；预设行的副标题是「需要什么」不是伪造的模型名 |
| §4.1 目录拆分独立一笔 | Task 2 |
| §4.2 两个列表合成一个 | Task 4 |
| §4.2 picker offer = 全量 + 自定义端点行 | Task 3 |
| §4.2 删除按钮禁用态 + 原因 | Task 5 |
| §4.3 后端零改动 | Global Constraints 末条；无任何任务触及 `src/` |
| §10 不碰 `FetchProvidersSection` | Task 2 Step 1 表格标注「原样搬走」 |
| §10 不改样板页行为 | Task 1 只改 generation 的**测试**段 |

**未在本计划内的 spec 条目**：§5（embedding）→ P2；§6（rerank）→ P3；§7（工具）+ §8（QA / 手机端）→ P4。

**类型一致性检查**

- `PickerRow` 字段名与 `preset_picker/mod.rs:74-88` 逐一对齐：`id / name / subtitle / icon_color / icon_glyph / configured / badge` ✓
- `BadgeState { is_default, verified }` 与 `provider_badge.rs` 一致 ✓
- `SearchBackendEntry` 七个字段与 `api/search.rs:6-22` 一致（`name / api_key / base_url / engine_id / engines / has_api_key / verified`）✓
- `SearchConfig` 构造与 `search.rs:175-187` 的既有字面量一致（11 个字段）✓
- `Chosen` 在 Task 3 定义、Task 3 测试与 `SearchPicker` 使用，命名一致 ✓
- `listed()` 在 Task 3 定义、Task 4 `list.rs` 通过 `super::picker::listed` 调用 ✓

**计划期已核实、不留给执行者猜的事实**

- `common.delete` 存在，en = `"Delete"` —— Task 5 无需新增键。
- 删除按钮的现有形状是 `confirming` 双态 + `<ConfirmButton confirming=… on_confirm=… width_class="flex-1" />`，Task 5 Step 3 贴的是**现场原文**，只在最外层加了一个分支。
- `ProviderDetailPanel` 的表单同步 Effect 已覆盖「选中未配置的预设」（`search.rs:568-576` 回落到预设 base_url），所以 search 不需要 `__preset__` 前缀 —— 这是本计划比 generation 少一层的根据，已在开头单列一节。
- `PRESENTATION` 表九条的顺序与 `CONFIGURABLE_SEARCH_PROVIDERS` 一致，`EVERY_ROW` 的九个 id 逐字取自协议表（tavily / brave / google / bing / searxng / exa / firecrawl / duckduckgo / jina）。

**唯一一处执行期才能定的**：Task 5 的那段 JSX 在搬家后位于 `detail_panel.rs` 的哪一行 —— 靠 `ConfirmButton` 这个唯一出现点定位即可。
