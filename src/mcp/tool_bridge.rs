//! Bridges `McpManagerEvent`s into the Phase-2 `ToolHandlerRegistry`.
//!
//! The MCP manager actor owns server lifecycle; it only emits events and never
//! imports the tool system (P1 low coupling). This bridge is the single place
//! that translates "a server's tools changed" into registry mutations, so the
//! live agent loop (`tool_registry_phase2` → `CoreDispatch` → harness `think`)
//! sees external MCP tools without the manager knowing the tool system exists.
//!
//! It also gates the builtin MCP utility tools — the readers
//! (`mcp_read_resource`, `mcp_get_prompt`) and their discovery twins
//! (`mcp_list_resources`, `mcp_list_prompts`): each pair is present only while
//! some connected server advertises that capability (a server of ANY project:
//! see `reconcile_capability_tools` for that cost), and the discovery twin is
//! always alongside the reader so the model can enumerate URIs/names instead
//! of guessing them (or falling back to `cat`).

use std::collections::HashMap;

use crate::sync_primitives::Arc;

use tokio::sync::broadcast::error::RecvError;
use tokio::task::JoinHandle;

use crate::builtin_tools::mcp_login::McpLoginTool;
use crate::builtin_tools::mcp_prompt::{McpGetPromptTool, McpListPromptsTool};
use crate::builtin_tools::mcp_resource::{
    McpListResourceTemplatesTool, McpListResourcesTool, McpReadResourceTool, VisibleServers,
};
use crate::mcp::manager::{McpManagerEvent, McpManagerHandle, McpTransportType};
use crate::tool_metadata::ToolCatalog;
use crate::tools::descriptor::ToolCapabilityDescriptor;
use crate::tools::handlers::builtin::BuiltinHandler;
use crate::tools::handlers::registration::{register_mcp_tools, unregister_mcp_tools};
use crate::tools::handlers::{McpServerFilter, ToolHandler};
use crate::tools::registration_scope::ToolRegistrationScope;
use crate::tools::registry::ToolHandlerRegistry;
use crate::tools::AlephToolDyn;

/// Registry name of the capability-gated resource-reading builtin.
pub(crate) const RESOURCE_TOOL: &str = "mcp_read_resource";
/// Registry name of the capability-gated resource-discovery builtin. Gated by
/// the same condition as [`RESOURCE_TOOL`]: without it the model can read a
/// resource but has no way to learn which resources exist (the gap that pushed
/// it to a raw `cat`).
pub(crate) const RESOURCE_LIST_TOOL: &str = "mcp_list_resources";
/// Registry name of the capability-gated resource-*template* discovery builtin.
/// Gated with the resource cluster: a server may expose resources ONLY by
/// template (concrete `resource_count == 0`), so the gate also fires on
/// `resource_template_count > 0` — otherwise a template-only server would strand
/// the model with no discoverable, readable handle (another `cat` fallback).
pub(crate) const RESOURCE_TEMPLATE_LIST_TOOL: &str = "mcp_list_resource_templates";
/// Registry name of the capability-gated prompt-fetching builtin.
pub(crate) const PROMPT_TOOL: &str = "mcp_get_prompt";
/// Registry name of the capability-gated prompt-discovery builtin. Gated by the
/// same condition as [`PROMPT_TOOL`].
pub(crate) const PROMPT_LIST_TOOL: &str = "mcp_list_prompts";
/// Registry name of the OAuth login builtin (present only with remote servers).
pub(crate) const LOGIN_TOOL: &str = "mcp_login";

/// The capability-gated bridge builtins that are pure reads, in the exact
/// spelling they register under.
///
/// Exists so `READ_ONLY_TOOLS` can pin its dynamic exceptions to the
/// definition site rather than re-typing the strings: these five never appear
/// in `BUILTIN_TOOL_DEFINITIONS`, so a rename here would otherwise orphan five
/// allowlist entries with nothing to fail. `LOGIN_TOOL` is deliberately absent
/// — an OAuth login is not a read.
#[cfg(test)]
pub(crate) const CAPABILITY_READ_BUILTIN_NAMES: &[&str] = &[
    RESOURCE_TOOL,
    RESOURCE_LIST_TOOL,
    RESOURCE_TEMPLATE_LIST_TOOL,
    PROMPT_TOOL,
    PROMPT_LIST_TOOL,
];

