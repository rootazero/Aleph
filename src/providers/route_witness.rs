//! Process-global per-session record of what the failover walk *actually dialed*.
//!
//! ## Why this exists
//!
//! The `ModelResolved` event is the only user-visible fallback signal Aleph has
//! — Panel, TUI, CLI and the channel reply notice all render **nothing** unless
//! its `is_fallback` flag is set. Until this module existed that flag was
//! produced by a second, parallel health table living on
//! `MultiProviderRegistry`, which nothing dialed from: it predicted a candidate
//! *before* the request, the prediction never reached the wire, and the failure
//! it recorded was attributed to the predicted provider rather than the one that
//! actually failed. So a real migration lit nothing, and a table that had merely
//! seen failures could light a notice naming a provider that was never tried.
//! That table is gone; this is its replacement, and it reports instead of
//! predicting.
//!
//! Prediction was never fixable here. The walk's real pick depends on route mode
//! and tier gating, live load ordering, round-robin rotation, rate-window
//! saturation and the circuit breaker — none of which a pre-request resolver
//! sees. Two implementations of "which endpoint answers this request" is the
//! shape that produced the `SlotKind` / `tier == Unknown` bug; the walk is the
//! only honest source, so the walk speaks.
//!
//! ## Shape
//!
//! Mirrors [`session_model_handle`](super::session_model_handle): one
//! process-global lock-guarded map, keyed by the canonical `SessionKey` string
//! so writer and reader agree. The writer is
//! [`FailoverProvider`](super::failover::FailoverProvider), which reads the key
//! from `RequestPayload.metadata["session_id"]` — the same value
//! `MeteringProvider` keys per-session cost on. The reader is the gateway
//! `run_loop`, which takes the record once the dispatch returns and emits the
//! corrective `ModelResolved`.
//!
//! ## Why the key goes through [`witness_key`]
//!
//! `run_loop` sends `SessionKey::to_key_string()` as the flow's `session_hint`,
//! `resolve_session`'s `Reuse` arm forwards it verbatim, and `harness_bridge`
//! parses it back with `from_key_string` before the harness stamps
//! `session_id.to_string()` into the payload metadata. **That round trip is not
//! the identity**, and assuming it was is how this seam nearly shipped broken:
//! a `DmScope::PerPeer` key with a non-empty channel serialises to
//! `agent:<a>:dm:<peer>` — dropping the channel — and parses back to a
//! channel-less key that re-serialises as `agent:<a>:peer:<peer>`. Writer and
//! reader would therefore have disagreed on **every channel DM session**
//! (Telegram, Slack, …), which is most real traffic, and the banner would have
//! gone quiet exactly where it is needed most — silently, since a missing
//! witness is indistinguishable from "nothing deviated".
//!
//! So neither side uses its own spelling. Both canonicalise through
//! [`witness_key`], which applies the same parse-and-re-serialise the harness
//! already performs. Fixing the asymmetry in `SessionKey` itself would be the
//! deeper fix, but those strings are persisted session identity — out of scope
//! here, and not required: canonicalisation is idempotent, which is the only
//! property this seam needs.
//!
//! ## What is deliberately NOT recorded
//!
//! * **A mid-run wobble that recovers.** Only the *last* successful dial is
//!   compared against the run's *first* attempt. A run whose second turn fell
//!   over and whose third came back to the primary reports no fallback — it was,
//!   in the end, served by what the caller asked for, and announcing
//!   `X → X` would be noise.
//! * **The nested-chain sentinel.** A `provider_hint` override chain is
//!   `[pinned provider, <the entire global chain>]`; the sentinel is not an
//!   endpoint (see `failover::NESTED_CHAIN_NODE`) and the inner chain records
//!   the real provider itself. Recording the sentinel would publish a provider
//!   name the operator never configured. Consequence, accepted: a pin that is
//!   abandoned in favour of a global chain which then succeeds on *its* first
//!   choice is under-reported here. That is an under-report, never a wrong
//!   report, and the abandonment is still in the log
//!   (`failover: provider unavailable, advancing chain`).
//!
//! In-memory and best-effort by design, exactly like the trace mirror: this must
//! never be able to fail a request or grow without bound. Entries are removed by
//! [`take`]; runs that never reach a taker (subagent and team child sessions get
//! their own session keys) are bounded by [`MAX_TRACKED_SESSIONS`].

use crate::sync_primitives::RwLock;
use std::collections::HashMap;
use std::sync::OnceLock;

