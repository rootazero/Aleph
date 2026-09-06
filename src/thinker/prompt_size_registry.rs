//! Latest measured prompt layout per session, for `context.breakdown`.
//!
//! # Why a registry and not a re-render
//!
//! "What is in this session's prompt?" has exactly one honest answer: the one
//! recorded at the moment the bytes were produced. Deriving it later means
//! re-running the pipeline against inputs that have since moved — a strategy
//! welded after the turn, a skill installed, a memory written — and reporting
//! the answer as if it described the prompt that was sent. That is the same
//! shape as the per-session prompt LRU deleted from
//! `orchestrator/harness_bridge/prompt_build.rs` (its post-mortem is the
//! comment above `build_system_prompt_cached_with_mode` there): a cached
//! prefix that silently served stale content whenever a tool mutated a stable
//! input mid-session.
//!
//! So the writer here sits at the production site that OWNS the facts:
//! `runner_impl` hands over the layout the prompt builder just measured
//! together with the tool schema sizes taken from the very `ToolService` the
//! harness is about to hand the model. Nothing in this module derives anything.
//!
//! # One turn, one write
//!
//! [`PromptSizeRegistry::record_turn`] is the ONLY writer, and it replaces the
//! whole record. That is deliberate and it is the second lesson from the same
//! post-mortem: two writers landing at different moments produced a record
//! carrying one turn's layers beside the next turn's tools, labelled with the
//! older turn — a record describing a prompt that never existed, which is the
//! deleted LRU's defect wearing different clothes. The layout arrives as an
//! `Option` because a turn can legitimately build no system prompt at all
//! (every layer source absent); that is a fact about the turn, not a reason to
//! keep the previous turn's.
//!
//! # What a missing record means
//!
//! `latest` returning `None` says "no turn has been measured for this session
//! in this process" — a fresh session, or a daemon that restarted. It does NOT
//! mean "the prompt is empty" (判据 §8), so the handler answers
//! `RESOURCE_NOT_FOUND` rather than a zero-filled breakdown.

use std::collections::HashMap;

use crate::capability::{CapabilitySlot, MissingSemantics, SlotStatus};
use crate::sync_primitives::{Arc, Mutex, PoisonError};
use crate::thinker::prompt_builder::PromptLayout;

/// How many sessions keep a measured record. A long-lived daemon serves many
/// sessions and each record is a few hundred bytes; the stalest is evicted.
pub const MAX_TRACKED_SESSIONS: usize = 256;

/// One session's most recently measured turn.
///
/// Every field describes the SAME turn — see the module doc's "One turn, one
/// write".
#[derive(Debug, Clone, Default)]
pub struct PromptSizeRecord {
    /// How many turns this process has measured for the session. Monotonic
    /// per process, and reset by a restart — it labels a measurement, it is
    /// not the session's turn count.
    pub turn: u64,
    /// `None` when this turn built no system prompt at all (`build_system_prompt`
    /// returned `None` because every layer source was absent, so the model got
    /// no system prompt). Distinct from a layout with an empty `layers`.
    pub layout: Option<PromptLayout>,
    /// `(tool name, schema bytes, description bytes)`. Measured where the
    /// turn's tool list is resolved, NOT as a prompt layer: production
    /// assembles the prompt with an empty tools slice because schemas travel
    /// as native `tool_use` (see `prompt_build.rs`'s note at the
    /// `build_system_prompt_cached_with_mode_measured` call). Layer bytes and
    /// tool bytes therefore cannot double-count.
    pub tools: Vec<(String, u64, u64)>,
    /// Write order within this process. Exists ONLY to give eviction a total
    /// order: a wall-clock stamp has millisecond resolution, and 256 inserts
    /// finish inside one millisecond, so `min_by_key` over a timestamp picks
    /// an arbitrary member of a large tie — deterministic for production
    /// (evicting any equally-stale record is fine) but a flake for any test
    /// that names the survivor. Not on the wire.
    write_seq: u64,
}

#[derive(Default)]
pub struct PromptSizeRegistry {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    records: HashMap<String, PromptSizeRecord>,
    next_seq: u64,
}

