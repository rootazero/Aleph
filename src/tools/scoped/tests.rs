use super::*;
// HookExecutor + make_command_hook are only exercised by the cfg(unix) hook tests below.
#[cfg(unix)]
use crate::extension::hooks::HookExecutor;
#[cfg(unix)]
use crate::extension::{HookAction, HookConfig, HookEvent, HookKind, HookPriority};
use crate::sync_primitives::Arc as StdArc;
use crate::tools::runtime::{LoopTool, LoopToolRegistry, ToolResult as LoopToolResult};
use serde_json::{json, Value};
#[cfg(unix)]
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex as StdMutex;

// -------------------------------------------------------------------------
// Stubs
// -------------------------------------------------------------------------

/// A `LoopTool` whose name is set at construction time (owned `String`).
/// Used by deferred-tier tests that need two tools with distinct names.
struct NamedStub(String);
impl NamedStub {
    fn new(n: &str) -> Self {
        Self(n.to_string())
    }
}
#[async_trait::async_trait]
impl LoopTool for NamedStub {
    fn name(&self) -> &str {
        &self.0
    }
    fn description(&self) -> &str {
        "stub"
    }
    fn schema(&self) -> Value {
        json!({ "type": "object" })
    }
    // Declared parallel-safe so the tier tests keep their contrast: a GATE
    // (permission Ask / tier argument filter) forcing `Global` must be
    // distinguishable from the inner claim, which requires the ungated inner
    // claim to be `Shared`. (The trait default is fail-closed `false`.)
    fn is_concurrent_safe(&self, _input: &Value) -> bool {
        true
    }
    async fn execute(&self, _input: Value, _cancel: CancellationToken) -> LoopToolResult {
        LoopToolResult::Success { output: json!({}) }
    }
}

/// Noop tool service stub used as `parent_tools` for SubagentTool in tests.
struct NoopParentTools;

#[async_trait::async_trait]
impl ToolService for NoopParentTools {
    async fn execute(&self, _name: &str, _input: Value) -> Result<ToolOutput, ToolError> {
        Err(ToolError::NotFound {
            name: "test".into(),
        })
    }
    async fn list(&self) -> Vec<ToolDefinition> {
        vec![]
    }
    async fn describe(&self, _: &str) -> Option<ToolDefinition> {
        None
    }
    fn metadata_schema(&self) -> Arc<[crate::tool_metadata::ToolDefinition]> {
        Arc::from([])
    }
}

fn in_mem_session() -> Arc<dyn crate::session::service::SessionService> {
    use crate::session::in_process::InProcessActorSessionService;
    use crate::session::store::{migrate_add_session_events, SessionEventStore, SqliteEventStore};
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    migrate_add_session_events(&conn).unwrap();
    let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
    Arc::new(InProcessActorSessionService::new(store))
}

struct StubTool {
    tool_name: &'static str,
}

#[async_trait::async_trait]
impl LoopTool for StubTool {
    fn name(&self) -> &str {
        self.tool_name
    }
    fn description(&self) -> &str {
        "stub"
    }
    fn schema(&self) -> Value {
        json!({ "type": "object" })
    }
    // Models a declared parallel-safe (read-only) tool; the trait default is
    // now fail-closed `false`, so safety must be explicit (`UnsafeStubTool`
    // below models the default/mutating side).
    fn is_concurrent_safe(&self, _input: &Value) -> bool {
        true
    }
    async fn execute(&self, _input: Value, _cancel: CancellationToken) -> LoopToolResult {
        LoopToolResult::Success {
            output: json!({ "tool": self.tool_name }),
        }
    }
}

fn make_registry(names: &[&'static str]) -> Arc<LoopToolRegistry> {
    let mut r = LoopToolRegistry::new();
    for &name in names {
        r.register(Box::new(StubTool { tool_name: name }));
    }
    Arc::new(r)
}

// -------------------------------------------------------------------------
// Test 1: list filters by allowed set
// -------------------------------------------------------------------------
#[tokio::test]
async fn list_filters_by_allowed_set() {
    let registry = make_registry(&["read_file", "write_file"]);
    let allowed = ["read_file".to_string()].into_iter().collect();
    let svc = ScopedToolService::new(registry, allowed);

    let defs = svc.list().await;
    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].name, "read_file");
}

// -------------------------------------------------------------------------
// Test 1b: definition rewriter mutates description and schema
// -------------------------------------------------------------------------

/// Rewriter that prepends an "[AGENT-A]" marker and stamps a custom
/// `x-agent` field into the schema. Lets us assert the rewriter's
/// output reaches the LLM-facing path (both `list()` and the
/// `metadata_schema()` cache).
struct StampingRewriter;
impl ToolDefinitionRewriter for StampingRewriter {
    fn rewrite(&self, def: &mut ToolDefinition) {
        def.description = format!("[AGENT-A] {}", def.description);
        if let Some(obj) = def.input_schema.as_object_mut() {
            obj.insert("x-agent".into(), json!("agent-a"));
        }
    }
}

#[tokio::test]
async fn list_applies_definition_rewriter() {
    let registry = make_registry(&["read_file"]);
    let svc = ScopedToolService::new(registry, std::collections::BTreeSet::new())
        .with_definition_rewriter(Arc::new(StampingRewriter));

    let defs = svc.list().await;
    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].description, "[AGENT-A] stub");
    assert_eq!(defs[0].input_schema.get("x-agent"), Some(&json!("agent-a")));
}

#[tokio::test]
async fn describe_applies_definition_rewriter() {
    let registry = make_registry(&["read_file"]);
    let svc = ScopedToolService::new(registry, std::collections::BTreeSet::new())
        .with_definition_rewriter(Arc::new(StampingRewriter));

    let def = svc.describe("read_file").await.expect("present");
    assert_eq!(def.description, "[AGENT-A] stub");
}

#[test]
fn metadata_schema_reflects_rewriter_after_cache_bump() {
    // metadata_schema() caches its output; assert the bump-generation
    // helper actually re-runs the rewriter chain so callers can opt-in
    // to a fresh pass without rebuilding the whole service.
    let registry = make_registry(&["read_file"]);
    let svc = ScopedToolService::new(registry, std::collections::BTreeSet::new())
        .with_definition_rewriter(Arc::new(StampingRewriter));

    let first = svc.metadata_schema();
    assert_eq!(first[0].description, "[AGENT-A] stub");

    // A second call without bumping → cache hit, same Arc.
    let second = svc.metadata_schema();
    assert!(Arc::ptr_eq(&first, &second), "expected cached identity");

    // After an explicit bump the schema is recomputed (rewriter runs
    // again). Identity differs from the first; content stays correct.
    svc.bump_cache_generation();
    let third = svc.metadata_schema();
    assert!(!Arc::ptr_eq(&first, &third), "cache must invalidate");
    assert_eq!(third[0].description, "[AGENT-A] stub");
}

// -------------------------------------------------------------------------
// Test 2: list includes subagent tool when set
// -------------------------------------------------------------------------
#[tokio::test]
async fn list_includes_subagent_tool_when_set() {
    use crate::agents::background_tracker::BackgroundAgentTracker;
    use crate::agents::AgentRegistry;
    use crate::harness::chain_context::ChainContext;
    use crate::providers::adapter::{ProviderResponse, RequestPayload};
    use crate::providers::AiProvider;
    use std::future::Future;
    use std::pin::Pin;

    struct MockProvider;
    impl AiProvider for MockProvider {
        fn process<'a>(
            &'a self,
            _p: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = crate::error::Result<ProviderResponse>> + Send + 'a>>
        {
            Box::pin(async { Ok(ProviderResponse::text_only("ok".into())) })
        }
        fn name(&self) -> &str {
            "mock"
        }
        fn color(&self) -> &str {
            "#000"
        }
    }

    let provider: Arc<dyn AiProvider> = Arc::new(MockProvider);
    let chain = ChainContext::new();
    let registry_arc = Arc::new(AgentRegistry::with_builtins());
    let tracker = Arc::new(BackgroundAgentTracker::new());
    let st = Arc::new(crate::agents::subagent_tool::SubagentTool::new(
        provider,
        chain,
        registry_arc,
        tracker,
        in_mem_session(),
        Arc::new(NoopParentTools),
    ));

    let registry = make_registry(&["read_file"]);
    let svc = ScopedToolService::new(registry, BTreeSet::new()).with_subagent_tool(st);

    let defs = svc.list().await;
    let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
    assert!(
        names.contains(&"subagent"),
        "subagent must be in list; got: {:?}",
        names
    );

    // The subagent tool is attached beside the registry, so it never passed
    // through the metadata builder and shipped a hardcoded default: no
    // wall-clock budget → any delegation slower than the harness fallback
    // aborted the PARENT run. Both definition paths must carry the budget.
    let expected = crate::tools::budget::builtin_tool_budget_ms("subagent");
    assert!(expected.is_some(), "subagent must be in the budget table");
    let listed = defs.iter().find(|d| d.name == "subagent").expect("listed");
    assert_eq!(listed.metadata.max_duration_ms, expected);
    let described = svc.describe("subagent").await.expect("subagent described");
    assert_eq!(described.metadata.max_duration_ms, expected);
}

// -------------------------------------------------------------------------
// Test 2b: subagent survives a non-empty allow set (production path).
//
// Regression for the gateway run_loop wiring: `allowed_names` is built
// from the builtin tool registry's tool definitions, which never contains
// "subagent" (SubagentTool is attached on top of the registry). Before
// the is_allowed exemption, list / describe / execute / metadata_schema
// all silently dropped subagent whenever the allow set was non-empty —
// i.e. every real LLM-facing call.
// -------------------------------------------------------------------------
#[tokio::test]
async fn subagent_survives_non_empty_allow_set() {
    use crate::agents::background_tracker::BackgroundAgentTracker;
    use crate::agents::AgentRegistry;
    use crate::harness::chain_context::ChainContext;
    use crate::providers::adapter::{ProviderResponse, RequestPayload};
    use crate::providers::AiProvider;
    use std::future::Future;
    use std::pin::Pin;

    struct MockProvider;
    impl AiProvider for MockProvider {
        fn process<'a>(
            &'a self,
            _p: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = crate::error::Result<ProviderResponse>> + Send + 'a>>
        {
            Box::pin(async { Ok(ProviderResponse::text_only("ok".into())) })
        }
        fn name(&self) -> &str {
            "mock"
        }
        fn color(&self) -> &str {
            "#000"
        }
    }

    let provider: Arc<dyn AiProvider> = Arc::new(MockProvider);
    let st = Arc::new(crate::agents::subagent_tool::SubagentTool::new(
        provider,
        ChainContext::new(),
        Arc::new(AgentRegistry::with_builtins()),
        Arc::new(BackgroundAgentTracker::new()),
        in_mem_session(),
        Arc::new(NoopParentTools),
    ));

    // Production-shaped allow set: only registry-known tool names, no
    // "subagent" entry — exactly what gateway run_loop produces.
    let registry = make_registry(&["read_file", "write_file"]);
    let allowed: BTreeSet<String> = ["read_file".into(), "write_file".into()].into();
    let svc = ScopedToolService::new(registry, allowed).with_subagent_tool(st);

    // (1) list() exposes subagent
    let names: Vec<String> = svc.list().await.into_iter().map(|d| d.name).collect();
    assert!(
        names.iter().any(|n| n == "subagent"),
        "list() must expose subagent under non-empty allow set; got {:?}",
        names
    );

    // (2) describe() returns subagent (used when LLM probes the schema)
    assert!(
        svc.describe("subagent").await.is_some(),
        "describe(subagent) must return Some under non-empty allow set"
    );

    // (3) metadata_schema (LLM-facing) includes subagent
    let schema_names: Vec<String> = svc
        .metadata_schema()
        .iter()
        .map(|t| t.name.clone())
        .collect();
    assert!(
        schema_names.iter().any(|n| n == "subagent"),
        "metadata_schema must include subagent; got {:?}",
        schema_names
    );

    // (4) execute("subagent", …) is not rejected as NotFound by the
    //     allow-filter (mock provider lets the call complete).
    let result = svc.execute("subagent", json!({ "task": "ping" })).await;
    assert!(
        !matches!(&result, Err(ToolError::NotFound { name }) if name == "subagent"),
        "execute(subagent) must not be NotFound under non-empty allow set; got {:?}",
        result
    );
}

// -------------------------------------------------------------------------
// Test 4: execute routes to subagent tool by name
// -------------------------------------------------------------------------
#[tokio::test]
async fn execute_routes_to_subagent_tool_by_name() {
    use crate::agents::background_tracker::BackgroundAgentTracker;
    use crate::agents::AgentRegistry;
    use crate::harness::chain_context::ChainContext;
    use crate::providers::adapter::{ProviderResponse, RequestPayload};
    use crate::providers::AiProvider;
    use std::future::Future;
    use std::pin::Pin;

    struct MockProvider;
    impl AiProvider for MockProvider {
        fn process<'a>(
            &'a self,
            _p: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = crate::error::Result<ProviderResponse>> + Send + 'a>>
        {
            Box::pin(async { Ok(ProviderResponse::text_only("subagent result".into())) })
        }
        fn name(&self) -> &str {
            "mock"
        }
        fn color(&self) -> &str {
            "#000"
        }
    }

    let provider: Arc<dyn AiProvider> = Arc::new(MockProvider);
    let chain = ChainContext::new();
    let registry_arc = Arc::new(AgentRegistry::with_builtins());
    let tracker = Arc::new(BackgroundAgentTracker::new());
    let st = Arc::new(crate::agents::subagent_tool::SubagentTool::new(
        provider,
        chain,
        registry_arc,
        tracker,
        in_mem_session(),
        Arc::new(NoopParentTools),
    ));

    // Registry has NO "subagent" tool — proves routing goes to st, not inner
    let registry = make_registry(&["read_file"]);
    let svc = ScopedToolService::new(registry, BTreeSet::new()).with_subagent_tool(st);

    // A valid subagent call; the mock provider returns "subagent result"
    let result = svc
        .execute("subagent", json!({ "task": "do something" }))
        .await;
    assert!(
        result.is_ok(),
        "subagent execute should succeed; got: {:?}",
        result.err()
    );
}

// -------------------------------------------------------------------------
// Test 5 (was: execute applies hook decorator) was removed with the
// legacy `ToolHookDecorator` trait (see the audit at
// review-results/agents-batch-6/tools/summary.json finding #3).
// -------------------------------------------------------------------------

// -------------------------------------------------------------------------
// Test 6: describe returns from filtered set (allowed / not-allowed)
// -------------------------------------------------------------------------
#[tokio::test]
async fn describe_returns_from_filtered_set() {
    let registry = make_registry(&["read_file", "write_file"]);
    let allowed = ["read_file".to_string()].into_iter().collect();
    let svc = ScopedToolService::new(registry, allowed);

    // Allowed tool: should return Some
    let def = svc.describe("read_file").await;
    assert!(def.is_some(), "read_file is allowed, must be found");
    assert_eq!(def.unwrap().name, "read_file");

    // Not-in-allowed tool: must return None
    let def = svc.describe("write_file").await;
    assert!(def.is_none(), "write_file is not in allowed set");

    // Totally unknown tool: must return None
    let def = svc.describe("nonexistent").await;
    assert!(def.is_none(), "unknown tool must return None");
}

// -------------------------------------------------------------------------
// Test 7: metadata_schema caches when no refresh signal
// -------------------------------------------------------------------------

#[test]
fn scoped_metadata_schema_caches_when_no_refresh_signal() {
    let registry = make_registry(&["a", "b"]);
    let svc = ScopedToolService::new(registry, BTreeSet::new());
    let s1 = svc.metadata_schema();
    let s2 = svc.metadata_schema();
    assert!(
        Arc::ptr_eq(&s1, &s2),
        "without refresh signal cache should hold across calls"
    );
    assert_eq!(s1.len(), 2);
}

// -------------------------------------------------------------------------
// Test 8: metadata_schema respects allowed filter
// -------------------------------------------------------------------------

#[test]
fn scoped_metadata_schema_respects_allowed_filter() {
    let registry = make_registry(&["a", "b"]);
    let mut allowed = BTreeSet::new();
    allowed.insert("a".to_string());
    let svc = ScopedToolService::new(registry, allowed);
    let s = svc.metadata_schema();
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].name, "a");
}

// Health gate tests
// -------------------------------------------------------------------------

use crate::tool_metadata::{HealthReason, ProbeResult, ToolHealthProbe};
use std::borrow::Cow;

struct AlwaysDead;

#[async_trait::async_trait]
impl ToolHealthProbe for AlwaysDead {
    async fn probe(&self) -> ProbeResult {
        ProbeResult::Unhealthy {
            reason: HealthReason::DependencyDown(Cow::Borrowed("test fixture")),
            retry_after: None,
        }
    }
}

// -------------------------------------------------------------------------
// Extension HookExecutor wiring
// -------------------------------------------------------------------------

/// Capturing tool that echoes the input value it actually receives, so
/// tests can assert on hook-rewritten input flowing into the underlying
/// tool implementation.
struct EchoTool;

#[async_trait::async_trait]
impl LoopTool for EchoTool {
    fn name(&self) -> &str {
        "echo"
    }
    fn description(&self) -> &str {
        "echoes input"
    }
    fn schema(&self) -> Value {
        json!({ "type": "object" })
    }
    async fn execute(&self, input: Value, _cancel: CancellationToken) -> LoopToolResult {
        LoopToolResult::Success { output: input }
    }
}

fn echo_registry() -> Arc<LoopToolRegistry> {
    let mut r = LoopToolRegistry::new();
    r.register(Box::new(EchoTool));
    Arc::new(r)
}

#[cfg(unix)]
fn make_command_hook(event: HookEvent, kind: HookKind, command: &str) -> HookConfig {
    HookConfig {
        event,
        kind,
        priority: HookPriority::Normal,
        matcher: None,
        actions: vec![HookAction::Command {
            command: command.to_string(),
        }],
        plugin_name: "test".to_string(),
        plugin_root: PathBuf::from("/tmp"),
        handler: None,
        timeout_secs: None,
    }
}

/// `apply_layer_two` always stringifies tool output (`Value::String`).
/// Tests that care about the structured shape parse it back here.
fn parse_tool_output(value: &Value) -> Value {
    match value {
        Value::String(s) => serde_json::from_str(s).unwrap_or_else(|_| value.clone()),
        other => other.clone(),
    }
}

