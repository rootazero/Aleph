//! Inherent `impl BuiltinToolRegistry` — constructors, deferred-injection
//! setters, `OnceCell` handle accessors, and metadata lookups.
#![allow(unused_imports)]

use crate::sync_primitives::Arc;
use std::collections::HashMap;
use std::pin::Pin;

use serde_json::Value;
use tracing::{debug, error, info};

use crate::builtin_tools::sessions::{SessionsListTool, SessionsSendTool};
use crate::error::{AlephError, Result};
use crate::gateway::channel_registry::ChannelRegistry;
use crate::gateway::context::GatewayContext;
use crate::tool_metadata::{ToolSource, UnifiedTool};
use crate::tools::AlephTool;
use tokio::sync::RwLock;

use super::super::BuiltinToolConfig;
use super::free_fns::{parse_caller_agent_id, resolve_plugin_handler_from_sources};
use super::BuiltinToolRegistry;

impl BuiltinToolRegistry {
    /// Create a new registry with default configuration
    pub async fn new() -> crate::error::Result<Self> {
        Self::with_config(BuiltinToolConfig::default()).await
    }

    /// Register an additional tool (e.g., plugin tools discovered at runtime)
    pub fn register_tool(&mut self, tool: UnifiedTool) {
        self.tools.insert(tool.name.clone(), tool);
    }

    /// Late-bind a `ConfigPatcher` into `self_config` and `moa_manage` tools.
    ///
    /// `ConfigPatcher` is built by `register_agent_handlers` *after* the
    /// `BuiltinToolRegistry`, so the constructor cannot see it. Without this
    /// setter those tools ship permanently without a patcher and every write
    /// returns "patcher not available", even though their schema is advertised
    /// to the LLM.
    ///
    /// Takes `&self` (not `&mut Arc<Self>`) so the call works through the
    /// already-shared `Arc` the boot path moves into `ExecutionEngine::new`.
    /// `Arc::get_mut` only succeeds on a sole-owner Arc and silently no-ops
    /// otherwise, so the tools' `config_patcher` field is `OnceLock`-backed to
    /// allow late binding through `&self`.
    pub fn set_config_patcher(&self, patcher: Arc<crate::config::patcher::ConfigPatcher>) {
        self.self_config_tool.set_patcher(Arc::clone(&patcher));
        self.moa_manage_tool.set_patcher(Arc::clone(&patcher));
        tracing::info!("ConfigPatcher late-bound into self_config + moa tools");
    }

    /// Late-bind the `ConfigChanged` broadcast hook into the `self_config` tool.
    ///
    /// Same boot-order constraint as [`Self::set_config_patcher`]: the registry
    /// is shared via `Arc` before startup wiring runs, so the hook is injected
    /// through `&self` into the tool's `OnceLock`. Without it, LLM-driven
    /// config writes (`update_config` / `rollback_config`) never notify
    /// connected Panels — only the RPC `config.patch` path broadcast.
    pub fn set_config_broadcaster(
        &self,
        broadcaster: crate::builtin_tools::self_config::ConfigBroadcaster,
    ) {
        self.self_config_tool.set_config_broadcaster(broadcaster);
        tracing::info!("ConfigChanged broadcaster late-bound into self_config tool");
    }

    /// Extract the caller's `agent_id` for the tool call currently executing.
    ///
    /// Prefers the per-turn `TURN_CONTEXT` task-local — scoped by the dispatch
    /// chokepoint ([`ScopedToolService::execute`]) around every tool call — over
    /// the shared, mutable `session_context_handle`. Both resolve to
    /// `session_key.agent_id()`, but a concurrent run can overwrite the shared
    /// handle mid-turn, so reading it would bind this call to the *wrong*
    /// agent's identity (memory scope, session routing, MCP curated store).
    /// The handle stays as the fallback for any call site outside a turn scope;
    /// `fallback` is the last resort when neither source is present or parseable.
    ///
    /// [`ScopedToolService::execute`]: crate::tools::scoped::ScopedToolService
    pub(super) fn caller_agent_id(&self, fallback: &str) -> String {
        if let Some(agent_id) = crate::tools::turn_context::current_agent_id() {
            return agent_id;
        }
        self.session_context_handle
            .as_ref()
            .and_then(|h| h.try_read().ok())
            .map_or_else(
                || fallback.to_string(),
                |ctx| parse_caller_agent_id(&ctx.session_key_str, fallback),
            )
    }

