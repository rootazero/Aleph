//! `get_tool_schema` — on-demand loader for the full input schema of a tool
//! whose schema was collapsed by `ProgressiveDisclosureRewriter`. Registered
//! per-request with a snapshot of every tool's ORIGINAL (pre-collapse) schema.

use std::collections::HashMap;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::sync_primitives::Arc;
use crate::tools::runtime::{LoopTool, ToolResult};

/// Serves original tool schemas from a per-request snapshot (`name → schema`).
pub struct SchemaLookupTool {
    schemas: Arc<HashMap<String, Value>>,
}

impl SchemaLookupTool {
    /// Tool name advertised to the model.
    pub const NAME: &'static str = "get_tool_schema";

    #[must_use]
    pub fn new(schemas: Arc<HashMap<String, Value>>) -> Self {
        Self { schemas }
    }
}

#[async_trait]
impl LoopTool for SchemaLookupTool {
    fn name(&self) -> &str {
        Self::NAME
    }

    // Pure read over an immutable schema snapshot — safe to run alongside any
    // other concurrent-safe call (the trait default is fail-closed `false`).
    fn is_concurrent_safe(&self, _input: &Value) -> bool {
        true
    }

    fn description(&self) -> &str {
        "Load the full JSON input schema for a tool whose parameters are collapsed. \
         Call this with the tool's exact name before invoking any tool whose description \
         says '[Parameters collapsed …]', then call that tool with the returned parameters."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "tool_name": { "type": "string", "description": "Exact name of the tool to load the schema for." }
            },
            "required": ["tool_name"]
        })
    }

    async fn execute(&self, input: Value, _cancel: CancellationToken) -> ToolResult {
        let name = input
            .get("tool_name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if name.is_empty() {
            return ToolResult::Error {
                error: "get_tool_schema requires a non-empty `tool_name`.".to_string(),
                retryable: false,
            };
        }
        if let Some(schema) = self.schemas.get(name) {
            ToolResult::Success {
                output: json!({ "found": true, "name": name, "parameters": schema }),
            }
        } else {
            let offered: Vec<&str> = self.schemas.keys().map(String::as_str).collect();
            let suggestions = crate::tools::name_repair::suggest_candidates(name, &offered, 5);
            ToolResult::Success {
                output: json!({
                    "found": false,
                    "name": name,
                    "error": format!("No tool named '{name}'."),
                    "suggestions": suggestions,
                }),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{SchemaLookupTool, ToolResult};
    use crate::tools::runtime::LoopTool;
    use serde_json::json;
    use tokio_util::sync::CancellationToken;

    fn tool() -> SchemaLookupTool {
        let mut m = std::collections::HashMap::new();
        m.insert(
            "browser_navigate".to_string(),
            json!({"type":"object","properties":{"url":{"type":"string"}}}),
        );
        SchemaLookupTool::new(std::sync::Arc::new(m))
    }

    #[tokio::test]
    async fn returns_full_schema_when_found() {
        let out = tool()
            .execute(
                json!({"tool_name":"browser_navigate"}),
                CancellationToken::new(),
            )
            .await;
        match out {
            ToolResult::Success { output } => {
                assert_eq!(output["found"], json!(true));
                assert!(output["parameters"]["properties"]["url"].is_object());
            }
            other => panic!("expected success, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn suggests_on_miss() {
        let out = tool()
            .execute(
                json!({"tool_name":"browser_navigat"}),
                CancellationToken::new(),
            )
            .await;
        match out {
            ToolResult::Success { output } => {
                assert_eq!(output["found"], json!(false));
                assert!(output["suggestions"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|s| s == "browser_navigate"));
            }
            other => panic!("expected success, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn errors_on_empty_name() {
        let out = tool().execute(json!({}), CancellationToken::new()).await;
        assert!(matches!(out, ToolResult::Error { .. }));
    }
}
