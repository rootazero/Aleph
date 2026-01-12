//! Type-Safe Tool Handler Trait
//!
//! This module provides the `ToolHandler` trait for type-safe tool execution
//! with automatic JSON Schema generation using schemars.
//!
//! # Architecture
//!
//! ```text
//! ToolHandler<P: ToolParams>
//!     ├── definition() -> ToolHandlerDef (auto-generated schema)
//!     ├── execute(params: P) -> ToolOutput (type-safe execution)
//!     └── execute_raw(json: &str) -> ToolOutput (for dynamic dispatch)
//! ```
//!
//! # Example
//!
//! ```rust,ignore
//! use crate::tools::handler::{ToolHandler, ToolHandlerDef};
//! use crate::tools::params::{SearchParams, ToolOutput, ToolParams};
//! use crate::routing::ToolSafetyLevel;
//!
//! pub struct SearchHandler {
//!     // ... handler state
//! }
//!
//! #[async_trait]
//! impl ToolHandler<SearchParams> for SearchHandler {
//!     fn name(&self) -> &str { "search" }
//!
//!     fn description(&self) -> &str { "Search the web for information" }
//!
//!     async fn execute(&self, params: SearchParams) -> ToolOutput {
//!         // Perform search with typed params
//!         ToolOutput::success(results, "Search complete")
//!     }
//! }
//! ```

use async_trait::async_trait;
use serde_json::Value;

use crate::routing::ToolSafetyLevel;
use crate::tools::params::{ToolOutput, ToolParams};

// =============================================================================
// ToolHandlerDef
// =============================================================================

/// Tool definition for LLM function calling
///
/// This struct contains all the information needed to register a tool
/// with an LLM for function calling, including the JSON Schema.
#[derive(Debug, Clone)]
pub struct ToolHandlerDef {
    /// Tool name (used in function calls)
    pub name: String,

    /// Human-readable description
    pub description: String,

    /// JSON Schema for parameters
    pub parameters: Value,

    /// Safety level for this tool
    pub safety_level: ToolSafetyLevel,

    /// Whether this tool requires user confirmation
    pub requires_confirmation: bool,
}

impl ToolHandlerDef {
    /// Create a new tool definition
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
            safety_level: ToolSafetyLevel::default(),
            requires_confirmation: false,
        }
    }

    /// Builder: set safety level
    pub fn with_safety_level(mut self, level: ToolSafetyLevel) -> Self {
        self.safety_level = level;
        self.requires_confirmation = level.requires_confirmation();
        self
    }

    /// Builder: set requires confirmation
    pub fn with_requires_confirmation(mut self, requires: bool) -> Self {
        self.requires_confirmation = requires;
        self
    }

    /// Convert to OpenAI function calling format
    pub fn to_openai_function(&self) -> Value {
        serde_json::json!({
            "type": "function",
            "function": {
                "name": self.name,
                "description": self.description,
                "parameters": self.parameters,
            }
        })
    }

    /// Convert to Anthropic tool format
    pub fn to_anthropic_tool(&self) -> Value {
        serde_json::json!({
            "name": self.name,
            "description": self.description,
            "input_schema": self.parameters,
        })
    }
}

// =============================================================================
// ToolHandler Trait
// =============================================================================

/// Type-safe tool handler trait
///
/// This trait provides a type-safe interface for tool execution with
/// automatic JSON Schema generation from the parameter type.
///
/// # Type Parameters
///
/// * `P` - The parameter type, must implement `ToolParams`
///
/// # Implementation Notes
///
/// The trait provides a default implementation of `execute_raw` that:
/// 1. Deserializes the JSON string to the parameter type
/// 2. Calls the typed `execute` method
///
/// Implementors only need to provide `name`, `description`, and `execute`.
#[async_trait]
pub trait ToolHandler<P: ToolParams>: Send + Sync {
    /// Get the tool name
    fn name(&self) -> &str;

    /// Get the tool description
    fn description(&self) -> &str;