    /// The memory PARTITION this tool call reads and writes — the composed
    /// answer, not the bare persona.
    ///
    /// [`Self::caller_agent_id`] returns `session_key.agent_id()`, i.e. `"main"`.
    /// That is the persona, and it is **not** where this run's memory lives:
    /// every producer composes it with the session's scope through
    /// [`crate::memory::project_scope::session_write_id`], so a scoped run
    /// writes to `main__u-alice` / `main__p-room`. A reader that skips the
    /// composition looks in `main` and finds nothing — with no error, and with
    /// every test green, because a test that builds the tool with a base id and
    /// asserts against that same base id never crosses the seam.
    ///
    /// This is NOT a multi-user-only concern: a zero-config loopback Panel
    /// session resolves to `(Some(OWNER_USER_ID), "operator")`, so the stock
    /// single-user partition is already `main__u-owner`.
    ///
    /// Both task-locals it depends on are live at tool dispatch (the turn
    /// context and the project root), because dispatch runs inside the scoped
    /// tool-execution task.
    pub(super) fn caller_memory_partition(&self, fallback: &str) -> String {
        crate::memory::project_scope::session_write_id(
            &self.caller_agent_id(fallback),
            self.memory_project_scoped,
            crate::projects::current_project_root().as_deref(),
        )
    }

    /// [`Self::caller_memory_partition`] for the ONE reader whose question has
    /// no answer in a shared room: the single human's profile.
    ///
    /// `None` inside a project room — a room has more than one person in it, so
    /// "there is no such thing here" is the honest answer rather than handing
    /// back the room's merged profile. That is precisely why
    /// [`crate::memory::project_scope::profile_floor_id`] returns an `Option`.
    pub(super) fn caller_profile_partition(&self, fallback: &str) -> Option<String> {
        crate::memory::project_scope::profile_floor_id(
            &self.caller_agent_id(fallback),
            self.memory_project_scoped,
            crate::projects::current_project_root().as_deref(),
        )
    }

    /// Get a handle to the `GatewayContext` `OnceCell` for deferred injection.
    ///
    /// Used by `agent_init` to inject `GatewayContext` after `ExecutionEngine` creation.
    #[must_use]
    pub fn gateway_context_cell(&self) -> Arc<tokio::sync::OnceCell<Arc<GatewayContext>>> {
        Arc::clone(&self.gateway_context)
    }

    /// Get a handle to the `ChannelRegistry` `OnceCell` for deferred injection.
    #[must_use]
    pub fn channel_registry_cell(&self) -> Arc<tokio::sync::OnceCell<Arc<ChannelRegistry>>> {
        Arc::clone(&self.channel_registry_cell)
    }

    /// Get a handle to the `ClarificationManager` `OnceCell` for deferred injection.
    #[must_use]
    pub fn clarification_manager_cell(
        &self,
    ) -> Arc<tokio::sync::OnceCell<Arc<crate::clarification::ClarificationManager>>> {
        Arc::clone(&self.clarification_manager_cell)
    }

