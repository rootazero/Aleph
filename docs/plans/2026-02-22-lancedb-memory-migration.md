# LanceDB Memory Migration Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace SQLite + sqlite-vec + FTS5 with a unified LanceDB backend for all memory storage.

**Architecture:** Define `MemoryStore`/`GraphStore`/`SessionStore` traits, implement them with LanceDB, then incrementally replace all 55 files that reference `Arc<VectorDatabase>` with the new trait-based interface. No SQLite data migration — clean start.

**Tech Stack:** `lancedb` crate (v0.26.x), `arrow-array`/`arrow-schema` (Arrow ecosystem), `async-trait`, existing `fastembed` for embeddings.

**Design Doc:** `docs/plans/2026-02-22-lancedb-memory-migration-design.md`

---

## Phase 1: Foundation — Dependency + Trait Definitions + Arrow Schema

### Task 1: Add `lancedb` dependency and verify build

**Files:**
- Modify: `core/Cargo.toml`

**Step 1: Add lancedb and arrow dependencies to Cargo.toml**

Add under `[dependencies]`:
```toml
lancedb = "0.26"
arrow-array = "57"
arrow-schema = "57"
arrow-cast = "57"
```

**Step 2: Verify the project still compiles**

Run: `cd core && cargo check`
Expected: SUCCESS (no breaking changes, just new dependencies)

**Step 3: Commit**

```bash
git add core/Cargo.toml
git commit -m "memory: add lancedb and arrow dependencies"
```

---

### Task 2: Define storage trait types (SearchFilter, ScoredFact)

**Files:**
- Create: `core/src/memory/store/types.rs`

**Step 1: Write the types file**

```rust
//! Common types for memory store traits

use crate::memory::context::{FactType, MemoryFact};
use crate::memory::namespace::NamespaceScope;

/// Filter for memory searches
#[derive(Debug, Clone, Default)]
pub struct SearchFilter {
    pub namespace: Option<NamespaceScope>,
    pub fact_type: Option<FactType>,
    pub is_valid: Option<bool>,
    pub path_prefix: Option<String>,
    pub min_confidence: Option<f32>,
    pub created_after: Option<i64>,
    pub created_before: Option<i64>,
}

impl SearchFilter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn valid_only() -> Self {
        Self {
            is_valid: Some(true),
            ..Default::default()
        }
    }

    pub fn with_namespace(mut self, ns: NamespaceScope) -> Self {
        self.namespace = Some(ns);
        self
    }

    pub fn with_fact_type(mut self, ft: FactType) -> Self {
        self.fact_type = Some(ft);
        self
    }

    pub fn with_valid_only(mut self) -> Self {
        self.is_valid = Some(true);
        self
    }

    pub fn with_path_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.path_prefix = Some(prefix.into());
        self
    }

    /// Build a LanceDB filter expression from this SearchFilter
    pub fn to_lance_filter(&self) -> Option<String> {
        let mut clauses = Vec::new();

        if let Some(ref ns) = self.namespace {
            let ns_val = ns.to_namespace_value();
            match ns {
                NamespaceScope::Owner => {
                    // Owner sees everything with namespace='owner' or no namespace filter
                    clauses.push(format!("namespace = '{}'", ns_val));
                }
                NamespaceScope::Guest(_) | NamespaceScope::Shared => {
                    clauses.push(format!("namespace = '{}'", ns_val));
                }
            }
        }

        if let Some(ref ft) = self.fact_type {
            clauses.push(format!("fact_type = '{}'", ft.as_str()));
        }

        if let Some(valid) = self.is_valid {
            clauses.push(format!("is_valid = {}", valid));
        }

        if let Some(ref prefix) = self.path_prefix {
            clauses.push(format!("path LIKE '{}%'", prefix));
        }

        if let Some(min_conf) = self.min_confidence {
            clauses.push(format!("confidence >= {}", min_conf));
        }

        if let Some(after) = self.created_after {
            clauses.push(format!("created_at >= {}", after));
        }

        if let Some(before) = self.created_before {
            clauses.push(format!("created_at <= {}", before));
        }

        if clauses.is_empty() {
            None
        } else {
            Some(clauses.join(" AND "))
        }
    }
}

/// A fact with its relevance score from search
#[derive(Debug, Clone)]
pub struct ScoredFact {
    pub fact: MemoryFact,
    pub score: f32,
}

/// Filter for raw memory searches
#[derive(Debug, Clone, Default)]
pub struct MemoryFilter {
    pub app_bundle_id: Option<String>,
    pub window_title: Option<String>,
    pub namespace: Option<NamespaceScope>,
    pub after_timestamp: Option<i64>,
}

impl MemoryFilter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn for_context(app_id: impl Into<String>, window: impl Into<String>) -> Self {
        Self {
            app_bundle_id: Some(app_id.into()),
            window_title: Some(window.into()),
            ..Default::default()
        }
    }

    /// Build a LanceDB filter expression
    pub fn to_lance_filter(&self) -> Option<String> {
        let mut clauses = Vec::new();

        if let Some(ref app) = self.app_bundle_id {
            clauses.push(format!("app_bundle_id = '{}'", app));
        }

        if let Some(ref window) = self.window_title {
            clauses.push(format!("window_title = '{}'", window));
        }

        if let Some(ref ns) = self.namespace {
            clauses.push(format!("namespace = '{}'", ns.to_namespace_value()));
        }

        if let Some(after) = self.after_timestamp {
            clauses.push(format!("timestamp >= {}", after));
        }

        if clauses.is_empty() {
            None
        } else {
            Some(clauses.join(" AND "))
        }
    }
}
```

