//! Tests for tool server and handle.

use serde_json::Value;

use super::AlephToolServer;
use crate::dispatcher::ToolDefinition;
use crate::error::{AlephError, Result};
use crate::tools::traits::AlephToolDyn;
use crate::tools::types::ToolRepairType;

/// A pure dynamic tool for testing (only implements AlephToolDyn, not AlephTool)
/// This allows dynamic name configuration needed for server tests.
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
            crate::dispatcher::ToolCategory::Builtin,
        )
    }

    fn call(
        &self,
        args: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value>> + Send + '_>> {
        let name = self.name.clone();
        Box::pin(async move {
            let input = args
                .get("input")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            Ok(serde_json::json!({ "output": format!("{}: {}", name, input) }))
        })
    }
}

#[tokio::test]
async fn test_server_add_and_call() {
    let server = AlephToolServer::new();

    server
        .add_tool(DynamicMockTool {
            name: "test".to_string(),
        })
        .await;

    assert!(server.has_tool("test").await);
    assert_eq!(server.len().await, 1);

    let result = server
        .call("test", serde_json::json!({"input": "hello"}))
        .await
        .unwrap();

    assert_eq!(result["output"], "test: hello");
}

#[tokio::test]
async fn test_server_remove_tool() {
    let server = AlephToolServer::new();

    server
        .add_tool(DynamicMockTool {
            name: "removable".to_string(),
        })
        .await;

    assert!(server.has_tool("removable").await);
    assert!(server.remove_tool("removable").await);
    assert!(!server.has_tool("removable").await);
    assert!(!server.remove_tool("nonexistent").await);
}

#[tokio::test]
async fn test_server_list_tools() {
    let server = AlephToolServer::new();

    server
        .add_tool(DynamicMockTool {
            name: "tool1".to_string(),
        })
        .await;
    server
        .add_tool(DynamicMockTool {
            name: "tool2".to_string(),
        })
        .await;

    let names = server.list_names().await;
    assert_eq!(names.len(), 2);
    assert!(names.contains(&"tool1".to_string()));
    assert!(names.contains(&"tool2".to_string()));

    let definitions = server.list_definitions().await;
    assert_eq!(definitions.len(), 2);
}

#[tokio::test]
async fn test_server_tool_not_found() {
    let server = AlephToolServer::new();

    let result = server.call("nonexistent", serde_json::json!({})).await;

    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        AlephError::ToolNotFound { .. }
    ));
}

#[tokio::test]
async fn test_server_handle() {
    let server = AlephToolServer::new();
    let handle = server.handle();

    // Add via server
    server
        .add_tool(DynamicMockTool {
            name: "shared".to_string(),
        })
        .await;

    // Access via handle
    assert!(handle.has_tool("shared").await);

    let result = handle
        .call("shared", serde_json::json!({"input": "test"}))
        .await
        .unwrap();

    assert_eq!(result["output"], "shared: test");
}

#[tokio::test]
async fn test_handle_clone() {
    let server = AlephToolServer::new();
    server
        .add_tool(DynamicMockTool {
            name: "cloned".to_string(),
        })
        .await;

    let handle1 = server.handle();
    let handle2 = handle1.clone();

    // Both handles see the same tools
    assert!(handle1.has_tool("cloned").await);
    assert!(handle2.has_tool("cloned").await);

    // Modifications via one handle are visible to the other
    handle1.remove_tool("cloned").await;
    assert!(!handle2.has_tool("cloned").await);
}

#[tokio::test]
async fn test_server_clear() {
    let server = AlephToolServer::new();

    server
        .add_tool(DynamicMockTool {
            name: "t1".to_string(),
        })
        .await;
    server
        .add_tool(DynamicMockTool {
            name: "t2".to_string(),
        })
        .await;

    assert_eq!(server.len().await, 2);

    server.clear().await;

    assert!(server.is_empty().await);
}

#[tokio::test]
async fn test_call_with_repair_exact_match() {
    let server = AlephToolServer::new();
    server
        .add_tool(DynamicMockTool {
            name: "search".to_string(),
        })
        .await;

    let (result, repair_info) = server
        .call_with_repair("search", serde_json::json!({"input": "test"}))
        .await;

    assert!(result.is_ok());
    assert!(repair_info.is_none()); // No repair needed
}

#[tokio::test]
async fn test_call_with_repair_case_insensitive() {
    let server = AlephToolServer::new();
    server
        .add_tool(DynamicMockTool {
            name: "search".to_string(),
        })
        .await;

    let (result, repair_info) = server
        .call_with_repair("Search", serde_json::json!({"input": "test"}))
        .await;

    assert!(result.is_ok());
    assert!(repair_info.is_some());
    let info = repair_info.unwrap();
    assert_eq!(info.original_name, "Search");
    assert_eq!(info.repaired_name, "search");
    assert_eq!(info.repair_type, ToolRepairType::CaseInsensitive);
    assert!(info.was_successful());
}

#[tokio::test]
async fn test_call_with_repair_snake_case() {
    let server = AlephToolServer::new();
    server
        .add_tool(DynamicMockTool {
            name: "web_search".to_string(),
        })
        .await;

    let (result, repair_info) = server
        .call_with_repair("WebSearch", serde_json::json!({"input": "test"}))
        .await;

    assert!(result.is_ok());
    assert!(repair_info.is_some());
    let info = repair_info.unwrap();
    assert_eq!(info.original_name, "WebSearch");
    assert_eq!(info.repaired_name, "web_search");
    assert_eq!(info.repair_type, ToolRepairType::SnakeCase);
    assert!(info.was_successful());
}

#[tokio::test]
async fn test_call_with_repair_invalid_fallback() {
    let server = AlephToolServer::new();

    // Add an "invalid" tool for fallback
    server
        .add_tool(DynamicMockTool {
            name: "invalid".to_string(),
        })
        .await;

    let (result, repair_info) = server
        .call_with_repair("nonexistent", serde_json::json!({"input": "test"}))
        .await;

    assert!(result.is_ok());
    assert!(repair_info.is_some());
    let info = repair_info.unwrap();
    assert_eq!(info.original_name, "nonexistent");
    assert_eq!(info.repaired_name, "invalid");
    assert_eq!(info.repair_type, ToolRepairType::InvalidFallback);
    assert!(!info.was_successful()); // Fallback is not a "successful" repair
}

#[tokio::test]
async fn test_call_with_repair_no_fallback() {
    let server = AlephToolServer::new();

    // No "invalid" tool, so should return error
    let (result, repair_info) = server
        .call_with_repair("nonexistent", serde_json::json!({}))
        .await;

    assert!(result.is_err());
    assert!(repair_info.is_none());
}

#[tokio::test]
async fn test_try_repair_tool_name() {
    let server = AlephToolServer::new();
    server
        .add_tool(DynamicMockTool {
            name: "web_search".to_string(),
        })
        .await;

    // Exact match
    assert_eq!(
        server.try_repair_tool_name("web_search").await,
        Some("web_search".to_string())
    );

    // Case insensitive
    assert_eq!(
        server.try_repair_tool_name("Web_Search").await,
        Some("web_search".to_string())
    );

    // Snake case conversion
    assert_eq!(
        server.try_repair_tool_name("WebSearch").await,
        Some("web_search".to_string())
    );

    // No match
    assert_eq!(server.try_repair_tool_name("nonexistent").await, None);
}
