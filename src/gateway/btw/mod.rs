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
    /// handling. The predicate this is meant to replace still lives in
    /// `inbound_router`, a channel-only module, so the TUI and Panel cannot
    /// reach it. That older predicate is still in place and still live for
    /// channels; until it is removed, the two must agree on what counts as a
    /// side question, and this one is the definition.
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
/// * `busy_queue` registers the arrival ticket, and only the FIFO front ticket
///   attempts delivery. A side question queued on the MAIN session's lane waits
///   behind the run it was asked about — which deletes the whole promise ("it
///   answers while the main run keeps going") one layer further out than the
///   engine, where nothing about the engine looks wrong.
/// * `ExecutionEngine::admit_run` claims the session's run slot and, when the
///   claim fails, applies the busy-input policy — so a side question on the main
///   key is steered, interrupted or queued against the running turn.
///
/// A **query**, deliberately, even though the engine goes on to write the
/// result into the request. The lane asks before the engine does, and the two
/// must agree; a mutation asked twice would derive the side key OF the side key
/// and land the run somewhere neither layer named. As a query it is idempotent
/// by construction: same input, same answer, however many layers ask.
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
    if metadata.contains_key(BTW_METADATA_KEY) {
        side_key_for(addressed_to)
    } else {
        addressed_to.clone()
    }
}

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
        ephemeral_id: format!("btw-{}", &hex::encode(digest)[..16]),
    }
}

#[cfg(test)]
mod tests;
