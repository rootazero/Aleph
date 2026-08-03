//! Agent Management Handlers
//!
//! RPC handlers for agent CRUD and workspace file operations:
//! - agents.list: List all agents with summaries
//! - agents.get: Get full agent definition
//! - agents.create: Create a new agent
//! - agents.update: Update an existing agent
//! - agents.delete: Delete an agent
//! - `agents.set_default`: Set the default agent
//! - agents.files.list: List workspace files
//! - agents.files.get: Read a workspace file
//! - agents.files.set: Write a workspace file
//! - agents.files.delete: Delete a workspace file

use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{debug, info};

use crate::config::agent_manager::{AgentManager, AgentPatch};
use crate::config::types::agents_def::{AgentDefinition, AgentIdentity, AgentModelRef};
use crate::sync_primitives::Arc;
use crate::thinker::soul_archetypes::SoulArchetype;

use super::super::agent_lifecycle::AgentLifecycleEvent;
use super::super::event_bus::{ConfigChangedEvent, GatewayEvent, GatewayEventBus};
use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR};
use super::parse_params;

// =============================================================================
// Runtime sync
// =============================================================================

/// Runtime-sync context for `agents.create` / `agents.delete`.
///
/// The agents RPC family persists to TOML via [`AgentManager`], but the
/// runtime [`AgentRegistry`](crate::gateway::agent_instance::AgentRegistry)
/// the inbound router resolves against is a separate list that boot builds
/// from that TOML. Without this context the two drift until restart: a
/// created agent is unusable (`agent_switch` / `channels.set_agent` reject
/// it, the router can't route to it) and a deleted agent keeps serving its
/// bound channels. Handlers that receive `Some(ctx)` keep the runtime
/// registry and channel bindings in lock-step with the TOML write.
pub struct AgentsRuntimeCtx {
    /// Runtime registry the inbound router resolves agents against.
    pub registry: Arc<crate::gateway::agent_instance::AgentRegistry>,
    /// Session store handed to lazily-registered instances.
    pub session_store: Arc<dyn crate::gateway::session_store::SessionStore>,
    /// Live app config — agent defaults + profiles + providers for resolution.
    pub config: Arc<tokio::sync::RwLock<crate::config::Config>>,
    /// Channel-binding store, for clearing bindings on delete.
    pub env_store: Arc<crate::gateway::agent_env::AgentEnvStore>,
}

// =============================================================================
// Response Types
// =============================================================================

/// Summary view of an agent for list responses
#[derive(Debug, Clone, Serialize)]
pub struct AgentSummary {
    pub id: String,
    pub name: Option<String>,
    pub emoji: Option<String>,
    pub description: Option<String>,
    pub model: Option<String>,
    pub is_default: bool,
}

impl From<&AgentDefinition> for AgentSummary {
    fn from(def: &AgentDefinition) -> Self {
        Self {
            id: def.id.clone(),
            name: def.name.clone(),
            emoji: def.identity.as_ref().and_then(|i| i.emoji.clone()),
            description: def.identity.as_ref().and_then(|i| i.description.clone()),
            model: def.model.as_ref().map(|m| match m {
                AgentModelRef::Legacy(s) => s.clone(),
                AgentModelRef::Qualified { provider, model } => format!("{provider}/{model}"),
            }),
            is_default: def.default,
        }
    }
}

