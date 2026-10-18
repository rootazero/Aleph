//! Process-global per-session model preference.
//!
//! The "A layer" of AI dynamic routing (R8): the `select_model` tool lets the
//! main-loop LLM pick the model for the rest of a conversation. Model binding
//! happens per-run at `pick_llm` time — once, before the Think→Act loop starts
//! — so a mid-run pick takes effect at the next *run* (the next user message),
//! not at the next loop iteration. This handle is that cross-run channel.
//!
//! Mirrors [`route_handle`](super::route_handle): a single process-global,
//! lock-guarded map read at run construction (`harness_bridge`) and written by
//! the tool. Keyed by the canonical `SessionKey` string so the writer (tool,
//! from `TURN_CONTEXT`) and reader (bridge, from its run key) agree.
//!
//! # The map is a cache; the session row is the record
//!
//! This map used to be the whole story, on the argument that a per-conversation
//! model preference is soft UX state that may as well reset on restart. That
//! argument does not survive contact with what the map is asked: a process-only
//! table does not answer "nothing was pinned" after a restart, it answers a
//! *different question* than the one asked, with the same shape — the user who
//! switched this conversation to a wider model yesterday gets silently served
//! by the agent default today, and the only symptom is the bill.
//!
//! So a pick is now also written to the session's `identity_meta.custom` under
//! [`MODEL_PIN_SESSION_KEY`], through a boot-installed [`SessionPinSink`], and
//! read back into this map by `execution_engine::turn_model` at the start of a
//! run whose session has no live entry. Every existing reader still reads the
//! map and is unchanged: rehydration happens before they run.
//!
//! The sink is a process-global rather than a constructor argument on purpose.
//! `set_session_model` is the single write seam every writer already funnels
//! through; wiring durability to one of the tool's three construction sites
//! would have left the other two writing to memory only, each with its own
//! green unit test.

use crate::capability::{CapabilitySlot, MissingSemantics, SlotStatus};
use crate::sync_primitives::RwLock;
use std::collections::HashMap;
use std::sync::OnceLock;

/// A session's LLM-chosen model, optionally pinned to a provider.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionModelPref {
    /// Provider id to pin (e.g. `"openai"`). `None` = let the bridge fall back
    /// to the live default provider (the model is still stamped onto it).
    pub provider: Option<String>,
    /// Model id to stamp onto every request for this session.
    pub model: String,
}

/// A session model preference together with the generation that produced it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionModelSnapshot {
    pub pref: Option<SessionModelPref>,
    pub generation: u64,
}

/// The result of one atomic live-map update.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionModelUpdate {
    pub pref: Option<SessionModelPref>,
    pub generation: u64,
}

/// `identity_meta.custom` key under which a session's model pick is persisted.
///
/// Fourth twin of `EXEC_TIER_SESSION_KEY` / `MODE_SESSION_KEY` /
/// `THINK_LEVEL_SESSION_KEY`: same carrier, same "absent means follow the
/// agent's configured model" contract, a different axis.
pub const MODEL_PIN_SESSION_KEY: &str = "model_pin";

/// `identity_meta.custom` key for the provider a pick pinned alongside its
/// model, when it named one.
///
/// A second key rather than a JSON object under [`MODEL_PIN_SESSION_KEY`]:
/// every other knob in that bag is a flat string, and `sessions.patch` merges
/// the bag key-by-key — a nested object would be replaced wholesale by any
/// writer that only meant to change the model.
pub const MODEL_PIN_PROVIDER_SESSION_KEY: &str = "model_pin_provider";

/// `identity_meta.custom` key for the generation of a session model pick.
///
/// It is kept beside the model and provider keys in the same session metadata
/// row, so the durable selection and its ordering are one carrier.
pub const MODEL_PIN_GENERATION_SESSION_KEY: &str = "model_pin_generation";

#[derive(Clone, Debug)]
struct SessionModelState {
    pref: Option<SessionModelPref>,
    generation: u64,
}

