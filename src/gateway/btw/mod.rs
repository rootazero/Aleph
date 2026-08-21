//! `/btw` side questions — the one derivation every surface must share.
//!
//! A side question must run as its own turn on a *derived* ephemeral
//! session: read-only, in its own busy-queue lane (so it can answer while
//! the main run keeps going), and never appended to the main conversation.
//! This module supplies only the derivations every one of those surfaces has
//! to agree on; the dispatch, tier and retirement machinery lives elsewhere
//! and depends on this, never the other way round.
//!
//! # Why this is not a sixth session knob
//!
//! The five knobs in `CLAUDE.md`'s table all share one mechanism: precedence
//! request > session > global, and a request-carried value is **written back
//! onto the session** so the choice outlives its turn. `btw` is the opposite:
//! it must affect exactly one call. Filing it with the knobs would make a
//! single side question permanently drop the main conversation to `Plan`.
//! It therefore does NOT appear in `turn_*.rs`, in `sessions.patch`'s
//! `knob_validators()`, or in `session_snapshot.rs`, and nothing downstream
//! should add it there.

pub(crate) mod seed;

use crate::routing::session_key::SessionKey;

/// Metadata key that marks a run request as a side question. Whichever
/// surface builds that request must stamp this key at the point it builds it;
/// `resolve` below only recognizes the input, it does not stamp anything
/// itself, so an unstamped request is indistinguishable from an ordinary one
/// all the way down.
pub const BTW_METADATA_KEY: &str = "btw";

/// A resolved `/btw` input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BtwTurn {
    /// The question, with its original case preserved for the model to read
    /// verbatim. Empty when `promote` is set.
    pub question: String,
    /// `/btw promote` — the user's explicit request that the latest side
    /// answer be moved into the main conversation. Explicit by construction:
    /// nothing crosses that boundary without the user asking out loud, which
    /// is why it is a distinct verb rather than a heuristic on the answer.
    pub promote: bool,
}

impl BtwTurn {
    /// Resolve a raw input into a side question.
    ///
    /// **Single source.** Every surface must resolve a side question through
    /// this function; none may re-derive "is this a btw" from its own string
    /// handling. There was a second predicate — `classify_special_slash`'s
    /// `btw` arm in `inbound_router`, a channel-only module the TUI and Panel
    /// cannot reach — and it did not merely duplicate this one: it also
    /// stripped the prefix and substituted a fresh ephemeral key, so a channel
    /// side question reached the engine with neither the stamp nor a derivable
    /// session. It is gone; the router now claims `/btw` by calling this, and
    /// hands the turn on untouched.
    #[must_use]
    pub fn resolve(input: &str) -> Option<Self> {
        let trimmed = input.trim();
        let (head, rest) = match trimmed.split_once(char::is_whitespace) {
            Some((h, r)) => (h, r),
            None => (trimmed, ""),
        };
        // Strip Telegram's `@botname` suffix before comparing.
        let cmd = head.split_once('@').map_or(head, |(c, _)| c);
        if !cmd.strip_prefix('/')?.eq_ignore_ascii_case("btw") {
            return None;
        }
        let body = rest.trim();
        if body.eq_ignore_ascii_case("promote") {
            return Some(Self {
                question: String::new(),
                promote: true,
            });
        }
        if body.is_empty() {
            // An empty side question has nowhere to go.
            return None;
        }
        Some(Self {
            question: body.to_string(),
            promote: false,
        })
    }
}

/// The session a run will execute on: the one it was addressed to, unless it
/// carries the side-question stamp.
///
/// **Every layer that keys anything on "which session is this run" must ask
/// this, not `request.session_key`.** There is more than one such layer and
/// they are not adjacent:
///
/// * `ExecutionEngine::admit_run` claims the session's run slot and, when the
///   claim fails, applies the busy-input policy — so a side question on the main
///   key is steered, interrupted or queued against the running turn.
/// * `busy_queue` registers the arrival ticket, one layer further out, where
///   nothing about the engine looks wrong. Two things go wrong there and they
///   point in opposite directions:
///   1. **The run's own ticket never leaves.** `SessionRunRegistry::try_claim`
///      calls `busy_queue::mark_admitted(claimed_key, run_id)`, which withdraws
///      the ticket from *that* lane and no other. A run that queues on the main
///      lane and then claims the side key withdraws nothing, so its ticket sits
///      at the front of the main lane for the whole side question and every
///      ordinary message in that conversation parks behind it — the promise
///      inverted, and the larger blast radius of the two.
///   2. **What it parks behind, and the slot it takes.** A main-lane ticket
///      waits behind whatever is *waiting* there — a `queue`-mode follower, a
///      steer deferred for attachments or at `max_pending_steering`, a burst —
///      and consumes one of that lane's `max_per_session` slots, so a full main
///      lane rejects the side question outright.
///
///   Note what is NOT among them: a *running* main run holds no ticket at all
///   (`try_claim` withdrew it — the lane is a waiting room, not a run
///   registry), so the bare "one run in flight, no waiters" case was never the
///   problem.
///
/// A **query**, deliberately, even though the engine goes on to write the
/// result into the request. The lane asks before the engine does, and the two
/// must agree; a mutation asked twice would derive the side key OF the side key
/// and land the run somewhere neither layer named. Asking is therefore free —
/// and it has to stay free for the third and fourth ask too, which is why the
/// already-derived case below returns its input unchanged rather than deriving
/// again.
///
/// The predicate is the metadata stamp, never the input text —
/// [`BtwTurn::resolve`] is the one resolver, and a layer re-deriving "is this a
/// btw" from the string would be a second answer to a question that already has
/// one. A layer that sits before whatever stamps must call the stamp first, not
/// invent its own test.
#[must_use]
pub fn execution_session(
    addressed_to: &SessionKey,
    metadata: &std::collections::HashMap<String, String>,
) -> SessionKey {
    if !metadata.contains_key(BTW_METADATA_KEY) || is_side_key(addressed_to) {
        return addressed_to.clone();
    }
    side_key_for(addressed_to)
}

