//! ScopedToolService — bridges LoopToolRegistry + SubagentTool to ToolService.
//!
//! Adapts the Gateway-side `LoopToolRegistry` (with optional SubagentTool,
//! ToolRefreshSource, and hook decorator) to the `ToolService` trait consumed
//! by `AgentHarness`. This is a read-only adapter; it does not modify the
//! underlying registry.

use std::collections::BTreeSet;

use async_trait::async_trait;
use serde_json::Value;

use crate::agents::subagent_tool::SubagentTool;
use crate::session::events::ToolOutput;
use crate::sync_primitives::Arc;
use crate::tools::refresh::ToolRefreshSource;
use crate::tools::runtime::{LoopTool, LoopToolRegistry};
use crate::tools::service::{
    ToolDefinition, ToolDefinitionMetadata, ToolError, ToolService, ToolSource,
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
        }
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

        // Apply allowed-set filter.
        if !self.allowed.is_empty() {
            defs.retain(|d| self.allowed.contains(&d.name));
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
        // Enforce allowed filter.
        if !self.is_allowed(name) {
            return Err(ToolError::NotFound {
                name: name.to_string(),
            });
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
}
