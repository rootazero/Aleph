# Panel 记忆 Tab 深度重构 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 Panel 记忆 tab 从「一个 RPC 两种形状 + 静默吞错 + 表格」重构成「每 RPC 单一形状 + `Loadable` 三态 + 卡片流」，并接上三条断线（`filter_notes` / `memory.trace` / 被丢弃的 DB 字段）、删掉两个死端点。

**Architecture:** 服务端先收敛契约（`graph.search` = 笔记 FTS 唯一入口，`memory.search` = 原始对话唯一入口，count 与 list 共用 WHERE 构造器），Panel 再在此之上重建。Panel 侧把 695 行 `views/memory/mod.rs` 拆成 10 个 ≤400 行文件，纯数据层留在 `data.rs`（host target 可测），UI 层只做渲染。全程 R4 纯 I/O，零 `src/harness/` 触碰。

**Tech Stack:** Rust 1.96 · Leptos 0.8 (CSR/WASM) · leptos_i18n 0.6 · rusqlite 0.37 · Tailwind v4 CSS vars

**Spec:** `docs/superpowers/specs/2026-07-26-panel-memory-tab-design.md`

## Global Constraints

- **分支**：`worktree-panel-memory-tab-refactor`（基线 `646173c7f`）。严禁触碰 `main`。
- **红线**：零 `src/harness/` 触碰（R10）。Gateway/Panel/CLI 层禁业务逻辑（R4 纯 I/O）。笔记生命周期归 dream daemon 与 `note_manage`，Panel 不得新增「创建笔记」入口（R7/R8）。
- **不动的文件**：`src/memory/notes/{ingest,dreaming,links}/**`（记忆算法）· `interfaces/webchat/src/platform/wide/views/canvas/**`（`search_query`/`search_nonce` 契约不变）· `interfaces/webchat/src/platform/phone/memory/**`（复用数据层，`SearchHits` 不进 phone chips）。
- **格式化**：只用 `rustfmt --edition 2021 <单个文件>`。**严禁无作用域 `cargo fmt`** —— baseline 非 fmt-clean，会 churn ~70 个无关文件。注意 `rustfmt` 会递归格式化 child mod，若连带改了非目标文件必须 `git checkout` 回滚它们。
- **cargo PATH**：本机 `cargo` 不在 PATH 上。每个 shell 命令前置
  `export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"`。
- **`--lib` 不编译 bin**：改了 `src/bin/aleph-server/**` 必须跑 `cargo check --bin aleph-server`，`cargo check -p alephcore --lib` 看不到它。
- **退出码陷阱**：`cargo test ... | tail` 会用 `tail` 的退出码掩盖失败。要判定成败就不接管道，或用 `set -o pipefail`。
- **i18n 对称由编译器强制**：`leptos_i18n` 在编译期校验 `locales/en.json` 与 `zh.json` 的键集合，缺键 = 编译失败。新增键必须两个文件同时加。
- **i18n 不用插值**：新增键一律纯静态文本，计数在 Rust 侧拼（沿用现有 `{move || n.to_string()} " " {t!(i18n, memory.batch_selected)}` 写法）。
- **无上限静默**：任何截断 / 封顶（导出 50 条、证据链 20 条、笔记窗口 1000 条）都必须在 UI 上说出来。
- **不弹阻塞模态**：破坏性操作用现有 `ConfirmButton` 二段确认，禁用 `window.confirm`（R5）。
- **基线回归数**：`cargo test -p aleph-panel --lib memory::` 当前 **17 passed**（本轮只增不减）。

## File Structure

**服务端（`src/`）**

| 文件 | 责任 | 变更 |
|---|---|---|
| `src/memory/store/sqlite/mod.rs` | raw_memories 投影：WHERE 单一源 | 新增 `raw_where` / `escape_like`；`get_raw_memories_dashboard` 与 `count_raw_memories` 改签名 |
| `src/memory/store/sqlite/raw_memories.rs` | 该模块测试 | 跟随签名 + 新增一致性测 |
| `src/gateway/handlers/memory.rs` | memory.* RPC | `handle_search` 去笔记分支；`handle_stats` agent-scoped；`handle_list_facts` 回 total + 字段直通 |
| `src/gateway/handlers/graph/search.rs` | 笔记 FTS 唯一入口 | DTO 字段直通 |
| `src/gateway/handlers/graph_types.rs` | graph DTO | `SearchResultDto` 扩字段；删 neighbors DTO |
| `src/gateway/handlers/graph/{mod,neighbors}.rs` | — | 删 `neighbors.rs` + re-export |
| `src/bin/aleph-server/.../builder/handlers/agents.rs` | RPC 注册 | 删 `graph.neighbors` 注册块 |
| `interfaces/cli/src/commands/memory_cmd.rs` | CLI | 对齐真实响应体 |

**Panel（`interfaces/webchat/src/`）**

| 文件 | 责任 | 行数目标 |
|---|---|---|
| `api/memory.rs` | memory.* DTO + 调用 | ~260 |
| `canvas_engine/adapter.rs` | graph DTO 镜像 | +5 行 |
| `platform/wide/views/memory/mod.rs` | 编排：Effects 装配 + 布局 + 事件路由 | ~200 |
| `.../memory/data.rs` | 纯数据层（host 可测）：facet / `Loadable` / 过滤 / 分页 / 导出 | ~400 |
| `.../memory/loader.rs` | 三条取数 → `Loadable` | ~150 |
| `.../memory/facets.rs` | 层 chips（含条件 SearchHits） | ~120 |
| `.../memory/cards.rs` | NoteCard / RawCard / CardList 三态 | ~280 |
| `.../memory/batch_bar.rs` | 批量操作条 | ~120 |
| `.../memory/pager.rs` | Pager + page-size | ~120 |
| `.../memory/toast.rs` | 模块私有 toast 栈 | ~70 |
| `.../memory/drawer.rs` | 详情抽屉 | ~350 |
| `.../memory/provenance.rs` | `memory.trace` 证据链区 | ~150 |
| `locales/{en,zh}.json` | i18n | +30 键 / −10 键 |

**依赖顺序**：Phase A（服务端契约，Task 1–7）→ Phase B（Panel 契约与纯函数，Task 8–11）→ Phase C（Panel UI，Task 12–18）→ Phase D（收尾，Task 19–20）。

---

## Phase A — 服务端：一个 RPC 一种形状

### Task 1: raw_memories 的 WHERE 单一源

**Files:**
- Modify: `src/memory/store/sqlite/mod.rs:150-240`
- Test: `src/memory/store/sqlite/mod.rs`（新增 `#[cfg(test)] mod raw_where_tests`）+ `src/memory/store/sqlite/raw_memories.rs:861`

**Interfaces:**
- Produces:
  - `fn raw_where(agent_id: Option<&str>, query: Option<&str>) -> (String, Vec<rusqlite::types::Value>)`（模块私有）
  - `fn escape_like(q: &str) -> String`（模块私有）
  - `pub fn get_raw_memories_dashboard(&self, agent_id: Option<&str>, query: Option<&str>, limit: usize, offset: usize) -> Result<Vec<RawMemory>, AlephError>`
  - `pub fn count_raw_memories(&self, agent_id: Option<&str>, query: Option<&str>) -> Result<i64, AlephError>`

- [ ] **Step 1: 写失败测试**

在 `src/memory/store/sqlite/mod.rs` 文件末尾追加：

```rust
#[cfg(test)]
mod raw_where_tests {
    use super::{escape_like, raw_where};
    use rusqlite::types::Value;

    #[test]
    fn baseline_always_excludes_telemetry() {
        let (sql, binds) = raw_where(None, None);
        assert_eq!(sql, " WHERE source != 'tool_invocation'");
        assert!(binds.is_empty());
    }

    #[test]
    fn agent_scope_adds_one_bind() {
        let (sql, binds) = raw_where(Some("main"), None);
        assert_eq!(
            sql,
            " WHERE source != 'tool_invocation' AND agent_id = ?"
        );
        assert_eq!(binds, vec![Value::Text("main".to_string())]);
    }

    #[test]
    fn query_adds_escaped_like_bind() {
        let (sql, binds) = raw_where(None, Some("deploy"));
        assert_eq!(
            sql,
            " WHERE source != 'tool_invocation' AND content LIKE ? ESCAPE '\\'"
        );
        assert_eq!(binds, vec![Value::Text("%deploy%".to_string())]);
    }

    #[test]
    fn agent_and_query_bind_in_positional_order() {
        let (sql, binds) = raw_where(Some("main"), Some("smoke"));
        assert_eq!(
            sql,
            " WHERE source != 'tool_invocation' AND agent_id = ? AND content LIKE ? ESCAPE '\\'"
        );
        assert_eq!(
            binds,
            vec![
                Value::Text("main".to_string()),
                Value::Text("%smoke%".to_string()),
            ]
        );
    }

    #[test]
    fn blank_query_is_not_a_filter() {
        // A whitespace-only search box must browse, not match every row that
        // happens to contain a space.
        assert_eq!(raw_where(None, Some("   ")).1.len(), 0);
        assert_eq!(raw_where(None, Some("")).1.len(), 0);
    }

    #[test]
    fn like_metacharacters_match_literally() {
        // Without escaping, a user searching for "100%" would match everything.
        assert_eq!(escape_like("100%"), r"100\%");
        assert_eq!(escape_like("a_b"), r"a\_b");
        assert_eq!(escape_like(r"c\d"), r"c\\d");
        assert_eq!(escape_like("plain"), "plain");
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
cargo test -p alephcore --lib raw_where_tests
```
Expected: FAIL —— `cannot find function raw_where in this scope`。

- [ ] **Step 3: 实现两个纯函数**

在 `src/memory/store/sqlite/mod.rs` 的 `impl` 块**之外**（文件顶层，紧邻 `get_raw_memories_dashboard` 所在 impl 之前）加：

```rust
/// Build the `WHERE` clause and its positional bind values for the
/// user-facing `raw_memories` projection.
///
/// `count_raw_memories` and `get_raw_memories_dashboard` both go through
/// here, so a filtered count can never disagree with the filtered list. That
/// disagreement was the phantom-page bug: the console paired a *global* count
/// with an *agent-scoped* list, so "next page" stayed enabled forever and
/// eventually landed on an empty page.
///
/// `tool_invocation` rows are per-call telemetry consumed by the Dream cycle
/// by source, never user-facing memories, so they are excluded unconditionally.
fn raw_where(agent_id: Option<&str>, query: Option<&str>) -> (String, Vec<rusqlite::types::Value>) {
    use rusqlite::types::Value;

    let mut sql = String::from(" WHERE source != 'tool_invocation'");
    let mut binds: Vec<Value> = Vec::new();

    if let Some(agent) = agent_id {
        sql.push_str(" AND agent_id = ?");
        binds.push(Value::Text(agent.to_string()));
    }
    // A blank search box browses; it does not match every row containing a space.
    if let Some(q) = query.map(str::trim).filter(|q| !q.is_empty()) {
        sql.push_str(r" AND content LIKE ? ESCAPE '\'");
        binds.push(Value::Text(format!("%{}%", escape_like(q))));
    }

    (sql, binds)
}

/// Escape SQL `LIKE` metacharacters so a query containing `%`, `_` or `\`
/// matches them literally. Without this, searching for "100%" matches every
/// row. Pairs with the `ESCAPE '\'` clause emitted by [`raw_where`].
fn escape_like(q: &str) -> String {
    let mut out = String::with_capacity(q.len());
    for ch in q.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}
```

- [ ] **Step 4: 跑测试确认通过**

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
cargo test -p alephcore --lib raw_where_tests
```
Expected: PASS（6 tests）。

- [ ] **Step 5: 改写两个查询方法**

把 `src/memory/store/sqlite/mod.rs:158-222` 的 `get_raw_memories_dashboard` 整体替换为：

```rust
    /// Get raw memory entries (session summaries / conversation records).
    ///
    /// `query` is an optional case-sensitive substring filter over `content`
    /// (`LIKE '%q%'`). `raw_memories` has no fts5 shadow table — this is a
    /// browse-UI filter, and building real FTS here would need a DDL migration
    /// plus sync triggers (deliberately out of scope).
    ///
    /// `offset` skips the first N rows of the `created_at DESC` ordering,
    /// enabling stable server-side pagination for the dashboard.
    ///
    /// The `WHERE` clause comes from [`raw_where`], shared with
    /// [`Self::count_raw_memories`] so list and count cannot drift.
    pub fn get_raw_memories_dashboard(
        &self,
        agent_id: Option<&str>,
        query: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<crate::memory::store::raw_memory::RawMemory>, AlephError> {
        use crate::memory::store::raw_memory::{RawMemory, RawMemorySource};
        use rusqlite::types::Value;

        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        let (where_sql, mut binds) = raw_where(agent_id, query);
        let sql = format!(
            "SELECT id, content, source, source_detail, agent_id, session_id, path, layer, \
             attachment_text, is_processed, created_at \
             FROM raw_memories{where_sql} \
             ORDER BY created_at DESC LIMIT ? OFFSET ?"
        );
        binds.push(Value::Integer(limit as i64));
        binds.push(Value::Integer(offset as i64));

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| AlephError::config(format!("get_raw_memories_dashboard prepare: {e}")))?;

        let row_mapper = |row: &rusqlite::Row| -> rusqlite::Result<RawMemory> {
            let source_str: String = row.get("source")?;
            let source_detail: Option<String> = row.get("source_detail")?;
            let is_processed_int: i64 = row.get("is_processed")?;
            Ok(RawMemory {
                id: row.get("id")?,
                content: row.get("content")?,
                source: RawMemorySource::from_persisted(&source_str, source_detail.as_deref()),
                agent_id: row.get("agent_id")?,
                session_id: row.get("session_id")?,
                path: row.get("path")?,
                layer: row.get("layer")?,
                attachment_text: row.get("attachment_text")?,
                is_processed: is_processed_int != 0,
                created_at: row.get("created_at")?,
            })
        };

        let rows = stmt
            .query_map(rusqlite::params_from_iter(binds), row_mapper)
            .map_err(|e| AlephError::config(format!("get_raw_memories_dashboard failed: {e}")))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| {
                AlephError::config(format!("get_raw_memories_dashboard row failed: {e}"))
            })?);
        }
        Ok(results)
    }
```

把 `count_raw_memories`（原 224-240 行）替换为：

```rust
    /// Count user-facing raw memory entries under the same filter the
    /// dashboard list uses.
    ///
    /// Shares [`raw_where`] with [`Self::get_raw_memories_dashboard`]: pass the
    /// same `(agent_id, query)` and the count is guaranteed to describe exactly
    /// that list. Excludes `tool_invocation` telemetry.
    pub fn count_raw_memories(
        &self,
        agent_id: Option<&str>,
        query: Option<&str>,
    ) -> Result<i64, AlephError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        let (where_sql, binds) = raw_where(agent_id, query);
        let sql = format!("SELECT COUNT(*) FROM raw_memories{where_sql}");

        conn.query_row(&sql, rusqlite::params_from_iter(binds), |row| row.get(0))
            .map_err(|e| AlephError::config(format!("count_raw_memories failed: {e}")))
    }
```

- [ ] **Step 6: 修调用点与旧测试，并加一致性回归测**

`src/gateway/handlers/memory.rs:159-163` 的调用改为（本 task 只让它编译，语义在 Task 2 改）：

```rust
    match db.get_raw_memories_dashboard(
        Some(agent_id),
        None,
        params.limit as usize,
        params.offset as usize,
    ) {
```

`src/gateway/handlers/memory.rs:365` 改为：

```rust
    let raw_count = db.count_raw_memories(None, None).unwrap_or(0);
```

`src/memory/store/sqlite/raw_memories.rs` 中原有断言（约 851-861 行）改为：

```rust
        // Dashboard (both agent-scoped and global) hides telemetry.
        let scoped = backend
            .get_raw_memories_dashboard(Some("noise"), None, 100, 0)
            .unwrap();
        assert_eq!(scoped.len(), 1);
        assert_eq!(scoped[0].content, "real conversation");

        let global = backend
            .get_raw_memories_dashboard(None, None, 100, 0)
            .unwrap();
        assert_eq!(global.len(), 1);

        // The user-facing count matches the dashboard (1, not 4).
        assert_eq!(backend.count_raw_memories(None, None).unwrap(), 1);

        // Count and list must agree under EVERY filter combination — a global
        // count paired with a scoped list is what produced phantom pages.
        for (agent, query) in [
            (None, None),
            (Some("noise"), None),
            (None, Some("real")),
            (Some("noise"), Some("real")),
            (Some("noise"), Some("nonexistent-needle")),
        ] {
            let listed = backend
                .get_raw_memories_dashboard(agent, query, 1000, 0)
                .unwrap()
                .len() as i64;
            let counted = backend.count_raw_memories(agent, query).unwrap();
            assert_eq!(
                listed, counted,
                "count/list disagree for agent={agent:?} query={query:?}"
            );
        }
```

- [ ] **Step 7: 全量验证**

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
cargo test -p alephcore --lib raw_where_tests
cargo test -p alephcore --lib raw_memories
cargo check --bin aleph-server
```
Expected: 全 PASS，`aleph-server` 编译干净。

- [ ] **Step 8: 格式化并提交**

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
rustfmt --edition 2021 src/memory/store/sqlite/mod.rs
git status --short   # 确认没有意外文件被 rustfmt 递归 churn；有则 git checkout 回滚
git add src/memory/store/sqlite/mod.rs src/memory/store/sqlite/raw_memories.rs src/gateway/handlers/memory.rs
git commit -m "memory: share one WHERE builder between raw count and list

A global count paired with an agent-scoped list gave the memory console
phantom pages: 'next' stayed enabled forever and eventually landed on an
empty page. raw_where() is now the single source both callers go through,
and the query filter it grows escapes LIKE metacharacters so searching
for '100%' does not match every row."
```

---

### Task 2: memory.search 收敛为原始对话唯一入口

**Files:**
- Modify: `src/gateway/handlers/memory.rs:14-24`（`MemoryEntry` 加 `session_id`）、`61-185`（`SearchParams` + `handle_search`）
- Test: `src/gateway/handlers/memory.rs`（新增 `#[cfg(test)] mod search_tests`）

**Interfaces:**
- Consumes: Task 1 的 `get_raw_memories_dashboard(agent, query, limit, offset)`
- Produces: `memory.search` 响应恒为 `{"memories": [MemoryEntry]}`，`MemoryEntry` 字段 = `id` / `agent_id` / `window_title` / `user_input` / `ai_output` / `session_id` / `timestamp`（`similarity_score` 删除）

- [ ] **Step 1: 写失败测试**

在 `src/gateway/handlers/memory.rs` 末尾追加：

```rust
#[cfg(test)]
mod search_tests {
    use super::*;
    use crate::memory::notes::KnowledgeNote;
    use crate::memory::notes::store::NoteStore;
    use crate::memory::store::raw_memory::{RawMemory, RawMemorySource, RawMemoryStore};
    use crate::memory::store::sqlite::SqliteMemoryBackend;
    use crate::sync_primitives::Arc;

    fn db() -> MemoryBackend {
        let path = std::env::temp_dir().join(format!("mem_search_test_{}", uuid::Uuid::new_v4()));
        Arc::new(SqliteMemoryBackend::new(&path).unwrap())
    }

    fn req(params: serde_json::Value) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "memory.search".to_string(),
            params: Some(params),
            id: Some(serde_json::json!(1)),
        }
    }

    async fn seed(db: &MemoryBackend) {
        // One raw conversation row…
        let raw = RawMemory {
            id: "raw-1".to_string(),
            content: "we should run smoke tests before deploy".to_string(),
            source: RawMemorySource::Conversation,
            agent_id: "main".to_string(),
            session_id: Some("s-77".to_string()),
            path: None,
            layer: None,
            attachment_text: None,
            is_processed: false,
            created_at: 1_700_000_000,
        };
        db.insert_raw_memory(&raw).await.unwrap();

        // …and one note whose body ALSO contains the word "smoke", so an
        // accidental note-FTS branch would be visible in the assertion.
        let note = KnowledgeNote {
            title: "deploy-notes".to_string(),
            category: "facts".to_string(),
            facts: vec!["smoke".to_string()],
            created_at: 1_700_000_000,
            updated_at: 1_700_000_500,
            content_hash: "h1".to_string(),
            ..Default::default()
        };
        db.index_note(&note, "main", "facts").await.unwrap();
    }

    /// The core regression: a query must NEVER return note rows. The old
    /// handler ran a note FTS search here and returned note paths as if they
    /// were conversation records, so the console's "Raw" tab showed note
    /// filenames and its delete button targeted a table that does not hold them.
    #[tokio::test]
    async fn query_returns_raw_rows_never_notes() {
        let db = db();
        seed(&db).await;

        let resp = handle_search(req(serde_json::json!({
            "agent_id": "main",
            "query": "smoke",
            "limit": 20
        })), db)
        .await;

        let memories = resp.result.expect("success")["memories"]
            .as_array()
            .expect("memories array")
            .clone();
        assert_eq!(memories.len(), 1, "only the raw row matches, not the note");
        assert_eq!(memories[0]["id"], "raw-1");
        assert_eq!(memories[0]["session_id"], "s-77");
        assert!(
            memories[0]["user_input"]
                .as_str()
                .unwrap()
                .contains("smoke tests"),
            "raw content must be returned verbatim, not a note filename"
        );
    }

    #[tokio::test]
    async fn empty_query_browses_all_raw_rows() {
        let db = db();
        seed(&db).await;

        let resp = handle_search(req(serde_json::json!({
            "agent_id": "main",
            "query": "",
            "limit": 20
        })), db)
        .await;

        assert_eq!(
            resp.result.expect("success")["memories"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn query_with_no_match_returns_empty_not_error() {
        let db = db();
        seed(&db).await;

        let resp = handle_search(req(serde_json::json!({
            "agent_id": "main",
            "query": "zzz-nothing-matches",
            "limit": 20
        })), db)
        .await;

        assert!(resp.error.is_none());
        assert!(resp.result.expect("success")["memories"]
            .as_array()
            .unwrap()
            .is_empty());
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
cargo test -p alephcore --lib search_tests
```
Expected: `query_returns_raw_rows_never_notes` FAIL —— 旧 handler 走笔记 FTS 返回 `deploy-notes`。若 `JsonRpcRequest` 字面构造字段不符，先 `grep -n "pub struct JsonRpcRequest" -A 10 src/gateway/protocol.rs` 对齐（其余测试代码不变）。

- [ ] **Step 3: 加 session_id 到 MemoryEntry、删 similarity_score**

替换 `src/gateway/handlers/memory.rs:13-24`：

```rust
/// Memory entry for JSON serialization.
///
/// One raw conversation record. `user_input` / `ai_output` stay separate so the
/// panel can style the two halves independently — joining them into one string
/// server-side threw that away.
#[derive(Debug, Clone, Serialize)]
pub struct MemoryEntry {
    pub id: String,
    pub agent_id: String,
    pub window_title: String,
    pub user_input: String,
    pub ai_output: String,
    /// Session the row was recorded in, when known. Already selected by the
    /// dashboard query — previously dropped on the floor here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub timestamp: i64,
}
```

- [ ] **Step 4: 改写 handle_search**

替换 `src/gateway/handlers/memory.rs:97-185`（`SearchParams` 的 `window_title` 字段与 `Default` impl 保持不动）：