**Step 2: Verify it compiles**

Run: `cd core && cargo check`
Expected: SUCCESS

**Step 3: Commit**

```bash
git add core/src/memory/store/types.rs
git commit -m "memory: add SearchFilter, ScoredFact, MemoryFilter types"
```

---

### Task 3: Define MemoryStore, GraphStore, SessionStore traits

**Files:**
- Create: `core/src/memory/store/mod.rs`

**Step 1: Write the trait definitions**

```rust
//! Memory store trait abstractions
//!
//! Three independent traits that LanceMemoryBackend implements:
//! - MemoryStore: Facts CRUD + vector/text/hybrid search
//! - GraphStore: Entity nodes and relationships
//! - SessionStore: Raw conversation logs

pub mod lance;
pub mod types;

use async_trait::async_trait;
use crate::error::AlephError;
use crate::memory::context::{FactType, MemoryEntry, MemoryFact};
use crate::memory::namespace::NamespaceScope;
use crate::config::types::memory::GraphDecayPolicy;
use types::{MemoryFilter, ScoredFact, SearchFilter};

/// Statistics about the memory store
#[derive(Debug, Clone, Default)]
pub struct StoreStats {
    pub total_facts: usize,
    pub valid_facts: usize,
    pub total_memories: usize,
    pub total_graph_nodes: usize,
    pub total_graph_edges: usize,
}

/// Result of graph decay operation
#[derive(Debug, Clone, Default)]
pub struct DecayStats {
    pub nodes_decayed: usize,
    pub nodes_pruned: usize,
    pub edges_decayed: usize,
    pub edges_pruned: usize,
}

/// A resolved entity from the graph
#[derive(Debug, Clone)]
pub struct ResolvedEntity {
    pub node_id: String,
    pub name: String,
    pub kind: String,
    pub aliases: Vec<String>,
    pub context_score: f32,
    pub ambiguous: bool,
}

/// A graph node
#[derive(Debug, Clone)]
pub struct GraphNode {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub aliases: Vec<String>,
    pub metadata_json: String,
    pub decay_score: f32,
    pub created_at: i64,
    pub updated_at: i64,
}

/// A graph edge
#[derive(Debug, Clone)]
pub struct GraphEdge {
    pub id: String,
    pub from_id: String,
    pub to_id: String,
    pub relation: String,
    pub weight: f32,
    pub confidence: f32,
    pub context_key: String,
    pub decay_score: f32,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_seen_at: i64,
}

/// VFS path entry for directory listing
#[derive(Debug, Clone)]
pub struct PathEntry {
    pub path: String,
    pub is_leaf: bool,
    pub child_count: usize,
}

// =============================================================================
// Core Traits
// =============================================================================

/// Core memory storage — Facts CRUD + search
#[async_trait]
pub trait MemoryStore: Send + Sync {
    // --- CRUD ---
    async fn insert_fact(&self, fact: &MemoryFact) -> Result<(), AlephError>;
    async fn get_fact(&self, id: &str) -> Result<Option<MemoryFact>, AlephError>;
    async fn update_fact(&self, fact: &MemoryFact) -> Result<(), AlephError>;
    async fn delete_fact(&self, id: &str) -> Result<(), AlephError>;
    async fn batch_insert_facts(&self, facts: &[MemoryFact]) -> Result<(), AlephError>;

    // --- Search ---
    async fn vector_search(
        &self,
        embedding: &[f32],
        dim_hint: u32,
        filter: &SearchFilter,
        limit: usize,
    ) -> Result<Vec<ScoredFact>, AlephError>;

    async fn text_search(
        &self,
        query: &str,
        filter: &SearchFilter,
        limit: usize,
    ) -> Result<Vec<ScoredFact>, AlephError>;

    async fn hybrid_search(
        &self,
        embedding: &[f32],
        dim_hint: u32,
        query_text: &str,
        vector_weight: f32,
        text_weight: f32,
        filter: &SearchFilter,
        limit: usize,
    ) -> Result<Vec<ScoredFact>, AlephError>;

    // --- VFS Path Operations ---
    async fn list_by_path(
        &self,
        parent_path: &str,
        ns: &NamespaceScope,
    ) -> Result<Vec<PathEntry>, AlephError>;

    async fn get_by_path(
        &self,
        path: &str,
        ns: &NamespaceScope,
    ) -> Result<Option<MemoryFact>, AlephError>;

    // --- Statistics ---
    async fn count_facts(&self, filter: &SearchFilter) -> Result<usize, AlephError>;

    async fn get_facts_by_type(
        &self,
        fact_type: FactType,
        ns: &NamespaceScope,
        limit: usize,
    ) -> Result<Vec<MemoryFact>, AlephError>;

    async fn get_all_facts(
        &self,
        include_invalid: bool,
    ) -> Result<Vec<MemoryFact>, AlephError>;

    // --- Mutation ---
    async fn invalidate_fact(
        &self,
        id: &str,
        reason: &str,
    ) -> Result<(), AlephError>;

    async fn update_fact_content(
        &self,
        id: &str,
        new_content: &str,
    ) -> Result<(), AlephError>;

    async fn find_similar_facts(
        &self,
        embedding: &[f32],
        dim_hint: u32,
        filter: &SearchFilter,
        threshold: f32,
        limit: usize,
    ) -> Result<Vec<ScoredFact>, AlephError>;
}

/// Graph storage — entity nodes and relationships
#[async_trait]
pub trait GraphStore: Send + Sync {
    async fn upsert_node(&self, node: &GraphNode) -> Result<(), AlephError>;
    async fn get_node(&self, id: &str) -> Result<Option<GraphNode>, AlephError>;
    async fn upsert_edge(&self, edge: &GraphEdge) -> Result<(), AlephError>;

    async fn resolve_entity(
        &self,
        query: &str,
        context_key: Option<&str>,
    ) -> Result<Vec<ResolvedEntity>, AlephError>;

    async fn get_edges_for_node(
        &self,
        node_id: &str,
        context_key: Option<&str>,
    ) -> Result<Vec<GraphEdge>, AlephError>;

    async fn count_edges_in_context(
        &self,
        node_id: &str,
        context_key: &str,
    ) -> Result<usize, AlephError>;

    async fn apply_decay(
        &self,
        policy: &GraphDecayPolicy,
    ) -> Result<DecayStats, AlephError>;
}

/// Raw session log storage
#[async_trait]
pub trait SessionStore: Send + Sync {
    async fn insert_memory(&self, memory: &MemoryEntry) -> Result<(), AlephError>;

    async fn search_memories(
        &self,
        embedding: &[f32],
        filter: &MemoryFilter,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>, AlephError>;

    async fn get_memories_for_entity(
        &self,
        entity_id: &str,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>, AlephError>;

    async fn get_recent_memories(
        &self,
        filter: &MemoryFilter,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>, AlephError>;

    async fn delete_memory(&self, id: &str) -> Result<(), AlephError>;

    async fn get_stats(&self) -> Result<StoreStats, AlephError>;
}
```

