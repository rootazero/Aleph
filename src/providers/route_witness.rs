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
//!   name the operator never configured.
//!
//!   A pin the outer chain *keeps* is no longer lost with it. The walk anchors
//!   on its plan head via [`record_attempt`] before it dials anything, so the
//!   pin is already in the record when the inner chain's [`record_success`]
//!   lands and fills in `served`. Until that anchor existed the inner chain was
//!   the only writer for a pinned run, so a failed pin followed by a failed
//!   *global* primary published an `original_model` naming an endpoint the user
//!   never selected — a wrong report, not the under-report this bullet used to
//!   claim.
//!
//!   A pin that route policy *drops* is still lost, and knowingly so: the
//!   sentinel then leads the plan, both writers exclude it, and the inner chain
//!   becomes the run's only writer again. See `CandidatePlan::anchor` for why
//!   anchoring the dropped head is not obviously the right answer either.
//! * **A walk whose payload carries no `session_id`.** Only the main Think turn
//!   stamps one (`harness::agent::think::build_request_payload`). MoA advisor
//!   dials in particular build a bare `RequestPayload::new(view)`
//!   (`providers::moa::fan_out::run_fan_out`), so an advisor — which shares the
//!   turn's session and would otherwise be free to anchor it — writes nothing
//!   here, before or after the plan-time anchor. The MoA *aggregation* dial does
//!   carry the metadata, and it is the dial that answers the user.
//!
//! ## What an anchor that never gets an answer reads as
//!
//! [`record_attempt`] stores `served` as a copy of the anchor, so a run whose
//! walk was anchored and then failed outright yields `first == served` at
//! [`take`] time and [`RouteWitness::deviated`] is false — no banner. That is
//! the right answer: a run that got no answer was not "served by a fallback",
//! and its failure is surfaced on its own path.
//!
//! In-memory and best-effort by design, exactly like the trace mirror: this must
//! never be able to fail a request or grow without bound. Entries are removed by
//! [`take`]; runs that never reach a taker (subagent and team child sessions get
//! their own session keys) are bounded by [`MAX_TRACKED_SESSIONS`], whose
//! overflow evicts the *least recently written* entry rather than wiping the
//! whole map — a bulk clear used to erase the witnesses of runs still in flight
//! (a session's record is only taken when its run ends, so every in-flight run
//! was collateral).
//!
//! Concurrency note: the key is the *session*, not the run — two runs sharing
//! one session key interleave on one record (run B's entry-time [`clear`] drops
//! run A's witness). Making the key run-scoped would need a run id in
//! `RequestPayload.metadata`, which today carries only `session_id`
//! (`harness::agent::think::build_request_payload`); threading one through the
//! harness↔gateway layers is out of scope for a best-effort diagnostic.

use crate::sync_primitives::RwLock;
use std::collections::HashMap;
use std::sync::OnceLock;

/// Upper bound on tracked sessions. Reaching it evicts the least recently
/// written entry rather than refusing further writes: this is best-effort
/// diagnostic state, and a permanently wedged recorder (which refusing writes
/// would produce once enough un-taken child sessions accumulated) is worse than
/// dropping the stalest history.
const MAX_TRACKED_SESSIONS: usize = 256;

/// Which model an endpoint was asked for — three genuinely different
/// statements, which is why this is not an `Option<String>`.
///
/// Collapsing [`Unresolved`](Self::Unresolved) into
/// [`ProviderDefault`](Self::ProviderDefault) would make "no model has been
/// chosen for this endpoint yet" render as "we deliberately let the provider
/// choose", and [`RouteWitness::deviated`] would then compare a model that was
/// never dialed against one that was — the wrong-report shape this module
/// exists to avoid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DialedModel {
    /// The walk stamped this model id on the request.
    Named(String),
    /// The walk sent no model id at all: the provider picks its own configured
    /// default. That is what an empty model list on a slot means.
    ProviderDefault,
    /// The endpoint is known but no request was ever built for it — the
    /// plan-time anchor ([`record_attempt`]).
    ///
    /// Which model that slot *would* dial is only settled later in the walk, by
    /// the capability floor (`retain_capable_models`) and the cooling sideline
    /// (`drop_cooling_models`). Guessing the catalog head here would name a
    /// model the walk then filtered out. The anchor is upgraded in place the
    /// moment that same endpoint is actually dialed, so this state reaches a
    /// reader only for an endpoint the run never dialed at all — which is not
    /// an edge case but the *headline* one: a primary passed over pre-dial by
    /// an open breaker, a rate ceiling or a 429 pacing park is precisely the
    /// migration this module exists to announce, and it never gets a model.
    Unresolved,
}

