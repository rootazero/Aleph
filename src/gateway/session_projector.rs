//! `MessageProjector` — a [`SessionEventObserver`] that materialises session
//! events into the `messages` table via a single ordered async drain task.
//!
//! Each assistant row carries the tokens of the single LLM call that produced
//! it, read straight off `AssistantMessage.usage` — the harness emits one
//! `AssistantMessage` per Think step, so calls and rows are 1:1.
//!
//! The observer itself is **non-blocking**: `on_appended` enqueues the event
//! onto an mpsc channel and returns immediately. On back-pressure (or an
//! unclean shutdown between an event's durable append to `session_events` and
//! its drain to `messages`) the event is dropped from this projection.
//!
//! Consistency model: `session_events` is the single source of truth and is
//! unaffected — the agent replays the event log in full, so **recovery of the
//! agent's context is complete**. The `messages` table is an *eventually
//! consistent* read projection for the Panel.
//!
//! A boot-time reconciler exists
//! ([`crate::gateway::projection_reconciler`]) and back-fills the
//! un-materialised tail of any session whose run markers read as interrupted.
//! It does **not** catch a drop in a session whose run then finished cleanly:
//! that session classifies as `Clean` and is skipped, so the row is lost from
//! the display permanently. See
//! `docs/superpowers/specs/2026-08-31-run-reduction-design.md` §8.1 — the fix
//! is a durable projection watermark, not a wider marker scan.

use std::sync::Arc;

use tokio::sync::mpsc;

use crate::gateway::event_bus::GatewayEventBus;
use crate::gateway::events::GatewayEventFrame;
use crate::gateway::session_store::types::MessageRecord;
use crate::gateway::session_store::SessionStore;
use crate::session::events::{SessionEvent, SessionEventRecord};
use crate::session::observer::SessionEventObserver;
use crate::session::projection::{project_row, row_id};
use crate::session::service::SessionId;

/// Capacity of the internal mpsc channel between the observer and the drain task.
const QUEUE_CAP: usize = 4096;

/// Materialises a session event stream into the `messages` store.
pub struct MessageProjector {
    tx: mpsc::Sender<(SessionId, SessionEventRecord)>,
}

impl MessageProjector {
    /// Create a new projector and spawn its drain task.
    ///
    /// The returned `Arc<MessageProjector>` implements [`SessionEventObserver`]
    /// and can be injected at boot (Task 5).
    ///
    /// `bus` is what makes this drain the producer of the live peer echo
    /// ([`GatewayEventFrame::SessionUserMessage`]). `None` keeps the projector
    /// fully usable without a running gateway (tests, tools that open the store
    /// directly) — it then only materialises rows, exactly as before.
    pub fn new(store: Arc<dyn SessionStore>, bus: Option<Arc<GatewayEventBus>>) -> Arc<Self> {
        let (tx, mut rx) = mpsc::channel::<(SessionId, SessionEventRecord)>(QUEUE_CAP);
        tokio::spawn(async move {
            while let Some((id, rec)) = rx.recv().await {
                project_event(&store, &id, &rec, None, bus.as_ref()).await;
            }
        });
        Arc::new(Self { tx })
    }
}

