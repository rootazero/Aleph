//! `LoopTool` trait and `LoopToolRegistry`.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use tokio_util::sync::CancellationToken;

// =============================================================================
// ToolResult
// =============================================================================

/// Outcome of a tool execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ToolResult {
    Success { output: Value },
    Error { error: String, retryable: bool },
}

// =============================================================================
// ToolDefinition (local, minimal)
// =============================================================================

/// Lightweight tool definition for LLM function calling.
///
/// Intentionally simpler than `crate::tool_metadata::ToolDefinition` —
/// no category, confirmation, or strict-mode fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value,
    /// Per-tool result size limit in estimated tokens. Falls back to global default if None.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_result_tokens: Option<usize>,
    /// Static "this tool is safe under parallel dispatch" hint, derived
    /// from `LoopTool::is_concurrent_safe(&Value::Null)` at definition build
    /// time. Informational only — the harness queries
    /// `ToolService::call_concurrency_claim(name, &actual_input)` at
    /// dispatch time for the authoritative answer, since input-dependent
    /// tools (e.g. `file_ops`) may flip per call.
    #[serde(default)]
    pub concurrent_safe: bool,
    /// Static "running this tool needs explicit user confirmation" hint,
    /// derived from [`LoopTool::requires_confirmation`] at definition build
    /// time. Mirrors `concurrent_safe`: informational for catalog consumers,
    /// while the live confirmation gate in `ScopedToolService` queries the
    /// registry directly at dispatch time. `#[serde(default)]` keeps legacy
    /// JSON (absent field) decoding to `false`.
    #[serde(default)]
    pub requires_confirmation: bool,
    /// Per-call wall-clock budget the tool declared for itself, from
    /// [`LoopTool::max_duration_ms`]. `None` = "declares nothing", NOT
    /// "unbudgeted": the `ToolService` definition builders resolve it through
    /// `tools::budget::resolve_tool_budget_ms` (declaration → builtin table →
    /// global default) so the harness never sees a tool without a budget.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_duration_ms: Option<u64>,
}

// =============================================================================
// LoopTool trait
// =============================================================================

/// Unified tool trait — one trait to rule them all.
///
/// Every tool (built-in, MCP, skill, extension) implements this single trait.
/// Schema is returned as a raw JSON value so tools can describe themselves
/// without pulling in heavy schema crates.
#[async_trait]
pub trait LoopTool: Send + Sync {
    /// Unique tool name used in function calls.
    fn name(&self) -> &str;

    /// Human-readable description for LLM.
    fn description(&self) -> &str;

    /// JSON Schema for input parameters.
    fn schema(&self) -> Value;

    /// Execute the tool with the given input.
    ///
    /// `cancel` carries opencode-parity `AbortSignal` semantics: the harness
    /// forks a per-call child token from the run's cancellation token
    /// (`run_cancel.child_token()`, `harness::agent::act`) and wraps the call in
    /// `tokio::select!`. (This used to name a `ChainContext::cancellation_token`
    /// that has never existed on that type.) Tools that
    /// run unbounded loops or want to emit partial results on abort should
    /// also `select!` against `cancel.cancelled()` cooperatively. Tools that
    /// just await a single I/O future can ignore the token — when the
    /// harness drops the call future on cancel, kill-on-drop / reqwest
    /// cancellation propagates naturally.
    async fn execute(&self, input: Value, cancel: CancellationToken) -> ToolResult;

    /// Whether this tool can safely run concurrently with other concurrent-safe tools.
    ///
    /// Returns `false` by default — the fail-closed side, matching
    /// [`is_idempotent`](LoopTool::is_idempotent) and the `READ_ONLY_TOOLS`
    /// allowlist inversion in `RegistryToolAdapter`: a tool parallelizes only
    /// by *explicit declaration*, so a future direct-registered mutator that
    /// forgets the override serializes (correct, just slower) instead of
    /// silently racing. (The default was `true` until 2026-07-17 — the exact
    /// opposite failure mode of the allowlist it coexisted with.) Override to
    /// `true` only for tools with no observable shared-state mutation.
    fn is_concurrent_safe(&self, _input: &Value) -> bool {
        false
    }

