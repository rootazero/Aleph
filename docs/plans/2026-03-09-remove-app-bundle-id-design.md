# Remove `app_bundle_id` from Memory System

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Remove the legacy `app_bundle_id` field from the entire memory system, since the Halo floating window has been removed and all interactions now go through the chat dialog window.

**Architecture:** `app_bundle_id` was designed for Halo era where users could trigger Aleph from any app (e.g., Notes, Safari). Now all interactions come through the chat window, making this field meaningless (always hardcoded to `"aleph.chat"`). We remove it from data structures, database schemas, filters, serialization, and UI. The `excluded_apps` config is also removed since it has no purpose without app identification. Desktop tool parameters (AxTree/Snapshot) that use `app_bundle_id` are **not** affected — those are agent-to-desktop targeting, not memory source tracking.

**Tech Stack:** Rust, LanceDB (Arrow), SQLite, Leptos (Panel UI)

**Breaking Changes:** LanceDB `memories` table schema changes from 11 → 10 columns. Existing data will need migration (drop column). SQLite `memories` table DDL changes. JSON-RPC `memory.search` and `memory.clear` API parameters change.

---

## Phase 1: Core Data Structures (4 tasks)

### Task 1: Remove `app_bundle_id` from `memory::context::ContextAnchor`

**Files:**
- Modify: `src/memory/context/mod.rs:27-76`

**Step 1: Read the file to verify current state**

Run: `cat -n src/memory/context/mod.rs | head -80`

**Step 2: Remove `app_bundle_id` from struct and all constructors**

```rust
/// Context anchor that identifies when and where an interaction occurred
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContextAnchor {
    /// Window title (e.g., "Project Plan.txt")
    pub window_title: String,
    /// Unix timestamp when interaction occurred
    pub timestamp: i64,
    /// Topic ID for associating memories with conversation topics
    /// For multi-turn: specific topic UUID; For single-turn: "single-turn" constant
    pub topic_id: String,
}

/// Default topic ID for single-turn interactions
pub const SINGLE_TURN_TOPIC_ID: &str = "single-turn";

impl ContextAnchor {
    /// Create a new context anchor with current timestamp (for single-turn)
    pub fn now(window_title: String) -> Self {
        Self::with_topic(window_title, SINGLE_TURN_TOPIC_ID.to_string())
    }

    /// Create context anchor with specific timestamp (for single-turn)
    pub fn with_timestamp(window_title: String, timestamp: i64) -> Self {
        Self {
            window_title,
            timestamp,
            topic_id: SINGLE_TURN_TOPIC_ID.to_string(),
        }
    }

    /// Create context anchor with topic ID (for multi-turn conversations)
    pub fn with_topic(window_title: String, topic_id: String) -> Self {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        Self {
            window_title,
            timestamp,
            topic_id,
        }
    }
}
```

**Step 3: Run `cargo check -p alephcore` to see all downstream breakages**

This will generate a list of every call site that needs updating. Use the compiler errors as a checklist.

**Step 4: Commit**

```bash
git add src/memory/context/mod.rs
git commit -m "memory: remove app_bundle_id from ContextAnchor"
```

---

### Task 2: Remove `app_bundle_id` from `CapturedContext` and `payload::ContextAnchor`

**Files:**
- Modify: `src/core/types.rs:31-36` — `CapturedContext`
- Modify: `src/core/types.rs:55-63` — `MemoryEntry` (API response type)
- Modify: `src/core/types.rs:67-70` — `AppMemoryInfo` (delete entire struct)
- Modify: `src/payload/mod.rs:64-101` — `payload::ContextAnchor`
- Modify: `src/event/types.rs:242-247` — `InputContext`

**Step 1: Update `CapturedContext`**

```rust
/// Captured context from active application (Swift → Rust)
#[derive(Debug, Clone)]
pub struct CapturedContext {
    pub window_title: Option<String>,
    pub attachments: Option<Vec<MediaAttachment>>,
    pub topic_id: Option<String>,
}
```

**Step 2: Remove `app_bundle_id` from `MemoryEntry` (API response type)**

```rust
/// Memory entry for API responses
#[derive(Debug, Clone)]
pub struct MemoryEntry {
    pub id: String,
    pub window_title: String,
    pub user_input: String,
    pub ai_output: String,
    pub timestamp: i64,
    pub similarity_score: Option<f32>,
}
```

**Step 3: Delete `AppMemoryInfo` struct entirely**

This struct only exists for app-based filtering which no longer applies.

**Step 4: Simplify `payload::ContextAnchor`**