**Step 2: Register the `store` module in `memory/mod.rs`**

Add `pub mod store;` to `core/src/memory/mod.rs` near the top module declarations.

**Step 3: Verify it compiles**

Run: `cd core && cargo check`
Expected: SUCCESS (traits only, no impl yet)

**Step 4: Commit**

```bash
git add core/src/memory/store/mod.rs core/src/memory/mod.rs
git commit -m "memory: define MemoryStore, GraphStore, SessionStore traits"
```

---

### Task 4: Define Arrow schema builders

**Files:**
- Create: `core/src/memory/store/lance/schema.rs`

**Step 1: Write schema definitions**

Define functions that return `Arc<arrow_schema::Schema>` for each LanceDB table (`facts`, `graph_nodes`, `graph_edges`, `memories`).

Key considerations:
- `facts` table has 3 nullable vector columns: `vec_384`, `vec_1024`, `vec_1536`
- Vector columns use `DataType::FixedSizeList(Arc::new(Field::new("item", DataType::Float32, true)), DIM)`
- List columns (tags, source_memory_ids) use `DataType::List(Arc::new(Field::new("item", DataType::Utf8, true)))`
- All schemas should be `pub fn X_schema() -> Arc<Schema>` functions

Reference the design doc Section 3.2 for exact column names and types.

