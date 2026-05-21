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
/// * `Multimodal` — one `UserMessage` event per entry (each may carry
///   non-text `blocks` that the LLM layer interprets).
///
/// Every emitted event shares a fresh `turn_id` except the trailing
/// `UserMessage` of `History`, which also emits a `TurnStarted` event so the
/// harness loop identifies the new user turn correctly.
pub(super) async fn seed_session(
    service: &dyn SessionService,
    session_id: &SessionId,
    input: FlowInput,
) -> Result<(), FlowError> {
    match input {
        FlowInput::Prompt(text) => {
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
            .await?;
        }
        FlowInput::Messages(msgs) => {
            for content in msgs {
                emit_message(service, session_id, content, true).await?;
            }
        }
        FlowInput::History { turns, prompt } => {
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
                    },
                )
                .await
                .map_err(|e| FlowError::Internal(format!("session seed: {e}")))?;
        }
        FlowInput::Multimodal(msgs) => {
            for content in msgs {
                emit_message(service, session_id, content, true).await?;
            }
        }
        FlowInput::Resume => {
            // No-op: the session log already contains the original
            // UserMessage and the full prior trajectory. The harness
            // replays it and continues; re-seeding would duplicate the
            // user message.
        }
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
        }
    } else {
        SessionEvent::AssistantMessage {
            turn_id: uuid::Uuid::new_v4(),
            content,
            at: now_ms(),
        }
    };
    service
        .emit_event(session_id, event)
        .await
        .map(|_| ())
        .map_err(|e| FlowError::Internal(format!("session seed: {e}")))
}
