//! ScopedToolService — bridges LoopToolRegistry + SubagentTool to ToolService.
//!
//! Adapts the Gateway-side `LoopToolRegistry` (with optional SubagentTool,
//! ToolRefreshSource, and hook decorator) to the `ToolService` trait consumed
//! by `AgentHarness`. This is a read-only adapter; it does not modify the
//! underlying registry.

use std::collections::BTreeSet;

use arc_swap::ArcSwap;
use async_trait::async_trait;
use serde_json::Value;

use crate::agents::subagent_tool::SubagentTool;
use crate::sandbox::exec_approval::gate::{ApprovalOutcome, ApprovalRequester};
use crate::session::events::ToolOutput;
use crate::sync_primitives::Arc;
use crate::tools::refresh::ToolRefreshSource;
use crate::tools::runtime::{LoopTool, LoopToolRegistry};
use crate::tools::service::{
    to_dispatcher_form, ToolDefinition, ToolDefinitionMetadata, ToolError, ToolService, ToolSource,
};

// =============================================================================
// ToolHookDecorator trait
// =============================================================================

/// Optional hook that wraps tool execution.
///
/// Implementations receive the tool name and input before execution and the
/// output/error after. This is intentionally minimal — the hook cannot cancel
/// or modify the call, only observe it.
pub trait ToolHookDecorator: Send + Sync {
    /// Called immediately before a tool is invoked.
    fn before_execute(&self, name: &str, input: &Value);

    /// Called after a tool invocation completes (success or error).
    fn after_execute(&self, name: &str, output: &Result<ToolOutput, ToolError>);
}

// =============================================================================
// ScopedToolService
// =============================================================================

/// Adapts a `LoopToolRegistry` snapshot to the `ToolService` consumer trait.
///
/// Construction:
/// ```text
/// ScopedToolService::new(registry, allowed)
///     .with_subagent_tool(tool)
///     .with_refresh(refresh_source)
///     .with_hook_decorator(decorator)
/// ```
///
/// `allowed` is a set of permitted tool names. Empty = allow-all.
pub struct ScopedToolService {
    inner: Arc<LoopToolRegistry>,
    allowed: BTreeSet<String>,
    subagent_tool: Option<Arc<SubagentTool>>,
    refresh: Option<Arc<dyn ToolRefreshSource>>,
    hook_decorator: Option<Arc<dyn ToolHookDecorator>>,
    /// Tool names that require user confirmation before they execute.
    confirm_tools: BTreeSet<String>,
    /// Transport used to obtain that confirmation. `None` = no approval
    /// channel wired, so confirm-required tools fail closed (denied).
    approval_requester: Option<Arc<dyn ApprovalRequester>>,
    /// Routing context of the agent turn this service serves. When set,
    /// `execute()` scopes it into the `TURN_CONTEXT` task-local so HITL tools
    /// (sandbox escalation, `requires_confirmation`, `ask_user`) can route a
    /// prompt back to the originating channel.
    turn_context: Option<crate::tools::turn_context::TurnContext>,
    schema_cache: ArcSwap<Option<(u64, Arc<[crate::dispatcher::ToolDefinition]>)>>,
    cache_generation: std::sync::atomic::AtomicU64,
}

