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
//! So the writers here sit at the two production sites that OWN the facts —
//! `runner_impl` records the layer sizes the prompt builder just measured, and
//! the tool schema sizes taken from the very `ToolService` the harness is about
//! to hand the model. Nothing in this module derives anything.
//!
//! # What a missing record means
//!
//! `latest` returning `None` says "no turn has been measured for this session
//! in this process" — a fresh session, or a daemon that restarted. It does NOT
//! mean "the prompt is empty" (判据 §8), so the handler answers
//! `RESOURCE_NOT_FOUND` rather than a zero-filled breakdown.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::capability::{CapabilitySlot, MissingSemantics, SlotStatus};
use crate::sync_primitives::Arc;
use crate::thinker::prompt_pipeline::LayerSize;

/// How many sessions keep a measured record. A long-lived daemon serves many
/// sessions and each record is a few hundred bytes; the oldest is evicted.
pub const MAX_TRACKED_SESSIONS: usize = 256;

/// One session's most recently measured prompt layout.
#[derive(Debug, Clone, Default)]
pub struct PromptSizeRecord {
    /// How many prompts this process has measured for the session. Monotonic
    /// per process, and reset by a restart — it labels a measurement, it is
    /// not the session's turn count.
    pub turn: u64,
    pub layers: Vec<LayerSize>,
    /// `(tool name, schema bytes, description bytes)`. Measured where the
    /// turn's tool list is resolved, NOT as a prompt layer: production
    /// assembles the prompt with an empty tools slice because schemas travel
    /// as native `tool_use` (see `prompt_build.rs`'s note at the
    /// `build_system_prompt_cached_with_mode_measured` call). Layer bytes and
    /// tool bytes therefore cannot double-count.
    pub tools: Vec<(String, u64, u64)>,
    pub recorded_at_ms: i64,
}

#[derive(Default)]
pub struct PromptSizeRegistry {
    inner: Mutex<HashMap<String, PromptSizeRecord>>,
}

impl PromptSizeRegistry {
    fn with<R>(&self, key: &str, f: impl FnOnce(&mut PromptSizeRecord) -> R) -> R {
        let mut map = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !map.contains_key(key) && map.len() >= MAX_TRACKED_SESSIONS {
            // Evict the stalest session; bounded memory for a long-lived daemon.
            if let Some(oldest) = map
                .iter()
                .min_by_key(|(_, r)| r.recorded_at_ms)
                .map(|(k, _)| k.clone())
            {
                map.remove(&oldest);
            }
        }
        let rec = map.entry(key.to_string()).or_default();
        rec.recorded_at_ms = chrono::Utc::now().timestamp_millis();
        f(rec)
    }

    /// Record the layer sizes of a prompt that was just built, and count it as
    /// one measured turn.
    pub fn record_layers(&self, session_key: &str, layers: Vec<LayerSize>) {
        self.with(session_key, |r| {
            r.turn += 1;
            r.layers = layers;
        });
    }

    /// Record the tool schema sizes for the same turn. Deliberately does NOT
    /// bump `turn`: the two writers describe one prompt from two sites, and a
    /// counter that both moved would report every turn twice.
    pub fn record_tools(&self, session_key: &str, tools: Vec<(String, u64, u64)>) {
        self.with(session_key, |r| r.tools = tools);
    }

    /// The latest record for `session_key`, or `None` when this process has
    /// measured no prompt for it.
    #[must_use]
    pub fn latest(&self, session_key: &str) -> Option<PromptSizeRecord> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(session_key)
            .cloned()
    }

    /// How many sessions are currently tracked. Used by the eviction test and
    /// by nothing in production — the bound is the point, not the number.
    #[cfg(test)]
    fn tracked(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
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

    #[test]
    fn latest_on_an_unknown_key_is_none_not_an_empty_record() {
        let reg = PromptSizeRegistry::default();
        assert!(
            reg.latest("never-measured").is_none(),
            "an unmeasured session must be distinguishable from a measured empty one"
        );
    }

    #[test]
    fn record_layers_bumps_the_turn_counter_and_record_tools_does_not() {
        let reg = PromptSizeRegistry::default();
        reg.record_layers("s1", vec![layer("soul", 40)]);
        assert_eq!(reg.latest("s1").unwrap().turn, 1);

        reg.record_tools("s1", vec![("grep".to_string(), 120, 30)]);
        let rec = reg.latest("s1").unwrap();
        assert_eq!(rec.turn, 1, "the tool writer describes the SAME turn");
        assert_eq!(rec.layers.len(), 1, "recording tools must not clear layers");
        assert_eq!(rec.tools.len(), 1);

        reg.record_layers("s1", vec![layer("soul", 41), layer("role", 9)]);
        let rec = reg.latest("s1").unwrap();
        assert_eq!(rec.turn, 2);
        assert_eq!(rec.layers.len(), 2, "layers are replaced, not appended");
        assert_eq!(
            rec.tools.len(),
            1,
            "a new turn's layers must not erase the tool sizes before they are re-recorded"
        );
    }

    #[test]
    fn the_registry_is_bounded_and_evicts_the_stalest_session() {
        let reg = PromptSizeRegistry::default();
        for i in 0..MAX_TRACKED_SESSIONS {
            reg.record_layers(&format!("s{i}"), vec![layer("soul", 10)]);
        }
        assert_eq!(reg.tracked(), MAX_TRACKED_SESSIONS);

        // Touch the oldest so it is no longer the stalest, then overflow.
        reg.record_layers("s0", vec![layer("soul", 11)]);
        reg.record_layers("overflow", vec![layer("soul", 12)]);

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
    }

    #[test]
    fn the_slot_is_declared_the_way_the_roster_expects() {
        let slot = global_prompt_size_registry_slot();
        assert_eq!(slot.id(), "thinker/prompt-size-registry");
        assert!(matches!(slot.missing(), MissingSemantics::ConsumerDecides));
    }
}
