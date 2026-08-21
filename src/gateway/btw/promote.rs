//! `/btw promote` — the one crossing back into the main conversation.
//!
//! [`seed`](super::seed) carries the main conversation *into* the side thread,
//! silently and on every question. This module is the only traffic in the other
//! direction, and it is the opposite in every way that matters: it happens once,
//! it happens because the user asked out loud, and what it carries is
//! **labelled as not the user's own words** all the way into the prompt.
//!
//! # What "the latest side exchange" means
//!
//! The latest one that actually *completed*. Two things make that stricter than
//! "the last question and the last answer":
//!
//! * The TUI supersedes a side question client-side — a superseded question is
//!   filed with whatever text had arrived while its run may still be going. The
//!   server has to refuse to promote that half-answer; the user asked for the
//!   answer, and a truncated one reads exactly like a complete one once it is
//!   sitting in the main transcript with no marker on it.
//! * A side session's log is not only its own. [`super::seed`] copies the main
//!   conversation's settled prefix into it, so the side log holds main turns
//!   too, and promoting one of those would push a slice of the main
//!   conversation back into the main conversation.
//!
//! Both are answered by the same structural fact, which is why this reads turn
//! markers rather than counting messages: **the side session's own turns open
//! with a [`SessionEvent::TurnStarted`] and close with a
//! [`SessionEvent::RunFinished`], and the copied main events carry neither.**
//! `fork::is_prompt_bearing` drops `TurnStarted` outright and keeps
//! `RunFinished` only for the `Cancelled` outcome, so a `RunFinished` naming
//! [`RunOutcome::Completed`] is something only a run that finished *here* can
//! have written.
//!
//! The failure direction is therefore "I found nothing" rather than "here is
//! half of something": a side answer whose terminal marker was lost (the emit
//! is fail-soft) is unpromotable and says so, which is the harmless half of the
//! trade.

use crate::session::events::{RunOutcome, SessionEvent, SessionEventRecord};
use crate::session::service::{SessionId, SessionService};

/// One completed question-and-answer from the side thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SideExchange {
    /// The question as the user asked it, with the `/btw` prefix resolved off
    /// by the one resolver rather than by a second string pass here.
    pub(crate) question: String,
    /// The final assistant text of that turn.
    pub(crate) answer: String,
}

/// The latest exchange in `records` that the side session finished answering.
///
/// Pure — no store, no runtime — so the whole boundary rule above is assertable
/// without building a session, which is what makes it a rule rather than an
/// intention.
pub(crate) fn latest_complete_exchange(records: &[SessionEventRecord]) -> Option<SideExchange> {
    let opens: Vec<usize> = records
        .iter()
        .enumerate()
        .filter(|(_, r)| matches!(r.event, SessionEvent::TurnStarted { .. }))
        .map(|(i, _)| i)
        .collect();

    for (nth, &start) in opens.iter().enumerate().rev() {
        // A turn ends when the next one opens or when the run does
        // (`SessionEvent::TurnStarted`'s own doc). Slicing on the next opener
        // is what keeps a later turn's assistant text from being read as this
        // turn's answer.
        let end = opens.get(nth + 1).copied().unwrap_or(records.len());
        let span = &records[start..end];
        if !completed_here(span) {
            continue;
        }
        if let Some(exchange) = exchange_in(span) {
            return Some(exchange);
        }
    }
    None
}

/// Did a run that executed on THIS session finish this turn successfully?
fn completed_here(span: &[SessionEventRecord]) -> bool {
    span.iter().any(|r| {
        matches!(
            r.event,
            SessionEvent::RunFinished {
                outcome: RunOutcome::Completed,
                ..
            }
        )
    })
}

/// The question and the final answer inside one closed turn.
///
/// The question is the turn's **first** real user message: the harness writes
/// its own scaffolding onto this role too (grace nudges, MAX_STEPS hints), and
/// those land later in the turn and are flagged `synthetic`.
///
/// The answer is the **last** assistant message with text in it. A tool-assisted
/// turn emits one `AssistantMessage` per Think step, and the intermediate ones
/// are the tool-calling steps — frequently text-free, and never the answer.
fn exchange_in(span: &[SessionEventRecord]) -> Option<SideExchange> {
    let question = span.iter().find_map(|r| match &r.event {
        SessionEvent::UserMessage {
            content,
            synthetic: false,
            ..
        } if !content.text.trim().is_empty() => Some(content.text.clone()),
        _ => None,
    })?;
    let answer = span.iter().rev().find_map(|r| match &r.event {
        SessionEvent::AssistantMessage { content, .. } if !content.text.trim().is_empty() => {
            Some(content.text.clone())
        }
        _ => None,
    })?;
    Some(SideExchange {
        question: asked_question(&question),
        answer,
    })
}

/// The question without its `/btw` prefix.
///
/// Resolved through [`BtwTurn`](aleph_protocol::btw::BtwTurn) — the one
/// resolver — rather than by stripping four characters here. A second string
/// pass would be a second answer to "is this a side question and what did it
/// ask", and it would be the one that stops recognising `/btw@botname`.
///
/// Anything the resolver declines is passed through unchanged: a side session's
/// log can hold a user message that was never a `/btw` (a mid-turn steer), and
/// the honest rendering of that is its own text.
fn asked_question(raw: &str) -> String {
    aleph_protocol::btw::BtwTurn::resolve(raw)
        .filter(|turn| !turn.question.is_empty())
        .map_or_else(|| raw.to_string(), |turn| turn.question)
}

/// Read the latest completed side exchange and append it to the main
/// conversation as a carrier the prompt layer can tell apart from user speech.
///
/// `Ok(None)` is a real answer — "there is nothing to promote" — and not a
/// swallowed failure: a store fault comes back as `Err` so the receipt can say
/// which of the two happened. The two must not collapse into one, or a broken
/// side log reads to the user as an empty one and they stop asking.
///
/// # The append is one event, deliberately
///
/// [`SessionEvent::synthetic_user`] is the single source for "a user-role
/// message with no human behind it": `synthetic: true` keeps the prompt builder
/// from wrapping the carrier a second time in the interjection fence (which
/// would re-classify it as words the user typed — the entire failure this
/// carrier exists to prevent), and `author_user_id: None` keeps a room from
/// attributing it to whoever happened to type `/btw promote`.
///
/// That one event reaches both readers this feature owes: the model reads
/// `session_events` directly, and `MessageProjector` — an observer on this same
/// emit — materialises it into `messages`, which is what a reload replays. No
/// second append, and therefore no way for the two faces to disagree about what
/// crossed.
pub(crate) async fn promote_latest_exchange(
    session: &dyn SessionService,
    side: &SessionId,
    main: &SessionId,
) -> Result<Option<SideExchange>, String> {
    let records = session
        .get_events(side, None, None)
        .await
        .map_err(|e| format!("btw: read side log: {e}"))?;
    let Some(exchange) = latest_complete_exchange(&records) else {
        return Ok(None);
    };
    let carrier =
        crate::thinker::nudges::promoted_side_answer(&exchange.question, &exchange.answer);
    session
        .emit_event(
            main,
            SessionEvent::synthetic_user(uuid::Uuid::new_v4(), carrier),
        )
        .await
        .map_err(|e| format!("btw: append the promoted answer: {e}"))?;
    Ok(Some(exchange))
}

#[cfg(test)]
mod tests;
