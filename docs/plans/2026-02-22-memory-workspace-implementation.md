# Memory Workspace Isolation — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add workspace-based domain isolation to Aleph's memory system, orthogonal to existing namespace access control.

**Architecture:** Add a `workspace` column (String, Scalar Index, default "default") to all 4 LanceDB tables. Promote `namespace` and add `workspace` to domain models (MemoryFact, MemoryEntry). Extend SearchFilter/MemoryFilter with WorkspaceFilter. Thread workspace context from Session through Agent Loop to all memory operations.

**Tech Stack:** Rust, LanceDB, Arrow (arrow-array), serde, schemars (JSON Schema)

**Design Doc:** `docs/plans/2026-02-22-memory-workspace-design.md`

---

## Phase 1: Foundation Types

### Task 1: Create Workspace Entity and WorkspaceFilter

**Files:**
- Create: `core/src/memory/workspace.rs`
- Modify: `core/src/memory/mod.rs` (add `pub mod workspace;` and re-exports)

**Step 1: Write the failing test**

Add test at the bottom of `core/src/memory/workspace.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_workspace_default() {
        let ws = Workspace::default_workspace();
        assert_eq!(ws.id, "default");
        assert!(ws.is_default);
        assert!(!ws.is_archived);
    }

    #[test]
    fn test_workspace_config_default() {
        let config = WorkspaceConfig::default();
        assert!(config.decay_rate.is_none());
        assert!(config.allowed_tools.is_none());
        assert!(config.permanent_fact_types.is_empty());
    }

    #[test]
    fn test_workspace_filter_to_sql() {
        let single = WorkspaceFilter::Single("crypto".into());
        assert_eq!(single.to_sql_filter(), "workspace = 'crypto'");

        let multi = WorkspaceFilter::Multiple(vec!["a".into(), "b".into()]);
        assert_eq!(multi.to_sql_filter(), "workspace IN ('a', 'b')");

        let all = WorkspaceFilter::All;
        assert_eq!(all.to_sql_filter(), "1=1");
    }

    #[test]
    fn test_workspace_serialization() {
        let ws = Workspace::default_workspace();
        let json = serde_json::to_string(&ws).unwrap();
        let deserialized: Workspace = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, "default");
        assert!(deserialized.is_default);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo test -p alephcore --lib memory::workspace::tests -- --nocapture`
Expected: FAIL — module doesn't exist yet

**Step 3: Write the implementation**

Create `core/src/memory/workspace.rs`:

```rust
use serde::{Deserialize, Serialize};
use crate::memory::context::FactType;

/// Workspace: domain isolation unit for Aleph's memory system.
///
/// Orthogonal to NamespaceScope (access control).
/// - workspace = "which domain does this memory belong to?"
/// - namespace = "who owns/can access this memory?"
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    pub config: WorkspaceConfig,
    pub is_default: bool,
    pub is_archived: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Workspace {
    pub fn default_workspace() -> Self {
        let now = chrono::Utc::now().timestamp();
        Self {
            id: "default".to_string(),
            name: "Default".to_string(),
            description: "Default workspace for unclassified memories".to_string(),
            icon: None,
            config: WorkspaceConfig::default(),
            is_default: true,
            is_archived: false,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        let now = chrono::Utc::now().timestamp();
        Self {
            id: id.into(),
            name: name.into(),
            description: String::new(),
            icon: None,
            config: WorkspaceConfig::default(),
            is_default: false,
            is_archived: false,
            created_at: now,
            updated_at: now,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceConfig {
    /// Override global decay rate (None = use global default)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decay_rate: Option<f32>,

    /// Fact types that should never decay in this workspace
    #[serde(default)]
    pub permanent_fact_types: Vec<FactType>,

    /// Override default LLM provider
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_provider: Option<String>,

    /// Override default LLM model
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,

    /// Override system prompt for this workspace
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt_override: Option<String>,

    /// Tool whitelist (None = all tools available)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<Vec<String>>,
}

impl Default for WorkspaceConfig {
    fn default() -> Self {
        Self {
            decay_rate: None,
            permanent_fact_types: Vec::new(),
            default_provider: None,
            default_model: None,
            system_prompt_override: None,
            allowed_tools: None,
        }
    }
}

/// Filter for workspace-scoped queries.
#[derive(Debug, Clone)]
pub enum WorkspaceFilter {
    /// Query a single workspace
    Single(String),
    /// Query multiple specific workspaces
    Multiple(Vec<String>),
    /// Query all workspaces (no filtering)
    All,
}

impl WorkspaceFilter {
    /// Generate SQL WHERE clause fragment for LanceDB filtering.
    pub fn to_sql_filter(&self) -> String {
        match self {
            WorkspaceFilter::Single(ws) => format!("workspace = '{}'", ws),
            WorkspaceFilter::Multiple(wss) => {
                let values: Vec<String> = wss.iter().map(|w| format!("'{}'", w)).collect();
                format!("workspace IN ({})", values.join(", "))
            }
            WorkspaceFilter::All => "1=1".to_string(),
        }
    }
}

/// Default workspace ID constant.
pub const DEFAULT_WORKSPACE: &str = "default";
```

Then add to `core/src/memory/mod.rs`:
- Add `pub mod workspace;`
- Add re-exports: `pub use workspace::{Workspace, WorkspaceConfig, WorkspaceFilter, DEFAULT_WORKSPACE};`

