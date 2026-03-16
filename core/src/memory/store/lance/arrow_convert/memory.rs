//! Arrow RecordBatch <-> MemoryEntry conversions.

use crate::sync_primitives::Arc;

use arrow_array::{FixedSizeListArray, Int64Array, RecordBatch, StringArray};

use crate::error::AlephError;
use crate::memory::context::{ContextAnchor, MemoryEntry};

use super::helpers::{
    build_vector_column, col, conv_err, normalize_embedding, read_nullable_string, read_vector,
};
use crate::memory::store::lance::schema::memories_schema;

/// Convert a slice of `MemoryEntry` into an Arrow `RecordBatch` matching
/// [`memories_schema`].
pub fn memories_to_record_batch(memories: &[MemoryEntry]) -> Result<RecordBatch, AlephError> {
    let schema = memories_schema();

    let id_arr = StringArray::from_iter_values(memories.iter().map(|m| m.id.as_str()));
    let window_arr =
        StringArray::from_iter_values(memories.iter().map(|m| m.context.window_title.as_str()));
    let user_input_arr =
        StringArray::from_iter_values(memories.iter().map(|m| m.user_input.as_str()));
    let ai_output_arr =
        StringArray::from_iter_values(memories.iter().map(|m| m.ai_output.as_str()));
    let timestamp_arr = Int64Array::from_iter_values(memories.iter().map(|m| m.context.timestamp));
    let session_id_arr = StringArray::from(
        memories
            .iter()
            .map(|m| Some(m.context.session_id.as_str()))
            .collect::<Vec<_>>(),
    );
    let session_key_arr = StringArray::from_iter_values(memories.iter().map(|_| "default"));
    let namespace_arr = StringArray::from_iter_values(memories.iter().map(|m| m.namespace.as_str()));
    let workspace_arr = StringArray::from_iter_values(memories.iter().map(|m| m.agent.as_str()));

    // Vector columns (multi-dimension coexistence, same pattern as facts)
    let normalized: Vec<Option<Vec<f32>>> = memories
        .iter()
        .map(|m| m.embedding.as_deref().and_then(normalize_embedding))
        .collect();

    let embeddings_768: Vec<Option<&[f32]>> = normalized
        .iter()
        .map(|e| e.as_deref().filter(|v| v.len() == 768))
        .collect();
    let vec_768 = build_vector_column(&embeddings_768, 768)?;

    let embeddings_1024: Vec<Option<&[f32]>> = normalized
        .iter()
        .map(|e| e.as_deref().filter(|v| v.len() == 1024))
        .collect();
    let vec_1024 = build_vector_column(&embeddings_1024, 1024)?;

    let embeddings_1536: Vec<Option<&[f32]>> = normalized
        .iter()
        .map(|e| e.as_deref().filter(|v| v.len() == 1536))
        .collect();
    let vec_1536 = build_vector_column(&embeddings_1536, 1536)?;

    let batch = RecordBatch::try_new(
        schema,
        vec![
            Arc::new(id_arr),           // 0 id
            Arc::new(window_arr),       // 1 window_title
            Arc::new(user_input_arr),   // 2 user_input
            Arc::new(ai_output_arr),    // 3 ai_output
            Arc::new(timestamp_arr),    // 4 timestamp
            Arc::new(session_id_arr),   // 5 session_id
            Arc::new(session_key_arr),  // 6 session_key
            Arc::new(namespace_arr),    // 7 namespace
            Arc::new(workspace_arr),    // 8 workspace
            Arc::new(vec_768),          // 9  vec_768
            Arc::new(vec_1024),         // 10 vec_1024
            Arc::new(vec_1536),         // 11 vec_1536
        ],
    )
    .map_err(conv_err)?;

    Ok(batch)
}

/// Convert an Arrow `RecordBatch` back into a `Vec<MemoryEntry>`.
pub fn record_batch_to_memories(batch: &RecordBatch) -> Result<Vec<MemoryEntry>, AlephError> {
    let n = batch.num_rows();
    if n == 0 {
        return Ok(Vec::new());
    }

    let id_col = col::<StringArray>(batch, "id")?;
    let window_col = col::<StringArray>(batch, "window_title")?;
    let user_input_col = col::<StringArray>(batch, "user_input")?;
    let ai_output_col = col::<StringArray>(batch, "ai_output")?;
    let timestamp_col = col::<Int64Array>(batch, "timestamp")?;
    let session_id_col = col::<StringArray>(batch, "session_id")
        .or_else(|_| col::<StringArray>(batch, "topic_id"))
        .ok();
    // namespace and workspace columns (with fallback for backward compatibility)
    let namespace_col = col::<StringArray>(batch, "namespace").ok();
    let workspace_col = col::<StringArray>(batch, "agent").ok();
    let vec_768_col = col::<FixedSizeListArray>(batch, "vec_768").ok();
    let vec_1024_col = col::<FixedSizeListArray>(batch, "vec_1024").ok();
    let vec_1536_col = col::<FixedSizeListArray>(batch, "vec_1536").ok();

    let mut entries = Vec::with_capacity(n);
    for i in 0..n {
        let session_id = session_id_col
            .and_then(|c| read_nullable_string(c, i))
            .unwrap_or_else(|| crate::memory::context::NO_SESSION.to_string());

        let context = ContextAnchor {
            window_title: window_col.value(i).to_string(),
            timestamp: timestamp_col.value(i),
            session_id,
        };

        // Prefer vec_768, then 1024, then 1536 (same priority as facts)
        let embedding = vec_768_col
            .and_then(|c| read_vector(c, i))
            .or_else(|| vec_1024_col.and_then(|c| read_vector(c, i)))
            .or_else(|| vec_1536_col.and_then(|c| read_vector(c, i)));

        entries.push(MemoryEntry {
            id: id_col.value(i).to_string(),
            context,
            user_input: user_input_col.value(i).to_string(),
            ai_output: ai_output_col.value(i).to_string(),
            embedding,
            namespace: namespace_col
                .map(|c| c.value(i).to_string())
                .unwrap_or_else(|| "owner".to_string()),
            agent: workspace_col
                .map(|c| c.value(i).to_string())
                .unwrap_or_else(|| "default".to_string()),
            similarity_score: None,
        });
    }

    Ok(entries)
}
