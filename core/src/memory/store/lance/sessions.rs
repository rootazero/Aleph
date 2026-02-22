//! SessionStore trait implementation for LanceMemoryBackend.
//!
//! Provides raw memory entry (Layer 1) insert, vector search,
//! recent-memories retrieval, deletion, and aggregate statistics
//! against the LanceDB `memories` table.

use arrow_array::{RecordBatch, RecordBatchIterator};
use async_trait::async_trait;
use futures::TryStreamExt;
use lancedb::query::{ExecutableQuery, QueryBase, Select};

use crate::error::AlephError;
use crate::memory::context::MemoryEntry;
use crate::memory::store::types::MemoryFilter;
use crate::memory::store::{SessionStore, StoreStats};

use super::arrow_convert::{memories_to_record_batch, record_batch_to_memories};
use super::LanceMemoryBackend;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Map a lancedb error to an AlephError.
fn lance_err(msg: impl std::fmt::Display) -> AlephError {
    AlephError::config(format!("LanceDB error: {}", msg))
}

/// Collect a LanceDB query stream into a vector of RecordBatches.
async fn collect_batches(
    stream: lancedb::arrow::SendableRecordBatchStream,
) -> Result<Vec<RecordBatch>, AlephError> {
    stream.try_collect().await.map_err(|e| lance_err(e))
}

/// Insert a RecordBatch into a LanceDB table.
async fn add_batch(table: &lancedb::Table, batch: RecordBatch) -> Result<(), AlephError> {
    let schema = batch.schema();
    let batches = RecordBatchIterator::new(vec![Ok(batch)], schema);
    table
        .add(batches)
        .execute()
        .await
        .map_err(|e| lance_err(e))?;
    Ok(())
}

/// Scan memories with an optional SQL filter.
async fn scan_memories(
    table: &lancedb::Table,
    filter: Option<&str>,
    limit: Option<usize>,
) -> Result<Vec<MemoryEntry>, AlephError> {
    let mut query = table.query();

    if let Some(f) = filter {
        query = query.only_if(f);
    }
    if let Some(lim) = limit {
        query = query.limit(lim);
    }

    query = query.select(Select::All);

    let stream = query.execute().await.map_err(|e| lance_err(e))?;
    let batches = collect_batches(stream).await?;

    let mut entries = Vec::new();
    for batch in &batches {
        let mut batch_entries = record_batch_to_memories(batch)?;
        entries.append(&mut batch_entries);
    }
    Ok(entries)
}

/// Count rows in a LanceDB table.
async fn count_rows(table: &lancedb::Table) -> Result<usize, AlephError> {
    let stream = table
        .query()
        .select(Select::columns(&["id"]))
        .execute()
        .await
        .map_err(|e| lance_err(e))?;
    let batches = collect_batches(stream).await?;
    Ok(batches.iter().map(|b| b.num_rows()).sum())
}

/// Count rows in a LanceDB table with a filter.
async fn count_rows_with_filter(
    table: &lancedb::Table,
    filter: &str,
) -> Result<usize, AlephError> {
    let stream = table
        .query()
        .only_if(filter)
        .select(Select::columns(&["id"]))
        .execute()
        .await
        .map_err(|e| lance_err(e))?;
    let batches = collect_batches(stream).await?;
    Ok(batches.iter().map(|b| b.num_rows()).sum())
}

/// Convert distance to a similarity score in [0, 1].
///
/// LanceDB uses L2 distance by default, so lower is better.
/// We convert: similarity = 1 / (1 + distance).
fn distance_to_similarity(distance: f32) -> f32 {
    1.0 / (1.0 + distance)
}

/// Extract `_distance` score from a RecordBatch at a given row.
fn read_distance(batch: &RecordBatch, row: usize) -> f32 {
    use arrow_array::Float32Array;
    batch
        .column_by_name("_distance")
        .and_then(|col| col.as_any().downcast_ref::<Float32Array>())
        .map(|arr| arr.value(row))
        .unwrap_or(0.0)
}

// ============================================================================
// SessionStore implementation
// ============================================================================

#[async_trait]
impl SessionStore for LanceMemoryBackend {
    async fn insert_memory(&self, memory: &MemoryEntry) -> Result<(), AlephError> {
        let batch = memories_to_record_batch(&[memory.clone()])?;
        add_batch(&self.memories_table, batch).await
    }

