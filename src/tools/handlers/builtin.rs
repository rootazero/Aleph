//! `BuiltinHandler` — wraps `AlephToolDyn` for `ToolHandler`.
//!
//! Mapping from `AlephToolDyn` → `ToolHandler`:
//!   name              → `BuiltinHandler::name` (stored at construction)
//!   call(args)        → invoke(input), errors stringified into `ToolError::Execution`
//!   `definition()`      → `tool_metadata::ToolDefinition`; we re-project its
//!                       name/description/parameters into the new
//!                       `service::ToolDefinition` and pin source=Builtin,
//!                       carrying `requires_confirmation` through metadata.

use crate::sync_primitives::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::executor::ToolRegistry;
use crate::mcp::tool_bridge::{
    LOGIN_TOOL, PROMPT_LIST_TOOL, PROMPT_TOOL, RESOURCE_LIST_TOOL, RESOURCE_TEMPLATE_LIST_TOOL,
    RESOURCE_TOOL,
};
use crate::session::events::{ToolOutput, ToolOutputMetadata};
use crate::tools::descriptor::ToolCapabilityDescriptor;
use crate::tools::handlers::ToolHandler;
use crate::tools::registration_scope::ToolRegistrationScope;
use crate::tools::registry::ToolHandlerRegistry;
use crate::tools::service::{ToolDefinition, ToolDefinitionMetadata, ToolError, ToolSource};
use crate::tools::AlephToolDyn;

pub struct BuiltinHandler {
    inner: Arc<dyn AlephToolDyn>,
    name: String,
    /// Whether this handler's output must be fence-quoted before the model
    /// sees it. `true` for bridge builtins whose payload crosses an untrusted
    /// MCP boundary (`mcp_read_resource` and friends); `false` for native
    /// builtins whose output is the harness's own JSON. Surfaced via
    /// [`ToolHandler::fences_output`] so the harness can route accordingly.
    fences_output: bool,
}

impl BuiltinHandler {
    pub fn new(name: String, inner: Arc<dyn AlephToolDyn>) -> Self {
        Self {
            inner,
            name,
            fences_output: false,
        }
    }

    /// Builder knob for [`BuiltinHandler::fences_output`]. The default `false`
    /// covers every native builtin; the MCP bridge turns it on for the five
    /// capability builtins whose output passes through a server the harness
    /// does not control.
    pub fn with_fences_output(mut self, value: bool) -> Self {
        self.fences_output = value;
        self
    }
}

#[async_trait]
impl ToolHandler for BuiltinHandler {
    async fn invoke(&self, input: Value) -> Result<ToolOutput, ToolError> {
        match self.inner.call(input).await {
            Ok(value) => Ok(ToolOutput {
                value,
                metadata: ToolOutputMetadata::default(),
            }),
            // Argument validation failures originate in `AlephTool::call_json`
            // (default impl): when the LLM sends arguments that fail
            // `serde_json::from_value`, the trait returns
            // `AlephError::Validation(<format_validation_error output>)`.
            // Map them to `ToolError::ValidationFailed` so the harness reports
            // them as fixable schema errors with the tool-supplied prose,
            // rather than opaque `Execution` failures.
            Err(crate::error::AlephError::Validation(cause)) => Err(ToolError::ValidationFailed {
                name: self.name.clone(),
                cause,
            }),
            Err(e) => Err(ToolError::Execution {
                name: self.name.clone(),
                cause: e.to_string(),
            }),
        }
    }

