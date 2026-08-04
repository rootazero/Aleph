//! `EventLog` — append-only audit trail for Dream cycles.
//!
//! Each Dream cycle produces one `DreamEvent` serialized as a JSON line
//! in `{agent_dir}/dream_events.jsonl`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use crate::error::AlephError;
use crate::memory::dreaming::report::DreamReport;
use crate::memory::dreaming::selector::{GateDecision, SelectionDecision};
use crate::memory::dreaming::strategy::DreamStrategy;
use crate::memory::dreaming::validation::DreamValidationReport;

const EVENT_LOG_FILENAME: &str = "dream_events.jsonl";

/// A single Dream cycle event, the unit of the audit trail.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DreamEvent {
    pub id: String,
    pub cycle: u32,
    pub strategy: DreamStrategy,
    pub selection: SelectionDecision,
    pub gate_decision: GateDecision,
    pub report: DreamReport,
    pub validation: DreamValidationReport,
    pub duration_ms: u64,
    pub created_at: i64,
}

/// Append-only event log stored as JSONL.
pub struct EventLog {
    agent_dir: PathBuf,
}

impl EventLog {
    pub fn new(agent_dir: impl Into<PathBuf>) -> Self {
        Self {
            agent_dir: agent_dir.into(),
        }
    }

    fn log_path(&self) -> PathBuf {
        self.agent_dir.join(EVENT_LOG_FILENAME)
    }

    /// Append one event to the log file.
    pub async fn append(&self, event: &DreamEvent) -> Result<(), AlephError> {
        tokio::fs::create_dir_all(&self.agent_dir)
            .await
            .map_err(|e| AlephError::config(format!("create agent dir: {e}")))?;

        let mut line = serde_json::to_string(event)
            .map_err(|e| AlephError::config(format!("serialize event: {e}")))?;
        line.push('\n');

        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.log_path())
            .await
            .map_err(|e| AlephError::config(format!("open event log: {e}")))?;

        file.write_all(line.as_bytes())
            .await
            .map_err(|e| AlephError::config(format!("write event log: {e}")))?;

        // tokio::fs::File buffers writes; dropping the handle does NOT flush
        // (Drop cannot be async). Without this, a subsequent read_last/next_cycle
        // can miss the just-appended line — an intermittent durability race.
        file.flush()
            .await
            .map_err(|e| AlephError::config(format!("flush event log: {e}")))?;