    async fn search_memories(
        &self,
        embedding: &[f32],
        filter: &MemoryFilter,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>, AlephError> {
        let mut query = self
            .memories_table
            .query()
            .nearest_to(embedding)
            .map_err(|e| lance_err(e))?
            .column("vec_384")
            .limit(limit);

        if let Some(f) = filter.to_lance_filter() {
            query = query.only_if(f);
        }

        let stream = query.execute().await.map_err(|e| lance_err(e))?;
        let batches = collect_batches(stream).await?;

        let mut results = Vec::new();
        for batch in &batches {
            let entries = record_batch_to_memories(batch)?;
            for (i, entry) in entries.into_iter().enumerate() {
                let distance = read_distance(batch, i);
                let score = distance_to_similarity(distance);
                results.push(entry.with_score(score));
            }
        }

        Ok(results)
    }

    async fn get_memories_for_entity(
        &self,
        _entity_id: &str,
        _limit: usize,
    ) -> Result<Vec<MemoryEntry>, AlephError> {
        // TODO: Wire entity-memory linking through graph edges.
        // This requires querying edges where relation='entity_mention' and
        // from_id or to_id matches entity_id, then fetching the associated
        // memories. Will be implemented during consumer migration phase.
        Ok(Vec::new())
    }

    async fn get_recent_memories(
        &self,
        filter: &MemoryFilter,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>, AlephError> {
        // Query with filter, then sort by timestamp DESC in Rust
        // (LanceDB sorting support is limited).
        let mut entries = scan_memories(
            &self.memories_table,
            filter.to_lance_filter().as_deref(),
            None,
        )
        .await?;

        // Sort by timestamp descending (most recent first).
        entries.sort_by(|a, b| b.context.timestamp.cmp(&a.context.timestamp));

        // Take only the requested number.
        entries.truncate(limit);

        Ok(entries)
    }

    async fn delete_memory(&self, id: &str) -> Result<(), AlephError> {
        self.memories_table
            .delete(&format!("id = '{}'", id))
            .await
            .map_err(|e| lance_err(e))?;
        Ok(())
    }

    async fn get_stats(&self) -> Result<StoreStats, AlephError> {
        let total_facts = count_rows(&self.facts_table).await?;
        let valid_facts = count_rows_with_filter(&self.facts_table, "is_valid = true").await?;
        let total_memories = count_rows(&self.memories_table).await?;
        let total_graph_nodes = count_rows(&self.nodes_table).await?;
        let total_graph_edges = count_rows(&self.edges_table).await?;

        Ok(StoreStats {
            total_facts,
            valid_facts,
            total_memories,
            total_graph_nodes,
            total_graph_edges,
        })
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::ContextAnchor;
    use crate::memory::store::GraphStore;

    /// Helper: create a test LanceMemoryBackend in a temp directory.
    async fn create_test_backend() -> (tempfile::TempDir, LanceMemoryBackend) {
        let tmp = tempfile::tempdir().unwrap();
        let backend = LanceMemoryBackend::open_or_create(tmp.path())
            .await
            .unwrap();
        (tmp, backend)
    }

    /// Helper: create a test MemoryEntry with embedding.
    fn make_test_memory(id: &str, user_input: &str, timestamp: i64) -> MemoryEntry {
        MemoryEntry::with_embedding(
            id.to_string(),
            ContextAnchor::with_timestamp(
                "com.test".to_string(),
                "test.txt".to_string(),
                timestamp,
            ),
            user_input.to_string(),
            "ai response".to_string(),
            vec![0.1_f32; 384],
        )
    }

    #[tokio::test]
    async fn test_insert_and_search_memory() {
        let (_tmp, backend) = create_test_backend().await;

        let mem1 = make_test_memory("mem-1", "What is Rust?", 1700000000);
        let mut mem2 = make_test_memory("mem-2", "Tell me about Python", 1700001000);
        mem2.embedding = Some(vec![0.9_f32; 384]);

        backend.insert_memory(&mem1).await.unwrap();
        backend.insert_memory(&mem2).await.unwrap();

        // Search with vector close to mem1
        let query_vec = vec![0.1_f32; 384];
        let results = backend
            .search_memories(&query_vec, &MemoryFilter::new(), 10)
            .await
            .unwrap();

        assert!(!results.is_empty());
        // The closest result should be mem1
        assert_eq!(results[0].id, "mem-1");
        assert!(results[0].similarity_score.is_some());
        assert!(results[0].similarity_score.unwrap() > 0.0);
    }

    #[tokio::test]
    async fn test_get_recent_memories() {
        let (_tmp, backend) = create_test_backend().await;

        let mem1 = make_test_memory("mem-1", "First message", 1700000000);
        let mem2 = make_test_memory("mem-2", "Second message", 1700001000);
        let mem3 = make_test_memory("mem-3", "Third message", 1700002000);

        backend.insert_memory(&mem1).await.unwrap();
        backend.insert_memory(&mem2).await.unwrap();
        backend.insert_memory(&mem3).await.unwrap();

        // Get 2 most recent
        let recent = backend
            .get_recent_memories(&MemoryFilter::new(), 2)
            .await
            .unwrap();

        assert_eq!(recent.len(), 2);
        // Should be sorted by timestamp descending
        assert_eq!(recent[0].id, "mem-3");
        assert_eq!(recent[1].id, "mem-2");
    }

    #[tokio::test]
    async fn test_delete_memory() {
        let (_tmp, backend) = create_test_backend().await;

        let mem = make_test_memory("mem-1", "To be deleted", 1700000000);
        backend.insert_memory(&mem).await.unwrap();

        // Verify it exists
        let before = backend
            .get_recent_memories(&MemoryFilter::new(), 10)
            .await
            .unwrap();
        assert_eq!(before.len(), 1);

        // Delete
        backend.delete_memory("mem-1").await.unwrap();

        // Verify it's gone
        let after = backend
            .get_recent_memories(&MemoryFilter::new(), 10)
            .await
            .unwrap();
        assert!(after.is_empty());
    }

    #[tokio::test]
    async fn test_get_stats() {
        let (_tmp, backend) = create_test_backend().await;

        // Insert some data
        let mem = make_test_memory("mem-1", "Hello", 1700000000);
        backend.insert_memory(&mem).await.unwrap();

        let node = crate::memory::store::GraphNode {
            id: "gn-001".to_string(),
            name: "TestEntity".to_string(),
            kind: "concept".to_string(),
            aliases: vec![],
            metadata_json: String::new(),
            decay_score: 1.0,
            created_at: 1700000000,
            updated_at: 1700000000,
        };
        backend.upsert_node(&node).await.unwrap();

        let stats = backend.get_stats().await.unwrap();
        assert_eq!(stats.total_memories, 1);
        assert_eq!(stats.total_graph_nodes, 1);
        assert_eq!(stats.total_graph_edges, 0);
        assert_eq!(stats.total_facts, 0);
        assert_eq!(stats.valid_facts, 0);
    }
}
