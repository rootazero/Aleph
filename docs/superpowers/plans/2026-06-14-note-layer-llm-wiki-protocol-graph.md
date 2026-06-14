# Note 层 llm_wiki 协议标准化 + 图智能 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 Aleph note 层升级为 Vault 级 llm_wiki 兼容，并新增 4 信号/Louvain/洞察 图智能子系统（物化缓存 + 后台并发重算），同步连线休眠能力并清理死代码。

**Architecture:** 新增独立纯函数模块 `src/memory/notes/graph/`（relevance/community/insights，输入 `GraphSnapshot`，零存储耦合）；新增 `notes_sources`/`notes_graph_cache`/`notes_graph_insights` 物化表，由新 dream 阶段 `GraphRecomputeStage`（`spawn_blocking` + `std::thread::scope` 并发）周期重算；新增 `overview.md`/`purpose.md` 由新 dream 阶段 `CorpusNarrativeStage` LLM 维护；`KnowledgeNote::to_markdown`/解析器补 `type`/`title`/`aliases` 实现 vault 字节兼容；4 信号注入 `note_retrieval`，insights 经 `note_manage` 新 action 暴露。

**Tech Stack:** Rust（tokio async + `std::thread::scope` 并发，**不引 rayon**）、SQLite（`sqlite-vec` 已在）、serde_yaml frontmatter、`async_trait` DreamStage。

---

## ⚠️ 执行约束（覆盖技能默认 TDD 跑测步骤）

用户强制约束：**完成后不跑任何 cargo（check/test/clippy/build），直接提交**；严禁触碰 main。因此：

- 每个 task 仍**先写测试**（红-绿结构保留，测试随代码提交供后续 CI/人工验证）。
- "运行测试验证失败/通过" 步骤一律替换为 **静态自审**：用 `grep`/`Read` 核对 match 臂穷尽、trait 新方法的所有 impl、enum 新变体的所有匹配点、类型签名一致性。**绝不运行 cargo。**
- 每 task 末尾 `git commit`（worktree `/Volumes/TBU4/Workspace/Aleph-wt-note`，分支 `note-protocol-graph`）。
- 提交信息用英文，格式 `<scope>: <desc>`，**不加 Co-Authored-By**（用户全局禁用归属）。
- 所有路径以 worktree 为根。

---

## 文件结构图

| 文件 | 职责 | 动作 |
|---|---|---|
| `src/memory/notes/graph/mod.rs` | `GraphNode`/`GraphSnapshot`/`GraphIndex` | 新建 |
| `src/memory/notes/graph/relevance.rs` | 4 信号评分 + 并发 `all_related` | 新建 |
| `src/memory/notes/graph/community.rs` | 手写 Louvain + cohesion | 新建 |
| `src/memory/notes/graph/insights.rs` | isolated/sparse/bridge/surprising | 新建 |
| `src/memory/notes/graph/tests.rs` | 图算法单测 | 新建 |
| `src/memory/notes/orientation/overview_md.rs` | overview.md 生成器 | 新建 |
| `src/memory/notes/orientation/purpose_md.rs` | purpose.md 生成器 | 新建 |
| `src/memory/dreaming/stages/corpus_narrative.rs` | LLM 维护 overview/purpose | 新建 |
| `src/memory/dreaming/stages/graph_recompute.rs` | 并发重算物化表 | 新建 |
| `src/memory/notes/note/mod.rs` | `KnowledgeNote` + `to_markdown` | 改 |
| `src/memory/notes/note/parsing.rs` | `Frontmatter` 解析 | 改 |
| `src/builtin_tools/note_manage.rs` | 删死代码 + `Insights` action | 改 |
| `src/memory/store/sqlite/schema/ddl.rs` | 3 新表 DDL | 改 |
| `src/memory/store/sqlite/schema/mod.rs` | init 调新表 | 改 |
| `src/memory/store/sqlite/notes.rs` | `index_note` 填 `notes_sources` + 新查询 | 改 |
| `src/memory/notes/store.rs` | `NoteStore` trait 新方法 | 改 |
| `src/memory/note_retrieval/mod.rs` | 4 信号注入图扩展 | 改 |
| `src/memory/notes/orientation/{mod,fs_orientation}.rs` | overview/purpose 接线 + `.obsidian` + compact_for_prompt | 改 |
| `src/memory/dreaming/stages/mod.rs` + `dreaming/mod.rs` | 注册 2 新阶段 | 改 |
| `docs/reference/memory/NOTES.md` | 刷新陈旧章节 | 改 |

各 phase 可独立提交、独立 review。Phase 0/1/2 不依赖 graph；Phase 5 依赖 Phase 3/4。

---

## Phase 0 — 熵减 + 连线先行（低风险）

### Task 0.1: 删除 `frontmatter_template` 死代码

**Files:**
- Modify: `src/builtin_tools/note_manage.rs`（删除 `frontmatter_template` fn 813-827 + 三个测试 `test_frontmatter_wiki_template`/`test_frontmatter_skill_template`/`test_frontmatter_default_template`）

**背景：** 真实写路径是 `KnowledgeNote::to_markdown`；`frontmatter_template` 仅被自身测试调用（已 grep 确认零生产调用），保留只会与 `to_markdown` 双源漂移。

- [ ] **Step 1: 删除函数与测试**

删除 `note_manage.rs` 中：
1. `frontmatter_template` 函数整体（含 doc 注释，约 809-827 行）。
2. `#[cfg(test)] mod tests` 内三个测试：`test_frontmatter_wiki_template`、`test_frontmatter_skill_template`、`test_frontmatter_default_template`。

- [ ] **Step 2: 静态自审（无 cargo）**

```bash
WT=/Volumes/TBU4/Workspace/Aleph-wt-note
grep -rn "frontmatter_template" "$WT/src/"   # 期望：0 结果
```
确认无其它调用点（应为空）。再 `Read` `note_manage.rs` 确认 `mod tests` 仍有保留的 `validate_category_*` / `create_surfaces_related_notes` 等测试、`use super::*;` 未被误删。

- [ ] **Step 3: Commit**

```bash
git -C "$WT" add src/builtin_tools/note_manage.rs
git -C "$WT" commit -m "refactor: drop dead frontmatter_template (real write path is to_markdown)"
```

---

### Task 0.2: 连线休眠的 `SchemaDoc::compact_for_prompt`

**Files:**
- Read first: `src/memory/notes/orientation/fs_orientation.rs`（`read_snapshot` impl，约 22-160 行）、`src/memory/notes/orientation/schema.rs`（`SchemaStore::read` → `SchemaDoc`，`compact_for_prompt` at :47）、`src/memory/notes/orientation/types.rs`（`OrientationSnapshot` 结构）
- Modify: `src/memory/notes/orientation/fs_orientation.rs`

**背景：** `compact_for_prompt()`（schema.rs:47，抽取 Tag Taxonomy/Page Thresholds/Update Policy 三段）零生产调用。`read_snapshot` 组装 `OrientationSnapshot`（含 schema 部分）喂 prompt。让快照的 schema 字段改用 `compact_for_prompt()`，既消除休眠又省 prompt token。

- [ ] **Step 1: 定位 snapshot 的 schema 装配点**

`Read` `fs_orientation.rs` 找到 `read_snapshot` 内读取 schema 的位置（调 `SchemaStore::read(...)` 得到 `SchemaDoc`，再把其文本放进 `OrientationSnapshot` 的 schema 字段）。记下 `SchemaDoc` 当前以何字段进入快照（很可能是 `.raw` 全文或 `.compact_for_prompt()` 尚未用）。

- [ ] **Step 2: 改用 compact_for_prompt**

把装配 schema 文本处由全文/原样改为：

```rust
// 之前：把 schema 全文塞进快照
// 之后：仅注入 compact 视图（Tag Taxonomy + Page Thresholds + Update Policy）
let schema_section = schema_doc.compact_for_prompt();
```

将 `schema_section` 用于 `OrientationSnapshot` 对应字段。若快照需要非空回退：`let schema_section = { let c = schema_doc.compact_for_prompt(); if c.trim().is_empty() { schema_doc.raw_or_default() } else { c } };`（仅当 `compact_for_prompt` 可能为空时；若 `SchemaDoc` 无 `raw` 公开访问器则直接用 compact 结果）。

- [ ] **Step 3: 调整/新增测试**

`read_snapshot` 已有测试 `read_snapshot_returns_all_three_parts`（fs_orientation.rs:258）。新增断言：快照 schema 段包含 compact 小节标题之一（当 SCHEMA.md 含该节时）：

```rust
#[tokio::test]
async fn snapshot_schema_uses_compact_view() {
    // 写一个含 "## Tag Taxonomy" 的 SCHEMA.md，bootstrap 后 read_snapshot，
    // 断言快照 schema 段包含 "Tag Taxonomy" 且不含被 compact 丢弃的其它大节。
    // （沿用 fs_orientation.rs 既有测试的 TempDir + bootstrap 脚手架。）
}
```

- [ ] **Step 4: 静态自审**

```bash
grep -rn "compact_for_prompt" "$WT/src/"   # 期望：现在出现在 fs_orientation.rs（生产调用）+ schema.rs（定义/测试）
```
确认 `compact_for_prompt` 已有生产调用点。`Read` 改动处确认 `SchemaDoc` 方法名/签名匹配（`pub fn compact_for_prompt(&self) -> String`）。

- [ ] **Step 5: Commit**

```bash
git -C "$WT" add src/memory/notes/orientation/fs_orientation.rs
git -C "$WT" commit -m "feat: inject compact schema view into orientation snapshot (wire dormant compact_for_prompt)"
```

---

### Task 0.3: 刷新陈旧的 NOTES.md

**Files:**
- Modify: `docs/reference/memory/NOTES.md`

**背景（已核实陈旧）：** §3 frontmatter 已含 governance/relations/source_notes/severity/confidence；§5.1 管道别名已支持；§8 `notes_links` 已含 `to_raw`/`relation` 列；§9 `src/wiki/` 已删除（index/log 由 `orientation/` 生成）；§12.3 部分过时。

- [ ] **Step 1: 更正各节**

- §3：补充真实 `Frontmatter` 字段集（见 `note/parsing.rs`：category/tags/created/updated/confidence/severity/source_notes/status/supersedes/superseded_by/permanent/relations）。注明 `to_markdown` 实际发射这些字段（Phase 1 后再加 type/title/aliases）。
- §5.1：删除"管道别名不支持"段，改为"`[[target|alias]]` 已支持（wikilink.rs 正则 + `extract_wikilinks_with_alias`）"。
- §8：`notes_links` DDL 补 `to_raw`、`relation` 列说明。
- §9：删除 `WikiGitManager`/`src/wiki/` 休眠段（目录已删），改为"index.md/log.md/SCHEMA.md 由 `src/memory/notes/orientation/` 生成并已连线（`IndexRefresherStage` + ingest 路径）；overview.md/purpose.md 由 `CorpusNarrativeStage` 维护（见新章节）"。
- 新增 §"Knowledge Graph 子系统"：简述 `notes/graph/`（4 信号/Louvain/insights）+ `notes_graph_cache` 物化 + `GraphRecomputeStage`。

