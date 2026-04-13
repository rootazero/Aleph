//! RecallSignalStore — tracks which notes get retrieved during conversations.
//!
//! Each retrieval hit is recorded as a signal keyed by (note_path, query_hash,
//! day_bucket, channel) for natural deduplication.  Aggregated signals later
//! feed the 8-dimensional promotion scorer that decides which short-term
//! memories should be promoted to long-term.

use chrono::Utc;
use rusqlite::params;
use sha2::{Digest, Sha256};

use crate::error::AlephError;

use super::SqliteMemoryBackend;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Aggregated recall signals for a single note.
#[derive(Debug, Clone)]
pub struct RecallAggregate {
    pub note_path: String,
    pub signal_count: i64,
    pub total_score: f64,
    pub unique_queries: i64,
    pub unique_channels: i64,
    pub recall_days: i64,
    pub first_recall: i64,
    pub last_recall: i64,
}

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
pub fn query_hash(query: &str) -> String {
    let normalized = query.trim().to_lowercase();
    let digest = Sha256::digest(normalized.as_bytes());
    hex::encode(&digest[..8]) // 8 bytes = 16 hex chars
}

/// Today's date as `YYYY-MM-DD`.
pub fn today_bucket() -> String {
    Utc::now().format("%Y-%m-%d").to_string()
}

// ---------------------------------------------------------------------------
// RecallSignalStore impl on SqliteMemoryBackend
// ---------------------------------------------------------------------------

impl SqliteMemoryBackend {
    /// Record retrieval signals for a batch of hits.
    ///
    /// Uses `INSERT OR IGNORE` so the same (fact_id, query_hash, day_bucket,
    /// channel) combination is recorded at most once per day per channel.
    ///
    /// Returns the number of newly inserted rows.
    pub fn record_signals(
        &self,
        query: &str,
        channel: &str,
        hits: &[RecallHit],
        session_id: Option<&str>,
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
                   (id, note_path, query_hash, query_text, channel, score, session_id, namespace, created_at, day_bucket) \
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)";

        let mut stmt = conn
            .prepare_cached(sql)
            .map_err(|e| AlephError::config(format!("record_signals prepare: {e}")))?;