/// Spawn the MCP → `ToolHandlerRegistry` bridge task.
///
/// The task subscribes to the manager's event broadcast and keeps `registry`
/// in sync with every server's discovered tools. Returns the `JoinHandle` so
/// callers may abort it on shutdown; dropping the handle merely detaches the
/// task, which exits on its own once the manager's event channel closes.
///
/// # The subscription is not enough on its own
///
/// A `broadcast` receiver only delivers what is sent *after* it subscribes, and
/// boot auto-starts every persisted server inside `McpManagerActor::run` — which
/// is spawned long before this function is called, because the bridge waits for
/// the tool catalog to exist so it can attach health probes. So by the time the
/// subscription opens, the ordinary deployment has already emitted (and lost)
/// one `ServerStarted` per configured server, and their tools would never reach
/// the registry: the manager reports them healthy, `mcp.list` shows their tool
/// counts, and the model is offered none of them. Only a later event — a manual
/// restart, an add, a crash, or a server-sent `tools/list_changed` — would ever
/// repair it.
///
/// The startup reconcile closes that window. It is also self-synchronising: the
/// actor answers `ListServers` from its command loop, which it does not enter
/// until auto-start has finished, so the first reconcile always observes the
/// complete set rather than a half-started one.
#[must_use]
pub fn spawn_tool_bridge(
    handle: McpManagerHandle,
    registry: Arc<ToolHandlerRegistry>,
    tool_catalog: Option<Arc<ToolCatalog>>,
) -> JoinHandle<()> {
    // Subscribe before the reconcile below, not after: anything that starts
    // while the reconcile is in flight then arrives as an event, and
    // `sync_server` is idempotent about the overlap.
    let mut events = handle.subscribe();
    tokio::spawn(async move {
        tracing::info!("MCP tool bridge started");
        // Whether each capability cluster is currently live in the registry.
        let mut resource_live = false;
        let mut prompt_live = false;
        let mut login_live = false;
        // Per-server ownership: every registry entry a server's sync makes is
        // tracked in that server's scope, keyed by `server_id`, so teardown is
        // generation-guarded and can never remove a replacement.
        let mut server_scopes: HashMap<String, ToolRegistrationScope> = HashMap::new();
        // Capability builtins live in their own scopes, separate from every
        // server and from each other, so a cluster can be disposed alone.
        let mut capabilities = CapabilityScopes::new();
        let catalog = tool_catalog.as_ref();
        // One-time residue sweep. A previous process, or the boot auto-start
        // path that ran before this bridge subscribed, may have left registry
        // entries that no scope owns. The scope map is still empty here, so a
        // name-based sweep cannot race a replacement.
        sweep_server_residue(&handle, &registry, catalog).await;
        resync_all(&handle, &registry, catalog, &mut server_scopes).await;
        reconcile_capability_tools(
            &handle,
            &registry,
            &mut capabilities,
            &mut resource_live,
            &mut prompt_live,
            &mut login_live,
        )
        .await;
        loop {
            match events.recv().await {
                Ok(event) => {
                    apply_event(&handle, &registry, catalog, &mut server_scopes, event).await;
                    reconcile_capability_tools(
                        &handle,
                        &registry,
                        &mut capabilities,
                        &mut resource_live,
                        &mut prompt_live,
                        &mut login_live,
                    )
                    .await;
                }
                Err(RecvError::Lagged(skipped)) => {
                    // A burst of events overran the broadcast buffer. We may
                    // have missed a transition, so reconcile every server
                    // against the registry rather than guessing.
                    tracing::warn!(skipped, "MCP tool bridge lagged; resyncing all servers");
                    resync_all(&handle, &registry, catalog, &mut server_scopes).await;
                    reconcile_capability_tools(
                        &handle,
                        &registry,
                        &mut capabilities,
                        &mut resource_live,
                        &mut prompt_live,
                        &mut login_live,
                    )
                    .await;
                }
                Err(RecvError::Closed) => {
                    tracing::info!("MCP manager event channel closed; tool bridge exiting");
                    dispose_bridge_scopes(&mut server_scopes, &mut capabilities).await;
                    break;
                }
            }
        }
    })
}

/// Translate a single manager event into registry mutations.
async fn apply_event(
    handle: &McpManagerHandle,
    registry: &ToolHandlerRegistry,
    tool_catalog: Option<&Arc<ToolCatalog>>,
    server_scopes: &mut HashMap<String, ToolRegistrationScope>,
    event: McpManagerEvent,
) {
    match event {
        McpManagerEvent::ServerStarted { server_id, .. }
        | McpManagerEvent::ToolsChanged { server_id, .. } => {
            sync_server(handle, registry, tool_catalog, server_scopes, &server_id).await;
        }
        McpManagerEvent::ServerStopped { server_id, .. }
        | McpManagerEvent::ServerCrashed { server_id, .. }
        | McpManagerEvent::ServerRemoved { server_id, .. } => {
            // Dispose exactly the departing server's scope. Its handles are
            // generation-guarded, so a stale scope can never delete a
            // replacement; every other server's scope is left untouched.
            if let Some(scope) = server_scopes.remove(&server_id) {
                dispose_server_scope(scope, &server_id).await;
            } else {
                // No scope ever owned this server (its sync failed, or the
                // entry predates the scope map). Fall back to the name-based
                // compatibility sweep so nothing leaks.
                let removed = unregister_mcp_tools(registry, tool_catalog, &server_id).await;
                if !removed.is_empty() {
                    tracing::info!(
                        server_id = %server_id,
                        count = removed.len(),
                        "MCP tool bridge: swept unowned tools for departed server"
                    );
                }
            }
        }
        // ServerRestarting is followed by ServerStarted (resync) or
        // ServerCrashed (unregister); no action needed on the transition.
        _ => {}
    }
}

