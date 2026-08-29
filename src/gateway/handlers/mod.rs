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
//! | agent | Agent execution |
//! | session | Session management |
//! | channel | Channel status |
//! | events | Event subscription |
//! | memory | Memory search |
//! | chat | Chat control |
//! | cron | Cron job management |
//! | exec_approvals | Exec approval management |
//! | exec_grants | Standing approval grants (list / revoke) |
//! | clarification | `ask_user` clarification pending/resolve (HITL P4) |
//! | pty | Embedded interactive terminal sessions (spawn/input/resize/close/list) |
//! | identity | Identity/soul management |
//! | workspace | Workspace isolation management |
//! | daemon | Daemon status, shutdown, logs |
//! | teams | Team management (list, get, disband, delete) |
//! | kanban | Structured, scope-filterable goal/loop listing for the project Kanban tab |

pub mod acp_config;
pub mod activity;
pub mod agent;
pub mod agents;
pub mod artifacts;
pub mod auth;
pub mod behavior_config;
pub mod browser_config;
pub mod bundled_sync;
pub mod canvas;
pub mod canvas_error;
pub mod channel;
pub mod chat;
pub mod clarification;
pub mod cluster;
pub mod commands;
pub mod config;
pub mod connect;
pub mod cron;
pub mod daemon_control;
pub mod debug;
pub mod diagnostics;
pub mod discord_panel;
pub mod echo;
pub mod embedding_providers;
pub mod events;
pub mod exec_approvals;
pub mod exec_grants;
pub mod execution_config;
pub mod fetch_config;
pub mod flow_admin;
pub mod fs;
pub mod gateway_credentials;
pub mod gateway_devices;
pub mod gateway_identity;
pub mod gateway_metrics;
pub mod gateway_ticket;
pub mod gateway_token;
pub mod general_config;
pub mod generation_config;
pub mod generation_providers;
pub mod graph;
pub mod graph_types;
pub mod group_chat;
pub mod health;
pub mod heartbeat;
pub mod hooks_admin;
pub mod identity;
pub mod insights;
pub mod kanban;
pub mod logs;
pub mod markdown_skills;
pub mod mcp;
pub mod mcp_config;
pub mod memory;
pub mod memory_config;
pub mod memory_curated;
pub mod memory_scope;
pub mod moa;
pub mod oauth;
pub mod pairing;
pub mod plugins;
pub mod projects;
pub mod providers;
pub mod pty;
pub mod request_state;
pub mod rerank_config;
pub mod resume;
pub mod route_config;
pub mod routing_rules;
pub mod runtimes;
pub mod search_config;
pub mod secret_migration;
pub mod secrets;
pub mod security_config;
pub mod services;
pub mod session;

pub mod dreaming;
pub mod extensions;
pub mod security_audit;
pub mod skills;
pub mod spend;
pub mod subagent;
pub mod system_info;
pub mod task_error;
pub mod teams;
pub mod tools_cancel;
pub mod tools_invoke;
pub mod tools_visibility;
pub mod trace_replay;
pub mod users;
pub mod version;
pub mod voice;
pub mod wizard;
pub mod workspace;

pub use config::{handle_get_full_config, handle_patch_config};
pub use identity::{IdentityHandlerContext, SharedIdentityCtx};

use crate::gateway::event_bus::GatewayEventBus;
use crate::gateway::security::store::SecurityStore;
use crate::gateway::security::SharedTokenManager;
use crate::sync_primitives::{Arc, AsyncRwLock};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use super::protocol::{
    JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS, METHOD_NOT_FOUND,
    SERVICE_UNAVAILABLE,
};

/// Build a phase-1 placeholder response indicating the named feature exists
/// but is not wired by the boot path in this build/mode. Used by
/// `HandlerRegistry::new()` to register a deterministic error in place of
/// fake-success stubs — phase-2 (boot) overrides with the real handler when
/// the dependency is available.
fn service_unavailable(req: JsonRpcRequest, reason: &'static str) -> JsonRpcResponse {
    JsonRpcResponse::error(req.id, SERVICE_UNAVAILABLE, reason.to_string())
}