```rust
/// Search raw memory (Layer 1 conversation records).
///
/// `query` filters `content` by substring; empty `query` browses. This handler
/// is the **only** raw-memory entry point, and it returns **only** raw rows.
///
/// It used to run a note full-text search when `query` was non-empty —
/// duplicating `graph.search`, which calls the same `search_notes_fts`. The
/// panel wired that branch into its raw-memory table, so searching showed note
/// filenames dressed as conversation records and the row delete button targeted
/// `delete_raw_memory` with a note path (always `Ok(false)` → error → swallowed).
/// Note search belongs to `graph.search`; keep it there.
pub async fn handle_search(request: JsonRpcRequest, db: MemoryBackend) -> JsonRpcResponse {
    let params: SearchParams = request
        .params
        .as_ref()
        .and_then(|p| serde_json::from_value(p.clone()).ok())
        .unwrap_or_default();

    // Scope to an agent namespace even when agent_id is omitted: passing None
    // drops the SQL `WHERE agent_id` clause and returns every agent's raw
    // memories, violating workspace isolation.
    let agent_id = params
        .agent_id
        .as_deref()
        .unwrap_or(crate::routing::DEFAULT_AGENT_ID);
    let query = params.query.as_deref().map(str::trim).filter(|q| !q.is_empty());

    match db.get_raw_memories_dashboard(
        Some(agent_id),
        query,
        params.limit as usize,
        params.offset as usize,
    ) {
        Ok(memories) => {
            let entries: Vec<MemoryEntry> = memories
                .into_iter()
                .map(|m| MemoryEntry {
                    id: m.id,
                    agent_id: m.agent_id,
                    window_title: String::new(),
                    user_input: m.content,
                    ai_output: String::new(),
                    session_id: m.session_id,
                    timestamp: m.created_at,
                })
                .collect();
            JsonRpcResponse::success(request.id, json!({ "memories": entries }))
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Search raw memories failed: {e}"),
        ),
    }
}
```

同时删掉 `handle_search` 顶部原有的 `use crate::memory::notes::store::NoteStore;`（本函数不再用它）。

- [ ] **Step 5: 跑测试确认通过**

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
cargo test -p alephcore --lib search_tests
cargo check --bin aleph-server
```
Expected: 3 tests PASS，bin 编译干净。

- [ ] **Step 6: 格式化并提交**

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
rustfmt --edition 2021 src/gateway/handlers/memory.rs
git status --short
git add src/gateway/handlers/memory.rs
git commit -m "memory: memory.search returns raw rows only, never notes

It had grown a second personality: with a non-empty query it ran the same
search_notes_fts that graph.search already owns, and returned note paths
shaped like conversation records. The panel rendered those in its raw
table, so 'search' showed note filenames and each row's delete button
called delete_raw_memory with a note path -- Ok(false) -> error -> dropped
by an is_ok() check, leaving an undeletable ghost row with no feedback.

Note search stays in graph.search. session_id now rides along (it was
already selected by the dashboard query) and user_input/ai_output stay
unjoined so the panel can style the halves separately."
```

---

### Task 3: memory.stats 按 agent 计数并申报 scope

**Files:**
- Modify: `src/gateway/handlers/memory.rs:362-387`（`handle_stats`）+ 新增 `StatsParams`
- Test: `src/gateway/handlers/memory.rs`（新增 `mod stats_tests`）

**Interfaces:**
- Consumes: Task 1 的 `count_raw_memories(agent, query)`；既有 `NoteStore::count_notes(agent)` / `count_all_notes()`
- Produces: `memory.stats` 参数 `{ agent_id?: string }`；响应
  `{ totalMemories: i64, totalFacts: i64, validFacts: i64, totalGraphNodes: i64|null, totalGraphEdges: i64|null, scope: "agent"|"global" }`

- [ ] **Step 1: 写失败测试**

在 `src/gateway/handlers/memory.rs` 末尾追加：

```rust
#[cfg(test)]
mod stats_tests {
    use super::*;
    use crate::memory::notes::store::NoteStore;
    use crate::memory::notes::KnowledgeNote;
    use crate::memory::store::raw_memory::{RawMemory, RawMemorySource, RawMemoryStore};
    use crate::memory::store::sqlite::SqliteMemoryBackend;
    use crate::sync_primitives::Arc;

    fn db() -> MemoryBackend {
        let path = std::env::temp_dir().join(format!("mem_stats_test_{}", uuid::Uuid::new_v4()));
        Arc::new(SqliteMemoryBackend::new(&path).unwrap())
    }

    fn req(params: Option<serde_json::Value>) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "memory.stats".to_string(),
            params,
            id: Some(serde_json::json!(1)),
        }
    }

    fn note(title: &str) -> KnowledgeNote {
        KnowledgeNote {
            title: title.to_string(),
            category: "facts".to_string(),
            facts: vec!["f".to_string()],
            created_at: 1_700_000_000,
            updated_at: 1_700_000_000,
            content_hash: format!("h-{title}"),
            ..Default::default()
        }
    }

    fn raw(id: &str, agent: &str) -> RawMemory {
        RawMemory {
            id: id.to_string(),
            content: "c".to_string(),
            source: RawMemorySource::Conversation,
            agent_id: agent.to_string(),
            session_id: None,
            path: None,
            layer: None,
            attachment_text: None,
            is_processed: false,
            created_at: 1_700_000_000,
        }
    }

    /// Two agents, asymmetric data. Scoped stats must describe ONE of them.
    async fn seed(db: &MemoryBackend) {
        db.index_note(&note("a1"), "alpha", "facts").await.unwrap();
        db.index_note(&note("a2"), "alpha", "facts").await.unwrap();
        db.index_note(&note("b1"), "beta", "facts").await.unwrap();
        db.insert_raw_memory(&raw("r1", "alpha")).await.unwrap();
        db.insert_raw_memory(&raw("r2", "alpha")).await.unwrap();
        db.insert_raw_memory(&raw("r3", "beta")).await.unwrap();
    }

    /// The regression: the stat cards used to show a cross-agent note count and
    /// a global raw count while the rows underneath were agent-scoped, so
    /// switching agents left the numbers describing a different population.
    #[tokio::test]
    async fn scoped_stats_describe_only_that_agent() {
        let db = db();
        seed(&db).await;

        let r = handle_stats(req(Some(serde_json::json!({ "agent_id": "alpha" }))), db).await;
        let v = r.result.expect("success");

        assert_eq!(v["scope"], "agent");
        assert_eq!(v["totalFacts"], 2, "alpha has 2 notes, not 3");
        assert_eq!(v["totalMemories"], 2, "alpha has 2 raw rows, not 3");
    }

    #[tokio::test]
    async fn unscoped_stats_are_global_and_disclaim_graph_counts() {
        let db = db();
        seed(&db).await;

        let r = handle_stats(req(None), db).await;
        let v = r.result.expect("success");

        assert_eq!(v["scope"], "global");
        assert_eq!(v["totalFacts"], 3, "all agents");
        assert_eq!(v["totalMemories"], 3, "all agents");
        // The note graph is inherently per-agent. Rather than silently report
        // the default agent's graph as if it were everyone's, an unscoped
        // request declines to answer.
        assert!(v["totalGraphNodes"].is_null());
        assert!(v["totalGraphEdges"].is_null());
    }

    #[tokio::test]
    async fn scoped_stats_answer_graph_counts() {
        let db = db();
        seed(&db).await;

        let r = handle_stats(req(Some(serde_json::json!({ "agent_id": "alpha" }))), db).await;
        let v = r.result.expect("success");
        assert_eq!(v["totalGraphNodes"], 2, "alpha's two notes are two nodes");
        assert!(v["totalGraphEdges"].is_i64());
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
cargo test -p alephcore --lib stats_tests
```
Expected: 三个全 FAIL —— 旧 handler 不认 `agent_id`、不发 `scope`。

- [ ] **Step 3: 实现**

替换 `src/gateway/handlers/memory.rs` 的 `handle_stats`（连同上方的 doc comment）：

```rust
/// Parameters for `memory.stats`.
#[derive(Debug, Default, Deserialize)]
pub struct StatsParams {
    /// Scope every count to one agent. Omitted = whole store.
    #[serde(default)]
    pub agent_id: Option<String>,
}

/// Get memory statistics.
///
/// **Every count in one response shares one scope.** Mixing a cross-agent note
/// count with an agent-scoped list is what made the console's stat cards
/// contradict the rows beneath them, and what fed the raw pager a total that
/// did not describe the list it was paging.
///
/// The note graph is inherently per-agent, so an unscoped request returns
/// `null` for the graph counts rather than passing the default agent's graph
/// off as everyone's.
pub async fn handle_stats(request: JsonRpcRequest, db: MemoryBackend) -> JsonRpcResponse {
    use crate::memory::notes::store::NoteStore;

    let params: StatsParams = request
        .params
        .as_ref()
        .and_then(|p| serde_json::from_value(p.clone()).ok())
        .unwrap_or_default();

    let agent = params.agent_id.as_deref();
    let scope = if agent.is_some() { "agent" } else { "global" };

    let raw_count = db.count_raw_memories(agent, None).unwrap_or(0);
    let note_count = match agent {
        Some(a) => db.count_notes(a).await.unwrap_or(0),
        None => db.count_all_notes().await.unwrap_or(0),
    };

    let (graph_nodes, graph_edges) = match agent {
        Some(a) => match db.get_graph_data(a, 10000).await {
            Ok((entries, links)) => (Some(entries.len() as i64), Some(links.len() as i64)),
            Err(_) => (Some(0), Some(0)),
        },
        None => (None, None),
    };

    JsonRpcResponse::success(
        request.id,
        json!({
            "totalMemories": raw_count,
            "totalFacts": note_count,
            // Notes have no invalidated state (unlike the retired fact model),
            // so this mirrors totalFacts. Kept for response compatibility.
            "validFacts": note_count,
            "totalGraphNodes": graph_nodes,
            "totalGraphEdges": graph_edges,
            "scope": scope,
        }),
    )
}
```

- [ ] **Step 4: 跑测试确认通过**

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
cargo test -p alephcore --lib stats_tests
```
Expected: 3 PASS。若 `scoped_stats_answer_graph_counts` 的节点数不是 2，用 `cargo test ... -- --nocapture` 打印实际值并按 `get_graph_data` 的真实语义调整断言（**不要**改实现去迎合断言）。

- [ ] **Step 5: 格式化并提交**

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
rustfmt --edition 2021 src/gateway/handlers/memory.rs
git status --short
git add src/gateway/handlers/memory.rs
git commit -m "memory: scope every stat in one response to one agent

The stat cards counted notes across all agents and raw rows globally while
the table under them was agent-scoped, so switching agents left four
numbers describing a different population. memory.stats now takes an
optional agent_id and reports which scope it answered in. Unscoped
requests return null graph counts instead of passing the default agent's
note graph off as everyone's."
```

---

### Task 4: memory.listFacts 回传 total 并停止丢弃字段

**Files:**
- Modify: `src/gateway/handlers/memory.rs`（`FactEntry` + `handle_list_facts`）
- Test: `src/gateway/handlers/memory.rs`（新增 `mod list_facts_tests`）

**Interfaces:**
- Consumes: 既有 `NoteStore::list_notes(agent)` / `count_notes(agent)`
- Produces: `memory.listFacts` 响应 `{ facts: [FactEntry], total: i64 }`；`FactEntry` 字段 = `id` / `agent_id` / `content` / `fact_type` / `created_at` / `updated_at` / `category` / `path` / `tags` / `link_count`

- [ ] **Step 1: 写失败测试**

```rust
#[cfg(test)]
mod list_facts_tests {
    use super::*;
    use crate::memory::notes::store::NoteStore;
    use crate::memory::notes::KnowledgeNote;
    use crate::memory::store::sqlite::SqliteMemoryBackend;
    use crate::sync_primitives::Arc;

    fn db() -> MemoryBackend {
        let path = std::env::temp_dir().join(format!("mem_lf_test_{}", uuid::Uuid::new_v4()));
        Arc::new(SqliteMemoryBackend::new(&path).unwrap())
    }

    fn req(params: serde_json::Value) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "memory.listFacts".to_string(),
            params: Some(params),
            id: Some(serde_json::json!(1)),
        }
    }

    #[tokio::test]
    async fn total_counts_the_whole_store_not_the_page() {
        let db = db();
        for i in 0..7 {
            let note = KnowledgeNote {
                title: format!("n{i}"),
                category: "facts".to_string(),
                facts: vec!["f".to_string()],
                created_at: 1_700_000_000,
                updated_at: 1_700_000_000,
                content_hash: format!("h{i}"),
                ..Default::default()
            };
            db.index_note(&note, "main", "facts").await.unwrap();
        }

        let v = handle_list_facts(
            req(serde_json::json!({ "agent_id": "main", "limit": 3, "offset": 0 })),
            db,
        )
        .await
        .result
        .expect("success");

        assert_eq!(v["facts"].as_array().unwrap().len(), 3, "page is capped");
        assert_eq!(v["total"], 7, "total describes the store, not the page");
    }

    /// tags / link_count / updated_at are already on every NoteIndexEntry the
    /// query returns. They used to be dropped here, which is why the panel had
    /// nothing to show per row beyond a filename.
    #[tokio::test]
    async fn passes_through_tags_link_count_and_updated_at() {
        let db = db();
        let mut note = KnowledgeNote {
            title: "tagged".to_string(),
            category: "facts".to_string(),
            facts: vec!["f".to_string()],
            created_at: 1_700_000_000,
            updated_at: 1_700_009_999,
            content_hash: "h".to_string(),
            ..Default::default()
        };
        note.tags = vec!["rust".to_string(), "ci".to_string()];
        db.index_note(&note, "main", "facts").await.unwrap();

        let v = handle_list_facts(
            req(serde_json::json!({ "agent_id": "main", "limit": 50, "offset": 0 })),
            db,
        )
        .await
        .result
        .expect("success");

        let row = &v["facts"][0];
        assert_eq!(row["updated_at"], 1_700_009_999_i64);
        let tags: Vec<String> = serde_json::from_value(row["tags"].clone()).unwrap();
        assert_eq!(tags, vec!["rust".to_string(), "ci".to_string()]);
        assert!(row["link_count"].is_u64());
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
cargo test -p alephcore --lib list_facts_tests
```
Expected: 两个都 FAIL（`total` 与 `tags` 键不存在）。

- [ ] **Step 3: 实现**

替换 `FactEntry` 结构体：

```rust
/// Fact entry for JSON serialization.
///
/// `tags` / `link_count` / `updated_at` are already carried by every
/// `NoteIndexEntry` the underlying query returns — this handler used to drop
/// them, leaving the panel with nothing per row but a filename.
#[derive(Debug, Clone, Serialize)]
pub struct FactEntry {
    pub id: String,
    pub agent_id: String,
    pub content: String,
    #[serde(rename = "fact_type")]
    pub note_type: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub category: String,
    pub path: String,
    pub tags: Vec<String>,
    pub link_count: usize,
}
```

替换 `handle_list_facts` 的 `match db.list_notes(agent_id).await { Ok(notes) => { ... } }` 成功臂：

```rust
    match db.list_notes(agent_id).await {
        Ok(notes) => {
            // `total` describes the whole agent store, so the pager can size
            // itself instead of guessing from a full page.
            let total = notes.len() as i64;
            let entries: Vec<FactEntry> = notes
                .into_iter()
                .skip(params.offset)
                .take(params.limit)
                .map(|n| FactEntry {
                    id: n.path.clone(),
                    agent_id: n.agent_id,
                    content: n.filename.clone(),
                    note_type: n.category.clone(),
                    created_at: n.created_at,
                    updated_at: n.updated_at,
                    category: n.category,
                    path: n.path,
                    tags: n.tags,
                    link_count: n.link_count,
                })
                .collect();

            JsonRpcResponse::success(request.id, json!({ "facts": entries, "total": total }))
        }
```

（`total` 从已取回的 `notes` 直接数，不额外发一次 `count_notes` —— 同一次查询里的两个视图不可能漂移。）

- [ ] **Step 4: 跑测试确认通过**

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
cargo test -p alephcore --lib list_facts_tests
```
Expected: 2 PASS。

- [ ] **Step 5: 格式化并提交**

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
rustfmt --edition 2021 src/gateway/handlers/memory.rs
git status --short
git add src/gateway/handlers/memory.rs
git commit -m "memory: listFacts returns a total and stops discarding row fields

tags, link_count and updated_at ride on every NoteIndexEntry the query
already returns; the handler threw them away, so the panel had nothing to
render per row but a filename. total lets the pager size itself instead of
inferring 'there is probably more' from a full page."
```

---

### Task 5: graph.search 成为承载完整笔记行的唯一 FTS 入口

**Files:**
- Modify: `src/gateway/handlers/graph_types.rs:172-179`（`SearchResultDto`）
- Modify: `src/gateway/handlers/graph/search.rs:40-60`（映射）
- Test: `src/gateway/handlers/graph/search.rs`（在既有 `mod tests` 内追加）

**Interfaces:**
- Produces: `SearchResultDto { id, name, category, match_field, agent_id, created_at, updated_at, tags, link_count }` —— Panel 的 SearchHits 卡片直接消费这一形状，不再需要二次取数

- [ ] **Step 1: 写失败测试**

在 `src/gateway/handlers/graph/search.rs` 既有 `mod tests` 内追加：

```rust
    /// The SearchHits layer renders these as note cards, so a hit must carry
    /// everything a card shows. All of it is already on NoteIndexEntry.
    #[tokio::test]
    async fn search_hits_carry_full_note_row() {
        let db = make_db();
        let mut note = make_note_with_fact("TaggedSearchNote", "distinctivefactword");
        note.tags = vec!["rust".to_string(), "ci".to_string()];
        db.index_note(&note, "main", "concept").await.unwrap();

        let req = search_request("distinctivefactword", 20, Some("main"));
        let resp = handle_search_impl(req, db).await;
        assert!(resp.error.is_none(), "expected success: {:?}", resp.error);

        let v = resp.result.expect("result");
        let hit = &v["results"][0];
        assert_eq!(hit["agent_id"], "main");
        assert!(hit["created_at"].is_i64(), "created_at must be present");
        assert!(hit["updated_at"].is_i64(), "updated_at must be present");
        assert!(hit["link_count"].is_u64(), "link_count must be present");
        let tags: Vec<String> = serde_json::from_value(hit["tags"].clone()).unwrap();
        assert_eq!(tags, vec!["rust".to_string(), "ci".to_string()]);
    }
```

- [ ] **Step 2: 跑测试确认失败**

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
cargo test -p alephcore --lib graph::search
```
Expected: `search_hits_carry_full_note_row` FAIL —— `agent_id` 键不存在（`v["results"][0]["agent_id"]` 是 `Null`）。

- [ ] **Step 3: 扩展 DTO**

替换 `src/gateway/handlers/graph_types.rs` 的 `SearchResultDto`：

```rust
/// One full-text search hit.
///
/// Carries the whole index row, not just an id: the panel renders hits as note
/// cards, and a hit that only knows its own name would force a second round
/// trip per row. Every field below is already on `NoteIndexEntry`.
#[derive(Debug, Serialize, Deserialize)]
pub struct SearchResultDto {
    pub id: String,
    pub name: String,
    pub category: String,
    /// `"title"` when the query matched the filename, `"content"` otherwise.
    pub match_field: String,
    pub agent_id: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub tags: Vec<String>,
    pub link_count: usize,
}
```

- [ ] **Step 4: 扩展映射**

替换 `src/gateway/handlers/graph/search.rs` 里构造 `SearchResultDto` 的那段（原 40-60 行）：

```rust
    let results: Vec<SearchResultDto> = entries
        .into_iter()
        .map(|entry| {
            // Match-field heuristic: did the query hit the filename?
            let match_field = if entry
                .filename
                .to_lowercase()
                .contains(&params.query.to_lowercase())
            {
                "title".to_string()
            } else {
                "content".to_string()
            };
            SearchResultDto {
                id: entry.path.clone(),
                name: entry.filename,
                category: entry.category,
                match_field,
                agent_id: entry.agent_id,
                created_at: entry.created_at,
                updated_at: entry.updated_at,
                tags: entry.tags,
                link_count: entry.link_count,
            }
        })
        .collect();
```

- [ ] **Step 5: 跑测试确认通过**

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
cargo test -p alephcore --lib graph::search
```
Expected: 全 PASS（含原有 search 测试）。

- [ ] **Step 6: 格式化并提交**

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
rustfmt --edition 2021 src/gateway/handlers/graph_types.rs src/gateway/handlers/graph/search.rs
git status --short
git add src/gateway/handlers/graph_types.rs src/gateway/handlers/graph/search.rs
git commit -m "graph: search hits carry the full note row

The panel renders hits as note cards. A hit that knew only its id and name
would force one graph.node_detail per row just to draw a tag chip, so the
DTO now carries agent_id / created_at / updated_at / tags / link_count --
all of which the FTS query already returns on NoteIndexEntry."
```

---

### Task 6: CUT graph.neighbors

**Files:**
- Delete: `src/gateway/handlers/graph/neighbors.rs`（316 行）
- Modify: `src/gateway/handlers/graph/mod.rs:10`（`mod neighbors;`）与 `:16`（re-export）
- Modify: `src/gateway/handlers/graph_types.rs:15-31`（params + 两个 const）与 `:186-197`（response）
- Modify: `src/bin/aleph-server/commands/start/builder/handlers/agents.rs:578-586`（注册块）

**Interfaces:**
- Produces: 无（纯删除）。`NoteStore::get_neighbors` **保留** —— `src/builtin_tools/note_graph_query.rs:254` 是活消费者。`entry_to_dto` / `undirected_key` / `notes_dir` 三个 `graph/mod.rs` 辅助函数**均保留** —— `query.rs` 与 `node_detail.rs` 仍在用。

- [ ] **Step 1: 复核零消费者（读后再删，不是删后再查）**

```bash
grep -rn "graph\.neighbors\|handle_neighbors_impl\|GraphNeighborsResponse\|GraphNeighborsParams" \
  src/ interfaces/ shared/ desktop/ mobile/ 2>/dev/null | grep -v "src/gateway/handlers/graph/neighbors.rs"
```
Expected: 恰好 4 处 —— `graph/mod.rs:16`、`graph_types.rs:17`、`graph_types.rs:188`、`agents.rs:582/584`。若出现任何其它调用方，**停下并报告**（说明有新消费者，CUT 决策需重议）。

- [ ] **Step 2: 删文件与注册**

```bash
git rm src/gateway/handlers/graph/neighbors.rs
```

`src/gateway/handlers/graph/mod.rs`：删掉第 10 行 `mod neighbors;` 与第 16 行 `pub use neighbors::handle_neighbors_impl;`。

`src/bin/aleph-server/commands/start/builder/handlers/agents.rs`：删掉整个第 578-586 行块：

```rust
    {
        let db = ::std::sync::Arc::clone(memory_db);
        server
            .handlers_mut()
            .register("graph.neighbors", move |req| {
                let db = ::std::sync::Arc::clone(&db);
                async move { graph::handle_neighbors_impl(req, db).await }
            });
    }
```

- [ ] **Step 3: 删 DTO**

`src/gateway/handlers/graph_types.rs`：删掉第 15-31 行整段（`// === graph.neighbors ===` 注释、`GraphNeighborsParams`、`default_depth`、`default_neighbor_limit`），以及第 186-197 行整段（`// === graph.neighbors response ... ===` 注释与 `GraphNeighborsResponse`）。

