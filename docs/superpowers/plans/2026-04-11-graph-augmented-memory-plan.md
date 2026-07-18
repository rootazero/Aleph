# Graph-Augmented Memory Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Elevate the knowledge graph from a disconnected appendage to the connective tissue of Aleph's memory system by completing the fact↔node bidirectional index, wiring graph expansion into retrieval, and synchronizing wiki wikilinks to graph edges.

**Architecture:** New `memory_entities` table bridges `facts` and `graph_nodes`. CompressionService auto-links extracted facts to graph nodes. A new `GraphExpander` enriches hybrid retrieval results with structurally related facts. Wiki-specific wikilink→graph sync is isolated in its own module.

**Tech Stack:** Rust, SQLite, async_trait, rusqlite, uuid, chrono

**Spec:** `docs/superpowers/specs/2026-04-11-graph-augmented-memory-design.md`

---

## File Structure

| File | Action | Responsibility |
|------|--------|---------------|
| `src/memory/store/sqlite/schema.rs` | Modify | Add `memory_entities` DDL + init |
| `src/memory/store/mod.rs` | Modify | Extend `GraphStore` trait with 4 new methods |
| `src/memory/store/sqlite/graph.rs` | Modify | Implement new trait methods + decay cascade |
| `src/memory/graph.rs` | Modify | Replace TODO `link_memory_entity` with real impl |
| `src/memory/wiki_sync.rs` | Create | Wikilink↔graph sync for Wiki facts |
| `src/memory/mod.rs` | Modify | Add `pub mod wiki_sync;` |
| `src/memory/compression/service.rs` | Modify | Add step 4y (fact↔node linking) |
| `src/memory/hybrid_retrieval/graph_expander.rs` | Create | `GraphExpander` implementation |
| `src/memory/hybrid_retrieval/mod.rs` | Modify | Re-export `GraphExpander` |
| `src/memory/dreaming/stages/wiki_lint.rs` | Modify | Add `SuggestedLink` from graph topology |
| `src/memory/dreaming/stages/decay.rs` | Modify | Add `memory_entities` cascade on graph decay |
| `src/memory/context/enums.rs` | Modify | Update module doc with Fact definition |

---

## Task 1: Add `memory_entities` DDL to Schema

**Files:**
- Modify: `src/memory/store/sqlite/schema.rs`

- [ ] **Step 1: Add the DDL constant**

After `GRAPH_EDGES_DDL` (line ~112), add:

```rust
const MEMORY_ENTITIES_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS memory_entities (
    id          TEXT PRIMARY KEY,
    fact_id     TEXT NOT NULL,
    node_id     TEXT NOT NULL,
    weight      REAL NOT NULL DEFAULT 1.0,
    source      TEXT NOT NULL DEFAULT 'extracted',
    created_at  INTEGER NOT NULL,
    agent       TEXT,

    UNIQUE(fact_id, node_id)
);

CREATE INDEX IF NOT EXISTS idx_me_fact_id ON memory_entities(fact_id);
CREATE INDEX IF NOT EXISTS idx_me_node_id ON memory_entities(node_id);
CREATE INDEX IF NOT EXISTS idx_me_agent   ON memory_entities(agent);
"#;
```

- [ ] **Step 2: Wire DDL into `init_schema()`**

In `init_schema()` (line ~349), after `GRAPH_EDGES_DDL` execution, add:

```rust
    conn.execute_batch(MEMORY_ENTITIES_DDL)
        .map_err(|e| AlephError::config(format!("Failed to create memory_entities table: {e}")))?;
```

- [ ] **Step 3: Run compile check**

Run: `cargo check -p alephcore`
Expected: compiles with no errors

- [ ] **Step 4: Commit**

```bash
git add src/memory/store/sqlite/schema.rs
git commit -m "memory: add memory_entities DDL for fact↔node bidirectional index"
```

---

## Task 2: Extend `GraphStore` Trait with Association Methods

**Files:**
- Modify: `src/memory/store/mod.rs`

- [ ] **Step 1: Add trait methods to `GraphStore`**

In `trait GraphStore` (after `apply_decay` at line ~388), add:

```rust
    /// Link a fact to a graph node with weight and source.
    async fn link_memory_entity(
        &self,
        fact_id: &str,
        node_id: &str,
        weight: f32,
        source: &str,
        workspace: &str,
    ) -> Result<(), AlephError>;

    /// Get all graph nodes associated with a fact.
    async fn get_nodes_for_fact(
        &self,
        fact_id: &str,
        workspace: &str,
    ) -> Result<Vec<(GraphNode, f32)>, AlephError>;

    /// Get all fact IDs associated with a graph node.
    async fn get_facts_for_node(
        &self,
        node_id: &str,
        workspace: &str,
    ) -> Result<Vec<(String, f32)>, AlephError>;

    /// Remove a fact↔node association.
    async fn unlink_memory_entity(
        &self,
        fact_id: &str,
        node_id: &str,
        workspace: &str,
    ) -> Result<(), AlephError>;

    /// Delete all memory_entities records for a given fact.
    async fn delete_memory_entities_for_fact(
        &self,
        fact_id: &str,
        workspace: &str,
    ) -> Result<usize, AlephError>;

    /// Delete all memory_entities records for a given node.
    async fn delete_memory_entities_for_node(
        &self,
        node_id: &str,
        workspace: &str,
    ) -> Result<usize, AlephError>;
```

- [ ] **Step 2: Run compile check**

