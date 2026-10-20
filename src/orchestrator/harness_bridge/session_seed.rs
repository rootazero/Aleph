//! Session seeding logic for harness bridge.

use crate::gateway::telecom::persisted_marker::PersistedTelecomMarker;
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
/// Run origin (R1 of the telecom addendum). Immediately BEFORE the final real
/// `UserMessage` of the seed — the one an unanswered-tail resume would
/// retrigger for — a [`SessionEvent::SeedProvenance`] is written carrying
/// `origin` and that message's exact `turn_id`. `origin` is the SAME
/// captured marker value the run's `RunStarted` will carry (`Ordinary` for every non-telecom run, `Public` for a telecom one),
/// so a crash between the seed and `RunStarted` leaves a server-authored,
/// typed, versioned record of the run's origin instead of nothing.
/// `FlowInput::Resume` seeds nothing and so writes no carrier.
///
/// Every emitted event shares a fresh `turn_id` except the trailing
/// `UserMessage` of `History` and the `Multimodal` entries, which also emit a
/// `TurnStarted` event so the harness loop identifies the new user turn
/// correctly.
pub(super) async fn seed_session(
    service: &dyn SessionService,
    session_id: &SessionId,
    input: FlowInput,
    origin: PersistedTelecomMarker,
) -> Result<(), FlowError> {
    match input {
        FlowInput::Prompt(text) => seed_prompt(service, session_id, text, origin).await,
        FlowInput::Messages(msgs) => seed_messages(service, session_id, msgs, origin).await,
        FlowInput::History { turns, prompt } => {
            seed_history(service, session_id, turns, prompt, origin).await
        }
        FlowInput::Multimodal(msgs) => seed_multimodal(service, session_id, msgs, origin).await,
        FlowInput::Resume => Ok(()),
    }
}

/// Write the [`SessionEvent::SeedProvenance`] carrier for the seed message
/// `turn_id` that is about to be written. Always called immediately before
/// that message: a crash between the two leaves a carrier with no message,
/// which reduces as `Clean` (nothing to resume) — never the reverse.
async fn emit_seed_provenance(
    service: &dyn SessionService,
    session_id: &SessionId,
    turn_id: uuid::Uuid,
    origin: PersistedTelecomMarker,
) -> Result<(), FlowError> {
    service
        .emit_event(
            session_id,
            SessionEvent::SeedProvenance {
                turn_id,
                telecom_provenance: origin,
                at: now_ms(),
            },
        )
        .await
        .map(|_| ())
        .map_err(|e| FlowError::Internal(format!("session seed: {e}")))
}

async fn seed_prompt(
    service: &dyn SessionService,
    session_id: &SessionId,
    text: String,
    origin: PersistedTelecomMarker,
) -> Result<(), FlowError> {
    let turn_id = uuid::Uuid::new_v4();
    emit_seed_provenance(service, session_id, turn_id, origin).await?;
    emit_user_message(
        service,
        session_id,
        turn_id,
        MessageContent {
            text,
            blocks: Vec::new(),
            thinking: None,
            thinking_signature: None,
        },
    )
    .await
}

async fn seed_messages(
    service: &dyn SessionService,
    session_id: &SessionId,
    msgs: Vec<MessageContent>,
    origin: PersistedTelecomMarker,
) -> Result<(), FlowError> {
    // Only the LAST message can be the unanswered one (the reducer takes the
    // last real `UserMessage`), so the carrier binds to it alone.
    let last = msgs.len().saturating_sub(1);
    for (i, content) in msgs.into_iter().enumerate() {
        if i == last {
            let turn_id = uuid::Uuid::new_v4();
            emit_seed_provenance(service, session_id, turn_id, origin.clone()).await?;
            emit_user_message(service, session_id, turn_id, content).await?;
        } else {
            emit_message(service, session_id, content, true).await?;
        }
    }
    Ok(())
}