    fn definition(&self) -> ToolDefinition {
        let inner_def = self.inner.definition();
        let idempotent = crate::tools::retry::is_idempotent_builtin_name(&self.name);
        // The static-dispatch `AlephTool` surface declares no budget of its
        // own, so this resolves table → default. Never `None`: an unbudgeted
        // definition is what turned a slow tool into a run-level abort.
        let max_duration_ms = crate::tools::budget::resolve_tool_budget_ms(&self.name, None);
        ToolDefinition {
            name: self.name.clone(),
            description: inner_def.description,
            input_schema: inner_def.parameters,
            source: ToolSource::Builtin,
            metadata: ToolDefinitionMetadata {
                hidden_from_llm: false,
                requires_approval: inner_def.requires_confirmation,
                tags: Vec::new(),
                idempotent,
                max_duration_ms: Some(max_duration_ms),
                // Same source as `idempotent`: `READ_ONLY_TOOLS` (via
                // `is_idempotent_builtin_name`) is the single list from which
                // read-only-ness, the `Shared` claim and the `Ask`-tier
                // exemption all derive, and read-only implies safe to run
                // alongside anything.
                //
                // This used to be a hard-coded `false` justified by "the
                // handler path is never picked up by the parallel fast path".
                // That was wrong for the bridge builtins (`mcp_read_resource`
                // and friends): `BuiltinHandler` IS their production path, and
                // `McpRegistryTool::from_registry_entry` copies this very flag
                // into `LoopTool::is_concurrent_safe`. They were on the
                // read-only list yet could never claim `Shared`.
                concurrent_safe: idempotent,
            },
        }
    }

    fn fences_output(&self) -> bool {
        self.fences_output
    }
}

/// A `ToolHandler` whose `invoke` and `definition` are sourced from a
/// `ToolRegistry` lookup keyed by [`name`](Self::name).
///
/// Phase-2-era `BuiltinHandler` wraps an `AlephToolDyn` directly; the agent
/// loop's new tool path resolves tools through `ToolRegistry` (the same
/// registry the agent loop dispatches every tool call against). This router
/// lets a `BuiltinHandler`-shaped handler stay the single shape the MCP bridge
/// and the run loop see, while delegating every actual operation to the
/// registry — so renaming, schema, and side-effect accounting all stay in one
/// place and the bridge doesn't carry a parallel dispatch table.
///
/// The router is a thin adapter, not a re-implementation: its `invoke` simply
/// forwards to the registry, and its `definition` only differs from a static
/// `BuiltinHandler`'s in that it has to project the registry's `UnifiedTool`
/// into a `service::ToolDefinition` for the harness.
pub struct BuiltinRegistryRouter {
    name: String,
    inner: Arc<dyn ToolRegistry>,
}

impl BuiltinRegistryRouter {
    pub fn new(name: String, inner: Arc<dyn ToolRegistry>) -> Self {
        Self { name, inner }
    }
}

#[async_trait]
impl ToolHandler for BuiltinRegistryRouter {
    async fn invoke(&self, input: Value) -> Result<ToolOutput, ToolError> {
        match self.inner.execute_tool(&self.name, input).await {
            Ok(value) => Ok(ToolOutput {
                value,
                metadata: ToolOutputMetadata::default(),
            }),
            // Argument validation failures from `ToolRegistry::execute_tool`
            // (the agent loop's adapter calls `format_validation_error` through
            // `AlephTool::call_json`) surface as `AlephError::Validation`; map
            // them to `ToolError::ValidationFailed` so the harness reports
            // them as fixable schema errors with the tool-supplied prose,
            // rather than opaque `Execution` failures.
            Err(crate::error::AlephError::Validation(cause)) => Err(ToolError::ValidationFailed {
                name: self.name.clone(),
                cause,
            }),
            Err(e) => Err(ToolError::Execution {
                name: self.name.clone(),
                cause: e.to_string(),
            }),
        }
    }

