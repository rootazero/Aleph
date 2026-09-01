//! `ProjectionReconciler` — boot-time events→messages back-fill for the
//! transcript projection.
//!
//! P1 made `session_events` the SSOT and materialised the `messages`
//! projection asynchronously via `MessageProjector`. On a hard crash *during* a
//! run, events durably in `session_events` may not have been drained to the
//! read projection — `transcript.jsonl` on the file backend, the `messages`
//! table on SQLite — and the Panel display loses those rows. This reconciler
//! runs at boot, asks [`crate::session::reduction::reduce_disposition`] about
//! the run markers it loads from the event store to find interrupted
//! sessions, and re-projects the un-materialised tail through the same
//! `project_event` the live drain uses.
//!
//! Scope (see `docs/superpowers/specs/2026-07-04-projection-reconciler-p2-design.md`):
//! interrupted runs only, no schema change — the source seq is recovered from
//! the `"{key}:{seq}"` id embedded in each projector-written transcript row.
//!
//! **Both backends.** This used to say "file backend only". The SQLite backend
//! stores the projector's seq in its own `source_seq` column and rebuilds the
//! same id through `projection::row_id` on read
//! (`session_manager/ops/crud.rs`), so `parse_source_seq` succeeds there too
//! and the back-fill covers it. A comment that names another module's
//! behaviour freezes that module without telling it; this one had already
//! drifted.
//!
//! **What it does NOT cover**: a row the live drain dropped under back-pressure
//! in a run that later finished cleanly. `reduce_disposition` calls that
//! session `Clean`, so this pass skips it and the row is gone from the display
//! for good. The trigger condition here is "the run was interrupted"; the
//! failure condition is "the projection has a gap", and the two are not the
//! same set. Fixing it needs a durable projection watermark — see
//! `docs/superpowers/specs/2026-08-31-run-reduction-design.md` §8.1.

use std::collections::HashSet;