- [ ] **Step 4: 验证编译，确认无连带死码**

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
cargo check -p alephcore --lib
cargo check --bin aleph-server
cargo clippy -p alephcore --lib -- -D warnings
```
Expected: 全部干净、**零 `dead_code` 警告**。若 clippy 报某个 helper 现在无人用，说明 Step 1 的复核漏了东西 —— 报告而非加 `#[allow]`。

- [ ] **Step 5: 提交**

```bash
git add -A src/gateway/handlers/graph/ src/gateway/handlers/graph_types.rs \
  src/bin/aleph-server/commands/start/builder/handlers/agents.rs
git commit -m "graph: cut the neighbors endpoint

Live registration, zero callers repo-wide: the 2D radial engine that
consumed it was retired and archived, and the galaxy loads the whole graph
in one shot (cap 500) so it has no use for hop-by-hop expansion. Docs have
carried a 'connect it or cut it' note since 2026-07-14; this is the cut.

NoteStore::get_neighbors stays -- note_graph_query still calls it."
```

---

### Task 7: CLI 对齐真实响应体

**Files:**
- Modify: `interfaces/cli/src/commands/memory_cmd.rs:19-42`（`search`）与 `:44-93`（`stats`）

**Interfaces:**
- Consumes: Task 2 的 `memory.search` 响应 `{memories:[{id, agent_id, user_input, ai_output, session_id, timestamp}]}`；Task 3 的 `memory.stats` 响应 `{totalMemories, totalFacts, validFacts, totalGraphNodes, totalGraphEdges, scope}`

- [ ] **Step 1: 先手工确认症状**

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
grep -n 'result.as_array()\|"total_facts"\|"total_sessions"\|"storage_size"' interfaces/cli/src/commands/memory_cmd.rs
```
Expected: 命中 —— `search` 对一个 JSON **对象**调 `as_array()`（恒 `None` ⇒ 恒空表），`stats` 读的三个键后端从不产出（恒 `-`）。这两处是纯 DTO 抄写漂移，无测试覆盖，所以一直没人发现。

- [ ] **Step 2: 改 search**

替换 `interfaces/cli/src/commands/memory_cmd.rs` 的 `search` 函数体中间段（从 `let mut rows` 到 `output::print_table(...)` 之前）：

```rust
    // The response is an object: {"memories": [...]}. Calling as_array() on it
    // returned None, so this table was unconditionally empty.
    let mut rows = Vec::new();
    if let Some(memories) = result.get("memories").and_then(serde_json::Value::as_array) {
        for item in memories {
            let ts = item
                .get("timestamp")
                .and_then(serde_json::Value::as_i64)
                .map_or_else(|| "-".to_string(), |t| t.to_string());
            let agent = item
                .get("agent_id")
                .and_then(|v| v.as_str())
                .unwrap_or("-");
            let content = item
                .get("user_input")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .or_else(|| item.get("ai_output").and_then(|v| v.as_str()))
                .unwrap_or("-");
            rows.push(vec![ts, agent.to_string(), truncate(content, 80)]);
        }
    }

    output::print_table(&["Timestamp", "Agent", "Content"], &rows, json, &result);
```

- [ ] **Step 3: 改 stats**

替换 `stats` 函数里的 `let pairs = vec![...]` 整块：

```rust
    // Keys are camelCase on the wire (see gateway handle_stats). The previous
    // snake_case reads plus two keys the server never emits made every row
    // print "-".
    let num = |key: &str| -> String {
        result
            .get(key)
            .and_then(serde_json::Value::as_i64)
            .map_or_else(|| "-".to_string(), |n| n.to_string())
    };

    let pairs = vec![
        (
            "Scope",
            result
                .get("scope")
                .and_then(|v| v.as_str())
                .unwrap_or("-")
                .to_string(),
        ),
        ("Note Memory", num("totalFacts")),
        ("Raw Memory", num("totalMemories")),
        // null when unscoped: the note graph is per-agent, so a store-wide
        // request has no honest single answer.
        ("Graph Nodes", num("totalGraphNodes")),
        ("Graph Edges", num("totalGraphEdges")),
    ];
```

- [ ] **Step 4: 验证编译**

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
cargo check -p aleph-cli
cargo clippy -p aleph-cli -- -D warnings
```
Expected: 编译干净。（package = `aleph-cli`，binary = `aleph`，已核对 `interfaces/cli/Cargo.toml`。）

- [ ] **Step 5: 端到端手验**

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
cargo run --bin aleph-server -- --daemon &
sleep 8
cargo run -p aleph-cli --bin aleph -- memory stats
cargo run -p aleph-cli --bin aleph -- memory search --query test --limit 5
```
Expected: `stats` 打印真实数字（不再全是 `-`）；`search` 打印 Timestamp/Agent/Content 三列（有数据时不再空表）。跑完 `pkill -f aleph-server`。若本机没有 memory 数据，`search` 空表是正常的 —— 关键是 `stats` 不再全 `-`。

- [ ] **Step 6: 格式化并提交**

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
rustfmt --edition 2021 interfaces/cli/src/commands/memory_cmd.rs
git status --short
git add interfaces/cli/src/commands/memory_cmd.rs
git commit -m "cli: read the memory responses the gateway actually sends

'aleph memory search' called as_array() on a JSON object and printed an
empty table every time. 'aleph memory stats' read snake_case keys plus
storage_size and last_compressed, which the handler has never emitted, so
every row printed '-'. Both were hand-copied DTOs with no test covering
the wire shape."
```

---

## Phase B — Panel 契约与纯函数

### Task 8: Loadable 三态 + SearchHits 层

**Files:**
- Modify: `interfaces/webchat/src/platform/wide/views/memory/data.rs`
- Test: 同文件的 `#[cfg(test)] mod tests`

**Interfaces:**
- Produces:
  - `pub enum Loadable<T> { Loading, Ready(T), Failed(String) }`
  - `impl<T> Loadable<T> { pub fn from_rpc(res: Result<T, String>) -> Self; pub fn as_ready(&self) -> Option<&T>; pub fn is_loading(&self) -> bool }`
  - `MemoryFacet::SearchHits` 变体
- 后续 Task 依赖：`loader.rs`（Task 15）、`cards.rs`（Task 14）、`facets.rs`（Task 16）、`pager.rs`（Task 12）

- [ ] **Step 1: 写失败测试**

在 `interfaces/webchat/src/platform/wide/views/memory/data.rs` 的 `mod tests` 内追加：

```rust
    // ── Loadable ────────────────────────────────────────────────────────────

    #[test]
    fn from_rpc_preserves_the_error_message() {
        // This is the whole point: the old loaders mapped Err to "no data", so
        // an RPC failure and an empty store rendered identically.
        let failed: Loadable<Vec<u32>> = Loadable::from_rpc(Err("gateway timeout".into()));
        assert_eq!(failed, Loadable::Failed("gateway timeout".to_string()));
        assert!(failed.as_ready().is_none());
    }

    #[test]
    fn from_rpc_wraps_ok_as_ready() {
        let ready: Loadable<Vec<u32>> = Loadable::from_rpc(Ok(vec![1, 2]));
        assert_eq!(ready.as_ready(), Some(&vec![1, 2]));
        assert!(!ready.is_loading());
    }

    #[test]
    fn an_empty_ok_is_ready_not_failed() {
        // An empty store is a legitimate Ready state, distinct from Failed.
        let empty: Loadable<Vec<u32>> = Loadable::from_rpc(Ok(vec![]));
        assert_eq!(empty.as_ready(), Some(&vec![]));
        assert!(matches!(empty, Loadable::Ready(_)));
    }

    #[test]
    fn loading_is_neither_ready_nor_failed() {
        let l: Loadable<Vec<u32>> = Loadable::Loading;
        assert!(l.is_loading());
        assert!(l.as_ready().is_none());
    }

    // ── SearchHits facet ────────────────────────────────────────────────────

    #[test]
    fn search_hits_is_note_shaped() {
        // Hits are notes, so every note-shaped affordance (drawer, locate-in-
        // graph, note delete path) applies to them.
        assert!(MemoryFacet::SearchHits.is_notes());
        assert!(!MemoryFacet::Raw.is_notes());
    }

    #[test]
    fn search_hits_never_slices_the_window() {
        // Hit rows arrive from graph.search on their own signal; slicing the
        // loaded window for this facet would silently show stale local rows.
        let facts = vec![fact("preference"), fact("feedback")];
        assert!(facet_slice(&facts, MemoryFacet::SearchHits).is_empty());
    }

    #[test]
    fn bucket_counts_ignores_search_hits() {
        // The chip badges describe the loaded window's four note buckets; the
        // hit count is reported separately by the hits signal.
        let facts = vec![fact("feedback"), fact("preference")];
        assert_eq!(bucket_counts(&facts), [2, 1, 1, 0]);
    }
```

- [ ] **Step 2: 跑测试确认失败**

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
cargo test -p aleph-panel --lib memory::data
```
Expected: FAIL —— `cannot find type Loadable` / `no variant SearchHits`。

- [ ] **Step 3: 实现 Loadable**

在 `data.rs` 的 `PAGE_SIZE` 常量之后插入：

```rust
/// The state of one fetch.
///
/// Replaces the `(loaded: bool, data: T)` pair the memory console used to carry
/// alongside `if let Ok(..)` loaders. Under that shape an RPC failure produced
/// an empty `data` with `loaded = true` — indistinguishable from an empty
/// store, so every gateway error rendered as "no memories yet". Making failure
/// its own variant means a renderer cannot match exhaustively without drawing
/// the error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Loadable<T> {
    Loading,
    Ready(T),
    Failed(String),
}

impl<T> Loadable<T> {
    /// Lift an RPC result into a load state, keeping the error text so the UI
    /// can show *what* went wrong rather than an empty list.
    #[must_use]
    pub fn from_rpc(res: Result<T, String>) -> Self {
        match res {
            Ok(v) => Self::Ready(v),
            Err(e) => Self::Failed(e),
        }
    }

    #[must_use]
    pub fn as_ready(&self) -> Option<&T> {
        match self {
            Self::Ready(v) => Some(v),
            _ => None,
        }
    }

    #[must_use]
    pub fn is_loading(&self) -> bool {
        matches!(self, Self::Loading)
    }
}
```

- [ ] **Step 4: 加 SearchHits 变体**

`data.rs` 中替换 `MemoryFacet` 与 `is_notes`：

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryFacet {
    AllNotes,
    Facts,
    Feedback,
    Lessons,
    /// Server-side full-text hits from `graph.search`.
    ///
    /// Note-shaped like the four buckets above, but its rows do NOT come from
    /// the loaded window — they arrive on their own signal, so `facet_slice`
    /// returns empty here and `bucket_counts` ignores it.
    SearchHits,
    Raw,
}

impl MemoryFacet {
    /// True for every note-shaped facet (i.e. not `Raw`). Drives which table /
    /// drawer / delete verb applies: note facets use `graph.delete_note`, `Raw`
    /// uses `memory.delete`. Mixing those two is what made search hits
    /// undeletable.
    #[must_use]
    pub fn is_notes(&self) -> bool {
        !matches!(self, MemoryFacet::Raw)
    }
}
```

在 `facet_slice` 中把 `MemoryFacet::Raw => Vec::new(),` 一臂改为：

```rust
        MemoryFacet::Raw | MemoryFacet::SearchHits => Vec::new(),
```

（`bucket_counts` 无需改动：它 `match fact_facet(..)`，而 `fact_facet` 只产出 `Facts` / `Feedback` / `Lessons` 三者。）

- [ ] **Step 5: 跑测试确认通过**

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
cargo test -p aleph-panel --lib memory::
```
Expected: 原 17 测 + 新 7 测全 PASS。若报 `Loadable<T>` 的 `Eq` 约束不满足（`T` 不是 `Eq`），把 derive 改为 `#[derive(Debug, Clone, PartialEq)]` 并在测试里保持 `assert_eq!`（`Vec<u32>` 满足 `PartialEq` 即可）。

- [ ] **Step 6: 检查 phone 侧是否被穷尽 match 波及**

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
cargo check -p aleph-panel --target wasm32-unknown-unknown
```
Expected: 干净。新增枚举变体会让任何 `MemoryFacet` 的穷尽 `match` 报错；phone 侧 `FACETS` 是常量数组（不 match），但 `views/memory/mod.rs::facet_total` 会报 —— 加一臂 `MemoryFacet::SearchHits => 0`（该函数在 Task 12 移入 `pager.rs` 时会重写，此处只求编译过）。

- [ ] **Step 7: 格式化并提交**

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
rustfmt --edition 2021 interfaces/webchat/src/platform/wide/views/memory/data.rs
git status --short
git add interfaces/webchat/src/platform/wide/views/memory/
git commit -m "panel/memory: make a failed fetch unrepresentable as empty

The console carried (loaded: bool, data) and filled it with if-let-Ok, so
every gateway error became an empty list that read as 'no memories yet'.
Loadable<T> gives failure its own variant with the message attached, so a
renderer cannot match exhaustively without drawing the error.

Adds MemoryFacet::SearchHits for server-side FTS hits: note-shaped, but
sourced from its own signal rather than the loaded window."
```

---

### Task 9: Markdown 导出纯函数

**Files:**
- Modify: `interfaces/webchat/src/platform/wide/views/memory/data.rs`
- Test: 同文件 `mod tests`

**Interfaces:**
- Produces:
  - `pub const EXPORT_MAX: usize = 50;`
  - `pub struct NoteExport { pub title: String, pub path: String, pub body: Result<String, String> }`
  - `pub struct RawExport { pub id: String, pub agent_id: String, pub session_id: Option<String>, pub created_at: String, pub user_input: String, pub ai_output: String }`
  - `pub fn notes_to_markdown(items: &[NoteExport]) -> String`
  - `pub fn raws_to_markdown(items: &[RawExport]) -> String`
- 后续 Task 依赖：`batch_bar.rs`（Task 17）

- [ ] **Step 1: 写失败测试**

在 `data.rs` 的 `mod tests` 内追加：

```rust
    // ── Markdown export ─────────────────────────────────────────────────────

    #[test]
    fn notes_export_writes_title_path_and_body() {
        let items = vec![NoteExport {
            title: "deploy-notes".into(),
            path: "facts/deploy-notes".into(),
            body: Ok("- smoke test first\n".into()),
        }];
        let md = notes_to_markdown(&items);
        assert!(md.contains("# deploy-notes"));
        assert!(md.contains("`facts/deploy-notes`"));
        assert!(md.contains("- smoke test first"));
    }

    #[test]
    fn notes_export_marks_unfetchable_bodies_instead_of_dropping_them() {
        // A note whose body failed to load must still appear, with the reason
        // visible. Silently omitting it would make the export look complete.
        let items = vec![NoteExport {
            title: "broken".into(),
            path: "facts/broken".into(),
            body: Err("node_detail: timeout".into()),
        }];
        let md = notes_to_markdown(&items);
        assert!(md.contains("# broken"));
        assert!(md.contains("<!-- body unavailable: node_detail: timeout -->"));
    }

    #[test]
    fn notes_export_separates_entries_with_a_blank_line() {
        let items = vec![
            NoteExport { title: "a".into(), path: "facts/a".into(), body: Ok("x".into()) },
            NoteExport { title: "b".into(), path: "facts/b".into(), body: Ok("y".into()) },
        ];
        let md = notes_to_markdown(&items);
        assert_eq!(md.matches("# ").count(), 2);
        assert!(md.contains("x\n\n# b"), "entries must be blank-line separated: {md:?}");
    }

    #[test]
    fn raws_export_labels_both_halves_and_keeps_session() {
        let items = vec![RawExport {
            id: "raw-1".into(),
            agent_id: "main".into(),
            session_id: Some("s-77".into()),
            created_at: "2026-07-24 14:02".into(),
            user_input: "why phantom pages?".into(),
            ai_output: "the total was global".into(),
        }];
        let md = raws_to_markdown(&items);
        assert!(md.contains("2026-07-24 14:02"));
        assert!(md.contains("main"));
        assert!(md.contains("s-77"));
        assert!(md.contains("**Q** why phantom pages?"));
        assert!(md.contains("**A** the total was global"));
    }

    #[test]
    fn raws_export_omits_an_empty_half() {
        // Raw rows built from a single-sided record must not emit a bare "**A**".
        let items = vec![RawExport {
            id: "raw-2".into(),
            agent_id: "main".into(),
            session_id: None,
            created_at: "2026-07-24 14:03".into(),
            user_input: "only a question".into(),
            ai_output: String::new(),
        }];
        let md = raws_to_markdown(&items);
        assert!(md.contains("**Q** only a question"));
        assert!(!md.contains("**A**"));
    }

    #[test]
    fn export_cap_is_fifty() {
        // The batch bar disables itself above this and says so; the constant is
        // the single source both the guard and the message read.
        assert_eq!(EXPORT_MAX, 50);
    }
```

- [ ] **Step 2: 跑测试确认失败**

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
cargo test -p aleph-panel --lib memory::data
```
Expected: FAIL —— `cannot find function notes_to_markdown` 等。

- [ ] **Step 3: 实现**

在 `data.rs` 末尾（`#[cfg(test)]` 之前）追加：

```rust
// ─── Markdown export ────────────────────────────────────────────────────────

/// Maximum entries one clipboard export may carry.
///
/// Each note needs its own `graph.node_detail` round trip, so an unbounded
/// "select all → copy" would fan out arbitrarily. The batch bar disables the
/// button above this and says the limit out loud — a silent truncation would
/// hand the user a partial export that looks complete.
pub const EXPORT_MAX: usize = 50;

/// One note staged for export. `body` is `Err` when its full text could not be
/// fetched; the renderer keeps the entry and records the reason rather than
/// dropping it.
#[derive(Debug, Clone)]
pub struct NoteExport {
    pub title: String,
    pub path: String,
    pub body: Result<String, String>,
}

/// One raw conversation row staged for export.
#[derive(Debug, Clone)]
pub struct RawExport {
    pub id: String,
    pub agent_id: String,
    pub session_id: Option<String>,
    /// Already-formatted display timestamp (see [`format_ts`]).
    pub created_at: String,
    pub user_input: String,
    pub ai_output: String,
}

/// Render staged notes as a markdown document, one `#` section per note.
#[must_use]
pub fn notes_to_markdown(items: &[NoteExport]) -> String {
    let mut out = String::new();
    for item in items {
        out.push_str(&format!("# {}\n\n`{}`\n\n", item.title, item.path));
        match &item.body {
            Ok(body) => out.push_str(body.trim_end()),
            Err(e) => out.push_str(&format!("<!-- body unavailable: {e} -->")),
        }
        out.push_str("\n\n");
    }
    // One trailing newline, not two, so round-tripping the text is stable.
    out.truncate(out.trim_end().len());
    out.push('\n');
    out
}

/// Render staged raw rows as a markdown document, one `#` section per turn.
#[must_use]
pub fn raws_to_markdown(items: &[RawExport]) -> String {
    let mut out = String::new();
    for item in items {
        let session = item
            .session_id
            .as_deref()
            .map(|s| format!(" · session {s}"))
            .unwrap_or_default();
        out.push_str(&format!(
            "# {}\n\n`{}` · {}{}\n\n",
            item.created_at, item.id, item.agent_id, session
        ));
        if !item.user_input.trim().is_empty() {
            out.push_str(&format!("**Q** {}\n\n", item.user_input.trim()));
        }
        if !item.ai_output.trim().is_empty() {
            out.push_str(&format!("**A** {}\n\n", item.ai_output.trim()));
        }
    }
    out.truncate(out.trim_end().len());
    out.push('\n');
    out
}
```

- [ ] **Step 4: 跑测试确认通过**

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
cargo test -p aleph-panel --lib memory::data
```
Expected: 全 PASS。`notes_export_separates_entries_with_a_blank_line` 若因末尾裁剪失败，检查 `out.contains("x\n\n# b")` 断言与实现的空行数是否一致（实现在每条后写 `\n\n`，body 已 `trim_end`，故 `x\n\n# b` 成立）。

- [ ] **Step 5: 格式化并提交**

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
rustfmt --edition 2021 interfaces/webchat/src/platform/wide/views/memory/data.rs
git status --short
git add interfaces/webchat/src/platform/wide/views/memory/data.rs
git commit -m "panel/memory: markdown export as pure functions

Notes whose body could not be fetched stay in the output as an explicit
'body unavailable' comment -- dropping them would hand the user a partial
export that looks complete. EXPORT_MAX is the one constant both the button
guard and its explanatory label read."
```

---

### Task 10: Panel DTO 对齐服务端

**Files:**
- Modify: `interfaces/webchat/src/api/memory.rs`
- Modify: `interfaces/webchat/src/canvas_engine/adapter.rs:80-86`（`SearchResultDto`）
- Test: `interfaces/webchat/src/api/memory.rs` 的 `mod tests`

**Interfaces:**
- Consumes: Task 2/3/4/5 的响应形状
- Produces:
  - `RawMemory { id, agent_id, user_input, ai_output, session_id, created_at }` + `fn display_text(&self) -> String`（`content` 字段删除）
  - `CompressedFact { id, agent_id, content, fact_type, created_at, updated_at, category, path, tags, link_count }`
  - `MemoryStats { total_facts, total_memories, valid_facts, total_graph_nodes: Option<u64>, total_graph_edges: Option<u64>, scope: String }`
  - `MemoryApi::list_facts(state, agent, limit, offset) -> Result<(Vec<CompressedFact>, u64), String>`
  - `MemoryApi::browse_raw(state, agent, query, limit, offset) -> Result<Vec<RawMemory>, String>`（原 `search` 改名，语义收窄为 raw）
  - `MemoryApi::stats(state, agent) -> Result<MemoryStats, String>`
  - `MemoryApi::trace(state, agent, target, kind: TraceKind, max_results: usize) -> Result<TraceResult, String>` + `pub enum TraceKind { Note, Raw }` + `pub struct TraceResult { target, notes, evidence }` + `pub struct EvidenceItem { raw_id, via_note, via_session, content, pruned }`
  - `canvas_engine::adapter::SearchResultDto` 追加 `agent_id` / `created_at` / `updated_at` / `tags` / `link_count`
  - `impl CompressedFact { pub fn from_search_hit(hit: &SearchResultDto) -> Self }`

- [ ] **Step 1: 写失败测试**

替换 `interfaces/webchat/src/api/memory.rs` 的 `mod tests` 为：

```rust
#[cfg(test)]
mod tests {
    use super::{CompressedFact, RawMemory};
    use crate::canvas_engine::adapter::SearchResultDto;

    #[test]
    fn stub_from_path_splits_category_and_filename() {
        let fact = CompressedFact::stub_from_path("facts/rust-notes.md");
        assert_eq!(fact.id, "facts/rust-notes.md");
        assert_eq!(fact.path, "facts/rust-notes.md");
        assert_eq!(fact.category, "facts");
        assert_eq!(fact.content, "rust-notes.md");
        assert_eq!(fact.agent_id, "");
        assert_eq!(fact.created_at, 0);
        assert!(fact.tags.is_empty());
        assert_eq!(fact.link_count, 0);
    }

    #[test]
    fn stub_from_path_falls_back_to_other_for_bare_filename() {
        let fact = CompressedFact::stub_from_path("rust-notes.md");
        assert_eq!(fact.category, "other");
        assert_eq!(fact.content, "rust-notes.md");
    }