**Step 2: Write test for schema construction**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_facts_schema_has_vector_columns() {
        let schema = facts_schema();
        assert!(schema.field_with_name("vec_384").is_ok());
        assert!(schema.field_with_name("vec_1024").is_ok());
        assert!(schema.field_with_name("vec_1536").is_ok());
        assert!(schema.field_with_name("content").is_ok());
        assert!(schema.field_with_name("id").is_ok());
    }

    #[test]
    fn test_graph_nodes_schema() {
        let schema = graph_nodes_schema();
        assert!(schema.field_with_name("name").is_ok());
        assert!(schema.field_with_name("kind").is_ok());
        assert!(schema.field_with_name("aliases").is_ok());
    }
}
```

**Step 3: Run tests**

Run: `cd core && cargo test store::lance::schema`
Expected: PASS

**Step 4: Commit**

```bash
git add core/src/memory/store/lance/schema.rs
git commit -m "memory: define Arrow schemas for LanceDB tables"
```

---

### Task 5: Implement Arrow ↔ MemoryFact conversion

**Files:**
- Create: `core/src/memory/store/lance/arrow_convert.rs`

**Step 1: Implement `MemoryFact → RecordBatch` and `RecordBatch → Vec<MemoryFact>` conversions**

Key functions:
- `pub fn facts_to_record_batch(facts: &[MemoryFact]) -> Result<RecordBatch>`
- `pub fn record_batch_to_facts(batch: &RecordBatch) -> Result<Vec<MemoryFact>>`
- `pub fn graph_node_to_record_batch(node: &GraphNode) -> Result<RecordBatch>`
- `pub fn record_batch_to_graph_nodes(batch: &RecordBatch) -> Result<Vec<GraphNode>>`
- `pub fn graph_edge_to_record_batch(edge: &GraphEdge) -> Result<RecordBatch>`
- `pub fn memory_to_record_batch(memory: &MemoryEntry) -> Result<RecordBatch>`

Use `arrow_array::{StringArray, Float32Array, Int64Array, BooleanArray, ListArray, FixedSizeListArray}` for column construction.

**Step 2: Write round-trip test**

```rust
#[test]
fn test_fact_roundtrip() {
    let fact = MemoryFact::new("Rust is a systems language", FactType::Learning, vec![]);
    let batch = facts_to_record_batch(&[fact.clone()]).unwrap();
    let recovered = record_batch_to_facts(&batch).unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].content, fact.content);
    assert_eq!(recovered[0].id, fact.id);
}
```

**Step 3: Run tests**

Run: `cd core && cargo test store::lance::arrow_convert`
Expected: PASS

**Step 4: Commit**

```bash
git add core/src/memory/store/lance/arrow_convert.rs
git commit -m "memory: implement Arrow <-> MemoryFact conversion"
```

---

## Phase 2: LanceDB Backend Implementation

### Task 6: Implement LanceMemoryBackend initialization

**Files:**
- Create: `core/src/memory/store/lance/mod.rs`

**Step 1: Implement `LanceMemoryBackend` struct and `open_or_create`**

```rust
use lancedb::{connect, Database, Table};
use std::path::Path;
use std::sync::Arc;

pub mod arrow_convert;
pub mod schema;

pub struct LanceMemoryBackend {
    db: Database,
    facts_table: Table,
    nodes_table: Table,
    edges_table: Table,
    memories_table: Table,
}

impl LanceMemoryBackend {
    pub async fn open_or_create(data_dir: &Path) -> Result<Self, AlephError> {
        let db_path = data_dir.join("memory.lance");
        let db = connect(db_path.to_str().unwrap()).execute().await
            .map_err(|e| AlephError::Internal(format!("LanceDB connect: {}", e)))?;

        let facts_table = Self::ensure_table(&db, "facts", schema::facts_schema()).await?;
        let nodes_table = Self::ensure_table(&db, "graph_nodes", schema::graph_nodes_schema()).await?;
        let edges_table = Self::ensure_table(&db, "graph_edges", schema::graph_edges_schema()).await?;
        let memories_table = Self::ensure_table(&db, "memories", schema::memories_schema()).await?;

        Ok(Self { db, facts_table, nodes_table, edges_table, memories_table })
    }

    async fn ensure_table(db: &Database, name: &str, schema: Arc<arrow_schema::Schema>) -> Result<Table, AlephError> {
        match db.open_table(name).execute().await {
            Ok(table) => Ok(table),
            Err(_) => db.create_empty_table(name, schema)
                .execute().await
                .map_err(|e| AlephError::Internal(format!("Create table {}: {}", name, e))),
        }
    }
}
```

**Step 2: Write integration test**

```rust
#[tokio::test]
async fn test_open_or_create_backend() {
    let tmp = tempfile::tempdir().unwrap();
    let backend = LanceMemoryBackend::open_or_create(tmp.path()).await.unwrap();
    // Tables exist — verify by listing table names
    let tables = backend.db.table_names().execute().await.unwrap();
    assert!(tables.contains(&"facts".to_string()));
    assert!(tables.contains(&"graph_nodes".to_string()));
    assert!(tables.contains(&"graph_edges".to_string()));
    assert!(tables.contains(&"memories".to_string()));
}
```

**Step 3: Run test**

Run: `cd core && cargo test store::lance::test_open_or_create`
Expected: PASS

**Step 4: Commit**

```bash
git add core/src/memory/store/lance/mod.rs
git commit -m "memory: implement LanceMemoryBackend initialization"
```

---

### Task 7: Implement MemoryStore — Facts CRUD

**Files:**
- Create: `core/src/memory/store/lance/facts.rs`

**Step 1: Implement `MemoryStore` trait for `LanceMemoryBackend` — CRUD operations**

Key implementations:
- `insert_fact`: Convert to RecordBatch via `arrow_convert::facts_to_record_batch`, call `table.add()`
- `get_fact`: Query with `only_if(format!("id = '{}'", id))`, convert result back
- `update_fact`: LanceDB update via `table.update()` with filter `id = ?`
- `delete_fact`: `table.delete(format!("id = '{}'", id))`
- `batch_insert_facts`: Same as insert_fact but batch

**Step 2: Write tests**

```rust
#[tokio::test]
async fn test_fact_insert_and_get() {
    let backend = create_test_backend().await;
    let fact = MemoryFact::new("Test fact", FactType::Learning, vec![]);
    backend.insert_fact(&fact).await.unwrap();

    let retrieved = backend.get_fact(&fact.id).await.unwrap();
    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap().content, "Test fact");
}