/// Re-fetch one server's tools and replace its registry entries.
///
/// Stale entries are cleared first so a server that drops a tool (a shrinking
/// `tools/list`) does not leave a dangling registry handler behind.
async fn sync_server(
    handle: &McpManagerHandle,
    registry: &ToolHandlerRegistry,
    tool_catalog: Option<&Arc<ToolCatalog>>,
    server_scopes: &mut HashMap<String, ToolRegistrationScope>,
    server_id: &str,
) {
    let client = match handle.get_client(server_id).await {
        Ok(Some(client)) => client,
        Ok(None) => {
            tracing::debug!(
                server_id,
                "MCP tool bridge: no client for server; skipping sync"
            );
            return;
        }
        Err(e) => {
            tracing::warn!(server_id, error = %e, "MCP tool bridge: get_client failed");
            return;
        }
    };
    // The server's configured request timeout bounds every tools/call
    // roundtrip; the handlers declare it so the harness's per-tool wall clock
    // sits above the MCP client's own and cannot preempt a call the client
    // would still have returned. Unknown config → `None` → the client default.
    let timeout_seconds = handle
        .list_server_configs()
        .await
        .ok()
        .and_then(|configs| configs.into_iter().find(|c| c.id == server_id))
        .and_then(|c| c.timeout_seconds);
    let tools = client.list_tools().await;
    // Replace this server's previous scope. Disposing it first is what makes a
    // shrinking `tools/list` safe: the stale generation-guarded handles cannot
    // remove the entries the fresh registration is about to create, and the
    // dropped tool has no surviving handler.
    if let Some(previous) = server_scopes.remove(server_id) {
        dispose_server_scope(previous, server_id).await;
    }
    let mut scope = ToolRegistrationScope::new(format!("mcp:server:{server_id}"));
    let registered = register_mcp_tools(
        registry,
        tool_catalog,
        client,
        server_id,
        &tools,
        timeout_seconds,
        &mut scope,
    )
    .await;
    server_scopes.insert(server_id.to_string(), scope);
    tracing::info!(
        server_id,
        count = registered.len(),
        "MCP tool bridge: synced server tools into registry"
    );
}

/// One-time startup cleanup of registry entries that no scope owns. Only ever
/// called before the bridge begins syncing, while the scope map is still
/// empty, so a name-based sweep for a known server cannot remove a replacement.
async fn sweep_server_residue(
    handle: &McpManagerHandle,
    registry: &ToolHandlerRegistry,
    tool_catalog: Option<&Arc<ToolCatalog>>,
) {
    match handle.list_servers().await {
        Ok(servers) => {
            for info in servers {
                let removed = unregister_mcp_tools(registry, tool_catalog, &info.id).await;
                if !removed.is_empty() {
                    tracing::info!(
                        server_id = %info.id,
                        count = removed.len(),
                        "MCP tool bridge: swept pre-existing tools for server"
                    );
                }
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "MCP tool bridge: residue sweep list_servers failed");
        }
    }
}

/// Dispose one server's scope, logging (but never aborting on) step failures —
/// a failed step must not skip the rest of the teardown.
async fn dispose_server_scope(scope: ToolRegistrationScope, server_id: &str) {
    let report = scope.dispose().await;
    for (step, error) in report.failures() {
        tracing::warn!(
            server_id,
            step,
            error = %error,
            "MCP tool bridge: server scope dispose step failed"
        );
    }
}

/// Reconcile every known server against the registry. Used after a broadcast
/// lag, where individual transitions may have been dropped.
async fn resync_all(
    handle: &McpManagerHandle,
    registry: &ToolHandlerRegistry,
    tool_catalog: Option<&Arc<ToolCatalog>>,
    server_scopes: &mut HashMap<String, ToolRegistrationScope>,
) {
    match handle.list_servers().await {
        Ok(servers) => {
            let present: std::collections::HashSet<String> =
                servers.iter().map(|s| s.id.clone()).collect();
            for info in &servers {
                sync_server(handle, registry, tool_catalog, server_scopes, &info.id).await;
            }
            // Departures missed while lagged: dispose the scope of any server
            // that left while we were not receiving its event.
            let departed: Vec<String> = server_scopes
                .keys()
                .filter(|id| !present.contains(*id))
                .cloned()
                .collect();
            for id in departed {
                if let Some(scope) = server_scopes.remove(&id) {
                    dispose_server_scope(scope, &id).await;
                }
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "MCP tool bridge: resync_all list_servers failed");
        }
    }
}

/// Independently-disposable owner scopes for the capability builtins.
///
/// Each cluster — the resource read+discovery trio, the prompt read+discovery
/// pair, and the OAuth login tool — is owned by its own scope, so removing one
/// cluster leaves the other clusters and every server's tools untouched.
struct CapabilityScopes {
    resource: ToolRegistrationScope,
    prompt: ToolRegistrationScope,
    login: ToolRegistrationScope,
}