// =============================================================================
// Request Parameters
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct AgentIdParams {
    pub id: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateAgentParams {
    pub id: String,
    pub name: Option<String>,
    pub identity: Option<AgentIdentity>,
    pub skills: Option<Vec<String>>,
    /// Soul archetype template (`expert | companion | assistant | maker`).
    /// `None` lets `AgentManager::create` fall back to `assistant`.
    #[serde(default)]
    pub archetype: Option<SoulArchetype>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateAgentParams {
    pub id: String,
    pub patch: AgentPatch,
}

#[derive(Debug, Deserialize)]
pub struct FileListParams {
    pub agent_id: String,
}

#[derive(Debug, Deserialize)]
pub struct FileParams {
    pub agent_id: String,
    pub filename: String,
}

#[derive(Debug, Deserialize)]
pub struct FileSetParams {
    pub agent_id: String,
    pub filename: String,
    pub content: String,
}

// =============================================================================
// Handlers
// =============================================================================

/// Handle agents.list — list all agents with summaries
pub async fn handle_list(request: JsonRpcRequest, manager: Arc<AgentManager>) -> JsonRpcResponse {
    debug!("Handling agents.list request");

    match manager.list() {
        Ok(agents) => {
            let default_id = agents
                .iter()
                .find(|a| a.default)
                .map(|a| a.id.clone())
                .unwrap_or_default();
            let summaries: Vec<AgentSummary> = agents.iter().map(AgentSummary::from).collect();
            JsonRpcResponse::success(
                request.id,
                json!({ "agents": summaries, "default_id": default_id }),
            )
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to list agents: {e}"),
        ),
    }
}

/// Handle agents.get — get full agent definition by ID
pub async fn handle_get(request: JsonRpcRequest, manager: Arc<AgentManager>) -> JsonRpcResponse {
    debug!("Handling agents.get request");

    let params: AgentIdParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    match manager.get(&params.id) {
        Ok(definition) => {
            let file_count = manager.list_files(&params.id).map_or(0, |f| f.len());
            JsonRpcResponse::success(
                request.id,
                json!({ "definition": definition, "file_count": file_count }),
            )
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to get agent '{}': {}", params.id, e),
        ),
    }
}

/// Handle agents.create — create a new agent definition
///
/// With `runtime: Some(..)`, the new definition is also resolved (same path
/// as boot) and registered in the runtime registry, so the agent is
/// immediately routable/switchable without a daemon restart.
pub async fn handle_create(
    request: JsonRpcRequest,
    manager: Arc<AgentManager>,
    event_bus: Arc<GatewayEventBus>,
    runtime: Option<Arc<AgentsRuntimeCtx>>,
) -> JsonRpcResponse {
    debug!("Handling agents.create request");

    let params: CreateAgentParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    let def = AgentDefinition {
        id: params.id.clone(),
        name: params.name,
        identity: params.identity,
        skills: params.skills,
        archetype: params.archetype,
        ..Default::default()
    };

    match manager.create(def.clone()) {
        Ok(()) => {
            info!("Agent '{}' created via RPC", params.id);

            // Hot-register in the runtime registry (TOML alone only takes
            // effect at next boot). Resolution mirrors boot's resolve_all.
            if let Some(rt) = runtime {
                let (defaults, profiles, providers) = {
                    let cfg = rt.config.read().await;
                    (
                        cfg.agents.defaults.clone(),
                        cfg.profiles.clone(),
                        cfg.providers.clone(),
                    )
                };
                let mut resolver = crate::config::agent_resolver::AgentDefinitionResolver::new();
                let resolved = resolver.resolve_one(&def, &defaults, &profiles, &providers);
                let config =
                    crate::gateway::agent_instance::AgentInstanceConfig::from_resolved(&resolved);
                let (workspace, model) = (config.workspace.clone(), config.model.clone());
                rt.registry
                    .register_config(config, Arc::clone(&rt.session_store))
                    .await;
                AgentLifecycleEvent::Registered {
                    agent_id: params.id.clone(),
                    workspace,
                    model,
                }
                .publish(&event_bus);
            }

            let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
                section: Some("agents".to_string()),
                value: json!({ "action": "created", "id": params.id }),
                timestamp: chrono::Utc::now().timestamp_millis(),
            });
            let _ = event_bus.publish_json(&event);

            JsonRpcResponse::success(request.id, json!({ "success": true, "id": params.id }))
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to create agent: {e}"),
        ),
    }
}

/// Handle agents.update — update an existing agent via patch
pub async fn handle_update(
    request: JsonRpcRequest,
    manager: Arc<AgentManager>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    debug!("Handling agents.update request");

    let params: UpdateAgentParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    match manager.update(&params.id, params.patch) {
        Ok(()) => {
            info!("Agent '{}' updated via RPC", params.id);

            let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
                section: Some("agents".to_string()),
                value: json!({ "action": "updated", "id": params.id }),
                timestamp: chrono::Utc::now().timestamp_millis(),
            });
            let _ = event_bus.publish_json(&event);

            JsonRpcResponse::success(request.id, json!({ "success": true }))
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to update agent '{}': {}", params.id, e),
        ),
    }
}