static SESSION_MODELS: OnceLock<RwLock<HashMap<String, SessionModelState>>> = OnceLock::new();

fn map() -> &'static RwLock<HashMap<String, SessionModelState>> {
    SESSION_MODELS.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Makes a pick outlive the process that recorded it.
///
/// Implemented in the gateway (the layer that owns the session store) and
/// installed once at boot, because `providers` must not depend on it. `pref`
/// is `None` for "the pin was cleared".
pub trait SessionPinSink: Send + Sync {
    /// Persist the pin for `session_key`, or clear it when `pref` is `None`.
    ///
    /// Best-effort and non-blocking: this is called from synchronous code on a
    /// path where a store failure must not fail the turn. The in-memory map has
    /// already been updated, so the pin governs *this* process either way; what
    /// a failure costs is durability across a restart.
    fn persist(&self, session_key: &str, pref: Option<&SessionModelPref>);

    /// Persist a pin with its generation. Existing sinks that only understand
    /// the legacy API remain valid and receive generation zero.
    fn persist_with_generation(
        &self,
        session_key: &str,
        pref: Option<&SessionModelPref>,
        _generation: u64,
    ) {
        self.persist(session_key, pref);
    }
}

/// `FailsClosed`: both readers are `if let Some(sink)` with no `else`
/// ([`set_session_model`], [`clear_session_model`]), so an uninstalled sink
/// means a pick governs this process and nothing else. Nothing is granted and
/// nothing durable is falsely claimed — [`install_pin_sink`]'s own doc calls
/// that "the honest degradation", and it is right about the in-process half.
///
/// The part that is dead and silent is the restart: a user who pins a model
/// gets no signal that the pin will not survive one.
static PIN_SINK: CapabilitySlot<std::sync::Arc<dyn SessionPinSink>> =
    CapabilitySlot::new("providers/session-pin-sink", MissingSemantics::FailsClosed);

/// The handle above, type-erased for the roster — see
/// [`crate::spend::global_ledger_slot`] for why this shape.
pub(crate) const fn pin_sink_slot() -> &'static dyn SlotStatus {
    &PIN_SINK
}

/// Install the durability sink. First call wins (one boot, one sink).
///
/// Absent — tests, pre-boot, embedded uses with no session store — picks stay
/// in-memory and behave exactly as they did before, which is the honest
/// degradation: nothing claims to have been saved.
pub fn install_pin_sink(sink: std::sync::Arc<dyn SessionPinSink>) {
    let _ = PIN_SINK.install(sink);
}

fn sink() -> Option<&'static std::sync::Arc<dyn SessionPinSink>> {
    PIN_SINK.get()
}

/// Record the model preference for `session_key`, overwriting any prior pick.
///
/// Writes through to the session row when a sink is installed, so the pick
/// survives a restart. Both halves happen here because this is the one seam
/// every writer uses — a caller cannot pick "memory only" by accident.
pub fn set_session_model(session_key: &str, provider: Option<String>, model: String) {
    update_session_model(session_key, Some(SessionModelPref { provider, model }));
}

/// Atomically update the live selection and return the exact selection and
/// generation sent to the durable sink.
pub(crate) fn update_session_model(
    session_key: &str,
    pref: Option<SessionModelPref>,
) -> SessionModelUpdate {
    let update = {
        let mut guard = map().write().unwrap_or_else(|e| e.into_inner());
        let generation = guard
            .get(session_key)
            .map_or(0, |state| state.generation)
            .saturating_add(1);
        guard.insert(
            session_key.to_string(),
            SessionModelState {
                pref: pref.clone(),
                generation,
            },
        );
        SessionModelUpdate { pref, generation }
    };
    if let Some(sink) = sink() {
        sink.persist_with_generation(session_key, update.pref.as_ref(), update.generation);
    }
    update
}