use crate::gateway::session_projector::project_event;
use crate::gateway::session_store::SessionStore;
use crate::session::projection::parse_source_seq;
use crate::session::reduction::{reduce_disposition, RunDisposition};
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
            match reduce_disposition(&markers) {
                RunDisposition::Clean => report.skipped_clean += 1,
                RunDisposition::Interrupted { .. } => {
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
    /// transcript, replay the full event log, and materialise the rows above
    /// the watermark (writes suppressed for `seq <= watermark`).
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
            tracing::debug!(
                session = ?session_id,
                "projection reconcile: skipped (legacy transcript, no projector seq ids)"
            );
            report.skipped_legacy += 1;
            return;
        }

        let watermark = seqs.iter().copied().max().unwrap_or(0);

        // Replay the FULL event log, not just the tail above the watermark.
        // Writes are suppressed for seq <= watermark (those rows are already
        // materialised — including any mixed/legacy rows whose ids are not
        // projector seqs), so no duplicate rows are produced, and the
        // suppression check short-circuits before the per-event retirement read.
        let events = match self.event_store.load_all_events(session_id).await {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(session = ?session_id, error = %e, "projection reconcile: load_all_events failed; skipping");
                return;
            }
        };

        // Idempotent no-op when nothing sits above the watermark.
        if events.iter().all(|r| r.seq <= watermark) {
            return;
        }

        let before = transcript.len();
        for rec in &events {
            // No bus: a boot-time back-fill is replaying messages that were
            // typed in a previous process, so it must not announce them as
            // live (`session_projector::peer_echo_frame` refuses them on the
            // watermark alone — this is the structural half of the same rule).
            project_event(&self.session_store, session_id, rec, Some(watermark), None).await;
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

    /// An assistant message billed `tin`/`tout` — the shape `think.rs` emits,
    /// one per LLM call.
    fn assistant(tid: TurnId, tin: u32, tout: u32, at: i64) -> SessionEvent {
        SessionEvent::AssistantMessage {
            turn_id: tid,
            content: mc("hello"),
            usage: Some(crate::orchestrator::dispatch::TokenBreakdown {
                input: tin,
                output: tout,
                ..Default::default()
            }),
            at,
        }
    }

    /// A minimal interrupted-run log: TurnStarted, UserMessage, RunStarted,
    /// AssistantMessage(tin,tout) — and NO RunFinished.
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
                    author_user_id: None,
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
            (6, assistant(tid, tin, tout, 6)),
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
        assert_eq!(report.scanned, 1);
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

    /// A two-call turn (tool round-trip in the middle) produces TWO assistant
    /// rows, each carrying its own call's tokens.
    ///
    /// This replaces a test that asserted ONE assistant row aggregating both
    /// calls (15/27). That was never the shape production emits — `think.rs`
    /// writes an `AssistantMessage` per Think step — and the aggregation it
    /// checked was fed by `LlmCallEnded` events the test itself was the only
    /// source of.
    fn two_call_turn(tid: TurnId) -> Vec<(u64, SessionEvent)> {
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
                    content: mc("q"),
                    at: 2,
                    synthetic: false,
                    author_user_id: None,
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
            (4, assistant(tid, 10, 20, 4)),
            (
                5,
                SessionEvent::ToolCallRequested {
                    turn_id: tid,
                    call_id: "c1".into(),
                    name: "bash_exec".into(),
                    input: serde_json::json!({"cmd":"ls"}),
                    at: 5,
                },
            ),
            (
                6,
                SessionEvent::ToolResult {
                    turn_id: tid,
                    call_id: "c1".into(),
                    output: ToolOutput {
                        value: serde_json::json!("ok"),
                        metadata: Default::default(),
                    },
                    at: 6,
                },
            ),
            (7, assistant(tid, 5, 7, 7)),
        ]
    }

    #[tokio::test]
    async fn each_assistant_row_carries_its_own_calls_tokens() {
        let event_store = mem_event_store();
        let (session_store, _dir) = temp_file_store();
        let id = SessionKey::ephemeral("recon-tokens");
        session_store.get_or_create(&id).await.unwrap();
        append_all(&event_store, &id, &two_call_turn(uuid::Uuid::new_v4())).await;

        ProjectionReconciler::new(event_store.clone(), session_store.clone())
            .reconcile_interrupted()
            .await;

        let hist = session_store.get_history(&id, None).await.unwrap();
        assert_eq!(hist.len(), 5, "user + assistant + 2 tool rows + assistant");
        let asst: Vec<_> = hist.iter().filter(|m| m.role == "assistant").collect();
        assert_eq!(asst.len(), 2, "one row per LLM call");
        assert_eq!((asst[0].input_tokens, asst[0].output_tokens), (10, 20));
        assert_eq!((asst[1].input_tokens, asst[1].output_tokens), (5, 7));
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

    #[tokio::test]
    async fn back_filled_row_carries_its_tokens_across_the_watermark() {
        // A crash mid-turn: the first assistant row and both tool rows were
        // flushed (seqs 2,4,5,6 ⇒ watermark 6); the SECOND assistant row (seq 7)
        // was not. The back-filled row must arrive with its own call's tokens.
        //
        // This used to be a straddling-accumulator test — the watermark sat
        // between the turn's two `LlmCallEnded` events and the reconciler had to
        // replay below it to re-sum them. That whole hazard is gone: a row's
        // tokens now ride on the very event that produces the row, so there is
        // nothing left to carry across a watermark. What is still worth pinning
        // is that the back-fill neither loses the tokens nor duplicates the rows
        // already below it.
        let event_store = mem_event_store();
        let (session_store, _dir) = temp_file_store();
        let id = SessionKey::ephemeral("recon-straddle");
        session_store.get_or_create(&id).await.unwrap();
        let key = id.to_key_string();

        append_all(&event_store, &id, &two_call_turn(uuid::Uuid::new_v4())).await;

        // Simulate the partial flush: pre-materialise seqs 2, 4, 5, 6 with
        // projector-style ids. The seq-7 assistant row is intentionally missing.
        for (seq, role, content, tin, tout, tool_name, tool_call_id) in [
            (2u64, "user", "q", 0i64, 0i64, None, None),
            (4, "assistant", "hello", 10, 20, None, None),
            (
                5,
                "tool",
                "ls",
                0,
                0,
                Some("bash_exec".to_string()),
                Some("c1".to_string()),
            ),
            (
                6,
                "tool",
                "ok",
                0,
                0,
                Some("bash_exec".to_string()),
                Some("c1".to_string()),
            ),
        ] {
            session_store
                .append_message(
                    &id,
                    MessageRecord {
                        id: format!("{key}:{seq}"),
                        role: role.into(),
                        content: content.into(),
                        timestamp: seq as i64,
                        metadata: None,
                        input_tokens: tin,
                        output_tokens: tout,
                        tool_call_id,
                        tool_name,
                    },
                )
                .await
                .unwrap();
        }

        ProjectionReconciler::new(event_store.clone(), session_store.clone())
            .reconcile_interrupted()
            .await;

        let hist = session_store.get_history(&id, None).await.unwrap();
        assert_eq!(
            hist.len(),
            5,
            "4 pre-flushed + 1 back-filled, no duplicates"
        );
        let asst: Vec<_> = hist.iter().filter(|m| m.role == "assistant").collect();
        assert_eq!(asst.len(), 2);
        assert_eq!(
            (asst[0].input_tokens, asst[0].output_tokens),
            (10, 20),
            "row below the watermark must be left exactly as it was"
        );
        assert_eq!(
            (asst[1].input_tokens, asst[1].output_tokens),
            (5, 7),
            "back-filled row must arrive with its own call's tokens"
        );
    }
}
