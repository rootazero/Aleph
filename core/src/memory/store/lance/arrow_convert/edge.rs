//! Arrow RecordBatch <-> GraphEdge conversions.

use crate::sync_primitives::Arc;

use arrow_array::{Float32Array, Int64Array, RecordBatch, StringArray};

use crate::error::AlephError;
use crate::memory::store::GraphEdge;

use super::helpers::{col, conv_err};
use crate::memory::store::lance::schema::graph_edges_schema;

/// Convert a slice of `GraphEdge` into an Arrow `RecordBatch`.
pub fn graph_edges_to_record_batch(edges: &[GraphEdge]) -> Result<RecordBatch, AlephError> {
    let schema = graph_edges_schema();

    let id_arr = StringArray::from_iter_values(edges.iter().map(|e| e.id.as_str()));
    let from_id_arr = StringArray::from_iter_values(edges.iter().map(|e| e.from_id.as_str()));
    let to_id_arr = StringArray::from_iter_values(edges.iter().map(|e| e.to_id.as_str()));
    let relation_arr = StringArray::from_iter_values(edges.iter().map(|e| e.relation.as_str()));
    let weight_arr = Float32Array::from_iter_values(edges.iter().map(|e| e.weight));
    let confidence_arr = Float32Array::from_iter_values(edges.iter().map(|e| e.confidence));
    let context_key_arr =
        StringArray::from_iter_values(edges.iter().map(|e| e.context_key.as_str()));
    let decay_score_arr = Float32Array::from_iter_values(edges.iter().map(|e| e.decay_score));
    let created_at_arr = Int64Array::from_iter_values(edges.iter().map(|e| e.created_at));
    let updated_at_arr = Int64Array::from_iter_values(edges.iter().map(|e| e.updated_at));
    let last_seen_at_arr = Int64Array::from_iter_values(edges.iter().map(|e| e.last_seen_at));
    let workspace_arr = StringArray::from_iter_values(edges.iter().map(|e| e.workspace.as_str()));

    let batch = RecordBatch::try_new(
        schema,
        vec![
            Arc::new(id_arr),            // 0  id
            Arc::new(from_id_arr),       // 1  from_id
            Arc::new(to_id_arr),         // 2  to_id
            Arc::new(relation_arr),      // 3  relation
            Arc::new(weight_arr),        // 4  weight
            Arc::new(confidence_arr),    // 5  confidence
            Arc::new(context_key_arr),   // 6  context_key
            Arc::new(decay_score_arr),   // 7  decay_score
            Arc::new(created_at_arr),    // 8  created_at
            Arc::new(updated_at_arr),    // 9  updated_at
            Arc::new(last_seen_at_arr),  // 10 last_seen_at
            Arc::new(workspace_arr),     // 11 workspace
        ],
    )
    .map_err(conv_err)?;

    Ok(batch)
}

/// Convert an Arrow `RecordBatch` back into a `Vec<GraphEdge>`.
pub fn record_batch_to_graph_edges(batch: &RecordBatch) -> Result<Vec<GraphEdge>, AlephError> {
    let n = batch.num_rows();
    if n == 0 {
        return Ok(Vec::new());
    }

    let id_col = col::<StringArray>(batch, "id")?;
    let from_id_col = col::<StringArray>(batch, "from_id")?;
    let to_id_col = col::<StringArray>(batch, "to_id")?;
    let relation_col = col::<StringArray>(batch, "relation")?;
    let weight_col = col::<Float32Array>(batch, "weight")?;
    let confidence_col = col::<Float32Array>(batch, "confidence")?;
    let context_key_col = col::<StringArray>(batch, "context_key")?;
    let decay_score_col = col::<Float32Array>(batch, "decay_score")?;
    let created_at_col = col::<Int64Array>(batch, "created_at")?;
    let updated_at_col = col::<Int64Array>(batch, "updated_at")?;
    let last_seen_at_col = col::<Int64Array>(batch, "last_seen_at")?;
    let workspace_col = col::<StringArray>(batch, "workspace").ok();

    let mut edges = Vec::with_capacity(n);
    for i in 0..n {
        edges.push(GraphEdge {
            id: id_col.value(i).to_string(),
            from_id: from_id_col.value(i).to_string(),
            to_id: to_id_col.value(i).to_string(),
            relation: relation_col.value(i).to_string(),
            weight: weight_col.value(i),
            confidence: confidence_col.value(i),
            context_key: context_key_col.value(i).to_string(),
            decay_score: decay_score_col.value(i),
            created_at: created_at_col.value(i),
            updated_at: updated_at_col.value(i),
            last_seen_at: last_seen_at_col.value(i),
            workspace: workspace_col
                .map(|c| c.value(i).to_string())
                .unwrap_or_else(|| "default".to_string()),
        });
    }

    Ok(edges)
}
