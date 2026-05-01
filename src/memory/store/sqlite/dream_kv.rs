//! Dream-pipeline per-consumer watermark persistence.
//!
//! Stages that consume from `raw_memories` (or any append-only source) keep a
//! `(consumer_name, agent_id) -> last_processed_created_at` cursor so the
//! pipeline does not re-process the same rows on every cycle.
//!
//! Backed by the existing `compression_metadata` (key, value) table —
//! namespaced under the `dream_watermark__` key prefix to avoid clashing with
//! `CompressionStore`'s own keys (`last_timestamp`, `session_*`).

use rusqlite::params;

use crate::error::AlephError;

use super::SqliteMemoryBackend;

fn key_for(consumer: &str, agent_id: &str) -> String {
    format!("dream_watermark__{consumer}__{agent_id}")
}

impl SqliteMemoryBackend {
    /// Read the watermark (`max(created_at)` last successfully processed) for
    /// the given consumer/agent pair. Returns `None` on first run or after a
    /// reset.
    pub fn get_dream_watermark(
        &self,
        consumer: &str,
        agent_id: &str,
    ) -> Result<Option<i64>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let key = key_for(consumer, agent_id);

        let mut stmt = conn
            .prepare("SELECT value FROM compression_metadata WHERE key = ?1")
            .map_err(|e| AlephError::config(format!("get_dream_watermark prepare: {e}")))?;

        let result = stmt.query_row(params![key], |row| row.get::<_, String>(0));

        match result {
            Ok(value) => {
                let ts = value.parse::<i64>().map_err(|e| {
                    AlephError::config(format!("get_dream_watermark parse {key}={value:?}: {e}"))
                })?;
                Ok(Some(ts))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(AlephError::config(format!("get_dream_watermark: {e}"))),
        }
    }

    /// Persist the watermark for the given consumer/agent pair. Idempotent —
    /// `INSERT OR REPLACE` keeps a single row per `(consumer, agent_id)`.
    pub fn set_dream_watermark(
        &self,
        consumer: &str,
        agent_id: &str,
        watermark: i64,
    ) -> Result<(), AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let key = key_for(consumer, agent_id);

        conn.execute(
            "INSERT INTO compression_metadata (key, value) \
             VALUES (?1, ?2) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, watermark.to_string()],
        )
        .map_err(|e| AlephError::config(format!("set_dream_watermark: {e}")))?;
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
    fn watermark_round_trips_per_agent_and_consumer() {
        let backend = make_backend();

        // Empty store returns None.
        assert_eq!(
            backend
                .get_dream_watermark("feedback_distill", "main")
                .unwrap(),
            None
        );

        // Write + read.
        backend
            .set_dream_watermark("feedback_distill", "main", 12345)
            .unwrap();
        assert_eq!(
            backend
                .get_dream_watermark("feedback_distill", "main")
                .unwrap(),
            Some(12345)
        );

        // Different consumer is isolated.
        assert_eq!(
            backend
                .get_dream_watermark("other_distill", "main")
                .unwrap(),
            None
        );

        // Different agent is isolated.
        assert_eq!(
            backend
                .get_dream_watermark("feedback_distill", "alice")
                .unwrap(),
            None
        );
    }

    #[test]
    fn watermark_overwrite_is_idempotent() {
        let backend = make_backend();
        backend
            .set_dream_watermark("feedback_distill", "main", 100)
            .unwrap();
        backend
            .set_dream_watermark("feedback_distill", "main", 200)
            .unwrap();
        backend
            .set_dream_watermark("feedback_distill", "main", 200)
            .unwrap();
        assert_eq!(
            backend
                .get_dream_watermark("feedback_distill", "main")
                .unwrap(),
            Some(200)
        );
    }
}