- [ ] **Step 2: 静态自审**

`Read` NOTES.md 通读改动节，确认无残留"管道别名不支持"/"src/wiki"/"WikiGitManager" 字样：
```bash
grep -n "管道别名\|src/wiki\|WikiGitManager\|pipe-alias" "$WT/docs/reference/memory/NOTES.md"
```

- [ ] **Step 3: Commit**

```bash
git -C "$WT" add docs/reference/memory/NOTES.md
git -C "$WT" commit -m "docs: refresh stale NOTES.md (frontmatter/wikilink/notes_links/wiki removal)"
```

---

## Phase 1 — Vault 兼容 frontmatter + `.obsidian`

### Task 1.1: 解析器绑定 `type`/`title`/`aliases`

**Files:**
- Modify: `src/memory/notes/note/parsing.rs`（`Frontmatter` struct）
- Modify: `src/memory/notes/note/mod.rs`（`KnowledgeNote` struct + `from_markdown` + `Default`）

- [ ] **Step 1: 写失败测试**

在 `src/memory/notes/note/tests.rs` 增：

```rust
#[test]
fn parses_vault_frontmatter_fields() {
    let md = "---\ncategory: reference\ntype: reference\ntitle: Rust Ownership\naliases: [\"ownership\", \"借用\"]\ntags: [\"rust\"]\ncreated: \"2026-06-14\"\nupdated: \"2026-06-14\"\n---\n\n- borrow checker enforces aliasing xor mutability\n";
    let note = KnowledgeNote::from_markdown("rust-ownership", md).unwrap();
    assert_eq!(note.note_type.as_deref(), Some("reference"));
    assert_eq!(note.aliases, vec!["ownership".to_string(), "借用".to_string()]);
    // title 显式优先于文件名
    assert_eq!(note.title, "rust-ownership"); // title 仍来自文件名参数（见 Step 3 注）
}

#[test]
fn legacy_note_without_vault_fields_defaults_empty() {
    let md = "---\ncategory: learning\ntags: []\ncreated: \"2026-01-01\"\nupdated: \"2026-01-01\"\n---\n\n- fact\n";
    let note = KnowledgeNote::from_markdown("x", md).unwrap();
    assert!(note.note_type.is_none());
    assert!(note.aliases.is_empty());
}
```

- [ ] **Step 2: 扩展 `Frontmatter`（parsing.rs）**

在 `Frontmatter` struct（parsing.rs:60）内追加（全部 `#[serde(default)]` 保证旧笔记零迁移）：

```rust
    #[serde(default, rename = "type")]
    pub(super) note_type: Option<String>,
    #[serde(default)]
    pub(super) title: Option<String>,
    #[serde(default)]
    pub(super) aliases: Vec<String>,
```

- [ ] **Step 3: 扩展 `KnowledgeNote`（mod.rs）**

在 `KnowledgeNote` struct（mod.rs:40）追加字段：

```rust
    /// From frontmatter `type` (Obsidian/llm_wiki page-type, mirrors category).
    /// `None` for legacy notes. Single source of truth remains the directory.
    pub note_type: Option<String>,
    /// Obsidian aliases from frontmatter `aliases:`. Empty for legacy notes.
    pub aliases: Vec<String>,
```

`Default` impl（mod.rs:90）追加：`note_type: None,` `aliases: Vec::new(),`

`from_markdown`（mod.rs:130 的 `Ok(Self { ... })`）追加映射：
```rust
            note_type: frontmatter.note_type,
            aliases: frontmatter.aliases,
```

> 注：`title` 不进 `KnowledgeNote`（已有 `title` 来自文件名，是 SSOT）；`Frontmatter.title` 仅用于 `to_markdown` 往返兼容（Task 1.2 发射）。`from_markdown` 不读 `frontmatter.title`，避免与文件名 title 冲突。为消除"未读字段"告警，`Frontmatter.title` 加 `#[allow(dead_code)]` 或在 to_markdown 路径不依赖它——见 Task 1.2 决定不存 title 进结构，故此处 `Frontmatter.title` 标 `#[allow(dead_code)]`。

- [ ] **Step 4: 静态自审**

```bash
grep -n "KnowledgeNote {" "$WT/src/memory/notes/note/mod.rs"   # 找所有结构字面量构造点
grep -rn "KnowledgeNote {" "$WT/src/"                          # 全仓构造点须补新字段或用 ..Default::default()
```
逐个构造点确认：要么用 `..Default::default()`（note_manage.rs:324 已是），要么显式补 `note_type`/`aliases`。重点核 `from_markdown` 的 `Ok(Self{})` 已含全部字段（数一遍字段数 = struct 字段数）。

- [ ] **Step 5: Commit**

```bash
git -C "$WT" add src/memory/notes/note/parsing.rs src/memory/notes/note/mod.rs src/memory/notes/note/tests.rs
git -C "$WT" commit -m "feat: parse vault frontmatter (type/title/aliases) into KnowledgeNote"
```

---

### Task 1.2: `to_markdown` 发射 `type`/`title`/`aliases`（vault 字节兼容）

**Files:**
- Modify: `src/memory/notes/note/mod.rs`（`to_markdown` at :167）

- [ ] **Step 1: 写失败测试**（note/tests.rs）

```rust
#[test]
fn to_markdown_emits_vault_fields_and_roundtrips() {
    let mut n = KnowledgeNote { title: "rust-ownership".into(), category: "reference".into(),
        tags: vec!["rust".into()], ..Default::default() };
    n.note_type = Some("reference".into());
    n.aliases = vec!["ownership".into()];
    n.facts = vec!["fact one".into()];
    let md = n.to_markdown();
    assert!(md.contains("type: reference"));
    assert!(md.contains("title: rust-ownership"));
    assert!(md.contains("aliases: [\"ownership\"]"));
    // round-trip：解析回来字段保持
    let back = KnowledgeNote::from_markdown("rust-ownership", &md).unwrap();
    assert_eq!(back.note_type.as_deref(), Some("reference"));
    assert_eq!(back.aliases, vec!["ownership".to_string()]);
}

#[test]
fn to_markdown_defaults_type_to_category_and_title_to_filename() {
    let n = KnowledgeNote { title: "editor-prefs".into(), category: "preference".into(),
        ..Default::default() };
    let md = n.to_markdown();
    assert!(md.contains("type: preference"));        // 缺省镜像 category
    assert!(md.contains("title: editor-prefs"));     // 缺省取文件名 title
}
```

- [ ] **Step 2: 修改 `to_markdown`**

在 `to_markdown`（mod.rs:175 起 `out.push_str("---\n")` 之后、`category:` 行**之前**）插入 vault 字段，保持其余字段顺序不变（纯前插，旧字段字节顺序不变；新字段对 Obsidian/llm_wiki 良性）：

```rust
        out.push_str("---\n");
        // Vault-compat header (Obsidian / llm_wiki): type/title/aliases.
        let note_type = self.note_type.clone().unwrap_or_else(|| self.category.clone());
        out.push_str(&format!("type: {note_type}\n"));
        out.push_str(&format!("title: {}\n", self.title));
        out.push_str(&format!("aliases: {}\n", yaml_inline_array(&self.aliases)));
        out.push_str(&format!("category: {}\n", self.category));
```
（即在原有 `out.push_str(&format!("category: {}\n", self.category));` 上方追加三行，并删除重复的 `out.push_str("---\n")`——保留一个开头 fence。）

`yaml_inline_array` 已在 mod.rs 引入（`use helpers::yaml_inline_array`）。

- [ ] **Step 3: 静态自审**

`Read` `to_markdown` 全文，确认：① 只有一处 `---\n` 开 fence；② 字段顺序为 type→title→aliases→category→tags→created→...（旧字段相对顺序不变）；③ `aliases` 用 `yaml_inline_array`（与 tags 同序列化器，`[]` 空时合法）。round-trip 测试覆盖解析侧（Task 1.1 已加字段）。

- [ ] **Step 4: Commit**

```bash
git -C "$WT" add src/memory/notes/note/mod.rs src/memory/notes/note/tests.rs
git -C "$WT" commit -m "feat: emit vault frontmatter (type/title/aliases) in to_markdown for Obsidian byte-compat"
```

---

### Task 1.3: `.obsidian/` vault 配置自动生成

**Files:**
- Create: `src/memory/notes/orientation/obsidian_config.rs`
- Modify: `src/memory/notes/orientation/mod.rs`（`pub mod obsidian_config;` + re-export）
- Modify: `src/memory/notes/orientation/fs_orientation.rs`（bootstrap 调 `ensure_obsidian_config`）

- [ ] **Step 1: 写生成器 + 测试**

`obsidian_config.rs`：

```rust
//! One-shot `.obsidian/` vault config so the per-agent note directory opens
//! cleanly in Obsidian (graph view + wikilinks). Idempotent: never overwrites
//! existing user config.

use std::path::Path;
use crate::error::AlephError;

const APP_JSON: &str = r#"{"alwaysUpdateLinks":true,"newLinkFormat":"shortest","useMarkdownLinks":false}"#;
const CORE_PLUGINS_JSON: &str = r#"["file-explorer","global-search","graph","backlink","outgoing-link","tag-pane","page-preview"]"#;
const GRAPH_JSON: &str = r#"{"collapse-filter":true,"showTags":true,"showAttachments":false,"hideUnresolved":false,"showOrphans":true}"#;

/// Write `.obsidian/{app,core-plugins,graph}.json` under `agent_dir` if absent.
/// Best-effort: an existing file is left untouched (user owns their config).
pub async fn ensure_obsidian_config(agent_dir: &Path) -> Result<(), AlephError> {
    let dir = agent_dir.join(".obsidian");
    tokio::fs::create_dir_all(&dir).await
        .map_err(|e| AlephError::other(format!("create .obsidian: {e}")))?;
    for (name, body) in [
        ("app.json", APP_JSON),
        ("core-plugins.json", CORE_PLUGINS_JSON),
        ("graph.json", GRAPH_JSON),
    ] {
        let p = dir.join(name);
        if !p.exists() {
            tokio::fs::write(&p, body).await
                .map_err(|e| AlephError::other(format!("write {name}: {e}")))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn writes_three_config_files_idempotently() {
        let d = tempfile::tempdir().unwrap();
        ensure_obsidian_config(d.path()).await.unwrap();
        for f in ["app.json","core-plugins.json","graph.json"] {
            assert!(d.path().join(".obsidian").join(f).exists());
        }
        // second call must not error / must not clobber
        tokio::fs::write(d.path().join(".obsidian/app.json"), "USER").await.unwrap();
        ensure_obsidian_config(d.path()).await.unwrap();
        let kept = tokio::fs::read_to_string(d.path().join(".obsidian/app.json")).await.unwrap();
        assert_eq!(kept, "USER");
    }
}
```

