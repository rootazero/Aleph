//! MCP Registrar — plugin-owned transient servers as a disposable effect, and
//! the per-agent MCP server scope (P3 Stage I).
//!
//! Historical note: this file used to host a `McpRegistrar` struct for a
//! two-phase `batch_register` write path, then nothing plugin-shaped at all
//! while a boot-time sync on `ExtensionManager` did the registration inline
//! from the loader's `.mcp.json` mirror. [`register_transient_servers`] is the plugin path now: it is the
//! `mcp_server` effect `lifecycle.rs::mount` records, and its disposer is what
//! `unmount` runs. `McpScope` below is the per-agent (sub-agent) scope and is
//! unrelated to plugin mounting.

// -- P3 Stage I — per-agent MCP scope ----------------------------------------

use crate::extension::registry::PluginRegistry;

/// Errors raised while provisioning or tearing down an [`McpScope`].
///
/// All variants are fail-loud: `subagent_spawner::spawn` maps any
/// `McpScopeError` to `"sub-agent failed: mcp scope: {err}"` and returns
/// `Err` (no fallback to global-only behavior).
#[derive(Debug, thiserror::Error)]
pub enum McpScopeError {
    #[error("name '{0}' is reserved by global registry; inline servers must use a fresh name")]
    NameConflict(String),
    #[error("reference '{0}' not found in global registry")]
    ReferenceNotFound(String),
    #[error("inline server '{name}' failed to start: {reason}")]
    InlineStartup { name: String, reason: String },
    #[error("inline server '{name}' failed to shut down: {reason}")]
    InlineShutdown { name: String, reason: String },
}

use crate::sync_primitives::{Arc, AtomicBool, Ordering};

/// RAII handle for a single inline MCP server process spawned for one
/// subagent's lifetime (P3 Stage I).
///
/// Production callers should construct via `McpScope::provision`; the
/// `new_for_test` constructor is `pub(crate)` and exists only for unit
/// tests of the Drop safety-net wiring (Task 5). The real `process`
/// field is an `Option<crate::mcp::external::McpServerConnection>` — see
/// Task 7 for the full implementation.
pub struct InlineMcpHandle {
    pub(crate) name: String,
    /// `None` in `new_for_test`; `Some(_)` after a successful spawn.
    pub(crate) process: Option<crate::mcp::external::McpServerConnection>,
    pub(crate) cleaned_up: Arc<AtomicBool>,
}

impl InlineMcpHandle {
    #[cfg(test)]
    pub(crate) fn new_for_test(name: String) -> Self {
        Self {
            name,
            process: None,
            cleaned_up: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Mark the handle as already cleaned up so `Drop` skips the safety-net.
    /// Called by `McpScope::shutdown` on the explicit-cleanup path.
    pub(crate) fn mark_cleaned(&self) {
        self.cleaned_up.store(true, Ordering::Release);
    }
}

impl std::fmt::Debug for InlineMcpHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InlineMcpHandle")
            .field("name", &self.name)
            .field("process", &self.process.is_some())
            .field("cleaned_up", &self.cleaned_up)
            .finish()
    }
}

impl Drop for InlineMcpHandle {
    fn drop(&mut self) {
        if self.cleaned_up.load(Ordering::Acquire) {
            return;
        }
        // Safety net: process leaked through cancel/panic/timeout. Log via
        // tracing; do NOT panic from Drop.
        tracing::error!(
            name = %self.name,
            "InlineMcpHandle leaked — Drop safety-net firing"
        );
        if let Some(proc) = self.process.take() {
            let name = self.name.clone();
            // Sync OS thread + ad-hoc tokio runtime — Drop has no async context.
            std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build();
                match rt {
                    Ok(rt) => {
                        if let Err(e) = rt.block_on(proc.close()) {
                            tracing::error!(
                                name = %name,
                                error = %e,
                                "inline MCP shutdown via Drop safety-net failed"
                            );
                        }
                    }
                    Err(e) => tracing::error!(
                        name = %name,
                        error = %e,
                        "failed to build runtime in Drop safety-net"
                    ),
                }
            });
        }
    }
}

