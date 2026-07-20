//! `DreamReportStore` — audit trail for dream pipeline executions.
//!
//! Each dream pipeline run (consolidation, decay, promotion, synthesis)
//! produces a report that is persisted here for observability and debugging.

use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::error::AlephError;

use super::SqliteMemoryBackend;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A persisted dream pipeline execution report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedDreamReport {
    pub id: String,
    pub pipeline_type: String,
    pub started_at: i64,
    pub finished_at: i64,
    pub duration_ms: i64,
    pub synthesis_count: u32,
    pub errors: Option<String>,
    pub namespace: String,
}

/// One `GROUP BY pipeline_type` bucket of dream activity within a time window.
/// The governance audit loop reads this in-core instead of shelling out to
/// `sqlite3` — same numbers the old `SELECT pipeline_type, count(*),
/// sum(synthesis_count) ... GROUP BY pipeline_type` probe produced, but without
/// punching a hole in the workspace sandbox for `~/.aleph/data`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DreamPipelineStat {
    /// Pipeline that ran (e.g. `full`, `consolidation`, `decay`).
    pub pipeline_type: String,
    /// Number of runs of this pipeline in the window.
    pub runs: u64,
    /// Total memories synthesised across those runs (`sum(synthesis_count)`).
    pub synthesis_sum: i64,
}

// ---------------------------------------------------------------------------
// DreamReportStore impl on SqliteMemoryBackend
// ---------------------------------------------------------------------------

