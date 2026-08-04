//! `EventLog` — append-only audit trail for Dream cycles.
//!
//! Each Dream cycle produces one `DreamEvent` serialized as a JSON line
//! in `{agent_dir}/dream_events.jsonl`.

use std::io::SeekFrom;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

use crate::error::AlephError;
use crate::memory::dreaming::report::DreamReport;
use crate::memory::dreaming::selector::{GateDecision, SelectionDecision};
use crate::memory::dreaming::strategy::DreamStrategy;
use crate::memory::dreaming::validation::DreamValidationReport;

const EVENT_LOG_FILENAME: &str = "dream_events.jsonl";

/// Bytes pulled from the end of the log on the first backwards step.
///
/// A `DreamEvent` line is counters plus digests — the synthesis *bodies* left
/// this struct when the regex oscillation detector did, so a line runs 1–3 KB
/// in practice and one step already covers the deepest window any caller asks
/// for (`DREAM_HISTORY_WINDOW`). The loop expands geometrically rather than
/// trusting that estimate.
const TAIL_STEP_BYTES: u64 = 64 * 1024;

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
    /// Every caller wants a small tail — the daemon's per-cycle rehydration
    /// (`DREAM_HISTORY_WINDOW`), `dreaming.list_insights`, and
    /// `note_manage(action="evolution")` — while the file itself is append-only
    /// and grows for the life of the corpus. So the read is a genuine tail:
    /// seek to `len - window`, parse backwards, and expand the window
    /// geometrically only if `n` parseable events were not recovered.
    ///
    /// This is load-bearing rather than cosmetic. The previous implementation
    /// bounded the *parse* but read the whole file into a `String` first, so a
    /// night with K project namespaces paid K × (whole history) — and the doc
    /// comment claimed the opposite, which is the reasoning anyone would use to
    /// decide the unbounded growth was affordable.
    ///
    /// Semantics are unchanged: the result is the last `n` *parseable* events,
    /// so a corrupt line in the tail is skipped rather than shortening the
    /// window.
    pub async fn read_last(&self, n: usize) -> Result<Vec<DreamEvent>, AlephError> {
        self.read_last_measured(n).await.map(|(events, _)| events)
    }

    /// [`Self::read_last`] plus the number of bytes actually pulled off disk.
    ///
    /// The byte count exists so the boundedness above is *testable*: a claim
    /// about I/O cost that no test can see is exactly what regressed here once.
    async fn read_last_measured(&self, n: usize) -> Result<(Vec<DreamEvent>, u64), AlephError> {
        if n == 0 {
            return Ok((Vec::new(), 0));
        }
        let path = self.log_path();
        let mut file = match tokio::fs::File::open(&path).await {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((Vec::new(), 0)),
            Err(e) => return Err(AlephError::config(format!("open event log: {e}"))),
        };
        let len = file
            .metadata()
            .await
            .map_err(|e| AlephError::config(format!("stat event log: {e}")))?
            .len();

        let mut events: Vec<DreamEvent> = Vec::with_capacity(n);
        let mut window = 0u64;
        let mut bytes_read = 0u64;
        loop {
            window = if window == 0 {
                TAIL_STEP_BYTES
            } else {
                window.saturating_mul(2)
            }
            .min(len);
            let start = len - window;

            file.seek(SeekFrom::Start(start))
                .await
                .map_err(|e| AlephError::config(format!("seek event log: {e}")))?;
            let mut buf = vec![0u8; window as usize];
            file.read_exact(&mut buf)
                .await
                .map_err(|e| AlephError::config(format!("read event log: {e}")))?;
            bytes_read += window;

            // Unless the window reaches the start of the file, its first line is
            // the tail of a line that begins before it. Dropping that fragment is
            // what keeps a partial read from looking like a corrupt line — the
            // two are indistinguishable once parsing fails.
            let tail: &[u8] = if start == 0 {
                &buf
            } else {
                match buf.iter().position(|b| *b == b'\n') {
                    Some(nl) => &buf[nl + 1..],
                    // No newline anywhere in the window: every byte belongs to a
                    // line that starts before it. Nothing to parse yet.
                    None => &[],
                }
            };

            events.clear();
            for line in tail.split(|b| *b == b'\n').rev() {
                // `\n` cannot appear inside a multi-byte UTF-8 sequence, so
                // every split point is a character boundary.
                let Ok(text) = std::str::from_utf8(line) else {
                    continue;
                };
                if text.trim().is_empty() {
                    continue;
                }
                if let Ok(event) = serde_json::from_str(text) {
                    events.push(event);
                    if events.len() == n {
                        break;
                    }
                }
            }

            if events.len() == n || start == 0 {
                break;
            }
        }
        events.reverse();
        Ok((events, bytes_read))
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

    /// The window, not the history, is what a read costs.
    ///
    /// This is the assertion the old doc comment made and the old code did not
    /// keep: it read the whole file into a `String` first. With per-namespace
    /// sub-cycles each rehydrating its own log every night, that cost is paid
    /// K times per night and grows for the life of every corpus.
    #[tokio::test]
    async fn reading_a_small_tail_does_not_read_the_whole_history() {
        let dir = tempdir().unwrap();
        let log = EventLog::new(dir.path().join("test_agent"));

        // >4 KB per line × 96 lines, i.e. several expansion steps deep.
        for cycle in 1..=96 {
            let mut event = make_event(cycle);
            event.report.errors = Some("x".repeat(4000));
            log.append(&event).await.unwrap();
        }
        let file_len = tokio::fs::metadata(dir.path().join("test_agent/dream_events.jsonl"))
            .await
            .unwrap()
            .len();
        assert!(
            file_len > 4 * TAIL_STEP_BYTES,
            "history must exceed several steps: {file_len}"
        );

        let (events, bytes_read) = log.read_last_measured(3).await.unwrap();

        assert_eq!(events.len(), 3);
        assert_eq!(events[2].cycle, 96, "the newest event must be last");
        assert_eq!(events[0].cycle, 94);
        assert_eq!(
            bytes_read, TAIL_STEP_BYTES,
            "a 3-event window must cost one step, not the whole {file_len}-byte history"
        );
    }

    /// A line longer than one step must still be recoverable — the loop expands
    /// rather than assuming the estimate behind `TAIL_STEP_BYTES` holds.
    #[tokio::test]
    async fn an_event_larger_than_one_step_still_reads() {
        let dir = tempdir().unwrap();
        let log = EventLog::new(dir.path().join("test_agent"));

        let mut event = make_event(9);
        event.report.errors = Some("y".repeat(200_000));
        log.append(&event).await.unwrap();

        let events = log.read_last(1).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].cycle, 9);
        assert_eq!(events[0].report.errors.as_ref().unwrap().len(), 200_000);
    }

    /// Unparseable trailing lines are skipped, not counted — a torn write must
    /// not silently shrink the gate's history window.
    #[tokio::test]
    async fn corrupt_tail_lines_do_not_shorten_the_window() {
        let dir = tempdir().unwrap();
        let agent_dir = dir.path().join("test_agent");
        let log = EventLog::new(&agent_dir);

        log.append(&make_event(1)).await.unwrap();
        log.append(&make_event(2)).await.unwrap();
        log.append(&make_event(3)).await.unwrap();
        tokio::fs::write(agent_dir.join("dream_events.jsonl"), {
            let mut s = tokio::fs::read_to_string(agent_dir.join("dream_events.jsonl"))
                .await
                .unwrap();
            s.push_str("{\"id\":\"torn\",\"cycl\n");
            s
        })
        .await
        .unwrap();

        let events = log.read_last(2).await.unwrap();
        assert_eq!(events.len(), 2, "the corrupt line must not consume a slot");
        assert_eq!(events[0].cycle, 2);
        assert_eq!(events[1].cycle, 3);
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