/// The live peer-echo frame for a row that is about to be appended, or `None`
/// when this row is not one.
///
/// Pure and separate from the write so the decision is unit-testable without a
/// store, a bus, or a runtime — every condition below is a way this has to be
/// able to say "no", and each one is load-bearing:
///
/// - **Live drain only.** `materialized_through` is `Some` exactly on the
///   boot-time [`ProjectionReconciler`](crate::gateway::projection_reconciler)
///   path, which replays a whole event log to back-fill rows a previous process
///   never flushed. Those are old messages; echoing them would replay a dead
///   conversation into every Panel that happens to be open at boot. (The
///   reconciler also passes no bus, so this is belt and braces — but the
///   criterion is "is this row new", not "did somebody hand me a bus".)
/// - **Real user messages only.** `synthetic` user events are the prompt
///   builder's `<system-reminder>` scaffolding, not something a human typed.
/// - **Attributed only.** See the frame's own doc: an author-less message
///   cannot be told apart from the viewer's own, so there is nobody it can be
///   safely rendered to. Outside a project room `ambient_room_author` is
///   `None`, which is why single-author deployments never emit this at all.
/// - **Non-empty only.** `hydrate_session_history` skips blank rows, so
///   emitting one would put up a bubble that the next reload takes away —
///   the precise failure this whole frame exists to avoid.
fn peer_echo_frame(
    session_key: &str,
    seq: u64,
    event: &SessionEvent,
    author_user_id: Option<&str>,
    record: &MessageRecord,
    materialized_through: Option<u64>,
) -> Option<GatewayEventFrame> {
    if materialized_through.is_some() {
        return None;
    }
    let SessionEvent::UserMessage {
        synthetic: false, ..
    } = event
    else {
        return None;
    };
    let author = author_user_id.filter(|a| !a.is_empty())?;
    if record.content.trim().is_empty() {
        return None;
    }
    Some(GatewayEventFrame::SessionUserMessage {
        session_key: session_key.to_string(),
        author_user_id: author.to_string(),
        content: record.content.clone(),
        // The record's own accessor, not a hand-rolled format of
        // `created_at_ms` — see the field's doc on the frame.
        timestamp: record.rfc3339(),
        seq,
    })
}

