//! Clarification Session Management
//!
//! Tracks pending clarification requests. The agent asks the user something
//! mid-task via the `ask_user` tool (or the `scratchpad` plan-approval gate),
//! parks on a oneshot, and resumes when the user's reply is routed back here by
//! the inbound router or the `clarification.resolve` RPC.
//!
//! This is the clarification twin of [`crate::exec::manager::ExecApprovalManager`]:
//! a per-session registry of pending interactions, each backed by a
//! `oneshot::Sender` the resolver fires.
//!
//! # The cursor, and why it is the plain-text fallback
//!
//! A request carries **N** questions. A rich client answers all N in one call
//! ([`ClarificationManager::resolve_many`]); a plain-text channel — or any
//! client that only understands the legacy single-question projection — answers
//! one per message, and the entry advances a cursor. Neither surface is a
//! degraded mode of the other: the parked tool receives the same
//! [`ClarificationAnswer`] vector either way, and cannot tell which arrived.
//!
//! The surface that receives the answer to question *k* is the one that must
//! deliver question *k+1*, and only it holds a transport — so
//! [`ResolveOutcome::More`] hands the caller the rendered next question rather
//! than the manager trying to send it.
//!
//! # Example
//!
//! ```rust,no_run
//! use std::time::Duration;
//! use alephcore::clarification::{ClarificationManager, ClarificationRequest};
//!
//! # async fn example() {
//! let manager = ClarificationManager::new();
//! let request = ClarificationRequest::text("Which language?");
//! let rx = manager
//!     .register("telegram:bot:123:user", request, Duration::from_secs(600), "")
//!     .await;
//! // ... inbound router calls `manager.resolve(session_key, reply)` ...
//! let result = rx.await;
//! # let _ = result;
//! # }
//! ```

use super::render::{self, RenderedQuestion};
use super::{
    ClarificationAnswer, ClarificationQuestion, ClarificationQuestionView, ClarificationRequest,
    ClarificationResult,
};
use crate::gateway::events::{ClarificationOutcome, GatewayEventFrame};
use crate::sync_primitives::{Arc, AsyncRwLock};
use serde::Serialize;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

/// Default time a clarification waits for the user before timing out.
///
/// Generous on purpose: users type thoughtful answers, unlike a yes/no
/// approval. Mirrors hermes-agent's 600s `clarify` timeout.
///
/// ⚠️ `tools::budget::BUILTIN_TOOL_BUDGETS_MS` gives `ask_user` a **larger**
/// wall clock than this on purpose, so the harness budget never fires first and
/// discards an answer the human already gave. That ordering is pinned by
/// `budget::tests::ask_user_budget_outlives_the_clarification_timeout`.
pub const DEFAULT_CLARIFY_TIMEOUT: Duration = Duration::from_secs(600);

/// One outstanding request, as a client needs to render and answer it.
#[derive(Debug, Clone, Serialize)]
pub struct PendingClarification {
    /// Key to pass back to [`ClarificationManager::resolve`].
    pub session_key: String,
    /// Legacy single-question projection: the prompt the **next** plain reply
    /// answers, i.e. the one at the cursor. A client that understands nothing
    /// but this field still drives the whole request to completion, one
    /// message at a time.
    pub question: String,
    /// Choice labels for that same question; empty for a free-text question.
    pub options: Vec<String>,
    /// Every question in the request, in order — the full-fidelity view, with
    /// per-option descriptions the legacy `options` array cannot carry.
    pub questions: Vec<ClarificationQuestionView>,
    /// How many of `questions` already have answers. A client that just
    /// connected resumes at this index.
    pub answered: usize,
}

/// What [`ClarificationManager::resolve`] did with a reply.
///
/// Replaces the former `bool`. The bool could say "consumed" but not "consumed,
/// and here is what to ask next" — and the caller is the only party holding a
/// transport, so it is the only one that can ask.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveOutcome {
    /// Nothing live was waiting: no such question, already answered, expired,
    /// or its run was aborted. The caller MUST NOT treat the message as
    /// consumed, or the user's text is silently eaten.
    Stale,
    /// The reply was recorded and questions remain.
    More {
        /// The caller delivers this on the transport the reply arrived over.
        next: Box<RenderedQuestion>,
        /// How many questions — `next` included — are still unanswered.
        /// Counted here rather than inferred by each face: "is there more" and
        /// "how much more" are the same fact, and a face that has to re-derive
        /// the second one from a boolean will get it wrong the first time it
        /// matters.
        remaining: usize,
    },
    /// The last question was answered and the parked waiter was unblocked.
    Done,
}

impl ResolveOutcome {
    /// Whether the reply was taken as an answer (and so must not also be
    /// treated as a new user message).
    #[must_use]
    pub const fn consumed(&self) -> bool {
        !matches!(self, Self::Stale)
    }
}

/// A clarification awaiting the user's reply.
struct PendingEntry {
    request: ClarificationRequest,
    /// Answers collected so far, in question order. `answers.len()` is the
    /// cursor: the index of the question the next plain reply answers.
    answers: Vec<ClarificationAnswer>,
    sender: Option<oneshot::Sender<ClarificationResult>>,
    /// Gateway run the question belongs to, so an advance can publish the next
    /// `AskUser` frame against the same correlation the first one used. Empty
    /// when the producer had no run (CLI subcommands, unit tests) — publishing
    /// is skipped rather than faked.
    run_id: String,
    created_at: Instant,
    timeout: Duration,
}