/// Handle agents.delete — delete an agent definition
///
/// With `runtime: Some(..)`, the agent is also evicted from the runtime
/// registry and every channel binding pointing at it is cleared — otherwise
/// the deleted agent keeps serving its bound channels until restart, and the
/// stale bindings then resolve to a ghost.
pub async fn handle_delete(
    request: JsonRpcRequest,
    manager: Arc<AgentManager>,
    event_bus: Arc<GatewayEventBus>,
    runtime: Option<Arc<AgentsRuntimeCtx>>,
) -> JsonRpcResponse {
    debug!("Handling agents.delete request");

    let params: AgentIdParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    match manager.delete(&params.id) {
        Ok(()) => {
            info!("Agent '{}' deleted via RPC", params.id);

            // Sync the runtime world: clear bindings first (inbound messages
            // fall back to default routing immediately), then evict the
            // instance. `manager.delete` already moved the dirs to trash.
            if let Some(rt) = runtime {
                if let Err(e) = rt.env_store.clear_bindings_for_agent(&params.id) {
                    tracing::warn!(agent_id = %params.id, error = %e,
                        "Failed to clear channel bindings for deleted agent");
                }
                let _ = rt.registry.remove(&params.id).await;
                AgentLifecycleEvent::Deleted {
                    agent_id: params.id.clone(),
                    workspace_archived: true,
                }
                .publish(&event_bus);
            }

            let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
                section: Some("agents".to_string()),
                value: json!({ "action": "deleted", "id": params.id }),
                timestamp: chrono::Utc::now().timestamp_millis(),
            });
            let _ = event_bus.publish_json(&event);

            JsonRpcResponse::success(request.id, json!({ "success": true }))
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to delete agent '{}': {}", params.id, e),
        ),
    }
}

/// Handle `agents.set_default` — set an agent as the default
pub async fn handle_set_default(
    request: JsonRpcRequest,
    manager: Arc<AgentManager>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    debug!("Handling agents.set_default request");

    let params: AgentIdParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    match manager.set_default(&params.id) {
        Ok(()) => {
            info!("Default agent set to '{}' via RPC", params.id);

            let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
                section: Some("agents".to_string()),
                value: json!({ "action": "default_changed", "id": params.id }),
                timestamp: chrono::Utc::now().timestamp_millis(),
            });
            let _ = event_bus.publish_json(&event);

            JsonRpcResponse::success(request.id, json!({ "success": true }))
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to set default agent '{}': {}", params.id, e),
        ),
    }
}

/// Handle agents.files.list — list files in an agent's workspace
pub async fn handle_files_list(
    request: JsonRpcRequest,
    manager: Arc<AgentManager>,
) -> JsonRpcResponse {
    debug!("Handling agents.files.list request");

    let params: FileListParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    match manager.list_files(&params.agent_id) {
        Ok(files) => JsonRpcResponse::success(request.id, json!({ "files": files })),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!(
                "Failed to list files for agent '{}': {}",
                params.agent_id, e
            ),
        ),
    }
}

/// Handle agents.files.get — read a file from an agent's workspace
pub async fn handle_files_get(
    request: JsonRpcRequest,
    manager: Arc<AgentManager>,
) -> JsonRpcResponse {
    debug!("Handling agents.files.get request");

    let params: FileParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    match manager.read_file(&params.agent_id, &params.filename) {
        Ok(content) => JsonRpcResponse::success(
            request.id,
            json!({ "content": content, "filename": params.filename }),
        ),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!(
                "Failed to read file '{}' for agent '{}': {}",
                params.filename, params.agent_id, e
            ),
        ),
    }
}

/// Handle agents.files.set — write a file to an agent's workspace
pub async fn handle_files_set(
    request: JsonRpcRequest,
    manager: Arc<AgentManager>,
) -> JsonRpcResponse {
    debug!("Handling agents.files.set request");

    let params: FileSetParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    match manager.write_file(&params.agent_id, &params.filename, &params.content) {
        Ok(()) => JsonRpcResponse::success(request.id, json!({ "success": true })),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!(
                "Failed to write file '{}' for agent '{}': {}",
                params.filename, params.agent_id, e
            ),
        ),
    }
}

