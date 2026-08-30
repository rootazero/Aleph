//! Tool Traits
//!
//! Defines the core tool traits for Aleph's tool system.

use async_trait::async_trait;
use schemars::{schema_for, JsonSchema};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

use crate::error::Result;
use crate::tool_metadata::{ToolCategory, ToolDefinition};

// =============================================================================
// AlephTool - Static Dispatch Trait
// =============================================================================

/// Static dispatch tool trait for compile-time known tools.
///
/// This trait is designed for builtin tools where the argument and output types
/// are known at compile time. It provides:
///
/// - Type-safe argument handling via generics
/// - Automatic JSON Schema generation from Args type
/// - Zero-cost abstraction over JSON serialization
///
/// # Example
///
/// ```rust,ignore
/// use crate::tools::AlephTool;
/// use schemars::JsonSchema;
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Clone)]
/// struct SearchTool { /* ... */ }
///
/// #[derive(Serialize, Deserialize, JsonSchema)]
/// struct SearchArgs {
///     query: String,
///     max_results: Option<u32>,
/// }
///
/// #[derive(Serialize)]
/// struct SearchOutput {
///     results: Vec<String>,
/// }
///
/// #[async_trait::async_trait]
/// impl AlephTool for SearchTool {
///     const NAME: &'static str = "search";
///     const DESCRIPTION: &'static str = "Search the web for information";
///
///     type Args = SearchArgs;
///     type Output = SearchOutput;
///
///     async fn call(&self, args: Self::Args) -> Result<Self::Output> {
///         // Implementation
///         Ok(SearchOutput { results: vec![] })
///     }
/// }
/// ```
#[async_trait]
pub trait AlephTool: Clone + Send + Sync + 'static {
    /// Tool name used in function calls (e.g., "search", "`file_read`")
    const NAME: &'static str;

    /// Human-readable description for LLM tool selection
    const DESCRIPTION: &'static str;

    /// Input argument type (must derive `JsonSchema` for auto-schema generation)
    type Args: Serialize + DeserializeOwned + JsonSchema + Send;

    /// Output type (serialized to JSON for LLM)
    type Output: Serialize + Send;

    /// Get tool category (default: Builtin)
    ///
    /// Override this for non-builtin tools.
    fn category(&self) -> ToolCategory {
        ToolCategory::Builtin
    }

    /// Whether this tool requires user confirmation before execution.
    ///
    /// Default is false. Override for destructive operations.
    fn requires_confirmation(&self) -> bool {
        false
    }

    /// Per-result token budget for this tool's output (Layer-2 spill/truncate).
    ///
    /// `Some(n)` caps the tool's result at `n` tokens before the overflow
    /// cascade (`compress → persist-if-large → truncate`) in
    /// [`crate::tools::result_processing::resolve_result_budget`); `None`
    /// (the default) defers to the global default budget. Override on tools
    /// whose output is reliably larger or smaller than the norm (e.g. a web
    /// fetch returns whole pages). This replaces the legacy hardcoded
    /// tool-name budget table — declare the budget where the tool lives.
    fn max_result_tokens(&self) -> Option<usize> {
        None
    }

    /// Whether this tool's schema is strict-mode compatible.
    ///
    /// When true, the tool's JSON Schema will be transformed for strict mode
    /// (additionalProperties: false, all properties required) and the strict
    /// flag will be sent to providers that support it (e.g., `OpenAI`).
    ///
    /// Default is true. Override to false for tools with dynamic schemas
    /// that cannot satisfy strict mode constraints.
    fn strict_schema(&self) -> bool {
        true
    }

    /// Get tool definition with auto-generated JSON Schema.
    ///
    /// The default implementation generates the schema from `Self::Args`.
    /// Override only if custom schema handling is needed.
    fn definition(&self) -> ToolDefinition {
        let schema = schema_for!(Self::Args);
        let parameters = serde_json::to_value(&schema)
            .inspect_err(|e| {
                tracing::error!(
                    tool = Self::NAME,
                    error = %e,
                    "Failed to serialize JSON Schema for tool definition"
                );
            })
            .unwrap_or_default();

        ToolDefinition::new(Self::NAME, Self::DESCRIPTION, parameters, self.category())
            .with_confirmation(self.requires_confirmation())
            .with_strict(self.strict_schema())
    }

    /// Format an LLM-facing prose for an argument validation failure.
    ///
    /// Called by the default [`Self::call_json`] when `serde_json::from_value`
    /// rejects the model's tool arguments. The returned string is surfaced as
    /// `ToolError::ValidationFailed { cause }` (via `AlephError::Validation`)
    /// and reaches the model on the next turn. Override to inject schema-shape
    /// hints or examples that nudge the model toward a valid rewrite.
    ///
    /// Default mirrors opencode's `InvalidArgumentsError.message`: it names the
    /// tool and instructs the model to rewrite the input to match the schema.
    #[must_use]
    fn format_validation_error(err: &serde_json::Error) -> String {
        format!(
            "The {tool} tool was called with invalid arguments: {detail}. \
             Please rewrite the input so it satisfies the expected schema.",
            tool = Self::NAME,
            detail = err
        )
    }

    /// Execute the tool with typed arguments.
    ///
    /// This is the main implementation point. Implement your tool logic here.
    async fn call(&self, args: Self::Args) -> Result<Self::Output>;

    /// Execute the tool with JSON arguments.
    ///
    /// Default implementation deserializes args, calls `call()`, and serializes output.
    /// Override only for special JSON handling needs.
    ///
    /// Argument deserialization failures are returned as
    /// `AlephError::Validation(<format_validation_error output>)` so the
    /// `BuiltinHandler` can map them to `ToolError::ValidationFailed` (which
    /// the harness exposes as a fixable, non-execution failure).
    ///
    /// Before giving up on a deserialization failure, a best-effort
    /// scalar-coercion pass is retried once: the model frequently emits a
    /// JSON string where the schema declares a scalar (`{"count": "5"}` for
    /// an integer field), which would otherwise burn a whole turn on an
    /// otherwise-correct call. See [`coerce_scalar_args`] for the (deliberately
    /// conservative) recovery rules. The borrowed `&args` deserialize keeps the
    /// happy path allocation-free while leaving `args` available for recovery.
    async fn call_json(&self, args: Value) -> Result<Value> {
        let typed: Self::Args = match Self::Args::deserialize(&args) {
            Ok(v) => v,
            Err(err) => {
                // Recovery: coerce top-level string→scalar fields against this
                // tool's own schema and retry once. Falls back to the original
                // validation error so genuinely malformed input still reaches
                // the model unchanged.
                match coerce_scalar_args(&args, &self.definition().parameters)
                    .and_then(|coerced| serde_json::from_value::<Self::Args>(coerced).ok())
                {
                    Some(v) => v,
                    None => {
                        return Err(crate::error::AlephError::Validation(
                            Self::format_validation_error(&err),
                        ));
                    }
                }
            }
        };
        let output = self.call(typed).await?;
        Ok(serde_json::to_value(&output)?)
    }
}

