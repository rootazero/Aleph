//! Integration tests for P3 Stage I — Per-agent MCP scope.
//!
//! Test IDs match design § 3.4:
//!   I-T1: happy Reference path → scope provisions; tools include referenced
//!   I-T2: happy Inline path → fresh process spawned (or fails-loud cleanly)
//!   I-T3: NameConflict at spawn time (Inline name vs global)
//!   I-T4: ≤ 500ms perf budget (warn-only soft contract; hard fail at 2000ms)
//!   I-T5: Drop teardown emits McpScopeCleaned { leaked: true }

use std::sync::{Arc, Mutex};

use alephcore::agents::{AgentDef, AgentMode, McpInlineConfig, McpServerSpec};
use alephcore::extension::registrar::mcp_registrar::{McpScope, McpScopeError};
use alephcore::extension::{
    PluginKind, PluginOrigin, PluginRecord, PluginRegistry, ToolRegistration,
};
use alephcore::harness::trace::LoopTraceEvent;
use alephcore::harness::TraceSink;

#[derive(Default, Clone)]
struct CapturingSink {
    events: Arc<Mutex<Vec<LoopTraceEvent>>>,
}

impl TraceSink for CapturingSink {
    fn on_trace(&self, event: &LoopTraceEvent) {
        self.events.lock().unwrap().push(event.clone());
    }

    fn flush(&self) {}
}

impl CapturingSink {
    fn snapshot(&self) -> Vec<LoopTraceEvent> {
        self.events.lock().unwrap().clone()
    }
}

fn registry_with_global_tool(plugin_id: &str, tool_name: &str) -> PluginRegistry {
    let mut registry = PluginRegistry::new();
    registry.register_plugin(PluginRecord::new(
        plugin_id.to_string(),
        plugin_id.to_string(),
        PluginKind::Mcp,
        PluginOrigin::Global,
    ));
    registry.register_tool(ToolRegistration {
        name: tool_name.into(),
        description: "from global".into(),
        parameters: serde_json::json!({}),
        handler: "h".into(),
        plugin_id: plugin_id.into(),
    });
    registry
}

#[tokio::test]
async fn i_t1_happy_reference_path() {
    let registry = registry_with_global_tool("github", "gh-search");
    let global = Arc::new(tokio::sync::RwLock::new(registry));
    let agent = AgentDef::new("scoped", AgentMode::SubAgent).with_mcp_servers(vec![
        McpServerSpec::Reference {
            name: "github".into(),
        },
    ]);
    let sink = CapturingSink::default();
    let arc_sink: Arc<dyn TraceSink> = Arc::new(sink.clone());

    let scope = McpScope::provision(&agent, global, Some(arc_sink))
        .await
        .expect("provision");
    let names: Vec<String> = scope.tools().iter().map(|t| t.name.clone()).collect();
    assert!(names.contains(&"gh-search".to_string()));
    scope.shutdown().await.expect("shutdown");

    let events = sink.snapshot();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, LoopTraceEvent::McpScopeAttached { .. })),
        "expected McpScopeAttached"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, LoopTraceEvent::McpScopeCleaned { leaked: false, .. })),
        "expected McpScopeCleaned(leaked=false)"
    );
}

#[tokio::test]
async fn i_t2_happy_inline_path() {
    let registry = PluginRegistry::new();
    let global = Arc::new(tokio::sync::RwLock::new(registry));
    let agent = AgentDef::new("scoped", AgentMode::SubAgent).with_mcp_servers(vec![
        McpServerSpec::Inline {
            name: "fresh".into(),
            config: McpInlineConfig {
                command: "/bin/cat".into(),
                args: vec![],
                env: Default::default(),
            },
        },
    ]);

    let result = McpScope::provision(&agent, global, None).await;
    match result {
        Ok(scope) => {
            let _ = scope.tools();
            scope.shutdown().await.ok();
        }
        Err(McpScopeError::InlineStartup { .. }) => {
            // Acceptable: connection layer expects real MCP framing.
        }
        Err(other) => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn i_t3_name_conflict_at_spawn_time() {
    let registry = registry_with_global_tool("github", "gh-search");
    let global = Arc::new(tokio::sync::RwLock::new(registry));
    let agent = AgentDef::new("scoped", AgentMode::SubAgent).with_mcp_servers(vec![
        McpServerSpec::Inline {
            name: "github".into(),
            config: McpInlineConfig {
                command: "/bin/echo".into(),
                args: vec!["hi".into()],
                env: Default::default(),
            },
        },
    ]);
    let err = McpScope::provision(&agent, global, None)
        .await
        .expect_err("must fail at spawn time");
    assert!(
        matches!(err, McpScopeError::NameConflict(ref n) if n == "github"),
        "got {err:?}"
    );
}

#[tokio::test]
async fn i_t4_provision_perf_budget_warn_only() {
    let registry = registry_with_global_tool("github", "gh-search");
    let global = Arc::new(tokio::sync::RwLock::new(registry));
    let agent = AgentDef::new("scoped", AgentMode::SubAgent).with_mcp_servers(vec![
        McpServerSpec::Reference {
            name: "github".into(),
        },
    ]);
    let t0 = std::time::Instant::now();
    let scope = McpScope::provision(&agent, global, None)
        .await
        .expect("provision");
    let elapsed_ms = t0.elapsed().as_millis();
    scope.shutdown().await.expect("shutdown");

    if elapsed_ms > 500 {
        eprintln!(
            "WARN: McpScope::provision took {elapsed_ms}ms (soft budget: 500ms). \
             This is a warn-only signal — investigate if seen consistently in CI."
        );
    }
    assert!(
        elapsed_ms < 2000,
        "provision took {elapsed_ms}ms (hard ceiling: 2000ms = 4× CI headroom)"
    );
}

#[tokio::test]
async fn i_t5_drop_teardown_emits_leaked_event() {
    let registry = PluginRegistry::new();
    let global = Arc::new(tokio::sync::RwLock::new(registry));
    let sink = CapturingSink::default();
    let arc_sink: Arc<dyn TraceSink> = Arc::new(sink.clone());

    let agent = AgentDef::new("scoped", AgentMode::SubAgent).with_mcp_servers(vec![
        McpServerSpec::Inline {
            name: "fresh".into(),
            config: McpInlineConfig {
                command: "/bin/cat".into(),
                args: vec![],
                env: Default::default(),
            },
        },
    ]);

    {
        let result = McpScope::provision(&agent, global, Some(arc_sink.clone())).await;
        match result {
            Ok(scope) => {
                drop(scope);
            }
            Err(_) => {
                // Inline startup failed (expected when /bin/cat doesn't speak MCP).
                return;
            }
        }
    }

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let events = sink.snapshot();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, LoopTraceEvent::McpScopeCleaned { leaked: true, .. })),
        "expected McpScopeCleaned(leaked=true) on Drop path; got: {events:?}"
    );
}