        let mut inserted = 0usize;
        for hit in hits {
            let id = uuid::Uuid::new_v4().to_string();
            let rows = stmt
                .execute(params![
                    id,
                    hit.note_path,
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

        Ok(inserted)
    }

    /// Aggregate recall signals for a set of fact IDs.
    ///
    /// Returns one `RecallAggregate` per fact that has at least one signal.
    /// SQLite max bound parameters per statement.
    const SQLITE_MAX_VARS: usize = 999;

    pub fn aggregate_for_facts(
        &self,
        note_paths: &[String],
    ) -> Result<Vec<RecallAggregate>, AlephError> {
        if note_paths.is_empty() {
            return Ok(Vec::new());
        }

        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        let mut results = Vec::new();

        // Chunk to stay under SQLite's 999-variable limit
        for chunk in note_paths.chunks(Self::SQLITE_MAX_VARS) {
            let placeholders: Vec<String> = (1..=chunk.len()).map(|i| format!("?{i}")).collect();
            let sql = format!(
                "SELECT \
                     note_path, \
                     COUNT(*)                    AS signal_count, \
                     SUM(score)                  AS total_score, \
                     COUNT(DISTINCT query_hash)  AS unique_queries, \
                     COUNT(DISTINCT channel)     AS unique_channels, \
                     COUNT(DISTINCT day_bucket)  AS recall_days, \
                     MIN(created_at)             AS first_recall, \
                     MAX(created_at)             AS last_recall \
                 FROM recall_signals \
                 WHERE note_path IN ({}) \
                 GROUP BY note_path",
                placeholders.join(", ")
            );

            let params: Vec<Box<dyn rusqlite::types::ToSql>> = chunk
                .iter()
                .map(|id| Box::new(id.clone()) as Box<dyn rusqlite::types::ToSql>)
                .collect();
            let param_refs: Vec<&dyn rusqlite::types::ToSql> =
                params.iter().map(|p| p.as_ref()).collect();

            let mut stmt = conn
                .prepare(&sql)
                .map_err(|e| AlephError::config(format!("aggregate_for_facts prepare: {e}")))?;

            let rows = stmt
                .query_map(param_refs.as_slice(), |row| {
                    Ok(RecallAggregate {
                        note_path: row.get("note_path")?,
                        signal_count: row.get("signal_count")?,
                        total_score: row.get("total_score")?,
                        unique_queries: row.get("unique_queries")?,
                        unique_channels: row.get("unique_channels")?,
                        recall_days: row.get("recall_days")?,
                        first_recall: row.get("first_recall")?,
                        last_recall: row.get("last_recall")?,
                    })
                })
                .map_err(|e| AlephError::config(format!("aggregate_for_facts query: {e}")))?;

            for row in rows {
                results.push(
                    row.map_err(|e| AlephError::config(format!("aggregate_for_facts row: {e}")))?,
                );
            }
        }

        Ok(results)
    }

    /// Delete recall signals older than `retention_days`.
    ///
    /// Returns the number of deleted rows.
    pub fn cleanup_old_signals(&self, retention_days: u32) -> Result<usize, AlephError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        let cutoff = Utc::now().timestamp() - (retention_days as i64 * 86400);

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
    fn record_and_aggregate_signals() {
        let store = setup();
        let hits = vec![
            RecallHit {
                note_path: "f1".into(),
                score: 0.9,
            },
            RecallHit {
                note_path: "f2".into(),
                score: 0.7,
            },
        ];

        let inserted = store
            .record_signals("hello world", "slack", &hits, Some("s1"), "owner")
            .unwrap();
        assert_eq!(inserted, 2);

        let agg = store
            .aggregate_for_facts(&["f1".into(), "f2".into()])
            .unwrap();
        assert_eq!(agg.len(), 2);

        let f1 = agg.iter().find(|a| a.note_path == "f1").unwrap();
        assert_eq!(f1.signal_count, 1);
        assert!((f1.total_score - 0.9).abs() < f64::EPSILON);
        assert_eq!(f1.unique_queries, 1);
        assert_eq!(f1.unique_channels, 1);
        assert_eq!(f1.recall_days, 1);
    }

    #[test]
    fn dedup_same_query_same_day_same_channel() {
        let store = setup();
        let hits = vec![RecallHit {
            note_path: "f1".into(),
            score: 0.8,
        }];

        let first = store
            .record_signals("test query", "slack", &hits, None, "owner")
            .unwrap();
        assert_eq!(first, 1);

        // Same query, same channel, same day => dedup
        let second = store
            .record_signals("test query", "slack", &hits, None, "owner")
            .unwrap();
        assert_eq!(second, 0);
    }

    #[test]
    fn different_channels_count_separately() {
        let store = setup();
        let hits = vec![RecallHit {
            note_path: "f1".into(),
            score: 0.5,
        }];

        store
            .record_signals("q", "slack", &hits, None, "owner")
            .unwrap();
        store
            .record_signals("q", "web", &hits, None, "owner")
            .unwrap();

        let agg = store.aggregate_for_facts(&["f1".into()]).unwrap();
        assert_eq!(agg.len(), 1);
        assert_eq!(agg[0].signal_count, 2);
        assert_eq!(agg[0].unique_channels, 2);
    }

    #[test]
    fn cleanup_removes_old_signals() {
        let store = setup();
        let hits = vec![RecallHit {
            note_path: "f1".into(),
            score: 0.6,
        }];

        store
            .record_signals("q", "slack", &hits, None, "owner")
            .unwrap();

        // retention_days=0 means cutoff = now, so all signals created at now are < now+1
        // but created_at == now, so cutoff == now means created_at < now is false.
        // Use a trick: signals just inserted have created_at = now, so retention_days=0
        // gives cutoff = now. created_at < now is false for same-second inserts.
        // We need to verify cleanup works, so we pass retention_days=0 which sets
        // cutoff to now. Signals created "at now" are not strictly less than now,
        // so let's verify with a direct check and then force by using the aggregate.

        // First verify signal exists
        let agg = store.aggregate_for_facts(&["f1".into()]).unwrap();
        assert_eq!(agg.len(), 1);

        // retention_days=0 means cutoff = now. Signals created at exactly now
        // won't be deleted (created_at < cutoff is false for same-second).
        // But that's correct behavior. Let's manually insert an old signal instead.
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO recall_signals (id, note_path, query_hash, query_text, channel, score, namespace, created_at, day_bucket) \
                 VALUES ('old1', 'f2', 'hash', 'old', 'slack', 0.5, 'owner', 1000, '2020-01-01')",
                [],
            ).unwrap();
        }

        let agg2 = store.aggregate_for_facts(&["f2".into()]).unwrap();
        assert_eq!(agg2.len(), 1);

        // Now cleanup with retention_days=0 => cutoff = now, old signal (created_at=1000) is deleted
        let deleted = store.cleanup_old_signals(0).unwrap();
        assert!(deleted >= 1);

        let agg3 = store.aggregate_for_facts(&["f2".into()]).unwrap();
        assert!(agg3.is_empty());
    }

    #[test]
    fn aggregate_empty_ids_returns_empty() {
        let store = setup();
        let agg = store.aggregate_for_facts(&[]).unwrap();
        assert!(agg.is_empty());
    }
}
