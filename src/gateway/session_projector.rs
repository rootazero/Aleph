//! `MessageProjector` — a [`SessionEventObserver`] that materialises session
//! events into the `messages` table via a single ordered async drain task.
//!
//! The projector aggregates token counts and model name across the
//! `LlmCallStarted` / `LlmCallEnded` events for a turn, then writes the
//! assistant row with those accumulated values when `AssistantMessage` fires.
//!
//! The observer itself is **non-blocking**: `on_appended` enqueues the event
//! onto an mpsc channel and returns immediately. On back-pressure (or an
//! unclean shutdown between an event's durable append to `session_events` and
//! its drain to `messages`) the event is dropped from this projection.
//!
//! Consistency model: `session_events` is the single source of truth and is
//! unaffected — the agent replays the event log in full, so **recovery of the
//! agent's context is complete**. The `messages` table is an *eventually
//! consistent* read projection for the Panel: a turn written to the log just
//! before a crash may be absent from the Panel display until re-materialised.
//! P1 has no events→messages reconciliation pass; a boot-time reconciler
//! (project any event whose seq exceeds the last-materialised row per session,
//! keyed off a per-row source-seq watermark) is a P2 follow-up. Panel display
//! is therefore best-effort; the SSOT is not.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::mpsc;

use crate::gateway::session_store::types::MessageRecord;
use crate::gateway::session_store::SessionStore;
use crate::session::events::{SessionEvent, SessionEventRecord, TurnId};
use crate::session::observer::SessionEventObserver;
use crate::session::projection::{project_row, row_id};
use crate::session::service::SessionId;

/// Capacity of the internal mpsc channel between the observer and the drain task.
const QUEUE_CAP: usize = 4096;

/// Per-turn accumulator for token counts and model identity.
#[derive(Default)]
pub(crate) struct TurnAccum {
    model: Option<String>,
    provider: Option<String>,
    tin: i64,
    tout: i64,
}

/// Per-turn token/model accumulator map, keyed by `(session_key, turn_id)`.
/// Shared by the live drain and the boot-time `ProjectionReconciler`.
pub(crate) type TurnAccums = HashMap<(String, TurnId), TurnAccum>;

/// Materialises a session event stream into the `messages` store.
pub struct MessageProjector {
    tx: mpsc::Sender<(SessionId, SessionEventRecord)>,
}

impl MessageProjector {
    /// Create a new projector and spawn its drain task.
    ///
    /// The returned `Arc<MessageProjector>` implements [`SessionEventObserver`]
    /// and can be injected at boot (Task 5).
    pub fn new(store: Arc<dyn SessionStore>) -> Arc<Self> {
        let (tx, mut rx) = mpsc::channel::<(SessionId, SessionEventRecord)>(QUEUE_CAP);
        tokio::spawn(async move {
            let mut accums: TurnAccums = TurnAccums::new();
            while let Some((id, rec)) = rx.recv().await {
                project_event(&store, &mut accums, &id, &rec, None).await;
            }
        });
        Arc::new(Self { tx })
    }
}

/// True when this event was retired (`chat.clear` / `chat.rewind`) after it was
/// enqueued — the drain is asynchronous, so a queued event can be retired
/// *before* it reaches `messages`, and writing it then would silently un-clear
/// the conversation the user just cleared.
///
/// Fails closed: an unreadable event log is reported as retired, so the failure
/// mode is a missing projection row (best-effort display, back-filled by
/// `ProjectionReconciler`) rather than resurrected content.
async fn event_retired(id: &SessionId, seq: u64) -> bool {
    match crate::session::store::is_event_retired(id, seq).await {
        Ok(retired) => retired,
        Err(e) => {
            tracing::warn!(
                session = ?id,
                seq,
                error = %e,
                "projector: retirement check failed; skipping row (fail-closed)"
            );
            true
        }
    }
}

