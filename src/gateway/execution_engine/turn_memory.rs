//! Per-turn memory-mode resolution — the fifth twin of
//! [`super::turn_permissions`], [`super::turn_thinking`], [`super::turn_mode`]
//! and [`super::turn_model`].
//!
//! Same shape, same session-metadata carrier, a fifth orthogonal knob: where
//! the tier resolves *how much the turn may do*, thinking *how hard it reasons*
//! and the mode *which tools it sees*, this resolves *what it is told it
//! already knows* — whether curated memory, the wiki orientation index and
//! per-query recall are injected into the prompt.
//!
//! Precedence (highest first):
//! 1. the value the request carried (`chat.send` / `agent.run` `memory` param,
//!    the TUI's `/memory`),
//! 2. the value previously stamped on the session — so a conversation muted
//!    yesterday is still muted when the terminal reopens, which is the entire
//!    point of the knob,
//! 3. the global `[memory] enabled`, read LIVE so a config change reaches the
//!    very next turn.
//!
//! R7: nothing here reads the user's message. Whether a conversation wants
//! memory is DECLARED by a human (or by the model's own R8 tool call); no code
//! infers it from what is being discussed.

use tracing::warn;

use super::engine::ExecutionEngine;
use super::RunRequest;
use crate::executor::ToolRegistry;
use crate::memory::session_memory_mode::{
    resolve_memory_mode, MemoryMode, MEMORY_MODE_SESSION_KEY,
};
use crate::thinker::ProviderRegistry as ThinkerProviderRegistry;

impl<P: ThinkerProviderRegistry + 'static, R: ToolRegistry + 'static> ExecutionEngine<P, R> {
    /// Resolve the memory mode for this turn, persisting a request-carried
    /// choice onto the session so it sticks across turns and restarts.
    pub(super) async fn resolve_turn_memory_mode(&self, request: &RunRequest) -> MemoryMode {
        let global_enabled = match self.app_config.as_ref() {
            Some(cfg) => cfg.read().await.memory.enabled,
            // No config handle (tests, embedded): the shipped default is on,
            // matching every release before this knob existed. Defaulting to
            // off here would silently strip memory from deployments that never
            // asked for the knob at all.
            None => true,
        };
        let requested = request
            .metadata
            .get(MEMORY_MODE_SESSION_KEY)
            .map(String::as_str)
            .and_then(MemoryMode::from_id);
        let stored = self.session_memory_mode(&request.session_key).await;

        if let Some(mode) = requested.filter(|m| stored != Some(*m)) {
            self.persist_session_memory_mode(&request.session_key, mode)
                .await;
        }
        resolve_memory_mode(global_enabled, requested, stored)
    }

    /// Read the mode previously stamped on the session. A malformed value is
    /// ignored (the turn falls back to the global default) rather than failing
    /// the run — the same posture as its four siblings.
    async fn session_memory_mode(
        &self,
        session_key: &crate::gateway::router::SessionKey,
    ) -> Option<MemoryMode> {
        let store = self.session_manager.as_ref()?;
        let meta = match store.get_metadata(session_key).await {
            Ok(meta) => meta?,
            Err(e) => {
                warn!(error = %e, "Failed to read session metadata — session memory mode skipped");
                return None;
            }
        };
        let raw = crate::gateway::session_snapshot::custom_str(&meta, MEMORY_MODE_SESSION_KEY)?;
        match MemoryMode::from_id(&raw) {
            Some(mode) => Some(mode),
            None => {
                warn!(
                    value = %raw,
                    "Unknown session memory_mode — turn falls back to the global default"
                );
                None
            }
        }
    }

    /// Stamp a request-carried mode onto the session. Best-effort: a store
    /// failure must not fail the run — the mode for THIS turn is already
    /// resolved and governs the prompt either way.
    async fn persist_session_memory_mode(
        &self,
        session_key: &crate::gateway::router::SessionKey,
        mode: MemoryMode,
    ) {
        use crate::gateway::session_store::types::SessionPatch;

        let Some(store) = self.session_manager.as_ref() else {
            return;
        };
        let patch = SessionPatch {
            metadata: Some(serde_json::json!({ MEMORY_MODE_SESSION_KEY: mode.id() })),
            ..Default::default()
        };
        if let Err(e) = store.patch_session(session_key, &patch).await {
            warn!(error = %e, mode = mode.id(), "Failed to persist session memory mode");
        }
    }
}