**Step 4: Run test to verify it passes**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo test -p alephcore --lib memory::workspace::tests -- --nocapture`
Expected: All 4 tests PASS

**Step 5: Commit**

```bash
git add core/src/memory/workspace.rs core/src/memory/mod.rs
git commit -m "memory: add Workspace entity and WorkspaceFilter types"
```

---

### Task 2: Extend SearchFilter and MemoryFilter with Workspace

**Files:**
- Modify: `core/src/memory/store/types.rs`

**Step 1: Write the failing test**

Add to existing tests in `core/src/memory/store/types.rs`:

```rust
#[test]
fn test_search_filter_workspace_single() {
    let filter = SearchFilter::new()
        .with_workspace(WorkspaceFilter::Single("crypto".into()));
    let sql = filter.to_lance_filter().unwrap();
    assert!(sql.contains("workspace = 'crypto'"));
}

#[test]
fn test_search_filter_workspace_multiple() {
    let filter = SearchFilter::new()
        .with_workspace(WorkspaceFilter::Multiple(vec!["a".into(), "b".into()]));
    let sql = filter.to_lance_filter().unwrap();
    assert!(sql.contains("workspace IN ('a', 'b')"));
}

#[test]
fn test_search_filter_workspace_all() {
    let filter = SearchFilter::new()
        .with_workspace(WorkspaceFilter::All);
    // WorkspaceFilter::All generates "1=1" which should not appear as a meaningful filter
    // or the workspace filter should be omitted entirely
    let sql = filter.to_lance_filter();
    // With only All filter, it should be None (no filtering needed)
    assert!(sql.is_none());
}

#[test]
fn test_search_filter_combined_namespace_workspace() {
    let filter = SearchFilter::new()
        .with_namespace(NamespaceScope::Owner)
        .with_workspace(WorkspaceFilter::Single("crypto".into()))
        .with_valid_only();
    let sql = filter.to_lance_filter().unwrap();
    assert!(sql.contains("workspace = 'crypto'"));
    assert!(sql.contains("is_valid = true"));
}

#[test]
fn test_memory_filter_workspace() {
    let filter = MemoryFilter {
        app_bundle_id: None,
        window_title: None,
        namespace: None,
        workspace: Some(WorkspaceFilter::Single("novel".into())),
        after_timestamp: None,
    };
    // Verify workspace field is accessible
    assert!(matches!(filter.workspace, Some(WorkspaceFilter::Single(_))));
}
```

**Step 2: Run test to verify it fails**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo test -p alephcore --lib memory::store::types -- --nocapture`
Expected: FAIL — `with_workspace` method and `workspace` field don't exist

**Step 3: Write the implementation**

In `core/src/memory/store/types.rs`:

1. Add import: `use crate::memory::workspace::WorkspaceFilter;`

2. Add `workspace` field to `SearchFilter`:
```rust
pub struct SearchFilter {
    pub namespace: Option<NamespaceScope>,
    pub workspace: Option<WorkspaceFilter>,  // NEW
    pub fact_type: Option<FactType>,
    pub is_valid: Option<bool>,
    pub path_prefix: Option<String>,
    pub min_confidence: Option<f32>,
    pub created_after: Option<i64>,
    pub created_before: Option<i64>,
}
```

3. Add builder method `with_workspace()`:
```rust
pub fn with_workspace(mut self, ws: WorkspaceFilter) -> Self {
    self.workspace = Some(ws);
    self
}
```

4. Update `to_lance_filter()` to include workspace clause:
- Add workspace filter generation to the conditions list
- `WorkspaceFilter::All` generates `"1=1"` (effectively no filter for workspace)
- `WorkspaceFilter::Single(ws)` generates `"workspace = '{ws}'"`
- `WorkspaceFilter::Multiple(wss)` generates `"workspace IN ('a', 'b')"`

5. Update `new()` and `valid_only()` to initialize `workspace: None`

6. Add `workspace` field to `MemoryFilter`:
```rust
pub struct MemoryFilter {
    pub app_bundle_id: Option<String>,
    pub window_title: Option<String>,
    pub namespace: Option<NamespaceScope>,
    pub workspace: Option<WorkspaceFilter>,  // NEW
    pub after_timestamp: Option<i64>,
}
```

7. Update all `MemoryFilter` constructors (`for_context`, etc.) to include `workspace: None`

**Step 4: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo test -p alephcore --lib memory::store::types -- --nocapture`
Expected: All tests PASS (old + new)

**Step 5: Commit**

```bash
git add core/src/memory/store/types.rs
git commit -m "memory: add workspace field to SearchFilter and MemoryFilter"
```

---

### Task 3: Add namespace and workspace fields to MemoryFact and MemoryEntry

**Files:**
- Modify: `core/src/memory/context.rs`

**Step 1: Write the failing test**

Add to `core/src/memory/context.rs` tests:

```rust
#[test]
fn test_memory_fact_workspace_defaults() {
    let fact = MemoryFact {
        // ... existing fields with defaults ...
        namespace: "owner".to_string(),
        workspace: "default".to_string(),
        // ... other fields ...
    };
    assert_eq!(fact.workspace, "default");
    assert_eq!(fact.namespace, "owner");
}

#[test]
fn test_memory_entry_workspace_defaults() {
    let entry = MemoryEntry {
        // ... existing fields with defaults ...
        namespace: "owner".to_string(),
        workspace: "default".to_string(),
    };
    assert_eq!(entry.workspace, "default");
    assert_eq!(entry.namespace, "owner");
}
```

**Step 2: Run test to verify it fails**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo test -p alephcore --lib memory::context -- --nocapture`
Expected: FAIL — `namespace` and `workspace` fields don't exist on structs

**Step 3: Write the implementation**

In `core/src/memory/context.rs`:

