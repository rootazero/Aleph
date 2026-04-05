//! Request Handlers
//!
//! Handlers for processing JSON-RPC 2.0 method calls.
//!
//! ## Handler Domains
//!
//! | Domain | Description |
//! |--------|-------------|
//! | health | Health checks, ping |
//! | echo | Echo/test |
//! | version | Version info |
//! | system_info | System metrics (CPU, memory, disk, uptime) |
//! | config | Configuration management |
//! | logs | Log level control |
//! | commands | Command listing |
//! | plugins | Plugin lifecycle |
//! | services | Background service lifecycle |
//! | skills | Skills management |
//! | mcp | MCP integration |
//! | providers | AI provider management |
//! | profiles | Auth profile management |
//! | generation | Content generation |
//! | group_chat | Multi-agent group chat orchestration |
//! | pairing | Device pairing |
//! | runs | Run wait/queue |
//! | auth | Authentication |
//! | agent | Agent execution |
//! | session | Session management |
//! | channel | Channel status |
//! | events | Event subscription |
//! | memory | Memory search |
//! | chat | Chat control |
//! | cron | Cron job management |
//! | exec_approvals | Exec approval management |
//! | wizard | Wizard session management |
//! | supervisor | Process supervision via PTY |
//! | identity | Identity/soul management |
//! | workspace | Workspace isolation management |
//! | daemon | Daemon status, shutdown, logs |
//! | arena | Multi-agent arena lifecycle |
//! | guests | Guest invitation management |
//! | teams | Team management (list, get, disband, delete) |

pub mod acp_config;
pub mod activity;
pub mod agent;
pub mod agent_config;
pub mod agents;
pub mod schema;
pub mod approval_bridge;
pub mod arena;
pub mod auth;
pub mod auth_tools;
pub mod behavior_config;
pub mod browser_config;
pub mod channel;
pub mod chat;
pub mod clawhub;
pub mod commands;
pub mod config;
pub mod config_ext;
pub mod cron;
pub mod daemon_control;
pub mod debug;
pub mod discord_panel;
pub mod echo;
pub mod embedding_providers;
pub mod events;
pub mod exec_approvals;
pub mod execution_config;
pub mod general_config;
pub mod generation;
pub mod generation_config;
pub mod generation_providers;
pub mod group_chat;
pub mod guests;
pub mod health;
pub mod heartbeat;
pub mod identity;
pub mod logs;
pub mod mcp;
pub mod mcp_config;
pub mod memory;
pub mod memory_config;
pub mod oauth;
pub mod pairing;
pub mod plugins;
pub mod profiles;
pub mod providers;
pub mod rerank_config;
pub mod request_state;
pub mod routing_rules;
pub mod runs;
pub mod search_config;
pub mod secret_approvals;
pub mod secret_migration;
pub mod security_config;
pub mod services;
pub mod session;
pub mod session_usage;
pub mod skills;
#[allow(dead_code)] // DTOs only — handlers deferred to Milestone 2
pub mod supervisor;
pub mod system_info;
pub mod teams;
pub mod trace_replay;
pub mod tools_visibility;
pub mod version;
pub mod wizard;
pub mod workspace;

pub use approval_bridge::{get_forward_targets, parse_session_target, ForwardMode};
pub use config::{handle_get_full_config, handle_patch_config};
pub use guests::SharedInvitationManager;
pub use identity::SharedIdentityResolver;

use crate::gateway::security::SharedTokenManager;
use crate::sync_primitives::Arc;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use super::protocol::{
    JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS, METHOD_NOT_FOUND,
};
use schema::HandlerSchema;

/// Resolve a secret from the vault by its full key. Returns `None` on missing or error.
pub(crate) fn resolve_vault_secret(key: &str, vault: &SharedTokenManager) -> Option<String> {
    match vault.get_secret(key) {
        Ok(Some(secret)) => Some(secret.expose().to_string()),
        Ok(None) => None,
        Err(e) => {
            tracing::warn!(key = %key, error = %e, "Failed to read secret from vault");
            None
        }
    }
}