- [ ] **Step 2: 注册 + 接线**

`orientation/mod.rs`：加 `pub mod obsidian_config;`，并 `pub use obsidian_config::ensure_obsidian_config;`

`fs_orientation.rs`：在 bootstrap 路径（即首次为 agent 建立 orientation 处，与 `SchemaStore::write` bootstrap 同一函数，约 :104-116）末尾调用：
```rust
        crate::memory::notes::orientation::ensure_obsidian_config(&agent_dir).await?;
```
（`agent_dir` 为 `memory_dir/{agent_id}`，bootstrap 上下文已有该路径变量；若变量名不同按实际改。）

- [ ] **Step 3: 静态自审**

```bash
grep -rn "ensure_obsidian_config" "$WT/src/"   # 期望：定义 + mod re-export + bootstrap 调用点
```
`Read` bootstrap 函数确认 `agent_dir` 路径变量存在且为 `.../{agent_id}`。

- [ ] **Step 4: Commit**

```bash
git -C "$WT" add src/memory/notes/orientation/obsidian_config.rs src/memory/notes/orientation/mod.rs src/memory/notes/orientation/fs_orientation.rs
git -C "$WT" commit -m "feat: auto-generate .obsidian vault config for note directory"
```

---

## Phase 2 — overview.md + purpose.md（LLM 维护）

### Task 2.1: `overview_md.rs` + `purpose_md.rs` 生成器

**Files:**
- Create: `src/memory/notes/orientation/overview_md.rs`
- Create: `src/memory/notes/orientation/purpose_md.rs`
- Modify: `src/memory/notes/orientation/mod.rs`（注册）

**背景：** 镜像 `index_md.rs` 的 `new(agent_dir)`/`write`/`read` 结构，但内容是 LLM 生成文本（生成器只负责落盘/读取，文本由 Task 2.3 的阶段提供）。

- [ ] **Step 1: 写生成器 + 测试**

`overview_md.rs`：
```rust
//! `overview.md` — LLM-maintained global synthesis of the note corpus.
//! The generator only does idempotent disk I/O (header + body); the synthesis
//! text is produced by `CorpusNarrativeStage`.

use std::path::PathBuf;
use crate::error::AlephError;

pub const OVERVIEW_FILENAME: &str = "overview.md";

pub struct OverviewMd { agent_dir: PathBuf }

impl OverviewMd {
    pub fn new(agent_dir: impl Into<PathBuf>) -> Self { Self { agent_dir: agent_dir.into() } }
    fn path(&self) -> PathBuf { self.agent_dir.join(OVERVIEW_FILENAME) }

    /// Write the synthesis body with an auto-generated header banner.
    pub async fn write(&self, body: &str) -> Result<(), AlephError> {
        tokio::fs::create_dir_all(&self.agent_dir).await
            .map_err(|e| AlephError::other(format!("create overview dir: {e}")))?;
        let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
        let text = format!("<!-- auto-generated by CorpusNarrativeStage | updated: {now} -->\n\n# Overview\n\n{}\n", body.trim());
        tokio::fs::write(self.path(), text).await
            .map_err(|e| AlephError::other(format!("write overview: {e}")))
    }

    /// Read current overview body (empty string if absent).
    pub async fn read(&self) -> String {
        tokio::fs::read_to_string(self.path()).await.unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn write_then_read_roundtrip() {
        let d = tempfile::tempdir().unwrap();
        let g = OverviewMd::new(d.path());
        g.write("The corpus centres on Rust async and the user's editor setup.").await.unwrap();
        let got = g.read().await;
        assert!(got.contains("# Overview"));
        assert!(got.contains("Rust async"));
    }
}
```

`purpose_md.rs`：同结构，`PURPOSE_FILENAME = "purpose.md"`，`struct PurposeMd`，header banner 文案 `# Purpose`，注释 "seeded + maintained by CorpusNarrativeStage; user-editable"。`write`/`read` 同形。测试同形（断言 `# Purpose`）。

- [ ] **Step 2: 注册**

`orientation/mod.rs`：`pub mod overview_md;` `pub mod purpose_md;` + `pub use overview_md::OverviewMd; pub use purpose_md::PurposeMd;`

- [ ] **Step 3: 静态自审**

```bash
grep -n "pub mod overview_md\|pub mod purpose_md" "$WT/src/memory/notes/orientation/mod.rs"
```
确认两生成器 `AlephError::other` 签名与 index_md.rs 一致（`other(String)`）。

- [ ] **Step 4: Commit**

```bash
git -C "$WT" add src/memory/notes/orientation/overview_md.rs src/memory/notes/orientation/purpose_md.rs src/memory/notes/orientation/mod.rs
git -C "$WT" commit -m "feat: overview.md + purpose.md orientation generators (disk I/O)"
```

---

### Task 2.2: `CorpusNarrativeStage` dream 阶段

**Files:**
- Create: `src/memory/dreaming/stages/corpus_narrative.rs`
- Modify: `src/memory/dreaming/stages/mod.rs`（注册 mod + re-export）
- Modify: `src/memory/dreaming/mod.rs`（Synthesize 策略加阶段 + 可选列入 `GLOBAL_ONLY_STAGES`）

**背景：** 镜像 `daily_digest.rs` 的 LLM 调用模式（`ctx.provider.process(RequestPayload::new(&msgs).with_system(...))` → `response.text_content()`）。读 index + 近期笔记摘要 + 现有 overview/purpose → 一次 LLM 调用产出新 overview（整库综述）+ 增量 purpose。落盘走 Task 2.1 生成器。R7/R9：综述是 LLM 语义生成，非确定性替代。

- [ ] **Step 1: 写阶段 + 测试（should_run 谓词）**

```rust
//! `CorpusNarrative` stage — LLM-maintained overview.md + purpose.md.
//!
//! Reads the index + recent note previews + current overview/purpose, then asks
//! the LLM to (re)write a global synthesis (overview) and refine the corpus
//! purpose. Runs on the high-growth Synthesize path. R7/R9: the synthesis is
//! LLM semantic generation, not deterministic substitution.

use async_trait::async_trait;

use crate::error::AlephError;
use crate::memory::dreaming::DreamContext;
use crate::memory::notes::orientation::{OverviewMd, PurposeMd};
use crate::providers::adapter::RequestPayload;
use crate::providers::message::UnifiedMessage;

use super::DreamStage;

/// Minimum notes before a corpus-level narrative is worth generating.
const MIN_NOTES_FOR_NARRATIVE: usize = 5;
/// How many recent notes to preview into the prompt.
const PREVIEW_NOTES: usize = 40;

pub struct CorpusNarrativeStage;

#[async_trait]
impl DreamStage for CorpusNarrativeStage {
    fn name(&self) -> &'static str { "corpus_narrative" }

    async fn should_run(&self, ctx: &DreamContext) -> bool {
        ctx.notes.len() >= MIN_NOTES_FOR_NARRATIVE
    }

    async fn execute(&self, mut ctx: DreamContext) -> Result<DreamContext, AlephError> {
        // agent vault dir = memory_dir/{agent_id}
        let agent_dir = ctx.indexer.memory_dir().join(&ctx.agent_id);
        let overview_gen = OverviewMd::new(&agent_dir);
        let purpose_gen = PurposeMd::new(&agent_dir);
        let cur_overview = overview_gen.read().await;
        let cur_purpose = purpose_gen.read().await;

        // Recent-note previews (most-recently-updated first).
        let mut idx: Vec<usize> = (0..ctx.notes.len()).collect();
        idx.sort_by_key(|&i| std::cmp::Reverse(ctx.notes[i].updated_at));
        idx.truncate(PREVIEW_NOTES);
        let mut previews = Vec::new();
        for &i in &idx {
            let path = ctx.notes[i].path.clone();
            let cat = ctx.notes[i].category.clone();
            let content = ctx.load_content(&path).await.unwrap_or_default();
            let preview: String = content.chars().take(160).collect();
            previews.push(format!("- {path} ({cat}): {preview}"));
        }

        let system = "You maintain a personal knowledge vault. Produce two sections separated by a line containing exactly '===PURPOSE==='. \
First an OVERVIEW: a tight global synthesis (5-10 sentences) of what the corpus is about, its major themes, and how they connect. \
Then a PURPOSE: 3-6 bullet points capturing why this vault exists — the owner's goals and the key questions it should answer. \
If a current purpose is given, refine it minimally rather than rewriting wholesale.";
        let prompt = format!(
            "Current overview:\n{cur_overview}\n\nCurrent purpose:\n{cur_purpose}\n\nRecent notes ({} shown):\n{}\n\nWrite the new OVERVIEW, then '===PURPOSE===', then the PURPOSE.",
            previews.len(), previews.join("\n")
        );

        let msgs = vec![UnifiedMessage::user(&prompt)];
        let response = ctx.provider
            .process(RequestPayload::new(&msgs).with_system(Some(system)))
            .await
            .map_err(|e| AlephError::other(format!("corpus narrative LLM call failed: {e}")))?;
        let text = response.text_content();

        // Split on the sentinel; fall back to overview-only if absent.
        let (overview_body, purpose_body) = match text.split_once("===PURPOSE===") {
            Some((o, p)) => (o.trim().to_string(), p.trim().to_string()),
            None => (text.trim().to_string(), String::new()),
        };
        if !overview_body.is_empty() {
            overview_gen.write(&overview_body).await?;
        }
        // Idempotent purpose: only rewrite when the model produced a non-empty,
        // materially different body (avoid churn on every synthesize cycle).
        if !purpose_body.is_empty() && purpose_body != cur_purpose.trim() {
            purpose_gen.write(&purpose_body).await?;
        }
        tracing::info!(notes = ctx.notes.len(), "corpus narrative regenerated");
        Ok(ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn split_sentinel_separates_overview_and_purpose() {
        let text = "Overview body here.\n===PURPOSE===\n- goal one\n- goal two";
        let (o, p) = text.split_once("===PURPOSE===").unwrap();
        assert!(o.trim().ends_with("here."));
        assert!(p.contains("goal one"));
    }
    #[test]
    fn missing_sentinel_falls_back_to_overview_only() {
        let text = "Just an overview, no purpose marker.";
        assert!(text.split_once("===PURPOSE===").is_none());
    }
}
```

> 校验点：`ctx.indexer.memory_dir()` 返回 `&Path`/`PathBuf`？`NoteIndexer::memory_dir()` 在 note_manage.rs:310 以 `.join(...)` 链式使用，证明返回可 `.join` 的路径引用——直接 `.join(&ctx.agent_id)` 可用。`UnifiedMessage::user` / `RequestPayload::new().with_system` / `response.text_content()` 全部与 `daily_digest.rs` 同款。

- [ ] **Step 2: 注册阶段**

`stages/mod.rs`：加 `pub mod corpus_narrative;` + `pub use corpus_narrative::CorpusNarrativeStage;`