use crate::agents::{AgentDef, McpServerSpec};
use crate::harness::trace::LoopTraceEvent;
use crate::harness::TraceSink;
use std::collections::HashSet;

/// Per-agent MCP server scope (P3 Stage I).
///
/// Composed of:
/// - `references`: names whitelisted from the global registry (read-only view).
/// - `inline_handles`: fresh process handles owned by this single subagent.
pub struct McpScope {
    pub(crate) references: HashSet<String>,
    pub(crate) inline_handles: Vec<InlineMcpHandle>,
    pub(crate) trace_sink: Option<Arc<dyn TraceSink>>,
    pub(crate) agent_id: String,
    /// P3 Stage I — referenced global tools, snapshotted at provision time.
    /// The subagent's tool surface is fixed at spawn (it must not drift
    /// mid-run), so the referenced tools are captured under a single read guard
    /// during `provision`; the scope holds no live-registry handle afterward
    /// (P5 — least knowledge).
    pub(crate) tools: Vec<crate::extension::registry::ToolRegistration>,
    /// P3 Stage I follow-up — inline-server tools, snapshotted from each
    /// spawned process's `tools/list` at provision time. Names are
    /// namespaced as `<server_name>:<tool_name>` (matches the global plugin
    /// convention used by `PluginRegistry`) and `plugin_id` is set to
    /// `inline:<server_name>` so the spawner can disambiguate from global
    /// plugin-owned tools when routing a call.
    pub(crate) inline_tools: Vec<crate::extension::registry::ToolRegistration>,
}

impl std::fmt::Debug for McpScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpScope")
            .field("agent_id", &self.agent_id)
            .field("references", &self.references)
            .field("inline_handles", &self.inline_handles)
            .field(
                "trace_sink",
                &self.trace_sink.as_ref().map(|_| "<dyn TraceSink>"),
            )
            .finish()
    }
}

