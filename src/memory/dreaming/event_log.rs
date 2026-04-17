//! EventLog — append-only audit trail for Dream cycles.
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

        Ok(())
    }

    /// Read the last N events from the log. Returns them in chronological order.
    pub async fn read_last(&self, n: usize) -> Result<Vec<DreamEvent>, AlephError> {
        let path = self.log_path();
        if !path.exists() {
            return Ok(Vec::new());
        }

        let content = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| AlephError::config(format!("read event log: {e}")))?;

        let events: Vec<DreamEvent> = content
            .lines()
            .filter(|line| !line.trim().is_empty())
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();

        let skip = events.len().saturating_sub(n);
        Ok(events.into_iter().skip(skip).collect())
    }

    /// Get the next cycle number (max existing + 1, or 1 if empty).
    pub async fn next_cycle(&self) -> Result<u32, AlephError> {
        let events = self.read_last(1).await?;
        Ok(events.last().map(|e| e.cycle + 1).unwrap_or(1))
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
}