#[tokio::test]
async fn test_fact_delete() {
    let backend = create_test_backend().await;
    let fact = MemoryFact::new("Delete me", FactType::Learning, vec![]);
    backend.insert_fact(&fact).await.unwrap();
    backend.delete_fact(&fact.id).await.unwrap();

    let retrieved = backend.get_fact(&fact.id).await.unwrap();
    assert!(retrieved.is_none());
}
```

**Step 3: Run tests**

Run: `cd core && cargo test store::lance::facts`
Expected: PASS

**Step 4: Commit**

```bash
git add core/src/memory/store/lance/facts.rs
git commit -m "memory: implement MemoryStore CRUD for LanceDB"
```

---

### Task 8: Implement MemoryStore — Vector + FTS + Hybrid search

**Files:**
- Modify: `core/src/memory/store/lance/facts.rs`

**Step 1: Implement search methods**

Key implementations:
- `vector_search`: Use `table.query().nearest_to(embedding).column(&format!("vec_{}", dim_hint)).only_if(filter).limit(limit)`
- `text_search`: Use `table.query().full_text_search(FullTextSearchQuery::new(query)).only_if(filter).limit(limit)`
- `hybrid_search`: Use `table.query().nearest_to(embedding).full_text_search(query).rerank(RRFReranker::new()).only_if(filter).limit(limit)`

**Step 2: Create FTS index before search tests**

```rust
// In test setup, create FTS index on content column
async fn create_test_backend_with_index() -> LanceMemoryBackend {
    let backend = create_test_backend().await;
    // Build FTS index on facts.content
    backend.facts_table
        .create_index(&["content"], lancedb::index::Index::FTS(Default::default()))
        .execute().await.unwrap();
    backend
}
```

**Step 3: Write search tests**

```rust
#[tokio::test]
async fn test_vector_search() {
    let backend = create_test_backend_with_index().await;
    let embedding = vec![0.1f32; 384];
    let mut fact = MemoryFact::new("Rust is fast", FactType::Learning, vec![]);
    fact.embedding = Some(embedding.clone());
    backend.insert_fact(&fact).await.unwrap();

    let results = backend.vector_search(
        &embedding, 384, &SearchFilter::valid_only(), 10,
    ).await.unwrap();
    assert!(!results.is_empty());
}

#[tokio::test]
async fn test_text_search() {
    let backend = create_test_backend_with_index().await;
    let fact = MemoryFact::new("Rust is a systems programming language", FactType::Learning, vec![]);
    backend.insert_fact(&fact).await.unwrap();

    let results = backend.text_search(
        "Rust programming", &SearchFilter::valid_only(), 10,
    ).await.unwrap();
    assert!(!results.is_empty());
}
```

**Step 4: Run tests**

Run: `cd core && cargo test store::lance::facts::test_vector_search store::lance::facts::test_text_search`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/memory/store/lance/facts.rs
git commit -m "memory: implement vector, text, and hybrid search for LanceDB"
```

---

### Task 9: Implement GraphStore

**Files:**
- Create: `core/src/memory/store/lance/graph.rs`

**Step 1: Implement `GraphStore` trait for `LanceMemoryBackend`**

Key implementations:
- `upsert_node`: Check if node exists by id → update or insert
- `upsert_edge`: Check if edge exists by id → update or insert
- `resolve_entity`: FTS on `graph_nodes.name`, then optionally filter edges for context disambiguation
- `get_edges_for_node`: `only_if("from_id = ? OR to_id = ?")`
- `count_edges_in_context`: Same with context_key filter, count results
- `apply_decay`: Scan all nodes/edges, apply exponential decay, delete below threshold

**Step 2: Write tests**

```rust
#[tokio::test]
async fn test_graph_upsert_and_resolve() {
    let backend = create_test_backend().await;
    let node = GraphNode {
        id: "n1".into(), name: "Rust".into(), kind: "technology".into(),
        aliases: vec!["rust-lang".into()], metadata_json: "{}".into(),
        decay_score: 1.0, created_at: now(), updated_at: now(),
    };
    backend.upsert_node(&node).await.unwrap();

    // Create FTS index on name for resolution
    // ... (setup index)

    let resolved = backend.resolve_entity("Rust", None).await.unwrap();
    assert!(!resolved.is_empty());
    assert_eq!(resolved[0].name, "Rust");
}
```

**Step 3: Run tests**

Run: `cd core && cargo test store::lance::graph`
Expected: PASS

**Step 4: Commit**

```bash
git add core/src/memory/store/lance/graph.rs
git commit -m "memory: implement GraphStore for LanceDB"
```

---

### Task 10: Implement SessionStore

**Files:**
- Create: `core/src/memory/store/lance/sessions.rs`

**Step 1: Implement `SessionStore` trait for `LanceMemoryBackend`**