    /// Resource-scope-aware concurrency claim for the parallel dispatch path.
    ///
    /// Names the *blast radius* of this call so the scheduler can run
    /// disjoint-scope mutations concurrently while still serializing ones that
    /// touch the same resource. The default delegates to
    /// [`is_concurrent_safe`](LoopTool::is_concurrent_safe): `true` →
    /// `ConcurrencyClaim::Shared`, `false` → `Exclusive { Global }`. Keeping
    /// the default a pure delegation makes every existing tool byte-identical;
    /// only tools that touch a *bounded* set of paths (file write/edit/patch,
    /// `file_ops` mutating operations) override this to return a bounded
    /// `Exclusive { Paths }` scope so disjoint-path calls can parallelize while
    /// same-path ones stay serial.
    fn concurrency_claim(&self, input: &Value) -> crate::tools::concurrency::ConcurrencyClaim {
        use crate::tools::concurrency::ConcurrencyClaim;
        if self.is_concurrent_safe(input) {
            ConcurrencyClaim::Shared
        } else {
            ConcurrencyClaim::global()
        }
    }

    /// Whether running this tool requires explicit user confirmation first.
    ///
    /// Returns `false` by default. Override to `true` on irreversible /
    /// sensitive tools (destructive deletes, credential writes, fund
    /// transfers, MCP / extension tools that declare themselves dangerous)
    /// so the live confirmation gate in `ScopedToolService` routes a user
    /// approval before dispatch. This is the per-tool, declaration-driven
    /// approval seam: any `LoopTool` (built-in, MCP, skill, extension) can
    /// opt in without being hard-coded into the gateway. Builtins declare it
    /// through `RegistryToolAdapter`'s `CONFIRMATION_REQUIRED_TOOLS` list
    /// (co-located with the `READ_ONLY_TOOLS` concurrency allowlist). Mirrors openclaw's per-tool policy and
    /// hermes's per-tool approval flags. Like `is_concurrent_safe`, the
    /// answer is static per tool; it is not input-dependent.
    fn requires_confirmation(&self) -> bool {
        false
    }

    /// Whether re-running this tool with identical input has no observable
    /// side effect — a declared pure read / naturally idempotent call.
    ///
    /// `false` by default — the same fail-closed side as
    /// [`is_concurrent_safe`](LoopTool::is_concurrent_safe): this answer feeds
    /// the `Ask` exec tier's "not idempotent = mutating" rule, so an unknown
    /// tool must land on the fail-closed side. Builtins answer through
    /// `RegistryToolAdapter` (the `READ_ONLY_TOOLS` allowlist, via
    /// `retry::is_idempotent_builtin_name`); MCP tools answer from the
    /// server's own `readOnlyHint` / `idempotentHint`. Like the other two
    /// flags, the answer is static per tool, not input-dependent.
    fn is_idempotent(&self) -> bool {
        false
    }

    /// Per-result token budget hint used by the Layer 2 result processor.
    ///
    /// - `Some(n)` — persist this tool's outputs to disk when they exceed
    ///   `n` estimated tokens; the LLM sees a `[Full output persisted: ...]`
    ///   marker instead of the full text.
    /// - `None` — fall back to the global name table / default budget in
    ///   [`crate::tools::result_processing::resolve_result_budget`].
    ///
    /// Default returns `None`; override on tools whose outputs are large
    /// enough to warrant offloading (bash, `web_fetch`, etc.) or whose
    /// outputs should never be persisted (`read_file`-family).
    fn max_result_tokens(&self) -> Option<usize> {
        None
    }

    /// Per-call wall-clock budget this tool declares for itself, in
    /// milliseconds.
    ///
    /// `None` (the default) does NOT mean "unbounded": the definition
    /// builders resolve it through
    /// [`crate::tools::budget::resolve_tool_budget_ms`], which falls back to
    /// the builtin table and then to `DEFAULT_TOOL_BUDGET_MS`. Override only
    /// when the tool owns a clock the static tables cannot know — an MCP tool
    /// whose server has a configured request timeout, say — so the harness's
    /// budget stays above it and the tool's own timeout is what fires.
    fn max_duration_ms(&self) -> Option<u64> {
        None
    }
}

