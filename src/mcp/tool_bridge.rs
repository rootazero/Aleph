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
//! (`mcp_list_resources`, `mcp_list_prompts`): each pair is present in the
//! registry only while at least one connected server advertises that
//! capability, so the agent is never offered a tool that every call would
//! reject, and the discovery twin is always alongside the reader so the model
//! can enumerate URIs/names instead of guessing them (or falling back to `cat`).

use crate::sync_primitives::Arc;

use tokio::sync::broadcast::error::RecvError;
use tokio::task::JoinHandle;

use crate::builtin_tools::mcp_login::McpLoginTool;
use crate::builtin_tools::mcp_prompt::{McpGetPromptTool, McpListPromptsTool};
use crate::builtin_tools::mcp_resource::{
    McpListResourceTemplatesTool, McpListResourcesTool, McpReadResourceTool,
};
use crate::mcp::manager::{McpManagerEvent, McpManagerHandle, McpTransportType};
use crate::tool_metadata::ToolCatalog;
use crate::tools::handlers::builtin::BuiltinHandler;
use crate::tools::handlers::registration::{register_mcp_tools, unregister_mcp_tools};
use crate::tools::handlers::ToolHandler;
use crate::tools::registry::ToolHandlerRegistry;
use crate::tools::AlephToolDyn;

