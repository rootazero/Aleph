//! Arrow RecordBatch <-> domain type conversions for LanceDB.
//!
//! Provides serialisation (domain -> Arrow) and deserialisation (Arrow -> domain)
//! for the four LanceDB tables: `facts`, `graph_nodes`, `graph_edges`, and
//! `memories`.

use std::sync::Arc;

use arrow_array::builder::{FixedSizeListBuilder, Float32Builder, ListBuilder, StringBuilder};
use arrow_array::{
    Array, BooleanArray, FixedSizeListArray, Float32Array, Int32Array, Int64Array, ListArray,
    RecordBatch, StringArray,
};

use crate::error::AlephError;
use crate::memory::context::{
    ContextAnchor, FactSource, FactSpecificity, FactType, MemoryEntry, MemoryFact, TemporalScope,
};
use crate::memory::store::{GraphEdge, GraphNode};

use super::schema::{facts_schema, graph_edges_schema, graph_nodes_schema, memories_schema};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Shorthand for building an AlephError from an arrow conversion failure.
fn conv_err(msg: impl std::fmt::Display) -> AlephError {
    AlephError::config(format!("Arrow conversion error: {}", msg))
}

/// Downcast a column by name to the concrete array type `T`.
fn col<'a, T: 'static>(batch: &'a RecordBatch, name: &str) -> Result<&'a T, AlephError> {
    let array = batch
        .column_by_name(name)
        .ok_or_else(|| conv_err(format!("missing column '{}'", name)))?;
    array
        .as_any()
        .downcast_ref::<T>()
        .ok_or_else(|| conv_err(format!("column '{}' has unexpected type", name)))
}

/// Build a nullable `FixedSizeList(Float32, dim)` array from optional embeddings.
fn build_vector_column(
    embeddings: &[Option<&Vec<f32>>],
    dim: i32,
) -> Result<FixedSizeListArray, AlephError> {
    let dim_usize = dim as usize;
    let mut builder = FixedSizeListBuilder::new(Float32Builder::new(), dim);

    for opt in embeddings {
        match opt {
            Some(emb) if emb.len() == dim_usize => {
                let values = builder.values();
                for &v in emb.iter() {
                    values.append_value(v);
                }
                builder.append(true);
            }
            _ => {
                // Append dim zeros, then mark the row as null.
                let values = builder.values();
                for _ in 0..dim_usize {
                    values.append_value(0.0);
                }
                builder.append(false);
            }
        }
    }

    Ok(builder.finish())
}

/// Read an embedding from a `FixedSizeListArray` at row `i`.
fn read_vector(array: &FixedSizeListArray, i: usize) -> Option<Vec<f32>> {
    if array.is_null(i) {
        return None;
    }
    let values = array.value(i);
    let float_arr = values.as_any().downcast_ref::<Float32Array>()?;
    Some(float_arr.iter().map(|v| v.unwrap_or(0.0)).collect())
}

/// Read a `List(Utf8)` cell and return a `Vec<String>`.
fn read_string_list(array: &ListArray, i: usize) -> Vec<String> {
    if array.is_null(i) {
        return Vec::new();
    }
    let values = array.value(i);
    let string_arr = match values.as_any().downcast_ref::<StringArray>() {
        Some(a) => a,
        None => return Vec::new(),
    };
    (0..string_arr.len())
        .filter_map(|j| {
            if string_arr.is_null(j) {
                None
            } else {
                Some(string_arr.value(j).to_string())
            }
        })
        .collect()
}

/// Read a nullable Utf8 column value.
fn read_nullable_string(array: &StringArray, i: usize) -> Option<String> {
    if array.is_null(i) {
        None
    } else {
        Some(array.value(i).to_string())
    }
}

/// Read a nullable Int64 column value.
fn read_nullable_i64(array: &Int64Array, i: usize) -> Option<i64> {
    if array.is_null(i) {
        None
    } else {
        Some(array.value(i))
    }
}

// ============================================================================
// MemoryFact <-> RecordBatch
// ============================================================================

/// Convert a slice of `MemoryFact` into an Arrow `RecordBatch` matching
/// [`facts_schema`].
pub fn facts_to_record_batch(facts: &[MemoryFact]) -> Result<RecordBatch, AlephError> {
    let schema = facts_schema();
    let n = facts.len();

    // Scalar string columns
    let id_arr = StringArray::from_iter_values(facts.iter().map(|f| f.id.as_str()));
    let content_arr = StringArray::from_iter_values(facts.iter().map(|f| f.content.as_str()));
    let fact_type_arr =
        StringArray::from_iter_values(facts.iter().map(|f| f.fact_type.as_str()));
    let fact_source_arr =
        StringArray::from_iter_values(facts.iter().map(|f| f.fact_source.as_str()));
    let specificity_arr =
        StringArray::from_iter_values(facts.iter().map(|f| f.specificity.as_str()));
    let temporal_scope_arr =
        StringArray::from_iter_values(facts.iter().map(|f| f.temporal_scope.as_str()));
    let path_arr = StringArray::from_iter_values(facts.iter().map(|f| f.path.as_str()));
    let parent_path_arr =
        StringArray::from_iter_values(facts.iter().map(|f| f.parent_path.as_str()));
    let namespace_arr = StringArray::from_iter_values(facts.iter().map(|_| "owner"));
    let content_hash_arr =
        StringArray::from_iter_values(facts.iter().map(|f| f.content_hash.as_str()));
    let embedding_model_arr =
        StringArray::from_iter_values(facts.iter().map(|f| f.embedding_model.as_str()));

    // Numeric columns
    let confidence_arr = Float32Array::from_iter_values(facts.iter().map(|f| f.confidence));
    let decay_score_arr = Float32Array::from_iter_values(facts.iter().map(|_| 1.0_f32));
    let created_at_arr = Int64Array::from_iter_values(facts.iter().map(|f| f.created_at));
    let updated_at_arr = Int64Array::from_iter_values(facts.iter().map(|f| f.updated_at));
    let version_arr = Int32Array::from_iter_values(facts.iter().map(|_| 1_i32));

    // Boolean
    let is_valid_arr = BooleanArray::from(facts.iter().map(|f| Some(f.is_valid)).collect::<Vec<_>>());

    // Nullable string
    let invalidation_reason_arr = StringArray::from(
        facts
            .iter()
            .map(|f| f.invalidation_reason.as_deref())
            .collect::<Vec<_>>(),
    );

    // Nullable Int64
    let decay_invalidated_at_arr = Int64Array::from(
        facts
            .iter()
            .map(|f| f.decay_invalidated_at)
            .collect::<Vec<_>>(),
    );

    // List(Utf8): tags — empty for now (MemoryFact has no tags field)
    let mut tags_builder = ListBuilder::new(StringBuilder::new());
    for _ in 0..n {
        // Append an empty list per row.
        tags_builder.append(true);
    }
    let tags_arr = tags_builder.finish();

    // List(Utf8): source_memory_ids
    let mut src_ids_builder = ListBuilder::new(StringBuilder::new());
    for fact in facts {
        for id in &fact.source_memory_ids {
            src_ids_builder.values().append_value(id);
        }
        src_ids_builder.append(true);
    }
    let src_ids_arr = src_ids_builder.finish();

    // Vector columns
    let embeddings_384: Vec<Option<&Vec<f32>>> = facts
        .iter()
        .map(|f| {
            f.embedding
                .as_ref()
                .filter(|e| e.len() == 384)
        })
        .collect();
    let vec_384 = build_vector_column(&embeddings_384, 384)?;

    let embeddings_1024: Vec<Option<&Vec<f32>>> = facts
        .iter()
        .map(|f| {
            f.embedding
                .as_ref()
                .filter(|e| e.len() == 1024)
        })
        .collect();
    let vec_1024 = build_vector_column(&embeddings_1024, 1024)?;

    let embeddings_1536: Vec<Option<&Vec<f32>>> = facts
        .iter()
        .map(|f| {
            f.embedding
                .as_ref()
                .filter(|e| e.len() == 1536)
        })
        .collect();
    let vec_1536 = build_vector_column(&embeddings_1536, 1536)?;

    // Column order must match facts_schema() exactly.
    let batch = RecordBatch::try_new(
        schema,
        vec![
            Arc::new(id_arr),                    // 0  id
            Arc::new(content_arr),               // 1  content
            Arc::new(fact_type_arr),             // 2  fact_type
            Arc::new(fact_source_arr),           // 3  fact_source
            Arc::new(specificity_arr),           // 4  specificity
            Arc::new(temporal_scope_arr),        // 5  temporal_scope
            Arc::new(path_arr),                  // 6  path
            Arc::new(parent_path_arr),           // 7  parent_path
            Arc::new(namespace_arr),             // 8  namespace
            Arc::new(tags_arr),                  // 9  tags
            Arc::new(src_ids_arr),               // 10 source_memory_ids
            Arc::new(content_hash_arr),          // 11 content_hash
            Arc::new(confidence_arr),            // 12 confidence
            Arc::new(decay_score_arr),           // 13 decay_score
            Arc::new(is_valid_arr),              // 14 is_valid
            Arc::new(invalidation_reason_arr),   // 15 invalidation_reason
            Arc::new(embedding_model_arr),       // 16 embedding_model
            Arc::new(created_at_arr),            // 17 created_at
            Arc::new(updated_at_arr),            // 18 updated_at
            Arc::new(decay_invalidated_at_arr),  // 19 decay_invalidated_at
            Arc::new(version_arr),               // 20 version
            Arc::new(vec_384),                   // 21 vec_384
            Arc::new(vec_1024),                  // 22 vec_1024
            Arc::new(vec_1536),                  // 23 vec_1536
        ],
    )
    .map_err(|e| conv_err(e))?;

    Ok(batch)
}

/// Convert an Arrow `RecordBatch` back into a `Vec<MemoryFact>`.
pub fn record_batch_to_facts(batch: &RecordBatch) -> Result<Vec<MemoryFact>, AlephError> {
    let n = batch.num_rows();
    if n == 0 {
        return Ok(Vec::new());
    }

    let id_col = col::<StringArray>(batch, "id")?;
    let content_col = col::<StringArray>(batch, "content")?;
    let fact_type_col = col::<StringArray>(batch, "fact_type")?;
    let fact_source_col = col::<StringArray>(batch, "fact_source")?;
    let specificity_col = col::<StringArray>(batch, "specificity")?;
    let temporal_scope_col = col::<StringArray>(batch, "temporal_scope")?;
    let path_col = col::<StringArray>(batch, "path")?;
    let parent_path_col = col::<StringArray>(batch, "parent_path")?;
    let content_hash_col = col::<StringArray>(batch, "content_hash")?;
    let embedding_model_col = col::<StringArray>(batch, "embedding_model")?;
    let confidence_col = col::<Float32Array>(batch, "confidence")?;
    let is_valid_col = col::<BooleanArray>(batch, "is_valid")?;
    let invalidation_reason_col = col::<StringArray>(batch, "invalidation_reason")?;
    let created_at_col = col::<Int64Array>(batch, "created_at")?;
    let updated_at_col = col::<Int64Array>(batch, "updated_at")?;
    let decay_invalidated_at_col = col::<Int64Array>(batch, "decay_invalidated_at")?;
    let src_ids_col = col::<ListArray>(batch, "source_memory_ids")?;

    // Vector columns (optional — may not all be present).
    let vec_384_col = col::<FixedSizeListArray>(batch, "vec_384").ok();
    let vec_1024_col = col::<FixedSizeListArray>(batch, "vec_1024").ok();
    let vec_1536_col = col::<FixedSizeListArray>(batch, "vec_1536").ok();

    let mut facts = Vec::with_capacity(n);
    for i in 0..n {
        // Determine embedding: prefer vec_384, then 1024, then 1536.
        let embedding = vec_384_col
            .and_then(|c| read_vector(c, i))
            .or_else(|| vec_1024_col.and_then(|c| read_vector(c, i)))
            .or_else(|| vec_1536_col.and_then(|c| read_vector(c, i)));

        let fact = MemoryFact {
            id: id_col.value(i).to_string(),
            content: content_col.value(i).to_string(),
            fact_type: FactType::from_str_or_other(fact_type_col.value(i)),
            fact_source: FactSource::from_str_or_default(fact_source_col.value(i)),
            specificity: FactSpecificity::from_str_or_default(specificity_col.value(i)),
            temporal_scope: TemporalScope::from_str_or_default(temporal_scope_col.value(i)),
            path: path_col.value(i).to_string(),
            parent_path: parent_path_col.value(i).to_string(),
            content_hash: content_hash_col.value(i).to_string(),
            embedding_model: embedding_model_col.value(i).to_string(),
            confidence: confidence_col.value(i),
            is_valid: is_valid_col.value(i),
            invalidation_reason: read_nullable_string(invalidation_reason_col, i),
            created_at: created_at_col.value(i),
            updated_at: updated_at_col.value(i),
            decay_invalidated_at: read_nullable_i64(decay_invalidated_at_col, i),
            source_memory_ids: read_string_list(src_ids_col, i),
            embedding,
            similarity_score: None,
        };
        facts.push(fact);
    }

    Ok(facts)
}

// ============================================================================
// GraphNode <-> RecordBatch
// ============================================================================

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
        ],
    )
    .map_err(|e| conv_err(e))?;

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
        });
    }

    Ok(nodes)
}