`dreaming/mod.rs`：在 **Synthesize** 分支（:199-225）的 `DailyDigestStage` 之前插入 `Box::new(stages::CorpusNarrativeStage),`。并把 `"corpus_narrative"` 加进 `GLOBAL_ONLY_STAGES`（:243，overview/purpose 跟随 agent 语料、不按 project 分叉）。

- [ ] **Step 3: 静态自审**

```bash
grep -n "CorpusNarrativeStage\|corpus_narrative" "$WT/src/memory/dreaming/mod.rs" "$WT/src/memory/dreaming/stages/mod.rs"
```
确认：① stages/mod.rs 有 mod+re-export；② Synthesize vec 内有 `Box::new(stages::CorpusNarrativeStage)`；③ `GLOBAL_ONLY_STAGES` 含 `"corpus_narrative"`。`Read` 确认 `DreamStage` trait 三方法签名匹配（name/should_run/execute）。

- [ ] **Step 4: Commit**

```bash
git -C "$WT" add src/memory/dreaming/stages/corpus_narrative.rs src/memory/dreaming/stages/mod.rs src/memory/dreaming/mod.rs
git -C "$WT" commit -m "feat: CorpusNarrativeStage — LLM-maintained overview.md + purpose.md (Synthesize)"
```

---

## Phase 3 — 图智能模块（纯函数，无存储耦合）

### Task 3.1: `graph/mod.rs` — 快照与索引类型

**Files:**
- Create: `src/memory/notes/graph/mod.rs`
- Modify: `src/memory/notes/mod.rs`（加 `pub mod graph;`）

- [ ] **Step 1: 写类型 + 测试**

```rust
//! Note knowledge-graph intelligence: 4-signal relevance, Louvain community
//! detection, graph-health insights. Pure functions over an immutable
//! `GraphSnapshot` — zero storage coupling (P4). Consumed by the offline
//! `GraphRecomputeStage` (materialization) and `note_retrieval` (seed
//! expansion). No external graph crate (R3); concurrency via std threads.

pub mod community;
pub mod insights;
pub mod relevance;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};

/// One node in the note graph.
#[derive(Debug, Clone)]
pub struct GraphNode {
    pub path: String,    // "category/filename"
    pub category: String,
    pub sources: Vec<String>, // frontmatter `source_notes`
}

/// Immutable snapshot of the note graph, built once per recompute.
#[derive(Debug, Clone, Default)]
pub struct GraphSnapshot {
    pub nodes: Vec<GraphNode>,
    /// Directed resolved edges (`category/filename` pairs); wikilinks + typed
    /// relations both live in `notes_links`.
    pub edges: Vec<(String, String)>,
}

/// Derived adjacency + lookup, shared by all three algorithms. Built once.
pub struct GraphIndex<'a> {
    pub nodes: &'a [GraphNode],
    idx_of: HashMap<&'a str, usize>,
    /// Undirected, deduped adjacency by node index.
    pub adj: Vec<HashSet<usize>>,
    /// Source-set per node index (from `source_notes`).
    pub sources: Vec<HashSet<&'a str>>,
}

impl<'a> GraphIndex<'a> {
    #[must_use]
    pub fn build(snap: &'a GraphSnapshot) -> Self {
        let mut idx_of = HashMap::with_capacity(snap.nodes.len());
        for (i, n) in snap.nodes.iter().enumerate() {
            idx_of.insert(n.path.as_str(), i);
        }
        let mut adj = vec![HashSet::new(); snap.nodes.len()];
        for (from, to) in &snap.edges {
            if let (Some(&a), Some(&b)) = (idx_of.get(from.as_str()), idx_of.get(to.as_str())) {
                if a != b {
                    adj[a].insert(b);
                    adj[b].insert(a);
                }
            }
        }
        let sources = snap.nodes.iter()
            .map(|n| n.sources.iter().map(String::as_str).collect::<HashSet<_>>())
            .collect();
        Self { nodes: snap.nodes, idx_of, adj, sources }
    }

    #[must_use] pub fn len(&self) -> usize { self.nodes.len() }
    #[must_use] pub fn is_empty(&self) -> bool { self.nodes.is_empty() }
    #[must_use] pub fn degree(&self, i: usize) -> usize { self.adj[i].len() }
    #[must_use] pub fn index_of(&self, path: &str) -> Option<usize> { self.idx_of.get(path).copied() }
}
```

`src/memory/notes/mod.rs`：加 `pub mod graph;`

- [ ] **Step 2: 静态自审**

```bash
grep -n "pub mod graph" "$WT/src/memory/notes/mod.rs"
```
（tests.rs 在 Task 3.4 末尾补总测试；此 task 先建空 `tests.rs`：`// graph algorithm tests — see Task 3.4`，避免 `mod tests` 引用缺文件。）

- [ ] **Step 3: Commit**

```bash
git -C "$WT" add src/memory/notes/graph/mod.rs src/memory/notes/graph/tests.rs src/memory/notes/mod.rs
git -C "$WT" commit -m "feat: graph module skeleton (GraphSnapshot/GraphIndex)"
```

---

### Task 3.2: `relevance.rs` — 4 信号 + 并发 `all_related`

**Files:**
- Create: `src/memory/notes/graph/relevance.rs`

- [ ] **Step 1: 写实现 + 测试**

```rust
//! 4-signal relevance: direct-link ×3, source-overlap ×4, Adamic-Adar ×1.5,
//! type-affinity ×1. Concurrency for the full pairwise pass via std threads.

use std::collections::HashSet;

use super::GraphIndex;

/// Tunable weights (defaults mirror the reference protocol).
#[derive(Debug, Clone, Copy)]
pub struct SignalWeights {
    pub direct_link: f32,
    pub source_overlap: f32,
    pub adamic_adar: f32,
    pub type_affinity: f32,
}
impl Default for SignalWeights {
    fn default() -> Self {
        Self { direct_link: 3.0, source_overlap: 4.0, adamic_adar: 1.5, type_affinity: 1.0 }
    }
}

/// Relatedness of nodes `a` and `b` (by index).
#[must_use]
pub fn score_pair(g: &GraphIndex, w: &SignalWeights, a: usize, b: usize) -> f32 {
    if a == b { return 0.0; }
    let mut s = 0.0;
    if g.adj[a].contains(&b) { s += w.direct_link; }
    let overlap = g.sources[a].intersection(&g.sources[b]).count();
    if overlap > 0 { s += w.source_overlap * overlap as f32; }
    let mut aa = 0.0_f32;
    for &c in g.adj[a].intersection(&g.adj[b]) {
        let d = g.degree(c);
        if d > 1 { aa += 1.0 / (d as f32).ln(); }
    }
    s += w.adamic_adar * aa;
    if g.nodes[a].category == g.nodes[b].category { s += w.type_affinity; }
    s
}

/// Top-`k` related nodes for `seed` path, descending score (ties by path).
/// Candidate set is bounded to the local 2-hop neighbourhood + source-sharing
/// nodes, so cost is local, not O(N).
#[must_use]
pub fn related(g: &GraphIndex, w: &SignalWeights, seed: usize, k: usize) -> Vec<(String, f32)> {
    let mut cand: HashSet<usize> = HashSet::new();
    for &n1 in &g.adj[seed] {
        cand.insert(n1);
        for &n2 in &g.adj[n1] { cand.insert(n2); }
    }
    if !g.sources[seed].is_empty() {
        for i in 0..g.len() {
            if i != seed && g.sources[i].intersection(&g.sources[seed]).next().is_some() {
                cand.insert(i);
            }
        }
    }
    cand.remove(&seed);
    let mut scored: Vec<(String, f32)> = cand.into_iter()
        .map(|c| (g.nodes[c].path.clone(), score_pair(g, w, seed, c)))
        .filter(|(_, sc)| *sc > 0.0)
        .collect();
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.0.cmp(&b.0))
    });
    scored.truncate(k);
    scored
}

/// Top-`k` related lists for every node, parallelised across `threads` OS
/// threads via `std::thread::scope` (no external dep). Run inside
/// `tokio::task::spawn_blocking`. Deterministic: output is in node-index order.
#[must_use]
pub fn all_related(g: &GraphIndex, w: &SignalWeights, k: usize, threads: usize)
    -> Vec<(String, Vec<(String, f32)>)>
{
    let n = g.len();
    if n == 0 { return Vec::new(); }
    let threads = threads.clamp(1, n);
    let chunk = n.div_ceil(threads);
    let mut out: Vec<Vec<(String, Vec<(String, f32)>)>> = Vec::new();
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads).map(|t| {
            let start = t * chunk;
            let end = ((t + 1) * chunk).min(n);
            scope.spawn(move || {
                let mut part = Vec::with_capacity(end.saturating_sub(start));
                for i in start..end {
                    part.push((g.nodes[i].path.clone(), related(g, w, i, k)));
                }
                part
            })
        }).collect();
        for h in handles { out.push(h.join().expect("relevance worker panicked")); }
    });
    out.into_iter().flatten().collect()
}
```

测试（写进 `graph/tests.rs`，见 Task 3.4 汇总；本 task 至少加这两条）：
```rust
#[test]
fn direct_link_and_type_affinity_score() {
    use crate::memory::notes::graph::*;
    let snap = GraphSnapshot {
        nodes: vec![
            GraphNode { path: "learning/a".into(), category: "learning".into(), sources: vec![] },
            GraphNode { path: "learning/b".into(), category: "learning".into(), sources: vec![] },
        ],
        edges: vec![("learning/a".into(), "learning/b".into())],
    };
    let g = GraphIndex::build(&snap);
    let w = relevance::SignalWeights::default();
    // direct link (3) + type affinity (1) = 4
    assert!((relevance::score_pair(&g, &w, 0, 1) - 4.0).abs() < 1e-4);
}

#[test]
fn source_overlap_scores() {
    use crate::memory::notes::graph::*;
    let snap = GraphSnapshot {
        nodes: vec![
            GraphNode { path: "p/a".into(), category: "x".into(), sources: vec!["raw/1".into()] },
            GraphNode { path: "p/b".into(), category: "y".into(), sources: vec!["raw/1".into()] },
        ],
        edges: vec![],
    };
    let g = GraphIndex::build(&snap);
    let w = relevance::SignalWeights::default();
    // one shared source = 4.0, no link, different type
    assert!((relevance::score_pair(&g, &w, 0, 1) - 4.0).abs() < 1e-4);
}
```

- [ ] **Step 2: 静态自审**

`Read` relevance.rs，核对：`std::thread::scope` 闭包捕获 `g`（`&GraphIndex`，Sync）、`w`（Copy）、`k`（Copy）合法；`GraphIndex` 字段 `idx_of` 私有不影响（`related` 用 `g.adj`/`g.sources`/`g.nodes` 公有）。`div_ceil` 是 std 稳定方法（Rust 1.73+，MSRV 1.95 满足）。

