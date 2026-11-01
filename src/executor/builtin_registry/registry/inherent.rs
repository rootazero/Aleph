//! Inherent `impl BuiltinToolRegistry` — constructors, deferred-injection
//! setters, `OnceCell` handle accessors, and metadata lookups.
#![allow(unused_imports)]

use crate::sync_primitives::Arc;
use std::collections::HashMap;
use std::pin::Pin;

use serde_json::Value;
use tracing::{debug, error, info};

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
        let name = tool.name.clone();
        if self.tools.insert(name.clone(), tool).is_some() {
            // Overwrites stay allowed (plugin reload paths depend on it), but a
            // silent swap of a same-named tool is invisible in every log.
            tracing::warn!(tool = %name, "register_tool: replacing an already-registered tool");
        }
    }

    /// Late-bind a `ConfigPatcher` into the `self_config` tool.
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
        tracing::info!("ConfigPatcher late-bound into self_config tool");
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
    /// **Falling through to `fallback` is logged at `error!`** — this is a
    /// SECURITY fail-open: with every identity source exhausted, the tool
    /// keeps running under the fallback identity (`"main"`, the
    /// highest-privilege persona, at most call sites), operating on its memory
    /// partition, ACL, and session routing. The error log measures the
    /// production hit rate before the planned switch to fail-closed.
    ///
    /// [`ScopedToolService::execute`]: crate::tools::scoped::ScopedToolService
    pub(super) fn caller_agent_id(&self, fallback: &str) -> String {
        if let Some(agent_id) = crate::tools::turn_context::current_agent_id() {
            return agent_id;
        }
        if let Some(ctx) = self
            .session_context_handle
            .as_ref()
            .and_then(|h| h.try_read().ok())
        {
            return parse_caller_agent_id(&ctx.session_key_str, fallback);
        }
        // SECURITY fail-open: with every identity source exhausted we continue
        // under `fallback` ("main" — the highest-privilege persona — at most
        // call sites). `error!` to size the production hit rate before the
        // planned fail-closed switch; semantics unchanged for now.
        tracing::error!(
            fallback = %fallback,
            "SECURITY fail-open: BuiltinToolRegistry::caller_agent_id found no per-turn context \
             and no readable session_context_handle (missing handle or contended lock); \
             continuing under the fallback identity — \"main\", the highest-privilege persona, \
             at most call sites. Configure a turn scope at the call site (dispatcher \
             misconfiguration otherwise); this fallback is planned to become fail-closed once \
             the hit rate is observed."
        );
        fallback.to_string()
    }

    /// Stamp `__channel` / `__conversation_id` onto a tool call so a task it
    /// creates knows where to deliver its results.
    ///
    /// Single source for the two schedulers. `cron_manage` had this inline;
    /// `heartbeat_create` did not, which is why every heartbeat task shipped
    /// with `delivery_config: None` and silently dropped the finding its L2
    /// turn had just spent a full LLM call producing.
    ///
    /// Prefers the race-free per-turn context: `session_context_handle` is
    /// process-global and rewritten at every run start, so a concurrent run of
    /// another agent can swap it mid-turn and the created task would deliver
    /// into the WRONG conversation. The mirror is the fallback only outside a
    /// scoped turn (non-gateway paths, tests).
    ///
    /// A turn with no channel gets NO route, not a route to the empty string:
    /// `channel_id` / `conversation_id` are `String`, and they are empty for
    /// exactly the turns that have no channel — a governance or steward cron, a
    /// heartbeat probe, a webhook turn, a `tools.invoke` call. `Some("")` is
    /// not "no route", it is a route that fails every `is_some()` test the
    /// delivery side runs: `cron_manage` would stamp
    /// `source_channel_id = Some("")` instead of falling back to the
    /// configured channel, and a heartbeat would carry a delivery target that
    /// names a channel that does not exist. `None` is the honest value, and
    /// every one of those paths already handles it.
    pub(super) fn inject_delivery_route(&self, args: &mut serde_json::Value) {
        let (channel, conversation_id) = match crate::tools::turn_context::current_turn_context() {
            Some(t) => (Some(t.channel_id.clone()), Some(t.conversation_id.clone())),
            None => self
                .session_context_handle
                .as_ref()
                .and_then(|h| match h.try_read() {
                    Ok(ctx) => Some(ctx),
                    Err(_) => {
                        tracing::warn!(
                            "inject_delivery_route: session context lock contended; the created \
                             task will be stamped with no delivery route \
                             (__channel/__conversation_id missing) and its result may be silently \
                             dropped"
                        );
                        None
                    }
                })
                .map(|ctx| (Some(ctx.channel.clone()), Some(ctx.conversation_id.clone())))
                .unwrap_or((None, None)),
        };
        let Some(obj) = args.as_object_mut() else {
            return;
        };
        if let Some(channel) = channel.filter(|c| !c.is_empty()) {
            obj.insert("__channel".into(), serde_json::Value::String(channel));
        }
        if let Some(conversation_id) = conversation_id.filter(|c| !c.is_empty()) {
            obj.insert(
                "__conversation_id".into(),
                serde_json::Value::String(conversation_id),
            );
        }
    }

    /// Snapshot the current channel onto the tool arguments, so the agent
    /// tools act on the channel of THIS turn rather than on whichever
    /// channel the process-global mirror happens to hold.
    ///
    /// Prefers the race-free per-turn channel: `session_context_handle` is
    /// rewritten at every run start, so a concurrent run of another agent can
    /// swap it mid-turn and `agent_switch` would rebind the WRONG channel's
    /// active agent. The mirror is the fallback only outside a scoped turn.
    pub(super) fn with_current_channel(&self, mut args: serde_json::Value) -> serde_json::Value {
        let channel = match crate::tools::turn_context::current_turn_context() {
            Some(t) => Some(t.channel_id.clone()),
            None => self
                .session_context_handle
                .as_ref()
                .and_then(|h| match h.try_read() {
                    Ok(ctx) => Some(ctx),
                    Err(_) => {
                        tracing::warn!(
                            "agent management tools: session context lock contended; channel \
                             unresolved — no __channel injected"
                        );
                        None
                    }
                })
                .map(|ctx| ctx.channel.clone()),
        };
        if let (Some(channel), Some(obj)) = (channel, args.as_object_mut()) {
            obj.insert("__channel".into(), serde_json::Value::String(channel));
        }
        args
    }

    /// The current session key — the id the session-scoped tools stamp onto
    /// their arguments and the memory tools read their rows under.
    ///
    /// Prefers the race-free per-task turn context: `session_context_handle`
    /// is process-global and rewritten at every run start, so a concurrent run
    /// of another agent can swap it mid-turn and the caller would act on the
    /// WRONG session (the rule `recall_context` and `memory_search
    /// scope=current_session` follow).
    ///
    /// `None` means "no session to name". What that MEANS is the caller's
    /// decision, not this helper's: a tool that can stamp the key when it has
    /// one warns and continues, while a tool that has nothing to work with
    /// without it reports that it is unavailable.
    pub(super) fn current_session_key(&self) -> Option<String> {
        crate::tools::turn_context::current_session_key().or_else(|| {
            self.session_context_handle
                .as_ref()
                .and_then(|h| match h.try_read() {
                    Ok(ctx) => Some(ctx),
                    Err(_) => {
                        tracing::warn!(
                            "session context lock contended; the current session key is \
                             unresolved"
                        );
                        None
                    }
                })
                .map(|ctx| ctx.session_key_str.clone())
        })
    }

    /// Stamp the current session key onto a tool call, so the session-scoped
    /// tools act on THIS turn's session.
    ///
    /// A turn with no resolvable session key gets NO `__session_key` rather
    /// than a wrong one: the tool falls back to its own resolution, which is
    /// the same value the injection would have carried.
    pub(super) fn inject_session_key(&self, args: &mut serde_json::Value) {
        let Some(session_key) = self.current_session_key() else {
            return;
        };
        let Some(obj) = args.as_object_mut() else {
            return;
        };
        obj.insert(
            "__session_key".into(),
            serde_json::Value::String(session_key),
        );
    }

    /// The BASE identity every memory partition this registry composes is
    /// derived from — one resolution, one fallback, one partition key.
    ///
    /// The fallback is [`crate::routing::DEFAULT_AGENT_ID`] ("main"), the
    /// constant the WRITE side falls back to (`note_manage`'s `agent_id()`).
    /// The two sides have to agree: a partition key is
    /// `scoped_agent_id(base, &ref_id)` (`main__u-alice`), so a reader that
    /// falls back to a different base looks in `default__u-alice` while the
    /// writer filed the note under `main__u-alice` — the write is then
    /// invisible to every reader, with no error anywhere. One derivation, one
    /// fallback, one partition key.
    pub(super) fn caller_partition_base(&self) -> String {
        self.caller_agent_id(crate::routing::DEFAULT_AGENT_ID)
    }

    /// The memory PARTITION this tool call reads and writes — the composed
    /// answer, not the bare persona.
    ///
    /// [`Self::caller_partition_base`] returns `session_key.agent_id()`, i.e.
    /// `"main"`. That is the persona, and it is **not** where this run's
    /// memory lives: every producer composes it with the session's scope
    /// through [`crate::memory::project_scope::session_write_id`], so a scoped
    /// run writes to `main__u-alice` / `main__p-room`. A reader that skips the
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
    pub(super) fn caller_memory_partition(&self) -> String {
        crate::memory::project_scope::session_write_id(
            &self.caller_partition_base(),
            self.memory_project_scoped,
            crate::projects::current_project_root().as_deref(),
        )
    }

    /// The read-side twin of [`Self::caller_memory_partition`]: every partition
    /// an in-turn memory READ may see — `[org tier, this session's partition]`
    /// under a session scope, `[base]` (or the legacy project-directory pair)
    /// without one. `project_scope::session_read_ids` — the derivation the
    /// gateway face (`handlers::memory_scope::read_partitions`) also calls, but
    /// NOT with the same inputs: the gateway face passes
    /// `project_scoped = false` and no project root, so the two agree only
    /// while `memory.project_scoped` is off (the default, and the only case
    /// `caller_memory_read_partitions_is_the_gateway_faces_derivation` pins).
    /// With it on and no session scope, this adds the project-directory
    /// partition and the gateway face does not (判据 §9).
    pub(super) fn caller_memory_read_partitions(&self) -> Vec<String> {
        crate::memory::project_scope::session_read_ids(
            &self.caller_partition_base(),
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
    pub(super) fn caller_profile_partition(&self) -> Option<String> {
        crate::memory::project_scope::profile_floor_id(
            &self.caller_partition_base(),
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

    /// Inject the gateway event bus so `project_manage`'s mutations announce
    /// themselves on the same channel the `projects.*` RPC face uses.
    ///
    /// Without it the tool still works and still writes; it just cannot tell
    /// anyone, which leaves the "one verb, two faces, only one publishes"
    /// asymmetry this injection exists to close.
    pub fn set_gateway_event_bus(&self, bus: Arc<crate::gateway::event_bus::GatewayEventBus>) {
        if self.gateway_event_bus.set(bus).is_ok() {
            info!("GatewayEventBus injected — `project_manage` mutations now publish projects.changed");
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