    /// Inject a `MemoryReflector` into the `memory_reflect` tool (Task 8 wiring).
    ///
    /// Must be called before the registry is wrapped in `Arc` (takes `&mut self`).
    /// After injection the tool will synthesise answers from memory; without it
    /// the tool returns a clear error rather than panicking.
    pub fn set_memory_reflector(
        &mut self,
        reflector: Arc<crate::memory::reflector::MemoryReflector>,
    ) {
        if let Some(ref mut tool) = self.memory_reflect_tool {
            *tool = tool.clone().with_reflector(reflector);
            info!("MemoryReflector injected into memory_reflect tool");
        }
    }

    /// Inject the cluster node registry, enabling cluster node tools.
    ///
    /// Takes `&self` so it works through `Arc` — the registry is wrapped in
    /// `Arc::new` in `agent_init` before the gateway's `NodeRegistry` is wired.
    pub fn set_node_registry(&self, registry: Arc<crate::cluster::NodeRegistry>) {
        if self.node_registry.set(registry).is_ok() {
            info!("NodeRegistry injected — cluster node tools (node_list / node_invoke / node_invoke_many / node_file) now available");
        }
    }

    /// Inject the security store holding the `role=node` device records,
    /// enabling `node_manage`.
    ///
    /// Separate from `set_node_registry` because the registry holds only LIVE
    /// sessions: enrolled-but-offline nodes, and the `revoked_at` flag that
    /// makes a deregister stick, live only in the device records.
    pub fn set_node_security_store(&self, store: Arc<crate::gateway::security::SecurityStore>) {
        if self.node_security_store.set(store).is_ok() {
            info!("Cluster SecurityStore injected — `node_manage` now available");
        }
    }

    /// Get a handle to the `MemoryContextProvider` `OnceCell` for deferred
    /// injection from the server builder.
    #[must_use]
    pub fn memory_context_provider_cell(
        &self,
    ) -> Arc<tokio::sync::OnceCell<Arc<crate::thinker::MemoryContextProvider>>> {
        Arc::clone(&self.memory_context_provider)
    }

    /// Inject a `QueryFiler` into the `memory_reflect` tool (Spec 8 Task 8 wiring).
    ///
    /// Must be called before the registry is wrapped in `Arc` (takes `&mut self`).
    /// After injection the tool will fire-and-forget file interesting syntheses;
    /// without it the tool still works but query filing is silently skipped.
    pub fn set_query_filer(
        &mut self,
        query_filer: Arc<dyn crate::memory::notes::query_filer::QueryFiler>,
    ) {
        if let Some(ref mut tool) = self.memory_reflect_tool {
            *tool = tool.clone().with_query_filer(query_filer);
            info!("QueryFiler injected into memory_reflect tool");
        }
    }

    /// Get the parameter schema for a tool by name.
    ///
    /// Returns the schema if the tool exists in the internal registry and has
    /// a `parameters_schema` set. Used to attach schemas to the `UnifiedTool`
    /// list sent to the LLM so it knows which arguments to pass.
    #[must_use]
    pub fn get_tool_schema(&self, name: &str) -> Option<Value> {
        self.tools
            .get(name)
            .and_then(|t| t.parameters_schema.clone())
    }

    /// Returns `true` if a tool with this name has been registered in the metadata map.
    #[must_use]
    pub fn has_tool(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    /// Iterate every `UnifiedTool` registered in the runtime metadata map.
    ///
    /// The constructor populates this map conditionally (a tool is only
    /// present when its dependencies are wired) and with full parameter
    /// schemas, so it is the authoritative answer to "what can this registry
    /// actually execute right now". The LLM tool list is completed from this
    /// iterator — `BUILTIN_TOOL_DEFINITIONS` alone is a static subset that
    /// misses conditionally-registered and LLM-only tools.
    pub fn unified_tools(&self) -> impl Iterator<Item = &UnifiedTool> {
        self.tools.values()
    }

    pub(crate) fn resolve_plugin_handler(&self, tool_name: &str) -> Option<(String, String)> {
        resolve_plugin_handler_from_sources(
            self.extension_manager.as_deref(),
            &self.tools,
            tool_name,
        )
    }
}