Run: `cargo check -p alephcore`
Expected: FAIL — `SqliteMemoryBackend` does not implement the new methods yet. This confirms the trait extension is correct.

- [ ] **Step 3: Commit**

```bash
git add src/memory/store/mod.rs
git commit -m "memory: extend GraphStore trait with fact↔node association methods"
```

---

## Task 3: Implement Association Methods in SQLite Backend

**Files:**
- Modify: `src/memory/store/sqlite/graph.rs`

- [ ] **Step 1: Write unit tests for the new methods**

At the bottom of the `#[cfg(test)] mod tests` block in `src/memory/store/sqlite/graph.rs`, add:

```rust
    #[tokio::test]
    async fn test_link_and_get_nodes_for_fact() {
        let (_tmp, backend) = create_test_backend();
        let node = make_test_node("gn-001", "Rust", "language");
        backend.upsert_node(&node, "default").await.unwrap();

        backend
            .link_memory_entity("fact-001", "gn-001", 0.8, "extracted", "default")
            .await
            .unwrap();

        let nodes = backend.get_nodes_for_fact("fact-001", "default").await.unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].0.id, "gn-001");
        assert!((nodes[0].1 - 0.8).abs() < f32::EPSILON);
    }

    #[tokio::test]
    async fn test_get_facts_for_node() {
        let (_tmp, backend) = create_test_backend();
        let node = make_test_node("gn-001", "Rust", "language");
        backend.upsert_node(&node, "default").await.unwrap();

        backend
            .link_memory_entity("fact-001", "gn-001", 0.9, "extracted", "default")
            .await
            .unwrap();
        backend
            .link_memory_entity("fact-002", "gn-001", 0.7, "wikilink", "default")
            .await
            .unwrap();

        let facts = backend.get_facts_for_node("gn-001", "default").await.unwrap();
        assert_eq!(facts.len(), 2);
    }

    #[tokio::test]
    async fn test_link_upserts_on_duplicate() {
        let (_tmp, backend) = create_test_backend();
        let node = make_test_node("gn-001", "Rust", "language");
        backend.upsert_node(&node, "default").await.unwrap();

        backend
            .link_memory_entity("fact-001", "gn-001", 0.5, "extracted", "default")
            .await
            .unwrap();
        // Upsert with higher weight
        backend
            .link_memory_entity("fact-001", "gn-001", 0.9, "extracted", "default")
            .await
            .unwrap();

        let nodes = backend.get_nodes_for_fact("fact-001", "default").await.unwrap();
        assert_eq!(nodes.len(), 1);
        assert!((nodes[0].1 - 0.9).abs() < f32::EPSILON);
    }

    #[tokio::test]
    async fn test_unlink_memory_entity() {
        let (_tmp, backend) = create_test_backend();
        let node = make_test_node("gn-001", "Rust", "language");
        backend.upsert_node(&node, "default").await.unwrap();

        backend
            .link_memory_entity("fact-001", "gn-001", 0.8, "extracted", "default")
            .await
            .unwrap();
        backend
            .unlink_memory_entity("fact-001", "gn-001", "default")
            .await
            .unwrap();

        let nodes = backend.get_nodes_for_fact("fact-001", "default").await.unwrap();
        assert!(nodes.is_empty());
    }

    #[tokio::test]
    async fn test_delete_memory_entities_for_fact() {
        let (_tmp, backend) = create_test_backend();
        let node_a = make_test_node("gn-a", "Rust", "language");
        let node_b = make_test_node("gn-b", "Python", "language");
        backend.upsert_node(&node_a, "default").await.unwrap();
        backend.upsert_node(&node_b, "default").await.unwrap();

        backend.link_memory_entity("fact-001", "gn-a", 0.8, "extracted", "default").await.unwrap();
        backend.link_memory_entity("fact-001", "gn-b", 0.7, "wikilink", "default").await.unwrap();

        let deleted = backend.delete_memory_entities_for_fact("fact-001", "default").await.unwrap();
        assert_eq!(deleted, 2);

        let nodes = backend.get_nodes_for_fact("fact-001", "default").await.unwrap();
        assert!(nodes.is_empty());
    }

    #[tokio::test]
    async fn test_delete_memory_entities_for_node() {
        let (_tmp, backend) = create_test_backend();
        let node = make_test_node("gn-001", "Rust", "language");
        backend.upsert_node(&node, "default").await.unwrap();

        backend.link_memory_entity("fact-001", "gn-001", 0.8, "extracted", "default").await.unwrap();
        backend.link_memory_entity("fact-002", "gn-001", 0.7, "extracted", "default").await.unwrap();

        let deleted = backend.delete_memory_entities_for_node("gn-001", "default").await.unwrap();
        assert_eq!(deleted, 2);

        let facts = backend.get_facts_for_node("gn-001", "default").await.unwrap();
        assert!(facts.is_empty());
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib memory::store::sqlite::graph::tests -- --nocapture 2>&1 | head -30`
Expected: FAIL — methods not implemented yet

- [ ] **Step 3: Implement the trait methods**

In `src/memory/store/sqlite/graph.rs`, inside `#[async_trait] impl GraphStore for SqliteMemoryBackend`, add after the `apply_decay` method:

```rust
    async fn link_memory_entity(
        &self,
        fact_id: &str,
        node_id: &str,
        weight: f32,
        source: &str,
        workspace: &str,
    ) -> Result<(), AlephError> {
        let conn = lock_conn!(self)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let id = format!("me_{}", uuid::Uuid::new_v4());

        conn.execute(
            "INSERT INTO memory_entities (id, fact_id, node_id, weight, source, created_at, agent) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
             ON CONFLICT(fact_id, node_id) DO UPDATE SET weight = ?4, source = ?5",
            params![id, fact_id, node_id, weight as f64, source, now, workspace],
        )
        .map_err(|e| AlephError::config(format!("link_memory_entity: {e}")))?;

        Ok(())
    }

    async fn get_nodes_for_fact(
        &self,
        fact_id: &str,
        workspace: &str,
    ) -> Result<Vec<(GraphNode, f32)>, AlephError> {
        let conn = lock_conn!(self)?;

        let mut stmt = conn
            .prepare(
                "SELECT gn.*, me.weight FROM memory_entities me \
                 JOIN graph_nodes gn ON gn.id = me.node_id \
                 WHERE me.fact_id = ?1 AND me.agent = ?2",
            )
            .map_err(|e| AlephError::config(format!("get_nodes_for_fact prepare: {e}")))?;

        let rows = stmt
            .query_map(params![fact_id, workspace], |row| {
                let node = row_to_node(row)?;
                let weight: f64 = row.get("weight")?;
                Ok((node, weight as f32))
            })
            .map_err(|e| AlephError::config(format!("get_nodes_for_fact query: {e}")))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(
                row.map_err(|e| AlephError::config(format!("get_nodes_for_fact row: {e}")))?
            );
        }
        Ok(results)
    }

    async fn get_facts_for_node(
        &self,
        node_id: &str,
        workspace: &str,
    ) -> Result<Vec<(String, f32)>, AlephError> {
        let conn = lock_conn!(self)?;

        let mut stmt = conn
            .prepare(
                "SELECT fact_id, weight FROM memory_entities \
                 WHERE node_id = ?1 AND agent = ?2",
            )
            .map_err(|e| AlephError::config(format!("get_facts_for_node prepare: {e}")))?;

        let rows = stmt
            .query_map(params![node_id, workspace], |row| {
                let fact_id: String = row.get(0)?;
                let weight: f64 = row.get(1)?;
                Ok((fact_id, weight as f32))
            })
            .map_err(|e| AlephError::config(format!("get_facts_for_node query: {e}")))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(
                row.map_err(|e| AlephError::config(format!("get_facts_for_node row: {e}")))?
            );
        }
        Ok(results)
    }

    async fn unlink_memory_entity(
        &self,
        fact_id: &str,
        node_id: &str,
        workspace: &str,
    ) -> Result<(), AlephError> {
        let conn = lock_conn!(self)?;
        conn.execute(
            "DELETE FROM memory_entities WHERE fact_id = ?1 AND node_id = ?2 AND agent = ?3",
            params![fact_id, node_id, workspace],
        )
        .map_err(|e| AlephError::config(format!("unlink_memory_entity: {e}")))?;
        Ok(())
    }

    async fn delete_memory_entities_for_fact(
        &self,
        fact_id: &str,
        workspace: &str,
    ) -> Result<usize, AlephError> {
        let conn = lock_conn!(self)?;
        let count = conn
            .execute(
                "DELETE FROM memory_entities WHERE fact_id = ?1 AND agent = ?2",
                params![fact_id, workspace],
            )
            .map_err(|e| AlephError::config(format!("delete_memory_entities_for_fact: {e}")))?;
        Ok(count)
    }

    async fn delete_memory_entities_for_node(
        &self,
        node_id: &str,
        workspace: &str,
    ) -> Result<usize, AlephError> {
        let conn = lock_conn!(self)?;
        let count = conn
            .execute(
                "DELETE FROM memory_entities WHERE node_id = ?1 AND agent = ?2",
                params![node_id, workspace],
            )
            .map_err(|e| AlephError::config(format!("delete_memory_entities_for_node: {e}")))?;
        Ok(count)
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib memory::store::sqlite::graph::tests -- --nocapture`
Expected: all new tests PASS

- [ ] **Step 5: Commit**

```bash
git add src/memory/store/sqlite/graph.rs
git commit -m "memory: implement fact↔node association methods in SQLite backend"
```

---

## Task 4: Replace TODO in `graph.rs` Wrapper

**Files:**
- Modify: `src/memory/graph.rs`

- [ ] **Step 1: Replace the TODO `link_memory_entity` method**

In `src/memory/graph.rs`, replace the existing TODO method (line ~382-391):

```rust
    /// Link a memory to a graph entity.
    ///
    /// TODO: Implement via MemoryBackend when memory_entities support is added.
    pub async fn link_memory_entity(
        &self,
        _memory_id: &str,
        _node_id: &str,
        _weight: f32,
        _source: &str,
    ) -> Result<(), AlephError> {
        // TODO: Delegate to store trait when memory_entities table is available in Lance
        Ok(())
    }
```

With:

```rust
    /// Link a memory fact to a graph entity via `memory_entities`.
    pub async fn link_memory_entity(
        &self,
        fact_id: &str,
        node_id: &str,
        weight: f32,
        source: &str,
    ) -> Result<(), AlephError> {
        use crate::memory::store::GraphStore as StoreGraphStore;
        StoreGraphStore::link_memory_entity(
            self.database.as_ref(), fact_id, node_id, weight, source, "default",
        )
        .await
    }
```

- [ ] **Step 2: Replace the TODO `get_memory_ids_for_entity` method**