    /// A search hit is a full note row, so it converts into the same card model
    /// the note layers use — no second round trip per row.
    #[test]
    fn from_search_hit_carries_the_whole_row() {
        let hit = SearchResultDto {
            id: "facts/deploy-notes".into(),
            name: "deploy-notes".into(),
            category: "facts".into(),
            match_field: "content".into(),
            agent_id: "main".into(),
            created_at: 1_700_000_000,
            updated_at: 1_700_009_999,
            tags: vec!["rust".into(), "ci".into()],
            link_count: 3,
        };
        let fact = CompressedFact::from_search_hit(&hit);
        assert_eq!(fact.path, "facts/deploy-notes");
        assert_eq!(fact.content, "deploy-notes");
        assert_eq!(fact.category, "facts");
        assert_eq!(fact.agent_id, "main");
        assert_eq!(fact.created_at, 1_700_000_000);
        assert_eq!(fact.updated_at, 1_700_009_999);
        assert_eq!(fact.tags, vec!["rust".to_string(), "ci".to_string()]);
        assert_eq!(fact.link_count, 3);
    }

    #[test]
    fn raw_display_text_joins_both_halves_only_when_present() {
        let both = RawMemory {
            id: "r1".into(),
            agent_id: "main".into(),
            user_input: "q".into(),
            ai_output: "a".into(),
            session_id: None,
            created_at: None,
        };
        assert_eq!(both.display_text(), "Q: q\nA: a");

        let q_only = RawMemory {
            id: "r2".into(),
            agent_id: "main".into(),
            user_input: "q".into(),
            ai_output: String::new(),
            session_id: None,
            created_at: None,
        };
        assert_eq!(q_only.display_text(), "q");

        let a_only = RawMemory {
            id: "r3".into(),
            agent_id: "main".into(),
            user_input: String::new(),
            ai_output: "a".into(),
            session_id: None,
            created_at: None,
        };
        assert_eq!(a_only.display_text(), "a");
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
cargo test -p aleph-panel --lib api::memory
```
Expected: FAIL —— `SearchResultDto` 缺字段、`from_search_hit` / `display_text` 不存在、`RawMemory` 无 `user_input`。

- [ ] **Step 3: 扩展 canvas_engine::adapter::SearchResultDto**

替换 `interfaces/webchat/src/canvas_engine/adapter.rs:80-86`：

```rust
/// One `graph.search` hit — a full note index row, mirroring the server DTO.
#[derive(Debug, Clone, Deserialize)]
pub struct SearchResultDto {
    pub id: String,
    pub name: String,
    pub category: String,
    pub match_field: String,
    #[serde(default)]
    pub agent_id: String,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub updated_at: i64,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub link_count: usize,
}
```

（`#[serde(default)]` 让旧服务端的窄响应仍能解析 —— 远程 Panel 可能连到未升级的 core。）

- [ ] **Step 4: 改写 api/memory.rs 的 DTO 与调用**

替换 `interfaces/webchat/src/api/memory.rs` 从文件开头到 `impl MemoryApi` 结束（保留末尾的 `format_timestamp_secs` 与新的 `mod tests`）：

```rust
use crate::canvas_engine::adapter::SearchResultDto;
use crate::context::DashboardState;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Raw memory entry (Layer 1 — one conversation record).
///
/// `user_input` / `ai_output` stay separate: the card renders the two halves
/// with different weights, which a pre-joined `content` string made impossible.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawMemory {
    pub id: String,
    #[serde(default)]
    pub agent_id: String,
    #[serde(default)]
    pub user_input: String,
    #[serde(default)]
    pub ai_output: String,
    /// Session the row was recorded in, when known.
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
}

impl RawMemory {
    /// Both halves as one string, for clipboard export and single-line previews.
    #[must_use]
    pub fn display_text(&self) -> String {
        match (self.user_input.is_empty(), self.ai_output.is_empty()) {
            (false, false) => format!("Q: {}\nA: {}", self.user_input, self.ai_output),
            (false, true) => self.user_input.clone(),
            _ => self.ai_output.clone(),
        }
    }
}

/// Compiled knowledge note (Layer 2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressedFact {
    pub id: String,
    #[serde(default)]
    pub agent_id: String,
    /// Display title (the note filename).
    pub content: String,
    pub fact_type: String,
    pub created_at: i64,
    #[serde(default)]
    pub updated_at: i64,
    pub category: String,
    pub path: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub link_count: usize,
}

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
            updated_at: 0,
            category: category.to_string(),
            path: path.to_string(),
            tags: Vec::new(),
            link_count: 0,
        }
    }

    /// Convert a `graph.search` hit into the same card model the note layers
    /// use. The hit carries the whole index row, so this needs no round trip.
    #[must_use]
    pub fn from_search_hit(hit: &SearchResultDto) -> Self {
        Self {
            id: hit.id.clone(),
            agent_id: hit.agent_id.clone(),
            content: hit.name.clone(),
            fact_type: hit.category.clone(),
            created_at: hit.created_at,
            updated_at: hit.updated_at,
            category: hit.category.clone(),
            path: hit.id.clone(),
            tags: hit.tags.clone(),
            link_count: hit.link_count,
        }
    }
}

/// Backend `list_facts` response wrapper.
#[derive(Debug, Clone, Deserialize)]
struct BackendListFactsResponse {
    #[serde(default)]
    facts: Vec<CompressedFact>,
    /// Total notes for the agent, independent of `limit`/`offset`.
    #[serde(default)]
    total: u64,
}

/// Backend `memory.search` response wrapper.
#[derive(Debug, Clone, Deserialize)]
struct BackendSearchResponse {
    #[serde(default)]
    memories: Vec<BackendMemoryEntry>,
}

#[derive(Debug, Clone, Deserialize)]
struct BackendMemoryEntry {
    id: String,
    #[serde(default)]
    agent_id: String,
    #[serde(default)]
    user_input: String,
    #[serde(default)]
    ai_output: String,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    timestamp: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryStats {
    #[serde(default)]
    pub total_facts: u64,
    #[serde(default)]
    pub total_memories: u64,
    #[serde(default)]
    pub valid_facts: u64,
    /// `None` when the server answered store-wide: the note graph is per-agent,
    /// so there is no honest single number.
    #[serde(default)]
    pub total_graph_nodes: Option<u64>,
    #[serde(default)]
    pub total_graph_edges: Option<u64>,
    /// `"agent"` or `"global"` — which population the counts describe.
    #[serde(default)]
    pub scope: String,
}

/// Which kind of target `memory.trace` walks. Mirrors the server's `TraceKind`.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceKind {
    /// A note path: walk DOWN to the raw rows it was distilled from.
    Note,
    /// A raw memory id: walk UP to the notes citing it.
    Raw,
}

/// One piece of ground-truth evidence.
#[derive(Debug, Clone, Deserialize)]
pub struct EvidenceItem {
    pub raw_id: String,
    #[serde(default)]
    pub via_note: Option<String>,
    #[serde(default)]
    pub via_session: Option<String>,
    /// First 800 chars of raw content; `None` when `pruned`.
    #[serde(default)]
    pub content: Option<String>,
    /// The raw id was cited but its row is gone from the store.
    #[serde(default)]
    pub pruned: bool,
}

/// Result of walking the evidence chain.
#[derive(Debug, Clone, Deserialize)]
pub struct TraceResult {
    #[serde(default)]
    pub notes: Vec<String>,
    #[serde(default)]
    pub evidence: Vec<EvidenceItem>,
}

pub struct MemoryApi;

impl MemoryApi {
    /// Browse / filter raw memories (Layer 1).
    ///
    /// `query` is a substring filter over raw content. This never returns
    /// notes — note full-text search is `GraphApi::search`.
    pub async fn browse_raw(
        state: &DashboardState,
        agent_id: &str,
        query: String,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<RawMemory>, String> {
        let params = serde_json::json!({
            "agent_id": agent_id,
            "query": query,
            "limit": limit,
            "offset": offset,
        });

        let result = state.rpc_call("memory.search", params).await?;
        let response: BackendSearchResponse = serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse memory.search: {e}"))?;

        Ok(response
            .memories
            .into_iter()
            .map(|entry| RawMemory {
                id: entry.id,
                agent_id: entry.agent_id,
                user_input: entry.user_input,
                ai_output: entry.ai_output,
                session_id: entry.session_id,
                created_at: (entry.timestamp > 0).then(|| format_timestamp_secs(entry.timestamp)),
            })
            .collect())
    }

    /// Delete one raw memory. Note deletion is `GraphApi::delete_note` —
    /// passing a note path here fails server-side by design.
    pub async fn delete(state: &DashboardState, memory_id: String) -> Result<(), String> {
        state
            .rpc_call("memory.delete", serde_json::json!({ "id": memory_id }))
            .await?;
        Ok(())
    }

    /// List knowledge notes (Layer 2). Returns the page plus the agent's total.
    pub async fn list_facts(
        state: &DashboardState,
        agent_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<(Vec<CompressedFact>, u64), String> {
        let params = serde_json::json!({
            "agent_id": agent_id,
            "limit": limit,
            "offset": offset,
        });

        let result = state.rpc_call("memory.listFacts", params).await?;
        let response: BackendListFactsResponse = serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse memory.listFacts: {e}"))?;

        Ok((response.facts, response.total))
    }

    /// Memory statistics scoped to one agent, so the numbers describe the same
    /// population as the rows shown beneath them.
    pub async fn stats(state: &DashboardState, agent_id: &str) -> Result<MemoryStats, String> {
        let result = state
            .rpc_call("memory.stats", serde_json::json!({ "agent_id": agent_id }))
            .await?;
        serde_json::from_value(result).map_err(|e| format!("Failed to parse memory.stats: {e}"))
    }

    /// Walk a memory claim down (or up) to ground-truth evidence.
    pub async fn trace(
        state: &DashboardState,
        agent_id: &str,
        target: &str,
        kind: TraceKind,
        max_results: usize,
    ) -> Result<TraceResult, String> {
        let params = serde_json::json!({
            "agent_id": agent_id,
            "target": target,
            "kind": kind,
            "max_results": max_results,
        });
        let result = state.rpc_call("memory.trace", params).await?;
        serde_json::from_value(result).map_err(|e| format!("Failed to parse memory.trace: {e}"))
    }
}
```

删掉旧的 `Value` 未使用 import 若 clippy 报警（`stats` 已不再传 `Value::Null`）。

- [ ] **Step 5: 修好所有调用点让它编译**

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
cargo check -p aleph-panel --target wasm32-unknown-unknown 2>&1 | grep -E "^error" -A 4 | head -60
```

按报错逐个修（这些文件在 Task 14-18 会被重写，此处只求编译）：
- `views/memory/mod.rs`：`MemoryApi::search(..)` → `MemoryApi::browse_raw(&state, &agent, query, PAGE_SIZE, page * PAGE_SIZE)`；`MemoryApi::list_facts(..)` 返回元组，改为 `if let Ok((facts, _total)) = ...`；`MemoryApi::stats(&state)` → `MemoryApi::stats(&state, &mem.agent_id.get_untracked())`；`stats.total_graph_nodes` / `total_graph_edges` 现为 `Option<u64>`，显示处改 `.map(|n| n.to_string()).unwrap_or_else(|| "—".to_string())`；`entry.content` → `entry.display_text()`。
- `views/memory/drawer.rs`：`raw.content` → `raw.display_text()`；删掉 `let sim = raw.similarity;` 与整个 similarity 展示块。
- `platform/phone/memory/*`：`MemoryApi::list_facts` 的元组解构（`mod.rs:92`）。

- [ ] **Step 6: 跑测试确认通过**

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
cargo test -p aleph-panel --lib
cargo check -p aleph-panel --target wasm32-unknown-unknown
```
Expected: 测试全 PASS（含新增 4 测），wasm check 干净。

- [ ] **Step 7: 格式化并提交**

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
rustfmt --edition 2021 interfaces/webchat/src/api/memory.rs interfaces/webchat/src/canvas_engine/adapter.rs
git status --short
git add -A interfaces/webchat/src/
git commit -m "panel/api: align the memory DTOs with what the gateway sends

RawMemory keeps user_input and ai_output apart so the card can weight the
halves differently, and gains session_id. CompressedFact gains tags,
link_count and updated_at -- the fields the handler stopped discarding.
MemoryStats graph counts are Option: null means 'store-wide, no honest
single answer'. search -> browse_raw, because it only ever returns raw
rows now; note search is GraphApi::search, and a hit converts straight
into a card via from_search_hit.

Drops RawMemory::similarity: both server branches always sent None, so the
drawer's similarity block could never render."
```

---

### Task 11: i18n 键（en / zh 对称）

**Files:**
- Modify: `interfaces/webchat/locales/en.json`（`memory` 段）
- Modify: `interfaces/webchat/locales/zh.json`（`memory` 段）

**Interfaces:**
- Produces: 下表全部键，供 Task 12–19 消费。`leptos_i18n` 在编译期校验两个 locale 的键集合，缺一边即编译失败 —— 所以对称性由编译器保证，不需要额外测试。

- [ ] **Step 1: 复核待删键确实无消费者**

```bash
for k in col_title col_agent col_type col_date col_content col_actions col_confidence \
         canvas_error_prefix similarity search_no_pagination searching; do
  n=$(grep -rn "memory\.$k\b" interfaces/webchat/src/ | grep -v "settings\.memory\." | wc -l | tr -d ' ')
  echo "$k -> $n consumer(s)"
done
```
Expected: 每个都是 `0`（Task 10 已删 similarity 用点；`col_*` / `searching` / `search_no_pagination` 的唯一消费者是 Task 18 才删的旧表 —— **若此处非 0，把本 Task 挪到 Task 18 之后执行**，否则删键会立刻打断编译）。

> **执行顺序注记**：本 Task 的「删键」部分依赖旧表已被移除。推荐做法 —— Step 2 只**新增**键（立即可做，Task 12+ 需要它们），Step 3 的**删键**留到 Task 18 完成后回来做。两步分开提交。

- [ ] **Step 2: 新增键（en.json）**

在 `interfaces/webchat/locales/en.json` 的 `"memory"` 对象内追加（保持既有键不动，除 `search_placeholder` 改文案）：

```json
    "search_placeholder": "Search notes and raw memory…",
    "search_hint_local": "Filtering loaded notes — press Enter for a full-text search",
    "search_hits_capped": "Showing the top full-text matches; refine the query to narrow it.",
    "facet_search_hits": "Search results",
    "no_search_hits": "No notes match that search.",
    "load_failed": "Couldn't load memories",
    "refresh": "Refresh",
    "scope_agent": "current agent",
    "scope_global": "all agents",
    "graph_scope_unavailable": "per-agent",
    "created": "created",
    "updated": "updated",
    "links": "links",
    "session": "session",
    "raw_question": "Q",
    "raw_answer": "A",
    "copy_link": "Copy link",
    "toast_link_copied": "Link copied",
    "toast_deleted": "Deleted",
    "toast_delete_failed": "Delete failed",
    "toast_saved": "Saved",
    "toast_renamed": "Renamed",
    "toast_copied": "Copied to clipboard",
    "toast_copy_failed": "Copy failed",
    "batch_select_page": "Select page",
    "batch_deselect_page": "Deselect page",
    "batch_copy_md": "Copy as Markdown",
    "batch_copy_cap": "Up to 50 entries per export",
    "batch_exporting": "Exporting…",
    "batch_export_partial": "some entries exported without their body",
    "page_size": "Per page",
    "provenance": "Evidence chain",
    "provenance_empty": "No ground-truth evidence recorded for this note.",
    "provenance_failed": "Couldn't load the evidence chain.",
    "provenance_pruned": "pruned",
    "provenance_via": "via",
    "provenance_capped": "Showing the first 20 pieces of evidence."
```

- [ ] **Step 3: 新增键（zh.json）—— 同一批键，逐一对应**

在 `interfaces/webchat/locales/zh.json` 的 `"memory"` 对象内追加：

```json
    "search_placeholder": "搜索笔记与原始记忆…",
    "search_hint_local": "正在过滤已加载的笔记 —— 按回车做全文搜索",
    "search_hits_capped": "仅显示前若干条全文命中；缩小查询范围可更精确。",
    "facet_search_hits": "搜索结果",
    "no_search_hits": "没有笔记匹配该搜索。",
    "load_failed": "记忆加载失败",
    "refresh": "刷新",
    "scope_agent": "当前 agent",
    "scope_global": "全部 agent",
    "graph_scope_unavailable": "按 agent 统计",
    "created": "创建",
    "updated": "更新",
    "links": "条链接",
    "session": "会话",
    "raw_question": "问",
    "raw_answer": "答",
    "copy_link": "复制链接",
    "toast_link_copied": "链接已复制",
    "toast_deleted": "已删除",
    "toast_delete_failed": "删除失败",
    "toast_saved": "已保存",
    "toast_renamed": "已重命名",
    "toast_copied": "已复制到剪贴板",
    "toast_copy_failed": "复制失败",
    "batch_select_page": "选中本页",
    "batch_deselect_page": "取消选中本页",
    "batch_copy_md": "复制为 Markdown",
    "batch_copy_cap": "每次最多导出 50 条",
    "batch_exporting": "导出中…",
    "batch_export_partial": "部分条目未能取到正文",
    "page_size": "每页",
    "provenance": "溯源证据链",
    "provenance_empty": "这条笔记没有记录到底层证据。",
    "provenance_failed": "证据链加载失败。",
    "provenance_pruned": "已清理",
    "provenance_via": "经由",
    "provenance_capped": "仅显示前 20 条证据。"
```

也把 zh.json 的 `search_placeholder` 原值替换为上面的新文案。

- [ ] **Step 4: 验证编译（键集合对称）**

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
cargo check -p aleph-panel --target wasm32-unknown-unknown
python3 -c "
import json
en = json.load(open('interfaces/webchat/locales/en.json'))['memory']
zh = json.load(open('interfaces/webchat/locales/zh.json'))['memory']
missing_zh = sorted(set(en) - set(zh)); missing_en = sorted(set(zh) - set(en))
print('missing in zh:', missing_zh); print('missing in en:', missing_en)
assert not missing_zh and not missing_en, 'locale key sets diverged'
print('symmetric,', len(en), 'keys')
"
```
Expected: wasm check 干净；脚本打印 `symmetric, N keys`（N ≈ 99）。

- [ ] **Step 5: 提交**

```bash
git add interfaces/webchat/locales/en.json interfaces/webchat/locales/zh.json
git commit -m "panel/i18n: keys for the memory card view, batch bar and evidence chain

search_placeholder said 'Search raw memories' while the box actually ran a
note full-text search -- reworded to match what it now does."
```

- [ ] **Step 6: 【Task 18 之后回来做】删死键**

旧表移除后，从 `en.json` 与 `zh.json` 的 `memory` 段各删这 11 个键：
`col_title` · `col_agent` · `col_type` · `col_date` · `col_content` · `col_actions` · `col_confidence` · `canvas_error_prefix` · `similarity` · `search_no_pagination` · `searching`

（`col_confidence` 与 `canvas_error_prefix` 在本轮之前就已零消费者；其余九个随旧表 / similarity 块一并死亡。注意 `settings.memory.searching` 是**另一个键**，别误删。）

```bash
for k in col_title col_agent col_type col_date col_content col_actions col_confidence \
         canvas_error_prefix similarity search_no_pagination searching; do
  n=$(grep -rn "memory\.$k\b" interfaces/webchat/src/ | grep -v "settings\.memory\." | wc -l | tr -d ' ')
  [ "$n" = "0" ] || echo "STILL USED: $k ($n)"
done
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
cargo check -p aleph-panel --target wasm32-unknown-unknown
git add interfaces/webchat/locales/en.json interfaces/webchat/locales/zh.json
git commit -m "panel/i18n: drop the memory keys the card view retired

Six table-column headers, the raw-table 'Searching…' state, the
'search results are not paginated' note, and three keys
(col_confidence / canvas_error_prefix / similarity) that already had zero
consumers before this refactor."
```

---

## Phase C — Panel UI

### Task 12: pager.rs（Pager + page-size）

**Files:**
- Create: `interfaces/webchat/src/platform/wide/views/memory/pager.rs`
- Modify: `interfaces/webchat/src/platform/wide/views/memory/mod.rs`（删除内联 `Pager` 组件、加 `mod pager;`）
- Modify: `interfaces/webchat/src/platform/wide/views/memory/data.rs`（`PAGE_SIZES` 常量）
- Test: `data.rs` 的 `mod tests`

**Interfaces:**
- Consumes: `data::page_count`
- Produces:
  - `pub const PAGE_SIZES: [u32; 3] = [25, 50, 100];`（在 `data.rs`）
  - `#[component] pub fn Pager(page: RwSignal<u32>, page_size: RwSignal<u32>, total: Signal<Option<u64>>, current_len: Signal<usize>) -> impl IntoView`

- [ ] **Step 1: 写失败测试**

在 `data.rs` 的 `mod tests` 内追加：

```rust
    #[test]
    fn page_sizes_start_at_the_current_default() {
        // 50 was the hardcoded page size; it stays the middle option so the
        // default view is unchanged for existing users.
        assert!(PAGE_SIZES.contains(&PAGE_SIZE));
        assert_eq!(PAGE_SIZES, [25, 50, 100]);
    }

    #[test]
    fn page_count_tracks_the_chosen_page_size() {
        assert_eq!(page_count(120, 25), 5);
        assert_eq!(page_count(120, 50), 3);
        assert_eq!(page_count(120, 100), 2);
    }
```

- [ ] **Step 2: 跑测试确认失败**

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
cargo test -p aleph-panel --lib memory::data
```
Expected: FAIL —— `cannot find value PAGE_SIZES`。

- [ ] **Step 3: 加常量**

在 `data.rs` 的 `PAGE_SIZE` 之后：

```rust
/// Page sizes offered by the pager's selector. `PAGE_SIZE` must appear here so
/// the default is reachable after the user changes it.
pub const PAGE_SIZES: [u32; 3] = [25, 50, 100];
```

- [ ] **Step 4: 跑测试确认通过**

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
cargo test -p aleph-panel --lib memory::data
```
Expected: PASS。

- [ ] **Step 5: 建 pager.rs**

创建 `interfaces/webchat/src/platform/wide/views/memory/pager.rs`：

```rust
//! Pagination controls for the memory console: prev / indicator / next plus a
//! page-size selector. Pure presentation — the parent owns `page` and
//! `page_size` and re-fetches when either changes (R4).

use leptos::prelude::*;

use super::data::{page_count, PAGE_SIZES};
use crate::i18n::{t, t_string, use_i18n};

/// `total` is `None` when the row count is unknown; the next button then falls
/// back to "this page came back full, so there is probably more".
#[component]
#[must_use]
pub fn Pager(
    page: RwSignal<u32>,
    page_size: RwSignal<u32>,
    total: Signal<Option<u64>>,
    current_len: Signal<usize>,
) -> impl IntoView {
    let i18n = use_i18n();

    let total_pages = Signal::derive(move || {
        total
            .get()
            .map(|t| page_count(t as usize, page_size.get()))
    });
    let has_prev = Signal::derive(move || page.get() > 0);
    let has_next = Signal::derive(move || match total_pages.get() {
        Some(tp) => page.get() + 1 < tp,
        None => current_len.get() as u32 >= page_size.get(),
    });

    view! {
        <div class="flex items-center justify-end gap-3 pt-1">
            <label class="flex items-center gap-1.5 text-xs text-text-tertiary">
                <span>{move || t_string!(i18n, memory.page_size).to_string()}</span>
                <select
                    class="rounded-md bg-surface-sunken border border-border px-1.5 py-1 text-xs \
                           text-text-primary focus:outline-none focus:border-primary/60"
                    on:change=move |ev| {
                        if let Ok(v) = event_target_value(&ev).parse::<u32>() {
                            page_size.set(v);
                            // Row N of the old paging is not row N of the new
                            // one; jumping back to the first page is the only
                            // position that means the same thing either way.
                            page.set(0);
                        }
                    }
                >
                    {PAGE_SIZES.iter().map(|n| {
                        let n = *n;
                        view! {
                            <option value=n.to_string() selected=move || page_size.get() == n>
                                {n.to_string()}
                            </option>
                        }
                    }).collect_view()}
                </select>
            </label>

            {move || {
                if !has_prev.get() && !has_next.get() {
                    return ().into_any();
                }
                let indicator = match total_pages.get() {
                    Some(tp) => format!("{} / {}", page.get() + 1, tp),
                    None => format!("{}", page.get() + 1),
                };
                view! {
                    <div class="flex items-center gap-2">
                        <button
                            class="px-3 py-1.5 text-sm rounded-lg border border-border bg-surface-raised \
                                   text-text-secondary hover:text-text-primary hover:bg-surface-sunken \
                                   disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
                            prop:disabled=move || !has_prev.get()
                            on:click=move |_| { let p = page.get(); if p > 0 { page.set(p - 1); } }
                        >
                            {t!(i18n, memory.prev_page)}
                        </button>
                        <span class="px-1 text-sm font-mono text-text-secondary tabular-nums">{indicator}</span>
                        <button
                            class="px-3 py-1.5 text-sm rounded-lg border border-border bg-surface-raised \
                                   text-text-secondary hover:text-text-primary hover:bg-surface-sunken \
                                   disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
                            prop:disabled=move || !has_next.get()
                            on:click=move |_| { if has_next.get() { page.set(page.get() + 1); } }
                        >
                            {t!(i18n, memory.next_page)}
                        </button>
                    </div>
                }.into_any()
            }}
        </div>
    }
}
```

- [ ] **Step 6: 接入并删旧 Pager**

`views/memory/mod.rs`：
- 顶部 `mod` 声明区加 `mod pager;`，并 `use pager::Pager;`
- 删掉文件内 `// ─── Pagination ───` 到 `fn Pager` 结束的整个内联组件（原 377-423 行）
- 新增 `let page_size = RwSignal::new(PAGE_SIZE);`，两处 `<Pager .../>` 调用补 `page_size=page_size`
- 所有 `PAGE_SIZE` 的取数处（`browse_raw` 的 limit/offset、`page_slice`）改用 `page_size.get()`

- [ ] **Step 7: 验证**

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
cargo check -p aleph-panel --target wasm32-unknown-unknown
cargo test -p aleph-panel --lib memory::
```
Expected: 干净 + 全 PASS。

- [ ] **Step 8: 格式化并提交**

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
rustfmt --edition 2021 interfaces/webchat/src/platform/wide/views/memory/pager.rs \
  interfaces/webchat/src/platform/wide/views/memory/data.rs
git status --short
git add -A interfaces/webchat/src/platform/wide/views/memory/
git commit -m "panel/memory: extract the pager and give it a page-size selector

Changing page size resets to page 0: row N under 25-per-page is not row N
under 100, so the first page is the only position that survives the change
meaning the same thing."
```

---

### Task 13: toast.rs

**Files:**
- Create: `interfaces/webchat/src/platform/wide/views/memory/toast.rs`
- Modify: `views/memory/mod.rs`（`mod toast;`）

**Interfaces:**
- Produces:
  - `pub enum ToastKind { Success, Error }`
  - `pub struct ToastMsg { pub text: String, pub kind: ToastKind }`
  - `pub type ToastSlot = RwSignal<Option<ToastMsg>>;`
  - `pub fn push_toast(slot: ToastSlot, text: String, kind: ToastKind)`（自动 2.4s 后清空）
  - `#[component] pub fn ToastHost(slot: ToastSlot) -> impl IntoView`
- 后续 Task 依赖：`cards.rs`（Task 14）、`batch_bar.rs`（Task 17）、`mod.rs`（Task 18）、`drawer.rs`（Task 19）

- [ ] **Step 1: 建 toast.rs**

```rust
//! Transient action feedback for the memory console.
//!
//! Module-private on purpose. The two other things in this panel called
//! "toast" (`settings/channels/config_template.rs`,
//! `components/extensions/install_flow.rs`) are inline banners with a
//! different shape and lifetime; abstracting across them now would be
//! speculative. Lift this into `components/ui/` when a second real consumer
//! shows up, not before.

use leptos::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Success,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToastMsg {
    pub text: String,
    pub kind: ToastKind,
}

/// A single-slot toast holder. A newer message replaces an older one rather
/// than stacking: these are confirmations of the user's own click, so the most
/// recent one is the only one still relevant.
pub type ToastSlot = RwSignal<Option<ToastMsg>>;

/// How long a toast stays up.
const TOAST_MS: u64 = 2_400;

/// Show `text`, clearing it after [`TOAST_MS`].
///
/// The timer is keyed on the message identity: if another toast replaces this
/// one before the timeout fires, the stale timer must not blank the new
/// message, so it checks before clearing.
pub fn push_toast(slot: ToastSlot, text: String, kind: ToastKind) {
    let msg = ToastMsg { text, kind };
    slot.set(Some(msg.clone()));
    set_timeout(
        move || {
            if slot.get_untracked().as_ref() == Some(&msg) {
                slot.set(None);
            }
        },
        std::time::Duration::from_millis(TOAST_MS),
    );
}

#[component]
#[must_use]
pub fn ToastHost(slot: ToastSlot) -> impl IntoView {
    view! {
        {move || slot.get().map(|m| {
            let tone = match m.kind {
                ToastKind::Success => "bg-success-subtle text-success border-success/30",
                ToastKind::Error => "bg-danger-subtle text-danger border-danger/30",
            };
            view! {
                <div
                    class=format!(
                        "fixed bottom-6 left-1/2 -translate-x-1/2 z-50 animate-pop-in \
                         rounded-lg border px-4 py-2 text-sm shadow-lg {tone}"
                    )
                    role="status"
                    aria-live="polite"
                >
                    {m.text}
                </div>
            }
        })}
    }
}
```

- [ ] **Step 2: 声明模块并验证编译**

`views/memory/mod.rs` 加 `mod toast;`，并 `use toast::{push_toast, ToastHost, ToastKind, ToastMsg};`（暂时可能触发 unused 警告，Task 18 接入后消失；若 clippy `-D warnings` 拦下，先只加 `mod toast;` 不 `use`）。

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
cargo check -p aleph-panel --target wasm32-unknown-unknown
```
Expected: 干净。若报 `set_timeout` 未找到，加 `use leptos::leptos_dom::helpers::set_timeout;`（`views/settings/generation.rs:103` 是同款调用，照它的 import 抄）。若报 `animate-pop-in` 不存在 —— 那是 CSS 类，编译不校验；`grep -n "animate-pop-in" interfaces/webchat/styles/tailwind.css` 确认它存在（`memory_hub/sidebar.rs:57` 已在用）。

- [ ] **Step 3: 提交**

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
rustfmt --edition 2021 interfaces/webchat/src/platform/wide/views/memory/toast.rs
git status --short
git add -A interfaces/webchat/src/platform/wide/views/memory/
git commit -m "panel/memory: module-private toast slot

Deleting a memory, saving a note and copying to the clipboard were all
silent. Single-slot rather than a stack: these confirm the user's own
click, so only the latest one still matters. Kept inside the memory module
-- the panel's two other 'toasts' are inline banners with a different
shape, and abstracting across them now would be speculative."
```

---

### Task 14: cards.rs（NoteCard / RawCard / CardList 三态）

**Files:**
- Create: `interfaces/webchat/src/platform/wide/views/memory/cards.rs`
- Modify: `views/memory/mod.rs`（`mod cards;`）

**Interfaces:**
- Consumes: `data::{Loadable, format_ts}` · `api::{CompressedFact, RawMemory}` · `canvas_engine::category_color::category_color` · `components::ui::Badge`
- Produces:
  - `#[component] pub fn NoteCard(fact: CompressedFact, checked: Signal<bool>, highlighted: Signal<bool>, on_toggle: impl Fn() + Clone + Send + 'static, on_open: impl Fn() + Clone + Send + 'static, on_locate: impl Fn() + Clone + Send + 'static, on_copy_link: impl Fn() + Clone + Send + 'static) -> impl IntoView`
  - `#[component] pub fn RawCard(raw: RawMemory, checked: Signal<bool>, on_toggle: impl Fn() + Clone + Send + 'static, on_open: impl Fn() + Clone + Send + 'static, on_delete: impl Fn() + Clone + Send + 'static) -> impl IntoView`
  - `#[component] pub fn CardListShell(state: Signal<Loadable<usize>>, empty_label: Signal<String>, on_retry: impl Fn() + Clone + Send + 'static, children: ChildrenFn) -> impl IntoView` —— 三态外壳：`Loading` 画骨架、`Failed` 画错误+Retry、`Ready(0)` 画空态、`Ready(n>0)` 画 `children`
- 后续 Task 依赖：`mod.rs`（Task 18）

- [ ] **Step 1: 建 cards.rs**

```rust
//! Card renderers for the memory console.
//!
//! Replaces the two HTML tables this view used to carry. Tables clamped every
//! row to one or two lines and overflowed on narrow windows; a card can show
//! the fields `memory.listFacts` stopped discarding (tags, link count, both
//! timestamps) and, for raw rows, both halves of a turn.
//!
//! Pure presentation: every mutation is a callback the parent owns (R4).

use leptos::prelude::*;

use super::data::{format_ts, Loadable};
use crate::api::{CompressedFact, RawMemory};
use crate::canvas_engine::category_color::category_color;
use crate::components::ui::{Badge, BadgeVariant};
use crate::i18n::{t, t_string, use_i18n};

/// Category → badge tone. Mirrors the stripe colour but in the panel's badge
/// vocabulary, so a note reads the same in a card as it does in the galaxy.
fn category_variant(category: &str) -> BadgeVariant {
    match category {
        "preference" | "personal" => BadgeVariant::Indigo,
        "learning" | "lesson" | "skill" | "goal-lessons" => BadgeVariant::Emerald,
        "plan" | "project" => BadgeVariant::Amber,
        "feedback" => BadgeVariant::Red,
        _ => BadgeVariant::Slate,
    }
}

// ─── Three-state shell ──────────────────────────────────────────────────────

/// Wraps a card list with its loading / failed / empty states.
///
/// `state` carries the row count so the shell can tell "loaded and empty" from
/// "still loading" from "failed" — the distinction the old table could not make,
/// because a failed fetch and an empty store both rendered as "No facts".
#[component]
#[must_use]
pub fn CardListShell(
    state: Signal<Loadable<usize>>,
    empty_label: Signal<String>,
    on_retry: impl Fn() + Clone + Send + 'static,
    children: ChildrenFn,
) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        {move || match state.get() {
            Loadable::Loading => view! {
                <div class="space-y-2" aria-busy="true">
                    {(0..5).map(|_| view! {
                        <div class="h-[82px] rounded-xl bg-surface-sunken animate-pulse"></div>
                    }).collect_view()}
                </div>
            }.into_any(),

            Loadable::Failed(err) => {
                let on_retry = on_retry.clone();
                view! {
                    <div class="rounded-xl border border-danger/20 bg-danger-subtle p-6 flex items-start gap-4">
                        <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                             stroke-width="2" stroke-linecap="round" stroke-linejoin="round"
                             class="text-danger flex-shrink-0 mt-0.5">
                            <circle cx="12" cy="12" r="10" />
                            <line x1="12" y1="8" x2="12" y2="12" />
                            <line x1="12" y1="16" x2="12.01" y2="16" />
                        </svg>
                        <div class="min-w-0">
                            <h3 class="text-danger font-semibold mb-1">{t!(i18n, memory.load_failed)}</h3>
                            <p class="text-xs text-text-secondary break-words font-mono">{err}</p>
                            <button
                                class="mt-3 px-3 py-1 text-xs rounded-lg border border-border \
                                       text-text-secondary hover:text-text-primary"
                                on:click=move |_| on_retry()
                            >
                                {t!(i18n, memory.retry)}
                            </button>
                        </div>
                    </div>
                }.into_any()
            }

            Loadable::Ready(0) => view! {
                <div class="rounded-xl border border-border-subtle bg-surface-raised p-10 text-center">
                    <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                         stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"
                         class="mx-auto mb-3 text-text-tertiary">
                        <ellipse cx="12" cy="5" rx="9" ry="3" />
                        <path d="M21 12c0 1.66-4 3-9 3s-9-1.34-9-3" />
                        <path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5" />
                    </svg>
                    <p class="text-sm text-text-tertiary">{move || empty_label.get()}</p>
                </div>
            }.into_any(),

            Loadable::Ready(_) => view! { <div class="space-y-2">{children()}</div> }.into_any(),
        }}
    }
}

// ─── Note card ──────────────────────────────────────────────────────────────

#[component]
#[must_use]
pub fn NoteCard(
    fact: CompressedFact,
    checked: Signal<bool>,
    highlighted: Signal<bool>,
    on_toggle: impl Fn() + Clone + Send + 'static,
    on_open: impl Fn() + Clone + Send + 'static,
    on_locate: impl Fn() + Clone + Send + 'static,
    on_copy_link: impl Fn() + Clone + Send + 'static,
) -> impl IntoView {
    let i18n = use_i18n();
    let stripe = category_color(&fact.category);
    let variant = category_variant(&fact.category);

    let title = fact.content.clone();
    let path = fact.path.clone();
    let agent_id = fact.agent_id.clone();
    let category = fact.category.clone();
    let tags = fact.tags.clone();
    let link_count = fact.link_count;
    let created = format_ts(fact.created_at);
    // Only show "updated" when it actually differs — a note written once
    // carries updated_at == created_at and repeating it is noise.
    let updated = (fact.updated_at > fact.created_at).then(|| format_ts(fact.updated_at));

    view! {
        <div
            class=move || {
                let base = "group relative flex items-start gap-3 rounded-xl border bg-surface-raised \
                            p-4 pl-5 cursor-pointer transition-colors";
                if highlighted.get() {
                    format!("{base} border-primary/50 ring-1 ring-primary/30")
                } else {
                    format!("{base} border-border-subtle hover:bg-surface-sunken")
                }
            }
            on:click={
                let on_open = on_open.clone();
                move |_| on_open()
            }
        >
            <div
                class="absolute left-0 top-3 bottom-3 w-[3px] rounded-full"
                style=format!("background:{stripe}")
            ></div>

            <input
                type="checkbox"
                class="mt-0.5 cursor-pointer flex-shrink-0"
                aria-label=t_string!(i18n, memory.batch_selected)
                prop:checked=move || checked.get()
                on:click=move |ev| ev.stop_propagation()
                on:change=move |_| on_toggle()
            />

            <div class="min-w-0 flex-1">
                <div class="text-sm font-medium text-text-primary break-words">{title}</div>
                <div class="text-xs text-text-tertiary font-mono mt-0.5 break-all">{path}</div>

                <div class="flex items-center gap-1.5 flex-wrap mt-2">
                    <Badge variant=variant>{category}</Badge>
                    <Badge variant=BadgeVariant::Indigo>{agent_id}</Badge>
                    {tags.into_iter().map(|tag| view! {
                        <span class="px-1.5 py-0.5 rounded text-[10px] font-medium bg-surface-sunken \
                                     text-text-secondary border border-border">
                            {tag}
                        </span>
                    }).collect_view()}
                    {(link_count > 0).then(|| view! {
                        <span class="inline-flex items-center gap-1 text-[10px] text-text-tertiary font-mono tabular-nums">
                            <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                                 stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                <path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71" />
                                <path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71" />
                            </svg>
                            {link_count.to_string()}" "{move || t_string!(i18n, memory.links).to_string()}
                        </span>
                    })}
                    <span class="text-[10px] text-text-tertiary font-mono tabular-nums">
                        {move || t_string!(i18n, memory.created).to_string()}" "{created.clone()}
                        {updated.clone().map(|u| format!(" · {} {u}", t_string!(i18n, memory.updated)))}
                    </span>
                </div>
            </div>

            <div class="flex items-center gap-1 flex-shrink-0 opacity-0 group-hover:opacity-100 transition-opacity">
                <button
                    class="p-1.5 rounded text-text-tertiary hover:text-primary"
                    title=t_string!(i18n, memory.copy_link)
                    on:click=move |ev| { ev.stop_propagation(); on_copy_link(); }
                >
                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                         stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                        <path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71" />
                        <path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71" />
                    </svg>
                </button>
                <button
                    class="p-1.5 rounded text-text-tertiary hover:text-primary"
                    title=t_string!(i18n, memory.view_in_graph)
                    on:click=move |ev| { ev.stop_propagation(); on_locate(); }
                >
                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                         stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                        <circle cx="5" cy="6" r="2" /><circle cx="19" cy="6" r="2" /><circle cx="12" cy="18" r="2" />
                        <line x1="6.6" y1="7.4" x2="10.6" y2="16.4" />
                        <line x1="17.4" y1="7.4" x2="13.4" y2="16.4" />
                        <line x1="7" y1="6" x2="17" y2="6" />
                    </svg>
                </button>
            </div>
        </div>
    }
}

// ─── Raw card ───────────────────────────────────────────────────────────────

#[component]
#[must_use]
pub fn RawCard(
    raw: RawMemory,
    checked: Signal<bool>,
    on_toggle: impl Fn() + Clone + Send + 'static,
    on_open: impl Fn() + Clone + Send + 'static,
    on_delete: impl Fn() + Clone + Send + 'static,
) -> impl IntoView {
    let i18n = use_i18n();
    let confirm = RwSignal::new(false);

    let agent_id = raw.agent_id.clone();
    let session_id = raw.session_id.clone();
    let created_at = raw
        .created_at
        .clone()
        .unwrap_or_else(|| "\u{2014}".to_string());
    let question = (!raw.user_input.is_empty()).then(|| raw.user_input.clone());
    let answer = (!raw.ai_output.is_empty()).then(|| raw.ai_output.clone());

    view! {
        <div
            class="group flex items-start gap-3 rounded-xl border border-border-subtle bg-surface-raised \
                   p-4 cursor-pointer hover:bg-surface-sunken transition-colors"
            on:click={
                let on_open = on_open.clone();
                move |_| on_open()
            }
        >
            <input
                type="checkbox"
                class="mt-0.5 cursor-pointer flex-shrink-0"
                aria-label=t_string!(i18n, memory.batch_selected)
                prop:checked=move || checked.get()
                on:click=move |ev| ev.stop_propagation()
                on:change=move |_| on_toggle()
            />

            <div class="min-w-0 flex-1 space-y-1">
                {question.map(|q| view! {
                    <div class="text-sm text-text-primary line-clamp-2 break-words">
                        <span class="mr-1.5 text-[10px] font-bold uppercase text-primary">
                            {move || t_string!(i18n, memory.raw_question).to_string()}
                        </span>
                        {q}
                    </div>
                })}
                {answer.map(|a| view! {
                    <div class="text-sm text-text-secondary line-clamp-2 break-words">
                        <span class="mr-1.5 text-[10px] font-bold uppercase text-success">
                            {move || t_string!(i18n, memory.raw_answer).to_string()}
                        </span>
                        {a}
                    </div>
                })}
                <div class="flex items-center gap-2 flex-wrap pt-1">
                    <Badge variant=BadgeVariant::Indigo>{agent_id}</Badge>
                    {session_id.map(|s| view! {
                        <span class="text-[10px] text-text-tertiary font-mono">
                            {move || t_string!(i18n, memory.session).to_string()}" "{s}
                        </span>
                    })}
                    <span class="text-[10px] text-text-tertiary font-mono tabular-nums">{created_at}</span>
                </div>
            </div>

            <div class="flex-shrink-0" on:click=move |ev| ev.stop_propagation()>
                {move || if confirm.get() {
                    let on_delete = on_delete.clone();
                    view! {
                        <ConfirmButton
                            confirming=confirm
                            on_confirm=move || on_delete()
                            size_class="px-2.5 py-1 text-xs"
                            stop_propagation=true
                        />
                    }.into_any()
                } else {
                    view! {
                        <button
                            class="p-1.5 rounded text-text-tertiary opacity-0 group-hover:opacity-100 \
                                   hover:text-danger transition-all"
                            title=t_string!(i18n, common.delete)
                            on:click=move |_| confirm.set(true)
                        >
                            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                                 stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                <polyline points="3 6 5 6 21 6" />
                                <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" />
                            </svg>
                        </button>
                    }.into_any()
                }}
            </div>
        </div>
    }
}
```

在 imports 里补 `use crate::components::ui::ConfirmButton;`。

- [ ] **Step 2: 声明模块并验证编译**

`views/memory/mod.rs` 加 `mod cards;`。

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
cargo check -p aleph-panel --target wasm32-unknown-unknown 2>&1 | grep -E "^(error|warning: unused)" -A 5 | head -50
```
Expected: 无 error。常见需要现场修正的三处：
1. `ChildrenFn` 未导入 → `use leptos::prelude::*` 已含；若报错改用 `Children` 并把 `CardListShell` 的 `children()` 只调一次（`Ready(_)` 臂）。因为 `match` 的其它臂不调 children，`Children`（FnOnce）也可行，但 `ChildrenFn` 更安全。
2. `Badge` 的 children 需要 `String` 而非 `&str` —— 现有用法 `<Badge variant=..>{agent_id}</Badge>` 传的是 `String`，照抄即可。
3. `ConfirmButton` 的 `on_confirm` 是泛型 `F: Fn() + ...`；`move || on_delete()` 需要 `on_delete` 可 `Clone`（签名已声明 `Clone`）。

- [ ] **Step 3: 提交**

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
rustfmt --edition 2021 interfaces/webchat/src/platform/wide/views/memory/cards.rs
git status --short
git add -A interfaces/webchat/src/platform/wide/views/memory/
git commit -m "panel/memory: card renderers with a three-state shell

CardListShell carries Loadable<usize>, so 'still loading', 'failed with
this message', and 'loaded and genuinely empty' are three different
renders. The old table collapsed the first two into 'No facts'.

Note cards show what listFacts stopped discarding -- category, tags, link
count, and an 'updated' stamp only when it differs from 'created'. Raw
cards show both halves of a turn plus its session, instead of clamping a
pre-joined 'Q: ...\\nA: ...' string to one line."
```

---

### Task 15: loader.rs（三条取数收敛成 Loadable）

**Files:**
- Create: `interfaces/webchat/src/platform/wide/views/memory/loader.rs`
- Modify: `views/memory/mod.rs`（`mod loader;`）

**Interfaces:**
- Consumes: `data::Loadable` · `MemoryApi::{list_facts, browse_raw}` · `GraphApi::search` · `CompressedFact::from_search_hit`
- Produces:
  - `pub struct NotesWindow { pub facts: Vec<CompressedFact>, pub total: u64 }`
  - `pub fn load_notes(state: DashboardState, agent: String, limit: usize, slot: RwSignal<Loadable<NotesWindow>>)`
  - `pub fn load_raw(state: DashboardState, agent: String, query: String, limit: u32, offset: u32, slot: RwSignal<Loadable<Vec<RawMemory>>>)`
  - `pub fn load_search_hits(state: DashboardState, agent: String, query: String, limit: usize, slot: RwSignal<Loadable<Vec<CompressedFact>>>)`
  - `pub fn load_stats(state: DashboardState, agent: String, slot: RwSignal<Loadable<MemoryStats>>)`
- 后续 Task 依赖：`mod.rs`（Task 18）

- [ ] **Step 1: 建 loader.rs**

```rust
//! The memory console's four fetches, each landing in a `Loadable` slot.
//!
//! Every one of these used to be an inline `Effect` doing `if let Ok(v) = ...`,
//! which turned a gateway error into an empty list. Routing them all through
//! `Loadable::from_rpc` means the error text survives to the renderer.
//!
//! Each function sets its slot to `Loading` before awaiting, so a slow refetch
//! shows skeletons rather than stale rows presented as current.

use leptos::prelude::*;
use leptos::task::spawn_local;

use super::data::Loadable;
use crate::api::graph::GraphApi;
use crate::api::{CompressedFact, MemoryApi, MemoryStats, RawMemory};
use crate::context::DashboardState;

/// One `list_facts` page plus the agent's total note count.
#[derive(Debug, Clone, PartialEq)]
pub struct NotesWindow {
    pub facts: Vec<CompressedFact>,
    /// Total notes for this agent, independent of the window cap — lets the
    /// pager size itself and the truncation notice tell the truth.
    pub total: u64,
}

pub fn load_notes(
    state: DashboardState,
    agent: String,
    limit: usize,
    slot: RwSignal<Loadable<NotesWindow>>,
) {
    slot.set(Loadable::Loading);
    spawn_local(async move {
        let res = MemoryApi::list_facts(&state, &agent, limit, 0)
            .await
            .map(|(facts, total)| NotesWindow { facts, total });
        slot.set(Loadable::from_rpc(res));
    });
}

pub fn load_raw(
    state: DashboardState,
    agent: String,
    query: String,
    limit: u32,
    offset: u32,
    slot: RwSignal<Loadable<Vec<RawMemory>>>,
) {
    slot.set(Loadable::Loading);
    spawn_local(async move {
        let res = MemoryApi::browse_raw(&state, &agent, query, limit, offset).await;
        slot.set(Loadable::from_rpc(res));
    });
}

/// Server-side note full-text search. Hits arrive as full index rows, so they
/// convert straight into the note card model with no follow-up round trip.
pub fn load_search_hits(
    state: DashboardState,
    agent: String,
    query: String,
    limit: usize,
    slot: RwSignal<Loadable<Vec<CompressedFact>>>,
) {
    slot.set(Loadable::Loading);
    spawn_local(async move {
        let res = GraphApi::search(&state, &agent, &query, limit)
            .await
            .map(|r| {
                r.results
                    .iter()
                    .map(CompressedFact::from_search_hit)
                    .collect::<Vec<_>>()
            });
        slot.set(Loadable::from_rpc(res));
    });
}

pub fn load_stats(state: DashboardState, agent: String, slot: RwSignal<Loadable<MemoryStats>>) {
    slot.set(Loadable::Loading);
    spawn_local(async move {
        let res = MemoryApi::stats(&state, &agent).await;
        slot.set(Loadable::from_rpc(res));
    });
}
```

- [ ] **Step 2: 验证编译**

`views/memory/mod.rs` 加 `mod loader;`。

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
cargo check -p aleph-panel --target wasm32-unknown-unknown
```
Expected: 干净。若 `Loadable<MemoryStats>` 报 `PartialEq` 缺失 —— `MemoryStats` 未 derive `PartialEq`；给它加 `PartialEq`（`api/memory.rs`），或把 `Loadable` 的 derive 降为 `#[derive(Debug, Clone)]` 并在 Task 8 的测试里用 `matches!` + 字段断言替代 `assert_eq!`。**优先给 `MemoryStats` / `RawMemory` / `CompressedFact` 加 `PartialEq`** —— 它们都是纯数据 DTO，加了还能让 Leptos 的信号相等性检查少做无谓重渲染。

- [ ] **Step 3: 提交**

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
rustfmt --edition 2021 interfaces/webchat/src/platform/wide/views/memory/loader.rs
git status --short
git add -A interfaces/webchat/src/
git commit -m "panel/memory: route all four fetches through Loadable

Each was an inline Effect doing if-let-Ok, so a gateway error became an
empty list. Every fetch now sets Loading before awaiting and lands in a
Loadable slot, so a slow refetch shows skeletons instead of presenting
stale rows as current, and a failure carries its message to the renderer."
```

---

### Task 16: facets.rs（层 chips + 条件 SearchHits）

**Files:**
- Modify: `interfaces/webchat/src/platform/wide/views/memory/facets.rs`

**Interfaces:**
- Consumes: `data::MemoryFacet`
- Produces: `#[component] pub fn FacetBar(active: RwSignal<MemoryFacet>, counts: Signal<[usize; 4]>, raw_count: Signal<Option<u64>>, hits_count: Signal<Option<usize>>, on_select: Callback<MemoryFacet>) -> impl IntoView`
  —— `hits_count` 为 `None` 时不渲染 SearchHits chip

- [ ] **Step 1: 改写 facets.rs**

保留现有 `FacetChip` 组件不动，替换 `FacetBar`：

```rust
/// Facet bar. `counts` = `[AllNotes, Facts, Feedback, Lessons]` over the loaded
/// window; `raw_count` = the agent's raw total; `hits_count` = number of
/// server-side search hits, or `None` when no search is active.
///
/// The hits chip appears only while a search is live. Leaving an empty
/// "Search results 0" chip behind after the box is cleared would imply the
/// store had been searched and found wanting.
#[component]
pub fn FacetBar(
    active: RwSignal<MemoryFacet>,
    counts: Signal<[usize; 4]>,
    raw_count: Signal<Option<u64>>,
    hits_count: Signal<Option<usize>>,
    on_select: Callback<MemoryFacet>,
) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div class="flex items-center gap-1 flex-wrap">
            <FacetChip
                facet=MemoryFacet::AllNotes active=active on_select=on_select
                label=t_string!(i18n, memory.facet_all_notes).to_string()
                badge=Signal::derive(move || counts.get()[0].to_string())
            />
            <FacetChip
                facet=MemoryFacet::Facts active=active on_select=on_select
                label=t_string!(i18n, memory.facet_facts).to_string()
                badge=Signal::derive(move || counts.get()[1].to_string())
            />
            <FacetChip
                facet=MemoryFacet::Feedback active=active on_select=on_select
                label=t_string!(i18n, memory.facet_feedback).to_string()
                badge=Signal::derive(move || counts.get()[2].to_string())
            />
            <FacetChip
                facet=MemoryFacet::Lessons active=active on_select=on_select
                label=t_string!(i18n, memory.facet_lessons).to_string()
                badge=Signal::derive(move || counts.get()[3].to_string())
            />
            <span class="mx-1 text-border select-none">"|"</span>
            <FacetChip
                facet=MemoryFacet::Raw active=active on_select=on_select
                label=t_string!(i18n, memory.facet_raw).to_string()
                badge=Signal::derive(move || raw_count.get().map(|c| c.to_string()).unwrap_or_default())
            />
            <Show when=move || hits_count.get().is_some()>
                <span class="mx-1 text-border select-none">"|"</span>
                <FacetChip
                    facet=MemoryFacet::SearchHits active=active on_select=on_select
                    label=t_string!(i18n, memory.facet_search_hits).to_string()
                    badge=Signal::derive(move || hits_count.get().map(|c| c.to_string()).unwrap_or_default())
                />
            </Show>
        </div>
    }
}
```

- [ ] **Step 2: 验证编译**

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
cargo check -p aleph-panel --target wasm32-unknown-unknown 2>&1 | grep -E "^error" -A 5 | head -20
```
Expected: 只剩 `mod.rs` 里 `FacetBar` 缺 `hits_count` 参数的 error（Task 18 修）。此处可临时在 `mod.rs` 的调用处补 `hits_count=Signal::derive(|| None)` 让编译过。

- [ ] **Step 3: 提交**

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
rustfmt --edition 2021 interfaces/webchat/src/platform/wide/views/memory/facets.rs
git status --short
git add -A interfaces/webchat/src/platform/wide/views/memory/
git commit -m "panel/memory: conditional search-results facet chip

The chip appears only while a search is live. A leftover 'Search results 0'
after clearing the box would claim the store had been searched and found
wanting."
```

---

### Task 17: batch_bar.rs

**Files:**
- Create: `interfaces/webchat/src/platform/wide/views/memory/batch_bar.rs`
- Modify: `views/memory/mod.rs`（`mod batch_bar;`）

**Interfaces:**
- Consumes: `data::EXPORT_MAX` · `toast::{push_toast, ToastKind, ToastSlot}` · `components::ui::{Button, ButtonSize, ButtonVariant, ConfirmButton}`
- Produces: `#[component] pub fn BatchBar(selected: RwSignal<HashSet<String>>, page_ids: Signal<Vec<String>>, exporting: RwSignal<Option<(usize, usize)>>, on_copy_md: impl Fn() + Clone + Send + 'static, on_delete: impl Fn() + Clone + Send + 'static) -> impl IntoView`
  —— `exporting` 为 `Some((done, total))` 时显示进度并禁用按钮
- 后续 Task 依赖：`mod.rs`（Task 18）

- [ ] **Step 1: 建 batch_bar.rs**

```rust
//! Bulk-action bar for the memory console.
//!
//! Sits ABOVE the list, not fixed to the viewport bottom. A bottom-fixed bar
//! covers the pager and, on a short window, puts "select page" out of reach —
//! the reference implementation (memos `MemoriesView`) documents having had to
//! back that out.

use std::collections::HashSet;

use leptos::prelude::*;

use super::data::EXPORT_MAX;
use crate::components::ui::{Button, ButtonSize, ButtonVariant, ConfirmButton};
use crate::i18n::{t, t_string, use_i18n};

#[component]
#[must_use]
pub fn BatchBar(
    selected: RwSignal<HashSet<String>>,
    /// Ids currently rendered, for the select-page toggle.
    page_ids: Signal<Vec<String>>,
    /// `Some((done, total))` while a clipboard export is in flight.
    exporting: RwSignal<Option<(usize, usize)>>,
    on_copy_md: impl Fn() + Clone + Send + 'static,
    on_delete: impl Fn() + Clone + Send + 'static,
) -> impl IntoView {
    let i18n = use_i18n();
    let confirm = RwSignal::new(false);

    let count = Signal::derive(move || selected.get().len());
    let page_all_selected = Signal::derive(move || {
        let sel = selected.get();
        let ids = page_ids.get();
        !ids.is_empty() && ids.iter().all(|id| sel.contains(id))
    });
    let over_cap = Signal::derive(move || count.get() > EXPORT_MAX);
    let busy = Signal::derive(move || exporting.get().is_some());

    view! {
        <Show when=move || count.get() > 0>
            <div
                class="flex items-center justify-between gap-3 flex-wrap rounded-lg border border-border \
                       bg-surface-sunken px-4 py-2"
                role="region"
                aria-label="bulk actions"
            >
                <span class="text-sm text-text-secondary tabular-nums">
                    {move || count.get().to_string()}" "{t!(i18n, memory.batch_selected)}
                </span>

                <div class="flex items-center gap-2 flex-wrap">
                    <button
                        class="px-3 py-1 text-xs rounded-lg border border-border text-text-secondary \
                               hover:text-text-primary transition-colors"
                        on:click=move |_| {
                            let ids = page_ids.get_untracked();
                            let all_in = page_all_selected.get_untracked();
                            selected.update(|s| {
                                for id in ids {
                                    if all_in { s.remove(&id); } else { s.insert(id); }
                                }
                            });
                        }
                    >
                        {move || if page_all_selected.get() {
                            t_string!(i18n, memory.batch_deselect_page).to_string()
                        } else {
                            t_string!(i18n, memory.batch_select_page).to_string()
                        }}
                    </button>

                    <div class="flex items-center gap-1.5">
                        <button
                            class="px-3 py-1 text-xs rounded-lg border border-border text-text-secondary \
                                   hover:text-text-primary disabled:opacity-40 \
                                   disabled:cursor-not-allowed transition-colors"
                            prop:disabled=move || over_cap.get() || busy.get()
                            on:click={
                                let on_copy_md = on_copy_md.clone();
                                move |_| on_copy_md()
                            }
                        >
                            {move || match exporting.get() {
                                Some((done, total)) => format!(
                                    "{} {done}/{total}",
                                    t_string!(i18n, memory.batch_exporting)
                                ),
                                None => t_string!(i18n, memory.batch_copy_md).to_string(),
                            }}
                        </button>
                        // A cap the user cannot see is a cap that looks like a bug.
                        <Show when=move || over_cap.get()>
                            <span class="text-[10px] text-warning">
                                {move || t_string!(i18n, memory.batch_copy_cap).to_string()}
                            </span>
                        </Show>
                    </div>

                    {move || if confirm.get() {
                        let on_delete = on_delete.clone();
                        view! {
                            <ConfirmButton
                                confirming=confirm
                                on_confirm=move || on_delete()
                                size_class="px-3 py-1 text-xs"
                            />
                        }.into_any()
                    } else {
                        view! {
                            <Button
                                variant=ButtonVariant::Destructive
                                size=ButtonSize::Sm
                                class="px-3 py-1 text-xs".to_string()
                                on:click=move |_| confirm.set(true)
                            >
                                {t!(i18n, memory.batch_delete)}
                            </Button>
                        }.into_any()
                    }}

                    <button
                        class="text-xs text-text-tertiary hover:text-text-secondary"
                        on:click=move |_| { selected.set(HashSet::new()); confirm.set(false); }
                    >
                        {t!(i18n, memory.batch_clear)}
                    </button>
                </div>
            </div>
        </Show>
    }
}
```

- [ ] **Step 2: 验证编译**

`views/memory/mod.rs` 加 `mod batch_bar;`。

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
cargo check -p aleph-panel --target wasm32-unknown-unknown 2>&1 | grep -E "^error" -A 5 | head -30
```
Expected: 无 error（`BatchBar` 尚无调用方，但组件本身要能编译）。若报 `Button` 的 `on:click` 用法 —— 照 `views/memory/mod.rs:565` 现有写法抄。

- [ ] **Step 3: 提交**

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
rustfmt --edition 2021 interfaces/webchat/src/platform/wide/views/memory/batch_bar.rs
git status --short
git add -A interfaces/webchat/src/platform/wide/views/memory/
git commit -m "panel/memory: bulk-action bar above the list

Above, not fixed to the viewport bottom: a bottom bar covers the pager and
on a short window puts 'select page' out of reach. The export cap is
rendered next to the disabled button -- a limit the user cannot see reads
as a broken button."
```

---

### Task 18: mod.rs 编排重写（双轨搜索 / 深链 / 刷新 / 删旧表）

**Files:**
- Rewrite: `interfaces/webchat/src/platform/wide/views/memory/mod.rs`（695 行 → ~200 行）
- Modify: `interfaces/webchat/src/platform/wide/views/memory_hub/sidebar.rs`（搜索框加本地过滤提示）

**Interfaces:**
- Consumes: Task 8–17 的全部产物
- Produces: `#[component] pub fn Memory() -> impl IntoView`（对外签名不变，`memory_hub/mod.rs` 无需改动）

- [ ] **Step 1: 删除被取代的旧组件**

从 `views/memory/mod.rs` 删掉这四段（它们已被 `cards.rs` / `pager.rs` 取代）：
- `// ─── Notes Table ───` 到 `fn NotesTable` 结束
- `// ─── Raw Memories Table ───` 到 `fn RawTable` 结束
- `fn RawRow` 整个组件
- `fn facet_total` 辅助函数（新编排直接从 `NotesWindow.total` 与 `stats` 取数）

- [ ] **Step 2: 重写 Memory 组件**

`views/memory/mod.rs` 的顶部与 `Memory` 组件替换为：

```rust
use std::collections::HashSet;

use leptos::leptos_dom::helpers::set_timeout_with_handle;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_location;

use crate::api::agents::AgentsApi;
use crate::api::graph::GraphApi;
use crate::api::{CompressedFact, MemoryApi, MemoryStats, RawMemory};
use crate::components::ui::Card;
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use crate::state::memory::{MemoryState, MemoryView};

pub mod data;
mod batch_bar;
mod cards;
mod drawer;
mod facets;
mod loader;
mod pager;
mod provenance;
mod toast;

use batch_bar::BatchBar;
use cards::{CardListShell, NoteCard, RawCard};
use data::{
    bucket_counts, facet_slice, filter_notes, locate_note, notes_to_markdown, page_slice,
    raws_to_markdown, Loadable, MemoryFacet, NoteExport, RawExport, EXPORT_MAX, NOTE_WINDOW,
    PAGE_SIZE,
};
use drawer::{DetailDrawer, DrawerTarget};
use facets::FacetBar;
use loader::{load_notes, load_raw, load_search_hits, load_stats, NotesWindow};
use pager::Pager;
use toast::{push_toast, ToastHost, ToastKind, ToastMsg};

/// Debounce before the search box filters the loaded window. Long enough that a
/// fast typist does not re-filter on every keystroke, short enough to feel live.
const SEARCH_DEBOUNCE_MS: u64 = 200;

/// Cap on server-side full-text hits. Surfaced in the UI (`search_hits_capped`)
/// because a silent cap reads as "the store only has this much".
const SEARCH_HITS_LIMIT: usize = 100;

/// Cap on evidence items requested per trace. Also surfaced.
pub(super) const TRACE_MAX_RESULTS: usize = 20;

/// Memory Vault console at `/memory?view=table`.
///
/// Layered over the note store (All / Facts / Feedback / Lessons), the raw
/// conversation log, and server-side full-text hits. Search is dual-track: the
/// box filters the loaded window locally as you type, and Enter runs a real
/// `graph.search` whose hits land in their own layer. Pure I/O — every mutation
/// is a JSON-RPC call (R4).
#[component]
#[must_use]
pub fn Memory() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let mem = expect_context::<MemoryState>();
    let i18n = use_i18n();

    // ── Load slots ──────────────────────────────────────────────────────────
    let stats = RwSignal::new(Loadable::<MemoryStats>::Loading);
    let notes = RwSignal::new(Loadable::<NotesWindow>::Loading);
    let raws = RwSignal::new(Loadable::<Vec<RawMemory>>::Loading);
    let hits = RwSignal::new(Loadable::<Vec<CompressedFact>>::Loading);

    // ── View state ──────────────────────────────────────────────────────────
    let facet = RwSignal::new(MemoryFacet::AllNotes);
    let page = RwSignal::new(0u32);
    let page_size = RwSignal::new(PAGE_SIZE);
    let raw_query = RwSignal::new(String::new());
    let local_query = RwSignal::new(String::new());
    let search_live = RwSignal::new(false);
    let selected = RwSignal::new(HashSet::<String>::new());
    let drawer_target = RwSignal::new(None::<DrawerTarget>);
    let highlight_id = RwSignal::new(None::<String>);
    let exporting = RwSignal::new(None::<(usize, usize)>);
    let toast_slot = RwSignal::new(None::<ToastMsg>);
    let refresh_nonce = RwSignal::new(0u32);

    // ── Agent bootstrap (only if the canvas hasn't populated it yet) ─────────
    Effect::new(move || {
        if !state.is_connected.get() || !mem.agents.get().is_empty() {
            return;
        }
        spawn_local(async move {
            if let Ok(resp) = AgentsApi::list(&state).await {
                mem.agents.set(resp.agents);
                if mem.agent_id.get_untracked() != resp.default_id {
                    mem.agent_id.set(resp.default_id);
                }
            }
        });
    });

    // ── Fetches ─────────────────────────────────────────────────────────────
    Effect::new(move || {
        refresh_nonce.get();
        if !state.is_connected.get() {
            stats.set(Loadable::Loading);
            notes.set(Loadable::Loading);
            return;
        }
        let agent = mem.agent_id.get();
        load_stats(state, agent.clone(), stats);
        load_notes(state, agent, NOTE_WINDOW, notes);
        page.set(0);
    });

    Effect::new(move || {
        refresh_nonce.get();
        if !state.is_connected.get() {
            raws.set(Loadable::Loading);
            return;
        }
        let (p, size, q, agent) = (
            page.get(),
            page_size.get(),
            raw_query.get(),
            mem.agent_id.get(),
        );
        load_raw(state, agent, q, size, p * size, raws);
    });

    // ── Dual-track search ───────────────────────────────────────────────────
    // Track 1: every keystroke debounce-filters the LOADED window. `filter_notes`
    // has existed (and been unit-tested) since the phone list shipped; the
    // desktop view never called it, so typing here used to do nothing to the
    // note layers.
    let debounce_handle = StoredValue::new(None::<leptos::leptos_dom::helpers::TimeoutHandle>);
    Effect::new(move || {
        let q = mem.search_query.get();
        if let Some(h) = debounce_handle.get_value() {
            h.clear();
        }
        let handle = set_timeout_with_handle(
            move || {
                local_query.set(q.clone());
                page.set(0);
            },
            std::time::Duration::from_millis(SEARCH_DEBOUNCE_MS),
        )
        .ok();
        debounce_handle.set_value(handle);
    });
    on_cleanup(move || {
        if let Some(h) = debounce_handle.get_value() {
            h.clear();
        }
    });

    // Track 2: Enter runs a real server-side search. Hits get their own layer;
    // they are NOT poured into the raw table (they are notes).
    Effect::new(move || {
        mem.search_nonce.get();
        let q = mem.search_query.get_untracked();
        let agent = mem.agent_id.get_untracked();
        page.set(0);
        if q.trim().is_empty() {
            search_live.set(false);
            if facet.get_untracked() == MemoryFacet::SearchHits {
                facet.set(MemoryFacet::AllNotes);
            }
            return;
        }
        search_live.set(true);
        facet.set(MemoryFacet::SearchHits);
        load_search_hits(state, agent, q, SEARCH_HITS_LIMIT, hits);
    });

    // The raw layer filters server-side (LIKE over content), so it needs the
    // committed query rather than the local one.
    Effect::new(move || {
        let q = local_query.get();
        if facet.get_untracked() == MemoryFacet::Raw {
            raw_query.set(q);
        }
    });
    Effect::new(move || {
        if facet.get() == MemoryFacet::Raw {
            raw_query.set(local_query.get_untracked());
        }
    });

    // ── Deep link: ?note=<path> ─────────────────────────────────────────────
    // `?view=` stays owned by memory_hub; this Effect owns `?note=` only, so the
    // two never overwrite each other. The param is scrubbed after consumption or
    // a reload would force the drawer open again.
    let location = use_location();
    Effect::new(move || {
        let search = location.search.get();
        let Some(path) = parse_note_param(&search) else {
            return;
        };
        let Some(window) = notes.get().as_ready().cloned() else {
            return; // wait for the window; this re-runs when it lands
        };
        match locate_note(&window.facts, &path) {
            Some((f, pg)) => {
                facet.set(f);
                page.set(pg);
                highlight_id.set(Some(path.clone()));
                if let Some(found) = window.facts.iter().find(|x| x.path == path) {
                    drawer_target.set(Some(DrawerTarget::Note(found.clone())));
                }
            }
            None => {
                // Outside the loaded window: open it directly rather than
                // shrugging with "not in the current window" like the old view.
                drawer_target.set(Some(DrawerTarget::Note(CompressedFact::stub_from_path(&path))));
            }
        }
        scrub_note_param();
    });

    // Reverse link from the galaxy's node detail panel.
    Effect::new(move || {
        let Some(path) = mem.highlight_note_id.get() else {
            return;
        };
        let Some(window) = notes.get().as_ready().cloned() else {
            return;
        };
        if let Some((f, pg)) = locate_note(&window.facts, &path) {
            facet.set(f);
            page.set(pg);
            highlight_id.set(Some(path.clone()));
            if let Some(found) = window.facts.iter().find(|x| x.path == path) {
                drawer_target.set(Some(DrawerTarget::Note(found.clone())));
            }
        } else {
            drawer_target.set(Some(DrawerTarget::Note(CompressedFact::stub_from_path(&path))));
        }
        mem.highlight_note_id.set(None);
    });

    // ── Derived rows ────────────────────────────────────────────────────────
    let note_rows = Signal::derive(move || {
        let f = facet.get();
        if f == MemoryFacet::SearchHits {
            return hits.get().as_ready().cloned().unwrap_or_default();
        }
        let Some(window) = notes.get().as_ready().cloned() else {
            return Vec::new();
        };
        filter_notes(&facet_slice(&window.facts, f), &local_query.get())
    });
    let counts = Signal::derive(move || {
        notes
            .get()
            .as_ready()
            .map(|w| bucket_counts(&w.facts))
            .unwrap_or([0; 4])
    });
    let is_notes = Memo::new(move |_| facet.get().is_notes());

    // Note rows are paginated client-side over the loaded window; raw rows are
    // server-paginated, so their pager reads the scoped stats total.
    let note_page_rows = Signal::derive(move || {
        page_slice(&note_rows.get(), page.get(), page_size.get())
    });
    let raw_rows = Signal::derive(move || raws.get().as_ready().cloned().unwrap_or_default());

    view! {
        <div class="px-8 pb-8 aleph-content-top max-w-7xl mx-auto space-y-6">
            <MemoryHeader stats=stats.into() on_refresh=move || refresh_nonce.update(|n| *n += 1) />
            <StatCards stats=stats.into() />

            <div class="flex items-center justify-between gap-3 flex-wrap border-b border-border pb-2">
                <FacetBar
                    active=facet
                    counts=counts
                    raw_count=Signal::derive(move || stats.get().as_ready().map(|s| s.total_memories))
                    hits_count=Signal::derive(move || {
                        search_live
                            .get()
                            .then(|| hits.get().as_ready().map(Vec::len).unwrap_or(0))
                    })
                    on_select=Callback::new(move |f: MemoryFacet| {
                        facet.set(f);
                        page.set(0);
                        selected.set(HashSet::new());
                    })
                />
            </div>

            <BatchBar
                selected=selected
                page_ids=Signal::derive(move || {
                    if is_notes.get() {
                        note_page_rows.get().into_iter().map(|f| f.path).collect()
                    } else {
                        raw_rows.get().into_iter().map(|r| r.id).collect()
                    }
                })
                exporting=exporting
                on_copy_md=move || { /* Step 3 */ }
                on_delete=move || { /* Step 3 */ }
            />