impl ScopedToolService {
    /// Create a new `ScopedToolService`.
    ///
    /// `allowed` — set of tool names visible through this service. Empty = allow all.
    pub fn new(inner: Arc<LoopToolRegistry>, allowed: BTreeSet<String>) -> Self {
        Self {
            inner,
            allowed,
            subagent_tool: None,
            refresh: None,
            hook_decorator: None,
            confirm_tools: BTreeSet::new(),
            approval_requester: None,
            turn_context: None,
            schema_cache: ArcSwap::from_pointee(None),
            cache_generation: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Require user confirmation for the named tools before they execute.
    ///
    /// When a tool in `confirm_tools` is invoked, `execute()` first routes a
    /// confirmation request through `requester`; the tool runs only on an
    /// `Approved` outcome. With no requester wired, confirm-required tools
    /// fail closed.
    pub fn with_confirmation(
        mut self,
        confirm_tools: BTreeSet<String>,
        requester: Arc<dyn ApprovalRequester>,
    ) -> Self {
        self.confirm_tools = confirm_tools;
        self.approval_requester = Some(requester);
        self
    }

    /// Attach the routing context of the agent turn this service serves.
    ///
    /// `execute()` scopes it into the `TURN_CONTEXT` task-local for the
    /// duration of every tool call, letting HITL tools route a prompt back to
    /// the originating channel.
    pub fn with_turn_context(mut self, ctx: crate::tools::turn_context::TurnContext) -> Self {
        self.turn_context = Some(ctx);
        self
    }

    /// Attach a `SubagentTool` that will appear in listings and can be executed.
    pub fn with_subagent_tool(mut self, tool: Arc<SubagentTool>) -> Self {
        self.subagent_tool = Some(tool);
        self
    }

    /// Attach a refresh source. `list()` will trigger a poll on each call.
    ///
    /// Note: because `LoopToolRegistry` is shared via `Arc`, callers that need
    /// live refresh should rebuild the registry externally and swap the `Arc`.
    /// This hook is provided for compatibility with the plan interface.
    pub fn with_refresh(mut self, refresh: Arc<dyn ToolRefreshSource>) -> Self {
        self.refresh = Some(refresh);
        self
    }

    /// Attach a hook decorator for observing tool execution.
    pub fn with_hook_decorator(mut self, hook: Arc<dyn ToolHookDecorator>) -> Self {
        self.hook_decorator = Some(hook);
        self
    }

    // -------------------------------------------------------------------------
    // Helpers
    // -------------------------------------------------------------------------

    fn is_allowed(&self, name: &str) -> bool {
        // Attached SubagentTool always passes the allow filter. It is appended
        // to listings independently of `allowed` (which is derived from the
        // builtin tool registry — subagent isn't registered there), so without
        // this exception `list()` / `dispatcher_schema()` / `execute()` would
        // hide subagent from the LLM whenever a non-empty allow set was
        // configured (i.e. every real gateway path).
        if self
            .subagent_tool
            .as_ref()
            .is_some_and(|st| st.name() == name)
        {
            return true;
        }
        self.allowed.is_empty() || self.allowed.contains(name)
    }

    fn loop_tool_to_definition(tool: &dyn LoopTool) -> ToolDefinition {
        ToolDefinition {
            name: tool.name().to_string(),
            description: tool.description().to_string(),
            input_schema: tool.schema(),
            source: ToolSource::Builtin,
            metadata: ToolDefinitionMetadata::default(),
        }
    }

    fn subagent_definition(tool: &SubagentTool) -> ToolDefinition {
        ToolDefinition {
            name: tool.name().to_string(),
            description: tool.description().to_string(),
            input_schema: tool.schema(),
            source: ToolSource::Builtin,
            metadata: ToolDefinitionMetadata::default(),
        }
    }

    fn tool_result_to_output(
        name: &str,
        result: crate::tools::runtime::ToolResult,
    ) -> Result<ToolOutput, ToolError> {
        use crate::session::events::ToolOutputMetadata;
        use crate::tools::runtime::ToolResult;
        match result {
            ToolResult::Success { output } | ToolResult::SuccessAndStopLoop { output } => {
                Ok(ToolOutput {
                    value: output,
                    metadata: ToolOutputMetadata::default(),
                })
            }
            ToolResult::Error { error, .. } => Err(ToolError::Execution {
                name: name.to_string(),
                cause: error,
            }),
        }
    }
}

#[async_trait]
impl ToolService for ScopedToolService {
    async fn list(&self) -> Vec<ToolDefinition> {
        // Trigger refresh poll if a source is configured.
        if let Some(ref refresh) = self.refresh {
            if refresh.poll_changes() {
                // Refresh signals that tools changed externally. The registry
                // is shared via Arc so callers needing live data swap it; here
                // we just acknowledge the signal without mutating our snapshot.
                let _ = refresh.fetch_tools();
            }
        }

        let mut defs: Vec<ToolDefinition> = self
            .inner
            .tool_definitions()
            .into_iter()
            .map(|d| ToolDefinition {
                name: d.name,
                description: d.description,
                input_schema: d.parameters,
                source: ToolSource::Builtin,
                metadata: ToolDefinitionMetadata::default(),
            })
            .collect();

        // Append subagent tool if configured.
        if let Some(ref st) = self.subagent_tool {
            defs.push(Self::subagent_definition(st.as_ref()));
        }

        // Apply allowed-set filter. `is_allowed` exempts the attached
        // subagent so it survives the retain even when `allowed` is non-empty
        // and doesn't list "subagent" (which it never does — see is_allowed).
        if !self.allowed.is_empty() {
            defs.retain(|d| self.is_allowed(&d.name));
        }

        defs
    }

    async fn describe(&self, name: &str) -> Option<ToolDefinition> {
        // Enforce allowed filter first.
        if !self.is_allowed(name) {
            return None;
        }

        // Check subagent tool.
        if let Some(ref st) = self.subagent_tool {
            if st.name() == name {
                return Some(Self::subagent_definition(st.as_ref()));
            }
        }

        // Check inner registry.
        self.inner.get(name).map(Self::loop_tool_to_definition)
    }

    async fn execute(&self, name: &str, input: Value) -> Result<ToolOutput, ToolError> {
        // Scope the turn's routing context so HITL tools (sandbox escalation,
        // `requires_confirmation`, `ask_user`) can reach the originating
        // channel. Scoped here — the immediate caller of every tool's
        // `execute` — so it stays visible without crossing a `tokio::spawn`.
        match self.turn_context.clone() {
            Some(turn) => {
                crate::tools::turn_context::TURN_CONTEXT
                    .scope(turn, self.execute_inner(name, input))
                    .await
            }
            None => self.execute_inner(name, input).await,
        }
    }

    fn dispatcher_schema(&self) -> std::sync::Arc<[crate::dispatcher::ToolDefinition]> {
        use std::sync::atomic::Ordering;

        // Bump generation if the refresh source signals external changes.
        if let Some(ref refresh) = self.refresh {
            if refresh.poll_changes() {
                let _ = refresh.fetch_tools();
                self.cache_generation.fetch_add(1, Ordering::AcqRel);
            }
        }
        let gen_now = self.cache_generation.load(Ordering::Acquire);

        // Cache hit?
        if let Some(ref cached) = **self.schema_cache.load() {
            if cached.0 == gen_now {
                return Arc::clone(&cached.1);
            }
        }

        // Cache miss: rebuild loop-side defs (matching list() body), then convert.
        let mut defs: Vec<ToolDefinition> = self
            .inner
            .tool_definitions()
            .into_iter()
            .map(|d| ToolDefinition {
                name: d.name,
                description: d.description,
                input_schema: d.parameters,
                source: ToolSource::Builtin,
                metadata: ToolDefinitionMetadata::default(),
            })
            .collect();
        if let Some(ref st) = self.subagent_tool {
            defs.push(Self::subagent_definition(st.as_ref()));
        }
        if !self.allowed.is_empty() {
            // Mirror list() — subagent is exempt from allow-filter via is_allowed.
            defs.retain(|d| self.is_allowed(&d.name));
        }
        let schema = to_dispatcher_form(&defs);
        self.schema_cache
            .store(Arc::new(Some((gen_now, Arc::clone(&schema)))));
        schema
    }
}

impl ScopedToolService {
    /// Tool dispatch proper. Wrapped by the `ToolService::execute` trait
    /// method, which scopes `TURN_CONTEXT` around it.
    async fn execute_inner(
        &self,
        name: &str,
        input: Value,
    ) -> Result<ToolOutput, ToolError> {
        // Enforce allowed filter.
        if !self.is_allowed(name) {
            return Err(ToolError::NotFound {
                name: name.to_string(),
            });
        }

        // Confirmation gate: tools flagged `requires_confirmation` must be
        // approved by the user before they run. Fails closed when no approval
        // transport is wired.
        if self.confirm_tools.contains(name) {
            match &self.approval_requester {
                Some(requester) => {
                    let reason =
                        format!("Tool `{name}` requires your confirmation to run.");
                    let outcome = requester.request_approval(name, &reason).await;
                    if outcome != ApprovalOutcome::Approved {
                        return Err(ToolError::Execution {
                            name: name.to_string(),
                            cause: format!(
                                "User did not approve running `{name}` ({outcome:?}). \
                                 Do not retry; ask the user how to proceed."
                            ),
                        });
                    }
                }
                None => {
                    return Err(ToolError::Execution {
                        name: name.to_string(),
                        cause: format!(
                            "Tool `{name}` requires confirmation but no approval \
                             channel is available. Do not retry."
                        ),
                    });
                }
            }
        }

        // Fire pre-hook.
        if let Some(ref hook) = self.hook_decorator {
            hook.before_execute(name, &input);
        }

        // Route to subagent tool if name matches.
        let result = if self
            .subagent_tool
            .as_ref()
            .is_some_and(|st| st.name() == name)
        {
            let st = self.subagent_tool.as_ref().unwrap();
            let raw = st.execute(input).await;
            Self::tool_result_to_output(name, raw)
        } else if self.inner.get(name).is_some() || self.inner.resolve(name).is_some() {
            let raw = self.inner.execute(name, input).await;
            Self::tool_result_to_output(name, raw)
        } else {
            Err(ToolError::NotFound {
                name: name.to_string(),
            })
        };

        // Fire post-hook.
        if let Some(ref hook) = self.hook_decorator {
            hook.after_execute(name, &result);
        }

        result
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::refresh::ToolRefreshSource;
    use crate::tools::runtime::{LoopTool, LoopToolRegistry, ToolResult as LoopToolResult};
    use serde_json::{json, Value};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc as StdArc;

    // -------------------------------------------------------------------------
    // Stubs
    // -------------------------------------------------------------------------

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
        fn dispatcher_schema(&self) -> std::sync::Arc<[crate::dispatcher::ToolDefinition]> {
            std::sync::Arc::from([])
        }
    }

    fn in_mem_session() -> Arc<dyn crate::session::service::SessionService> {
        use crate::session::in_process::InProcessActorSessionService;
        use crate::session::store::{
            migrate_add_session_events, SessionEventStore, SqliteEventStore,
        };
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
        async fn execute(&self, _input: Value) -> LoopToolResult {
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

    // Stub refresh source that records whether fetch was called.
    struct StubRefresh {
        has_changes: AtomicBool,
        fetched: StdArc<AtomicBool>,
    }

    impl StubRefresh {
        fn new(has_changes: bool, fetched: StdArc<AtomicBool>) -> Self {
            Self {
                has_changes: AtomicBool::new(has_changes),
                fetched,
            }
        }
    }

    impl ToolRefreshSource for StubRefresh {
        fn poll_changes(&self) -> bool {
            self.has_changes.load(Ordering::Acquire)
        }
        fn fetch_tools(&self) -> Vec<Box<dyn LoopTool>> {
            self.fetched.store(true, Ordering::Release);
            vec![]
        }
    }

    // Stub hook decorator that counts calls.
    struct StubHook {
        before_count: StdArc<AtomicUsize>,
        after_count: StdArc<AtomicUsize>,
    }

    impl ToolHookDecorator for StubHook {
        fn before_execute(&self, _name: &str, _input: &Value) {
            self.before_count.fetch_add(1, Ordering::Relaxed);
        }
        fn after_execute(&self, _name: &str, _output: &Result<ToolOutput, ToolError>) {
            self.after_count.fetch_add(1, Ordering::Relaxed);
        }
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
            Arc::new(crate::sandbox::NoopSandbox),
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
    }

    // -------------------------------------------------------------------------
    // Test 2b: subagent survives a non-empty allow set (production path).
    //
    // Regression for the gateway run_loop wiring: `allowed_names` is built
    // from the builtin tool registry's tool definitions, which never contains
    // "subagent" (SubagentTool is attached on top of the registry). Before
    // the is_allowed exemption, list / describe / execute / dispatcher_schema
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
            Arc::new(crate::sandbox::NoopSandbox),
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

        // (3) dispatcher_schema (LLM-facing) includes subagent
        let schema_names: Vec<String> = svc
            .dispatcher_schema()
            .iter()
            .map(|t| t.name.clone())
            .collect();
        assert!(
            schema_names.iter().any(|n| n == "subagent"),
            "dispatcher_schema must include subagent; got {:?}",
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
    // Test 3: list triggers refresh on first call (when poll_changes is true)
    // -------------------------------------------------------------------------
    #[tokio::test]
    async fn list_triggers_refresh_on_first_call() {
        let fetched = StdArc::new(AtomicBool::new(false));
        let refresh = Arc::new(StubRefresh::new(true, StdArc::clone(&fetched)));

        let registry = make_registry(&["tool_a"]);
        let svc = ScopedToolService::new(registry, BTreeSet::new()).with_refresh(refresh);

        svc.list().await;
        assert!(
            fetched.load(Ordering::Acquire),
            "fetch_tools must be called when poll_changes returns true"
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
            Arc::new(crate::sandbox::NoopSandbox),
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
    // Test 5: execute applies hook decorator (both before and after fire)
    // -------------------------------------------------------------------------
    #[tokio::test]
    async fn execute_applies_hook_decorator() {
        let before = StdArc::new(AtomicUsize::new(0));
        let after = StdArc::new(AtomicUsize::new(0));
        let hook = Arc::new(StubHook {
            before_count: StdArc::clone(&before),
            after_count: StdArc::clone(&after),
        });

        let registry = make_registry(&["read_file"]);
        let svc = ScopedToolService::new(registry, BTreeSet::new()).with_hook_decorator(hook);

        let _ = svc.execute("read_file", json!({})).await;

        assert_eq!(
            before.load(Ordering::Relaxed),
            1,
            "before_execute must fire once"
        );
        assert_eq!(
            after.load(Ordering::Relaxed),
            1,
            "after_execute must fire once"
        );
    }

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
    // Test 7: dispatcher_schema caches when no refresh signal
    // -------------------------------------------------------------------------

    #[test]
    fn scoped_dispatcher_schema_caches_when_no_refresh_signal() {
        let registry = make_registry(&["a", "b"]);
        let svc = ScopedToolService::new(registry, BTreeSet::new());
        let s1 = svc.dispatcher_schema();
        let s2 = svc.dispatcher_schema();
        assert!(
            std::sync::Arc::ptr_eq(&s1, &s2),
            "without refresh signal cache should hold across calls"
        );
        assert_eq!(s1.len(), 2);
    }

    // -------------------------------------------------------------------------
    // Test 8: dispatcher_schema respects allowed filter
    // -------------------------------------------------------------------------

    #[test]
    fn scoped_dispatcher_schema_respects_allowed_filter() {
        let registry = make_registry(&["a", "b"]);
        let mut allowed = BTreeSet::new();
        allowed.insert("a".to_string());
        let svc = ScopedToolService::new(registry, allowed);
        let s = svc.dispatcher_schema();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].name, "a");
    }
}