/// Create an ad-hoc `CapabilityLedger` for stateless runtime handlers.
///
/// Each call loads (or creates) the ledger from `~/.aleph/runtimes/ledger.json`.
/// This avoids threading shared state through the registry while remaining correct —
/// the ledger is persisted on disk, so concurrent callers see the same data.
pub fn make_runtime_ledger(
) -> Result<Arc<AsyncRwLock<crate::runtimes::ledger::CapabilityLedger>>, String> {
    let dir = crate::runtimes::get_runtimes_dir().map_err(|e| format!("runtimes dir: {e}"))?;
    let ledger = crate::runtimes::ledger::CapabilityLedger::load_or_create(dir.join("ledger.json"));
    Ok(Arc::new(AsyncRwLock::new(ledger)))
}

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

/// Remove every occurrence of `name` from a fallback/failover chain,
/// returning `true` when the chain actually changed.
///
/// Provider and search-backend deletion call this so a removed provider does
/// not linger as a dangling chain reference. An orphan chain entry is skipped
/// at chain-build time, but it silently survives in the config file — which
/// surprised operators who deleted a provider from the panel and still saw it
/// in `[fallback_provider].chain`.
pub(crate) fn remove_from_chain(chain: &mut Vec<String>, name: &str) -> bool {
    let before = chain.len();
    chain.retain(|entry| entry != name);
    chain.len() != before
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
                format!("Invalid params: {e}"),
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

        // Logs handlers
        registry.register("logs.getLevel", logs::handle_get_level);
        registry.register("logs.setLevel", logs::handle_set_level);
        registry.register("logs.getDirectory", logs::handle_get_directory);

        // Commands handlers
        //
        // This base registration is a ToolCatalog-less fallback: it is
        // overridden at startup (see agent_init) with the registry-backed
        // `handle_list_from_registry`. Without the registry we cannot enumerate
        // commands, so return an empty list rather than phantom built-ins that
        // would not actually resolve.
        registry.register("commands.list", |req| async move {
            JsonRpcResponse::success(req.id, serde_json::json!({ "commands": [] }))
        });
        registry.register("command.execute", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "command.execute requires ToolRegistry — wire in Gateway startup".to_string(),
            )
        });

        // User-level hooks file admin (`~/.aleph/hooks.json`).
        // No context needed — the handlers reach the extension manager
        // via the process-global accessor.
        registry.register("hooks.list", hooks_admin::handle_hooks_list);
        registry.register("hooks.add", hooks_admin::handle_hooks_add);
        registry.register("hooks.remove", hooks_admin::handle_hooks_remove);
        registry.register("hooks.reload", hooks_admin::handle_hooks_reload);
        registry.register("hooks.events", hooks_admin::handle_hooks_events);
        registry.register("hooks.registry", hooks_admin::handle_hooks_registry);

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
        registry.register("plugin.install", plugins::handle_install_unified);
        registry.register("plugin.installFromZip", plugins::handle_install_from_zip);
        registry.register("plugin.uninstall", plugins::handle_uninstall);
        registry.register("plugin.update", plugins::handle_update);
        registry.register("plugin.enable", plugins::handle_enable);
        registry.register("plugin.disable", plugins::handle_disable);
        registry.register("plugin.reload", plugins::handle_reload);
        // Per-plugin configuration. The manifest could declare a
        // `config_schema` since the type existed; nothing could read or write
        // a value against it until these two.
        registry.register("plugin.config.get", plugins::handle_config_get);
        registry.register("plugin.config.set", plugins::handle_config_set);

        // Plugin marketplace handlers
        registry.register("plugin.marketplace.list", plugins::handle_marketplace_list);
        // `list` returns the registrations; `browse` returns what is inside
        // them. Without the second one, installing by name required already
        // knowing the name.
        registry.register(
            "plugin.marketplace.browse",
            plugins::handle_marketplace_browse,
        );
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

        // Embedded PTY terminal handlers — operator-only on both faces via the
        // "pty." prefix (see `gateway::handlers::pty` module doc for why both
        // faces matter). Stateless: they reach the process-global
        // `pty::manager()` accessor; the event bus is attached once in
        // `GatewayServer::build_router`, so no boot-time wiring is required here.
        //
        // `pty.spawn` is NOT registered here — it needs the live
        // `Arc<RwLock<Config>>` to read `[policies.terminal]` (the session
        // gate, and the scrollback/session-cap values), which this
        // stateless registry has no handle to. It is registered instead by
        // `register_pty_handlers` in
        // `src/bin/aleph-server/commands/start/builder/handlers/system.rs`,
        // via `register_handler!`, alongside the rest of the server's
        // config-carrying handlers.
        registry.register("pty.input", pty::handle_input);
        registry.register("pty.resize", pty::handle_resize);
        registry.register("pty.close", pty::handle_close);
        registry.register("pty.list", pty::handle_list);
        registry.register("pty.attach", pty::handle_attach);

        // Heartbeat handlers — real handlers wired with HeartbeatService in Gateway startup.

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

        // Session compact (requires SessionStore — placeholder). Manual
        // compaction itself still operates on the session *event log* (the
        // source the prompt is rebuilt from) through process-wide handles,
        // not the `messages` read projection — but the RPC now also needs
        // the store for the P1 visibility gate (the response discloses the
        // addressed session's real conversation summary), so this can no
        // longer be registered for real without one. See `builder/handlers/
        // session.rs::register_session_handlers` for the wired version.
        registry.register("session.compact", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "session.compact requires SessionStore — wire in Gateway startup".to_string(),
            )
        });

        // Session truncate (requires SessionStore — placeholder)
        registry.register("session.truncate", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "session.truncate requires SessionStore — wire in Gateway startup".to_string(),
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
        registry.register("skills.install", markdown_skills::handle_install);
        registry.register("bundled.sync", bundled_sync::handle_sync);

        // Identity handlers (identity.get/set/clear/list) are wired against the
        // live agent identity directory via `SharedIdentityCtx` by
        // `register_identity_handlers` at server boot. No placeholder is
        // registered here: an unwired registry correctly reports
        // METHOD_NOT_FOUND rather than a misleading stub error.

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
        registry.register("workspace.unarchive", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "workspace.unarchive requires AgentEnvStore - wire Gateway runtime first"
                    .to_string(),
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

        // The principal registry these defaults share. ONE instance for both
        // the `projects.*` and `users.*` blocks below: the roster verbs reject
        // a member id that names no active principal, so wiring them to a
        // different users table than `users.list` serves would make a test
        // harness contradict itself. Fresh in-memory (owner auto-bootstrapped
        // by `SecurityStore::in_memory()`); real wiring at boot uses the SAME
        // Arc as connect auth (see `commands/start/mod.rs`).
        let default_security_store = Arc::new(
            SecurityStore::in_memory().expect("in-memory SecurityStore for default registrations"),
        );

        // Project catalogue — backed by `~/.aleph/data/projects.db`. Every
        // consumer shares ONE connection (`ProjectStore::shared`); an ad-hoc
        // per-closure store would open its own connection and race for the
        // SQLite write lock. Real wiring happens in
        // `register_projects_handlers`; this default registration keeps
        // `projects.*` usable in test harnesses that boot via
        // `HandlerRegistry::new()` directly (under `cfg(test)` the shared
        // handle is in-memory, so this never touches the developer's home).
        //
        // `default_event_bus` is inert like `default_security_store` — this
        // registry has no live subscriber, so a `projects.changed` frame
        // published through it goes nowhere. Real wiring uses the SAME
        // `GatewayEventBus` the socket layer forwards from (see
        // `commands/start/mod.rs`).
        {
            let default_store = crate::projects::ProjectStore::shared();
            let default_event_bus = Arc::new(GatewayEventBus::new());
            let s = default_store.clone();
            registry.register("projects.list", move |req| {
                let s = s.clone();
                async move { projects::handle_list(req, s).await }
            });
            let s = default_store.clone();
            let b = default_event_bus.clone();
            registry.register("projects.add", move |req| {
                let s = s.clone();
                let b = b.clone();
                async move { projects::handle_add(req, s, b).await }
            });
            let s = default_store.clone();
            let b = default_event_bus.clone();
            registry.register("projects.create_blank", move |req| {
                let s = s.clone();
                let b = b.clone();
                async move { projects::handle_create_blank(req, s, b).await }
            });
            let s = default_store.clone();
            let u = default_security_store.clone();
            let b = default_event_bus.clone();
            registry.register("projects.remove", move |req| {
                let s = s.clone();
                let u = u.clone();
                let b = b.clone();
                async move { projects::handle_remove(req, s, u, b).await }
            });
            let s = default_store.clone();
            registry.register("projects.touch", move |req| {
                let s = s.clone();
                async move { projects::handle_touch(req, s).await }
            });
            let s = default_store.clone();
            registry.register("projects.get", move |req| {
                let s = s.clone();
                async move { projects::handle_get(req, s).await }
            });
            let s = default_store.clone();
            let b = default_event_bus.clone();
            registry.register("projects.create", move |req| {
                let s = s.clone();
                let b = b.clone();
                async move { projects::handle_create(req, s, b).await }
            });
            let s = default_store.clone();
            let u = default_security_store.clone();
            let b = default_event_bus.clone();
            registry.register("projects.rename", move |req| {
                let s = s.clone();
                let u = u.clone();
                let b = b.clone();
                async move { projects::handle_rename(req, s, u, b).await }
            });
            let s = default_store.clone();
            let u = default_security_store.clone();
            let b = default_event_bus.clone();
            registry.register("projects.archive", move |req| {
                let s = s.clone();
                let u = u.clone();
                let b = b.clone();
                async move { projects::handle_archive(req, s, u, b).await }
            });
            let s = default_store.clone();
            let u = default_security_store.clone();
            let b = default_event_bus.clone();
            registry.register("projects.bind_workspace", move |req| {
                let s = s.clone();
                let u = u.clone();
                let b = b.clone();
                async move { projects::handle_bind_workspace(req, s, u, b).await }
            });
            let s = default_store.clone();
            let u = default_security_store.clone();
            let b = default_event_bus.clone();
            registry.register("projects.member.add", move |req| {
                let s = s.clone();
                let u = u.clone();
                let b = b.clone();
                async move { projects::handle_member_add(req, s, u, b).await }
            });
            let s = default_store.clone();
            let u = default_security_store.clone();
            let b = default_event_bus;
            registry.register("projects.member.remove", move |req| {
                let s = s.clone();
                let u = u.clone();
                let b = b.clone();
                async move { projects::handle_member_remove(req, s, u, b).await }
            });
            let s = default_store.clone();
            registry.register("projects.member.list", move |req| {
                let s = s.clone();
                async move { projects::handle_member_list(req, s).await }
            });
            let s = default_store.clone();
            registry.register("projects.workspace.list", move |req| {
                let s = s.clone();
                async move { projects::handle_workspace_list(req, s).await }
            });
            let s = default_store.clone();
            registry.register("projects.workspace.read", move |req| {
                let s = s.clone();
                async move { projects::handle_workspace_read(req, s).await }
            });
            let s = default_store;
            registry.register("projects.room_session", move |req| {
                let s = s.clone();
                async move { projects::handle_room_session(req, s).await }
            });
        }

        // User (principal) catalogue — backed by `SecurityStore`'s `users`
        // table (Task 1 of the P0 identity foundation). Shares
        // `default_security_store` with the `projects.*` block above — this
        // keeps `users.*` usable in test harnesses that boot via
        // `HandlerRegistry::new()` directly. Real wiring happens at boot with
        // the SAME `SecurityStore` Arc used for connect auth, and the real
        // connection map / event bus for the deactivation device kick (see
        // `commands/start/mod.rs`).
        {
            let default_store = default_security_store;
            let default_kick = users::UserDeactivationKick {
                connections: Arc::new(AsyncRwLock::new(HashMap::new())),
                event_bus: Arc::new(GatewayEventBus::new()),
                // Inert like its two siblings above: this registry has no
                // channel axis, so there is nothing for deactivation to
                // withdraw. Boot re-registers `users.update` against the same
                // store the inbound router and `channel.pairing.*` share.
                pairing: Arc::new(
                    crate::gateway::pairing_store::SqlitePairingStore::in_memory()
                        .expect("in-memory PairingStore for default registrations"),
                ),
            };
            let s = default_store.clone();
            registry.register("users.me", move |req| {
                let s = s.clone();
                async move { users::handle_me(req, s).await }
            });
            let s = default_store.clone();
            registry.register("users.list", move |req| {
                let s = s.clone();
                async move { users::handle_list(req, s).await }
            });
            let s = default_store.clone();
            registry.register("users.create", move |req| {
                let s = s.clone();
                async move { users::handle_create(req, s).await }
            });
            let s = default_store;
            let kick = default_kick;
            registry.register("users.update", move |req| {
                let s = s.clone();
                let kick = kick.clone();
                async move { users::handle_update(req, s, kick).await }
            });
        }

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
        // Phase-1 placeholders — agent_init.rs overrides at boot.
        registry.register("trace.list", |req| async move {
            service_unavailable(req, "trace.list requires state database (boot phase 2)")
        });
        registry.register("trace.get", |req| async move {
            service_unavailable(req, "trace.get requires state database (boot phase 2)")
        });
        registry.register("gateway.identity.get", |req| async move {
            service_unavailable(
                req,
                "gateway.identity.get requires gateway state (boot phase 2)",
            )
        });
        registry.register("gateway.metrics.lanes", |req| async move {
            service_unavailable(
                req,
                "gateway.metrics.lanes requires LaneManager (boot phase 2)",
            )
        });
        registry.register("gateway.metrics.run_concurrency", |req| async move {
            service_unavailable(
                req,
                "gateway.metrics.run_concurrency requires AgentRunManager (boot phase 2)",
            )
        });
        registry.register("gateway.credentials", |req| async move {
            service_unavailable(
                req,
                "gateway.credentials requires gateway config (boot phase 2)",
            )
        });
        // Whiteboard canvas family — the real handlers need the boot-built
        // `CanvasStore` (rooted at `utils::paths::get_canvas_root()`, wired
        // with the event bus); `register_canvas_handlers` overrides at boot.
        // Deterministic placeholders keep the family honest in harnesses
        // that construct a registry without booting.
        for method in [
            "canvas.create",
            "canvas.list",
            "canvas.get",
            "canvas.apply",
            "canvas.delete",
            "canvas.asset.put",
            "canvas.asset.get",
            "canvas.selection.set",
        ] {
            registry.register(method, |req| async move {
                service_unavailable(req, "canvas.* requires CanvasStore (boot phase 2)")
            });
        }
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
        registry.register("teams.list_tasks", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "teams.list_tasks requires CoordTaskStore — wire in Gateway startup".to_string(),
            )
        });
        registry.register("teams.update_task", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "teams.update_task requires CoordTaskStore — wire in Gateway startup".to_string(),
            )
        });
        registry.register("teams.create_task", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "teams.create_task requires CoordTaskStore — wire in Gateway startup".to_string(),
            )
        });
        registry.register("teams.usage", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "teams.usage requires TeamStore + state.db — wire in Gateway startup".to_string(),
            )
        });

        // Runtime capability handlers (ad-hoc ledger per call — no shared state required)
        registry.register("runtimes.list", |req| async move {
            let ledger = match make_runtime_ledger() {
                Ok(l) => l,
                Err(e) => return JsonRpcResponse::error(req.id, INTERNAL_ERROR, e),
            };
            runtimes::handle_list(req, ledger).await
        });
        registry.register("runtimes.refresh", |req| async move {
            let ledger = match make_runtime_ledger() {
                Ok(l) => l,
                Err(e) => return JsonRpcResponse::error(req.id, INTERNAL_ERROR, e),
            };
            runtimes::handle_refresh(req, ledger).await
        });
        // runtimes.install needs event_bus — placeholder wired in Gateway startup
        registry.register("runtimes.install", |req| async move {
            JsonRpcResponse::error(
                req.id,
                INTERNAL_ERROR,
                "runtimes.install requires event_bus — wire in Gateway startup".to_string(),
            )
        });

        // Graph visualization handlers (backed by NoteStore — Knowledge Notes are
        // the source of truth) are wired at Gateway boot in
        // `register_graph_handlers` (agent_init), which needs the live
        // `MemoryBackend` / `NoteIndexer`. No placeholder is registered here: an
        // unwired registry correctly reports METHOD_NOT_FOUND rather than a
        // misleading stub error (see B7 cleanup — the previous placeholders were
        // dead code, always overridden before any request could reach them).

        // Tools visibility handlers — phase-1 placeholders; agent_init.rs overrides at boot.
        registry.register("tools.catalog", |req| async move {
            service_unavailable(req, "tools.catalog requires ToolRegistry (boot phase 2)")
        });
        registry.register("tools.effective", |req| async move {
            service_unavailable(req, "tools.effective requires ToolRegistry (boot phase 2)")
        });
        registry.register("tools.invoke", |req| async move {
            service_unavailable(req, "tools.invoke requires ToolRegistry (boot phase 2)")
        });

        // Dreaming admin handler — bypasses scheduler for deterministic E2E.
        // The handler always reads DREAM_DAEMON (a OnceCell) so it does not
        // need two-phase wiring; if the daemon was never initialized (memory
        // disabled or simulated mode), it simply returns an error.
        registry.register("dreaming.run_now", dreaming::handle_run_now);

        // Kanban data plane — structured goal/loop listing for the project
        // Kanban tab (design doc 2026-08-24-multiuser-teamchat-p3 §B1). Both
        // read a process-global store (`goal::global()` / `looping::global()`,
        // installed unconditionally at boot) so, like `dreaming.run_now`
        // above, they need no two-phase wiring. See `kanban.rs`'s module doc
        // for the `scope_id` filter's visibility rule.
        registry.register("goal.list", kanban::handle_list);
        registry.register("loop.list", kanban::handle_loop_list);

        // Background sub-agent tree snapshot — reads the process-global
        // BackgroundAgentTracker (no per-boot state), but P1 (spec §11-1c)
        // added a SessionStore dependency for per-user visibility filtering,
        // so this is now a phase-1 placeholder like the others below;
        // `register_artifact_handlers`-adjacent wiring overrides it at boot
        // phase 2 once the store exists. Backs the panel's cold-start; live
        // deltas arrive via the `run.subagent_tree` relay.
        registry.register("subagent.tree", |req| async move {
            service_unavailable(req, "subagent.tree requires SessionStore (boot phase 2)")
        });

        // Insights introspection handler — needs MemoryBackend; the real
        // handler is wired in Gateway startup (register_memory_handlers).
        registry.register("insights.tools", |req| async move {
            service_unavailable(req, "insights.tools requires MemoryBackend (boot phase 2)")
        });

        // Its sibling from the same registration function, which had no
        // phase-1 placeholder: before boot phase 2 it answered
        // METHOD_NOT_FOUND, which a client reads as "this core is too old to
        // have dream insights" rather than "ask again in a moment". Two
        // methods registered side by side should not disagree about what
        // "not ready yet" looks like.
        registry.register("dreaming.list_insights", |req| async move {
            service_unavailable(
                req,
                "dreaming.list_insights requires MemoryBackend (boot phase 2)",
            )
        });

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

        // Wizard — phase-1 stubs; boot path replaces with real handlers via install_wizard_handlers
        for method in [
            "wizard.start",
            "wizard.next",
            "wizard.answer",
            "wizard.cancel",
            "wizard.status",
        ] {
            registry.register(method, |req| async move {
                service_unavailable(req, "wizard manager not yet initialised")
            });
        }

        registry
    }

    /// Overlay real wizard handlers — call from the boot path once the
    /// session manager exists.
    pub fn install_wizard_handlers(
        &mut self,
        manager: Arc<crate::gateway::handlers::wizard::WizardSessionManager>,
    ) {
        for (name, handler) in crate::gateway::handlers::wizard::handlers(manager) {
            self.handlers.insert(name.to_string(), handler);
        }
    }

    /// Create an empty handler registry
    #[must_use]
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
    #[must_use]
    pub fn has_method(&self, method: &str) -> bool {
        self.handlers.contains_key(method)
    }

    /// Get a list of all registered method names
    #[must_use]
    pub fn methods(&self) -> Vec<String> {
        self.handlers.keys().cloned().collect()
    }

    /// Get the number of registered handlers
    #[must_use]
    pub fn len(&self) -> usize {
        self.handlers.len()
    }

    /// Check if the registry is empty
    #[must_use]
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

    #[test]
    fn remove_from_chain_drops_named_entry_and_reports_change() {
        // Deleting a provider that is present in the failover chain must scrub
        // it and signal the caller to persist the change. Regression guard for
        // the panel-delete bug that left `302ai` dangling in the chain.
        let mut chain = vec!["kimi-for-coding".to_string(), "302ai".to_string()];
        assert!(remove_from_chain(&mut chain, "302ai"));
        assert_eq!(chain, vec!["kimi-for-coding".to_string()]);
    }

    #[test]
    fn remove_from_chain_is_noop_when_absent() {
        // A provider that was never in the chain leaves it untouched and reports
        // "unchanged" so the caller skips an unnecessary section write.
        let mut chain = vec!["kimi-for-coding".to_string()];
        assert!(!remove_from_chain(&mut chain, "302ai"));
        assert_eq!(chain, vec!["kimi-for-coding".to_string()]);
    }

    #[test]
    fn remove_from_chain_drops_every_duplicate() {
        let mut chain = vec!["a".to_string(), "x".to_string(), "a".to_string()];
        assert!(remove_from_chain(&mut chain, "a"));
        assert_eq!(chain, vec!["x".to_string()]);
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
        assert!(registry.has_method("bundled.sync"));
    }

    #[test]
    fn test_identity_handlers_not_registered_until_boot() {
        // identity.get/set/clear/list are wired against the live agent
        // identity directory (SharedIdentityCtx) by register_identity_handlers
        // at server boot, not in HandlerRegistry::new(). An unwired registry
        // must report METHOD_NOT_FOUND rather than a misleading placeholder stub.
        let registry = HandlerRegistry::new();
        assert!(!registry.has_method("identity.get"));
        assert!(!registry.has_method("identity.set"));
        assert!(!registry.has_method("identity.clear"));
        assert!(!registry.has_method("identity.list"));
    }

    #[test]
    fn test_workspace_handlers_registered() {
        let registry = HandlerRegistry::new();
        assert!(registry.has_method("workspace.create"));
        assert!(registry.has_method("workspace.list"));
        assert!(registry.has_method("workspace.get"));
        assert!(registry.has_method("workspace.update"));
        assert!(registry.has_method("workspace.archive"));
        assert!(registry.has_method("workspace.unarchive"));
        assert!(registry.has_method("channels.set_agent"));
        assert!(registry.has_method("agents.bindings"));
    }

    /// Every `workspace.` method this registry knows about is admin-gated.
    ///
    /// Derived from `registry.methods()` rather than written out, because a
    /// literal list is the enumeration mistake one level up: it covers the
    /// family as it stood the day it was typed, and a method added later slips
    /// past it by being absent. There are already three hand-copied copies of
    /// this family across `method_admin.rs` and `method_visibility.rs`, and
    /// `workspace.unarchive` had to be added to each of them by hand.
    ///
    /// The gate is by prefix (`ADMIN_PREFIXES` carries `"workspace."`), so this
    /// cannot fail as things stand. What it pins is that carving one of them
    /// out becomes a visible act — deleting an assertion — rather than a silent
    /// gap between a new method and a table nobody remembered to update.
    #[test]
    fn every_registered_workspace_method_is_admin_gated() {
        let registry = HandlerRegistry::new();
        let family: Vec<String> = registry
            .methods()
            .into_iter()
            .filter(|m| m.starts_with("workspace."))
            .collect();
        assert!(
            !family.is_empty(),
            "the workspace family must be registered here — an empty filter \
             would make the loop below vacuous"
        );
        for method in family {
            assert!(
                crate::gateway::method_admin::method_requires_admin(&method),
                "{method} is reachable by a member"
            );
        }
    }

    #[test]
    fn test_wizard_stub_handlers_registered() {
        let registry = HandlerRegistry::new();
        assert!(registry.has_method("wizard.start"));
        assert!(registry.has_method("wizard.next"));
        assert!(registry.has_method("wizard.answer"));
        assert!(registry.has_method("wizard.cancel"));
        assert!(registry.has_method("wizard.status"));
    }
}