/// True when this event was retired after it was enqueued — the drain is
/// asynchronous, so a queued event can be retired *before* it reaches
/// `messages`, and writing it then would silently un-clear the conversation the
/// user just cleared.
///
/// ⚠️ `retired_at` has two writers with **opposite** intent, and this gate reads
/// only the flag. `retire_from` (`chat.clear` / `chat.rewind`) means "erase" —
/// suppressing the row is the whole point. `retire_through` (manual `/compact`,
/// `context::compact::manual`) means "stop replaying, keep everything" — for it
/// the suppression is collateral: a compacted event that had not yet drained
/// loses its Panel row. Bounded, not free: the compacted prefix sits at least a
/// whole `keep_tokens` budget behind the head, so the drain must be lagging by
/// that much for the two to meet, and the projection is already declared
/// best-effort (`on_appended` drops on queue-full). Distinguishing the two
/// would take a retirement *reason* on the row; that is a schema change with no
/// observed failure behind it. If one ever shows up, this is the place.
///
/// Fails closed: an unreadable event log is reported as retired, so the failure
/// mode is a missing projection row rather than resurrected content. That is
/// not always a recoverable miss: `ProjectionReconciler` only back-fills a
/// session whose run marker reads as interrupted, so if this session's run
/// then finishes cleanly, the row is gone from the display for good — see the
/// module doc.
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
/// When `rec.seq <= materialized_through`, a row-producing event's WRITE is
/// suppressed: that row is already in the projection, so re-projecting it is a
/// no-op (reconcile idempotency, and dup-avoidance for mixed/legacy rows below
/// the watermark).
///
/// `bus`, when present, publishes the live peer echo for a newly-materialised
/// user row (see [`peer_echo_frame`]). It is published from HERE rather than
/// from the run engines because this is the one point every producer of a user
/// message passes through — `harness_bridge::session_seed` (the main path),
/// `fast_path` (which re-emits the event by hand for exactly this reason),
/// `SimpleExecutionEngine`, and mid-run `steering` — and because it is the only
/// point where the text being announced is, by construction, the text
/// `chat.history` will replay.
pub(crate) async fn project_event(
    store: &Arc<dyn SessionStore>,
    id: &SessionId,
    rec: &SessionEventRecord,
    materialized_through: Option<u64>,
    bus: Option<&Arc<GatewayEventBus>>,
) {
    let key = id.to_key_string();
    let suppress =
        materialized_through.is_some_and(|w| rec.seq <= w) || event_retired(id, rec.seq).await;
    match &rec.event {
        SessionEvent::AssistantMessage { content, usage, .. } => {
            if suppress {
                return;
            }
            // The tokens of the one call that produced this message. The
            // cross-event accumulator this replaced (`LlmCallStarted` /
            // `LlmCallEnded` folded per turn_id) was a correct design for an
            // event pair no production code has ever emitted, so it summed
            // nothing and wrote 0 onto every assistant row since the projector
            // was written. Both events are gone; the number now rides on the
            // message that spent it.
            let usage = usage.clone().unwrap_or_default();
            if let Err(e) = store
                .append_message(
                    id,
                    MessageRecord {
                        id: row_id(&key, rec.seq),
                        role: "assistant".into(),
                        content: content.text.clone(),
                        timestamp: rec.created_at_ms,
                        metadata: None,
                        input_tokens: i64::from(usage.input),
                        output_tokens: i64::from(usage.output),
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
            model,
            model_provider,
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
                model: model.clone(),
                model_provider: model_provider.clone(),
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
            //
            // THE run's report, not A report: `add_message_full` no longer adds
            // each message row's tokens onto these same three columns. It did,
            // silently, for as long as the rows carried zeros; the moment the
            // rows became real (above) that stopped being a harmless no-op and
            // started double-billing the session.
            if *input_tokens > 0 || *output_tokens > 0 || cost_usd.is_some() {
                if let Err(e) = store
                    .update_session_usage(
                        id,
                        i64::from(*input_tokens),
                        i64::from(*output_tokens),
                        cost_usd.unwrap_or(0.0),
                        model.as_deref(),
                        model_provider.as_deref(),
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
                let record = MessageRecord {
                    id: row_id(&key, rec.seq),
                    role: row.role,
                    content: row.text,
                    timestamp: rec.created_at_ms,
                    // String-valued, matching every other key in this
                    // bag (`agent_instance::build_message_metadata`);
                    // the history handler reads them all with `as_str`.
                    metadata: row
                        .author_user_id
                        .as_ref()
                        .map(|u| serde_json::json!({ "author_user_id": u })),
                    input_tokens: 0,
                    output_tokens: 0,
                    tool_call_id: row.tool_call_id,
                    tool_name: row.tool_name,
                };
                // Decided before the move, published only after the write
                // succeeds: this frame's contract is "the transcript gained
                // this row", so announcing a failed append would put a bubble
                // on screen that no reload can reproduce.
                let echo = peer_echo_frame(
                    &key,
                    rec.seq,
                    other,
                    row.author_user_id.as_deref(),
                    &record,
                    materialized_through,
                );
                if let Err(e) = store.append_message(id, record).await {
                    tracing::warn!(error = %e, "projector append failed");
                    return;
                }
                if let (Some(bus), Some(frame)) = (bus, echo) {
                    let _ = bus.publish_frame(&frame);
                }
            }
        }
    }
}

impl SessionEventObserver for MessageProjector {
    fn on_appended(&self, id: &SessionId, record: &SessionEventRecord) {
        // Busy-lane wake edge, taken here rather than inside the drain task
        // below: `try_send` DROPS on a full queue (the projection is
        // eventually consistent by design, the SSOT log is not), and a dropped
        // wake would put a backpressure-deferred steer back on the 30 s
        // fallback tick — the exact staleness this edge exists to remove.
        //
        // This observer is the gateway's one "an event was appended" seam, so
        // it sees every producer of an assistant turn (harness run, fast path,
        // simple engine). The predicate for which events matter belongs to
        // steering, next to the count it resets; everything below the
        // `matches!` costs nothing for the events that are not assistant turns.
        crate::gateway::execution_engine::wake_lane_if_burst_drained(
            &id.to_key_string(),
            &record.event,
        );
        match self.tx.try_send((id.clone(), record.clone())) {
            Ok(()) => {}
            // Expected back-pressure. The event stays in the SSOT log (agent
            // recovery unaffected). The Panel projection may lose this row for
            // good if the run then finishes cleanly — see the module doc.
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
    use crate::orchestrator::dispatch::TokenBreakdown;
    use crate::session::events::{EventSeq, MessageContent, ToolOutput, TurnId};
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

    /// A user event as a room member's message: attributed, real, non-empty.
    fn room_msg(author: Option<&str>, synthetic: bool) -> SessionEvent {
        SessionEvent::UserMessage {
            turn_id: uuid::Uuid::new_v4(),
            content: msg_content("where did we land on the migration?"),
            at: 0,
            synthetic,
            author_user_id: author.map(String::from),
        }
    }

    /// The row `project_event` is about to append for [`room_msg`].
    fn row_for(text: &str) -> MessageRecord {
        MessageRecord {
            id: "agent:main:main:7".into(),
            role: "user".into(),
            content: text.into(),
            timestamp: 1_762_000_000_000,
            metadata: None,
            input_tokens: 0,
            output_tokens: 0,
            tool_call_id: None,
            tool_name: None,
        }
    }

    /// The happy path: another member's message, live, becomes a frame whose
    /// text and timestamp are the row's — not the request's, not a re-format.
    #[test]
    fn peer_echo_announces_an_attributed_room_message() {
        let row = row_for("where did we land on the migration?");
        let frame = peer_echo_frame(
            "agent:main:main",
            7,
            &room_msg(Some("u-alice"), false),
            Some("u-alice"),
            &row,
            None,
        )
        .expect("an attributed live user row must be announced");
        let GatewayEventFrame::SessionUserMessage {
            session_key,
            author_user_id,
            content,
            timestamp,
            seq,
        } = frame
        else {
            panic!("wrong frame variant");
        };
        assert_eq!(session_key, "agent:main:main");
        assert_eq!(author_user_id, "u-alice");
        assert_eq!(content, row.content);
        assert_eq!(seq, 7);
        // The record's accessor, so a live bubble and its reloaded twin sort
        // identically. Formatting `created_at_ms` by hand here would diverge on
        // whichever backend stores seconds.
        assert_eq!(timestamp, row.rfc3339());
    }

    /// The boot reconciler back-fills rows a dead process never flushed. Those
    /// messages are old; announcing them would replay a finished conversation
    /// into every Panel open at boot.
    #[test]
    fn peer_echo_is_silent_on_the_boot_reconcile_path() {
        assert!(peer_echo_frame(
            "agent:main:main",
            7,
            &room_msg(Some("u-alice"), false),
            Some("u-alice"),
            &row_for("hi"),
            Some(99),
        )
        .is_none());
    }

    /// No author ⇒ nobody can tell this from their own message, so there is no
    /// viewer it can be safely rendered to. This is every single-author session.
    #[test]
    fn peer_echo_is_silent_without_an_author() {
        assert!(
            peer_echo_frame(
                "agent:main:main",
                7,
                &room_msg(None, false),
                None,
                &row_for("hi"),
                None,
            )
            .is_none(),
            "an unattributed message must not be echoed"
        );
        assert!(
            peer_echo_frame(
                "agent:main:main",
                7,
                &room_msg(Some(""), false),
                Some(""),
                &row_for("hi"),
                None,
            )
            .is_none(),
            "an empty author id is an absent one, not a user named \"\""
        );
    }

    /// `synthetic` user events are the prompt builder's `<system-reminder>`
    /// scaffolding wearing a user role — nobody typed them.
    #[test]
    fn peer_echo_is_silent_for_synthetic_scaffolding() {
        assert!(peer_echo_frame(
            "agent:main:main",
            7,
            &room_msg(Some("u-alice"), true),
            Some("u-alice"),
            &row_for("hi"),
            None,
        )
        .is_none());
    }

    /// `hydrate_session_history` skips blank rows, so echoing one would show a
    /// bubble the next reload silently removes — the exact contradiction this
    /// frame exists to avoid.
    #[test]
    fn peer_echo_is_silent_for_a_row_the_panel_would_not_render() {
        assert!(peer_echo_frame(
            "agent:main:main",
            7,
            &room_msg(Some("u-alice"), false),
            Some("u-alice"),
            &row_for("   \n "),
            None,
        )
        .is_none());
    }

    fn user_msg(tid: TurnId) -> SessionEvent {
        SessionEvent::UserMessage {
            turn_id: tid,
            content: msg_content("hi"),
            at: 0,
            synthetic: false,
            author_user_id: None,
        }
    }

    fn assistant_msg(tid: TurnId) -> SessionEvent {
        SessionEvent::AssistantMessage {
            turn_id: tid,
            content: msg_content("hello"),
            usage: None,
            at: 0,
        }
    }

    fn assistant_msg_billed(tid: TurnId, input: u32, output: u32) -> SessionEvent {
        SessionEvent::AssistantMessage {
            turn_id: tid,
            content: msg_content("hello"),
            usage: Some(TokenBreakdown {
                input,
                output,
                ..Default::default()
            }),
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

    /// The busy lane's burst-drain wake edge has to survive the seam it is
    /// fired from. Asserting the call would prove nothing — throw the notify
    /// away and a call-count guard stays green — so this asserts the effect:
    /// a waiter parked on the lane is released by an assistant turn arriving at
    /// the observer, and is NOT released by a user turn (which does not drain
    /// anything). Fired before the `try_send` below on purpose: that send drops
    /// on a full queue, and a dropped wake puts the message back on the 30 s
    /// fallback tick.
    #[tokio::test]
    async fn an_appended_assistant_turn_wakes_a_backpressure_deferred_waiter() {
        let temp = tempdir().unwrap();
        let config = SessionManagerConfig {
            db_path: temp.path().join("proj_wake.db"),
            ..Default::default()
        };
        let manager = SessionManager::new(config).unwrap();
        let id = SessionId::ephemeral("proj_wake");
        manager.get_or_create(&id).await.unwrap();
        let store: Arc<dyn SessionStore> = Arc::new(manager);
        let projector = MessageProjector::new(store, None);

        // The lane is keyed exactly as the observer will render this session.
        let key = id.to_key_string();
        let ticket = crate::gateway::busy_queue::register(&key, 8, "proj-wake-run")
            .expect("lane accepts the deferred steer");
        crate::gateway::busy_queue::mark_awaiting_burst_drain(&key, "proj-wake-run");
        let wake = ticket.wake_handle();
        let parked = wake.notified();
        tokio::pin!(parked);

        let tid = uuid::Uuid::new_v4();
        projector.on_appended(&id, &rec(1, user_msg(tid)));
        assert!(
            tokio::time::timeout(Duration::from_millis(10), &mut parked)
                .await
                .is_err(),
            "a user turn adds to the burst; it cannot drain it"
        );

        projector.on_appended(&id, &rec(2, assistant_msg(tid)));
        tokio::time::timeout(Duration::from_millis(500), &mut parked)
            .await
            .expect("an assistant turn at the observer must reach the lane");
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
        let projector = MessageProjector::new(store.clone(), None);

        let tid = uuid::Uuid::new_v4();
        let events: &[(EventSeq, SessionEvent)] = &[
            (1, user_msg(tid)),
            (2, assistant_msg_billed(tid, 100, 50)),
            (
                3,
                SessionEvent::AssistantRunMeta {
                    turn_id: tid,
                    run_id: "run_xyz".into(),
                    context_tokens: 1234,
                    context_window: 200_000,
                    total_tokens: 5678,
                    input_tokens: 4000,
                    output_tokens: 1678,
                    cost_usd: Some(0.12),
                    model: Some("claude".into()),
                    model_provider: Some("anthropic".into()),
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
        let projector = MessageProjector::new(store.clone(), None);

        // Two Think steps — two LLM calls, two assistant rows — then the run's
        // one billing report. This is the shape production actually emits; the
        // `LlmCallStarted`/`LlmCallEnded` pair this test used to hand-feed was
        // emitted by nothing but this test.
        let tid = uuid::Uuid::new_v4();
        let events: [(EventSeq, SessionEvent); 6] = [
            (1, user_msg(tid)),
            (2, assistant_msg_billed(tid, 10, 20)),
            (3, tool_req(tid)),
            (4, tool_res(tid)),
            (5, assistant_msg_billed(tid, 30, 5)),
            (
                6,
                SessionEvent::AssistantRunMeta {
                    turn_id: tid,
                    run_id: "run_1".into(),
                    context_tokens: 40,
                    context_window: 200_000,
                    total_tokens: 65,
                    // The run's billed total. Deliberately NOT 40/25 (the sum of
                    // the two rows): a retry-discarded call is billed but never
                    // becomes a message, so the session total is a superset of
                    // its rows. Distinct numbers here are what make the
                    // double-count assertion below meaningful.
                    input_tokens: 45,
                    output_tokens: 25,
                    cost_usd: Some(0.02),
                    model: Some("claude".into()),
                    model_provider: Some("anthropic".into()),
                    at: 5,
                },
            ),
        ];
        for (seq, ev) in events {
            projector.on_appended(&id, &rec(seq, ev));
        }

        // 5 row-producing events (user + 2 assistant + tool_req + tool_res;
        // AssistantRunMeta stamps rather than appends).
        let msgs = poll_history(&store, &id, 5, Duration::from_secs(2)).await;

        assert_eq!(
            msgs.iter().filter(|m| m.role == "user").count(),
            1,
            "expected exactly 1 user row"
        );

        // Each assistant row carries the tokens of the ONE call that produced
        // it — not the turn's sum, and not zero (which is what every assistant
        // row in every real deployment carried).
        let asst: Vec<_> = msgs.iter().filter(|m| m.role == "assistant").collect();
        assert_eq!(asst.len(), 2, "expected one row per Think step");
        assert_eq!((asst[0].input_tokens, asst[0].output_tokens), (10, 20));
        assert_eq!((asst[1].input_tokens, asst[1].output_tokens), (30, 5));

        let meta = store
            .get_metadata(&id)
            .await
            .unwrap()
            .expect("missing session metadata");
        // The run's report is the session's ONLY token writer. If
        // `add_message_full` also accumulated each row (as it did until the rows
        // stopped being zeros), these would read 45+40 / 25+25.
        assert_eq!(meta.input_tokens, 45, "session input_tokens double-counted");
        assert_eq!(
            meta.output_tokens, 25,
            "session output_tokens double-counted"
        );
        assert_eq!(
            meta.model.as_deref(),
            Some("claude"),
            "sessions.model must be written from the run's report"
        );
        assert_eq!(meta.model_provider.as_deref(), Some("anthropic"));

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
        for (seq, ev) in &turn {
            project_event(&store, &id, &rec(*seq, ev.clone()), None, None).await;
        }

        assert!(
            store.get_history(&id, None).await.unwrap().is_empty(),
            "a retired event must not be materialised by a late drain"
        );
    }

    /// The receipt has to reach the surface clients actually read.
    ///
    /// `chat.history` serves the `messages` projection, not the event log, so
    /// persisting the block reason in `session_events` alone would leave a
    /// reloading tab exactly where it was: an unanswered user message and no
    /// explanation. Asserts the row a re-attaching client is served, not that
    /// the projector was called.
    #[tokio::test]
    async fn a_refusal_receipt_is_served_to_a_reattaching_client() {
        let temp = tempdir().unwrap();
        let config = SessionManagerConfig {
            db_path: temp.path().join("refusal.db"),
            max_messages: 10_000,
            compaction_keep: 5_000,
            ..Default::default()
        };
        let manager = SessionManager::new(config).unwrap();
        let id = SessionId::ephemeral("refusal");
        manager.get_or_create(&id).await.unwrap();
        let store: Arc<dyn SessionStore> = Arc::new(manager);

        let tid = uuid::Uuid::new_v4();
        project_event(&store, &id, &rec(1, user_msg(tid)), None, None).await;
        project_event(
            &store,
            &id,
            &rec(
                2,
                SessionEvent::Error {
                    turn_id: Some(tid),
                    kind: crate::session::events::ErrorKind::Guardrail,
                    message: "blocked by pii guardrail".into(),
                    recoverable: false,
                    at: 0,
                },
            ),
            None,
            None,
        )
        .await;

        let rows = store.get_history(&id, None).await.unwrap();
        assert_eq!(rows.len(), 2, "the receipt must be a row of its own");
        // `system` is what the Panel renders as a centred notice rather than a
        // bubble attributed to somebody — nobody said this, the run did.
        assert_eq!(rows[1].role, "system");
        assert!(
            rows[1].content.contains("blocked by pii guardrail"),
            "the reason must survive the projection, got {:?}",
            rows[1].content
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
        // Everything at or below seq 1 is already materialised.
        let watermark = Some(1u64);

        // seq 1 is at the watermark → its write is suppressed.
        project_event(&store, &id, &rec(1, user_msg(tid)), watermark, None).await;
        assert!(
            store.get_history(&id, None).await.unwrap().is_empty(),
            "a materialised seq must not be re-written"
        );

        // seq 2 is above the watermark → written.
        project_event(&store, &id, &rec(2, user_msg(tid)), watermark, None).await;
        let rows = store.get_history(&id, None).await.unwrap();
        assert_eq!(rows.len(), 1, "an unseen seq must be written");
        assert_eq!(rows[0].role, "user");
    }

    /// The wire itself, not the decision that feeds it.
    ///
    /// [`peer_echo_frame`]'s own tests all stay green if the `publish_frame`
    /// call is deleted — they assert what the frame WOULD be, which is the
    /// classic "guards the origin, not the connection" hole. This one fails if
    /// the publish is removed, if it moves ahead of the append, or if it stops
    /// carrying the row's text.
    #[tokio::test]
    async fn an_attributed_user_row_is_announced_on_the_bus_as_it_is_written() {
        let temp = tempdir().unwrap();
        let config = SessionManagerConfig {
            db_path: temp.path().join("echo.db"),
            max_messages: 10_000,
            compaction_keep: 5_000,
            ..Default::default()
        };
        let manager = SessionManager::new(config).unwrap();
        let id = SessionId::ephemeral("echo");
        manager.get_or_create(&id).await.unwrap();
        let store: Arc<dyn SessionStore> = Arc::new(manager);

        let bus = Arc::new(crate::gateway::event_bus::GatewayEventBus::new());
        let mut rx = bus.subscribe_typed();

        project_event(
            &store,
            &id,
            &rec(1, room_msg(Some("u-alice"), false)),
            None,
            Some(&bus),
        )
        .await;

        let frame = rx.try_recv().expect("the appended row must be announced");
        let GatewayEventFrame::SessionUserMessage {
            session_key,
            author_user_id,
            content,
            seq,
            ..
        } = frame
        else {
            panic!("wrong frame variant on the bus");
        };
        assert_eq!(session_key, id.to_key_string());
        assert_eq!(author_user_id, "u-alice");
        assert_eq!(seq, 1);

        // The announced text is the row's text — the property that makes a
        // live bubble and its reloaded twin the same bubble.
        let rows = store.get_history(&id, None).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(content, rows[0].content);
    }

    /// Same call, unattributed: a row is still written, and nothing is said.
    /// This is every single-author session, i.e. the default deployment.
    #[tokio::test]
    async fn an_unattributed_user_row_is_written_but_not_announced() {
        let temp = tempdir().unwrap();
        let config = SessionManagerConfig {
            db_path: temp.path().join("echo_solo.db"),
            max_messages: 10_000,
            compaction_keep: 5_000,
            ..Default::default()
        };
        let manager = SessionManager::new(config).unwrap();
        let id = SessionId::ephemeral("echo_solo");
        manager.get_or_create(&id).await.unwrap();
        let store: Arc<dyn SessionStore> = Arc::new(manager);

        let bus = Arc::new(crate::gateway::event_bus::GatewayEventBus::new());
        let mut rx = bus.subscribe_typed();

        project_event(
            &store,
            &id,
            &rec(1, room_msg(None, false)),
            None,
            Some(&bus),
        )
        .await;

        assert_eq!(
            store.get_history(&id, None).await.unwrap().len(),
            1,
            "the row must still be materialised"
        );
        assert!(
            rx.try_recv().is_err(),
            "nothing to announce without an author"
        );
    }
}