// =============================================================================
// Argument coercion (model type-confusion recovery)
// =============================================================================

/// Best-effort recovery for model type-confusion in tool arguments.
///
/// When strict deserialization of `args` fails, the model has often sent a
/// value of the wrong JSON kind for a top-level field:
///
/// * a string where the schema declares a scalar — `{"count": "5"}` for an
///   integer field, `{"enabled": "true"}` for a boolean;
/// * a number or boolean where the schema declares a string —
///   `{"port": 8080}` for a string field;
/// * a bare value where the schema declares an array — open-weight models
///   (DeepSeek/Qwen/GLM) routinely emit `{"paths": "src/a.rs"}` (or the
///   JSON-encoded `{"paths": "[\"src/a.rs\"]"}`) for an array field. The
///   string form is first tried as embedded JSON; otherwise the value is
///   wrapped in a single-element array.
///
/// This walks the **top-level** properties of `schema` and applies the
/// matching conversion. Returns `Some(coerced)` only when at least one field
/// actually changed, so the caller can retry deserialization; `None` means
/// there was nothing safe to coerce (preserve the original validation error).
///
/// Conservative by construction — it never touches nested objects, fields
/// whose value already matches the declared kind, or fields without an
/// explicit declared type. A successful re-parse of the result against the
/// concrete `Args` type is the real safety gate: if coercion produced a shape
/// the type rejects, the caller discards it and reports the original error.
fn coerce_scalar_args(args: &Value, schema: &Value) -> Option<Value> {
    let obj = args.as_object()?;
    let props = schema.get("properties")?.as_object()?;
    let mut changed = false;
    let mut out = serde_json::Map::with_capacity(obj.len());
    for (key, val) in obj {
        let next = match props.get(key).and_then(declared_prop_type) {
            Some(ty) => match coerce_value_to_type(val, ty) {
                Some(coerced) => {
                    changed = true;
                    coerced
                }
                None => val.clone(),
            },
            None => val.clone(),
        };
        out.insert(key.clone(), next);
    }
    changed.then_some(Value::Object(out))
}

