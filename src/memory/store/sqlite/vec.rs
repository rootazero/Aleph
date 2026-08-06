//! Vector operation helpers for sqlite-vec.
//!
//! Provides extension registration, embedding serialization, and dimension-to-
//! table mapping for the `notes_vec_*` virtual tables. (Legacy `facts_vec_*`
//! helpers were removed alongside the facts table.)

use crate::error::AlephError;
use sqlite_vec::sqlite3_vec_init;

/// Register the sqlite-vec extension for all future connections.
///
/// Must be called **before** opening any `Connection` that needs vec0 tables.
///
/// # Safety
///
/// Uses `sqlite3_auto_extension` with the C entrypoint from the sqlite-vec
/// crate. This follows the same pattern as `StateDatabase::register_sqlite_vec_extension`.
pub fn register_sqlite_vec() {
    // SAFETY: sqlite3_vec_init is the C entrypoint for the extension.
    // sqlite3_auto_extension registers it to be loaded for all new connections.
    // rust-doctor-disable-next-line unsafe-block-audit
    unsafe {
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute::<
            *const (),
            unsafe extern "C" fn(
                *mut rusqlite::ffi::sqlite3,
                *mut *mut std::ffi::c_char,
                *const rusqlite::ffi::sqlite3_api_routines,
            ) -> std::ffi::c_int,
        >(sqlite3_vec_init as *const ())));
    }
}

/// Every embedding dimension the vector index can store, paired with the vec0
/// table names that hold it.
///
/// **Single source of truth.** Table creation (`schema::init_*_vec_tables`),
/// dimension lookup, and the delete-path sweep all derive from this one array;
/// adding a dimension is a one-line change here. It was previously spelled out
/// in five places, which is how a deployment could end up with a table set that
/// silently disagreed with what the lookup accepted.
///
/// 384 covers `all-MiniLM-L6-v2` (the most common local embedding model) and
/// 3072 covers `text-embedding-3-large`. Before they were listed here, either
/// one produced a deployment with **no note vectors at all**: every
/// `upsert_embedding` failed into a swallowed `warn!` on the write side, and
/// every hybrid query failed on the read side.
///
/// The names are compile-time constants, so interpolating them into SQL stays
/// injection-safe.
pub const EMBEDDING_DIM_TABLES: &[(u32, &str, &str)] = &[
    (384, "notes_vec_384", "routing_exp_vec_384"),
    (768, "notes_vec_768", "routing_exp_vec_768"),
    (1024, "notes_vec_1024", "routing_exp_vec_1024"),
    (1536, "notes_vec_1536", "routing_exp_vec_1536"),
    (3072, "notes_vec_3072", "routing_exp_vec_3072"),
];

/// Every notes vec0 table, one per supported embedding dimension. Used by
/// delete paths that must clear an embedding without knowing its dimension.
///
/// Derived from [`EMBEDDING_DIM_TABLES`] rather than restated, so a delete
/// sweep can never miss a table that the write path is allowed to insert into.
pub fn all_notes_vec_tables() -> impl Iterator<Item = &'static str> {
    EMBEDDING_DIM_TABLES.iter().map(|(_, notes, _)| *notes)
}

/// Human-readable list of supported dimensions, for error messages.
fn supported_dims_label() -> String {
    EMBEDDING_DIM_TABLES
        .iter()
        .map(|(d, ..)| d.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Map an embedding dimension to the corresponding notes vec0 table name.
///
/// Returns an error for a dimension outside [`EMBEDDING_DIM_TABLES`].
pub fn notes_vec_table_for_dim(dim: u32) -> Result<&'static str, AlephError> {
    EMBEDDING_DIM_TABLES
        .iter()
        .find(|(d, ..)| *d == dim)
        .map(|(_, notes, _)| *notes)
        .ok_or_else(|| {
            AlephError::config(format!(
                "unsupported embedding dimension: {dim} (expected one of {})",
                supported_dims_label()
            ))
        })
}

/// Map an embedding dimension to the corresponding routing experience vec0 table name.
///
/// Returns an error for a dimension outside [`EMBEDDING_DIM_TABLES`].
pub fn routing_exp_vec_table_for_dim(dim: u32) -> Result<&'static str, AlephError> {
    EMBEDDING_DIM_TABLES
        .iter()
        .find(|(d, ..)| *d == dim)
        .map(|(.., routing)| *routing)
        .ok_or_else(|| {
            AlephError::config(format!(
                "unsupported embedding dimension: {dim} (expected one of {})",
                supported_dims_label()
            ))
        })
}

/// Serialize a float embedding to a little-endian byte blob for sqlite-vec.
#[must_use]
pub fn embedding_to_blob(embedding: &[f32]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(embedding.len() * 4);
    for &val in embedding {
        buf.extend_from_slice(&val.to_le_bytes());
    }
    buf
}