Key implementations:
- `insert_memory`: Convert MemoryEntry to RecordBatch, add to memories table
- `search_memories`: Vector search on memories table with context filter
- `get_memories_for_entity`: Lookup graph_edges where relation='entity_mention', get from_id list, filter memories
- `get_recent_memories`: Sort by timestamp DESC with filter
- `delete_memory`: Delete by id
- `get_stats`: Count rows across all tables

**Step 2: Write tests**

```rust
#[tokio::test]
async fn test_session_insert_and_search() {
    let backend = create_test_backend().await;
    let embedding = vec![0.1f32; 384];
    let memory = MemoryEntry::with_embedding(
        "m1".into(),
        ContextAnchor::now("com.test".into(), "test.txt".into()),
        "hello".into(), "world".into(), embedding.clone(),
    );
    backend.insert_memory(&memory).await.unwrap();

    let filter = MemoryFilter::for_context("com.test", "test.txt");
    let results = backend.search_memories(&embedding, &filter, 10).await.unwrap();
    assert_eq!(results.len(), 1);
}
```

**Step 3: Run tests**

Run: `cd core && cargo test store::lance::sessions`
Expected: PASS

**Step 4: Commit**

```bash
git add core/src/memory/store/lance/sessions.rs
git commit -m "memory: implement SessionStore for LanceDB"
```

---

### Task 11: Implement FTS index creation

**Files:**
- Modify: `core/src/memory/store/lance/mod.rs`

**Step 1: Add index creation methods to `LanceMemoryBackend`**

```rust
impl LanceMemoryBackend {
    /// Create FTS and ANN indexes on all tables (idempotent)
    pub async fn ensure_indexes(&self) -> Result<(), AlephError> {
        // FTS on facts.content
        self.create_fts_index_if_needed(&self.facts_table, &["content"]).await?;
        // FTS on graph_nodes.name
        self.create_fts_index_if_needed(&self.nodes_table, &["name"]).await?;
        // FTS on memories.user_input, memories.ai_output
        self.create_fts_index_if_needed(&self.memories_table, &["user_input", "ai_output"]).await?;
        Ok(())
    }

    async fn create_fts_index_if_needed(&self, table: &Table, columns: &[&str]) -> Result<(), AlephError> {
        // LanceDB create_index is idempotent if replace=true
        let fts_builder = lancedb::index::scalar::FtsIndexBuilder::default()
            .with_position(true);
        table.create_index(columns, lancedb::index::Index::FTS(fts_builder))
            .replace(true)
            .execute().await
            .map_err(|e| AlephError::Internal(format!("FTS index: {}", e)))?;
        Ok(())
    }
}
```

**Step 2: Call `ensure_indexes()` at end of `open_or_create`**

Note: Index creation on empty tables may be a no-op in LanceDB. Indexes should be rebuilt after first data insertion. The implementation should handle this gracefully.

**Step 3: Commit**

```bash
git add core/src/memory/store/lance/mod.rs
git commit -m "memory: add FTS and ANN index creation to LanceMemoryBackend"
```

---

## Phase 3: Integration — Wire New Backend Into Existing System

### Task 12: Add LanceDB config section to MemoryConfig

**Files:**
- Modify: `core/src/config/types/memory.rs`

**Step 1: Add `LanceDbConfig` struct and field**

```rust
/// LanceDB-specific configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LanceDbConfig {
    /// Data directory for LanceDB files
    #[serde(default = "default_lance_data_dir")]
    pub data_dir: String,
    /// ANN index type: "IVF_PQ", "IVF_HNSW_SQ", or "none"
    #[serde(default = "default_ann_index_type")]
    pub ann_index_type: String,
    /// Row count threshold to auto-build ANN index
    #[serde(default = "default_ann_index_threshold")]
    pub ann_index_threshold: usize,
    /// FTS tokenizer: "default", "jieba", "simple"
    #[serde(default = "default_fts_tokenizer")]
    pub fts_tokenizer: String,
}
```

Add to `MemoryConfig`: `pub lancedb: LanceDbConfig`

Change `default_vector_db()` to return `"lancedb"`.

**Step 2: Verify**

Run: `cd core && cargo check`
Expected: SUCCESS

**Step 3: Commit**

```bash
git add core/src/config/types/memory.rs
git commit -m "memory: add LanceDB configuration to MemoryConfig"
```

---

### Task 13: Create MemoryBackend facade type

**Files:**
- Modify: `core/src/memory/store/mod.rs`

**Step 1: Add a unified `MemoryBackend` type alias**

This is the bridge type that existing code will migrate to. It wraps `LanceMemoryBackend` behind `Arc` and provides access to all three trait objects:

```rust
use std::sync::Arc;

/// Unified memory backend — provides MemoryStore + GraphStore + SessionStore
pub type MemoryBackend = Arc<LanceMemoryBackend>;
```

All existing `Arc<VectorDatabase>` references will eventually become `MemoryBackend` (which is `Arc<LanceMemoryBackend>`). Since `LanceMemoryBackend` implements all three traits, callers can use it directly or through trait objects.