#[tokio::test]
#[cfg(unix)] // POSIX-only: shell hook uses sh (echo quoting / printf / '/tmp')
async fn before_tool_hook_block_returns_execution_error() {
    let executor = Arc::new(HookExecutor::new(vec![make_command_hook(
        HookEvent::BeforeToolCall,
        HookKind::Interceptor,
        "echo 'block: blocked by policy'",
    )]));
    let svc = ScopedToolService::new(echo_registry(), BTreeSet::new())
        .with_hook_executor(executor, "test-session");

    match svc.execute("echo", json!({})).await {
        Err(ToolError::Execution { name, cause }) => {
            assert_eq!(name, "echo");
            assert!(
                cause.contains("blocked by policy"),
                "unexpected cause: {cause}"
            );
        }
        other => panic!("expected Execution error from block hook, got: {other:?}"), // rust-doctor-disable-line panic-in-library
    }
}

#[tokio::test]
#[cfg(unix)] // POSIX-only: shell hook uses sh (echo quoting / printf / '/tmp')
async fn before_tool_hook_deny_returns_permission_denied() {
    let executor = Arc::new(HookExecutor::new(vec![make_command_hook(
        HookEvent::BeforeToolCall,
        HookKind::Interceptor,
        "echo 'deny: hard policy stop'",
    )]));
    let svc = ScopedToolService::new(echo_registry(), BTreeSet::new())
        .with_hook_executor(executor, "test-session");

    match svc.execute("echo", json!({})).await {
        Err(ToolError::PermissionDenied { name, reason }) => {
            assert_eq!(name, "echo");
            assert!(
                reason.contains("hard policy stop"),
                "unexpected reason: {reason}"
            );
        }
        other => panic!("expected PermissionDenied from deny hook, got: {other:?}"), // rust-doctor-disable-line panic-in-library
    }
}

#[tokio::test]
#[cfg(unix)] // POSIX-only: shell hook uses sh (echo quoting / printf / '/tmp')
async fn before_tool_hook_update_input_rewrites_args() {
    // Hook rewrites the tool input to a fixed JSON value. The EchoTool
    // returns whatever input it receives, so we can assert by reading the
    // tool output.
    let executor = Arc::new(HookExecutor::new(vec![make_command_hook(
        HookEvent::BeforeToolCall,
        HookKind::Interceptor,
        r#"echo 'update_input: {"path":"/etc/hosts","force":true}'"#,
    )]));
    let svc = ScopedToolService::new(echo_registry(), BTreeSet::new())
        .with_hook_executor(executor, "test-session");

    let output = svc
        .execute("echo", json!({ "path": "/tmp/original" }))
        .await
        .expect("execute should succeed when hook only rewrites input");
    // Layer 2 stringifies tool output; parse it back to inspect fields.
    let parsed = parse_tool_output(&output.value);
    assert_eq!(parsed["path"], json!("/etc/hosts"));
    assert_eq!(parsed["force"], json!(true));
}

#[tokio::test]
#[cfg(unix)] // POSIX-only: shell hook uses sh (echo quoting / printf / '/tmp')
async fn before_tool_hook_context_wraps_tool_output_for_llm() {
    // BeforeToolCall hook emits `context:` lines. Historically these
    // landed in `HookResult.additional_contexts` but nothing consumed
    // them — the LLM never saw them. This test guards the wiring fix
    // (scoped.rs wraps them as `<system-reminder>` blocks on the tool
    // output value so they reach the model next turn).
    let executor = Arc::new(HookExecutor::new(vec![make_command_hook(
        HookEvent::BeforeToolCall,
        HookKind::Interceptor,
        "echo 'context: file auto-formatted'\necho 'context: lint passed'",
    )]));
    let svc = ScopedToolService::new(echo_registry(), BTreeSet::new())
        .with_hook_executor(executor, "test-session");

    let output = svc
        .execute("echo", json!({ "path": "/tmp/x" }))
        .await
        .expect("execute should succeed");
    let s = output
        .value
        .as_str()
        .expect("hook contexts wrap result as a string");
    assert!(
        s.contains("<system-reminder>"),
        "missing reminder wrapper: {s}"
    );
    assert!(
        s.contains("file auto-formatted"),
        "missing context line: {s}"
    );
    assert!(s.contains("lint passed"), "missing context line: {s}");
}

#[tokio::test]
#[cfg(unix)] // POSIX-only: shell hook uses sh (echo quoting / printf / '/tmp')
async fn after_tool_hook_observer_fires_on_success() {
    // Observer writes the tool name to a tempfile so we can prove it
    // fired with the right context. Run inside a per-test tempdir to
    // avoid interference when tests run in parallel.
    let dir = tempfile::tempdir().expect("create tempdir");
    let marker = dir.path().join("after.flag");
    let marker_str = marker.to_string_lossy().to_string();
    let cmd = format!(r#"printf '%s' "$TOOL_NAME" > '{marker_str}'"#);
    let executor = Arc::new(HookExecutor::new(vec![make_command_hook(
        HookEvent::AfterToolCall,
        HookKind::Observer,
        &cmd,
    )]));
    let svc = ScopedToolService::new(echo_registry(), BTreeSet::new())
        .with_hook_executor(executor, "test-session");

    let _ = svc
        .execute("echo", json!({}))
        .await
        .expect("execute should succeed");

    let contents = tokio::fs::read_to_string(&marker)
        .await
        .expect("observer must have written marker");
    assert_eq!(contents.trim(), "echo");
}

#[tokio::test]
#[cfg(unix)] // POSIX-only: shell hook uses sh (echo quoting / printf / '/tmp')
async fn after_tool_failure_hook_fires_when_tool_errors() {
    // Construct a registry with a tool that always errors.
    struct ErrTool;
    #[async_trait::async_trait]
    impl LoopTool for ErrTool {
        fn name(&self) -> &str {
            "boom"
        }
        fn description(&self) -> &str {
            "always errors"
        }
        fn schema(&self) -> Value {
            json!({ "type": "object" })
        }
        async fn execute(&self, _input: Value, _cancel: CancellationToken) -> LoopToolResult {
            LoopToolResult::Error {
                error: "kaboom".to_string(),
                retryable: false,
            }
        }
    }
    let mut r = LoopToolRegistry::new();
    r.register(Box::new(ErrTool));
    let registry = Arc::new(r);

    let dir = tempfile::tempdir().expect("create tempdir");
    let marker = dir.path().join("fail.flag");
    let marker_str = marker.to_string_lossy().to_string();
    let cmd = format!(r#"printf '%s' "$TOOL_NAME" > '{marker_str}'"#);
    let executor = Arc::new(HookExecutor::new(vec![make_command_hook(
        HookEvent::AfterToolCallFailure,
        HookKind::Observer,
        &cmd,
    )]));
    let svc = ScopedToolService::new(registry, BTreeSet::new())
        .with_hook_executor(executor, "test-session");

    let err = svc
        .execute("boom", json!({}))
        .await
        .expect_err("tool returns error");
    assert!(matches!(err, ToolError::Execution { .. }));

    let contents = tokio::fs::read_to_string(&marker)
        .await
        .expect("failure observer must have written marker");
    assert_eq!(contents.trim(), "boom");
}

#[tokio::test]
async fn no_hooks_means_no_change_in_behaviour() {
    // Sanity: without `.with_hook_executor`, ScopedToolService behaves
    // identically to before (regression guard).
    let svc = ScopedToolService::new(echo_registry(), BTreeSet::new());
    let out = svc
        .execute("echo", json!({ "k": "v" }))
        .await
        .expect("execute succeeds");
    // Layer 2 stringifies tool output; parse it back to compare structurally.
    assert_eq!(parse_tool_output(&out.value), json!({ "k": "v" }));
}

#[tokio::test]
async fn list_strips_unhealthy_tools() {
    let registry = make_registry(&["alive", "dead"]);
    let health = Arc::new(ToolHealthCache::new());
    health.register_probe("dead", Arc::new(AlwaysDead));
    health.refresh("dead").await;
    let svc = ScopedToolService::new(registry, BTreeSet::new()).with_health(health);
    let defs = svc.list().await;
    let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
    assert!(names.contains(&"alive"));
    assert!(!names.contains(&"dead"), "got: {names:?}");
}

#[test]
fn metadata_schema_strips_unhealthy_tools_and_invalidates_on_flip() {
    // Driven sync from outside an async runtime — populate the cache via
    // a small block_on island.
    let registry = make_registry(&["alive", "dead"]);
    let health = Arc::new(ToolHealthCache::new());
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        health.register_probe("dead", Arc::new(AlwaysDead));
        health.refresh("dead").await;
    });
    let svc = ScopedToolService::new(registry, BTreeSet::new()).with_health(Arc::clone(&health));

    let s1 = svc.metadata_schema();
    let names: Vec<&str> = s1.iter().map(|d| d.name.as_str()).collect();
    assert!(names.contains(&"alive"));
    assert!(!names.contains(&"dead"), "first call should strip dead");

    // Flip "dead" healthy via invalidation; the schema cache must
    // invalidate so the next call surfaces "dead" again.
    health.invalidate_all();
    let s2 = svc.metadata_schema();
    assert!(
        !Arc::ptr_eq(&s1, &s2),
        "cache must rotate when health generation flips"
    );
    let names2: Vec<&str> = s2.iter().map(|d| d.name.as_str()).collect();
    assert!(
        names2.contains(&"dead"),
        "after invalidation, dead reappears; got: {names2:?}"
    );
}

// -------------------------------------------------------------------------
// CRITICAL-1 — describe() populates per-tool budget + idempotency metadata
// from the static tables, so the harness's per-tool wall-clock budget can
// actually fire (it was always `None` while metadata was hardcoded default).
// -------------------------------------------------------------------------
#[tokio::test]
async fn describe_populates_builtin_budget_metadata() {
    // `memory_search` is in BUILTIN_TOOL_BUDGETS_MS (5_000ms) and is a
    // declared pure read (`is_idempotent_builtin_name`).
    let registry = make_registry(&["memory_search"]);
    let svc = ScopedToolService::new(registry, BTreeSet::new());
    let def = svc.describe("memory_search").await.expect("tool present");
    assert_eq!(def.metadata.max_duration_ms, Some(5_000));
    assert!(def.metadata.idempotent);
}

#[tokio::test]
async fn describe_falls_back_to_the_default_budget_for_untabled_tool() {
    // A tool absent from the table used to advertise `None`, which the harness
    // read as "no per-tool budget" and escalated into a run-level StalledTurn
    // abort. Every definition now carries a budget, so a slow call is a
    // recoverable tool error instead.
    let registry = make_registry(&["some_custom_tool"]);
    let svc = ScopedToolService::new(registry, BTreeSet::new());
    let def = svc
        .describe("some_custom_tool")
        .await
        .expect("tool present");
    assert_eq!(
        def.metadata.max_duration_ms,
        Some(crate::tools::budget::DEFAULT_TOOL_BUDGET_MS)
    );
    assert!(!def.metadata.idempotent);
}

/// A `LoopTool` that declares its own wall-clock budget — the seam MCP tools
/// use to surface their owning server's configured request timeout.
struct SelfBudgetedTool;

#[async_trait::async_trait]
impl LoopTool for SelfBudgetedTool {
    fn name(&self) -> &str {
        "self_budgeted"
    }
    fn description(&self) -> &str {
        "declares its own budget"
    }
    fn schema(&self) -> Value {
        json!({ "type": "object" })
    }
    async fn execute(&self, _input: Value, _cancel: CancellationToken) -> LoopToolResult {
        LoopToolResult::Success { output: json!({}) }
    }
    fn max_duration_ms(&self) -> Option<u64> {
        Some(777_000)
    }
}

#[tokio::test]
async fn declared_budget_wins_over_the_table_and_the_default() {
    let mut registry = LoopToolRegistry::new();
    registry.register(Box::new(SelfBudgetedTool));
    let svc = ScopedToolService::new(Arc::new(registry), BTreeSet::new());
    let def = svc.describe("self_budgeted").await.expect("tool present");
    assert_eq!(def.metadata.max_duration_ms, Some(777_000));
    // list() and metadata_schema() rebuild definitions separately from
    // describe(); a budget that only reaches one of them is a budget the
    // harness may still miss.
    let listed = svc.list().await;
    let listed = listed
        .iter()
        .find(|d| d.name == "self_budgeted")
        .expect("listed");
    assert_eq!(listed.metadata.max_duration_ms, Some(777_000));
}

#[tokio::test]
async fn every_listed_definition_carries_a_budget() {
    // The invariant the harness depends on: no definition leaving the
    // registry may be unbudgeted, whatever its provenance.
    let registry = make_registry(&["memory_search", "bash", "ask_user", "some_custom_tool"]);
    let svc = ScopedToolService::new(registry, BTreeSet::new());
    for def in svc.list().await {
        assert!(
            def.metadata.max_duration_ms.is_some(),
            "tool {} left the registry without a wall-clock budget",
            def.name
        );
    }
}

// -------------------------------------------------------------------------
// call_concurrency_claim — dispatch parallelism query
// -------------------------------------------------------------------------

/// A LoopTool that always reports false for parallel safety, for tests
/// that need to see the harness route around the fast path.
struct UnsafeStubTool;

#[async_trait::async_trait]
impl LoopTool for UnsafeStubTool {
    fn name(&self) -> &str {
        "unsafe_tool"
    }
    fn description(&self) -> &str {
        "stub that mutates shared state"
    }
    fn schema(&self) -> Value {
        json!({ "type": "object" })
    }
    async fn execute(&self, _input: Value, _cancel: CancellationToken) -> LoopToolResult {
        LoopToolResult::Success {
            output: json!({"ok": true}),
        }
    }
    fn is_concurrent_safe(&self, _input: &Value) -> bool {
        false
    }
}

#[tokio::test]
async fn call_concurrency_claim_shared_for_safe_stub_tool() {
    // `StubTool` explicitly declares `is_concurrent_safe -> true` (the trait
    // default is now fail-closed `false`), so its declared safety flows
    // through the registry → ScopedToolService chain as a Shared claim.
    let registry = make_registry(&["safe_tool"]);
    let svc = ScopedToolService::new(registry, BTreeSet::new());
    assert!(matches!(
        svc.call_concurrency_claim("safe_tool", &json!({})).await,
        crate::tools::concurrency::ConcurrencyClaim::Shared
    ));
}

#[tokio::test]
async fn call_concurrency_claim_exclusive_for_unsafe_tool_override() {
    // Hand-rolled unsafe tool — propagates non-Shared claim through the
    // same chain.
    let mut r = LoopToolRegistry::new();
    r.register(Box::new(UnsafeStubTool));
    let registry = Arc::new(r);
    let svc = ScopedToolService::new(registry, BTreeSet::new());
    assert!(!matches!(
        svc.call_concurrency_claim("unsafe_tool", &json!({})).await,
        crate::tools::concurrency::ConcurrencyClaim::Shared
    ));
}

#[tokio::test]
async fn call_concurrency_claim_exclusive_for_unknown_tool() {
    // Conservative default — the harness must not parallel-dispatch a
    // tool it cannot find a definition for.
    let registry = make_registry(&[]);
    let svc = ScopedToolService::new(registry, BTreeSet::new());
    assert!(!matches!(
        svc.call_concurrency_claim("nope", &json!({})).await,
        crate::tools::concurrency::ConcurrencyClaim::Shared
    ));
}

#[tokio::test]
async fn call_concurrency_claim_exclusive_for_disallowed_tool() {
    // Tools outside the allow list are conservatively unsafe — the
    // harness's parallel fast-path scan should never see them as
    // candidates.
    let registry = make_registry(&["safe_tool", "other"]);
    let mut allowed = BTreeSet::new();
    allowed.insert("other".to_string());
    let svc = ScopedToolService::new(registry, allowed);
    assert!(!matches!(
        svc.call_concurrency_claim("safe_tool", &json!({})).await,
        crate::tools::concurrency::ConcurrencyClaim::Shared
    ));
    assert!(matches!(
        svc.call_concurrency_claim("other", &json!({})).await,
        crate::tools::concurrency::ConcurrencyClaim::Shared
    ));
}

#[tokio::test]
async fn list_propagates_concurrent_safe_flag_from_inner_tool() {
    // The metadata view served to gateway / inspection APIs should
    // surface the inner LoopTool's static parallel-safety hint.
    let mut r = LoopToolRegistry::new();
    r.register(Box::new(StubTool {
        tool_name: "safe_tool",
    }));
    r.register(Box::new(UnsafeStubTool));
    let registry = Arc::new(r);
    let svc = ScopedToolService::new(registry, BTreeSet::new());
    let defs = svc.list().await;
    let safe = defs
        .iter()
        .find(|d| d.name == "safe_tool")
        .expect("safe_tool listed");
    let unsafe_def = defs
        .iter()
        .find(|d| d.name == "unsafe_tool")
        .expect("unsafe_tool listed");
    assert!(safe.metadata.concurrent_safe);
    assert!(!unsafe_def.metadata.concurrent_safe);
}

// -------------------------------------------------------------------------
// execute_with_cancel — opencode-parity AbortSignal end-to-end
// -------------------------------------------------------------------------

/// LoopTool that sleeps 5s on every call. Used to prove the cancel token
/// propagates from `ScopedToolService::execute_with_cancel` → `LoopToolRegistry`
/// → the resolved `LoopTool::execute` without ever firing the inner sleep.
struct SlowLoopTool;

#[async_trait::async_trait]
impl LoopTool for SlowLoopTool {
    fn name(&self) -> &str {
        "slow_tool"
    }
    fn description(&self) -> &str {
        "sleeps forever"
    }
    fn schema(&self) -> Value {
        json!({ "type": "object" })
    }
    async fn execute(&self, _input: Value, cancel: CancellationToken) -> LoopToolResult {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => LoopToolResult::Error {
                error: "cancelled cooperatively".into(),
                retryable: false,
            },
            _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => LoopToolResult::Success {
                output: json!({"slept": true}),
            },
        }
    }
}

#[tokio::test]
async fn execute_with_cancel_short_circuits_inner_loop_tool() {
    let mut r = LoopToolRegistry::new();
    r.register(Box::new(SlowLoopTool));
    let registry = Arc::new(r);
    let svc = ScopedToolService::new(registry, BTreeSet::new());

    let cancel = CancellationToken::new();
    let cancel_for_spawn = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        cancel_for_spawn.cancel();
    });

    let started = std::time::Instant::now();
    let result = svc
        .execute_with_cancel("slow_tool", json!({}), cancel)
        .await;
    let elapsed = started.elapsed();

    // Must short-circuit well below the 5s inner sleep — cooperatively
    // via the tool's own `select!`, not via wrapper drop.
    assert!(
        elapsed < std::time::Duration::from_millis(500),
        "expected cancel to abort fast; took {elapsed:?}"
    );
    let err = result.expect_err("cancelled call should surface a ToolError");
    let msg = err.to_string();
    assert!(
        msg.contains("cancelled"),
        "expected cancellation error, got: {msg}"
    );
}