1. Add to `MemoryFact` struct (after `embedding_model` field):
```rust
    /// Access control scope: "owner", "guest:xxx", "shared"
    #[serde(default = "default_namespace")]
    pub namespace: String,
    /// Domain isolation workspace ID
    #[serde(default = "default_workspace_id")]
    pub workspace: String,
```

2. Add to `MemoryEntry` struct (after `similarity_score` field):
```rust
    /// Access control scope
    #[serde(default = "default_namespace")]
    pub namespace: String,
    /// Domain isolation workspace ID
    #[serde(default = "default_workspace_id")]
    pub workspace: String,
```

3. Add default functions:
```rust
fn default_namespace() -> String { "owner".to_string() }
fn default_workspace_id() -> String { "default".to_string() }
```

4. Update ALL existing places that construct MemoryFact or MemoryEntry to include the new fields. Search for `MemoryFact {` and `MemoryEntry {` across the codebase. For each construction site, add:
```rust
    namespace: "owner".to_string(),
    workspace: "default".to_string(),
```

**IMPORTANT:** This will cause compilation errors across the codebase. You MUST fix every construction site. Common locations:
- `core/src/memory/store/lance/arrow_convert.rs` (record_batch_to_facts, record_batch_to_memories)
- `core/src/memory/compression/` (fact extraction)
- `core/src/memory/context.rs` (any builder/constructor methods)
- `core/src/memory/retrieval.rs`
- `core/src/memory/fact_retrieval.rs`
- Test files

**Step 4: Run full build to verify compilation**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo build -p alephcore 2>&1 | head -100`
Expected: Compilation succeeds (or fix remaining construction sites)

Then: `cd /Users/zouguojun/Workspace/Aleph && cargo test -p alephcore --lib memory::context -- --nocapture`
Expected: All tests PASS

**Step 5: Commit**

```bash
git add -u core/src/
git commit -m "memory: add namespace and workspace fields to MemoryFact and MemoryEntry"
```

---

## Phase 2: Schema & Serialization

### Task 4: Update LanceDB Schemas

**Files:**
- Modify: `core/src/memory/store/lance/schema.rs`

**Step 1: Write the failing test**

Add to schema tests:

```rust
#[test]
fn test_facts_schema_has_workspace() {
    let schema = facts_schema();
    let field = schema.field_with_name("workspace");
    assert!(field.is_ok(), "facts schema should have workspace field");
    assert_eq!(field.unwrap().data_type(), &DataType::Utf8);
    assert!(!field.unwrap().is_nullable());
}

#[test]
fn test_memories_schema_has_workspace() {
    let schema = memories_schema();
    let field = schema.field_with_name("workspace");
    assert!(field.is_ok(), "memories schema should have workspace field");
}

#[test]
fn test_graph_nodes_schema_has_workspace() {
    let schema = graph_nodes_schema();
    let field = schema.field_with_name("workspace");
    assert!(field.is_ok(), "graph_nodes schema should have workspace field");
}

#[test]
fn test_graph_edges_schema_has_workspace() {
    let schema = graph_edges_schema();
    let field = schema.field_with_name("workspace");
    assert!(field.is_ok(), "graph_edges schema should have workspace field");
}
```

**Step 2: Run test to verify it fails**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo test -p alephcore --lib memory::store::lance::schema -- --nocapture`
Expected: FAIL — workspace field doesn't exist in schemas

**Step 3: Write the implementation**

In `core/src/memory/store/lance/schema.rs`, add a `Field::new("workspace", DataType::Utf8, false)` to each of the 4 schema functions:

- `facts_schema()`: Add after the `namespace` field (column 8). The new column becomes index 9, shifting subsequent columns.
- `memories_schema()`: Add after the `namespace` field.
- `graph_nodes_schema()`: Add as new field (this table previously had no namespace).
- `graph_edges_schema()`: Add as new field.

**Step 4: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo test -p alephcore --lib memory::store::lance::schema -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/memory/store/lance/schema.rs
git commit -m "memory: add workspace column to all 4 LanceDB table schemas"
```

---

### Task 5: Update Arrow Serialization — Fix Hardcoding + Add Workspace

**Files:**
- Modify: `core/src/memory/store/lance/arrow_convert.rs`

**Step 1: Write the failing test**

Add to arrow_convert tests:

```rust
#[test]
fn test_facts_to_record_batch_preserves_workspace() {
    let mut fact = create_test_fact(); // use existing test helper
    fact.namespace = "guest:abc".to_string();
    fact.workspace = "crypto-trading".to_string();

    let batch = facts_to_record_batch(&[fact]).unwrap();

    let ns_col = batch.column_by_name("namespace").unwrap();
    let ns_arr = ns_col.as_any().downcast_ref::<StringArray>().unwrap();
    assert_eq!(ns_arr.value(0), "guest:abc"); // NOT hardcoded "owner"

    let ws_col = batch.column_by_name("workspace").unwrap();
    let ws_arr = ws_col.as_any().downcast_ref::<StringArray>().unwrap();
    assert_eq!(ws_arr.value(0), "crypto-trading");
}

#[test]
fn test_record_batch_to_facts_reads_workspace() {
    let mut fact = create_test_fact();
    fact.namespace = "shared".to_string();
    fact.workspace = "novel-writing".to_string();

    let batch = facts_to_record_batch(&[fact]).unwrap();
    let facts = record_batch_to_facts(&batch).unwrap();

    assert_eq!(facts[0].namespace, "shared");
    assert_eq!(facts[0].workspace, "novel-writing");
}