/// One endpoint the walk dialed: a provider, and the model it was asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dialed {
    /// Provider name — the same key the circuit breaker uses.
    pub provider: String,
    /// What model this endpoint was asked for.
    pub model: DialedModel,
}

impl Dialed {
    /// Build a record for an endpoint the walk built a request for; `model` is
    /// `None` when that request carried no model id.
    pub fn new(provider: impl Into<String>, model: Option<String>) -> Self {
        Self {
            provider: provider.into(),
            model: model.map_or(DialedModel::ProviderDefault, DialedModel::Named),
        }
    }

    /// Build the plan-time anchor for `provider`: the endpoint the walk set out
    /// for, before any request was built for it.
    pub fn endpoint(provider: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model: DialedModel::Unresolved,
        }
    }

    /// The model id stamped on the request, if there was one.
    #[must_use]
    pub fn model_id(&self) -> Option<&str> {
        match &self.model {
            DialedModel::Named(m) => Some(m),
            DialedModel::ProviderDefault | DialedModel::Unresolved => None,
        }
    }

    /// How this endpoint reads in a user-facing fallback notice.
    ///
    /// Neither model-less state has a model id to name. Naming the requested one
    /// instead would be a lie, and the bare provider name would read as a model
    /// — so each says what actually happened.
    ///
    /// [`Unresolved`](DialedModel::Unresolved) spells out its *kind* because of
    /// where it lands: it is the label of the anchor half of the headline
    /// notice (a primary skipped before any dial), it is carried in the
    /// model-shaped `ModelInfo::original_model`, and three of the four
    /// renderers print that field where a model id belongs
    /// (`"model fallback: {original} → {model}"`). "openai (never dialed)"
    /// there reads as a model named openai; "provider openai (never dialed)"
    /// cannot.
    #[must_use]
    pub fn label(&self) -> String {
        match &self.model {
            DialedModel::Named(m) => m.clone(),
            DialedModel::ProviderDefault => format!("{}'s default model", self.provider),
            DialedModel::Unresolved => format!("provider {} (never dialed)", self.provider),
        }
    }
}

