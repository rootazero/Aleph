//! `RecallSignalStore` — tracks which notes get retrieved during conversations.
//!
//! Each retrieval hit is recorded as a signal scoped to the recording
//! `agent_id` and keyed by (`note_path`, `query_hash`, `day_bucket`, channel)
//! for natural deduplication. The only live consumer of the
//! aggregate is `recall_hit_counts` (`signal_count` per note), which feeds
//! retrieval-time reinforcement so frequently-recalled notes float to the top
//! (hot-surfacing); the dream daemon's co-recall / hit-rate metrics read the same rows.


use chrono::Utc;
use rusqlite::params;
use sha2::{Digest, Sha256};

use crate::error::AlephError;

use super::SqliteMemoryBackend;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A single retrieval hit produced by the search pipeline.
#[derive(Debug, Clone)]
pub struct RecallHit {
    pub note_path: String,
    pub score: f64,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// SHA-256 first 16 hex chars of the trimmed, lowercased query.
#[must_use]
pub fn query_hash(query: &str) -> String {
    let normalized = query.trim().to_lowercase();
    let digest = Sha256::digest(normalized.as_bytes());
    hex::encode(&digest[..8]) // 8 bytes = 16 hex chars
}

/// Today's date as `YYYY-MM-DD`.
#[must_use]
pub fn today_bucket() -> String {
    Utc::now().format("%Y-%m-%d").to_string()
}

// ---------------------------------------------------------------------------
// RecallSignalStore impl on SqliteMemoryBackend
// ---------------------------------------------------------------------------

impl SqliteMemoryBackend {
    /// Record retrieval signals for a batch of hits, scoped to `agent_id`.
    ///
    /// Uses `INSERT OR IGNORE` on the dedup key
    /// (`agent_id`, `note_path`, `query_hash`, `day_bucket`, `channel`), so the
    /// same note/query/day/channel is recorded at most once per day per channel
    /// *per agent* — two agents recalling the same relative path stay distinct.
    ///
    /// Returns the number of newly inserted rows.
    pub fn record_signals(
        &self,
        query: &str,
        channel: &str,
        hits: &[RecallHit],
        session_id: Option<&str>,
        agent_id: &str,
        namespace: &str,
    ) -> Result<usize, AlephError> {
        if hits.is_empty() {
            return Ok(0);
        }

        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        let qhash = query_hash(query);
        let bucket = today_bucket();
        let now = Utc::now().timestamp();

        let sql = "INSERT OR IGNORE INTO recall_signals \
                   (id, note_path, agent_id, query_hash, query_text, channel, score, session_id, namespace, created_at, day_bucket) \
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)";

        let tx = conn
            .unchecked_transaction()
            .map_err(|e| AlephError::config(format!("record_signals transaction: {e}")))?;
        let inserted = {
            let mut stmt = tx
                .prepare_cached(sql)
                .map_err(|e| AlephError::config(format!("record_signals prepare: {e}")))?;

            let mut inserted = 0usize;
            for hit in hits {
                let id = uuid::Uuid::new_v4().to_string();
                let rows = stmt
                    .execute(params![
                        id,
                        hit.note_path,
                        agent_id,
                        qhash,
                        query,
                        channel,
                        hit.score,
                        session_id,
                        namespace,
                        now,
                        bucket,
                    ])
                    .map_err(|e| AlephError::config(format!("record_signals insert: {e}")))?;
                inserted += rows;
            }
            inserted
        };
        tx.commit()
            .map_err(|e| AlephError::config(format!("record_signals commit: {e}")))?;

        Ok(inserted)
    }

    /// Aggregate co-recall pairs: notes retrieved together by the same query
    /// event (same `query_hash` + `day_bucket` + `channel` — the dedup key of
    /// one retrieval). Returns `(note_a, note_b, co_hit_count)` with
    /// `note_a < note_b` (canonical undirected pair), strongest first, only
    /// pairs with at least `min_co_hits` distinct co-occurrences.
    ///
    /// Behavioral analog of codebase-memory-mcp's `FILE_CHANGES_WITH` edge,
    /// transposed to recall events. Pure aggregation — consumed by the
    /// `co_recall_edges` dream stage.
    pub fn co_recall_pairs(
        &self,
        agent_id: &str,
        min_co_hits: i64,
        limit: usize,
    ) -> Result<Vec<(String, String, i64)>, AlephError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        let mut stmt = conn
            .prepare(
                "SELECT a.note_path, b.note_path, COUNT(*) AS co_hits \
                 FROM recall_signals a \
                 JOIN recall_signals b \
                   ON a.query_hash = b.query_hash \
                  AND a.day_bucket = b.day_bucket \
                  AND a.channel    = b.channel \
                  AND a.note_path  < b.note_path \
                  AND a.agent_id   = ?3 \
                  AND b.agent_id   = ?3 \
                 GROUP BY a.note_path, b.note_path \
                 HAVING COUNT(*) >= ?1 \
                 ORDER BY co_hits DESC, a.note_path, b.note_path \
                 LIMIT ?2",
            )
            .map_err(|e| AlephError::config(format!("co_recall_pairs prepare: {e}")))?;

