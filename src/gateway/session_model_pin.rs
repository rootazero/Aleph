//! Durability for a session's `select_model` pin.
//!
//! Implements [`SessionPinSink`] against the session store, so a pick recorded
//! in `providers::session_model_handle`'s process-global map also lands in the
//! session's `identity_meta.custom` — the same bag the exec tier, usage mode,
//! thinking depth and memory mode already live in.
//!
//! The layering is why this file exists at all: `providers` must not depend on
//! the gateway, so the map cannot reach a session store. It publishes a trait
//! instead and this module fills it in, installed once at boot. That keeps the
//! *write seam* singular — every writer already funnels through
//! `set_session_model`, and none of them can opt out of durability by
//! construction.

use std::sync::Arc;

use tracing::warn;

use crate::gateway::session_store::types::SessionPatch;
use crate::gateway::session_store::SessionStore;
use crate::providers::session_model_handle::{
    SessionModelPref, SessionPinSink, MODEL_PIN_PROVIDER_SESSION_KEY, MODEL_PIN_SESSION_KEY,
};
use crate::routing::session_key::SessionKey;

/// Writes model pins onto the session row.
pub struct StoreBackedPinSink {
    store: Arc<dyn SessionStore>,
}

impl StoreBackedPinSink {
    #[must_use]
    pub const fn new(store: Arc<dyn SessionStore>) -> Self {
        Self { store }
    }

    /// Install this sink as the process-wide one. First call wins.
    pub fn install(store: Arc<dyn SessionStore>) {
        crate::providers::session_model_handle::install_pin_sink(Arc::new(Self::new(store)));
    }

    /// The patch that records (or clears) a pin.
    ///
    /// Clearing writes explicit `null`s rather than omitting the keys: both
    /// stores merge this bag key-by-key, so an omitted key means "leave it
    /// alone" — a clear that omitted them would leave the old pin on the row to
    /// be rehydrated by the next run, which is worse than never clearing.
    fn patch_for(pref: Option<&SessionModelPref>) -> SessionPatch {
        let (model, provider) = match pref {
            Some(p) => (
                serde_json::Value::String(p.model.clone()),
                p.provider.as_ref().map_or(serde_json::Value::Null, |s| {
                    serde_json::Value::String(s.clone())
                }),
            ),
            None => (serde_json::Value::Null, serde_json::Value::Null),
        };
        SessionPatch {
            metadata: Some(serde_json::json!({
                MODEL_PIN_SESSION_KEY: model,
                MODEL_PIN_PROVIDER_SESSION_KEY: provider,
            })),
            ..Default::default()
        }
    }
}

impl SessionPinSink for StoreBackedPinSink {
    fn persist(&self, session_key: &str, pref: Option<&SessionModelPref>) {
        // A key this store cannot address is not an error worth logging every
        // turn: sub-agent and ephemeral keys legitimately pin models on
        // sessions that have no row. The in-memory map still governs them for
        // as long as they exist, which is less than a process lifetime anyway.
        let Some(key) = SessionKey::from_key_string(session_key) else {
            return;
        };
        let patch = Self::patch_for(pref);
        let store = Arc::clone(&self.store);

        // The caller is a synchronous fn on a path where a store failure must
        // not fail the turn (same posture as `persist_session_think_level`).
        // Outside a runtime — unit tests, embedded uses — there is nothing to
        // spawn onto and the pin stays in memory, which is exactly what it did
        // before this sink existed.
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        handle.spawn(async move {
            if let Err(e) = store.patch_session(&key, &patch).await {
                warn!(error = %e, "Failed to persist session model pin");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(patch: &SessionPatch) -> serde_json::Map<String, serde_json::Value> {
        patch
            .metadata
            .clone()
            .and_then(|v| v.as_object().cloned())
            .expect("metadata object")
    }

    #[test]
    fn a_pick_writes_both_keys() {
        let patch = StoreBackedPinSink::patch_for(Some(&SessionModelPref {
            provider: Some("anthropic".to_string()),
            model: "claude-opus-5".to_string(),
        }));
        let m = keys(&patch);
        assert_eq!(m[MODEL_PIN_SESSION_KEY], "claude-opus-5");
        assert_eq!(m[MODEL_PIN_PROVIDER_SESSION_KEY], "anthropic");
    }

    #[test]
    fn a_pick_without_a_provider_nulls_the_provider_key() {
        // Not "leaves it alone": a previous pick may have pinned a provider,
        // and merging would keep it — stamping the new model onto the old
        // vendor.
        let patch = StoreBackedPinSink::patch_for(Some(&SessionModelPref {
            provider: None,
            model: "gpt-5".to_string(),
        }));
        let m = keys(&patch);
        assert_eq!(m[MODEL_PIN_SESSION_KEY], "gpt-5");
        assert!(m[MODEL_PIN_PROVIDER_SESSION_KEY].is_null());
    }

    #[test]
    fn a_clear_nulls_both_keys_rather_than_omitting_them() {
        let patch = StoreBackedPinSink::patch_for(None);
        let m = keys(&patch);
        assert!(m[MODEL_PIN_SESSION_KEY].is_null());
        assert!(m[MODEL_PIN_PROVIDER_SESSION_KEY].is_null());
        assert_eq!(
            m.len(),
            2,
            "an omitted key means \"leave it alone\" to both stores — a clear \
             that omitted them would be undone by the next run's rehydrate"
        );
    }
}