/// Convert `val` toward the declared JSON-Schema `ty`, or `None` when the
/// value already matches or no safe conversion exists (leave it unchanged).
fn coerce_value_to_type(val: &Value, ty: &'static str) -> Option<Value> {
    match ty {
        "integer" | "number" | "boolean" => val.as_str().and_then(|s| coerce_str_to_scalar(s, ty)),
        "string" => match val {
            // The model emitted a bare scalar for a string field — stringify.
            Value::Number(n) => Some(Value::String(n.to_string())),
            Value::Bool(b) => Some(Value::String(b.to_string())),
            _ => None,
        },
        "array" => {
            if val.is_array() || val.is_null() {
                return None;
            }
            // A string may be a JSON-encoded array (double-encoding drift);
            // anything else gets wrapped as a single-element array.
            if let Some(s) = val.as_str() {
                if let Ok(parsed) = serde_json::from_str::<Value>(s.trim()) {
                    if parsed.is_array() {
                        return Some(parsed);
                    }
                }
            }
            Some(Value::Array(vec![val.clone()]))
        }
        _ => None,
    }
}

/// Declared kind of a JSON-Schema property, for the kinds coercion can act on
/// (integer / number / boolean / string / array). Handles both
/// `"type": "integer"` and the nullable `"type": ["integer", "null"]` shape
/// schemars emits for `Option<T>`.
fn declared_prop_type(prop: &Value) -> Option<&'static str> {
    fn pick(s: &str) -> Option<&'static str> {
        match s {
            "integer" => Some("integer"),
            "number" => Some("number"),
            "boolean" => Some("boolean"),
            "string" => Some("string"),
            "array" => Some("array"),
            _ => None,
        }
    }
    match prop.get("type")? {
        Value::String(s) => pick(s),
        Value::Array(items) => items.iter().filter_map(Value::as_str).find_map(pick),
        _ => None,
    }
}

/// Parse a string into the requested scalar JSON value, or `None` if it does
/// not cleanly represent that scalar (leaving the original string in place).
fn coerce_str_to_scalar(s: &str, ty: &str) -> Option<Value> {
    let t = s.trim();
    match ty {
        "integer" => t
            .parse::<i64>()
            .map(Value::from)
            .ok()
            .or_else(|| t.parse::<u64>().map(Value::from).ok()),
        "number" => t
            .parse::<f64>()
            .ok()
            .and_then(|f| serde_json::Number::from_f64(f).map(Value::Number)),
        "boolean" => match t {
            "true" => Some(Value::Bool(true)),
            "false" => Some(Value::Bool(false)),
            _ => None,
        },
        _ => None,
    }
}