/// What the walk did for one session's run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteWitness {
    /// The first endpoint the walk *set out for* in this run — its own first
    /// choice, which is what would have served the run had nothing failed.
    ///
    /// Set out for, not succeeded and not even dialed. The single most common
    /// migration is a primary that is down for the whole run, and every reason
    /// the walk has to pass a candidate over — an open circuit, a rate ceiling,
    /// a denied escalation, a 429 pacing park — is settled *before* the first
    /// request is built. Anchoring on the first success would make that case
    /// read as "nothing deviated"; anchoring on the first dial would do the same
    /// from the second run of an outage onward, because by then the first dial
    /// *is* the fallback.
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
        if self.first.provider != self.served.provider {
            return true;
        }
        match (&self.first.model, &self.served.model) {
            // Same endpoint, and one side is a plan-time anchor whose model was
            // never settled. There is no model pair to compare, so the only
            // honest answer is "nothing observed deviated" — claiming a
            // migration here would render a model id the walk never dialed.
            (DialedModel::Unresolved, _) | (_, DialedModel::Unresolved) => false,
            (a, b) => a != b,
        }
    }

    /// Fold one write into this record.
    ///
    /// The two writers ([`record_attempt`] and [`record_success`]) share this
    /// one merge rule so they cannot disagree about what `first` means:
    ///
    /// * `first` is written once, by whoever gets here first, and is thereafter
    ///   only ever *refined* — an [`Unresolved`](DialedModel::Unresolved) anchor
    ///   adopts the model of the first real dial of that same endpoint. A dial
    ///   of any other endpoint leaves it alone: the run still set out for the
    ///   anchor, which is the whole point of anchoring before the dial.
    /// * `served` follows the latest answer. While no answer has arrived it
    ///   mirrors `first`, so an anchored run that never gets one reads as
    ///   "nothing deviated" instead of inventing a migration.
    fn absorb(&mut self, attempted: &Dialed, served: Option<Dialed>) {
        if self.first.model == DialedModel::Unresolved
            && self.first.provider == attempted.provider
            && attempted.model != DialedModel::Unresolved
        {
            // `served == first` is exactly "nothing has answered yet"; keep the
            // two in step so the mirror above survives the refinement.
            if self.served == self.first {
                self.served = attempted.clone();
            }
            self.first = attempted.clone();
        }
        if let Some(served) = served {
            self.served = served;
        }
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

/// The bounded store behind the process-global map. Holds an insertion clock
/// alongside each record so overflow can evict the *least recently written*
/// entry (LRU) instead of clearing everything — the old bulk `clear()` at the
/// cap wiped the witnesses of runs still in flight, which are exactly the
/// records about to be read.
///
/// A record's age refreshes on every write: a session with a run in flight
/// (many turns, one `record_success` each) is the *freshest* thing in the map
/// and therefore the last to be evicted.
#[derive(Default)]
struct BoundedWitnesses {
    /// Monotonic insertion clock; each write takes the next tick.
    seq: u64,
    map: HashMap<String, (u64, RouteWitness)>,
}

impl BoundedWitnesses {
    /// The single insert path, so both writers share one LRU discipline and one
    /// merge rule. `served` is `None` for the plan-time anchor: nothing has
    /// answered yet, so such a write may create an entry but never rewrites the
    /// `served` an earlier turn already filled in.
    fn record(&mut self, key: String, attempted: Dialed, served: Option<Dialed>) {
        if !self.map.contains_key(&key) && self.map.len() >= MAX_TRACKED_SESSIONS {
            // Evict exactly the stalest entry. O(n) at the cap only, and the
            // cap is the exceptional path — cheap insurance against wedging.
            if let Some(oldest) = self
                .map
                .iter()
                .min_by_key(|(_, (tick, _))| *tick)
                .map(|(k, _)| k.clone())
            {
                self.map.remove(&oldest);
            }
        }
        self.seq += 1;
        let tick = self.seq;
        match self.map.get_mut(&key) {
            Some((t, witness)) => {
                // An anchor for a session already in the map states nothing new
                // about recency; only a real answer refreshes the eviction age.
                if served.is_some() {
                    *t = tick;
                }
                witness.absorb(&attempted, served);
            }
            None => {
                let first = attempted;
                let served = served.unwrap_or_else(|| first.clone());
                self.map.insert(key, (tick, RouteWitness { first, served }));
            }
        }
    }

    fn take(&mut self, key: &str) -> Option<RouteWitness> {
        self.map.remove(key).map(|(_, w)| w)
    }

    fn clear(&mut self, key: &str) {
        self.map.remove(key);
    }

    /// Undo a plan-time anchor for `provider`, and only that.
    ///
    /// Three conditions must all hold, because this is the one path that
    /// *removes* a record rather than folding into it: the anchor still names
    /// `provider`, it was never refined by a dial (`Unresolved`), and nothing
    /// has answered yet (`served` is still the mirror). A run spans many walks
    /// — one per Think turn — so without the last two a turn-2 denial would
    /// delete turn 1's real, dialed record.
    fn retract(&mut self, key: &str, provider: &str) {
        let is_bare_anchor = self.map.get(key).is_some_and(|(_, w)| {
            w.first.provider == provider
                && w.first.model == DialedModel::Unresolved
                && w.served == w.first
        });
        if is_bare_anchor {
            self.map.remove(key);
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.map.len()
    }
}

static WITNESSES: OnceLock<RwLock<BoundedWitnesses>> = OnceLock::new();

fn map() -> &'static RwLock<BoundedWitnesses> {
    WITNESSES.get_or_init(|| RwLock::new(BoundedWitnesses::default()))
}

/// Record a successful dial for `session_key`.
///
/// `attempted` is the first endpoint *this walk* tried; `served` is the one that
/// answered. The earliest `attempted` of a run wins (a run spans many walks, one
/// per Think turn) and the latest `served` wins, so the stored record reads
/// "this run set out for A and last got its answer from B".
pub fn record_success(session_key: &str, attempted: Dialed, served: Dialed) {
    let key = witness_key(session_key);
    map()
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .record(key, attempted, Some(served));
}

/// Anchor `session_key`'s run on an endpoint the walk set out for, before any
/// dial.
///
/// The walk calls this twice. Once with [`Dialed::endpoint`] the moment its
/// candidate plan exists — every reason to pass a candidate over (open circuit,
/// rate ceiling, denied escalation, 429 pacing park) is decided before the first
/// request is built, so waiting for a dial anchors on the fallback and reports
/// an ongoing outage as "nothing deviated". And once more with the fully
/// resolved [`Dialed`] of the first endpoint it actually dials, which refines
/// that anchor's model (see [`RouteWitness::absorb`]).
///
/// The earliest anchor of a run wins: a later walk (there is one per Think turn)
/// and a chain nested behind `NESTED_CHAIN_NODE` both find the entry present and
/// leave `first` alone.
pub fn record_attempt(session_key: &str, attempted: Dialed) {
    let key = witness_key(session_key);
    map()
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .record(key, attempted, None);
}

/// Undo `session_key`'s plan-time anchor on `provider` — the walk set out for
/// that endpoint and then learned it does not get to have it.
///
/// The one caller is the escalation gate's *denial* arm. An approval-gated
/// cross-tier head is anchored like any other (the cheap gates ahead of the
/// prompt — open breaker, rate ceiling — pass it over without ever asking, and
/// being skipped for being dead is a migration worth announcing), but an actual
/// refusal is not a migration: telling someone who just declined to borrow the
/// cloud that their run "moved off" it is the wrong-report shape this module
/// exists to avoid. So the anchor is written first and withdrawn on the one
/// event that invalidates it, rather than never written at all.
///
/// A no-op unless the record is still that bare anchor — see
/// [`BoundedWitnesses::retract`].
pub fn retract(session_key: &str, provider: &str) {
    let key = witness_key(session_key);
    map()
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .retract(&key, provider);
}

/// Remove and return `session_key`'s record.
///
/// Taking (rather than reading) is what keeps the map bounded on the happy path
/// and what stops one run's migration from being re-announced by the next.
pub fn take(session_key: &str) -> Option<RouteWitness> {
    map()
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .take(&witness_key(session_key))
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
        .clear(&witness_key(session_key));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::session_key::SessionKey;

    /// Keys are namespaced per test so no two tests write the same entry.
    ///
    /// ⚠️ That buys less than it reads like. Namespacing rules out COLLISION;
    /// it does nothing about CAPACITY, which every test in this binary shares.
    /// Two tests here used to fill the process-global map with
    /// `MAX_TRACKED_SESSIONS * 2` entries, and the LRU then evicted whatever
    /// the rest of the binary had in flight -- including the eleven witness
    /// tests in `failover::tests`, each of which writes a record and reads it
    /// back one `.await` later. They now drive a private [`BoundedWitnesses`]
    /// (判据 §3: the guard covered only the shape it recognised).
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

    /// Fill a private store past the cap, the way an un-taken child session
    /// backlog would.
    ///
    /// Private, not the process-global map: overflowing the shared store is how
    /// these two tests used to evict other tests' in-flight records. Nothing is
    /// lost by moving them -- the global map IS a `BoundedWitnesses` and
    /// `record` is its single insert path (see its doc), so the bound the
    /// global map has is the bound proven here.
    fn flooded_store(prefix: &str) -> BoundedWitnesses {
        let mut store = BoundedWitnesses::default();
        for i in 0..(MAX_TRACKED_SESSIONS * 2) {
            let d = Dialed::new("p", None);
            store.record(key(&format!("{prefix}-{i}")), d.clone(), Some(d));
        }
        store
    }

    #[test]
    fn the_map_stays_bounded_when_records_are_never_taken() {
        // Child sessions (subagents, team members) get their own session keys
        // and no taker, so the un-taken case is the normal one, not an edge.
        let store = flooded_store("bounded");
        let len = store.len();
        assert!(
            len <= MAX_TRACKED_SESSIONS,
            "witness map grew to {len}, past the {MAX_TRACKED_SESSIONS} cap"
        );
    }

    #[test]
    fn overflow_clears_rather_than_wedging_the_recorder() {
        // Refusing writes at the cap would permanently silence the banner once
        // enough un-taken child sessions accumulated.
        let mut store = flooded_store("wedge");
        let k = key("wedge-after-overflow");
        let d = Dialed::new("openai", Some("gpt-5".into()));
        store.record(k.clone(), d.clone(), Some(d));
        assert!(
            store.take(&k).is_some(),
            "the recorder must still accept writes after an overflow clear"
        );
    }

    /// Eviction unit tests drive `BoundedWitnesses` directly: the process-global
    /// map is shared with every other test in this binary, so asserting *which*
    /// entry was evicted requires a private store.
    fn store_with(keys: &[&str]) -> BoundedWitnesses {
        let mut store = BoundedWitnesses::default();
        for (i, k) in keys.iter().enumerate() {
            store.record(
                (*k).to_string(),
                Dialed::new(format!("p{i}"), None),
                Some(Dialed::new(format!("p{i}"), None)),
            );
        }
        store
    }

    #[test]
    fn retract_removes_a_bare_anchor_and_nothing_else() {
        // The escalation denial arm's primitive. Three guards, one per way a
        // blind `remove` would delete a record that is still wanted.
        let mut store = BoundedWitnesses::default();

        // 1. The bare anchor it is meant for.
        store.record("k".to_string(), Dialed::endpoint("openai"), None);
        store.retract("k", "openai");
        assert!(store.take("k").is_none(), "the withdrawn anchor is gone");

        // 2. An anchor already refined by a real dial of that endpoint — this
        //    is a LATER turn of the same run, and turn 1 really did dial it.
        store.record("k".to_string(), Dialed::endpoint("openai"), None);
        store.record(
            "k".to_string(),
            Dialed::new("openai", Some("gpt-5".into())),
            None,
        );
        store.retract("k", "openai");
        assert!(
            store.take("k").is_some(),
            "a dialed record must survive a later turn's denial"
        );

        // 3. An anchor that already has an answer, and an anchor for someone
        //    else. Neither is this candidate's bare anchor.
        store.record(
            "k".to_string(),
            Dialed::endpoint("openai"),
            Some(Dialed::new("ollama", None)),
        );
        store.retract("k", "openai");
        assert!(
            store.take("k").is_some(),
            "a run that already got an answer is not withdrawable"
        );
        store.record("k".to_string(), Dialed::endpoint("openai"), None);
        store.retract("k", "anthropic");
        assert!(
            store.take("k").is_some(),
            "another candidate's denial must not touch this anchor"
        );
    }

    #[test]
    fn overflow_evicts_only_the_stalest_entry() {
        // The old cap behaviour was `map.clear()`: it dropped EVERY in-flight
        // run's witness to make room for one new key. Now exactly one entry —
        // the least recently written — pays for the newcomer.
        let keys: Vec<String> = (0..MAX_TRACKED_SESSIONS).map(|i| format!("k{i}")).collect();
        let refs: Vec<&str> = keys.iter().map(String::as_str).collect();
        let mut store = store_with(&refs);

        store.record(
            "new".to_string(),
            Dialed::new("p", None),
            Some(Dialed::new("p", None)),
        );

        assert_eq!(store.len(), MAX_TRACKED_SESSIONS);
        assert!(store.take("k0").is_none(), "the oldest entry is evicted");
        assert!(store.take("k1").is_some(), "its neighbour survives");
        assert!(store.take("new").is_some(), "and the newcomer was admitted");
    }

    #[test]
    fn writing_refreshes_eviction_age_so_in_flight_runs_survive() {
        // A run in flight keeps writing (one record per turn); LRU aging makes
        // it the freshest entry in the map, so overflow evicts the stale idle
        // sessions first instead of the run about to be read.
        let keys: Vec<String> = (0..MAX_TRACKED_SESSIONS).map(|i| format!("k{i}")).collect();
        let refs: Vec<&str> = keys.iter().map(String::as_str).collect();
        let mut store = store_with(&refs);

        // k0 is the oldest — but it is still being written (in-flight run).
        store.record(
            "k0".to_string(),
            Dialed::new("p0", None),
            Some(Dialed::new("p0-next", None)),
        );
        store.record(
            "new".to_string(),
            Dialed::new("p", None),
            Some(Dialed::new("p", None)),
        );

        assert!(
            store.take("k0").is_some(),
            "an actively-written (in-flight) record must survive the overflow"
        );
        assert!(
            store.take("k1").is_none(),
            "the now-stalest idle entry pays instead"
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
