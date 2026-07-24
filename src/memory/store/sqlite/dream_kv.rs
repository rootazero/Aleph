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
use serde::{Deserialize, Serialize};

use crate::error::AlephError;

use super::SqliteMemoryBackend;

/// A rejected distill edit, remembered across cycles.
///
/// Extends the original fingerprint-only buffer with human-readable context so
/// the *next* distill reflection can be told what was already tried and
/// rejected — SkillOpt's "rejected-edit buffer fed back into reflection". The
/// `fingerprint` alone still drives the O(1) code-level dedup drop; `target` /
/// `summary` / `reason` exist purely to render negative feedback into the LLM
/// prompt so it stops re-proposing the same losing edit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DistillRejectRecord {
    pub fingerprint: String,
    /// Path of the note the rejected edit targeted (e.g. `skill/foo`).
    #[serde(default)]
    pub target: String,
    /// Short human-readable summary (the proposed title).
    #[serde(default)]
    pub summary: String,
    /// Why the gate rejected it.
    #[serde(default)]
    pub reason: String,
}

fn key_for(consumer: &str, agent_id: &str) -> String {
    format!("dream_watermark__{consumer}__{agent_id}")
}

fn rejects_key_for(agent_id: &str) -> String {
    format!("distill_rejects__{agent_id}")
}

fn best_health_key_for(agent_id: &str) -> String {
    format!("dream_best_health__{agent_id}")
}