// =============================================================================
// AlephToolDyn - Dynamic Dispatch Trait
// =============================================================================

/// Dynamic dispatch tool trait for runtime-loaded tools.
///
/// This trait is used for:
/// - MCP (Model Context Protocol) tools loaded at runtime
/// - Plugin tools with dynamic registration
/// - Hot-reloaded tools
///
/// Unlike `AlephTool`, this trait uses `Value` for arguments and output,
/// enabling runtime flexibility at the cost of compile-time type safety.
///
/// # Object Safety
///
/// This trait is object-safe and can be used with `dyn AlephToolDyn`.
pub trait AlephToolDyn: Send + Sync {
    /// Get the tool name
    fn name(&self) -> &str;

    /// Get the tool definition
    fn definition(&self) -> ToolDefinition;

    /// Execute the tool with JSON arguments
    ///
    /// Returns a boxed future for object safety.
    fn call(&self, args: Value) -> Pin<Box<dyn Future<Output = Result<Value>> + Send + '_>>;

    /// Per-result token budget, forwarded from [`AlephTool::max_result_tokens`].
    /// Defaulted so manual `AlephToolDyn` impls (tests, adapters) need not
    /// change; the blanket impl below overrides it to forward the concrete
    /// tool's value.
    fn max_result_tokens(&self) -> Option<usize> {
        None
    }
}

// =============================================================================
// Blanket Implementation: AlephTool → AlephToolDyn
// =============================================================================

/// Blanket implementation allowing any `AlephTool` to be used as `AlephToolDyn`.
///
/// This enables storing static tools in dynamic collections:
///
/// ```rust,ignore
/// let tools: Vec<Box<dyn AlephToolDyn>> = vec![
///     Box::new(SearchTool::with_registry(registry)),
///     Box::new(WebFetchTool::new()),
/// ];
/// ```
impl<T: AlephTool> AlephToolDyn for T {
    fn name(&self) -> &str {
        T::NAME
    }

    fn definition(&self) -> ToolDefinition {
        AlephTool::definition(self)
    }