**Step 2: Add re-exports in `memory/mod.rs`**

```rust
pub use store::{
    MemoryBackend, MemoryStore, GraphStore, SessionStore,
    StoreStats, DecayStats, ResolvedEntity, GraphNode, GraphEdge, PathEntry,
    types::{SearchFilter, ScoredFact, MemoryFilter},
    lance::LanceMemoryBackend,
};
```

**Step 3: Commit**

```bash
git add core/src/memory/store/mod.rs core/src/memory/mod.rs
git commit -m "memory: add MemoryBackend type alias and re-exports"
```

---

### Task 14: Full integration test — end-to-end flow

**Files:**
- Create: `core/src/memory/store/lance/integration_tests.rs`

**Step 1: Write comprehensive integration test**

Test the full flow: create backend → insert facts with embeddings → build indexes → hybrid search → graph operations → verify results.

```rust
#[tokio::test]
async fn test_full_memory_lifecycle() {
    let tmp = tempfile::tempdir().unwrap();
    let backend = LanceMemoryBackend::open_or_create(tmp.path()).await.unwrap();

    // 1. Insert facts with embeddings
    let embedding = vec![0.5f32; 384];
    let mut fact1 = MemoryFact::new("Aleph uses WebSocket for gateway", FactType::Project, vec![]);
    fact1.embedding = Some(embedding.clone());
    backend.insert_fact(&fact1).await.unwrap();

    let mut fact2 = MemoryFact::new("Rust is used for the core system", FactType::Learning, vec![]);
    fact2.embedding = Some(vec![0.3f32; 384]);
    backend.insert_fact(&fact2).await.unwrap();

    // 2. Build indexes
    backend.ensure_indexes().await.unwrap();

    // 3. Hybrid search
    let results = backend.hybrid_search(
        &embedding, 384, "WebSocket gateway",
        0.7, 0.3, &SearchFilter::valid_only(), 10,
    ).await.unwrap();
    assert!(!results.is_empty());
    assert_eq!(results[0].fact.content, "Aleph uses WebSocket for gateway");

    // 4. Graph operations
    let node = GraphNode {
        id: "aleph".into(), name: "Aleph".into(), kind: "project".into(),
        aliases: vec!["aleph-ai".into()], metadata_json: "{}".into(),
        decay_score: 1.0, created_at: 0, updated_at: 0,
    };
    backend.upsert_node(&node).await.unwrap();

    let retrieved = backend.get_node("aleph").await.unwrap();
    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap().name, "Aleph");

    // 5. Stats
    let stats = backend.get_stats().await.unwrap();
    assert_eq!(stats.total_facts, 2);
    assert_eq!(stats.total_graph_nodes, 1);
}
```

**Step 2: Run integration test**

Run: `cd core && cargo test store::lance::integration_tests`
Expected: PASS

**Step 3: Commit**

```bash
git add core/src/memory/store/lance/integration_tests.rs
git commit -m "memory: add full lifecycle integration test for LanceDB backend"
```

---

## Phase 4: Consumer Migration (Incremental)

> **Strategy:** This phase migrates callers from `Arc<VectorDatabase>` to `MemoryBackend` (which is `Arc<LanceMemoryBackend>`). This is done incrementally — each task migrates one logical group of files. The approach:
>
> 1. For each caller, replace `Arc<VectorDatabase>` with `MemoryBackend`
> 2. Replace direct SQLite method calls with trait method calls
> 3. Keep the same public API signatures where possible
> 4. Run tests after each group

### Task 15: Migrate HybridRetrieval

**Files:**
- Modify: `core/src/memory/hybrid_retrieval/hybrid.rs`

Replace `Arc<VectorDatabase>` with `MemoryBackend`. Replace manual vector+FTS fusion logic with `MemoryStore::hybrid_search()` call. The old manual BM25 normalization + score fusion code is no longer needed — LanceDB handles this internally with RRF.

---

### Task 16: Migrate retrieval.rs and fact_retrieval.rs

**Files:**
- Modify: `core/src/memory/retrieval.rs`
- Modify: `core/src/memory/fact_retrieval.rs`

Replace `Arc<VectorDatabase>` with `MemoryBackend`. Update search calls to use `MemoryStore` trait methods.

---

### Task 17: Migrate graph.rs

**Files:**
- Modify: `core/src/memory/graph.rs`

Replace direct SQLite graph queries with `GraphStore` trait method calls. The business logic (entity extraction, co-occurrence edge building, decay scheduling) stays, only the storage calls change.

---

### Task 18: Migrate compression, ingestion, VFS

**Files:**
- Modify: `core/src/memory/compression/service.rs`
- Modify: `core/src/memory/compression/conflict.rs`
- Modify: `core/src/memory/ingestion.rs`
- Modify: `core/src/memory/vfs/mod.rs`
- Modify: `core/src/memory/vfs/l1_generator.rs`

Replace `Arc<VectorDatabase>` with `MemoryBackend` in all compression, ingestion, and VFS modules.

---

### Task 19: Migrate builtin tools

