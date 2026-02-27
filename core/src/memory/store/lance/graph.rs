//! GraphStore trait implementation for LanceMemoryBackend.
//!
//! Provides knowledge-graph node/edge CRUD, entity resolution,
//! context-scoped edge queries, and temporal decay operations
//! against the LanceDB `graph_nodes` and `graph_edges` tables.

use arrow_array::{RecordBatch, RecordBatchIterator};
use async_trait::async_trait;
use futures::TryStreamExt;
use lancedb::query::{ExecutableQuery, QueryBase, Select};

use crate::config::types::memory::GraphDecayPolicy;
use crate::error::AlephError;
use crate::memory::store::{DecayStats, GraphEdge, GraphNode, GraphStore, ResolvedEntity};

use super::arrow_convert::{
    graph_edges_to_record_batch, graph_nodes_to_record_batch, record_batch_to_graph_edges,
    record_batch_to_graph_nodes,
};
use super::LanceMemoryBackend;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Collect a LanceDB query stream into a vector of RecordBatches.
async fn collect_batches(
    stream: lancedb::arrow::SendableRecordBatchStream,
) -> Result<Vec<RecordBatch>, AlephError> {
    stream.try_collect().await.map_err(super::lance_err)
}

/// Insert a RecordBatch into a LanceDB table.
async fn add_batch(table: &lancedb::Table, batch: RecordBatch) -> Result<(), AlephError> {
    let schema = batch.schema();
    let batches = RecordBatchIterator::new(vec![Ok(batch)], schema);
    table
        .add(batches)
        .execute()
        .await
        .map_err(super::lance_err)?;
    Ok(())
}

/// Scan graph nodes with an optional SQL filter.
async fn scan_nodes(
    table: &lancedb::Table,
    filter: Option<&str>,
    limit: Option<usize>,
) -> Result<Vec<GraphNode>, AlephError> {
    let mut query = table.query();

    if let Some(f) = filter {
        query = query.only_if(f);
    }
    if let Some(lim) = limit {
        query = query.limit(lim);
    }

    query = query.select(Select::All);

    let stream = query.execute().await.map_err(super::lance_err)?;
    let batches = collect_batches(stream).await?;

    let mut nodes = Vec::new();
    for batch in &batches {
        let mut batch_nodes = record_batch_to_graph_nodes(batch)?;
        nodes.append(&mut batch_nodes);
    }
    Ok(nodes)
}

/// Scan graph edges with an optional SQL filter.
async fn scan_edges(
    table: &lancedb::Table,
    filter: Option<&str>,
    limit: Option<usize>,
) -> Result<Vec<GraphEdge>, AlephError> {
    let mut query = table.query();

    if let Some(f) = filter {
        query = query.only_if(f);
    }
    if let Some(lim) = limit {
        query = query.limit(lim);
    }

    query = query.select(Select::All);

    let stream = query.execute().await.map_err(super::lance_err)?;
    let batches = collect_batches(stream).await?;

    let mut edges = Vec::new();
    for batch in &batches {
        let mut batch_edges = record_batch_to_graph_edges(batch)?;
        edges.append(&mut batch_edges);
    }
    Ok(edges)
}

// ============================================================================
// GraphStore implementation
// ============================================================================

#[async_trait]
impl GraphStore for LanceMemoryBackend {
    async fn upsert_node(&self, node: &GraphNode, workspace: &str) -> Result<(), AlephError> {
        // Delete existing node with same ID if present, then insert.
        let existing = self.get_node(&node.id, workspace).await?;
        if existing.is_some() {
            self.nodes_table
                .delete(&format!("id = '{}'", node.id))
                .await
                .map_err(super::lance_err)?;
        }

        let batch = graph_nodes_to_record_batch(std::slice::from_ref(node))?;
        add_batch(&self.nodes_table, batch).await
    }

    async fn get_node(&self, id: &str, workspace: &str) -> Result<Option<GraphNode>, AlephError> {
        let filter = format!("id = '{}' AND workspace = '{}'", id, workspace);
        let nodes = scan_nodes(&self.nodes_table, Some(&filter), Some(1)).await?;
        Ok(nodes.into_iter().next())
    }