// ============================================================================
// GraphEdge <-> RecordBatch
// ============================================================================

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
        ],
    )
    .map_err(|e| conv_err(e))?;

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
        });
    }

    Ok(edges)
}

// ============================================================================
// MemoryEntry <-> RecordBatch
// ============================================================================

/// Convert a slice of `MemoryEntry` into an Arrow `RecordBatch` matching
/// [`memories_schema`].
pub fn memories_to_record_batch(memories: &[MemoryEntry]) -> Result<RecordBatch, AlephError> {
    let schema = memories_schema();

    let id_arr = StringArray::from_iter_values(memories.iter().map(|m| m.id.as_str()));
    let app_arr =
        StringArray::from_iter_values(memories.iter().map(|m| m.context.app_bundle_id.as_str()));
    let window_arr =
        StringArray::from_iter_values(memories.iter().map(|m| m.context.window_title.as_str()));
    let user_input_arr =
        StringArray::from_iter_values(memories.iter().map(|m| m.user_input.as_str()));
    let ai_output_arr =
        StringArray::from_iter_values(memories.iter().map(|m| m.ai_output.as_str()));
    let timestamp_arr = Int64Array::from_iter_values(memories.iter().map(|m| m.context.timestamp));
    let topic_id_arr = StringArray::from(
        memories
            .iter()
            .map(|m| Some(m.context.topic_id.as_str()))
            .collect::<Vec<_>>(),
    );
    let session_key_arr = StringArray::from_iter_values(memories.iter().map(|_| "default"));
    let namespace_arr = StringArray::from_iter_values(memories.iter().map(|_| "owner"));

    // Vector column
    let embeddings: Vec<Option<&Vec<f32>>> = memories
        .iter()
        .map(|m| m.embedding.as_ref().filter(|e| e.len() == 384))
        .collect();
    let vec_384 = build_vector_column(&embeddings, 384)?;

    let batch = RecordBatch::try_new(
        schema,
        vec![
            Arc::new(id_arr),           // 0 id
            Arc::new(app_arr),          // 1 app_bundle_id
            Arc::new(window_arr),       // 2 window_title
            Arc::new(user_input_arr),   // 3 user_input
            Arc::new(ai_output_arr),    // 4 ai_output
            Arc::new(timestamp_arr),    // 5 timestamp
            Arc::new(topic_id_arr),     // 6 topic_id
            Arc::new(session_key_arr),  // 7 session_key
            Arc::new(namespace_arr),    // 8 namespace
            Arc::new(vec_384),          // 9 vec_384
        ],
    )
    .map_err(|e| conv_err(e))?;

    Ok(batch)
}