        let rows = stmt
            .query_map(params![min_co_hits, limit as i64, agent_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .map_err(|e| AlephError::config(format!("co_recall_pairs query: {e}")))?;

        let mut pairs = Vec::new();
        for row in rows {
            pairs.push(row.map_err(|e| AlephError::config(format!("co_recall_pairs row: {e}")))?);
        }
        Ok(pairs)
    }

    /// Count recall signals for a given channel.
    ///
    /// Returns the number of rows in `recall_signals` with the given channel.
    /// Exposed as a public helper primarily for integration tests.
    pub fn count_recall_signals_for_channel(&self, channel: &str) -> Result<u64, AlephError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM recall_signals WHERE channel = ?1",
                rusqlite::params![channel],
                |row: &rusqlite::Row| row.get::<_, i64>(0),
            )
            .map_err(|e| AlephError::config(format!("count_recall_signals_for_channel: {e}")))?;

        Ok(count as u64)
    }

    /// Delete recall signals older than `retention_days`.
    ///
    /// Returns the number of deleted rows.
    ///
    /// TODO: this cleanup is currently never invoked — `recall_signals` grows
    /// unbounded. Wiring it into a production schedule (dream cycle or flush)
    /// was deliberately deferred: the table is small today and the right
    /// retention cadence is a product call, not a wire.
    pub fn cleanup_old_signals(&self, retention_days: u32) -> Result<usize, AlephError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        let cutoff = Utc::now().timestamp() - (i64::from(retention_days) * 86400);

        let deleted = conn
            .execute(
                "DELETE FROM recall_signals WHERE created_at < ?1",
                params![cutoff],
            )
            .map_err(|e| AlephError::config(format!("cleanup_old_signals: {e}")))?;

        Ok(deleted)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn setup() -> SqliteMemoryBackend {
        let tmp = tempdir().unwrap();
        SqliteMemoryBackend::new(tmp.path()).unwrap()
    }

    #[test]
    fn dedup_same_query_same_day_same_channel() {
        let store = setup();
        let hits = vec![RecallHit {
            note_path: "f1".into(),
            score: 0.8,
        }];

        let first = store
            .record_signals("test query", "slack", &hits, None, "owner", "owner")
            .unwrap();
        assert_eq!(first, 1);

        // Same query, same channel, same day => dedup
        let second = store
            .record_signals("test query", "slack", &hits, None, "owner", "owner")
            .unwrap();
        assert_eq!(second, 0);
    }

    fn hit(path: &str) -> RecallHit {
        RecallHit {
            note_path: path.into(),
            score: 0.5,
        }
    }

    #[test]
    fn co_recall_pairs_counts_shared_query_events() {
        let store = setup();
        // Two queries both surface (a, b); one also surfaces c.
        store
            .record_signals(
                "q1",
                "web",
                &[hit("n/a"), hit("n/b"), hit("n/c")],
                None,
                "owner",
                "owner",
            )
            .unwrap();
        store
            .record_signals(
                "q2",
                "web",
                &[hit("n/a"), hit("n/b")],
                None,
                "owner",
                "owner",
            )
            .unwrap();

        let pairs = store.co_recall_pairs("owner", 2, 10).unwrap();
        // Only (a, b) reaches 2 co-hits; (a, c) and (b, c) have 1.
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0], ("n/a".to_string(), "n/b".to_string(), 2));
    }

    #[test]
    fn co_recall_pairs_are_canonical_and_threshold_gated() {
        let store = setup();
        store
            .record_signals(
                "q1",
                "web",
                &[hit("n/b"), hit("n/a")],
                None,
                "owner",
                "owner",
            )
            .unwrap();

        // Threshold 1 surfaces the single co-occurrence, canonically ordered.
        let pairs = store.co_recall_pairs("owner", 1, 10).unwrap();
        assert_eq!(pairs, vec![("n/a".to_string(), "n/b".to_string(), 1)]);
        // Threshold 2 filters it out.
        assert!(store.co_recall_pairs("owner", 2, 10).unwrap().is_empty());
    }

    #[test]
    fn co_recall_pairs_ignore_solo_recalls_and_cross_query_hits() {
        let store = setup();
        // Different queries each surfacing one note — never co-recalled.
        store
            .record_signals("q1", "web", &[hit("n/a")], None, "owner", "owner")
            .unwrap();
        store
            .record_signals("q2", "web", &[hit("n/b")], None, "owner", "owner")
            .unwrap();
        assert!(store.co_recall_pairs("owner", 1, 10).unwrap().is_empty());
    }
}