    async fn upsert_edge(&self, edge: &GraphEdge, workspace: &str) -> Result<(), AlephError> {
        // Delete existing edge with same ID if present, then insert.
        let filter = format!("id = '{}' AND workspace = '{}'", edge.id, workspace);
        let existing = scan_edges(&self.edges_table, Some(&filter), Some(1)).await?;
        if !existing.is_empty() {
            self.edges_table
                .delete(&format!("id = '{}'", edge.id))
                .await
                .map_err(super::lance_err)?;
        }

        let batch = graph_edges_to_record_batch(std::slice::from_ref(edge))?;
        add_batch(&self.edges_table, batch).await
    }

    async fn resolve_entity(
        &self,
        query: &str,
        context_key: Option<&str>,
        workspace: &str,
    ) -> Result<Vec<ResolvedEntity>, AlephError> {
        // Try exact match on the name column (FTS index may not exist in tests).
        let filter = format!("name = '{}' AND workspace = '{}'", query, workspace);
        let candidates = scan_nodes(&self.nodes_table, Some(&filter), None).await?;

        if candidates.is_empty() {
            return Ok(Vec::new());
        }

        let ambiguous = candidates.len() > 1;

        let mut results = Vec::new();
        for node in &candidates {
            // If context_key is provided and there are multiple candidates,
            // count edges in that context to compute a relevance score.
            let context_score = if let Some(ctx) = context_key {
                let count = self.count_edges_in_context(&node.id, ctx, workspace).await?;
                count as f32
            } else {
                0.0
            };

            results.push(ResolvedEntity {
                node_id: node.id.clone(),
                name: node.name.clone(),
                kind: node.kind.clone(),
                aliases: node.aliases.clone(),
                context_score,
                ambiguous,
            });
        }

        // Sort by context_score descending so the most relevant entity is first.
        results.sort_by(|a, b| {
            b.context_score
                .partial_cmp(&a.context_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(results)
    }

    async fn get_edges_for_node(
        &self,
        node_id: &str,
        context_key: Option<&str>,
        workspace: &str,
    ) -> Result<Vec<GraphEdge>, AlephError> {
        let base_filter = format!("(from_id = '{}' OR to_id = '{}') AND workspace = '{}'", node_id, node_id, workspace);
        let filter = if let Some(ctx) = context_key {
            format!("({}) AND context_key = '{}'", base_filter, ctx)
        } else {
            base_filter
        };

        scan_edges(&self.edges_table, Some(&filter), None).await
    }

    async fn count_edges_in_context(
        &self,
        node_id: &str,
        context_key: &str,
        workspace: &str,
    ) -> Result<usize, AlephError> {
        let filter = format!(
            "(from_id = '{}' OR to_id = '{}') AND context_key = '{}' AND workspace = '{}'",
            node_id, node_id, context_key, workspace
        );
        let edges = scan_edges(&self.edges_table, Some(&filter), None).await?;
        Ok(edges.len())
    }

    async fn apply_decay(&self, policy: &GraphDecayPolicy, workspace: &str) -> Result<DecayStats, AlephError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let seconds_per_day: f64 = 86400.0;
        let mut stats = DecayStats::default();

        // --- Decay nodes ---
        let ws_filter = format!("workspace = '{}'", workspace);
        let all_nodes = scan_nodes(&self.nodes_table, Some(&ws_filter), None).await?;
        let mut nodes_to_prune: Vec<String> = Vec::new();
        let mut nodes_to_update: Vec<GraphNode> = Vec::new();

        for node in all_nodes {
            let days_since_update =
                (now - node.updated_at) as f64 / seconds_per_day;
            let new_score = node.decay_score - (days_since_update as f32 * policy.node_decay_per_day);

            if new_score < policy.min_score {
                nodes_to_prune.push(node.id.clone());
                stats.nodes_pruned += 1;
            } else if (new_score - node.decay_score).abs() > f32::EPSILON {
                let mut updated = node.clone();
                updated.decay_score = new_score;
                nodes_to_update.push(updated);
                stats.nodes_decayed += 1;
            }
        }

        // Delete pruned nodes
        for id in &nodes_to_prune {
            self.nodes_table
                .delete(&format!("id = '{}'", id))
                .await
                .map_err(super::lance_err)?;
        }

        // Update decayed nodes (delete + re-insert)
        for node in &nodes_to_update {
            self.nodes_table
                .delete(&format!("id = '{}'", node.id))
                .await
                .map_err(super::lance_err)?;
            let batch = graph_nodes_to_record_batch(std::slice::from_ref(node))?;
            add_batch(&self.nodes_table, batch).await?;
        }

        // --- Decay edges ---
        let all_edges = scan_edges(&self.edges_table, Some(&ws_filter), None).await?;
        let mut edges_to_prune: Vec<String> = Vec::new();
        let mut edges_to_update: Vec<GraphEdge> = Vec::new();

        for edge in all_edges {
            let days_since_update =
                (now - edge.updated_at) as f64 / seconds_per_day;
            let new_score = edge.decay_score - (days_since_update as f32 * policy.edge_decay_per_day);

            if new_score < policy.min_score {
                edges_to_prune.push(edge.id.clone());
                stats.edges_pruned += 1;
            } else if (new_score - edge.decay_score).abs() > f32::EPSILON {
                let mut updated = edge.clone();
                updated.decay_score = new_score;
                edges_to_update.push(updated);
                stats.edges_decayed += 1;
            }
        }

        // Delete pruned edges
        for id in &edges_to_prune {
            self.edges_table
                .delete(&format!("id = '{}'", id))
                .await
                .map_err(super::lance_err)?;
        }

        // Update decayed edges (delete + re-insert)
        for edge in &edges_to_update {
            self.edges_table
                .delete(&format!("id = '{}'", edge.id))
                .await
                .map_err(super::lance_err)?;
            let batch = graph_edges_to_record_batch(std::slice::from_ref(edge))?;
            add_batch(&self.edges_table, batch).await?;
        }

        Ok(stats)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create a test LanceMemoryBackend in a temp directory.
    async fn create_test_backend() -> (tempfile::TempDir, LanceMemoryBackend) {
        let tmp = tempfile::tempdir().unwrap();
        let backend = LanceMemoryBackend::open_or_create(tmp.path())
            .await
            .unwrap();
        (tmp, backend)
    }

    /// Helper: create a test GraphNode.
    fn make_test_node(id: &str, name: &str, kind: &str) -> GraphNode {
        GraphNode {
            id: id.to_string(),
            name: name.to_string(),
            kind: kind.to_string(),
            aliases: vec![],
            metadata_json: String::new(),
            decay_score: 1.0,
            created_at: 1700000000,
            updated_at: 1700000000,
            workspace: "default".to_string(),
        }
    }

    /// Helper: create a test GraphEdge.
    fn make_test_edge(
        id: &str,
        from_id: &str,
        to_id: &str,
        relation: &str,
        context_key: &str,
    ) -> GraphEdge {
        GraphEdge {
            id: id.to_string(),
            from_id: from_id.to_string(),
            to_id: to_id.to_string(),
            relation: relation.to_string(),
            weight: 1.0,
            confidence: 0.9,
            context_key: context_key.to_string(),
            decay_score: 1.0,
            created_at: 1700000000,
            updated_at: 1700000000,
            last_seen_at: 1700000000,
            workspace: "default".to_string(),
        }
    }

    #[tokio::test]
    async fn test_upsert_and_get_node() {
        let (_tmp, backend) = create_test_backend().await;
        let node = make_test_node("gn-001", "Rust", "language");

        backend.upsert_node(&node, "default").await.unwrap();

        let retrieved = backend.get_node("gn-001", "default").await.unwrap();
        assert!(retrieved.is_some());
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.name, "Rust");
        assert_eq!(retrieved.kind, "language");
    }

    #[tokio::test]
    async fn test_upsert_node_updates_existing() {
        let (_tmp, backend) = create_test_backend().await;
        let node = make_test_node("gn-001", "Rust", "language");
        backend.upsert_node(&node, "default").await.unwrap();

        // Update name
        let mut updated_node = node.clone();
        updated_node.name = "Rust Lang".to_string();
        updated_node.aliases = vec!["rust-lang".to_string()];
        backend.upsert_node(&updated_node, "default").await.unwrap();

        let retrieved = backend.get_node("gn-001", "default").await.unwrap().unwrap();
        assert_eq!(retrieved.name, "Rust Lang");
        assert_eq!(retrieved.aliases, vec!["rust-lang".to_string()]);
    }

    #[tokio::test]
    async fn test_upsert_and_get_edges() {
        let (_tmp, backend) = create_test_backend().await;
        let edge = make_test_edge("ge-001", "gn-001", "gn-002", "uses", "app:test");

        backend.upsert_edge(&edge, "default").await.unwrap();

        let edges = backend
            .get_edges_for_node("gn-001", None, "default")
            .await
            .unwrap();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].id, "ge-001");
        assert_eq!(edges[0].relation, "uses");
    }