impl PendingEntry {
    fn is_expired(&self) -> bool {
        self.created_at.elapsed() > self.timeout
    }

    /// Whether a waiter is still parked on this entry.
    ///
    /// A closed oneshot receiver PROVES nobody is waiting: the only holder is
    /// the parked tool's future, so a closed channel means that future was
    /// dropped — an aborted / cancelled run. Such an entry is a zombie: it can
    /// never be answered, yet `list_pending` would resurrect it as a card in a
    /// freshly loaded Panel and `has_pending` would let it eat the user's next
    /// message.
    fn is_live(&self) -> bool {
        !self.is_expired() && self.sender.as_ref().is_some_and(|s| !s.is_closed())
    }

    /// The question the next plain reply answers, if any remain.
    fn current(&self) -> Option<&ClarificationQuestion> {
        self.request.questions.get(self.answers.len())
    }
}

/// Announce that the request on `session_key` is over.
///
/// The terminal twin of the `AskUser` frame: without it a client cannot know a
/// question ended and keeps the card (and its Enter hijack) alive — in every
/// window, not just the one that answered. Best-effort: no bus is wired in CLI
/// subcommands and unit tests, and a clarification must still resolve there.
fn publish_ended(session_key: &str, outcome: ClarificationOutcome) {
    let Some(bus) = crate::gateway::event_emitter::gateway_event_bus() else {
        return;
    };
    if let Err(e) = bus.publish_frame(&GatewayEventFrame::ClarificationEnded {
        session_key: session_key.to_string(),
        outcome,
    }) {
        tracing::debug!(error = %e, "failed to publish ClarificationEnded");
    }
}

/// Build the `AskUser` frame for the question at `cursor`.
///
/// The single producer of that frame — `ask_user` publishes the first one and
/// [`ClarificationManager`] publishes each advance, so the legacy
/// `question`/`options` projection and the full `questions` array are derived
/// here exactly once and cannot drift apart.
///
/// `question`/`options` describe the question **at the cursor**, not
/// `questions[0]`: a client that understands only those two fields is answering
/// sequentially, and what it needs to render is what its next reply will
/// answer.
#[must_use]
pub fn ask_user_frame(
    run_id: &str,
    session_key: &str,
    request: &ClarificationRequest,
    cursor: usize,
) -> GatewayEventFrame {
    let current = request.questions.get(cursor).unwrap_or_else(|| {
        request
            .first()
            .expect("a ClarificationRequest built by a constructor is never empty")
    });
    GatewayEventFrame::AskUser {
        run_id: run_id.to_string(),
        // The run's emitter owns the stream sequence counter and an
        // out-of-band producer has no handle on it. Same convention as the
        // operator approval requester's out-of-band frames.
        seq: 0,
        session_key: session_key.to_string(),
        question: current.prompt.clone(),
        options: current.options.iter().map(|o| o.label.clone()).collect(),
        questions: request
            .questions
            .iter()
            .map(ClarificationQuestionView::from)
            .collect(),
        answered: cursor,
    }
}

/// Publish the next question's `AskUser` frame after an advance.
///
/// Skipped when the producer carried no run id: an `AskUser` frame with an
/// empty `run_id` cannot be correlated by any client, and
/// `EventVisibilityIndex` would have nothing to seed from.
fn publish_advance(entry: &PendingEntry, session_key: &str) {
    if entry.run_id.is_empty() {
        return;
    }
    let Some(bus) = crate::gateway::event_emitter::gateway_event_bus() else {
        return;
    };
    let frame = ask_user_frame(
        &entry.run_id,
        session_key,
        &entry.request,
        entry.answers.len(),
    );
    if let Err(e) = bus.publish_frame(&frame) {
        tracing::debug!(error = %e, "failed to publish the next AskUser question");
    }
}

/// Per-session registry of pending clarifications.
///
/// Keyed by channel session key. At most one clarification is pending per
/// session — registering a second one supersedes the first, and the
/// superseded waiter receives [`ClarificationResult::cancelled`] so its
/// parked tool call unblocks instead of hanging.
#[derive(Clone, Default)]
pub struct ClarificationManager {
    pending: Arc<AsyncRwLock<HashMap<String, PendingEntry>>>,
}

impl ClarificationManager {
    /// Create an empty manager.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a clarification for `session_key`.
    ///
    /// Returns a receiver that resolves when every question has been answered
    /// (via [`resolve`](Self::resolve) / [`resolve_many`](Self::resolve_many)),
    /// is superseded by another registration on the same `session_key`, or is
    /// reaped on the next [`register`](Self::register) call after its timeout
    /// expires. There is no background timer; stale entries are cleared
    /// opportunistically by the next registration, mirroring
    /// [`ExecApprovalManager::register_pending`].
    ///
    /// `run_id` correlates the advance frames with the run that asked. Pass
    /// `""` when there is no gateway run — publishing is then skipped rather
    /// than emitting an uncorrelatable frame.
    pub async fn register(
        &self,
        session_key: impl Into<String>,
        request: ClarificationRequest,
        timeout: Duration,
        run_id: impl Into<String>,
    ) -> oneshot::Receiver<ClarificationResult> {
        // Opportunistic sweep: an `ask_user` whose future was dropped (aborted
        // agent run) never reaches its own `cancel`, so its entry would linger
        // forever — `cleanup_expired` has no scheduled caller. Each new
        // registration reaps anything already past its deadline, mirroring
        // [`ExecApprovalManager::register_pending`]. Bounded work, no background
        // task (R10).
        self.cleanup_expired().await;

        let (tx, rx) = oneshot::channel();
        let entry = PendingEntry {
            request,
            answers: Vec::new(),
            sender: Some(tx),
            run_id: run_id.into(),
            created_at: Instant::now(),
            timeout,
        };
        let session_key = session_key.into();
        let mut pending = self.pending.write().await;
        let superseded = match pending.insert(session_key.clone(), entry) {
            Some(mut old) => {
                // Supersede: unblock the old waiter so its tool call doesn't hang.
                if let Some(sender) = old.sender.take() {
                    let _ = sender.send(ClarificationResult::cancelled());
                }
                true
            }
            None => false,
        };
        drop(pending);
        if superseded {
            // Honor the same "one terminal frame per clarification" invariant as
            // resolve/cleanup so clients drop the old card in every window.
            // `Cancelled` is the canonical outcome for "nobody can answer any
            // more (… superseded)" per the `ClarificationOutcome` docstring.
            publish_ended(&session_key, ClarificationOutcome::Cancelled);
        }
        rx
    }

