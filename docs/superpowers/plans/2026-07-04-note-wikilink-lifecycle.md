# Note Wiki-Link 生命周期与关联连线深化 — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 打通 note wikilink 的完整生命周期（置信度解析链 / 墓碑删除 / 即时回填 / rename 接通 / typed-relation 工具化 / alias 保真），把已算出的关联（相似边 / confidence / insights）呈现到星系图与 Panel，正文 `[[链接]]` 三视图可点击，并新增 mention 软边自织网。

**Spec:** `docs/superpowers/specs/2026-07-04-note-wikilink-lifecycle-design.md`（已用户确认）。

**Architecture:** C 的方法、A 的目的地——新建 `src/memory/notes/links/` 纯政策模块（解析链 + 提及扫描，复刻 `graph/` 纯函数模式），薄 SQL 留在 `store_impl`，生命周期触发点在 `indexer`/`note_manage`/gateway 就地接线；Panel 侧扩展 adapter DTO + galaxy 构建纯变换 + 三视图导航。

**Tech Stack:** Rust (tokio + serde + rusqlite)、Leptos/WASM Panel、SQLite。**零新依赖**。

## Global Constraints

- **R10**: 零 `src/harness/` 改动。
- **R3/依赖禁令**: 零新 crate。Panel 端 wikilink 解析用手写扫描（webchat 不引 regex）。
- **R7/P8**: 解析链与提及扫描是确定性机械匹配（同 FTS 性质）；语义关联仍由 LLM 产出。
- **cargo 极度节制**（用户规则）: 开发期**不跑任何 cargo 命令**，编译信号只看编辑器注入的 `<new-diagnostics>`；唯一验证门在 Task 17（`cargo test -p alephcore --lib` 一次 + `cargo test -p aleph-panel --lib` 一次，运行前须用户在场知情）。计划中每个 Task 的 "verify" 步骤指"确认 diagnostics 无新错误"。
- **提交规范**: English, `<scope>: <description>`（如 `notes: add link resolution chain`）。**不加** Co-Authored-By 尾注（用户全局 settings 已禁用归属）。
- **分支策略**: 单分支，main 直作（项目约定）。
- **Panel 刷新链**: 改 panel 后运行时看效果需 `just wasm` → 重编 server → 替换运行中 binary（Task 17 才做，开发期不做）。
- **数值约定**（spec §2.3）: 解析链 confidence = 精确路径 1.0 / 精确文件名 0.95 / alias 0.85 / 归一化 0.7 / 悬空 0.0；mention 边 confidence 0.35；status ∈ `active | dangling | tombstone`。
- **NoteStore trait 惯例**: 本计划新增的每个 trait 方法（`get_outgoing_link_rows` / `backfill_inbound_links` / `related_edges_between` / `replace_mention_links`）都按既有惯例带 **no-op 默认实现**（`Ok(vec![])` / `Ok(0)` / `Ok(())`，参数 `let _ = …;`）——"Default impls keep any non-SQLite store compiling; the real bodies live on `SqliteMemoryBackend`"（store.rs 内注释原话）。真实现全部落 `store_impl.rs`。
- **已确认的 spec 偏差**（实现前已批准，见各 Task 注）:
  1. spec §2.1 的 `links/lifecycle.rs` 不建独立文件——回填/墓碑是单条 SQL 操作，落为 `NoteStore` trait 方法 + indexer 触发点直调（独立编排文件是死间接层）。
  2. spec B6 说 `extract_wikilinks_with_alias`「已存在」——实际不存在（NOTES.md 文档超前），Task 2 新建。
  3. spec S2 的 confidence→透明度：GL 边无 per-edge alpha 通道，用**亮度缩放**实现（`edge_bright` 并行数组，乘进边颜色；>1.0 触发既有 bloom 发光——surprising 边借此实现 S3 强调）。视觉语义等价。

---

## File Structure（改动全景）

**Core 新建:**
- `src/memory/notes/links/mod.rs` — 类型 + re-export
- `src/memory/notes/links/resolve.rs` — 解析策略链（纯函数）
- `src/memory/notes/links/mentions.rs` — 提及扫描器（纯函数）
- `src/memory/dreaming/stages/mention_weave.rs` — mention 软边 dream stage

**Core 修改:**
- `src/memory/store/sqlite/schema/{ddl.rs,migrations.rs,mod.rs}` — 三新列迁移
- `src/memory/notes/wikilink.rs` — `extract_wikilinks_with_alias`
- `src/memory/notes/mod.rs` — 模块注册 + re-export
- `src/memory/store/sqlite/notes/store_impl.rs` — resolve 委托、全字段行、tombstone、backfill、related_edges、outgoing rows
- `src/memory/store/sqlite/notes/helpers.rs` — `build_resolve_context`、`collect_edges_between` 扩列
- `src/memory/notes/store.rs` — trait 新方法 + `GraphEdgeRow`/`OutgoingLinkRow`
- `src/memory/notes/indexer.rs` — finalize_write/rename 接 backfill
- `src/memory/dreaming/stages/note_lint.rs` — 墓碑感知、去 purge
- `src/memory/dreaming/stages/mod.rs`、`src/memory/dreaming/mod.rs` — stage 注册
- `src/builtin_tools/note_manage.rs` — rename action + relations 参数
- `src/gateway/handlers/graph.rs` — rename/delete impl、删占位 stub、node_detail outgoing、query 富化
- `src/gateway/handlers/graph_types.rs` — 新参数/DTO 字段
- `src/gateway/handlers/mod.rs` — 删死注册
- `src/bin/aleph-server/commands/start/builder/handlers/agents.rs` — 注册新 RPC

**Panel 修改:**
- `interfaces/webchat/src/canvas_engine/adapter.rs` — DTO 扩展
- `interfaces/webchat/src/canvas_engine/markdown_excerpt.rs` — wikilink 渲染
- `interfaces/webchat/src/api/graph.rs` — rename/delete 客户端
- `interfaces/webchat/src/platform/wide/views/canvas/galaxy_build.rs` — 相似边/亮度/bridge
- `interfaces/webchat/src/platform/wide/views/canvas/gl/{mod.rs,edges.rs,scene.rs}` — kind 5/6/7 + edge_bright
- `interfaces/webchat/src/platform/wide/views/canvas/node_detail_panel.rs` — 出链徽标 + wikilink 点击 + rename/delete
- `interfaces/webchat/src/platform/wide/views/memory/drawer.rs` — 导航 + chips + rename/delete
- `interfaces/webchat/src/platform/phone/memory/detail.rs` — 导航 + chips

---

## Phase ① 生命周期核心（Task 1–8）

### Task 1: notes_links 三列 schema 迁移

**Files:**
- Modify: `src/memory/store/sqlite/schema/ddl.rs`（`NOTES_LINKS_DDL`）
- Modify: `src/memory/store/sqlite/schema/migrations.rs`（新迁移函数）
- Modify: `src/memory/store/sqlite/schema/mod.rs`（`init_schema` 接线）
- Test: `src/memory/store/sqlite/schema/tests.rs`

**Interfaces:**
- Produces: `notes_links` 新列 `resolved_by TEXT`（NULL=legacy）、`status TEXT NOT NULL DEFAULT 'active'`、`label TEXT`；迁移时旧悬空行（`to_note==to_raw` 且无 `/`）一次性回填 `status='dangling'`。后续所有 Task 依赖这三列存在。

- [ ] **Step 1: 写失败测试**（`schema/tests.rs`，仿既有迁移测试风格）

```rust
#[test]
fn migrate_notes_links_lifecycle_adds_columns_and_backfills_dangling() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    // Legacy-shaped table: no resolved_by / status / label.
    conn.execute_batch(
        "CREATE TABLE notes_links (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            agent_id TEXT NOT NULL DEFAULT 'default',
            from_note TEXT NOT NULL,
            to_note TEXT NOT NULL,
            to_raw TEXT NOT NULL,
            relation TEXT,
            confidence REAL NOT NULL DEFAULT 1.0,
            UNIQUE(agent_id, from_note, to_note));
         INSERT INTO notes_links (agent_id, from_note, to_note, to_raw)
         VALUES ('a', 'p/x', 'rust', 'rust');          -- legacy dangling marker
         INSERT INTO notes_links (agent_id, from_note, to_note, to_raw)
         VALUES ('a', 'p/x', 'ref/rust', 'rust');       -- resolved",
    )
    .unwrap();

    super::migrations::migrate_notes_links_lifecycle(&conn).unwrap();

    let (status_dangling, status_active): (String, String) = conn
        .query_row(
            "SELECT
               (SELECT status FROM notes_links WHERE to_note = 'rust'),
               (SELECT status FROM notes_links WHERE to_note = 'ref/rust')",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(status_dangling, "dangling");
    assert_eq!(status_active, "active");

    // Idempotent: second run is a no-op and must not re-flip statuses.
    conn.execute("UPDATE notes_links SET status = 'tombstone' WHERE to_note = 'rust'", [])
        .unwrap();
    super::migrations::migrate_notes_links_lifecycle(&conn).unwrap();
    let s: String = conn
        .query_row("SELECT status FROM notes_links WHERE to_note = 'rust'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(s, "tombstone", "re-run must not re-backfill");
}
```

- [ ] **Step 2: 实现迁移**（`migrations.rs` 末尾追加，签名/风格仿 `migrate_notes_links_confidence`）

```rust
/// Add the link-lifecycle columns to `notes_links` for existing databases:
/// `resolved_by` (resolution-strategy provenance, NULL = legacy), `status`
/// (`active | dangling | tombstone`), `label` (`[[target|label]]` display
/// alias). One-time backfill: rows carrying the legacy dangling marker
/// (`to_note == to_raw` with no '/') become `status = 'dangling'`.
/// Safe to call multiple times (checks column existence first).
pub fn migrate_notes_links_lifecycle(conn: &Connection) -> Result<(), AlephError> {
    let mut stmt = conn
        .prepare("PRAGMA table_info(notes_links)")
        .map_err(|e| AlephError::config(format!("PRAGMA table_info notes_links: {e}")))?;
    let has_status = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|e| AlephError::config(format!("table_info query: {e}")))?
        .any(|name| name.is_ok_and(|n| n == "status"));
    drop(stmt);

    if has_status {
        return Ok(());
    }
    conn.execute_batch(
        "ALTER TABLE notes_links ADD COLUMN resolved_by TEXT; \
         ALTER TABLE notes_links ADD COLUMN status TEXT NOT NULL DEFAULT 'active'; \
         ALTER TABLE notes_links ADD COLUMN label TEXT; \
         UPDATE notes_links SET status = 'dangling' \
          WHERE to_note = to_raw AND instr(to_raw, '/') = 0;",
    )
    .map_err(|e| AlephError::config(format!("migrate notes_links lifecycle: {e}")))?;
    Ok(())
}
```

- [ ] **Step 3: DDL 更新**（`ddl.rs` 的 `NOTES_LINKS_DDL` 全量替换为）

```rust
pub const NOTES_LINKS_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS notes_links (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    agent_id    TEXT NOT NULL DEFAULT 'default',
    from_note   TEXT NOT NULL,
    to_note     TEXT NOT NULL,
    to_raw      TEXT NOT NULL,
    relation    TEXT,
    confidence  REAL NOT NULL DEFAULT 1.0,
    resolved_by TEXT,
    status      TEXT NOT NULL DEFAULT 'active',
    label       TEXT,
    UNIQUE(agent_id, from_note, to_note)
);
CREATE INDEX IF NOT EXISTS idx_notes_links_from ON notes_links(agent_id, from_note);
CREATE INDEX IF NOT EXISTS idx_notes_links_to ON notes_links(agent_id, to_note);
CREATE INDEX IF NOT EXISTS idx_notes_links_to_raw ON notes_links(agent_id, to_raw);
"#;
```

（新增 `idx_notes_links_to_raw`：Task 5 的定向回填按 `to_raw` 查询。`CREATE INDEX IF NOT EXISTS` 幂等，老库自动补建。）

- [ ] **Step 4: init_schema 接线**（`schema/mod.rs`，紧跟 `migrate_notes_links_confidence` 之后）

```rust
    migrations::migrate_notes_links_confidence(conn)
        .map_err(|e| AlephError::config(format!("migrate notes_links confidence: {e}")))?;
    migrations::migrate_notes_links_lifecycle(conn)?;
```

- [ ] **Step 5: verify** — 确认 `<new-diagnostics>` 无新错误。
- [ ] **Step 6: Commit** — `git add src/memory/store/sqlite/schema/ && git commit -m "store: add notes_links lifecycle columns (resolved_by/status/label)"`

---

### Task 2: `extract_wikilinks_with_alias`

**Files:**
- Modify: `src/memory/notes/wikilink.rs`
- Modify: `src/memory/notes/mod.rs`（re-export）
- Test: 同文件 `#[cfg(test)]`

**Interfaces:**
- Produces: `pub fn extract_wikilinks_with_alias(text: &str) -> Vec<(String, Option<String>)>` — 返回 `(target, display_label)` 对，`crate::memory::notes::extract_wikilinks_with_alias` 可达。Task 4 消费。

- [ ] **Step 1: 写失败测试**（追加进 `wikilink.rs` tests mod）

```rust
    #[test]
    fn extract_with_alias_returns_target_and_label() {
        let text = "see [[rust|Rust 学习]] and [[plain]]";
        assert_eq!(
            extract_wikilinks_with_alias(text),
            vec![
                ("rust".to_string(), Some("Rust 学习".to_string())),
                ("plain".to_string(), None),
            ]
        );
    }

    #[test]
    fn extract_with_alias_empty_label_is_none() {
        // `[[x|]]` — empty alias capture must not surface as Some("").
        assert_eq!(
            extract_wikilinks_with_alias("[[x|]]"),
            vec![("x".to_string(), None)]
        );
    }
```

- [ ] **Step 2: 实现**（`wikilink.rs`，`extract_wikilinks` 之后）

```rust
/// Extract `(target, display_label)` pairs from `text`. The label is the
/// `|alias` part of `[[target|alias]]`; `None` for plain `[[target]]` and for
/// an empty alias (`[[target|]]`).
pub fn extract_wikilinks_with_alias(text: &str) -> Vec<(String, Option<String>)> {
    WIKILINK_RE
        .captures_iter(text)
        .map(|cap| {
            let label = cap
                .get(2)
                .map(|m| m.as_str().to_string())
                .filter(|s| !s.is_empty());
            (cap[1].to_string(), label)
        })
        .collect()
}
```

- [ ] **Step 3: re-export**（`notes/mod.rs`）

```rust
pub use wikilink::{
    extract_wikilinks, extract_wikilinks_with_alias, remove_wikilink, rewrite_wikilinks,
};
```

- [ ] **Step 4: verify diagnostics；Commit** — `notes: add extract_wikilinks_with_alias`

---

### Task 3: `links/` 模块 — 解析策略链（纯函数）

**Files:**
- Create: `src/memory/notes/links/mod.rs`
- Create: `src/memory/notes/links/resolve.rs`
- Modify: `src/memory/notes/mod.rs`（`pub mod links;`）
- Test: `resolve.rs` 内 `#[cfg(test)]`

**Interfaces:**
- Produces（Task 4/5/16 消费，路径 `crate::memory::notes::links::`）:

```rust
pub enum ResolveStrategy { ExactPath, ExactFilename, Alias, Normalized }
impl ResolveStrategy {
    pub const fn as_str(&self) -> &'static str;   // "exact_path" | "exact_filename" | "alias" | "normalized"
    pub const fn confidence(&self) -> f32;        // 1.0 | 0.95 | 0.85 | 0.7
}
pub enum LinkStatus { Active, Dangling, Tombstone }
impl LinkStatus { pub const fn as_str(&self) -> &'static str; pub fn parse(s: &str) -> Self /* unknown → Active */ }
pub struct ResolvedLink { pub target: Option<String>, pub confidence: f32, pub resolved_by: Option<ResolveStrategy> }
pub struct LinkResolveContext { /* 私有四张查找表 */ }
impl LinkResolveContext {
    /// entries: (path, filename, aliases) — 一次性预取自 notes_index
    pub fn new(entries: Vec<(String, String, Vec<String>)>) -> Self;
    pub fn is_empty(&self) -> bool;
}
pub fn resolve(raw_target: &str, ctx: &LinkResolveContext) -> ResolvedLink;
pub fn normalize_link_key(s: &str) -> String;    // 小写 + 全角ASCII折半角 + trim（mentions.rs 复用）
```

