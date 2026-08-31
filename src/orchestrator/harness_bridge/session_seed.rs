//! Session seeding logic for harness bridge.

use crate::orchestrator::errors::FlowError;
use crate::orchestrator::flow_spec::{FlowHistoryTurn, FlowInput};
use crate::session::events::{now_ms, MessageContent, SessionEvent, TurnTrigger};
use crate::session::service::{SessionId, SessionService};

/// Seed the session log with the events required for the given [`FlowInput`].
///
/// * `Prompt` — one `UserMessage` event.
/// * `Messages` — one `UserMessage` event per entry.
/// * `History` — each turn replayed in order as the role-appropriate event,
///   then the `prompt` as a trailing `UserMessage`.
/// * `Multimodal` — a `TurnStarted` followed by one `UserMessage` event per
///   entry (each may carry non-text `blocks` — images — that
///   `harness::agent::prompt` replays into the provider message).
///
/// Every emitted event shares a fresh `turn_id` except the trailing
/// `UserMessage` of `History` and the `Multimodal` entries, which also emit a
/// `TurnStarted` event so the harness loop identifies the new user turn
/// correctly.
pub(super) async fn seed_session(
    service: &dyn SessionService,
    session_id: &SessionId,
    input: FlowInput,
) -> Result<(), FlowError> {
    match input {
        FlowInput::Prompt(text) => seed_prompt(service, session_id, text).await,
        FlowInput::Messages(msgs) => seed_messages(service, session_id, msgs).await,
        FlowInput::History { turns, prompt } => {
            seed_history(service, session_id, turns, prompt).await
        }
        FlowInput::Multimodal(msgs) => seed_multimodal(service, session_id, msgs).await,
        FlowInput::Resume => Ok(()),
    }
}

async fn seed_prompt(
    service: &dyn SessionService,
    session_id: &SessionId,
    text: String,
) -> Result<(), FlowError> {
    emit_message(
        service,
        session_id,
        MessageContent {
            text,
            blocks: Vec::new(),
            thinking: None,
            thinking_signature: None,
        },
        true,
    )
    .await
}

async fn seed_messages(
    service: &dyn SessionService,
    session_id: &SessionId,
    msgs: Vec<MessageContent>,
) -> Result<(), FlowError> {
    for content in msgs {
        emit_message(service, session_id, content, true).await?;
    }
    Ok(())
}

async fn seed_history(
    service: &dyn SessionService,
    session_id: &SessionId,
    turns: Vec<FlowHistoryTurn>,
    prompt: String,
) -> Result<(), FlowError> {
    // Only seed prior turns into a fresh (empty) log. For a continuation
    // (log already non-empty) the turns are already persisted — re-seeding
    // would duplicate the whole history. The new user turn below always runs.
    let existing = service
        .get_events(session_id, None, Some(1))
        .await
        .map(|e| !e.is_empty())
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "session log read failed; treating as empty for seed guard");
            false
        });
    if !existing {
        for turn in turns {
            match turn {
                FlowHistoryTurn::User(content) => {
                    emit_message(service, session_id, content, true).await?;
                }
                FlowHistoryTurn::Assistant(content) => {
                    emit_message(service, session_id, content, false).await?;
                }
            }
        }
    }
    // Announce a new user turn so the harness scans from the right
    // tail boundary (see `tail_start_index` in agent.rs).
    let turn_id = uuid::Uuid::new_v4();
    service
        .emit_event(
            session_id,
            SessionEvent::TurnStarted {
                turn_id,
                trigger: TurnTrigger::UserMessage,
                at: now_ms(),
            },
        )
        .await
        .map_err(|e| FlowError::Internal(format!("session seed: {e}")))?;
    service
        .emit_event(
            session_id,
            SessionEvent::UserMessage {
                turn_id,
                content: MessageContent {
                    text: prompt,
                    blocks: Vec::new(),
                    thinking: None,
                    thinking_signature: None,
                },
                at: now_ms(),
                synthetic: false,
                author_user_id: crate::scope::ambient_room_author(),
            },
        )
        .await
        .map_err(|e| FlowError::Internal(format!("session seed: {e}")))?;
    Ok(())
}

async fn seed_multimodal(
    service: &dyn SessionService,
    session_id: &SessionId,
    msgs: Vec<MessageContent>,
) -> Result<(), FlowError> {
    // Same turn framing as `History`'s trailing prompt: a multimodal
    // turn continues an existing conversation, so it must open its own
    // turn — otherwise `current_turn_id` inherits the PREVIOUS turn's
    // id and this turn's assistant reply is filed under it.
    let turn_id = uuid::Uuid::new_v4();
    service
        .emit_event(
            session_id,
            SessionEvent::TurnStarted {
                turn_id,
                trigger: TurnTrigger::UserMessage,
                at: now_ms(),
            },
        )
        .await
        .map_err(|e| FlowError::Internal(format!("session seed: {e}")))?;
    for content in msgs {
        service
            .emit_event(
                session_id,
                SessionEvent::UserMessage {
                    turn_id,
                    content,
                    at: now_ms(),
                    synthetic: false,
                    author_user_id: crate::scope::ambient_room_author(),
                },
            )
            .await
            .map_err(|e| FlowError::Internal(format!("session seed: {e}")))?;
    }
    Ok(())
}

pub(super) async fn emit_message(
    service: &dyn SessionService,
    session_id: &SessionId,
    content: MessageContent,
    is_user: bool,
) -> Result<(), FlowError> {
    let event = if is_user {
        SessionEvent::UserMessage {
            turn_id: uuid::Uuid::new_v4(),
            content,
            at: now_ms(),
            synthetic: false,
            // Also stamps `History`'s replayed prior turns, which is right for
            // the same reason the stamp exists: that transcript was supplied by
            // *this* caller, so in a room it belongs to them and nobody else.
            author_user_id: crate::scope::ambient_room_author(),
        }
    } else {
        SessionEvent::AssistantMessage {
            turn_id: uuid::Uuid::new_v4(),
            content,
            // Replayed history, not an observed call — no tokens to attribute.
            usage: None,
            at: now_ms(),
        }
    };
    service
        .emit_event(session_id, event)
        .await
        .map(|_| ())
        .map_err(|e| FlowError::Internal(format!("session seed: {e}")))
}