async fn seed_history(
    service: &dyn SessionService,
    session_id: &SessionId,
    turns: Vec<FlowHistoryTurn>,
    prompt: String,
    origin: PersistedTelecomMarker,
) -> Result<(), FlowError> {
    // Only seed prior turns into a fresh (empty) log. For a continuation
    // (log already non-empty) the turns are already persisted — re-seeding
    // would duplicate the whole history. The new user turn below always runs.
    //
    // `get_events(id, from, to)` takes a seq RANGE, not a limit: the store
    // selects `seq >= from AND seq < to`, so `to` is EXCLUSIVE. Seq allocation
    // starts at 1 (`load_head_seq` answers 0 for an empty log, and the actor
    // hands out `head_seq + 1`), so the cheapest "is there anything at all"
    // probe asks for `seq < 2`.
    //
    // Naming it, because the literal is a trap: `Some(2)` reads like "two
    // events" to anyone who assumes a limit, and a review pass lowered it to
    // `Some(1)` on exactly that reading. `seq < 1` matches nothing on any log,
    // so the guard answered "empty" every time and every continuation re-seeded
    // its whole history — the duplication this guard exists to prevent, caused
    // by the edit that claimed to fix it.
    // Regression covered by `tests::history_input_does_not_reseed_when_log_nonempty`.
    const FIRST_EVENT_EXCLUSIVE_END: crate::session::events::EventSeq = 2;
    let existing = service
        .get_events(session_id, None, Some(FIRST_EVENT_EXCLUSIVE_END))
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
    // tail boundary (see `tail_start_index` in agent.rs). The pair has one
    // constructor (`SessionEvent::user_turn`, shared with the L0 fast path);
    // the seed's write shape — two `emit_event`s — is unchanged.
    let turn_id = uuid::Uuid::new_v4();
    emit_seed_provenance(service, session_id, turn_id, origin).await?;
    for event in SessionEvent::user_turn(
        turn_id,
        MessageContent {
            text: prompt,
            blocks: Vec::new(),
            thinking: None,
            thinking_signature: None,
        },
        crate::scope::ambient_room_author(),
        now_ms(),
    ) {
        service
            .emit_event(session_id, event)
            .await
            .map_err(|e| FlowError::Internal(format!("session seed: {e}")))?;
    }
    Ok(())
}

async fn seed_multimodal(
    service: &dyn SessionService,
    session_id: &SessionId,
    msgs: Vec<MessageContent>,
    origin: PersistedTelecomMarker,
) -> Result<(), FlowError> {
    // Same turn framing as `History`'s trailing prompt: a multimodal
    // turn continues an existing conversation, so it must open its own
    // turn — otherwise `current_turn_id` inherits the PREVIOUS turn's
    // id and this turn's assistant reply is filed under it.
    let turn_id = uuid::Uuid::new_v4();
    // Every entry shares `turn_id`, so one carrier (before the `TurnStarted`
    // that precedes them all) binds the whole turn. Nothing to vouch for when
    // there is no message.
    if !msgs.is_empty() {
        emit_seed_provenance(service, session_id, turn_id, origin).await?;
    }
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

/// The real seed `UserMessage` with a caller-chosen `turn_id` (so a
/// [`SessionEvent::SeedProvenance`] can name it before it exists). Byte-for-byte
/// the event `emit_message(.., true)` writes. The `author_user_id` stamp is
/// right for `History`'s replayed prior turns too: that transcript was
/// supplied by *this* caller, so in a room it belongs to them and nobody else.
async fn emit_user_message(
    service: &dyn SessionService,
    session_id: &SessionId,
    turn_id: uuid::Uuid,
    content: MessageContent,
) -> Result<(), FlowError> {
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
        .map(|_| ())
        .map_err(|e| FlowError::Internal(format!("session seed: {e}")))
}

pub(super) async fn emit_message(
    service: &dyn SessionService,
    session_id: &SessionId,
    content: MessageContent,
    is_user: bool,
) -> Result<(), FlowError> {
    if is_user {
        // Also stamps `History`'s replayed prior turns, which is right for the
        // same reason the stamp exists (see `emit_user_message`): that
        // transcript was supplied by *this* caller.
        return emit_user_message(service, session_id, uuid::Uuid::new_v4(), content).await;
    }
    let event = SessionEvent::AssistantMessage {
        turn_id: uuid::Uuid::new_v4(),
        content,
        // Replayed history, not an observed call — no tokens to attribute.
        usage: None,
        at: now_ms(),
        model: None,
        model_provider: None,
        model_generation: None,
    };
    service
        .emit_event(session_id, event)
        .await
        .map(|_| ())
        .map_err(|e| FlowError::Internal(format!("session seed: {e}")))
}