/// Handle agents.files.delete — delete a file from an agent's workspace
pub async fn handle_files_delete(
    request: JsonRpcRequest,
    manager: Arc<AgentManager>,
) -> JsonRpcResponse {
    debug!("Handling agents.files.delete request");

    let params: FileParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    match manager.delete_file(&params.agent_id, &params.filename) {
        Ok(()) => JsonRpcResponse::success(request.id, json!({ "success": true })),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!(
                "Failed to delete file '{}' for agent '{}': {}",
                params.filename, params.agent_id, e
            ),
        ),
    }
}

/// Handle `agents.tools_schema` — return tool category metadata for Panel UI
pub async fn handle_tools_schema(request: JsonRpcRequest) -> JsonRpcResponse {
    use crate::executor::{BUILTIN_TOOL_DEFINITIONS, TOOL_CATEGORIES};

    let categories: Vec<serde_json::Value> = TOOL_CATEGORIES
        .iter()
        .map(|cat| {
            let tools: Vec<serde_json::Value> = cat
                .tools
                .iter()
                .map(|tool_name| {
                    let description = BUILTIN_TOOL_DEFINITIONS
                        .iter()
                        .find(|d| d.name == *tool_name)
                        .map_or("", |d| d.description);
                    json!({
                        "name": tool_name,
                        "description": description,
                    })
                })
                .collect();
            json!({
                "id": cat.id,
                "name": cat.name,
                "tools": tools,
            })
        })
        .collect();

    // Keep "groups" key in JSON response for webchat compatibility
    JsonRpcResponse::success(request.id, json!({ "groups": categories }))
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::agent_env::{AgentEnvStore, AgentEnvStoreConfig};
    use crate::gateway::agent_instance::{AgentInstanceConfig, AgentRegistry};
    use crate::gateway::session_manager::{SessionManager, SessionManagerConfig};
    use tempfile::TempDir;

    fn test_manager(dir: &TempDir) -> Arc<AgentManager> {
        let config_path = dir.path().join("config.toml");
        std::fs::write(
            &config_path,
            r#"
[agents]

[[agents.list]]
id = "main"
default = true
name = "Main Agent"

[[agents.list]]
id = "coder"
name = "Coder"
"#,
        )
        .unwrap();
        let workspace_root = dir.path().join("workspaces");
        let agents_root = dir.path().join("agents");
        let trash_root = dir.path().join("trash");
        std::fs::create_dir_all(&workspace_root).unwrap();
        std::fs::create_dir_all(&agents_root).unwrap();
        std::fs::create_dir_all(&trash_root).unwrap();
        Arc::new(AgentManager::new(
            config_path,
            workspace_root,
            agents_root,
            trash_root,
        ))
    }

    fn test_session_store(dir: &TempDir) -> Arc<dyn crate::gateway::session_store::SessionStore> {
        Arc::new(
            SessionManager::new(SessionManagerConfig {
                db_path: dir.path().join("sessions.db"),
                ..Default::default()
            })
            .unwrap(),
        )
    }

    fn test_env_store(dir: &TempDir) -> Arc<AgentEnvStore> {
        Arc::new(
            AgentEnvStore::new(AgentEnvStoreConfig {
                db_path: dir.path().join("env.db"),
                default_profile: "default".to_string(),
                archive_after_days: 0,
            })
            .unwrap(),
        )
    }

    /// Build a runtime-sync ctx whose config resolves workspaces/state under
    /// the temp dir — resolve_one must never touch the real ~/.aleph.
    fn test_runtime_ctx(dir: &TempDir, registry: Arc<AgentRegistry>) -> Arc<AgentsRuntimeCtx> {
        let mut cfg = crate::config::Config::default();
        cfg.agents.defaults.workspace_root = Some(dir.path().join("workspaces"));
        cfg.agents.defaults.agents_root = Some(dir.path().join("agents"));
        Arc::new(AgentsRuntimeCtx {
            registry,
            session_store: test_session_store(dir),
            config: Arc::new(tokio::sync::RwLock::new(cfg)),
            env_store: test_env_store(dir),
        })
    }

    async fn registry_with(dir: &TempDir, ids: &[&str]) -> Arc<AgentRegistry> {
        let registry = Arc::new(AgentRegistry::new());
        for id in ids {
            registry
                .register_config(
                    AgentInstanceConfig {
                        agent_id: (*id).to_string(),
                        workspace: dir.path().join("workspaces").join(id),
                        agent_dir: dir.path().join("agents").join(id),
                        ..Default::default()
                    },
                    test_session_store(dir),
                )
                .await;
        }
        registry
    }

    fn req(method: &str, params: serde_json::Value) -> JsonRpcRequest {
        JsonRpcRequest::with_id(method, Some(params), json!(1))
    }

    /// agents.create with a runtime ctx must hot-register the new agent so it
    /// is immediately routable/switchable — TOML alone only takes effect at
    /// the next boot (the old behavior left the agent unusable until restart).
    #[tokio::test]
    async fn create_hot_registers_runtime_agent() {
        let dir = TempDir::new().unwrap();
        let manager = test_manager(&dir);
        let registry = registry_with(&dir, &["main", "coder"]).await;
        let ctx = test_runtime_ctx(&dir, Arc::clone(&registry));
        let bus = Arc::new(GatewayEventBus::new());

        let resp = handle_create(
            req("agents.create", json!({"id": "quant"})),
            Arc::clone(&manager),
            bus,
            Some(ctx),
        )
        .await;
        assert!(resp.is_success(), "create failed: {:?}", resp.error);

        // TOML world updated…
        assert!(manager.get("quant").is_ok());
        // …and the runtime world too — no restart needed.
        assert!(
            registry.contains("quant").await,
            "created agent must be hot-registered in the runtime registry"
        );
    }

    /// agents.delete with a runtime ctx must evict the runtime instance and
    /// clear every channel binding — otherwise the deleted agent keeps
    /// serving its channels until restart, then strands them on a ghost.
    #[tokio::test]
    async fn delete_evicts_runtime_agent_and_clears_bindings() {
        let dir = TempDir::new().unwrap();
        let manager = test_manager(&dir);
        let registry = registry_with(&dir, &["main", "coder"]).await;
        let ctx = test_runtime_ctx(&dir, Arc::clone(&registry));
        ctx.env_store.set_active_agent("telegram", "coder").unwrap();
        ctx.env_store.set_active_agent("discord", "coder").unwrap();
        let bus = Arc::new(GatewayEventBus::new());

        let resp = handle_delete(
            req("agents.delete", json!({"id": "coder"})),
            Arc::clone(&manager),
            bus,
            Some(Arc::clone(&ctx)),
        )
        .await;
        assert!(resp.is_success(), "delete failed: {:?}", resp.error);

        // TOML world updated…
        assert!(manager.get("coder").is_err());
        // …runtime instance evicted…
        assert!(!registry.contains("coder").await);
        // …and no channel still points at the ghost.
        assert!(ctx
            .env_store
            .get_active_agent("telegram")
            .unwrap()
            .is_none());
        assert!(ctx.env_store.get_active_agent("discord").unwrap().is_none());
    }

    /// `agents.files.list` enumerates MEMORY.md, so the Panel file editor
    /// offers it for editing — but the file is the curated store's on-disk
    /// form. These handlers carry no filename guard of their own (R4: pure
    /// I/O); the refusal must arrive from `AgentManager`, which is where the
    /// single-source predicate lives.
    #[tokio::test]
    async fn files_set_and_delete_refuse_memory_md() {
        let dir = TempDir::new().unwrap();
        let manager = test_manager(&dir);

        let agent_dir = manager.agents_root.join("main");
        std::fs::create_dir_all(&agent_dir).unwrap();
        let memory = agent_dir.join("MEMORY.md");
        let curated_body = "fact one\n\u{a7}\n";
        std::fs::write(&memory, curated_body).unwrap();

        let resp = handle_files_set(
            req(
                "agents.files.set",
                json!({
                    "agent_id": "main",
                    "filename": "MEMORY.md",
                    "content": "# free-form notes",
                }),
            ),
            Arc::clone(&manager),
        )
        .await;
        let msg = resp
            .error
            .as_ref()
            .expect("must be refused")
            .message
            .clone();
        assert!(msg.contains("remember"), "msg was: {msg}");
        assert_eq!(
            std::fs::read_to_string(&memory).unwrap(),
            curated_body,
            "curated body must survive the refused write"
        );

        let resp = handle_files_delete(
            req(
                "agents.files.delete",
                json!({ "agent_id": "main", "filename": "MEMORY.md" }),
            ),
            Arc::clone(&manager),
        )
        .await;
        assert!(!resp.is_success(), "delete must be refused too");
        assert!(memory.exists());
    }
}
