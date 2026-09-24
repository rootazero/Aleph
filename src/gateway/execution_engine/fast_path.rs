use super::{ExecutionEngine, ExecutionError, RunRequest, RunState};
use crate::gateway::agent_instance::AgentInstance;
use crate::gateway::event_emitter::{EventEmitter, RunSummary, StreamEvent};
use crate::resilience::TaskStatus;
use crate::session::events::{
    now_ms, EventSeq, MessageContent, RunEnvelopeSnapshot, RunOutcome, SessionEvent, Timestamp,
    ToolOutput, TurnId,
};
use crate::session::service::{SessionError, SessionId, SessionService};
use crate::sync_primitives::Arc;
use tracing::warn;

/// The L0 fast path's session journal — the same shape the harness leaves:
/// `[TurnStarted, UserMessage, RunStarted{envelope}, ToolCallRequested]`
/// BEFORE the tool runs (one batch; its durability is the store's policy
/// over its members, not a choice made here), `[ToolResult|ToolError,
/// AssistantMessage, RunFinished]` after. A crash between the two reads
/// `Interrupted` with one dangling call, and the existing three-arm repair
/// tells the model the outcome is unknown (§5.3) — no second run shape.
///
/// Before this, the fast path wrote its `UserMessage` + `AssistantMessage`
/// AFTER the tool ran, as two rows with no markers: a crash mid-tool left no
/// trace of the dispatch at all, and the reducer read a clean log.
pub(super) struct FastPathJournal {
    svc: Arc<dyn SessionService>,
    session: SessionId,
    pub(super) turn_id: TurnId,
    pub(super) run_id: String,
    pub(super) call_id: String,
    tool: String,
}

impl FastPathJournal {
    pub(super) fn new(svc: Arc<dyn SessionService>, session: SessionId, tool: &str) -> Self {
        Self {
            svc,
            session,
            turn_id: TurnId::new_v4(),
            // The one marker id still minted locally: a slash-command turn is not
            // an engine run — no `execute()`, no `AssistantRunMeta` — so there is
            // nothing for this bracket's id to join (spec 2026-09-24 §2).
            run_id: format!("slash-{}", uuid::Uuid::new_v4()),
            call_id: format!("slash-{}", uuid::Uuid::new_v4()),
            tool: tool.to_string(),
        }
    }

    /// The open batch: the seed pair, the run marker with the envelope the
    /// gate resolved, and the dispatch. Written before the tool executes.
    pub(super) async fn open(
        &self,
        text: String,
        author_user_id: Option<String>,
        envelope: RunEnvelopeSnapshot,
        input: serde_json::Value,
    ) -> Result<Vec<EventSeq>, SessionError> {
        // One instant for the whole batch: it is one moment, and four
        // `now_ms()` calls can straddle a millisecond.
        let at = now_ms();
        let [turn, user] = SessionEvent::user_turn(
            self.turn_id,
            MessageContent {
                text,
                blocks: Vec::new(),
                thinking: None,
                thinking_signature: None,
            },
            author_user_id,
            at,
        );
        let events = vec![
            turn,
            user,
            SessionEvent::RunStarted {
                run_id: self.run_id.clone(),
                at,
                project_root: None,
                envelope: Some(envelope),
            },
            SessionEvent::ToolCallRequested {
                turn_id: self.turn_id,
                call_id: self.call_id.clone(),
                name: self.tool.clone(),
                input,
                at,
            },
        ];
        self.svc.emit_batch(&self.session, events, None).await
    }

    /// The close batch after a tool that returned: its receipt, the reply the
    /// user saw, `RunFinished { Completed }`.
    pub(super) async fn close_ok(
        &self,
        result: serde_json::Value,
        reply: String,
    ) -> Result<Vec<EventSeq>, SessionError> {
        let receipt = |at| SessionEvent::ToolResult {
            turn_id: self.turn_id,
            call_id: self.call_id.clone(),
            output: ToolOutput {
                value: result,
                metadata: Default::default(),
            },
            at,
        };
        self.close(receipt, reply, RunOutcome::Completed).await
    }

