//! Process-global per-session model preference.
//!
//! Model binding is resolved at each provider request boundary through
//! `SessionModelProvider`; the live session snapshot is not frozen at run
//! construction. A selection therefore affects the next request, while the
//! request already in flight keeps its immutable provider/model target.
//!
//! Mirrors [`route_handle`](super::route_handle): a single process-global,
//! lock-guarded map holds the live per-session selection; `SessionModelProvider`
//! reads that selection at each request boundary and `select_model` updates it.
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

use tracing::warn;

mod selectable;
pub use selectable::{ModelSelectionError, SelectableModelsSnapshot};

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
    /// Whether the live map changed.
    pub accepted: bool,
    /// Whether the update was durably accepted. Legacy synchronous callers may
    /// leave this false when no async store path is available.
    pub durable: bool,
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
#[async_trait::async_trait]
pub trait SessionPinSink: Send + Sync {
    /// Persist the pin for `session_key` with its ordering generation, or
    /// clear it when `pref` is `None`.
    ///
    /// This is required so every installed sink receives the generation-aware
    /// write. A sink cannot accidentally fall back to an unconditional legacy
    /// persistence path and allow an older spawned task to overwrite newer
    /// durable state.
    fn persist_with_generation(
        &self,
        session_key: &str,
        pref: Option<&SessionModelPref>,
        generation: u64,
    );

    /// Persist through the async session-store control seam and report whether
    /// the durable row accepted this generation. Selection control uses this
    /// method so a failed durable write cannot be reported as success.
    async fn persist_with_generation_durable(
        &self,
        session_key: &str,
        pref: Option<&SessionModelPref>,
        generation: u64,
    ) -> Result<(), String> {
        let _ = (session_key, pref, generation);
        Err("durable session pin sink is unsupported".to_string())
    }

    /// Compatibility entry point for callers that predate generations.
    /// Legacy writes use generation zero.
    fn persist(&self, session_key: &str, pref: Option<&SessionModelPref>) {
        self.persist_with_generation(session_key, pref, 0);
    }
}

/// `FailsClosed`: the selectable resolver refuses control selections until a
/// sink is installed. Legacy in-process setters remain available for
/// rehydration and compatibility, but they do not make a durable selection
/// claim when the sink is absent.
static PIN_SINK: CapabilitySlot<std::sync::Arc<dyn SessionPinSink>> =
    CapabilitySlot::new("providers/session-pin-sink", MissingSemantics::FailsClosed);

/// The handle above, type-erased for the roster — see
/// [`crate::spend::global_ledger_slot`] for why this shape.
pub(crate) const fn pin_sink_slot() -> &'static dyn SlotStatus {
    &PIN_SINK
}

/// Install the durability sink. First call wins (one boot, one sink).
///
/// Absent — compatibility setters and isolated tests may still update the
/// in-memory map, but the production selectable control path refuses to report
/// success until a session store is installed.
pub fn install_pin_sink(sink: std::sync::Arc<dyn SessionPinSink>) {
    let _ = PIN_SINK.install(sink);
}

fn sink() -> Option<&'static std::sync::Arc<dyn SessionPinSink>> {
    PIN_SINK.get()
}

