//! Tests for the live surface of `AlephToolServer`.

use serde_json::Value;

use super::AlephToolServer;
use crate::error::Result;
use crate::tool_metadata::ToolDefinition;
use crate::tools::traits::AlephToolDyn;

struct DynamicMockTool {
    name: String,
}

impl AlephToolDyn for DynamicMockTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            &self.name,
            "A dynamic mock tool for testing",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "input": { "type": "string" }
                },
                "required": ["input"]
            }),
            crate::tool_metadata::ToolCategory::Builtin,
        )
    }

    fn call(
        &self,
        _args: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value>> + Send + '_>> {
        let name = self.name.clone();
        Box::pin(async move { Ok(serde_json::json!({ "name": name })) })
    }
}

#[tokio::test]
async fn replace_tool_registers_a_new_tool() {
    let server = AlephToolServer::new();

    let info = server
        .replace_tool(DynamicMockTool {
            name: "alpha".to_string(),
        })
        .await;
    assert_eq!(info.tool_name, "alpha");
    assert!(!info.was_replaced);

    let tools = server.list_tools_arc().await;
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name(), "alpha");
}

#[tokio::test]
async fn replace_tool_overwrites_an_existing_entry() {
    let server = AlephToolServer::new();

    server
        .replace_tool(DynamicMockTool {
            name: "beta".to_string(),
        })
        .await;
    let info = server
        .replace_tool(DynamicMockTool {
            name: "beta".to_string(),
        })
        .await;
    assert_eq!(info.tool_name, "beta");
    assert!(info.was_replaced);

    let tools = server.list_tools_arc().await;
    assert_eq!(tools.len(), 1);
}
