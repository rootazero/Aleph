//! Compare-and-set storage for the corpus-local feedback batch.
//! The caller owns the versioned payload and its state-machine validation.

use rusqlite::{params, OptionalExtension};

use super::SqliteMemoryBackend;
use crate::error::AlephError;

fn batch_key(corpus: &str) -> String {
    format!("dream_feedback_batch__{corpus}")
}

impl SqliteMemoryBackend {
    pub(crate) fn feedback_batch_record(&self, corpus: &str) -> Result<Option<String>, AlephError> {
        self.with_connection(|conn| {
            conn.query_row(
                "SELECT value FROM compression_metadata WHERE key = ?1",
                [batch_key(corpus)],
                |row| row.get(0),
            )
            .optional()
            .map_err(|err| AlephError::other(format!("read feedback batch state: {err}")))
        })
    }

    pub(crate) fn compare_exchange_feedback_batch(
        &self,
        corpus: &str,
        previous: Option<&str>,
        next: &str,
    ) -> Result<bool, AlephError> {
        self.with_connection(|conn| {
            let key = batch_key(corpus);
            let changed = match previous {
                Some(expected) => conn.execute(
                    "UPDATE compression_metadata SET value = ?1 WHERE key = ?2 AND value = ?3",
                    params![next, key, expected],
                ),
                None => conn.execute(
                    "INSERT INTO compression_metadata (key, value) VALUES (?1, ?2)
                     ON CONFLICT(key) DO NOTHING",
                    params![key, next],
                ),
            }
            .map_err(|err| AlephError::other(format!("persist feedback batch state: {err}")))?;
            Ok(changed == 1)
        })
    }
}