// =============================================================================
// LoopToolRegistry
// =============================================================================

/// Flat registry mapping tool names to trait objects.
pub struct LoopToolRegistry {
    tools: HashMap<String, Box<dyn LoopTool>>,
}

impl LoopToolRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// Register a tool. Overwrites any existing tool with the same name.
    pub fn register(&mut self, tool: Box<dyn LoopTool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    /// Look up a tool by name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&dyn LoopTool> {
        self.tools.get(name).map(|b| b.as_ref())
    }

    /// Resolve a tool by name with dot/underscore alias fallback.
    ///
    /// Same resolution logic as `execute()`, but returns the tool reference
    /// without running it. Useful for pre-execution validation.
    #[must_use]
    pub fn resolve(&self, name: &str) -> Option<&dyn LoopTool> {
        if let Some(tool) = self.get(name) {
            return Some(tool);
        }
        let alt = if name.contains('.') {
            name.replace('.', "_")
        } else if name.contains('_') {
            name.replace('_', ".")
        } else {
            return None;
        };
        self.get(&alt)
    }

    /// Number of registered tools.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Whether the registry is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Execute a tool by name.
    ///
    /// If exact match fails, tries swapping dots/underscores as a fallback,
    /// since LLMs sometimes confuse `file_ops` with `file.ops` (or vice versa).
    ///
    /// `cancel` is forwarded verbatim to the resolved tool. See
    /// [`LoopTool::execute`] for the cancellation contract.
    pub async fn execute(&self, name: &str, input: Value, cancel: CancellationToken) -> ToolResult {
        if let Some(tool) = self.get(name) {
            return tool.execute(input, cancel).await;
        }

        // Fallback: try swapping dots ↔ underscores
        let alt = if name.contains('.') {
            name.replace('.', "_")
        } else if name.contains('_') {
            name.replace('_', ".")
        } else {
            return ToolResult::Error {
                error: format!("unknown tool: {name}"),
                retryable: false,
            };
        };

        if let Some(tool) = self.get(&alt) {
            tracing::debug!(original = name, resolved = %alt, "Tool name normalized");
            return tool.execute(input, cancel).await;
        }

        ToolResult::Error {
            error: format!("unknown tool: {name}"),
            retryable: false,
        }
    }

    /// Remove tools whose names do not satisfy the predicate.
    pub fn retain(&mut self, f: impl Fn(&str) -> bool) {
        self.tools.retain(|name, _| f(name));
    }

    /// Collect definitions for all registered tools (sorted by name for determinism).
    #[must_use]
    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        let mut defs: Vec<ToolDefinition> = self
            .tools
            .values()
            .map(|t| ToolDefinition {
                name: t.name().to_string(),
                description: t.description().to_string(),
                parameters: t.schema(),
                max_result_tokens: t.max_result_tokens(),
                concurrent_safe: t.is_concurrent_safe(&Value::Null),
                requires_confirmation: t.requires_confirmation(),
                max_duration_ms: t.max_duration_ms(),
            })
            .collect();
        defs.sort_by(|a, b| a.name.cmp(&b.name));
        defs
    }

    /// Resolve the named tool's resource-scope-aware concurrency claim for the
    /// given input (see [`LoopTool::concurrency_claim`]). Returns `None` if the
    /// tool is unknown to this registry — callers should treat unknown as the
    /// conservative whole-world [`crate::tools::concurrency::ConcurrencyClaim::global`].
    #[must_use]
    pub fn call_concurrency_claim(
        &self,
        name: &str,
        input: &Value,
    ) -> Option<crate::tools::concurrency::ConcurrencyClaim> {
        self.resolve(name).map(|t| t.concurrency_claim(input))
    }

    /// Whether the named tool declares that it requires explicit user
    /// confirmation before running (see [`LoopTool::requires_confirmation`]).
    /// Unknown tools return `false` — they are gated elsewhere (the allowed
    /// filter rejects them with `NotFound` before any confirmation check).
    /// Uses dot/underscore alias resolution so a confirmation-required tool
    /// is still recognized when the LLM emits the aliased spelling.
    #[must_use]
    pub fn requires_confirmation(&self, name: &str) -> bool {
        self.resolve(name)
            .is_some_and(|t| t.requires_confirmation())
    }

    /// Whether the named tool declares itself a pure read (see
    /// [`LoopTool::is_idempotent`]). Unknown tools return `false` — the
    /// fail-closed answer the `Ask` exec tier depends on.
    #[must_use]
    pub fn is_idempotent(&self, name: &str) -> bool {
        self.resolve(name).is_some_and(|t| t.is_idempotent())
    }

    /// Return the per-result token budget that the named tool declared via
    /// [`LoopTool::max_result_tokens`], if the tool is registered. Used by
    /// `ScopedToolService::apply_layer_two` to look up the budget without
    /// rebuilding a full `ToolDefinition`.
    #[must_use]
    pub fn max_result_tokens_for(&self, name: &str) -> Option<usize> {
        let tool = self.resolve(name)?;
        tool.max_result_tokens()
    }
}