/// Cap on remembered rejected-edit fingerprints per agent (FIFO eviction).
/// Sized well above `skill_distill_max_per_cycle` × a few weeks of cycles.
const MAX_DISTILL_REJECTS: usize = 64;

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

    /// Full rejected-edit records for this agent, oldest first. Empty on first
    /// run or after a reset. Reads the rich `DistillRejectRecord` list, falling
    /// back to the legacy fingerprint-only `Vec<String>` format (mapped to
    /// records with empty context) so pre-upgrade buffers still load.
    pub fn distill_reject_records(
        &self,
        agent_id: &str,
    ) -> Result<Vec<DistillRejectRecord>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let key = rejects_key_for(agent_id);

        let mut stmt = conn
            .prepare("SELECT value FROM compression_metadata WHERE key = ?1")
            .map_err(|e| AlephError::config(format!("distill_rejects prepare: {e}")))?;

        match stmt.query_row(params![key], |row| row.get::<_, String>(0)) {
            Ok(value) => {
                // New rich format first; fall back to the legacy fingerprint list.
                if let Ok(records) = serde_json::from_str::<Vec<DistillRejectRecord>>(&value) {
                    Ok(records)
                } else if let Ok(fps) = serde_json::from_str::<Vec<String>>(&value) {
                    Ok(fps
                        .into_iter()
                        .map(|fingerprint| DistillRejectRecord {
                            fingerprint,
                            target: String::new(),
                            summary: String::new(),
                            reason: String::new(),
                        })
                        .collect())
                } else {
                    Err(AlephError::config(format!(
                        "distill_rejects parse {key}={value:?}"
                    )))
                }
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(Vec::new()),
            Err(e) => Err(AlephError::config(format!("distill_rejects: {e}"))),
        }
    }

    /// Remember a rejected distill edit so the same bad edit is dropped on later
    /// cycles without re-running the gate, *and* so its context can be replayed
    /// as negative feedback into the next distill prompt. Deduped by fingerprint;
    /// FIFO-capped at [`MAX_DISTILL_REJECTS`].
    pub fn record_distill_reject(
        &self,
        agent_id: &str,
        record: &DistillRejectRecord,
    ) -> Result<(), AlephError> {
        let mut rejects = self.distill_reject_records(agent_id)?;
        if rejects.iter().any(|r| r.fingerprint == record.fingerprint) {
            return Ok(());
        }
        rejects.push(record.clone());
        if rejects.len() > MAX_DISTILL_REJECTS {
            let overflow = rejects.len() - MAX_DISTILL_REJECTS;
            rejects.drain(..overflow);
        }

        let value = serde_json::to_string(&rejects)
            .map_err(|e| AlephError::config(format!("record_distill_reject encode: {e}")))?;
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "INSERT INTO compression_metadata (key, value) \
             VALUES (?1, ?2) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![rejects_key_for(agent_id), value],
        )
        .map_err(|e| AlephError::config(format!("record_distill_reject: {e}")))?;
        Ok(())
    }

    /// Read the best-ever memory-health score for this agent. Persisted across
    /// restarts so the evolution gate's best-checkpoint (SkillOpt's best-ever
    /// score) survives a reboot instead of resetting to 0 and re-accepting a
    /// worse-than-historical cycle as a "new best". `None` on first run.
    pub fn get_best_health(&self, agent_id: &str) -> Result<Option<f64>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let key = best_health_key_for(agent_id);

        let mut stmt = conn
            .prepare("SELECT value FROM compression_metadata WHERE key = ?1")
            .map_err(|e| AlephError::config(format!("get_best_health prepare: {e}")))?;

        match stmt.query_row(params![key], |row| row.get::<_, String>(0)) {
            Ok(value) => {
                let v = value.parse::<f64>().map_err(|e| {
                    AlephError::config(format!("get_best_health parse {key}={value:?}: {e}"))
                })?;
                Ok(Some(v))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(AlephError::config(format!("get_best_health: {e}"))),
        }
    }

    /// Persist the best-ever memory-health score for this agent. Idempotent
    /// upsert keyed by agent; only the monotonic best need be written.
    pub fn set_best_health(&self, agent_id: &str, value: f64) -> Result<(), AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "INSERT INTO compression_metadata (key, value) \
             VALUES (?1, ?2) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![best_health_key_for(agent_id), value.to_string()],
        )
        .map_err(|e| AlephError::config(format!("set_best_health: {e}")))?;
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

    fn fp_record(fp: &str) -> DistillRejectRecord {
        DistillRejectRecord {
            fingerprint: fp.to_string(),
            target: format!("skill/{fp}"),
            summary: format!("summary for {fp}"),
            reason: "recall-evidence gate".to_string(),
        }
    }

    /// The stored fingerprints, oldest first (dedup key for the gate).
    fn fps(backend: &SqliteMemoryBackend, agent: &str) -> Vec<String> {
        backend
            .distill_reject_records(agent)
            .unwrap()
            .into_iter()
            .map(|r| r.fingerprint)
            .collect()
    }

    #[test]
    fn distill_rejects_round_trip_dedupe_and_agent_isolation() {
        let backend = make_backend();

        // Empty store returns empty.
        assert!(fps(&backend, "main").is_empty());

        backend.record_distill_reject("main", &fp_record("fp_a")).unwrap();
        backend.record_distill_reject("main", &fp_record("fp_b")).unwrap();
        // Duplicate is a no-op.
        backend.record_distill_reject("main", &fp_record("fp_a")).unwrap();
        assert_eq!(
            fps(&backend, "main"),
            vec!["fp_a".to_string(), "fp_b".to_string()]
        );
        // Rich context survives the round-trip for prompt feedback.
        let records = backend.distill_reject_records("main").unwrap();
        assert_eq!(records[0].summary, "summary for fp_a");
        assert_eq!(records[0].target, "skill/fp_a");

        // Different agent is isolated.
        assert!(fps(&backend, "alice").is_empty());
    }

    #[test]
    fn distill_rejects_reads_legacy_fingerprint_only_format() {
        // A buffer written by the pre-upgrade code is a bare JSON string array.
        let backend = make_backend();
        {
            let conn = backend.conn.lock().unwrap_or_else(|e| e.into_inner());
            conn.execute(
                "INSERT INTO compression_metadata (key, value) VALUES (?1, ?2)",
                params![rejects_key_for("main"), r#"["legacy_a","legacy_b"]"#],
            )
            .unwrap();
        }
        let records = backend.distill_reject_records("main").unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].fingerprint, "legacy_a");
        assert!(records[0].summary.is_empty(), "legacy rows carry no context");
        assert_eq!(
            fps(&backend, "main"),
            vec!["legacy_a".to_string(), "legacy_b".to_string()]
        );
    }

    #[test]
    fn best_health_round_trips_and_is_agent_isolated() {
        let backend = make_backend();

        // Empty store returns None (first run → gate starts best at 0.0).
        assert_eq!(backend.get_best_health("main").unwrap(), None);

        backend.set_best_health("main", 0.73).unwrap();
        assert_eq!(backend.get_best_health("main").unwrap(), Some(0.73));

        // Idempotent overwrite keeps a single row.
        backend.set_best_health("main", 0.81).unwrap();
        assert_eq!(backend.get_best_health("main").unwrap(), Some(0.81));

        // Different agent is isolated.
        assert_eq!(backend.get_best_health("alice").unwrap(), None);
    }

    #[test]
    fn distill_rejects_evict_oldest_beyond_cap() {
        let backend = make_backend();
        for i in 0..(MAX_DISTILL_REJECTS + 3) {
            backend
                .record_distill_reject("main", &fp_record(&format!("fp_{i}")))
                .unwrap();
        }
        let rejects = fps(&backend, "main");
        assert_eq!(rejects.len(), MAX_DISTILL_REJECTS);
        // Oldest three evicted, newest retained.
        assert_eq!(rejects.first().unwrap(), "fp_3");
        assert_eq!(
            rejects.last().unwrap(),
            &format!("fp_{}", MAX_DISTILL_REJECTS + 2)
        );
    }
}