- [ ] **Step 3: Commit**

```bash
git -C "$WT" add src/memory/notes/graph/relevance.rs src/memory/notes/graph/tests.rs
git -C "$WT" commit -m "feat: 4-signal note relevance with std::thread::scope parallel pass"
```

---

### Task 3.3: `community.rs` — 手写 Louvain + cohesion

**Files:**
- Create: `src/memory/notes/graph/community.rs`

> ⚠️ **本 task 是全计划最高风险点**（复杂算法 + 无 cargo 验证）。静态自审须逐行核对模块度数学；附带的 barbell 测试是收敛正确性的关键守卫，CI 恢复后**第一个**要跑。

- [ ] **Step 1: 写实现 + 测试**

```rust
//! Louvain community detection (modularity maximisation) over the undirected
//! note graph. Hand-rolled, no external crate (R3). Deterministic: nodes are
//! visited in index order and ties break to the lower community id, so the same
//! graph always yields the same partition.

use std::collections::HashMap;

use super::GraphIndex;

/// Community assignment + per-community cohesion (intra-edge density).
pub struct Communities {
    /// Dense community id (0..k) per node index.
    pub of_node: Vec<usize>,
    /// Cohesion per community id: actual intra edges / possible intra edges.
    pub cohesion: Vec<f32>,
}

type Adj = Vec<Vec<(usize, f64)>>; // weighted adjacency

#[must_use]
pub fn detect(g: &GraphIndex) -> Communities {
    let n = g.len();
    if n == 0 { return Communities { of_node: vec![], cohesion: vec![] }; }
    // Base weighted adjacency (unit weights) from undirected adjacency.
    let adj: Adj = (0..n)
        .map(|i| g.adj[i].iter().map(|&j| (j, 1.0)).collect())
        .collect();
    let total_w: f64 = adj.iter().flat_map(|nbrs| nbrs.iter().map(|(_, w)| *w)).sum::<f64>() / 2.0;
    if total_w == 0.0 {
        // No edges → each node is its own singleton community.
        return Communities { of_node: (0..n).collect(), cohesion: vec![0.0; n] };
    }
    let raw = louvain(&adj, total_w);
    let of_node = renumber(&raw);
    let k = of_node.iter().copied().max().map_or(0, |m| m + 1);
    let cohesion = cohesion_per_community(g, &of_node, k);
    Communities { of_node, cohesion }
}

/// Iterated Louvain: local-moving to a modularity optimum, aggregate, repeat
/// until no further merging. Returns base-node → community id.
fn louvain(adj0: &Adj, total_w: f64) -> Vec<usize> {
    let mut node_comm: Vec<usize> = (0..adj0.len()).collect();
    let mut adj = adj0.clone();
    loop {
        let part = local_moving(&adj, total_w);
        let dense = renumber(&part);
        let k = dense.iter().copied().max().map_or(0, |m| m + 1);
        if k == adj.len() { break; } // nothing merged → converged
        for c in &mut node_comm { *c = dense[*c]; }
        // Aggregate communities into super-nodes (self-loops carry intra weight).
        let mut acc: Vec<HashMap<usize, f64>> = vec![HashMap::new(); k];
        for (u, nbrs) in adj.iter().enumerate() {
            let cu = dense[u];
            for &(v, w) in nbrs {
                *acc[cu].entry(dense[v]).or_default() += w;
            }
        }
        adj = acc.into_iter().map(|m| m.into_iter().collect()).collect();
        if k == 1 { break; }
    }
    node_comm
}

/// One pass of local moving to a modularity optimum on weighted `adj`.
fn local_moving(adj: &Adj, total_w: f64) -> Vec<usize> {
    let n = adj.len();
    let mut comm: Vec<usize> = (0..n).collect();
    // Weighted degree (incident weight, self-loops counted once per their weight).
    let k: Vec<f64> = adj.iter().map(|nbrs| nbrs.iter().map(|(_, w)| *w).sum()).collect();
    let mut sigma_tot = k.clone(); // total incident weight per community
    let two_m = 2.0 * total_w;
    let mut improved = true;
    while improved {
        improved = false;
        for i in 0..n {
            let ci = comm[i];
            sigma_tot[ci] -= k[i]; // remove i from its community
            // Weight from i to each neighbouring community.
            let mut w_to: HashMap<usize, f64> = HashMap::new();
            for &(j, w) in &adj[i] {
                if j != i {
                    *w_to.entry(comm[j]).or_default() += w;
                }
            }
            // Best community by ΔQ = w_to(c) - k_i * sigma_tot(c) / (2m).
            let mut best_c = ci;
            let mut best_gain = w_to.get(&ci).copied().unwrap_or(0.0) - k[i] * sigma_tot[ci] / two_m;
            for (&c, &wic) in &w_to {
                let gain = wic - k[i] * sigma_tot[c] / two_m;
                if gain > best_gain + 1e-12 || (gain > best_gain - 1e-12 && c < best_c) {
                    best_gain = gain;
                    best_c = c;
                }
            }
            comm[i] = best_c;
            sigma_tot[best_c] += k[i];
            if best_c != ci { improved = true; }
        }
    }
    comm
}

/// Renumber arbitrary community labels to dense 0..k in first-seen order.
fn renumber(comm: &[usize]) -> Vec<usize> {
    let mut map: HashMap<usize, usize> = HashMap::new();
    let mut next = 0;
    comm.iter().map(|&c| *map.entry(c).or_insert_with(|| { let v = next; next += 1; v })).collect()
}

/// Intra-edge density per community over the *base* graph.
fn cohesion_per_community(g: &GraphIndex, of_node: &[usize], k: usize) -> Vec<f32> {
    let mut size = vec![0usize; k];
    for &c in of_node { size[c] += 1; }
    let mut intra = vec![0usize; k];
    for i in 0..g.len() {
        for &j in &g.adj[i] {
            if i < j && of_node[i] == of_node[j] {
                intra[of_node[i]] += 1;
            }
        }
    }
    (0..k).map(|c| {
        let s = size[c];
        if s < 2 { return 0.0; }
        let possible = s * (s - 1) / 2;
        intra[c] as f32 / possible as f32
    }).collect()
}
```

测试（graph/tests.rs）：
```rust
#[test]
fn louvain_splits_barbell_into_two_communities() {
    use crate::memory::notes::graph::*;
    // Two triangles {a,b,c} and {d,e,f}, joined by a single c-d bridge edge.
    let node = |p: &str| GraphNode { path: p.into(), category: "x".into(), sources: vec![] };
    let snap = GraphSnapshot {
        nodes: vec![node("g/a"), node("g/b"), node("g/c"), node("g/d"), node("g/e"), node("g/f")],
        edges: vec![
            ("g/a".into(),"g/b".into()), ("g/b".into(),"g/c".into()), ("g/a".into(),"g/c".into()),
            ("g/d".into(),"g/e".into()), ("g/e".into(),"g/f".into()), ("g/d".into(),"g/f".into()),
            ("g/c".into(),"g/d".into()),
        ],
    };
    let g = GraphIndex::build(&snap);
    let c = community::detect(&g);
    // a,b,c same community; d,e,f same community; the two differ.
    assert_eq!(c.of_node[0], c.of_node[1]);
    assert_eq!(c.of_node[1], c.of_node[2]);
    assert_eq!(c.of_node[3], c.of_node[4]);
    assert_eq!(c.of_node[4], c.of_node[5]);
    assert_ne!(c.of_node[0], c.of_node[3]);
    // each triangle is fully cohesive (3 intra edges / 3 possible = 1.0)
    assert!((c.cohesion[c.of_node[0]] - 1.0).abs() < 1e-4);
}

#[test]
fn louvain_empty_and_edgeless() {
    use crate::memory::notes::graph::*;
    let g0 = GraphIndex::build(&GraphSnapshot::default());
    assert!(community::detect(&g0).of_node.is_empty());
    let snap = GraphSnapshot {
        nodes: vec![GraphNode{path:"p/a".into(),category:"x".into(),sources:vec![]},
                    GraphNode{path:"p/b".into(),category:"x".into(),sources:vec![]}],
        edges: vec![],
    };
    let g = GraphIndex::build(&snap);
    let c = community::detect(&g);
    assert_ne!(c.of_node[0], c.of_node[1]); // singletons
}
```

- [ ] **Step 2: 静态自审（逐行核模块度）**

`Read` community.rs 全文，核对：① `local_moving` 中 `sigma_tot[ci] -= k[i]` 后、选定 `best_c` 前不把 i 计入任何社区（`comm[i]` 暂未改，但 `w_to` 用 `comm[j]`，j≠i 故不含 i 自身——OK）；② ΔQ 公式 `wic - k[i]*sigma_tot[c]/two_m` 对应标准 Louvain 增益；③ 聚合 `acc[cu].entry(dense[v])` 含 cu==cv 自环（intra 权重保留）；④ 终止条件 `k == adj.len()`（无合并）/`k==1`。确认 barbell 测试逻辑与断言自洽。

- [ ] **Step 3: Commit**

```bash
git -C "$WT" add src/memory/notes/graph/community.rs src/memory/notes/graph/tests.rs
git -C "$WT" commit -m "feat: hand-rolled Louvain community detection + cohesion (no external crate, R3)"
```

---

### Task 3.4: `insights.rs` — 孤岛/稀疏/桥/惊喜 + 汇总测试

**Files:**
- Create: `src/memory/notes/graph/insights.rs`
- Modify: `src/memory/notes/graph/tests.rs`（聚合各 task 的测试 + insights 测试）

- [ ] **Step 1: 写实现**