/// Upper bound on tracked sessions. Reaching it clears the map rather than
/// refusing further writes: this is best-effort diagnostic state, and a
/// permanently wedged recorder (which refusing writes would produce once enough
/// un-taken child sessions accumulated) is worse than dropping some history.
const MAX_TRACKED_SESSIONS: usize = 256;

/// One endpoint the walk dialed: a provider, and the model it was asked for.
///
/// `model` is `None` when the walk let the provider pick its own configured
/// default — that is what an empty model list on a slot means, and it is a
/// genuinely different statement from "we asked for model X".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dialed {
    /// Provider name — the same key the circuit breaker uses.
    pub provider: String,
    /// Model id stamped on the request, or `None` for the provider's default.
    pub model: Option<String>,
}

impl Dialed {
    /// Build a record for `provider` / `model`.
    pub fn new(provider: impl Into<String>, model: Option<String>) -> Self {
        Self {
            provider: provider.into(),
            model,
        }
    }

    /// How this endpoint reads in a user-facing fallback notice.
    ///
    /// A `None` model means the walk let the provider choose, and no model id
    /// exists to name. Naming the requested one instead would be a lie, and the
    /// bare provider name would read as a model — so say what actually happened.
    #[must_use]
    pub fn label(&self) -> String {
        self.model
            .clone()
            .unwrap_or_else(|| format!("{}'s default model", self.provider))
    }
}

/// What the walk did for one session's run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteWitness {
    /// The first endpoint the walk *attempted* in this run — its own first
    /// choice, which is what would have served the run had nothing failed.
    ///
    /// Attempted, not succeeded: the single most common migration is a primary
    /// that is down for the whole run, where every successful dial is already on
    /// the fallback. Anchoring on the first success would make exactly that case
    /// read as "nothing deviated".
    pub first: Dialed,
    /// The endpoint that answered the most recent successful dial.
    pub served: Dialed,
}

impl RouteWitness {
    /// Whether the run ended up somewhere other than the walk's first choice.
    ///
    /// This is the sole producer of the `is_fallback` flag the four notice
    /// surfaces gate on, so it is deliberately strict: a differing provider *or*
    /// a differing model both count, and a run that came back to its first
    /// choice does not.
    #[must_use]
    pub fn deviated(&self) -> bool {
        self.first != self.served
    }
}

/// Canonical witness key for a session-key string.
///
/// The single answer to "what is this session's witness key", used by both the
/// writer (the failover walk, keying off `metadata["session_id"]`) and the
/// reader (the gateway `run_loop`, keying off its `SessionKey`). It reproduces
/// the parse-and-re-serialise the harness performs on its way to the payload,
/// so the two sides cannot spell the same session differently — see the module
/// doc for the DM key that made this necessary.
///
/// Unparseable input is returned unchanged: such a string never came from
/// `to_key_string`, so there is no canonical form to move it to.
#[must_use]
pub fn witness_key(session_key: &str) -> String {
    crate::routing::session_key::SessionKey::from_key_string(session_key)
        .map_or_else(|| session_key.to_string(), |k| k.to_key_string())
}

static WITNESSES: OnceLock<RwLock<HashMap<String, RouteWitness>>> = OnceLock::new();

fn map() -> &'static RwLock<HashMap<String, RouteWitness>> {
    WITNESSES.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Record a successful dial for `session_key`.
///
/// `attempted` is the first endpoint *this walk* tried; `served` is the one that
/// answered. The earliest `attempted` of a run wins (a run spans many walks, one
/// per Think turn) and the latest `served` wins, so the stored record reads
/// "this run set out for A and last got its answer from B".
pub fn record_success(session_key: &str, attempted: Dialed, served: Dialed) {
    let key = witness_key(session_key);
    let mut guard = map().write().unwrap_or_else(|e| e.into_inner());
    if guard.len() >= MAX_TRACKED_SESSIONS && !guard.contains_key(&key) {
        guard.clear();
    }
    guard
        .entry(key)
        .and_modify(|w| w.served = served.clone())
        .or_insert_with(|| RouteWitness {
            first: attempted,
            served,
        });
}

/// Remove and return `session_key`'s record.
///
/// Taking (rather than reading) is what keeps the map bounded on the happy path
/// and what stops one run's migration from being re-announced by the next.
pub fn take(session_key: &str) -> Option<RouteWitness> {
    map()
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&witness_key(session_key))
}