    #[tokio::test]
    async fn test_get_edges_for_node() {
        let (_tmp, backend) = create_test_backend().await;

        // Create edges: gn-001 -> gn-002, gn-001 -> gn-003, gn-004 -> gn-005
        let edge1 = make_test_edge("ge-001", "gn-001", "gn-002", "uses", "ctx-a");
        let edge2 = make_test_edge("ge-002", "gn-001", "gn-003", "knows", "ctx-b");
        let edge3 = make_test_edge("ge-003", "gn-004", "gn-005", "works_on", "ctx-a");

        backend.upsert_edge(&edge1, "default").await.unwrap();
        backend.upsert_edge(&edge2, "default").await.unwrap();
        backend.upsert_edge(&edge3, "default").await.unwrap();

        // Get all edges for gn-001
        let edges = backend
            .get_edges_for_node("gn-001", None, "default")
            .await
            .unwrap();
        assert_eq!(edges.len(), 2);

        // Get edges for gn-001 in context ctx-a only
        let edges_ctx = backend
            .get_edges_for_node("gn-001", Some("ctx-a"), "default")
            .await
            .unwrap();
        assert_eq!(edges_ctx.len(), 1);
        assert_eq!(edges_ctx[0].id, "ge-001");

        // Get edges for gn-005 (appears as to_id)
        let edges_to = backend
            .get_edges_for_node("gn-005", None, "default")
            .await
            .unwrap();
        assert_eq!(edges_to.len(), 1);
        assert_eq!(edges_to[0].id, "ge-003");
    }