/// Whether the production selection seam can preserve a pin across restart.
/// Tests may install a task-local selectable snapshot without booting the
/// gateway, but the production resolver must not report success while its
/// durable sink is absent.
pub(crate) fn pin_sink_installed() -> bool {
    sink().is_some()
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
/// generation sent to the compatibility sink. The control path uses
/// `update_session_model_durable` below to await durable acceptance. A
/// generation at `u64::MAX` is terminal...
pub(crate) fn update_session_model(
    session_key: &str,
    pref: Option<SessionModelPref>,
) -> SessionModelUpdate {
    let update = {
        let mut guard = map().write().unwrap_or_else(|e| e.into_inner());
        if let Some(existing) = guard.get(session_key) {
            if existing.generation == u64::MAX {
                warn!(
                    session_key,
                    "refusing session model update because generation reached u64::MAX"
                );
                return SessionModelUpdate {
                    pref: existing.pref.clone(),
                    generation: u64::MAX,
                    accepted: false,
                    durable: false,
                };
            }
        }
        let generation = guard.get(session_key).map_or(0, |state| state.generation) + 1;
        guard.insert(
            session_key.to_string(),
            SessionModelState {
                pref: pref.clone(),
                generation,
            },
        );
        SessionModelUpdate {
            pref,
            generation,
            accepted: true,
            durable: sink().is_some(),
        }
    };
    if update.accepted {
        if let Some(sink) = sink() {
            sink.persist_with_generation(session_key, update.pref.as_ref(), update.generation);
        }
    }
    update
}

/// Update the live selection only when its durable session row accepts the
/// same generation. This is the control path used by `select_model`; the
/// synchronous compatibility path above remains for legacy internal callers.
pub(crate) async fn update_session_model_durable(
    session_key: &str,
    pref: Option<SessionModelPref>,
) -> SessionModelUpdate {
    #[cfg(test)]
    if TEST_SELECTABLE_MODELS.try_with(|_| true).unwrap_or(false) {
        let mut update = update_session_model(session_key, pref);
        update.durable = true;
        return update;
    }

    let (previous, update) = {
        let mut guard = map().write().unwrap_or_else(|e| e.into_inner());
        let previous = guard.get(session_key).cloned();
        if previous
            .as_ref()
            .is_some_and(|state| state.generation == u64::MAX)
        {
            return SessionModelUpdate {
                pref: previous.and_then(|state| state.pref),
                generation: u64::MAX,
                accepted: false,
                durable: false,
            };
        }
        let generation = previous.as_ref().map_or(0, |state| state.generation) + 1;
        guard.insert(
            session_key.to_string(),
            SessionModelState {
                pref: pref.clone(),
                generation,
            },
        );
        (
            previous,
            SessionModelUpdate {
                pref,
                generation,
                accepted: true,
                durable: false,
            },
        )
    };

    let Some(sink) = sink().cloned() else {
        rollback_session_model(session_key, &previous, update.generation);
        return SessionModelUpdate {
            pref: previous.as_ref().and_then(|state| state.pref.clone()),
            generation: previous.as_ref().map_or(0, |state| state.generation),
            accepted: false,
            durable: false,
        };
    };

    match sink
        .persist_with_generation_durable(session_key, update.pref.as_ref(), update.generation)
        .await
    {
        Ok(()) => SessionModelUpdate {
            durable: true,
            ..update
        },
        Err(error) => {
            warn!(session_key, %error, "refusing session model update after durable persistence failure");
            rollback_session_model(session_key, &previous, update.generation);
            SessionModelUpdate {
                pref: previous.as_ref().and_then(|state| state.pref.clone()),
                generation: previous.map_or(0, |state| state.generation),
                accepted: false,
                durable: false,
            }
        }
    }
}

fn rollback_session_model(
    session_key: &str,
    previous: &Option<SessionModelState>,
    generation: u64,
) {
    let mut guard = map().write().unwrap_or_else(|e| e.into_inner());
    if guard
        .get(session_key)
        .is_some_and(|state| state.generation == generation)
    {
        match previous {
            Some(state) => {
                guard.insert(session_key.to_string(), state.clone());
            }
            None => {
                guard.remove(session_key);
            }
        }
    }
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
/// undone by a later request-boundary rehydrate — the pin would come back
/// from the row, which is worse than never having been clearable.
pub fn clear_session_model(session_key: &str) {
    update_session_model(session_key, None);
}

/// One live publication for provider keys, model roster, and readiness.
/// Missing boot publication refuses selection in every build, including tests.
static PINNABLE_PROVIDERS: CapabilitySlot<RwLock<std::sync::Arc<SelectableModelsSnapshot>>> =
    CapabilitySlot::new("providers/pinnable-set", MissingSemantics::FailsClosed);

/// The handle above, type-erased for the roster — see
/// [`crate::spend::global_ledger_slot`] for why this shape.
pub(crate) const fn pinnable_providers_slot() -> &'static dyn SlotStatus {
    &PINNABLE_PROVIDERS
}

/// Install the boot projection. Reloads swap this same slot's immutable value.
pub fn publish_selectable_models(snapshot: SelectableModelsSnapshot) {
    if let Some(handle) = PINNABLE_PROVIDERS.get() {
        *handle.write().unwrap_or_else(|e| e.into_inner()) = std::sync::Arc::new(snapshot);
    } else {
        let _ = PINNABLE_PROVIDERS.install(RwLock::new(std::sync::Arc::new(snapshot)));
    }
}

/// Refresh the roster/readiness after an existing live provider reload. The
/// routable keys remain those actually wired by the run builder at boot.
/// No boot publication means no permission to install one via a config write.
pub fn refresh_selectable_models(
    config: &crate::config::Config,
    available: &std::collections::BTreeSet<String>,
    authenticated: impl Fn(&str) -> bool,
) {
    let Some(handle) = PINNABLE_PROVIDERS.get() else {
        return;
    };
    let mut guard = handle.write().unwrap_or_else(|e| e.into_inner());
    *guard = std::sync::Arc::new(SelectableModelsSnapshot::from_config(
        config,
        guard.routable.clone(),
        available,
        authenticated,
    ));
}

/// Record a provider-less boot; selection still fails closed.
pub fn decline_pinnable_providers(because: &'static str) {
    PINNABLE_PROVIDERS.decline(because);
}

/// A caller retains one immutable generation while a reload publishes another.
pub fn selectable_models() -> Option<std::sync::Arc<SelectableModelsSnapshot>> {
    #[cfg(test)]
    if let Ok(snapshot) = TEST_SELECTABLE_MODELS.try_with(Clone::clone) {
        return snapshot;
    }
    PINNABLE_PROVIDERS
        .get()
        .map(|handle| handle.read().unwrap_or_else(|e| e.into_inner()).clone())
}

#[cfg(test)]
tokio::task_local! {
    static TEST_SELECTABLE_MODELS: Option<std::sync::Arc<SelectableModelsSnapshot>>;
}

/// Explicit task-scoped installation avoids global-test races, including
/// tests for missing publication. The production path has no bypass.
#[cfg(test)]
pub(crate) async fn with_test_selectable_models<T>(
    snapshot: Option<SelectableModelsSnapshot>,
    future: impl std::future::Future<Output = T>,
) -> T {
    TEST_SELECTABLE_MODELS
        .scope(snapshot.map(std::sync::Arc::new), future)
        .await
}

#[must_use]
pub fn pinnable_providers() -> Option<std::collections::BTreeSet<String>> {
    selectable_models().map(|snapshot| snapshot.routable.clone())
}

/// Resolve before any session or durable state is written. No publication is
/// an explicit refusal, not a test/preboot bypass.
pub fn resolve_selectable_model(
    provider: Option<&str>,
    model: &str,
) -> Result<SessionModelPref, ModelSelectionError> {
    #[cfg(test)]
    let test_publication = TEST_SELECTABLE_MODELS.try_with(|_| true).unwrap_or(false);
    #[cfg(not(test))]
    let test_publication = false;
    if !test_publication && !pin_sink_installed() {
        return Err(ModelSelectionError::Unavailable);
    }
    selectable_models()
        .ok_or(ModelSelectionError::Unavailable)?
        .resolve(provider, model)
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
    fn max_generation_refuses_update_without_changing_live_state() {
        let key = "test:session:generation-max";
        let old = SessionModelPref {
            provider: Some("openai".to_string()),
            model: "old-model".to_string(),
        };
        let new = SessionModelPref {
            provider: Some("anthropic".to_string()),
            model: "new-model".to_string(),
        };
        map().write().unwrap().insert(
            key.to_string(),
            SessionModelState {
                pref: Some(old.clone()),
                generation: u64::MAX,
            },
        );

        let update = update_session_model(key, Some(new));

        assert!(!update.accepted);
        assert_eq!(update.generation, u64::MAX);
        assert_eq!(update.pref, Some(old.clone()));
        assert_eq!(get_session_model_snapshot(key).pref, Some(old));
        map().write().unwrap().remove(key);
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
                MissingSemantics::FailsClosed
            ),
            "`providers/pinnable-set` is classified FailsClosed from its consumers; changing that \
             means re-reading them, not re-typing this line"
        );
    }
}
