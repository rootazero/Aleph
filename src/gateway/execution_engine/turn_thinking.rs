//! Per-turn thinking-depth resolution — the twin of [`super::turn_permissions`].
//!
//! Same shape, same session-metadata mechanism, different knob: where
//! `turn_permissions` resolves *how much the turn is allowed to do*, this
//! resolves *how hard the model is asked to think* — and therefore what the
//! turn costs, since reasoning tokens bill at the output rate.
//!
//! Precedence (highest first):
//! 1. the value the request carried (composer pill / `chat.send` `thinking`
//!    param / the model's own `self_config` call),
//! 2. the value previously stamped on the session (so a choice outlives the
//!    turn that carried it — later turns send nothing, a page reload reads it
//!    back),
//! 3. `None` — send no thinking directive at all.
//!
//! Case 3 is deliberately NOT "some default level". Omitting the field leaves
//! each provider on its own default, which is the behaviour every Aleph release
//! to date has shipped; picking a level here instead would change how every
//! existing deployment's model reasons. The knob is opt-in.
//!
//! R7: nothing here reads the user's message. The depth is DECLARED by a human
//! or by the model; no code classifies the task's difficulty.

use tracing::warn;

use super::engine::ExecutionEngine;
use super::RunRequest;
use crate::agents::thinking::{normalize_think_level, ThinkLevel, THINK_LEVEL_SESSION_KEY};
use crate::executor::ToolRegistry;
use crate::thinker::ProviderRegistry as ThinkerProviderRegistry;

impl<P: ThinkerProviderRegistry + 'static, R: ToolRegistry + 'static> ExecutionEngine<P, R> {
    /// Resolve the thinking level for this turn, persisting a request-carried
    /// choice onto the session so it sticks.
    pub(super) async fn resolve_turn_think_level(
        &self,
        request: &RunRequest,
    ) -> Option<ThinkLevel> {
        let requested = request
            .metadata
            .get(THINK_LEVEL_SESSION_KEY)
            .map(String::as_str)
            .and_then(normalize_think_level);
        let stored = self.session_think_level(&request.session_key).await;

        if let Some(level) = super::knob_to_stamp(requested, stored, request.is_resume()) {
            self.persist_session_think_level(&request.session_key, level)
                .await;
        }
        requested.or(stored)
    }

    /// Read the level previously stamped on the session. A malformed or unknown
    /// value is ignored (the turn falls back to "no directive") rather than
    /// failing the run.
    async fn session_think_level(
        &self,
        session_key: &crate::gateway::router::SessionKey,
    ) -> Option<ThinkLevel> {
        let store = self.session_manager.as_ref()?;
        let meta = match store.get_metadata(session_key).await {
            Ok(meta) => meta?,
            Err(e) => {
                warn!(error = %e, "Failed to read session metadata — session think level skipped");
                return None;
            }
        };
        let raw = meta
            .identity_meta?
            .custom
            .get(THINK_LEVEL_SESSION_KEY)?
            .as_str()?
            .to_string();
        match normalize_think_level(&raw) {
            Some(level) => Some(level),
            None => {
                warn!(
                    value = %raw,
                    "Unknown session think_level — turn sends no thinking directive"
                );
                None
            }
        }
    }

    /// Stamp a request-carried level onto the session. Best-effort: a store
    /// failure must not fail the run — the level for THIS turn is already
    /// resolved and will be sent either way.
    async fn persist_session_think_level(
        &self,
        session_key: &crate::gateway::router::SessionKey,
        level: ThinkLevel,
    ) {
        use crate::gateway::session_store::types::SessionPatch;

        let Some(store) = self.session_manager.as_ref() else {
            return;
        };
        let patch = SessionPatch {
            metadata: Some(serde_json::json!({ THINK_LEVEL_SESSION_KEY: level.id() })),
            ..Default::default()
        };
        if let Err(e) = store.patch_session(session_key, &patch).await {
            warn!(error = %e, level = level.id(), "Failed to persist session think level");
        }
    }
}
