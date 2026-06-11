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

use crate::session::events::{ToolOutput, ToolOutputMetadata};
use crate::tools::handlers::ToolHandler;
use crate::tools::service::{ToolDefinition, ToolDefinitionMetadata, ToolError, ToolSource};
use crate::tools::AlephToolDyn;

pub struct BuiltinHandler {
    inner: Arc<dyn AlephToolDyn>,
    name: String,
}

impl BuiltinHandler {
    pub fn new(name: String, inner: Arc<dyn AlephToolDyn>) -> Self {
        Self { inner, name }
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
        let max_duration_ms = crate::tools::budget::builtin_tool_budget_ms(&self.name);
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
                max_duration_ms,
                // The static-dispatch `AlephTool` trait surface this handler
                // wraps doesn't expose a concurrent-safety hint. The
                // LoopTool-based production path (`ScopedToolService`) gets
                // the real flag from `LoopTool::is_concurrent_safe`; this
                // handler-path falls back to a conservative `false` and
                // is therefore never picked up by the parallel fast path.
                concurrent_safe: false,
            },
        }
    }
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
    fn definition_leaves_max_duration_ms_none_for_unlisted_tool() {
        let handler = BuiltinHandler::new("unknown_custom_tool".to_string(), Arc::new(FakeTool));
        let def = handler.definition();
        assert_eq!(def.metadata.max_duration_ms, None);
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
}
