//! Tool Traits
//!
//! Defines the core tool traits for Aether's tool system.

use async_trait::async_trait;
use schemars::{schema_for, JsonSchema};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

use crate::dispatcher::{ToolCategory, ToolDefinition};
use crate::error::Result;

// =============================================================================
// AetherTool - Static Dispatch Trait
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
/// use crate::tools::AetherTool;
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
/// impl AetherTool for SearchTool {
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
pub trait AetherTool: Clone + Send + Sync + 'static {
    /// Tool name used in function calls (e.g., "search", "file_read")
    const NAME: &'static str;

    /// Human-readable description for LLM tool selection
    const DESCRIPTION: &'static str;

    /// Input argument type (must derive JsonSchema for auto-schema generation)
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

    /// Provide usage examples for LLM context (Few-shot learning).
    ///
    /// Returns a list of example usage strings that demonstrate how to use the tool.
    /// These examples are injected into the tool definition's `llm_context` field
    /// to help the LLM understand proper usage patterns.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// fn examples(&self) -> Option<Vec<String>> {
    ///     Some(vec![
    ///         "search(query='Rust async patterns', max_results=5)".to_string(),
    ///         "search(query='Machine learning basics')".to_string(),
    ///     ])
    /// }
    /// ```
    ///
    /// Default implementation returns None (no examples).
    fn examples(&self) -> Option<Vec<String>> {
        None
    }

    /// Get tool definition with auto-generated JSON Schema.
    ///
    /// The default implementation generates the schema from `Self::Args`
    /// and includes examples in the `llm_context` field if provided.
    /// Override only if custom schema handling is needed.
    fn definition(&self) -> ToolDefinition {
        let schema = schema_for!(Self::Args);
        let parameters = serde_json::to_value(&schema).unwrap_or_default();

        let mut def = ToolDefinition::new(Self::NAME, Self::DESCRIPTION, parameters, self.category())
            .with_confirmation(self.requires_confirmation());

        // Inject examples as llm_context if available
        if let Some(examples) = self.examples() {
            let examples_text = examples
                .iter()
                .enumerate()
                .map(|(i, ex)| format!("{}. {}", i + 1, ex))
                .collect::<Vec<_>>()
                .join("\n");

            let context = format!("## Usage Examples\n\n{}", examples_text);
            def = def.with_llm_context(context);
        }

        def
    }

    /// Execute the tool with typed arguments.
    ///
    /// This is the main implementation point. Implement your tool logic here.
    async fn call(&self, args: Self::Args) -> Result<Self::Output>;

    /// Execute the tool with JSON arguments.
    ///
    /// Default implementation deserializes args, calls `call()`, and serializes output.
    /// Override only for special JSON handling needs.
    async fn call_json(&self, args: Value) -> Result<Value> {
        let typed: Self::Args = serde_json::from_value(args)?;
        let output = self.call(typed).await?;
        Ok(serde_json::to_value(&output)?)
    }
}

// =============================================================================
// AetherToolDyn - Dynamic Dispatch Trait
// =============================================================================

/// Dynamic dispatch tool trait for runtime-loaded tools.
///
/// This trait is used for:
/// - MCP (Model Context Protocol) tools loaded at runtime
/// - Plugin tools with dynamic registration
/// - Hot-reloaded tools
///
/// Unlike `AetherTool`, this trait uses `Value` for arguments and output,
/// enabling runtime flexibility at the cost of compile-time type safety.
///
/// # Object Safety
///
/// This trait is object-safe and can be used with `dyn AetherToolDyn`.
pub trait AetherToolDyn: Send + Sync {
    /// Get the tool name
    fn name(&self) -> &str;

    /// Get the tool definition
    fn definition(&self) -> ToolDefinition;

    /// Execute the tool with JSON arguments
    ///
    /// Returns a boxed future for object safety.
    fn call(&self, args: Value) -> Pin<Box<dyn Future<Output = Result<Value>> + Send + '_>>;
}

// =============================================================================
// Blanket Implementation: AetherTool → AetherToolDyn
// =============================================================================

/// Blanket implementation allowing any `AetherTool` to be used as `AetherToolDyn`.
///
/// This enables storing static tools in dynamic collections:
///
/// ```rust,ignore
/// let tools: Vec<Box<dyn AetherToolDyn>> = vec![
///     Box::new(SearchTool::new()),
///     Box::new(WebFetchTool::new()),
/// ];
/// ```
impl<T: AetherTool> AetherToolDyn for T {
    fn name(&self) -> &str {
        T::NAME
    }

    fn definition(&self) -> ToolDefinition {
        AetherTool::definition(self)
    }

    fn call(&self, args: Value) -> Pin<Box<dyn Future<Output = Result<Value>> + Send + '_>> {
        Box::pin(async move { self.call_json(args).await })
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
    impl AetherTool for TestTool {
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
        let def = AetherTool::definition(&tool);

        assert_eq!(def.name, "test_tool");
        assert_eq!(def.description, "A test tool");
        assert!(!def.requires_confirmation);
    }

    #[tokio::test]
    async fn test_tool_call() {
        let tool = TestTool;
        // Use fully qualified syntax to avoid ambiguity with blanket impl
        let result = AetherTool::call(&tool, TestArgs {
            message: "hello".to_string(),
        })
        .await
        .unwrap();

        assert_eq!(result.result, "Echo: hello");
    }

    #[tokio::test]
    async fn test_tool_call_json() {
        let tool = TestTool;
        let args = serde_json::json!({ "message": "world" });
        let result = AetherTool::call_json(&tool, args).await.unwrap();

        assert_eq!(result["result"], "Echo: world");
    }

    #[tokio::test]
    async fn test_tool_dyn_dispatch() {
        let tool: Box<dyn AetherToolDyn> = Box::new(TestTool);

        assert_eq!(tool.name(), "test_tool");

        let args = serde_json::json!({ "message": "dynamic" });
        let result = tool.call(args).await.unwrap();

        assert_eq!(result["result"], "Echo: dynamic");
    }

    // Tool with examples for testing
    #[derive(Clone)]
    struct ExampleTool;

    #[async_trait]
    impl AetherTool for ExampleTool {
        const NAME: &'static str = "example_tool";
        const DESCRIPTION: &'static str = "A tool with examples";

        type Args = TestArgs;
        type Output = TestOutput;

        fn examples(&self) -> Option<Vec<String>> {
            Some(vec![
                "example_tool(message='Hello World')".to_string(),
                "example_tool(message='Test example')".to_string(),
            ])
        }

        async fn call(&self, args: Self::Args) -> Result<Self::Output> {
            Ok(TestOutput {
                result: format!("Example: {}", args.message),
            })
        }
    }

    #[test]
    fn test_tool_with_examples() {
        let tool = ExampleTool;
        let def = AetherTool::definition(&tool);

        assert_eq!(def.name, "example_tool");
        assert!(def.llm_context.is_some());

        let context = def.llm_context.unwrap();
        assert!(context.contains("## Usage Examples"));
        assert!(context.contains("1. example_tool(message='Hello World')"));
        assert!(context.contains("2. example_tool(message='Test example')"));
    }

    #[test]
    fn test_tool_without_examples() {
        let tool = TestTool;
        let def = AetherTool::definition(&tool);

        assert_eq!(def.name, "test_tool");
        assert!(def.llm_context.is_none());
    }
}