Replace the existing TODO method (line ~421-428):

```rust
    /// Get memory IDs linked to an entity.
    ///
    /// TODO: Implement via MemoryBackend when memory_entities support is added.
    pub async fn get_memory_ids_for_entity(
        &self,
        _node_id: &str,
    ) -> Result<Vec<String>, AlephError> {
        // TODO: Delegate to store trait when memory_entities table is available in Lance
        Ok(Vec::new())
    }
```

With:

```rust
    /// Get fact IDs linked to a graph entity via `memory_entities`.
    pub async fn get_fact_ids_for_entity(
        &self,
        node_id: &str,
    ) -> Result<Vec<(String, f32)>, AlephError> {
        use crate::memory::store::GraphStore as StoreGraphStore;
        StoreGraphStore::get_facts_for_node(
            self.database.as_ref(), node_id, "default",
        )
        .await
    }

    /// Get graph nodes linked to a fact via `memory_entities`.
    pub async fn get_nodes_for_fact(
        &self,
        fact_id: &str,
    ) -> Result<Vec<(GraphNode, f32)>, AlephError> {
        use crate::memory::store::GraphStore as StoreGraphStore;
        let store_nodes = StoreGraphStore::get_nodes_for_fact(
            self.database.as_ref(), fact_id, "default",
        )
        .await?;
        Ok(store_nodes
            .into_iter()
            .map(|(sn, w)| (GraphNode::from(sn), w))
            .collect())
    }
```

- [ ] **Step 3: Fix any call sites that referenced the old method name**

Run: `cargo check -p alephcore 2>&1 | head -40`