impl McpScope {
    /// Build scope from agent def. Validates inline-name collisions against
    /// `global` BEFORE starting any process; then starts inline servers
    /// eagerly + in parallel via `futures::future::try_join_all`.
    ///
    /// Each successfully-spawned inline server has its `tools/list` queried
    /// and the entries are converted to [`ToolRegistration`] (named
    /// `<server>:<tool>`, `plugin_id = "inline:<server>"`); the snapshot is
    /// stored on the scope so [`Self::tools`] can surface them without an
    /// async re-query. A server that fails to list (e.g. an old MCP server
    /// that doesn't implement `tools/list`) does NOT abort the spawn — the
    /// agent still runs with whatever tools it can see, and the missing
    /// surface is logged.
    pub async fn provision(
        agent_def: &AgentDef,
        registry: Arc<tokio::sync::RwLock<PluginRegistry>>,
        trace_sink: Option<Arc<dyn TraceSink>>,
    ) -> Result<Self, McpScopeError> {
        let mut references: HashSet<String> = HashSet::new();
        let mut inline_specs: Vec<(String, crate::agents::McpInlineConfig)> = Vec::new();
        let mut tools: Vec<crate::extension::registry::ToolRegistration> = Vec::new();

        // Phase 1: classify specs + validate collisions BEFORE spawning
        // anything, and snapshot referenced tools — all under a single read
        // guard so validation and the tool snapshot observe one consistent
        // view. The guard is scoped to drop before Phase 2 so the registry lock
        // is never held across the inline-spawn await points.
        {
            let reg = registry.read().await;
            for spec in &agent_def.mcp_servers {
                match spec {
                    McpServerSpec::Reference { name } => {
                        if !reg
                            .get_plugin(name)
                            .is_some_and(|plugin| plugin.status.is_active())
                        {
                            return Err(McpScopeError::ReferenceNotFound(name.clone()));
                        }
                        for tool in reg.list_tools_for_plugin(name) {
                            tools.push(tool.clone());
                        }
                        references.insert(name.clone());
                    }
                    McpServerSpec::Inline { name, config } => {
                        if reg.get_plugin(name).is_some() {
                            return Err(McpScopeError::NameConflict(name.clone()));
                        }
                        inline_specs.push((name.clone(), config.clone()));
                    }
                }
            }
        }

        // Phase 2: spawn all inline servers eagerly in parallel.
        let spawn_futures = inline_specs
            .into_iter()
            .map(|(name, config)| async move { spawn_inline(name, config).await });
        let inline_handles: Vec<InlineMcpHandle> =
            futures::future::try_join_all(spawn_futures).await?;

        // Phase 3: snapshot each inline server's tool surface. We query AFTER
        // every spawn has succeeded so a single broken server doesn't cancel
        // a healthy peer. `McpServerConnection::list_tools` reads from the
        // connection's pre-populated cache (filled during the handshake's
        // initial `tools/list` exchange), so it cannot fail in the I/O sense
        // — an empty list just means the server advertises no tools.
        let mut inline_tools: Vec<crate::extension::registry::ToolRegistration> = Vec::new();
        for handle in &inline_handles {
            let Some(proc) = handle.process.as_ref() else {
                continue;
            };
            let server_tools = proc.list_tools().await;
            if server_tools.is_empty() {
                tracing::debug!(
                    agent_id = %agent_def.id,
                    inline_server = %handle.name,
                    "McpScope: inline server advertises no tools"
                );
            }
            for mcp_tool in server_tools {
                inline_tools.push(crate::extension::registry::ToolRegistration {
                    name: format!("{}:{}", handle.name, mcp_tool.name),
                    description: mcp_tool.description,
                    parameters: mcp_tool.input_schema,
                    // Inline-server tools are dispatched via the McpClient
                    // that owns the spawned process. The handler string is
                    // opaque to the spawner — see the agent_spawner wiring
                    // which routes `inline:<server>:<tool>` calls back to the
                    // matching `InlineMcpHandle`.
                    handler: handle.name.clone(),
                    plugin_id: format!("inline:{}", handle.name),
                });
            }
        }

        let scope = Self {
            references,
            inline_handles,
            trace_sink,
            agent_id: agent_def.id.clone(),
            tools,
            inline_tools,
        };

        if let Some(sink) = scope.trace_sink.as_ref() {
            sink.on_trace(&LoopTraceEvent::McpScopeAttached {
                agent_id: scope.agent_id.clone(),
                references: scope.references.iter().cloned().collect(),
                inline_count: scope.inline_handles.len(),
            });
        }

        Ok(scope)
    }

    /// Tools visible to the child harness:
    /// - All tools from the global registry whose plugin name is in `references`.
    /// - All tools discovered on inline servers at provision time, namespaced
    ///   as `<server>:<tool>` with `plugin_id = "inline:<server>"`.
    ///
    /// Result is layered UNDER `AllowlistToolService` by the spawner.
    #[must_use]
    pub fn tools(&self) -> Vec<crate::extension::registry::ToolRegistration> {
        // Referenced global tools were snapshotted under a read guard at
        // provision time; inline tools were snapshotted by calling
        // `list_tools()` on each spawned `InlineMcpHandle` immediately after
        // Phase 2 succeeded. The two lists are disjoint by construction
        // (inline tools carry `plugin_id = "inline:..."`, referenced tools
        // carry the global plugin id) so a plain concat is correct.
        let mut all = Vec::with_capacity(self.tools.len() + self.inline_tools.len());
        all.extend(self.tools.iter().cloned());
        all.extend(self.inline_tools.iter().cloned());
        all
    }