**Files:**
- Modify: `core/src/builtin_tools/memory_search.rs`
- Modify: `core/src/builtin_tools/memory_browse.rs`

Replace `Arc<VectorDatabase>` with `MemoryBackend`. These tools call into retrieval/VFS layers.

---

### Task 20: Migrate gateway handlers

**Files:**
- Modify: `core/src/gateway/handlers/memory.rs`

Replace `Arc<VectorDatabase>` with `MemoryBackend` in the gateway memory handler.

---

### Task 21: Migrate remaining consumers (dispatcher, agent_loop, capability, cortex)

**Files (batch):**
- `core/src/dispatcher/experience_replay_layer.rs`
- `core/src/dispatcher/tool_index/coordinator.rs`
- `core/src/dispatcher/tool_index/retrieval.rs`
- `core/src/dispatcher/model_router/core/context.rs`
- `core/src/agent_loop/meta_cognition_integration.rs`
- `core/src/agent_loop/cortex_telemetry.rs`
- `core/src/capability/mod.rs`
- `core/src/capability/strategy.rs`
- `core/src/capability/strategies/memory.rs`
- `core/src/memory/cortex/*.rs`
- `core/src/memory/dreaming.rs`
- `core/src/memory/lazy_decay.rs`
- `core/src/memory/cleanup.rs`
- `core/src/memory/audit.rs`
- `core/src/memory/cli/commands.rs`
- `core/src/memory/embedding_migration.rs`
- `core/src/memory/evolution/*.rs`
- `core/src/memory/ripple/*.rs`
- `core/src/memory/consolidation/*.rs`
- `core/src/memory/transcript_indexer/*.rs`
- `core/src/memory/database/resilience/**/*.rs`
- `core/src/init_unified/coordinator.rs`

This is the largest task. For each file:
1. Replace `Arc<VectorDatabase>` with `MemoryBackend`
2. Update method calls to use trait methods
3. Verify compilation

After all files compile, run: `cd core && cargo test`

---

### Task 22: Update initialization coordinator

**Files:**
- Modify: `core/src/init_unified/coordinator.rs`

This is where `VectorDatabase::new()` is called to create the database. Replace with `LanceMemoryBackend::open_or_create()` and pass the resulting `MemoryBackend` to all consumers.

---

## Phase 5: Cleanup

### Task 23: Remove old SQLite database code

**Files:**
- Delete: `core/src/memory/database/core.rs` (VectorDatabase)
- Delete: `core/src/memory/database/facts/` (old CRUD/search)
- Delete: `core/src/memory/database/memory_ops.rs`
- Delete: `core/src/memory/database/retention.rs`
- Delete: `core/src/memory/database/compression.rs`
- Delete: `core/src/memory/database/audit.rs`
- Delete: `core/src/memory/database/dreaming.rs`
- Delete: `core/src/memory/database/experiences.rs`
- Modify: `core/src/memory/database/mod.rs` (remove VectorDatabase re-export, keep resilience)

**Note:** The `resilience/` module under `database/` may need special handling — check if it depends on VectorDatabase or can be refactored to use MemoryBackend.

**Step 1: Remove old files and fix imports**
**Step 2: Run full test suite**

Run: `cd core && cargo test`
Expected: PASS

**Step 3: Commit**

```bash
git commit -m "memory: remove old SQLite/sqlite-vec database layer"
```

---

### Task 24: Remove SQLite dependencies from Cargo.toml

**Files:**
- Modify: `core/Cargo.toml`

Remove `rusqlite` and `sqlite-vec` from dependencies. Run `cargo check` to verify nothing else depends on them. If other modules (e.g., sessions database, non-memory tables) still use SQLite, keep the dependency but note it in comments.

**Step 1: Remove or gate dependencies**
**Step 2: Full build and test**

Run: `cd core && cargo build && cargo test`
Expected: SUCCESS

**Step 3: Commit**

```bash
git commit -m "memory: remove rusqlite and sqlite-vec dependencies"
```

---

### Task 25: Update documentation

**Files:**
- Modify: `docs/MEMORY_SYSTEM.md`
- Modify: `CLAUDE.md` (update tech stack section)

Update docs to reflect LanceDB as the memory backend. Remove SQLite/sqlite-vec references from memory system docs.

**Step 1: Update docs**
**Step 2: Commit**

```bash
git commit -m "docs: update memory system docs for LanceDB backend"
```

---

## Summary

| Phase | Tasks | Description |
|-------|-------|-------------|
| **Phase 1** | 1-5 | Foundation: deps, traits, schema, Arrow conversion |
| **Phase 2** | 6-11 | LanceDB backend implementation with full tests |
| **Phase 3** | 12-14 | Integration: config, facade type, e2e test |
| **Phase 4** | 15-22 | Consumer migration (55 files, incremental) |
| **Phase 5** | 23-25 | Cleanup: remove SQLite, update docs |

**Total: 25 tasks across 5 phases.**

**Key risk mitigation:** Phase 4 is the largest — it touches 55+ files. The trait abstraction ensures each file can be migrated independently. If any migration is blocked, the old code continues to work until resolved.