/// Normalize an optional string: trim whitespace, return None if empty.
pub(crate) fn normalize_optional_string(value: Option<String>) -> Option<String> {
    value.and_then(|v| {
        let trimmed = v.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

/// Parse and deserialize JSON-RPC request params into a typed struct.
///
/// Returns `Err(JsonRpcResponse)` with `INVALID_PARAMS` on missing or
/// malformed params — callers can early-return this directly.
// JsonRpcResponse is 152+ bytes but boxing it would complicate all handler call sites
#[allow(clippy::result_large_err)]
pub(crate) fn parse_params<T: serde::de::DeserializeOwned>(
    request: &JsonRpcRequest,
) -> Result<T, JsonRpcResponse> {
    match &request.params {
        Some(p) => serde_json::from_value(p.clone()).map_err(|e| {
            JsonRpcResponse::error(
                request.id.clone(),
                INVALID_PARAMS,
                format!("Invalid params: {}", e),
            )
        }),
        None => Err(JsonRpcResponse::error(
            request.id.clone(),
            INVALID_PARAMS,
            "Missing params",
        )),
    }
}

/// Type alias for async handler functions
pub type HandlerFn = Arc<
    dyn Fn(JsonRpcRequest) -> Pin<Box<dyn Future<Output = JsonRpcResponse> + Send>> + Send + Sync,
>;

/// Registry for JSON-RPC method handlers
///
/// Maps method names to their handler functions. Handlers are invoked
/// asynchronously when a request with a matching method is received.
pub struct HandlerRegistry {
    handlers: HashMap<String, HandlerFn>,
}

impl HandlerRegistry {
    /// Create a new handler registry with built-in handlers
    pub fn new() -> Self {
        let mut registry = Self {
            handlers: HashMap::new(),
        };

        // Register built-in handlers
        registry.register("health", health::handle);
        registry.register("echo", echo::handle);
        registry.register("version", version::handle);
        registry.register("system.info", system_info::handle);
        registry.register("request.state", request_state::handle);

        // Config handlers (schema is stateless)
        registry.register("config.schema", config::handle_schema);

        // Schema introspection handlers
        registry.register("schema.list", schema::handle_schema_list);
        registry.register("schema.get", schema::handle_schema_get);
        registry.register("schema.protocol", schema::handle_schema_protocol);
        registry.register("schema.openapi", schema::handle_schema_openapi);

        // Logs handlers
        registry.register("logs.getLevel", logs::handle_get_level);
        registry.register("logs.setLevel", logs::handle_set_level);
        registry.register("logs.getDirectory", logs::handle_get_directory);

        // Commands handlers
        registry.register("commands.list", commands::handle_list);
        registry.register("command.execute", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "command.execute requires ToolRegistry — wire in Gateway startup".to_string(),
            )
        });

        // Plugin handlers (plural — legacy namespace, kept for backward compatibility)
        registry.register("plugins.list", plugins::handle_list);
        registry.register("plugins.install", plugins::handle_install);
        registry.register("plugins.installFromZip", plugins::handle_install_from_zip);
        registry.register("plugins.uninstall", plugins::handle_uninstall);
        registry.register("plugins.enable", plugins::handle_enable);
        registry.register("plugins.disable", plugins::handle_disable);
        registry.register("plugins.load", plugins::handle_load);
        registry.register("plugins.unload", plugins::handle_unload);
        registry.register("plugins.callTool", plugins::handle_call_tool);
        registry.register("plugins.executeCommand", plugins::handle_execute_command);

        // Plugin handlers (singular — canonical CC-compatible namespace)
        registry.register("plugin.list", plugins::handle_list);
        registry.register("plugin.install", plugins::handle_install);
        registry.register("plugin.installFromZip", plugins::handle_install_from_zip);
        registry.register("plugin.uninstall", plugins::handle_uninstall);
        registry.register("plugin.enable", plugins::handle_enable);
        registry.register("plugin.disable", plugins::handle_disable);
        registry.register("plugin.load", plugins::handle_load);
        registry.register("plugin.unload", plugins::handle_unload);
        registry.register("plugin.reload", plugins::handle_reload);
        registry.register("plugin.callTool", plugins::handle_call_tool);
        registry.register("plugin.executeCommand", plugins::handle_execute_command);

        // Plugin marketplace handlers
        registry.register("plugin.marketplace.list", plugins::handle_marketplace_list);
        registry.register("plugin.marketplace.add", plugins::handle_marketplace_add);
        registry.register(
            "plugin.marketplace.update",
            plugins::handle_marketplace_update,
        );
        registry.register(
            "plugin.marketplace.remove",
            plugins::handle_marketplace_remove,
        );
        registry.register(
            "plugin.marketplace.install",
            plugins::handle_marketplace_install,
        );

        // Service handlers
        registry.register("services.start", services::handle_start);
        registry.register("services.stop", services::handle_stop);
        registry.register("services.list", services::handle_list);
        registry.register("services.status", services::handle_status);

        // Cron handlers (stubs — real handlers wired with CronService in Gateway startup)
        registry.register("cron.list", cron::handle_list_stub);
        registry.register("cron.get", cron::handle_get_stub);
        registry.register("cron.create", cron::handle_create_stub);
        registry.register("cron.update", cron::handle_update_stub);
        registry.register("cron.delete", cron::handle_delete_stub);
        registry.register("cron.status", cron::handle_status_stub);
        registry.register("cron.run", cron::handle_run_stub);
        registry.register("cron.runs", cron::handle_runs_stub);
        registry.register("cron.toggle", cron::handle_toggle_stub);

        // Heartbeat handlers (stubs — real handlers wired with HeartbeatService in Gateway startup)
        registry.register("heartbeat.list", heartbeat::handle_list_stub);
        registry.register("heartbeat.get", heartbeat::handle_get_stub);
        registry.register("heartbeat.create", heartbeat::handle_create_stub);
        registry.register("heartbeat.update", heartbeat::handle_update_stub);
        registry.register("heartbeat.delete", heartbeat::handle_delete_stub);
        registry.register("heartbeat.toggle", heartbeat::handle_toggle_stub);
        registry.register("heartbeat.wake", heartbeat::handle_wake_stub);
        registry.register("heartbeat.runs", heartbeat::handle_runs_stub);

        // Chat handlers (placeholders - actual handlers wired in Gateway::new())
        registry.register("chat.send", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "chat.send requires Gateway runtime - use Gateway::new()".to_string(),
            )
        });
        registry.register("chat.abort", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "chat.abort requires Gateway runtime - use Gateway::new()".to_string(),
            )
        });
        registry.register("chat.history", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "chat.history requires Gateway runtime - use Gateway::new()".to_string(),
            )
        });
        registry.register("chat.clear", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "chat.clear requires Gateway runtime - use Gateway::new()".to_string(),
            )
        });

        // Session usage (requires SessionStore -- placeholder)
        registry.register("session.usage", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "session.usage requires SessionStore — wire in Gateway startup".to_string(),
            )
        });

        // Session compact (requires SessionStore — placeholder)
        registry.register("session.compact", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "session.compact requires SessionStore — wire in Gateway startup".to_string(),
            )
        });

        // MCP Approval handlers
        registry.register(
            "mcp.list_pending_approvals",
            mcp::handle_list_pending_approvals,
        );
        registry.register("mcp.respond_approval", mcp::handle_respond_approval);
        registry.register("mcp.cancel_approval", mcp::handle_cancel_approval);

        // Unified skill management (via SkillSystem)
        registry.register("skills.status", skills::handle_status);
        registry.register("skills.update", skills::handle_update);
        registry.register("skills.install_dep", skills::handle_install_dep);
        registry.register("skills.remove", skills::handle_remove);

        // ClawHub handlers
        registry.register("clawhub.search", clawhub::handle_search);
        registry.register("clawhub.browse", clawhub::handle_browse);
        registry.register("clawhub.detail", clawhub::handle_detail);
        registry.register("clawhub.install", clawhub::handle_install);

        // Identity handlers (placeholders - actual handlers wired with IdentityResolver)
        registry.register("identity.get", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "identity.get requires IdentityResolver - wire SharedIdentityResolver first"
                    .to_string(),
            )
        });
        registry.register("identity.set", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "identity.set requires IdentityResolver - wire SharedIdentityResolver first"
                    .to_string(),
            )
        });
        registry.register("identity.clear", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "identity.clear requires IdentityResolver - wire SharedIdentityResolver first"
                    .to_string(),
            )
        });
        registry.register("identity.list", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "identity.list requires IdentityResolver - wire SharedIdentityResolver first"
                    .to_string(),
            )
        });

        // Guest handlers (placeholders - actual handlers wired with InvitationManager)
        registry.register("guests.createInvitation", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "guests.createInvitation requires InvitationManager - wire SharedInvitationManager first".to_string(),
            )
        });
        registry.register("guests.listPending", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "guests.listPending requires InvitationManager - wire SharedInvitationManager first".to_string(),
            )
        });
        registry.register("guests.revokeInvitation", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "guests.revokeInvitation requires InvitationManager - wire SharedInvitationManager first".to_string(),
            )
        });

        // Workspace handlers (placeholders - actual handlers wired with AgentEnvStore)
        registry.register("workspace.create", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "workspace.create requires AgentEnvStore - wire Gateway runtime first".to_string(),
            )
        });
        registry.register("workspace.list", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "workspace.list requires AgentEnvStore - wire Gateway runtime first".to_string(),
            )
        });
        registry.register("workspace.get", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "workspace.get requires AgentEnvStore - wire Gateway runtime first".to_string(),
            )
        });
        registry.register("workspace.update", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "workspace.update requires AgentEnvStore - wire Gateway runtime first".to_string(),
            )
        });
        registry.register("workspace.archive", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "workspace.archive requires AgentEnvStore - wire Gateway runtime first".to_string(),
            )
        });
        registry.register("channels.set_agent", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "channels.set_agent requires AgentEnvStore - wire Gateway runtime first"
                    .to_string(),
            )
        });
        registry.register("agents.bindings", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "agents.bindings requires AgentEnvStore - wire Gateway runtime first".to_string(),
            )
        });

        // Group Chat handlers (placeholders - actual handlers wired with GroupChatOrchestrator)
        registry.register("group_chat.start", group_chat::handle_start_placeholder);
        registry.register(
            "group_chat.continue",
            group_chat::handle_continue_placeholder,
        );
        registry.register("group_chat.mention", group_chat::handle_mention_placeholder);
        registry.register("group_chat.end", group_chat::handle_end_placeholder);
        registry.register("group_chat.list", group_chat::handle_list_placeholder);
        registry.register("group_chat.history", group_chat::handle_history_placeholder);

        // Daemon control handlers
        // daemon.logs is stateless (reads from filesystem) — register directly
        registry.register("daemon.logs", daemon_control::handle_logs);
        registry.register("trace.list", trace_replay::handle_list_stub);
        registry.register("trace.get", trace_replay::handle_get_stub);
        // daemon.status and daemon.shutdown need runtime state — placeholders
        registry.register("daemon.status", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "daemon.status requires runtime state — wire in Gateway startup".to_string(),
            )
        });
        registry.register("daemon.shutdown", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "daemon.shutdown requires runtime state — wire in Gateway startup".to_string(),
            )
        });

        // Memory utility handlers (stateless — no shared state required)
        // NOTE: rerank_config.test is registered in register_config_handlers with vault access
        registry.register(
            "memory.retrieve_with_trace",
            memory_config::handle_retrieve_with_trace,
        );

        // Arena handlers (placeholders - actual handlers wired with ArenaManager)
        registry.register("arena.create", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "arena.create requires ArenaManager - wire SharedArenaManager first".to_string(),
            )
        });
        registry.register("arena.query", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "arena.query requires ArenaManager - wire SharedArenaManager first".to_string(),
            )
        });
        registry.register("arena.settle", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "arena.settle requires ArenaManager - wire SharedArenaManager first".to_string(),
            )
        });

        // Team management handlers (placeholders — actual handlers wired with TeamStore in Gateway startup)
        registry.register("teams.list", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "teams.list requires TeamStore — wire in Gateway startup".to_string(),
            )
        });
        registry.register("teams.get", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "teams.get requires TeamStore — wire in Gateway startup".to_string(),
            )
        });
        registry.register("teams.disband", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "teams.disband requires TeamStore — wire in Gateway startup".to_string(),
            )
        });
        registry.register("teams.delete", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "teams.delete requires TeamStore — wire in Gateway startup".to_string(),
            )
        });
        registry.register("agents.teams", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "agents.teams requires TeamStore — wire in Gateway startup".to_string(),
            )
        });

        // Tools visibility handlers (placeholders — actual handlers wired with ToolRegistry)
        registry.register("tools.catalog", tools_visibility::handle_catalog_stub);
        registry.register("tools.effective", tools_visibility::handle_effective_stub);

        // Agent management handlers (placeholders — actual handlers wired with AgentManager)
        registry.register("agents.list", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "agents.list requires AgentManager — wire in Gateway startup".to_string(),
            )
        });
        registry.register("agents.get", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "agents.get requires AgentManager — wire in Gateway startup".to_string(),
            )
        });
        registry.register("agents.create", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "agents.create requires AgentManager — wire in Gateway startup".to_string(),
            )
        });
        registry.register("agents.update", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "agents.update requires AgentManager — wire in Gateway startup".to_string(),
            )
        });
        registry.register("agents.delete", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "agents.delete requires AgentManager — wire in Gateway startup".to_string(),
            )
        });
        registry.register("agents.set_default", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "agents.set_default requires AgentManager — wire in Gateway startup".to_string(),
            )
        });
        registry.register("agents.files.list", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "agents.files.list requires AgentManager — wire in Gateway startup".to_string(),
            )
        });
        registry.register("agents.files.get", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "agents.files.get requires AgentManager — wire in Gateway startup".to_string(),
            )
        });
        registry.register("agents.files.set", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "agents.files.set requires AgentManager — wire in Gateway startup".to_string(),
            )
        });
        registry.register("agents.files.delete", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "agents.files.delete requires AgentManager — wire in Gateway startup".to_string(),
            )
        });
        registry.register("agents.tools_schema", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "agents.tools_schema requires initialization — wire in Gateway startup".to_string(),
            )
        });

        registry
    }

    /// Create an empty handler registry
    pub fn empty() -> Self {
        Self {
            handlers: HashMap::new(),
        }
    }

    /// Register a handler for a method
    ///
    /// # Arguments
    ///
    /// * `method` - The method name to handle
    /// * `handler` - An async function that takes a request and returns a response
    pub fn register<F, Fut>(&mut self, method: &str, handler: F)
    where
        F: Fn(JsonRpcRequest) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = JsonRpcResponse> + Send + 'static,
    {
        self.handlers.insert(
            method.to_string(),
            Arc::new(move |req| Box::pin(handler(req))),
        );
    }

    /// Unregister a handler
    ///
    /// # Arguments
    ///
    /// * `method` - The method name to unregister
    ///
    /// # Returns
    ///
    /// `true` if a handler was removed
    pub fn unregister(&mut self, method: &str) -> bool {
        self.handlers.remove(method).is_some()
    }

    /// Handle a request by dispatching to the appropriate handler
    ///
    /// # Arguments
    ///
    /// * `request` - The JSON-RPC request to handle
    ///
    /// # Returns
    ///
    /// The response from the handler, or a method not found error
    pub async fn handle(&self, request: &JsonRpcRequest) -> JsonRpcResponse {
        if let Some(handler) = self.handlers.get(&request.method) {
            handler(request.clone()).await
        } else {
            JsonRpcResponse::error(
                request.id.clone(),
                METHOD_NOT_FOUND,
                format!("Method not found: {}", request.method),
            )
        }
    }

    /// Check if a method is registered
    pub fn has_method(&self, method: &str) -> bool {
        self.handlers.contains_key(method)
    }

    /// Get a list of all registered method names
    pub fn methods(&self) -> Vec<String> {
        self.handlers.keys().cloned().collect()
    }

    /// Get the number of registered handlers
    pub fn len(&self) -> usize {
        self.handlers.len()
    }

    /// Check if the registry is empty
    pub fn is_empty(&self) -> bool {
        self.handlers.is_empty()
    }
}

