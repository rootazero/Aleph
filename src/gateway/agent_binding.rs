//! Single-source channel ↔ agent binding operations.
//!
//! "Switching" an agent (FEATURE_LOCATOR §4.6) means re-binding the current
//! channel's active agent in [`AgentEnvStore`]. Two surfaces perform that
//! write — the `agent_switch` LLM tool and the Panel `channels.set_agent`
//! RPC — and they previously each hand-rolled validation, persistence, and
//! event emission, drifting apart (the RPC path skipped the no-op check,
//! the previous-agent capture, and the `Bound` lifecycle event entirely).
//!
//! This module is the one seam both surfaces call. It owns, in order:
//! 1. ghost validation (never bind a channel to an agent the runtime
//!    registry doesn't know — the inbound router would strand every message
//!    on that channel with `AgentNotFound`),
//! 2. no-op detection (already bound to the target → no write, no event),
//! 3. the atomic upsert / delete in [`AgentEnvStore`],
//! 4. the `Bound` / `Unbound` lifecycle event on the bus.

use tracing::info;

use crate::gateway::agent_env::AgentEnvStore;
use crate::gateway::agent_instance::AgentRegistry;
use crate::gateway::agent_lifecycle::AgentLifecycleEvent;
use crate::gateway::event_bus::GatewayEventBus;

/// Why a bind/unbind request was refused.
#[derive(Debug)]
pub enum BindError {
    /// No channel context (tool invoked outside a routed conversation).
    EmptyChannel,
    /// Target agent does not exist in the runtime registry.
    UnknownAgent {
        agent_id: String,
        /// Sorted list of registered agent IDs, for the error message.
        available: Vec<String>,
    },
    /// Underlying store failure.
    Store(String),
}

