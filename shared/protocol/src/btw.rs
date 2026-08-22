//! `/btw` — the one resolver every surface shares.
//!
//! # Why this type lives in the protocol crate rather than in core
//!
//! "Is this input a side question" has to be answered identically by the
//! channel router, the execution engine, and every thin client that offers a
//! `/btw` affordance of its own. Two of those live in `alephcore`; the TUI
//! does not and may not — `interfaces/tui/Cargo.toml` says so in its own
//! words ("This crate MUST NOT depend on alephcore"). A client that re-derived
//! the predicate from its own string handling would be a second answer to a
//! question that already has one, and the second answer drifts first: the
//! resolver this replaced was itself a de-duplication of exactly that
//! (`classify_special_slash`'s `btw` arm, which did not merely duplicate the
//! test but also stripped the prefix and substituted a fresh ephemeral key).
//!
//! So the type moves to the crate both sides already depend on, and
//! `alephcore::gateway::btw` re-exports it — the shape this repo uses whenever
//! a contract is needed on both sides of the `alephcore` boundary
//! (`session_thread`, `providers`, `tool_permissions`). Every existing
//! core-side path keeps its spelling; nothing gained a copy.
//!
//! What deliberately did **not** move: `side_key_for` / `execution_session` /
//! `BTW_METADATA_KEY`. Those derive and stamp server-side state — the side key
//! hashes the main key *including its epoch*, which no client holds — and a
//! client that computed one would address a session the server has never heard
//! of. A client identifies a side question's traffic by the **run id** the
//! gateway handed it back, never by re-deriving the key.

use serde::{Deserialize, Serialize};

/// A resolved `/btw` input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