#[test]
fn test_memories_to_record_batch_preserves_workspace() {
    let mut memory = create_test_memory(); // use existing test helper
    memory.namespace = "owner".to_string();
    memory.workspace = "health-advisor".to_string();

    let batch = memories_to_record_batch(&[memory]).unwrap();

    let ws_col = batch.column_by_name("workspace").unwrap();
    let ws_arr = ws_col.as_any().downcast_ref::<StringArray>().unwrap();
    assert_eq!(ws_arr.value(0), "health-advisor");
}

#[test]
fn test_record_batch_to_memories_reads_workspace() {
    let mut memory = create_test_memory();
    memory.workspace = "crypto".to_string();

    let batch = memories_to_record_batch(&[memory]).unwrap();
    let memories = record_batch_to_memories(&batch);

    assert_eq!(memories[0].workspace, "crypto");
}
```

**Step 2: Run test to verify it fails**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo test -p alephcore --lib memory::store::lance::arrow_convert -- --nocapture`
Expected: FAIL — workspace column doesn't exist in serialization

**Step 3: Write the implementation**

In `core/src/memory/store/lance/arrow_convert.rs`:

1. **Fix namespace hardcoding in `facts_to_record_batch()`** (~line 147):
```rust
// BEFORE:
let namespace_arr = StringArray::from_iter_values(facts.iter().map(|_| "owner"));

// AFTER:
let namespace_arr = StringArray::from_iter_values(facts.iter().map(|f| f.namespace.as_str()));
```

2. **Add workspace array in `facts_to_record_batch()`** (after namespace_arr):
```rust
let workspace_arr = StringArray::from_iter_values(facts.iter().map(|f| f.workspace.as_str()));
```

3. **Add workspace_arr to the RecordBatch column list** (insert after namespace_arr in the Arc::new() array).

4. **Update `record_batch_to_facts()`** to read namespace and workspace from columns:
```rust
// Read namespace from column (instead of hardcoding)
let namespace = get_string_column(batch, "namespace", i)
    .unwrap_or_else(|| "owner".to_string());
let workspace = get_string_column(batch, "workspace", i)
    .unwrap_or_else(|| "default".to_string());
```
Then assign to the MemoryFact struct: `namespace, workspace,`

5. **Fix namespace hardcoding in `memories_to_record_batch()`** (~line 523):
```rust
// BEFORE:
let namespace_arr = StringArray::from_iter_values(memories.iter().map(|_| "owner"));

// AFTER:
let namespace_arr = StringArray::from_iter_values(memories.iter().map(|m| m.namespace.as_str()));
let workspace_arr = StringArray::from_iter_values(memories.iter().map(|m| m.workspace.as_str()));
```

6. **Add workspace_arr to memories RecordBatch column list.**

7. **Update `record_batch_to_memories()`** to read workspace:
```rust
let namespace = get_string_column(batch, "namespace", i)
    .unwrap_or_else(|| "owner".to_string());
let workspace = get_string_column(batch, "workspace", i)
    .unwrap_or_else(|| "default".to_string());
```

8. **Update graph node/edge serialization** similarly (add workspace column).

**Step 4: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo test -p alephcore --lib memory::store::lance::arrow_convert -- --nocapture`
Expected: All tests PASS

Then full build: `cd /Users/zouguojun/Workspace/Aleph && cargo build -p alephcore`

**Step 5: Commit**

```bash
git add core/src/memory/store/lance/arrow_convert.rs
git commit -m "memory: fix namespace hardcoding, add workspace to Arrow serialization"
```

---

## Phase 3: Store Trait & Implementation Updates

### Task 6: Update Store Traits

**Files:**
- Modify: `core/src/memory/store/mod.rs`

**Step 1: Identify and update trait signatures**

In `core/src/memory/store/mod.rs`:

1. **MemoryStore** — Update methods that take `NamespaceScope` to also take workspace:
```rust
// list_by_path: add workspace parameter
async fn list_by_path(
    &self,
    parent_path: &str,
    ns: &NamespaceScope,
    workspace: &str,          // NEW
) -> Result<Vec<PathEntry>, AlephError>;

// get_by_path: add workspace parameter
async fn get_by_path(
    &self,
    path: &str,
    ns: &NamespaceScope,
    workspace: &str,          // NEW
) -> Result<Option<MemoryFact>, AlephError>;