impl SqliteMemoryBackend {
    /// Insert a dream pipeline report into the audit log.
    pub fn insert_dream_report(&self, report: &PersistedDreamReport) -> Result<(), AlephError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        conn.execute(
            "INSERT INTO dream_reports \
             (id, pipeline_type, started_at, finished_at, duration_ms, \
              synthesis_count, errors, namespace) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                report.id,
                report.pipeline_type,
                report.started_at,
                report.finished_at,
                report.duration_ms,
                report.synthesis_count,
                report.errors,
                report.namespace,
            ],
        )
        .map_err(|e| AlephError::config(format!("insert_dream_report: {e}")))?;

        Ok(())
    }

    /// Query recent dream reports, ordered by `started_at` descending.
    pub fn recent_dream_reports(
        &self,
        limit: usize,
    ) -> Result<Vec<PersistedDreamReport>, AlephError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        let mut stmt = conn
            .prepare(
                "SELECT id, pipeline_type, started_at, finished_at, duration_ms, \
                 synthesis_count, errors, namespace \
                 FROM dream_reports ORDER BY started_at DESC LIMIT ?1",
            )
            .map_err(|e| AlephError::config(format!("recent_dream_reports prepare: {e}")))?;

        let rows = stmt
            .query_map(params![limit as i64], |row| {
                Ok(PersistedDreamReport {
                    id: row.get("id")?,
                    pipeline_type: row.get("pipeline_type")?,
                    started_at: row.get("started_at")?,
                    finished_at: row.get("finished_at")?,
                    duration_ms: row.get("duration_ms")?,
                    synthesis_count: row.get("synthesis_count")?,
                    errors: row.get("errors")?,
                    namespace: row.get("namespace")?,
                })
            })
            .map_err(|e| AlephError::config(format!("recent_dream_reports query: {e}")))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(
                row.map_err(|e| AlephError::config(format!("recent_dream_reports row: {e}")))?,
            );
        }
        Ok(results)
    }

    /// Dream activity grouped by `pipeline_type` for runs started after
    /// `since_started_at` (inclusive-exclusive: `started_at > since`). Powers the
    /// governance audit's "dreaming 近N天" reality probe. `started_at` is in
    /// **epoch seconds** (memory.db convention). Buckets are ordered by
    /// `pipeline_type` for deterministic output.
    pub fn dream_report_distribution_since(
        &self,
        since_started_at: i64,
    ) -> Result<Vec<DreamPipelineStat>, AlephError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        let mut stmt = conn
            .prepare(
                "SELECT pipeline_type, count(*) AS runs, \
                 COALESCE(sum(synthesis_count), 0) AS synthesis_sum \
                 FROM dream_reports \
                 WHERE started_at > ?1 \
                 GROUP BY pipeline_type \
                 ORDER BY pipeline_type ASC",
            )
            .map_err(|e| {
                AlephError::config(format!("dream_report_distribution_since prepare: {e}"))
            })?;

        let rows = stmt
            .query_map(params![since_started_at], |row| {
                Ok(DreamPipelineStat {
                    pipeline_type: row.get("pipeline_type")?,
                    runs: row.get::<_, i64>("runs")?.max(0) as u64,
                    synthesis_sum: row.get("synthesis_sum")?,
                })
            })
            .map_err(|e| {
                AlephError::config(format!("dream_report_distribution_since query: {e}"))
            })?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| {
                AlephError::config(format!("dream_report_distribution_since row: {e}"))
            })?);
        }
        Ok(results)
    }

    /// Return the most recent `finished_at` timestamp, or `None` if no reports exist.
    pub fn latest_dream_report_ts(&self) -> Result<Option<i64>, AlephError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        let ts: Option<i64> = conn
            .query_row("SELECT MAX(finished_at) FROM dream_reports", [], |row| {
                row.get(0)
            })
            .map_err(|e| AlephError::config(format!("latest_dream_report_ts: {e}")))?;

        Ok(ts)
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

    fn sample_report(id: &str, started: i64, finished: i64) -> PersistedDreamReport {
        PersistedDreamReport {
            id: id.to_string(),
            pipeline_type: "full".to_string(),
            started_at: started,
            finished_at: finished,
            duration_ms: finished - started,
            synthesis_count: 1,
            errors: None,
            namespace: "owner".to_string(),
        }
    }

    #[test]
    fn insert_and_query_report() {
        let store = setup();
        let report = sample_report("r1", 1000, 2000);

        store.insert_dream_report(&report).unwrap();

        let reports = store.recent_dream_reports(10).unwrap();
        assert_eq!(reports.len(), 1);

        let r = &reports[0];
        assert_eq!(r.id, "r1");
        assert_eq!(r.pipeline_type, "full");
        assert_eq!(r.started_at, 1000);
        assert_eq!(r.finished_at, 2000);
        assert_eq!(r.duration_ms, 1000);
        assert_eq!(r.synthesis_count, 1);
        assert!(r.errors.is_none());
        assert_eq!(r.namespace, "owner");
    }

    #[test]
    fn latest_ts_empty() {
        let store = setup();
        let ts = store.latest_dream_report_ts().unwrap();
        assert_eq!(ts, None);
    }

    #[test]
    fn latest_ts_after_insert() {
        let store = setup();

        store
            .insert_dream_report(&sample_report("r1", 1000, 2000))
            .unwrap();
        store
            .insert_dream_report(&sample_report("r2", 3000, 5000))
            .unwrap();

        let ts = store.latest_dream_report_ts().unwrap();
        assert_eq!(ts, Some(5000));
    }

    fn report_of(id: &str, pipeline: &str, started: i64, synthesis: u32) -> PersistedDreamReport {
        PersistedDreamReport {
            id: id.to_string(),
            pipeline_type: pipeline.to_string(),
            started_at: started,
            finished_at: started + 10,
            duration_ms: 10,
            synthesis_count: synthesis,
            errors: None,
            namespace: "owner".to_string(),
        }
    }

    #[test]
    fn distribution_groups_by_pipeline_and_sums_synthesis() {
        let store = setup();
        // Two `full` runs (synthesis 2 + 3 = 5) and one `decay` run (0) after the
        // watermark; one stale `full` run at/below it must be excluded.
        store
            .insert_dream_report(&report_of("stale", "full", 100, 9))
            .unwrap();
        store
            .insert_dream_report(&report_of("a", "full", 1001, 2))
            .unwrap();
        store
            .insert_dream_report(&report_of("b", "full", 1002, 3))
            .unwrap();
        store
            .insert_dream_report(&report_of("c", "decay", 1003, 0))
            .unwrap();

        let dist = store.dream_report_distribution_since(1000).unwrap();

        // Ordered by pipeline_type: decay before full.
        assert_eq!(
            dist,
            vec![
                DreamPipelineStat {
                    pipeline_type: "decay".to_string(),
                    runs: 1,
                    synthesis_sum: 0,
                },
                DreamPipelineStat {
                    pipeline_type: "full".to_string(),
                    runs: 2,
                    synthesis_sum: 5,
                },
            ]
        );
    }

    #[test]
    fn distribution_empty_when_nothing_in_window() {
        let store = setup();
        store
            .insert_dream_report(&report_of("old", "full", 100, 1))
            .unwrap();
        assert!(store
            .dream_report_distribution_since(1000)
            .unwrap()
            .is_empty());
    }
}
