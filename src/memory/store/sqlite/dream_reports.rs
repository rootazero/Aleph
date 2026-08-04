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
///
/// The activity counters (`notes_consolidated` / `notes_woven` /
/// `notes_archived` / `feedback_distilled`) mirror the same-named fields of the
/// in-flight `DreamReport`. They exist so the governance audit ring has a
/// reality signal that is non-zero on a healthy *consolidate* night —
/// `synthesis_count` alone is 0 on every consolidate run (the `NoteSynthesis`
/// stage lives only in the rarer `Synthesize` pipeline), which made the old
/// `synthesis_sum`-only probe a false-negative machine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedDreamReport {
    pub id: String,
    pub pipeline_type: String,
    pub started_at: i64,
    pub finished_at: i64,
    pub duration_ms: i64,
    pub synthesis_count: u32,
    /// Notes merged/consolidated by `NoteConsolidate` (consolidate + synthesize).
    pub notes_consolidated: u32,
    /// Orphan notes woven into the link graph by `NoteWeave` (consolidate path).
    pub notes_woven: u32,
    /// Notes archived by `NoteDecay` (consolidate path).
    pub notes_archived: u32,
    /// Feedback rules distilled from user corrections by `FeedbackDistill`
    /// (runs on BOTH consolidate and synthesize). This is the counter-metric the
    /// Dreaming×correction Goodhart check pairs against `corrections`.
    pub feedback_distilled: u32,
    pub errors: Option<String>,
    pub namespace: String,
    /// Serialized `EvolutionOutcome` for this cycle (SkillOpt gate verdict:
    /// baseline/candidate/best health + accept/reject). `None` for rows written
    /// before the column existed, or when the cycle produced no gate decision
    /// (e.g. a Conserve run). Stored as JSON so the schema stays forward-
    /// compatible if the outcome shape grows. Read by `dreaming.list_insights`.
    pub evolution_json: Option<String>,
    /// Serialized `CycleDecision` — which strategy ran, why, what the churn gate
    /// said, which stages executed, and whether validation passed. `None` on
    /// pre-migration rows. This is what lets the Panel answer "why is dreaming
    /// always conserving?"; before it existed the answer was only in
    /// `dream_events.jsonl`, i.e. visible to the model but not to the operator.
    pub decision_json: Option<String>,
}