impl Default for HandlerRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_builtin_handlers() {
        let registry = HandlerRegistry::new();

        assert!(registry.has_method("health"));
        assert!(registry.has_method("echo"));
        assert!(registry.has_method("version"));
    }

    #[tokio::test]
    async fn test_custom_handler() {
        let mut registry = HandlerRegistry::empty();

        registry.register("custom", |req| async move {
            JsonRpcResponse::success(req.id, json!({"custom": true}))
        });

        let request = JsonRpcRequest::with_id("custom", None, json!(1));
        let response = registry.handle(&request).await;

        assert!(response.is_success());
    }

    #[tokio::test]
    async fn test_method_not_found() {
        let registry = HandlerRegistry::empty();

        let request = JsonRpcRequest::with_id("nonexistent", None, json!(1));
        let response = registry.handle(&request).await;

        assert!(response.is_error());
        assert_eq!(response.error.unwrap().code, METHOD_NOT_FOUND);
    }

    #[tokio::test]
    async fn test_unregister() {
        let mut registry = HandlerRegistry::new();

        assert!(registry.has_method("health"));
        assert!(registry.unregister("health"));
        assert!(!registry.has_method("health"));
    }

    #[test]
    fn test_methods_list() {
        let registry = HandlerRegistry::new();
        let methods = registry.methods();

        assert!(methods.contains(&"health".to_string()));
        assert!(methods.contains(&"echo".to_string()));
        assert!(methods.contains(&"version".to_string()));
    }

    #[test]
    fn test_plugin_handlers_registered() {
        let registry = HandlerRegistry::new();
        assert!(registry.has_method("plugins.list"));
        assert!(registry.has_method("plugins.install"));
        assert!(registry.has_method("plugins.installFromZip"));
        assert!(registry.has_method("plugins.uninstall"));
        assert!(registry.has_method("plugins.enable"));
        assert!(registry.has_method("plugins.disable"));
        assert!(registry.has_method("plugins.load"));
        assert!(registry.has_method("plugins.unload"));
        assert!(registry.has_method("plugins.callTool"));
        assert!(registry.has_method("plugins.executeCommand"));
    }

    #[test]
    fn test_chat_handlers_registered() {
        let registry = HandlerRegistry::new();
        assert!(registry.has_method("chat.send"));
        assert!(registry.has_method("chat.abort"));
        assert!(registry.has_method("chat.history"));
        assert!(registry.has_method("chat.clear"));
    }

    #[test]
    fn test_heartbeat_handlers_registered() {
        let registry = HandlerRegistry::new();
        assert!(registry.has_method("heartbeat.list"));
        assert!(registry.has_method("heartbeat.get"));
        assert!(registry.has_method("heartbeat.create"));
        assert!(registry.has_method("heartbeat.update"));
        assert!(registry.has_method("heartbeat.delete"));
        assert!(registry.has_method("heartbeat.toggle"));
        assert!(registry.has_method("heartbeat.wake"));
        assert!(registry.has_method("heartbeat.runs"));
    }

    #[test]
    fn test_cron_handlers_registered() {
        let registry = HandlerRegistry::new();
        assert!(registry.has_method("cron.list"));
        assert!(registry.has_method("cron.get"));
        assert!(registry.has_method("cron.create"));
        assert!(registry.has_method("cron.update"));
        assert!(registry.has_method("cron.delete"));
        assert!(registry.has_method("cron.status"));
        assert!(registry.has_method("cron.run"));
        assert!(registry.has_method("cron.runs"));
        assert!(registry.has_method("cron.toggle"));
    }

    #[test]
    fn test_services_handlers_registered() {
        let registry = HandlerRegistry::new();
        assert!(registry.has_method("services.start"));
        assert!(registry.has_method("services.stop"));
        assert!(registry.has_method("services.list"));
        assert!(registry.has_method("services.status"));
    }

    #[test]
    fn test_mcp_approval_handlers_registered() {
        let registry = HandlerRegistry::new();
        assert!(registry.has_method("mcp.list_pending_approvals"));
        assert!(registry.has_method("mcp.respond_approval"));
        assert!(registry.has_method("mcp.cancel_approval"));
    }

    #[test]
    fn test_skill_system_handlers_registered() {
        let registry = HandlerRegistry::new();
        assert!(registry.has_method("skills.status"));
        assert!(registry.has_method("skills.update"));
        assert!(registry.has_method("skills.install_dep"));
        assert!(registry.has_method("skills.remove"));
    }

    #[test]
    fn test_identity_handlers_registered() {
        let registry = HandlerRegistry::new();
        assert!(registry.has_method("identity.get"));
        assert!(registry.has_method("identity.set"));
        assert!(registry.has_method("identity.clear"));
        assert!(registry.has_method("identity.list"));
    }

    #[test]
    fn test_arena_handlers_registered() {
        let registry = HandlerRegistry::new();
        assert!(registry.has_method("arena.create"));
        assert!(registry.has_method("arena.query"));
        assert!(registry.has_method("arena.settle"));
    }

    #[test]
    fn test_workspace_handlers_registered() {
        let registry = HandlerRegistry::new();
        assert!(registry.has_method("workspace.create"));
        assert!(registry.has_method("workspace.list"));
        assert!(registry.has_method("workspace.get"));
        assert!(registry.has_method("workspace.update"));
        assert!(registry.has_method("workspace.archive"));
        assert!(registry.has_method("channels.set_agent"));
        assert!(registry.has_method("agents.bindings"));
    }
}