// get_facts_by_type: add workspace parameter
async fn get_facts_by_type(
    &self,
    fact_type: FactType,
    ns: &NamespaceScope,
    workspace: &str,          // NEW
    limit: usize,
) -> Result<Vec<MemoryFact>, AlephError>;
```

2. **GraphStore** — Add workspace to all methods:
```rust
async fn upsert_node(&self, node: &GraphNode, workspace: &str) -> Result<(), AlephError>;
async fn get_node(&self, id: &str, workspace: &str) -> Result<Option<GraphNode>, AlephError>;
async fn upsert_edge(&self, edge: &GraphEdge, workspace: &str) -> Result<(), AlephError>;
async fn resolve_entity(
    &self,
    query: &str,
    context_key: Option<&str>,
    workspace: &str,          // NEW
) -> Result<Vec<ResolvedEntity>, AlephError>;
async fn get_edges_for_node(
    &self,
    node_id: &str,
    context_key: Option<&str>,
    workspace: &str,          // NEW
) -> Result<Vec<GraphEdge>, AlephError>;
async fn count_edges_in_context(
    &self,
    node_id: &str,
    context_key: &str,
    workspace: &str,          // NEW
) -> Result<usize, AlephError>;
async fn apply_decay(
    &self,
    policy: &GraphDecayPolicy,
    workspace: &str,          // NEW
) -> Result<DecayStats, AlephError>;
```

3. **SessionStore** — Add workspace to `get_memories_since`:
```rust
async fn get_memories_since(
    &self,
    since_timestamp: i64,
    namespace: &NamespaceScope,
    workspace: &str,          // NEW
) -> Result<Vec<MemoryEntry>, AlephError>;
```

**Step 2: This will cause compile errors in all implementations. Don't run tests yet — proceed to Task 7.**

**Step 3: Commit (compile-broken, intermediate)**

```bash
git add core/src/memory/store/mod.rs
git commit -m "memory: update Store trait signatures with workspace parameter"
```

---

### Task 7: Update LanceDB Fact Store Implementation

**Files:**
- Modify: `core/src/memory/store/lance/facts.rs`

**Step 1: Update implementations to match new trait signatures**

1. **`list_by_path()`** — Add workspace parameter and filter:
```rust
async fn list_by_path(
    &self,
    parent_path: &str,
    ns: &NamespaceScope,
    workspace: &str,
) -> Result<Vec<PathEntry>, AlephError> {
    let ns_value = ns.to_namespace_value();
    let mut filter = format!("parent_path = '{}'", parent_path);
    if !matches!(ns, NamespaceScope::Owner) {
        filter.push_str(&format!(" AND namespace = '{}'", ns_value));
    }
    filter.push_str(&format!(" AND workspace = '{}'", workspace));
    // ... rest unchanged ...
}
```

2. **`get_by_path()`** — Same pattern: add workspace to filter.

3. **`get_facts_by_type()`** — Same pattern: add workspace to filter.

4. **Any other methods that build SQL filters manually** — add workspace condition.

**Step 2: Run compilation check**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo build -p alephcore 2>&1 | head -50`
Expected: May still fail due to other files calling these methods without the new parameter

**Step 3: Commit**

```bash
git add core/src/memory/store/lance/facts.rs
git commit -m "memory: update LanceDB fact store with workspace filtering"
```

---

### Task 8: Update LanceDB Graph Store Implementation

**Files:**
- Modify: `core/src/memory/store/lance/graph.rs`

**Step 1: Update all GraphStore method implementations**

For every method, add `workspace: &str` parameter and include `AND workspace = '{workspace}'` in SQL filters.

Key methods:
- `upsert_node()`: Write workspace field when inserting
- `get_node()`: Filter by workspace
- `upsert_edge()`: Write workspace field when inserting
- `resolve_entity()`: Filter by workspace
- `get_edges_for_node()`: Filter by workspace
- `count_edges_in_context()`: Filter by workspace
- `apply_decay()`: Scope decay to workspace

**Step 2: Run compilation check**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo build -p alephcore 2>&1 | head -50`

**Step 3: Commit**

```bash
git add core/src/memory/store/lance/graph.rs
git commit -m "memory: update LanceDB graph store with workspace filtering"
```

---

### Task 9: Update LanceDB Session Store Implementation

**Files:**
- Modify: `core/src/memory/store/lance/memories.rs` (or `sessions.rs`)

**Step 1: Update SessionStore implementation**

1. **`get_memories_since()`** — Add workspace parameter and filter
2. **`search_memories()`** — MemoryFilter already has workspace field; generate workspace clause in SQL
3. **`get_recent_memories()`** — Same: use MemoryFilter.workspace

**Step 2: Compile check**

**Step 3: Commit**

```bash
git add core/src/memory/store/lance/memories.rs
git commit -m "memory: update LanceDB session store with workspace filtering"
```

---

### Task 10: Fix All Callers — Compile the Full Crate

**Files:**
- Modify: Multiple files across the codebase that call the updated trait methods

**Step 1: Run full build and fix every compilation error**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo build -p alephcore 2>&1`

For each error, the fix pattern is consistent:
- **Callers of `list_by_path(path, ns)`** → Add `"default"` as third parameter: `list_by_path(path, ns, "default")`
- **Callers of `get_by_path(path, ns)`** → Add `"default"`: `get_by_path(path, ns, "default")`
- **Callers of `get_facts_by_type(ft, ns, limit)`** → Add `"default"`: `get_facts_by_type(ft, ns, "default", limit)`
- **Callers of GraphStore methods** → Add `"default"` workspace parameter
- **Callers of `get_memories_since(ts, ns)`** → Add `"default"`: `get_memories_since(ts, ns, "default")`

Common caller locations to fix:
- `core/src/memory/vfs/mod.rs` — VFS bootstrap
- `core/src/memory/vfs/l1_generator.rs` — L1 generation
- `core/src/memory/retrieval.rs` — Memory retrieval
- `core/src/memory/fact_retrieval.rs` — Fact retrieval
- `core/src/memory/dreaming.rs` — Dream daemon
- `core/src/memory/compression/` — Compression service
- `core/src/memory/evolution/` — Contradiction detection
- `core/src/memory/ripple/` — Graph exploration
- `core/src/memory/cortex/` — Cortex integration
- `core/src/builtin_tools/memory_browse.rs` — Memory browse tool
- `core/src/builtin_tools/memory_search.rs` — Memory search tool
- `core/src/agent_loop/` — Agent loop integration
- `core/src/gateway/handlers/` — RPC handlers

**IMPORTANT:** At this stage, use `"default"` as the workspace value everywhere. This maintains backward compatibility. The workspace will be properly threaded from session context in Phase 4.

**Step 2: Keep fixing until `cargo build -p alephcore` succeeds**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo build -p alephcore`
Expected: BUILD SUCCESS

**Step 3: Run all tests**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo test -p alephcore 2>&1 | tail -20`
Expected: All existing tests still pass