impl PromptSizeRegistry {
    /// Publish one turn's measurements for `session_key`, replacing whatever
    /// was there.
    ///
    /// `layout` is `None` when the turn built no system prompt. Replacement
    /// (not merge) is what makes a mixed-turn record unrepresentable.
    pub fn record_turn(
        &self,
        session_key: &str,
        layout: Option<PromptLayout>,
        tools: Vec<(String, u64, u64)>,
    ) {
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let seq = inner.next_seq;
        inner.next_seq += 1;
        if !inner.records.contains_key(session_key) && inner.records.len() >= MAX_TRACKED_SESSIONS {
            // Evict the stalest session; bounded memory for a long-lived daemon.
            if let Some(oldest) = inner
                .records
                .iter()
                .min_by_key(|(_, r)| r.write_seq)
                .map(|(k, _)| k.clone())
            {
                inner.records.remove(&oldest);
            }
        }
        let turn = inner.records.get(session_key).map_or(0, |r| r.turn) + 1;
        inner.records.insert(
            session_key.to_string(),
            PromptSizeRecord {
                turn,
                layout,
                tools,
                write_seq: seq,
            },
        );
    }

    /// The latest record for `session_key`, or `None` when this process has
    /// measured no turn for it.
    #[must_use]
    pub fn latest(&self, session_key: &str) -> Option<PromptSizeRecord> {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .records
            .get(session_key)
            .cloned()
    }

    /// How many sessions are currently tracked. Used by the eviction test and
    /// by nothing in production — the bound is the point, not the number.
    #[cfg(test)]
    fn tracked(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .records
            .len()
    }
}

/// Process-wide handle. `ConsumerDecides`: the single consumer
/// (`context.breakdown`) reads a missing registry as "nothing measured", which
/// is the same answer an installed-but-empty registry gives — an honest one in
/// both cases, and the RPC is a read-only introspection surface either way.
static GLOBAL: CapabilitySlot<Arc<PromptSizeRegistry>> = CapabilitySlot::new(
    "thinker/prompt-size-registry",
    MissingSemantics::ConsumerDecides,
);

/// Install the process-wide registry. Called once, unconditionally, at daemon
/// boot. Idempotent: a second call is ignored.
///
/// There is deliberately no `decline_*` twin. Boot's install here has no
/// fallible input to decline over — a `PromptSizeRegistry::default()` always
/// constructs — so the `Declined` arm would have no reachable caller, and a
/// slot's third state (`outcome() == None`, "boot never got here") already
/// says the true thing about a process that died before this line.
pub fn set_global_prompt_size_registry(registry: Arc<PromptSizeRegistry>) {
    let _ = GLOBAL.install(registry);
}

/// Fetch the process-wide registry, if boot installed one.
#[inline]
#[must_use]
pub fn global_prompt_size_registry() -> Option<Arc<PromptSizeRegistry>> {
    GLOBAL.get().cloned()
}

/// The handle above, type-erased for the roster — see
/// [`crate::spend::global_ledger_slot`] for why this shape.
pub(crate) const fn global_prompt_size_registry_slot() -> &'static dyn SlotStatus {
    &GLOBAL
}

