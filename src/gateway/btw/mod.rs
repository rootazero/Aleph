//! `/btw` side questions — the one derivation every surface shares.
//!
//! A side question runs as its own turn on a *derived* ephemeral session:
//! read-only, in its own busy-queue lane (so it answers while the main run
//! keeps going), and never appended to the main conversation.
//!
//! # Why this is not a sixth session knob
//!
//! The five knobs in `CLAUDE.md`'s table all share one mechanism: precedence
//! request > session > global, and a request-carried value is **written back
//! onto the session** so the choice outlives its turn. `btw` is the opposite:
//! it must affect exactly one call. Filing it with the knobs would make a
//! single side question permanently drop the main conversation to `Plan`.
//! It therefore does NOT appear in `turn_*.rs`, in `sessions.patch`'s
//! `knob_validators()`, or in `session_snapshot.rs`, and no later task
//! should add it there.

use crate::routing::session_key::SessionKey;

/// Metadata key stamped on a run request that is a side question.
pub const BTW_METADATA_KEY: &str = "btw";

/// A resolved `/btw` input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BtwTurn {
    /// The question, with its original case preserved for the model to read
    /// verbatim. Empty when `promote` is set.
    pub question: String,
    /// `/btw promote` — move the latest side answer into the main
    /// conversation. Explicit by construction: nothing crosses that boundary
    /// without the user asking out loud.
    pub promote: bool,
}

impl BtwTurn {
    /// Resolve a raw input into a side question.
    ///
    /// **Single source.** Every surface calls this one function; none of them
    /// re-derives "is this a btw" from its own string handling. The predicate
    /// that this replaced lived in `inbound_router`, a channel-only module, so
    /// the TUI and Panel could not reach it at all.
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
/// 2. The previous side session becomes unaddressable, so the retirement hook
///    only has to *delete* it, never also to hide it. A missed retirement
///    leaves disk residue, never a crossed side thread.
///
/// Hashed with `Sha256` rather than `std::hash::Hash` / `DefaultHasher`: this
/// id is persisted to disk (`workspaces/<sha256(session)[..16]>` is this
/// repo's existing precedent for exactly this shape), and `DefaultHasher`'s
/// algorithm is explicitly unspecified across Rust versions — a toolchain
/// bump would silently reset every user's side thread and orphan its
/// directory, with no error and no red test to catch it.
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
