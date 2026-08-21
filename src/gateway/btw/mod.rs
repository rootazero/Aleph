//! `/btw` side questions — the one derivation every surface must share.
//!
//! A side question must run as its own turn on a *derived* ephemeral
//! session: read-only, in its own busy-queue lane (so it can answer while
//! the main run keeps going), and never appended to the main conversation.
//! This module supplies the derivations every one of those surfaces has to
//! agree on, plus the re-export of the one that a thin client needs too:
//! [`BtwTurn`] lives in `aleph_protocol::btw` and is re-exported here, so
//! core-side callers and clients that may not depend on `alephcore` resolve a
//! side question through the same function. The dispatch, tier and retirement
//! machinery lives elsewhere and depends on this, never the other way round.
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

pub(crate) mod promote;
pub(crate) mod seed;

use crate::routing::session_key::SessionKey;

/// Metadata key that marks a run request as a side question. Whichever
/// surface builds that request must stamp this key at the point it builds it;
/// `resolve` below only recognizes the input, it does not stamp anything
/// itself, so an unstamped request is indistinguishable from an ordinary one
/// all the way down.
pub const BTW_METADATA_KEY: &str = "btw";

/// The value [`BTW_METADATA_KEY`] carries when the turn is `/btw promote`
/// rather than a question.
///
/// The stamp is a single string field holding two different things — the
/// question's text, or this sentinel — so it needs an owner rather than a
/// literal at each end. [`is_promote`] is the only reader of the sentinel and
/// `stamp_btw` is the only writer; a second spelling of `"promote"` anywhere
/// is a second answer to "which kind of side turn is this".
///
/// The two cannot collide: [`BtwTurn::resolve`] routes a body that reads
/// `promote` (in any case) to the promote arm, so a stamp carrying a question
/// is never this string.
pub(crate) const PROMOTE_STAMP: &str = "promote";

/// Does this stamped request ask to **promote** rather than to ask?
///
/// The first reader of the stamp's *value*. The other three
/// ([`execution_session`], `resolve_turn_permissions`'s read-only ceiling and
/// `steering::carries_more_than_text`) ask `contains_key`, which is right for
/// them — a promote is still a side turn for the purpose of which lane it uses,
/// what tier it runs at, and whether it may be folded into a running sibling.
/// It is only the *dispatch* that differs, and only this predicate decides it.
///
/// False for an unstamped request by construction, so a surface that forgot to
/// stamp gets an ordinary turn rather than an unasked-for crossing.
#[must_use]
pub(crate) fn is_promote(metadata: &std::collections::HashMap<String, String>) -> bool {
    metadata.get(BTW_METADATA_KEY).map(String::as_str) == Some(PROMOTE_STAMP)
}

/// The one `/btw` resolver, re-exported from the crate both sides share.
///
/// **This is a re-export, not a second definition, and it must stay one.**
/// The resolver moved to [`aleph_protocol::btw`] because a thin client needs
/// the same predicate and cannot depend on `alephcore`
/// (`interfaces/tui/Cargo.toml` forbids it in as many words). Every core-side
/// path keeps spelling it `crate::gateway::btw::BtwTurn`, so the move cost no
/// call site and created no copy — which is the whole point: a client that
/// re-derived "is this a btw" from its own string handling would be a second
/// answer to a question that already has one.
///
/// What did **not** move, and must not: [`side_key_for`] / [`execution_session`]
/// / [`BTW_METADATA_KEY`]. Those derive and stamp server-side state. The side
/// key hashes the main key *including its epoch*, which no client holds; a
/// client that computed one would address a session the server has never heard
/// of, and would get it wrong for the first time only after someone ran
/// `/new`. A client identifies a side question's traffic by the **run id** the
/// gateway handed back from its own `agent.run` call.
///
/// Guarded by [`tests::the_core_side_name_is_the_protocol_crate_s_type`],
/// which is a type-identity check rather than a text scan: re-adding a local
/// `struct BtwTurn` here shadows the re-export and stops that test compiling.
pub use aleph_protocol::btw::BtwTurn;

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

/// Prefix a side answer so it is visually separable from the main run's
/// replies, which arrive in the same conversation.
///
/// A side answer deliberately does NOT wait behind the main session's queued
/// replies: ordering protects a causal chain (reply B may quote reply A), and
/// a side answer is on no such chain. Making it queue would trade the entire
/// value of the feature — an immediate answer — for an ordering property that
/// says nothing about it. The visible cost is that a side answer can land
/// between two main replies; this marker is what makes that legible.
///
/// # Where it is applied
///
/// On the **channel face only**, and never before that face's sanitizer: a
/// marker prepended earlier would be a prefix no model wrote, handed to a pass
/// whose job is stripping model-authored framing. The three faces that deliver
/// a channel run's final text each apply it at the point that text settles —
/// the base `ReplyEmitter`'s outbound chokepoint, its two edit-based finals and
/// its already-settled (`StreamAction::Done`) case, Feishu's streaming card
/// `close`, and Telegram's orchestrated answer-lane `finalize`. Two of those
/// run after `sanitize_llm_output` / `sanitize_final_response`; the Feishu card
/// has no sanitizer of its own (`sanitize_llm_output` has no callers outside
/// `reply_emitter/` — the card accumulates the raw delta), so on that face
/// "after sanitization" is vacuously satisfied rather than arranged.
///
/// All three learn they are a side answer once, at emitter construction, from
/// [`BtwTurn::resolve`] — `BTW_METADATA_KEY` is not stamped until `stamp_btw`
/// runs, which is after the emitter exists on every path, so re-deriving from
/// a string prefix there would be a second answer to a question this module
/// already answers.
///
/// # Where it is deliberately NOT applied, and why
///
/// [`crate::gateway::event_emitter::origin_fanout::OriginFanoutEmitter`] also
/// delivers a run's final reply to a channel, and it has four construction
/// sites. **None of them can carry a side question**, each for its own reason,
/// so none of them marks:
///
/// * `announce_delivery.rs` — both halves are machine-authored. `input` is a
///   `[system] …` literal (`subagent_announce.rs`, `process_announce.rs`) and
///   the metadata map is built fresh from one key, so nothing inherits the
///   stamp and `stamp_btw` resolves the input to `None`.
/// * `resume_coordinator.rs` — `input` is `String::new()` (`FlowInput::Resume`
///   ignores it), which `resolve` rejects, and `resume_metadata` never writes
///   `BTW_METADATA_KEY`.
/// * `execute.rs`'s `spawn_continuation_run` — `continuation_metadata` starts
///   from `carry_policy_metadata`, a four-key allowlist that does not include
///   `BTW_METADATA_KEY`, and the prompt is a goal AUDIT contract or loop tick
///   directive.
/// * `handlers::agent`'s `start_run` — reached only through the Simulated
///   fallback registration of `agent.run` / `chat.send`; the real-engine
///   registrations use a plain `GatewayEventEmitter` with no fan-out at all.
///   `SimpleExecutionEngine` has no btw handling whatsoever — no stamp, no
///   side-session redirect, no read-only ceiling — so on that adapter a
///   `/btw …` is not a side question anywhere in the system.
///
/// A fifth fan-out site owes the same question before it inherits that answer.
#[must_use]
pub(crate) fn format_side_answer(text: &str) -> String {
    format!("💬 {text}")
}

#[cfg(test)]
mod guard_tests;
#[cfg(test)]
mod tests;