    /// Explicit shutdown. Calls `proc.close()` on each inline handle and marks
    /// successful closes as cleaned. First failure surfaces as `InlineShutdown`;
    /// failed handles retain the Drop safety-net.
    pub async fn shutdown(self) -> Result<(), McpScopeError> {
        let agent_id = self.agent_id.clone();
        let trace_sink = self.trace_sink.clone();
        let mut shutdown_errors: Vec<(String, String)> = Vec::new();

        for h in &self.inline_handles {
            if let Some(proc) = h.process.as_ref() {
                if let Err(e) = proc.close().await {
                    shutdown_errors.push((h.name.clone(), e.to_string()));
                    continue;
                }
            }
            h.mark_cleaned();
        }

        if let Some(sink) = trace_sink.as_ref() {
            sink.on_trace(&LoopTraceEvent::McpScopeCleaned {
                agent_id: agent_id.clone(),
                leaked: false,
            });
        }

        if let Some((name, reason)) = shutdown_errors.into_iter().next() {
            return Err(McpScopeError::InlineShutdown { name, reason });
        }
        Ok(())
    }
}

impl Drop for McpScope {
    fn drop(&mut self) {
        let any_leaked = self
            .inline_handles
            .iter()
            .any(|h| !h.cleaned_up.load(Ordering::Acquire));
        if !any_leaked {
            return;
        }
        if let Some(sink) = self.trace_sink.as_ref() {
            sink.on_trace(&LoopTraceEvent::McpScopeCleaned {
                agent_id: self.agent_id.clone(),
                leaked: true,
            });
        }
        tracing::error!(
            agent_id = %self.agent_id,
            leaked_handles = self.inline_handles.iter().filter(|h| !h.cleaned_up.load(Ordering::Acquire)).count(),
            "McpScope leaked — relying on InlineMcpHandle Drops for kill"
        );
    }
}

/// Spawn a single inline MCP server via `McpServerConnection::connect`.
async fn spawn_inline(
    name: String,
    config: crate::agents::McpInlineConfig,
) -> Result<InlineMcpHandle, McpScopeError> {
    let connection = crate::mcp::external::McpServerConnection::connect(
        name.clone(),
        &config.command,
        &config.args,
        &config.env,
        None,
        None,
        // Agent-scoped inline servers run outside `McpClient` and so have no
        // sampling handler to hand them. Passing `None` keeps the declared
        // capabilities honest: the server is never asked for something this
        // connection could only answer with an error.
        None,
    )
    .await
    .map_err(|e| McpScopeError::InlineStartup {
        name: name.clone(),
        reason: e.to_string(),
    })?;

    Ok(InlineMcpHandle {
        name,
        process: Some(connection),
        cleaned_up: Arc::new(AtomicBool::new(false)),
    })
}

use crate::extension::effects::{async_disposer, Disposer};
use crate::mcp::{McpManagerConfig, McpManagerHandle};
use std::collections::HashMap;
use tokio::sync::oneshot;

/// The actor's answer to one enqueued transient-server start.
pub type ServerStartReceiver = oneshot::Receiver<Result<(), String>>;