/// Install a pin read back from durable storage, **without** disturbing a live
/// one.
///
/// Called by `execution_engine::turn_model` at run start. The guard matters:
/// after a restart the map is empty and the stored value is the only truth, but
/// within a live process the map is at least as new as the row (every write
/// goes to both, in that order), so a rehydrate that overwrote would resurrect
/// a pick the user just replaced — for exactly the window between the write and
/// the store's acknowledgement.
///
/// Returns `true` when the pin was installed.
pub fn hydrate_session_model(session_key: &str, pref: SessionModelPref) -> bool {
    hydrate_session_model_at_generation(session_key, pref, 0)
}

/// Install a durable selection only when it is newer than the live state.
/// Generation zero is the compatibility value for rows written before the
/// generation key existed.
pub fn hydrate_session_model_at_generation(
    session_key: &str,
    pref: SessionModelPref,
    generation: u64,
) -> bool {
    let mut guard = map().write().unwrap_or_else(|e| e.into_inner());
    if guard
        .get(session_key)
        .is_some_and(|state| state.generation >= generation)
    {
        return false;
    }
    guard.insert(
        session_key.to_string(),
        SessionModelState {
            pref: Some(pref),
            generation,
        },
    );
    true
}

/// Read the model preference for `session_key`, if one was set this run.
#[must_use]
pub fn get_session_model(session_key: &str) -> Option<SessionModelPref> {
    get_session_model_snapshot(session_key).pref
}

/// Read the live preference and its ordering generation as one snapshot.
#[must_use]
pub fn get_session_model_snapshot(session_key: &str) -> SessionModelSnapshot {
    map()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .get(session_key)
        .map_or(
            SessionModelSnapshot {
                pref: None,
                generation: 0,
            },
            |state| SessionModelSnapshot {
                pref: state.pref.clone(),
                generation: state.generation,
            },
        )
}

/// Drop a session's preference (revert to the agent/flow default).
///
/// Clears the durable record too. A clear that only emptied the map would be
/// undone by the next run's rehydrate — the pin would come back from the row,
/// which is worse than never having been clearable.
pub fn clear_session_model(session_key: &str) {
    update_session_model(session_key, None);
}

/// The provider keys a `select_model(provider=…)` pin can actually resolve to.
///
/// `FailsOpen`, and unlike its neighbour above this one really is a gate.
/// `builtin_tools::select_model::refuse_unpinnable_provider` opens with
/// `let known = pinnable_providers()?;` — a `?` on an `Option<..>` returning
/// `Option<String>`, so "no set published" produces NO REFUSAL. The
/// consequence is not hypothetical and is written out in the setter's doc
/// below: `select_model(provider="openai")` on an Anthropic-only deployment
/// returns `ok: true`, the run silently falls back to the default chain, and
/// the mis-attributed `(provider, model)` pair is written into the
/// routing-experience store the model later reads back as verified.
static PINNABLE_PROVIDERS: CapabilitySlot<std::collections::BTreeSet<String>> =
    CapabilitySlot::new("providers/pinnable-set", MissingSemantics::FailsOpen);

/// The handle above, type-erased for the roster — see
/// [`crate::spend::global_ledger_slot`] for why this shape.
pub(crate) const fn pinnable_providers_slot() -> &'static dyn SlotStatus {
    &PINNABLE_PROVIDERS
}

/// Publish the provider keys the run builder will resolve a pin against.
///
/// Registered by the production boot path from the *same map* `harness_bridge`
/// later looks the pin up in, so the tool's validation and the runtime's
/// resolution cannot disagree. Without it, `select_model(provider="openai")` on
/// an Anthropic-only deployment returned `ok: true`, then silently substituted
/// the default chain at run time — the user got an answer from a different
/// vendor than the one the tool confirmed, and the mis-attributed
/// `(provider, model)` pair was written into the routing-experience store the
/// model later reads back as "verified routing experience".
///
/// First call wins (one chain assembly per boot), matching
/// [`route_observe`](super::route_observe)'s global.
pub fn set_pinnable_providers(names: impl IntoIterator<Item = String>) {
    let _ = PINNABLE_PROVIDERS.install(names.into_iter().collect());
}

