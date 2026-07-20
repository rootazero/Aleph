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

    /// Inject `GatewayContext` after construction (breaks circular dependency).
    ///
    /// `BuiltinToolRegistry` is created before `ExecutionAdapter` exists, but
    /// `GatewayContext` needs `ExecutionAdapter`. This method allows deferred
    /// injection once all components are ready, enabling session.list and
    /// session.send tools.
    ///
    /// Takes `&self` (not `&mut self`) so it works through `Arc`.
    pub fn set_gateway_context(&self, context: Arc<GatewayContext>) {
        if self.gateway_context.set(context).is_ok() {
            info!("GatewayContext injected — session.list and session.send now available");
        }
    }

    /// Get a handle to the `GatewayContext` `OnceCell` for deferred injection.
    ///
    /// Used by `agent_init` to inject `GatewayContext` after `ExecutionEngine` creation.
    #[must_use]
    pub fn gateway_context_cell(&self) -> Arc<tokio::sync::OnceCell<Arc<GatewayContext>>> {
        Arc::clone(&self.gateway_context)
    }

    /// Inject `ChannelRegistry` after construction (deferred — channels are created after tools).
    ///
    /// Enables the `channel_pairing` tool for pairing code management.
    /// Takes `&self` (not `&mut self`) so it works through `Arc`.
    pub fn set_channel_registry(&self, registry: Arc<ChannelRegistry>) {
        if self.channel_registry_cell.set(registry).is_ok() {
            info!("ChannelRegistry injected — channel_pairing tool now available");
        }
    }

    /// Get a handle to the `ChannelRegistry` `OnceCell` for deferred injection.
    #[must_use]
    pub fn channel_registry_cell(&self) -> Arc<tokio::sync::OnceCell<Arc<ChannelRegistry>>> {
        Arc::clone(&self.channel_registry_cell)
    }

    /// Inject the `ClarificationManager` after construction (deferred — the
    /// manager is created alongside the channels). Enables the `ask_user` tool.
    /// Takes `&self` so it works through `Arc`.
    pub fn set_clarification_manager(
        &self,
        manager: Arc<crate::clarification::ClarificationManager>,
    ) {
        if self.clarification_manager_cell.set(manager).is_ok() {
            info!("ClarificationManager injected — ask_user tool now available");
        }
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

    /// Inject a `MemoryContextProvider` so the `remember` tool can resolve
    /// the per-agent `CuratedMemoryStore` at call time.
    ///
    /// Takes `&self` (not `&mut self`) so it works through `Arc` — the MCP is
    /// constructed after the registry has been wrapped in `Arc::new` in
    /// `agent_init`.
    pub fn set_memory_context_provider(&self, mcp: Arc<crate::thinker::MemoryContextProvider>) {
        if self.memory_context_provider.set(mcp).is_ok() {
            info!("MemoryContextProvider injected — `remember` tool now available");
        }
    }

    /// 注入集群节点登记表，启用集群节点工具。
    ///
    /// Takes `&self` so it works through `Arc` — the registry is wrapped in
    /// `Arc::new` in `agent_init` before the gateway's `NodeRegistry` is wired.
    pub fn set_node_registry(&self, registry: Arc<crate::cluster::NodeRegistry>) {
        if self.node_registry.set(registry).is_ok() {
            info!("NodeRegistry injected — cluster node tools (node_list / node_invoke / node_invoke_many / node_file) now available");
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