#[tokio::test]
async fn execute_with_cancel_runs_to_completion_when_token_never_fires() {
    // Sanity check: when the caller passes a never-fired token, the
    // tool's own `select!` lets it complete normally — i.e. the cancel
    // arm is biased but does not preempt a fresh token.
    struct InstantTool;
    #[async_trait::async_trait]
    impl LoopTool for InstantTool {
        fn name(&self) -> &str {
            "instant"
        }
        fn description(&self) -> &str {
            "returns immediately"
        }
        fn schema(&self) -> Value {
            json!({ "type": "object" })
        }
        async fn execute(&self, _input: Value, _cancel: CancellationToken) -> LoopToolResult {
            LoopToolResult::Success {
                output: json!({"ok": true}),
            }
        }
    }

    let mut r = LoopToolRegistry::new();
    r.register(Box::new(InstantTool));
    let registry = Arc::new(r);
    let svc = ScopedToolService::new(registry, BTreeSet::new());

    let out = svc
        .execute_with_cancel("instant", json!({}), CancellationToken::new())
        .await
        .expect("never-fired token should let the tool complete");
    // ScopedToolService routes the inner JSON through `apply_layer_two`,
    // which can re-render the value as a JSON-encoded string for token
    // accounting. Accept either representation — what we care about
    // here is that the call ran to completion rather than being cancelled.
    let matches = match &out.value {
        serde_json::Value::Object(_) => out.value == json!({"ok": true}),
        serde_json::Value::String(s) => {
            serde_json::from_str::<Value>(s).ok() == Some(json!({"ok": true}))
        }
        _ => false,
    };
    assert!(matches, "unexpected output shape: {:?}", out.value);
}

// -------------------------------------------------------------------------
// Panic containment — a panicking tool body costs one call, not the batch.
// -------------------------------------------------------------------------

/// Panics on every call, with a message distinctive enough to prove the
/// payload survived the catch rather than being replaced by a placeholder.
struct PanickingTool;

#[async_trait::async_trait]
impl LoopTool for PanickingTool {
    fn name(&self) -> &str {
        "panicker"
    }
    fn description(&self) -> &str {
        "panics"
    }
    fn schema(&self) -> Value {
        json!({ "type": "object" })
    }
    fn is_concurrent_safe(&self, _input: &Value) -> bool {
        true
    }
    async fn execute(&self, _input: Value, _cancel: CancellationToken) -> LoopToolResult {
        panic!("tool body exploded: {}", 42);
    }
}

/// Sleeps before succeeding, so a batch sibling is genuinely in flight (not
/// merely queued) at the moment the panicking call unwinds, and flips a flag
/// on completion — a dropped future never reaches it.
struct SlowSiblingTool(StdArc<std::sync::atomic::AtomicBool>);

#[async_trait::async_trait]
impl LoopTool for SlowSiblingTool {
    fn name(&self) -> &str {
        "sibling"
    }
    fn description(&self) -> &str {
        "completes after a yield"
    }
    fn schema(&self) -> Value {
        json!({ "type": "object" })
    }
    fn is_concurrent_safe(&self, _input: &Value) -> bool {
        true
    }
    async fn execute(&self, _input: Value, _cancel: CancellationToken) -> LoopToolResult {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        self.0.store(true, Ordering::SeqCst);
        LoopToolResult::Success {
            output: json!({ "sibling": "done" }),
        }
    }
}

#[tokio::test]
async fn a_panicking_tool_body_becomes_an_attributed_per_call_error() {
    let mut r = LoopToolRegistry::new();
    r.register(Box::new(PanickingTool));
    let svc = ScopedToolService::new(Arc::new(r), BTreeSet::new());

    let err = svc
        .execute("panicker", json!({}))
        .await
        .expect_err("a panicking tool body must resolve to an error, not unwind the caller");

    match &err {
        ToolError::Execution { name, cause } => {
            assert_eq!(
                name, "panicker",
                "the error must name the tool that panicked"
            );
            assert!(
                cause.contains("tool panicked"),
                "cause must say a panic happened, got: {cause}"
            );
            assert!(
                cause.contains("tool body exploded: 42"),
                "cause must carry the panic payload, got: {cause}"
            );
            // The synthesized error is built above `execute_inner`, so it has
            // to reach back into the same sanitizer; both fence halves prove
            // it did (a panic body is untrusted, unbounded text).
            assert!(
                cause.contains(crate::security::content_sanitizer::FENCE_OPEN_PREFIX)
                    && cause.contains(crate::security::content_sanitizer::FENCE_CLOSE_PREFIX),
                "panic body must be fenced like every other tool error, got: {cause}"
            );
        }
        other => panic!("expected ToolError::Execution, got: {other:?}"),
    }
    // Deterministic panics must not be respun into a second panic.
    assert!(!err.is_retryable(), "a panic is not a transient failure");
}

/// The documented PRICE of catching at the outermost seam.
///
/// `execute_with_cancel`'s `catch_unwind` sits above `execute_inner`, so the
/// unwind jumps over this call's post-hooks, ledger entry and artifact harvest.
/// That is a deliberate trade — a catch inside every gate stage would be the
/// alternative — and until now it was stated in one comment and nowhere else.
///
/// A cost recorded only in prose is one refactor away from being quietly paid
/// twice or quietly repaid: someone moves the seam, and nothing goes red either
/// way. So pin it, from both sides, because the absence half alone would stay
/// green against a hook that was simply never wired: the SAME service, the SAME
/// hooks, one tool that returns and one that panics.
///
/// Both post-hook events are registered, because the panic is above the fork
/// between them: `AfterToolCallFailure` is the arm a reader would expect to
/// still fire, and it does not either.
#[tokio::test]
#[cfg(unix)] // POSIX-only: shell hook uses sh
async fn a_panicking_call_skips_its_post_hooks_and_a_returning_one_does_not() {
    let dir = tempfile::tempdir().expect("create tempdir");
    let marker = dir.path().join("after.log");
    let marker_str = marker.to_string_lossy().to_string();
    let cmd = format!(r#"printf '%s\n' "$TOOL_NAME" >> '{marker_str}'"#);
    let executor = Arc::new(HookExecutor::new(vec![
        make_command_hook(HookEvent::AfterToolCall, HookKind::Observer, &cmd),
        make_command_hook(HookEvent::AfterToolCallFailure, HookKind::Observer, &cmd),
    ]));
    let mut r = LoopToolRegistry::new();
    r.register(Box::new(EchoTool));
    r.register(Box::new(PanickingTool));
    let svc = ScopedToolService::new(Arc::new(r), BTreeSet::new())
        .with_hook_executor(executor, "test-session");

    // Presence: an ordinary call reaches its post-hook.
    svc.execute("echo", json!({}))
        .await
        .expect("the echo tool succeeds");
    let after_ok = tokio::fs::read_to_string(&marker).await.unwrap_or_default();
    assert_eq!(
        after_ok.split_whitespace().collect::<Vec<_>>(),
        vec!["echo"],
        "a returning call must reach its post-hook, or the absence below proves nothing"
    );

    // The ruled-out side: a panicking call reaches neither post-hook event.
    svc.execute("panicker", json!({}))
        .await
        .expect_err("a panicking tool body resolves to an error");
    let after_panic = tokio::fs::read_to_string(&marker).await.unwrap_or_default();
    assert_eq!(
        after_panic.split_whitespace().collect::<Vec<_>>(),
        vec!["echo"],
        "a panicking call added a post-hook line: the unwind no longer jumps over \
         this call's post-hooks (and with them its ledger entry and artifact \
         harvest). If that is intended, the comment at `execute_with_cancel`'s \
         catch_unwind is the only place this cost is written down — update it too."
    );
}

#[tokio::test]
async fn a_panicking_call_does_not_take_its_batch_siblings_down() {
    use futures::StreamExt;

    // Mirrors the Act phase: several tool futures polled by ONE task via
    // `stream::iter(..).buffer_unordered(n)`. Without containment the unwind
    // escapes that task and drop-cancels every sibling with it.
    let sibling_finished = StdArc::new(std::sync::atomic::AtomicBool::new(false));
    let mut r = LoopToolRegistry::new();
    r.register(Box::new(SlowSiblingTool(sibling_finished.clone())));
    r.register(Box::new(PanickingTool));
    let svc = ScopedToolService::new(Arc::new(r), BTreeSet::new());

    // Sibling first so it is parked on its sleep when the panic fires.
    let calls = vec![
        svc.execute_with_cancel("sibling", json!({}), CancellationToken::new()),
        svc.execute_with_cancel("panicker", json!({}), CancellationToken::new()),
    ];
    let results: Vec<_> = futures::stream::iter(calls)
        .buffer_unordered(2)
        .collect()
        .await;

    assert_eq!(results.len(), 2, "both calls must report an outcome");
    assert!(
        sibling_finished.load(Ordering::SeqCst),
        "the sibling was dropped mid-flight instead of running to completion"
    );
    assert!(
        results.iter().any(|r| r.is_ok()),
        "the sibling's success must survive the panicking call"
    );
    assert!(
        results.iter().any(|r| matches!(
            r, Err(ToolError::Execution { name, .. }) if name == "panicker"
        )),
        "the panicking call must surface as its own attributed failure"
    );
}

// -------------------------------------------------------------------------
// Per-tool confirmation gate — `LoopTool::requires_confirmation()` honored
// by the dispatch gate.
// -------------------------------------------------------------------------

/// A tool that declares itself confirmation-required without being in any
/// hard-coded gateway list — the MCP / extension / skill opt-in path.
struct ConfirmTool;

#[async_trait::async_trait]
impl LoopTool for ConfirmTool {
    fn name(&self) -> &str {
        "danger"
    }
    fn description(&self) -> &str {
        "irreversible stub"
    }
    fn schema(&self) -> Value {
        json!({ "type": "object" })
    }
    async fn execute(&self, _input: Value, _cancel: CancellationToken) -> LoopToolResult {
        LoopToolResult::Success {
            output: json!({ "ran": true }),
        }
    }
    fn is_concurrent_safe(&self, _input: &Value) -> bool {
        false
    }
    fn requires_confirmation(&self) -> bool {
        true
    }
}

/// Records every approval request (count + the actions the human was shown)
/// and returns a fixed outcome.
struct FakeRequester {
    outcome: crate::sandbox::exec_approval::gate::ApprovalOutcome,
    calls: AtomicUsize,
    seen: StdMutex<Vec<crate::sandbox::exec_approval::ApprovalAction>>,
}

impl FakeRequester {
    fn new(outcome: crate::sandbox::exec_approval::gate::ApprovalOutcome) -> Self {
        Self {
            outcome,
            calls: AtomicUsize::new(0),
            seen: StdMutex::new(Vec::new()),
        }
    }

    /// The summaries put in front of the human, in order.
    fn summaries(&self) -> Vec<String> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .map(|a| a.summary.clone())
            .collect()
    }

    /// The chain rule each card named, in order.
    fn rule_ids(&self) -> Vec<Option<&'static str>> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .map(|a| a.rule_id)
            .collect()
    }
}

#[async_trait::async_trait]
impl crate::sandbox::exec_approval::gate::ApprovalRequester for FakeRequester {
    async fn request_approval(
        &self,
        action: &crate::sandbox::exec_approval::ApprovalAction,
    ) -> crate::sandbox::exec_approval::gate::ApprovalResponse {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.seen.lock().unwrap().push(action.clone());
        self.outcome.into()
    }
}

fn confirm_registry() -> Arc<LoopToolRegistry> {
    let mut r = LoopToolRegistry::new();
    r.register(Box::new(ConfirmTool));
    r.register(Box::new(StubTool { tool_name: "plain" }));
    Arc::new(r)
}

#[test]
fn registry_reports_per_tool_requires_confirmation() {
    let reg = confirm_registry();
    assert!(reg.requires_confirmation("danger"));
    assert!(!reg.requires_confirmation("plain"));
    // Unknown tool is conservatively not gated here (allowed-filter rejects it).
    assert!(!reg.requires_confirmation("nope"));
    // Alias resolution: dotted spelling still resolves to the same tool.
    let mut r = LoopToolRegistry::new();
    r.register(Box::new(ConfirmTool));
    assert!(r.requires_confirmation("danger"));
}

#[tokio::test]
async fn declared_confirmation_tool_runs_when_approved() {
    let requester = StdArc::new(FakeRequester::new(
        crate::sandbox::exec_approval::gate::ApprovalOutcome::Approved,
    ));
    // Gating comes solely from the tool's own `requires_confirmation()`
    // declaration.
    let svc = ScopedToolService::new(confirm_registry(), BTreeSet::new())
        .with_confirmation(StdArc::clone(&requester) as _);

    let out = svc
        .execute("danger", json!({}))
        .await
        .expect("approved → runs");
    let ran = match &out.value {
        Value::Object(_) => out.value == json!({"ran": true}),
        Value::String(s) => serde_json::from_str::<Value>(s).ok() == Some(json!({"ran": true})),
        _ => false,
    };
    assert!(ran, "unexpected output: {:?}", out.value);
    assert_eq!(
        requester.calls.load(Ordering::SeqCst),
        1,
        "gate must prompt once"
    );
}

#[tokio::test]
async fn an_expired_approval_card_is_not_a_refusal() {
    use crate::sandbox::exec_approval::gate::ApprovalOutcome;
    let requester = StdArc::new(FakeRequester::new(ApprovalOutcome::Timeout));
    let svc = ScopedToolService::new(confirm_registry(), BTreeSet::new())
        .with_confirmation(StdArc::clone(&requester) as _);

    let err = svc
        .execute("danger", json!({}))
        .await
        .expect_err("an unanswered card cannot run the tool");

    // The distinction is the whole point. A card nobody answered used to arrive
    // as a non-retryable `Execution` error reading "The user did not approve …
    // Do not retry" — which (a) told the model a lie about what the human did,
    // and (b) let the harness's cross-batch memo ban the call permanently,
    // contradicting `DenialLedger`, which deliberately drops a Timeout because
    // "an expired card is not a decision".
    assert!(
        matches!(err, ToolError::ApprovalExpired { .. }),
        "expected ApprovalExpired, got {err:?}"
    );
    assert!(
        err.is_retryable(),
        "an expired card must not be banned by the cross-batch failure memo"
    );
    let text = err.to_string();
    assert!(
        !text.contains("did not approve"),
        "must not speak an expiry as a refusal: {text}"
    );
}

/// Build a `TurnContext` with a unique `SessionKey` so each test isolates its
/// entry in the process-wide session approval memory.
/// `agent` MUST be unique per test.
///
/// `SessionKey::main` is deterministic, and the stores this key addresses — the
/// session approval memory and the denial ledger — are process-wide, with a
/// sticky session-pause flag. Two tests reusing one agent name therefore share a
/// bucket: a grant remembered by one satisfies the other's prompt, a denial
/// recorded by one auto-refuses the other, and three denials between them pause
/// both. The failures are order-dependent, so they surface as flakes, not as a
/// clean red test.
///
/// (The sandbox workspace tests get this for free — see `sid()` there, which
/// mints an ephemeral uuid. Here it rests on naming discipline.)
fn turn_ctx(agent: &str) -> crate::tools::turn_context::TurnContext {
    crate::tools::turn_context::TurnContext {
        session_key: crate::routing::session_key::SessionKey::main(agent),
        run_id: String::new(),
        channel_id: "test".to_string(),
        conversation_id: "conv".to_string(),
        caller_role: None,
        channel_tool_permissions: None,
        unattended: false,
        plan_gate: None,
        side_question: false,
    }
}

#[tokio::test]
async fn session_grant_skips_reprompt_within_session() {
    // An "approve for session" must be remembered for the SAME call: the first
    // prompts, the second (identical arguments) is satisfied by the memory.
    let requester = StdArc::new(FakeRequester::new(
        crate::sandbox::exec_approval::gate::ApprovalOutcome::ApprovedForSession,
    ));
    let svc = ScopedToolService::new(confirm_registry(), BTreeSet::new())
        .with_turn_context(turn_ctx("agent-session-grant"))
        .with_confirmation(StdArc::clone(&requester) as _);

    svc.execute("danger", json!({})).await.expect("first runs");
    svc.execute("danger", json!({})).await.expect("second runs");

    assert_eq!(
        requester.calls.load(Ordering::SeqCst),
        1,
        "session grant must suppress the second prompt"
    );
}

/// The invariant the action-keyed grant exists for: "allow session" grants the
/// ACTION the user read, never the tool. Fails on a name-keyed store (1 call).
#[tokio::test]
async fn session_grant_does_not_carry_to_different_arguments() {
    use crate::config::types::policies::ExecTier;
    use crate::sandbox::exec_approval::gate::ApprovalOutcome;
    let requester = StdArc::new(FakeRequester::new(ApprovalOutcome::ApprovedForSession));
    let svc = ScopedToolService::new(tier_registry(), BTreeSet::new())
        .with_exec_tier(ExecTier::Auto)
        .with_turn_context(turn_ctx("agent-grant-per-action"))
        .with_confirmation(StdArc::clone(&requester) as _);

    // The operator approves deleting a scratch file for the session…
    svc.execute(
        "file_ops",
        json!({"operation": "delete", "path": "/tmp/junk"}),
    )
    .await
    .expect("approved");
    // …which must NOT authorize deleting their home directory.
    svc.execute(
        "file_ops",
        json!({"operation": "delete", "path": "/home/u/Documents"}),
    )
    .await
    .expect("approved");

    assert_eq!(
        requester.calls.load(Ordering::SeqCst),
        2,
        "a session grant on one action must not authorize a different action of \
         the same tool — that is what threw away the tier's argument filter"
    );
}

/// The card must show WHAT will run. A `reason` that only names the tool is an
/// operator clicking "allow" on a string they cannot evaluate.
#[tokio::test]
async fn approval_card_carries_the_action_not_just_the_tool_name() {
    use crate::config::types::policies::ExecTier;
    use crate::sandbox::exec_approval::gate::ApprovalOutcome;
    let requester = StdArc::new(FakeRequester::new(ApprovalOutcome::Approved));
    let svc = ScopedToolService::new(tier_registry(), BTreeSet::new())
        .with_exec_tier(ExecTier::Auto)
        .with_turn_context(turn_ctx("agent-card-content"))
        .with_confirmation(StdArc::clone(&requester) as _);

    svc.execute(
        "file_ops",
        json!({"operation": "delete", "path": "/home/u/Documents"}),
    )
    .await
    .expect("approved");

    let summaries = requester.summaries();
    assert_eq!(summaries.len(), 1);
    assert!(
        summaries[0].contains("delete") && summaries[0].contains("/home/u/Documents"),
        "the human must see the operation and the path: {}",
        summaries[0]
    );
}