/// Record that boot reached this slot and had nothing to install.
///
/// This is one of the repo's three `FailsOpen` handles: with no published set,
/// `select_model(provider=…)` validates nothing and the paragraph above
/// describes what follows. Boot installs it from inside
/// `initialize_orchestrator`, which is gated on a default provider plus a
/// session service — so a provider-less deployment leaves the gate open and
/// says nothing. `because` is quoted verbatim to an operator.
pub fn decline_pinnable_providers(because: &'static str) {
    PINNABLE_PROVIDERS.decline(because);
}

/// Whether `provider` can be pinned, plus the valid set for the error message.
///
/// `None` means "no set was published" (tests, pre-boot) — callers must treat
/// that as *unvalidated*, never as "nothing is pinnable".
#[must_use]
pub fn pinnable_providers() -> Option<&'static std::collections::BTreeSet<String>> {
    PINNABLE_PROVIDERS.get()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_get_clear_roundtrip() {
        let key = "test:session:roundtrip";
        assert_eq!(get_session_model(key), None);
        set_session_model(key, Some("openai".to_string()), "gpt-5".to_string());
        assert_eq!(
            get_session_model(key),
            Some(SessionModelPref {
                provider: Some("openai".to_string()),
                model: "gpt-5".to_string(),
            })
        );
        // Overwrite with a provider-less pick.
        set_session_model(key, None, "claude-opus-4".to_string());
        let got = get_session_model(key).unwrap();
        assert_eq!(got.model, "claude-opus-4");
        assert_eq!(got.provider, None);
        clear_session_model(key);
        assert_eq!(get_session_model(key), None);
    }

    #[test]
    fn hydrate_fills_an_empty_slot() {
        // This process-global map may already contain a tombstone for a reused
        // literal key. Use a fresh key so this test exercises a genuinely cold
        // slot while `clear_session_model` can keep its stale-row guard.
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let key = format!(
            "test:session:hydrate-cold:{}",
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        assert!(hydrate_session_model(
            &key,
            SessionModelPref {
                provider: Some("anthropic".to_string()),
                model: "claude-opus-5".to_string(),
            }
        ));
        assert_eq!(
            get_session_model(&key).map(|p| p.model),
            Some("claude-opus-5".to_string())
        );
        clear_session_model(&key);
    }

    #[test]
    fn hydrate_never_overwrites_a_live_pick() {
        // Within a live process the map is at least as new as the row. A
        // rehydrate that clobbered it would resurrect the model the user just
        // switched away from, for the width of one store round-trip.
        let key = "test:session:hydrate-live";
        set_session_model(key, None, "picked-just-now".to_string());
        assert!(!hydrate_session_model(
            key,
            SessionModelPref {
                provider: None,
                model: "stale-from-disk".to_string(),
            }
        ));
        assert_eq!(
            get_session_model(key).map(|p| p.model),
            Some("picked-just-now".to_string())
        );
        clear_session_model(key);
    }

    #[test]
    fn distinct_sessions_isolated() {
        set_session_model("test:session:a", None, "model-a".to_string());
        set_session_model("test:session:b", None, "model-b".to_string());
        assert_eq!(
            get_session_model("test:session:a").unwrap().model,
            "model-a"
        );
        assert_eq!(
            get_session_model("test:session:b").unwrap().model,
            "model-b"
        );
        clear_session_model("test:session:a");
        clear_session_model("test:session:b");
    }

    #[test]
    fn generation_is_monotonic_across_update_clear_and_reselect() {
        let key = "test:session:generation-cycle";
        clear_session_model(key);

        let first = update_session_model(
            key,
            Some(SessionModelPref {
                provider: Some("openai".to_string()),
                model: "gpt-5".to_string(),
            }),
        );
        let cleared = update_session_model(key, None);
        let reselected = update_session_model(
            key,
            Some(SessionModelPref {
                provider: None,
                model: "claude-opus-5".to_string(),
            }),
        );

        assert!(first.generation < cleared.generation);
        assert!(cleared.generation < reselected.generation);
        assert_eq!(cleared.pref, None);
        assert_eq!(get_session_model_snapshot(key).pref, reselected.pref);
        clear_session_model(key);
    }

    #[test]
    fn cold_rehydration_restores_the_durable_generation() {
        let key = "test:session:generation-cold";
        clear_session_model(key);
        let pref = SessionModelPref {
            provider: Some("anthropic".to_string()),
            model: "claude-opus-5".to_string(),
        };

        assert!(hydrate_session_model_at_generation(key, pref.clone(), 41));
        assert_eq!(
            get_session_model_snapshot(key),
            SessionModelSnapshot {
                pref: Some(pref),
                generation: 41,
            }
        );
        clear_session_model(key);
    }

    #[test]
    fn live_rehydration_cannot_overwrite_a_newer_generation() {
        let key = "test:session:generation-live";
        let live = update_session_model(
            key,
            Some(SessionModelPref {
                provider: None,
                model: "picked-just-now".to_string(),
            }),
        );

        assert!(!hydrate_session_model_at_generation(
            key,
            SessionModelPref {
                provider: None,
                model: "stale-from-disk".to_string(),
            },
            live.generation,
        ));
        assert_eq!(
            get_session_model_snapshot(key).pref.unwrap().model,
            "picked-just-now"
        );
        clear_session_model(key);
    }

    #[test]
    fn canonical_session_keys_keep_generations_isolated() {
        let first_key = "agent:main:telegram:dm:user-a";
        let second_key = "agent:main:telegram:dm:user-b";
        clear_session_model(first_key);
        clear_session_model(second_key);

        let first = update_session_model(
            first_key,
            Some(SessionModelPref {
                provider: None,
                model: "model-a".to_string(),
            }),
        );
        let second = update_session_model(
            second_key,
            Some(SessionModelPref {
                provider: None,
                model: "model-b".to_string(),
            }),
        );

        assert_eq!(
            get_session_model_snapshot(first_key).pref.unwrap().model,
            "model-a"
        );
        assert_eq!(
            get_session_model_snapshot(second_key).pref.unwrap().model,
            "model-b"
        );
        assert_ne!(first.pref, second.pref);
        clear_session_model(first_key);
        clear_session_model(second_key);
    }

    /// The variant is the operator-facing severity of this handle going
    /// missing (`FailsOpen` => Error and a non-zero `aleph doctor`;
    /// `IndistinguishableDefault` / `ConsumerDecides` => Warning;
    /// `FailsClosed` => Info), and it is DERIVED from the consumers named on
    /// the static above. Pinned in the module that owns the handle, because
    /// that is the only place a reclassification and a re-read of those
    /// consumers can be made to happen together — the aggregate figure in
    /// FEATURE_LOCATOR cannot tell a reclassification from a new slot.
    /// `census::every_slot_pins_its_own_missing_semantics` requires this by
    /// slot id.
    #[test]
    fn the_pin_sink_slot_pins_its_missing_semantics() {
        assert_eq!(pin_sink_slot().id(), "providers/session-pin-sink");
        assert!(
            matches!(pin_sink_slot().missing(), MissingSemantics::FailsClosed),
            "`providers/session-pin-sink` is classified FailsClosed from its consumers; changing that \
             means re-reading them, not re-typing this line"
        );
    }

    /// See the sibling pin above.
    #[test]
    fn the_pinnable_set_slot_pins_its_missing_semantics() {
        assert_eq!(pinnable_providers_slot().id(), "providers/pinnable-set");
        assert!(
            matches!(
                pinnable_providers_slot().missing(),
                MissingSemantics::FailsOpen
            ),
            "`providers/pinnable-set` is classified FailsOpen from its consumers; changing that \
             means re-reading them, not re-typing this line"
        );
    }
}
