//! `ProjectionReconciler` — boot-time events→messages back-fill for the file
//! backend's transcript projection.
//!
//! P1 made `session_events` the SSOT and materialised the `messages`
//! projection asynchronously via `MessageProjector`. On a hard crash *during* a
//! run, events durably in `session_events` may not have been drained to
//! `transcript.jsonl` — the Panel display loses those rows. This reconciler
//! runs at boot, finds interrupted sessions (via `ResumeCoordinator`'s run
//! markers), and re-projects the un-materialised tail through the same
//! `project_event` the live drain uses.
//!
//! Scope (see `docs/superpowers/specs/2026-07-04-projection-reconciler-p2-design.md`):
//! file backend only, interrupted runs only, no schema change — the source seq
//! is recovered from the `"{key}:{seq}"` id embedded in each projector-written
//! transcript row.

use std::collections::HashSet;

use crate::gateway::resume_coordinator::{classify_markers, ScanVerdict};
use crate::gateway::session_projector::{project_event, TurnAccums};
use crate::gateway::session_store::SessionStore;
use crate::session::service::SessionId;
use crate::session::store::SessionEventStore;
use crate::sync_primitives::Arc;

/// Summary of one `reconcile_interrupted` pass — for the boot log and tests.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ReconcileReport {
    /// Sessions with run markers inspected.
    pub scanned: usize,
    /// Interrupted sessions that had ≥1 row filled.
    pub reconciled: usize,
    /// Total transcript rows appended.
    pub rows_filled: usize,
    /// Sessions skipped because the newest marker is `RunFinished`.
    pub skipped_clean: usize,
    /// Sessions skipped because the transcript is non-empty but carries no
    /// parseable source seq (foreign / pre-P1 rows — never touched).
    pub skipped_legacy: usize,
}

/// Parse the source event seq embedded in a projector-written row id.
///
/// Projector ids have the form `"{session_key}:{seq}"`. Returns `seq` only when
/// the prefix equals `session_key` exactly — rejecting legacy / foreign ids
/// that carry no such suffix. `session_key` may itself contain `':'`; the split
/// is on the LAST `':'`, which is the separator the projector appended.
fn parse_source_seq(id: &str, session_key: &str) -> Option<u64> {
    let (prefix, suffix) = id.rsplit_once(':')?;
    if prefix != session_key {
        return None;
    }
    suffix.parse::<u64>().ok()
}

/// Boot-time reconciler. Constructed with the durable event store and the
/// projection target (the same `SessionStore` `MessageProjector` writes to).
pub struct ProjectionReconciler {
    event_store: Arc<dyn SessionEventStore>,
    session_store: Arc<dyn SessionStore>,
}

impl ProjectionReconciler {
    pub fn new(
        event_store: Arc<dyn SessionEventStore>,
        session_store: Arc<dyn SessionStore>,
    ) -> Self {
        Self {
            event_store,
            session_store,
        }
    }

    /// Scan run markers; for each interrupted session, fill the un-materialised
    /// transcript tail from the event log. Best-effort: any failure is logged
    /// and skipped; never panics, never blocks boot.
    pub async fn reconcile_interrupted(&self) -> ReconcileReport {
        let mut report = ReconcileReport::default();

        let groups = match self.event_store.load_run_markers().await {
            Ok(g) => g,
            Err(e) => {
                tracing::warn!(error = %e, "projection reconcile: load_run_markers failed; skipping");
                return report;
            }
        };

        for (session_id, markers) in groups {
            report.scanned += 1;
            match classify_markers(&markers) {
                ScanVerdict::Clean => report.skipped_clean += 1,
                ScanVerdict::Interrupted { .. } => {
                    self.reconcile_session(&session_id, &mut report).await;
                }
            }
        }

        tracing::info!(
            scanned = report.scanned,
            reconciled = report.reconciled,
            rows_filled = report.rows_filled,
            skipped_clean = report.skipped_clean,
            skipped_legacy = report.skipped_legacy,
            "projection reconcile scan complete"
        );
        report
    }

    /// Reconcile one interrupted session: derive the watermark from the
    /// transcript, load the un-projected tail, replay it (writes suppressed for
    /// already-materialised seqs).
    async fn reconcile_session(&self, session_id: &SessionId, report: &mut ReconcileReport) {
        let session_key = session_id.to_key_string();

        let transcript = match self.session_store.get_history(session_id, None).await {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(session = ?session_id, error = %e, "projection reconcile: get_history failed; skipping");
                return;
            }
        };