    /// Whether `session_key` has a live pending clarification — one that is
    /// neither expired nor abandoned by its waiter (see [`PendingEntry::is_live`]).
    pub async fn has_pending(&self, session_key: &str) -> bool {
        self.pending
            .read()
            .await
            .get(session_key)
            .is_some_and(PendingEntry::is_live)
    }

    /// Snapshot every live pending clarification.
    ///
    /// The clarification twin of [`ExecApprovalManager::list_pending`]: the
    /// `AskUser` frame is a one-shot push, so a Panel that connects (or
    /// reloads) mid-question would otherwise never learn a tool is parked on
    /// its answer and would strand it until its timeout.
    ///
    /// Results are sorted by `session_key` so the snapshot is stable across
    /// calls — the underlying map is a `HashMap<String, PendingEntry>` whose
    /// iteration order is per-process-random, and a list that reorders
    /// between refreshes would jitter the Panel cards (and any future caller
    /// asserting on card order) for no reason.
    pub async fn list_pending(&self) -> Vec<PendingClarification> {
        let mut out: Vec<PendingClarification> = self
            .pending
            .read()
            .await
            .iter()
            .filter(|(_, e)| e.is_live())
            .map(|(session_key, e)| {
                let current = e.current().unwrap_or_else(|| {
                    e.request
                        .first()
                        .expect("a ClarificationRequest built by a constructor is never empty")
                });
                PendingClarification {
                    session_key: session_key.clone(),
                    question: current.prompt.clone(),
                    options: current.options.iter().map(|o| o.label.clone()).collect(),
                    questions: e
                        .request
                        .questions
                        .iter()
                        .map(ClarificationQuestionView::from)
                        .collect(),
                    answered: e.answers.len(),
                }
            })
            .collect();
        out.sort_unstable_by(|a, b| a.session_key.cmp(&b.session_key));
        out
    }

    /// Resolve one question from the user's reply.
    ///
    /// The reply is interpreted against the question **at the cursor**: for a
    /// menu question a 1-based numeric reply picks that option, a reply
    /// matching an option label/value (case-insensitive) selects it, and
    /// anything else is taken as free text.
    pub async fn resolve(&self, session_key: &str, reply: &str) -> ResolveOutcome {
        self.resolve_many(session_key, std::slice::from_ref(&reply))
            .await
    }

    /// Resolve consecutive questions from `replies`, one per question, starting
    /// at the cursor.
    ///
    /// This is the rich-client path: a Panel that rendered all N questions
    /// posts all N answers at once. Extra replies beyond the last question are
    /// ignored rather than rejected — a client that raced an advance would
    /// otherwise lose the answers it did get right.
    pub async fn resolve_many(&self, session_key: &str, replies: &[&str]) -> ResolveOutcome {
        /// What the write-locked inspection decided, so the acting half runs
        /// with no outstanding borrow of the map.
        enum Step {
            /// No entry at all.
            Absent,
            /// Past its deadline: time the waiter out and say so.
            Expired,
            /// Waiter gone (run cancelled): retire the zombie card.
            Abandoned,
            /// Answers recorded, questions remain.
            More(Box<RenderedQuestion>, usize),
            /// Answers recorded, none remain.
            Complete,
        }

        let mut pending = self.pending.write().await;
        let step = match pending.get_mut(session_key) {
            None => Step::Absent,
            Some(entry) if entry.is_expired() => Step::Expired,
            // An abandoned waiter (run cancelled) can never be unblocked.
            // Reporting anything but `Stale` would let the caller eat the
            // user's message and deliver a follow-up question into the void.
            Some(entry) if !entry.is_live() => Step::Abandoned,
            Some(entry) => {
                for reply in replies {
                    let Some(question) = entry.current() else {
                        break;
                    };
                    let answer = interpret_reply(question, reply);
                    entry.answers.push(answer);
                }
                let cursor = entry.answers.len();
                let total = entry.request.len();
                match entry.current() {
                    Some(question) => {
                        let rendered = render::render(question, cursor, total);
                        publish_advance(entry, session_key);
                        Step::More(Box::new(rendered), total - cursor)
                    }
                    None => Step::Complete,
                }
            }
        };

        match step {
            Step::Absent => ResolveOutcome::Stale,
            Step::Expired | Step::Abandoned => {
                let expired = matches!(step, Step::Expired);
                let mut entry = pending
                    .remove(session_key)
                    .expect("invariant: observed under this same write lock");
                if let Some(sender) = entry.sender.take() {
                    // A closed receiver makes this a no-op, which is exactly
                    // the abandoned case.
                    let _ = sender.send(ClarificationResult::timeout());
                }
                drop(pending);
                // Retiring the zombie is the point: without a terminal frame a
                // cancelled run's card outlives it in every open window until
                // the 600 s deadline.
                publish_ended(
                    session_key,
                    if expired {
                        ClarificationOutcome::Expired
                    } else {
                        ClarificationOutcome::Cancelled
                    },
                );
                ResolveOutcome::Stale
            }
            // The entry stays parked; the caller owns the transport and delivers.
            Step::More(next, remaining) => ResolveOutcome::More { next, remaining },
            Step::Complete => {
                let mut entry = pending
                    .remove(session_key)
                    .expect("invariant: observed under this same write lock");
                let answers = std::mem::take(&mut entry.answers);
                let delivered = match entry.sender.take() {
                    Some(sender) => sender.send(ClarificationResult::answered(answers)).is_ok(),
                    None => false,
                };
                drop(pending);
                if delivered {
                    publish_ended(session_key, ClarificationOutcome::Resolved);
                    ResolveOutcome::Done
                } else {
                    // The waiter vanished between the liveness check and the
                    // send. The answer reached nobody, so the message was not
                    // consumed.
                    ResolveOutcome::Stale
                }
            }
        }
    }

