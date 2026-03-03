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
//! | markdown_skills | Markdown skill runtime management |
//! | mcp | MCP integration |
//! | providers | AI provider management |
//! | profiles | Auth profile management |
//! | generation | Content generation |
//! | pairing | Device pairing |
//! | runs | Run wait/queue |
//! | auth | Authentication |
//! | agent | Agent execution |
//! | session | Session management |
//! | channel | Channel status |
//! | events | Event subscription |
//! | memory | Memory search |
//! | models | Model discovery |
//! | chat | Chat control |
//! | cron | Cron job management |
//! | exec_approvals | Exec approval management |
//! | wizard | Wizard session management |
//! | supervisor | Process supervision via PTY |
//! | poe | POE (Principle-Operation-Evaluation) task execution |
//! | identity | Identity/soul management |
//! | workspace | Workspace isolation management |
//! | guests | Guest invitation management |

pub mod health;
pub mod echo;
pub mod version;
pub mod agent;
pub mod agent_config;
pub mod general_config;
pub mod shortcuts_config;
pub mod behavior_config;
pub mod generation_config;
pub mod search_config;
pub mod session;
pub mod auth;
pub mod events;
pub mod channel;
pub mod config;
pub mod config_ext;
pub mod logs;
pub mod commands;
pub mod memory;
pub mod plugins;
pub mod services;
pub mod skills;
pub mod markdown_skills;
pub mod mcp;
pub mod mcp_config;
pub mod memory_config;
pub mod providers;
pub mod routing_rules;
pub mod security_config;
pub mod profiles;
pub mod generation;
pub mod embedding_providers;
pub mod generation_providers;
pub mod pairing;
pub mod runs;
pub mod models;
pub mod chat;
pub mod cron;
pub mod exec_approvals;
pub mod wizard;
pub mod supervisor;
pub mod approval_bridge;
pub mod poe;
pub mod identity;
pub mod debug;
pub mod guests;
pub mod workspace;
pub mod secret_approvals;
pub mod system_info;
pub mod discord_panel;

pub use approval_bridge::{parse_session_target, get_forward_targets, ForwardMode};
pub use identity::SharedIdentityResolver;
pub use guests::SharedInvitationManager;
pub use config::{handle_get_full_config, handle_patch_config};

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use crate::sync_primitives::Arc;