impl CapabilityScopes {
    fn new() -> Self {
        Self {
            resource: ToolRegistrationScope::new("mcp:capability:resource"),
            prompt: ToolRegistrationScope::new("mcp:capability:prompt"),
            login: ToolRegistrationScope::new("mcp:capability:login"),
        }
    }
}

/// Register or unregister the capability builtins so each is present only
/// while at least one connected server advertises that capability.
///
/// The presence switch is process-global, like the bridge: a server counts
/// whichever project's plugin declared it. What a builtin then *sees* is
/// per-run — it is registered as a [`CapabilityHandler`] that the run loop's
/// MCP join binds to that run's visible servers. Accepted cost: in a project
/// that can see no resource (or prompt) server, the builtins are still
/// offered, and the listers answer an empty list and the readers "no such
/// server" — exactly what they answer for a server that does not exist.
async fn reconcile_capability_tools(
    handle: &McpManagerHandle,
    registry: &ToolHandlerRegistry,
    scopes: &mut CapabilityScopes,
    resource_live: &mut bool,
    prompt_live: &mut bool,
    login_live: &mut bool,
) {
    let servers = match handle.list_servers().await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "MCP tool bridge: list_servers failed during reconcile");
            return;
        }
    };
    // The whole resource cluster (read + concrete-list + template-list) shares
    // one gate: a template-only server (resource_count == 0) still needs the
    // reader live to fetch a filled template URI, so templates count toward it.
    let want_resource = servers
        .iter()
        .any(|s| s.resource_count > 0 || s.resource_template_count > 0);
    let want_prompt = servers.iter().any(|s| s.prompt_count > 0);
    // OAuth only applies to remote transports; with stdio-only servers the
    // login tool would be pure noise in the tool surface.
    let want_login = servers
        .iter()
        .any(|s| !matches!(s.transport, McpTransportType::Stdio));

    if want_resource != *resource_live {
        if want_resource {
            // The read + discovery tools share the resource capability gate:
            // offering one without the other either strands the model (read
            // with no way to discover) or dangles a discovery tool over
            // nothing. The template-list tool rides the same gate (see
            // `RESOURCE_TEMPLATE_LIST_TOOL`).
            set_capability(
                registry,
                handle,
                &mut scopes.resource,
                RESOURCE_LIST_TOOL,
                |s| Arc::new(McpListResourcesTool::new(s)),
            );
            set_capability(
                registry,
                handle,
                &mut scopes.resource,
                RESOURCE_TEMPLATE_LIST_TOOL,
                |s| Arc::new(McpListResourceTemplatesTool::new(s)),
            );
            *resource_live =
                set_capability(registry, handle, &mut scopes.resource, RESOURCE_TOOL, |s| {
                    Arc::new(McpReadResourceTool::new(s))
                });
        } else {
            dispose_capability_scope(&mut scopes.resource, "resource").await;
            *resource_live = false;
        }
    }
    if want_prompt != *prompt_live {
        if want_prompt {
            set_capability(
                registry,
                handle,
                &mut scopes.prompt,
                PROMPT_LIST_TOOL,
                |s| Arc::new(McpListPromptsTool::new(s)),
            );
            *prompt_live = set_capability(registry, handle, &mut scopes.prompt, PROMPT_TOOL, |s| {
                Arc::new(McpGetPromptTool::new(s))
            });
        } else {
            dispose_capability_scope(&mut scopes.prompt, "prompt").await;
            *prompt_live = false;
        }
    }
    if want_login != *login_live {
        if want_login {
            *login_live = set_capability(registry, handle, &mut scopes.login, LOGIN_TOOL, |s| {
                Arc::new(McpLoginTool::new(s))
            });
        } else {
            dispose_capability_scope(&mut scopes.login, "login").await;
            *login_live = false;
        }
    }
}

async fn dispose_bridge_scopes(
    server_scopes: &mut HashMap<String, ToolRegistrationScope>,
    capabilities: &mut CapabilityScopes,
) {
    let server_ids: Vec<String> = server_scopes.keys().cloned().collect();
    for server_id in server_ids {
        if let Some(scope) = server_scopes.remove(&server_id) {
            dispose_server_scope(scope, &server_id).await;
        }
    }
    dispose_capability_scope(&mut capabilities.resource, "resource").await;
    dispose_capability_scope(&mut capabilities.prompt, "prompt").await;
    dispose_capability_scope(&mut capabilities.login, "login").await;
}

/// Dispose a capability cluster's scope and replace it with a fresh empty one
/// so a later enable re-registers cleanly. Step failures are logged, never
/// propagated: one bad step must not skip the rest of the teardown.
async fn dispose_capability_scope(scope: &mut ToolRegistrationScope, owner: &str) {
    let previous = std::mem::replace(
        scope,
        ToolRegistrationScope::new(format!("mcp:capability:{owner}")),
    );
    let report = previous.dispose().await;
    for (step, error) in report.failures() {
        tracing::warn!(
            owner,
            step,
            error = %error,
            "MCP capability builtin scope dispose step failed"
        );
    }
}