    fn definition(&self) -> ToolDefinition {
        use crate::ToolSource as CatalogSource;
        match self.inner.get_tool(&self.name) {
            Some(unified) => {
                // Same resolution chain as `BuiltinHandler`: declared → table
                // → default. Never `None`: an unbudgeted definition is what
                // turned a slow tool into a run-level abort.
                let max_duration_ms =
                    crate::tools::budget::resolve_tool_budget_ms(&self.name, None);
                // Same source as `BuiltinHandler`: the read-only list is the
                // single authority on idempotency, which in turn drives the `Shared`
                // claim. Routed tools must agree with their own catalog entry, not
                // declare a stale "always serial" baseline that would silently
                // serialize read-only MCP bridge calls behind the agent loop.
                let idempotent = crate::tools::retry::is_idempotent_builtin_name(&self.name);
                // MCP / extension / other non-builtin sources answer through
                // this router for tools the agent loop already has registered.
                // The MCP source variant carries the server id; projecting it
                // here means the harness can read the origin straight from the
                // definition without a second lookup.
                let source = match &unified.source {
                    CatalogSource::Builtin => ToolSource::Builtin,
                    CatalogSource::Mcp { server } => ToolSource::Mcp {
                        server_id: server.clone(),
                    },
                    CatalogSource::Plugin { plugin_id } => ToolSource::Extension {
                        plugin_id: plugin_id.clone(),
                    },
                    CatalogSource::Skill { id, plugin_id } => ToolSource::Extension {
                        plugin_id: plugin_id.clone().unwrap_or_else(|| id.clone()),
                    },
                    CatalogSource::Native | CatalogSource::Custom { .. } => ToolSource::Builtin,
                };
                ToolDefinition {
                    name: self.name.clone(),
                    description: unified.description.clone(),
                    // MCP / extension tools frequently publish no schema;
                    // default to a permissive JSON object so the harness's
                    // schema validator has something to read rather than
                    // rejecting on missing.
                    input_schema: unified
                        .parameters_schema
                        .clone()
                        .unwrap_or_else(|| serde_json::json!({"type": "object"})),
                    source,
                    // Project each `&UnifiedTool` field straight through so
                    // the harness sees the catalog's declared semantics:
                    // a router-launched tool is callable, and its
                    // `requires_confirmation` / idempotency / budget must
                    // come from the same source the registry itself uses.
                    metadata: ToolDefinitionMetadata {
                        hidden_from_llm: false,
                        requires_approval: unified.requires_confirmation,
                        tags: Vec::new(),
                        idempotent,
                        max_duration_ms: Some(max_duration_ms),
                        concurrent_safe: idempotent,
                    },
                }
            }
            // The tool was registered at handler-build time but has since
            // disappeared from the registry (a hot-reload dropped it, an
            // extension was disabled, etc.). Returning a definition rather
            // than panicking lets the harness surface "tool no longer
            // available" as a structured failure on the next call instead of
            // a crash on definition lookup. The schema is the same
            // permissive object the present-but-schemaless branch above
            // returns, so the model still gets a valid JSON Schema to read.
            None => ToolDefinition {
                name: self.name.clone(),
                description: String::new(),
                input_schema: serde_json::json!({"type": "object"}),
                source: ToolSource::Builtin,
                metadata: ToolDefinitionMetadata {
                    hidden_from_llm: true,
                    requires_approval: true,
                    tags: Vec::new(),
                    idempotent: false,
                    max_duration_ms: Some(crate::tools::budget::resolve_tool_budget_ms(
                        &self.name, None,
                    )),
                    concurrent_safe: false,
                },
            },
        }
    }
}

/// Names of the MCP-bridge capability builtins that are registered with the
/// `ToolHandlerRegistry` through the [`BuiltinRegistryRouter`] adapter rather
/// than the direct `BuiltinHandler` path. They go through
/// `ToolRegistry::execute_tool` so they pick up the same descriptor /
/// revision accounting as every other tool the agent loop sees.
///
/// Re-exported from [`crate::mcp::tool_bridge`] rather than re-typed: the
/// bridge owns these names as its single source of truth, and a second list
/// here is the exact "two copies of the same fact" drift this project's
/// redlines forbid. The bridge's `pub(crate)` constants are visible here
/// (same crate) despite the module cycle — Rust resolves items crate-wide.