/// Drop `session_key`'s record without reading it.
///
/// Called at the *start* of a run, because a run that ends in an error never
/// reaches [`take`] — and an inherited record would make the next run in that
/// session announce a migration belonging to the previous one. Clearing on entry
/// covers every terminal path at once, which beats adding a `clear` to each of
/// them and missing one.
pub fn clear(session_key: &str) {
    map()
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&witness_key(session_key));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::session_key::SessionKey;

    /// Keys are namespaced per test so the process-global map cannot make these
    /// order-dependent.
    fn key(name: &str) -> String {
        format!("agent:route-witness-test:{name}")
    }

    /// One turn whose walk succeeded on its own first attempt — the happy path,
    /// and the shape most of these tests are composed from.
    fn clean_dial(session: &str, dialed: Dialed) {
        record_success(session, dialed.clone(), dialed);
    }

    #[test]
    fn a_single_dial_is_not_a_deviation() {
        let k = key("single");
        clean_dial(&k, Dialed::new("openai", Some("gpt-5".into())));

        let w = take(&k).expect("recorded");
        assert_eq!(w.first, w.served);
        assert!(!w.deviated());
    }

    #[test]
    fn a_walk_that_migrated_within_one_turn_is_a_deviation() {
        // The single most common migration: the primary is down, so the turn's
        // only *success* is already on the fallback. `first` is the attempt, not
        // the success, which is the whole reason this case reports at all.
        let k = key("migrated-in-one-turn");
        record_success(
            &k,
            Dialed::new("primary", Some("gpt-5".into())),
            Dialed::new("fallback", Some("claude-sonnet-5".into())),
        );

        let w = take(&k).expect("recorded");
        assert_eq!(w.first.provider, "primary");
        assert_eq!(w.served.provider, "fallback");
        assert!(w.deviated());
    }

    #[test]
    fn the_earliest_attempt_wins_and_the_latest_success_wins() {
        // A run spans many walks (one per Think turn). Turn 1 sets `first`;
        // later turns only move `served`.
        let k = key("earliest-attempt");
        record_success(
            &k,
            Dialed::new("primary", Some("gpt-5".into())),
            Dialed::new("primary", Some("gpt-5".into())),
        );
        record_success(
            &k,
            Dialed::new("later-turn-first-choice", None),
            Dialed::new("fallback", Some("claude-sonnet-5".into())),
        );

        let w = take(&k).expect("recorded");
        assert_eq!(
            w.first,
            Dialed::new("primary", Some("gpt-5".into())),
            "a later turn must not rewrite what the run set out for"
        );
        assert_eq!(w.served.provider, "fallback");
    }

    #[test]
    fn the_first_dial_is_kept_and_the_last_one_wins() {
        let k = key("first-and-last");
        clean_dial(&k, Dialed::new("openai", Some("gpt-5".into())));
        clean_dial(&k, Dialed::new("anthropic", Some("claude-sonnet-5".into())));
        clean_dial(&k, Dialed::new("kimi", Some("kimi-k2".into())));

        let w = take(&k).expect("recorded");
        assert_eq!(w.first, Dialed::new("openai", Some("gpt-5".into())));
        assert_eq!(w.served, Dialed::new("kimi", Some("kimi-k2".into())));
        assert!(w.deviated());
    }

    #[test]
    fn a_run_that_comes_back_to_its_first_choice_reports_no_fallback() {
        // The wobble is real but the run was, in the end, served by what the
        // caller asked for. Announcing `X -> X` would be noise; see the module
        // doc's "deliberately NOT recorded".
        let k = key("recovered");
        clean_dial(&k, Dialed::new("openai", Some("gpt-5".into())));
        clean_dial(&k, Dialed::new("anthropic", Some("claude-sonnet-5".into())));
        clean_dial(&k, Dialed::new("openai", Some("gpt-5".into())));

        let w = take(&k).expect("recorded");
        assert!(!w.deviated());
    }

    #[test]
    fn a_same_provider_model_migration_is_a_deviation() {
        // Sibling-model migration after a model-scoped 429: same provider, and
        // the user still did not get the model the walk first chose.
        let k = key("sibling-model");
        clean_dial(&k, Dialed::new("openai", Some("gpt-5".into())));
        clean_dial(&k, Dialed::new("openai", Some("gpt-5-mini".into())));

        let w = take(&k).expect("recorded");
        assert!(w.deviated());
    }

    #[test]
    fn a_default_model_dial_is_distinct_from_a_named_one() {
        // `None` means "the provider picked its own default", which is not the
        // same statement as "we asked for the model it happens to default to".
        let k = key("default-vs-named");
        clean_dial(&k, Dialed::new("ollama", Some("qwen3".into())));
        clean_dial(&k, Dialed::new("ollama", None));

        let w = take(&k).expect("recorded");
        assert!(w.deviated());
    }

    #[test]
    fn taking_removes_the_record_so_the_next_run_starts_clean() {
        let k = key("take-clears");
        clean_dial(&k, Dialed::new("openai", Some("gpt-5".into())));
        assert!(take(&k).is_some());
        assert!(take(&k).is_none(), "a taken witness must not linger");
    }

    #[test]
    fn clearing_drops_the_record_without_reading_it() {
        let k = key("clear");
        clean_dial(&k, Dialed::new("openai", Some("gpt-5".into())));
        clear(&k);
        assert!(take(&k).is_none());
    }

    #[test]
    fn the_map_stays_bounded_when_records_are_never_taken() {
        // Child sessions (subagents, team members) get their own session keys
        // and no taker, so the un-taken case is the normal one, not an edge.
        for i in 0..(MAX_TRACKED_SESSIONS * 2) {
            clean_dial(&key(&format!("bounded-{i}")), Dialed::new("p", None));
        }
        let len = map().read().unwrap_or_else(|e| e.into_inner()).len();
        assert!(
            len <= MAX_TRACKED_SESSIONS,
            "witness map grew to {len}, past the {MAX_TRACKED_SESSIONS} cap"
        );
    }

    #[test]
    fn overflow_clears_rather_than_wedging_the_recorder() {
        // Refusing writes at the cap would permanently silence the banner once
        // enough un-taken child sessions accumulated.
        for i in 0..(MAX_TRACKED_SESSIONS * 2) {
            clean_dial(&key(&format!("wedge-{i}")), Dialed::new("p", None));
        }
        let k = key("wedge-after-overflow");
        clean_dial(&k, Dialed::new("openai", Some("gpt-5".into())));
        assert!(
            take(&k).is_some(),
            "the recorder must still accept writes after an overflow clear"
        );
    }

    /// Every session-key shape `run_loop` can hand this module.
    fn every_key_shape() -> Vec<SessionKey> {
        use crate::routing::session_key::DmScope;
        vec![
            SessionKey::main("main"),
            SessionKey::dm("main", "telegram", "user123", DmScope::PerPeer),
            SessionKey::dm("main", "slack", "U42", DmScope::PerChannelPeer),
            SessionKey::dm("main", "", "user123", DmScope::PerPeer),
            SessionKey::dm("main", "telegram", "user123", DmScope::Main),
        ]
    }

    #[test]
    fn the_session_key_round_trip_is_not_the_identity() {
        // Documents the trap this module's key handling exists for, so nobody
        // "simplifies" `witness_key` away: a PerPeer DM key with a non-empty
        // channel serialises to `:dm:` (dropping the channel) and parses back to
        // a channel-less key that re-serialises as `:peer:`. Writer and reader
        // spelling the session differently is invisible at runtime — a missing
        // witness looks exactly like "nothing deviated".
        let wire = SessionKey::dm(
            "main",
            "telegram",
            "user123",
            crate::routing::session_key::DmScope::PerPeer,
        )
        .to_key_string();
        let reparsed = SessionKey::from_key_string(&wire)
            .expect("a key produced by to_key_string must parse back")
            .to_key_string();
        assert_ne!(
            reparsed, wire,
            "if this ever becomes the identity, the DM asymmetry was fixed \
             upstream and `witness_key` can be revisited"
        );
    }

    #[test]
    fn canonicalisation_is_idempotent_for_every_key_shape() {
        // The one property the seam needs: however many times a key is passed
        // through, both sides land on the same string.
        for key in every_key_shape() {
            let wire = key.to_key_string();
            let once = witness_key(&wire);
            assert_eq!(
                witness_key(&once),
                once,
                "witness_key must be idempotent for {wire}"
            );
        }
    }

    #[test]
    fn writer_and_reader_agree_on_every_key_shape() {
        // The writer sees what the harness stamped — `from_key_string(hint)`
        // re-serialised. The reader starts from its own `SessionKey`. Both must
        // reach the same slot, including for the DM shapes where the raw
        // strings differ.
        for key in every_key_shape() {
            let reader_side = key.to_key_string();
            let harness_side = SessionKey::from_key_string(&reader_side)
                .map_or_else(|| reader_side.clone(), |k| k.to_key_string());

            clean_dial(&harness_side, Dialed::new("fallback", None));
            assert!(
                take(&reader_side).is_some(),
                "reader could not find the witness written for {reader_side}"
            );
        }
    }
}