    /// Cancel the pending clarification for `session_key`, if any, unblocking
    /// its waiter with [`ClarificationResult::cancelled`].
    pub async fn cancel(&self, session_key: &str) -> bool {
        let mut pending = self.pending.write().await;
        let Some(mut entry) = pending.remove(session_key) else {
            return false;
        };
        if let Some(sender) = entry.sender.take() {
            let _ = sender.send(ClarificationResult::cancelled());
        }
        drop(pending);
        publish_ended(session_key, ClarificationOutcome::Cancelled);
        true
    }

    /// Retire the clarification on `session_key` **only if nobody is parked on
    /// it any more** — its receiver is closed (the asking run was cancelled, or
    /// hit the engine's deadline, and its whole future was dropped) or it is
    /// already past its own deadline.
    ///
    /// The liveness gate is what makes this safe to fire from a drop guard,
    /// which cannot know whether a successor `ask` has since claimed the same
    /// key: a fresh sibling question is live, so it is never touched.
    ///
    /// Returns whether an entry was retired.
    pub async fn cancel_abandoned(&self, session_key: &str) -> bool {
        let mut pending = self.pending.write().await;
        // Peek first (immutably) so we can distinguish "waiter was abandoned"
        // (`Cancelled` is the canonical outcome) from "entry is past its
        // deadline" (`Expired` matches what `cleanup_expired` would have
        // published and what the waiter observes). Without this branch the
        // model sees a `Cancelled` outcome for a question that was actually
        // timed out, drifting apart from the `cleanup_expired` contract.
        let (is_expired, mut entry) = match pending.get(session_key) {
            Some(e) if !e.is_live() => {
                let expired = e.is_expired();
                let entry = pending
                    .remove(session_key)
                    .expect("invariant: observed under this same write lock");
                (expired, entry)
            }
            _ => return false,
        };
        if let Some(sender) = entry.sender.take() {
            // A closed receiver makes this a no-op for the abandoned case
            // (waiter gone); for an expired entry whose waiter is still
            // parked, the variant on the wire matches what the parked tool
            // would otherwise have observed on its own timeout.
            let _ = sender.send(if is_expired {
                ClarificationResult::timeout()
            } else {
                ClarificationResult::cancelled()
            });
        }
        drop(pending);
        publish_ended(
            session_key,
            if is_expired {
                ClarificationOutcome::Expired
            } else {
                ClarificationOutcome::Cancelled
            },
        );
        true
    }

    /// Whether an entry is registered for `session_key` at all — live, expired
    /// or abandoned.
    ///
    /// Tests only. Production faces ask [`has_pending`](Self::has_pending),
    /// which answers `false` for a zombie as well as for a removed entry and so
    /// cannot witness a retirement.
    #[cfg(test)]
    pub(crate) async fn is_registered(&self, session_key: &str) -> bool {
        self.pending.read().await.contains_key(session_key)
    }

    /// Drop *dead* entries — expired OR abandoned — unblocking live waiters
    /// with a timeout result and announcing the terminal frame for every
    /// reaped session.
    ///
    /// Returns the number of entries reaped. An *abandoned* entry is one whose
    /// parked `oneshot::Receiver` has been dropped (the run was cancelled);
    /// the sender is closed by then, so the post-send is a no-op and the
    /// terminal frame is `Cancelled` to match what the caller already saw on
    /// the run side.
    ///
    /// The opportunistic sweep on each [`register`](Self::register) is the only
    /// way abandoned entries are reaped in practice — they have no scheduled
    /// caller, and a long-running gateway with many cancelled runs would
    /// otherwise leak them in the pending map.
    pub async fn cleanup_expired(&self) -> usize {
        let reaped: Vec<(String, ClarificationOutcome)> = {
            let mut pending = self.pending.write().await;
            let mut reaped = Vec::new();
            pending.retain(|session_key, entry| {
                if entry.is_expired() {
                    if let Some(sender) = entry.sender.take() {
                        let _ = sender.send(ClarificationResult::timeout());
                    }
                    reaped.push((session_key.clone(), ClarificationOutcome::Expired));
                    false
                } else if !entry.is_live() {
                    // Abandoned (sender closed but not past the deadline): the
                    // run was cancelled, the receiver is gone, the send is a
                    // no-op. Tell the client side so the card is retired.
                    entry.sender.take();
                    reaped.push((session_key.clone(), ClarificationOutcome::Cancelled));
                    false
                } else {
                    true
                }
            });
            reaped
        };
        for (session_key, outcome) in &reaped {
            publish_ended(session_key, *outcome);
        }
        reaped.len()
    }
}