```rust
//! Graph-health insights: isolated nodes, sparse communities, bridge nodes,
//! surprising cross-community/cross-type connections.

use std::collections::HashSet;

use super::community::Communities;
use super::GraphIndex;

pub const SPARSE_COHESION_MAX: f32 = 0.15;
pub const SPARSE_MIN_SIZE: usize = 3;
pub const BRIDGE_MIN_COMMUNITIES: usize = 3;
pub const SURPRISING_CAP: usize = 20;

#[derive(Debug, Clone)]
pub struct SparseCommunity { pub community_id: usize, pub size: usize, pub cohesion: f32, pub exemplar: String }
#[derive(Debug, Clone)]
pub struct SurprisingEdge { pub from: String, pub to: String, pub score: f32 }

#[derive(Debug, Clone, Default)]
pub struct GraphInsights {
    pub isolated: Vec<String>,
    pub sparse_communities: Vec<SparseCommunity>,
    pub bridges: Vec<String>,
    pub surprising: Vec<SurprisingEdge>,
}

#[must_use]
pub fn detect(g: &GraphIndex, c: &Communities) -> GraphInsights {
    // Isolated: degree <= 1.
    let isolated = (0..g.len()).filter(|&i| g.degree(i) <= 1)
        .map(|i| g.nodes[i].path.clone()).collect();

    // Sparse communities: cohesion below threshold with >= 3 members.
    let mut size = vec![0usize; c.cohesion.len()];
    for &cid in &c.of_node { size[cid] += 1; }
    let mut sparse = Vec::new();
    for cid in 0..c.cohesion.len() {
        if size[cid] >= SPARSE_MIN_SIZE && c.cohesion[cid] < SPARSE_COHESION_MAX {
            let exemplar = (0..g.len()).find(|&i| c.of_node[i] == cid)
                .map(|i| g.nodes[i].path.clone()).unwrap_or_default();
            sparse.push(SparseCommunity { community_id: cid, size: size[cid], cohesion: c.cohesion[cid], exemplar });
        }
    }

    // Bridges: neighbour communities span >= 3 distinct ids.
    let mut bridges = Vec::new();
    for i in 0..g.len() {
        let mut comms: HashSet<usize> = HashSet::new();
        comms.insert(c.of_node[i]);
        for &nb in &g.adj[i] { comms.insert(c.of_node[nb]); }
        if comms.len() >= BRIDGE_MIN_COMMUNITIES { bridges.push(g.nodes[i].path.clone()); }
    }

    // Surprising: cross-community or cross-type edges; peripheral endpoints more surprising.
    let mut surprising = Vec::new();
    for i in 0..g.len() {
        for &j in &g.adj[i] {
            if i >= j { continue; }
            let cross_comm = c.of_node[i] != c.of_node[j];
            let cross_type = g.nodes[i].category != g.nodes[j].category;
            if cross_comm || cross_type {
                let di = g.degree(i).max(1) as f32;
                let dj = g.degree(j).max(1) as f32;
                let base = if cross_comm { 1.0 } else { 0.0 } + if cross_type { 0.5 } else { 0.0 };
                let score = base * (1.0 / di + 1.0 / dj);
                surprising.push(SurprisingEdge { from: g.nodes[i].path.clone(), to: g.nodes[j].path.clone(), score });
            }
        }
    }
    surprising.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal)
        .then_with(|| (a.from.as_str(), a.to.as_str()).cmp(&(b.from.as_str(), b.to.as_str()))));
    surprising.truncate(SURPRISING_CAP);

    GraphInsights { isolated, sparse_communities: sparse, bridges, surprising }
}
```

- [ ] **Step 2: insights 测试（graph/tests.rs）**

```rust
#[test]
fn detects_isolated_and_bridge() {
    use crate::memory::notes::graph::*;
    let node = |p: &str, cat: &str| GraphNode { path: p.into(), category: cat.into(), sources: vec![] };
    // hub h links three otherwise-separate single-node clusters → bridge;
    // lone l has no edges → isolated.
    let snap = GraphSnapshot {
        nodes: vec![node("c/h","x"), node("c/a","a"), node("c/b","b"), node("c/d","d"), node("c/l","z")],
        edges: vec![("c/h".into(),"c/a".into()), ("c/h".into(),"c/b".into()), ("c/h".into(),"c/d".into())],
    };
    let g = GraphIndex::build(&snap);
    let com = community::detect(&g);
    let ins = insights::detect(&g, &com);
    assert!(ins.isolated.contains(&"c/l".to_string()));
    // h neighbours span >=3 communities (a,b,d distinct) → bridge
    assert!(ins.bridges.contains(&"c/h".to_string()));
    // cross-type edges present → surprising non-empty
    assert!(!ins.surprising.is_empty());
}
```

- [ ] **Step 3: 静态自审**

`Read` insights.rs，确认 `Communities` 字段名 `of_node`/`cohesion` 与 community.rs 一致；`size` 向量长度用 `c.cohesion.len()`（= 社区数 k）。`grep -n "mod insights\|mod community\|mod relevance" graph/mod.rs` 确认三子模块声明齐全；tests.rs 含全部测试（4 信号×2、Louvain×2、insights×1）。

- [ ] **Step 4: Commit**

```bash
git -C "$WT" add src/memory/notes/graph/insights.rs src/memory/notes/graph/tests.rs
git -C "$WT" commit -m "feat: graph-health insights (isolated/sparse/bridge/surprising)"
```

---

## Phase 4 — 物化层 + 并发重算阶段

### Task 4.1: 3 新表 DDL

**Files:**
- Read first: `src/memory/store/sqlite/schema/ddl.rs`（找 `notes_links` DDL 模板）、`schema/mod.rs`（`init_schema` 调用序）
- Modify: `src/memory/store/sqlite/schema/ddl.rs`、`schema/mod.rs`

- [ ] **Step 1: 加 DDL 常量**（ddl.rs，紧邻 `notes_links` DDL）

```rust
pub const CREATE_NOTES_SOURCES: &str = "\
CREATE TABLE IF NOT EXISTS notes_sources (
    agent_id   TEXT NOT NULL DEFAULT 'default',
    note_path  TEXT NOT NULL,
    source_ref TEXT NOT NULL,
    UNIQUE(agent_id, note_path, source_ref)
);";
pub const CREATE_NOTES_SOURCES_IDX: &str =
    "CREATE INDEX IF NOT EXISTS idx_notes_sources_ref ON notes_sources(agent_id, source_ref);";

pub const CREATE_NOTES_GRAPH_CACHE: &str = "\
CREATE TABLE IF NOT EXISTS notes_graph_cache (
    agent_id     TEXT NOT NULL DEFAULT 'default',
    node_path    TEXT NOT NULL,
    community_id INTEGER NOT NULL,
    cohesion     REAL NOT NULL DEFAULT 0,
    degree       INTEGER NOT NULL DEFAULT 0,
    updated_at   INTEGER NOT NULL,
    PRIMARY KEY (agent_id, node_path)
);";
pub const CREATE_NOTES_GRAPH_INSIGHTS: &str = "\
CREATE TABLE IF NOT EXISTS notes_graph_insights (
    agent_id   TEXT NOT NULL DEFAULT 'default',
    kind       TEXT NOT NULL,         -- isolated | sparse | bridge | surprising
    payload    TEXT NOT NULL,         -- JSON
    created_at INTEGER NOT NULL
);";
pub const CREATE_NOTES_GRAPH_INSIGHTS_IDX: &str =
    "CREATE INDEX IF NOT EXISTS idx_notes_graph_insights ON notes_graph_insights(agent_id, kind);";
```

- [ ] **Step 2: 在 `init_schema` 执行**（schema/mod.rs，紧随 notes_links 创建之后）

```rust
    conn.execute(ddl::CREATE_NOTES_SOURCES, [])
        .map_err(|e| AlephError::config(format!("create notes_sources: {e}")))?;
    conn.execute(ddl::CREATE_NOTES_SOURCES_IDX, [])
        .map_err(|e| AlephError::config(format!("idx notes_sources: {e}")))?;
    conn.execute(ddl::CREATE_NOTES_GRAPH_CACHE, [])
        .map_err(|e| AlephError::config(format!("create notes_graph_cache: {e}")))?;
    conn.execute(ddl::CREATE_NOTES_GRAPH_INSIGHTS, [])
        .map_err(|e| AlephError::config(format!("create notes_graph_insights: {e}")))?;
    conn.execute(ddl::CREATE_NOTES_GRAPH_INSIGHTS_IDX, [])
        .map_err(|e| AlephError::config(format!("idx notes_graph_insights: {e}")))?;
```
（参照 schema/mod.rs:80 既有 `notes_links` 的 `conn.execute(...).map_err(...)` 写法；`ddl::` 路径与既有引用一致。）

- [ ] **Step 3: schema 测试**（schema/tests.rs）

加断言：`init_schema` 后三表存在：
```rust
#[test]
fn graph_tables_created() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    init_schema(&conn).unwrap();
    for t in ["notes_sources","notes_graph_cache","notes_graph_insights"] {
        let n: i64 = conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
            [t], |r| r.get(0)).unwrap();
        assert_eq!(n, 1, "missing table {t}");
    }
}
```
（参照 schema/tests.rs 既有 `init_schema` 用法。）

- [ ] **Step 4: 静态自审**

```bash
grep -n "CREATE_NOTES_GRAPH_CACHE\|CREATE_NOTES_SOURCES\|notes_graph_insights" "$WT/src/memory/store/sqlite/schema/ddl.rs" "$WT/src/memory/store/sqlite/schema/mod.rs"
```
确认 DDL 常量被 `init_schema` 引用；`init_schema` 是 `IF NOT EXISTS` 幂等。

- [ ] **Step 5: Commit**

```bash
git -C "$WT" add src/memory/store/sqlite/schema/ddl.rs src/memory/store/sqlite/schema/mod.rs src/memory/store/sqlite/schema/tests.rs
git -C "$WT" commit -m "feat: notes_sources + notes_graph_cache + notes_graph_insights tables"
```

---

### Task 4.2: `index_note` 填充 `notes_sources` + `NoteStore` 图方法

**Files:**
- Read first: `src/memory/store/sqlite/notes.rs`（`index_note` impl 约 :150-230，含 notes_links 写入）、`src/memory/notes/store.rs`（`NoteStore` trait）
- Modify: `src/memory/store/sqlite/notes.rs`、`src/memory/notes/store.rs`

- [ ] **Step 1: `index_note` 写 `notes_sources`**

在 `SqliteMemoryBackend::index_note` 内（与写 `notes_links` 同一事务/路径），写完 links 后追加：先删旧 source 行再插当前（与 links 的 replace 语义一致）：
```rust
        // Rebuild notes_sources from the note's source_notes (mirrors links).
        conn.execute("DELETE FROM notes_sources WHERE agent_id=?1 AND note_path=?2",
            rusqlite::params![agent_id, path])?;
        for src in &note.source_notes {
            conn.execute(
                "INSERT OR IGNORE INTO notes_sources (agent_id, note_path, source_ref) VALUES (?1,?2,?3)",
                rusqlite::params![agent_id, path, src])?;
        }
```
（`note.source_notes` 是 `KnowledgeNote` 已有字段；`path`=`category/filename`，与 notes_links 同 key。`remove_note_index` 处同样追加 `DELETE FROM notes_sources WHERE agent_id=?1 AND note_path=?2` 以保证删除级联。）

- [ ] **Step 2: `NoteStore` trait 加图方法**（store.rs）

```rust
    /// Load the full note graph for `agent_id`: every node (path/category) plus
    /// its `source_notes`, and every resolved edge from `notes_links`.
    async fn load_graph_snapshot(&self, agent_id: &str)
        -> Result<crate::memory::notes::graph::GraphSnapshot>;

    /// Replace the materialized graph cache (community/cohesion/degree) for `agent_id`.
    async fn replace_graph_cache(&self, agent_id: &str,
        rows: &[(String, usize, f32, usize)]) -> Result<()>; // (node_path, community_id, cohesion, degree)

    /// Replace materialized insights (kind, json payload) for `agent_id`.
    async fn replace_graph_insights(&self, agent_id: &str,
        rows: &[(String, String)]) -> Result<()>; // (kind, payload_json)

    /// Read materialized insights for `agent_id`, optionally filtered by kind.
    async fn read_graph_insights(&self, agent_id: &str, kind: Option<&str>)
        -> Result<Vec<(String, String)>>;
```