**Step 4: Commit**

```bash
git add -u core/src/
git commit -m "memory: update all callers with default workspace parameter"
```

---

## Phase 4: Workspace Context Threading

### Task 11: Add WorkspaceContext and Thread Through Session

**Files:**
- Create or modify: `core/src/memory/workspace.rs` (add WorkspaceContext)
- Modify: Session-related structures (look for `SessionIdentityMeta` or equivalent)

**Step 1: Add WorkspaceContext to workspace.rs**

```rust
use crate::memory::namespace::NamespaceScope;
use crate::memory::store::types::SearchFilter;

/// Runtime context for the active workspace.
/// Created from Session, flows through Agent Loop to all memory operations.
pub struct WorkspaceContext {
    pub workspace_id: String,
    pub namespace: NamespaceScope,
}

impl WorkspaceContext {
    pub fn new(workspace_id: impl Into<String>, namespace: NamespaceScope) -> Self {
        Self {
            workspace_id: workspace_id.into(),
            namespace,
        }
    }

    pub fn default_owner() -> Self {
        Self {
            workspace_id: DEFAULT_WORKSPACE.to_string(),
            namespace: NamespaceScope::Owner,
        }
    }

    pub fn to_search_filter(&self) -> SearchFilter {
        SearchFilter::new()
            .with_namespace(self.namespace.clone())
            .with_workspace(WorkspaceFilter::Single(self.workspace_id.clone()))
            .with_valid_only()
    }

    pub fn workspace_id(&self) -> &str {
        &self.workspace_id
    }
}
```

**Step 2: Write test**

```rust
#[test]
fn test_workspace_context_to_search_filter() {
    let ctx = WorkspaceContext::new("crypto", NamespaceScope::Owner);
    let filter = ctx.to_search_filter();
    let sql = filter.to_lance_filter().unwrap();
    assert!(sql.contains("workspace = 'crypto'"));
    assert!(sql.contains("is_valid = true"));
}
```

**Step 3: Run test**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo test -p alephcore --lib memory::workspace -- --nocapture`

**Step 4: Commit**

```bash
git add core/src/memory/workspace.rs
git commit -m "memory: add WorkspaceContext for runtime workspace propagation"
```

---

### Task 12: Add Workspace to SearchFilter SQL Generation (to_lance_filter)

**Files:**
- Modify: `core/src/memory/store/types.rs`

Ensure the `to_lance_filter()` method properly generates workspace SQL. This should have been partially done in Task 2, but verify and complete:

**Step 1: Verify workspace filter generation**

The `to_lance_filter()` method should include workspace in the generated SQL:

```rust
// In the conditions vec building:
if let Some(ref ws) = self.workspace {
    match ws {
        WorkspaceFilter::All => {}, // no filter needed
        _ => conditions.push(ws.to_sql_filter()),
    }
}
```

**Step 2: Run all filter tests**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo test -p alephcore --lib memory::store::types -- --nocapture`

**Step 3: Commit if changes were needed**

---

## Phase 5: Workspace CRUD Operations

### Task 13: Implement Workspace CRUD via MemoryStore

**Files:**
- Create: `core/src/memory/workspace_store.rs` (or add methods to workspace.rs)

**Step 1: Write tests for Workspace CRUD**

```rust
#[tokio::test]
async fn test_workspace_crud() {
    let backend = create_test_backend().await; // use existing test helper

    // Create
    let ws = Workspace::new("ws-test", "Test Workspace");
    workspace_store::create_workspace(&backend, &ws).await.unwrap();

    // List
    let list = workspace_store::list_workspaces(&backend).await.unwrap();
    assert!(list.iter().any(|w| w.id == "ws-test"));

    // Get
    let fetched = workspace_store::get_workspace(&backend, "ws-test").await.unwrap();
    assert!(fetched.is_some());
    assert_eq!(fetched.unwrap().name, "Test Workspace");

    // Archive
    workspace_store::archive_workspace(&backend, "ws-test").await.unwrap();
    let fetched = workspace_store::get_workspace(&backend, "ws-test").await.unwrap();
    assert!(fetched.unwrap().is_archived);
}
```

**Step 2: Implement Workspace CRUD functions**

These functions serialize/deserialize Workspace as MemoryFact with `fact_type = WorkspaceDefinition`:

```rust
pub async fn create_workspace(db: &MemoryBackend, ws: &Workspace) -> Result<(), AlephError> {
    let content = serde_json::to_string(ws)?;
    let fact = MemoryFact {
        id: ws.id.clone(),
        content,
        fact_type: FactType::Other, // or add WorkspaceDefinition variant
        path: format!("aleph://system/workspaces/{}", ws.id),
        parent_path: "aleph://system/workspaces/".to_string(),
        namespace: "owner".to_string(),
        workspace: DEFAULT_WORKSPACE.to_string(),
        // ... other fields with defaults ...
    };
    db.insert_fact(&fact).await
}

pub async fn list_workspaces(db: &MemoryBackend) -> Result<Vec<Workspace>, AlephError> {
    let filter = SearchFilter::new()
        .with_path_prefix("aleph://system/workspaces/".into())
        .with_valid_only();
    let facts = db.text_search("", &filter, 100).await?;
    // Deserialize each fact.content into Workspace
}

pub async fn get_workspace(db: &MemoryBackend, id: &str) -> Result<Option<Workspace>, AlephError> {
    let path = format!("aleph://system/workspaces/{}", id);
    if let Some(fact) = db.get_by_path(&path, &NamespaceScope::Owner, DEFAULT_WORKSPACE).await? {
        Ok(Some(serde_json::from_str(&fact.content)?))
    } else {
        Ok(None)
    }
}
```

