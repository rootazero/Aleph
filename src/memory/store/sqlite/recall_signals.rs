//! `RecallSignalStore` — tracks which notes get retrieved during conversations.
//!
//! Each retrieval hit is recorded as a signal scoped to the recording
//! `agent_id` and keyed by (`note_path`, `query_hash`, `day_bucket`, channel)
//! for natural deduplication. The only live consumer of the
//! aggregate is `recall_hit_counts` (`signal_count` per note), which feeds
//! retrieval-time reinforcement so frequently-recalled notes float to the top
//! (hot-surfacing); the dream daemon's co-recall / hit-rate metrics read the same rows.
//! (The other `RecallAggregate` fields are currently unconsumed — see the struct.)

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

    /// Aggregate recall signals for a set of fact IDs.
    ///
    /// Returns one `RecallAggregate` per fact that has at least one signal.
    /// `SQLite` max bound parameters per statement.
    const SQLITE_MAX_VARS: usize = 999;

    pub fn aggregate_for_facts(
        &self,
        agent_id: &str,
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

        // Chunk to stay under SQLite's 999-variable limit — reserve one slot
        // for the trailing `agent_id` bind appended to each chunk.
        for chunk in note_paths.chunks(Self::SQLITE_MAX_VARS - 1) {
            let placeholders: Vec<String> = (1..=chunk.len()).map(|i| format!("?{i}")).collect();
            let agent_ph = format!("?{}", chunk.len() + 1);
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
                 WHERE note_path IN ({}) AND agent_id = {agent_ph} \
                 GROUP BY note_path",
                placeholders.join(", ")
            );

            let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = chunk
                .iter()
                .map(|id| Box::new(id.clone()) as Box<dyn rusqlite::types::ToSql>)
                .collect();
            params.push(Box::new(agent_id.to_string()) as Box<dyn rusqlite::types::ToSql>);
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
            .record_signals("hello world", "slack", &hits, Some("s1"), "owner", "owner")
            .unwrap();
        assert_eq!(inserted, 2);

        let agg = store
            .aggregate_for_facts("owner", &["f1".into(), "f2".into()])
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
            .record_signals("test query", "slack", &hits, None, "owner", "owner")
            .unwrap();
        assert_eq!(first, 1);

        // Same query, same channel, same day => dedup
        let second = store
            .record_signals("test query", "slack", &hits, None, "owner", "owner")
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
            .record_signals("q", "slack", &hits, None, "owner", "owner")
            .unwrap();
        store
            .record_signals("q", "web", &hits, None, "owner", "owner")
            .unwrap();

        let agg = store.aggregate_for_facts("owner", &["f1".into()]).unwrap();
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
            .record_signals("q", "slack", &hits, None, "owner", "owner")
            .unwrap();

        // retention_days=0 means cutoff = now, so all signals created at now are < now+1
        // but created_at == now, so cutoff == now means created_at < now is false.
        // Use a trick: signals just inserted have created_at = now, so retention_days=0
        // gives cutoff = now. created_at < now is false for same-second inserts.
        // We need to verify cleanup works, so we pass retention_days=0 which sets
        // cutoff to now. Signals created "at now" are not strictly less than now,
        // so let's verify with a direct check and then force by using the aggregate.

        // First verify signal exists
        let agg = store.aggregate_for_facts("owner", &["f1".into()]).unwrap();
        assert_eq!(agg.len(), 1);

        // retention_days=0 means cutoff = now. Signals created at exactly now
        // won't be deleted (created_at < cutoff is false for same-second).
        // But that's correct behavior. Let's manually insert an old signal instead.
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO recall_signals (id, note_path, agent_id, query_hash, query_text, channel, score, namespace, created_at, day_bucket) \
                 VALUES ('old1', 'f2', 'owner', 'hash', 'old', 'slack', 0.5, 'owner', 1000, '2020-01-01')",
                [],
            ).unwrap();
        }

        let agg2 = store.aggregate_for_facts("owner", &["f2".into()]).unwrap();
        assert_eq!(agg2.len(), 1);

        // Now cleanup with retention_days=0 => cutoff = now, old signal (created_at=1000) is deleted
        let deleted = store.cleanup_old_signals(0).unwrap();
        assert!(deleted >= 1);

        let agg3 = store.aggregate_for_facts("owner", &["f2".into()]).unwrap();
        assert!(agg3.is_empty());
    }

    #[test]
    fn aggregate_empty_ids_returns_empty() {
        let store = setup();
        let agg = store.aggregate_for_facts("owner", &[]).unwrap();
        assert!(agg.is_empty());
    }

    #[test]
    fn signals_are_scoped_per_agent() {
        let store = setup();
        // Two agents each record a recall for the SAME relative note_path,
        // same query/day/channel — the dedup key includes agent_id, so both
        // rows survive instead of the second being ignored.
        store
            .record_signals(
                "q",
                "slack",
                &[hit("skill/shared")],
                None,
                "agent-a",
                "owner",
            )
            .unwrap();
        store
            .record_signals(
                "q",
                "slack",
                &[hit("skill/shared")],
                None,
                "agent-b",
                "owner",
            )
            .unwrap();

        // Each agent sees only its own signal — no cross-agent pollution.
        let a = store
            .aggregate_for_facts("agent-a", &["skill/shared".into()])
            .unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].signal_count, 1);

        let b = store
            .aggregate_for_facts("agent-b", &["skill/shared".into()])
            .unwrap();
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].signal_count, 1);

        // An agent with no signals of its own sees nothing for that path.
        let c = store
            .aggregate_for_facts("agent-c", &["skill/shared".into()])
            .unwrap();
        assert!(c.is_empty());
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