/// Maximum length of a user reply before truncation.
const MAX_REPLY_LENGTH: usize = 10_000;

/// Trim `reply` and cap it on a char boundary.
fn normalize(reply: &str) -> &str {
    let trimmed = reply.trim();
    if trimmed.len() <= MAX_REPLY_LENGTH {
        return trimmed;
    }
    let mut end = MAX_REPLY_LENGTH;
    while end > 0 && !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    &trimmed[..end]
}

/// Index of the option `token` names — by 1-based number, or by
/// case-insensitive value/label.
///
/// Case folding is locale-blind (`str::to_lowercase`): fine for ASCII labels
/// and the only behaviour the rendered hint ("Reply with the number **or your
/// answer**") promises. Locale-sensitive folding (Turkish I, German ß) would
/// be a deliberate departure from the current expectation and is left for
/// the caller to opt into.
fn match_option(question: &ClarificationQuestion, token: &str) -> Option<u32> {
    let token = token.trim();
    if let Ok(n) = token.parse::<usize>() {
        if n >= 1 && n <= question.options.len() {
            return u32::try_from(n - 1).ok();
        }
    }
    let lower = token.to_lowercase();
    question
        .options
        .iter()
        .position(|opt| opt.value.to_lowercase() == lower || opt.label.to_lowercase() == lower)
        .and_then(|i| u32::try_from(i).ok())
}

