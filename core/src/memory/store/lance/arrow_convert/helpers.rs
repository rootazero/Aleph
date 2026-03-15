//! Shared helper functions for Arrow <-> domain type conversions.

use arrow_array::builder::{FixedSizeListBuilder, Float32Builder};
use arrow_array::{Array, FixedSizeListArray, Float32Array, Int64Array, ListArray, RecordBatch, StringArray};
use tracing::warn;

use crate::error::AlephError;
use crate::memory::embedding_provider::truncate_and_normalize;

/// Shorthand for building an AlephError from an arrow conversion failure.
pub(super) fn conv_err(msg: impl std::fmt::Display) -> AlephError {
    AlephError::config(format!("Arrow conversion error: {}", msg))
}

/// Downcast a column by name to the concrete array type `T`.
pub(super) fn col<'a, T: 'static>(batch: &'a RecordBatch, name: &str) -> Result<&'a T, AlephError> {
    let array = batch
        .column_by_name(name)
        .ok_or_else(|| conv_err(format!("missing column '{}'", name)))?;
    array
        .as_any()
        .downcast_ref::<T>()
        .ok_or_else(|| conv_err(format!("column '{}' has unexpected type", name)))
}

/// Build a nullable `FixedSizeList(Float32, dim)` array from optional embeddings.
pub(super) fn build_vector_column(
    embeddings: &[Option<&[f32]>],
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
pub(super) fn read_vector(array: &FixedSizeListArray, i: usize) -> Option<Vec<f32>> {
    if array.is_null(i) {
        return None;
    }
    let values = array.value(i);
    let float_arr = values.as_any().downcast_ref::<Float32Array>()?;
    Some(float_arr.iter().map(|v| v.unwrap_or(0.0)).collect())
}

/// Read a `List(Utf8)` cell and return a `Vec<String>`.
pub(super) fn read_string_list(array: &ListArray, i: usize) -> Vec<String> {
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
pub(super) fn read_nullable_string(array: &StringArray, i: usize) -> Option<String> {
    if array.is_null(i) {
        None
    } else {
        Some(array.value(i).to_string())
    }
}

/// Read a nullable Int64 column value.
pub(super) fn read_nullable_i64(array: &Int64Array, i: usize) -> Option<i64> {
    if array.is_null(i) {
        None
    } else {
        Some(array.value(i))
    }
}

/// Supported standard embedding dimensions.
pub(super) const STANDARD_DIMS: &[usize] = &[768, 1024, 1536];

/// Normalize a non-standard embedding to the nearest smaller supported dimension.
///
/// - If the dimension exactly matches a standard size, returns it unchanged.
/// - If larger than 768 but not a standard size, truncates + normalizes to
///   the nearest smaller standard dimension.
/// - If smaller than 768, returns `None` (too small to store usefully).
pub(super) fn normalize_embedding(embedding: &[f32]) -> Option<Vec<f32>> {
    let dim = embedding.len();

    // Exact match -- no normalization needed
    if STANDARD_DIMS.contains(&dim) {
        return Some(embedding.to_vec());
    }

    // Find the largest standard dim that is strictly smaller
    let target = STANDARD_DIMS
        .iter()
        .rev()
        .find(|&&d| d < dim)
        .copied();

    match target {
        Some(target_dim) => {
            warn!(
                original_dim = dim,
                target_dim,
                "Non-standard embedding dimension, truncating to nearest supported size"
            );
            Some(truncate_and_normalize(embedding.to_vec(), target_dim))
        }
        None => {
            warn!(dim, "Embedding dimension too small for any supported column, skipping storage");
            None
        }
    }
}
