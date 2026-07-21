//! MCP Registrar — collect-then-batch capability registration for MCP plugins
//!
//! MCP registration uses a two-phase pattern:
//! - Phase 1 (async): Probe MCP server for capabilities, collect declarations
//! - Phase 2 (sync): Acquire registry lock, batch-write all declarations
//!
//! This avoids holding `RwLockWriteGuard` across await points.

use crate::extension::capability::CapabilityDeclaration;
use crate::extension::manifest::PluginPermission;
use crate::extension::registrar::api::CapabilityApi;
use crate::extension::registry::PluginRegistry;
use anyhow::Result;

/// Registrar for MCP-based plugins using collect-then-batch pattern.
pub struct McpRegistrar {
    pub plugin_id: String,
    pub permissions: Vec<PluginPermission>,
}

impl McpRegistrar {
    #[must_use]
    pub const fn new(plugin_id: String, permissions: Vec<PluginPermission>) -> Self {
        Self {
            plugin_id,
            permissions,
        }
    }

    /// Phase 2 (sync): Write collected capabilities into registry.
    /// Lock should be held briefly — caller acquires it just before calling this.
    pub fn batch_register(
        &self,
        caps: Vec<CapabilityDeclaration>,
        registry: &mut PluginRegistry,
    ) -> Result<()> {
        let mut api =
            CapabilityApi::new(registry, self.plugin_id.clone(), self.permissions.clone());
        for cap in caps {
            api.register_capability(cap)?;
        }
        Ok(())
    }
}

// -- P3 Stage I — per-agent MCP scope ----------------------------------------

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

        let scope = Self {
            references,
            inline_handles,
            trace_sink,
            agent_id: agent_def.id.clone(),
            tools,
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
    /// - **Inline tool surfacing is deferred to a follow-up** — see concern below.
    ///
    /// Result is layered UNDER `AllowlistToolService` by the spawner.
    #[must_use]
    pub fn tools(&self) -> Vec<crate::extension::registry::ToolRegistration> {
        // Referenced global tools were snapshotted under a read guard at
        // provision time (see the `tools` field). Inline-server tool surfacing
        // still requires async list_tools() + McpTool→ToolRegistration
        // conversion; deferred to a Stage I follow-up.
        self.tools.clone()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::capability::*;
    use crate::extension::registry::{ServiceRegistration, ToolRegistration};
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

    #[test]
    fn test_batch_register_tools() {
        let mut registry = make_registry_with_plugin("test-mcp");

        let registrar = McpRegistrar::new("test-mcp".into(), vec![]);
        let caps = vec![CapabilityDeclaration::Tool(ToolRegistration {
            name: "mcp-tool".into(),
            description: "A tool from MCP".into(),
            parameters: serde_json::json!({}),
            handler: "mcp_handler".into(),
            plugin_id: "test-mcp".into(),
        })];

        registrar.batch_register(caps, &mut registry).unwrap();
        assert!(registry.get_tool("mcp-tool").is_some());
    }

    #[test]
    fn test_batch_register_multiple_tools() {
        let mut registry = make_registry_with_plugin("test-mcp");

        let registrar = McpRegistrar::new("test-mcp".into(), vec![]);
        let caps = vec![
            CapabilityDeclaration::Tool(ToolRegistration {
                name: "tool-a".into(),
                description: "Tool A".into(),
                parameters: serde_json::json!({}),
                handler: "handle_a".into(),
                plugin_id: "test-mcp".into(),
            }),
            CapabilityDeclaration::Tool(ToolRegistration {
                name: "tool-b".into(),
                description: "Tool B".into(),
                parameters: serde_json::json!({}),
                handler: "handle_b".into(),
                plugin_id: "test-mcp".into(),
            }),
        ];

        registrar.batch_register(caps, &mut registry).unwrap();
        assert!(registry.get_tool("tool-a").is_some());
        assert!(registry.get_tool("tool-b").is_some());
    }

    #[test]
    fn test_batch_register_permission_check_service() {
        let mut registry = make_registry_with_plugin("test-mcp");

        // No Background permission → Service registration should fail
        let registrar = McpRegistrar::new("test-mcp".into(), vec![]);
        let caps = vec![CapabilityDeclaration::Service(ServiceRegistration {
            id: "test-service".into(),
            name: "Test Service".into(),
            start_handler: "start".into(),
            stop_handler: "stop".into(),
            plugin_id: "test-mcp".into(),
            auto_start: true,
        })];

        assert!(registrar.batch_register(caps, &mut registry).is_err());
    }

    #[test]
    fn test_batch_register_empty_caps() {
        let mut registry = make_registry_with_plugin("test-mcp");
        let registrar = McpRegistrar::new("test-mcp".into(), vec![]);
        // Empty capability list should succeed with no changes
        assert!(registrar.batch_register(vec![], &mut registry).is_ok());
    }

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
}