- **多候选绝不猜**：任何一档命中 >1 条即落悬空（不是降级到下一档——同名歧义时 alias 档反而可能猜错人）。精确路径档除外（路径唯一）。

- [ ] **Step 1: 写失败测试**（`resolve.rs` 底部）

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> LinkResolveContext {
        LinkResolveContext::new(vec![
            ("reference/rust".into(), "rust".into(), vec![]),
            ("personal/bob-smith".into(), "bob-smith".into(), vec!["Bob".into()]),
            ("project/API Design".into(), "API Design".into(), vec![]),
            // Two notes share filename "dup" → filename tier is ambiguous.
            ("a/dup".into(), "dup".into(), vec![]),
            ("b/dup".into(), "dup".into(), vec![]),
        ])
    }

    #[test]
    fn tier1_exact_path_wins() {
        let r = resolve("reference/rust", &ctx());
        assert_eq!(r.target.as_deref(), Some("reference/rust"));
        assert!((r.confidence - 1.0).abs() < 1e-6);
        assert!(matches!(r.resolved_by, Some(ResolveStrategy::ExactPath)));
    }

    #[test]
    fn tier1_unknown_path_dangles() {
        // Contains '/' but not indexed → dangling, NOT filename fallback.
        let r = resolve("nope/rust", &ctx());
        assert!(r.target.is_none());
        assert_eq!(r.confidence, 0.0);
    }

    #[test]
    fn tier2_unique_filename() {
        let r = resolve("rust", &ctx());
        assert_eq!(r.target.as_deref(), Some("reference/rust"));
        assert!((r.confidence - 0.95).abs() < 1e-6);
        assert!(matches!(r.resolved_by, Some(ResolveStrategy::ExactFilename)));
    }

    #[test]
    fn tier3_alias_when_no_filename_hit() {
        let r = resolve("Bob", &ctx());
        assert_eq!(r.target.as_deref(), Some("personal/bob-smith"));
        assert!((r.confidence - 0.85).abs() < 1e-6);
    }

    #[test]
    fn tier4_normalized_unique() {
        // Case fold: "api design" → "API Design"; full-width fold: "ｒｕｓｔ" → "rust".
        let r = resolve("api design", &ctx());
        assert_eq!(r.target.as_deref(), Some("project/API Design"));
        assert!((r.confidence - 0.7).abs() < 1e-6);
        let r2 = resolve("ｒｕｓｔ", &ctx());
        assert_eq!(r2.target.as_deref(), Some("reference/rust"));
    }

    #[test]
    fn ambiguity_never_guesses() {
        let r = resolve("dup", &ctx());
        assert!(r.target.is_none(), "2 filename candidates must dangle");
        assert!(r.resolved_by.is_none());
    }

    #[test]
    fn miss_dangles_with_zero_confidence() {
        let r = resolve("no-such-note", &ctx());
        assert!(r.target.is_none());
        assert_eq!(r.confidence, 0.0);
    }

    #[test]
    fn normalize_folds_case_and_fullwidth() {
        assert_eq!(normalize_link_key("ＡＢＣ　ｄｅｆ"), "abc def");
        assert_eq!(normalize_link_key("  API Design "), "api design");
    }
}
```

- [ ] **Step 2: 实现 `resolve.rs`**

```rust
//! Wikilink resolution strategy chain — pure functions over a prefetched
//! candidate context (mirrors `graph/`'s pure-over-snapshot pattern, P4).
//!
//! Chain: exact path (1.0) → unique exact filename (0.95) → unique exact
//! alias (0.85) → unique normalized filename-or-alias (0.7) → dangling (0.0).
//! Ambiguity (>1 candidates at a tier) NEVER guesses: a wrong link in a
//! personal vault is worse than no link (deliberately more conservative than
//! codebase-memory-mcp's fuzzy tiers). Reference: registry.c strategy chain.

use std::collections::HashMap;

/// Which strategy resolved a link — persisted into `notes_links.resolved_by`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveStrategy {
    ExactPath,
    ExactFilename,
    Alias,
    Normalized,
}

impl ResolveStrategy {
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::ExactPath => "exact_path",
            Self::ExactFilename => "exact_filename",
            Self::Alias => "alias",
            Self::Normalized => "normalized",
        }
    }

    /// Per-tier confidence (spec §2.3).
    #[must_use]
    pub const fn confidence(&self) -> f32 {
        match self {
            Self::ExactPath => 1.0,
            Self::ExactFilename => 0.95,
            Self::Alias => 0.85,
            Self::Normalized => 0.7,
        }
    }
}

/// Lifecycle status of a `notes_links` row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkStatus {
    Active,
    Dangling,
    Tombstone,
}

impl LinkStatus {
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Dangling => "dangling",
            Self::Tombstone => "tombstone",
        }
    }

    /// Unknown values fall back to `Active` so a foreign writer cannot make
    /// rows invisible to the graph (P7: fail toward visibility).
    #[must_use]
    pub fn parse(s: &str) -> Self {
        match s {
            "dangling" => Self::Dangling,
            "tombstone" => Self::Tombstone,
            _ => Self::Active,
        }
    }
}

/// Result of resolving one raw wikilink target.
#[derive(Debug, Clone)]
pub struct ResolvedLink {
    /// `Some("category/filename")` when uniquely resolved; `None` = dangling.
    pub target: Option<String>,
    /// Strategy confidence; 0.0 when dangling.
    pub confidence: f32,
    pub resolved_by: Option<ResolveStrategy>,
}

impl ResolvedLink {
    fn dangling() -> Self {
        Self {
            target: None,
            confidence: 0.0,
            resolved_by: None,
        }
    }

    fn hit(path: &str, strategy: ResolveStrategy) -> Self {
        Self {
            target: Some(path.to_string()),
            confidence: strategy.confidence(),
            resolved_by: Some(strategy),
        }
    }
}

/// Prefetched candidate tables. Built once per store operation from
/// `notes_index` rows; `resolve` then runs with zero I/O.
pub struct LinkResolveContext {
    paths: HashMap<String, ()>,
    filename_to_paths: HashMap<String, Vec<String>>,
    alias_to_paths: HashMap<String, Vec<String>>,
    /// Normalized filename+alias → paths (tier 4). One merged table: a
    /// normalized key hitting both a filename and an alias of DIFFERENT notes
    /// is ambiguous and must dangle.
    normalized_to_paths: HashMap<String, Vec<String>>,
}

impl LinkResolveContext {
    #[must_use]
    pub fn new(entries: Vec<(String, String, Vec<String>)>) -> Self {
        let mut paths = HashMap::new();
        let mut filename_to_paths: HashMap<String, Vec<String>> = HashMap::new();
        let mut alias_to_paths: HashMap<String, Vec<String>> = HashMap::new();
        let mut normalized_to_paths: HashMap<String, Vec<String>> = HashMap::new();
        let mut push_unique = |m: &mut HashMap<String, Vec<String>>, k: String, p: &str| {
            let v = m.entry(k).or_default();
            if !v.iter().any(|x| x == p) {
                v.push(p.to_string());
            }
        };
        for (path, filename, aliases) in entries {
            push_unique(&mut filename_to_paths, filename.clone(), &path);
            push_unique(
                &mut normalized_to_paths,
                normalize_link_key(&filename),
                &path,
            );
            for a in &aliases {
                push_unique(&mut alias_to_paths, a.clone(), &path);
                push_unique(&mut normalized_to_paths, normalize_link_key(a), &path);
            }
            paths.insert(path, ());
        }
        Self {
            paths,
            filename_to_paths,
            alias_to_paths,
            normalized_to_paths,
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }
}

/// Lowercase + fold full-width ASCII (U+FF01..=U+FF5E and ideographic space
/// U+3000) to half-width + trim. Zero-dep normalization for tier 4.
#[must_use]
pub fn normalize_link_key(s: &str) -> String {
    s.trim()
        .chars()
        .map(|c| match c {
            '\u{3000}' => ' ',
            '\u{FF01}'..='\u{FF5E}' => {
                char::from_u32(c as u32 - 0xFF00 + 0x20).unwrap_or(c)
            }
            _ => c,
        })
        .collect::<String>()
        .to_lowercase()
}

/// Run the strategy chain for one raw wikilink target.
#[must_use]
pub fn resolve(raw_target: &str, ctx: &LinkResolveContext) -> ResolvedLink {
    // Tier 1: contains '/' → exact path or dangling (never falls through:
    // a path-form link names one specific note; guessing another is wrong).
    if raw_target.contains('/') {
        if ctx.paths.contains_key(raw_target) {
            return ResolvedLink::hit(raw_target, ResolveStrategy::ExactPath);
        }
        return ResolvedLink::dangling();
    }
    // Tier 2: unique exact filename.
    match ctx.filename_to_paths.get(raw_target).map(Vec::as_slice) {
        Some([one]) => return ResolvedLink::hit(one, ResolveStrategy::ExactFilename),
        Some([_, ..]) => return ResolvedLink::dangling(), // ambiguous — never guess
        _ => {}
    }
    // Tier 3: unique exact alias.
    match ctx.alias_to_paths.get(raw_target).map(Vec::as_slice) {
        Some([one]) => return ResolvedLink::hit(one, ResolveStrategy::Alias),
        Some([_, ..]) => return ResolvedLink::dangling(),
        _ => {}
    }
    // Tier 4: unique normalized filename-or-alias.
    match ctx
        .normalized_to_paths
        .get(&normalize_link_key(raw_target))
        .map(Vec::as_slice)
    {
        Some([one]) => ResolvedLink::hit(one, ResolveStrategy::Normalized),
        _ => ResolvedLink::dangling(),
    }
}
```

- [ ] **Step 3: `links/mod.rs`**

```rust
//! Link subsystem policy layer — pure functions, zero storage coupling (P4).
//! Resolution strategy chain (`resolve`) + unlinked-mention scanner
//! (`mentions`, Task 15). SQL plumbing stays in `store/sqlite/notes/`;
//! lifecycle triggers are wired at `indexer` / `note_manage` / gateway.

pub mod mentions;
pub mod resolve;

pub use resolve::{
    normalize_link_key, resolve, LinkResolveContext, LinkStatus, ResolveStrategy, ResolvedLink,
};
```

（`mentions` 到 Task 15 才有内容——本 Task 先建空文件 `pub const MENTION_RELATION: &str = "mention";` 占位，编译可过。）

- [ ] **Step 4: `mentions.rs` 占位**

```rust
//! Unlinked-mention scanner (Task 15). Constant lives here from day one so
//! store/stage code can reference the relation label before the scanner lands.

/// Relation label for auto-detected unlinked mentions in `notes_links`.
pub const MENTION_RELATION: &str = "mention";
/// Confidence for mention soft edges (spec §2.3).
pub const MENTION_CONFIDENCE: f32 = 0.35;
```

- [ ] **Step 5: `notes/mod.rs` 注册** — `pub mod links;`（紧跟 `pub mod keyword_linker;` 后）
- [ ] **Step 6: verify diagnostics；Commit** — `notes: add links policy module with resolution strategy chain`

---

### Task 4: store 解析委托 + 全字段边行 + 读端过滤

**Files:**
- Modify: `src/memory/store/sqlite/notes/helpers.rs`（`build_resolve_context` + `collect_edges_between` 扩列）
- Modify: `src/memory/store/sqlite/notes/store_impl.rs`（`index_note` / `load_graph_snapshot` / `relink_unresolved` / `add_link_with_relation`）
- Modify: `src/memory/notes/store.rs`（`GraphEdgeRow`、`get_graph_data` 返回类型）
- Modify: `src/gateway/handlers/graph.rs`（`handle_query_impl`/`handle_neighbors_impl` 适配新返回类型——仅机械适配，富化在 Task 9）
- Test: `store.rs` 底部既有测试模块 + `store_impl` 相关既有测试适配

**Interfaces:**
- Produces:
  - `helpers::build_resolve_context(conn, agent_id) -> rusqlite::Result<links::LinkResolveContext>`
  - `notes/store.rs`: `pub struct GraphEdgeRow { pub from: String, pub to: String, pub relation: Option<String>, pub label: Option<String>, pub confidence: f32 }`
  - `get_graph_data` 返回 `(Vec<NoteIndexEntry>, Vec<GraphEdgeRow>)`
  - `notes_links` 每行写满 `confidence / resolved_by / status / label`
- 行为不变量: 现行解析结果字节不变（新第④档只解析原本悬空的），`load_graph_snapshot`/`collect_edges_between` 只返回 `status='active'` 行。

- [ ] **Step 1: 写失败测试**（`notes/store.rs` 既有 tests mod 追加）

```rust
    #[tokio::test]
    async fn index_note_writes_resolution_provenance() {
        let (store, agent) = make_store().await; // 仿既有 helper；若名称不同用同文件既有构造方式
        // Target note first.
        store
            .index_note(
                &KnowledgeNote {
                    title: "rust".into(),
                    category: "reference".into(),
                    facts: vec!["body".into()],
                    content_hash: "h0".into(),
                    ..Default::default()
                },
                agent,
                "reference",
            )
            .await
            .unwrap();
        // Linker: one resolvable bare link, one dangling.
        store
            .index_note(
                &KnowledgeNote {
                    title: "a".into(),
                    category: "preference".into(),
                    body: Some("see [[rust|The Rust Note]] and [[ghost]]".into()),
                    facts: vec![],
                    links: vec!["rust".into(), "ghost".into()],
                    content_hash: "h1".into(),
                    ..Default::default()
                },
                agent,
                "preference",
            )
            .await
            .unwrap();

        let rows = store.get_outgoing_link_rows("preference/a", agent).await.unwrap();
        let rust = rows.iter().find(|r| r.to_note == "reference/rust").unwrap();
        assert_eq!(rust.status, "active");
        assert_eq!(rust.resolved_by.as_deref(), Some("exact_filename"));
        assert!((rust.confidence - 0.95).abs() < 1e-6);
        assert_eq!(rust.label.as_deref(), Some("The Rust Note"));

        let ghost = rows.iter().find(|r| r.to_note == "ghost").unwrap();
        assert_eq!(ghost.status, "dangling");
        assert_eq!(ghost.confidence, 0.0);
        assert!(ghost.resolved_by.is_none());
    }

