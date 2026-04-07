//! Vector operation helpers for sqlite-vec.
//!
//! Provides extension registration, embedding serialization, and KNN search
//! against the `facts_vec_*` virtual tables.

use crate::error::AlephError;
use rusqlite::Connection;
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
        >(
            sqlite3_vec_init as *const (),
        )));
    }
}

/// Map an embedding dimension to the corresponding vec0 table name.
///
/// # Panics
///
/// Panics if `dim` is not one of 768, 1024, or 1536.
pub fn vec_table_for_dim(dim: u32) -> &'static str {
    match dim {
        768 => "facts_vec_768",
        1024 => "facts_vec_1024",
        1536 => "facts_vec_1536",
        _ => panic!("unsupported embedding dimension: {dim}"),
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

/// Insert an embedding vector into the appropriate vec0 table.
pub fn insert_vec(
    conn: &Connection,
    rowid: i64,
    embedding: &[f32],
    dim: u32,
) -> Result<(), AlephError> {
    let table = vec_table_for_dim(dim);
    let blob = embedding_to_blob(embedding);

    conn.execute(
        &format!("INSERT INTO {table}(rowid, embedding) VALUES (?1, ?2)"),
        rusqlite::params![rowid, blob],
    )
    .map_err(|e| AlephError::config(format!("Failed to insert vec into {table}: {e}")))?;

    Ok(())
}

/// Delete an embedding vector from a specific vec0 table.
pub fn delete_vec(conn: &Connection, rowid: i64, dim: u32) -> Result<(), AlephError> {
    let table = vec_table_for_dim(dim);

    conn.execute(
        &format!("DELETE FROM {table} WHERE rowid = ?1"),
        rusqlite::params![rowid],
    )
    .map_err(|e| AlephError::config(format!("Failed to delete vec from {table}: {e}")))?;

    Ok(())
}

/// Delete an embedding vector from all three vec0 tables.
///
/// Silently ignores tables where the rowid does not exist.
pub fn delete_vec_all_dims(conn: &Connection, rowid: i64) -> Result<(), AlephError> {
    for dim in [768, 1024, 1536] {
        delete_vec(conn, rowid, dim)?;
    }
    Ok(())
}

/// Perform a K-nearest-neighbor search against a vec0 table.
///
/// Returns `(rowid, distance)` pairs ordered by ascending distance.
pub fn knn_search(
    conn: &Connection,
    query_embedding: &[f32],
    dim: u32,
    k: usize,
) -> Result<Vec<(i64, f64)>, AlephError> {
    let table = vec_table_for_dim(dim);
    let blob = embedding_to_blob(query_embedding);

    let mut stmt = conn
        .prepare(&format!(
            "SELECT rowid, distance FROM {table} WHERE embedding MATCH ?1 ORDER BY distance LIMIT ?2"
        ))
        .map_err(|e| AlephError::config(format!("Failed to prepare KNN query on {table}: {e}")))?;

    let rows = stmt
        .query_map(rusqlite::params![blob, k as i64], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?))
        })
        .map_err(|e| AlephError::config(format!("KNN query failed on {table}: {e}")))?;

    let mut results = Vec::with_capacity(k);
    for row in rows {
        let pair =
            row.map_err(|e| AlephError::config(format!("Failed to read KNN row: {e}")))?;
        results.push(pair);
    }

    Ok(results)
}