/// Register a batch of [`BuiltinRegistryRouter`] adapters against the given
/// `ToolHandlerRegistry`, one per `name` in `names`. The function owns the
/// registrations through a single [`ToolRegistrationScope`] tagged
/// `capability:builtins` so the bridge can dispose them as one unit.
///
/// The six capability builtins (`mcp_read_resource` and friends — see the
/// `RESOURCE_TOOL`/`PROMPT_TOOL`/`LOGIN_TOOL` consts imported above) are
/// skipped when present in `names`: they are registered by the MCP bridge
/// itself when an MCP server boots, and re-registering them here would either
/// fail with `Duplicate` or — worse — overwrite the bridge's handler with a
/// router that has no server id in its scope and silently swallows every call.
pub async fn register_builtin_routers(
    registry: &ToolHandlerRegistry,
    tool_registry: Arc<dyn ToolRegistry>,
    names: impl IntoIterator<Item = String>,
) -> Result<ToolRegistrationScope, ToolError> {
    let mut scope = ToolRegistrationScope::new("capability:builtins");
    for name in names {
        // Skip the six MCP-bridge capability builtins. They are registered by
        // the MCP bridge at server-boot time; touching them here would race
        // the bridge and produce a Duplicate (or, if the bridge later
        // replaces them, a stale router with no server scope).
        if matches!(
            name.as_str(),
            RESOURCE_TOOL
                | RESOURCE_LIST_TOOL
                | RESOURCE_TEMPLATE_LIST_TOOL
                | PROMPT_TOOL
                | PROMPT_LIST_TOOL
                | LOGIN_TOOL
        ) {
            continue;
        }
        let handler: Arc<dyn ToolHandler> = Arc::new(BuiltinRegistryRouter::new(
            name.clone(),
            Arc::clone(&tool_registry),
        ));
        let descriptor = ToolCapabilityDescriptor::from_definition(&handler.definition(), 0);
        match registry.register(descriptor, handler) {
            Ok(handle) => scope.track(handle),
            Err(e) => {
                // Partial registration: roll back everything this call has
                // tracked so far before surfacing the error, so the caller
                // never sees a half-populated capability bundle.
                let _ = scope.dispose().await;
                return Err(e);
            }
        }
    }
    Ok(scope)
}

#[cfg(test)]
mod builtin_handler_tests {
    use super::*;
    use std::future::Future;
    use std::pin::Pin;

    struct FakeTool;

    impl crate::tools::AlephToolDyn for FakeTool {
        fn name(&self) -> &str {
            "fake_tool"
        }

        fn definition(&self) -> crate::tool_metadata::ToolDefinition {
            crate::tool_metadata::ToolDefinition::new(
                "fake_tool",
                "A fake tool for testing",
                serde_json::Value::Null,
                crate::tool_metadata::ToolCategory::Builtin,
            )
        }

        fn call(
            &self,
            _args: serde_json::Value,
        ) -> Pin<Box<dyn Future<Output = crate::error::Result<serde_json::Value>> + Send + '_>>
        {
            Box::pin(async { Ok(serde_json::Value::Null) })
        }
    }

    #[test]
    fn definition_populates_max_duration_ms_from_table() {
        let handler = BuiltinHandler::new("memory_search".to_string(), Arc::new(FakeTool));
        let def = handler.definition();
        assert_eq!(def.metadata.max_duration_ms, Some(5_000));
    }

    #[test]
    fn read_only_bridge_builtins_advertise_concurrent_safe() {
        // Severed wire: the five `mcp_*` bridge builtins are on
        // `READ_ONLY_TOOLS`, but their only production path is
        // `BuiltinHandler` -> `McpRegistryTool::from_registry_entry`, which
        // copies `metadata.concurrent_safe` straight into
        // `LoopTool::is_concurrent_safe`. Hard-coding `false` here meant the
        // list granted them idempotency but never the `Shared` claim, so a
        // batch of pure MCP capability reads always serialized.
        let handler = BuiltinHandler::new("mcp_list_resources".to_string(), Arc::new(FakeTool));
        assert!(handler.definition().metadata.concurrent_safe);
    }

    #[test]
    fn unlisted_bridge_builtins_stay_conservatively_serial() {
        let handler = BuiltinHandler::new("unknown_custom_tool".to_string(), Arc::new(FakeTool));
        assert!(!handler.definition().metadata.concurrent_safe);
    }

    #[test]
    fn definition_falls_back_to_default_budget_for_unlisted_tool() {
        // Regression: an unlisted tool used to advertise `None`, which the
        // harness read as "no per-tool budget" and escalated a slow call into
        // a run-level abort. Every definition now carries a budget.
        let handler = BuiltinHandler::new("unknown_custom_tool".to_string(), Arc::new(FakeTool));
        let def = handler.definition();
        assert_eq!(
            def.metadata.max_duration_ms,
            Some(crate::tools::budget::DEFAULT_TOOL_BUDGET_MS)
        );
    }

    /// A `AlephTool` whose `call_json` will reject malformed args via the
    /// default `format_validation_error` prose. Used to verify the
    /// `AlephError::Validation` → `ToolError::ValidationFailed` mapping in
    /// `BuiltinHandler::invoke`.
    #[derive(Clone)]
    struct StrictTool;