/// Mount-time effect: hand every server a plugin's `.mcp.json` declares to
/// the MCP manager as a **transient** server, and return the disposer that
/// removes them all plus one receiver per server for the actor's verdict.
///
/// Each add is *enqueued*, not awaited (see
/// `McpManagerHandle::add_transient_server_detached`). The receivers are
/// returned to the caller — `lifecycle.rs::watch_server_starts` awaits them
/// on one task per plugin and logs each outcome; P3 turns that watcher into
/// the readiness (`Pending`) writer. A failed start is therefore NOT a mount
/// failure — the same warn-and-continue contract the boot-time sync it
/// replaced had — but a closed command channel is, because then nothing can
/// ever start.
///
/// Partial failure is all-or-none at this step's granularity: if the k-th
/// enqueue fails, the k−1 already enqueued are removed before returning
/// `Err`, so the caller never has to dispose a half-registered step.
pub(crate) async fn register_transient_servers(
    handle: McpManagerHandle,
    configs: HashMap<String, McpManagerConfig>,
) -> Result<(Disposer, Vec<(String, ServerStartReceiver)>), String> {
    let mut server_ids: Vec<String> = configs.keys().cloned().collect();
    server_ids.sort();
    let mut enqueued: Vec<String> = Vec::with_capacity(server_ids.len());
    let mut receivers: Vec<(String, ServerStartReceiver)> = Vec::with_capacity(server_ids.len());
    for server_id in &server_ids {
        let config = configs
            .get(server_id)
            .cloned()
            .expect("id came from this map");
        match handle.add_transient_server_detached(config).await {
            Ok(rx) => {
                enqueued.push(server_id.clone());
                receivers.push((server_id.clone(), rx));
            }
            Err(e) => {
                for already in &enqueued {
                    if let Err(re) = handle.remove_transient_server(already.clone()).await {
                        tracing::warn!(server_id = %already, error = %re, "rollback remove failed");
                    }
                }
                return Err(format!("cannot enqueue MCP server '{server_id}': {e}"));
            }
        }
    }
    let disposer = async_disposer(move || async move {
        let mut failures = Vec::new();
        for server_id in enqueued {
            if let Err(e) = handle.remove_transient_server(server_id.clone()).await {
                failures.push(format!("{server_id}: {e}"));
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "remove_transient_server failed for {}",
                failures.join("; ")
            ))
        }
    });
    Ok((disposer, receivers))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::registry::ToolRegistration;
    use crate::extension::types::{PluginKind, PluginOrigin, PluginRecord};

    fn make_registry_with_plugin(plugin_id: &str) -> PluginRegistry {
        let mut registry = PluginRegistry::new();
        let record = PluginRecord::new(
            plugin_id.to_string(),
            plugin_id.to_string(),
            PluginKind::Mcp,
            PluginOrigin::Global,
        );
        registry.register_plugin(record);
        registry
    }

    // The four `test_batch_register_*` tests that stood here exercised
    // `McpRegistrar::batch_register`, removed with the struct itself (see the
    // module header). They were left behind, and `cargo check` does not compile
    // `#[cfg(test)]` code, so the lib test target simply stopped building and
    // nothing said so.
    #[test]
    fn mcp_scope_error_displays_name_conflict() {
        let e = McpScopeError::NameConflict("github".into());
        let s = format!("{e}");
        assert!(s.contains("name 'github'"));
        assert!(s.contains("global registry"));
    }

    #[test]
    fn mcp_scope_error_displays_reference_not_found() {
        let e = McpScopeError::ReferenceNotFound("missing".into());
        assert!(format!("{e}").contains("reference 'missing' not found"));
    }

    #[test]
    fn mcp_scope_error_displays_inline_startup() {
        let e = McpScopeError::InlineStartup {
            name: "fresh".into(),
            reason: "exec failed: ENOENT".into(),
        };
        let s = format!("{e}");
        assert!(s.contains("inline server 'fresh'"));
        assert!(s.contains("ENOENT"));
    }

    #[test]
    fn mcp_scope_error_displays_inline_shutdown() {
        let e = McpScopeError::InlineShutdown {
            name: "fresh".into(),
            reason: "kill -TERM timed out".into(),
        };
        assert!(format!("{e}").contains("failed to shut down"));
    }

    #[test]
    fn inline_mcp_handle_drop_without_cleanup_logs_leak() {
        use crate::sync_primitives::Ordering;
        let handle = InlineMcpHandle::new_for_test("zombie".into());
        let cleaned = handle.cleaned_up.clone();
        drop(handle);
        assert!(
            !cleaned.load(Ordering::Acquire),
            "no explicit cleanup → flag stays false"
        );
    }

    #[test]
    fn inline_mcp_handle_mark_cleaned_skips_drop_safety_net() {
        use crate::sync_primitives::Ordering;
        let handle = InlineMcpHandle::new_for_test("clean".into());
        let cleaned = handle.cleaned_up.clone();
        handle.mark_cleaned();
        drop(handle);
        assert!(
            cleaned.load(Ordering::Acquire),
            "explicit cleanup must flip the flag"
        );
    }

    #[tokio::test]
    async fn mcp_scope_provision_reference_resolves_from_global() {
        use crate::agents::{AgentDef, AgentMode, McpServerSpec};
        use crate::sync_primitives::Arc;

        let mut registry = make_registry_with_plugin("global-mcp");
        let tool = ToolRegistration {
            name: "global-tool".into(),
            description: "from global mcp".into(),
            parameters: serde_json::json!({}),
            handler: "global_handler".into(),
            plugin_id: "global-mcp".into(),
        };
        registry.register_tool(tool);
        let global = Arc::new(tokio::sync::RwLock::new(registry));

        let agent = AgentDef::new("test", AgentMode::SubAgent).with_mcp_servers(vec![
            McpServerSpec::Reference {
                name: "global-mcp".into(),
            },
        ]);

        let scope = McpScope::provision(&agent, global, None)
            .await
            .expect("provision succeeds");
        assert_eq!(scope.references.len(), 1);
        assert!(scope.references.contains("global-mcp"));
        assert_eq!(scope.inline_handles.len(), 0);
        scope.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn mcp_scope_provision_reference_not_found_fails_loud() {
        use crate::agents::{AgentDef, AgentMode, McpServerSpec};
        use crate::sync_primitives::Arc;

        let registry = make_registry_with_plugin("only-this");
        let global = Arc::new(tokio::sync::RwLock::new(registry));
        let agent = AgentDef::new("test", AgentMode::SubAgent).with_mcp_servers(vec![
            McpServerSpec::Reference {
                name: "missing".into(),
            },
        ]);

        let err = McpScope::provision(&agent, global, None)
            .await
            .expect_err("should fail");
        assert!(matches!(err, McpScopeError::ReferenceNotFound(ref n) if n == "missing"));
    }

    #[tokio::test]
    async fn mcp_scope_provision_inline_name_conflict_at_spawn_time() {
        use crate::agents::{AgentDef, AgentMode, McpInlineConfig, McpServerSpec};
        use crate::sync_primitives::Arc;

        let registry = make_registry_with_plugin("github");
        let global = Arc::new(tokio::sync::RwLock::new(registry));

        let agent = AgentDef::new("test", AgentMode::SubAgent).with_mcp_servers(vec![
            McpServerSpec::Inline {
                name: "github".into(),
                config: McpInlineConfig {
                    command: "node".into(),
                    args: vec!["server.js".into()],
                    env: Default::default(),
                },
            },
        ]);

        let err = McpScope::provision(&agent, global, None)
            .await
            .expect_err("name conflict must fail at spawn time");
        assert!(matches!(err, McpScopeError::NameConflict(ref n) if n == "github"));
    }

    #[tokio::test]
    async fn mcp_scope_tools_includes_referenced_global_tools() {
        use crate::agents::{AgentDef, AgentMode, McpServerSpec};
        use crate::sync_primitives::Arc;

        let mut registry = make_registry_with_plugin("global-mcp");
        registry.register_tool(ToolRegistration {
            name: "global-tool".into(),
            description: "from global".into(),
            parameters: serde_json::json!({}),
            handler: "h".into(),
            plugin_id: "global-mcp".into(),
        });
        let global = Arc::new(tokio::sync::RwLock::new(registry));

        let agent = AgentDef::new("test", AgentMode::SubAgent).with_mcp_servers(vec![
            McpServerSpec::Reference {
                name: "global-mcp".into(),
            },
        ]);
        let scope = McpScope::provision(&agent, global, None)
            .await
            .expect("provision");

        let tools = scope.tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(
            names.contains(&"global-tool"),
            "tools() must include the referenced tool: {names:?}"
        );

        scope.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn mcp_scope_provision_inline_failed_start_returns_inline_startup() {
        use crate::agents::{AgentDef, AgentMode, McpInlineConfig, McpServerSpec};
        use crate::sync_primitives::Arc;

        let registry = PluginRegistry::new();
        let global = Arc::new(tokio::sync::RwLock::new(registry));

        let agent = AgentDef::new("test", AgentMode::SubAgent).with_mcp_servers(vec![
            McpServerSpec::Inline {
                name: "broken".into(),
                config: McpInlineConfig {
                    command: "/definitely/not/a/real/binary/aleph-stage-i".into(),
                    args: vec![],
                    env: Default::default(),
                },
            },
        ]);

        let err = McpScope::provision(&agent, global, None)
            .await
            .expect_err("nonexistent binary must fail to start");
        assert!(
            matches!(err, McpScopeError::InlineStartup { ref name, .. } if name == "broken"),
            "got {err:?}"
        );
    }

    /// Drives a real actor. The server command does not exist, so the add
    /// FAILS inside the actor — that is the point: the producer must return
    /// before the actor answers, the failure must be observable on the
    /// receiver, and the disposer must still send the remove (a no-op for a
    /// server that never started, `actor.rs:624-641`).
    #[tokio::test]
    async fn register_transient_servers_enqueues_without_waiting_and_disposer_removes() {
        use crate::mcp::manager::{McpManagerActor, McpManagerConfig, McpManagerEvent};
        let dir = tempfile::tempdir().unwrap();
        let (actor, handle) = McpManagerActor::new(Some(dir.path().join("mcp.json")))
            .await
            .unwrap();
        tokio::spawn(actor.run());
        let mut events = handle.subscribe();

        let mut configs = std::collections::HashMap::new();
        configs.insert(
            "plugin:qa/never".to_string(),
            McpManagerConfig::stdio(
                "plugin:qa/never",
                "never (qa)",
                "qa-nonexistent-mcp-binary-9f3a",
            ),
        );

        let started = std::time::Instant::now();
        let (disposer, receivers) = register_transient_servers(handle.clone(), configs)
            .await
            .expect("enqueue succeeds while the actor is alive");
        assert!(
            started.elapsed() < std::time::Duration::from_millis(500),
            "producer must not wait for the handshake ({:?})",
            started.elapsed()
        );
        assert_eq!(receivers.len(), 1);
        assert_eq!(
            receivers[0].0, "plugin:qa/never",
            "one receiver per server, keyed by id"
        );

        // The disposer is a remove for the same id; it must be accepted even
        // though the add is still in flight / has failed.
        disposer()
            .await
            .expect("remove_transient_server is a no-op for an unknown id");

        // The actor's verdict arrives on the receiver the caller was handed.
        let (_, rx) = receivers.into_iter().next().unwrap();
        let verdict = tokio::time::timeout(std::time::Duration::from_secs(30), rx)
            .await
            .expect("actor answers")
            .expect("sender not dropped");
        assert!(
            verdict.is_err(),
            "a nonexistent binary fails to start: {verdict:?}"
        );

        // No ServerStarted for a binary that does not exist; and nothing panicked.
        let got_started = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                match events.recv().await {
                    Ok(McpManagerEvent::ServerStarted { server_id, .. })
                        if server_id == "plugin:qa/never" =>
                    {
                        break true
                    }
                    Ok(_) => continue,
                    Err(_) => break false,
                }
            }
        })
        .await
        .unwrap_or(false);
        assert!(!got_started, "a nonexistent binary cannot have started");
    }

    #[tokio::test]
    async fn register_transient_servers_fails_when_the_actor_is_gone() {
        use crate::mcp::manager::{McpManagerActor, McpManagerConfig};
        let dir = tempfile::tempdir().unwrap();
        let (actor, handle) = McpManagerActor::new(Some(dir.path().join("mcp.json")))
            .await
            .unwrap();
        drop(actor); // never run: the command channel's receiver is dropped
        let mut configs = std::collections::HashMap::new();
        configs.insert(
            "plugin:qa/x".to_string(),
            McpManagerConfig::stdio("plugin:qa/x", "x", "true"),
        );
        let err = register_transient_servers(handle, configs)
            .await
            .err()
            .expect("a closed channel is a step failure, not a silent skip");
        assert!(err.contains("plugin:qa/x"), "names the server: {err}");
    }
}