- [ ] **Step 3: SQLite 实现**（notes.rs，`impl NoteStore for SqliteMemoryBackend`）

```rust
    async fn load_graph_snapshot(&self, agent_id: &str) -> Result<GraphSnapshot> {
        let conn = self.conn().await; // 按既有 conn 获取惯例（参照本文件其它方法）
        // nodes
        let mut nodes = Vec::new();
        {
            let mut stmt = conn.prepare("SELECT path, category FROM notes_index WHERE agent_id=?1")?;
            let rows = stmt.query_map([agent_id], |r| Ok((r.get::<_,String>(0)?, r.get::<_,String>(1)?)))?;
            for row in rows {
                let (path, category) = row?;
                let mut sources = Vec::new();
                let mut s2 = conn.prepare("SELECT source_ref FROM notes_sources WHERE agent_id=?1 AND note_path=?2")?;
                let srows = s2.query_map(rusqlite::params![agent_id, path], |r| r.get::<_,String>(0))?;
                for s in srows { sources.push(s?); }
                nodes.push(crate::memory::notes::graph::GraphNode { path, category, sources });
            }
        }
        // edges (resolved to_note only)
        let mut edges = Vec::new();
        {
            let mut stmt = conn.prepare("SELECT from_note, to_note FROM notes_links WHERE agent_id=?1 AND to_note IS NOT NULL AND to_note <> ''")?;
            let rows = stmt.query_map([agent_id], |r| Ok((r.get::<_,String>(0)?, r.get::<_,String>(1)?)))?;
            for row in rows { let (f,t) = row?; edges.push((f,t)); }
        }
        Ok(GraphSnapshot { nodes, edges })
    }

    async fn replace_graph_cache(&self, agent_id: &str, rows: &[(String, usize, f32, usize)]) -> Result<()> {
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn().await;
        conn.execute("DELETE FROM notes_graph_cache WHERE agent_id=?1", [agent_id])?;
        for (path, comm, coh, deg) in rows {
            conn.execute("INSERT OR REPLACE INTO notes_graph_cache (agent_id,node_path,community_id,cohesion,degree,updated_at) VALUES (?1,?2,?3,?4,?5,?6)",
                rusqlite::params![agent_id, path, *comm as i64, *coh as f64, *deg as i64, now])?;
        }
        Ok(())
    }

    async fn replace_graph_insights(&self, agent_id: &str, rows: &[(String, String)]) -> Result<()> {
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn().await;
        conn.execute("DELETE FROM notes_graph_insights WHERE agent_id=?1", [agent_id])?;
        for (kind, payload) in rows {
            conn.execute("INSERT INTO notes_graph_insights (agent_id,kind,payload,created_at) VALUES (?1,?2,?3,?4)",
                rusqlite::params![agent_id, kind, payload, now])?;
        }
        Ok(())
    }

    async fn read_graph_insights(&self, agent_id: &str, kind: Option<&str>) -> Result<Vec<(String, String)>> {
        let conn = self.conn().await;
        let mut out = Vec::new();
        match kind {
            Some(k) => {
                let mut stmt = conn.prepare("SELECT kind,payload FROM notes_graph_insights WHERE agent_id=?1 AND kind=?2")?;
                let rows = stmt.query_map(rusqlite::params![agent_id, k], |r| Ok((r.get::<_,String>(0)?, r.get::<_,String>(1)?)))?;
                for row in rows { out.push(row?); }
            }
            None => {
                let mut stmt = conn.prepare("SELECT kind,payload FROM notes_graph_insights WHERE agent_id=?1")?;
                let rows = stmt.query_map([agent_id], |r| Ok((r.get::<_,String>(0)?, r.get::<_,String>(1)?)))?;
                for row in rows { out.push(row?); }
            }
        }
        Ok(out)
    }
```
> 校验点：`self.conn().await` 是占位——**按 notes.rs 既有方法的真实 conn 获取方式**改写（该文件其它 `NoteStore` 方法如何拿连接/在哪跑 blocking，照抄其模式，可能是 `self.with_conn(|conn| {...})` 或 `tokio::task::spawn_blocking`）。`GraphSnapshot`/`GraphNode` 需 `use crate::memory::notes::graph::{GraphSnapshot, GraphNode};`。

- [ ] **Step 4: 其它 `NoteStore` impl 补全**

```bash
grep -rln "impl NoteStore for" "$WT/src/"
```
若存在 SQLite 之外的 `NoteStore` impl（如 mock/test backend），为新 4 方法加最小实现（snapshot 返回 `GraphSnapshot::default()`，replace/read 返回 `Ok(())`/`Ok(vec![])`）。否则 trait 不满足会编译失败。

- [ ] **Step 5: 静态自审**

```bash
grep -rn "fn load_graph_snapshot\|fn replace_graph_cache\|fn replace_graph_insights\|fn read_graph_insights" "$WT/src/"
```
确认 trait 声明数 == 每个 impl 的实现数。`Read` notes.rs 确认 conn 获取与同文件其它方法一致、`source_notes` 字段名正确。

- [ ] **Step 6: Commit**

```bash
git -C "$WT" add src/memory/store/sqlite/notes.rs src/memory/notes/store.rs
git -C "$WT" commit -m "feat: notes_sources population + NoteStore graph snapshot/cache/insights methods"
```

---

### Task 4.3: `GraphRecomputeStage`（spawn_blocking + 并发）

**Files:**
- Create: `src/memory/dreaming/stages/graph_recompute.rs`
- Modify: `src/memory/dreaming/stages/mod.rs`、`src/memory/dreaming/mod.rs`（Consolidate + Conserve 注册）

- [ ] **Step 1: 写阶段**

```rust
//! `GraphRecompute` stage — materialize the note knowledge graph.
//!
//! Loads the full graph snapshot, runs the 4-signal / Louvain / insights
//! algorithms inside `spawn_blocking` (CPU-bound, std-thread parallel), and
//! upserts `notes_graph_cache` + `notes_graph_insights`. Pure deterministic
//! aggregation — zero LLM call (R7/R10-safe analytics infrastructure).

use async_trait::async_trait;

use crate::error::AlephError;
use crate::memory::dreaming::DreamContext;
use crate::memory::notes::graph::{community, insights, GraphIndex, GraphSnapshot};
use crate::memory::notes::store::NoteStore;

use super::DreamStage;

pub struct GraphRecomputeStage;

#[async_trait]
impl DreamStage for GraphRecomputeStage {
    fn name(&self) -> &'static str { "graph_recompute" }

    async fn should_run(&self, ctx: &DreamContext) -> bool {
        ctx.notes.len() >= 2
    }

    async fn execute(&self, ctx: DreamContext) -> Result<DreamContext, AlephError> {
        let agent_id = ctx.agent_id.clone();
        let store = ctx.indexer.store();
        let snapshot: GraphSnapshot = store.load_graph_snapshot(&agent_id).await?;

        // CPU-bound compute off the async runtime.
        let computed = tokio::task::spawn_blocking(move || compute(&snapshot))
            .await
            .map_err(|e| AlephError::other(format!("graph recompute join: {e}")))?;

        store.replace_graph_cache(&agent_id, &computed.cache).await?;
        store.replace_graph_insights(&agent_id, &computed.insights).await?;
        tracing::info!(agent = %agent_id, nodes = computed.node_count, "graph cache recomputed");
        Ok(ctx)
    }
}

struct Computed {
    cache: Vec<(String, usize, f32, usize)>, // node_path, community, cohesion, degree
    insights: Vec<(String, String)>,         // kind, json
    node_count: usize,
}

fn compute(snap: &GraphSnapshot) -> Computed {
    let g = GraphIndex::build(snap);
    if g.is_empty() {
        return Computed { cache: vec![], insights: vec![], node_count: 0 };
    }
    let com = community::detect(&g);
    let ins = insights::detect(&g, &com);

    let cache = (0..g.len()).map(|i| {
        let cid = com.of_node[i];
        (g.nodes[i].path.clone(), cid, com.cohesion[cid], g.degree(i))
    }).collect();

    // Serialize insights to JSON rows (one row per kind).
    let insights_rows = vec![
        ("isolated".to_string(), serde_json::to_string(&ins.isolated).unwrap_or_else(|_| "[]".into())),
        ("sparse".to_string(), serde_json::to_string(
            &ins.sparse_communities.iter().map(|s| serde_json::json!({
                "community_id": s.community_id, "size": s.size, "cohesion": s.cohesion, "exemplar": s.exemplar
            })).collect::<Vec<_>>()).unwrap_or_else(|_| "[]".into())),
        ("bridge".to_string(), serde_json::to_string(&ins.bridges).unwrap_or_else(|_| "[]".into())),
        ("surprising".to_string(), serde_json::to_string(
            &ins.surprising.iter().map(|e| serde_json::json!({
                "from": e.from, "to": e.to, "score": e.score
            })).collect::<Vec<_>>()).unwrap_or_else(|_| "[]".into())),
    ];

    Computed { cache, insights: insights_rows, node_count: g.len() }
}
```
> 校验点：`ctx.indexer.store()` 返回 `&Arc<SqliteMemoryBackend>` 或 `Arc<...>`（note_manage.rs:402 用 `self.indexer.store().search_notes_fts(...)`，证明 `.store()` 可用且返回带 `NoteStore` 方法者）。若 `store()` 返回引用而 `spawn_blocking` 需 `'static`：snapshot 已先 `.await` 取出并 `move` 进闭包（不借 store），故 OK；store 调用都在 spawn_blocking 之外的 async 上下文。

- [ ] **Step 2: 注册**

`stages/mod.rs`：`pub mod graph_recompute;` + `pub use graph_recompute::GraphRecomputeStage;`

`dreaming/mod.rs`：在 **Consolidate** 分支 `IndexRefresherStage`（:179）之后、`NoteWeaveStage` 之前插 `Box::new(stages::GraphRecomputeStage),`（让 weave/decay 能消费新鲜社区数据——见 Phase 5）；在 **Conserve** 分支（:226-230）`IndexRefresherStage` 之后插同一行。`graph_recompute` 是 per-agent（不进 GLOBAL_ONLY）。

- [ ] **Step 3: 静态自审**

```bash
grep -n "GraphRecomputeStage\|graph_recompute" "$WT/src/memory/dreaming/mod.rs" "$WT/src/memory/dreaming/stages/mod.rs"
```
确认 Consolidate + Conserve 两分支各有一次 `Box::new(stages::GraphRecomputeStage)`；`serde_json` 已在依赖（note_manage 已用）；`compute` 不触 LLM/IO（纯函数，可单测）。

- [ ] **Step 4: Commit**