/// Map a user's free-text reply onto a [`ClarificationAnswer`] for `question`.
///
/// `pub(crate)` so the inbound router can reuse the exact same number / label /
/// free-text interpretation when resolving a workflow `clarify` step (which is
/// backed by a durable `coord_task`, not this in-memory registry).
///
/// Free text is never an error. A menu is a shortcut, not a constraint — every
/// surface says so ("Reply with the number **or your answer**"), which is also
/// what makes an explicit "Other" option unnecessary here: codex and hermes
/// both append one because their pickers cannot accept anything else.
pub(crate) fn interpret_reply(
    question: &ClarificationQuestion,
    reply: &str,
) -> ClarificationAnswer {
    let trimmed = normalize(reply);
    let free_text = |value: &str| ClarificationAnswer {
        question_id: question.id.clone(),
        selected_indices: Vec::new(),
        value: value.to_string(),
    };

    if !question.has_options() {
        return free_text(trimmed);
    }

    if question.multi_select {
        // Every token must name an option, or the whole reply is free text.
        // Partial matching would silently drop the part of the answer the
        // interpreter did not understand, which is worse than not matching:
        // the model would be told the user picked a strict subset of what
        // they wrote.
        let tokens: Vec<&str> = trimmed
            .split(',')
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .collect();
        if tokens.is_empty() {
            return free_text(trimmed);
        }
        let mut indices: Vec<u32> = Vec::with_capacity(tokens.len());
        for token in &tokens {
            match match_option(question, token) {
                Some(i) if !indices.contains(&i) => indices.push(i),
                // A duplicate pick is not a failure — dedupe and carry on.
                Some(_) => {}
                None => return free_text(trimmed),
            }
        }
        let value = indices
            .iter()
            .filter_map(|i| question.options.get(*i as usize))
            .map(|o| o.value.clone())
            .collect::<Vec<_>>()
            .join(", ");
        return ClarificationAnswer {
            question_id: question.id.clone(),
            selected_indices: indices,
            value,
        };
    }

    match match_option(question, trimmed) {
        Some(i) => ClarificationAnswer {
            question_id: question.id.clone(),
            selected_indices: vec![i],
            value: question
                .options
                .get(i as usize)
                .map(|o| o.value.clone())
                .unwrap_or_default(),
        },
        None => free_text(trimmed),
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clarification::{
        ClarificationOption, ClarificationQuestion, ClarificationResultType,
    };

    fn text_request() -> ClarificationRequest {
        ClarificationRequest::text("Which language?")
    }

    fn select_request() -> ClarificationRequest {
        ClarificationRequest::select(
            "Pick a style:",
            vec![
                ClarificationOption::new("pro", "Professional"),
                ClarificationOption::new("casual", "Casual"),
            ],
        )
    }

    fn two_question_request() -> ClarificationRequest {
        ClarificationRequest::new(vec![
            ClarificationQuestion::select(
                "env",
                "Where?",
                vec![
                    ClarificationOption::new("staging", "staging"),
                    ClarificationOption::new("prod", "production"),
                ],
            ),
            ClarificationQuestion::text("ticket", "Ticket id?"),
        ])
        .expect("two questions")
    }

    #[tokio::test]
    async fn register_then_resolve_delivers_text_reply() {
        let mgr = ClarificationManager::new();
        let rx = mgr
            .register("sess-a", text_request(), DEFAULT_CLARIFY_TIMEOUT, "")
            .await;
        assert!(mgr.has_pending("sess-a").await);

        assert_eq!(
            mgr.resolve("sess-a", "  Spanish  ").await,
            ResolveOutcome::Done
        );
        let result = rx.await.unwrap();
        assert_eq!(result.value(), Some("Spanish"));
        assert!(!mgr.has_pending("sess-a").await);
    }

    #[tokio::test]
    async fn resolve_select_by_number_picks_option() {
        let mgr = ClarificationManager::new();
        let rx = mgr
            .register("sess-b", select_request(), DEFAULT_CLARIFY_TIMEOUT, "")
            .await;

        assert!(mgr.resolve("sess-b", "2").await.consumed());
        let result = rx.await.unwrap();
        assert_eq!(result.selected_index(), Some(1));
        assert_eq!(result.value(), Some("casual"));
    }

    #[tokio::test]
    async fn resolve_select_by_label_picks_option() {
        let mgr = ClarificationManager::new();
        let rx = mgr
            .register("sess-c", select_request(), DEFAULT_CLARIFY_TIMEOUT, "")
            .await;

        assert!(mgr.resolve("sess-c", "professional").await.consumed());
        let result = rx.await.unwrap();
        assert_eq!(result.selected_index(), Some(0));
        assert_eq!(result.value(), Some("pro"));
    }

    #[tokio::test]
    async fn resolve_select_out_of_range_falls_back_to_text() {
        let mgr = ClarificationManager::new();
        let rx = mgr
            .register("sess-d", select_request(), DEFAULT_CLARIFY_TIMEOUT, "")
            .await;

        // "9" is not a valid 1-based index for a 2-option request.
        assert!(mgr.resolve("sess-d", "9").await.consumed());
        let result = rx.await.unwrap();
        assert_eq!(result.selected_index(), None);
        assert_eq!(result.value(), Some("9"));
    }

    #[tokio::test]
    async fn resolve_unknown_session_is_stale() {
        let mgr = ClarificationManager::new();
        let outcome = mgr.resolve("nope", "anything").await;
        assert_eq!(outcome, ResolveOutcome::Stale);
        assert!(!outcome.consumed(), "a stale reply must not be eaten");
    }

    /// The sequential path: a plain-text surface answers one question per
    /// message, and each answer hands back the next question to deliver.
    #[tokio::test]
    async fn sequential_replies_walk_the_cursor_and_hand_back_the_next_question() {
        let mgr = ClarificationManager::new();
        let rx = mgr
            .register(
                "sess-seq",
                two_question_request(),
                DEFAULT_CLARIFY_TIMEOUT,
                "",
            )
            .await;

        let first = mgr.resolve("sess-seq", "2").await;
        match &first {
            ResolveOutcome::More { next, remaining } => {
                assert_eq!(*remaining, 1, "one question left after the first answer");
                assert!(next.text.contains("Ticket id?"), "{}", next.text);
                // Progress marker: the user is halfway through a queue of two.
                assert!(next.text.contains("(2/2)"), "{}", next.text);
                assert!(next.keyboard.is_none(), "free-text question has no buttons");
            }
            other => panic!("expected More, got {other:?}"),
        }
        assert!(first.consumed());
        // Still parked — the tool has not been unblocked yet.
        assert!(mgr.has_pending("sess-seq").await);

        assert_eq!(
            mgr.resolve("sess-seq", "ALEPH-42").await,
            ResolveOutcome::Done
        );
        let result = rx.await.unwrap();
        assert_eq!(result.result_type, ClarificationResultType::Answered);
        assert_eq!(result.answers.len(), 2);
        assert_eq!(result.answers[0].question_id, "env");
        assert_eq!(result.answers[0].value, "prod");
        assert_eq!(result.answers[1].question_id, "ticket");
        assert_eq!(result.answers[1].value, "ALEPH-42");
    }

    /// The rich-client path: one call carries every answer. Same result vector
    /// as the sequential walk above — the parked tool cannot tell them apart.
    #[tokio::test]
    async fn resolve_many_answers_every_question_at_once() {
        let mgr = ClarificationManager::new();
        let rx = mgr
            .register(
                "sess-batch",
                two_question_request(),
                DEFAULT_CLARIFY_TIMEOUT,
                "",
            )
            .await;

        assert_eq!(
            mgr.resolve_many("sess-batch", &["staging", "ALEPH-7"])
                .await,
            ResolveOutcome::Done
        );
        let result = rx.await.unwrap();
        assert_eq!(result.answers.len(), 2);
        assert_eq!(result.answers[0].value, "staging");
        assert_eq!(result.answers[1].value, "ALEPH-7");
    }

    /// A short batch is not an error — it advances as far as it can and the
    /// caller is told what still needs asking.
    #[tokio::test]
    async fn a_short_batch_advances_and_asks_for_the_rest() {
        let mgr = ClarificationManager::new();
        let _rx = mgr
            .register(
                "sess-short",
                two_question_request(),
                DEFAULT_CLARIFY_TIMEOUT,
                "",
            )
            .await;
        match mgr.resolve_many("sess-short", &["1"]).await {
            ResolveOutcome::More { next, .. } => assert!(next.text.contains("Ticket id?")),
            other => panic!("expected More, got {other:?}"),
        }
    }

    /// Extra replies past the last question are ignored, not rejected: a client
    /// that raced an advance would otherwise lose the answers it got right.
    #[tokio::test]
    async fn an_overlong_batch_ignores_the_surplus() {
        let mgr = ClarificationManager::new();
        let rx = mgr
            .register("sess-long", text_request(), DEFAULT_CLARIFY_TIMEOUT, "")
            .await;
        assert_eq!(
            mgr.resolve_many("sess-long", &["Spanish", "ignored", "also ignored"])
                .await,
            ResolveOutcome::Done
        );
        let result = rx.await.unwrap();
        assert_eq!(result.answers.len(), 1);
        assert_eq!(result.value(), Some("Spanish"));
    }

    /// `list_pending` must describe what the NEXT reply answers, not the first
    /// question of the request — a client that connects mid-walk would
    /// otherwise render a question that is already answered.
    #[tokio::test]
    async fn list_pending_follows_the_cursor() {
        let mgr = ClarificationManager::new();
        let _rx = mgr
            .register(
                "sess-list",
                two_question_request(),
                DEFAULT_CLARIFY_TIMEOUT,
                "",
            )
            .await;

        let before = mgr.list_pending().await;
        assert_eq!(before[0].question, "Where?");
        assert_eq!(before[0].answered, 0);
        assert_eq!(before[0].questions.len(), 2);

        let _ = mgr.resolve("sess-list", "1").await;
        let after = mgr.list_pending().await;
        assert_eq!(after[0].question, "Ticket id?");
        assert_eq!(after[0].answered, 1);
        // The full-fidelity array still carries BOTH questions: `answered` is
        // where the client resumes, not where the list starts.
        assert_eq!(after[0].questions.len(), 2);
    }

    #[tokio::test]
    async fn multi_select_accepts_a_comma_list_and_dedupes() {
        let question = ClarificationQuestion::select(
            "langs",
            "Which languages?",
            vec![
                ClarificationOption::new("rust", "Rust"),
                ClarificationOption::new("go", "Go"),
                ClarificationOption::new("ts", "TypeScript"),
            ],
        )
        .with_multi_select(true);

        let answer = interpret_reply(&question, " 1, 3 , rust ");
        assert_eq!(answer.selected_indices, vec![0, 2]);
        assert_eq!(answer.value, "rust, ts");

        // One unmatched token ⇒ the WHOLE reply is free text. Partial matching
        // would tell the model the user picked a strict subset of what they
        // actually wrote.
        let mixed = interpret_reply(&question, "1, elixir");
        assert!(mixed.is_custom());
        assert_eq!(mixed.value, "1, elixir");
    }

    #[tokio::test]
    async fn registering_again_supersedes_and_cancels_old_waiter() {
        let mgr = ClarificationManager::new();
        let rx_old = mgr
            .register("sess-e", text_request(), DEFAULT_CLARIFY_TIMEOUT, "")
            .await;
        let _rx_new = mgr
            .register("sess-e", text_request(), DEFAULT_CLARIFY_TIMEOUT, "")
            .await;

        // The superseded waiter is cancelled, not left hanging.
        let old = rx_old.await.unwrap();
        assert_eq!(old.result_type, ClarificationResultType::Cancelled);
    }

    #[tokio::test]
    async fn cancel_unblocks_waiter() {
        let mgr = ClarificationManager::new();
        let rx = mgr
            .register("sess-f", text_request(), DEFAULT_CLARIFY_TIMEOUT, "")
            .await;

        assert!(mgr.cancel("sess-f").await);
        let result = rx.await.unwrap();
        assert_eq!(result.result_type, ClarificationResultType::Cancelled);
        assert!(!mgr.has_pending("sess-f").await);
    }

    /// The zombie a cancelled run leaves behind: its receiver is closed, so
    /// nothing can ever answer it and no return path of the parked tool runs to
    /// say so. Retiring it is this method's whole job — and it must retire only
    /// that, because an entry someone is still parked on is a live question.
    #[tokio::test]
    async fn cancel_abandoned_retires_only_an_entry_nobody_is_parked_on() {
        let mgr = ClarificationManager::new();

        // Nothing registered: nothing to retire.
        assert!(!mgr.cancel_abandoned("sess-none").await);

        let rx = mgr
            .register("sess-live", text_request(), DEFAULT_CLARIFY_TIMEOUT, "")
            .await;
        assert!(!mgr.cancel_abandoned("sess-live").await);
        assert!(
            mgr.has_pending("sess-live").await,
            "a question with a waiter parked on it must survive"
        );

        // The run was cancelled: its future, and with it the receiver, is gone.
        drop(rx);
        assert!(mgr.cancel_abandoned("sess-live").await);
        assert!(
            !mgr.is_registered("sess-live").await,
            "the entry must be removed, not merely reported dead"
        );
    }

    /// The re-register race: a cancelled run's guard fires after a fresh `ask`
    /// has already claimed the same session key. Retiring by key alone would
    /// kill the successor's question — the liveness gate is what makes the late
    /// guard a no-op.
    #[tokio::test]
    async fn cancel_abandoned_never_touches_a_successor_ask() {
        let mgr = ClarificationManager::new();
        let dead = mgr
            .register("sess-race", text_request(), DEFAULT_CLARIFY_TIMEOUT, "")
            .await;
        drop(dead);
        let live = mgr
            .register(
                "sess-race",
                two_question_request(),
                DEFAULT_CLARIFY_TIMEOUT,
                "",
            )
            .await;

        assert!(!mgr.cancel_abandoned("sess-race").await);
        assert!(mgr.has_pending("sess-race").await);
        // The successor still answers, which its retired predecessor could not.
        assert!(mgr.resolve("sess-race", "1").await.consumed());
        drop(live);
    }

    #[tokio::test]
    async fn cleanup_expired_times_out_stale_entries() {
        let mgr = ClarificationManager::new();
        let rx = mgr
            .register("sess-g", text_request(), Duration::from_millis(1), "")
            .await;
        tokio::time::sleep(Duration::from_millis(10)).await;

        assert!(!mgr.has_pending("sess-g").await);
        assert_eq!(mgr.cleanup_expired().await, 1);
        let result = rx.await.unwrap();
        assert_eq!(result.result_type, ClarificationResultType::Timeout);
    }

    /// An expired entry answered before anyone reaped it must time out rather
    /// than accept the answer — and must report `Stale`, so its channel does
    /// not swallow the user's message.
    #[tokio::test]
    async fn resolving_an_expired_entry_times_out_and_is_stale() {
        let mgr = ClarificationManager::new();
        let rx = mgr
            .register("sess-h", text_request(), Duration::from_millis(1), "")
            .await;
        tokio::time::sleep(Duration::from_millis(10)).await;

        assert_eq!(
            mgr.resolve("sess-h", "too late").await,
            ResolveOutcome::Stale
        );
        assert_eq!(
            rx.await.unwrap().result_type,
            ClarificationResultType::Timeout
        );
    }

    /// The frame carries the same fact twice — a flat `question`/`options` pair
    /// for clients that predate the structured view, and the full `questions`
    /// array. Two representations of one fact drift the moment they have two
    /// authors, so both are built here, from the same request, and this pins
    /// that the flat half describes the question **at the cursor** rather than
    /// `questions[0]`. Getting that wrong is not cosmetic: a sequential client
    /// renders the question it is about to answer, so pointing it at the first
    /// one would make every reply after the first answer the wrong prompt on
    /// screen.
    #[test]
    fn the_frames_legacy_projection_is_the_question_at_the_cursor() {
        let request = two_question_request();
        let frame = ask_user_frame("run-1", "sess", &request, 1);
        match frame {
            GatewayEventFrame::AskUser {
                run_id,
                session_key,
                question,
                options,
                questions,
                answered,
                ..
            } => {
                assert_eq!(run_id, "run-1");
                assert_eq!(session_key, "sess");
                assert_eq!(question, "Ticket id?", "flat prompt must follow the cursor");
                assert!(options.is_empty(), "the cursor question has no options");
                assert_eq!(answered, 1);
                // Full fidelity keeps BOTH questions and the descriptions the
                // flat array structurally cannot carry.
                assert_eq!(questions.len(), 2);
                assert_eq!(questions[0].id, "env");
                assert_eq!(questions[0].options.len(), 2);
            }
            other => panic!("expected an AskUser frame, got {other:?}"),
        }
    }

    /// An option description is wired onto the option type and rendered by
    /// channels; it reaches every other surface only through this projection.
    #[test]
    fn the_frame_carries_option_descriptions() {
        let request = ClarificationRequest::select(
            "Strategy?",
            vec![
                ClarificationOption::new("in-place", "in-place").with_description("brief downtime"),
                ClarificationOption::new("blue-green", "blue-green"),
            ],
        );
        let GatewayEventFrame::AskUser { questions, .. } = ask_user_frame("r", "s", &request, 0)
        else {
            panic!("expected an AskUser frame");
        };
        assert_eq!(
            questions[0].options[0].description.as_deref(),
            Some("brief downtime")
        );
        assert!(questions[0].options[1].description.is_none());
    }

    /// An out-of-range cursor (a frame minted while a completion was in flight)
    /// must degrade to the first question rather than panic on the way to a
    /// client.
    #[test]
    fn an_out_of_range_cursor_falls_back_to_the_first_question() {
        let request = two_question_request();
        let GatewayEventFrame::AskUser { question, .. } = ask_user_frame("r", "s", &request, 9)
        else {
            panic!("expected an AskUser frame");
        };
        assert_eq!(question, "Where?");
    }

    /// An abandoned waiter (the run was cancelled and dropped the receiver) can
    /// never be unblocked, so a reply aimed at it is NOT consumed — otherwise
    /// the user's next message is eaten by a zombie and a follow-up question is
    /// delivered into the void.
    #[tokio::test]
    async fn an_abandoned_waiter_does_not_eat_the_reply() {
        let mgr = ClarificationManager::new();
        let rx = mgr
            .register(
                "sess-i",
                two_question_request(),
                DEFAULT_CLARIFY_TIMEOUT,
                "",
            )
            .await;
        drop(rx);

        let outcome = mgr.resolve("sess-i", "1").await;
        assert_eq!(outcome, ResolveOutcome::Stale);
        assert!(!outcome.consumed());
    }

    /// The opportunistic sweep on `register` is the only thing that reaps
    /// abandoned entries. Without it, a long-running gateway would leak
    /// entries whose run was cancelled but never reaped.
    #[tokio::test]
    async fn register_reaps_abandoned_entries_opportunistically() {
        let mgr = ClarificationManager::new();
        for i in 0..3 {
            let rx = mgr
                .register(
                    format!("sess-leak-{i}"),
                    text_request(),
                    DEFAULT_CLARIFY_TIMEOUT,
                    "",
                )
                .await;
            drop(rx); // simulate run cancellation
        }
        // Re-register (any session is fine) — the sweep rides along.
        let _rx = mgr
            .register("sess-fresh", text_request(), DEFAULT_CLARIFY_TIMEOUT, "")
            .await;
        // The leaked entries are gone; only the fresh one remains.
        for i in 0..3 {
            assert!(
                !mgr.has_pending(&format!("sess-leak-{i}")).await,
                "abandoned entry {i} must be reaped"
            );
        }
        assert!(mgr.has_pending("sess-fresh").await);
    }
}