/// Builds one capability builtin over the servers a caller may see.
type BuildCapability = fn(VisibleServers) -> Arc<dyn AlephToolDyn>;

/// Add a single capability builtin to its cluster scope; returns whether it
/// registered. The returned handle is tracked in `scope`, so removing the
/// whole cluster is one generation-guarded `scope.dispose()`.
fn set_capability(
    registry: &ToolHandlerRegistry,
    handle: &McpManagerHandle,
    scope: &mut ToolRegistrationScope,
    name: &'static str,
    build: BuildCapability,
) -> bool {
    // rust-doctor-disable-next-line excessive-clone
    let handle = handle.clone();
    let handler: Arc<dyn ToolHandler> = Arc::new(CapabilityHandler::new(name, handle, build));
    // Project the descriptor from the handler's own definition so the
    // registry pairing check is satisfied by construction.
    let descriptor = ToolCapabilityDescriptor::from_definition(&handler.definition(), 0);
    match registry.register(descriptor, handler) {
        Ok(handle) => {
            // Own the generation-guarded disposer so the cluster scope can
            // tear this builtin down without a name-based `unregister` that
            // could remove a replacement.
            debug_assert_eq!(handle.name(), name);
            scope.track(handle);
            tracing::info!(
                tool = name,
                "MCP tool bridge: capability builtin registered"
            );
            true
        }
        Err(e) => {
            tracing::warn!(tool = name, error = ?e, "MCP capability builtin register failed");
            false
        }
    }
}

/// A capability builtin as the bridge registers it.
///
/// The bridge is process-global and knows no run, so the registered form sees
/// NO server at all. The run loop's MCP join binds it to that run's visible
/// servers ([`ToolHandler::bind_visible_servers`]) and joins only the bound
/// form, which is a plain [`BuiltinHandler`] — so its definition, budget and
/// concurrency claim are exactly what the registered form advertises.
struct CapabilityHandler {
    name: &'static str,
    handle: McpManagerHandle,
    build: BuildCapability,
    unbound: BuiltinHandler,
}

impl CapabilityHandler {
    fn new(name: &'static str, handle: McpManagerHandle, build: BuildCapability) -> Self {
        let none: McpServerFilter = Arc::new(|_| false);
        // rust-doctor-disable-next-line excessive-clone
        let tool = build(VisibleServers::new(handle.clone(), none));
        // Bridge builtins cross an untrusted MCP boundary on every call:
        // a server can return text containing `<function_results>` /
        // `<tool_result>` markers and try to inject another model turn, so the
        // harness fences their output before showing it to the model. The
        // trait default already answers `true` for `ToolSource::Mcp`, but
        // these five are registered as `Builtin` (the bridge is process-global
        // and knows no per-server source at registration time); turning the
        // knob on here keeps the fence decision at the handler that actually
        // owns the cross-boundary call.
        let unbound = BuiltinHandler::new(name.to_string(), tool).with_fences_output(true);
        Self {
            name,
            handle,
            build,
            unbound,
        }
    }
}

#[async_trait::async_trait]
impl ToolHandler for CapabilityHandler {
    async fn invoke(
        &self,
        input: serde_json::Value,
    ) -> Result<crate::session::events::ToolOutput, crate::tools::service::ToolError> {
        self.unbound.invoke(input).await
    }

    fn definition(&self) -> crate::tools::service::ToolDefinition {
        self.unbound.definition()
    }

    fn bind_visible_servers(&self, visible: &McpServerFilter) -> Option<Arc<dyn ToolHandler>> {
        // rust-doctor-disable-next-line excessive-clone
        let servers = VisibleServers::new(self.handle.clone(), Arc::clone(visible));
        let tool = (self.build)(servers);
        Some(Arc::new(
            BuiltinHandler::new(self.name.to_string(), tool).with_fences_output(true),
        ))
    }

    fn fences_output(&self) -> bool {
        self.unbound.fences_output()
    }
}