```bash
git -C "$WT" add src/memory/dreaming/stages/graph_recompute.rs src/memory/dreaming/stages/mod.rs src/memory/dreaming/mod.rs
git -C "$WT" commit -m "feat: GraphRecomputeStage — materialize graph cache+insights (spawn_blocking, zero LLM)"
```

---

## Phase 5 — 连线：检索 + 工具 + dream 消费

### Task 5.1: `note_manage` 新增只读 `Insights` action（R8）

**Files:**
- Modify: `src/builtin_tools/note_manage.rs`

- [ ] **Step 1: 写失败测试**（note_manage.rs `mod tests`）

```rust
#[tokio::test]
async fn insights_action_returns_ok_on_empty_graph() {
    let (_d, tool) = mk_tool();
    let args = NoteManageArgs {
        action: NoteManageAction::Insights,
        category: None, filename: None, title: None, content: None,
        facts: None, links: None, tags: None, query: None, limit: None, agent_id: None,
    };
    let r = tool.call(args).await.unwrap();
    assert!(r.success);
}
```

- [ ] **Step 2: 加 enum 变体**

`NoteManageAction`（:51）加：
```rust
    /// Read materialized graph-health insights (knowledge gaps, bridges,
    /// surprising connections). Read-only.
    Insights,
```

- [ ] **Step 3: handler + dispatch**

加 handler：
```rust
    async fn handle_insights(&self, args: &NoteManageArgs) -> Result<NoteManageResult> {
        let agent_id_owned = self.resolve_agent_id(args);
        let agent_id = agent_id_owned.as_str();
        let rows = self.indexer.store().read_graph_insights(agent_id, None).await
            .map_err(|e| AlephError::tool(format!("read insights failed: {e}")))?;
        let mut content = String::from("# Knowledge Graph Insights\n\n");
        if rows.is_empty() {
            content.push_str("_No materialized insights yet (graph recompute runs during dreaming)._\n");
        } else {
            for (kind, payload) in &rows {
                content.push_str(&format!("## {kind}\n```json\n{payload}\n```\n\n"));
            }
        }
        Ok(NoteManageResult {
            related_notes: None, success: true,
            message: format!("Graph insights ({} kinds)", rows.len()),
            note_path: None, content: Some(content), notes: None,
        })
    }
```

`call()`（:791 match）加臂：`NoteManageAction::Insights => self.handle_insights(&args).await,`

`record_lifecycle_event`（:254 的 `Query | List => return`）改为：`NoteManageAction::Query | NoteManageAction::List | NoteManageAction::Insights => return,`

更新 tool `DESCRIPTION`/`examples` 提到 `insights`（可选）。

- [ ] **Step 4: 静态自审**

```bash
grep -n "NoteManageAction::" "$WT/src/builtin_tools/note_manage.rs"
```
确认 `Insights` 在所有 match 点被处理：`call()` 6→7 臂、`record_lifecycle_event` return 臂含 Insights。无 `#[non_exhaustive]` 遗漏。`read_graph_insights` 签名与 Task 4.2 trait 一致。

- [ ] **Step 5: Commit**

```bash
git -C "$WT" add src/builtin_tools/note_manage.rs
git -C "$WT" commit -m "feat: note_manage insights action — expose graph health to LLM (R8)"
```

---

### Task 5.2: 4 信号注入 `note_retrieval` 图扩展

**Files:**
- Read first: `src/memory/note_retrieval/mod.rs`（图扩展/邻居扩展相，约 :111-162）、`src/memory/note_retrieval/scoring.rs`
- Modify: `src/memory/note_retrieval/mod.rs`

**背景：** 检索末端保留 recency/reinforcement/MMR（互补）；本 task 在**种子扩展**处叠加 4 信号——优先读物化 `notes_graph_cache` 同社区节点作为扩展候选；若 cache 为空回退现有邻居扩展。保守、可回退（P7）。

- [ ] **Step 1: 加 store 读取方法**（store.rs + notes.rs）

```rust
    /// Node paths sharing `node_path`'s community (materialized cache).
    async fn community_peers(&self, agent_id: &str, node_path: &str, limit: usize)
        -> Result<Vec<String>>;
```
SQLite 实现：
```rust
    async fn community_peers(&self, agent_id: &str, node_path: &str, limit: usize) -> Result<Vec<String>> {
        let conn = self.conn().await; // 同 4.2 校验点：按真实 conn 模式
        let cid: Option<i64> = conn.query_row(
            "SELECT community_id FROM notes_graph_cache WHERE agent_id=?1 AND node_path=?2",
            rusqlite::params![agent_id, node_path], |r| r.get(0)).optional()?;
        let Some(cid) = cid else { return Ok(vec![]); };
        let mut stmt = conn.prepare(
            "SELECT node_path FROM notes_graph_cache WHERE agent_id=?1 AND community_id=?2 AND node_path<>?3 LIMIT ?4")?;
        let rows = stmt.query_map(rusqlite::params![agent_id, cid, node_path, limit as i64], |r| r.get::<_,String>(0))?;
        let mut out = Vec::new();
        for row in rows { out.push(row?); }
        Ok(out)
    }
```
（`.optional()` 需 `use rusqlite::OptionalExtension;`——确认本文件已 import 或加。）其它 `NoteStore` impl 补 `Ok(vec![])`。

- [ ] **Step 2: 在图扩展处叠加**

`Read` `note_retrieval/mod.rs` 找到现有 seed→邻居扩展处（用 `get_neighbors`/`get_outgoing_links` 之类）。在收集扩展候选时，对每个 top 搜索命中 seed 追加 `community_peers(agent_id, seed_path, N)` 的结果（去重并入候选池，喂给后续打分）。回退：cache 空→`community_peers` 返回空→行为与改动前一致。

```rust
    // Graph-cache community expansion (4-signal materialized): peers in the same
    // Louvain community as each seed are strong relatedness candidates. Empty
    // cache (pre-first-dream) degrades to the existing neighbour expansion.
    for seed in &seed_paths {
        if let Ok(peers) = store.community_peers(agent_id, seed, 8).await {
            for p in peers { candidate_paths.insert(p); }
        }
    }
```
（`seed_paths`/`candidate_paths`/`store`/`agent_id` 按 mod.rs 实际变量名对接。）

- [ ] **Step 3: 测试**

加单测：构造一个有 cache 行的 backend，`community_peers` 返回同社区其它节点；cache 空时返回空 vec。

- [ ] **Step 4: 静态自审**

```bash
grep -rn "fn community_peers" "$WT/src/"     # trait 声明 == impl 数
grep -n "community_peers\|candidate" "$WT/src/memory/note_retrieval/mod.rs"
```
确认叠加点不破坏既有打分；回退路径成立（空 cache 不改变结果集顺序，只是不新增候选）。

- [ ] **Step 5: Commit**

```bash
git -C "$WT" add src/memory/notes/store.rs src/memory/store/sqlite/notes.rs src/memory/note_retrieval/mod.rs
git -C "$WT" commit -m "feat: 4-signal community expansion in note retrieval (cache-backed, graceful fallback)"
```

---

### Task 5.3: `note_weave` 消费物化孤岛（统一健康来源 · 熵减）

**Files:**
- Read first: `src/memory/dreaming/stages/note_weave.rs`（孤岛检测 SQL 约 :54-87）
- Modify: `src/memory/dreaming/stages/note_weave.rs`

**背景：** `note_weave` 现用自有 SQL 找孤岛；`GraphRecomputeStage` 在同一 Consolidate 周期**更早**运行（Task 4.3 已置于 weave 之前），已物化 `isolated` 洞察。改 weave 优先读 `read_graph_insights(agent, Some("isolated"))`，回退原 SQL（保持鲁棒）。

- [ ] **Step 1: 改孤岛来源**

在 weave 取孤岛集处，先尝试物化：
```rust
    // Prefer the materialized isolated set (GraphRecomputeStage ran earlier this
    // cycle); fall back to the local SQL scan when the cache is cold.
    let isolated: Vec<String> = match ctx.indexer.store()
        .read_graph_insights(&ctx.agent_id, Some("isolated")).await
    {
        Ok(rows) if !rows.is_empty() => {
            serde_json::from_str::<Vec<String>>(&rows[0].1).unwrap_or_default()
        }
        _ => { /* existing SQL orphan scan, unchanged */ existing_orphan_scan },
    };
```
（`existing_orphan_scan` 代表原有逻辑产出的 `Vec<String>`；把原逻辑包成该回退分支。其余 weave 逻辑——LLM 关键词提取 + `pair_by_overlap` 配对——不变。）

- [ ] **Step 2: 静态自审**

`Read` note_weave.rs 确认：物化命中时跳过原 SQL；回退分支保留原行为；`read_graph_insights` 返回 `Vec<(kind,payload)>`，payload 是 `isolated` 的 JSON 数组（与 Task 4.3 序列化一致）。

- [ ] **Step 3: Commit**

```bash
git -C "$WT" add src/memory/dreaming/stages/note_weave.rs
git -C "$WT" commit -m "refactor: note_weave consumes materialized isolated insight (unified health source)"
```

---

## Self-Review（落计划后对照 spec）

**Spec 覆盖核对：**
- G1 vault frontmatter + `.obsidian` → Task 1.1/1.2/1.3 ✅
- G2 overview/purpose + CorpusNarrativeStage → Task 2.1/2.2 ✅
- G3 graph 模块（relevance/community/insights）→ Task 3.1/3.2/3.3/3.4 ✅
- G4 物化表 + 并发重算 → Task 4.1/4.2/4.3 ✅
- G5 连线（4 信号检索 / insights tool / compact_for_prompt）→ Task 5.2/5.1/0.2 ✅
- G6 熵减（删 frontmatter_template / 刷新 NOTES.md）→ Task 0.1/0.3 ✅
- insights 连 note_weave → Task 5.3 ✅（note_lint 桥/稀疏报告标为可选增强，未排专门 task——若需要在 5.3 后追加）

**Placeholder 扫描：** 无 TBD/TODO；占位仅有明确标注的"按真实 conn 模式对接"校验点（Task 4.2/5.2 的 `self.conn().await`），已说明须照 notes.rs 既有模式改写——属实现细节非占位。

**类型一致性：** `Communities{of_node,cohesion}`、`GraphInsights{isolated,sparse_communities,bridges,surprising}`、`SignalWeights{direct_link,source_overlap,adamic_adar,type_affinity}`、`GraphSnapshot{nodes,edges}`、`GraphNode{path,category,sources}` 跨 Task 3.x/4.3 一致；`NoteStore` 新方法 `load_graph_snapshot/replace_graph_cache/replace_graph_insights/read_graph_insights/community_peers` 在 store.rs 声明与 notes.rs 实现 + 5.1/5.2/5.3 调用签名一致。

**最大风险（已标注）：** Task 3.3 Louvain 无 cargo 验证——barbell 测试是收敛守卫，CI 恢复后首先跑。Task 4.2/5.2 的 conn 获取须照 notes.rs 既有模式（不可照抄占位 `self.conn().await`）。
