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

/// Map an embedding dimension to the corresponding notes vec0 table name.
///
/// Returns an error if `dim` is not one of 768, 1024, or 1536.
pub fn notes_vec_table_for_dim(dim: u32) -> Result<&'static str, AlephError> {
    match dim {
        768 => Ok("notes_vec_768"),
        1024 => Ok("notes_vec_1024"),
        1536 => Ok("notes_vec_1536"),
        _ => Err(AlephError::config(format!(
            "unsupported embedding dimension: {dim} (expected 768, 1024, or 1536)"
        ))),
    }
}

/// Serialize a float embedding to a little-endian byte blob for sqlite-vec.
pub fn embedding_to_blob(embedding: &[f32]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(embedding.len() * 4);
    for &val in embedding {
        buf.extend_from_slice(&val.to_le_bytes());
    }
    buf
}