Search for uses of `get_memory_ids_for_entity` and update to `get_fact_ids_for_entity`. If no call sites exist (current code doesn't call the TODO method), this step is a no-op.

- [ ] **Step 4: Verify compilation**

Run: `cargo check -p alephcore`
Expected: compiles with no errors

- [ ] **Step 5: Commit**

```bash
git add src/memory/graph.rs
git commit -m "memory: replace TODO link_memory_entity with real implementation"
```

---

## Task 5: Create Wiki Sync Module

**Files:**
- Create: `src/memory/wiki_sync.rs`
- Modify: `src/memory/mod.rs`

- [ ] **Step 1: Create the wiki_sync module**

Create `src/memory/wiki_sync.rs`:

```rust
//! Wiki wikilink ↔ graph synchronization.
//!
//! Parses `[[wikilink]]` from Wiki-type facts and synchronizes them
//! as `references` edges in the knowledge graph, plus `memory_entities`
//! associations with `source=wikilink`.

use crate::error::AlephError;
use crate::memory::context::{FactType, MemoryFact};
use crate::memory::graph::GraphStore;
use crate::memory::store::GraphStore as StoreGraphStore;
use crate::wiki::wikilink::extract_wikilinks;

/// Synchronize wikilinks in a Wiki fact to the knowledge graph.
///
/// For each `[[target]]` found in the fact content:
/// 1. Upsert a graph node for the target (kind="wiki")
/// 2. Create a `memory_entities` link (source="wikilink", weight=1.0)
/// 3. Create a `references` edge from source page to target page
///
/// Only operates on `FactType::Wiki` facts; returns `Ok(())` for others.
pub async fn sync_wikilinks_to_graph(
    fact: &MemoryFact,
    graph_store: &GraphStore,
) -> Result<SyncReport, AlephError> {
    let mut report = SyncReport::default();

    if fact.fact_type != FactType::Wiki {
        return Ok(report);
    }

    let wikilinks = extract_wikilinks(&fact.content);
    if wikilinks.is_empty() {
        return Ok(report);
    }

    // Clear existing wikilink associations for this fact before re-syncing.
    // Only clears source="wikilink"; source="extracted" associations are untouched.
    let db = graph_store.database_ref();
    let cleared = StoreGraphStore::delete_memory_entities_by_source(
        db, &fact.id, "wikilink", "default",
    )
    .await
    .unwrap_or(0);
    report.cleared = cleared;

    // Derive source page slug from fact path
    let source_slug = fact
        .path
        .split('/')
        .next_back()
        .unwrap_or(&fact.id)
        .trim_end_matches(".md");

    let source_node = graph_store
        .upsert_node(source_slug, "wiki", &[], None)
        .await?;

    for link_target in &wikilinks {
        // 1. Upsert target node
        let target_node = graph_store
            .upsert_node(link_target, "wiki", &[], None)
            .await?;

        // 2. Link fact → target node
        graph_store
            .link_memory_entity(&fact.id, &target_node.id, 1.0, "wikilink")
            .await?;

        // 3. Create references edge
        graph_store
            .upsert_edge(
                &source_node.id,
                &target_node.id,
                "references",
                "",
                1.0,
                1.0,
            )
            .await?;

        report.linked += 1;
    }

    Ok(report)
}

/// Summary of a wikilink sync operation.
#[derive(Debug, Clone, Default)]
pub struct SyncReport {
    /// Number of old wikilink associations cleared.
    pub cleared: usize,
    /// Number of new wikilink associations created.
    pub linked: usize,
}
```

- [ ] **Step 2: Add `delete_memory_entities_by_source` to GraphStore trait and impl**

In `src/memory/store/mod.rs`, add to `trait GraphStore`:

```rust
    /// Delete memory_entities records for a fact filtered by source.
    async fn delete_memory_entities_by_source(
        &self,
        fact_id: &str,
        source: &str,
        workspace: &str,
    ) -> Result<usize, AlephError>;
```

In `src/memory/store/sqlite/graph.rs`, add the implementation:

```rust
    async fn delete_memory_entities_by_source(
        &self,
        fact_id: &str,
        source: &str,
        workspace: &str,
    ) -> Result<usize, AlephError> {
        let conn = lock_conn!(self)?;
        let count = conn
            .execute(
                "DELETE FROM memory_entities WHERE fact_id = ?1 AND source = ?2 AND agent = ?3",
                params![fact_id, source, workspace],
            )
            .map_err(|e| AlephError::config(format!("delete_memory_entities_by_source: {e}")))?;
        Ok(count)
    }
```

- [ ] **Step 3: Add `database_ref()` accessor to `graph::GraphStore`**

In `src/memory/graph.rs`, add to `impl GraphStore`:

```rust
    /// Access the underlying MemoryBackend for direct trait calls.
    pub fn database_ref(&self) -> &dyn crate::memory::store::GraphStore {
        self.database.as_ref()
    }
```

- [ ] **Step 4: Register the module**

In `src/memory/mod.rs`, add alongside existing module declarations:

```rust
pub mod wiki_sync;
```

- [ ] **Step 5: Verify compilation**

Run: `cargo check -p alephcore`
Expected: compiles with no errors

- [ ] **Step 6: Commit**

```bash
git add src/memory/wiki_sync.rs src/memory/mod.rs src/memory/store/mod.rs src/memory/store/sqlite/graph.rs src/memory/graph.rs
git commit -m "memory: add wiki_sync module for wikilink↔graph synchronization"
```

---

## Task 6: Wire Fact↔Node Linking into CompressionService

**Files:**
- Modify: `src/memory/compression/service.rs`

- [ ] **Step 1: Add step 4y after step 4x**

In `compress_in_workspace()`, after the `upsert_relationship` loop (line ~360), before the comment `// 4a. Invalidate consumed raw chunks`, add:

```rust
        // 4y. Build fact ↔ graph node associations for stored facts
        for fact_id in &stored_fact_ids {
            // Retrieve the stored fact to get its content
            if let Ok(Some(stored_fact)) = self.database.get_fact(fact_id).await {
                let entity_names =
                    crate::memory::graph::GraphStore::extract_entities_from_text(&stored_fact.content);
                for name in &entity_names {
                    if let Ok(resolved) = self.graph_store.resolve_entity(name, None).await {
                        if let Some(best) = resolved.first() {
                            if let Err(e) = self
                                .graph_store
                                .link_memory_entity(fact_id, &best.node_id, 0.8, "extracted")
                                .await
                            {
                                tracing::warn!(
                                    error = %e,
                                    fact_id = %fact_id,
                                    node = %best.node_id,
                                    "Failed to link fact to entity"
                                );
                            }
                        }
                    }
                }

                // Wiki-specific: sync wikilinks to graph
                if stored_fact.fact_type == crate::memory::context::FactType::Wiki {
                    if let Err(e) =
                        crate::memory::wiki_sync::sync_wikilinks_to_graph(&stored_fact, &self.graph_store)
                            .await
                    {
                        tracing::warn!(error = %e, fact_id = %fact_id, "Failed to sync wikilinks");
                    }
                }
            }
        }
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p alephcore`
Expected: compiles with no errors

- [ ] **Step 3: Commit**

```bash
git add src/memory/compression/service.rs
git commit -m "memory: wire fact↔node linking and wiki sync into CompressionService"
```

---

## Task 7: Create GraphExpander for Retrieval

**Files:**
- Create: `src/memory/hybrid_retrieval/graph_expander.rs`
- Modify: `src/memory/hybrid_retrieval/mod.rs`

- [ ] **Step 1: Create the GraphExpander module**

Create `src/memory/hybrid_retrieval/graph_expander.rs`:

```rust
//! Graph-augmented retrieval expansion.
//!
//! Given a set of candidate facts from vector + FTS search, expands them
//! by traversing the knowledge graph to discover structurally related facts
//! that may not be semantically similar but are knowledge-linked.

use std::collections::HashSet;

use crate::error::AlephError;
use crate::memory::store::types::ScoredFact;
use crate::memory::store::{GraphStore, MemoryBackend, MemoryStore};

/// Configuration for graph expansion.
#[derive(Debug, Clone)]
pub struct GraphExpansionConfig {
    /// Whether graph expansion is enabled (default: true).
    pub enabled: bool,
    /// Maximum traversal hops (default: 1, direct neighbors only).
    pub max_hops: usize,
    /// Maximum expanded facts per seed fact (default: 3).
    pub max_expanded_per_seed: usize,
    /// Total cap on expanded results (default: 10).
    pub max_total_expanded: usize,
    /// Minimum association weight to include (default: 0.3).
    pub min_weight: f32,
    /// Score decay factor per hop (default: 0.7).
    pub hop_decay: f32,
}

impl Default for GraphExpansionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_hops: 1,
            max_expanded_per_seed: 3,
            max_total_expanded: 10,
            min_weight: 0.3,
            hop_decay: 0.7,
        }
    }
}

/// A fact discovered through graph expansion.
#[derive(Debug, Clone)]
pub struct ExpandedFact {
    /// The discovered fact (with adjusted score).
    pub scored_fact: ScoredFact,
    /// The seed fact ID that led to this discovery.
    pub seed_fact_id: String,
    /// The graph path description (e.g., "via entity 'Rust' → co_occurs → entity 'Aleph'").
    pub expansion_path: String,
}

/// Expands retrieval results using knowledge graph traversal.
pub struct GraphExpander {
    database: MemoryBackend,
    config: GraphExpansionConfig,
}

impl GraphExpander {
    pub fn new(database: MemoryBackend, config: GraphExpansionConfig) -> Self {
        Self { database, config }
    }

    /// Expand a set of seed facts by traversing the knowledge graph.
    ///
    /// For each seed fact:
    /// 1. Look up associated graph nodes via `memory_entities`
    /// 2. Traverse edges to find neighbor nodes
    /// 3. Look up facts associated with neighbor nodes
    /// 4. Score and filter expanded facts
    pub async fn expand(
        &self,
        seeds: &[ScoredFact],
        workspace: &str,
    ) -> Result<Vec<ExpandedFact>, AlephError> {
        if !self.config.enabled || seeds.is_empty() {
            return Ok(Vec::new());
        }

        let seed_ids: HashSet<String> = seeds.iter().map(|s| s.fact.id.clone()).collect();
        let mut all_expanded: Vec<ExpandedFact> = Vec::new();

        for seed in seeds {
            if all_expanded.len() >= self.config.max_total_expanded {
                break;
            }

            let mut per_seed_count = 0;

            // 1. Get graph nodes associated with this seed fact
            let nodes = GraphStore::get_nodes_for_fact(
                self.database.as_ref(),
                &seed.fact.id,
                workspace,
            )
            .await?;

            for (node, link_weight) in &nodes {
                if per_seed_count >= self.config.max_expanded_per_seed {
                    break;
                }
                if *link_weight < self.config.min_weight {
                    continue;
                }

                // 2. Get edges from this node to neighbors
                let edges = GraphStore::get_edges_for_node(
                    self.database.as_ref(),
                    &node.id,
                    None,
                    workspace,
                )
                .await?;

                for edge in &edges {
                    if per_seed_count >= self.config.max_expanded_per_seed {
                        break;
                    }

                    let neighbor_id = if edge.from_id == node.id {
                        &edge.to_id
                    } else {
                        &edge.from_id
                    };

                    // 3. Get facts associated with the neighbor node
                    let neighbor_facts = GraphStore::get_facts_for_node(
                        self.database.as_ref(),
                        neighbor_id,
                        workspace,
                    )
                    .await?;

                    for (fact_id, fact_link_weight) in &neighbor_facts {
                        if per_seed_count >= self.config.max_expanded_per_seed {
                            break;
                        }
                        if all_expanded.len() >= self.config.max_total_expanded {
                            break;
                        }

                        // Skip if already a seed or already expanded
                        if seed_ids.contains(fact_id) {
                            continue;
                        }
                        if all_expanded.iter().any(|e| e.scored_fact.fact.id == *fact_id) {
                            continue;
                        }
                        if *fact_link_weight < self.config.min_weight {
                            continue;
                        }

                        // 4. Load the fact and score it
                        if let Ok(Some(fact)) =
                            MemoryStore::get_fact(self.database.as_ref(), fact_id).await
                        {
                            if !fact.is_valid {
                                continue;
                            }

                            let expanded_score = seed.score
                                * edge.weight
                                * link_weight
                                * fact_link_weight
                                * self.config.hop_decay;

                            let path_desc = format!(
                                "via entity '{}' → {} → neighbor",
                                node.name, edge.relation,
                            );

                            all_expanded.push(ExpandedFact {
                                scored_fact: ScoredFact {
                                    fact,
                                    score: expanded_score,
                                },
                                seed_fact_id: seed.fact.id.clone(),
                                expansion_path: path_desc,
                            });

                            per_seed_count += 1;
                        }
                    }
                }
            }
        }

        // Sort by score descending
        all_expanded.sort_by(|a, b| {
            b.scored_fact
                .score
                .partial_cmp(&a.scored_fact.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Truncate to max_total_expanded
        all_expanded.truncate(self.config.max_total_expanded);

        Ok(all_expanded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_conservative_values() {
        let config = GraphExpansionConfig::default();
        assert!(config.enabled);
        assert_eq!(config.max_hops, 1);
        assert_eq!(config.max_expanded_per_seed, 3);
        assert_eq!(config.max_total_expanded, 10);
        assert!((config.min_weight - 0.3).abs() < f32::EPSILON);
        assert!((config.hop_decay - 0.7).abs() < f32::EPSILON);
    }
}
```

- [ ] **Step 2: Register the module**

In `src/memory/hybrid_retrieval/mod.rs`, add:

```rust
pub mod graph_expander;

pub use graph_expander::{ExpandedFact, GraphExpander, GraphExpansionConfig};
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p alephcore`
Expected: compiles with no errors

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore --lib memory::hybrid_retrieval::graph_expander -- --nocapture`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/memory/hybrid_retrieval/graph_expander.rs src/memory/hybrid_retrieval/mod.rs
git commit -m "memory: add GraphExpander for graph-augmented retrieval"
```

---

## Task 8: Add Decay Cascade for `memory_entities`

**Files:**
- Modify: `src/memory/store/sqlite/graph.rs` (decay cascade)
- Modify: `src/memory/store/sqlite/facts.rs` (invalidation cascade)

- [ ] **Step 1: Add cascade to `apply_decay` node pruning**

In `src/memory/store/sqlite/graph.rs`, in the `apply_decay` method, inside the `if new_score < policy.min_score` block for nodes (around line ~386), add before the node deletion:

```rust
                    // Cascade: delete memory_entities for this pruned node
                    conn.execute(
                        "DELETE FROM memory_entities WHERE node_id = ?1 AND agent = ?2",
                        params![node.id, workspace],
                    )
                    .map_err(|e| {
                        let _ = conn.execute_batch("ROLLBACK");
                        AlephError::config(format!("apply_decay cascade memory_entities: {e}"))
                    })?;
```

- [ ] **Step 2: Add cascade to `invalidate_fact`**

In `src/memory/store/sqlite/facts.rs`, in the `invalidate_fact` method (line ~1000), after the `self.update_fact(&fact).await` call, add cascade cleanup:

```rust
    async fn invalidate_fact(&self, id: &str, reason: &str) -> Result<(), AlephError> {
        let existing = self.get_fact(id).await?;
        let mut fact =
            existing.ok_or_else(|| AlephError::NotFound(format!("Fact '{}'", id)))?;

        fact.is_valid = false;
        fact.invalidation_reason = Some(reason.to_string());
        fact.decay_invalidated_at = Some(now_unix());
        fact.updated_at = now_unix();

        self.update_fact(&fact).await?;

        // Cascade: clean up memory_entities for invalidated fact
        use crate::memory::store::GraphStore;
        let _ = GraphStore::delete_memory_entities_for_fact(self, id, &fact.agent).await;

        Ok(())
    }
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p alephcore`
Expected: compiles with no errors

- [ ] **Step 4: Commit**

```bash
git add src/memory/store/sqlite/graph.rs src/memory/store/sqlite/facts.rs
git commit -m "memory: add memory_entities cascade on fact invalidation and node pruning"
```

---

## Task 9: Enhance Wiki Lint with Suggested Links

**Files:**
- Modify: `src/memory/dreaming/stages/wiki_lint.rs`

- [ ] **Step 1: Add `SuggestedLink` struct**

At the top of the file, after `WikiLintReport`, add:

```rust
/// A suggested wikilink based on graph topology.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SuggestedLink {
    /// Source wiki page slug.
    pub from_page: String,
    /// Suggested target wiki page slug.
    pub to_page: String,
    /// Reason for suggestion (graph relation type).
    pub reason: String,
    /// Association confidence.
    pub confidence: f32,
}
```

And add the field to `WikiLintReport`:

```rust
#[derive(Debug, Clone, Default, Serialize)]
pub struct WikiLintReport {
    pub broken_links: Vec<(String, String)>,
    pub orphan_pages: Vec<String>,
    pub stale_pages: Vec<String>,
    pub suggested_pages: Vec<String>,
    pub suggested_links: Vec<SuggestedLink>,
    pub auto_fixed: usize,
}
```

- [ ] **Step 2: Add graph-based link suggestion logic**

At the end of the `execute` method, before `Ok(ctx)`, add:

```rust
        // Graph-based link suggestions: find wiki nodes connected by graph edges
        // but not connected by wikilinks in content.
        if let Some(graph_store) = &ctx.graph_store_wrapper {
            for fact in &wiki_facts {
                let slug = fact
                    .path
                    .split('/')
                    .next_back()
                    .unwrap_or(&fact.id)
                    .trim_end_matches(".md")
                    .to_string();

                let content_links: std::collections::HashSet<String> =
                    extract_wikilinks(&fact.content)
                        .into_iter()
                        .map(|l| l.to_lowercase())
                        .collect();

                // Find nodes associated with this fact
                use crate::memory::store::GraphStore as StoreGS;
                if let Ok(nodes) =
                    StoreGS::get_nodes_for_fact(ctx.database.as_ref(), &fact.id, "default").await
                {
                    for (node, _weight) in &nodes {
                        // Get neighbor nodes via edges
                        if let Ok(edges) = StoreGS::get_edges_for_node(
                            ctx.database.as_ref(),
                            &node.id,
                            None,
                            "default",
                        )
                        .await
                        {
                            for edge in &edges {
                                let neighbor_id = if edge.from_id == node.id {
                                    &edge.to_id
                                } else {
                                    &edge.from_id
                                };
                                if let Ok(Some(neighbor)) =
                                    StoreGS::get_node(ctx.database.as_ref(), neighbor_id, "default")
                                        .await
                                {
                                    if neighbor.kind == "wiki"
                                        && !content_links.contains(&neighbor.name.to_lowercase())
                                        && neighbor.name.to_lowercase() != slug.to_lowercase()
                                    {
                                        report.suggested_links.push(SuggestedLink {
                                            from_page: slug.clone(),
                                            to_page: neighbor.name.clone(),
                                            reason: edge.relation.clone(),
                                            confidence: edge.weight,
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        info!(
            broken = report.broken_links.len(),
            orphans = report.orphan_pages.len(),
            suggested_links = report.suggested_links.len(),
            "WikiLintStage complete"
        );
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p alephcore`
Expected: compiles (may need to adjust based on `DreamContext` field names — check if `graph_store_wrapper` or similar exists)

- [ ] **Step 4: Commit**

```bash
git add src/memory/dreaming/stages/wiki_lint.rs
git commit -m "memory: enhance wiki lint with graph-based suggested links"
```

---

## Task 10: Update Fact Definition Documentation

**Files:**
- Modify: `src/memory/context/enums.rs`

- [ ] **Step 1: Update module doc comment**

Replace the existing module doc (line 1):

```rust
//! Enum definitions for memory fact classification and metadata.
```

With:

```rust
//! Enum definitions for memory fact classification and metadata.
//!
//! In Aleph's memory system, a "Fact" ([`MemoryFact`](super::MemoryFact)) is the
//! universal unit of persisted knowledge — not limited to factual statements, but
//! encompassing preferences, wiki pages, skills, transcripts, synthesized insights,
//! and agent experiences. Each Fact is connected to the knowledge graph via the
//! `memory_entities` table, enabling structural retrieval across all knowledge types.
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p alephcore`
Expected: compiles with no errors

- [ ] **Step 3: Run full test suite**

Run: `cargo test -p alephcore --lib`
Expected: all tests PASS

- [ ] **Step 4: Commit**

```bash
git add src/memory/context/enums.rs
git commit -m "docs: update Fact definition to reflect universal knowledge unit semantics"
```

---

## Task 11: Integration Test — End-to-End Flow

**Files:**
- Modify: `src/memory/integration_tests/mod.rs` (or create a new test file)

- [ ] **Step 1: Write integration test**

Add a test that validates the full flow: create a fact → extract entities → link to graph → query via graph expansion:

```rust
#[tokio::test]
async fn test_graph_augmented_retrieval_flow() {
    // 1. Set up test backend
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test_integration.db");
    let backend = crate::memory::store::SqliteMemoryBackend::new(&db_path).unwrap();
    let backend = std::sync::Arc::new(backend);

    // 2. Create two facts about related topics
    use crate::memory::context::{FactType, MemoryFact};
    let fact_a = MemoryFact::new("The user prefers Rust for systems programming", FactType::Preference);
    let fact_b = MemoryFact::new("Aleph is built with Rust and uses axum", FactType::Project);

    use crate::memory::store::MemoryStore;
    backend.insert_fact(&fact_a).await.unwrap();
    backend.insert_fact(&fact_b).await.unwrap();

    // 3. Create graph node "Rust" and link both facts
    use crate::memory::store::GraphStore;
    let rust_node = crate::memory::store::GraphNode {
        id: "gn-rust".to_string(),
        name: "Rust".to_string(),
        kind: "technology".to_string(),
        aliases: vec![],
        metadata_json: String::new(),
        decay_score: 1.0,
        created_at: 1700000000,
        updated_at: 1700000000,
        agent: "default".to_string(),
    };
    backend.upsert_node(&rust_node, "default").await.unwrap();

    backend.link_memory_entity(&fact_a.id, "gn-rust", 0.8, "extracted", "default").await.unwrap();
    backend.link_memory_entity(&fact_b.id, "gn-rust", 0.9, "extracted", "default").await.unwrap();

    // 4. Verify bidirectional lookups
    let nodes = backend.get_nodes_for_fact(&fact_a.id, "default").await.unwrap();
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].0.name, "Rust");

    let facts = backend.get_facts_for_node("gn-rust", "default").await.unwrap();
    assert_eq!(facts.len(), 2);

    // 5. Test GraphExpander
    use crate::memory::hybrid_retrieval::graph_expander::{GraphExpander, GraphExpansionConfig};
    use crate::memory::store::types::ScoredFact;

    // Create an edge so fact_b is reachable from fact_a via graph
    let aleph_node = crate::memory::store::GraphNode {
        id: "gn-aleph".to_string(),
        name: "Aleph".to_string(),
        kind: "project".to_string(),
        aliases: vec![],
        metadata_json: String::new(),
        decay_score: 1.0,
        created_at: 1700000000,
        updated_at: 1700000000,
        agent: "default".to_string(),
    };
    backend.upsert_node(&aleph_node, "default").await.unwrap();
    backend.link_memory_entity(&fact_b.id, "gn-aleph", 0.9, "extracted", "default").await.unwrap();

    let edge = crate::memory::store::GraphEdge {
        id: "ge-rust-aleph".to_string(),
        from_id: "gn-rust".to_string(),
        to_id: "gn-aleph".to_string(),
        relation: "used_by".to_string(),
        weight: 1.0,
        confidence: 0.9,
        context_key: String::new(),
        decay_score: 1.0,
        created_at: 1700000000,
        updated_at: 1700000000,
        last_seen_at: 1700000000,
        agent: "default".to_string(),
    };
    backend.upsert_edge(&edge, "default").await.unwrap();

    // Seed with fact_a only; fact_b should be discovered via graph
    let seeds = vec![ScoredFact {
        fact: fact_a.clone(),
        score: 0.9,
    }];

    let expander = GraphExpander::new(backend.clone(), GraphExpansionConfig::default());
    let expanded = expander.expand(&seeds, "default").await.unwrap();

    assert!(!expanded.is_empty(), "Should discover fact_b via graph expansion");
    assert_eq!(expanded[0].scored_fact.fact.id, fact_b.id);
    assert!(
        expanded[0].scored_fact.score < 0.9,
        "Expanded score should be lower than seed score"
    );
}
```

- [ ] **Step 2: Run integration test**

Run: `cargo test -p alephcore --lib test_graph_augmented_retrieval_flow -- --nocapture`
Expected: PASS

- [ ] **Step 3: Run full test suite**

Run: `cargo test -p alephcore --lib`
Expected: all tests PASS

- [ ] **Step 4: Commit**

```bash
git add src/memory/integration_tests/
git commit -m "test: add end-to-end integration test for graph-augmented retrieval"
```