**Step 3: Run tests**

**Step 4: Commit**

```bash
git add core/src/memory/workspace_store.rs core/src/memory/mod.rs
git commit -m "memory: implement Workspace CRUD operations via MemoryStore"
```

---

## Phase 6: LanceDB Migration & Index

### Task 14: Handle Schema Migration for Existing Data

**Files:**
- Modify: `core/src/memory/store/lance/mod.rs` (open_or_create / ensure_indexes)

**Step 1: Add workspace column migration logic**

In the `open_or_create()` method, after opening existing tables, check if workspace column exists. If not, add it with default value "default".

LanceDB approach: Since LanceDB uses Arrow schemas, adding a column requires reading existing data, adding the column, and writing back. Alternatively, handle missing columns gracefully in `record_batch_to_facts()` by defaulting to "default".

**The safest approach:** In `record_batch_to_facts()` and `record_batch_to_memories()`, handle the case where workspace column doesn't exist (return "default"). This provides automatic migration on read.

```rust
// In record_batch_to_facts:
let workspace = batch.column_by_name("workspace")
    .and_then(|col| col.as_any().downcast_ref::<StringArray>())
    .map(|arr| arr.value(i).to_string())
    .unwrap_or_else(|| "default".to_string());
```

**Step 2: Add workspace to `ensure_indexes()`**

After creating the table (if new), create scalar index on workspace column:

```rust
// In ensure_indexes or create_table:
table.create_index(&["workspace"], Index::Auto).await?;
```

**Step 3: Commit**

```bash
git add core/src/memory/store/lance/
git commit -m "memory: handle workspace schema migration and indexing"
```

---

## Phase 7: Gateway Integration

### Task 15: Add Workspace RPC Handlers

**Files:**
- Create: `core/src/gateway/handlers/workspace.rs`
- Modify: `core/src/gateway/handlers/mod.rs` (register handlers)
- Modify: Router/dispatch to wire up workspace.* methods

**Step 1: Implement RPC handlers**

```rust
// workspace.create
pub async fn handle_workspace_create(params: Value, db: &MemoryBackend) -> Result<Value> { ... }

// workspace.list
pub async fn handle_workspace_list(db: &MemoryBackend) -> Result<Value> { ... }

// workspace.get
pub async fn handle_workspace_get(params: Value, db: &MemoryBackend) -> Result<Value> { ... }

// workspace.update
pub async fn handle_workspace_update(params: Value, db: &MemoryBackend) -> Result<Value> { ... }

// workspace.archive
pub async fn handle_workspace_archive(params: Value, db: &MemoryBackend) -> Result<Value> { ... }
```

**Step 2: Wire handlers into the RPC router**

Add `"workspace.create" | "workspace.list" | "workspace.get" | "workspace.update" | "workspace.archive"` to the method dispatch match.

**Step 3: Write integration test**

```rust
#[tokio::test]
async fn test_workspace_rpc_create_and_list() {
    // Start test gateway, send workspace.create RPC, then workspace.list
    // Verify created workspace appears in list
}
```

**Step 4: Commit**

```bash
git add core/src/gateway/handlers/workspace.rs core/src/gateway/handlers/mod.rs
git commit -m "gateway: add workspace.* RPC handlers"
```

---

### Task 16: Add workspace_id to Session

**Files:**
- Modify: Session-related files (find exact location with `grep -r "SessionIdentityMeta\|struct Session " core/src/`)

**Step 1: Add workspace_id field to session structures**

```rust
pub struct SessionIdentityMeta {
    // ... existing fields ...
    pub workspace_id: String, // NEW, defaults to "default"
}
```

**Step 2: Update session.start handler to accept workspace_id**

**Step 3: Commit**

```bash
git add -u core/src/gateway/
git commit -m "gateway: add workspace_id to Session and session.start RPC"
```

---

## Phase 8: Built-in Tools Update

### Task 17: Update memory_search and memory_browse Tools

**Files:**
- Modify: `core/src/builtin_tools/memory_search.rs`
- Modify: `core/src/builtin_tools/memory_browse.rs`

**Step 1: Add workspace parameter to tool args**

For `memory_search`:
```rust
pub struct MemorySearchArgs {
    pub query: String,
    pub max_results: usize,
    #[serde(default = "default_workspace")]
    pub workspace: Option<String>,  // NEW: defaults to current session workspace
}
```

For `memory_browse`:
```rust
pub struct MemoryBrowseArgs {
    pub action: BrowseAction,
    pub path: String,
    pub pattern: Option<String>,
    #[serde(default)]
    pub workspace: Option<String>,  // NEW
}
```

**Step 2: Update tool execution to pass workspace to memory operations**

**Step 3: Commit**

```bash
git add core/src/builtin_tools/memory_search.rs core/src/builtin_tools/memory_browse.rs
git commit -m "tools: add workspace parameter to memory_search and memory_browse"
```

---

## Phase 9: Integration Testing

### Task 18: Write Workspace Isolation Integration Tests

**Files:**
- Create: `core/tests/workspace_isolation.rs` (or add to existing integration test file)

**Step 1: Write comprehensive integration tests**