    #[tokio::test]
    async fn test_count_edges_in_context() {
        let (_tmp, backend) = create_test_backend().await;

        let edge1 = make_test_edge("ge-001", "gn-001", "gn-002", "uses", "ctx-a");
        let edge2 = make_test_edge("ge-002", "gn-001", "gn-003", "knows", "ctx-a");
        let edge3 = make_test_edge("ge-003", "gn-001", "gn-004", "works_on", "ctx-b");

        backend.upsert_edge(&edge1, "default").await.unwrap();
        backend.upsert_edge(&edge2, "default").await.unwrap();
        backend.upsert_edge(&edge3, "default").await.unwrap();

        let count_a = backend
            .count_edges_in_context("gn-001", "ctx-a", "default")
            .await
            .unwrap();
        assert_eq!(count_a, 2);

        let count_b = backend
            .count_edges_in_context("gn-001", "ctx-b", "default")
            .await
            .unwrap();
        assert_eq!(count_b, 1);

        let count_none = backend
            .count_edges_in_context("gn-001", "ctx-nonexistent", "default")
            .await
            .unwrap();
        assert_eq!(count_none, 0);
    }

    #[tokio::test]
    async fn test_apply_decay() {
        let (_tmp, backend) = create_test_backend().await;

        // Create a node that was last updated a long time ago (should be pruned)
        let mut old_node = make_test_node("gn-old", "OldEntity", "concept");
        old_node.decay_score = 0.15;
        old_node.updated_at = 1600000000; // far in the past

        // Create a recent node (should survive)
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let mut recent_node = make_test_node("gn-recent", "RecentEntity", "concept");
        recent_node.decay_score = 1.0;
        recent_node.updated_at = now;

        backend.upsert_node(&old_node, "default").await.unwrap();
        backend.upsert_node(&recent_node, "default").await.unwrap();

        // Create an old edge (should be pruned)
        let mut old_edge = make_test_edge("ge-old", "gn-old", "gn-recent", "mentions", "ctx");
        old_edge.decay_score = 0.15;
        old_edge.updated_at = 1600000000;

        backend.upsert_edge(&old_edge, "default").await.unwrap();

        let policy = GraphDecayPolicy {
            node_decay_per_day: 0.02,
            edge_decay_per_day: 0.03,
            min_score: 0.1,
        };

        let stats = backend.apply_decay(&policy, "default").await.unwrap();

        // Old node and edge should have been pruned
        assert!(stats.nodes_pruned >= 1, "expected at least 1 node pruned, got {}", stats.nodes_pruned);
        assert!(stats.edges_pruned >= 1, "expected at least 1 edge pruned, got {}", stats.edges_pruned);

        // Old node should be gone
        assert!(backend.get_node("gn-old", "default").await.unwrap().is_none());

        // Recent node should still exist
        assert!(backend.get_node("gn-recent", "default").await.unwrap().is_some());
    }
}