            // …list + pager + drawer + ToastHost (Step 3)
            <ToastHost slot=toast_slot />
            <DetailDrawer target=drawer_target />
        </div>
    }
}

/// Read `note=<path>` out of a URL query string. `None` when absent, so the
/// Effect leaves the current selection alone.
#[must_use]
pub(crate) fn parse_note_param(search: &str) -> Option<String> {
    let s = search.strip_prefix('?').unwrap_or(search);
    for pair in s.split('&') {
        if let Some(v) = pair.strip_prefix("note=") {
            let decoded = js_sys::decode_uri_component(v).ok()?;
            let decoded: String = decoded.into();
            if !decoded.is_empty() {
                return Some(decoded);
            }
        }
    }
    None
}

/// Drop `note=` from the address bar after consuming it, so a reload does not
/// force the drawer open again. Mirrors `context::scrub_credentials_from_url`.
fn scrub_note_param() {
    let Some(win) = web_sys::window() else { return };
    let Ok(href) = win.location().href() else { return };
    let Some((base, query)) = href.split_once('?') else {
        return;
    };
    let kept: Vec<&str> = query
        .split('&')
        .filter(|p| !p.starts_with("note="))
        .collect();
    let next = if kept.is_empty() {
        base.to_string()
    } else {
        format!("{base}?{}", kept.join("&"))
    };
    if let Ok(history) = win.history() {
        let _ = history.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&next));
    }
}
```

- [ ] **Step 3: 补齐列表体、批量回调与统计卡组件**

在同文件继续添加（承接 Step 2 里标 `/* Step 3 */` 的两处与 `// …list + pager` 注释处）。列表体替换那条注释：