    #[derive(serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
    struct StrictArgs {
        query: String,
    }

    #[async_trait::async_trait]
    impl crate::tools::AlephTool for StrictTool {
        const NAME: &'static str = "strict_tool";
        const DESCRIPTION: &'static str = "Demands a query field";
        type Args = StrictArgs;
        type Output = serde_json::Value;

        async fn call(&self, _args: Self::Args) -> crate::error::Result<Self::Output> {
            Ok(serde_json::Value::String("ok".into()))
        }
    }

    #[tokio::test]
    async fn invoke_maps_validation_error_to_validation_failed() {
        let handler = BuiltinHandler::new("strict_tool".to_string(), Arc::new(StrictTool));
        // Missing the required `query` field.
        let bad_input = serde_json::json!({});
        let err = handler
            .invoke(bad_input)
            .await
            .expect_err("should fail validation");
        match err {
            ToolError::ValidationFailed { name, cause } => {
                assert_eq!(name, "strict_tool");
                assert!(
                    cause.contains("strict_tool"),
                    "prose should name the tool: {cause}"
                );
                assert!(
                    cause.contains("rewrite the input"),
                    "default prose should instruct rewrite: {cause}"
                );
            }
            other => panic!("expected ValidationFailed, got {other:?}"),
        }
    }

    /// Tool overrides `format_validation_error` to inject a custom hint.
    #[derive(Clone)]
    struct CustomProseTool;

    #[async_trait::async_trait]
    impl crate::tools::AlephTool for CustomProseTool {
        const NAME: &'static str = "custom_prose_tool";
        const DESCRIPTION: &'static str = "Has custom validation prose";
        type Args = StrictArgs;
        type Output = serde_json::Value;

        fn format_validation_error(err: &serde_json::Error) -> String {
            format!("[CUSTOM HINT] expected {{query: string}}; got: {err}")
        }

        async fn call(&self, _args: Self::Args) -> crate::error::Result<Self::Output> {
            Ok(serde_json::Value::Null)
        }
    }

    #[tokio::test]
    async fn invoke_uses_custom_validation_prose_when_overridden() {
        let handler =
            BuiltinHandler::new("custom_prose_tool".to_string(), Arc::new(CustomProseTool));
        let err = handler
            .invoke(serde_json::json!({}))
            .await
            .expect_err("should fail validation");
        match err {
            ToolError::ValidationFailed { cause, .. } => {
                assert!(
                    cause.starts_with("[CUSTOM HINT]"),
                    "expected custom prose, got: {cause}"
                );
            }
            other => panic!("expected ValidationFailed, got {other:?}"),
        }
    }

    use std::collections::HashMap;

    /// Mock `ToolRegistry` for `register_builtin_routers` tests: `get_tool`
    /// resolves nothing (so `BuiltinRegistryRouter::definition()` takes the
    /// conservative fallback path), while `execute_tool` answers from a
    /// name→value table so `invoke` can be observed returning a caller-chosen
    /// sentinel.
    struct MockToolRegistry {
        results: HashMap<String, serde_json::Value>,
    }

    impl crate::executor::ToolRegistry for MockToolRegistry {
        fn get_tool(&self, _name: &str) -> Option<&crate::tool_metadata::UnifiedTool> {
            None
        }

        fn execute_tool(
            &self,
            tool_name: &str,
            _arguments: serde_json::Value,
        ) -> Pin<Box<dyn Future<Output = crate::error::Result<serde_json::Value>> + Send + '_>>
        {
            let value = self.results.get(tool_name).cloned();
            let name = tool_name.to_string();
            Box::pin(
                async move { value.ok_or_else(|| crate::error::AlephError::tool_not_found(&name)) },
            )
        }
    }