/// Registry name of the capability-gated resource-reading builtin.
const RESOURCE_TOOL: &str = "mcp_read_resource";
/// Registry name of the capability-gated resource-discovery builtin. Gated by
/// the same condition as [`RESOURCE_TOOL`]: without it the model can read a
/// resource but has no way to learn which resources exist (the gap that pushed
/// it to a raw `cat`).
const RESOURCE_LIST_TOOL: &str = "mcp_list_resources";
/// Registry name of the capability-gated resource-*template* discovery builtin.
/// Gated with the resource cluster: a server may expose resources ONLY by
/// template (concrete `resource_count == 0`), so the gate also fires on
/// `resource_template_count > 0` — otherwise a template-only server would strand
/// the model with no discoverable, readable handle (another `cat` fallback).
const RESOURCE_TEMPLATE_LIST_TOOL: &str = "mcp_list_resource_templates";
/// Registry name of the capability-gated prompt-fetching builtin.
const PROMPT_TOOL: &str = "mcp_get_prompt";
/// Registry name of the capability-gated prompt-discovery builtin. Gated by the
/// same condition as [`PROMPT_TOOL`].
const PROMPT_LIST_TOOL: &str = "mcp_list_prompts";
/// Registry name of the OAuth login builtin (present only with remote servers).
const LOGIN_TOOL: &str = "mcp_login";

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
        // Whether each capability builtin is currently in the registry.
        let mut resource_live = false;
        let mut prompt_live = false;
        let mut login_live = false;
        let catalog = tool_catalog.as_ref();
        resync_all(&handle, &registry, catalog).await;
        reconcile_capability_tools(
            &handle,
            &registry,
            &mut resource_live,
            &mut prompt_live,
            &mut login_live,
        )
        .await;
        loop {
            match events.recv().await {
                Ok(event) => {
                    apply_event(&handle, &registry, catalog, event).await;
                    reconcile_capability_tools(
                        &handle,
                        &registry,
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
                    resync_all(&handle, &registry, catalog).await;
                    reconcile_capability_tools(
                        &handle,
                        &registry,
                        &mut resource_live,
                        &mut prompt_live,
                        &mut login_live,
                    )
                    .await;
                }
                Err(RecvError::Closed) => {
                    tracing::info!("MCP manager event channel closed; tool bridge exiting");
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
    event: McpManagerEvent,
) {
    match event {
        McpManagerEvent::ServerStarted { server_id, .. }
        | McpManagerEvent::ToolsChanged { server_id, .. } => {
            sync_server(handle, registry, tool_catalog, &server_id).await;
        }
        McpManagerEvent::ServerStopped { server_id, .. }
        | McpManagerEvent::ServerCrashed { server_id, .. }
        | McpManagerEvent::ServerRemoved { server_id, .. } => {
            let removed = unregister_mcp_tools(registry, tool_catalog, &server_id).await;
            if !removed.is_empty() {
                tracing::info!(
                    server_id = %server_id,
                    count = removed.len(),
                    "MCP tool bridge: unregistered tools for departed server"
                );
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
    let _ = unregister_mcp_tools(registry, tool_catalog, server_id).await;
    let registered = register_mcp_tools(
        registry,
        tool_catalog,
        client,
        server_id,
        &tools,
        timeout_seconds,
    )
    .await;
    tracing::info!(
        server_id,
        count = registered.len(),
        "MCP tool bridge: synced server tools into registry"
    );
}

/// Reconcile every known server against the registry. Used after a broadcast
/// lag, where individual transitions may have been dropped.
async fn resync_all(
    handle: &McpManagerHandle,
    registry: &ToolHandlerRegistry,
    tool_catalog: Option<&Arc<ToolCatalog>>,
) {
    match handle.list_servers().await {
        Ok(servers) => {
            for info in servers {
                sync_server(handle, registry, tool_catalog, &info.id).await;
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "MCP tool bridge: resync_all list_servers failed");
        }
    }
}

/// Register or unregister the capability builtins so each is present only
/// while at least one connected server advertises that capability.
async fn reconcile_capability_tools(
    handle: &McpManagerHandle,
    registry: &ToolHandlerRegistry,
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
        // The read + discovery tools share the resource capability gate: offering
        // one without the other either strands the model (read with no way to
        // discover) or dangles a discovery tool over nothing. The template-list
        // tool rides the same gate (see `RESOURCE_TEMPLATE_LIST_TOOL`).
        // rust-doctor-disable-next-line excessive-clone
        let read: Arc<dyn AlephToolDyn> = Arc::new(McpReadResourceTool::new(handle.clone()));
        // rust-doctor-disable-next-line excessive-clone
        let list: Arc<dyn AlephToolDyn> = Arc::new(McpListResourcesTool::new(handle.clone()));
        // rust-doctor-disable-next-line excessive-clone
        // rust-doctor-disable-next-line excessive-clone
        let templates: Arc<dyn AlephToolDyn> =
            Arc::new(McpListResourceTemplatesTool::new(handle.clone()));
        set_builtin(registry, RESOURCE_LIST_TOOL, want_resource, list);
        set_builtin(
            registry,
            RESOURCE_TEMPLATE_LIST_TOOL,
            want_resource,
            templates,
        );
        *resource_live = set_builtin(registry, RESOURCE_TOOL, want_resource, read);
    }
    if want_prompt != *prompt_live {
        // rust-doctor-disable-next-line excessive-clone
        let get: Arc<dyn AlephToolDyn> = Arc::new(McpGetPromptTool::new(handle.clone()));
        // rust-doctor-disable-next-line excessive-clone
        let list: Arc<dyn AlephToolDyn> = Arc::new(McpListPromptsTool::new(handle.clone()));
        set_builtin(registry, PROMPT_LIST_TOOL, want_prompt, list);
        *prompt_live = set_builtin(registry, PROMPT_TOOL, want_prompt, get);
    }
    if want_login != *login_live {
        // rust-doctor-disable-next-line excessive-clone
        let tool: Arc<dyn AlephToolDyn> = Arc::new(McpLoginTool::new(handle.clone()));
        *login_live = set_builtin(registry, LOGIN_TOOL, want_login, tool);
    }
}

/// Add or remove a single builtin from the registry; returns its resulting
/// live state (`true` == registered).
fn set_builtin(
    registry: &ToolHandlerRegistry,
    name: &str,
    want: bool,
    tool: Arc<dyn AlephToolDyn>,
) -> bool {
    if want {
        let handler: Arc<dyn ToolHandler> = Arc::new(BuiltinHandler::new(name.to_string(), tool));
        match registry.register(name.to_string(), handler) {
            Ok(()) => {
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
    } else {
        let _ = registry.unregister(name);
        tracing::info!(tool = name, "MCP tool bridge: capability builtin removed");
        false
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::manager::{HealthStatus, McpCommand, McpServerInfo, McpTransportType};
    use crate::mcp::McpTool;
    use crate::tools::handlers::registration::register_mcp_tools;
    use tokio::sync::{broadcast, mpsc};

    fn server_info(id: &str) -> McpServerInfo {
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

    /// A manager that is already up with `servers` connected, answering only
    /// what the bridge asks. It never emits an event — which is the whole point:
    /// the real one emits them during `run()`, before any bridge exists.
    fn fake_manager(servers: Vec<McpServerInfo>) -> (McpManagerHandle, JoinHandle<()>) {
        let (tx, mut rx) = mpsc::channel::<McpCommand>(32);
        let (event_tx, _) = broadcast::channel(32);
        let handle = McpManagerHandle::new(tx, event_tx);
        let task = tokio::spawn(async move {
            while let Some(cmd) = rx.recv().await {
                match cmd {
                    McpCommand::ListServers { respond_to } => {
                        let _ = respond_to.send(servers.clone());
                    }
                    McpCommand::GetClient { respond_to, .. } => {
                        // A client with no transport: `list_tools()` is empty,
                        // so a sync registers nothing and only the stale-entry
                        // sweep is observable.
                        let _ = respond_to.send(Some(Arc::new(crate::mcp::McpClient::new())));
                    }
                    McpCommand::ListServerConfigs { respond_to } => {
                        let _ = respond_to.send(Vec::new());
                    }
                    _ => {}
                }
            }
        });
        (handle, task)
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
        )
        .await;
        assert!(
            registry.snapshot().contains_key("srv__ghost"),
            "precondition: the registry starts with a stale entry for srv"
        );

        let (handle, manager) = fake_manager(vec![server_info("srv")]);
        let bridge = spawn_tool_bridge(handle, Arc::clone(&registry), None);

        assert!(
            eventually_absent(&registry, "srv__ghost").await,
            "the bridge never reconciled the already-connected server: it is \
             waiting for an event that was emitted before it subscribed"
        );

        bridge.abort();
        manager.abort();
    }
}