impl std::fmt::Display for BindError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyChannel => write!(
                f,
                "Cannot switch agent: no active channel context for this conversation."
            ),
            Self::UnknownAgent {
                agent_id,
                available,
            } => write!(
                f,
                "Agent '{agent_id}' not found. Available agents: {}",
                available.join(", ")
            ),
            Self::Store(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for BindError {}

/// Result of a successful bind request.
#[derive(Debug)]
pub struct BindOutcome {
    /// The agent previously bound to the channel, if any.
    pub previous_agent: Option<String>,
    /// `true` when the channel was already bound to the target — nothing was
    /// written and no event was emitted.
    pub no_op: bool,
}

/// Bind `agent_id` as the active agent for `channel`.
///
/// Validates the target against the runtime registry, persists via
/// [`AgentEnvStore::set_active_agent`], and emits
/// [`AgentLifecycleEvent::Bound`]. The inbound router reads the binding on
/// the next message, so the switch takes effect for subsequent turns.
///
/// `registry: None` skips ghost validation — that mode exists only for
/// minimal servers with no runtime registry wired (the Panel RPC's legacy
/// fallback); every surface that has a registry must pass it.
pub async fn bind_channel_agent(
    registry: Option<&AgentRegistry>,
    store: &AgentEnvStore,
    bus: Option<&GatewayEventBus>,
    channel: &str,
    agent_id: &str,
) -> Result<BindOutcome, BindError> {
    let channel = channel.trim();
    if channel.is_empty() {
        return Err(BindError::EmptyChannel);
    }

    // Ghost validation — existence check only, never instantiates a lazy
    // entry (validation should not pay the full AgentInstance build).
    if let Some(registry) = registry {
        if !registry.contains(agent_id).await {
            let mut available = registry.list().await;
            available.sort();
            return Err(BindError::UnknownAgent {
                agent_id: agent_id.to_string(),
                available,
            });
        }
    }

    // Previous binding, for reporting + the lifecycle event. A store read
    // failure here is deliberately not fatal (the bind below would surface
    // any real DB problem); it only degrades `previous_agent` to None.
    let previous_agent = store.get_active_agent(channel).unwrap_or_default();

    // No-op: already bound to the requested agent — no write, no event.
    if previous_agent.as_deref() == Some(agent_id) {
        return Ok(BindOutcome {
            previous_agent,
            no_op: true,
        });
    }

    store.set_active_agent(channel, agent_id).map_err(|e| {
        BindError::Store(format!(
            "Failed to bind agent '{agent_id}' to channel '{channel}': {e}"
        ))
    })?;

    if let Some(bus) = bus {
        AgentLifecycleEvent::Bound {
            agent_id: agent_id.to_string(),
            channel: channel.to_string(),
            previous_agent: previous_agent.clone(),
        }
        .publish(bus);
    }

    info!(
        agent_id = %agent_id,
        channel = %channel,
        previous = ?previous_agent,
        "Channel agent binding updated"
    );

    Ok(BindOutcome {
        previous_agent,
        no_op: false,
    })
}

/// Clear the explicit agent binding for `channel` (back to default routing).
///
/// Returns the agent that was bound, if any. Emits
/// [`AgentLifecycleEvent::Unbound`] only when a binding actually existed.
pub fn unbind_channel_agent(
    store: &AgentEnvStore,
    bus: Option<&GatewayEventBus>,
    channel: &str,
) -> Result<Option<String>, BindError> {
    let channel = channel.trim();
    if channel.is_empty() {
        return Err(BindError::EmptyChannel);
    }

    let previous_agent = store.get_active_agent(channel).unwrap_or_default();

    store
        .clear_active_agent(channel)
        .map_err(|e| BindError::Store(format!("Failed to unbind channel '{channel}': {e}")))?;

    if previous_agent.is_some() {
        if let Some(bus) = bus {
            AgentLifecycleEvent::Unbound {
                channel: channel.to_string(),
                previous_agent: previous_agent.clone(),
            }
            .publish(bus);
        }
        info!(channel = %channel, previous = ?previous_agent, "Channel agent binding cleared");
    }

    Ok(previous_agent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::agent_env::AgentEnvStoreConfig;
    use crate::gateway::agent_instance::{AgentInstance, AgentInstanceConfig};
    use crate::gateway::session_manager::{SessionManager, SessionManagerConfig};
    use crate::sync_primitives::Arc;
    use tempfile::tempdir;

    fn test_store() -> (tempfile::TempDir, Arc<AgentEnvStore>) {
        let temp = tempdir().unwrap();
        let config = AgentEnvStoreConfig {
            db_path: temp.path().join("test.db"),
            default_profile: "default".to_string(),
        };
        (temp, Arc::new(AgentEnvStore::new(config).unwrap()))
    }

    async fn registry_with(ids: &[&str]) -> (Vec<tempfile::TempDir>, AgentRegistry) {
        let registry = AgentRegistry::new();
        // Guards must OUTLIVE the registry they back — binding them here and
        // returning only the registry deletes each agent's tree while the
        // instance still points at it.
        let mut scratch = Vec::new();
        for id in ids {
            let root_guard = tempdir().unwrap();
            let root = root_guard.path().to_path_buf();
            scratch.push(root_guard);
            let session_store: Arc<dyn crate::gateway::session_store::SessionStore> = Arc::new(
                SessionManager::new(SessionManagerConfig {
                    db_path: root.join("sessions.db"),
                    ..Default::default()
                })
                .expect("session manager"),
            );
            let config = AgentInstanceConfig {
                agent_id: (*id).to_string(),
                workspace: root.join("workspace"),
                agent_dir: root.join("state"),
                ..Default::default()
            };
            registry
                .register(AgentInstance::new(config, session_store).expect("instance"))
                .await;
        }
        (scratch, registry)
    }

    #[tokio::test]
    async fn bind_rejects_unknown_agent_with_available_list() {
        let (_reg_scratch, registry) = registry_with(&["trader"]).await;
        let (_scratch, store) = test_store();
        let err = bind_channel_agent(Some(&registry), &store, None, "telegram", "ghost")
            .await
            .unwrap_err();
        match err {
            BindError::UnknownAgent { available, .. } => {
                assert_eq!(available, vec!["trader".to_string()]);
            }
            other => panic!("expected UnknownAgent, got {other:?}"),
        }
        assert!(store.get_active_agent("telegram").unwrap().is_none());
    }

    #[tokio::test]
    async fn bind_rejects_empty_channel() {
        let (_reg_scratch, registry) = registry_with(&["trader"]).await;
        let (_scratch, store) = test_store();
        let err = bind_channel_agent(Some(&registry), &store, None, "  ", "trader")
            .await
            .unwrap_err();
        assert!(matches!(err, BindError::EmptyChannel));
    }

    #[tokio::test]
    async fn bind_persists_and_reports_previous() {
        let (_reg_scratch, registry) = registry_with(&["trader", "coder"]).await;
        let (_scratch, store) = test_store();

        let first = bind_channel_agent(Some(&registry), &store, None, "telegram", "trader")
            .await
            .unwrap();
        assert!(first.previous_agent.is_none());
        assert!(!first.no_op);

        let second = bind_channel_agent(Some(&registry), &store, None, "telegram", "coder")
            .await
            .unwrap();
        assert_eq!(second.previous_agent.as_deref(), Some("trader"));
        assert_eq!(
            store.get_active_agent("telegram").unwrap().as_deref(),
            Some("coder")
        );
    }

    #[tokio::test]
    async fn bind_is_idempotent_no_op() {
        let (_reg_scratch, registry) = registry_with(&["trader"]).await;
        let (_scratch, store) = test_store();
        bind_channel_agent(Some(&registry), &store, None, "telegram", "trader")
            .await
            .unwrap();
        let again = bind_channel_agent(Some(&registry), &store, None, "telegram", "trader")
            .await
            .unwrap();
        assert!(again.no_op);
    }

    #[tokio::test]
    async fn unbind_returns_previous_and_clears() {
        let (_reg_scratch, registry) = registry_with(&["trader"]).await;
        let (_scratch, store) = test_store();
        bind_channel_agent(Some(&registry), &store, None, "telegram", "trader")
            .await
            .unwrap();

        let prev = unbind_channel_agent(&store, None, "telegram").unwrap();
        assert_eq!(prev.as_deref(), Some("trader"));
        assert!(store.get_active_agent("telegram").unwrap().is_none());

        // Unbinding an already-unbound channel is a quiet no-op.
        let prev = unbind_channel_agent(&store, None, "telegram").unwrap();
        assert!(prev.is_none());
    }
}