    /// Get the tool's safety level
    ///
    /// Override this to specify a different safety level.
    /// Default: ReadOnly
    fn safety_level(&self) -> ToolSafetyLevel {
        ToolSafetyLevel::default()
    }

    /// Execute the tool with typed parameters
    async fn execute(&self, params: P) -> ToolOutput;

    /// Get the tool definition (auto-generated)
    ///
    /// This method generates a `ToolHandlerDef` with the JSON Schema
    /// automatically derived from the parameter type.
    fn definition(&self) -> ToolHandlerDef {
        ToolHandlerDef::new(self.name(), self.description(), P::schema_object())
            .with_safety_level(self.safety_level())
    }

    /// Execute the tool with raw JSON parameters
    ///
    /// This method is used for dynamic dispatch when the parameter type
    /// is not known at compile time. It deserializes the JSON and calls
    /// the typed `execute` method.
    async fn execute_raw(&self, params_json: &str) -> ToolOutput {
        match serde_json::from_str::<P>(params_json) {
            Ok(params) => self.execute(params).await,
            Err(e) => ToolOutput::failure(format!("Invalid parameters: {}", e)),
        }
    }

    /// Execute the tool with a JSON Value
    ///
    /// This is a convenience method that accepts a `serde_json::Value`
    /// instead of a string.
    async fn execute_value(&self, params: Value) -> ToolOutput {
        match serde_json::from_value::<P>(params) {
            Ok(params) => self.execute(params).await,
            Err(e) => ToolOutput::failure(format!("Invalid parameters: {}", e)),
        }
    }
}

// =============================================================================
// DynToolHandler
// =============================================================================

/// Type-erased wrapper for dynamic tool dispatch
///
/// This wrapper allows storing different `ToolHandler` implementations
/// with different parameter types in the same collection.
#[async_trait]
pub trait DynToolHandler: Send + Sync {
    /// Get the tool name
    fn name(&self) -> &str;

    /// Get the tool description
    fn description(&self) -> &str;

    /// Get the tool's safety level
    fn safety_level(&self) -> ToolSafetyLevel;

    /// Get the tool definition
    fn definition(&self) -> ToolHandlerDef;

    /// Execute with raw JSON parameters
    async fn execute_raw(&self, params_json: &str) -> ToolOutput;

    /// Execute with a JSON Value
    async fn execute_value(&self, params: Value) -> ToolOutput;
}

/// Wrapper to convert a typed ToolHandler into a DynToolHandler
///
/// This struct wraps a `ToolHandler<P>` and implements `DynToolHandler`,
/// allowing it to be stored in a type-erased collection.
pub struct TypedHandlerWrapper<P: ToolParams + 'static, T: ToolHandler<P>> {
    handler: T,
    _marker: std::marker::PhantomData<P>,
}

impl<P: ToolParams + 'static, T: ToolHandler<P>> TypedHandlerWrapper<P, T> {
    /// Create a new wrapper around a typed handler
    pub fn new(handler: T) -> Self {
        Self {
            handler,
            _marker: std::marker::PhantomData,
        }
    }
}

#[async_trait]
impl<P: ToolParams + 'static, T: ToolHandler<P>> DynToolHandler for TypedHandlerWrapper<P, T> {
    fn name(&self) -> &str {
        self.handler.name()
    }

    fn description(&self) -> &str {
        self.handler.description()
    }

    fn safety_level(&self) -> ToolSafetyLevel {
        self.handler.safety_level()
    }

    fn definition(&self) -> ToolHandlerDef {
        self.handler.definition()
    }

    async fn execute_raw(&self, params_json: &str) -> ToolOutput {
        self.handler.execute_raw(params_json).await
    }

    async fn execute_value(&self, params: Value) -> ToolOutput {
        self.handler.execute_value(params).await
    }
}