    /// The close batch after a tool that failed in-process: its error
    /// receipt, the error echo the user saw, `RunFinished { Errored }`. The
    /// run is never left open on an in-process failure — only a process
    /// death leaves it open.
    pub(super) async fn close_err(
        &self,
        error: String,
        reply: String,
    ) -> Result<Vec<EventSeq>, SessionError> {
        let receipt = |at| SessionEvent::ToolError {
            turn_id: self.turn_id,
            call_id: self.call_id.clone(),
            error,
            at,
        };
        self.close(receipt, reply, RunOutcome::Errored).await
    }

    /// `receipt` is built here rather than by the caller so the receipt, the
    /// reply and the marker carry the batch's one instant.
    async fn close(
        &self,
        receipt: impl FnOnce(Timestamp) -> SessionEvent,
        reply: String,
        outcome: RunOutcome,
    ) -> Result<Vec<EventSeq>, SessionError> {
        let at = now_ms();
        let events = vec![
            receipt(at),
            SessionEvent::AssistantMessage {
                turn_id: self.turn_id,
                content: MessageContent {
                    text: reply,
                    blocks: Vec::new(),
                    thinking: None,
                    thinking_signature: None,
                },
                // Slash-command reply — no LLM call, nothing billed.
                usage: None,
                at,
            },
            SessionEvent::RunFinished {
                run_id: self.run_id.clone(),
                outcome,
                at,
            },
        ];
        self.svc.emit_batch(&self.session, events, None).await
    }
}

