//! Helper functions for LanceDB fact operations.

use arrow_array::{Float32Array, RecordBatch, RecordBatchIterator};
use futures::TryStreamExt;
use lancedb::query::{ExecutableQuery, QueryBase, Select};

use crate::error::AlephError;
use crate::memory::context::MemoryFact;

use super::super::arrow_convert::record_batch_to_facts;

/// Collect a LanceDB query stream into a vector of RecordBatches.
pub(super) async fn collect_batches(
    stream: lancedb::arrow::SendableRecordBatchStream,
) -> Result<Vec<RecordBatch>, AlephError> {
    stream.try_collect().await.map_err(super::super::lance_err)
}

/// Execute a filtered scan and return all matching facts.
pub(super) async fn scan_facts(
    table: &lancedb::Table,
    filter: Option<&str>,
    limit: Option<usize>,
) -> Result<Vec<MemoryFact>, AlephError> {
    scan_facts_with_offset(table, filter, limit, 0).await
}

/// Scan facts with offset-based pagination.
/// LanceDB doesn't support SQL OFFSET, so we use limit(offset+count) and skip.
pub(super) async fn scan_facts_with_offset(
    table: &lancedb::Table,
    filter: Option<&str>,
    limit: Option<usize>,
    offset: usize,
) -> Result<Vec<MemoryFact>, AlephError> {
    let mut query = table.query();

    if let Some(f) = filter {
        query = query.only_if(f);
    }
    // Fetch offset + limit rows, then skip the first `offset`
    if let Some(lim) = limit {
        query = query.limit(offset + lim);
    }

    query = query.select(Select::All);

    let stream = query.execute().await.map_err(super::super::lance_err)?;
    let batches = collect_batches(stream).await?;

    let mut facts = Vec::new();
    for batch in &batches {
        let mut batch_facts = record_batch_to_facts(batch)?;
        facts.append(&mut batch_facts);
    }

    // Skip offset rows
    if offset > 0 && offset < facts.len() {
        facts = facts.split_off(offset);
    } else if offset >= facts.len() {
        facts.clear();
    }

    // Apply limit after offset
    if let Some(lim) = limit {
        facts.truncate(lim);
    }

    Ok(facts)
}

/// Insert a RecordBatch into the facts table.
pub(super) async fn add_batch(
    table: &lancedb::Table,
    batch: RecordBatch,
) -> Result<(), AlephError> {
    let schema = batch.schema();
    let batches = RecordBatchIterator::new(vec![Ok(batch)], schema);
    table
        .add(batches)
        .execute()
        .await
        .map_err(super::super::lance_err)?;
    Ok(())
}

/// Extract `_distance` score from a RecordBatch at a given row.
pub(super) fn read_distance(batch: &RecordBatch, row: usize) -> f32 {
    batch
        .column_by_name("_distance")
        .and_then(|col| col.as_any().downcast_ref::<Float32Array>())
        .map(|arr| arr.value(row))
        .unwrap_or(0.0)
}

/// Extract `_score` from a RecordBatch at a given row (FTS relevance).
pub(super) fn read_score(batch: &RecordBatch, row: usize) -> f32 {
    batch
        .column_by_name("_score")
        .and_then(|col| col.as_any().downcast_ref::<Float32Array>())
        .map(|arr| arr.value(row))
        .unwrap_or(0.0)
}

/// Extract `_relevance_score` from a RecordBatch at a given row (hybrid search).
pub(super) fn read_relevance_score(batch: &RecordBatch, row: usize) -> f32 {
    batch
        .column_by_name("_relevance_score")
        .and_then(|col| col.as_any().downcast_ref::<Float32Array>())
        .map(|arr| arr.value(row))
        .unwrap_or(0.0)
}

/// Convert distance to a similarity score in [0, 1].
///
/// LanceDB uses L2 distance by default, so lower is better.
/// We convert: similarity = 1 / (1 + distance).
pub(super) fn distance_to_similarity(distance: f32) -> f32 {
    1.0 / (1.0 + distance)
}