/// Helper function to wrap a typed handler into a boxed DynToolHandler
pub fn wrap_handler<P: ToolParams + 'static, T: ToolHandler<P> + 'static>(
    handler: T,
) -> Box<dyn DynToolHandler> {
    Box::new(TypedHandlerWrapper::new(handler))
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::params::SearchParams;

    /// Test handler for unit tests
    struct TestSearchHandler;

    #[async_trait]
    impl ToolHandler<SearchParams> for TestSearchHandler {
        fn name(&self) -> &str {
            "test_search"
        }

        fn description(&self) -> &str {
            "Test search tool"
        }

        fn safety_level(&self) -> ToolSafetyLevel {
            ToolSafetyLevel::ReadOnly
        }

        async fn execute(&self, params: SearchParams) -> ToolOutput {
            ToolOutput::success(
                serde_json::json!({
                    "query": params.query,
                    "max_results": params.max_results,
                    "results": ["result1", "result2"]
                }),
                format!("Found results for: {}", params.query),
            )
        }
    }

    #[test]
    fn test_tool_handler_definition() {
        let handler = TestSearchHandler;
        let def = handler.definition();

        assert_eq!(def.name, "test_search");
        assert_eq!(def.description, "Test search tool");
        assert_eq!(def.safety_level, ToolSafetyLevel::ReadOnly);
        assert!(!def.requires_confirmation);

        // Check that parameters schema contains expected fields
        let params = def.parameters.as_object().unwrap();
        assert!(params.contains_key("properties") || params.contains_key("type"));
    }

    #[test]
    fn test_tool_handler_def_openai_format() {
        let def = ToolHandlerDef::new(
            "search",
            "Search the web",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" }
                },
                "required": ["query"]
            }),
        );

        let openai = def.to_openai_function();
        assert_eq!(openai["type"], "function");
        assert_eq!(openai["function"]["name"], "search");
    }

    #[test]
    fn test_tool_handler_def_anthropic_format() {
        let def = ToolHandlerDef::new(
            "search",
            "Search the web",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" }
                }
            }),
        );

        let anthropic = def.to_anthropic_tool();
        assert_eq!(anthropic["name"], "search");
        assert!(anthropic["input_schema"].is_object());
    }

    #[tokio::test]
    async fn test_tool_handler_execute() {
        let handler = TestSearchHandler;
        let params = SearchParams {
            query: "test query".to_string(),
            max_results: 10,
        };

        let output = handler.execute(params).await;
        assert!(output.success);
        assert!(output.message.contains("test query"));
    }

    #[tokio::test]
    async fn test_tool_handler_execute_raw() {
        let handler = TestSearchHandler;
        let params_json = r#"{"query": "test query", "max_results": 5}"#;

        let output = handler.execute_raw(params_json).await;
        assert!(output.success);
    }

    #[tokio::test]
    async fn test_tool_handler_execute_raw_invalid() {
        let handler = TestSearchHandler;
        let params_json = r#"{"invalid": "params"}"#;

        let output = handler.execute_raw(params_json).await;
        assert!(!output.success);
        assert!(output.error.is_some());
    }

    #[tokio::test]
    async fn test_tool_handler_execute_value() {
        let handler = TestSearchHandler;
        let params = serde_json::json!({
            "query": "value test",
            "max_results": 3
        });

        let output = handler.execute_value(params).await;
        assert!(output.success);
    }

    #[test]
    fn test_tool_handler_def_safety_level() {
        let def = ToolHandlerDef::new("delete", "Delete files", serde_json::json!({}))
            .with_safety_level(ToolSafetyLevel::IrreversibleHighRisk);

        assert_eq!(def.safety_level, ToolSafetyLevel::IrreversibleHighRisk);
        assert!(def.requires_confirmation);
    }

    #[test]
    fn test_dyn_tool_handler() {
        let handler: Box<dyn DynToolHandler> = wrap_handler(TestSearchHandler);

        assert_eq!(handler.name(), "test_search");
        assert_eq!(handler.safety_level(), ToolSafetyLevel::ReadOnly);

        let def = handler.definition();
        assert_eq!(def.name, "test_search");
    }

    #[test]
    fn test_typed_handler_wrapper() {
        let wrapper = TypedHandlerWrapper::new(TestSearchHandler);
        assert_eq!(wrapper.name(), "test_search");
        assert_eq!(wrapper.description(), "Test search tool");
    }
}