/// Project one session event into `store` — the single source of projection
/// truth shared by the live drain (`materialized_through = None`) and the
/// boot-time `ProjectionReconciler` (`materialized_through = Some(watermark)`).
///
/// A row-producing event whose seq has been RETIRED is suppressed the same way,
/// so a clear/rewind that races the drain queue cannot re-materialise.
///
/// When `rec.seq <= materialized_through`, a row-producing event still advances
/// the accumulator but its WRITE is suppressed: that row is already in the
/// projection, so re-projecting it is a no-op (reconcile idempotency, and
/// dup-avoidance for mixed/legacy rows below the watermark). The reconciler
/// replays the *full* event log with the watermark set to the last materialised
/// source seq, so a turn's token accumulator sees every `LlmCall*` event even
/// when the turn straddles the watermark, while its earlier rows stay
/// suppressed.
pub(crate) async fn project_event(
    store: &Arc<dyn SessionStore>,
    accums: &mut TurnAccums,
    id: &SessionId,
    rec: &SessionEventRecord,
    materialized_through: Option<u64>,
) {
    let key = id.to_key_string();
    let suppress =
        materialized_through.is_some_and(|w| rec.seq <= w) || event_retired(id, rec.seq).await;
    match &rec.event {
        SessionEvent::LlmCallStarted {
            turn_id,
            provider,
            model,
            ..
        } => {
            let a = accums.entry((key, *turn_id)).or_default();
            a.model = Some(model.clone());
            a.provider = Some(provider.clone());
        }
        SessionEvent::LlmCallEnded {
            turn_id,
            tokens_in,
            tokens_out,
            ..
        } => {
            let a = accums.entry((key, *turn_id)).or_default();
            a.tin += *tokens_in as i64;
            a.tout += *tokens_out as i64;
        }
        SessionEvent::AssistantMessage {
            turn_id, content, ..
        } => {
            // Consume the turn's accumulator regardless of suppression so
            // accumulator state advances identically to the live drain.
            let a = accums.remove(&(key.clone(), *turn_id)).unwrap_or_default();
            if suppress {
                return;
            }
            if let Err(e) = store
                .append_message(
                    id,
                    MessageRecord {
                        id: row_id(&key, rec.seq),
                        role: "assistant".into(),
                        content: content.text.clone(),
                        timestamp: rec.created_at_ms,
                        metadata: None,
                        input_tokens: a.tin,
                        output_tokens: a.tout,
                        model: a.model,
                        model_provider: a.provider,
                        tool_call_id: None,
                        tool_name: None,
                    },
                )
                .await
            {
                tracing::warn!(error = %e, "projector assistant append failed");
            }
        }
        SessionEvent::AssistantRunMeta {
            run_id,
            context_tokens,
            context_window,
            total_tokens,
            input_tokens,
            output_tokens,
            cost_usd,
            ..
        } => {
            // A run-meta at/below the watermark already stamped its assistant
            // row during the live drain; the reconciler's full-log replay must
            // not re-stamp it (keeps suppression a complete no-op). The same
            // guard makes the spend accumulation below idempotent — a replay
            // must not bill the session twice for one run.
            if suppress {
                return;
            }
            let occupancy = crate::gateway::execution_engine::helpers::RunContextOccupancy {
                context_tokens: *context_tokens,
                context_window: *context_window,
                total_tokens: *total_tokens,
                input_tokens: *input_tokens,
                output_tokens: *output_tokens,
                cost_usd: *cost_usd,
            };
            if let Some(meta) = crate::gateway::agent_instance::build_message_metadata(
                Some(run_id),
                Some(occupancy),
            ) {
                if let Err(e) = store.stamp_last_assistant_metadata(id, &meta).await {
                    tracing::warn!(error = %e, "projector: stamp run-meta failed");
                }
            }

            // Accumulate this run's spend onto the session row. This resurrects
            // `update_session_usage`, which was written, tested, and never called
            // from production: its only feeder was `SessionEvent::LlmCallEnded`,
            // an event no production code has ever emitted. So the session's
            // token columns were permanently 0, and `estimated_cost_usd` had no
            // writer AND no column — yet both were surfaced to the model (the
            // `sessions` tool) and to the Panel, which read them as "this session
            // cost nothing".
            if *input_tokens > 0 || *output_tokens > 0 || cost_usd.is_some() {
                if let Err(e) = store
                    .update_session_usage(
                        id,
                        i64::from(*input_tokens),
                        i64::from(*output_tokens),
                        cost_usd.unwrap_or(0.0),
                        None,
                        None,
                    )
                    .await
                {
                    tracing::warn!(error = %e, "projector: session usage accumulation failed");
                }
            }
        }
        other => {
            if suppress {
                return;
            }
            if let Some(row) = project_row(other) {
                if let Err(e) = store
                    .append_message(
                        id,
                        MessageRecord {
                            id: row_id(&key, rec.seq),
                            role: row.role,
                            content: row.text,
                            timestamp: rec.created_at_ms,
                            metadata: None,
                            input_tokens: 0,
                            output_tokens: 0,
                            model: None,
                            model_provider: None,
                            tool_call_id: row.tool_call_id,
                            tool_name: row.tool_name,
                        },
                    )
                    .await
                {
                    tracing::warn!(error = %e, "projector append failed");
                }
            }
        }
    }
}