    /// The six MCP-bridge capability names are registered by the MCP bridge
    /// itself; `register_builtin_routers` must skip them. Feeding the full six
    /// alongside one ordinary name registers only the ordinary one.
    #[tokio::test]
    async fn register_builtin_routers_skips_the_six_bridge_names() {
        let registry = ToolHandlerRegistry::new();
        let tool_registry: Arc<dyn ToolRegistry> = Arc::new(MockToolRegistry {
            results: HashMap::new(),
        });
        let names: Vec<String> = vec![
            crate::mcp::tool_bridge::RESOURCE_TOOL.to_string(),
            crate::mcp::tool_bridge::RESOURCE_LIST_TOOL.to_string(),
            crate::mcp::tool_bridge::RESOURCE_TEMPLATE_LIST_TOOL.to_string(),
            crate::mcp::tool_bridge::PROMPT_TOOL.to_string(),
            crate::mcp::tool_bridge::PROMPT_LIST_TOOL.to_string(),
            crate::mcp::tool_bridge::LOGIN_TOOL.to_string(),
            "ordinary_builtin".to_string(),
        ];
        let scope = register_builtin_routers(&registry, tool_registry, names)
            .await
            .expect("registration should succeed");
        assert_eq!(scope.len(), 1, "only the ordinary name is tracked");
        assert!(
            registry.resolve("ordinary_builtin").is_some(),
            "ordinary name must be registered"
        );
        for bridge_name in [
            crate::mcp::tool_bridge::RESOURCE_TOOL,
            crate::mcp::tool_bridge::RESOURCE_LIST_TOOL,
            crate::mcp::tool_bridge::RESOURCE_TEMPLATE_LIST_TOOL,
            crate::mcp::tool_bridge::PROMPT_TOOL,
            crate::mcp::tool_bridge::PROMPT_LIST_TOOL,
            crate::mcp::tool_bridge::LOGIN_TOOL,
        ] {
            assert!(
                registry.resolve(bridge_name).is_none(),
                "bridge builtin {bridge_name} must not be registered by the router helper"
            );
        }
        assert!(scope.dispose().await.all_ok());
    }

    /// A routed ordinary tool's `invoke` must delegate to the registry's
    /// `execute_tool` (returning the sentinel), not answer locally.
    #[tokio::test]
    async fn routed_handler_invoke_delegates_to_registry_execute() {
        let registry = ToolHandlerRegistry::new();
        let sentinel = serde_json::json!({ "sentinel": 7 });
        let mut results = HashMap::new();
        results.insert("ordinary_builtin".to_string(), sentinel.clone());
        let tool_registry: Arc<dyn ToolRegistry> = Arc::new(MockToolRegistry { results });

        register_builtin_routers(
            &registry,
            Arc::clone(&tool_registry),
            vec!["ordinary_builtin".to_string()],
        )
        .await
        .expect("registration should succeed");

        let handler = registry
            .resolve("ordinary_builtin")
            .expect("ordinary name is registered");
        let output = handler
            .invoke(serde_json::json!({}))
            .await
            .expect("invoke should succeed");
        assert_eq!(
            output.value, sentinel,
            "invoke must forward to execute_tool"
        );
    }

    /// A duplicate partway through the batch rolls back every earlier
    /// registration in the same call, leaving only the pre-existing entry.
    #[tokio::test]
    async fn register_builtin_routers_rolls_back_on_partial_failure() {
        let registry = ToolHandlerRegistry::new();
        let tool_registry: Arc<dyn ToolRegistry> = Arc::new(MockToolRegistry {
            results: HashMap::new(),
        });

        // Pre-register "already_there" so the second batch hits a Duplicate.
        register_builtin_routers(
            &registry,
            Arc::clone(&tool_registry),
            vec!["already_there".to_string()],
        )
        .await
        .expect("first registration should succeed");

        // "new_tool" registers fine, then "already_there" fails → rollback.
        let err = match register_builtin_routers(
            &registry,
            tool_registry,
            vec!["new_tool".to_string(), "already_there".to_string()],
        )
        .await
        {
            Ok(_scope) => panic!("duplicate must fail the batch"),
            Err(e) => e,
        };
        match err {
            ToolError::Duplicate { name } => assert_eq!(name, "already_there"),
            other => panic!("expected Duplicate, got {other:?}"),
        }
        assert!(
            registry.resolve("new_tool").is_none(),
            "the partial registration must be rolled back"
        );
        assert!(
            registry.resolve("already_there").is_some(),
            "the pre-existing entry must be untouched"
        );
    }
}