use super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS, METHOD_NOT_FOUND};
use crate::config::Config;

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

        // Config handlers (schema is stateless)
        registry.register("config.schema", config::handle_schema);

        // Logs handlers
        registry.register("logs.getLevel", logs::handle_get_level);
        registry.register("logs.setLevel", logs::handle_set_level);
        registry.register("logs.getDirectory", logs::handle_get_directory);

        // Commands handlers
        registry.register("commands.list", commands::handle_list);

        // Plugin handlers
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

        // Service handlers
        registry.register("services.start", services::handle_start);
        registry.register("services.stop", services::handle_stop);
        registry.register("services.list", services::handle_list);
        registry.register("services.status", services::handle_status);

        // Models handlers (use default config as placeholder)
        let models_config = Arc::new(Config::default());
        let cfg = models_config.clone();
        registry.register("models.list", move |req| {
            let config = cfg.clone();
            async move { models::handle_list(req, config).await }
        });
        let cfg = models_config.clone();
        registry.register("models.get", move |req| {
            let config = cfg.clone();
            async move { models::handle_get(req, config).await }
        });
        let cfg = models_config.clone();
        registry.register("models.capabilities", move |req| {
            let config = cfg.clone();
            async move { models::handle_capabilities(req, config).await }
        });

        // Cron handlers
        registry.register("cron.list", cron::handle_list);
        registry.register("cron.status", cron::handle_status);
        registry.register("cron.run", cron::handle_run);

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

        // POE handlers (placeholders - actual handlers wired with PoeRunManager)
        registry.register("poe.run", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "poe.run requires POE runtime - wire PoeRunManager first".to_string(),
            )
        });
        registry.register("poe.status", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "poe.status requires POE runtime - wire PoeRunManager first".to_string(),
            )
        });
        registry.register("poe.cancel", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "poe.cancel requires POE runtime - wire PoeRunManager first".to_string(),
            )
        });
        registry.register("poe.list", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "poe.list requires POE runtime - wire PoeRunManager first".to_string(),
            )
        });

        // POE Contract Signing handlers (placeholders - actual handlers wired with PoeContractService)
        registry.register("poe.prepare", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "poe.prepare requires POE runtime - wire PoeContractService first".to_string(),
            )
        });
        registry.register("poe.sign", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "poe.sign requires POE runtime - wire PoeContractService first".to_string(),
            )
        });
        registry.register("poe.reject", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "poe.reject requires POE runtime - wire PoeContractService first".to_string(),
            )
        });
        registry.register("poe.pending", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "poe.pending requires POE runtime - wire PoeContractService first".to_string(),
            )
        });

        // MCP Approval handlers
        registry.register("mcp.list_pending_approvals", mcp::handle_list_pending_approvals);
        registry.register("mcp.respond_approval", mcp::handle_respond_approval);
        registry.register("mcp.cancel_approval", mcp::handle_cancel_approval);

        // Markdown Skills handlers
        registry.register("markdown_skills.install", markdown_skills::handle_install);
        registry.register("markdown_skills.load", markdown_skills::handle_load);
        registry.register("markdown_skills.reload", markdown_skills::handle_reload);
        registry.register("markdown_skills.list", markdown_skills::handle_list);
        registry.register("markdown_skills.unload", markdown_skills::handle_unload);

        // Skills handlers (SKILL.md file-based skills)
        registry.register("skills.list", skills::handle_list);
        registry.register("skills.install", skills::handle_install);
        registry.register("skills.installFromZip", skills::handle_install_from_zip);
        registry.register("skills.delete", skills::handle_delete);

        // Identity handlers (placeholders - actual handlers wired with IdentityResolver)
        registry.register("identity.get", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "identity.get requires IdentityResolver - wire SharedIdentityResolver first".to_string(),
            )
        });
        registry.register("identity.set", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "identity.set requires IdentityResolver - wire SharedIdentityResolver first".to_string(),
            )
        });
        registry.register("identity.clear", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "identity.clear requires IdentityResolver - wire SharedIdentityResolver first".to_string(),
            )
        });
        registry.register("identity.list", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "identity.list requires IdentityResolver - wire SharedIdentityResolver first".to_string(),
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

        // Workspace handlers (placeholders - actual handlers wired with MemoryBackend)
        registry.register("workspace.create", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "workspace.create requires MemoryBackend - wire Gateway runtime first".to_string(),
            )
        });
        registry.register("workspace.list", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "workspace.list requires MemoryBackend - wire Gateway runtime first".to_string(),
            )
        });
        registry.register("workspace.get", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "workspace.get requires MemoryBackend - wire Gateway runtime first".to_string(),
            )
        });
        registry.register("workspace.update", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "workspace.update requires MemoryBackend - wire Gateway runtime first".to_string(),
            )
        });
        registry.register("workspace.archive", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "workspace.archive requires MemoryBackend - wire Gateway runtime first".to_string(),
            )
        });
        registry.register("workspace.switch", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "workspace.switch requires WorkspaceManager - wire Gateway runtime first".to_string(),
            )
        });
        registry.register("workspace.getActive", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "workspace.getActive requires WorkspaceManager - wire Gateway runtime first".to_string(),
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
    fn test_models_handlers_registered() {
        let registry = HandlerRegistry::new();
        assert!(registry.has_method("models.list"));
        assert!(registry.has_method("models.get"));
        assert!(registry.has_method("models.capabilities"));
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
    fn test_cron_handlers_registered() {
        let registry = HandlerRegistry::new();
        assert!(registry.has_method("cron.list"));
        assert!(registry.has_method("cron.status"));
        assert!(registry.has_method("cron.run"));
    }

    #[test]
    fn test_poe_handlers_registered() {
        let registry = HandlerRegistry::new();
        // Direct execution methods
        assert!(registry.has_method("poe.run"));
        assert!(registry.has_method("poe.status"));
        assert!(registry.has_method("poe.cancel"));
        assert!(registry.has_method("poe.list"));
        // Contract signing methods
        assert!(registry.has_method("poe.prepare"));
        assert!(registry.has_method("poe.sign"));
        assert!(registry.has_method("poe.reject"));
        assert!(registry.has_method("poe.pending"));
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
    fn test_markdown_skills_handlers_registered() {
        let registry = HandlerRegistry::new();
        assert!(registry.has_method("markdown_skills.load"));
        assert!(registry.has_method("markdown_skills.reload"));
        assert!(registry.has_method("markdown_skills.list"));
        assert!(registry.has_method("markdown_skills.unload"));
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
    fn test_workspace_handlers_registered() {
        let registry = HandlerRegistry::new();
        assert!(registry.has_method("workspace.create"));
        assert!(registry.has_method("workspace.list"));
        assert!(registry.has_method("workspace.get"));
        assert!(registry.has_method("workspace.update"));
        assert!(registry.has_method("workspace.archive"));
        assert!(registry.has_method("workspace.switch"));
        assert!(registry.has_method("workspace.getActive"));
    }
}