impl SessionEventObserver for MessageProjector {
    fn on_appended(&self, id: &SessionId, record: &SessionEventRecord) {
        match self.tx.try_send((id.clone(), record.clone())) {
            Ok(()) => {}
            // Expected back-pressure. The event stays in the SSOT log (agent
            // recovery unaffected); the Panel projection is eventually
            // consistent and may lag until a P2 reconciler catches it up.
            Err(mpsc::error::TrySendError::Full(_)) => {
                tracing::warn!(
                    session = ?id,
                    seq = record.seq,
                    "projector queue full; dropping (Panel eventually-consistent; SSOT intact)"
                );
            }
            // The drain task has stopped/panicked — a real incident, not routine
            // back-pressure. Surface it as an error so it is not masked as "full".
            Err(mpsc::error::TrySendError::Closed(_)) => {
                tracing::error!(
                    session = ?id,
                    seq = record.seq,
                    "projector drain task stopped; event lost"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::session_manager::{SessionManager, SessionManagerConfig};
    use crate::session::events::{EventSeq, MessageContent, ToolOutput};
    use std::time::Duration;
    use tempfile::tempdir;

    fn rec(seq: EventSeq, ev: SessionEvent) -> SessionEventRecord {
        SessionEventRecord {
            seq,
            event: ev,
            created_at_ms: 0,
        }
    }

    fn msg_content(text: &str) -> MessageContent {
        MessageContent {
            text: text.into(),
            blocks: vec![],
            thinking: None,
            thinking_signature: None,
        }
    }

    fn user_msg(tid: TurnId) -> SessionEvent {
        SessionEvent::UserMessage {
            turn_id: tid,
            content: msg_content("hi"),
            at: 0,
            synthetic: false,
        }
    }

    fn assistant_msg(tid: TurnId) -> SessionEvent {
        SessionEvent::AssistantMessage {
            turn_id: tid,
            content: msg_content("hello"),
            at: 0,
        }
    }

    fn tool_req(tid: TurnId) -> SessionEvent {
        SessionEvent::ToolCallRequested {
            turn_id: tid,
            call_id: "c1".into(),
            name: "bash_exec".into(),
            input: serde_json::json!({"cmd": "ls"}),
            at: 0,
        }
    }

    fn tool_res(tid: TurnId) -> SessionEvent {
        SessionEvent::ToolResult {
            turn_id: tid,
            call_id: "c1".into(),
            output: ToolOutput {
                value: serde_json::json!("ok"),
                metadata: Default::default(),
            },
            at: 0,
        }
    }

    async fn poll_history(
        store: &Arc<dyn SessionStore>,
        id: &SessionId,
        want: usize,
        timeout: Duration,
    ) -> Vec<MessageRecord> {
        let iters = (timeout.as_millis() / 20).max(1) as usize;
        for _ in 0..iters {
            let rows = store.get_history(id, None).await.unwrap_or_default();
            if rows.len() >= want {
                return rows;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        store.get_history(id, None).await.unwrap_or_default()
    }

    #[tokio::test]
    async fn projector_stamps_run_meta_on_assistant_row() {
        let temp = tempdir().unwrap();
        let config = SessionManagerConfig {
            db_path: temp.path().join("proj_meta.db"),
            max_messages: 10_000,
            compaction_keep: 5_000,
            ..Default::default()
        };
        let manager = SessionManager::new(config).unwrap();
        let id = SessionId::ephemeral("proj_meta");
        manager.get_or_create(&id).await.unwrap();

        let store: Arc<dyn SessionStore> = Arc::new(manager);
        let projector = MessageProjector::new(store.clone());

        let tid = uuid::Uuid::new_v4();
        let events: &[(EventSeq, SessionEvent)] = &[
            (1, user_msg(tid)),
            (
                2,
                SessionEvent::LlmCallStarted {
                    turn_id: tid,
                    provider: "anthropic".into(),
                    model: "claude".into(),
                    at: 1,
                },
            ),
            (
                3,
                SessionEvent::LlmCallEnded {
                    turn_id: tid,
                    tokens_in: 100,
                    tokens_out: 50,
                    finish_reason: "stop".into(),
                    at: 2,
                },
            ),
            (4, assistant_msg(tid)),
            (
                5,
                SessionEvent::AssistantRunMeta {
                    turn_id: tid,
                    run_id: "run_xyz".into(),
                    context_tokens: 1234,
                    context_window: 200_000,
                    total_tokens: 5678,
                    input_tokens: 4000,
                    output_tokens: 1678,
                    cost_usd: Some(0.12),
                    at: 3,
                },
            ),
        ];
        for (seq, ev) in events {
            projector.on_appended(&id, &rec(*seq, ev.clone()));
        }

        // Wait for the assistant row to appear (user + assistant = 2 rows).
        let _ = poll_history(&store, &id, 2, Duration::from_secs(2)).await;

        // Poll until the metadata stamp is visible (the drain task processes
        // AssistantRunMeta immediately after AssistantMessage, but the two DB
        // writes are async so give it a few extra cycles).
        let asst = {
            let mut found = None;
            for _ in 0..30 {
                let msgs = store.get_history(&id, None).await.unwrap_or_default();
                if let Some(row) = msgs
                    .into_iter()
                    .find(|m| m.role == "assistant" && m.metadata.is_some())
                {
                    found = Some(row);
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            found.expect("assistant row with metadata must appear")
        };

        let meta = asst.metadata.as_ref().unwrap();
        assert_eq!(
            meta.get("run_id").and_then(|v| v.as_str()),
            Some("run_xyz"),
            "run_id mismatch"
        );
        // build_message_metadata stores occupancy values as strings.
        assert_eq!(
            meta.get("context_tokens").and_then(|v| v.as_str()),
            Some("1234"),
            "context_tokens mismatch"
        );
        assert_eq!(
            meta.get("context_window").and_then(|v| v.as_str()),
            Some("200000"),
            "context_window mismatch"
        );
    }

    #[tokio::test]
    async fn projector_materializes_events_into_store_with_tokens() {
        let temp = tempdir().unwrap();
        let config = SessionManagerConfig {
            db_path: temp.path().join("proj.db"),
            max_messages: 10_000,
            compaction_keep: 5_000,
            ..Default::default()
        };
        let manager = SessionManager::new(config).unwrap();
        let id = SessionId::ephemeral("proj");
        manager.get_or_create(&id).await.unwrap();

        let store: Arc<dyn SessionStore> = Arc::new(manager);
        let projector = MessageProjector::new(store.clone());

        let tid = uuid::Uuid::new_v4();
        let events: [(EventSeq, SessionEvent); 6] = [
            (1, user_msg(tid)),
            (
                2,
                SessionEvent::LlmCallStarted {
                    turn_id: tid,
                    provider: "anthropic".into(),
                    model: "claude".into(),
                    at: 1,
                },
            ),
            (
                3,
                SessionEvent::LlmCallEnded {
                    turn_id: tid,
                    tokens_in: 10,
                    tokens_out: 20,
                    finish_reason: "stop".into(),
                    at: 2,
                },
            ),
            (4, assistant_msg(tid)),
            (5, tool_req(tid)),
            (6, tool_res(tid)),
        ];
        for (seq, ev) in events {
            projector.on_appended(&id, &rec(seq, ev));
        }

        // The drain task is async; poll until all rows appear or the timeout
        // elapses. The 6 events produce exactly 4 message rows (user + assistant
        // + tool_req + tool_res; the two Llm* events write no rows), so poll for
        // 4 — polling for more would always burn the full timeout.
        let msgs = poll_history(&store, &id, 4, Duration::from_secs(2)).await;

        assert_eq!(
            msgs.iter().filter(|m| m.role == "user").count(),
            1,
            "expected exactly 1 user row"
        );

        // Per-message token aggregation: the assistant row carries the turn's
        // accumulated `LlmCallEnded` tokens (the Panel token gauge feature).
        let asst = msgs
            .iter()
            .find(|m| m.role == "assistant")
            .expect("missing assistant row");
        assert_eq!(asst.input_tokens, 10, "assistant input_tokens mismatch");
        assert_eq!(asst.output_tokens, 20, "assistant output_tokens mismatch");

        // Model aggregation: `SessionManager`'s `messages` table has no
        // per-message model column (`get_history` returns `model: None`), so
        // model round-trips at the *session* level — which is what the Panel
        // token gauge actually reads. The projector forwards the aggregated
        // model into `append_message`, which UPDATEs `sessions.model`.
        let meta = store
            .get_metadata(&id)
            .await
            .unwrap()
            .expect("missing session metadata");
        assert_eq!(
            meta.model.as_deref(),
            Some("claude"),
            "session model mismatch (projector should aggregate LlmCallStarted model)"
        );
        assert_eq!(meta.input_tokens, 10, "session input_tokens mismatch");
        assert_eq!(meta.output_tokens, 20, "session output_tokens mismatch");

        assert!(
            msgs.iter()
                .any(|m| m.role == "tool" && m.tool_name.as_deref() == Some("bash_exec")),
            "missing tool row with tool_name=bash_exec"
        );
    }

    /// A turn is appended to the SSOT, the user clears it, and only *then* does
    /// the async drain reach those events. Before the write-time retirement
    /// check they were written into `messages` anyway — the clear silently
    /// un-clearing itself in the transcript milliseconds later.
    #[tokio::test]
    async fn clear_before_the_drain_lands_writes_no_rows() {
        use crate::session::store::SessionEventStore;

        let events = crate::session::store::install_test_event_store();
        let temp = tempdir().unwrap();
        let config = SessionManagerConfig {
            db_path: temp.path().join("clear_race.db"),
            max_messages: 10_000,
            compaction_keep: 5_000,
            ..Default::default()
        };
        let manager = SessionManager::new(config).unwrap();
        let id = SessionId::ephemeral("clear-race");
        manager.get_or_create(&id).await.unwrap();
        let store: Arc<dyn SessionStore> = Arc::new(manager);

        let tid = uuid::Uuid::new_v4();
        let turn: [(EventSeq, SessionEvent); 2] = [(1, user_msg(tid)), (2, assistant_msg(tid))];
        for (seq, ev) in &turn {
            events.append(&id, *seq, ev, 0).await.unwrap();
        }

        // `chat.clear` retires the log while the events are still queued.
        events.retire_from(&id, 1).await.unwrap();

        // The drain now reaches them.
        let mut accums = TurnAccums::new();
        for (seq, ev) in &turn {
            project_event(&store, &mut accums, &id, &rec(*seq, ev.clone()), None).await;
        }

        assert!(
            store.get_history(&id, None).await.unwrap().is_empty(),
            "a retired event must not be materialised by a late drain"
        );
    }

    #[tokio::test]
    async fn project_event_suppresses_already_materialised_seq() {
        let temp = tempdir().unwrap();
        let config = SessionManagerConfig {
            db_path: temp.path().join("suppress.db"),
            max_messages: 10_000,
            compaction_keep: 5_000,
            ..Default::default()
        };
        let manager = SessionManager::new(config).unwrap();
        let id = SessionId::ephemeral("suppress");
        manager.get_or_create(&id).await.unwrap();
        let store: Arc<dyn SessionStore> = Arc::new(manager);

        let tid = uuid::Uuid::new_v4();
        let mut accums = TurnAccums::new();
        // Everything at or below seq 1 is already materialised.
        let watermark = Some(1u64);

        // seq 1 is at the watermark → its write is suppressed.
        project_event(&store, &mut accums, &id, &rec(1, user_msg(tid)), watermark).await;
        assert!(
            store.get_history(&id, None).await.unwrap().is_empty(),
            "a materialised seq must not be re-written"
        );

        // seq 2 is above the watermark → written.
        project_event(&store, &mut accums, &id, &rec(2, user_msg(tid)), watermark).await;
        let rows = store.get_history(&id, None).await.unwrap();
        assert_eq!(rows.len(), 1, "an unseen seq must be written");
        assert_eq!(rows[0].role, "user");
    }
}