    #[tokio::test]
    async fn graph_reads_exclude_non_active_rows() {
        let (store, agent) = make_store().await;
        // ... seed 同上：一条 active(a→reference/rust) + 一条 dangling(a→ghost)
        // （复用上个测试的 seeding 代码）
        let snap = store.load_graph_snapshot(agent).await.unwrap();
        assert!(snap.edges.iter().all(|e| e.to == "reference/rust"),
            "snapshot must contain only active resolved edges");
        let (_, edges) = store.get_graph_data(agent, 50).await.unwrap();
        assert!(edges.iter().all(|e| e.to == "reference/rust"));
        assert_eq!(edges[0].label.as_deref(), Some("The Rust Note"));
    }
```

（`get_outgoing_link_rows` 与 `OutgoingLinkRow` 在 Step 3 一起加——它同时是 Task 9 node_detail 的地基。）

- [ ] **Step 2: `helpers.rs` 加 `build_resolve_context`**

```rust
/// Prefetch every note's (path, filename, aliases) for the agent and build
/// the pure resolution context. One query replaces the per-target lookups
/// the old `resolve_target` closure issued (personal-vault scale: fine).
pub(crate) fn build_resolve_context(
    conn: &rusqlite::Connection,
    agent_id: &str,
) -> rusqlite::Result<crate::memory::notes::links::LinkResolveContext> {
    let mut stmt = conn.prepare(
        "SELECT path, filename, aliases_json FROM notes_index WHERE agent_id = ?1",
    )?;
    let entries = stmt
        .query_map(rusqlite::params![agent_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .filter_map(|r| r.ok())
        .map(|(path, filename, aliases_json)| {
            let aliases: Vec<String> = serde_json::from_str(&aliases_json).unwrap_or_default();
            (path, filename, aliases)
        })
        .collect();
    Ok(crate::memory::notes::links::LinkResolveContext::new(entries))
}
```

- [ ] **Step 3: trait 类型 + 新读方法**（`notes/store.rs`）

`NoteIndexEntry` 定义附近加：

```rust
/// Full active edge row for graph feeds (query/canvas).
#[derive(Debug, Clone)]
pub struct GraphEdgeRow {
    pub from: String,
    pub to: String,
    pub relation: Option<String>,
    pub label: Option<String>,
    pub confidence: f32,
}

/// Full outgoing link row for node detail (includes non-active rows so the
/// panel can show dangling/tombstone links — they are invisible in the graph
/// but must be visible in detail).
#[derive(Debug, Clone)]
pub struct OutgoingLinkRow {
    pub to_note: String,
    pub to_raw: String,
    pub relation: Option<String>,
    pub label: Option<String>,
    pub confidence: f32,
    pub resolved_by: Option<String>,
    pub status: String,
}
```

trait 内：`get_graph_data` 返回类型改为 `Result<(Vec<NoteIndexEntry>, Vec<GraphEdgeRow>), AlephError>`；新增：

```rust
    /// All outgoing link rows for one note, including dangling and tombstone.
    async fn get_outgoing_link_rows(
        &self,
        path: &str,
        agent_id: &str,
    ) -> Result<Vec<OutgoingLinkRow>, AlephError>;
```

- [ ] **Step 4: `index_note` 改造**（store_impl.rs:84-188 区域）。删除 `resolve_target` 闭包，替换为：

```rust
        use crate::memory::notes::links::{self, LinkStatus};
        use crate::memory::notes::wikilink::extract_wikilinks_with_alias;

        let resolve_ctx = super::helpers::build_resolve_context(&conn, agent_id)
            .map_err(|e| AlephError::config(format!("index_note resolve ctx: {e}")))?;

        // Body labels: raw target → display label from `[[target|label]]`.
        let labels: HashMap<String, String> = note
            .body
            .as_deref()
            .map(|b| {
                extract_wikilinks_with_alias(b)
                    .into_iter()
                    .filter_map(|(t, l)| l.map(|l| (t, l)))
                    .collect()
            })
            .unwrap_or_default();

        /// Desired row value per to_note key.
        struct DesiredEdge {
            to_raw: String,
            relation: Option<String>,
            confidence: f32,
            resolved_by: Option<&'static str>,
            status: &'static str,
            label: Option<String>,
        }

        // to_note -> DesiredEdge. Body wikilinks first; typed relations
        // override on the same resolved target (unchanged precedence).
        let mut desired: HashMap<String, DesiredEdge> = HashMap::new();
        for raw_target in &note.links {
            let r = links::resolve(raw_target, &resolve_ctx);
            let (to_note, status) = match &r.target {
                Some(t) => (t.clone(), LinkStatus::Active.as_str()),
                None => (raw_target.clone(), LinkStatus::Dangling.as_str()),
            };
            desired.entry(to_note).or_insert_with(|| DesiredEdge {
                to_raw: raw_target.clone(),
                relation: None,
                confidence: r.confidence,
                resolved_by: r.resolved_by.map(|s| s.as_str()),
                status,
                label: labels.get(raw_target).cloned(),
            });
        }
        for rel in &note.relations {
            let r = links::resolve(&rel.to, &resolve_ctx);
            let (to_note, status) = match &r.target {
                Some(t) => (t.clone(), LinkStatus::Active.as_str()),
                None => (rel.to.clone(), LinkStatus::Dangling.as_str()),
            };
            // Typed relation overrides a plain wikilink to the same target;
            // its confidence is the LLM/tool judgement, not the resolver tier.
            desired.insert(
                to_note,
                DesiredEdge {
                    to_raw: rel.to.clone(),
                    relation: Some(rel.rel_type.clone()),
                    confidence: rel.confidence.clamp(0.0, 1.0),
                    resolved_by: r.resolved_by.map(|s| s.as_str()),
                    status,
                    label: None,
                },
            );
        }
```

existing 扫描 SELECT 扩为 `SELECT to_note, to_raw, relation, confidence, resolved_by, status, label`；unchanged 判断比较全部六个字段；UPSERT 改为：

```rust
            conn.execute(
                "INSERT INTO notes_links \
                   (agent_id, from_note, to_note, to_raw, relation, confidence, resolved_by, status, label) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) \
                 ON CONFLICT(agent_id, from_note, to_note) \
                 DO UPDATE SET to_raw = excluded.to_raw, relation = excluded.relation, \
                               confidence = excluded.confidence, resolved_by = excluded.resolved_by, \
                               status = excluded.status, label = excluded.label",
                params![agent_id, path, to_note, d.to_raw, d.relation, d.confidence, d.resolved_by, d.status, d.label],
            )
```

- [ ] **Step 5: 读端过滤与扩列**
  - `load_graph_snapshot` 边查询 WHERE 追加 `AND status = 'active'`（原 `instr` 谓词保留作 belt——tombstone 行 to_note 含 `/` 但必须被 status 挡掉）。
  - `helpers::collect_edges_between`：SELECT 改 `from_note, to_note, relation, label, confidence`，WHERE 追加 `AND status = 'active'`，返回 `Vec<crate::memory::notes::store::GraphEdgeRow>`。
  - `get_graph_data` 直接透传新类型；`get_neighbors` 内部把 `GraphEdgeRow` 映射回 `(from, to)`（保持 trait 签名不变）。
  - 实现 `get_outgoing_link_rows`（SELECT 全列 WHERE from_note）。

- [ ] **Step 6: `relink_unresolved` 升级** — 谓词改 `WHERE agent_id = ?1 AND status = 'dangling'`；每行用 `links::resolve`（上下文构建一次），命中则：

```rust
                conn.execute(
                    "UPDATE notes_links SET to_note = ?1, confidence = ?2, \
                            resolved_by = ?3, status = 'active' WHERE id = ?4",
                    params![target, r.confidence, r.resolved_by.map(|s| s.as_str()), id],
                )
```

（旧 per-row 双查询删除——上下文一次预取顺带修掉了 N+1。）

- [ ] **Step 7: `add_link_with_relation`**（note_weave 写入口）INSERT 列补 `status` 明确 `'active'`（confidence 已有默认）：VALUES 加 `, 'active'` 对应列 `status`。`replace_co_recall_links` 同样补 `status='active'` 列（否则依赖 DDL 默认值——显式写出，两处一致）。

- [ ] **Step 8: gateway 机械适配** — `handle_query_impl` 的 edges 映射从三元组改为 `GraphEdgeRow`：

```rust
    let edges: Vec<NoteLinkDto> = links
        .into_iter()
        .map(|row| NoteLinkDto {
            from: row.from,
            to: row.to,
            label: row.label,
            kind: Some(row.relation.unwrap_or_else(|| "wikilink".to_string())),
        })
        .collect();
```

（C5 的 label 硬编码 None 在此消失；confidence 字段 Task 9 才上 DTO。）测试 `graph_query_uses_explicit_agent_id` 等应保持绿。

- [ ] **Step 9: 适配既有测试** — store.rs/store_impl 相关既有测试若解构三元组需机械改字段访问；`lint_resolves_pending_links_after_target_appears`（note_lint tests）依赖 relink 行为不变，应保持绿——若红，先查 status 迁移是否把 seed 行标对（`index_note` seed 走新码天然带 status）。
- [ ] **Step 10: verify diagnostics；Commit** — `store: delegate wikilink resolution to links chain, persist provenance columns`

---

### Task 5: 墓碑删除 + 定向即时回填

**Files:**
- Modify: `src/memory/store/sqlite/notes/store_impl.rs`（`remove_note_index` 拆链语义 + 新 `backfill_inbound_links`）
- Modify: `src/memory/notes/store.rs`（trait 方法）
- Modify: `src/memory/notes/indexer.rs`（`finalize_write` / `rename_note` 触发回填）
- Test: `notes/store.rs` tests + `indexer/tests.rs`

**Interfaces:**
- Produces（trait）:

```rust
    /// Re-resolve dangling/tombstone inbound rows whose to_raw matches any of
    /// `keys` (the new/renamed note's filename, full path, and aliases).
    /// Returns rows revived. Targeted via idx_notes_links_to_raw — NOT a
    /// full-table relink sweep.
    async fn backfill_inbound_links(
        &self,
        agent_id: &str,
        keys: &[String],
    ) -> Result<usize, AlephError>;
```

- 语义变更: `remove_note_index` 从「双向删边」改为「出链删 / 入链墓碑」。**全调用者受此影响并有意接受**：`delete_note`（正是想要的 D1）、`rename_note`（墓碑行随后被源笔记重索引 reconcile 清理或回填复活）、`full_rebuild` 孤儿清理（文件已消失＝事实删除，墓碑正确）、`Supersede`（旧笔记被替代，入链墓碑正确）。

- [ ] **Step 1: 写失败测试**（`notes/store.rs` tests）

```rust
    #[tokio::test]
    async fn delete_tombstones_inbound_and_recreate_revives() {
        let (store, agent) = make_store().await;
        // b links to a.
        store.index_note(&note_with_body("a", "reference", "target body"), agent, "reference").await.unwrap();
        store.index_note(&note_with_body_links("b", "plan", "see [[a]]", &["a"]), agent, "plan").await.unwrap();

        // Delete a → inbound row survives as tombstone, outgoing rows of a gone.
        store.remove_note_index("reference/a", agent).await.unwrap();
        let rows = store.get_outgoing_link_rows("plan/b", agent).await.unwrap();
        let t = rows.iter().find(|r| r.to_raw == "a").expect("row must survive");
        assert_eq!(t.status, "tombstone");
        // Tombstone must NOT appear in graph feeds.
        let snap = store.load_graph_snapshot(agent).await.unwrap();
        assert!(snap.edges.is_empty());

        // Recreate a (same filename) → backfill revives the edge.
        store.index_note(&note_with_body("a", "reference", "reborn"), agent, "reference").await.unwrap();
        let revived = store
            .backfill_inbound_links(agent, &["a".into(), "reference/a".into()])
            .await
            .unwrap();
        assert_eq!(revived, 1);
        let rows = store.get_outgoing_link_rows("plan/b", agent).await.unwrap();
        let r = rows.iter().find(|r| r.to_raw == "a").unwrap();
        assert_eq!(r.status, "active");
        assert_eq!(r.to_note, "reference/a");
    }
```

（`note_with_body`/`note_with_body_links` 若无现成 helper，就地写两个小构造函数，字段仿 Task 4 测试。）

- [ ] **Step 2: `remove_note_index` 改造** — 把 links 一条 DELETE 拆为：

```rust
        tx.execute(
            "DELETE FROM notes_links WHERE from_note = ?1 AND agent_id = ?2",
            params![path, agent_id],
        )
        .map_err(|e| AlephError::config(format!("remove_note_index outgoing links: {e}")))?;

        // D1 tombstone semantics: inbound rows are marked, never destroyed —
        // the source note's body keeps its [[link]] text and the row revives
        // via backfill_inbound_links if a same-name note is recreated.
        tx.execute(
            "UPDATE notes_links SET status = 'tombstone' WHERE to_note = ?1 AND agent_id = ?2",
            params![path, agent_id],
        )
        .map_err(|e| AlephError::config(format!("remove_note_index tombstone inbound: {e}")))?;
```

- [ ] **Step 3: 实现 `backfill_inbound_links`**（store_impl，放 `relink_unresolved` 旁）

```rust
    async fn backfill_inbound_links(
        &self,
        agent_id: &str,
        keys: &[String],
    ) -> Result<usize, AlephError> {
        use crate::memory::notes::links;
        if keys.is_empty() {
            return Ok(0);
        }
        let conn = lock_conn!(self)?;
        let placeholders: Vec<String> = (2..=keys.len() + 1).map(|i| format!("?{i}")).collect();
        let sql = format!(
            "SELECT id, to_raw FROM notes_links \
             WHERE agent_id = ?1 AND status IN ('dangling','tombstone') \
               AND to_raw IN ({})",
            placeholders.join(", ")
        );
        let mut params_v: Vec<Box<dyn rusqlite::types::ToSql>> =
            vec![Box::new(agent_id.to_string())];
        for k in keys {
            params_v.push(Box::new(k.clone()));
        }
        let params_ref: Vec<&dyn rusqlite::types::ToSql> =
            params_v.iter().map(|p| p.as_ref()).collect();
        let rows: Vec<(i64, String)> = {
            let mut stmt = conn
                .prepare(&sql)
                .map_err(|e| AlephError::config(format!("backfill prep: {e}")))?;
            stmt.query_map(params_ref.as_slice(), |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(|e| AlephError::config(format!("backfill scan: {e}")))?
            .filter_map(|r| r.ok())
            .collect()
        };
        if rows.is_empty() {
            return Ok(0);
        }
        let ctx = super::helpers::build_resolve_context(&conn, agent_id)
            .map_err(|e| AlephError::config(format!("backfill ctx: {e}")))?;
        let mut revived = 0usize;
        for (id, raw) in rows {
            let r = links::resolve(&raw, &ctx);
            if let Some(target) = r.target {
                conn.execute(
                    "UPDATE notes_links SET to_note = ?1, confidence = ?2, \
                            resolved_by = ?3, status = 'active' WHERE id = ?4",
                    params![target, r.confidence, r.resolved_by.map(|s| s.as_str()), id],
                )
                .map_err(|e| AlephError::config(format!("backfill update: {e}")))?;
                revived += 1;
            }
        }
        Ok(revived)
    }
```

**注意 UNIQUE 冲突面**: 复活 tombstone 行时若源笔记已另建同 target 的 active 行（同 `(agent,from,to)`），UPDATE 会撞 UNIQUE。防御：UPDATE 前先 `DELETE FROM notes_links WHERE agent_id=? AND from_note=(SELECT from_note FROM notes_links WHERE id=?) AND to_note=? AND id != ?`——实现为先查该行 from_note，再执行一条 `DELETE ... WHERE from_note=? AND to_note=? AND id != ?`，然后 UPDATE。测试补一条 UNIQUE 冲突回归（同 from 先 dangling 后手写重链再 backfill 不 panic）。

- [ ] **Step 4: indexer 触发点**
  - `finalize_write`（indexer.rs:189）在 `index_note` 成功后追加（best-effort）：

```rust
        // Backfill: this write may resolve other notes' dangling links to this
        // note (create / recreate-after-delete) — targeted by to_raw, P7 best-effort.
        let mut keys: Vec<String> = vec![
            safe_title.to_string(),
            format!("{category}/{safe_title}"),
        ];
        keys.extend(reparsed.aliases.iter().cloned());
        if let Err(e) = self.store.backfill_inbound_links(agent_id, &keys).await {
            tracing::warn!(error = %e, "finalize_write: inbound backfill failed (non-fatal)");
        }
```

（`reparsed` 是该函数既有变量；注意在 `index_note` 调用后、orientation 前插入，`reparsed` 须先于消费克隆 aliases——直接借用即可。）
  - `rename_note`（indexer.rs:626）末尾（两个 notify_orientation 之前）追加同样的 best-effort 回填，keys = `[safe_new.clone(), format!("{category}/{safe_new}")]`（rename 后的 aliases 未变，链接改写已由级联处理；新名可能解析别人的悬空链）。

- [ ] **Step 5: indexer 测试**（`indexer/tests.rs` 追加）：`write_note` 创建目标后，另一 agent 内既有 dangling 行被复活（走 finalize_write 路径端到端）。
- [ ] **Step 6: verify diagnostics；Commit** — `notes: tombstone delete semantics + targeted inbound backfill`

---

### Task 6: NoteLint 墓碑感知（去破坏性 purge）

**Files:**
- Modify: `src/memory/dreaming/stages/note_lint.rs`
- Test: 同文件 tests mod

**Interfaces:**
- Consumes: Task 4 的 `get_outgoing_link_rows`（含 status）。
- 行为: ① 只对 `status='dangling'` 行做 fuzzy 修复；② `tombstone` 行完全跳过（正文 `[[已删]]` 保留——D1）；③ **删除 purge 分支**（`remove_wikilink` 调用及其 TOCTOU recheck 一并删），`links_purged` 计数保留但恒 0（报告字段兼容）；④ 末尾 `relink_unresolved` sweep 保留。

- [ ] **Step 1: 写失败测试**

```rust
    #[tokio::test]
    async fn lint_never_purges_tombstoned_links() {
        let (ctx, store) = build_test_dream_ctx().await;
        let agent = ctx.agent_id.clone();
        // On-disk source note whose body carries [[gone]].
        let dir = ctx.indexer.memory_dir().join(&agent).join("plan");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("keeper.md"),
            "---\ncategory: plan\ntags: []\ncreated: 2026-01-01\nupdated: 2026-01-01\n---\n\nsee [[gone]]\n",
        )
        .unwrap();
        ctx.indexer
            .index_file(&agent, "plan", &dir.join("keeper.md"))
            .await
            .unwrap();
        // Simulate delete of the (never-indexed-here) target by hand-marking
        // the row tombstone — the shape delete_note now produces.
        // (Direct SQL via store helper: use remove-note flow instead when the
        // target existed; here the row is dangling → flip it to tombstone.)
        store
            .set_link_status_for_test("plan/keeper", "gone", "tombstone", &agent)
            .await;

        let mut lint_ctx = ctx;
        lint_ctx.notes = vec![NoteEntry {
            path: "plan/keeper".into(),
            category: "plan".into(),
            tags: vec![],
            created_at: 0,
            updated_at: 0,
            content_hash: "h".into(),
        }];
        NoteLintStage.execute(lint_ctx).await.unwrap();

        let body = std::fs::read_to_string(dir.join("keeper.md")).unwrap();
        assert!(body.contains("[[gone]]"), "tombstoned link text must survive lint");
    }
```

（`set_link_status_for_test`：在 store_impl 的 `#[cfg(test)]` impl 块加一个直写 UPDATE 的测试 helper；或改用真实 delete 流程 seed——实现者二选一，测试意图不变：**墓碑行过 lint 后正文不动**。）

- [ ] **Step 2: 改造检测循环** — `get_outgoing_links(path,...)` 换成 `get_outgoing_link_rows(path, ...)`；循环体开头：

```rust
            for row in outgoing {
                match crate::memory::notes::links::LinkStatus::parse(&row.status) {
                    // Active rows resolve by construction; tombstones are
                    // deleted targets whose body text is preserved (D1).
                    crate::memory::notes::links::LinkStatus::Active
                    | crate::memory::notes::links::LinkStatus::Tombstone => continue,
                    crate::memory::notes::links::LinkStatus::Dangling => {}
                }
                let target = row.to_raw.clone();
                broken_links_found += 1;
                // ... 原 fuzzy_matches 逻辑保留（对 target 做大小写唯一匹配 → rewrite_wikilinks）...
```

原「`candidates.is_empty()` → find_by_filename 前置检查」整段删除（status 已是单一真源）；`fuzzy_matches.is_empty()` 分支从 purge 改为 `continue`（留悬空，下周期 backfill/relink 再试）；删除 `remove_wikilink` import。

- [ ] **Step 3: 适配既有测试** — `lint_resolves_pending_links_after_target_appears` / `lint_leaves_ambiguous_links_unresolved` 依赖 relink sweep 行为，应保持绿。任何引用 purge 行为的断言删除。
- [ ] **Step 4: verify diagnostics；Commit** — `dreaming: note_lint respects tombstones, drops destructive purge`

---

### Task 7: note_manage rename action + relations 参数

**Files:**
- Modify: `src/builtin_tools/note_manage.rs`
- Modify: `src/memory/notes/indexer.rs`（新 `append_relations` 方法）
- Test: `note_manage.rs` tests mod + `indexer/tests.rs`

**Interfaces:**
- Produces:
  - `NoteManageAction::Rename`，args 新增 `new_title: Option<String>`、`relations: Option<Vec<NoteRelationArg>>`；
  - `pub struct NoteRelationArg { pub to: String, #[serde(rename = "type")] pub rel_type: String }`（derive 同 args：Debug/Clone/Serialize/Deserialize/JsonSchema）；
  - `NoteIndexer::append_relations(&self, agent_id: &str, note_path: &str, relations: &[Relation]) -> Result<(), AlephError>`。
- 语义: 工具写入的 typed relation confidence=1.0（显式声明）；create/update 在 `set_body`/`add_links` 后合并 relations（按 `(to, rel_type)` 去重）；append 带 relations 时走 `append_relations`；rename 直通 `NoteIndexer::rename_note`（其内含入链级联 + Task 5 的回填）。

- [ ] **Step 1: 写失败测试**

```rust
    #[tokio::test]
    async fn rename_action_renames_and_cascades_inbound_links() {
        let (_d, tool) = mk_tool();
        tool.call(create_args("old-name", "- body")).await.unwrap();
        // linker references old-name
        let mut linker = create_args("linker", "- see [[old-name]]");
        linker.links = Some(vec!["old-name".into()]);
        tool.call(linker).await.unwrap();

        let r = tool
            .call(NoteManageArgs {
                action: NoteManageAction::Rename,
                category: Some("learning".into()),
                filename: Some("old-name".into()),
                new_title: Some("new-name".into()),
                ..blank_args()
            })
            .await
            .unwrap();
        assert!(r.success);
        assert_eq!(r.note_path.as_deref(), Some("learning/new-name"));
        // Inbound body text rewritten by the cascade.
        let linker_body = std::fs::read_to_string(
            tool_memory_dir(&tool).join("default/learning/linker.md"),
        )
        .unwrap();
        assert!(linker_body.contains("[[new-name]]"));
        assert!(!linker_body.contains("[[old-name]]"));
    }

    #[tokio::test]
    async fn create_with_relations_lands_in_frontmatter() {
        let (_d, tool) = mk_tool();
        let mut args = create_args("super-note", "- replaces the old one");
        args.relations = Some(vec![NoteRelationArg {
            to: "learning/old-note".into(),
            rel_type: "supersedes".into(),
        }]);
        let r = tool.call(args).await.unwrap();
        assert!(r.success);
        let body = std::fs::read_to_string(
            tool_memory_dir(&tool).join("default/learning/super-note.md"),
        )
        .unwrap();
        assert!(body.contains("relations:"), "got:\n{body}");
        assert!(body.contains("to: learning/old-note"));
        assert!(body.contains("type: supersedes"));
    }
```

（`blank_args()`：把既有 `create_args` 改造出一个全 None 基座 helper，避免逐字段列写；`tool_memory_dir`：给测试加 `impl NoteManageTool { #[cfg(test)] fn memory_dir(&self)... }` 或直接用 `tempdir` 路径。既有 tests 里 `create_args` 需补 `new_title: None, relations: None` 两字段——用 `..blank_args()` 或逐个补齐。）

- [ ] **Step 2: args/enum 扩展** — `NoteManageAction` 加 `Rename`；`NoteManageArgs` 加两字段（`#[serde(default)]`，doc 注释说明 rename 用 `filename`+`new_title`、relations 的 `supersedes/superseded_by/contradicts` 是检索强边）；定义 `NoteRelationArg`。

- [ ] **Step 3: `handle_rename`**

```rust
    async fn handle_rename(&self, args: &NoteManageArgs) -> Result<NoteManageResult> {
        let agent_id_owned = self.resolve_agent_id(args)?;
        let agent_id = agent_id_owned.as_str();
        let filename = args
            .filename
            .as_deref()
            .ok_or_else(|| AlephError::tool("filename is required for rename"))?;
        let new_title = args
            .new_title
            .as_deref()
            .ok_or_else(|| AlephError::tool("new_title is required for rename"))?;
        let safe_old = sanitize_title(filename)?;
        let safe_new = sanitize_title(new_title)?;
        if safe_old == safe_new {
            return Err(AlephError::tool("new_title equals current filename"));
        }
        // rename_note locates the category itself (find_by_filename); with
        // duplicate filenames across categories it renames the first hit —
        // callers can disambiguate by deleting/recreating instead.
        self.indexer
            .rename_note(agent_id, &safe_old, &safe_new)
            .await
            .map_err(|e| AlephError::tool(format!("Failed to rename note: {e}")))?;
        // Resolve the new category for an honest note_path in the result.
        let new_paths = self
            .indexer
            .store()
            .find_by_filename(&safe_new, agent_id)
            .await
            .unwrap_or_default();
        let note_path = new_paths
            .first()
            .cloned()
            .unwrap_or_else(|| format!("other/{safe_new}"));
        info!(old = %safe_old, new = %safe_new, "Note renamed");
        self.refresh_embedding(
            agent_id,
            note_path.split('/').next().unwrap_or("other"),
            &safe_new,
        )
        .await;
        Ok(NoteManageResult {
            related_notes: None,
            success: true,
            message: format!(
                "Renamed '{safe_old}' → '{safe_new}'. Inbound [[wikilinks]] were rewritten."
            ),
            note_path: Some(note_path),
            content: None,
            notes: None,
        })
    }
```

`call()` 的 match 加 `NoteManageAction::Rename => self.handle_rename(&args).await,`；`record_lifecycle_event` 的 match 给 Rename 走 `log_note_updated(note_path, String::new(), "note_manage rename".to_string(), EventActor::Agent)`。

- [ ] **Step 4: relations 合并（create/update/append）** — `handle_create` 与 `handle_update` 在 `add_links` 调用后追加：

```rust
        if let Some(rels) = &args.relations {
            merge_relations(&mut note, rels);
        }
```

helper（文件底部 Helpers 区）：

```rust
/// Merge tool-authored typed relations into the note's frontmatter set,
/// deduped by (to, rel_type). Tool-authored = explicit statement → confidence 1.0.
fn merge_relations(note: &mut KnowledgeNote, rels: &[NoteRelationArg]) {
    for r in rels {
        let exists = note
            .relations
            .iter()
            .any(|x| x.to == r.to && x.rel_type == r.rel_type);
        if !exists {
            note.relations.push(
                crate::memory::notes::Relation {
                    to: r.to.clone(),
                    rel_type: r.rel_type.clone(),
                    confidence: 1.0,
                }
                .clamped(),
            );
        }
    }
}
```

`handle_append` 在 `append_to_note` 成功后：

```rust
        if let Some(rels) = &args.relations {
            let parsed: Vec<crate::memory::notes::Relation> = rels
                .iter()
                .map(|r| crate::memory::notes::Relation {
                    to: r.to.clone(),
                    rel_type: r.rel_type.clone(),
                    confidence: 1.0,
                })
                .collect();
            self.indexer
                .append_relations(agent_id, &note_path, &parsed)
                .await
                .map_err(|e| AlephError::tool(format!("Failed to append relations: {e}")))?;
        }
```

- [ ] **Step 5: `NoteIndexer::append_relations`**（indexer.rs，放 `append_to_note` 后；镜像其读-改-写形态）

```rust
    /// Merge typed relations into an existing note's frontmatter (deduped by
    /// (to, rel_type)), bump updated_at, rewrite + re-index. No-op when every
    /// relation already exists.
    pub async fn append_relations(
        &self,
        agent_id: &str,
        note_path: &str,
        relations: &[crate::memory::notes::Relation],
    ) -> Result<(), AlephError> {
        let (category, filename) =
            note_path
                .split_once('/')
                .ok_or_else(|| AlephError::ConfigError {
                    message: format!(
                        "Invalid note_path (expected 'category/filename'): {note_path}"
                    ),
                    suggestion: None,
                })?;
        let safe_cat = sanitize_title(category).unwrap_or_else(|_| "other".to_string());
        let safe_title = sanitize_title(filename)?;
        let file_path = self
            .memory_dir
            .join(agent_id)
            .join(&safe_cat)
            .join(format!("{safe_title}.md"));
        let content = fs::read_to_string(&file_path)
            .await
            .map_err(|e| AlephError::config(format!("append_relations read: {e}")))?;
        let mut note = KnowledgeNote::from_markdown(filename, &content)?;
        let mut added = false;
        for r in relations {
            if !note
                .relations
                .iter()
                .any(|x| x.to == r.to && x.rel_type == r.rel_type)
            {
                note.relations.push(r.clone().clamped());
                added = true;
            }
        }
        if !added {
            return Ok(());
        }
        note.updated_at = chrono::Utc::now().timestamp();
        let md = note.to_markdown();
        note.content_hash = sha2_hash(&md);
        atomic_write_file(&file_path, &md).await?;
        self.store.index_note(&note, agent_id, &safe_cat).await?;
        self.notify_orientation(agent_id, &safe_cat, &safe_title);
        Ok(())
    }
```

- [ ] **Step 6: DESCRIPTION/examples 更新** — DESCRIPTION 增补一句：`'rename' renames a note and rewrites every inbound [[wikilink]]. Typed 'relations' ([{to, type}]) declare semantic edges; supersedes/superseded_by/contradicts are force-surfaced at retrieval.`；examples 加 `note_manage(action='rename', filename='old-name', new_title='new-name')` 与 `note_manage(action='update', ..., relations=[{to: 'plan/old-roadmap', type: 'supersedes'}])`。
- [ ] **Step 7: verify diagnostics；Commit** — `tools: note_manage rename action + typed relations authoring`

---

### Task 8: gateway rename/delete RPC + 死注册清理

**Files:**
- Modify: `src/gateway/handlers/graph_types.rs`（两个 Params）
- Modify: `src/gateway/handlers/graph.rs`（两个 impl + 删 5 个占位 stub）
- Modify: `src/gateway/handlers/mod.rs`（删 5 行死注册，graph.rs:47-97 的占位函数一并删）
- Modify: `src/bin/aleph-server/commands/start/builder/handlers/agents.rs`（`register_graph_handlers` 注册两个新方法）
- Test: `graph.rs` tests mod

**Interfaces:**
- Produces（JSON-RPC，Panel 消费，权限面与 `graph.update_note` 完全一致——注册在同一 `if let Some(indexer)` 块，经登录墙后 operator 全权）:
  - `graph.rename_note {node_id, new_title, agent_id?}` → `{node_id, new_id, renamed: true}`
  - `graph.delete_note {node_id, agent_id?}` → `{node_id, deleted: true}`

- [ ] **Step 1: 写失败测试**（graph.rs tests，仿 `update_note_persists_content_verbatim` 构造）

```rust
    #[tokio::test]
    async fn rename_note_moves_file_and_reindexes() {
        let memory_dir = std::env::temp_dir().join(format!("rename_rpc_{}", Uuid::new_v4()));
        let db = make_db();
        let indexer = Arc::new(NoteIndexer::new(memory_dir.clone(), db.clone()));
        let agent = crate::routing::DEFAULT_AGENT_ID;
        // Seed a real on-disk note through the indexer write path.
        indexer
            .write_note_raw(agent, "reference", "OldTitle",
                "---\ncategory: reference\ntags: []\ncreated: \"2024-01-01\"\nupdated: \"2024-01-01\"\n---\n\n- fact\n")
            .await
            .unwrap();

        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "graph.rename_note".into(),
            params: Some(serde_json::json!({
                "node_id": "reference/OldTitle", "new_title": "NewTitle", "agent_id": agent })),
            id: Some(serde_json::json!(1)),
        };
        let resp = handle_rename_note_impl(req, indexer).await;
        assert!(resp.error.is_none(), "{:?}", resp.error);
        assert!(memory_dir.join(agent).join("reference/NewTitle.md").exists());
        assert!(!memory_dir.join(agent).join("reference/OldTitle.md").exists());
        assert!(db.get_note_index("reference/NewTitle", agent).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn delete_note_removes_file_and_index() {
        let memory_dir = std::env::temp_dir().join(format!("delete_rpc_{}", Uuid::new_v4()));
        let db = make_db();
        let indexer = Arc::new(NoteIndexer::new(memory_dir.clone(), db.clone()));
        let agent = crate::routing::DEFAULT_AGENT_ID;
        indexer
            .write_note_raw(agent, "plan", "Doomed",
                "---\ncategory: plan\ntags: []\ncreated: \"2024-01-01\"\nupdated: \"2024-01-01\"\n---\n\n- x\n")
            .await
            .unwrap();
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "graph.delete_note".into(),
            params: Some(serde_json::json!({ "node_id": "plan/Doomed", "agent_id": agent })),
            id: Some(serde_json::json!(1)),
        };
        let resp = handle_delete_note_impl(req, indexer).await;
        assert!(resp.error.is_none(), "{:?}", resp.error);
        assert!(!memory_dir.join(agent).join("plan/Doomed.md").exists());
        assert!(db.get_note_index("plan/Doomed", agent).await.unwrap().is_none());
    }
```

- [ ] **Step 2: Params**（graph_types.rs）

```rust
// === graph.rename_note ===
#[derive(Debug, Deserialize)]
pub struct GraphRenameNoteParams {
    /// Note path `"category/title"` (same id as `graph.node_detail`).
    pub node_id: String,
    /// New filename (without `.md`); sanitized server-side.
    pub new_title: String,
    #[serde(default)]
    pub agent_id: Option<String>,
}

// === graph.delete_note ===
#[derive(Debug, Deserialize)]
pub struct GraphDeleteNoteParams {
    pub node_id: String,
    #[serde(default)]
    pub agent_id: Option<String>,
}
```

- [ ] **Step 3: impl**（graph.rs，放 `handle_update_note_impl` 后；traversal 守卫逐字复用 update_note 的检查）

```rust
/// Real implementation of `graph.rename_note`: renames the file, rewrites
/// every inbound `[[old]]` wikilink, re-indexes, and backfills.
pub async fn handle_rename_note_impl(
    req: JsonRpcRequest,
    indexer: Arc<NoteIndexer<SqliteMemoryBackend>>,
) -> JsonRpcResponse {
    let params: GraphRenameNoteParams = match req
        .params
        .as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok())
    {
        Some(p) => p,
        None => {
            return JsonRpcResponse::error(
                req.id,
                INVALID_PARAMS,
                "Missing required params: node_id, new_title".to_string(),
            )
        }
    };
    let agent_id = params
        .agent_id
        .as_deref()
        .unwrap_or(crate::routing::DEFAULT_AGENT_ID);
    let Some((category, title)) = params.node_id.split_once('/') else {
        return JsonRpcResponse::error(
            req.id,
            INVALID_PARAMS,
            format!("Invalid node_id (expected \"category/title\"): {}", params.node_id),
        );
    };
    if category.contains("..")
        || category.contains('\\')
        || agent_id.contains("..")
        || agent_id.contains('/')
        || agent_id.contains('\\')
    {
        return JsonRpcResponse::error(
            req.id,
            INVALID_PARAMS,
            "node_id / agent_id must not contain path traversal components".to_string(),
        );
    }
    match indexer.rename_note(agent_id, title, &params.new_title).await {
        Ok(()) => JsonRpcResponse::success(
            req.id,
            serde_json::json!({
                "node_id": params.node_id,
                "new_id": format!("{category}/{}", params.new_title),
                "renamed": true
            }),
        ),
        Err(e) => {
            JsonRpcResponse::error(req.id, INTERNAL_ERROR, format!("rename_note failed: {e}"))
        }
    }
}

/// Real implementation of `graph.delete_note`: removes file + index; inbound
/// links become tombstones (D1 — source bodies untouched, revivable).
pub async fn handle_delete_note_impl(
    req: JsonRpcRequest,
    indexer: Arc<NoteIndexer<SqliteMemoryBackend>>,
) -> JsonRpcResponse {
    let params: GraphDeleteNoteParams = match req
        .params
        .as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok())
    {
        Some(p) => p,
        None => {
            return JsonRpcResponse::error(
                req.id,
                INVALID_PARAMS,
                "Missing required param: node_id".to_string(),
            )
        }
    };
    let agent_id = params
        .agent_id
        .as_deref()
        .unwrap_or(crate::routing::DEFAULT_AGENT_ID);
    let Some((category, title)) = params.node_id.split_once('/') else {
        return JsonRpcResponse::error(
            req.id,
            INVALID_PARAMS,
            format!("Invalid node_id (expected \"category/title\"): {}", params.node_id),
        );
    };
    if category.contains("..")
        || category.contains('\\')
        || agent_id.contains("..")
        || agent_id.contains('/')
        || agent_id.contains('\\')
    {
        return JsonRpcResponse::error(
            req.id,
            INVALID_PARAMS,
            "node_id / agent_id must not contain path traversal components".to_string(),
        );
    }
    match indexer.delete_note(agent_id, category, title).await {
        Ok(()) => JsonRpcResponse::success(
            req.id,
            serde_json::json!({ "node_id": params.node_id, "deleted": true }),
        ),
        Err(e) => {
            JsonRpcResponse::error(req.id, INTERNAL_ERROR, format!("delete_note failed: {e}"))
        }
    }
}
```

graph_types 的 use 行补 `GraphRenameNoteParams, GraphDeleteNoteParams`。

- [ ] **Step 4: 死注册清理（B7）** — `gateway/handlers/mod.rs` 删这 5 行：

```rust
        registry.register("graph.query", graph::handle_query);
        registry.register("graph.neighbors", graph::handle_neighbors);
        registry.register("graph.node_detail", graph::handle_node_detail);
        registry.register("graph.search", graph::handle_search);
        registry.register("graph.update_note", graph::handle_update_note);
```

连同 graph.rs:44-97 的五个占位 `handle_query`/`handle_neighbors`/`handle_node_detail`/`handle_search`/`handle_update_note` 函数删除（live 版本在 agent_init 注册；未 wire 时"method not found"比假 internal error 诚实）。grep 确认无其他引用。

- [ ] **Step 5: 注册**（agents.rs `register_graph_handlers` 的 `if let Some(indexer)` 块内追加）

```rust
        {
            let indexer = ::std::sync::Arc::clone(indexer);
            server
                .handlers_mut()
                .register("graph.rename_note", move |req| {
                    let indexer = ::std::sync::Arc::clone(&indexer);
                    async move { graph::handle_rename_note_impl(req, indexer).await }
                });
        }
        {
            let indexer = ::std::sync::Arc::clone(indexer);
            server
                .handlers_mut()
                .register("graph.delete_note", move |req| {
                    let indexer = ::std::sync::Arc::clone(&indexer);
                    async move { graph::handle_delete_note_impl(req, indexer).await }
                });
        }
```

（注意原块只克隆一次 indexer 注册 update_note——改为块内先各自 clone，保持既有 update_note 注册不动。）

- [ ] **Step 6: verify diagnostics；Commit** — `gateway: graph.rename_note + graph.delete_note RPCs, drop dead placeholder registrations`

---

## Phase ② surface 连线（Task 9–11）

### Task 9: graph.query / node_detail 富化（core 侧）

**Files:**
- Modify: `src/memory/notes/store.rs`（trait 新方法 `related_edges_between`）
- Modify: `src/memory/store/sqlite/notes/store_impl.rs`（实现）
- Modify: `src/gateway/handlers/graph_types.rs`（DTO 字段）
- Modify: `src/gateway/handlers/graph.rs`（query 拼装 + node_detail outgoing）
- Test: graph.rs tests + store.rs tests

**Interfaces:**
- Produces:
  - trait: `async fn related_edges_between(&self, agent_id: &str, visible: &std::collections::HashSet<String>, per_node: usize) -> Result<Vec<(String, String, f32)>, AlephError>` — `notes_graph_related` 中两端都可见的行，每 node 取 score 最高的 `per_node` 条。
  - `NoteLinkDto` 新增 `#[serde(default, skip_serializing_if = "Option::is_none")] pub confidence: Option<f32>`；
  - `GraphQueryResponse` 新增 `#[serde(default, skip_serializing_if = "Vec::is_empty")] pub bridge_nodes: Vec<String>` 与 `pub surprising_edges: Vec<(String, String)>`（同 serde 属性）；
  - `NoteDetailResponse` 新增 `pub outgoing: Vec<OutgoingLinkDto>`，其中：

```rust
#[derive(Debug, Serialize)]
pub struct OutgoingLinkDto {
    pub to: String,          // resolved path (active/tombstone) or raw (dangling)
    pub raw: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub confidence: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_by: Option<String>,
    pub status: String,      // active | dangling | tombstone
}
```

  - 相似边 kind 字符串 = `"related_similarity"`，confidence=None（score 不伪装成置信度）；每节点 top-3；与真实边按无向对去重。
  - `sparse` insight 有意不进 canvas（spec S3）。

- [ ] **Step 1: 写失败测试**（graph.rs tests）

```rust
    #[tokio::test]
    async fn graph_query_carries_similarity_edges_and_insights() {
        let db = make_db();
        let agent = crate::routing::DEFAULT_AGENT_ID;
        let a = make_note("A", "concept", vec![]);
        let b = make_note("B", "concept", vec![]);
        db.index_note(&a, agent, "concept").await.unwrap();
        db.index_note(&b, agent, "concept").await.unwrap();
        // Materialized artifacts (what GraphRecompute would write).
        db.replace_graph_related(agent, &[("concept/A".into(), "concept/B".into(), 3.2)])
            .await
            .unwrap();
        db.replace_graph_insights(
            agent,
            &[
                ("bridge".into(), serde_json::json!(["concept/A"]).to_string()),
                (
                    "surprising".into(),
                    serde_json::json!([{"from": "concept/A", "to": "concept/B", "score": 0.9}])
                        .to_string(),
                ),
            ],
        )
        .await
        .unwrap();

        let resp = handle_query_impl(query_request(50, Some(agent)), db).await;
        assert!(resp.error.is_none(), "{:?}", resp.error);
        let result: GraphQueryResponse =
            serde_json::from_value(resp.result.unwrap()).unwrap();
        assert!(
            result.edges.iter().any(|e| e.kind.as_deref() == Some("related_similarity")),
            "similarity edge must surface: {:?}",
            result.edges
        );
        assert_eq!(result.bridge_nodes, vec!["concept/A"]);
        assert_eq!(result.surprising_edges, vec![("concept/A".into(), "concept/B".into())]);
    }

    #[tokio::test]
    async fn node_detail_lists_outgoing_with_provenance() {
        let db = make_db();
        let agent = crate::routing::DEFAULT_AGENT_ID;
        db.index_note(&make_note("t", "concept", vec![]), agent, "concept").await.unwrap();
        db.index_note(&make_note("s", "concept", vec!["concept/t", "ghost"]), agent, "concept")
            .await
            .unwrap();
        let resp = handle_node_detail_impl(node_detail_request("concept/s", Some(agent)), db).await;
        let v = resp.result.unwrap();
        let outgoing = v.get("outgoing").unwrap().as_array().unwrap();
        assert_eq!(outgoing.len(), 2);
        let ghost = outgoing.iter().find(|o| o["raw"] == "ghost").unwrap();
        assert_eq!(ghost["status"], "dangling");
    }
```

- [ ] **Step 2: store 实现 `related_edges_between`**

```rust
    async fn related_edges_between(
        &self,
        agent_id: &str,
        visible: &std::collections::HashSet<String>,
        per_node: usize,
    ) -> Result<Vec<(String, String, f32)>, AlephError> {
        let conn = lock_conn!(self)?;
        let mut stmt = conn
            .prepare(
                "SELECT node_path, related_path, score FROM notes_graph_related \
                 WHERE agent_id = ?1 ORDER BY node_path, score DESC",
            )
            .map_err(|e| AlephError::config(format!("related_edges prep: {e}")))?;
        let rows = stmt
            .query_map(params![agent_id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, f64>(2)? as f32,
                ))
            })
            .map_err(|e| AlephError::config(format!("related_edges query: {e}")))?;
        let mut out = Vec::new();
        let mut count_for: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        for row in rows {
            let (node, related, score) =
                row.map_err(|e| AlephError::config(format!("related_edges row: {e}")))?;
            if !visible.contains(&node) || !visible.contains(&related) {
                continue;
            }
            let c = count_for.entry(node.clone()).or_insert(0);
            if *c >= per_node {
                continue; // rows are score-DESC within node_path → top-K kept
            }
            *c += 1;
            out.push((node, related, score));
        }
        Ok(out)
    }
```

- [ ] **Step 3: `handle_query_impl` 拼装** — 在 edges 映射后追加：

```rust
    // Similarity edges (5-signal + MinHash, materialized by GraphRecompute) —
    // top-3 per node, deduped against real links by undirected pair.
    let visible: std::collections::HashSet<String> =
        entries.iter().map(|e| e.path.clone()).collect();
    let mut seen: std::collections::HashSet<(String, String)> = edges
        .iter()
        .map(|e| undirected_key(&e.from, &e.to))
        .collect();
    let mut edges = edges;
    if let Ok(related) = db.related_edges_between(agent_id, &visible, 3).await {
        for (from, to, _score) in related {
            let key = undirected_key(&from, &to);
            if seen.insert(key) {
                edges.push(NoteLinkDto {
                    from,
                    to,
                    label: None,
                    kind: Some("related_similarity".to_string()),
                    confidence: None,
                });
            }
        }
    }

    // Graph-health emphasis payloads (bridge nodes + surprising edges).
    // `sparse` stays orientation-only by design (spec S3).
    let bridge_nodes: Vec<String> = db
        .read_graph_insights(agent_id, Some("bridge"))
        .await
        .ok()
        .and_then(|rows| {
            rows.into_iter()
                .find_map(|(_, p)| serde_json::from_str::<Vec<String>>(&p).ok())
        })
        .unwrap_or_default()
        .into_iter()
        .filter(|p| visible.contains(p))
        .collect();
    #[derive(serde::Deserialize)]
    struct SurprisingRow {
        from: String,
        to: String,
    }
    let surprising_edges: Vec<(String, String)> = db
        .read_graph_insights(agent_id, Some("surprising"))
        .await
        .ok()
        .and_then(|rows| {
            rows.into_iter()
                .find_map(|(_, p)| serde_json::from_str::<Vec<SurprisingRow>>(&p).ok())
        })
        .unwrap_or_default()
        .into_iter()
        .filter(|r| visible.contains(&r.from) && visible.contains(&r.to))
        .map(|r| (r.from, r.to))
        .collect();
```

真实边的 DTO 映射补 `confidence: Some(row.confidence)`；响应结构体补三个新字段。文件底部加：

```rust
fn undirected_key(a: &str, b: &str) -> (String, String) {
    if a <= b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    }
}
```

- [ ] **Step 4: `handle_node_detail_impl`** — backlinks 之后追加：

```rust
    let outgoing: Vec<OutgoingLinkDto> = db
        .get_outgoing_link_rows(&params.node_id, agent_id)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|r| OutgoingLinkDto {
            to: r.to_note,
            raw: r.to_raw,
            relation: r.relation,
            label: r.label,
            confidence: r.confidence,
            resolved_by: r.resolved_by,
            status: r.status,
        })
        .collect();
```

响应加 `outgoing` 字段。
- [ ] **Step 5: verify diagnostics；Commit** — `gateway: surface similarity edges, insights and link provenance in graph RPCs`

---

### Task 10: panel 星系图 — 相似边/mention 色档、confidence 亮度、bridge/surprising 强调

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/adapter.rs`（DTO 字段）
- Modify: `interfaces/webchat/src/platform/wide/views/canvas/gl/edges.rs`（kind 5/6/7）
- Modify: `interfaces/webchat/src/platform/wide/views/canvas/gl/mod.rs`（`GraphData.edge_bright`）
- Modify: `interfaces/webchat/src/platform/wide/views/canvas/gl/scene.rs`（filtered_edge_bright 镜像）
- Modify: `interfaces/webchat/src/platform/wide/views/canvas/galaxy_build.rs`（亮度计算 + bridge/surprising）
- Test: `galaxy_build.rs` + `edges.rs` tests（native target）

**Interfaces:**
- Consumes: Task 9 的 wire 字段（`NoteLinkDto.confidence`、`GraphQueryResponse.bridge_nodes/surprising_edges`）。
- Produces:
  - `adapter::NoteLinkDto` 加 `#[serde(default)] pub confidence: Option<f32>`；`GraphQueryResponse` 加 `#[serde(default)] pub bridge_nodes: Vec<String>` 与 `#[serde(default)] pub surprising_edges: Vec<(String, String)>`；
  - `edge_kind_code`: `"mention"`→5、`"related_similarity"`→6、**新 code 7 = surprising 强调**（build 期覆盖原 kind）；`edge_kind_color`: 5→暗板岩 `[0.42,0.45,0.55]`、6→淡紫 `[0.48,0.40,0.72]`、7→亮金 `[1.35,1.15,0.55]`（>1.0 触发 bloom 发光）；
  - `GraphData` 加 `pub edge_bright: Vec<f32>`（与 edges/edge_kinds 平行；亮度乘进边色）；
  - 亮度公式（galaxy_build 纯函数 `edge_brightness`）：wikilink 类 conf `c` → `0.55 + 0.45*c`；kind 5 (mention) → 0.5；kind 6 (similarity) → 0.55；kind 7 (surprising) → 1.0（颜色本身已 >1）；confidence 缺省 → 1.0；
  - bridge 节点 → 节点色 ×1.25（进 bloom 微光）。
- **spec 偏差 3 落点**（confidence→透明度 ⇒ 亮度缩放）。

- [ ] **Step 1: 写失败测试**（galaxy_build.rs tests）

```rust
    #[test]
    fn edge_brightness_maps_confidence_and_kinds() {
        assert!((edge_brightness(Some(1.0), 0) - 1.0).abs() < 1e-6);
        assert!((edge_brightness(Some(0.35), 0) - (0.55 + 0.45 * 0.35)).abs() < 1e-6);
        assert!((edge_brightness(None, 0) - 1.0).abs() < 1e-6);
        assert!((edge_brightness(None, 5) - 0.5).abs() < 1e-6);   // mention
        assert!((edge_brightness(None, 6) - 0.55).abs() < 1e-6);  // similarity
        assert!((edge_brightness(Some(0.2), 7) - 1.0).abs() < 1e-6); // surprising ignores conf
    }

    #[test]
    fn build_galaxy_flags_surprising_and_bridge() {
        use crate::canvas_engine::adapter::{GraphQueryResponse, NoteLinkDto, NoteNodeDto};
        let node = |id: &str| NoteNodeDto {
            id: id.into(), name: id.into(), path: id.into(), category: "c".into(),
            tags: vec![], link_count: 1, community_id: None, updated_at: None,
        };
        let resp = GraphQueryResponse {
            nodes: vec![node("a/x"), node("a/y")],
            edges: vec![NoteLinkDto {
                from: "a/x".into(), to: "a/y".into(),
                label: None, kind: Some("wikilink".into()), confidence: Some(1.0),
            }],
            total: None,
            bridge_nodes: vec!["a/x".into()],
            surprising_edges: vec![("a/x".into(), "a/y".into())],
        };
        let data = build_galaxy(&resp);
        assert_eq!(data.edge_kinds, vec![7], "surprising overrides kind");
        assert_eq!(data.edge_bright.len(), data.edges.len());
        // Bridge node brighter than its sibling.
        assert!(data.nodes[0].color[0] > data.nodes[1].color[0]);
    }
```

（`edges.rs` tests 补 `edge_kind_code(Some("mention"))==5` / `Some("related_similarity")==6` 与 `edge_kind_color(7).unwrap()[0] > 1.0` 断言。）

- [ ] **Step 2: adapter DTO**（三个 `#[serde(default)]` 字段照 Interfaces 加）。
- [ ] **Step 3: edges.rs** — `edge_kind_code` match 在 `Some(_) => 4` 之前插入 `Some("mention") => 5, Some("related_similarity") => 6,`（7 不由字符串映射，是 build 期覆盖）；`edge_kind_color` 加 `5 => Some([0.42, 0.45, 0.55]), 6 => Some([0.48, 0.40, 0.72]), 7 => Some([1.35, 1.15, 0.55]),`。`upload_indexed` 签名加 `edge_bright: &[f32]` 参数，颜色写入处改：

```rust
            let bright = edge_bright.get(i).copied().unwrap_or(1.0);
            match edge_kinds.get(i).copied().and_then(edge_kind_color) {
                Some(c) => {
                    let c = [c[0] * bright, c[1] * bright, c[2] * bright];
                    col_a.extend_from_slice(&c);
                    col_b.extend_from_slice(&c);
                }
                None => {
                    col_a.extend_from_slice(&[na.color[0] * bright, na.color[1] * bright, na.color[2] * bright]);
                    col_b.extend_from_slice(&[nb.color[0] * bright, nb.color[1] * bright, nb.color[2] * bright]);
                }
            }
```

- [ ] **Step 4: gl/mod.rs `GraphData`** — 加 `pub edge_bright: Vec<f32>,`；mod.rs:86/99 两处构造默认值分别 `vec![1.0; 3]` / `vec![]`（与 edge_kinds 同形）。
- [ ] **Step 5: scene.rs 镜像** — `filtered_edge_bright: Vec<f32>` 字段 + 构造空 vec；`recompute_filtered_edges` 里仿 `kind_at` 加 `bright_at`（default 1.0），三处 filtered_edge_kinds 赋值处平行维护 `filtered_edge_bright`；`debug_assert_eq!` 补一条；`upload_indexed` 的 3 个调用点（scene.rs:122/141/338 附近）补传 `&self.filtered_edge_bright`。
- [ ] **Step 6: galaxy_build.rs** —

```rust
/// Per-edge brightness: confidence dims the backbone (floor 0.55 keeps weak
/// links visible); mention/similarity kinds are fixed-dim; surprising (7) is
/// full — its >1.0 color already carries the bloom glow.
fn edge_brightness(confidence: Option<f32>, kind: u8) -> f32 {
    match kind {
        5 => 0.5,
        6 => 0.55,
        7 => 1.0,
        _ => confidence.map_or(1.0, |c| 0.55 + 0.45 * c.clamp(0.0, 1.0)),
    }
}
```

`build_galaxy`: surprising 集合先建（`HashSet<(u32,u32)>` 经 id_index 映射 + min/max 归一）；`dedup_undirected_edges` 输入三元组扩为 `(a, b, kind, bright)` 四元组、返回 `(edges, kinds, brights)`（first-seen 语义不变）；喂入前：

```rust
        let (edges, edge_kinds, edge_bright) =
            dedup_undirected_edges(resp.edges.iter().filter_map(|e| {
                let a = *id_index.get(&e.from)?;
                let b = *id_index.get(&e.to)?;
                let mut kind = gl::edges::edge_kind_code(e.kind.as_deref());
                if surprising.contains(&(a.min(b), a.max(b))) {
                    kind = 7; // insight emphasis overrides the base kind
                }
                Some((a, b, kind, edge_brightness(e.confidence, kind)))
            }));
```

bridge 强调（nodes 映射内，`category_rgb` 后）：

```rust
            let bridge_boost = if bridge_set.contains(&n.id) { 1.25 } else { 1.0 };
            // color: base * recency * bridge_boost（三处分量同乘）
```

`GraphData { nodes, edges, edge_kinds, edge_bright }`。既有 dedup 测试改四元组签名。
- [ ] **Step 7: verify diagnostics；Commit** — `canvas: similarity/mention edge kinds, confidence brightness, insight emphasis`

---

### Task 11: node_detail 出链徽标区（wide 详情栏）

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/adapter.rs`（`NoteDetailResponse.outgoing` + `OutgoingLinkDto`）
- Modify: `interfaces/webchat/src/platform/wide/views/canvas/node_detail_panel.rs`
- Test: adapter 反序列化测试（native）

**Interfaces:**
- Consumes: Task 9 的 `node_detail.outgoing`。
- Produces: `NodeExcerpt` 加 `pub outgoing: Vec<OutgoingLinkDto>`；详情栏 Backlinks 区块下方新增 "Links" 区块——逐行 `目标 · relation徽标 · conf% · status`，`dangling` 灰斜体、`tombstone` 划线 + 🪦 图标字符；active 行可点击导航（`mem.selected_node.set`）。

- [ ] **Step 1: adapter** —

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct OutgoingLinkDto {
    pub to: String,
    pub raw: String,
    #[serde(default)]
    pub relation: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    pub confidence: f32,
    #[serde(default)]
    pub resolved_by: Option<String>,
    pub status: String,
}
```

`NoteDetailResponse` 加 `#[serde(default)] pub outgoing: Vec<OutgoingLinkDto>`（default 兼容旧 server）。加一个 serde 反序列化单测（含缺省字段的旧响应 JSON 仍可解析）。

- [ ] **Step 2: NodeExcerpt + fetch Effect** — `NodeExcerpt` 加 `pub outgoing: Vec<crate::canvas_engine::adapter::OutgoingLinkDto>`；fetch 处 `outgoing: detail.outgoing`。
- [ ] **Step 3: 视图区块**（Backlinks 区块之后，同样式语言）

```rust
            {(!outgoing.is_empty()).then(|| {
                let ol = outgoing.clone();
                view! {
                    <div style="margin-top:10px">
                        <div style="text-transform:uppercase;font-size:9.5px;color:var(--text-meta);letter-spacing:0.05em;margin-bottom:4px">
                            "Links"
                        </div>
                        <ul style="list-style:none;padding:0;margin:0;display:flex;flex-direction:column;gap:3px">
                            {ol.into_iter().map(|l| {
                                let is_active = l.status == "active";
                                let target = l.to.clone();
                                let display = l.label.clone().unwrap_or_else(|| l.raw.clone());
                                let badge = l.relation.clone().map(|r| format!(" · {r}")).unwrap_or_default();
                                let meta = match l.status.as_str() {
                                    "dangling" => " · dangling".to_string(),
                                    "tombstone" => " · 🪦 deleted".to_string(),
                                    _ => format!(" · {:.0}%", l.confidence * 100.0),
                                };
                                let style = match l.status.as_str() {
                                    "dangling" => "font-size:11px;color:var(--text-meta);font-style:italic;padding:3px 6px",
                                    "tombstone" => "font-size:11px;color:var(--text-meta);text-decoration:line-through;padding:3px 6px",
                                    _ => "font-size:11px;color:var(--cat-reference);padding:3px 6px;border-radius:4px;background:rgba(96,165,250,0.08);cursor:pointer",
                                };
                                view! {
                                    <li
                                        style=style
                                        on:click=move |_| {
                                            if is_active {
                                                mem.selected_node.set(Some(target.clone()));
                                            }
                                        }
                                    >
                                        {display}{badge}{meta}
                                    </li>
                                }
                            }).collect_view()}
                        </ul>
                    </div>
                }
            })}
```

（`outgoing` 在 `DetailFor` 顶部随 backlinks 一起 clone 出局部变量。）
- [ ] **Step 4: verify diagnostics；Commit** — `panel: outgoing links with provenance badges in node detail`

---

## Phase ③ Panel wiki 交互（Task 12–14）

### Task 12: render_excerpt 支持 `[[wikilink]]`

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/markdown_excerpt.rs`
- Test: 同文件 tests

**Interfaces:**
- Produces:
  - 纯函数 `pub(crate) fn split_wikilinks(text: &str) -> Vec<WikiSegment>`，`pub(crate) enum WikiSegment<'a> { Text(&'a str), Link { target: &'a str, label: Option<&'a str> } }`（手写扫描，**不引 regex**）；
  - `render_excerpt` 输出中 `[[t]]`/`[[t|label]]` 变 `<a class="wl" data-wl="t">label或t</a>`（target HTML-escape 进属性）；
  - `pub fn wikilink_click_target(ev: &web_sys::MouseEvent) -> Option<String>`（`#[cfg(target_arch = "wasm32")]`）——事件委托 helper：从 `ev.target()` 向上找带 `data-wl` 的元素并返回其值。Task 13 三视图消费。

- [ ] **Step 1: 写失败测试**

```rust
    #[test]
    fn renders_wikilink_as_clickable_span() {
        let out = render_excerpt("see [[rust-notes]] here");
        assert!(
            out.contains(r#"<a class="wl" data-wl="rust-notes">rust-notes</a>"#),
            "got: {out}"
        );
    }

    #[test]
    fn renders_wikilink_alias_label() {
        let out = render_excerpt("see [[rust-notes|My Rust]] here");
        assert!(out.contains(r#"data-wl="rust-notes">My Rust</a>"#), "got: {out}");
    }

    #[test]
    fn wikilink_target_is_escaped() {
        let out = render_excerpt(r#"[[a"b]]"#);
        assert!(out.contains("data-wl=\"a&quot;b\""), "got: {out}");
        assert!(!out.contains(r#"data-wl="a"b""#));
    }

    #[test]
    fn split_wikilinks_handles_mixed_text() {
        let segs = split_wikilinks("x [[a]] y [[b|B]] [[unclosed");
        assert_eq!(segs.len(), 5); // "x ", link a, " y ", link b, " [[unclosed"
        assert!(matches!(segs[1], WikiSegment::Link { target: "a", label: None }));
        assert!(matches!(segs[3], WikiSegment::Link { target: "b", label: Some("B") }));
        assert!(matches!(segs[4], WikiSegment::Text(" [[unclosed")));
    }
```

- [ ] **Step 2: 实现 `split_wikilinks`**

```rust
/// One segment of text after wikilink splitting.
#[derive(Debug, PartialEq)]
pub(crate) enum WikiSegment<'a> {
    Text(&'a str),
    Link { target: &'a str, label: Option<&'a str> },
}

/// Hand-rolled `[[target]]` / `[[target|label]]` scanner (no regex dep in the
/// panel). Unclosed `[[` and empty targets fall through as plain text.
pub(crate) fn split_wikilinks(text: &str) -> Vec<WikiSegment<'_>> {
    let mut out = Vec::new();
    let mut rest = text;
    loop {
        let Some(open) = rest.find("[[") else {
            if !rest.is_empty() {
                out.push(WikiSegment::Text(rest));
            }
            return out;
        };
        let Some(close_rel) = rest[open + 2..].find("]]") else {
            if !rest.is_empty() {
                out.push(WikiSegment::Text(rest));
            }
            return out;
        };
        let inner = &rest[open + 2..open + 2 + close_rel];
        if inner.is_empty() || inner.contains("[[") {
            // Empty or nested-open: emit up to and including "[[" as text and rescan.
            out.push(WikiSegment::Text(&rest[..open + 2]));
            rest = &rest[open + 2..];
            continue;
        }
        if open > 0 {
            out.push(WikiSegment::Text(&rest[..open]));
        }
        let (target, label) = match inner.split_once('|') {
            Some((t, l)) if !l.is_empty() => (t, Some(l)),
            Some((t, _)) => (t, None),
            None => (inner, None),
        };
        out.push(WikiSegment::Link { target, label });
        rest = &rest[open + 2 + close_rel + 2..];
    }
}
```

- [ ] **Step 3: 接进 `render_excerpt`** — `Event::Text(t)` 分支改为经 `split_wikilinks(&take)`：Text 段照旧 `html_escape` 输出；Link 段输出：

```rust
                        WikiSegment::Link { target, label } => {
                            out.push_str("<a class=\"wl\" data-wl=\"");
                            out.push_str(&html_escape(target));
                            out.push_str("\">");
                            out.push_str(&html_escape(label.unwrap_or(target)));
                            out.push_str("</a>");
                        }
```

（chars_used 按 label 计。注意：截断在 take 之后做 split——`[[` 被 180 字截断劈开时安全退化为纯文本。）

- [ ] **Step 4: `wikilink_click_target`**

```rust
/// Event-delegation helper: walk up from the click target to the nearest
/// element carrying `data-wl` and return its value. Views attach one
/// `on:click` on the inner_html container instead of per-link closures.
#[cfg(target_arch = "wasm32")]
pub fn wikilink_click_target(ev: &web_sys::MouseEvent) -> Option<String> {
    use wasm_bindgen::JsCast;
    let el = ev.target()?.dyn_into::<web_sys::Element>().ok()?;
    let hit = el.closest("[data-wl]").ok()??;
    hit.get_attribute("data-wl")
}
```

- [ ] **Step 5: verify diagnostics；Commit** — `panel: render [[wikilinks]] as clickable anchors in markdown excerpts`

---

### Task 13: 三视图导航接线 + backlink chips 统一

**Files:**
- Modify: `interfaces/webchat/src/platform/wide/views/canvas/node_detail_panel.rs`
- Modify: `interfaces/webchat/src/platform/wide/views/memory/drawer.rs`
- Modify: `interfaces/webchat/src/platform/phone/memory/detail.rs`
- Test: 手测清单（Task 17 QA），本 Task 逻辑均为薄接线

**Interfaces:**
- Consumes: Task 12 的 `wikilink_click_target`；`GraphApi::search`（既有）。
- 导航语义（统一 helper 逻辑，各视图落自己的动作）: `data-wl` 值含 `/` → 直接当 node id；否则 `GraphApi::search(state, agent, target, 1)` 取第一命中 id；拿到 id 后——星系图详情栏 `mem.selected_node.set(Some(id))`（既有 Effect 自动 fly-to）；记忆抽屉 `target.set(Some(DrawerTarget::Note(fact)))`（用 id 构造最小 `CompressedFact`）；手机详情 `st.selected.set(Some(fact))`（同构造，路由不变刷新内容）。
- backlink chips: drawer 与 phone 的 backlinks 从纯文本 `<li>`/`<div>` 改为与星系图详情栏一致的可点击样式，点击走同一导航动作。

- [ ] **Step 1: 共享小构造**（drawer.rs 顶部或 `api/memory.rs` 旁——放 `state/memory.rs` 更中性；实现者按就近原则放 drawer.rs 并在 phone/detail.rs 复用 `crate::platform::wide::views::memory::drawer::fact_from_path` 会造成跨平台依赖——**放 `src/api/memory.rs`**）:

```rust
impl CompressedFact {
    /// Minimal fact for drill-into-note navigation: only `path`/`category`/
    /// `content`(title) are load-bearing for the detail views' fetch flow.
    #[must_use]
    pub fn stub_from_path(path: &str) -> Self {
        let (category, filename) = path.split_once('/').unwrap_or(("other", path));
        Self {
            id: path.to_string(),
            agent_id: String::new(),
            content: filename.to_string(),
            fact_type: String::new(),
            created_at: 0,
            category: category.to_string(),
            path: path.to_string(),
        }
    }
}
```

- [ ] **Step 2: 星系图详情栏** — `DetailFor` 的正文 `inner_html` div 加：

```rust
                        <div
                            class="node-card-full__excerpt"
                            style="color:var(--text-body);font-size:12px;line-height:1.55"
                            inner_html=html
                            on:click=move |ev| {
                                if let Some(t) = crate::canvas_engine::markdown_excerpt::wikilink_click_target(&ev) {
                                    navigate_wl(&state, &mem, t);
                                }
                            }
                        ></div>
```

`navigate_wl` 放本文件底部：

```rust
/// Resolve a clicked wikilink target to a node id and select it.
/// Path-form targets navigate directly; bare names resolve via graph.search
/// (first hit — same resolution surface the sidebar search uses).
fn navigate_wl(state: &DashboardState, mem: &MemoryState, target: String) {
    let state = *state;
    let mem = *mem;
    spawn_local(async move {
        let id = if target.contains('/') {
            Some(target)
        } else {
            let agent = mem.agent_id.get_untracked();
            GraphApi::search(&state, &agent, &target, 1)
                .await
                .ok()
                .and_then(|r| r.results.first().map(|f| f.id.clone()))
        };
        if let Some(id) = id {
            mem.push_recent(id.clone());
            mem.selected_node.set(Some(id));
        }
    });
}
```

（`DashboardState`/`MemoryState` 均为 Copy 上下文结构——与本文件既有捕获习惯一致；若非 Copy 按既有 `expect_context` 用法在闭包内重取。）
- [ ] **Step 3: 记忆抽屉** — `NoteDetail` 正文 div 加同样 `on:click`，动作换成：

```rust
                                    if let Some(t) = wikilink_click_target(&ev) {
                                        navigate_drawer(&state, &mem, target, t);
                                    }
```

`navigate_drawer(state, mem, target_signal, wl)`：解析 id 同 `navigate_wl`，然后 `target_signal.set(Some(DrawerTarget::Note(CompressedFact::stub_from_path(&id))))`（drawer 重挂载自动 fetch）。`NoteDetail` 需把 `target: RwSignal<Option<DrawerTarget>>` 作为参数传入（`DrawerShell` 已持有，把它透传给 `NoteDetail`）。backlinks `<li>` 改 chips：样式复制星系图详情栏 backlink 行（`cursor:pointer` + 背景），`on:click` 走 `navigate_drawer`。
- [ ] **Step 4: 手机详情** — 正文 div 加 `on:click`，动作 `st.selected.set(Some(CompressedFact::stub_from_path(&id)))`（fetch Effect 订阅 `st.selected` 自动重载；遵循手机下钻法则——同屏内容替换 + 返回键仍回列表）。backlinks cell 加 `on:click` 同动作 + `cursor:pointer`。
- [ ] **Step 5: verify diagnostics；Commit** — `panel: wikilink click navigation across galaxy/drawer/phone views`

---

### Task 14: Panel rename/delete 入口 + GraphApi 扩展

**Files:**
- Modify: `interfaces/webchat/src/api/graph.rs`
- Modify: `interfaces/webchat/src/platform/wide/views/canvas/node_detail_panel.rs`
- Modify: `interfaces/webchat/src/platform/wide/views/memory/drawer.rs`
- Test: 手测清单（Task 17 QA）

**Interfaces:**
- Consumes: Task 8 的两个 RPC。
- Produces（GraphApi）:

```rust
    pub async fn rename_note(
        state: &DashboardState,
        agent_id: &str,
        node_id: &str,
        new_title: &str,
    ) -> Result<String, String> {
        let params = json!({ "agent_id": agent_id, "node_id": node_id, "new_title": new_title });
        let result = state.rpc_call("graph.rename_note", params).await?;
        result
            .get("new_id")
            .and_then(|v| v.as_str())
            .map(String::from)
            .ok_or_else(|| "rename_note: missing new_id in response".to_string())
    }

    pub async fn delete_note(
        state: &DashboardState,
        agent_id: &str,
        node_id: &str,
    ) -> Result<(), String> {
        let params = json!({ "agent_id": agent_id, "node_id": node_id });
        state.rpc_call("graph.delete_note", params).await?;
        Ok(())
    }
```

- UI 语义: 星系图详情栏 + 抽屉的 Edit 按钮旁加 `Rename`（内联输入 + 确认）与 `Delete`（**tap-to-confirm**：首击变红显示 "Confirm delete?"，二击才发 RPC——agents 下钻既有先例）。成功后：rename → 更新 `mem.selected_node` 为 new_id 并清 excerpt 缓存旧键；delete → `mem.selected_node.set(None)` / 抽屉 `target.set(None)`，星系图侧触发一次 graph.query 重拉（复用 galaxy-build Effect：加一个 `mem.graph_refresh_nonce: RwSignal<u64>`？——**不加新状态**，简化为提示用户图将于下次刷新更新，节点从 detail 消失即时可见）。

- [ ] **Step 1: GraphApi 两方法**（如上）。
- [ ] **Step 2: 详情栏 UI** — Edit 按钮行改为三按钮排（Edit / Rename / Delete）。Rename：`is_renaming: RwSignal<bool>` + `rename_draft: RwSignal<String>`（初值当前 name）+ 确认按钮调 `GraphApi::rename_note`，成功后 `excerpts.update(|m| { m.remove(&old_id); })` + `mem.selected_node.set(Some(new_id))`。Delete：`confirm_delete: RwSignal<bool>`，首击置 true（按钮文案变 "Confirm delete?" 红底），二击调 `GraphApi::delete_note` 成功后 `mem.selected_node.set(None)`；点击其他区域/3s 后重置——用 `on:blur` 或简单地在再次渲染时不自动重置（保守：只有再点别处节点自然消失）。错误落既有 `error` 信号。
- [ ] **Step 3: 抽屉 UI** — 同款三按钮；delete 成功 `target.set(None)`；rename 成功用 `stub_from_path(&new_id)` 重设 target。
- [ ] **Step 4: verify diagnostics；Commit** — `panel: rename/delete note actions in detail surfaces`

---

## Phase ④ 自织网（Task 15–16）

### Task 15: `links/mentions.rs` 提及扫描器（纯函数）

**Files:**
- Modify: `src/memory/notes/links/mentions.rs`（替换 Task 3 的占位）
- Test: 同文件 tests

**Interfaces:**
- Produces:

```rust
pub const MENTION_RELATION: &str = "mention";
pub const MENTION_CONFIDENCE: f32 = 0.35;
/// Per-note cap on emitted mentions (spec M1 guard).
pub const MAX_MENTIONS_PER_NOTE: usize = 5;

/// One note's scan input.
pub struct MentionDoc {
    pub path: String,
    /// filename + frontmatter aliases (the names other bodies may mention).
    pub names: Vec<String>,
    /// frontmatter-stripped body text.
    pub body: String,
    /// raw wikilink targets already present in the body (skip re-linking).
    pub linked_raw: Vec<String>,
}

/// Deterministic unlinked-mention scan across the corpus.
/// Returns (from_path, to_path) pairs, ≤ MAX_MENTIONS_PER_NOTE per from-note,
/// deterministic order (sorted by (from, to)).
pub fn scan_mentions(docs: &[MentionDoc]) -> Vec<(String, String)>;
```

- 护栏（spec §2.1/M1）: 名字最短长度 ASCII≥4 字符 / 含 CJK≥2 字；ASCII 名词边界匹配（两侧非字母数字）、CJK 名子串匹配；跳过自指；跳过正文已 `[[链接]]`（按 raw target 与 names 双向比对）；**名字归一化后与多个笔记冲突（同名/同 alias 撞车）→ 该名字整个跳过**（多候选不猜——与解析链同一纪律）；每 from-note ≤5。

- [ ] **Step 1: 写失败测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn doc(path: &str, names: &[&str], body: &str, linked: &[&str]) -> MentionDoc {
        MentionDoc {
            path: path.into(),
            names: names.iter().map(|s| s.to_string()).collect(),
            body: body.into(),
            linked_raw: linked.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn detects_ascii_mention_with_word_boundary() {
        let docs = vec![
            doc("a/rust-notes", &["rust-notes"], "body of target", &[]),
            doc("b/diary", &["diary"], "today I reread rust-notes again", &[]),
        ];
        assert_eq!(
            scan_mentions(&docs),
            vec![("b/diary".to_string(), "a/rust-notes".to_string())]
        );
    }

    #[test]
    fn word_boundary_rejects_substring_for_ascii() {
        let docs = vec![
            doc("a/rust", &["rust"], "x", &[]),
            doc("b/d", &["d"], "we trust the process", &[]), // "rust" inside "trust"
        ];
        assert!(scan_mentions(&docs).is_empty());
    }

    #[test]
    fn cjk_mention_matches_substring() {
        let docs = vec![
            doc("a/记忆系统", &["记忆系统"], "x", &[]),
            doc("b/日记", &["日记"], "今天研究了记忆系统的检索", &[]),
        ];
        assert_eq!(scan_mentions(&docs), vec![("b/日记".into(), "a/记忆系统".into())]);
    }

    #[test]
    fn short_names_never_match() {
        let docs = vec![
            doc("a/app", &["app"], "x", &[]),          // ASCII len 3 < 4
            doc("a/图", &["图"], "y", &[]),             // CJK len 1 < 2
            doc("b/d", &["dddd"], "the app draws a 图", &[]),
        ];
        assert!(scan_mentions(&docs).is_empty());
    }

    #[test]
    fn skips_already_linked_and_self() {
        let docs = vec![
            doc("a/rust-notes", &["rust-notes"], "rust-notes mentions itself", &[]),
            doc("b/diary", &["diary"], "see [[rust-notes]] and rust-notes prose", &["rust-notes"]),
        ];
        assert!(scan_mentions(&docs).is_empty(), "self + already-linked must not edge");
    }

    #[test]
    fn ambiguous_name_is_skipped_entirely() {
        let docs = vec![
            doc("a/notes", &["notes"], "x", &[]),
            doc("b/notes", &["notes"], "y", &[]),
            doc("c/diary", &["diary"], "my notes about things", &[]),
        ];
        assert!(scan_mentions(&docs).is_empty(), "duplicate name must never guess");
    }

    #[test]
    fn per_note_cap_applies() {
        let mut docs: Vec<MentionDoc> = (0..8)
            .map(|i| doc(&format!("t/target-{i:02}"), &[&format!("target-{i:02}")], "x", &[]))
            .collect();
        let body: String = (0..8).map(|i| format!("target-{i:02} ")).collect();
        docs.push(doc("s/spammy", &["spammy"], &body, &[]));
        let hits = scan_mentions(&docs);
        assert_eq!(hits.len(), MAX_MENTIONS_PER_NOTE);
        assert!(hits.iter().all(|(f, _)| f == "s/spammy"));
    }
}
```

- [ ] **Step 2: 实现**

```rust
//! Unlinked-mention scanner (spec M1, D2): a note body mentioning another
//! note's filename/alias — without a `[[wikilink]]` — earns a low-confidence
//! `mention` soft edge. Deterministic exact matching, zero LLM (R7-clean:
//! same class as FTS). Bodies are never modified (D2: only humans/LLMs write
//! real `[[links]]`).

use std::collections::HashMap;

use super::resolve::normalize_link_key;

pub const MENTION_RELATION: &str = "mention";
pub const MENTION_CONFIDENCE: f32 = 0.35;
pub const MAX_MENTIONS_PER_NOTE: usize = 5;

pub struct MentionDoc {
    pub path: String,
    pub names: Vec<String>,
    pub body: String,
    pub linked_raw: Vec<String>,
}

/// A name qualifies when: ASCII-only names have ≥4 chars; names containing
/// CJK have ≥2 chars. Short names ("app", "图") produce only noise.
fn name_qualifies(name: &str) -> bool {
    let has_cjk = name.chars().any(is_cjk);
    let n = name.chars().count();
    if has_cjk {
        n >= 2
    } else {
        n >= 4
    }
}

const fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0x3040..=0x30FF | 0xAC00..=0xD7AF)
}

/// ASCII names need word boundaries (both neighbours non-alphanumeric);
/// CJK names match as substrings (no word boundaries in CJK text).
fn body_mentions(body_norm: &str, name_norm: &str, cjk: bool) -> bool {
    if cjk {
        return body_norm.contains(name_norm);
    }
    let mut from = 0;
    while let Some(rel) = body_norm[from..].find(name_norm) {
        let start = from + rel;
        let end = start + name_norm.len();
        let before_ok = start == 0
            || !body_norm[..start]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric());
        let after_ok = end >= body_norm.len()
            || !body_norm[end..]
                .chars()
                .next()
                .is_some_and(|c| c.is_alphanumeric());
        if before_ok && after_ok {
            return true;
        }
        from = end;
    }
    false
}

#[must_use]
pub fn scan_mentions(docs: &[MentionDoc]) -> Vec<(String, String)> {
    // Dictionary: normalized name → owning paths. Ambiguous names (owned by
    // >1 note) are dropped wholesale — mirror the resolver's never-guess rule.
    let mut dict: HashMap<String, Vec<&str>> = HashMap::new();
    for d in docs {
        for name in &d.names {
            if name_qualifies(name) {
                dict.entry(normalize_link_key(name))
                    .or_default()
                    .push(d.path.as_str());
            }
        }
    }
    dict.retain(|_, owners| {
        owners.dedup();
        owners.len() == 1
    });

    let mut out: Vec<(String, String)> = Vec::new();
    for d in docs {
        let body_norm = normalize_link_key(&d.body);
        let linked: Vec<String> = d.linked_raw.iter().map(|s| normalize_link_key(s)).collect();
        let mut hits: Vec<(String, String)> = Vec::new();
        for (name_norm, owners) in &dict {
            let target = owners[0];
            if target == d.path {
                continue; // self
            }
            if linked.iter().any(|l| l == name_norm) {
                continue; // already a real [[link]]
            }
            let cjk = name_norm.chars().any(is_cjk);
            if body_mentions(&body_norm, name_norm, cjk) {
                hits.push((d.path.clone(), target.to_string()));
            }
        }
        hits.sort();
        hits.dedup();
        hits.truncate(MAX_MENTIONS_PER_NOTE);
        out.extend(hits);
    }
    out.sort();
    out
}
```

（注意 `linked` 比对：raw target 可能是 `cat/name` 路径形——归一化比较仍防大小写漂移；路径形 raw 与 name_norm 不等时不拦，但同 pair 的真实边会在 Task 16 的 INSERT `ON CONFLICT DO NOTHING` 处兜底——真实边永远赢。）

- [ ] **Step 3: verify diagnostics；Commit** — `notes: unlinked-mention scanner (deterministic, never-guess)`

---

### Task 16: `MentionWeaveStage` + `replace_mention_links`

**Files:**
- Create: `src/memory/dreaming/stages/mention_weave.rs`
- Modify: `src/memory/dreaming/stages/mod.rs`（导出）
- Modify: `src/memory/dreaming/mod.rs`（Consolidate 管线接线）
- Modify: `src/memory/notes/store.rs` + `src/memory/store/sqlite/notes/store_impl.rs`（`replace_mention_links`）
- Test: `mention_weave.rs` tests + store tests

**Interfaces:**
- Produces（trait，镜像 `replace_co_recall_links`）:

```rust
    /// Full refresh of `relation='mention'` rows (recomputed each dream cycle).
    /// An existing semantic link for the pair always wins (DO NOTHING on conflict).
    async fn replace_mention_links(
        &self,
        agent_id: &str,
        rows: &[(String, String)],
    ) -> Result<(), AlephError>;
```

- Stage 语义: Consolidate path，`NoteWeaveStage` 之后、`note_decay()` 之前（新 mention 边即时计入 link_weight——与 weave 同理）。`should_run`: `ctx.notes.len() >= 2`。执行: `list_notes` → `get_notes_with_content` → `spawn_blocking`（`KnowledgeNote::from_markdown` 逐篇 parse 出 names/body/linked_raw → `scan_mentions`）→ `replace_mention_links`。每周期总量上限 `MAX_MENTIONS_PER_CYCLE = 200`（`log` 截断量到 report.extra）。失败非致命（skip 本周期）。
- **最终一致性**（spec §2.3）: mention 行会被 `index_note` 的 per-from_note reconcile 清掉，下周期重物化——接受，不加保留逻辑。

- [ ] **Step 1: 写失败测试**（store 侧）

```rust
    #[tokio::test]
    async fn replace_mention_links_full_refresh_and_semantic_wins() {
        let (store, agent) = make_store().await;
        store.index_note(&make_note_simple("a", "x"), agent, "x").await.unwrap();
        store.index_note(&make_note_simple("b", "x"), agent, "x").await.unwrap();
        // Pre-existing semantic link a→b.
        store.add_link_with_relation(agent, "x/a", "x/b", "related").await.unwrap();

        store
            .replace_mention_links(agent, &[
                ("x/a".into(), "x/b".into()),   // conflicts with semantic → must not clobber
                ("x/b".into(), "x/a".into()),   // fresh mention
            ])
            .await
            .unwrap();
        let a_rows = store.get_outgoing_link_rows("x/a", agent).await.unwrap();
        assert_eq!(a_rows[0].relation.as_deref(), Some("related"), "semantic wins");
        let b_rows = store.get_outgoing_link_rows("x/b", agent).await.unwrap();
        let m = b_rows.iter().find(|r| r.to_note == "x/a").unwrap();
        assert_eq!(m.relation.as_deref(), Some("mention"));
        assert!((m.confidence - 0.35).abs() < 1e-6);

        // Second refresh with empty set clears stale mention rows.
        store.replace_mention_links(agent, &[]).await.unwrap();
        let b_rows = store.get_outgoing_link_rows("x/b", agent).await.unwrap();
        assert!(!b_rows.iter().any(|r| r.relation.as_deref() == Some("mention")));
    }
```

- [ ] **Step 2: store 实现**（镜像 `replace_co_recall_links`，store_impl）

```rust
    async fn replace_mention_links(
        &self,
        agent_id: &str,
        rows: &[(String, String)],
    ) -> Result<(), AlephError> {
        use crate::memory::notes::links::mentions::{MENTION_CONFIDENCE, MENTION_RELATION};
        let conn = lock_conn!(self)?;
        conn.execute(
            "DELETE FROM notes_links WHERE agent_id = ?1 AND relation = ?2",
            params![agent_id, MENTION_RELATION],
        )
        .map_err(|e| AlephError::config(format!("replace_mention_links delete: {e}")))?;
        for (from, to) in rows {
            conn.execute(
                "INSERT INTO notes_links \
                   (agent_id, from_note, to_note, to_raw, relation, confidence, resolved_by, status) \
                 VALUES (?1, ?2, ?3, ?3, ?4, ?5, 'mention_scan', 'active') \
                 ON CONFLICT(agent_id, from_note, to_note) DO NOTHING",
                params![agent_id, from, to, MENTION_RELATION, f64::from(MENTION_CONFIDENCE)],
            )
            .map_err(|e| AlephError::config(format!("replace_mention_links insert: {e}")))?;
        }
        Ok(())
    }
```

- [ ] **Step 3: stage**（mention_weave.rs，形态照 `co_recall_edges.rs`）

```rust
//! `MentionWeave` stage — materialize unlinked-mention soft edges (spec M1).
//!
//! A note body mentioning another note's filename/alias without a real
//! `[[wikilink]]` earns a `mention` edge (confidence 0.35). Deterministic
//! scan, zero LLM, bodies never modified (D2). Full refresh per cycle —
//! reconcile-wiped rows re-materialize next cycle (accepted eventual
//! consistency, same as co_recalled).

use async_trait::async_trait;

use crate::error::AlephError;
use crate::memory::dreaming::DreamContext;
use crate::memory::notes::links::mentions::{scan_mentions, MentionDoc};
use crate::memory::notes::store::NoteStore;
use crate::memory::notes::KnowledgeNote;

use super::DreamStage;

/// Cycle-wide cap on materialized mention edges (pathological-corpus guard).
const MAX_MENTIONS_PER_CYCLE: usize = 200;

pub struct MentionWeaveStage;

#[async_trait]
impl DreamStage for MentionWeaveStage {
    fn name(&self) -> &'static str {
        "mention_weave"
    }