/// Arguments reach a human-visible card and a log line — a credential in them
/// must not.
#[tokio::test]
async fn secrets_in_arguments_are_redacted_before_the_human_sees_them() {
    use crate::extension::PermissionAction;
    use crate::sandbox::exec_approval::gate::ApprovalOutcome;
    const KEY: &str = "sk-ant-api03-BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB";
    let requester = StdArc::new(FakeRequester::new(ApprovalOutcome::Approved));
    let svc = ScopedToolService::new(make_registry(&["alpha"]), BTreeSet::new())
        .with_turn_context(turn_ctx("agent-redact"))
        .with_tool_permissions(perms(
            PermissionAction::Allow,
            &[("alpha", PermissionAction::Ask)],
        ))
        .with_confirmation(StdArc::clone(&requester) as _);

    svc.execute("alpha", json!({ "token": KEY }))
        .await
        .expect("approved");

    let summaries = requester.summaries();
    assert_eq!(summaries.len(), 1);
    assert!(
        !summaries[0].contains(KEY),
        "a credential must never reach the approval card: {}",
        summaries[0]
    );
}

#[tokio::test]
async fn one_shot_approval_reprompts_each_call() {
    // A plain one-shot `Approved` is NOT remembered — every call re-prompts.
    let requester = StdArc::new(FakeRequester::new(
        crate::sandbox::exec_approval::gate::ApprovalOutcome::Approved,
    ));
    let svc = ScopedToolService::new(confirm_registry(), BTreeSet::new())
        .with_turn_context(turn_ctx("agent-one-shot"))
        .with_confirmation(StdArc::clone(&requester) as _);

    svc.execute("danger", json!({})).await.expect("first runs");
    svc.execute("danger", json!({})).await.expect("second runs");

    assert_eq!(
        requester.calls.load(Ordering::SeqCst),
        2,
        "one-shot approval must re-prompt each call"
    );
}

