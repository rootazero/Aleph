//! Backfill `session_events` from legacy `messages` rows.
//!
//! Called at run-start when a session may have `MessageRecord`s in the
//! gateway store but no corresponding `session_events`. The function is
//! idempotent: it returns `Ok(0)` immediately if any events already exist
//! for the session.
//!
//! # Detach contract
//! Events are appended directly to the raw [`SessionEventStore`] — bypassing
//! `svc.emit_event` and the `MessageProjector` — to avoid duplicate rows.
//! Before writing, the function calls `service.detach(session_id)` so the
//! live actor (if any) is torn down and cannot collide on the same seq values.

use crate::gateway::session_store::types::MessageRecord;
use crate::session::events::{EventSeq, MessageContent, SessionEvent};
use crate::session::service::{SessionError, SessionId, SessionService};
use crate::session::store::SessionEventStore;

/// Reconstruct `session_events` from legacy `messages` rows.
///
/// # Idempotency
/// Returns `Ok(0)` when any events already exist for `session_id` — the log
/// is already populated and backfill is skipped.
///
/// # Detach contract
/// Calls `service.detach(session_id)` before writing to the store so the
/// live actor's in-memory `head_seq` cannot collide with the inserted rows.
///
/// # Role mapping
/// | `MessageRecord.role`          | `SessionEvent` variant |
/// |-------------------------------|------------------------|
/// | `"user"`                      | `UserMessage`          |
/// | `"assistant"`                 | `AssistantMessage`     |
/// | `"system"` / `"tool"` / other | skipped                |
///
/// Only the conversation turns (`user` / `assistant`) the harness replays are
/// reconstructed. `system` rows (e.g. a stored system prompt) and `tool` rows
/// are skipped so they neither pollute the replayed log nor consume a seq.
///
/// # Returns
/// Number of events appended. `0` when the session already has events or
/// when `messages` is empty.
pub(crate) async fn backfill_events_from_messages(
    session_id: &SessionId,
    service: &dyn SessionService,
    event_store: &dyn SessionEventStore,
    messages: &[MessageRecord],
) -> Result<usize, SessionError> {
    // Idempotency: session already has events — nothing to do.
    let head = event_store.load_head_seq(session_id).await?;
    if head > 0 {
        return Ok(0);
    }

    // Nothing to backfill.
    if messages.is_empty() {
        return Ok(0);
    }

    // Detach any live actor BEFORE writing directly to the store to prevent
    // the actor's in-memory head_seq from colliding with the new rows.
    service.detach(session_id).await?;

    let mut seq: EventSeq = 0;
    let mut count: usize = 0;

    for msg in messages {
        let event: Option<SessionEvent> = match msg.role.as_str() {
            "user" => Some(SessionEvent::UserMessage {
                turn_id: uuid::Uuid::new_v4(),
                content: MessageContent {
                    text: msg.content.clone(),
                    // rust-doctor-disable-next-line unnecessary-allocation
                    blocks: Vec::new(),
                    thinking: None,
                    thinking_signature: None,
                },
                at: msg.timestamp,
                synthetic: false,
                author_user_id: None,
            }),
            "assistant" => Some(SessionEvent::AssistantMessage {
                turn_id: uuid::Uuid::new_v4(),
                content: MessageContent {
                    text: msg.content.clone(),
                    // rust-doctor-disable-next-line unnecessary-allocation
                    blocks: Vec::new(),
                    thinking: None,
                    thinking_signature: None,
                },
                // Rebuilt from a stored row, not observed from a call: the row
                // records what was billed but not how (cache split, reasoning),
                // and this event exists to restore conversational context, not
                // to re-bill. Absent, not zero.
                usage: None,
                at: msg.timestamp,
            }),
            // "system", "tool", and unknown roles are skipped: backfill only
            // reconstructs the conversation turns the harness replays. Skipped
            // rows do not consume a seq.
            _ => None,
        };

        if let Some(event) = event {
            seq += 1;
            event_store
                .append(session_id, seq, &event, msg.timestamp)
                .await?;
            count += 1;
        }
    }

    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::session_key::SessionKey;
    use crate::session::in_process::InProcessActorSessionService;
    use crate::session::store::{migrate_add_session_events, SqliteEventStore};
    use std::sync::Arc;

    // -------------------------------------------------------------------------
    // Helpers
    // -------------------------------------------------------------------------

    fn make_store() -> Arc<SqliteEventStore> {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        migrate_add_session_events(&conn).unwrap();
        Arc::new(SqliteEventStore::new(conn))
    }

    fn make_service(store: Arc<SqliteEventStore>) -> InProcessActorSessionService {
        InProcessActorSessionService::new(store as Arc<dyn SessionEventStore>)
    }

    fn sample_id(label: &str) -> SessionId {
        SessionKey::ephemeral(label)
    }

    fn msg(role: &str, content: &str, ts: i64) -> MessageRecord {
        MessageRecord {
            id: uuid::Uuid::new_v4().to_string(),
            role: role.to_string(),
            content: content.to_string(),
            timestamp: ts,
            metadata: None,
            input_tokens: 0,
            output_tokens: 0,
            tool_call_id: None,
            tool_name: None,
        }
    }

    // -------------------------------------------------------------------------
    // Tests
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn empty_messages_returns_zero_without_error() {
        let store = make_store();
        let svc = make_service(store.clone());
        let id = sample_id("bf-empty");

        let n = backfill_events_from_messages(&id, &svc, store.as_ref(), &[])
            .await
            .unwrap();

        assert_eq!(n, 0);
        assert_eq!(store.load_head_seq(&id).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn user_and_assistant_messages_are_backfilled() {
        let store = make_store();
        let svc = make_service(store.clone());
        let id = sample_id("bf-roles");

        let messages = vec![
            msg("user", "hello there", 1_000),
            msg("assistant", "hi back", 2_000),
        ];

        let n = backfill_events_from_messages(&id, &svc, store.as_ref(), &messages)
            .await
            .unwrap();

        assert_eq!(n, 2);
        let events = store.load_all_events(&id).await.unwrap();
        assert_eq!(events.len(), 2);
        assert!(
            matches!(events[0].event, SessionEvent::UserMessage { .. }),
            "first event must be UserMessage"
        );
        assert!(
            matches!(events[1].event, SessionEvent::AssistantMessage { .. }),
            "second event must be AssistantMessage"
        );
        assert_eq!(events[0].created_at_ms, 1_000);
        assert_eq!(events[1].created_at_ms, 2_000);
    }

    #[tokio::test]
    async fn tool_messages_are_skipped() {
        let store = make_store();
        let svc = make_service(store.clone());
        let id = sample_id("bf-tool");

        let messages = vec![
            msg("user", "run the tool", 1_000),
            msg("tool", r#"{"result":"ok"}"#, 1_500),
            msg("assistant", "done", 2_000),
        ];

        let n = backfill_events_from_messages(&id, &svc, store.as_ref(), &messages)
            .await
            .unwrap();

        // tool row is skipped; only user + assistant survive
        assert_eq!(n, 2);
        let events = store.load_all_events(&id).await.unwrap();
        assert_eq!(events.len(), 2);
    }

    #[tokio::test]
    async fn idempotent_when_events_already_exist() {
        let store = make_store();
        let svc = make_service(store.clone());
        let id = sample_id("bf-idem");

        // Pre-populate one event directly on the store.
        store
            .append(
                &id,
                1,
                &SessionEvent::UserMessage {
                    turn_id: uuid::Uuid::new_v4(),
                    content: MessageContent {
                        text: "pre-existing".into(),
                        blocks: Vec::new(),
                        thinking: None,
                        thinking_signature: None,
                    },
                    at: 500,
                    synthetic: false,
                    author_user_id: None,
                },
                500,
            )
            .await
            .unwrap();

        // Backfill should be a no-op because head_seq > 0.
        let messages = vec![msg("user", "should be ignored", 1_000)];
        let n = backfill_events_from_messages(&id, &svc, store.as_ref(), &messages)
            .await
            .unwrap();

        assert_eq!(n, 0, "must return 0 when events already exist");
        // Only the original event remains.
        let events = store.load_all_events(&id).await.unwrap();
        assert_eq!(events.len(), 1);
    }

    #[tokio::test]
    async fn system_messages_are_skipped() {
        let store = make_store();
        let svc = make_service(store.clone());
        let id = sample_id("bf-sys");

        // A system row wedged between a user and an assistant turn: it must
        // produce NO event and must NOT consume a seq (user=1, assistant=2).
        let messages = vec![
            msg("user", "hi", 1_000),
            msg("system", "you are helpful", 1_500),
            msg("assistant", "hello", 2_000),
        ];
        let n = backfill_events_from_messages(&id, &svc, store.as_ref(), &messages)
            .await
            .unwrap();

        assert_eq!(n, 2, "system row must not be backfilled");
        let events = store.load_all_events(&id).await.unwrap();
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0].event, SessionEvent::UserMessage { .. }));
        assert!(matches!(
            events[1].event,
            SessionEvent::AssistantMessage { .. }
        ));
        // Contiguous seqs with no gap for the skipped system row.
        assert_eq!(events[0].seq, 1);
        assert_eq!(events[1].seq, 2);
        assert!(
            !events
                .iter()
                .any(|e| matches!(e.event, SessionEvent::SystemMessage { .. })),
            "no SystemMessage event should be produced"
        );
    }

    #[tokio::test]
    async fn unknown_role_is_skipped() {
        let store = make_store();
        let svc = make_service(store.clone());
        let id = sample_id("bf-unknown");

        let messages = vec![
            msg("function", "ignored", 1_000),
            msg("user", "kept", 2_000),
        ];

        let n = backfill_events_from_messages(&id, &svc, store.as_ref(), &messages)
            .await
            .unwrap();

        assert_eq!(n, 1);
        let events = store.load_all_events(&id).await.unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0].event, SessionEvent::UserMessage { .. }));
    }

    #[tokio::test]
    async fn seq_is_monotonically_increasing_from_one() {
        let store = make_store();
        let svc = make_service(store.clone());
        let id = sample_id("bf-seq");

        let messages = vec![
            msg("user", "first", 1_000),
            msg("assistant", "second", 2_000),
            msg("user", "third", 3_000),
        ];

        backfill_events_from_messages(&id, &svc, store.as_ref(), &messages)
            .await
            .unwrap();

        let events = store.load_all_events(&id).await.unwrap();
        assert_eq!(events[0].seq, 1);
        assert_eq!(events[1].seq, 2);
        assert_eq!(events[2].seq, 3);
    }

    #[tokio::test]
    async fn tool_rows_between_user_and_assistant_do_not_break_seq() {
        let store = make_store();
        let svc = make_service(store.clone());
        let id = sample_id("bf-seq-gap");

        // user=1, tool=skip, assistant=2
        let messages = vec![
            msg("user", "call a tool", 1_000),
            msg("tool", "result", 1_500),
            msg("assistant", "done", 2_000),
        ];

        let n = backfill_events_from_messages(&id, &svc, store.as_ref(), &messages)
            .await
            .unwrap();
        assert_eq!(n, 2);

        let events = store.load_all_events(&id).await.unwrap();
        // seq must still be contiguous (1, 2) with no gaps
        assert_eq!(events[0].seq, 1);
        assert_eq!(events[1].seq, 2);
    }
}