```rust
#[tokio::test]
async fn test_workspace_isolation_facts() {
    let backend = create_test_backend().await;

    // Insert fact into workspace A
    let mut fact_a = create_test_fact();
    fact_a.workspace = "ws-a".to_string();
    fact_a.content = "Bitcoin price is $100k".to_string();
    backend.insert_fact(&fact_a).await.unwrap();

    // Insert fact into workspace B
    let mut fact_b = create_test_fact();
    fact_b.id = uuid::Uuid::new_v4().to_string();
    fact_b.workspace = "ws-b".to_string();
    fact_b.content = "Chapter 3 outline complete".to_string();
    backend.insert_fact(&fact_b).await.unwrap();

    // Search in workspace A — should only find A's fact
    let filter_a = SearchFilter::new()
        .with_workspace(WorkspaceFilter::Single("ws-a".into()));
    let results = backend.text_search("Bitcoin", &filter_a, 10).await.unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0].fact.content.contains("Bitcoin"));

    // Search in workspace B — should only find B's fact
    let filter_b = SearchFilter::new()
        .with_workspace(WorkspaceFilter::Single("ws-b".into()));
    let results = backend.text_search("Chapter", &filter_b, 10).await.unwrap();
    assert_eq!(results.len(), 1);

    // Cross-workspace search
    let filter_all = SearchFilter::new()
        .with_workspace(WorkspaceFilter::All);
    let results = backend.text_search("", &filter_all, 10).await.unwrap();
    assert!(results.len() >= 2);
}

#[tokio::test]
async fn test_workspace_isolation_graph() {
    let backend = create_test_backend().await;

    // Create node in workspace A
    let node_a = GraphNode { name: "Bitcoin".into(), kind: "asset".into(), .. };
    backend.upsert_node(&node_a, "ws-a").await.unwrap();

    // Create node in workspace B
    let node_b = GraphNode { name: "Bitcoin".into(), kind: "character".into(), .. };
    backend.upsert_node(&node_b, "ws-b").await.unwrap();

    // Same name, different workspaces, different entities
    let resolved_a = backend.resolve_entity("Bitcoin", None, "ws-a").await.unwrap();
    assert_eq!(resolved_a[0].kind, "asset");

    let resolved_b = backend.resolve_entity("Bitcoin", None, "ws-b").await.unwrap();
    assert_eq!(resolved_b[0].kind, "character");
}

#[tokio::test]
async fn test_default_workspace_backward_compat() {
    let backend = create_test_backend().await;

    // Insert fact without explicit workspace (defaults to "default")
    let fact = create_test_fact();
    backend.insert_fact(&fact).await.unwrap();

    // Search without workspace filter — should find in "default"
    let filter = SearchFilter::new()
        .with_workspace(WorkspaceFilter::Single("default".into()));
    let results = backend.text_search(&fact.content, &filter, 10).await.unwrap();
    assert!(!results.is_empty());
}
```

**Step 2: Run integration tests**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo test -p alephcore workspace_isolation -- --nocapture`
Expected: All PASS

**Step 3: Commit**

```bash
git add core/tests/workspace_isolation.rs
git commit -m "test: add workspace isolation integration tests"
```

---

## Phase 10: Final Verification

### Task 19: Full Test Suite and Cleanup

**Step 1: Run full test suite**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo test -p alephcore 2>&1 | tail -30`
Expected: All tests PASS

**Step 2: Run clippy**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo clippy -p alephcore 2>&1 | head -30`
Expected: No new warnings

**Step 3: Verify backward compatibility**

- All existing tests pass unchanged
- Default workspace "default" is used when no workspace specified
- Namespace functionality preserved

**Step 4: Final commit**

```bash
git add -u
git commit -m "memory: workspace isolation implementation complete"
```

---

## Task Dependency Graph

```
Task 1 (Workspace Entity)
    ↓
Task 2 (SearchFilter/MemoryFilter) ←── Task 1
    ↓
Task 3 (MemoryFact/MemoryEntry fields) ←── Task 1
    ↓
Task 4 (LanceDB Schemas) ←── Task 3
    ↓
Task 5 (Arrow Serialization) ←── Task 3, Task 4
    ↓
Task 6 (Store Traits) ←── Task 2
    ↓
Task 7 (Fact Store Impl) ←── Task 5, Task 6
Task 8 (Graph Store Impl) ←── Task 5, Task 6
Task 9 (Session Store Impl) ←── Task 5, Task 6
    ↓
Task 10 (Fix All Callers) ←── Task 7, 8, 9
    ↓
Task 11 (WorkspaceContext) ←── Task 2
Task 12 (Filter SQL) ←── Task 2
    ↓
Task 13 (Workspace CRUD) ←── Task 10
Task 14 (Migration) ←── Task 10
    ↓
Task 15 (RPC Handlers) ←── Task 13
Task 16 (Session workspace) ←── Task 11
    ↓
Task 17 (Built-in Tools) ←── Task 15, Task 16
    ↓
Task 18 (Integration Tests) ←── Task 17
    ↓
Task 19 (Final Verification) ←── Task 18
```

## Summary

| Phase | Tasks | Description |
|-------|-------|-------------|
| 1 | 1-3 | Foundation types: Workspace, WorkspaceFilter, domain model fields |
| 2 | 4-5 | Schema & serialization: LanceDB schemas, Arrow conversion fix |
| 3 | 6-10 | Store traits & implementations: all 3 stores + fix callers |
| 4 | 11-12 | Context threading: WorkspaceContext, filter SQL |
| 5 | 13 | CRUD operations: create/list/get/update/archive workspace |
| 6 | 14 | Migration: handle existing data, indexing |
| 7 | 15-16 | Gateway: RPC handlers, session workspace |
| 8 | 17 | Tools: memory_search, memory_browse |
| 9 | 18 | Integration tests: isolation verification |
| 10 | 19 | Final verification: full test suite |