    async fn should_run(&self, ctx: &DreamContext) -> bool {
        ctx.notes.len() >= 2
    }

    async fn execute(&self, mut ctx: DreamContext) -> Result<DreamContext, AlephError> {
        let store = ctx.indexer.store().clone();
        let agent_id = ctx.agent_id.clone();

        let hydrated = match async {
            let entries = store.list_notes(&agent_id).await?;
            let paths: Vec<String> = entries.into_iter().map(|e| e.path).collect();
            store.get_notes_with_content(&agent_id, &paths).await
        }
        .await
        {
            Ok(h) => h,
            Err(e) => {
                tracing::warn!(error = %e, "mention_weave: body load failed, skipping cycle");
                return Ok(ctx);
            }
        };

        // Parse + scan off the async runtime (CPU-bound over the whole corpus).
        let mut edges = tokio::task::spawn_blocking(move || {
            let docs: Vec<MentionDoc> = hydrated
                .into_iter()
                .filter_map(|r| {
                    let note = KnowledgeNote::from_markdown(&r.filename, &r.content).ok()?;
                    let mut names = vec![note.title.clone()];
                    names.extend(note.aliases.iter().cloned());
                    Some(MentionDoc {
                        path: r.path,
                        names,
                        body: note.body_text(),
                        linked_raw: note.links.clone(),
                    })
                })
                .collect();
            scan_mentions(&docs)
        })
        .await
        .map_err(|e| AlephError::other(format!("mention_weave join: {e}")))?;

        if edges.len() > MAX_MENTIONS_PER_CYCLE {
            tracing::info!(
                dropped = edges.len() - MAX_MENTIONS_PER_CYCLE,
                "mention_weave: per-cycle cap applied"
            );
            edges.truncate(MAX_MENTIONS_PER_CYCLE);
        }
        let edge_count = edges.len();
        store.replace_mention_links(&agent_id, &edges).await?;

        ctx.report
            .extra
            .insert("mention_edges".into(), edge_count.to_string());
        tracing::info!(agent = %agent_id, edges = edge_count, "mention edges materialized");
        Ok(ctx)
    }
}
```

stage 单测：构造 2 篇笔记（同 note_lint 的 `build_test_dream_ctx` 形态）走 execute，断言 `notes_links` 出现 mention 行 + report.extra 计数。
- [ ] **Step 4: 接线** — `stages/mod.rs` 加 `mod mention_weave; pub use mention_weave::MentionWeaveStage;`（对齐既有导出风格）；`dreaming/mod.rs` Consolidate 列表 `NoteWeaveStage` 与 `note_decay()` 之间插入：

```rust
                // Materialize unlinked-mention soft edges AFTER weave (real
                // links win) and BEFORE decay (mention edges count toward
                // link_weight the same cycle). Deterministic, zero LLM.
                Box::new(stages::MentionWeaveStage),
