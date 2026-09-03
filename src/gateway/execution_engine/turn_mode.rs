//! Per-turn session-mode resolution — the third twin of
//! [`super::turn_permissions`] and [`super::turn_thinking`].
//!
//! Same shape, same session-metadata mechanism, a third orthogonal knob:
//! where `turn_permissions` resolves *how much the turn may do* and
//! `turn_thinking` *how hard the model is asked to think*, this resolves
//! *which tool-presentation surface the turn gets* — the chat / work / code
//! usage mode.
//!
//! Precedence (highest first):
//! 1. the value the request carried (composer mode pill / `chat.send` `mode`
//!    param / the model's own `session_set_mode` call),
//! 2. the value previously stamped on the session,
//! 3. the global `[policies] mode` default (read LIVE from the shared config,
//!    so a config change reaches the very next turn) — `Work` when
//!    unconfigured, whose partition is byte-identical to pre-mode behavior.
//!
//! Unlike `turn_thinking` there is no "send nothing" case: every turn runs in
//! exactly one mode, and the default mode is the identity partition.
//!
//! R7: nothing here reads the user's message. The mode is DECLARED by a human
//! (pill / config) or by the model's own R8 tool call; no code classifies
//! what the message is about.

use tracing::warn;

use super::engine::ExecutionEngine;
use super::RunRequest;
use crate::config::types::policies::{SessionMode, MODE_SESSION_KEY};
use crate::executor::ToolRegistry;
use crate::thinker::ProviderRegistry as ThinkerProviderRegistry;

/// Pure precedence: requested > stored > global default. Split out (mirroring
/// `resolve_exec_tier`) so the contract is pinned by tests without a live
/// engine. No channel clamp: a mode grants nothing — safety stays with the
/// exec tier, whose clamp still applies independently.
pub(super) fn resolve_session_mode(
    global: SessionMode,
    requested: Option<SessionMode>,
    stored: Option<SessionMode>,
) -> SessionMode {
    requested.or(stored).unwrap_or(global)
}

impl<P: ThinkerProviderRegistry + 'static, R: ToolRegistry + 'static> ExecutionEngine<P, R> {
    /// Resolve the usage mode for this turn, persisting a request-carried
    /// choice onto the session so it sticks across turns and reloads.
    pub(super) async fn resolve_turn_mode(&self, request: &RunRequest) -> SessionMode {
        let global = match self.app_config.as_ref() {
            Some(cfg) => cfg.read().await.policies.mode,
            None => SessionMode::default(),
        };
        let requested = request
            .metadata
            .get(MODE_SESSION_KEY)
            .map(String::as_str)
            .and_then(SessionMode::from_id);
        let stored = self.session_mode(&request.session_key).await;

        if let Some(mode) = super::knob_to_stamp(requested, stored, request.is_resume()) {
            self.persist_session_mode(&request.session_key, mode).await;
        }
        resolve_session_mode(global, requested, stored)
    }

    /// Read the mode previously stamped on the session. A malformed or unknown
    /// value is ignored (the turn falls back to the global default) rather
    /// than failing the run.
    async fn session_mode(
        &self,
        session_key: &crate::gateway::router::SessionKey,
    ) -> Option<SessionMode> {
        let store = self.session_manager.as_ref()?;
        let meta = match store.get_metadata(session_key).await {
            Ok(meta) => meta?,
            Err(e) => {
                warn!(error = %e, "Failed to read session metadata — session mode skipped");
                return None;
            }
        };
        let raw = meta
            .identity_meta?
            .custom
            .get(MODE_SESSION_KEY)?
            .as_str()?
            .to_string();
        match SessionMode::from_id(&raw) {
            Some(mode) => Some(mode),
            None => {
                warn!(
                    value = %raw,
                    "Unknown session mode — turn falls back to the global default"
                );
                None
            }
        }
    }

    /// Stamp a request-carried mode onto the session. Best-effort: a store
    /// failure must not fail the run — the mode for THIS turn is already
    /// resolved and will partition the surface either way.
    async fn persist_session_mode(
        &self,
        session_key: &crate::gateway::router::SessionKey,
        mode: SessionMode,
    ) {
        use crate::gateway::session_store::types::SessionPatch;

        let Some(store) = self.session_manager.as_ref() else {
            return;
        };
        let patch = SessionPatch {
            metadata: Some(serde_json::json!({ MODE_SESSION_KEY: mode.id() })),
            ..Default::default()
        };
        if let Err(e) = store.patch_session(session_key, &patch).await {
            warn!(error = %e, mode = mode.id(), "Failed to persist session mode");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precedence_is_requested_then_stored_then_global() {
        let global = SessionMode::Work;
        // Nothing declared → global default.
        assert_eq!(resolve_session_mode(global, None, None), SessionMode::Work);
        // A stored session mode outlives the turn that carried it.
        assert_eq!(
            resolve_session_mode(global, None, Some(SessionMode::Code)),
            SessionMode::Code
        );
        // The request's explicit pick beats the stored value — that is the
        // composer pill switching an existing conversation.
        assert_eq!(
            resolve_session_mode(global, Some(SessionMode::Chat), Some(SessionMode::Code)),
            SessionMode::Chat
        );
    }

    #[test]
    fn global_default_rung_follows_config() {
        // A `[policies] mode = "chat"` install runs unmarked sessions in chat.
        assert_eq!(
            resolve_session_mode(SessionMode::Chat, None, None),
            SessionMode::Chat
        );
    }
}