```rust
            {move || if is_notes.get() {
                let empty_key = if facet.get() == MemoryFacet::SearchHits {
                    t_string!(i18n, memory.no_search_hits).to_string()
                } else {
                    t_string!(i18n, memory.no_facts).to_string()
                };
                let src = if facet.get() == MemoryFacet::SearchHits {
                    hits.get().as_ready().map(|_| ()).map_or_else(
                        || match hits.get() {
                            Loadable::Loading => Loadable::Loading,
                            Loadable::Failed(e) => Loadable::Failed(e),
                            Loadable::Ready(_) => unreachable!(),
                        },
                        |()| Loadable::Ready(note_rows.get().len()),
                    )
                } else {
                    match notes.get() {
                        Loadable::Loading => Loadable::Loading,
                        Loadable::Failed(e) => Loadable::Failed(e),
                        Loadable::Ready(_) => Loadable::Ready(note_rows.get().len()),
                    }
                };
                view! {
                    <CardListShell
                        state=Signal::derive(move || src.clone())
                        empty_label=Signal::derive(move || empty_key.clone())
                        on_retry=move || refresh_nonce.update(|n| *n += 1)
                    >
                        <For
                            each=move || note_page_rows.get()
                            key=|f| f.path.clone()
                            children=move |fact| {
                                let path = fact.path.clone();
                                let p_sel = path.clone();
                                let p_tog = path.clone();
                                let p_hl = path.clone();
                                let p_loc = path.clone();
                                let p_link = path.clone();
                                let fact_open = fact.clone();
                                view! {
                                    <NoteCard
                                        fact=fact
                                        checked=Signal::derive(move || selected.get().contains(&p_sel))
                                        highlighted=Signal::derive(move || highlight_id.get().as_deref() == Some(p_hl.as_str()))
                                        on_toggle=move || selected.update(|s| { if !s.remove(&p_tog) { s.insert(p_tog.clone()); } })
                                        on_open=move || drawer_target.set(Some(DrawerTarget::Note(fact_open.clone())))
                                        on_locate=move || {
                                            mem.selected_node.set(Some(p_loc.clone()));
                                            mem.memory_view.set(MemoryView::Graph);
                                        }
                                        on_copy_link=move || {
                                            copy_note_link(&p_link);
                                            push_toast(toast_slot, t_string!(i18n, memory.toast_link_copied).to_string(), ToastKind::Success);
                                        }
                                    />
                                }
                            }
                        />
                    </CardListShell>
                    // No silent caps: say when the window or the hit list is capped.
                    {move || (facet.get() == MemoryFacet::SearchHits
                        && hits.get().as_ready().map(|h| h.len() >= SEARCH_HITS_LIMIT).unwrap_or(false))
                        .then(|| view! {
                            <p class="text-xs text-text-tertiary italic pt-1">{t!(i18n, memory.search_hits_capped)}</p>
                        })}
                    {move || notes.get().as_ready()
                        .map(|w| w.total as usize > w.facts.len())
                        .unwrap_or(false)
                        .then(|| view! {
                            <p class="text-xs text-text-tertiary italic pt-1">{t!(i18n, memory.notes_truncated)}</p>
                        })}
                    <Pager
                        page=page
                        page_size=page_size
                        total=Signal::derive(move || Some(note_rows.get().len() as u64))
                        current_len=Signal::derive(move || note_page_rows.get().len())
                    />
                }.into_any()
            } else {
                let empty_key = t_string!(i18n, memory.no_raw).to_string();
                let src = match raws.get() {
                    Loadable::Loading => Loadable::Loading,
                    Loadable::Failed(e) => Loadable::Failed(e),
                    Loadable::Ready(v) => Loadable::Ready(v.len()),
                };
                view! {
                    <CardListShell
                        state=Signal::derive(move || src.clone())
                        empty_label=Signal::derive(move || empty_key.clone())
                        on_retry=move || refresh_nonce.update(|n| *n += 1)
                    >
                        <For
                            each=move || raw_rows.get()
                            key=|r| r.id.clone()
                            children=move |raw| {
                                let id = raw.id.clone();
                                let id_sel = id.clone();
                                let id_tog = id.clone();
                                let id_del = id.clone();
                                let raw_open = raw.clone();
                                view! {
                                    <RawCard
                                        raw=raw
                                        checked=Signal::derive(move || selected.get().contains(&id_sel))
                                        on_toggle=move || selected.update(|s| { if !s.remove(&id_tog) { s.insert(id_tog.clone()); } })
                                        on_open=move || drawer_target.set(Some(DrawerTarget::Raw(raw_open.clone())))
                                        on_delete={
                                            let id_del = id_del.clone();
                                            move || {
                                                let id = id_del.clone();
                                                spawn_local(async move {
                                                    match MemoryApi::delete(&state, id).await {
                                                        Ok(()) => {
                                                            push_toast(toast_slot, t_string!(i18n, memory.toast_deleted).to_string(), ToastKind::Success);
                                                            refresh_nonce.update(|n| *n += 1);
                                                        }
                                                        // The old view checked is_ok() and dropped the
                                                        // reason, so a failed delete left the row in
                                                        // place with no explanation.
                                                        Err(e) => push_toast(toast_slot, format!("{}: {e}", t_string!(i18n, memory.toast_delete_failed)), ToastKind::Error),
                                                    }
                                                });
                                            }
                                        }
                                    />
                                }
                            }
                        />
                    </CardListShell>
                    <Pager
                        page=page
                        page_size=page_size
                        total=Signal::derive(move || stats.get().as_ready().map(|s| s.total_memories))
                        current_len=Signal::derive(move || raw_rows.get().len())
                    />
                }.into_any()
            }}
```