```

（Synthesize/Conserve 路径不加——mention 是廉价但全库扫描，日常 Consolidate 一条通路足够，spec M1。）
- [ ] **Step 5: verify diagnostics；Commit** — `dreaming: mention_weave stage materializes unlinked-mention soft edges`

---

## Phase ⑤ 完整性校验（Task 17，用户指定加强项）

### Task 17: 文档同步 + 验证门 + 连线审计复检 + 运行时 QA

**Files:**
- Modify: `docs/reference/memory/NOTES.md`（§5 解析链/status 生命周期/with_alias；§8 三新列；§11 rename/relations；§14 mention stage）
- Modify: `docs/reference/FEATURE_LOCATOR.md`（§2.5① 锚点补 links/ 模块、墓碑语义、mention stage、新 RPC）
- 无代码文件（本 Task 是验证与文档）

**Interfaces:** 消费前 16 个 Task 的全部产物。

- [ ] **Step 1: 文档更新** — NOTES.md §5.2 重写解析算法为四档链（附 confidence 表）+ §5.3 补 status/resolved_by/label 列语义与墓碑生命周期；§8 `notes_links` DDL 更新；§11 action 枚举加 Rename、args 加 relations；§14 消费者列表加 MentionWeaveStage 与 canvas 相似边。FEATURE_LOCATOR §2.5① 锚点行追加：`src/memory/notes/links/{resolve,mentions}.rs`、`backfill_inbound_links`、`graph.rename_note/delete_note`、墓碑语义一句话。
- [ ] **Step 2: 唯一 cargo 验证门**（须用户在场知情后执行；这是全计划仅有的 cargo 调用）:

```bash
cargo test -p alephcore --lib 2>&1 | tail -20   # 期望: 全绿（含本计划全部新测试）
cargo test -p aleph-panel --lib 2>&1 | tail -10 # 期望: 全绿（galaxy_build/markdown_excerpt/edges）
```

任一红 → 修复后只重跑失败的具体测试（`cargo test -p alephcore --lib <test_name>`），不重复全量。
- [ ] **Step 3: 连线审计复检**（spec §9.1）— 派独立 read-only 审计 subagent，输入 = spec 行为清单 B1–B7/S1–S4/P1–P3/M1–M2 + 本计划，要求逐条给出「行为 → 实现锚点 file:line → 消费者可达性」三元组，特别核查：① 新列是否有零消费者（label/resolved_by 必须到达 Panel）；② 墓碑行是否泄漏进任何图谱读路径；③ mention 边是否进 galaxy 配色；④ 回填触发点覆盖 create/rename/同名重建三场景；⑤ 无新增"算而未用"。发现断线 → 修复后复审该条。
- [ ] **Step 4: 提交文档** — `docs: sync NOTES/FEATURE_LOCATOR for wikilink lifecycle`
- [ ] **Step 5: 运行时 QA**（重建链: `just wasm` → 重编 server → 替换运行中 binary/重建 macOS App，见 DESKTOP_SHELL.md）。浏览器（:18790）实测清单:
  1. 星系图: 相似边（淡紫细线）出现且少于真实边；mention 边（暗板岩）在一次 dream 周期（`dreaming.run_now`）后出现；surprising 边金色发光；bridge 节点偏亮
  2. 详情栏: Links 区块含 confidence%/relation 徽标；悬空灰斜体、墓碑划线
  3. 正文 `[[链接]]` 点击: 星系图详情栏 fly-to / 记忆抽屉换页 / 手机模拟（窄窗）下钻替换
  4. rename: 改名后旧引用笔记正文已改写 `[[新名]]`、图上节点名更新
  5. delete: tap-to-confirm 两击生效；源笔记正文 `[[原文]]` 保留；同名重建后旧边复活（图上重新连线）
  6. note_manage 走 LLM 会话各动作冒烟（rename / relations / create 带 links）
- [ ] **Step 6: 记录 QA 结果**，回填全局记忆（跨会话状态：DONE/未推/未部署等按实际）。

---

## Self-Review 记录（计划作者自查）

1. **Spec 覆盖**: B1(T4) B2(T5) B3(T7+T8) B4(T5+T6+T8) B5(T7) B6(T2+T4) B7(T8) / S1(T9+T10) S2(T4+T9+T10+T11) S3(T9+T10) S4(T10) / P1(T12+T13) P2(T14) P3(T13) / M1(T15+T16) M2(T15 cap+T9 top-3；MinHash 每节点上限已存在=零工作) / 阶段⑤(T17)。Schema=T1。全覆盖。
2. **占位符扫描**: 无 TBD/TODO；两处实现者二选一（T6 测试 seeding、T13 helper 位置）均已给出明确默认路径。
3. **类型一致性**: `GraphEdgeRow`/`OutgoingLinkRow`（T4 定义 → T9 消费）、`backfill_inbound_links(agent_id, keys)`（T5 定义 → T5 indexer/T8 经 rename 间接消费）、`MENTION_RELATION/MENTION_CONFIDENCE`（T3 占位 → T15 正体 → T16 消费）、`edge_bright`（T10 内闭环）、`wikilink_click_target`（T12 → T13）——签名逐一核对一致。
4. **已知风险**（实现者注意）: ① T4 改 `get_graph_data` 返回类型是编译期涟漪最大点，gateway 适配必须同 Task 完成；② T5 的 UNIQUE 冲突面已给防御方案，务必带回归测试；③ T10 的 scene.rs 三处并行数组镜像漏一处会导致 debug_assert 失败——按 `edge_kinds` 逐 grep 处理。




