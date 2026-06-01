//! Embedding vector-space signature persistence.
//!
//! Records `provider:model:dim` of the model that produced the current note
//! vectors (written after each successful `reembed_all`), so a later
//! provider/model switch can be detected and a reembed recommended. See
//! [`crate::memory::embedding_signature`].
//!
//! Reuses the generic `compression_metadata` (key, value) table under a fixed
//! key — the signature is global (the whole store shares one vector space), so
//! no agent namespacing is needed. Same pattern as [`super::dream_kv`].

use rusqlite::params;

use crate::error::AlephError;

use super::SqliteMemoryBackend;

const EMBEDDING_SIGNATURE_KEY: &str = "embedding_signature";

impl SqliteMemoryBackend {
    /// Read the recorded embedding signature, or `None` if a reembed has never
    /// run (fresh store or data predating this feature).
    pub fn get_embedding_signature(&self) -> Result<Option<String>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn
            .prepare("SELECT value FROM compression_metadata WHERE key = ?1")
            .map_err(|e| AlephError::config(format!("get_embedding_signature prepare: {e}")))?;

        match stmt.query_row(params![EMBEDDING_SIGNATURE_KEY], |row| row.get::<_, String>(0)) {
            Ok(value) => Ok(Some(value)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(AlephError::config(format!("get_embedding_signature: {e}"))),
        }
    }

    /// Persist the embedding signature. Idempotent — a single row is kept.
    pub fn set_embedding_signature(&self, signature: &str) -> Result<(), AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "INSERT INTO compression_metadata (key, value) \
             VALUES (?1, ?2) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![EMBEDDING_SIGNATURE_KEY, signature],
        )
        .map_err(|e| AlephError::config(format!("set_embedding_signature: {e}")))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_backend() -> SqliteMemoryBackend {
        SqliteMemoryBackend::in_memory().expect("in-memory backend")
    }

    #[test]
    fn signature_round_trips_and_overwrites() {
        let backend = make_backend();

        // Fresh store has no signature.
        assert_eq!(backend.get_embedding_signature().unwrap(), None);

        // Write + read.
        backend
            .set_embedding_signature("openai:text-embedding-3-small:1536")
            .unwrap();
        assert_eq!(
            backend.get_embedding_signature().unwrap().as_deref(),
            Some("openai:text-embedding-3-small:1536")
        );

        // Overwrite keeps a single row.
        backend
            .set_embedding_signature("siliconflow:BAAI/bge-m3:1024")
            .unwrap();
        assert_eq!(
            backend.get_embedding_signature().unwrap().as_deref(),
            Some("siliconflow:BAAI/bge-m3:1024")
        );
    }
}