/// A fake MCP manager for the capability-builtin tests here, in
/// `builtin_tools` and in `run_loop`: already up with `servers` connected, it
/// answers only what the bridge and the builtins ask, and records every
/// server id a client or a status was asked for.
#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use crate::mcp::manager::{HealthStatus, McpCommand, McpManagerConfig, McpServerInfo};
    use crate::mcp::manager::{McpServerStatusDetail, ServerHealth};
    use tokio::sync::{broadcast, mpsc};

    pub(crate) fn server_info(id: &str) -> McpServerInfo {
        McpServerInfo {
            id: id.to_string(),
            name: id.to_string(),
            transport: McpTransportType::Stdio,
            tool_count: 0,
            resource_count: 0,
            resource_template_count: 0,
            prompt_count: 0,
            health: HealthStatus::default(),
        }
    }

    /// A remote server advertising every capability, so all six builtins are
    /// switched on.
    pub(crate) fn capable_server(id: &str) -> McpServerInfo {
        McpServerInfo {
            transport: McpTransportType::Http,
            resource_count: 1,
            resource_template_count: 1,
            prompt_count: 1,
            ..server_info(id)
        }
    }

    pub(crate) struct FakeManager {
        pub(crate) handle: McpManagerHandle,
        asked: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        task: JoinHandle<()>,
    }

    impl FakeManager {
        /// Every server id a client or a status was asked for, in order.
        pub(crate) fn asked(&self) -> Vec<String> {
            self.asked.lock().unwrap_or_else(|e| e.into_inner()).clone()
        }

        pub(crate) fn forget_asked(&self) {
            self.asked.lock().unwrap_or_else(|e| e.into_inner()).clear();
        }
    }

    impl Drop for FakeManager {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    /// It never emits an event — the real one emits them during `run()`,
    /// before any bridge exists. A client or a status exists only for a
    /// server in `servers`; the client has no transport, so every list it
    /// answers is empty. The status reports a stdio config, so a login that
    /// got past resolution fails without touching the network.
    pub(crate) fn fake_manager(servers: Vec<McpServerInfo>) -> FakeManager {
        let (tx, mut rx) = mpsc::channel::<McpCommand>(32);
        let (event_tx, _) = broadcast::channel(32);
        let handle = McpManagerHandle::new(tx, event_tx);
        let asked = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = std::sync::Arc::clone(&asked);
        let task = tokio::spawn(async move {
            let known = |id: &str| servers.iter().find(|s| s.id == id).cloned();
            let note = |id: &str| {
                log.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(id.to_string());
            };
            while let Some(cmd) = rx.recv().await {
                match cmd {
                    McpCommand::ListServers { respond_to } => {
                        let _ = respond_to.send(servers.clone());
                    }
                    McpCommand::GetClient {
                        server_id,
                        respond_to,
                    } => {
                        note(&server_id);
                        let client =
                            known(&server_id).map(|_| Arc::new(crate::mcp::McpClient::new()));
                        let _ = respond_to.send(client);
                    }
                    McpCommand::GetStatus {
                        server_id,
                        respond_to,
                    } => {
                        note(&server_id);
                        let detail = known(&server_id).map(|info| McpServerStatusDetail {
                            config: McpManagerConfig::stdio(&info.id, &info.name, "true"),
                            id: info.id,
                            name: info.name,
                            transport: info.transport,
                            health: ServerHealth::default(),
                            tools: Vec::new(),
                            resources: Vec::new(),
                            prompts: Vec::new(),
                        });
                        let _ = respond_to.send(detail);
                    }
                    McpCommand::ListServerConfigs { respond_to } => {
                        let _ = respond_to.send(Vec::new());
                    }
                    _ => {}
                }
            }
        });
        FakeManager {
            handle,
            asked,
            task,
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::test_support::{fake_manager, server_info};
    use super::*;
    use crate::mcp::McpTool;
    use crate::tools::handlers::mcp::McpHandler;
    use crate::tools::handlers::registration::register_mcp_tools;

    fn mcp_tool(name: &str) -> McpTool {
        McpTool {
            name: name.into(),
            description: "d".into(),
            input_schema: serde_json::json!({"type": "object"}),
            requires_confirmation: false,
            read_only: false,
            idempotent: false,
        }
    }

    /// Each capability builtin exactly as `reconcile_capability_tools`
    /// registers it over `servers`, bound to `visible` the way the run loop's
    /// MCP join binds it.
    async fn bound_builtins(
        fake: &test_support::FakeManager,
        visible: &McpServerFilter,
    ) -> std::collections::HashMap<String, Arc<dyn ToolHandler>> {
        let registry = ToolHandlerRegistry::new();
        let (mut resource, mut prompt, mut login) = (false, false, false);
        let mut capabilities = CapabilityScopes::new();
        reconcile_capability_tools(
            &fake.handle,
            &registry,
            &mut capabilities,
            &mut resource,
            &mut prompt,
            &mut login,
        )
        .await;
        assert!(
            resource && prompt && login,
            "premise: every builtin is switched on"
        );
        registry
            .snapshot()
            .iter()
            .map(|(name, handler)| {
                let bound = handler
                    .bind_visible_servers(visible)
                    .expect("a capability builtin binds to the run's servers");
                (name.clone(), bound)
            })
            .collect()
    }

    /// Face ⑤ inside the capability builtins. Run A has both servers but may
    /// not see `plugin:proj/srv`; run B's manager never had it. Every
    /// resolving builtin answers A exactly what it answers B — string-equal,
    /// so no error text can name another project's plugin — and the listers
    /// never ask A's manager about the invisible server at all.
    #[tokio::test]
    async fn a_bound_capability_builtin_treats_an_invisible_server_as_absent() {
        use serde_json::json;
        use test_support::{capable_server, fake_manager};
        const HIDDEN: &str = "plugin:proj/srv";
        let a = fake_manager(vec![capable_server(HIDDEN), capable_server("github")]);
        let b = fake_manager(vec![capable_server("github")]);
        let hide: McpServerFilter = Arc::new(|id| id != HIDDEN);
        let all: McpServerFilter = Arc::new(|_| true);
        let in_a = bound_builtins(&a, &hide).await;
        let in_b = bound_builtins(&b, &all).await;
        assert_eq!(in_a.len(), 6, "all six builtins: {:?}", in_a.keys());

        for (name, args) in [
            (
                RESOURCE_TOOL,
                json!({ "uri": format!("{HIDDEN}:file:///x") }),
            ),
            (PROMPT_TOOL, json!({ "name": format!("{HIDDEN}:p") })),
            (LOGIN_TOOL, json!({ "server": HIDDEN })),
        ] {
            let hidden = in_a[name].invoke(args.clone()).await.map(|o| o.value);
            let absent = in_b[name].invoke(args).await.map(|o| o.value);
            let hidden = hidden.expect_err("an invisible server resolves to nothing");
            let absent = absent.expect_err("an absent server resolves to nothing");
            assert_eq!(
                hidden.to_string(),
                absent.to_string(),
                "{name}: an invisible server must answer what an unknown one answers"
            );
        }

        for name in [
            RESOURCE_LIST_TOOL,
            RESOURCE_TEMPLATE_LIST_TOOL,
            PROMPT_LIST_TOOL,
        ] {
            a.forget_asked();
            in_a[name].invoke(json!({})).await.expect("the lister runs");
            assert_eq!(a.asked(), ["github"], "{name} asked about");
            let narrowed = json!({ "server": HIDDEN });
            let hidden = in_a[name].invoke(narrowed.clone()).await.unwrap().value;
            let absent = in_b[name].invoke(narrowed).await.unwrap().value;
            assert_eq!(hidden, absent, "{name} narrowed to the invisible server");
        }
    }

    #[tokio::test]
    async fn bridge_scope_shutdown_removes_server_and_capability_tools() {
        use test_support::capable_server;
        let registry = ToolHandlerRegistry::new();
        let manager = fake_manager(vec![capable_server("remote")]);
        let mut capabilities = CapabilityScopes::new();
        let (mut resource_live, mut prompt_live, mut login_live) = (false, false, false);
        reconcile_capability_tools(
            &manager.handle,
            &registry,
            &mut capabilities,
            &mut resource_live,
            &mut prompt_live,
            &mut login_live,
        )
        .await;
        assert!(resource_live && prompt_live && login_live);

        let mut server_scopes = HashMap::new();
        let client = Arc::new(crate::mcp::McpClient::new());
        let mut server_scope = ToolRegistrationScope::new("mcp:server:local");
        register_mcp_tools(
            &registry,
            None,
            client,
            "local",
            &[mcp_tool("owned")],
            None,
            &mut server_scope,
        )
        .await;
        server_scopes.insert("local".to_string(), server_scope);

        assert_eq!(registry.snapshot().len(), 7);
        dispose_bridge_scopes(&mut server_scopes, &mut capabilities).await;
        assert!(registry.snapshot().is_empty());
        assert!(server_scopes.is_empty());
    }

    async fn eventually_absent(registry: &ToolHandlerRegistry, name: &str) -> bool {
        for _ in 0..100 {
            if !registry.snapshot().contains_key(name) {
                return true;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        false
    }

    /// **The bridge must reconcile once at startup, not wait for an event.**
    ///
    /// Boot auto-starts every persisted server inside `McpManagerActor::run`,
    /// which is spawned well before `spawn_tool_bridge` (the bridge waits for
    /// the tool catalog). A `broadcast` receiver delivers nothing sent before it
    /// subscribed, so an events-only bridge learns about *none* of the servers
    /// the deployment actually has: the manager reports them healthy and
    /// `mcp.list` shows their tool counts, while the model is offered none of
    /// their tools until someone restarts a server by hand.
    ///
    /// Observed on a live daemon three boots running (2026-08-04, real
    /// chrome-devtools-mcp: 29 tools healthy, 0 reaching the model).
    ///
    /// The assertion is on the registry, not on the call: a stale entry for an
    /// already-connected server is swept only if `sync_server` actually ran for
    /// it, and `sync_server` is reached at startup only through the reconcile.
    #[tokio::test]
    async fn a_server_that_connected_before_the_bridge_existed_is_still_reconciled() {
        let registry = Arc::new(ToolHandlerRegistry::new());
        register_mcp_tools(
            &registry,
            None,
            Arc::new(crate::mcp::McpClient::new()),
            "srv",
            &[McpTool {
                name: "ghost".into(),
                description: "left over from a previous connection".into(),
                input_schema: serde_json::json!({"type": "object"}),
                requires_confirmation: false,
                read_only: false,
                idempotent: false,
            }],
            None,
            &mut ToolRegistrationScope::new("mcp:server:srv"),
        )
        .await;
        assert!(
            registry.snapshot().contains_key("srv__ghost"),
            "precondition: the registry starts with a stale entry for srv"
        );

        // The fake's client has no transport: `list_tools()` is empty, so a
        // sync registers nothing and only the stale-entry sweep is observable.
        let manager = fake_manager(vec![server_info("srv")]);
        let bridge = spawn_tool_bridge(manager.handle.clone(), Arc::clone(&registry), None);

        assert!(
            eventually_absent(&registry, "srv__ghost").await,
            "the bridge never reconciled the already-connected server: it is \
             waiting for an event that was emitted before it subscribed"
        );

        bridge.abort();
    }

    /// A resync (a shrinking `tools/list`) must dispose the server's previous
    /// scope and register the current list into a fresh one, so a tool the
    /// server dropped leaves no dangling handler behind.
    #[tokio::test]
    async fn sync_disposes_previous_scope_so_a_removed_tool_leaves_the_registry() {
        let registry = ToolHandlerRegistry::new();
        let manager = fake_manager(vec![server_info("srv")]);
        let mut scopes: HashMap<String, ToolRegistrationScope> = HashMap::new();

        // Seed a scope owning a tool the server will no longer advertise.
        let mut previous = ToolRegistrationScope::new("mcp:server:srv");
        register_mcp_tools(
            &registry,
            None,
            Arc::new(crate::mcp::McpClient::new()),
            "srv",
            &[mcp_tool("old")],
            None,
            &mut previous,
        )
        .await;
        assert!(registry.snapshot().contains_key("srv__old"));
        scopes.insert("srv".to_string(), previous);

        // The fake client lists no tools, so the sync disposes the old scope
        // and registers nothing.
        sync_server(&manager.handle, &registry, None, &mut scopes, "srv").await;

        assert!(
            !registry.snapshot().contains_key("srv__old"),
            "a tool the server dropped must not leak after a resync"
        );
        assert!(
            scopes.contains_key("srv"),
            "the server keeps a fresh (empty) owning scope"
        );
    }

    /// A departure event disposes exactly the departing server's scope; the
    /// other servers' registrations are untouched.
    #[tokio::test]
    async fn departure_event_disposes_only_the_departing_servers_scope() {
        let registry = ToolHandlerRegistry::new();
        let client = Arc::new(crate::mcp::McpClient::new());
        let mut scopes: HashMap<String, ToolRegistrationScope> = HashMap::new();
        for (id, tool) in [("alpha", "a"), ("beta", "b")] {
            let mut scope = ToolRegistrationScope::new(format!("mcp:server:{id}"));
            register_mcp_tools(
                &registry,
                None,
                Arc::clone(&client),
                id,
                &[mcp_tool(tool)],
                None,
                &mut scope,
            )
            .await;
            scopes.insert(id.to_string(), scope);
        }
        assert!(registry.snapshot().contains_key("alpha__a"));
        assert!(registry.snapshot().contains_key("beta__b"));

        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let (event_tx, _) = tokio::sync::broadcast::channel(8);
        let handle = McpManagerHandle::new(tx, event_tx);
        apply_event(
            &handle,
            &registry,
            None,
            &mut scopes,
            McpManagerEvent::ServerStopped {
                server_id: "alpha".to_string(),
                server_name: "alpha".to_string(),
            },
        )
        .await;

        assert!(
            !registry.snapshot().contains_key("alpha__a"),
            "the departed server's tools are gone"
        );
        assert!(
            registry.snapshot().contains_key("beta__b"),
            "the other server's tools are untouched"
        );
        assert!(!scopes.contains_key("alpha"));
        assert!(scopes.contains_key("beta"));
    }

    /// The generation-guarded handles a scope owns must never delete a
    /// replacement: disposing a superseded server scope after a newer
    /// registration took the same qualified name is a no-op.
    #[tokio::test]
    async fn stale_server_scope_dispose_does_not_remove_a_replacement() {
        let registry = ToolHandlerRegistry::new();
        let client = Arc::new(crate::mcp::McpClient::new());
        let mut stale = ToolRegistrationScope::new("mcp:server:srv");
        register_mcp_tools(
            &registry,
            None,
            Arc::clone(&client),
            "srv",
            &[mcp_tool("t")],
            None,
            &mut stale,
        )
        .await;

        // A newer registration takes over the same qualified name.
        let replacement: Arc<dyn ToolHandler> = Arc::new(McpHandler::new(
            Arc::clone(&client),
            "srv".to_string(),
            "t".to_string(),
            "replacement".to_string(),
            serde_json::json!({"type": "object"}),
        ));
        let descriptor = ToolCapabilityDescriptor::from_definition(&replacement.definition(), 0);
        registry
            .replace(descriptor, Arc::clone(&replacement))
            .expect("replace succeeds");

        assert!(stale.dispose().await.all_ok());
        let live = registry.resolve("srv__t");
        assert!(
            live.is_some(),
            "a superseded scope must not remove the replacement"
        );
        assert!(Arc::ptr_eq(&live.unwrap(), &replacement));
    }
}