批量回调（替换 Step 2 的两处 `/* Step 3 */`）：

```rust
                on_copy_md=move || {
                    let ids: Vec<String> = selected.get_untracked().into_iter().collect();
                    if ids.is_empty() || ids.len() > EXPORT_MAX {
                        return;
                    }
                    let agent = mem.agent_id.get_untracked();
                    if is_notes.get_untracked() {
                        let rows: Vec<CompressedFact> = note_rows
                            .get_untracked()
                            .into_iter()
                            .filter(|f| ids.contains(&f.path))
                            .collect();
                        let total = rows.len();
                        exporting.set(Some((0, total)));
                        spawn_local(async move {
                            let mut staged = Vec::with_capacity(total);
                            let mut failures = 0usize;
                            for (i, f) in rows.into_iter().enumerate() {
                                let body = GraphApi::node_detail(&state, &agent, &f.path)
                                    .await
                                    .map(|d| d.content);
                                if body.is_err() {
                                    failures += 1;
                                }
                                staged.push(NoteExport {
                                    title: f.content.clone(),
                                    path: f.path.clone(),
                                    body,
                                });
                                exporting.set(Some((i + 1, total)));
                            }
                            write_clipboard(&notes_to_markdown(&staged));
                            exporting.set(None);
                            let msg = if failures > 0 {
                                format!(
                                    "{} — {}",
                                    t_string!(i18n, memory.toast_copied),
                                    t_string!(i18n, memory.batch_export_partial)
                                )
                            } else {
                                t_string!(i18n, memory.toast_copied).to_string()
                            };
                            push_toast(toast_slot, msg, ToastKind::Success);
                        });
                    } else {
                        let staged: Vec<RawExport> = raw_rows
                            .get_untracked()
                            .into_iter()
                            .filter(|r| ids.contains(&r.id))
                            .map(|r| RawExport {
                                id: r.id,
                                agent_id: r.agent_id,
                                session_id: r.session_id,
                                created_at: r.created_at.unwrap_or_default(),
                                user_input: r.user_input,
                                ai_output: r.ai_output,
                            })
                            .collect();
                        write_clipboard(&raws_to_markdown(&staged));
                        push_toast(toast_slot, t_string!(i18n, memory.toast_copied).to_string(), ToastKind::Success);
                    }
                }
                on_delete=move || {
                    let ids: Vec<String> = selected.get_untracked().into_iter().collect();
                    if ids.is_empty() {
                        return;
                    }
                    // Two stores, two verbs. Notes are curated files
                    // (graph.delete_note); raw rows live in raw_memories
                    // (memory.delete). Sending a note path to memory.delete is
                    // exactly the mix-up that produced undeletable ghost rows.
                    let notes_layer = is_notes.get_untracked();
                    let agent = mem.agent_id.get_untracked();
                    spawn_local(async move {
                        let mut failed = 0usize;
                        for id in ids {
                            let res = if notes_layer {
                                GraphApi::delete_note(&state, &agent, &id).await
                            } else {
                                MemoryApi::delete(&state, id).await
                            };
                            if res.is_err() {
                                failed += 1;
                            }
                        }
                        selected.set(HashSet::new());
                        refresh_nonce.update(|n| *n += 1);
                        if failed > 0 {
                            push_toast(toast_slot, format!("{} ({failed})", t_string!(i18n, memory.toast_delete_failed)), ToastKind::Error);
                        } else {
                            push_toast(toast_slot, t_string!(i18n, memory.toast_deleted).to_string(), ToastKind::Success);
                        }
                    });
                }
```

两个小工具函数与两个展示组件，加在文件末尾（`parse_note_param` 附近）：

```rust
/// Write `text` to the system clipboard. Mirrors `chat/messages.rs`.
fn write_clipboard(text: &str) {
    if let Some(win) = web_sys::window() {
        let _promise = win.navigator().clipboard().write_text(text);
    }
}

/// Build a shareable deep link to one note and put it on the clipboard.
fn copy_note_link(path: &str) {
    let Some(win) = web_sys::window() else { return };
    let origin = win.location().origin().unwrap_or_default();
    let encoded: String = js_sys::encode_uri_component(path).into();
    write_clipboard(&format!("{origin}/memory?view=table&note={encoded}"));
}

#[component]
fn MemoryHeader(
    stats: Signal<Loadable<MemoryStats>>,
    on_refresh: impl Fn() + Clone + Send + 'static,
) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <header class="flex items-start justify-between gap-4">
            <div>
                <h2 class="text-3xl font-bold tracking-tight mb-2 flex items-center gap-3 text-text-primary">
                    <svg width="32" height="32" attr:class="w-8 h-8 text-primary" viewBox="0 0 24 24"
                         fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                        <ellipse cx="12" cy="5" rx="9" ry="3" />
                        <path d="M21 12c0 1.66-4 3-9 3s-9-1.34-9-3" />
                        <path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5" />
                    </svg>
                    {t!(i18n, memory.title)}
                </h2>
                <p class="text-text-secondary">{t!(i18n, memory.description)}</p>
            </div>
            <button
                class="flex items-center gap-1.5 px-3 py-1.5 text-xs rounded-lg border border-border \
                       text-text-secondary hover:text-text-primary transition-colors flex-shrink-0"
                on:click=move |_| on_refresh()
            >
                <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                     stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                    <path d="M21 12a9 9 0 1 1-3-6.7" /><polyline points="21 3 21 9 15 9" />
                </svg>
                {t!(i18n, memory.refresh)}
            </button>
        </header>
    }
}

#[component]
fn StatCards(stats: Signal<Loadable<MemoryStats>>) -> impl IntoView {
    let i18n = use_i18n();
    // "—" for both Loading and Failed: the header's Retry and the list's error
    // card already carry the failure; four red boxes would be noise.
    let num = move |pick: fn(&MemoryStats) -> Option<u64>| {
        Signal::derive(move || {
            stats
                .get()
                .as_ready()
                .and_then(pick)
                .map(|n| n.to_string())
                .unwrap_or_else(|| "\u{2014}".to_string())
        })
    };
    let scope_label = Signal::derive(move || {
        match stats.get().as_ready().map(|s| s.scope.as_str()) {
            Some("global") => t_string!(i18n, memory.scope_global).to_string(),
            Some(_) => t_string!(i18n, memory.scope_agent).to_string(),
            None => String::new(),
        }
    });

    let facts = num(|s| Some(s.total_facts));
    let raws = num(|s| Some(s.total_memories));
    let nodes = num(|s| s.total_graph_nodes);
    let edges = num(|s| s.total_graph_edges);

    view! {
        <div>
            <div class="grid grid-cols-2 md:grid-cols-4 gap-6">
                <StatCard tone="primary" label=t_string!(i18n, memory.compressed_facts).to_string() value=facts />
                <StatCard tone="success" label=t_string!(i18n, memory.raw_memories).to_string() value=raws />
                <StatCard tone="primary" label=t_string!(i18n, memory.graph_nodes).to_string() value=nodes />
                <StatCard tone="success" label=t_string!(i18n, memory.graph_edges).to_string() value=edges />
            </div>
            // Say which population the numbers describe — they used to be a
            // cross-agent mix presented next to an agent-scoped list.
            <p class="text-[10px] text-text-tertiary mt-1.5 uppercase tracking-widest">
                {move || scope_label.get()}
            </p>
        </div>
    }
}

#[component]
fn StatCard(tone: &'static str, label: String, value: Signal<String>) -> impl IntoView {
    let (bg, fg) = if tone == "success" {
        ("bg-success-subtle border-success/10", "text-success")
    } else {
        ("bg-primary-subtle border-primary/10", "text-primary")
    };
    view! {
        <Card class=format!("{bg} p-6 flex flex-col items-start")>
            <span class=format!("text-[10px] font-bold {fg} uppercase tracking-widest mb-1.5")>{label}</span>
            <span class="text-3xl font-bold font-mono">{move || value.get()}</span>
        </Card>
    }
}
```

- [ ] **Step 4: 侧栏搜索框加本地过滤提示**

`views/memory_hub/sidebar.rs` 的搜索 `<input>` 之后插入：

```rust
                    <Show when=move || !mem.search_query.get().trim().is_empty()>
                        <p class="mt-1 text-[10px] leading-snug text-text-tertiary">
                            {move || t_string!(i18n, memory.search_hint_local).to_string()}
                        </p>
                    </Show>
```

并把 `placeholder` 的 i18n key 保持为 `memory.search_placeholder`（Task 11 已改文案）。

- [ ] **Step 5: 编译并逐个消灭报错**

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
cargo check -p aleph-panel --target wasm32-unknown-unknown 2>&1 | grep -E "^error" -A 8 | head -80
```

已知需要现场决断的点：
1. **`js_sys::decode_uri_component` / `encode_uri_component`** 返回 `Result<JsString, JsValue>` / `JsString`。若签名不符，改用 `js_sys::decode_URIComponent` 的实际绑定名（`grep -rn "decode_uri" ~/.cargo/registry --include=*.rs | head` 或直接用 `web_sys::UrlSearchParams`，后者更稳）。**推荐直接换 `web_sys::UrlSearchParams::new_with_str(&search)` + `.get("note")`**，并在 `Cargo.toml` 的 web-sys features 加 `"UrlSearchParams"`、`"Location"`、`"History"`。
2. **`CardListShell` 的 `state` 用了 `Signal::derive(move || src.clone())`** —— `src` 是每次 `move ||` 闭包里算的局部值，这样写会捕获一个快照而非保持反应性。**改成把整个 `src` 计算搬进 `Signal::derive` 内部**：
   ```rust
   state=Signal::derive(move || match notes.get() {
       Loadable::Loading => Loadable::Loading,
       Loadable::Failed(e) => Loadable::Failed(e),
       Loadable::Ready(_) => Loadable::Ready(note_rows.get().len()),
   })
   ```
   SearchHits 层同理（读 `hits` 而非 `notes`）。**这一条必须改，否则三态不会随取数更新。**
3. **`empty_label` 同理** —— 把 `t_string!` 调用搬进 `Signal::derive` 内部。
4. `Card` 的 `class` prop 若要求 `String` 而非 `impl Into<String>`，加 `.to_string()`。
5. `on_cleanup` 需 `use leptos::prelude::on_cleanup;`（`prelude::*` 通常已含）。

- [ ] **Step 6: 全量验证**

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
cargo check -p aleph-panel --target wasm32-unknown-unknown
cargo test -p aleph-panel --lib
cargo clippy -p aleph-panel --target wasm32-unknown-unknown -- -D warnings
wc -l interfaces/webchat/src/platform/wide/views/memory/*.rs
```
Expected: 全部干净；每个文件 ≤ 400 行。

- [ ] **Step 7: 回到 Task 11 Step 6 删死键**

旧表已移除，现在执行 Task 11 的 Step 6（删 11 个死 i18n 键）并单独提交。

- [ ] **Step 8: 格式化并提交**

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
for f in interfaces/webchat/src/platform/wide/views/memory/mod.rs \
         interfaces/webchat/src/platform/wide/views/memory_hub/sidebar.rs; do
  rustfmt --edition 2021 "$f"
done
git status --short
git add -A interfaces/webchat/src/
git commit -m "panel/memory: dual-track search, card list, deep links, refresh

Typing now filters the loaded window through filter_notes -- which has been
unit-tested since the phone list shipped but which the desktop view never
called, so the search box did nothing to the note layers. Enter still runs
a real graph.search, but its hits land in their own note-shaped layer
instead of being poured into the raw table.

Bulk delete dispatches by layer: graph.delete_note for notes,
memory.delete for raw rows. Sending a note path to memory.delete is the
mix-up that produced undeletable ghost rows, and every delete now reports
its outcome instead of being swallowed by an is_ok() check.

?note=<path> opens one memory directly (scrubbed after use so a reload does
not force the drawer open again); a note outside the loaded window is
fetched rather than shrugged at. Stat cards say which population they
describe. Header gains Refresh, and note edits invalidate the window --
previously only the galaxy refreshed."
```

---

## Phase D — 溯源与收尾

### Task 19: provenance.rs（接 memory.trace）+ drawer 瘦身

**Files:**
- Create: `interfaces/webchat/src/platform/wide/views/memory/provenance.rs`
- Modify: `interfaces/webchat/src/platform/wide/views/memory/drawer.rs`

**Interfaces:**
- Consumes: `MemoryApi::trace` · `TraceKind` · `TraceResult` · `EvidenceItem` · `data::Loadable` · `mod::TRACE_MAX_RESULTS`
- Produces: `#[component] pub fn ProvenanceSection(agent: Signal<String>, target: String, kind: TraceKind) -> impl IntoView`

- [ ] **Step 1: 建 provenance.rs**