impl Default for LoopToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A trivial echo tool for testing.
    struct EchoTool;

    #[async_trait]
    impl LoopTool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }
        fn description(&self) -> &str {
            "Echoes the input back"
        }
        fn schema(&self) -> Value {
            json!({
                "type": "object",
                "properties": {
                    "message": { "type": "string" }
                },
                "required": ["message"]
            })
        }
        async fn execute(&self, input: Value, _cancel: CancellationToken) -> ToolResult {
            ToolResult::Success { output: input }
        }
    }

    /// A tool that always fails — for testing error paths.
    struct FailTool;

    #[async_trait]
    impl LoopTool for FailTool {
        fn name(&self) -> &str {
            "fail"
        }
        fn description(&self) -> &str {
            "Always fails"
        }
        fn schema(&self) -> Value {
            json!({ "type": "object", "properties": {} })
        }
        async fn execute(&self, _input: Value, _cancel: CancellationToken) -> ToolResult {
            ToolResult::Error {
                error: "intentional failure".into(),
                retryable: true,
            }
        }
    }

    #[tokio::test]
    async fn test_minimal_tool_execute() {
        let tool = EchoTool;
        let input = json!({ "message": "hello" });
        let result = tool.execute(input.clone(), CancellationToken::new()).await;

        match result {
            ToolResult::Success { output } => {
                assert_eq!(output, input);
            }
            ToolResult::Error { .. } => panic!("expected success"),
        }
    }

    #[tokio::test]
    async fn test_minimal_tool_registry() {
        let mut registry = LoopToolRegistry::new();
        assert!(registry.is_empty());

        registry.register(Box::new(EchoTool));
        registry.register(Box::new(FailTool));
        assert_eq!(registry.len(), 2);

        // Get existing tool
        assert!(registry.get("echo").is_some());
        assert_eq!(registry.get("echo").unwrap().name(), "echo");

        // Get non-existent tool
        assert!(registry.get("nope").is_none());

        // Execute existing tool
        let result = registry
            .execute("echo", json!({ "message": "hi" }), CancellationToken::new())
            .await;
        match result {
            ToolResult::Success { output } => {
                assert_eq!(output, json!({ "message": "hi" }));
            }
            ToolResult::Error { .. } => panic!("expected success"),
        }

        // Execute unknown tool
        let result = registry
            .execute("unknown", json!({}), CancellationToken::new())
            .await;
        match result {
            ToolResult::Error {
                error, retryable, ..
            } => {
                assert!(error.contains("unknown tool"));
                assert!(!retryable);
            }
            ToolResult::Success { .. } => {
                panic!("expected error")
            }
        }
    }

    #[test]
    fn test_default_concurrent_safe_is_fail_closed() {
        let tool = EchoTool;
        // The default is fail-closed: a tool parallelizes only by explicit
        // declaration, so a forgotten override serializes instead of racing.
        assert!(!tool.is_concurrent_safe(&json!({})));
        assert!(!tool.is_concurrent_safe(&json!({"message": "hello"})));
    }

    /// A tool that overrides is_concurrent_safe to return false.
    struct ExclusiveTool;

    #[async_trait]
    impl LoopTool for ExclusiveTool {
        fn name(&self) -> &str {
            "exclusive"
        }
        fn description(&self) -> &str {
            "A tool that mutates shared state"
        }
        fn schema(&self) -> Value {
            json!({ "type": "object", "properties": {} })
        }
        async fn execute(&self, _input: Value, _cancel: CancellationToken) -> ToolResult {
            ToolResult::Success {
                output: json!("done"),
            }
        }
        fn is_concurrent_safe(&self, _input: &Value) -> bool {
            false
        }
    }

    #[test]
    fn test_exclusive_tool_not_concurrent_safe() {
        let tool = ExclusiveTool;
        assert!(!tool.is_concurrent_safe(&json!({})));
        assert!(!tool.is_concurrent_safe(&json!({"key": "value"})));
    }

    #[tokio::test]
    async fn test_registry_schemas() {
        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(EchoTool));
        registry.register(Box::new(FailTool));

        let defs = registry.tool_definitions();
        assert_eq!(defs.len(), 2);

        // Sorted by name: echo < fail
        assert_eq!(defs[0].name, "echo");
        assert_eq!(defs[0].description, "Echoes the input back");
        assert_eq!(defs[0].parameters["required"], json!(["message"]));

        assert_eq!(defs[1].name, "fail");
        assert_eq!(defs[1].description, "Always fails");
    }

    /// A minimal tool parameterized by name — for testing registry operations.
    struct NamedTool(String);

    #[async_trait]
    impl LoopTool for NamedTool {
        fn name(&self) -> &str {
            &self.0
        }
        fn description(&self) -> &str {
            "named tool"
        }
        fn schema(&self) -> Value {
            json!({ "type": "object", "properties": {} })
        }
        async fn execute(&self, _input: Value, _cancel: CancellationToken) -> ToolResult {
            ToolResult::Success {
                output: json!(null),
            }
        }
    }

    #[tokio::test]
    async fn test_retain_filters_tools() {
        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(NamedTool("alpha".into())));
        registry.register(Box::new(NamedTool("beta".into())));
        registry.register(Box::new(NamedTool("gamma".into())));
        assert_eq!(registry.len(), 3);

        registry.retain(|name| name == "alpha" || name == "gamma");

        assert_eq!(registry.len(), 2);
        assert!(registry.get("alpha").is_some());
        assert!(registry.get("beta").is_none());
        assert!(registry.get("gamma").is_some());
    }

    // -----------------------------------------------------------------
    // LoopTool::max_result_tokens — wires the previously-dead
    // ToolDefinition.max_result_tokens field end-to-end.
    // -----------------------------------------------------------------

    /// A tool that opts into the Layer 2 budget hint via the trait method.
    struct BudgetedTool;

    #[async_trait]
    impl LoopTool for BudgetedTool {
        fn name(&self) -> &str {
            "budgeted"
        }
        fn description(&self) -> &str {
            "Declares a 4000-token result budget"
        }
        fn schema(&self) -> Value {
            json!({ "type": "object", "properties": {} })
        }
        async fn execute(&self, _input: Value, _cancel: CancellationToken) -> ToolResult {
            ToolResult::Success { output: json!({}) }
        }
        fn max_result_tokens(&self) -> Option<usize> {
            Some(4_000)
        }
    }

    #[test]
    fn max_result_tokens_default_is_none() {
        let tool = EchoTool;
        assert_eq!(tool.max_result_tokens(), None);
    }

    #[test]
    fn registry_propagates_max_result_tokens_to_definitions() {
        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(BudgetedTool));
        registry.register(Box::new(EchoTool));
        let defs = registry.tool_definitions();
        let budgeted = defs.iter().find(|d| d.name == "budgeted").expect("found");
        assert_eq!(budgeted.max_result_tokens, Some(4_000));
        let echo = defs.iter().find(|d| d.name == "echo").expect("found");
        assert_eq!(echo.max_result_tokens, None);
    }

    #[test]
    fn registry_max_result_tokens_for_returns_per_tool_value() {
        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(BudgetedTool));
        registry.register(Box::new(EchoTool));
        assert_eq!(registry.max_result_tokens_for("budgeted"), Some(4_000));
        assert_eq!(registry.max_result_tokens_for("echo"), None);
        // Unknown tool returns None.
        assert_eq!(registry.max_result_tokens_for("nonexistent"), None);
    }
}
