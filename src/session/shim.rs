//! Dual-write helper during Phase 1 migration.
//!
//! Each write on `SessionManager` mirrors to `SessionService` so the new
//! `session_events` table stays populated in parallel with the legacy
//! `messages` table. Read paths are unchanged. The shim is removed in
//! Phase 6 when Gateway RPC also migrates onto `SessionService`.
//!
//! All helpers swallow errors with a `tracing::warn!` log — dual-write
//! failures must never break the primary `SessionManager` path.

use std::sync::Arc;

use tracing::warn;

use crate::session::events::{now_ms, MessageContent, SessionEvent, TurnId, TurnTrigger};
use crate::session::service::{SessionId, SessionService};

/// Mirrors a user message into the session event log.
pub async fn mirror_user_message(
    svc: &Arc<dyn SessionService>,
    id: &SessionId,
    turn_id: TurnId,
    text: String,
) {
    if let Err(e) = svc
        .emit_event(
            id,
            SessionEvent::UserMessage {
                turn_id,
                content: MessageContent {
                    text,
                    blocks: vec![],
                },
                at: now_ms(),
            },
        )
        .await
    {
        warn!(session_id = ?id, error = ?e, "dual-write mirror_user_message failed");
    }
}

/// Mirrors an assistant message into the session event log.
pub async fn mirror_assistant_message(
    svc: &Arc<dyn SessionService>,
    id: &SessionId,
    turn_id: TurnId,
    text: String,
) {
    if let Err(e) = svc
        .emit_event(
            id,
            SessionEvent::AssistantMessage {
                turn_id,
                content: MessageContent {
                    text,
                    blocks: vec![],
                },
                at: now_ms(),
            },
        )
        .await
    {
        warn!(session_id = ?id, error = ?e, "dual-write mirror_assistant_message failed");
    }
}

/// Mirrors a system message into the session event log.
pub async fn mirror_system_message(
    svc: &Arc<dyn SessionService>,
    id: &SessionId,
    turn_id: TurnId,
    text: String,
) {
    if let Err(e) = svc
        .emit_event(
            id,
            SessionEvent::SystemMessage {
                turn_id,
                content: text,
                at: now_ms(),
            },
        )
        .await
    {
        warn!(session_id = ?id, error = ?e, "dual-write mirror_system_message failed");
    }
}

/// Mirrors a tool-result into the session event log.
///
/// `call_id` is the opaque identifier correlating the tool call with its
/// earlier `ToolCallRequested` event. When the caller lacks one (e.g. the
/// legacy `SessionManager::add_message("tool", ...)` path), it can pass a
/// freshly-generated UUID string — correlation is optional for the mirror.
pub async fn mirror_tool_result(
    svc: &Arc<dyn SessionService>,
    id: &SessionId,
    turn_id: TurnId,
    call_id: String,
    output: serde_json::Value,
) {
    if let Err(e) = svc
        .emit_event(
            id,
            SessionEvent::ToolResult {
                turn_id,
                call_id,
                output: crate::session::events::ToolOutput {
                    value: output,
                    metadata: Default::default(),
                },
                at: now_ms(),
            },
        )
        .await
    {
        warn!(session_id = ?id, error = ?e, "dual-write mirror_tool_result failed");
    }
}

/// Mirrors a turn-start. Call before the first message in a turn is appended.
pub async fn mirror_turn_started(
    svc: &Arc<dyn SessionService>,
    id: &SessionId,
    turn_id: TurnId,
    trigger: TurnTrigger,
) {
    if let Err(e) = svc
        .emit_event(
            id,
            SessionEvent::TurnStarted {
                turn_id,
                trigger,
                at: now_ms(),
            },
        )
        .await
    {
        warn!(session_id = ?id, error = ?e, "dual-write mirror_turn_started failed");
    }
}

/// Dispatch a `SessionManager::add_message`-shaped write into the right
/// mirror helper based on the role string recorded in SQLite.
///
/// Unknown roles are logged at `debug` and skipped. A fresh `TurnId` is
/// minted per call because the legacy `add_message` API does not carry a
/// turn correlation — once call sites migrate to `SessionService` directly
/// they will thread a real turn_id through.
pub async fn mirror_message_by_role(
    svc: &Arc<dyn SessionService>,
    id: &SessionId,
    role: &str,
    content: &str,
) {
    let turn_id: TurnId = uuid::Uuid::new_v4();
    let text = content.to_string();
    match role {
        "user" => mirror_user_message(svc, id, turn_id, text).await,
        "assistant" => mirror_assistant_message(svc, id, turn_id, text).await,
        "system" => mirror_system_message(svc, id, turn_id, text).await,
        "tool" => {
            mirror_tool_result(
                svc,
                id,
                turn_id,
                uuid::Uuid::new_v4().to_string(),
                serde_json::json!({ "text": text }),
            )
            .await
        }
        other => {
            tracing::debug!(
                session_id = ?id,
                role = %other,
                "dual-write mirror: skipping unknown role"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::session_key::SessionKey;
    use crate::session::events::SessionEvent;
    use crate::session::in_process::InProcessActorSessionService;
    use crate::session::store::{migrate_add_session_events, SqliteEventStore, SessionEventStore};

    async fn fresh_service() -> Arc<dyn SessionService> {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        migrate_add_session_events(&conn).unwrap();
        let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
        Arc::new(InProcessActorSessionService::new(store))
    }

    #[tokio::test]
    async fn mirror_user_emits_one_event() {
        let svc = fresh_service().await;
        let id: SessionId = SessionKey::ephemeral("shim-user");
        let tid = uuid::Uuid::new_v4();
        mirror_user_message(&svc, &id, tid, "hi".into()).await;

        let events = svc.get_events(&id, None, None).await.unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0].event, SessionEvent::UserMessage { .. }));
    }

    #[tokio::test]
    async fn mirror_assistant_emits_one_event() {
        let svc = fresh_service().await;
        let id: SessionId = SessionKey::ephemeral("shim-assistant");
        let tid = uuid::Uuid::new_v4();
        mirror_assistant_message(&svc, &id, tid, "hello".into()).await;

        let events = svc.get_events(&id, None, None).await.unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0].event,
            SessionEvent::AssistantMessage { .. }
        ));
    }

    #[tokio::test]
    async fn mirror_by_role_dispatches_correctly() {
        let svc = fresh_service().await;
        let id: SessionId = SessionKey::ephemeral("shim-dispatch");

        mirror_message_by_role(&svc, &id, "user", "u").await;
        mirror_message_by_role(&svc, &id, "assistant", "a").await;
        mirror_message_by_role(&svc, &id, "system", "s").await;
        mirror_message_by_role(&svc, &id, "tool", "t").await;
        mirror_message_by_role(&svc, &id, "unknown-role", "x").await; // skipped

        let events = svc.get_events(&id, None, None).await.unwrap();
        assert_eq!(events.len(), 4, "unknown role must be skipped");
        assert!(matches!(events[0].event, SessionEvent::UserMessage { .. }));
        assert!(matches!(
            events[1].event,
            SessionEvent::AssistantMessage { .. }
        ));
        assert!(matches!(
            events[2].event,
            SessionEvent::SystemMessage { .. }
        ));
        assert!(matches!(events[3].event, SessionEvent::ToolResult { .. }));
    }
}
