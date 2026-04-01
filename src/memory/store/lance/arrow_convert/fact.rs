//! Arrow RecordBatch <-> MemoryFact conversions.

use crate::sync_primitives::Arc;

use arrow_array::builder::{ListBuilder, StringBuilder};
use arrow_array::{
    BooleanArray, FixedSizeListArray, Float32Array, Int32Array, Int64Array, ListArray, RecordBatch,
    StringArray,
};

use crate::error::AlephError;
use crate::memory::context::{
    FactSource, FactSpecificity, FactType, MemoryCategory, MemoryFact, MemoryLayer, MemoryScope,
    MemoryTier, TemporalScope,
};

use super::helpers::{
    build_vector_column, col, conv_err, normalize_embedding, read_nullable_i64,
    read_nullable_string, read_string_list, read_vector,
};
use crate::memory::store::lance::schema::facts_schema;

/// Convert a slice of `MemoryFact` into an Arrow `RecordBatch` matching
/// [`facts_schema`].
pub fn facts_to_record_batch(facts: &[MemoryFact]) -> Result<RecordBatch, AlephError> {
    let schema = facts_schema();
    let n = facts.len();

    // Scalar string columns
    let id_arr = StringArray::from_iter_values(facts.iter().map(|f| f.id.as_str()));
    let content_arr = StringArray::from_iter_values(facts.iter().map(|f| f.content.as_str()));
    let fact_type_arr = StringArray::from_iter_values(facts.iter().map(|f| f.fact_type.as_str()));
    let fact_source_arr =
        StringArray::from_iter_values(facts.iter().map(|f| f.fact_source.as_str()));
    let specificity_arr =
        StringArray::from_iter_values(facts.iter().map(|f| f.specificity.as_str()));
    let temporal_scope_arr =
        StringArray::from_iter_values(facts.iter().map(|f| f.temporal_scope.as_str()));
    let layer_arr = StringArray::from_iter_values(facts.iter().map(|f| f.layer.as_str()));
    let category_arr = StringArray::from_iter_values(facts.iter().map(|f| f.category.as_str()));
    let path_arr = StringArray::from_iter_values(facts.iter().map(|f| f.path.as_str()));
    let parent_path_arr =
        StringArray::from_iter_values(facts.iter().map(|f| f.parent_path.as_str()));
    let namespace_arr = StringArray::from_iter_values(facts.iter().map(|f| f.namespace.as_str()));
    let workspace_arr = StringArray::from_iter_values(facts.iter().map(|f| f.agent.as_str()));
    let content_hash_arr =
        StringArray::from_iter_values(facts.iter().map(|f| f.content_hash.as_str()));
    let embedding_model_arr =
        StringArray::from_iter_values(facts.iter().map(|f| f.embedding_model.as_str()));

    // Numeric columns
    let confidence_arr = Float32Array::from_iter_values(facts.iter().map(|f| f.confidence));
    let decay_score_arr = Float32Array::from_iter_values(facts.iter().map(|f| f.strength));
    let created_at_arr = Int64Array::from_iter_values(facts.iter().map(|f| f.created_at));
    let updated_at_arr = Int64Array::from_iter_values(facts.iter().map(|f| f.updated_at));
    let version_arr = Int32Array::from_iter_values(facts.iter().map(|_| 1_i32));

    // Boolean
    let is_valid_arr =
        BooleanArray::from(facts.iter().map(|f| Some(f.is_valid)).collect::<Vec<_>>());

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

    // List(Utf8): tags -- empty for now (MemoryFact has no tags field)
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

    // ACMA fields
    let tier_arr = StringArray::from_iter_values(facts.iter().map(|f| f.tier.as_str()));
    let scope_arr = StringArray::from_iter_values(facts.iter().map(|f| f.scope.as_str()));
    let persona_id_arr = StringArray::from(
        facts
            .iter()
            .map(|f| f.persona_id.as_deref())
            .collect::<Vec<_>>(),
    );
    let strength_arr = Float32Array::from_iter_values(facts.iter().map(|f| f.strength));
    let access_count_arr = Int32Array::from_iter_values(
        facts
            .iter()
            .map(|f| f.access_count.min(i32::MAX as u32) as i32),
    );
    let last_accessed_at_arr =
        Int64Array::from(facts.iter().map(|f| f.last_accessed_at).collect::<Vec<_>>());

    // Vector columns (multi-dimension coexistence).
    // Non-standard dimensions are normalized (truncated + L2-normalized) to
    // the nearest smaller supported size (768, 1024, 1536).
    let normalized: Vec<Option<Vec<f32>>> = facts
        .iter()
        .map(|f| f.embedding.as_deref().and_then(normalize_embedding))
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

    // Column order must match facts_schema() exactly.
    let batch = RecordBatch::try_new(
        schema,
        vec![
            Arc::new(id_arr),                   // 0  id
            Arc::new(content_arr),              // 1  content
            Arc::new(fact_type_arr),            // 2  fact_type
            Arc::new(fact_source_arr),          // 3  fact_source
            Arc::new(specificity_arr),          // 4  specificity
            Arc::new(temporal_scope_arr),       // 5  temporal_scope
            Arc::new(layer_arr),                // 6  layer
            Arc::new(category_arr),             // 7  category
            Arc::new(path_arr),                 // 8  path
            Arc::new(parent_path_arr),          // 9  parent_path
            Arc::new(namespace_arr),            // 10 namespace
            Arc::new(workspace_arr),            // 11 workspace
            Arc::new(tags_arr),                 // 12 tags
            Arc::new(src_ids_arr),              // 13 source_memory_ids
            Arc::new(content_hash_arr),         // 14 content_hash
            Arc::new(confidence_arr),           // 15 confidence
            Arc::new(decay_score_arr),          // 16 decay_score
            Arc::new(is_valid_arr),             // 17 is_valid
            Arc::new(invalidation_reason_arr),  // 18 invalidation_reason
            Arc::new(embedding_model_arr),      // 19 embedding_model
            Arc::new(created_at_arr),           // 20 created_at
            Arc::new(updated_at_arr),           // 21 updated_at
            Arc::new(decay_invalidated_at_arr), // 22 decay_invalidated_at
            Arc::new(version_arr),              // 23 version
            Arc::new(tier_arr),                 // 24 tier
            Arc::new(scope_arr),                // 25 scope
            Arc::new(persona_id_arr),           // 26 persona_id
            Arc::new(strength_arr),             // 27 strength
            Arc::new(access_count_arr),         // 28 access_count
            Arc::new(last_accessed_at_arr),     // 29 last_accessed_at
            Arc::new(vec_768),                  // 30 vec_768
            Arc::new(vec_1024),                 // 31 vec_1024
            Arc::new(vec_1536),                 // 32 vec_1536
        ],
    )
    .map_err(conv_err)?;

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
    let layer_col = col::<StringArray>(batch, "layer").ok();
    let category_col = col::<StringArray>(batch, "category").ok();
    let path_col = col::<StringArray>(batch, "path")?;
    let parent_path_col = col::<StringArray>(batch, "parent_path")?;
    let content_hash_col = col::<StringArray>(batch, "content_hash")?;
    let embedding_model_col = col::<StringArray>(batch, "embedding_model")?;
    // namespace and workspace columns (with fallback for backward compatibility)
    let namespace_col = col::<StringArray>(batch, "namespace").ok();
    let workspace_col = col::<StringArray>(batch, "agent").ok();
    let confidence_col = col::<Float32Array>(batch, "confidence")?;
    let is_valid_col = col::<BooleanArray>(batch, "is_valid")?;
    let invalidation_reason_col = col::<StringArray>(batch, "invalidation_reason")?;
    let created_at_col = col::<Int64Array>(batch, "created_at")?;
    let updated_at_col = col::<Int64Array>(batch, "updated_at")?;
    let decay_invalidated_at_col = col::<Int64Array>(batch, "decay_invalidated_at")?;
    let src_ids_col = col::<ListArray>(batch, "source_memory_ids")?;

    // ACMA columns (optional -- backward compatible with old data).
    let tier_col = col::<StringArray>(batch, "tier").ok();
    let scope_col = col::<StringArray>(batch, "scope").ok();
    let persona_id_col = col::<StringArray>(batch, "persona_id").ok();
    let strength_col = col::<Float32Array>(batch, "strength").ok();
    let access_count_col = col::<Int32Array>(batch, "access_count").ok();
    let last_accessed_at_col = col::<Int64Array>(batch, "last_accessed_at").ok();

    // Vector columns (optional -- may not all be present).
    let vec_768_col = col::<FixedSizeListArray>(batch, "vec_768").ok();
    let vec_1024_col = col::<FixedSizeListArray>(batch, "vec_1024").ok();
    let vec_1536_col = col::<FixedSizeListArray>(batch, "vec_1536").ok();

    let mut facts = Vec::with_capacity(n);
    for i in 0..n {
        let fact_type = FactType::from_str_or_other(fact_type_col.value(i));
        let layer = layer_col
            .map(|c| MemoryLayer::from_str_or_default(c.value(i)))
            .unwrap_or(MemoryLayer::L2Detail);
        let category = category_col
            .map(|c| MemoryCategory::from_str_or_default(c.value(i)))
            .unwrap_or_else(|| fact_type.default_category());

        // Determine embedding: prefer vec_768, then 1024, then 1536.
        let embedding = vec_768_col
            .and_then(|c| read_vector(c, i))
            .or_else(|| vec_1024_col.and_then(|c| read_vector(c, i)))
            .or_else(|| vec_1536_col.and_then(|c| read_vector(c, i)));

        // ACMA fields with backward-compatible defaults
        let tier = tier_col
            .map(|c| MemoryTier::from_str_or_default(c.value(i)))
            .unwrap_or(MemoryTier::ShortTerm);
        let scope = scope_col
            .map(|c| MemoryScope::from_str_or_default(c.value(i)))
            .unwrap_or(MemoryScope::Global);
        let persona_id = persona_id_col.and_then(|c| read_nullable_string(c, i));
        let strength = strength_col.map(|c| c.value(i)).unwrap_or(1.0);
        let access_count = access_count_col.map(|c| c.value(i) as u32).unwrap_or(0);
        let last_accessed_at = last_accessed_at_col.and_then(|c| read_nullable_i64(c, i));

        let fact = MemoryFact {
            id: id_col.value(i).to_string(),
            content: content_col.value(i).to_string(),
            fact_type,
            fact_source: FactSource::from_str_or_default(fact_source_col.value(i)),
            specificity: FactSpecificity::from_str_or_default(specificity_col.value(i)),
            temporal_scope: TemporalScope::from_str_or_default(temporal_scope_col.value(i)),
            path: path_col.value(i).to_string(),
            layer,
            category,
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
            namespace: namespace_col
                .map(|c| c.value(i).to_string())
                .unwrap_or_else(|| "owner".to_string()),
            agent: workspace_col
                .map(|c| c.value(i).to_string())
                .unwrap_or_else(|| "default".to_string()),
            embedding,
            similarity_score: None,
            tier,
            scope,
            persona_id,
            strength,
            access_count,
            last_accessed_at,
        };
        facts.push(fact);
    }

    Ok(facts)
}