/// The side session derived from `main`, or `None` when `main` **is** one.
///
/// For the paths that have to reach a side question without one having asked
/// for it — stopping, above all. A `/stop` in a conversation has to reach the
/// side question asked from it, because the user's mental model is one
/// conversation and nothing on their screen distinguishes the two sessions.
/// That is the same reasoning that makes stopping by session key walk delegated
/// child runs; a side session is a derived child, and the only reason the walk
/// did not already cover it is that nothing told the walk about the derivation.
///
/// `None` for a key that is already derived is what keeps that walk one level
/// deep: a side session has no side session of its own, and asking for one
/// would mint a phantom key nothing ever ran on.
#[must_use]
pub fn side_session_of(main: &SessionKey) -> Option<SessionKey> {
    (!is_side_key(main)).then(|| side_key_for(main))
}

/// Has the redirect already been applied to this key?
///
/// # This is not the side-question predicate, and the difference matters
///
/// [`crate::tools::turn_context::TurnContext::side_question`] answers "is this
/// turn a side question", and its doc says — correctly — that it is
/// deliberately NOT derived from the key's shape, because the request already
/// carries that fact and a string match on a prefix keeps working right up
/// until someone renames it. That stands, and nothing here weakens it: the
/// metadata stamp is still the only thing that decides `side_question`.
///
/// This answers a different question — "has [`execution_session`] already run
/// on this key" — and the key's shape is the only fact that can answer it. The
/// stamp cannot: it is identical before and after the redirect, by design (it
/// has to survive, or the ceiling would come off).
///
/// # Why the question has to be asked at all
///
/// [`execution_session`] is asked by two arrival layers and then, on one path,
/// by a **re-entry** layer. `steering::build_steering_rescue_request` builds a
/// continuation from `metadata.clone()` and `session_key.clone()` — so a
/// completed side question with an unanswered steering burst re-enters
/// `execute()` carrying the stamp AND the already-derived key. Without this
/// check the rescue derives the side key OF the side key: a third session that
/// nothing can address, retire or list, seeded from the side transcript, which
/// the rescue then answers into where no one is looking.
///
/// The rescue builder strips re-entry residue by name, and that list is exactly
/// the "列举法" shape — a new marker joins the world without joining the list.
/// Stripping the stamp there would not be a fix either: it would drop
/// `side_question` and let the rescue run mutating tools on the side session
/// under the conversation's ordinary tier. The marker must survive; the
/// derivation must not repeat. So the derivation is the thing made idempotent.
fn is_side_key(key: &SessionKey) -> bool {
    matches!(
        key,
        SessionKey::Ephemeral { ephemeral_id, .. } if ephemeral_id.starts_with(SIDE_KEY_PREFIX)
    )
}

/// The `ephemeral_id` prefix every derived side key carries.
///
/// Written once so [`side_key_for`] and [`is_side_key`] cannot disagree about
/// what a side key looks like — the second reader of a format is where a
/// prefix quietly becomes two prefixes.
const SIDE_KEY_PREFIX: &str = "btw-";

/// The side session key for `main`.
///
/// **Single source — write and read must be this same function.** Two call
/// sites each hashing the key "the same way" are byte-identical at epoch 0 and
/// diverge only on a machine that has run `/new`, which is exactly the shape
/// that never reproduces locally.
///
/// The derivation includes the epoch (via `to_key_string`, see
/// `SessionKey::append_epoch`). That buys two things:
///
/// 1. `/new` bumps the epoch, so the derived key changes and the side thread
///    starts empty **by construction** — not because anyone remembered to
///    clear it.
/// 2. The previous side session becomes unaddressable, so retirement only
///    needs to *delete* it, never also to hide it. A missed retirement leaves
///    disk residue, never a crossed side thread — which is what makes the
///    cleanup path allowed to be best-effort.
///
/// Hashed with `Sha256` rather than `std::hash::Hash` / `DefaultHasher`: this
/// id is persisted to disk, and `DefaultHasher`'s algorithm is explicitly
/// unspecified across Rust versions — a toolchain bump would silently reset
/// every user's side thread and orphan its directory, with no error and no
/// red test to catch it. `workspaces/<sha256(session)[..16]>`
/// (`src/sandbox/workspace/path.rs`) is this repo's existing precedent for
/// hashing a `SessionKey` to a filesystem-safe id, though not for this exact
/// width: that precedent keeps 16 *bytes* (32 hex chars) of the digest, while
/// this keeps 16 *hex chars* (8 bytes, 64 bits). The narrower width is a
/// deliberate choice, not an oversight — 64 bits inside a per-agent
/// namespace is not a practical collision risk (a collision would mean two
/// main sessions sharing one side thread), and a shorter id keeps the
/// on-disk name more readable.
#[must_use]
pub fn side_key_for(main: &SessionKey) -> SessionKey {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(main.to_key_string().as_bytes());
    SessionKey::Ephemeral {
        agent_id: main.agent_id().to_string(),
        ephemeral_id: format!("{SIDE_KEY_PREFIX}{}", &hex::encode(digest)[..16]),
    }
}

#[cfg(test)]
mod tests;