impl<P, R> ExecutionEngine<P, R>
where
    P: crate::thinker::ProviderRegistry + 'static,
    R: crate::executor::ToolRegistry + 'static,
{
    /// Finalize a successful slash command fast-path execution.
    pub(super) async fn finalize_fast_path_success<E: EventEmitter + Send + Sync + 'static>(
        &self,
        run_id: &str,
        request: &RunRequest,
        agent: &AgentInstance,
        emitter: &crate::sync_primitives::Arc<E>,
        response: String,
        task_persisted: bool,
    ) -> Result<(), ExecutionError> {
        let (started_at, steps_completed, final_seq) = {
            let mut runs = self.active_runs.write().await;
            if let Some(run) = runs.get_mut(run_id) {
                run.state = RunState::Completed;
                run.completed_at = Some(chrono::Utc::now());
                run.cancel_tx = None;
                (run.started_at, run.steps_completed, run.next_seq())
            } else {
                (chrono::Utc::now(), 0, 0)
            }
        };

        // Session claim + concurrency permit release: handled by `_run_slot`'s
        // `Drop` when `execute()` returns right after this call (Task 6 — the
        // per-agent `AgentState::Idle` reset that used to sit here is gone).
        let duration_ms = (chrono::Utc::now() - started_at).num_milliseconds().max(0) as u64;
        if task_persisted {
            self.persist_run_task_status(run_id, TaskStatus::Completed)
                .await;
        }

        // The transcript rows (user command, tool receipt, reply, run markers)
        // are the `FastPathJournal`'s, written in `execute_direct_tool` — the
        // open batch before the tool ran, the close batch right after. Nothing
        // is appended here.
        let _ = emitter
            .emit(StreamEvent::RunComplete {
                run_id: run_id.to_string(),
                seq: final_seq,
                summary: RunSummary {
                    // 0 is correct: the L0 slash-command fast path bypasses
                    // the agent loop and makes no LLM call (commands that
                    // need an LLM fall through to the loop instead).
                    total_tokens: 0,
                    tool_calls: 1,
                    loops: steps_completed,
                    final_response: Some(response),
                    ..Default::default()
                },
                total_duration_ms: duration_ms,
            })
            .await;
        self.publish_session_updated(
            &request.session_key,
            request.metadata.get("channel_id").map(String::as_str),
            &request.run_id,
        );

        // Clear the session-level "running" marker now that the final message
        // has been persisted.
        agent.set_session_idle(&request.session_key).await;

        // Remove from active runs after a short delay (same as normal path)
        let runs_clone = self.active_runs.clone();
        let run_id_owned = run_id.to_string();
        tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            runs_clone.write().await.remove(&run_id_owned);
        });

        Ok(())
    }

    /// Finalize a failed slash command fast-path execution (non-fallthrough error).
    pub(super) async fn finalize_fast_path_error<E: EventEmitter + Send + Sync + 'static>(
        &self,
        run_id: &str,
        request: &RunRequest,
        agent: &AgentInstance,
        emitter: &crate::sync_primitives::Arc<E>,
        error_msg: &str,
        task_persisted: bool,
    ) -> Result<(), ExecutionError> {
        let (started_at, final_seq) = {
            let mut runs = self.active_runs.write().await;
            if let Some(run) = runs.get_mut(run_id) {
                run.state = RunState::Failed {
                    error: error_msg.to_string(),
                };
                run.completed_at = Some(chrono::Utc::now());
                run.cancel_tx = None;
                (run.started_at, run.next_seq())
            } else {
                (chrono::Utc::now(), 0)
            }
        };

        // Session claim + concurrency permit release: handled by `_run_slot`'s
        // `Drop` when `execute()` returns right after this call (Task 6 — the
        // per-agent `AgentState::Idle` reset that used to sit here is gone).
        let duration_ms = (chrono::Utc::now() - started_at).num_milliseconds().max(0) as u64;
        if task_persisted {
            self.persist_run_task_status(run_id, TaskStatus::Failed)
                .await;
        }
        let error_response = format!("❌ {error_msg}");

        // Transcript rows: the `FastPathJournal`'s (see the success twin).
        // When the failure came from the tool, `execute_direct_tool` already
        // wrote the close batch (`ToolError`, this same echo, `RunFinished {
        // Errored }`). When it came from one of the three pre-dispatch
        // `Failed` arms (invalid mode JSON, missing `tool_id`, unknown mode
        // type — all unreachable through `try_resolve_slash_command` and the
        // channel router's `serialize_parsed_command`), or from the open
        // batch itself being refused (one transaction: nothing landed), no
        // run was ever opened and this turn leaves NO transcript row: the
        // user still gets the echo below via `ResponseChunk` + `RunComplete`.
        // A missing row, never a wrong one (criterion #17).
        let _ = emitter
            .emit(StreamEvent::ResponseChunk {
                run_id: run_id.to_string(),
                seq: 1,
                delta: error_response.clone(),
                full_text: String::new(),
                chunk_index: 0,
                is_final: true,
                is_intermediate: false,
            })
            .await;
        let _ = emitter
            .emit(StreamEvent::RunComplete {
                run_id: run_id.to_string(),
                seq: final_seq,
                summary: RunSummary {
                    // 0 is correct: the L0 slash-command fast path bypasses
                    // the agent loop and makes no LLM call.
                    total_tokens: 0,
                    tool_calls: 1,
                    loops: 0,
                    final_response: Some(error_response),
                    ..Default::default()
                },
                total_duration_ms: duration_ms,
            })
            .await;
        self.publish_session_updated(
            &request.session_key,
            request.metadata.get("channel_id").map(String::as_str),
            &request.run_id,
        );
        warn!(
            run_id = %run_id,
            error = %error_msg,
            "Slash command fast path failed, returning error to user"
        );

        // Clear the session-level "running" marker now that the error receipt
        // has been persisted.
        agent.set_session_idle(&request.session_key).await;

        // Remove from active runs after a short delay (same as normal path)
        let runs_clone = self.active_runs.clone();
        let run_id_owned = run_id.to_string();
        tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            runs_clone.write().await.remove(&run_id_owned);
        });

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::FastPathJournal;
    use crate::routing::session_key::SessionKey;
    use crate::session::events::{RunEnvelopeSnapshot, RunOutcome, SessionEvent, Timestamp};
    use crate::session::store::{event_type_tag, migrate_add_session_events, SqliteEventStore};
    use crate::session::{
        reduce_run, DanglingProvenance, InProcessActorSessionService, RunDisposition,
        SessionEventRecord, SessionEventStore, SessionService,
    };
    use crate::sync_primitives::Arc;

    /// A real actor service over a real in-memory `SqliteEventStore` — the
    /// production transcript path, as `execution_engine/tests.rs` builds it.
    fn svc() -> (Arc<dyn SessionService>, Arc<dyn SessionEventStore>) {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        migrate_add_session_events(&conn).unwrap();
        let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
        (
            Arc::new(InProcessActorSessionService::new(store.clone())),
            store,
        )
    }

    fn kinds(log: &[SessionEventRecord]) -> Vec<&'static str> {
        log.iter().map(|r| event_type_tag(&r.event)).collect()
    }

    /// The payload `at` of every kind the journal writes.
    fn at_of(event: &SessionEvent) -> Timestamp {
        match event {
            SessionEvent::TurnStarted { at, .. }
            | SessionEvent::UserMessage { at, .. }
            | SessionEvent::RunStarted { at, .. }
            | SessionEvent::ToolCallRequested { at, .. }
            | SessionEvent::ToolResult { at, .. }
            | SessionEvent::ToolError { at, .. }
            | SessionEvent::AssistantMessage { at, .. }
            | SessionEvent::RunFinished { at, .. } => *at,
            other => panic!("the journal never writes {other:?}"),
        }
    }

    /// One batch, one instant (the T3/T4 ruling): every row of a batch
    /// carries the same payload `at`.
    fn assert_one_instant(rows: &[SessionEventRecord]) {
        let stamps: Vec<Timestamp> = rows.iter().map(|r| at_of(&r.event)).collect();
        assert!(
            stamps.windows(2).all(|w| w[0] == w[1]),
            "a batch is one moment; got {stamps:?}"
        );
    }

    /// §5.3: the fact lands before the effect. The open batch is ONE batch
    /// (four consecutive seqs), it has the run's shape, and a process death
    /// right after it reads back as an interrupted run with one dangling
    /// call — exactly what the boundary repair already knows how to tell the
    /// model. Nothing here is a second run shape.
    #[tokio::test]
    async fn the_open_batch_is_durable_before_the_tool_runs_and_reads_interrupted_if_it_never_closes(
    ) {
        let (svc, store) = svc();
        let sid = SessionKey::main("fp");
        let j = FastPathJournal::new(svc, sid.clone(), "select_model");
        let seqs = j
            .open(
                "/model x".into(),
                None,
                RunEnvelopeSnapshot::default(),
                serde_json::json!({"model": "x"}),
            )
            .await
            .unwrap();
        assert_eq!(seqs, vec![1, 2, 3, 4]);
        let log = store.load_all_events(&sid).await.unwrap();
        assert_eq!(
            kinds(&log),
            [
                "turn_started",
                "user_message",
                "run_started",
                "tool_call_requested"
            ]
        );
        // <- the process dies here: the same shape the harness leaves.
        let r = reduce_run(&log).unwrap();
        assert_eq!(r.disposition, RunDisposition::Interrupted { attempts: 0 });
        assert_eq!(r.dangling.len(), 1);
        assert_eq!(r.dangling[0].tool_name, "select_model");
        assert_eq!(r.dangling[0].call_id, j.call_id);
        assert_eq!(r.dangling[0].provenance, DanglingProvenance::ThisRestart);
    }

    /// The four seed rows share the journal's one `turn_id`, the marker
    /// carries the envelope the gate resolved, and the dispatch carries the
    /// tool's real input — the run's shape, not a lookalike.
    #[tokio::test]
    async fn the_open_batch_carries_one_turn_the_envelope_and_the_input() {
        let (svc, store) = svc();
        let sid = SessionKey::main("fp-shape");
        let j = FastPathJournal::new(svc, sid.clone(), "session_rename");
        let envelope = RunEnvelopeSnapshot {
            exec_tier: Some("full".into()),
            session_mode: Some("code".into()),
            ..RunEnvelopeSnapshot::default()
        };
        j.open(
            "/rename topic".into(),
            Some("u-alice".into()),
            envelope.clone(),
            serde_json::json!({"topic": "topic"}),
        )
        .await
        .unwrap();
        let log = store.load_all_events(&sid).await.unwrap();
        assert_one_instant(&log[..4]);
        match &log[0].event {
            SessionEvent::TurnStarted { turn_id, .. } => assert_eq!(*turn_id, j.turn_id),
            other => panic!("expected TurnStarted, got {other:?}"),
        }
        match &log[1].event {
            SessionEvent::UserMessage {
                turn_id,
                content,
                synthetic,
                author_user_id,
                ..
            } => {
                assert_eq!(*turn_id, j.turn_id);
                assert_eq!(content.text, "/rename topic");
                assert!(!synthetic, "the user typed this");
                assert_eq!(author_user_id.as_deref(), Some("u-alice"));
            }
            other => panic!("expected UserMessage, got {other:?}"),
        }
        match &log[2].event {
            SessionEvent::RunStarted {
                run_id,
                envelope: e,
                ..
            } => {
                assert_eq!(*run_id, j.run_id);
                assert_eq!(e.as_ref(), Some(&envelope));
            }
            other => panic!("expected RunStarted, got {other:?}"),
        }
        match &log[3].event {
            SessionEvent::ToolCallRequested {
                turn_id,
                call_id,
                name,
                input,
                ..
            } => {
                assert_eq!(*turn_id, j.turn_id);
                assert_eq!(*call_id, j.call_id);
                assert_eq!(name, "session_rename");
                assert_eq!(*input, serde_json::json!({"topic": "topic"}));
            }
            other => panic!("expected ToolCallRequested, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_closed_fast_path_reduces_clean_with_a_receipt_and_an_answer() {
        let (svc, store) = svc();
        let sid = SessionKey::main("fp2");
        let j = FastPathJournal::new(svc, sid.clone(), "session_rename");
        j.open(
            "/rename x".into(),
            None,
            RunEnvelopeSnapshot::default(),
            serde_json::json!({"topic": "x"}),
        )
        .await
        .unwrap();
        let seqs = j
            .close_ok(serde_json::json!({"message": "renamed"}), "renamed".into())
            .await
            .unwrap();
        assert_eq!(seqs, vec![5, 6, 7]);
        let log = store.load_all_events(&sid).await.unwrap();
        assert_eq!(
            &kinds(&log)[4..],
            ["tool_result", "assistant_message", "run_finished"]
        );
        assert_one_instant(&log[4..]);
        match &log[6].event {
            SessionEvent::RunFinished {
                run_id, outcome, ..
            } => {
                assert_eq!(*run_id, j.run_id);
                assert_eq!(*outcome, RunOutcome::Completed);
            }
            other => panic!("expected RunFinished, got {other:?}"),
        }
        let r = reduce_run(&log).unwrap();
        assert_eq!(r.disposition, RunDisposition::Clean);
        assert!(r.dangling.is_empty() && r.contradictions.is_empty());
        assert_eq!(r.progress.tool_calls_answered, 1);
    }

    /// An in-process tool failure still closes the run: a `ToolError`
    /// receipt, the error echo the user saw, and `RunFinished { Errored }`.
    /// Only a process death leaves the dispatch open.
    #[tokio::test]
    async fn a_failed_fast_path_closes_errored_and_reduces_clean() {
        let (svc, store) = svc();
        let sid = SessionKey::main("fp3");
        let j = FastPathJournal::new(svc, sid.clone(), "session_rename");
        j.open(
            "/rename x".into(),
            None,
            RunEnvelopeSnapshot::default(),
            serde_json::json!({"topic": "x"}),
        )
        .await
        .unwrap();
        j.close_err("boom".into(), "❌ boom".into()).await.unwrap();
        let log = store.load_all_events(&sid).await.unwrap();
        assert_eq!(
            &kinds(&log)[4..],
            ["tool_error", "assistant_message", "run_finished"]
        );
        assert_one_instant(&log[4..]);
        match &log[4].event {
            SessionEvent::ToolError { call_id, error, .. } => {
                assert_eq!(*call_id, j.call_id);
                assert_eq!(error, "boom");
            }
            other => panic!("expected ToolError, got {other:?}"),
        }
        match &log[5].event {
            SessionEvent::AssistantMessage { content, usage, .. } => {
                assert_eq!(content.text, "❌ boom");
                assert!(usage.is_none(), "no LLM call, nothing billed");
            }
            other => panic!("expected AssistantMessage, got {other:?}"),
        }
        match &log[6].event {
            SessionEvent::RunFinished { outcome, .. } => {
                assert_eq!(*outcome, RunOutcome::Errored);
            }
            other => panic!("expected RunFinished, got {other:?}"),
        }
        let r = reduce_run(&log).unwrap();
        assert_eq!(r.disposition, RunDisposition::Clean);
        assert!(r.dangling.is_empty() && r.contradictions.is_empty());
        assert_eq!(r.progress.tool_calls_answered, 1);
    }
}