        Ok(())
    }

    /// Read the last N events from the log. Returns them in chronological order.
    ///
    /// Parsing walks the file backwards and stops as soon as `n` events have
    /// been recovered, so the cost is bounded by the window rather than by the
    /// whole history. That matters because a `DreamEvent` carries the full
    /// cycle report (synthesis assertions included) while every caller wants a
    /// small tail: the daemon's per-cycle rehydration, `dreaming.list_insights`
    /// and `note_manage(action="evolution")` would otherwise each deserialize
    /// years of nightly cycles to throw all but a handful away.
    ///
    /// Semantics are unchanged: the result is the last `n` *parseable* events,
    /// so a corrupt line in the tail is skipped rather than shortening the
    /// window.
    pub async fn read_last(&self, n: usize) -> Result<Vec<DreamEvent>, AlephError> {
        let path = self.log_path();
        if !path.exists() || n == 0 {
            return Ok(Vec::new());
        }

        let content = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| AlephError::config(format!("read event log: {e}")))?;

        let mut events: Vec<DreamEvent> = Vec::with_capacity(n);
        for line in content.lines().rev() {
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(event) = serde_json::from_str(line) {
                events.push(event);
                if events.len() == n {
                    break;
                }
            }
        }
        events.reverse();
        Ok(events)
    }

    /// Get the next cycle number (max existing + 1, or 1 if empty).
    pub async fn next_cycle(&self) -> Result<u32, AlephError> {
        let events = self.read_last(1).await?;
        Ok(events.last().map_or(1, |e| e.cycle + 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::dreaming::validation::ValidationTier;
    use tempfile::tempdir;

    fn make_event(cycle: u32) -> DreamEvent {
        DreamEvent {
            id: format!("dream_test_{cycle}"),
            cycle,
            strategy: DreamStrategy::Consolidate,
            selection: SelectionDecision {
                strategy: DreamStrategy::Consolidate,
                rationale: "test".into(),
                personality_adjustment: 0.0,
            },
            gate_decision: GateDecision::Allow,
            report: DreamReport::default(),
            validation: DreamValidationReport {
                l1_format: ValidationTier {
                    passed: true,
                    checks_run: 1,
                    checks_passed: 1,
                    issues: vec![],
                },
                l2_consistency: ValidationTier {
                    passed: true,
                    checks_run: 1,
                    checks_passed: 1,
                    issues: vec![],
                },
                l3_semantic: None,
                l4_retrospective: None,
            },
            duration_ms: 100,
            created_at: 1_700_000_000,
        }
    }

    #[tokio::test]
    async fn append_and_read_events() {
        let dir = tempdir().unwrap();
        let log = EventLog::new(dir.path().join("test_agent"));

        log.append(&make_event(1)).await.unwrap();
        log.append(&make_event(2)).await.unwrap();
        log.append(&make_event(3)).await.unwrap();

        let events = log.read_last(2).await.unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].cycle, 2);
        assert_eq!(events[1].cycle, 3);
    }

    #[tokio::test]
    async fn read_from_empty_log() {
        let dir = tempdir().unwrap();
        let log = EventLog::new(dir.path().join("test_agent"));
        let events = log.read_last(10).await.unwrap();
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn read_more_than_available() {
        let dir = tempdir().unwrap();
        let log = EventLog::new(dir.path().join("test_agent"));
        log.append(&make_event(1)).await.unwrap();
        let events = log.read_last(100).await.unwrap();
        assert_eq!(events.len(), 1);
    }

    #[tokio::test]
    async fn next_cycle_number_from_empty() {
        let dir = tempdir().unwrap();
        let log = EventLog::new(dir.path().join("test_agent"));
        assert_eq!(log.next_cycle().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn next_cycle_increments() {
        let dir = tempdir().unwrap();
        let log = EventLog::new(dir.path().join("test_agent"));
        log.append(&make_event(5)).await.unwrap();
        assert_eq!(log.next_cycle().await.unwrap(), 6);
    }

    #[test]
    fn event_serde_roundtrip() {
        let event = make_event(42);
        let json = serde_json::to_string(&event).unwrap();
        let back: DreamEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back.cycle, 42);
        assert_eq!(back.id, "dream_test_42");
    }

    #[tokio::test]
    async fn distill_actions_survive_round_trip() {
        use crate::memory::dreaming::distill_action::{DistillActionRecord, DistillOutcome};

        let dir = tempdir().unwrap();
        let log = EventLog::new(dir.path().join("test_agent"));

        // Synthesize a cycle that exercises all three outcomes so the
        // on-disk format covers every variant the evolver-parity path
        // can produce.
        let mut event = make_event(7);
        event.report.distill_actions = vec![
            DistillActionRecord {
                stage: "skill_distill".into(),
                action_kind: "new".into(),
                target_path: None,
                title: Some("async-error".into()),
                confidence: Some(0.85),
                severity: Some("high".into()),
                outcome: DistillOutcome::Applied,
                error: None,
            },
            DistillActionRecord {
                stage: "skill_distill".into(),
                action_kind: "strengthen".into(),
                target_path: Some("skill/async-error".into()),
                title: None,
                confidence: None,
                severity: None,
                outcome: DistillOutcome::FilteredNonCandidate,
                error: None,
            },
            DistillActionRecord {
                stage: "feedback_distill".into(),
                action_kind: "supersede".into(),
                target_path: Some("feedback/typo".into()),
                title: Some("fix-typo".into()),
                confidence: Some(0.6),
                severity: Some("med".into()),
                outcome: DistillOutcome::Error,
                error: Some("indexer offline".into()),
            },
        ];

        log.append(&event).await.unwrap();
        let read_back = log.read_last(1).await.unwrap();
        assert_eq!(read_back.len(), 1);
        let actions = &read_back[0].report.distill_actions;
        assert_eq!(actions.len(), 3);
        assert_eq!(actions[0].action_kind, "new");
        assert_eq!(actions[1].outcome, DistillOutcome::FilteredNonCandidate);
        assert_eq!(actions[2].error.as_deref(), Some("indexer offline"));
    }

    #[test]
    fn pre_existing_event_log_lines_deserialize_without_distill_actions() {
        // Backward-compat: events written before `distill_actions` existed
        // must deserialize cleanly (#[serde(default)] empties the vec).
        let legacy_json = r#"{
            "id": "dream_legacy_1",
            "cycle": 1,
            "strategy": "consolidate",
            "selection": {"strategy": "consolidate", "rationale": "ok", "personality_adjustment": 0.0},
            "gate_decision": {"type": "allow"},
            "report": {
                "pipeline_type": "consolidate",
                "started_at": 0,
                "finished_at": 0,
                "duration_ms": 0,
                "notes_consolidated": 0,
                "contradictions_found": 0,
                "notes_marked_stale": 0,
                "synthesis_count": 0,
                "format_fixed": 0,
                "broken_links_found": 0,
                "links_repaired": 0,
                "links_purged": 0,
                "notes_archived": 0,
                "notes_protected": 0,
                "errors": null
            },
            "validation": {
                "l1_format": {"passed": true, "checks_run": 0, "checks_passed": 0, "issues": []},
                "l2_consistency": {"passed": true, "checks_run": 0, "checks_passed": 0, "issues": []},
                "l3_semantic": null,
                "l4_retrospective": null
            },
            "duration_ms": 0,
            "created_at": 0
        }"#;
        let parsed: DreamEvent = serde_json::from_str(legacy_json).unwrap();
        assert!(parsed.report.distill_actions.is_empty());
    }
}