/// One corpus's dream history at a glance: how many cycles it has run, and what
/// the most recent one decided.
///
/// The operator-facing counterpart to the per-namespace event logs. A project
/// namespace governs itself — its own churn gate, personality and best-health
/// checkpoint, all folded out of its own `dream_events.jsonl` — which made its
/// history readable by the *model* (`note_manage(action="evolution")` resolves
/// the scoped agent id) and by nobody else. This is the row that lets the Panel
/// answer "did that project's corpus run last night, and is it stuck
/// conserving?" without shelling out to the JSONL.
///
/// `last_decision_json` is handed over unparsed on purpose: the RPC layer
/// already owns one JSON-blob parser for `decision_json`, and a second one here
/// would be a second place for the `CycleDecision` shape to drift.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DreamNamespaceStat {
    /// Storage partition key — the base agent id, or `{base}__proj-*`.
    pub namespace: String,
    /// Cycles this corpus has recorded, all time.
    pub runs: u64,
    /// `started_at` of the most recent cycle (epoch seconds).
    pub last_started_at: i64,
    /// Pipeline the most recent cycle ran.
    pub last_pipeline_type: String,
    /// Serialized `CycleDecision` of the most recent cycle; `None` on
    /// pre-migration rows.
    pub last_decision_json: Option<String>,
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
    /// Note: 0 on consolidate runs by design — read the counters below for the
    /// distillation actually done on a consolidate night.
    pub synthesis_sum: i64,
    /// Total notes consolidated across those runs (`sum(notes_consolidated)`).
    pub consolidated_sum: i64,
    /// Total orphan notes woven across those runs (`sum(notes_woven)`).
    pub woven_sum: i64,
    /// Total notes archived across those runs (`sum(notes_archived)`).
    pub archived_sum: i64,
    /// Total feedback rules distilled across those runs (`sum(feedback_distilled)`).
    pub feedback_distilled_sum: i64,
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
              synthesis_count, notes_consolidated, notes_woven, notes_archived, \
              feedback_distilled, errors, namespace, evolution_json, decision_json) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                report.id,
                report.pipeline_type,
                report.started_at,
                report.finished_at,
                report.duration_ms,
                report.synthesis_count,
                report.notes_consolidated,
                report.notes_woven,
                report.notes_archived,
                report.feedback_distilled,
                report.errors,
                report.namespace,
                report.evolution_json,
                report.decision_json,
            ],
        )
        .map_err(|e| AlephError::config(format!("insert_dream_report: {e}")))?;

        Ok(())
    }

    /// Query recent dream reports, ordered by `started_at` descending.
    ///
    /// `namespace` scopes the result to one corpus (the base agent id, or a
    /// `{base}__proj-*` project namespace); `None` returns every corpus
    /// interleaved. Scoping matters now that project sub-cycles write rows of
    /// their own: an unscoped window of 30 covers 30/(K+1) nights once K project
    /// namespaces are active, so the base agent's history would quietly thin out
    /// as the user opens more projects.
    pub fn recent_dream_reports(
        &self,
        namespace: Option<&str>,
        limit: usize,
    ) -> Result<Vec<PersistedDreamReport>, AlephError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        let mut stmt = conn
            .prepare(
                "SELECT id, pipeline_type, started_at, finished_at, duration_ms, \
                 synthesis_count, notes_consolidated, notes_woven, notes_archived, \
                 feedback_distilled, errors, namespace, evolution_json, decision_json \
                 FROM dream_reports \
                 WHERE (?1 IS NULL OR namespace = ?1) \
                 ORDER BY started_at DESC LIMIT ?2",
            )
            .map_err(|e| AlephError::config(format!("recent_dream_reports prepare: {e}")))?;

        let rows = stmt
            .query_map(params![namespace, limit as i64], |row| {
                Ok(PersistedDreamReport {
                    id: row.get("id")?,
                    pipeline_type: row.get("pipeline_type")?,
                    started_at: row.get("started_at")?,
                    finished_at: row.get("finished_at")?,
                    duration_ms: row.get("duration_ms")?,
                    synthesis_count: row.get("synthesis_count")?,
                    notes_consolidated: row.get("notes_consolidated")?,
                    notes_woven: row.get("notes_woven")?,
                    notes_archived: row.get("notes_archived")?,
                    feedback_distilled: row.get("feedback_distilled")?,
                    errors: row.get("errors")?,
                    namespace: row.get("namespace")?,
                    evolution_json: row.get("evolution_json")?,
                    decision_json: row.get("decision_json")?,
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

    /// Per-corpus dream history, most recently active first, capped at `limit`.
    ///
    /// One row per `namespace` carrying its all-time cycle count and the shape
    /// of its latest cycle. Ordered by recency (not by name) precisely *because*
    /// it is capped: dropping the corpora nobody has touched is the only honest
    /// way to truncate this list.
    pub fn dream_namespace_rollup(
        &self,
        limit: usize,
    ) -> Result<Vec<DreamNamespaceStat>, AlephError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        // `id` breaks the ORDER BY tie deterministically: the base cycle and its
        // project sub-cycles can finish inside the same second.
        let mut stmt = conn
            .prepare(
                "SELECT namespace, runs, started_at, pipeline_type, decision_json FROM ( \
                   SELECT namespace, started_at, pipeline_type, decision_json, \
                          COUNT(*) OVER (PARTITION BY namespace) AS runs, \
                          ROW_NUMBER() OVER ( \
                            PARTITION BY namespace ORDER BY started_at DESC, id DESC \
                          ) AS rn \
                     FROM dream_reports \
                 ) WHERE rn = 1 \
                 ORDER BY started_at DESC LIMIT ?1",
            )
            .map_err(|e| AlephError::config(format!("dream_namespace_rollup prepare: {e}")))?;

        let rows = stmt
            .query_map(params![limit as i64], |row| {
                Ok(DreamNamespaceStat {
                    namespace: row.get("namespace")?,
                    runs: row.get::<_, i64>("runs")?.max(0) as u64,
                    last_started_at: row.get("started_at")?,
                    last_pipeline_type: row.get("pipeline_type")?,
                    last_decision_json: row.get("decision_json")?,
                })
            })
            .map_err(|e| AlephError::config(format!("dream_namespace_rollup query: {e}")))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(
                row.map_err(|e| AlephError::config(format!("dream_namespace_rollup row: {e}")))?,
            );
        }
        Ok(results)
    }

    /// Dream activity grouped by `pipeline_type` for runs of `namespace` started
    /// after `since_started_at` (inclusive-exclusive: `started_at > since`).
    /// Powers the governance audit's "dreaming last N days" reality probe.
    /// `started_at` is in **epoch seconds** (memory.db convention). Buckets are
    /// ordered by `pipeline_type` for deterministic output.
    ///
    /// The `namespace` scope keeps this probe answering the question it was
    /// built to answer once project sub-cycles started writing rows too. Summing
    /// every corpus would inflate `runs` by the number of open projects while
    /// leaving `feedback_distilled_sum` flat — that stage is global-only and
    /// never runs in a sub-cycle — i.e. exactly the direction that makes the
    /// Dreaming × correction Goodhart pairing read healthier than it is.
    pub fn dream_report_distribution_since(
        &self,
        namespace: &str,
        since_started_at: i64,
    ) -> Result<Vec<DreamPipelineStat>, AlephError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        let mut stmt = conn
            .prepare(
                "SELECT pipeline_type, count(*) AS runs, \
                 COALESCE(sum(synthesis_count), 0) AS synthesis_sum, \
                 COALESCE(sum(notes_consolidated), 0) AS consolidated_sum, \
                 COALESCE(sum(notes_woven), 0) AS woven_sum, \
                 COALESCE(sum(notes_archived), 0) AS archived_sum, \
                 COALESCE(sum(feedback_distilled), 0) AS feedback_distilled_sum \
                 FROM dream_reports \
                 WHERE started_at > ?1 AND namespace = ?2 \
                 GROUP BY pipeline_type \
                 ORDER BY pipeline_type ASC",
            )
            .map_err(|e| {
                AlephError::config(format!("dream_report_distribution_since prepare: {e}"))
            })?;

        let rows = stmt
            .query_map(params![since_started_at, namespace], |row| {
                Ok(DreamPipelineStat {
                    pipeline_type: row.get("pipeline_type")?,
                    runs: row.get::<_, i64>("runs")?.max(0) as u64,
                    synthesis_sum: row.get("synthesis_sum")?,
                    consolidated_sum: row.get("consolidated_sum")?,
                    woven_sum: row.get("woven_sum")?,
                    archived_sum: row.get("archived_sum")?,
                    feedback_distilled_sum: row.get("feedback_distilled_sum")?,
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
            notes_consolidated: 2,
            notes_woven: 3,
            notes_archived: 4,
            feedback_distilled: 5,
            errors: None,
            namespace: "main".to_string(),
            evolution_json: None,
            decision_json: None,
        }
    }

    #[test]
    fn insert_and_query_report() {
        let store = setup();
        let report = sample_report("r1", 1000, 2000);

        store.insert_dream_report(&report).unwrap();

        let reports = store.recent_dream_reports(None, 10).unwrap();
        assert_eq!(reports.len(), 1);

        let r = &reports[0];
        assert_eq!(r.id, "r1");
        assert_eq!(r.pipeline_type, "full");
        assert_eq!(r.started_at, 1000);
        assert_eq!(r.finished_at, 2000);
        assert_eq!(r.duration_ms, 1000);
        assert_eq!(r.synthesis_count, 1);
        assert_eq!(r.notes_consolidated, 2);
        assert_eq!(r.notes_woven, 3);
        assert_eq!(r.notes_archived, 4);
        assert_eq!(r.feedback_distilled, 5);
        assert!(r.errors.is_none());
        assert_eq!(r.namespace, "main");
        assert!(r.evolution_json.is_none());
    }

    #[test]
    fn evolution_json_round_trips() {
        let store = setup();
        let mut report = sample_report("r1", 1000, 2000);
        report.evolution_json = Some(r#"{"outcome":"accept_new_best","best":0.7}"#.to_string());
        store.insert_dream_report(&report).unwrap();

        let reports = store.recent_dream_reports(None, 10).unwrap();
        assert_eq!(
            reports[0].evolution_json.as_deref(),
            Some(r#"{"outcome":"accept_new_best","best":0.7}"#)
        );
    }

    /// A project namespace's rows must not thin out the base agent's window.
    #[test]
    fn recent_reports_scope_to_one_corpus() {
        let store = setup();
        for (id, ns, started) in [
            ("base-1", "main", 1000),
            ("proj-1", "main__proj-aaaa", 1001),
            ("proj-2", "main__proj-bbbb", 1002),
            ("base-2", "main", 1003),
        ] {
            let mut r = sample_report(id, started, started + 5);
            r.namespace = ns.to_string();
            store.insert_dream_report(&r).unwrap();
        }

        let base = store.recent_dream_reports(Some("main"), 10).unwrap();
        assert_eq!(
            base.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            vec!["base-2", "base-1"]
        );

        let proj = store
            .recent_dream_reports(Some("main__proj-aaaa"), 10)
            .unwrap();
        assert_eq!(proj.len(), 1);
        assert_eq!(proj[0].id, "proj-1");

        assert_eq!(store.recent_dream_reports(None, 10).unwrap().len(), 4);
    }

    #[test]
    fn namespace_rollup_counts_runs_and_carries_the_latest_decision() {
        let store = setup();
        for (id, ns, started) in [
            ("base-1", "main", 1000),
            ("base-2", "main", 1003),
            ("proj-1", "main__proj-aaaa", 1001),
        ] {
            let mut r = sample_report(id, started, started + 5);
            r.namespace = ns.to_string();
            r.decision_json = Some(format!(r#"{{"strategy":"conserve","from":"{id}"}}"#));
            store.insert_dream_report(&r).unwrap();
        }

        let rollup = store.dream_namespace_rollup(10).unwrap();
        // Most recently active corpus first — that ordering is what makes the
        // cap honest.
        assert_eq!(
            rollup
                .iter()
                .map(|s| s.namespace.as_str())
                .collect::<Vec<_>>(),
            vec!["main", "main__proj-aaaa"]
        );
        assert_eq!(rollup[0].runs, 2);
        assert_eq!(rollup[0].last_started_at, 1003);
        assert!(rollup[0]
            .last_decision_json
            .as_deref()
            .unwrap()
            .contains("base-2"));
        assert_eq!(rollup[1].runs, 1);
        assert_eq!(rollup[1].last_started_at, 1001);
    }

    #[test]
    fn namespace_rollup_caps_by_recency() {
        let store = setup();
        for i in 0..5 {
            let mut r = sample_report(&format!("r{i}"), 1000 + i, 1000 + i + 1);
            r.namespace = format!("main__proj-{i}");
            store.insert_dream_report(&r).unwrap();
        }
        let rollup = store.dream_namespace_rollup(2).unwrap();
        assert_eq!(rollup.len(), 2);
        assert_eq!(rollup[0].namespace, "main__proj-4");
        assert_eq!(rollup[1].namespace, "main__proj-3");
    }

    #[test]
    fn namespace_rollup_is_empty_without_rows() {
        assert!(setup().dream_namespace_rollup(10).unwrap().is_empty());
    }

    fn report_of(id: &str, pipeline: &str, started: i64, synthesis: u32) -> PersistedDreamReport {
        PersistedDreamReport {
            id: id.to_string(),
            pipeline_type: pipeline.to_string(),
            started_at: started,
            finished_at: started + 10,
            duration_ms: 10,
            synthesis_count: synthesis,
            notes_consolidated: 0,
            notes_woven: 0,
            notes_archived: 0,
            feedback_distilled: 0,
            errors: None,
            namespace: "main".to_string(),
            evolution_json: None,
            decision_json: None,
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

        let dist = store.dream_report_distribution_since("main", 1000).unwrap();

        // Ordered by pipeline_type: decay before full.
        assert_eq!(
            dist,
            vec![
                DreamPipelineStat {
                    pipeline_type: "decay".to_string(),
                    runs: 1,
                    synthesis_sum: 0,
                    consolidated_sum: 0,
                    woven_sum: 0,
                    archived_sum: 0,
                    feedback_distilled_sum: 0,
                },
                DreamPipelineStat {
                    pipeline_type: "full".to_string(),
                    runs: 2,
                    synthesis_sum: 5,
                    consolidated_sum: 0,
                    woven_sum: 0,
                    archived_sum: 0,
                    feedback_distilled_sum: 0,
                },
            ]
        );
    }

    #[test]
    fn distribution_sums_activity_counters() {
        // The consolidate path produces 0 synthesis but real distillation work.
        // The audit's reality signal must sum those counters, not just synthesis.
        let store = setup();
        let mut a = report_of("a", "consolidate", 1001, 0);
        a.notes_consolidated = 3;
        a.notes_woven = 1;
        a.notes_archived = 2;
        a.feedback_distilled = 1;
        let mut b = report_of("b", "consolidate", 1002, 0);
        b.notes_consolidated = 4;
        b.notes_woven = 0;
        b.notes_archived = 5;
        b.feedback_distilled = 2;
        store.insert_dream_report(&a).unwrap();
        store.insert_dream_report(&b).unwrap();

        let dist = store.dream_report_distribution_since("main", 1000).unwrap();
        assert_eq!(dist.len(), 1);
        let s = &dist[0];
        assert_eq!(s.pipeline_type, "consolidate");
        assert_eq!(s.runs, 2);
        assert_eq!(s.synthesis_sum, 0);
        assert_eq!(s.consolidated_sum, 7);
        assert_eq!(s.woven_sum, 1);
        assert_eq!(s.archived_sum, 7);
        assert_eq!(s.feedback_distilled_sum, 3);
    }

    #[test]
    fn distribution_empty_when_nothing_in_window() {
        let store = setup();
        store
            .insert_dream_report(&report_of("old", "full", 100, 1))
            .unwrap();
        assert!(store
            .dream_report_distribution_since("main", 1000)
            .unwrap()
            .is_empty());
    }
}
