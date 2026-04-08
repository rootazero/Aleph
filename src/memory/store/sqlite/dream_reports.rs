//! DreamReportStore — audit trail for dream pipeline executions.
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
    pub facts_collected: u32,
    pub clusters_found: u32,
    pub drift_detected: bool,
    pub drift_summary: Option<String>,
    pub candidates_evaluated: u32,
    pub facts_promoted: u32,
    pub promotion_details: Option<String>,
    pub facts_decayed: u32,
    pub facts_pruned: u32,
    pub nodes_decayed: u32,
    pub edges_decayed: u32,
    pub synthesis_count: u32,
    pub errors: Option<String>,
    pub namespace: String,
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
              facts_collected, clusters_found, drift_detected, drift_summary, \
              candidates_evaluated, facts_promoted, promotion_details, \
              facts_decayed, facts_pruned, nodes_decayed, edges_decayed, \
              synthesis_count, errors, namespace) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
            params![
                report.id,
                report.pipeline_type,
                report.started_at,
                report.finished_at,
                report.duration_ms,
                report.facts_collected,
                report.clusters_found,
                report.drift_detected as i32,
                report.drift_summary,
                report.candidates_evaluated,
                report.facts_promoted,
                report.promotion_details,
                report.facts_decayed,
                report.facts_pruned,
                report.nodes_decayed,
                report.edges_decayed,
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
                 facts_collected, clusters_found, drift_detected, drift_summary, \
                 candidates_evaluated, facts_promoted, promotion_details, \
                 facts_decayed, facts_pruned, nodes_decayed, edges_decayed, \
                 synthesis_count, errors, namespace \
                 FROM dream_reports ORDER BY started_at DESC LIMIT ?1",
            )
            .map_err(|e| AlephError::config(format!("recent_dream_reports prepare: {e}")))?;

        let rows = stmt
            .query_map(params![limit as i64], |row| {
                let drift_int: i32 = row.get("drift_detected")?;
                Ok(PersistedDreamReport {
                    id: row.get("id")?,
                    pipeline_type: row.get("pipeline_type")?,
                    started_at: row.get("started_at")?,
                    finished_at: row.get("finished_at")?,
                    duration_ms: row.get("duration_ms")?,
                    facts_collected: row.get("facts_collected")?,
                    clusters_found: row.get("clusters_found")?,
                    drift_detected: drift_int != 0,
                    drift_summary: row.get("drift_summary")?,
                    candidates_evaluated: row.get("candidates_evaluated")?,
                    facts_promoted: row.get("facts_promoted")?,
                    promotion_details: row.get("promotion_details")?,
                    facts_decayed: row.get("facts_decayed")?,
                    facts_pruned: row.get("facts_pruned")?,
                    nodes_decayed: row.get("nodes_decayed")?,
                    edges_decayed: row.get("edges_decayed")?,
                    synthesis_count: row.get("synthesis_count")?,
                    errors: row.get("errors")?,
                    namespace: row.get("namespace")?,
                })
            })
            .map_err(|e| AlephError::config(format!("recent_dream_reports query: {e}")))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(
                row.map_err(|e| {
                    AlephError::config(format!("recent_dream_reports row: {e}"))
                })?,
            );
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
            .query_row(
                "SELECT MAX(finished_at) FROM dream_reports",
                [],
                |row| row.get(0),
            )
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
            facts_collected: 42,
            clusters_found: 3,
            drift_detected: true,
            drift_summary: Some("topic shift detected".to_string()),
            candidates_evaluated: 10,
            facts_promoted: 2,
            promotion_details: Some("promoted f1, f2".to_string()),
            facts_decayed: 5,
            facts_pruned: 1,
            nodes_decayed: 3,
            edges_decayed: 2,
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
        assert_eq!(r.facts_collected, 42);
        assert_eq!(r.clusters_found, 3);
        assert!(r.drift_detected);
        assert_eq!(r.drift_summary.as_deref(), Some("topic shift detected"));
        assert_eq!(r.candidates_evaluated, 10);
        assert_eq!(r.facts_promoted, 2);
        assert_eq!(r.promotion_details.as_deref(), Some("promoted f1, f2"));
        assert_eq!(r.facts_decayed, 5);
        assert_eq!(r.facts_pruned, 1);
        assert_eq!(r.nodes_decayed, 3);
        assert_eq!(r.edges_decayed, 2);
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

        store.insert_dream_report(&sample_report("r1", 1000, 2000)).unwrap();
        store.insert_dream_report(&sample_report("r2", 3000, 5000)).unwrap();

        let ts = store.latest_dream_report_ts().unwrap();
        assert_eq!(ts, Some(5000));
    }
}
