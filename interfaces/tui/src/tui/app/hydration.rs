//! Pure, staged construction of a completed session transcript.
//!
//! The caller fetches history and `trace.by_runs` independently, then builds
//! this value off-screen. An active run has no replay/live watermark, so its
//! persisted prefix is never stitched to the current stream.

use std::collections::HashSet;

use aleph_protocol::AgentTraceEvent;
use serde_json::Value;
use shared_ui_logic::transcript::{Transcript, TranscriptEntry};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryState {
    Complete,
    /// The trace request failed or its reply was malformed. History remains
    /// visible, but must not be advertised as a complete tool transcript.
    HistoryOnly,
    /// Some individual runs could not be decoded; their text is preserved.
    Partial(Vec<String>),
}

#[derive(Debug, Clone)]
pub struct HydrationResult {
    pub session_key: String,
    pub generation: u64,
    pub transcript: Transcript,
    pub recovery: RecoveryState,
}

impl HydrationResult {
    #[must_use]
    pub fn matches(&self, session_key: &str, generation: u64) -> bool {
        self.session_key == session_key && self.generation == generation
    }
}

/// `traces` is the `runs` object from `trace.by_runs`; `None` means the RPC
/// failed. Never substitute the active run's persisted prefix for live data.
/// The result is all-or-nothing at the screen replacement boundary.
pub fn build_completed_history(
    session_key: String,
    generation: u64,
    rows: &[Value],
    traces: Option<&Value>,
    active_run: Option<&str>,
) -> HydrationResult {
    let runs = traces
        .and_then(|v| v.get("runs"))
        .and_then(Value::as_object);
    let mut transcript = Transcript::new();
    let mut replayed = HashSet::new();
    let mut failed = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let role = row.get("role").and_then(Value::as_str).unwrap_or("");
        let content = row.get("content").and_then(Value::as_str).unwrap_or("");
        let run_id = row.get("run_id").and_then(Value::as_str).or_else(|| {
            row.get("metadata")
                .and_then(|m| m.get("run_id"))
                .and_then(Value::as_str)
        });
        let id = format!("history-{index}");
        match role {
            "user" => {
                transcript.push_user(content, None, Vec::new());
            }
            "system" => {
                transcript.import_legacy(TranscriptEntry::SystemNotice {
                    id,
                    text: content.to_owned(),
                });
            }
            "assistant" => {
                if run_id.is_some_and(|r| Some(r) == active_run) {
                    // The server has not supplied a persisted/live watermark.
                    // This row is a prefix, not a completed historical answer.
                    continue;
                }
                let completed_run = run_id;
                let mut used_trace = false;
                let before = transcript.entries().len();
                if let Some(run_id) = completed_run {
                    if !replayed.contains(run_id) {
                        if let Some(events) =
                            runs.and_then(|m| m.get(run_id)).and_then(Value::as_array)
                        {
                            if !events.is_empty() {
                                match events
                                    .iter()
                                    .map(|v| serde_json::from_value::<AgentTraceEvent>(v.clone()))
                                    .collect::<Result<Vec<_>, _>>()
                                {
                                    Ok(events) => {
                                        replayed.insert(run_id.to_owned());
                                        for event in &events {
                                            transcript.apply_replay(event);
                                        }
                                        transcript.finish_replay();
                                        used_trace = true;
                                    }
                                    Err(_) => failed.push(run_id.to_owned()),
                                }
                            } else {
                                failed.push(run_id.to_owned());
                            }
                        } else if runs.is_some() {
                            failed.push(run_id.to_owned());
                        }
                    } else {
                        // One run can have several assistant history rows;
                        // its replay and final answer were inserted already.
                        continue;
                    }
                }
                if used_trace {
                    // Persisted trace rows can omit the final text (older
                    // records, failed runs). The history answer then wins.
                    let has_answer =
                        transcript.entries()[before..]
                            .iter()
                            .any(|entry| match entry {
                                TranscriptEntry::AssistantText { .. } => true,
                                TranscriptEntry::Step(step) => step
                                    .text
                                    .as_deref()
                                    .is_some_and(|text| text.trim_end() == content.trim_end()),
                                _ => false,
                            });
                    if !has_answer && !content.trim().is_empty() {
                        transcript.import_legacy(TranscriptEntry::AssistantText {
                            id,
                            markdown: content.to_owned(),
                            streaming: false,
                        });
                    }
                } else if !content.trim().is_empty() {
                    transcript.import_legacy(TranscriptEntry::AssistantText {
                        id,
                        markdown: content.to_owned(),
                        streaming: false,
                    });
                }
            }
            _ => {}
        }
    }
    let recovery = if !failed.is_empty() {
        RecoveryState::Partial(failed)
    } else if runs.is_none() {
        RecoveryState::HistoryOnly
    } else {
        RecoveryState::Complete
    };
    HydrationResult {
        session_key,
        generation,
        transcript,
        recovery,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn active_run_is_not_stitched_and_replacement_is_generation_scoped() {
        let rows = vec![
            json!({"role":"user","content":"question"}),
            json!({"role":"assistant","content":"live","run_id":"r"}),
        ];
        let traces = json!({"runs":{"r":[{"type":"turn_started","iteration":1}]}});
        let result = build_completed_history("s".into(), 4, &rows, Some(&traces), Some("r"));
        assert!(result.matches("s", 4));
        assert!(!result.matches("s", 5));
        assert!(!result.matches("other", 4));
        assert_eq!(result.transcript.entries().len(), 1);
        assert!(result
            .transcript
            .entries()
            .iter()
            .all(|e| !matches!(e, TranscriptEntry::Step(_))));
    }

    #[test]
    fn failed_trace_keeps_legacy_answer_and_marks_recovery() {
        let rows = vec![json!({"role":"assistant","content":"answer","run_id":"r"})];
        let traces = json!({"runs":{"r":[{"unknown":true}]}});
        let result = build_completed_history("s".into(), 1, &rows, Some(&traces), None);
        assert_eq!(result.recovery, RecoveryState::Partial(vec!["r".into()]));
        assert!(
            matches!(result.transcript.entries(), [TranscriptEntry::AssistantText { markdown, .. }] if markdown == "answer")
        );
    }
}