```rust
//! Evidence-chain section for the memory detail drawer.
//!
//! `memory.trace` has been registered and reachable by the LLM (`memory_trace`
//! tool) since 2026-06-27, but the panel never called it: the drawer showed a
//! note's body and backlinks with no way to ask "what conversation is this
//! claim actually based on?".
//!
//! Both directions are wired. From a note we walk DOWN to the raw rows it was
//! distilled from; from a raw row we walk UP to the notes citing it.

use leptos::prelude::*;
use leptos::task::spawn_local;

use super::data::Loadable;
use super::TRACE_MAX_RESULTS;
use crate::api::{EvidenceItem, MemoryApi, TraceKind, TraceResult};
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};

#[component]
#[must_use]
pub fn ProvenanceSection(
    agent: Signal<String>,
    target: String,
    kind: TraceKind,
) -> impl IntoView {
    let i18n = use_i18n();
    let state = expect_context::<DashboardState>();
    let trace = RwSignal::new(Loadable::<TraceResult>::Loading);
    let expanded = RwSignal::new(None::<String>);

    {
        let target = target.clone();
        Effect::new(move |_| {
            let target = target.clone();
            let agent = agent.get_untracked();
            trace.set(Loadable::Loading);
            spawn_local(async move {
                let res =
                    MemoryApi::trace(&state, &agent, &target, kind, TRACE_MAX_RESULTS).await;
                trace.set(Loadable::from_rpc(res));
            });
        });
    }

    view! {
        <div class="mt-4">
            <div class="text-[10px] uppercase tracking-widest text-text-tertiary mb-1.5">
                {t!(i18n, memory.provenance)}
            </div>

            {move || match trace.get() {
                Loadable::Loading => view! {
                    <div class="h-12 rounded-lg bg-surface-sunken animate-pulse"></div>
                }.into_any(),

                // A trace failure is not "no evidence" — say which it is.
                Loadable::Failed(e) => view! {
                    <div class="text-xs" style="color:var(--cat-error,#f44336)">
                        {t!(i18n, memory.provenance_failed)}" "<span class="font-mono">{e}</span>
                    </div>
                }.into_any(),

                Loadable::Ready(res) if res.evidence.is_empty() => view! {
                    <p class="text-[11px] italic text-text-tertiary">{t!(i18n, memory.provenance_empty)}</p>
                }.into_any(),

                Loadable::Ready(res) => {
                    let capped = res.evidence.len() >= TRACE_MAX_RESULTS;
                    view! {
                        <ul class="space-y-1.5">
                            {res.evidence.into_iter().map(|item| view! {
                                <EvidenceRow item=item expanded=expanded />
                            }).collect_view()}
                        </ul>
                        // No silent caps.
                        {capped.then(|| view! {
                            <p class="text-[10px] italic text-text-tertiary mt-1">
                                {t!(i18n, memory.provenance_capped)}
                            </p>
                        })}
                    }.into_any()
                }
            }}
        </div>
    }
}

#[component]
fn EvidenceRow(item: EvidenceItem, expanded: RwSignal<Option<String>>) -> impl IntoView {
    let i18n = use_i18n();
    let raw_id = item.raw_id.clone();
    let id_for_click = raw_id.clone();
    let id_for_open = raw_id.clone();
    let is_open = Signal::derive(move || expanded.get().as_deref() == Some(id_for_open.as_str()));
    let has_body = item.content.is_some();
    let content = item.content.clone();
    let via_note = item.via_note.clone();
    let via_session = item.via_session.clone();
    let pruned = item.pruned;

    view! {
        <li class="rounded-lg border border-border-subtle bg-surface-sunken px-2.5 py-2">
            <button
                class="w-full text-left"
                prop:disabled=!has_body
                on:click=move |_| {
                    if !has_body { return; }
                    expanded.update(|e| {
                        *e = if e.as_deref() == Some(id_for_click.as_str()) {
                            None
                        } else {
                            Some(id_for_click.clone())
                        };
                    });
                }
            >
                <div class="flex items-center gap-2 flex-wrap">
                    <span class="text-[11px] font-mono text-text-secondary break-all">{raw_id}</span>
                    {via_session.map(|s| view! {
                        <span class="text-[10px] text-text-tertiary font-mono">
                            {move || t_string!(i18n, memory.session).to_string()}" "{s}
                        </span>
                    })}
                    {via_note.map(|n| view! {
                        <span class="text-[10px] text-text-tertiary font-mono break-all">
                            {move || t_string!(i18n, memory.provenance_via).to_string()}" "{n}
                        </span>
                    })}
                    // A cited row whose source is gone is a real state, not an
                    // absence of evidence. Label it rather than hiding it.
                    {pruned.then(|| view! {
                        <span class="px-1.5 py-0.5 rounded text-[10px] bg-warning-subtle text-warning border border-warning/20">
                            {move || t_string!(i18n, memory.provenance_pruned).to_string()}
                        </span>
                    })}
                </div>
            </button>
            {move || (is_open.get() && has_body).then(|| view! {
                <pre class="mt-1.5 whitespace-pre-wrap break-words text-[11px] leading-relaxed \
                            text-text-secondary font-sans">
                    {content.clone().unwrap_or_default()}
                </pre>
            })}
        </li>
    }
}
```

- [ ] **Step 2: 接进 drawer**

`views/memory/drawer.rs`：
1. 顶部加 `use super::provenance::ProvenanceSection;` 与 `use crate::api::TraceKind;`
2. `NoteDetail` 里，backlinks 区之后、`note_lifecycle_managed` 说明之前插入：
   ```rust
            <ProvenanceSection
                agent=Signal::derive(move || mem.agent_id.get())
                target=path.clone()
                kind=TraceKind::Note
            />
   ```
3. `RawDetail` 需要拿到 `raw.id` 与 agent。把签名改为 `fn RawDetail(raw: RawMemory) -> impl IntoView`（不变），内部取 `let mem = expect_context::<MemoryState>();`，并在 `<pre>` 之后插入：
   ```rust
            <ProvenanceSection
                agent=Signal::derive(move || mem.agent_id.get())
                target=raw_id.clone()
                kind=TraceKind::Raw
            />
   ```
   （在函数开头 `let raw_id = raw.id.clone();`。）
4. `RawDetail` 的正文改为 `raw.display_text()`（Task 10 已改；此处确认）。
5. 确认 similarity 块已删（Task 10 Step 5）。
6. `NoteDetail` 的保存 / 重命名 / 删除成功后出 toast：把 `toast_slot` 作为 prop 传进 `DetailDrawer` → `NoteDetail`，成功臂调 `push_toast(slot, t_string!(i18n, memory.toast_saved).to_string(), ToastKind::Success)`（重命名用 `toast_renamed`，删除用 `toast_deleted`）。删除成功时同时 `mem.highlight_note_id.set(None)` 并让父级刷新 —— 通过新增 prop `on_mutated: Callback<()>`，`mod.rs` 传 `Callback::new(move |()| refresh_nonce.update(|n| *n += 1))`。

- [ ] **Step 3: 验证**

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
cargo check -p aleph-panel --target wasm32-unknown-unknown
cargo clippy -p aleph-panel --target wasm32-unknown-unknown -- -D warnings
wc -l interfaces/webchat/src/platform/wide/views/memory/drawer.rs
```
Expected: 干净；`drawer.rs` ≤ 400 行。

- [ ] **Step 4: 格式化并提交**

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
rustfmt --edition 2021 interfaces/webchat/src/platform/wide/views/memory/provenance.rs \
  interfaces/webchat/src/platform/wide/views/memory/drawer.rs
git status --short
git add -A interfaces/webchat/src/platform/wide/views/memory/
git commit -m "panel/memory: wire the evidence chain into the drawer

memory.trace has been registered and callable by the model since
2026-06-27, but the panel never asked it anything: the drawer showed a
note's body and backlinks with no way to see which conversation a claim
came from. Both directions are live -- a note walks down to its source raw
rows, a raw row walks up to the notes citing it.

Cited rows whose source is gone are labelled 'pruned' rather than hidden,
and a trace that fails says so instead of looking like an absence of
evidence."
```

---

### Task 20: 全量验证与文档

**Files:**
- Modify: `docs/reference/FEATURE_LOCATOR.md`
- Modify: `docs/reference/MEMORY_SYSTEM.md`

**Interfaces:**
- Consumes: Task 1–19 全部

- [ ] **Step 1: 全量测试与静态检查**

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
set -o pipefail
cargo test -p alephcore --lib 2>&1 | tail -20
cargo test -p aleph-panel --lib 2>&1 | tail -20
cargo check --bin aleph-server
cargo check -p aleph-cli
cargo check -p aleph-panel --target wasm32-unknown-unknown
cargo clippy --all-targets -- -D warnings 2>&1 | tail -20
```
Expected: 全绿。`aleph-panel` 记忆相关测试数应 ≥ 17（基线）+ 新增（Loadable 4 + SearchHits 3 + 导出 6 + page-size 2 + api 4 = 19）≈ **36**。若某个 pre-existing 测试失败，先 `git stash` 确认它在基线上也失败，是则如实报告而不当作本轮回归。

- [ ] **Step 2: 文件行数体检**

```bash
wc -l interfaces/webchat/src/platform/wide/views/memory/*.rs | sort -rn
```
Expected: 每个 ≤ 400 行。若 `mod.rs` 超了，把 `MemoryHeader` / `StatCards` / `StatCard` 三个展示组件抽到新文件 `header.rs`。

- [ ] **Step 3: 端到端手验（真实 server + 浏览器）**

```bash
export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"
just wasm && cargo build --bin aleph-server && ./target/debug/aleph-server &
sleep 10
```
在浏览器打开 `http://127.0.0.1:18790/memory?view=table`，逐条走查：
1. 四张统计卡下方显示 scope 文案；切 agent 后数字随之变化
2. 键入搜索词 → 笔记卡即时过滤（不切页签）；侧栏出现"按回车做全文搜索"提示
3. 按回车 → 出现「搜索结果」chip 并切过去，显示**笔记卡**（不是 Raw 表）
4. 切到 Raw chip → 只有真正的对话卡（Q/答两段 + session）
5. 停掉 server 再点 Refresh → 出现红色错误卡 + Retry（**不是**空列表）
6. 勾选若干笔记 → batch-bar 出现在列表**上方**；Copy as Markdown 出进度并 toast；选超 50 条时按钮禁用且旁边有说明
7. 删除一条 raw → toast「已删除」；删除失败时 toast 带原因
8. 卡片 hover 点「复制链接」→ 粘贴到新标签能直达该笔记并自动开抽屉，地址栏的 `note=` 已被清掉
9. 抽屉底部「溯源证据链」有内容 / 空态 / 失败态三者之一，`pruned` 有标签
10. 分页选择器切 25/50/100 → 回到第 1 页且总页数随之变化
11. 切到 Graph 视图再切回 → 表格状态保留（keep-alive 未破）

跑完 `pkill -f aleph-server`。任何一条不符 —— **记录下来，不要跳过**。

- [ ] **Step 4: 更新 FEATURE_LOCATOR.md**

① 表格区（约第 86 行「Memory Graph Galaxy Canvas」行之后）新增一行：

```markdown
| UI | 记忆 tab / Vault 列表 / 记忆卡片 / 搜索没反应 / 搜到的是文件名 / 删不掉 / 统计数字不对 / 分页翻到空页 / 溯源证据 / 批量导出 / 记忆深链 | Memory Vault Panel | `interfaces/webchat/src/platform/wide/views/memory/`(`mod.rs` 编排 · `data.rs` 纯数据层+`Loadable` · `loader.rs` · `cards.rs` · `facets.rs` · `batch_bar.rs` · `pager.rs` · `toast.rs` · `drawer.rs` · `provenance.rs`) · `src/gateway/handlers/memory.rs` · `src/memory/store/sqlite/mod.rs::raw_where` | ✅ 深度重构(§6.7, 2026-07-26) |
```

② §6.3 的 `graph.neighbors` ⚠️ 段落（约第 861 行）整段替换为：

```markdown
  - ✅ **`graph.neighbors` 已 CUT（2026-07-26）**：316 行 handler + `GraphNeighborsParams`/`Response` + `mod.rs` re-export + `agents.rs` 注册点全部删除。消费它的 2D radial 引擎早已退役归档，星系一次性载入全图（cap 500）不需要按跳展开。`NoteStore::get_neighbors` **保留**——`src/builtin_tools/note_graph_query.rs` 是活消费者。附录 A #10 同步结案。
```

③ 附录 A #10 那一行（约第 978 行）的状态改为 `✅ **已 CUT（2026-07-26，§6.7）**`，处置栏写「按熵减删除，见 §6.3」。

④ Context 段（第 34 行）`memory.trace` 的「✅ 已连(2026-06-27)」改为「✅ 工具面已连(2026-06-27) + Panel 抽屉已连(2026-07-26, §6.7)」。

⑤ 在 §6.6 之后新增 §6.7 完整条目：

```markdown
### 6.7 记忆 Vault 面板 (Memory Vault Panel) ✅

- **口语关键词**：记忆 tab、Vault 列表、记忆卡片、搜索没反应、搜出来是文件名、这条删不掉、统计数字和列表对不上、分页翻到空页、溯源、证据链、批量导出、复制成 Markdown、记忆链接分享、刷新后还是旧的
- **代码锚点**：
  - **Panel** `interfaces/webchat/src/platform/wide/views/memory/`：`mod.rs`（编排：四个 `Loadable` 槽 + 双轨搜索 + `?note=` 深链 + `refresh_nonce`）· **`data.rs`（纯数据层，host 单测）**：`Loadable<T>`/`from_rpc`、`MemoryFacet`（含 `SearchHits`）、`filter_notes`、`page_slice`/`page_count`、`locate_note`、`notes_to_markdown`/`raws_to_markdown`、`EXPORT_MAX`/`PAGE_SIZES` · `loader.rs`（四条取数→`Loadable`）· `cards.rs`（`CardListShell` 三态 + `NoteCard`/`RawCard`）· `facets.rs`（条件 SearchHits chip）· `batch_bar.rs` · `pager.rs`（含 page-size）· `toast.rs`（模块私有）· `drawer.rs` · `provenance.rs`（`memory.trace`）
  - **服务端**：`src/gateway/handlers/memory.rs`（`handle_search` raw-only / `handle_stats` agent-scoped + `scope` / `handle_list_facts` 回 `total` + 字段直通）· `src/gateway/handlers/graph/search.rs` + `graph_types.rs::SearchResultDto`（笔记 FTS 唯一入口，承载完整索引行）· `src/memory/store/sqlite/mod.rs::raw_where`/`escape_like`（count 与 list 共用 WHERE）
  - **CLI**：`interfaces/cli/src/commands/memory_cmd.rs`（DTO 对齐）
- **职责（单一真相）**：**一个 RPC 一种形状**。`graph.search` = 笔记 FTS 唯一入口；`memory.search` = 原始对话唯一入口；`memory.listFacts` = 笔记列表 + total；`memory.stats` = 单一 scope 的四项计数。Panel 侧纯 I/O（R4），所有取数落进 `Loadable` 三态。
- **状态**：✅ **深度重构（2026-07-26，gap-analysis vs MemOS `apps/memos-local-plugin/viewer`）**。**修复的 12 个缺陷**：① `memory.search` 带 query 时返回笔记却被渲染进 Raw 表 → 搜索显示文件名、行删除永久失败且零提示（`memory.delete` 收到 note path → `Ok(false)` → error 被 `.is_ok()` 吞）；② 三个 loader 全 `if let Ok` → 任何 RPC 失败静默变空列表（phone 侧反而有 error+Retry）；③ `handle_stats` 跨 agent 计数 + graph 硬编码 default agent；④ Raw 分页 total 取全局而 list 是 agent-scoped → 幻影页；⑤ `filter_notes` 有实现+3 单测但桌面从未调用 → 搜索框对笔记层无效；⑥ `similarity` 后端恒 `None` 的死链；⑦ `memory.trace` Panel 零消费者；⑧ `graph.neighbors` 零消费者（CUT）；⑨ `listFacts` 不返 total；⑩ 两个死 i18n 键；⑪ `aleph memory search` 对 JSON 对象调 `as_array()` → 恒空表；⑫ `aleph memory stats` 读 snake_case + 两个后端从不发的键 → 每行恒 `-`。**架构映射非照搬**：memos 的卡片承载 score/tools/steps 是因其 L1 trace 是 step 粒度；Aleph 笔记的对应信息（tags/link_count/双时间戳/session_id）**本来就在数据库行里**，只是被 handler 丢了 —— 所以卡片化在 Aleph 侧是「停止丢弃」而非新增数据面。**熵减**：−~360（neighbors 全链）−~265（两张表+RawRow）−~20（similarity 死链）−11 死 i18n 键 −~16（CLI 幻影行）。
- **打磨话术**：「记忆 tab 改动先分层：**搜索行为不对**→ `mod.rs` 的双轨（键入 = `filter_notes` 本地过滤；Enter = `graph.search` 落 `SearchHits` 层）——**别再让 `memory.search` 返回笔记**，那是本轮修掉的核心 bug，`memory.search` 只装 raw。**列表空白但应该有数据**→ 看 `Loadable` 落到哪一臂；`Failed` 会画红卡带原因，画成空态说明有人绕过了 `Loadable::from_rpc`。**统计数字和列表对不上**→ `handle_stats` 必须收到 `agent_id`，且响应的 `scope` 要与列表 scope 一致。**分页翻到空页**→ count 与 list 是否都过 `raw_where`（那是唯一 WHERE 构造器，改一个必改两个）。**删不掉**→ 分层：笔记走 `graph.delete_note`，raw 走 `memory.delete`，混用必失败。**卡片缺字段**→ 先查 handler 有没有把 `NoteIndexEntry` 的字段发下来（历史 bug 是查出来却不发）。**导出/证据链看着不全**→ 两处都有上限（`EXPORT_MAX=50` / `TRACE_MAX_RESULTS=20`）且都在 UI 上明说，改上限记得同步文案。纯函数（`Loadable`/`filter_notes`/`page_*`/`locate_note`/`*_to_markdown`/`raw_where`/`escape_like`）**全部有测试，改这些必须补测**。改 Panel 记得重编 binary（rust_embed 嵌入链，见 CLAUDE.md）。」
```

- [ ] **Step 5: 更新 MEMORY_SYSTEM.md**

在文档中「Panel / 呈现面」相关章节（若无则新建一节 `## Panel 呈现面与 RPC 形状`）加入 RPC 对照表：

```markdown
| RPC | 返回什么 | 谁消费 |
|---|---|---|
| `memory.listFacts` | 笔记页 + `total`（含 tags / link_count / updated_at） | Panel 笔记层、phone Vault |
| `memory.search` | **只有**原始对话行（`query` 做 content LIKE 过滤） | Panel Raw 层、CLI `memory search` |
| `graph.search` | **只有**笔记 FTS 命中（完整索引行） | Panel SearchHits 层、星系高亮、抽屉 wikilink 解析 |
| `memory.stats` | 单一 scope 的四项计数 + `scope` 字段 | Panel 统计卡、CLI `memory stats` |
| `memory.trace` | 证据链（notes + evidence，含 `pruned`） | `memory_trace` 工具、Panel 抽屉溯源区 |
```

- [ ] **Step 6: 提交文档**

```bash
git add docs/reference/FEATURE_LOCATOR.md docs/reference/MEMORY_SYSTEM.md
git commit -m "docs: record the memory Vault panel refactor

Adds FEATURE_LOCATOR 6.7, closes the graph.neighbors 'connect it or cut it'
note from 2026-07-14 (cut), and corrects the memory.trace entry -- it was
marked fully wired in 2026-06-27 when only the tool surface was."
```

- [ ] **Step 7: 汇报**

汇总给用户：改了哪些文件、测试数从 17 → N、§6.3 手验 11 条的逐条结果（含任何不符项）、`wc -l` 行数体检结果。**不要**在有未解决项时报告完成。

---

## Self-Review

**1. Spec coverage** — 逐节核对 spec：

| Spec 节 | 实现 Task |
|---|---|
| §4.1 RPC 契约（5 项） | Task 2 / 3 / 4 / 5 / 6 |
| §4.2 store 层 WHERE 单一源 | Task 1 |
| §4.3 CUT neighbors | Task 6 |
| §4.4 CLI 对齐 | Task 7 |
| §5 双轨搜索 | Task 8（SearchHits）+ Task 18（debounce/Enter/层路由） |
| §6.1 十文件布局 | Task 12–19（`mod.rs` / `data.rs` / `loader.rs` / `facets.rs` / `cards.rs` / `batch_bar.rs` / `pager.rs` / `toast.rs` / `drawer.rs` / `provenance.rs`）+ Task 20 Step 2 行数体检 |
| §6.2 `Loadable` | Task 8 |
| §6.3 卡片形态 | Task 14（字段来自 Task 4 / 5 / 10） |
| §6.4 删旧代码 | Task 18 Step 1 + Task 12 Step 6 |
| §7.1 CONNECT memory.trace | Task 19 |
| §7.2 CUT similarity | Task 10（DTO/UI）+ Task 11 Step 6（i18n 键） |
| §8 深链 / 批量 / 刷新 / toast | Task 18（深链 + 批量回调 + refresh）+ Task 13（toast）+ Task 17（batch-bar） |
| §9 熵减清单 | Task 6 / 10 / 11 Step 6 / 12 Step 6 / 18 Step 1 |
| §11 验证矩阵 | Task 20 |
| §13 文档后续 | Task 20 Step 4–5 |

无遗漏。

**2. Placeholder scan** — 全篇无 `TBD` / `TODO` / `implement later` / "similar to Task N"。Task 18 Step 2 里的 `/* Step 3 */` 两处是**同一 Task 内的显式前向引用**，其内容在 Step 3 全文给出，不是占位符。

**3. Type consistency** — 跨 Task 核对过的签名：
- `raw_where` / `count_raw_memories` / `get_raw_memories_dashboard`：Task 1 定义 → Task 2、3 消费，参数顺序 `(agent, query[, limit, offset])` 一致
- `Loadable::from_rpc` / `as_ready` / `is_loading`：Task 8 定义 → Task 14（`CardListShell`）、15（四个 loader）、18、19 消费
- `MemoryFacet::SearchHits`：Task 8 定义 → Task 16（chip）、18（层路由）消费
- `NotesWindow { facts, total }`：Task 15 定义 → Task 18 消费（`.facts` / `.total`）
- `MemoryApi::{browse_raw, list_facts, stats, trace}`：Task 10 定义 → Task 15、19 消费；`list_facts` 返回元组已在 Task 10 Step 5 修全部调用点
- `CompressedFact::from_search_hit`：Task 10 定义 → Task 15 消费
- `SearchResultDto` 九字段：Task 5（服务端）与 Task 10 Step 3（Panel 镜像）字段名逐一对应
- `EXPORT_MAX` / `NoteExport` / `RawExport` / `notes_to_markdown` / `raws_to_markdown`：Task 9 定义 → Task 17（cap 显示）、18（导出回调）消费
- `push_toast` / `ToastKind` / `ToastMsg` / `ToastHost`：Task 13 定义 → Task 18、19 消费
- `Pager(page, page_size, total, current_len)`：Task 12 定义 → Task 18 两处调用参数一致
- `BatchBar(selected, page_ids, exporting, on_copy_md, on_delete)`：Task 17 定义 → Task 18 调用一致
- `ProvenanceSection(agent, target, kind)`：Task 19 定义 → 同 Task 内 drawer 两处调用一致
- `TRACE_MAX_RESULTS`：Task 18 定义（`mod.rs`，`pub(super)`）→ Task 19 通过 `use super::TRACE_MAX_RESULTS` 消费

**已修的一处不一致**：Task 18 Step 5 注记 #2/#3 指出 `Signal::derive(move || src.clone())` 会捕获快照而非保持反应性 —— 这是我在起草 Step 3 时写下的真实错误，已在 Step 5 明确要求改成把整个 `match` 搬进 `Signal::derive` 内部，并标注「这一条必须改，否则三态不会随取数更新」。

**4. 执行顺序依赖**（实现者必须遵守）：
- Task 11 Step 6（删 i18n 死键）**必须**在 Task 18 之后执行 —— Task 11 内已用醒目注记说明并给出复核脚本
- Task 12 Step 6 依赖 Task 8 已加 `SearchHits`（否则 `facet_total` 的穷尽 match 报错）
- Task 19 依赖 Task 18 已定义 `TRACE_MAX_RESULTS`