#[tokio::test]
async fn declared_confirmation_tool_blocked_when_denied() {
    let requester = StdArc::new(FakeRequester::new(
        crate::sandbox::exec_approval::gate::ApprovalOutcome::Denied,
    ));
    let svc = ScopedToolService::new(confirm_registry(), BTreeSet::new())
        .with_confirmation(StdArc::clone(&requester) as _);

    match svc.execute("danger", json!({})).await {
        Err(ToolError::Execution { name, .. }) => assert_eq!(name, "danger"),
        other => panic!("denied confirmation must block, got: {other:?}"), // rust-doctor-disable-line panic-in-library
    }
    assert_eq!(requester.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn declared_confirmation_tool_fails_closed_without_requester() {
    // No approval transport wired → confirm-required tool must fail closed,
    // never silently auto-run.
    let svc = ScopedToolService::new(confirm_registry(), BTreeSet::new());
    match svc.execute("danger", json!({})).await {
        Err(ToolError::Execution { name, cause }) => {
            assert_eq!(name, "danger");
            assert!(
                cause.contains("approval channel is available"),
                "unexpected cause: {cause}"
            );
            // The fail-closed message names the rule too, so a run that dies
            // here says which gate it died on rather than just "no channel".
            assert!(
                cause.contains("declares its own confirmation gate"),
                "unexpected cause: {cause}"
            );
        }
        other => panic!("expected fail-closed Execution error, got: {other:?}"), // rust-doctor-disable-line panic-in-library
    }
}

#[tokio::test]
async fn plain_tool_unaffected_by_confirmation_gate() {
    // A tool that does not declare requires_confirmation runs normally even
    // with no requester wired — the change is byte-identical for it.
    let svc = ScopedToolService::new(confirm_registry(), BTreeSet::new());
    let out = svc
        .execute("plain", json!({}))
        .await
        .expect("plain tool runs");
    let ok = matches!(&out.value, Value::Object(_) | Value::String(_));
    assert!(ok, "plain tool should produce output");
}

#[tokio::test]
async fn declared_confirmation_tool_never_parallel() {
    let svc = ScopedToolService::new(confirm_registry(), BTreeSet::new());
    assert!(
        !matches!(
            svc.call_concurrency_claim("danger", &json!({})).await,
            crate::tools::concurrency::ConcurrencyClaim::Shared
        ),
        "confirm-required tool must be forced onto the serial path"
    );
    assert!(
        matches!(
            svc.call_concurrency_claim("plain", &json!({})).await,
            crate::tools::concurrency::ConcurrencyClaim::Shared
        ),
        "plain concurrent-safe tool stays parallelizable"
    );
}

#[tokio::test]
async fn describe_surfaces_requires_approval_metadata() {
    let svc = ScopedToolService::new(confirm_registry(), BTreeSet::new());
    let danger = svc.describe("danger").await.expect("danger described");
    assert!(
        danger.metadata.requires_approval,
        "describe() metadata must reflect the tool's requires_confirmation()"
    );
    let plain = svc.describe("plain").await.expect("plain described");
    assert!(!plain.metadata.requires_approval);
}

// -------------------------------------------------------------------------
// SESSION_ID scoping at the dispatch chokepoint.
//
// Regression: `code_exec` / `bash` / `code_check` refuse to run with
// "no active session context" because `current_session()` reads the
// `SESSION_ID` task-local, which the gateway path never scoped — only
// `TURN_CONTEXT` was scoped here. The dispatch chokepoint must scope BOTH
// from the turn's `session_key` so exec-class tools target the right
// per-session workspace instead of looping on a deterministic refusal.
// -------------------------------------------------------------------------

/// Tool that captures `current_session()` observed during `execute`.
struct SessionProbeTool {
    seen: StdArc<crate::sync_primitives::Mutex<Option<crate::session::service::SessionId>>>,
}

#[async_trait::async_trait]
impl LoopTool for SessionProbeTool {
    fn name(&self) -> &str {
        "session_probe"
    }
    fn description(&self) -> &str {
        "captures current_session() during execute"
    }
    fn schema(&self) -> Value {
        json!({ "type": "object" })
    }
    async fn execute(&self, _input: Value, _cancel: CancellationToken) -> LoopToolResult {
        *self.seen.lock().unwrap_or_else(|e| e.into_inner()) =
            crate::sandbox::context::current_session();
        LoopToolResult::Success { output: json!({}) }
    }
}

#[tokio::test]
async fn execute_scopes_session_id_from_turn_context() {
    let seen = StdArc::new(crate::sync_primitives::Mutex::new(None));
    let mut reg = LoopToolRegistry::new();
    reg.register(Box::new(SessionProbeTool { seen: seen.clone() }));
    let registry = Arc::new(reg);

    let sid = crate::routing::session_key::SessionKey::ephemeral("scoped-sess");
    let turn = crate::tools::turn_context::TurnContext {
        session_key: sid.clone(),
        run_id: String::new(),
        channel_id: String::new(),
        conversation_id: String::new(),
        caller_role: None,
        channel_tool_permissions: None,
        unattended: false,
        plan_gate: None,
        side_question: false,
    };
    let svc = ScopedToolService::new(registry, BTreeSet::new()).with_turn_context(turn);

    svc.execute("session_probe", json!({}))
        .await
        .expect("probe executes");

    let captured = seen.lock().unwrap_or_else(|e| e.into_inner()).clone();
    assert_eq!(
        captured.as_ref(),
        Some(&sid),
        "execute must scope SESSION_ID from turn_context so exec-class tools see the session"
    );
}

/// Without a turn context there is no session to scope — exec-class tools
/// still fall back to the no-session policy. Locks in that the new scoping
/// is gated on `turn_context` being present and never invents a session.
#[tokio::test]
async fn execute_without_turn_context_leaves_session_unset() {
    let seen = StdArc::new(crate::sync_primitives::Mutex::new(Some(
        crate::routing::session_key::SessionKey::ephemeral("stale"),
    )));
    let mut reg = LoopToolRegistry::new();
    reg.register(Box::new(SessionProbeTool { seen: seen.clone() }));
    let registry = Arc::new(reg);

    let svc = ScopedToolService::new(registry, BTreeSet::new());
    svc.execute("session_probe", json!({}))
        .await
        .expect("probe executes");

    let captured = seen.lock().unwrap_or_else(|e| e.into_inner()).clone();
    assert!(
        captured.is_none(),
        "no turn_context ⇒ no SESSION_ID scope; current_session() must be None"
    );
}

// -------------------------------------------------------------------------
// Config-tier authorization gate — chat-tier connections must be denied
// when they call operator-only (config-mutating) tools. Operator-tier and
// no-turn-context (internal/cron) runs must pass through unobstructed.
// -------------------------------------------------------------------------

#[tokio::test]
async fn chat_tier_blocked_from_config_tool() {
    use crate::routing::session_key::SessionKey;
    let registry = make_registry(&["cron_manage"]);
    let svc = ScopedToolService::new(registry, BTreeSet::new()).with_turn_context(
        crate::tools::turn_context::TurnContext {
            // Distinct from every other test's key on purpose: `main()` is
            // deterministic, and the approval stores it keys (session memory +
            // denial ledger) are process-global with a sticky session-pause
            // flag. Two tests on one key share a bucket and go order-dependent.
            session_key: SessionKey::main("agent-cfg-chat-tier"),
            run_id: String::new(),
            channel_id: String::new(),
            conversation_id: String::new(),
            caller_role: Some("guest".to_string()),
            channel_tool_permissions: None,
            unattended: false,
            plan_gate: None,
            side_question: false,
        },
    );
    let err = svc.execute("cron_manage", json!({})).await.unwrap_err();
    assert!(
        matches!(err, ToolError::PermissionDenied { .. }),
        "chat tier must be denied config tool, got {err:?}"
    );
}

#[tokio::test]
async fn operator_tier_allowed_config_tool() {
    use crate::routing::session_key::SessionKey;
    let registry = make_registry(&["cron_manage"]);
    let svc = ScopedToolService::new(registry, BTreeSet::new()).with_turn_context(
        crate::tools::turn_context::TurnContext {
            session_key: SessionKey::main("agent-cfg-operator-tier"),
            run_id: String::new(),
            channel_id: String::new(),
            conversation_id: String::new(),
            caller_role: Some("operator".to_string()),
            channel_tool_permissions: None,
            unattended: false,
            plan_gate: None,
            side_question: false,
        },
    );
    assert!(svc.execute("cron_manage", json!({})).await.is_ok());
}

#[tokio::test]
async fn no_turn_context_allows_config_tool() {
    let registry = make_registry(&["cron_manage"]);
    let svc = ScopedToolService::new(registry, BTreeSet::new());
    assert!(
        svc.execute("cron_manage", json!({})).await.is_ok(),
        "internal/non-gateway run (no turn context) must pass"
    );
}

// -------------------------------------------------------------------------
// Phase 2b: live operator sudo approval for chat-tier config tools.
//
// When `config_approval_requester` is wired, a chat-tier connection's
// attempt to run a config tool suspends for operator approval via
// `confirm_with_memory`. Approval → falls through to normal execution.
// Denial → `PermissionDenied`. No requester (None) → hard-reject (fail
// closed) — `chat_tier_blocked_from_config_tool` above already covers this.
// -------------------------------------------------------------------------

struct StubApprover(crate::sandbox::exec_approval::gate::ApprovalOutcome);

#[async_trait::async_trait]
impl crate::sandbox::exec_approval::gate::ApprovalRequester for StubApprover {
    async fn request_approval(
        &self,
        _action: &crate::sandbox::exec_approval::ApprovalAction,
    ) -> crate::sandbox::exec_approval::gate::ApprovalResponse {
        self.0.into()
    }
}

#[tokio::test]
async fn chat_tier_config_tool_approved_executes() {
    use crate::routing::session_key::SessionKey;
    use crate::sandbox::exec_approval::gate::ApprovalOutcome;
    let mut reg = LoopToolRegistry::new();
    reg.register(Box::new(StubTool {
        tool_name: "cron_manage",
    }));
    let svc = ScopedToolService::new(Arc::new(reg), BTreeSet::new())
        .with_turn_context(crate::tools::turn_context::TurnContext {
            // Distinct session key per test: confirm_with_memory writes to the
            // process-global denial_ledger / session_memory keyed by session, so
            // sharing a key across tests would let the `denied` test's ledger
            // entry auto-reject this one (order-dependent flake).
            session_key: SessionKey::main("cfg-approve-test"),
            run_id: String::new(),
            channel_id: String::new(),
            conversation_id: String::new(),
            caller_role: Some("guest".to_string()),
            channel_tool_permissions: None,
            unattended: false,
            plan_gate: None,
            side_question: false,
        })
        .with_config_approval(Arc::new(StubApprover(ApprovalOutcome::Approved)));
    assert!(
        svc.execute("cron_manage", json!({})).await.is_ok(),
        "operator-approved config tool must execute"
    );
}

#[tokio::test]
async fn chat_tier_config_tool_denied_rejected() {
    use crate::routing::session_key::SessionKey;
    use crate::sandbox::exec_approval::gate::ApprovalOutcome;
    let mut reg = LoopToolRegistry::new();
    reg.register(Box::new(StubTool {
        tool_name: "cron_manage",
    }));
    let svc = ScopedToolService::new(Arc::new(reg), BTreeSet::new())
        .with_turn_context(crate::tools::turn_context::TurnContext {
            session_key: SessionKey::main("cfg-deny-test"),
            run_id: String::new(),
            channel_id: String::new(),
            conversation_id: String::new(),
            caller_role: Some("guest".to_string()),
            channel_tool_permissions: None,
            unattended: false,
            plan_gate: None,
            side_question: false,
        })
        .with_config_approval(Arc::new(StubApprover(ApprovalOutcome::Denied)));
    let err = svc.execute("cron_manage", json!({})).await.unwrap_err();
    assert!(
        matches!(err, ToolError::PermissionDenied { .. }),
        "operator-denied config tool must be PermissionDenied, got {err:?}"
    );
}

/// A requester that denies WITH the human's stated reason attached, as the
/// channel bridge does for `/deny <reason>`.
struct ReasonedDenier(&'static str);

#[async_trait::async_trait]
impl crate::sandbox::exec_approval::gate::ApprovalRequester for ReasonedDenier {
    async fn request_approval(
        &self,
        _action: &crate::sandbox::exec_approval::ApprovalAction,
    ) -> crate::sandbox::exec_approval::gate::ApprovalResponse {
        crate::sandbox::exec_approval::gate::ApprovalResponse {
            outcome: crate::sandbox::exec_approval::gate::ApprovalOutcome::Denied,
            deny_reason: Some(self.0.to_string()),
        }
    }
}

/// `/deny <reason>` must reach the model verbatim: the human's own words are
/// the difference between a re-plan and a blind retry (hermes parity).
#[tokio::test]
async fn deny_reason_reaches_the_model_facing_error() {
    let requester = StdArc::new(ReasonedDenier("use the staging DB instead"));
    let svc = ScopedToolService::new(confirm_registry(), BTreeSet::new())
        .with_confirmation(StdArc::clone(&requester) as _);

    let err = svc
        .execute("danger", json!({}))
        .await
        .expect_err("denied → error");
    let text = err.to_string();
    assert!(
        text.contains("use the staging DB instead"),
        "the user's stated reason must be relayed verbatim, got: {text}"
    );
}

/// A tool that is BOTH operator-gated and confirm-gated, like the real
/// `vault_store` / `agent_delete` (in `OPERATOR_TOOLS` ∩
/// `CONFIRMATION_REQUIRED_TOOLS`).
struct OperatorConfirmTool;

#[async_trait::async_trait]
impl LoopTool for OperatorConfirmTool {
    fn name(&self) -> &str {
        "vault_store"
    }
    fn description(&self) -> &str {
        "operator + confirm gated stub"
    }
    fn schema(&self) -> Value {
        json!({ "type": "object" })
    }
    async fn execute(&self, _input: Value, _cancel: CancellationToken) -> LoopToolResult {
        LoopToolResult::Success {
            output: json!({ "ran": true }),
        }
    }
    fn requires_confirmation(&self) -> bool {
        true
    }
}

/// One decision per call: when the operator gate has just approved this exact
/// call (`AllowOnce`, which writes nothing into session memory), the
/// confirmation gate must NOT re-prompt it. `vault_store` sits in both gate
/// sets, so without the skip the requester's own channel would be asked to
/// confirm the action the operator already read and authorized.
#[tokio::test]
async fn operator_approval_is_not_double_prompted_by_the_confirm_gate() {
    use crate::routing::session_key::SessionKey;
    use crate::sandbox::exec_approval::gate::ApprovalOutcome;
    let mut reg = LoopToolRegistry::new();
    reg.register(Box::new(OperatorConfirmTool));

    let operator = StdArc::new(FakeRequester::new(ApprovalOutcome::Approved));
    // If the confirm gate were consulted it would DENY — so a successful run
    // proves it was skipped, and the call counter proves it was never asked.
    let own_channel = StdArc::new(FakeRequester::new(ApprovalOutcome::Denied));

    let svc = ScopedToolService::new(Arc::new(reg), BTreeSet::new())
        .with_turn_context(crate::tools::turn_context::TurnContext {
            session_key: SessionKey::main("cfg-no-double-prompt"),
            run_id: String::new(),
            channel_id: String::new(),
            conversation_id: String::new(),
            caller_role: Some("guest".to_string()),
            channel_tool_permissions: None,
            unattended: false,
            plan_gate: None,
            side_question: false,
        })
        .with_config_approval(StdArc::clone(&operator) as _)
        .with_confirmation(StdArc::clone(&own_channel) as _);

    svc.execute("vault_store", json!({"key": "k", "value": "v"}))
        .await
        .expect("operator-approved call must run without a second prompt");
    assert_eq!(
        operator.calls.load(Ordering::SeqCst),
        1,
        "the operator gate must prompt exactly once"
    );
    assert_eq!(
        own_channel.calls.load(Ordering::SeqCst),
        0,
        "the confirm gate must not re-prompt a call the operator just approved"
    );
}

/// The skip is scoped to the operator-approved call path only: an
/// operator-tier caller passes the config gate WITHOUT an approval, so the
/// confirmation gate must still fire for it.
#[tokio::test]
async fn operator_tier_caller_still_hits_the_confirm_gate() {
    use crate::routing::session_key::SessionKey;
    use crate::sandbox::exec_approval::gate::ApprovalOutcome;
    let mut reg = LoopToolRegistry::new();
    reg.register(Box::new(OperatorConfirmTool));

    let own_channel = StdArc::new(FakeRequester::new(ApprovalOutcome::Approved));
    let svc = ScopedToolService::new(Arc::new(reg), BTreeSet::new())
        .with_turn_context(crate::tools::turn_context::TurnContext {
            session_key: SessionKey::main("cfg-operator-still-confirms"),
            run_id: String::new(),
            channel_id: String::new(),
            conversation_id: String::new(),
            caller_role: Some("operator".to_string()),
            channel_tool_permissions: None,
            unattended: false,
            plan_gate: None,
            side_question: false,
        })
        .with_confirmation(StdArc::clone(&own_channel) as _);

    svc.execute("vault_store", json!({"key": "k", "value": "v"}))
        .await
        .expect("approved confirm-gated call runs");
    assert_eq!(
        own_channel.calls.load(Ordering::SeqCst),
        1,
        "an operator-tier caller skipped the config gate, so the confirm gate must still ask"
    );
}

// =============================================================================
// Tool permission policy (`[policies.tool_permissions]`) gating
// =============================================================================

fn perms(
    default: crate::extension::PermissionAction,
    overrides: &[(&str, crate::extension::PermissionAction)],
) -> crate::config::types::policies::ToolPermissionsConfig {
    crate::config::types::policies::ToolPermissionsConfig {
        default,
        overrides: overrides
            .iter()
            .map(|(n, a)| ((*n).to_string(), *a))
            .collect(),
    }
}

#[tokio::test]
async fn deny_tool_hidden_from_list_and_describe() {
    use crate::extension::PermissionAction;
    let svc = ScopedToolService::new(make_registry(&["alpha", "beta"]), BTreeSet::new())
        .with_tool_permissions(perms(
            PermissionAction::Allow,
            &[("beta", PermissionAction::Deny)],
        ));
    let names: Vec<String> = svc.list().await.into_iter().map(|d| d.name).collect();
    assert!(names.contains(&"alpha".to_string()));
    assert!(
        !names.contains(&"beta".to_string()),
        "Deny tool must be invisible to the LLM"
    );
    assert!(svc.describe("beta").await.is_none());
    // metadata_schema mirrors list().
    let schema = svc.metadata_schema();
    let schema_names: Vec<&str> = schema.iter().map(|d| d.name.as_str()).collect();
    assert!(!schema_names.contains(&"beta"));
}

#[tokio::test]
async fn deny_tool_execute_rejected_with_permission_denied() {
    use crate::extension::PermissionAction;
    let svc = ScopedToolService::new(make_registry(&["alpha", "beta"]), BTreeSet::new())
        .with_tool_permissions(perms(
            PermissionAction::Allow,
            &[("beta", PermissionAction::Deny)],
        ));
    let err = svc.execute("beta", json!({})).await.unwrap_err();
    assert!(
        matches!(err, ToolError::PermissionDenied { .. }),
        "policy-denied tool must be PermissionDenied, got {err:?}"
    );
    // Non-denied sibling still runs.
    assert!(svc.execute("alpha", json!({})).await.is_ok());
}

#[tokio::test]
async fn ask_tool_routes_through_confirmation_gate() {
    use crate::extension::PermissionAction;
    use crate::sandbox::exec_approval::gate::ApprovalOutcome;
    let requester = StdArc::new(FakeRequester::new(ApprovalOutcome::Approved));
    let svc = ScopedToolService::new(make_registry(&["alpha"]), BTreeSet::new())
        .with_turn_context(turn_ctx("agent-perm-ask"))
        .with_tool_permissions(perms(
            PermissionAction::Allow,
            &[("alpha", PermissionAction::Ask)],
        ))
        .with_confirmation(StdArc::clone(&requester) as _);
    svc.execute("alpha", json!({}))
        .await
        .expect("approved Ask tool runs");
    assert_eq!(
        requester.calls.load(Ordering::SeqCst),
        1,
        "Ask policy must prompt exactly once"
    );
}

#[tokio::test]
async fn ask_tool_without_requester_fails_closed() {
    use crate::extension::PermissionAction;
    let svc = ScopedToolService::new(make_registry(&["alpha"]), BTreeSet::new())
        .with_turn_context(turn_ctx("agent-perm-ask-closed"))
        .with_tool_permissions(perms(
            PermissionAction::Allow,
            &[("alpha", PermissionAction::Ask)],
        ));
    let err = svc.execute("alpha", json!({})).await.unwrap_err();
    assert!(
        matches!(err, ToolError::Execution { .. }),
        "Ask without approval transport must fail closed, got {err:?}"
    );
}

#[tokio::test]
async fn default_deny_exposes_only_explicit_allow() {
    use crate::extension::PermissionAction;
    let svc = ScopedToolService::new(make_registry(&["alpha", "beta"]), BTreeSet::new())
        .with_tool_permissions(perms(
            PermissionAction::Deny,
            &[("alpha", PermissionAction::Allow)],
        ));
    let names: Vec<String> = svc.list().await.into_iter().map(|d| d.name).collect();
    assert_eq!(names, vec!["alpha".to_string()]);
    let err = svc.execute("beta", json!({})).await.unwrap_err();
    assert!(matches!(err, ToolError::PermissionDenied { .. }));
}

/// An `Ask`-gated tool keeps its inner claim and may join a parallel batch:
/// its approval card correlates via the ambient `CallIdentity` (exact per
/// call), so batch exclusivity is no longer the correlation crutch it used to
/// be — the card pends concurrently with its siblings' execution.
#[tokio::test]
async fn ask_policy_no_longer_serializes_the_claim() {
    use crate::extension::PermissionAction;
    let svc =
        ScopedToolService::new(make_registry(&["alpha"]), BTreeSet::new()).with_tool_permissions(
            perms(PermissionAction::Allow, &[("alpha", PermissionAction::Ask)]),
        );
    assert!(
        matches!(
            svc.call_concurrency_claim("alpha", &json!({})).await,
            crate::tools::concurrency::ConcurrencyClaim::Shared
        ),
        "an Ask-gated tool surfaces its inner claim — the execute-time gate \
         still fires, but it no longer forces batch exclusivity"
    );
}

#[tokio::test]
async fn no_policy_means_pre_wiring_behavior() {
    // Without `with_tool_permissions`, everything lists and executes —
    // byte-identical to the pre-wiring default for unconfigured installs.
    let svc = ScopedToolService::new(make_registry(&["alpha"]), BTreeSet::new());
    assert!(svc.describe("alpha").await.is_some());
    assert!(svc.execute("alpha", json!({})).await.is_ok());
}

// =============================================================================
// Exec tier at the enforcement chokepoint (`permission_for`).
//
// Every name below is one the registry can actually emit: MCP tools are
// registered as `{server_id}__{tool}` (`McpHandler::qualified_name`), builtins
// under their own names. A tier rule that only holds for invented names holds
// for nothing.
// =============================================================================

/// Registry over the names a tier test asserts about.
fn tier_registry() -> Arc<LoopToolRegistry> {
    let mut r = LoopToolRegistry::new();
    for name in [
        "bash",
        "file_ops",
        "system",
        "browser_evaluate",
        "github__create_issue",
        "slack__send_message",
        "search",
        "memory_search",
        "web_fetch",
        "agent_delete",
    ] {
        r.register(Box::new(NamedStub::new(name)));
    }
    Arc::new(r)
}

fn tiered(tier: crate::config::types::policies::ExecTier) -> ScopedToolService {
    ScopedToolService::new(tier_registry(), BTreeSet::new()).with_exec_tier(tier)
}

#[test]
fn ask_tier_asks_for_every_mutating_tool_the_glob_table_missed() {
    use crate::config::types::policies::ExecTier;
    use crate::extension::PermissionAction;
    let svc = tiered(ExecTier::Ask);
    for name in [
        // MCP tools — registered as `{server_id}__{tool}`, never `mcp__*`.
        "github__create_issue",
        "slack__send_message",
        // The whole browser family, including arbitrary JS in the user's
        // logged-in browser.
        "browser_evaluate",
        // Plain mutators.
        "system",
        "bash",
        "file_ops",
    ] {
        assert_eq!(
            svc.permission_for(name),
            PermissionAction::Ask,
            "`{name}` mutates — the Ask tier promises it stops for a human"
        );
    }
}

#[test]
fn ask_tier_is_fail_closed_for_an_unknown_tool() {
    use crate::config::types::policies::ExecTier;
    use crate::extension::PermissionAction;
    // A tool nobody has classified (not even registered) declares nothing →
    // non-idempotent → Ask. New tools are covered on arrival.
    assert_eq!(
        tiered(ExecTier::Ask).permission_for("brand_new_tool"),
        PermissionAction::Ask
    );
}

#[test]
fn ask_tier_leaves_declared_read_only_tools_allowed() {
    use crate::config::types::policies::ExecTier;
    use crate::extension::PermissionAction;
    let svc = tiered(ExecTier::Ask);
    for name in ["search", "memory_search", "web_fetch"] {
        assert_eq!(
            svc.permission_for(name),
            PermissionAction::Allow,
            "`{name}` is a declared pure read — the model must still investigate freely"
        );
    }
}

/// A registry-shaped MCP tool that declares its own idempotency, exactly as
/// `McpRegistryTool` does from the server's `readOnlyHint` / `idempotentHint`.
/// The builtin allowlist can never speak for a name like this.
struct DeclaringMcpStub {
    name: String,
    idempotent: bool,
}
#[async_trait::async_trait]
impl LoopTool for DeclaringMcpStub {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &str {
        "mcp stub"
    }
    fn schema(&self) -> Value {
        json!({ "type": "object" })
    }
    fn is_idempotent(&self) -> bool {
        self.idempotent
    }
    async fn execute(&self, _input: Value, _cancel: CancellationToken) -> LoopToolResult {
        LoopToolResult::Success { output: json!({}) }
    }
}

#[test]
fn ask_tier_honors_an_mcp_servers_read_only_declaration() {
    use crate::config::types::policies::ExecTier;
    use crate::extension::PermissionAction;
    // `tool_facts` must read idempotency off the DECLARATION seam, not off the
    // builtin name allowlist: neither of these names can ever appear there, so
    // a name-keyed lookup answers `false` for both and the Ask tier raises a
    // card on a pure-read docs search — the prompt fatigue that makes users
    // abandon the tier.
    let mut r = LoopToolRegistry::new();
    r.register(Box::new(DeclaringMcpStub {
        name: "docs__search".to_string(),
        idempotent: true,
    }));
    r.register(Box::new(DeclaringMcpStub {
        name: "docs__publish".to_string(),
        idempotent: false,
    }));
    let svc = ScopedToolService::new(Arc::new(r), BTreeSet::new()).with_exec_tier(ExecTier::Ask);

    assert_eq!(
        svc.permission_for("docs__search"),
        PermissionAction::Allow,
        "a server-declared readOnlyHint must reach the tier rule"
    );
    assert_eq!(
        svc.permission_for("docs__publish"),
        PermissionAction::Ask,
        "an MCP tool that declares nothing stays fail-closed"
    );
}

#[test]
fn auto_tier_only_guards_the_destructive_tail() {
    use crate::config::types::policies::ExecTier;
    use crate::extension::PermissionAction;
    let svc = tiered(ExecTier::Auto);
    assert_eq!(svc.permission_for("agent_delete"), PermissionAction::Ask);
    for name in ["bash", "file_ops", "github__create_issue", "search"] {
        assert_eq!(svc.permission_for(name), PermissionAction::Allow);
    }
}

#[test]
fn full_tier_asks_for_nothing() {
    use crate::config::types::policies::ExecTier;
    use crate::extension::PermissionAction;
    let svc = tiered(ExecTier::Full);
    for name in ["bash", "agent_delete", "github__create_issue", "system"] {
        assert_eq!(svc.permission_for(name), PermissionAction::Allow);
    }
}

#[test]
fn explicit_override_wins_over_the_tier_rule() {
    use crate::config::types::policies::ExecTier;
    use crate::extension::PermissionAction;
    let svc = ScopedToolService::new(tier_registry(), BTreeSet::new())
        .with_exec_tier(ExecTier::Ask)
        .with_tool_permissions(perms(
            PermissionAction::Allow,
            &[
                // Named by the operator: a deliberate decision beats the tier.
                ("bash", PermissionAction::Allow),
                // A glob entry is explicit too.
                ("github__*", PermissionAction::Deny),
                // And an override can tighten a tool the tier left alone.
                ("search", PermissionAction::Deny),
            ],
        ));
    assert_eq!(svc.permission_for("bash"), PermissionAction::Allow);
    assert_eq!(
        svc.permission_for("github__create_issue"),
        PermissionAction::Deny
    );
    assert_eq!(svc.permission_for("search"), PermissionAction::Deny);
    // Everything the overrides don't name still answers to the tier.
    assert_eq!(
        svc.permission_for("browser_evaluate"),
        PermissionAction::Ask
    );
}

/// The tier TIGHTENS the operator's baseline; it never widens it. Before this
/// was folded into the restrictiveness lattice the tier was consulted BEFORE
/// the configured `default`, so its `Ask` verdict was returned first and a
/// `default = "deny"` install silently became ask-by-default for exactly the
/// dangerous half of the toolset — every tool the tier wanted to guard.
///
/// Production always wires a tier (`tool_service_builder`), so this — not the
/// `exec_tier: None` case — is the posture a deny-by-default operator gets.
#[tokio::test]
async fn exec_tier_never_widens_a_deny_default() {
    use crate::config::types::policies::ExecTier;
    use crate::extension::PermissionAction;
    for tier in [ExecTier::Ask, ExecTier::Auto, ExecTier::Full] {
        let svc = ScopedToolService::new(tier_registry(), BTreeSet::new())
            .with_exec_tier(tier)
            .with_tool_permissions(perms(
                PermissionAction::Deny,
                &[("search", PermissionAction::Allow)],
            ));
        // The destructive tail `Auto` would raise to `Ask`, and the mutating
        // body `Ask` would raise to `Ask`, both stay DENIED.
        for name in [
            "agent_delete",
            "bash",
            "file_ops",
            "system",
            "github__create_issue",
        ] {
            assert_eq!(
                svc.permission_for(name),
                PermissionAction::Deny,
                "{tier:?} must not widen a deny default for `{name}`"
            );
        }
        // ...and the box still exposes exactly the explicit allow.
        let names: Vec<String> = svc.list().await.into_iter().map(|d| d.name).collect();
        assert_eq!(names, vec!["search".to_string()]);
    }
}

/// An `Ask` baseline is a floor too: the tier cannot lower it back to `Allow`
/// for the read-only tools it has nothing to say about.
#[test]
fn exec_tier_never_widens_an_ask_default() {
    use crate::config::types::policies::ExecTier;
    use crate::extension::PermissionAction;
    let svc = ScopedToolService::new(tier_registry(), BTreeSet::new())
        .with_exec_tier(ExecTier::Full)
        .with_tool_permissions(perms(PermissionAction::Ask, &[]));
    for name in ["search", "memory_search", "bash"] {
        assert_eq!(
            svc.permission_for(name),
            PermissionAction::Ask,
            "`{name}`: Full has nothing to say, so the operator's Ask baseline holds"
        );
    }
}

#[tokio::test]
async fn auto_tier_asks_before_a_destructive_file_ops_call() {
    use crate::config::types::policies::ExecTier;
    use crate::sandbox::exec_approval::gate::ApprovalOutcome;
    let requester = StdArc::new(FakeRequester::new(ApprovalOutcome::Approved));
    let svc = ScopedToolService::new(tier_registry(), BTreeSet::new())
        .with_exec_tier(ExecTier::Auto)
        .with_turn_context(turn_ctx("agent-tier-fileops"))
        .with_confirmation(StdArc::clone(&requester) as _);

    // A read-shaped call runs untouched — `file_ops` is not destructive per se.
    svc.execute("file_ops", json!({"operation": "list", "path": "/tmp"}))
        .await
        .expect("list runs");
    assert_eq!(requester.calls.load(Ordering::SeqCst), 0);

    // The delete hiding behind the same tool name does stop for a human.
    svc.execute("file_ops", json!({"operation": "delete", "path": "/tmp/x"}))
        .await
        .expect("approved delete runs");
    assert_eq!(
        requester.calls.load(Ordering::SeqCst),
        1,
        "Auto promises irreversible operations ask first — including `file_ops` delete"
    );
}

/// Approval gates no longer bleed into claims: an Auto-tier destructive
/// `file_ops` call (which WILL stop for a human at execute time) surfaces the
/// inner tool's declared claim — here the stub's `Shared` — instead of a
/// forced `Global`. Correlation is the ambient `CallIdentity` the harness
/// scopes per execute future, and the pending-approval store is a keyed map,
/// so concurrently-pending cards each stamp their own call id. (The REAL
/// `file_ops` per-argument path claims — same-path conflicts, disjoint-path
/// parallelism — are pinned in `registry_adapter`'s claim tests.)
#[tokio::test]
async fn tier_gated_destructive_file_ops_surface_the_inner_claim() {
    use crate::config::types::policies::ExecTier;
    use crate::tools::concurrency::ConcurrencyClaim;
    use crate::tools::service::ToolService;

    let svc = tiered(ExecTier::Auto);
    for op in ["delete", "move", "batch_move", "organize", "list"] {
        assert_eq!(
            svc.call_concurrency_claim("file_ops", &json!({"operation": op, "path": "/tmp/a"}))
                .await,
            ConcurrencyClaim::Shared,
            "`file_ops {op}` must surface the inner declared claim — gated or \
             not, the gate no longer forces Global"
        );
    }
}

/// The config-tier operator gate no longer bleeds into claims either: gated
/// or not, the inner declared claim flows through for every caller role. The
/// gate still fires at execute time (suspend-for-operator-approval); its
/// correlation rides the ambient `CallIdentity`, so it needs no batch
/// exclusivity. `cron_manage` is on `tool_requires_operator`'s list.
#[tokio::test]
async fn approval_gates_no_longer_force_global_claims() {
    use crate::tools::concurrency::ConcurrencyClaim;
    use crate::tools::service::ToolService;

    let ctx_with_role = |role: Option<&str>| crate::tools::turn_context::TurnContext {
        session_key: crate::routing::session_key::SessionKey::main("op-gate-test"),
        run_id: String::new(),
        channel_id: "test".to_string(),
        conversation_id: "conv".to_string(),
        caller_role: role.map(String::from),
        channel_tool_permissions: None,
        unattended: false,
        plan_gate: None,
        side_question: false,
    };

    for role in [Some("guest"), Some("operator")] {
        let mut r = LoopToolRegistry::new();
        r.register(Box::new(NamedStub::new("cron_manage")));
        let svc = ScopedToolService::new(Arc::new(r), BTreeSet::new())
            .with_turn_context(ctx_with_role(role));
        assert_eq!(
            svc.call_concurrency_claim("cron_manage", &json!({})).await,
            ConcurrencyClaim::Shared,
            "the inner declared claim flows through regardless of caller role \
             ({role:?}) — approval gates correlate ambiently, not by batch \
             exclusivity"
        );
    }
}

/// Claims must judge the CANONICAL name, mirroring `execute_inner`: an alias
/// spelling (`file.ops`) must resolve to the same inner tool and yield the
/// same bounded claim the canonical spelling gets — otherwise the alias falls
/// to the conservative `Global` and over-serializes.
#[tokio::test]
async fn claims_judge_the_canonical_name_not_the_alias() {
    use crate::config::types::policies::ExecTier;
    use crate::tools::concurrency::ConcurrencyClaim;
    use crate::tools::service::ToolService;

    let svc = tiered(ExecTier::Auto);
    let input = json!({"operation": "delete", "path": "/tmp/a"});
    let via_alias = svc.call_concurrency_claim("file.ops", &input).await;
    let via_canonical = svc.call_concurrency_claim("file_ops", &input).await;
    assert_eq!(
        via_alias, via_canonical,
        "alias and canonical spellings must yield the same claim"
    );
    assert_ne!(
        via_alias,
        ConcurrencyClaim::global(),
        "…and that claim is the tool's bounded scope, not the conservative \
         fallback (which would make the equality vacuous)"
    );
}

/// The `unattended` fail-closed gate: a run nobody is watching (cron with no
/// origin channel, heartbeat, A2A delegation, goal/loop continuation) must
/// auto-deny a confirm-gated tool INSTEAD of publishing an approval card into
/// the void and parking on it until the 120 s timeout. The requester is wired
/// and would happily approve — the point is that it is never even asked.
#[tokio::test]
async fn unattended_run_auto_denies_a_confirm_gated_tool_without_prompting() {
    use crate::config::types::policies::ExecTier;
    use crate::sandbox::exec_approval::gate::ApprovalOutcome;

    let requester = StdArc::new(FakeRequester::new(ApprovalOutcome::Approved));
    let svc = ScopedToolService::new(tier_registry(), BTreeSet::new())
        .with_exec_tier(ExecTier::Auto)
        .with_turn_context(turn_ctx("agent-unattended-deny"))
        .with_confirmation(StdArc::clone(&requester) as _)
        .with_unattended(true);

    // `agent_delete` is destructive → the Auto tier raises it to `Ask`.
    let err = svc.execute("agent_delete", json!({})).await.unwrap_err();
    assert!(
        matches!(err, ToolError::Execution { .. }),
        "an unattended confirm-gated call must fail closed, got {err:?}"
    );
    assert_eq!(
        requester.calls.load(Ordering::SeqCst),
        0,
        "no card may be raised on a run with no human to answer it"
    );
    // An ungated tool still runs — the marker is a confirm-gate policy, not a
    // blanket freeze on autonomous work.
    assert!(svc.execute("search", json!({})).await.is_ok());
}

/// Every card names the rule that raised it, in the token the trail keys on.
///
/// `gate_chain`'s module doc has always called `GateRule::id` "the stable token
/// the ledger and the tests key on" — and only the tests did. A signed approval
/// row that records THAT an approval happened but not WHICH rule required it
/// cannot answer the question an auditor brings to it: whether the gate that
/// fired was one an operator could have removed. The prose in `reason` carries
/// the same fact for a human, but a sentence is not a key and gets reworded.
///
/// The two gates outside `confirmation_rule` are pinned here too: they were the
/// last places where a card's sentence was still hand-written at its call site.
#[tokio::test]
async fn every_card_names_the_rule_that_raised_it() {
    use crate::config::types::policies::ExecTier;
    use crate::sandbox::exec_approval::gate::ApprovalOutcome;

    // Tier-raised: `agent_delete` is destructive, nothing names it.
    let requester = StdArc::new(FakeRequester::new(ApprovalOutcome::Approved));
    let svc = ScopedToolService::new(tier_registry(), BTreeSet::new())
        .with_exec_tier(ExecTier::Auto)
        .with_turn_context(turn_ctx("agent-rule-id"))
        .with_confirmation(StdArc::clone(&requester) as _);
    svc.execute("agent_delete", json!({})).await.unwrap();
    assert_eq!(requester.rule_ids(), vec![Some("tier_raised")]);

    // The operator-escalation card, whose prose used to be a literal at its
    // call site — the last gate in this file that did not go through the chain.
    let operator = StdArc::new(FakeRequester::new(ApprovalOutcome::Approved));
    let mut reg = LoopToolRegistry::new();
    reg.register(Box::new(StubTool {
        tool_name: "cron_manage",
    }));
    let guest = ScopedToolService::new(Arc::new(reg), BTreeSet::new())
        .with_turn_context(crate::tools::turn_context::TurnContext {
            session_key: crate::routing::session_key::SessionKey::main("rule-id-operator-gate"),
            run_id: String::new(),
            channel_id: String::new(),
            conversation_id: String::new(),
            caller_role: Some("guest".to_string()),
            channel_tool_permissions: None,
            unattended: false,
            plan_gate: None,
            side_question: false,
        })
        .with_config_approval(StdArc::clone(&operator) as _);
    guest.execute("cron_manage", json!({})).await.unwrap();
    assert_eq!(operator.rule_ids(), vec![Some("operator_required")]);
    assert!(
        operator.seen.lock().unwrap()[0]
            .reason
            .contains("chat-tier device"),
        "the escalation card keeps its sentence — it just comes from the chain now"
    );
}

/// A refusal nobody made must not be reported as one, and must not stick.
///
/// The requester here stands in for the four production sites that answer
/// without ever showing a card: an unwired requester, an unroutable turn, a
/// Telegram delivery that failed, and a channel with no approval capability.
/// All four returned `Denied`, so the confirm gate filed a `UserRejected`:
///
/// * the intent became sticky for the rest of the session — the SECOND call
///   below never reaches the requester at all, and
/// * the sentence handed to the model (which it relays to the person it is
///   talking to) said the user had declined something they never saw.
///
/// Three of those in one conversation also crossed the brute-force threshold
/// and paused every gate in it for five minutes.
#[tokio::test]
async fn a_refusal_nobody_made_is_not_reported_as_the_users() {
    use crate::config::types::policies::ExecTier;
    use crate::sandbox::exec_approval::gate::ApprovalOutcome;

    let requester = StdArc::new(FakeRequester::new(ApprovalOutcome::Unavailable));
    let svc = ScopedToolService::new(tier_registry(), BTreeSet::new())
        .with_exec_tier(ExecTier::Auto)
        .with_turn_context(turn_ctx("agent-unreachable"))
        .with_confirmation(StdArc::clone(&requester) as _);

    let err = svc.execute("agent_delete", json!({})).await.unwrap_err();
    let text = err.to_string();
    assert!(
        !text.contains("The user did not approve"),
        "no user decided this — {text}"
    );
    assert!(
        text.contains("nobody was asked"),
        "the model has to be told WHY it was refused — {text}"
    );

    // Not sticky: the identical call asks again rather than being auto-refused
    // by the ledger on the strength of a decision that never happened.
    let _ = svc.execute("agent_delete", json!({})).await;
    assert_eq!(
        requester.calls.load(Ordering::SeqCst),
        2,
        "a transport failure must leave the intent askable"
    );
}

/// The other half of the pin: the SAME gated call on an ATTENDED run does
/// prompt. Without this, a gate that denies everything would pass the test above.
#[tokio::test]
async fn an_attended_run_still_prompts_for_the_same_call() {
    use crate::config::types::policies::ExecTier;
    use crate::sandbox::exec_approval::gate::ApprovalOutcome;

    let requester = StdArc::new(FakeRequester::new(ApprovalOutcome::Approved));
    let svc = ScopedToolService::new(tier_registry(), BTreeSet::new())
        .with_exec_tier(ExecTier::Auto)
        .with_turn_context(turn_ctx("agent-attended-prompt"))
        .with_confirmation(StdArc::clone(&requester) as _);

    svc.execute("agent_delete", json!({}))
        .await
        .expect("approved → runs");
    assert_eq!(requester.calls.load(Ordering::SeqCst), 1);
}

// -------------------------------------------------------------------------
// Deferred-tier exposure: dropped from list/metadata_schema, reachable via
// describe/execute (so a model that finds the tool via `tool_search` can
// still call it).
// -------------------------------------------------------------------------

#[tokio::test]
async fn deferred_tools_dropped_from_list_but_still_describable_and_executable() {
    // Registry with two tools; defer "beta".
    let mut reg = LoopToolRegistry::new();
    reg.register(Box::new(NamedStub::new("alpha")));
    reg.register(Box::new(NamedStub::new("beta")));
    let deferred = crate::tools::scoped::DeferredTools::new(["beta".to_string()].into());
    let svc =
        ScopedToolService::new(Arc::new(reg), BTreeSet::new()).with_deferred(deferred.clone());

    // list() and metadata_schema() omit the deferred tool.
    let names: Vec<String> = svc.list().await.into_iter().map(|d| d.name).collect();
    assert!(names.contains(&"alpha".to_string()));
    assert!(
        !names.contains(&"beta".to_string()),
        "deferred tool must not be listed"
    );
    let meta_names: Vec<String> = svc
        .metadata_schema()
        .iter()
        .map(|d| d.name.clone())
        .collect();
    assert!(!meta_names.contains(&"beta".to_string()));

    // describe() and execute() still reach it.
    assert!(
        svc.describe("beta").await.is_some(),
        "deferred tool must stay describable"
    );
    assert!(
        svc.execute("beta", json!({})).await.is_ok(),
        "deferred tool must stay executable"
    );

    // …but describable+executable is NOT callable. The model can only invoke a
    // tool that appears in `metadata_schema()` — that array IS the native
    // tool_use channel. This test used to stop above, and its comment claimed
    // "searched → callable"; nothing verified the tool ever came BACK, and
    // nothing ever put it back. `defer_mcp_tools` shipped as a trap.
    assert!(
        svc.dispatchable_list()
            .await
            .iter()
            .any(|d| d.name == "beta"),
        "a deferred tool is still dispatchable, so name-repair must be able to see it"
    );

    deferred.undefer(&["beta".to_string()]);

    let meta_names: Vec<String> = svc
        .metadata_schema()
        .iter()
        .map(|d| d.name.clone())
        .collect();
    assert!(
        meta_names.contains(&"beta".to_string()),
        "a discovered tool must re-enter the model's tool array, or it is uncallable"
    );
}

#[tokio::test]
async fn empty_deferred_set_is_byte_identical() {
    let mut reg = LoopToolRegistry::new();
    reg.register(Box::new(NamedStub::new("alpha")));
    let svc = ScopedToolService::new(Arc::new(reg), BTreeSet::new());
    let names: Vec<String> = svc.list().await.into_iter().map(|d| d.name).collect();
    assert!(names.contains(&"alpha".to_string()));
}

// -------------------------------------------------------------------------
// Ingress hygiene (Layer 2) — end-to-end through the real dispatch seam
// -------------------------------------------------------------------------

/// A tool shaped exactly like `bash`: a typed struct whose `stdout` field holds
/// the multi-line log. That shape is the whole point — `Value::to_string()`
/// escapes its newlines, so anything downstream that reasons about lines sees
/// one line.
struct FailingTestRunner;

#[async_trait::async_trait]
impl LoopTool for FailingTestRunner {
    fn name(&self) -> &str {
        "bash"
    }
    fn description(&self) -> &str {
        "runs a test suite"
    }
    fn schema(&self) -> Value {
        json!({ "type": "object" })
    }
    async fn execute(&self, _input: Value, _cancel: CancellationToken) -> LoopToolResult {
        let mut log = String::from("$ cargo test --lib\n\nrunning 2001 tests\n");
        for i in 0..2000 {
            log.push_str(&format!("test suite::case_{i} ... ok\n"));
        }
        log.push_str("test suite::the_broken_one ... FAILED\n\nfailures:\n\n");
        log.push_str("---- suite::the_broken_one stdout ----\n");
        log.push_str("thread 'suite::the_broken_one' panicked at src/widget.rs:42:9:\n");
        log.push_str("  assertion `left == right` failed\n    left: 1\n   right: 2\n\n");
        log.push_str("failures:\n    suite::the_broken_one\n\n");
        log.push_str("test result: FAILED. 2000 passed; 1 failed; 0 ignored\n");
        LoopToolResult::Success {
            output: json!({
                "success": false,
                "exit_code": 101,
                "stdout": log,
                "stderr": "",
                "language": "shell",
            }),
        }
    }
}

fn hygiene_store(
    _name: &str,
) -> (
    tempfile::TempDir,
    StdArc<crate::tools::result_store::ToolResultStore>,
) {
    let (scratch, base) = crate::utils::scratch::scratch_root();
    std::fs::create_dir_all(&base).unwrap();
    (
        scratch,
        StdArc::new(crate::tools::result_store::ToolResultStore::with_dir_for_tests(base)),
    )
}

/// The round's headline behaviour, asserted where it actually has to hold.
///
/// A failing `cargo test` run is the single most common oversized tool result in
/// this repo. Before ingress hygiene the model received the first ~400 characters
/// of the **JSON envelope** (`{"success":false,"exit_code":101,"stdout":"\n
/// running 2001 tests\ntest suite::case_0 ... ok\n…`) above a persist marker: the
/// panic, the assertion diff and the name of the failing test were all gone, so
/// the only available next move was to re-run the suite.
#[tokio::test]
async fn a_failing_test_run_reaches_the_model_as_signal_not_as_json_envelope_head() {
    let (_scratch, hygiene) = hygiene_store("failing_test_run");
    let mut registry = LoopToolRegistry::new();
    registry.register(Box::new(FailingTestRunner));
    let svc = ScopedToolService::new(StdArc::new(registry), std::collections::BTreeSet::new())
        .with_result_store(hygiene);

    let out = svc.execute("bash", json!({})).await.expect("tool succeeds");
    let text = out.value.as_str().expect("layer 2 flattens to text");

    // The three things the model needs to act, none of which survived before.
    assert!(
        text.contains("the_broken_one"),
        "the failing test's name must reach the model:\n{text}"
    );
    assert!(
        text.contains("src/widget.rs:42:9") || text.contains("panicked"),
        "the panic location must reach the model:\n{text}"
    );
    assert!(
        text.contains("2000 passed; 1 failed"),
        "the verdict line must reach the model:\n{text}"
    );

    // The passing-test noise is what paid for it.
    assert!(
        !text.contains("case_500"),
        "2000 passing-test lines must not be inlined:\n{}",
        &text[..text.len().min(400)]
    );

    // Still recoverable: the untouched original is offloaded, not the reduction.
    assert!(
        text.contains("[Full output persisted: "),
        "the recovery handle must be present:\n{text}"
    );

    // And the result is genuinely smaller than what the tool produced.
    let tokens = crate::context::budget::pressure::estimate_tokens_smart(text);
    assert!(
        tokens < 8_000,
        "the model-facing result must land inside bash's declared budget, got {tokens}"
    );
}

/// The no-op guarantee: a small result is passed through byte-for-byte, so the
/// overwhelming majority of tool calls are unaffected by any of this.
#[tokio::test]
async fn a_small_result_is_untouched_by_ingress_hygiene() {
    struct Tiny;
    #[async_trait::async_trait]
    impl LoopTool for Tiny {
        fn name(&self) -> &str {
            "bash"
        }
        fn description(&self) -> &str {
            "tiny"
        }
        fn schema(&self) -> Value {
            json!({ "type": "object" })
        }
        async fn execute(&self, _i: Value, _c: CancellationToken) -> LoopToolResult {
            LoopToolResult::Success {
                output: json!({ "stdout": "error: one line\n", "exit_code": 1 }),
            }
        }
    }
    let (_scratch, hygiene) = hygiene_store("small_untouched");
    let mut registry = LoopToolRegistry::new();
    registry.register(Box::new(Tiny));
    let svc = ScopedToolService::new(StdArc::new(registry), std::collections::BTreeSet::new())
        .with_result_store(hygiene);

    let out = svc.execute("bash", json!({})).await.unwrap();
    let text = out.value.as_str().unwrap();
    assert_eq!(
        text,
        json!({ "stdout": "error: one line\n", "exit_code": 1 }).to_string(),
        "under-budget results must be byte-identical"
    );
}

// -------------------------------------------------------------------------
// Extension usage recording at the chokepoint
//
// These assert the EFFECT (a row exists in the sidecar), not that a function
// was called: delete the `record_call_detached` line in `execute_inner` and
// they go red. A test that only counted calls would stay green if the write
// were routed to a store nobody reads.
// -------------------------------------------------------------------------

/// A tool that reports a provenance, the way `McpRegistryTool` and the plugin
/// arm of `RegistryToolAdapter` do.
struct OriginTool {
    tool_name: &'static str,
    origin: Option<(&'static str, &'static str)>,
    fails: bool,
}

#[async_trait::async_trait]
impl LoopTool for OriginTool {
    fn name(&self) -> &str {
        self.tool_name
    }
    fn description(&self) -> &str {
        "origin stub"
    }
    fn schema(&self) -> Value {
        json!({ "type": "object" })
    }
    fn is_concurrent_safe(&self, _input: &Value) -> bool {
        true
    }
    fn usage_origin(&self) -> Option<crate::tools::usage::UsageOrigin<'_>> {
        use crate::tools::usage::UsageOrigin;
        match self.origin {
            Some(("mcp", id)) => Some(UsageOrigin::Mcp(id)),
            Some(("plugin", id)) => Some(UsageOrigin::Plugin(id)),
            _ => None,
        }
    }
    async fn execute(&self, _input: Value, _cancel: CancellationToken) -> LoopToolResult {
        if self.fails {
            LoopToolResult::Error {
                error: "boom".into(),
                retryable: false,
            }
        } else {
            LoopToolResult::Success {
                output: json!({ "ok": true }),
            }
        }
    }
}

fn origin_service(tools: Vec<OriginTool>) -> ScopedToolService {
    let mut registry = LoopToolRegistry::new();
    for t in tools {
        registry.register(Box::new(t));
    }
    ScopedToolService::new(StdArc::new(registry), std::collections::BTreeSet::new())
}

fn usage_snapshot() -> std::collections::HashMap<String, crate::tools::usage::OriginUsage> {
    crate::tools::usage::ToolUsageStore::default_path()
        .map(|s| s.snapshot())
        .unwrap_or_default()
}

#[tokio::test]
async fn a_dispatched_mcp_tool_lands_in_the_usage_sidecar() {
    let _home = crate::utils::paths::IsolatedAlephHome::new();
    let svc = origin_service(vec![OriginTool {
        tool_name: "query_docs",
        origin: Some(("mcp", "ctx7")),
        fails: false,
    }]);

    svc.execute("query_docs", json!({})).await.unwrap();
    svc.execute("query_docs", json!({})).await.unwrap();

    let row = usage_snapshot()
        .remove("mcp:ctx7")
        .expect("the dispatch must have written an `mcp:ctx7` row");
    assert_eq!(row.call_count, 2);
    assert_eq!(row.error_count, 0);
    assert_eq!(row.tools.get("query_docs"), Some(&2));
    assert!(row.last_used_at.is_some());
}

#[tokio::test]
async fn a_failing_call_still_counts_as_usage_and_records_the_error() {
    let _home = crate::utils::paths::IsolatedAlephHome::new();
    let svc = origin_service(vec![OriginTool {
        tool_name: "flaky",
        origin: Some(("plugin", "p1")),
        fails: true,
    }]);

    // The call errors; the point is that the server/plugin was still exercised.
    let _ = svc.execute("flaky", json!({})).await;

    let row = usage_snapshot()
        .remove("plugin:p1")
        .expect("a failed call is still usage");
    assert_eq!(row.call_count, 1);
    assert_eq!(row.error_count, 1);
    assert!(row.last_error_at.is_some());
}

/// Builtins carry no origin, so the hot path must never touch the disk. If this
/// goes red, every `file_read` in every turn just became a locked file write.
#[tokio::test]
async fn a_builtin_writes_nothing() {
    let _home = crate::utils::paths::IsolatedAlephHome::new();
    let svc = origin_service(vec![OriginTool {
        tool_name: "file_read",
        origin: None,
        fails: false,
    }]);

    svc.execute("file_read", json!({})).await.unwrap();

    assert!(
        usage_snapshot().is_empty(),
        "a builtin must not create a usage row (or the sidecar file at all)"
    );
    assert!(
        crate::utils::paths::tool_usage_path().is_some_and(|p| !p.exists()),
        "the sidecar must not even be created by builtin traffic"
    );
}

/// A refused call is not usage: the server was never reached. It IS recorded,
/// with its reason, in the signed identity ledger — recording it here too would
/// make "40 calls" mean two different things.
#[tokio::test]
async fn a_permission_denied_call_is_not_counted_as_usage() {
    use crate::extension::PermissionAction;
    let _home = crate::utils::paths::IsolatedAlephHome::new();

    let mut registry = LoopToolRegistry::new();
    registry.register(Box::new(OriginTool {
        tool_name: "denied_tool",
        origin: Some(("mcp", "walled")),
        fails: false,
    }));
    let svc = ScopedToolService::new(StdArc::new(registry), std::collections::BTreeSet::new())
        .with_tool_permissions(perms(
            PermissionAction::Allow,
            &[("denied_tool", PermissionAction::Deny)],
        ));

    let err = svc.execute("denied_tool", json!({})).await.unwrap_err();
    assert!(matches!(err, ToolError::PermissionDenied { .. }));
    assert!(
        usage_snapshot().is_empty(),
        "a call the gate refused never reached the server; it is not usage"
    );
}

// -------------------------------------------------------------------------
// The ordered approval chain: named rules, and decisions the human already made
// -------------------------------------------------------------------------

/// PINS AN ADJUDICATED DECISION — a session grant does **not** carry into an
/// unattended continuation of the same session.
///
/// The `if self.unattended` auto-deny sits ABOVE the session-grant
/// short-circuit in `confirm_with_memory`, so an action the human cleared with
/// "allow for this session" is refused again once the same `SessionKey`
/// continues autonomously. Swapping those two blocks makes "approve once, the
/// loop stops asking" work and is the obvious-looking repair when a user
/// reports that their grant stopped applying; it was evaluated on 2026-08-07
/// and **ruled against by the user** (SECURITY.md *Unattended = fail closed*,
/// FEATURE_LOCATOR §5.3). The order IS the trust boundary: running something
/// with nobody watching must rest on a present decision, never on a remembered
/// click from earlier in the session.
///
/// Until now nothing enforced that. The ruling lived only in prose, one
/// two-block move away from being silently undone — which is exactly what
/// happened while this round was being written. This is the guard.
#[tokio::test]
async fn a_session_grant_does_not_survive_into_an_unattended_run() {
    use crate::sandbox::exec_approval::gate::ApprovalOutcome;

    let requester = StdArc::new(FakeRequester::new(ApprovalOutcome::ApprovedForSession));
    let ctx = turn_ctx("agent-grant-vs-unattended");

    // Attended turn: the human grants this exact call for the session…
    let attended = ScopedToolService::new(confirm_registry(), BTreeSet::new())
        .with_turn_context(ctx.clone())
        .with_confirmation(StdArc::clone(&requester) as _);
    attended
        .execute("danger", json!({}))
        .await
        .expect("granted");
    assert_eq!(requester.calls.load(Ordering::SeqCst), 1);
    // …and it does suppress the re-prompt while a human is still attached, so
    // the assertion below is about the unattended flag and nothing else.
    attended
        .execute("danger", json!({}))
        .await
        .expect("the grant holds within the attended session");
    assert_eq!(requester.calls.load(Ordering::SeqCst), 1);

    // The same session continues with nobody watching: refused anyway.
    let unattended = ScopedToolService::new(confirm_registry(), BTreeSet::new())
        .with_turn_context(ctx)
        .with_confirmation(StdArc::clone(&requester) as _)
        .with_unattended(true);
    let err = unattended.execute("danger", json!({})).await.unwrap_err();
    assert!(
        matches!(err, ToolError::Execution { .. }),
        "a remembered grant must not authorize an unattended run, got {err:?}"
    );
    assert_eq!(
        requester.calls.load(Ordering::SeqCst),
        1,
        "and no card was raised into the void"
    );
}

/// PINS THE SAME ADJUDICATED DECISION, one tier wider — a **persistent** grant
/// does not carry into an unattended continuation either.
///
/// The 2026-08-07 ruling was made about session grants; a grant that outlives
/// the process is strictly wider, so the reasoning applies a fortiori:
/// executing something with nobody watching must rest on a present decision,
/// not on a remembered click — and "remembered from last month" is further from
/// present than "remembered ten minutes ago", not closer.
///
/// The block order is what enforces it (`if self.unattended` precedes BOTH
/// standing-grant short-circuits, which are now one call), so this test and its
/// session-scoped sibling fail together if anybody reorders them. It is here as
/// its own case because "the persistent tier is exactly the feature people
/// reach for to make cron stop asking" is precisely the argument that would be
/// made for moving it, and the answer is to make the continuation attended.
#[tokio::test]
async fn a_persistent_grant_does_not_survive_into_an_unattended_run() {
    use crate::sandbox::exec_approval::gate::ApprovalOutcome;

    // `bash` under the `Ask` tier on an operator turn: a card that really does
    // offer the persistent tier, so this test is about the unattended flag and
    // not about the offer.
    let requester = StdArc::new(FakeRequester::new(ApprovalOutcome::ApprovedAlways));
    let ctx = turn_ctx("agent-always-vs-unattended");

    let attended = ScopedToolService::new(tier_registry(), BTreeSet::new())
        .with_exec_tier(crate::config::types::policies::ExecTier::Ask)
        .with_turn_context(ctx.clone())
        .with_confirmation(StdArc::clone(&requester) as _);
    attended
        .execute("bash", json!({"probe": "always-vs-unattended"}))
        .await
        .expect("granted");
    assert_eq!(requester.calls.load(Ordering::SeqCst), 1);
    attended
        .execute("bash", json!({"probe": "always-vs-unattended"}))
        .await
        .expect("the persistent grant holds while a human is attached");
    assert_eq!(requester.calls.load(Ordering::SeqCst), 1);

    let unattended = ScopedToolService::new(tier_registry(), BTreeSet::new())
        .with_exec_tier(crate::config::types::policies::ExecTier::Ask)
        .with_turn_context(ctx)
        .with_confirmation(StdArc::clone(&requester) as _)
        .with_unattended(true);
    let err = unattended
        .execute("bash", json!({"probe": "always-vs-unattended"}))
        .await
        .unwrap_err();
    assert!(
        matches!(err, ToolError::Execution { .. }),
        "a persistent grant must not authorize an unattended run, got {err:?}"
    );
    assert_eq!(
        requester.calls.load(Ordering::SeqCst),
        1,
        "and no card was raised into the void"
    );
}

/// The difference the persistent tier actually buys: it satisfies the gate in a
/// **different session**, where a session grant structurally cannot.
///
/// Without this the feature would be indistinguishable from the session tier in
/// a single-process test run — which is how a store that silently wrote to the
/// wrong bucket would still look green.
#[tokio::test]
async fn a_persistent_grant_satisfies_a_later_session() {
    use crate::sandbox::exec_approval::gate::ApprovalOutcome;

    let args = json!({"probe": "always-across-sessions"});
    let requester = StdArc::new(FakeRequester::new(ApprovalOutcome::ApprovedAlways));
    let first = ScopedToolService::new(tier_registry(), BTreeSet::new())
        .with_exec_tier(crate::config::types::policies::ExecTier::Ask)
        .with_turn_context(turn_ctx("agent-always-session-a"))
        .with_confirmation(StdArc::clone(&requester) as _);
    first.execute("bash", args.clone()).await.expect("granted");
    assert_eq!(requester.calls.load(Ordering::SeqCst), 1);

    // A different conversation entirely — a session grant would re-prompt here.
    let second = ScopedToolService::new(tier_registry(), BTreeSet::new())
        .with_exec_tier(crate::config::types::policies::ExecTier::Ask)
        .with_turn_context(turn_ctx("agent-always-session-b"))
        .with_confirmation(StdArc::clone(&requester) as _);
    second
        .execute("bash", args)
        .await
        .expect("the persistent grant is not session-scoped");
    assert_eq!(
        requester.calls.load(Ordering::SeqCst),
        1,
        "the second session must not have raised a card"
    );
}

/// A requester that answers wider than the card offered cannot mint a
/// persistent grant.
///
/// `ApprovalRequester` implementations return an `ApprovalOutcome` **directly**
/// — the manager's clamp only covers the ones that route through it, and that
/// trait has several. `danger` declares its own confirmation floor, so its card
/// never offers the persistent tier; an outcome claiming one is recorded as a
/// session grant instead. Observable end-to-end: the grant holds inside the
/// session it was taken in, and does NOT cross into another one.
#[tokio::test]
async fn an_outcome_wider_than_the_card_cannot_mint_a_persistent_grant() {
    use crate::sandbox::exec_approval::gate::ApprovalOutcome;

    let args = json!({"probe": "wider-than-offered"});
    let requester = StdArc::new(FakeRequester::new(ApprovalOutcome::ApprovedAlways));
    let first = ScopedToolService::new(confirm_registry(), BTreeSet::new())
        .with_turn_context(turn_ctx("agent-wider-a"))
        .with_confirmation(StdArc::clone(&requester) as _);
    first
        .execute("danger", args.clone())
        .await
        .expect("granted");
    first
        .execute("danger", args.clone())
        .await
        .expect("it is still a grant — within this session");
    assert_eq!(requester.calls.load(Ordering::SeqCst), 1);

    let second = ScopedToolService::new(confirm_registry(), BTreeSet::new())
        .with_turn_context(turn_ctx("agent-wider-b"))
        .with_confirmation(StdArc::clone(&requester) as _);
    second.execute("danger", args).await.expect("approved");
    assert_eq!(
        requester.calls.load(Ordering::SeqCst),
        2,
        "a grant the card never offered must not have outlived its session"
    );
}

/// An operator's persistent grant does not silently cover a MEMBER's identical
/// call.
///
/// The grant is install-wide by design — it is the per-call sibling of a
/// `[policies.tool_permissions]` `allow` entry — but the person clicking
/// "always" on their own card was not asked whether a member may issue the
/// byte-identical call without stopping. Since a member's card never offers the
/// tier, it is also never satisfied by it: one derivation, both directions
/// (`GrantStore::granted_within`). Without this, the operator-escalation card
/// for that call would stop being raised for everybody, permanently, from one
/// click on an unrelated surface.
#[tokio::test]
async fn an_operators_persistent_grant_does_not_cover_a_members_identical_call() {
    use crate::sandbox::exec_approval::gate::ApprovalOutcome;

    let args = json!({"probe": "always-across-tiers"});
    let operator_requester = StdArc::new(FakeRequester::new(ApprovalOutcome::ApprovedAlways));
    let operator = ScopedToolService::new(tier_registry(), BTreeSet::new())
        .with_exec_tier(crate::config::types::policies::ExecTier::Ask)
        .with_turn_context(turn_ctx("agent-always-operator"))
        .with_confirmation(StdArc::clone(&operator_requester) as _);
    operator
        .execute("bash", args.clone())
        .await
        .expect("granted");
    assert_eq!(operator_requester.calls.load(Ordering::SeqCst), 1);

    // Same action, same arguments — a member's turn. Their card never offered
    // the persistent tier, so it is not satisfied by one either: they are asked.
    let member_requester = StdArc::new(FakeRequester::new(ApprovalOutcome::Approved));
    let mut member_ctx = turn_ctx("agent-always-member");
    member_ctx.caller_role = Some("member".to_string());
    let member = ScopedToolService::new(tier_registry(), BTreeSet::new())
        .with_exec_tier(crate::config::types::policies::ExecTier::Ask)
        .with_turn_context(member_ctx)
        .with_confirmation(StdArc::clone(&member_requester) as _);
    member.execute("bash", args).await.expect("approved");
    assert_eq!(
        member_requester.calls.load(Ordering::SeqCst),
        1,
        "the member must still have been asked"
    );
}

/// A member's card must not offer the persistent tier, and the tool's own
/// declared floor must not offer it to anybody.
///
/// This is the production narrowing scenario `allowed_decisions` exists for —
/// before it, the Panel drew three fixed buttons and the field had no consumer
/// that could ever disagree with them. The set is asserted on the ACTION the
/// gate handed the requester, which is the same object every surface renders
/// from, so this covers the Panel, the channels and the TUI at once.
#[tokio::test]
async fn the_offered_decision_set_narrows_by_rule_and_by_tier() {
    use crate::exec::socket::ApprovalDecisionType;
    use crate::sandbox::exec_approval::gate::ApprovalOutcome;

    // `danger` declares its own confirmation gate (`tool_declared`): no tier,
    // no `allow` entry and no button may switch it off — including for an
    // operator, whose turn this is (`caller_role: None`).
    let requester = StdArc::new(FakeRequester::new(ApprovalOutcome::Approved));
    let operator = ScopedToolService::new(confirm_registry(), BTreeSet::new())
        .with_turn_context(turn_ctx("agent-offer-declared"))
        .with_confirmation(StdArc::clone(&requester) as _);
    operator
        .execute("danger", json!({"probe": "declared"}))
        .await
        .expect("approved");
    let offered = requester.seen.lock().unwrap()[0].allowed_decisions.clone();
    assert!(
        !offered.contains(&ApprovalDecisionType::AllowAlways),
        "the declared-floor card said no configuration can switch it off; \
         offering a permanent grant makes that sentence false as it is read"
    );

    // A tier-raised card on the same turn DOES offer it… (`bash` is a plain
    // mutator: gated by the `Ask` tier, not by the operator gate, so this
    // exercises the decision set and nothing else.)
    let requester = StdArc::new(FakeRequester::new(ApprovalOutcome::Approved));
    let operator = ScopedToolService::new(tier_registry(), BTreeSet::new())
        .with_exec_tier(crate::config::types::policies::ExecTier::Ask)
        .with_turn_context(turn_ctx("agent-offer-tier"))
        .with_confirmation(StdArc::clone(&requester) as _);
    operator
        .execute("bash", json!({"probe": "tier"}))
        .await
        .expect("approved");
    let offered = requester.seen.lock().unwrap()[0].allowed_decisions.clone();
    assert!(
        offered.contains(&ApprovalDecisionType::AllowAlways),
        "an operator-tier turn outside the declared floor may create a standing grant"
    );

    // …and the same card raised by a MEMBER does not. A persistent grant is
    // install-wide; a member creating one would authorize everybody else's
    // identical call.
    let requester = StdArc::new(FakeRequester::new(ApprovalOutcome::Approved));
    let mut member_ctx = turn_ctx("agent-offer-member");
    member_ctx.caller_role = Some("member".to_string());
    let member = ScopedToolService::new(tier_registry(), BTreeSet::new())
        .with_exec_tier(crate::config::types::policies::ExecTier::Ask)
        .with_turn_context(member_ctx)
        .with_confirmation(StdArc::clone(&requester) as _);
    member
        .execute("bash", json!({"probe": "member"}))
        .await
        .expect("approved");
    let offered = requester.seen.lock().unwrap()[0].allowed_decisions.clone();
    assert!(
        !offered.contains(&ApprovalDecisionType::AllowAlways),
        "a member's card must not offer an install-wide grant"
    );
}

/// The card must name the rule that gated the call.
///
/// `danger` declares its own confirmation gate, which no tier and no explicit
/// `allow` can switch off. The old text ("Tool `danger` requires your
/// confirmation to run") was the same sentence a stray `"*" = "ask"` glob
/// produced, and it invited the one repair that cannot work here.
#[tokio::test]
async fn the_approval_card_names_the_rule_that_gated_the_call() {
    use crate::sandbox::exec_approval::gate::ApprovalOutcome;

    let requester = StdArc::new(FakeRequester::new(ApprovalOutcome::Approved));
    let svc = ScopedToolService::new(confirm_registry(), BTreeSet::new())
        .with_turn_context(turn_ctx("agent-card-reason"))
        .with_confirmation(StdArc::clone(&requester) as _);
    svc.execute("danger", json!({})).await.expect("approved");

    let reasons: Vec<String> = requester
        .seen
        .lock()
        .unwrap()
        .iter()
        .map(|a| a.reason.clone())
        .collect();
    assert_eq!(reasons.len(), 1);
    assert!(
        reasons[0].contains("declares its own confirmation gate"),
        "the card must say which rule stopped the call: {}",
        reasons[0]
    );
    assert!(
        reasons[0].contains("full"),
        "…and that this one survives every tier: {}",
        reasons[0]
    );
}

/// A policy `deny` tells the model which entry denied it, so the sentence it
/// relays to the user names something they can actually edit.
#[tokio::test]
async fn a_policy_deny_names_the_entry_that_denied_it() {
    use crate::config::types::policies::ToolPermissionsConfig;
    use crate::extension::PermissionAction;

    let svc = ScopedToolService::new(tier_registry(), BTreeSet::new()).with_tool_permissions(
        ToolPermissionsConfig {
            default: PermissionAction::Allow,
            overrides: [("*_delete".to_string(), PermissionAction::Deny)]
                .into_iter()
                .collect(),
        },
    );
    match svc.execute("agent_delete", json!({})).await {
        Err(ToolError::PermissionDenied { reason, .. }) => {
            assert!(reason.contains("*_delete"), "unexpected reason: {reason}");
        }
        other => panic!("expected PermissionDenied, got {other:?}"), // rust-doctor-disable-line panic-in-library
    }
}

/// REGRESSION — one dispatch, two cards.
///
/// `confirm_with_memory` documents that "a grant taken at one satisfies the
/// others for the same call and the user is never double-prompted". That held
/// only for session-scoped grants: after an "allow once" at the confirm gate, a
/// `BeforeToolCall` hook's `ask` raised a SECOND card for the identical
/// fingerprint in the same dispatch.
#[tokio::test]
#[cfg(unix)] // POSIX-only: shell hook uses sh
async fn a_hook_ask_does_not_re_prompt_a_call_a_gate_already_approved() {
    use crate::sandbox::exec_approval::gate::ApprovalOutcome;

    let executor = Arc::new(HookExecutor::new(vec![make_command_hook(
        HookEvent::BeforeToolCall,
        HookKind::Interceptor,
        r#"echo '{"hookSpecificOutput": {"permissionDecision": "ask", "permissionDecisionReason": "hook wants a look"}}'"#,
    )]));
    // AllowOnce on purpose: a session grant would have masked the bug.
    let requester = StdArc::new(FakeRequester::new(ApprovalOutcome::Approved));
    let svc = ScopedToolService::new(confirm_registry(), BTreeSet::new())
        .with_turn_context(turn_ctx("agent-hook-double-prompt"))
        .with_confirmation(StdArc::clone(&requester) as _)
        .with_hook_executor(executor, "agent-hook-double-prompt");

    svc.execute("danger", json!({})).await.expect("approved");
    assert_eq!(
        requester.calls.load(Ordering::SeqCst),
        1,
        "one call, one card"
    );
}

/// …and a hook `ask` on an UNGATED call still asks. Otherwise the dedupe above
/// could be a hook seam that silently stopped working.
#[tokio::test]
#[cfg(unix)] // POSIX-only: shell hook uses sh
async fn a_hook_ask_still_prompts_when_no_gate_ran_first() {
    use crate::sandbox::exec_approval::gate::ApprovalOutcome;

    let executor = Arc::new(HookExecutor::new(vec![make_command_hook(
        HookEvent::BeforeToolCall,
        HookKind::Interceptor,
        r#"echo '{"hookSpecificOutput": {"permissionDecision": "ask", "permissionDecisionReason": "hook wants a look"}}'"#,
    )]));
    let requester = StdArc::new(FakeRequester::new(ApprovalOutcome::Approved));
    let svc = ScopedToolService::new(confirm_registry(), BTreeSet::new())
        .with_turn_context(turn_ctx("agent-hook-single-prompt"))
        .with_confirmation(StdArc::clone(&requester) as _)
        .with_hook_executor(executor, "agent-hook-single-prompt");

    // `plain` declares no gate of its own, so the hook is the only thing asking.
    svc.execute("plain", json!({})).await.expect("approved");
    assert_eq!(requester.calls.load(Ordering::SeqCst), 1);
}

// =============================================================================
// Plan mode — the read-only planning tier and the plan → build handoff.
//
// Two halves, and the split is the design: `ExecTier::Plan::rule_for` answers
// at the NAME level (so every `permission_for` consumer inherits it, including
// the slash fast path), and this service answers the per-CALL half, because it
// is the only place that holds the arguments.
// =============================================================================

/// A tool that declares nothing — i.e. mutating, on both fail-closed defaults.
///
/// `NamedStub` cannot play this part: it declares itself parallel-safe so the
/// tier tests can tell a GATE's forced `Global` from an inner claim, and under
/// plan mode that same declaration reads as "this call is a pure read".
struct MutatingStub(String);

#[async_trait::async_trait]
impl LoopTool for MutatingStub {
    fn name(&self) -> &str {
        &self.0
    }
    fn description(&self) -> &str {
        "stub that mutates"
    }
    fn schema(&self) -> Value {
        json!({ "type": "object" })
    }
    async fn execute(&self, _input: Value, _cancel: CancellationToken) -> LoopToolResult {
        LoopToolResult::Success { output: json!({}) }
    }
}

/// A read/write multiplexer shaped like the real `file_ops`: one name, a
/// `Shared` claim for its read arm and an exclusive one for everything else.
/// The per-call half of plan mode exists for exactly this shape — repo
/// exploration is what a plan is built out of, and it arrives under the same
/// tool name as `delete`.
struct MuxStub;

#[async_trait::async_trait]
impl LoopTool for MuxStub {
    fn name(&self) -> &str {
        "file_ops"
    }
    fn description(&self) -> &str {
        "read/write multiplexer"
    }
    fn schema(&self) -> Value {
        json!({ "type": "object" })
    }
    fn concurrency_claim(&self, input: &Value) -> crate::tools::concurrency::ConcurrencyClaim {
        use crate::tools::concurrency::ConcurrencyClaim;
        match input.get("operation").and_then(Value::as_str) {
            Some("list") => ConcurrencyClaim::Shared,
            _ => ConcurrencyClaim::global(),
        }
    }
    async fn execute(&self, _input: Value, _cancel: CancellationToken) -> LoopToolResult {
        LoopToolResult::Success { output: json!({}) }
    }
}

fn plan_registry() -> Arc<LoopToolRegistry> {
    let mut r = LoopToolRegistry::new();
    // Declared read-only: `NamedStub` reports parallel-safe, which is what the
    // `READ_ONLY_TOOLS` allowlist means for a real builtin.
    r.register(Box::new(NamedStub::new("file_read")));
    // Mutating, declaring nothing.
    r.register(Box::new(MutatingStub("file_write".to_string())));
    r.register(Box::new(MutatingStub("bash".to_string())));
    // The two carve-outs: the plan file and the human channel. Both mutate.
    r.register(Box::new(MutatingStub("scratchpad".to_string())));
    r.register(Box::new(MutatingStub("ask_user".to_string())));
    // The multiplexer.
    r.register(Box::new(MuxStub));
    Arc::new(r)
}

/// A planning service plus the gate a human approval would flip.
fn planning(
    restore: crate::config::types::policies::ExecTier,
) -> (ScopedToolService, StdArc<crate::tools::plan_gate::PlanGate>) {
    let gate = StdArc::new(crate::tools::plan_gate::PlanGate::new(restore));
    let mut ctx = turn_ctx("planner");
    ctx.plan_gate = Some(StdArc::clone(&gate));
    let svc = ScopedToolService::new(plan_registry(), BTreeSet::new())
        .with_exec_tier(crate::config::types::policies::ExecTier::Plan)
        .with_turn_context(ctx);
    (svc, gate)
}

/// The chokepoint must be able to SAY what it enforces, through the exact
/// chain a spawned sub-agent's calls travel.
///
/// A child gets no tool service of its own — `subagent_spawner` wraps this
/// very object (`parent_view_for_children`) in `McpScopedToolService` and
/// `AllowlistToolService` — so its prompt can only state a regime that
/// matches reality if the answer survives both wrappers. It travelled through
/// neither until this round, which is why a child spawned mid-plan was told
/// nothing about the tier that refuses every mutating call it makes.
///
/// Asserted after the release too: the answer is the LIVE gate, not a
/// snapshot. A tier threaded through `SpawnRequest` would have gone stale the
/// moment a human approved the plan.
#[tokio::test]
async fn the_enforced_tier_survives_the_wrappers_a_subagent_runs_behind() {
    use crate::config::types::policies::ExecTier;
    use crate::tools::service::ToolService;

    let (svc, gate) = planning(ExecTier::Auto);
    assert_eq!(
        svc.enforced_exec_tier(),
        Some(ExecTier::Plan),
        "the chokepoint must report what `permission_for` will actually apply"
    );

    let parent: Arc<dyn ToolService> = Arc::new(svc);
    let with_mcp: Arc<dyn ToolService> = Arc::new(
        crate::tools::mcp_scope_view::McpScopedToolService::new(parent, Vec::new()),
    );
    let child: Arc<dyn ToolService> = Arc::new(
        crate::agents::allowlist_tool_service::AllowlistToolService::new(
            with_mcp,
            StdArc::new(crate::agents::AgentDef::new(
                "explorer",
                crate::agents::AgentMode::SubAgent,
            )),
        ),
    );

    assert_eq!(
        child.enforced_exec_tier(),
        Some(ExecTier::Plan),
        "a decorator that drops this degrades the child's prompt to silence \
         about a gate that refuses every mutating call it makes"
    );

    assert!(gate.release());
    assert_eq!(
        child.enforced_exec_tier(),
        Some(ExecTier::Auto),
        "the answer must be the live gate — a snapshot taken at spawn time \
         goes stale the instant a human approves the plan"
    );
}

/// A service with no tier wired must say so, not default.
///
/// The caller renders nothing on `None`; a default here would put a regime in
/// a prompt that nothing applies — the expensive half of 判据 §0.
#[tokio::test]
async fn a_service_with_no_tier_wired_reports_none() {
    use crate::tools::service::ToolService;

    let svc = ScopedToolService::new(plan_registry(), BTreeSet::new());
    assert!(svc.enforced_exec_tier().is_none());
    assert!(
        crate::agents::allowlist_tool_service::AllowlistToolService::new(
            Arc::new(svc),
            StdArc::new(crate::agents::AgentDef::new(
                "explorer",
                crate::agents::AgentMode::SubAgent,
            )),
        )
        .enforced_exec_tier()
        .is_none(),
        "silence must propagate as silence"
    );
}

/// The tier's whole promise: nothing that mutates runs, and reads do.
#[tokio::test]
async fn planning_refuses_mutation_and_lets_reads_through() {
    let (svc, _gate) = planning(crate::config::types::policies::ExecTier::Auto);

    for name in ["file_write", "bash"] {
        let err = svc.execute(name, json!({})).await.unwrap_err();
        match err {
            ToolError::PermissionDenied { reason, .. } => {
                assert!(
                    reason.contains("PLANNING"),
                    "the refusal must name plan mode, not a config entry: {reason}"
                );
                assert!(
                    reason.contains("request_approval"),
                    "and it must name the way out: {reason}"
                );
            }
            other => panic!("`{name}` must be refused while planning, got {other:?}"),
        }
    }
    svc.execute("file_read", json!({}))
        .await
        .expect("a declared read runs while planning");
}

/// The two carve-outs. Without them the tier is circular: you would need
/// approval to ask for approval, and could not write the plan the approval is
/// about.
#[tokio::test]
async fn planning_keeps_the_plan_file_and_the_human_channel_open() {
    let (svc, _gate) = planning(crate::config::types::policies::ExecTier::Auto);
    for name in ["scratchpad", "ask_user"] {
        svc.execute(name, json!({}))
            .await
            .unwrap_or_else(|e| panic!("`{name}` must stay reachable while planning: {e:?}"));
    }
}

/// The per-call half. One tool name, two answers, decided by the arguments —
/// which is why it cannot live in `rule_for`, and why `file_ops list` (the
/// exploration a plan is built out of) is not collateral damage.
#[tokio::test]
async fn planning_admits_the_read_arm_of_a_multiplexer_and_refuses_the_rest() {
    let (svc, _gate) = planning(crate::config::types::policies::ExecTier::Auto);
    svc.execute("file_ops", json!({ "operation": "list" }))
        .await
        .expect("the read arm explores");
    let err = svc
        .execute("file_ops", json!({ "operation": "delete" }))
        .await
        .unwrap_err();
    assert!(matches!(err, ToolError::PermissionDenied { .. }));
    // A missing `operation` is not a read: the claim degrades to exclusive,
    // which is the fail-closed direction.
    assert!(svc.execute("file_ops", json!({})).await.is_err());
}

/// The handoff itself: the SAME service, the SAME call, before and after a
/// human approval. This is what "the harness switches modes" means — no new
/// turn, no re-resolution, and not one line inside `src/harness/`.
#[tokio::test]
async fn approving_the_plan_lets_the_next_call_build() {
    let (svc, gate) = planning(crate::config::types::policies::ExecTier::Auto);
    assert!(svc.execute("file_write", json!({})).await.is_err());

    assert!(gate.release(), "first release wins");
    svc.execute("file_write", json!({}))
        .await
        .expect("the very next call after approval runs — that is the handoff");
    svc.execute("bash", json!({}))
        .await
        .expect("and so does everything else the restore tier allows");
}

/// A planning turn shows the model its whole toolbelt. A plan that another
/// agent could implement needs the vocabulary of what can be done, and hiding
/// half of it for the planning half of a turn would also swap the cached tools
/// block twice per plan.
#[tokio::test]
async fn planning_still_lists_the_tools_it_refuses() {
    let (svc, _gate) = planning(crate::config::types::policies::ExecTier::Auto);
    let names: Vec<String> = svc.list().await.into_iter().map(|d| d.name).collect();
    for expected in ["file_write", "bash", "file_ops", "file_read", "scratchpad"] {
        assert!(
            names.iter().any(|n| n == expected),
            "`{expected}` must stay listed while planning: {names:?}"
        );
    }
    assert!(svc.describe("file_write").await.is_some());
}

/// An operator's own `deny` is not plan mode wearing a hat: it stays hidden,
/// it keeps reporting itself, and approving a plan does not lift it.
#[tokio::test]
async fn an_operator_deny_survives_the_plan_and_still_reports_itself() {
    use crate::extension::PermissionAction;

    let gate = StdArc::new(crate::tools::plan_gate::PlanGate::new(
        crate::config::types::policies::ExecTier::Auto,
    ));
    let mut ctx = turn_ctx("planner-denied");
    ctx.plan_gate = Some(StdArc::clone(&gate));
    let svc = ScopedToolService::new(plan_registry(), BTreeSet::new())
        .with_exec_tier(crate::config::types::policies::ExecTier::Plan)
        .with_turn_context(ctx)
        .with_tool_permissions(perms(
            PermissionAction::Allow,
            &[("bash", PermissionAction::Deny)],
        ));

    let names: Vec<String> = svc.list().await.into_iter().map(|d| d.name).collect();
    assert!(
        !names.iter().any(|n| n == "bash"),
        "a real deny stays hidden"
    );

    let err = svc.execute("bash", json!({})).await.unwrap_err();
    let ToolError::PermissionDenied { reason, .. } = err else {
        panic!("expected a denial")
    };
    assert!(
        reason.contains("tool permission policy"),
        "an operator's deny must keep naming itself, not plan mode: {reason}"
    );

    gate.release();
    assert!(
        svc.execute("bash", json!({})).await.is_err(),
        "approving a plan lifts the PLAN gate, not the operator's policy"
    );
}

/// A run builds its tool service TWICE — once for itself, once as the parent
/// view handed to spawned children (`parent_view_for_children`). Both are
/// built from the same turn context, so both hold the same gate: one human
/// approval lifts planning for the run AND for anything it spawned.
///
/// This is the property that makes `subagent` admissible while planning at all
/// — a child with its own gate would be a child no approval could ever
/// release, and a child with no gate would be the hole.
#[tokio::test]
async fn every_service_built_from_one_turn_shares_one_gate() {
    let gate = StdArc::new(crate::tools::plan_gate::PlanGate::new(
        crate::config::types::policies::ExecTier::Auto,
    ));
    let mut ctx = turn_ctx("planner-fanout");
    ctx.plan_gate = Some(StdArc::clone(&gate));

    let build = || {
        ScopedToolService::new(plan_registry(), BTreeSet::new())
            .with_exec_tier(crate::config::types::policies::ExecTier::Plan)
            .with_turn_context(ctx.clone())
    };
    let own = build();
    let child_view = build();

    assert!(own.execute("file_write", json!({})).await.is_err());
    assert!(child_view.execute("file_write", json!({})).await.is_err());

    gate.release();

    own.execute("file_write", json!({})).await.expect("parent");
    child_view
        .execute("file_write", json!({}))
        .await
        .expect("a child built from the same turn is released by the same approval");
}

/// A turn with no plan gate is byte-identical to a build with no plan mode:
/// the same service at the same tier answers exactly as it always has.
#[tokio::test]
async fn a_turn_with_no_plan_gate_is_unchanged() {
    let svc = ScopedToolService::new(plan_registry(), BTreeSet::new())
        .with_exec_tier(crate::config::types::policies::ExecTier::Auto)
        .with_turn_context(turn_ctx("builder"));
    svc.execute("file_write", json!({}))
        .await
        .expect("Auto runs mutating tools");
    let names: Vec<String> = svc.list().await.into_iter().map(|d| d.name).collect();
    assert!(names.iter().any(|n| n == "file_write"));
}

/// A service resolved to `Plan` for the given `side_question` flag. `Plan` is
/// what every `/btw` turn composes to (`ExecTier::most_restrictive`), so the
/// carve-out revocation is only meaningful measured against it.
fn side_question_service(side_question: bool) -> ScopedToolService {
    let mut ctx = turn_ctx("side-question");
    ctx.side_question = side_question;
    ScopedToolService::new(plan_registry(), BTreeSet::new())
        .with_exec_tier(crate::config::types::policies::ExecTier::Plan)
        .with_turn_context(ctx)
}

/// The two `Plan` carve-outs are revoked for a side question, and the
/// revocation is DERIVED from `PLAN_REACHABLE_TOOLS` rather than restating
/// it. A third member added to that constant is denied for btw automatically
/// — the safe direction — and this test names it so the author must confirm.
#[test]
fn a_side_question_revokes_every_plan_carve_out() {
    use crate::config::types::policies::PLAN_REACHABLE_TOOLS;
    use crate::extension::PermissionAction;

    for tool in PLAN_REACHABLE_TOOLS {
        let svc = side_question_service(true);
        assert_eq!(
            svc.permission_for(tool),
            PermissionAction::Deny,
            "{tool} is reachable under Plan but must not be during a side question"
        );
    }

    // Control: without the side-question flag the carve-outs still hold, so
    // this test cannot pass by breaking Plan mode itself.
    for tool in PLAN_REACHABLE_TOOLS {
        let svc = side_question_service(false);
        assert_ne!(svc.permission_for(tool), PermissionAction::Deny, "{tool}");
    }
}

/// A mutating tool is refused during a side question, and the reason names
/// the side question rather than the plan handoff — pointing the reader at
/// "get your plan approved" would name a repair that cannot work here.
#[test]
fn a_side_question_refusal_names_itself_not_the_plan_handoff() {
    let svc = side_question_service(true);
    let rule = svc.deny_rule("file_write").expect("file_write is refused");
    assert!(
        matches!(rule, super::gate_chain::GateRule::SideQuestion),
        "expected SideQuestion, got {rule:?}"
    );
}