/// The shared registry every test in this binary observes.
///
/// The slot is install-once, so a per-test instance would be silently
/// discarded for every test but the first — the loser would then measure a
/// registry nobody wrote to. Same idiom as
/// `session::store::install_test_event_store`; tests keep to their own session
/// keys.
#[cfg(test)]
pub(crate) fn install_test_prompt_size_registry() -> Arc<PromptSizeRegistry> {
    static TEST_REGISTRY: std::sync::OnceLock<Arc<PromptSizeRegistry>> = std::sync::OnceLock::new();
    let registry = TEST_REGISTRY
        .get_or_init(|| Arc::new(PromptSizeRegistry::default()))
        .clone();
    set_global_prompt_size_registry(registry.clone());
    registry
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_layer::LayerStability;
    use crate::thinker::prompt_pipeline::LayerSize;

    fn layer(name: &'static str, bytes: usize) -> LayerSize {
        LayerSize {
            priority: 100,
            name,
            stability: LayerStability::Stable,
            chars: bytes,
            bytes,
            tokens: bytes / 4,
        }
    }

    fn layout(layers: Vec<LayerSize>) -> PromptLayout {
        let dynamic_bytes_sent = layers.iter().map(|l| l.bytes as u64).sum();
        PromptLayout {
            layers,
            dynamic_bytes_sent,
        }
    }

    fn tool(name: &str) -> (String, u64, u64) {
        (name.to_string(), 120, 30)
    }

    #[test]
    fn latest_on_an_unknown_key_is_none_not_an_empty_record() {
        let reg = PromptSizeRegistry::default();
        assert!(
            reg.latest("never-measured").is_none(),
            "an unmeasured session must be distinguishable from a measured empty one"
        );
    }

    #[test]
    fn each_write_is_one_whole_turn_and_bumps_the_counter_once() {
        let reg = PromptSizeRegistry::default();
        reg.record_turn(
            "s1",
            Some(layout(vec![layer("soul", 40)])),
            vec![tool("grep")],
        );
        let rec = reg.latest("s1").unwrap();
        assert_eq!(rec.turn, 1);
        assert_eq!(rec.layout.as_ref().unwrap().layers.len(), 1);
        assert_eq!(rec.tools.len(), 1);

        reg.record_turn(
            "s1",
            Some(layout(vec![layer("soul", 41), layer("role", 9)])),
            vec![tool("grep"), tool("file_read")],
        );
        let rec = reg.latest("s1").unwrap();
        assert_eq!(rec.turn, 2, "one write, one turn");
        assert_eq!(
            rec.layout.as_ref().unwrap().layers.len(),
            2,
            "a turn REPLACES the previous layout, it does not merge with it"
        );
        assert_eq!(rec.tools.len(), 2);
    }

    /// The defect this shape exists to make unrepresentable: a record carrying
    /// one turn's layers beside a later turn's tools, labelled with the older
    /// turn. The old two-writer API could produce it whenever a turn built no
    /// system prompt; with one writer per turn there is no interleaving to have.
    #[test]
    fn a_turn_that_built_no_prompt_clears_the_previous_turns_layout() {
        let reg = PromptSizeRegistry::default();
        reg.record_turn(
            "s1",
            Some(layout(vec![layer("soul", 40)])),
            vec![tool("grep")],
        );
        reg.record_turn("s1", None, vec![tool("grep"), tool("file_read")]);

        let rec = reg.latest("s1").unwrap();
        assert_eq!(rec.turn, 2, "the no-prompt turn is still a measured turn");
        assert!(
            rec.layout.is_none(),
            "turn 1's layers must not survive beside turn 2's tools"
        );
        assert_eq!(rec.tools.len(), 2, "the tools are this turn's");
    }

    #[test]
    fn the_registry_is_bounded_and_evicts_the_stalest_session() {
        let reg = PromptSizeRegistry::default();
        for i in 0..MAX_TRACKED_SESSIONS {
            reg.record_turn(
                &format!("s{i}"),
                Some(layout(vec![layer("soul", 10)])),
                vec![],
            );
        }
        assert_eq!(reg.tracked(), MAX_TRACKED_SESSIONS);

        // Touch the first-inserted so it is no longer the stalest, then
        // overflow. Staleness is the monotonic `write_seq`, not a wall clock,
        // so this ordering is exact — a millisecond timestamp would tie for all
        // 258 writes on any reasonably fast host and evict an arbitrary member.
        reg.record_turn("s0", Some(layout(vec![layer("soul", 11)])), vec![]);
        reg.record_turn("overflow", Some(layout(vec![layer("soul", 12)])), vec![]);

        assert_eq!(
            reg.tracked(),
            MAX_TRACKED_SESSIONS,
            "the map must stay bounded"
        );
        assert!(
            reg.latest("overflow").is_some(),
            "the newest write must survive its own eviction pass"
        );
        assert!(
            reg.latest("s0").is_some(),
            "eviction must pick the STALEST, not the first-inserted"
        );
        assert!(
            reg.latest("s1").is_none(),
            "s1 was the stalest once s0 was touched, so it is the one evicted"
        );
    }

    #[test]
    fn the_slot_is_declared_the_way_the_roster_expects() {
        let slot = global_prompt_size_registry_slot();
        assert_eq!(slot.id(), "thinker/prompt-size-registry");
        assert!(matches!(slot.missing(), MissingSemantics::ConsumerDecides));
    }
}