    fn call(&self, args: Value) -> Pin<Box<dyn Future<Output = Result<Value>> + Send + '_>> {
        Box::pin(async move { self.call_json(args).await })
    }

    fn max_result_tokens(&self) -> Option<usize> {
        AlephTool::max_result_tokens(self)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Clone)]
    struct TestTool;

    #[derive(Serialize, Deserialize, JsonSchema)]
    struct TestArgs {
        message: String,
    }

    #[derive(Serialize)]
    struct TestOutput {
        result: String,
    }

    #[async_trait]
    impl AlephTool for TestTool {
        const NAME: &'static str = "test_tool";
        const DESCRIPTION: &'static str = "A test tool";

        type Args = TestArgs;
        type Output = TestOutput;

        async fn call(&self, args: Self::Args) -> Result<Self::Output> {
            Ok(TestOutput {
                result: format!("Echo: {}", args.message),
            })
        }
    }

    #[test]
    fn test_tool_definition() {
        let tool = TestTool;
        // Use fully qualified syntax to avoid ambiguity with blanket impl
        let def = AlephTool::definition(&tool);

        assert_eq!(def.name, "test_tool");
        assert_eq!(def.description, "A test tool");
        assert!(!def.requires_confirmation);
    }

    #[tokio::test]
    async fn test_tool_call() {
        let tool = TestTool;
        // Use fully qualified syntax to avoid ambiguity with blanket impl
        let result = AlephTool::call(
            &tool,
            TestArgs {
                message: "hello".to_string(),
            },
        )
        .await
        .unwrap();

        assert_eq!(result.result, "Echo: hello");
    }

    #[tokio::test]
    async fn test_tool_call_json() {
        let tool = TestTool;
        let args = serde_json::json!({ "message": "world" });
        let result = AlephTool::call_json(&tool, args).await.unwrap();

        assert_eq!(result["result"], "Echo: world");
    }

    #[tokio::test]
    async fn test_tool_dyn_dispatch() {
        let tool: Box<dyn AlephToolDyn> = Box::new(TestTool);

        assert_eq!(tool.name(), "test_tool");

        let args = serde_json::json!({ "message": "dynamic" });
        let result = tool.call(args).await.unwrap();

        assert_eq!(result["result"], "Echo: dynamic");
    }

    // -- Argument coercion (model type-confusion recovery) --------------------

    #[derive(Clone)]
    struct CoerceTool;

    #[derive(Serialize, Deserialize, JsonSchema)]
    struct CoerceArgs {
        count: u32,
        ratio: f64,
        enabled: bool,
        label: String,
        note: Option<i64>,
        flexible: serde_json::Value,
    }

    #[derive(Serialize)]
    struct CoerceOutput {
        count: u32,
        ratio: f64,
        enabled: bool,
        label: String,
        note: Option<i64>,
        flexible: serde_json::Value,
    }

    #[async_trait]
    impl AlephTool for CoerceTool {
        const NAME: &'static str = "coerce_tool";
        const DESCRIPTION: &'static str = "A tool with scalar args";

        type Args = CoerceArgs;
        type Output = CoerceOutput;

        async fn call(&self, a: Self::Args) -> Result<Self::Output> {
            Ok(CoerceOutput {
                count: a.count,
                ratio: a.ratio,
                enabled: a.enabled,
                label: a.label,
                note: a.note,
                flexible: a.flexible,
            })
        }
    }

    #[tokio::test]
    async fn call_json_coerces_string_scalars_but_protects_strings_and_untyped() {
        // Model sent every scalar as a JSON string. `label` is schema-typed
        // `string` and `flexible` is untyped `Value`; both must survive as the
        // literal string "…", while count/ratio/enabled/note coerce.
        let args = serde_json::json!({
            "count": "5",
            "ratio": "1.5",
            "enabled": "true",
            "label": "7",
            "note": "3",
            "flexible": "123",
        });
        let r = AlephTool::call_json(&CoerceTool, args).await.unwrap();
        assert_eq!(r["count"], 5);
        assert_eq!(r["ratio"], 1.5);
        assert_eq!(r["enabled"], true);
        assert_eq!(r["label"], "7"); // string field never coerced
        assert_eq!(r["note"], 3);
        assert_eq!(r["flexible"], "123"); // untyped Value never coerced
    }

    #[tokio::test]
    async fn call_json_happy_path_is_unchanged() {
        let args = serde_json::json!({
            "count": 5,
            "ratio": 1.5,
            "enabled": true,
            "label": "x",
            "note": null,
            "flexible": 1,
        });
        let r = AlephTool::call_json(&CoerceTool, args).await.unwrap();
        assert_eq!(r["count"], 5);
        assert_eq!(r["note"], serde_json::Value::Null);
    }

    #[tokio::test]
    async fn call_json_uncoercible_input_returns_validation_error() {
        // "abc" cannot become an integer → coercion finds nothing safe to
        // change and the original validation error is preserved.
        let args = serde_json::json!({
            "count": "abc",
            "ratio": 1.5,
            "enabled": true,
            "label": "x",
            "note": null,
            "flexible": 1,
        });
        assert!(AlephTool::call_json(&CoerceTool, args).await.is_err());
    }

    #[test]
    fn coerce_returns_none_when_nothing_to_change() {
        let schema = serde_json::json!({"properties": {"count": {"type": "integer"}}});
        // already an integer → not a string → no change.
        assert!(coerce_scalar_args(&serde_json::json!({"count": 5}), &schema).is_none());
        // string value whose schema type is string → never coerced.
        let schema2 = serde_json::json!({"properties": {"label": {"type": "string"}}});
        assert!(coerce_scalar_args(&serde_json::json!({"label": "5"}), &schema2).is_none());
    }

    #[test]
    fn coerce_handles_nullable_and_scalars() {
        let schema = serde_json::json!({"properties": {
            "n": {"type": ["integer", "null"]},
            "f": {"type": "number"},
            "b": {"type": "boolean"},
        }});
        let out = coerce_scalar_args(
            &serde_json::json!({"n": "7", "f": "2.5", "b": "false"}),
            &schema,
        )
        .unwrap();
        assert_eq!(out["n"], 7);
        assert_eq!(out["f"], 2.5);
        assert_eq!(out["b"], false);
    }

    #[test]
    fn declared_prop_type_reads_array_and_string() {
        assert_eq!(
            declared_prop_type(&serde_json::json!({"type": "integer"})),
            Some("integer")
        );
        assert_eq!(
            declared_prop_type(&serde_json::json!({"type": ["number", "null"]})),
            Some("number")
        );
        assert_eq!(
            declared_prop_type(&serde_json::json!({"type": "string"})),
            Some("string")
        );
        assert_eq!(
            declared_prop_type(&serde_json::json!({"type": "array"})),
            Some("array")
        );
        assert_eq!(declared_prop_type(&serde_json::json!({})), None);
        // Object-typed fields are never coercion targets.
        assert_eq!(
            declared_prop_type(&serde_json::json!({"type": "object"})),
            None
        );
    }

    #[test]
    fn coerce_wraps_bare_value_for_array_field() {
        let schema = serde_json::json!({"properties": {"paths": {"type": "array"}}});
        // Bare string → single-element array.
        let out = coerce_scalar_args(&serde_json::json!({"paths": "src/a.rs"}), &schema).unwrap();
        assert_eq!(out["paths"], serde_json::json!(["src/a.rs"]));
        // Bare number → single-element array.
        let out = coerce_scalar_args(&serde_json::json!({"paths": 5}), &schema).unwrap();
        assert_eq!(out["paths"], serde_json::json!([5]));
        // Already an array → nothing to change.
        assert!(coerce_scalar_args(&serde_json::json!({"paths": ["a"]}), &schema).is_none());
        // Null (e.g. omitted Option) → nothing to change.
        assert!(coerce_scalar_args(&serde_json::json!({"paths": null}), &schema).is_none());
    }

    #[test]
    fn coerce_parses_json_encoded_array_string() {
        // Double-encoding drift: the model wrapped the array in a string.
        let schema = serde_json::json!({"properties": {"paths": {"type": "array"}}});
        let out = coerce_scalar_args(
            &serde_json::json!({"paths": "[\"src/a.rs\", \"src/b.rs\"]"}),
            &schema,
        )
        .unwrap();
        assert_eq!(out["paths"], serde_json::json!(["src/a.rs", "src/b.rs"]));
        // A string that parses to a non-array still gets wrapped, not parsed.
        let out = coerce_scalar_args(&serde_json::json!({"paths": "{\"x\":1}"}), &schema).unwrap();
        assert_eq!(out["paths"], serde_json::json!(["{\"x\":1}"]));
    }

    #[test]
    fn coerce_stringifies_bare_scalar_for_string_field() {
        let schema = serde_json::json!({"properties": {"port": {"type": "string"}}});
        let out = coerce_scalar_args(&serde_json::json!({"port": 8080}), &schema).unwrap();
        assert_eq!(out["port"], "8080");
        let out = coerce_scalar_args(&serde_json::json!({"port": true}), &schema).unwrap();
        assert_eq!(out["port"], "true");
        // A string value for a string field stays untouched.
        assert!(coerce_scalar_args(&serde_json::json!({"port": "8080"}), &schema).is_none());
    }
}