/// Convert an Arrow `RecordBatch` back into a `Vec<MemoryEntry>`.
pub fn record_batch_to_memories(batch: &RecordBatch) -> Result<Vec<MemoryEntry>, AlephError> {
    let n = batch.num_rows();
    if n == 0 {
        return Ok(Vec::new());
    }

    let id_col = col::<StringArray>(batch, "id")?;
    let app_col = col::<StringArray>(batch, "app_bundle_id")?;
    let window_col = col::<StringArray>(batch, "window_title")?;
    let user_input_col = col::<StringArray>(batch, "user_input")?;
    let ai_output_col = col::<StringArray>(batch, "ai_output")?;
    let timestamp_col = col::<Int64Array>(batch, "timestamp")?;
    let topic_id_col = col::<StringArray>(batch, "topic_id").ok();
    let vec_384_col = col::<FixedSizeListArray>(batch, "vec_384").ok();

    let mut entries = Vec::with_capacity(n);
    for i in 0..n {
        let topic_id = topic_id_col
            .and_then(|c| read_nullable_string(c, i))
            .unwrap_or_else(|| crate::memory::context::SINGLE_TURN_TOPIC_ID.to_string());

        let context = ContextAnchor {
            app_bundle_id: app_col.value(i).to_string(),
            window_title: window_col.value(i).to_string(),
            timestamp: timestamp_col.value(i),
            topic_id,
        };

        let embedding = vec_384_col.and_then(|c| read_vector(c, i));

        entries.push(MemoryEntry {
            id: id_col.value(i).to_string(),
            context,
            user_input: user_input_col.value(i).to_string(),
            ai_output: ai_output_col.value(i).to_string(),
            embedding,
            similarity_score: None,
        });
    }

    Ok(entries)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create a test MemoryFact with an embedding.
    fn make_fact_with_embedding() -> MemoryFact {
        let mut fact = MemoryFact::new(
            "User prefers Rust for systems programming".to_string(),
            FactType::Preference,
            vec!["mem-001".to_string(), "mem-002".to_string()],
        );
        fact.id = "fact-test-001".to_string();
        fact.confidence = 0.95;
        fact.specificity = FactSpecificity::Pattern;
        fact.temporal_scope = TemporalScope::Permanent;
        fact.fact_source = FactSource::Extracted;
        fact.content_hash = "abc123".to_string();
        fact.embedding_model = "bge-small-zh-v1.5".to_string();
        fact.embedding = Some(vec![0.1_f32; 384]);
        fact.path = "aleph://user/preferences/coding/".to_string();
        fact.parent_path = "aleph://user/preferences/".to_string();
        fact.created_at = 1700000000;
        fact.updated_at = 1700000100;
        fact
    }

    /// Helper: create a test MemoryFact without embedding.
    fn make_fact_no_embedding() -> MemoryFact {
        let mut fact = MemoryFact::new(
            "User is learning WebAssembly".to_string(),
            FactType::Learning,
            vec!["mem-003".to_string()],
        );
        fact.id = "fact-test-002".to_string();
        fact.confidence = 0.8;
        fact.is_valid = false;
        fact.invalidation_reason = Some("superseded by newer fact".to_string());
        fact.decay_invalidated_at = Some(1700001000);
        fact.content_hash = "def456".to_string();
        fact.embedding_model = "".to_string();
        fact.created_at = 1700000200;
        fact.updated_at = 1700000300;
        fact
    }

    #[test]
    fn test_fact_roundtrip() {
        let original = make_fact_with_embedding();
        let batch = facts_to_record_batch(&[original.clone()]).expect("to_batch");
        assert_eq!(batch.num_rows(), 1);

        let recovered = record_batch_to_facts(&batch).expect("from_batch");
        assert_eq!(recovered.len(), 1);

        let f = &recovered[0];
        assert_eq!(f.id, original.id);
        assert_eq!(f.content, original.content);
        assert_eq!(f.fact_type, original.fact_type);
        assert_eq!(f.fact_source, original.fact_source);
        assert_eq!(f.specificity, original.specificity);
        assert_eq!(f.temporal_scope, original.temporal_scope);
        assert_eq!(f.path, original.path);
        assert_eq!(f.parent_path, original.parent_path);
        assert_eq!(f.content_hash, original.content_hash);
        assert_eq!(f.embedding_model, original.embedding_model);
        assert!((f.confidence - original.confidence).abs() < f32::EPSILON);
        assert_eq!(f.is_valid, original.is_valid);
        assert_eq!(f.invalidation_reason, original.invalidation_reason);
        assert_eq!(f.created_at, original.created_at);
        assert_eq!(f.updated_at, original.updated_at);
        assert_eq!(f.decay_invalidated_at, original.decay_invalidated_at);
        assert_eq!(f.source_memory_ids, original.source_memory_ids);

        // Embedding roundtrip
        let emb = f.embedding.as_ref().expect("should have embedding");
        assert_eq!(emb.len(), 384);
        assert!((emb[0] - 0.1).abs() < f32::EPSILON);
    }

    #[test]
    fn test_fact_roundtrip_no_embedding() {
        let original = make_fact_no_embedding();
        let batch = facts_to_record_batch(&[original.clone()]).expect("to_batch");
        assert_eq!(batch.num_rows(), 1);

        let recovered = record_batch_to_facts(&batch).expect("from_batch");
        assert_eq!(recovered.len(), 1);

        let f = &recovered[0];
        assert_eq!(f.id, original.id);
        assert_eq!(f.content, original.content);
        assert_eq!(f.fact_type, original.fact_type);
        assert!(!f.is_valid);
        assert_eq!(
            f.invalidation_reason,
            Some("superseded by newer fact".to_string())
        );
        assert_eq!(f.decay_invalidated_at, Some(1700001000));
        assert!(f.embedding.is_none());
    }

    #[test]
    fn test_fact_batch_multiple() {
        let facts = vec![make_fact_with_embedding(), make_fact_no_embedding()];
        let batch = facts_to_record_batch(&facts).expect("to_batch");
        assert_eq!(batch.num_rows(), 2);

        let recovered = record_batch_to_facts(&batch).expect("from_batch");
        assert_eq!(recovered.len(), 2);
        assert_eq!(recovered[0].id, "fact-test-001");
        assert_eq!(recovered[1].id, "fact-test-002");
        assert!(recovered[0].embedding.is_some());
        assert!(recovered[1].embedding.is_none());
    }

    #[test]
    fn test_fact_empty_batch() {
        let batch = facts_to_record_batch(&[]).expect("empty to_batch");
        assert_eq!(batch.num_rows(), 0);
        let recovered = record_batch_to_facts(&batch).expect("empty from_batch");
        assert!(recovered.is_empty());
    }

    #[test]
    fn test_graph_node_roundtrip() {
        let node = GraphNode {
            id: "gn_test_001".to_string(),
            name: "Rust".to_string(),
            kind: "language".to_string(),
            aliases: vec!["rust-lang".to_string(), "Rust Programming".to_string()],
            metadata_json: r#"{"category":"systems"}"#.to_string(),
            decay_score: 0.95,
            created_at: 1700000000,
            updated_at: 1700000100,
        };

        let batch = graph_nodes_to_record_batch(&[node.clone()]).expect("to_batch");
        assert_eq!(batch.num_rows(), 1);

        let recovered = record_batch_to_graph_nodes(&batch).expect("from_batch");
        assert_eq!(recovered.len(), 1);

        let n = &recovered[0];
        assert_eq!(n.id, node.id);
        assert_eq!(n.name, node.name);
        assert_eq!(n.kind, node.kind);
        assert_eq!(n.aliases, node.aliases);
        assert_eq!(n.metadata_json, node.metadata_json);
        assert!((n.decay_score - node.decay_score).abs() < f32::EPSILON);
        assert_eq!(n.created_at, node.created_at);
        assert_eq!(n.updated_at, node.updated_at);
    }

    #[test]
    fn test_graph_node_empty_aliases() {
        let node = GraphNode {
            id: "gn_test_002".to_string(),
            name: "WebAssembly".to_string(),
            kind: "technology".to_string(),
            aliases: vec![],
            metadata_json: String::new(),
            decay_score: 1.0,
            created_at: 1700000000,
            updated_at: 1700000000,
        };

        let batch = graph_nodes_to_record_batch(&[node.clone()]).expect("to_batch");
        let recovered = record_batch_to_graph_nodes(&batch).expect("from_batch");
        assert_eq!(recovered.len(), 1);
        assert!(recovered[0].aliases.is_empty());
        assert!(recovered[0].metadata_json.is_empty());
    }

    #[test]
    fn test_graph_edge_roundtrip() {
        let edge = GraphEdge {
            id: "ge_test_001".to_string(),
            from_id: "gn_001".to_string(),
            to_id: "gn_002".to_string(),
            relation: "uses".to_string(),
            weight: 2.5,
            confidence: 0.9,
            context_key: "app:com.test|window:doc".to_string(),
            decay_score: 0.85,
            created_at: 1700000000,
            updated_at: 1700000100,
            last_seen_at: 1700000200,
        };

        let batch = graph_edges_to_record_batch(&[edge.clone()]).expect("to_batch");
        assert_eq!(batch.num_rows(), 1);

        let recovered = record_batch_to_graph_edges(&batch).expect("from_batch");
        assert_eq!(recovered.len(), 1);

        let e = &recovered[0];
        assert_eq!(e.id, edge.id);
        assert_eq!(e.from_id, edge.from_id);
        assert_eq!(e.to_id, edge.to_id);
        assert_eq!(e.relation, edge.relation);
        assert!((e.weight - edge.weight).abs() < f32::EPSILON);
        assert!((e.confidence - edge.confidence).abs() < f32::EPSILON);
        assert_eq!(e.context_key, edge.context_key);
        assert!((e.decay_score - edge.decay_score).abs() < f32::EPSILON);
        assert_eq!(e.created_at, edge.created_at);
        assert_eq!(e.updated_at, edge.updated_at);
        assert_eq!(e.last_seen_at, edge.last_seen_at);
    }

    #[test]
    fn test_graph_edge_empty_batch() {
        let batch = graph_edges_to_record_batch(&[]).expect("empty to_batch");
        assert_eq!(batch.num_rows(), 0);
        let recovered = record_batch_to_graph_edges(&batch).expect("empty from_batch");
        assert!(recovered.is_empty());
    }

    #[test]
    fn test_memory_entry_roundtrip() {
        let context = ContextAnchor {
            app_bundle_id: "com.apple.Notes".to_string(),
            window_title: "Project Plan".to_string(),
            timestamp: 1700000000,
            topic_id: "topic-abc".to_string(),
        };
        let entry = MemoryEntry {
            id: "mem-test-001".to_string(),
            context,
            user_input: "What is Rust?".to_string(),
            ai_output: "Rust is a systems programming language.".to_string(),
            embedding: Some(vec![0.5_f32; 384]),
            similarity_score: None,
        };

        let batch = memories_to_record_batch(&[entry.clone()]).expect("to_batch");
        assert_eq!(batch.num_rows(), 1);

        let recovered = record_batch_to_memories(&batch).expect("from_batch");
        assert_eq!(recovered.len(), 1);

        let m = &recovered[0];
        assert_eq!(m.id, entry.id);
        assert_eq!(m.context.app_bundle_id, entry.context.app_bundle_id);
        assert_eq!(m.context.window_title, entry.context.window_title);
        assert_eq!(m.context.timestamp, entry.context.timestamp);
        assert_eq!(m.context.topic_id, "topic-abc");
        assert_eq!(m.user_input, entry.user_input);
        assert_eq!(m.ai_output, entry.ai_output);

        let emb = m.embedding.as_ref().expect("should have embedding");
        assert_eq!(emb.len(), 384);
        assert!((emb[0] - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn test_memory_entry_no_embedding() {
        let context = ContextAnchor {
            app_bundle_id: "com.test.app".to_string(),
            window_title: "Window".to_string(),
            timestamp: 1700000500,
            topic_id: crate::memory::context::SINGLE_TURN_TOPIC_ID.to_string(),
        };
        let entry = MemoryEntry {
            id: "mem-test-002".to_string(),
            context,
            user_input: "Hello".to_string(),
            ai_output: "Hi there!".to_string(),
            embedding: None,
            similarity_score: None,
        };

        let batch = memories_to_record_batch(&[entry.clone()]).expect("to_batch");
        let recovered = record_batch_to_memories(&batch).expect("from_batch");
        assert_eq!(recovered.len(), 1);
        assert!(recovered[0].embedding.is_none());
        assert_eq!(recovered[0].context.topic_id, "single-turn");
    }

    #[test]
    fn test_memory_entry_empty_batch() {
        let batch = memories_to_record_batch(&[]).expect("empty to_batch");
        assert_eq!(batch.num_rows(), 0);
        let recovered = record_batch_to_memories(&batch).expect("empty from_batch");
        assert!(recovered.is_empty());
    }
}