        let seqs: HashSet<u64> = transcript
            .iter()
            .filter_map(|m| parse_source_seq(&m.id, &session_key))
            .collect();

        // Legacy guard: a non-empty transcript with no parseable seq is
        // foreign / pre-P1 content — never touch it (would risk duplicates).
        if !transcript.is_empty() && seqs.is_empty() {
            report.skipped_legacy += 1;
            return;
        }

        let watermark = seqs.iter().copied().max().unwrap_or(0);

        let tail = match self
            .event_store
            .load_events_range(session_id, Some(watermark + 1), None)
            .await
        {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(session = ?session_id, error = %e, "projection reconcile: load_events_range failed; skipping");
                return;
            }
        };
        if tail.is_empty() {
            return;
        }

        let before = transcript.len();
        let mut accums = TurnAccums::new();
        for rec in &tail {
            project_event(
                &self.session_store,
                &mut accums,
                session_id,
                rec,
                Some(&seqs),
            )
            .await;
        }
        let after = self
            .session_store
            .get_history(session_id, None)
            .await
            .map(|t| t.len())
            .unwrap_or(before);

        let filled = after.saturating_sub(before);
        if filled > 0 {
            report.rows_filled += filled;
            report.reconciled += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::session_store::file_backend::{FileSessionStore, FileSessionStoreConfig};
    use crate::gateway::session_store::types::MessageRecord;
    use crate::routing::session_key::SessionKey;
    use crate::session::events::{
        MessageContent, RunOutcome, SessionEvent, ToolOutput, TurnId, TurnTrigger,
    };
    use crate::session::store::{migrate_add_session_events, SqliteEventStore};

    fn mem_event_store() -> Arc<dyn SessionEventStore> {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        migrate_add_session_events(&conn).unwrap();
        Arc::new(SqliteEventStore::new(conn))
    }

    fn temp_file_store() -> (Arc<dyn SessionStore>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let config = FileSessionStoreConfig {
            base_dir: dir.path().to_path_buf(),
            ..Default::default()
        };
        (Arc::new(FileSessionStore::new(config).unwrap()), dir)
    }

    fn mc(text: &str) -> MessageContent {
        MessageContent {
            text: text.into(),
            blocks: vec![],
            thinking: None,
            thinking_signature: None,
        }
    }

    /// A minimal interrupted-run log: TurnStarted, UserMessage, RunStarted,
    /// LlmCallStarted/Ended(tin,tout), AssistantMessage — and NO RunFinished.
    fn interrupted_turn(tid: TurnId, tin: u32, tout: u32) -> Vec<(u64, SessionEvent)> {
        vec![
            (
                1,
                SessionEvent::TurnStarted {
                    turn_id: tid,
                    trigger: TurnTrigger::UserMessage,
                    at: 1,
                },
            ),
            (
                2,
                SessionEvent::UserMessage {
                    turn_id: tid,
                    content: mc("hi"),
                    at: 2,
                    synthetic: false,
                },
            ),
            (
                3,
                SessionEvent::RunStarted {
                    run_id: "r1".into(),
                    at: 3,
                    project_root: None,
                },
            ),
            (
                4,
                SessionEvent::LlmCallStarted {
                    turn_id: tid,
                    provider: "anthropic".into(),
                    model: "claude".into(),
                    at: 4,
                },
            ),
            (
                5,
                SessionEvent::LlmCallEnded {
                    turn_id: tid,
                    tokens_in: tin,
                    tokens_out: tout,
                    finish_reason: "stop".into(),
                    at: 5,
                },
            ),
            (
                6,
                SessionEvent::AssistantMessage {
                    turn_id: tid,
                    content: mc("hello"),
                    at: 6,
                },
            ),
        ]
    }

    async fn append_all(
        store: &Arc<dyn SessionEventStore>,
        id: &SessionId,
        evs: &[(u64, SessionEvent)],
    ) {
        for (seq, ev) in evs {
            store.append(id, *seq, ev, *seq as i64).await.unwrap();
        }
    }

    #[tokio::test]
    async fn fills_missing_tail_into_empty_transcript() {
        let event_store = mem_event_store();
        let (session_store, _dir) = temp_file_store();
        let id = SessionKey::ephemeral("recon-fill");
        session_store.get_or_create(&id).await.unwrap();
        append_all(
            &event_store,
            &id,
            &interrupted_turn(uuid::Uuid::new_v4(), 10, 20),
        )
        .await;

        let report = ProjectionReconciler::new(event_store.clone(), session_store.clone())
            .reconcile_interrupted()
            .await;

        assert_eq!(report.scanned, 1);
        assert_eq!(report.reconciled, 1);
        assert_eq!(report.rows_filled, 2, "user + assistant");
        let hist = session_store.get_history(&id, None).await.unwrap();
        assert_eq!(hist.len(), 2);
        assert_eq!(hist[0].role, "user");
        assert_eq!(hist[0].content, "hi");
        assert_eq!(hist[1].role, "assistant");
        assert_eq!(hist[1].content, "hello");
        assert_eq!(hist[1].input_tokens, 10);
        assert_eq!(hist[1].output_tokens, 20);
    }

    #[tokio::test]
    async fn reconcile_is_idempotent() {
        let event_store = mem_event_store();
        let (session_store, _dir) = temp_file_store();
        let id = SessionKey::ephemeral("recon-idem");
        session_store.get_or_create(&id).await.unwrap();
        append_all(
            &event_store,
            &id,
            &interrupted_turn(uuid::Uuid::new_v4(), 1, 1),
        )
        .await;

        let reconciler = ProjectionReconciler::new(event_store.clone(), session_store.clone());
        let r1 = reconciler.reconcile_interrupted().await;
        assert_eq!(r1.rows_filled, 2);
        let r2 = reconciler.reconcile_interrupted().await;
        assert_eq!(r2.rows_filled, 0, "second pass fills nothing");
        assert_eq!(r2.reconciled, 0);
        assert_eq!(
            session_store.get_history(&id, None).await.unwrap().len(),
            2,
            "no duplicate rows"
        );
    }

    #[tokio::test]
    async fn clean_session_is_skipped() {
        let event_store = mem_event_store();
        let (session_store, _dir) = temp_file_store();
        let id = SessionKey::ephemeral("recon-clean");
        session_store.get_or_create(&id).await.unwrap();
        let mut evs = interrupted_turn(uuid::Uuid::new_v4(), 1, 1);
        evs.push((
            7,
            SessionEvent::RunFinished {
                run_id: "r1".into(),
                outcome: RunOutcome::Completed,
                at: 7,
            },
        ));
        append_all(&event_store, &id, &evs).await;

        let report = ProjectionReconciler::new(event_store.clone(), session_store.clone())
            .reconcile_interrupted()
            .await;

        assert_eq!(report.skipped_clean, 1);
        assert_eq!(report.reconciled, 0);
        assert_eq!(report.rows_filled, 0);
        assert!(session_store
            .get_history(&id, None)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn legacy_transcript_without_seq_ids_is_skipped() {
        let event_store = mem_event_store();
        let (session_store, _dir) = temp_file_store();
        let id = SessionKey::ephemeral("recon-legacy");
        session_store.get_or_create(&id).await.unwrap();
        append_all(
            &event_store,
            &id,
            &interrupted_turn(uuid::Uuid::new_v4(), 1, 1),
        )
        .await;
        // Pre-existing legacy row with a non-seq id.
        session_store
            .append_message(
                &id,
                MessageRecord {
                    id: "legacy-row-1".into(),
                    role: "user".into(),
                    content: "old".into(),
                    timestamp: 0,
                    metadata: None,
                    input_tokens: 0,
                    output_tokens: 0,
                    model: None,
                    model_provider: None,
                    tool_call_id: None,
                    tool_name: None,
                },
            )
            .await
            .unwrap();

        let report = ProjectionReconciler::new(event_store.clone(), session_store.clone())
            .reconcile_interrupted()
            .await;

        assert_eq!(report.skipped_legacy, 1);
        assert_eq!(report.reconciled, 0);
        let hist = session_store.get_history(&id, None).await.unwrap();
        assert_eq!(hist.len(), 1, "legacy transcript untouched");
        assert_eq!(hist[0].id, "legacy-row-1");
    }

    #[tokio::test]
    async fn assistant_row_aggregates_multi_call_tokens() {
        let event_store = mem_event_store();
        let (session_store, _dir) = temp_file_store();
        let id = SessionKey::ephemeral("recon-tokens");
        session_store.get_or_create(&id).await.unwrap();
        let tid = uuid::Uuid::new_v4();
        let evs = vec![
            (
                1,
                SessionEvent::TurnStarted {
                    turn_id: tid,
                    trigger: TurnTrigger::UserMessage,
                    at: 1,
                },
            ),
            (
                2,
                SessionEvent::UserMessage {
                    turn_id: tid,
                    content: mc("q"),
                    at: 2,
                    synthetic: false,
                },
            ),
            (
                3,
                SessionEvent::RunStarted {
                    run_id: "r1".into(),
                    at: 3,
                    project_root: None,
                },
            ),
            (
                4,
                SessionEvent::LlmCallStarted {
                    turn_id: tid,
                    provider: "anthropic".into(),
                    model: "claude".into(),
                    at: 4,
                },
            ),
            (
                5,
                SessionEvent::LlmCallEnded {
                    turn_id: tid,
                    tokens_in: 10,
                    tokens_out: 20,
                    finish_reason: "tool_use".into(),
                    at: 5,
                },
            ),
            (
                6,
                SessionEvent::ToolCallRequested {
                    turn_id: tid,
                    call_id: "c1".into(),
                    name: "bash_exec".into(),
                    input: serde_json::json!({"cmd":"ls"}),
                    at: 6,
                },
            ),
            (
                7,
                SessionEvent::ToolResult {
                    turn_id: tid,
                    call_id: "c1".into(),
                    output: ToolOutput {
                        value: serde_json::json!("ok"),
                        metadata: Default::default(),
                    },
                    at: 7,
                },
            ),
            (
                8,
                SessionEvent::LlmCallStarted {
                    turn_id: tid,
                    provider: "anthropic".into(),
                    model: "claude".into(),
                    at: 8,
                },
            ),
            (
                9,
                SessionEvent::LlmCallEnded {
                    turn_id: tid,
                    tokens_in: 5,
                    tokens_out: 7,
                    finish_reason: "stop".into(),
                    at: 9,
                },
            ),
            (
                10,
                SessionEvent::AssistantMessage {
                    turn_id: tid,
                    content: mc("final"),
                    at: 10,
                },
            ),
        ];
        append_all(&event_store, &id, &evs).await;

        ProjectionReconciler::new(event_store.clone(), session_store.clone())
            .reconcile_interrupted()
            .await;

        let hist = session_store.get_history(&id, None).await.unwrap();
        // rows: user, tool(req), tool(res), assistant
        assert_eq!(hist.len(), 4, "user + 2 tool rows + assistant");
        let asst = hist.iter().find(|m| m.role == "assistant").unwrap();
        assert_eq!(asst.input_tokens, 15, "10 + 5 across both LLM calls");
        assert_eq!(asst.output_tokens, 27, "20 + 7 across both LLM calls");
    }

    #[tokio::test]
    async fn filled_rows_precede_later_appends() {
        let event_store = mem_event_store();
        let (session_store, _dir) = temp_file_store();
        let id = SessionKey::ephemeral("recon-order");
        session_store.get_or_create(&id).await.unwrap();
        append_all(
            &event_store,
            &id,
            &interrupted_turn(uuid::Uuid::new_v4(), 1, 1),
        )
        .await;

        ProjectionReconciler::new(event_store.clone(), session_store.clone())
            .reconcile_interrupted()
            .await;

        // A later append (mirrors ResumeCoordinator's re-triggered reply) must
        // land AFTER the back-filled rows.
        session_store
            .append_message(
                &id,
                MessageRecord {
                    id: format!("{}:99", id.to_key_string()),
                    role: "assistant".into(),
                    content: "fresh reply".into(),
                    timestamp: 100,
                    metadata: None,
                    input_tokens: 0,
                    output_tokens: 0,
                    model: None,
                    model_provider: None,
                    tool_call_id: None,
                    tool_name: None,
                },
            )
            .await
            .unwrap();

        let hist = session_store.get_history(&id, None).await.unwrap();
        assert_eq!(
            hist.first().unwrap().role,
            "user",
            "back-filled prompt is first"
        );
        assert_eq!(
            hist.last().unwrap().content,
            "fresh reply",
            "later append is last"
        );
    }

    #[test]
    fn parse_source_seq_accepts_projector_ids_only() {
        assert_eq!(
            parse_source_seq("agent:main:reflect:42", "agent:main:reflect"),
            Some(42)
        );
        assert_eq!(parse_source_seq("m-user-5", "agent:main"), None, "no colon");
        assert_eq!(
            parse_source_seq("other:7", "agent:main"),
            None,
            "prefix mismatch"
        );
        assert_eq!(
            parse_source_seq("agent:main:xyz", "agent:main"),
            None,
            "non-numeric suffix"
        );
    }
}
