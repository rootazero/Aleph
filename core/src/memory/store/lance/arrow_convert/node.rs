//! Arrow RecordBatch <-> GraphNode conversions.

use crate::sync_primitives::Arc;

use arrow_array::builder::{ListBuilder, StringBuilder};
use arrow_array::{Float32Array, Int64Array, ListArray, RecordBatch, StringArray};

use crate::error::AlephError;
use crate::memory::store::GraphNode;

use super::helpers::{col, conv_err, read_nullable_string, read_string_list};
use crate::memory::store::lance::schema::graph_nodes_schema;

/// Convert a slice of `GraphNode` into an Arrow `RecordBatch`.
pub fn graph_nodes_to_record_batch(nodes: &[GraphNode]) -> Result<RecordBatch, AlephError> {
    let schema = graph_nodes_schema();

    let id_arr = StringArray::from_iter_values(nodes.iter().map(|n| n.id.as_str()));
    let name_arr = StringArray::from_iter_values(nodes.iter().map(|n| n.name.as_str()));
    let kind_arr = StringArray::from_iter_values(nodes.iter().map(|n| n.kind.as_str()));
    let metadata_arr = StringArray::from(
        nodes
            .iter()
            .map(|n| {
                if n.metadata_json.is_empty() {
                    None
                } else {
                    Some(n.metadata_json.as_str())
                }
            })
            .collect::<Vec<_>>(),
    );
    let decay_score_arr = Float32Array::from_iter_values(nodes.iter().map(|n| n.decay_score));
    let created_at_arr = Int64Array::from_iter_values(nodes.iter().map(|n| n.created_at));
    let updated_at_arr = Int64Array::from_iter_values(nodes.iter().map(|n| n.updated_at));
    let workspace_arr = StringArray::from_iter_values(nodes.iter().map(|n| n.workspace.as_str()));

    // List(Utf8): aliases
    let mut aliases_builder = ListBuilder::new(StringBuilder::new());
    for node in nodes {
        for alias in &node.aliases {
            aliases_builder.values().append_value(alias);
        }
        aliases_builder.append(true);
    }
    let aliases_arr = aliases_builder.finish();

    let batch = RecordBatch::try_new(
        schema,
        vec![
            Arc::new(id_arr),          // 0 id
            Arc::new(name_arr),        // 1 name
            Arc::new(kind_arr),        // 2 kind
            Arc::new(aliases_arr),     // 3 aliases
            Arc::new(metadata_arr),    // 4 metadata
            Arc::new(decay_score_arr), // 5 decay_score
            Arc::new(created_at_arr),  // 6 created_at
            Arc::new(updated_at_arr),  // 7 updated_at
            Arc::new(workspace_arr),   // 8 workspace
        ],
    )
    .map_err(conv_err)?;

    Ok(batch)
}

/// Convert an Arrow `RecordBatch` back into a `Vec<GraphNode>`.
pub fn record_batch_to_graph_nodes(batch: &RecordBatch) -> Result<Vec<GraphNode>, AlephError> {
    let n = batch.num_rows();
    if n == 0 {
        return Ok(Vec::new());
    }

    let id_col = col::<StringArray>(batch, "id")?;
    let name_col = col::<StringArray>(batch, "name")?;
    let kind_col = col::<StringArray>(batch, "kind")?;
    let aliases_col = col::<ListArray>(batch, "aliases")?;
    let metadata_col = col::<StringArray>(batch, "metadata")?;
    let decay_score_col = col::<Float32Array>(batch, "decay_score")?;
    let created_at_col = col::<Int64Array>(batch, "created_at")?;
    let updated_at_col = col::<Int64Array>(batch, "updated_at")?;
    let workspace_col = col::<StringArray>(batch, "workspace").ok();

    let mut nodes = Vec::with_capacity(n);
    for i in 0..n {
        nodes.push(GraphNode {
            id: id_col.value(i).to_string(),
            name: name_col.value(i).to_string(),
            kind: kind_col.value(i).to_string(),
            aliases: read_string_list(aliases_col, i),
            metadata_json: read_nullable_string(metadata_col, i).unwrap_or_default(),
            decay_score: decay_score_col.value(i),
            created_at: created_at_col.value(i),
            updated_at: updated_at_col.value(i),
            workspace: workspace_col
                .map(|c| c.value(i).to_string())
                .unwrap_or_else(|| "default".to_string()),
        });
    }

    Ok(nodes)
}
