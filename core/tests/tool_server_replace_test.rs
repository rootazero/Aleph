//! Integration tests for ToolServer replace API

use alephcore::tools::{AlephTool, AlephToolServer};
use alephcore::Result;
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// Test tool v1
#[derive(Clone)]
struct TestToolV1;

#[derive(Serialize, Deserialize, JsonSchema)]
struct TestArgs {
    message: String,
}

#[derive(Serialize)]
struct TestOutput {
    result: String,
}

#[async_trait]
impl AlephTool for TestToolV1 {
    const NAME: &'static str = "test_tool";
    const DESCRIPTION: &'static str = "Test tool version 1";

    type Args = TestArgs;
    type Output = TestOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        Ok(TestOutput {
            result: format!("v1: {}", args.message),
        })
    }
}

// Test tool v2
#[derive(Clone)]
struct TestToolV2;

#[async_trait]
impl AlephTool for TestToolV2 {
    const NAME: &'static str = "test_tool";
    const DESCRIPTION: &'static str = "Test tool version 2 (updated)";

    type Args = TestArgs;
    type Output = TestOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        Ok(TestOutput {
            result: format!("v2: {}", args.message),
        })
    }
}

#[tokio::test]
async fn test_replace_tool_new_addition() {
    let server = AlephToolServer::new();

    // Add a new tool
    let update_info = server.replace_tool(TestToolV1).await;

    assert_eq!(update_info.tool_name, "test_tool");
    assert!(!update_info.was_replaced);
    assert!(update_info.is_new());
    assert!(update_info.old_description.is_none());
    assert_eq!(update_info.new_description, "Test tool version 1");

    // Verify tool is registered
    assert!(server.has_tool("test_tool").await);
}

#[tokio::test]
async fn test_replace_tool_update_existing() {
    let server = AlephToolServer::new();

    // Add v1
    server.add_tool(TestToolV1).await;

    // Replace with v2
    let update_info = server.replace_tool(TestToolV2).await;

    assert_eq!(update_info.tool_name, "test_tool");
    assert!(update_info.was_replaced);
    assert!(update_info.is_replacement());
    assert_eq!(
        update_info.old_description.as_deref(),
        Some("Test tool version 1")
    );
    assert_eq!(update_info.new_description, "Test tool version 2 (updated)");

    // Verify tool is updated
    let def = server.get_definition("test_tool").await.unwrap();
    assert_eq!(def.description, "Test tool version 2 (updated)");
}

#[tokio::test]
async fn test_replace_tool_execution_updated() {
    let server = AlephToolServer::new();

    // Add v1
    server.add_tool(TestToolV1).await;

    // Call v1
    let result = server
        .call("test_tool", serde_json::json!({"message": "hello"}))
        .await
        .unwrap();
    assert_eq!(result["result"], "v1: hello");

    // Replace with v2
    server.replace_tool(TestToolV2).await;

    // Call v2
    let result = server
        .call("test_tool", serde_json::json!({"message": "hello"}))
        .await
        .unwrap();
    assert_eq!(result["result"], "v2: hello");
}

#[tokio::test]
async fn test_replace_tool_handle() {
    let server = AlephToolServer::new();
    let handle = server.handle();

    // Add v1 via handle
    let update_info = handle.replace_tool(TestToolV1).await;
    assert!(!update_info.was_replaced);

    // Replace with v2 via handle
    let update_info = handle.replace_tool(TestToolV2).await;
    assert!(update_info.was_replaced);

    // Verify via server
    let def = server.get_definition("test_tool").await.unwrap();
    assert_eq!(def.description, "Test tool version 2 (updated)");
}

#[tokio::test]
async fn test_multiple_replacements() {
    let server = AlephToolServer::new();

    // First addition
    let info1 = server.replace_tool(TestToolV1).await;
    assert!(!info1.was_replaced);

    // First replacement
    let info2 = server.replace_tool(TestToolV2).await;
    assert!(info2.was_replaced);

    // Second replacement (v2 -> v1)
    let info3 = server.replace_tool(TestToolV1).await;
    assert!(info3.was_replaced);
    assert_eq!(
        info3.old_description.as_deref(),
        Some("Test tool version 2 (updated)")
    );
}
