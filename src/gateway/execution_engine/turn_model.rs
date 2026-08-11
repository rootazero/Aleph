//! Per-turn model-pin rehydration — the fourth twin of
//! [`super::turn_permissions`], [`super::turn_thinking`] and
//! [`super::turn_mode`].
//!
//! Unlike its three siblings this one does not *resolve* anything for the turn:
//! which model serves a run is decided further down, by the provider chain, and
//! three call sites already read the pin out of
//! `providers::session_model_handle`'s process-global map. What this does is
//! make that map tell the truth after a restart.
//!
//! The map is per-process. The pin is not — a user who switched this
//! conversation to a wider model expects the conversation to still be on it
//! tomorrow, and a process-only table does not answer "nothing was pinned"
//! after a restart, it answers a *different question* with the same shape. So a
//! pick is written through to the session row (`gateway::session_model_pin`),
//! and this module reads it back into the map at the start of any run whose
//! session has no live entry — **before** the three readers run, so none of
//! them changed.
//!
//! Rehydration never overwrites a live entry: inside one process the map is at
//! least as new as the row (every write goes to both, in that order), so
//! clobbering would resurrect a model the user just switched away from for the
//! width of one store round-trip. The guard lives in
//! `session_model_handle::hydrate_session_model` so both the rule and its
//! reason sit with the map they protect.

use tracing::warn;

use super::engine::ExecutionEngine;
use super::RunRequest;
use crate::executor::ToolRegistry;
use crate::providers::session_model_handle::{
    hydrate_session_model, SessionModelPref, MODEL_PIN_PROVIDER_SESSION_KEY, MODEL_PIN_SESSION_KEY,
};
use crate::thinker::ProviderRegistry as ThinkerProviderRegistry;

impl<P: ThinkerProviderRegistry + 'static, R: ToolRegistry + 'static> ExecutionEngine<P, R> {
    /// Reinstate this session's stored model pin when the process has none.
    ///
    /// Call before anything reads `get_session_model` for the run. Silent and
    /// cheap on the common path: a session with a live pin, or none at all,
    /// costs one metadata read that the sibling resolvers already make.
    pub(super) async fn rehydrate_turn_model_pin(&self, request: &RunRequest) {
        let key_str = request.session_key.to_key_string();
        if crate::providers::session_model_handle::get_session_model(&key_str).is_some() {
            return;
        }
        let Some(pref) = self.stored_model_pin(&request.session_key).await else {
            return;
        };
        if hydrate_session_model(&key_str, pref.clone()) {
            tracing::debug!(
                session = %key_str,
                model = %pref.model,
                "Restored session model pin from the session row"
            );
        }
    }

    /// Read the pin stamped on the session, if any.
    ///
    /// A pin with no model id is not a pin: the two keys are written together
    /// and a provider without a model cannot be applied to anything, so a bag
    /// holding only the provider reads as absent rather than as a half-pin the
    /// binder would have to guess at.
    async fn stored_model_pin(
        &self,
        session_key: &crate::gateway::router::SessionKey,
    ) -> Option<SessionModelPref> {
        let store = self.session_manager.as_ref()?;
        let meta = match store.get_metadata(session_key).await {
            Ok(meta) => meta?,
            Err(e) => {
                warn!(error = %e, "Failed to read session metadata — model pin not restored");
                return None;
            }
        };
        let model = crate::gateway::session_snapshot::custom_str(&meta, MODEL_PIN_SESSION_KEY)
            .filter(|m| !m.trim().is_empty())?;
        let provider =
            crate::gateway::session_snapshot::custom_str(&meta, MODEL_PIN_PROVIDER_SESSION_KEY)
                .filter(|p| !p.trim().is_empty());
        Some(SessionModelPref { provider, model })
    }
}