```rust
/// Context anchor - captures the context at interaction time
#[derive(Debug, Clone)]
pub struct ContextAnchor {
    /// Window title (if available)
    pub window_title: Option<String>,
}

impl ContextAnchor {
    /// Create a new context anchor
    pub fn new(window_title: Option<String>) -> Self {
        Self { window_title }
    }

    /// Create from CapturedContext
    pub fn from_captured_context(ctx: &crate::core::CapturedContext) -> Self {
        Self {
            window_title: ctx.window_title.clone(),
        }
    }
}
```

Also remove the `app_name` field and the `app_bundle_id.split('.').next_back()` derivation logic.

**Step 5: Remove `app_bundle_id` from `InputContext`**

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputContext {
    pub app_name: Option<String>,
    pub window_title: Option<String>,
    pub selected_text: Option<String>,
}
```

**Step 6: Update the test `test_context_anchor_creation` in `src/payload/mod.rs:253-264`**

**Step 7: Commit**

```bash
git add src/core/types.rs src/payload/mod.rs src/event/types.rs
git commit -m "types: remove app_bundle_id from CapturedContext, payload, InputContext"
```

---

### Task 3: Remove `excluded_apps` from `MemoryConfig`

**Files:**
- Modify: `src/config/types/memory.rs:31-33` — remove `excluded_apps` field
- Modify: `src/config/types/memory.rs:575-580` — remove from `Default` impl

**Step 1: Remove the field**

Delete `pub excluded_apps: Vec<String>,` from `MemoryConfig` struct and remove the `excluded_apps: vec![...]` from the `Default` impl.

**Step 2: Commit**

```bash
git add src/config/types/memory.rs
git commit -m "config: remove excluded_apps (no longer needed without app_bundle_id)"
```

---

### Task 4: Remove `app_bundle_id` from conversation session

**Files:**
- Modify: `src/conversation/session.rs:98-101` — delete `origin_app()` method

**Step 1: Delete the `origin_app()` method**

Remove lines 98-101.

**Step 2: Grep for callers of `origin_app()` and fix them**

Run: `rg "origin_app" src/` — should find no remaining callers (or fix them).

**Step 3: Commit**

```bash
git add src/conversation/session.rs
git commit -m "session: remove origin_app() method"
```

---

## Phase 2: Database Schema & Serialization (3 tasks)

### Task 5: Remove `app_bundle_id` from LanceDB memories schema

**Files:**
- Modify: `src/memory/store/lance/schema.rs:114-129` — drop column
- Modify: `src/memory/store/lance/schema.rs:235-238` — update test (11 → 10 columns)

**Step 1: Remove `app_bundle_id` from `memories_schema()`**

Remove `Field::new("app_bundle_id", DataType::Utf8, false),` from the schema. Update comment `11 columns` → `10 columns`.

**Step 2: Update test**

```rust
#[test]
fn memories_schema_is_valid() {
    let schema = memories_schema();
    assert_eq!(schema.fields().len(), 10);
}
```

**Step 3: Commit**

```bash
git add src/memory/store/lance/schema.rs
git commit -m "schema: remove app_bundle_id from memories table (11 → 10 columns)"
```

---

### Task 6: Remove `app_bundle_id` from Arrow serialization/deserialization

**Files:**
- Modify: `src/memory/store/lance/arrow_convert.rs`

**Step 1: Find and remove all `app_bundle_id` / `app_arr` / `app_col` references**

In the serialization function (around line 641): remove the `app_arr` StringBuilder and its `append_value(memory.context.app_bundle_id)` call. Remove from the `RecordBatch::try_new` column list.

In the deserialization function (around line 695, 713): remove `app_col` extraction and the `app_bundle_id` assignment to `ContextAnchor`.

Update the column index comments and any column count assertions.

**Step 2: Update roundtrip tests**

Fix any test that creates memories with `app_bundle_id` in this file.

**Step 3: Run tests**

Run: `cargo test -p alephcore --lib arrow_convert`

**Step 4: Commit**

```bash
git add src/memory/store/lance/arrow_convert.rs
git commit -m "arrow: remove app_bundle_id from memories serialization"
```

---

### Task 7: Remove `app_bundle_id` from SQLite schema (state_database)

**Files:**
- Modify: `src/resilience/database/state_database.rs:58-70`

**Step 1: Remove from DDL**

Remove `app_bundle_id TEXT NOT NULL,` from the `CREATE TABLE` statement.
Remove `CREATE INDEX IF NOT EXISTS idx_context ON memories(app_bundle_id, window_title);`
Add a simpler index if needed: `CREATE INDEX IF NOT EXISTS idx_window_title ON memories(window_title);`

**Step 2: Grep for any SQLite queries that reference `app_bundle_id` in resilience/**

Run: `rg "app_bundle_id" src/resilience/` and fix all hits.

**Step 3: Commit**

```bash
git add src/resilience/
git commit -m "sqlite: remove app_bundle_id from memories DDL"
```

---

## Phase 3: Storage Layer & Filters (2 tasks)

### Task 8: Remove `app_bundle_id` from `MemoryFilter` and LanceDB session queries

**Files:**
- Modify: `src/memory/store/types.rs:293-357` — `MemoryFilter`
- Modify: `src/memory/store/lance/sessions.rs:258-262` — `clear_memories`

**Step 1: Remove `app_bundle_id` from `MemoryFilter`**

```rust
#[derive(Debug, Clone, Default)]
pub struct MemoryFilter {
    /// Filter by window title.
    pub window_title: Option<String>,
    /// Restrict to a specific namespace scope.
    pub namespace: Option<NamespaceScope>,
    /// Restrict to a specific workspace.
    pub workspace: Option<WorkspaceFilter>,
    /// Only memories created at or after this Unix timestamp (seconds).
    pub after_timestamp: Option<i64>,
}
```

Remove `for_context()` method (or simplify to only take `window_title`).
Remove `app_bundle_id` clause from `to_lance_filter()`.

**Step 2: Update `clear_memories` in sessions.rs**

The `app_filter` parameter maps to `app_bundle_id` in LanceDB filter. Remove the `app_filter` parameter entirely:

```rust
async fn clear_memories(
    &self,
    window_filter: Option<&str>,
) -> Result<u64, AlephError> {
```

Or update the `SessionStore` trait if it defines this signature.

**Step 3: Update tests**

Fix `memory_filter_for_context` test — remove `app_bundle_id` assertions.

**Step 4: Commit**

```bash
git add src/memory/store/types.rs src/memory/store/lance/sessions.rs
git commit -m "store: remove app_bundle_id from MemoryFilter and session queries"
```

---

### Task 9: Update `SessionStore` trait signature

**Files:**
- Modify: `src/memory/store/mod.rs` (or wherever `SessionStore` trait is defined)

**Step 1: Find the trait definition**

Run: `rg "fn clear_memories" src/memory/store/` to find trait + impls.

**Step 2: Update trait signature** to remove `app_filter` parameter.

**Step 3: Update all implementations.**

**Step 4: Commit**

```bash
git add src/memory/store/
git commit -m "store: update SessionStore::clear_memories signature"
```

---

## Phase 4: Business Logic (5 tasks)

### Task 10: Update memory ingestion

**Files:**
- Modify: `src/memory/ingestion.rs:66-87,142-148`

**Step 1: Remove `excluded_apps` check** (lines 81-87)

Delete the entire `if self.config.excluded_apps.contains(...)` block.

**Step 2: Remove `app` from tracing spans** (lines 67, 144)

Change `app = %context.app_bundle_id,` → remove.

**Step 3: Update tests**

Remove `test_store_memory_excluded_app` test.
Update all `ContextAnchor::now(...)` calls to use the new 1-arg signature.

**Step 4: Commit**

```bash
git add src/memory/ingestion.rs
git commit -m "ingestion: remove app_bundle_id checks and excluded_apps"
```

---

### Task 11: Update memory retrieval

**Files:**
- Modify: `src/memory/retrieval.rs:42-65,82-158,176-257`

**Step 1: Simplify `resolve_entity_filter`**

Change context key from `format!("app:{}|window:{}", context.app_bundle_id, context.window_title)` to just `format!("window:{}", context.window_title)`.

**Step 2: Remove `app` from tracing spans** (lines 89, 153, 184, 247)

**Step 3: Simplify `MemoryFilter::for_context` calls**

Change `MemoryFilter::for_context(&context.app_bundle_id, &context.window_title)` to use the new filter API (window_title only or just `MemoryFilter::new()`).

**Step 4: Update tests**

Update `ContextAnchor::now(...)` calls.

**Step 5: Commit**

```bash
git add src/memory/retrieval.rs
git commit -m "retrieval: remove app_bundle_id from retrieval pipeline"
```

---

### Task 12: Update dreaming and graph modules

**Files:**
- Modify: `src/memory/dreaming.rs:144-148,580-617` — `DreamCluster`, `cluster_memories`, `build_summary`
- Modify: `src/memory/graph.rs:446` — context key

**Step 1: Simplify `DreamCluster`**

```rust
struct DreamCluster {
    window_title: String,
    memories: Vec<MemoryEntry>,
}
```

**Step 2: Update `cluster_memories`**

Change clustering key from `format!("{}::{}", app_bundle_id, window_title)` to just `window_title`.

**Step 3: Update `build_summary`**

Change label from `format!("{} / {}", cluster.app_bundle_id, cluster.window_title)` to just `cluster.window_title`.

**Step 4: Update `graph.rs` context key**

Change `format!("app:{}|window:{}", ...)` to `format!("window:{}", ...)`.

**Step 5: Commit**

```bash
git add src/memory/dreaming.rs src/memory/graph.rs
git commit -m "dreaming/graph: remove app_bundle_id from clustering and context keys"
```

---

### Task 13: Update AI retrieval and memory search tool

**Files:**
- Modify: `src/memory/ai_retrieval.rs:41-53` — `MemoryCandidate`
- Modify: `src/builtin_tools/memory_search.rs:305` — display format

**Step 1: Remove `app_bundle_id` from `MemoryCandidate`**

```rust
pub struct MemoryCandidate {
    pub id: String,
    pub user_input: String,
    pub ai_output: String,
    pub timestamp: i64,
}
```

Update the `From<&MemoryEntry>` impl to not copy `app_bundle_id`.

**Step 2: Update memory search tool display**

Change `format!("{} - {}", t.context.app_bundle_id, t.context.window_title)` to just `t.context.window_title.clone()`.

**Step 3: Commit**

```bash
git add src/memory/ai_retrieval.rs src/builtin_tools/memory_search.rs
git commit -m "ai_retrieval/memory_search: remove app_bundle_id from candidates and display"
```

---

### Task 14: Update payload formatter and capability strategy

**Files:**
- Modify: `src/payload/assembler/formatters.rs:44` — remove `App:` line
- Modify: `src/capability/strategies/memory.rs:183,190-194` — remove tracing and fix `MemoryContextAnchor::with_timestamp` call
- Modify: `src/capability/mod.rs:266` — remove log field

**Step 1: Remove `App:` line from formatter**

Delete `lines.push(format!("   App: {}", entry.context.app_bundle_id));`

**Step 2: Fix memory capability strategy**

Update `MemoryContextAnchor::with_timestamp(...)` call to not pass `app_bundle_id`:
```rust
let memory_anchor = MemoryContextAnchor::with_timestamp(
    anchor.window_title.clone().unwrap_or_default(),
    payload.meta.timestamp,
);
```

Remove `app = %anchor.app_bundle_id` from tracing macros.

**Step 3: Commit**

```bash
git add src/payload/assembler/formatters.rs src/capability/strategies/memory.rs src/capability/mod.rs
git commit -m "payload/capability: remove app_bundle_id from formatters and memory strategy"
```

---

## Phase 5: Gateway & Panel UI (3 tasks)

### Task 15: Update Gateway memory handlers

**Files:**
- Modify: `src/gateway/handlers/memory.rs`
- Modify: `src/gateway/execution_engine/engine.rs:2015-2019`

**Step 1: Remove `app_bundle_id` from handler types**

Remove `app_bundle_id` from:
- `MemoryEntry` struct (line 19)
- `SearchParams` struct (line 74)
- `ClearParams` struct (line 193)

Delete `AppMemoryInfo` struct and `handle_app_list` handler entirely.

**Step 2: Update `handle_search` mapping**

Remove `app_bundle_id: m.context.app_bundle_id,` from the mapping (line 134).
Remove `app_bundle_id: params.app_bundle_id.clone(),` from filter construction (line 117).

**Step 3: Fix `write_conversation_memory` in engine.rs**

```rust
let context = ContextAnchor::with_topic(
    session_key.clone(),
    session_key,
);
```

**Step 4: Update tests**

Fix all test structs to not include `app_bundle_id`.

**Step 5: Commit**

```bash
git add src/gateway/handlers/memory.rs src/gateway/execution_engine/engine.rs
git commit -m "gateway: remove app_bundle_id from memory RPC handlers"
```

---

### Task 16: Update Panel API

**Files:**
- Modify: `apps/panel/src/api.rs:56-72,134-138`

**Step 1: Remove `app_bundle_id` from `BackendMemoryEntry`**

Delete `pub app_bundle_id: String,` field.

**Step 2: Update `RawMemory` source mapping**

Change the source mapping logic (lines 134-138). Since there's no `app_bundle_id` anymore, set `source` to `None` or remove the `source` field from `RawMemory` entirely.

Option A (minimal): always set `source: None`.
Option B (cleaner): remove `source` from `RawMemory` struct.

Recommend **Option B** — clean removal.

**Step 3: Commit**

```bash
git add apps/panel/src/api.rs
git commit -m "panel: remove app_bundle_id from API types"
```

---

### Task 17: Update Panel Memory view

**Files:**
- Modify: `apps/panel/src/views/memory.rs:358,366,383-393`

**Step 1: If `source` was removed from `RawMemory` (Option B)**

Remove the `source` prop from `MemoryRow` component and remove the source Badge column from the table.

**Step 2: If `source` was kept as `None` (Option A)**

No changes needed, the badge just won't show.

**Step 3: Commit**

```bash
git add apps/panel/src/views/memory.rs
git commit -m "panel: remove source column from memory table"
```

---

## Phase 6: Fix All Remaining Compiler Errors (1 task)

### Task 18: Fix all remaining call sites

**Step 1: Run full compile**

Run: `cargo check -p alephcore 2>&1 | head -100`

**Step 2: Fix every remaining error**

These will be call sites that pass `app_bundle_id` to `ContextAnchor::now()`, `::with_timestamp()`, `::with_topic()`, or access `.context.app_bundle_id` on a memory entry.

Common patterns to fix:
- `ContextAnchor::now("com.apple.Notes".to_string(), "Test.txt".to_string())` → `ContextAnchor::now("Test.txt".to_string())`
- `ContextAnchor::with_topic("aleph.chat".to_string(), key.clone(), key)` → `ContextAnchor::with_topic(key.clone(), key)`
- `memory.context.app_bundle_id` → delete/replace

Files likely affected:
- `src/memory/context/tests/fact_tests.rs`
- `src/memory/compression/extractor.rs` (test fixtures)
- `tests/world/memory_ctx.rs`
- `src/conversation/manager.rs` (test `CapturedContext`)
- `src/memory/store/lance/arrow_convert.rs` (roundtrip tests)
- `src/resilience/database/memory_events.rs`

**Step 3: Run tests**

Run: `cargo test -p alephcore --lib`

**Step 4: Commit**

```bash
git add -A
git commit -m "cleanup: fix all remaining app_bundle_id references across codebase"
```

---

## Phase 7: Verify (1 task)

### Task 19: Final verification

**Step 1: Grep for any remaining references**

Run: `rg "app_bundle_id" src/ apps/panel/src/` — should return 0 results (excluding desktop tool parameters in `src/desktop/` and `src/builtin_tools/desktop.rs`).

**Step 2: Run full test suite**

Run: `cargo test -p alephcore --lib`

**Step 3: Check Panel compiles**

Run: `cargo check -p panel` (or however the panel crate is named)

**Step 4: Commit any final fixes**

---

## Summary of Deletions

| What | Where |
|------|-------|
| `ContextAnchor.app_bundle_id` | `memory::context::mod.rs` |
| `CapturedContext.app_bundle_id` | `core::types.rs` |
| `payload::ContextAnchor.app_bundle_id` + `app_name` | `payload/mod.rs` |
| `InputContext.app_bundle_id` | `event/types.rs` |
| `AppMemoryInfo` struct (entire) | `core::types.rs` + `gateway::handlers::memory.rs` |
| `MemoryFilter.app_bundle_id` | `store/types.rs` |
| `DreamCluster.app_bundle_id` | `dreaming.rs` |
| `MemoryCandidate.app_bundle_id` | `ai_retrieval.rs` |
| `MemoryConfig.excluded_apps` | `config/types/memory.rs` |
| `origin_app()` method | `conversation/session.rs` |
| `handle_app_list` handler | `gateway/handlers/memory.rs` |
| `app_bundle_id` Arrow column | `lance/schema.rs` + `arrow_convert.rs` |
| `app_bundle_id` SQLite column + index | `state_database.rs` |
| `source` column in Panel UI | `panel/views/memory.rs` |
| `BackendMemoryEntry.app_bundle_id` | `panel/api.rs` |

## Not Deleted (desktop tool parameters)

| What | Where | Reason |
|------|-------|--------|
| `DesktopRequest::AxTree { app_bundle_id }` | `desktop/types.rs` | Agent targeting |
| `DesktopRequest::Snapshot { app_bundle_id }` | `desktop/types.rs` | Agent targeting |
| Desktop tool parameter schemas | `builtin_tools/desktop.rs` | Agent targeting |
