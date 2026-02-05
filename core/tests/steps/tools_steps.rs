//! Step definitions for Tool Server features

use cucumber::{given, when, then};

use crate::world::{AlephWorld, ToolsContext};
use alephcore::tools::{AlephTool, AlephToolServer};
use alephcore::builtin_tools::bash_exec::BashExecTool;
use alephcore::builtin_tools::search::SearchTool;
use alephcore::Result;
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// =============================================================================
// Test Tool Definitions (from tool_server_replace_test.rs)
// =============================================================================

/// Test tool v1
#[derive(Clone)]
pub struct TestToolV1;

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct TestArgs {
    message: String,
}

#[derive(Serialize)]
pub struct TestOutput {
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

/// Test tool v2
#[derive(Clone)]
pub struct TestToolV2;

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

// =============================================================================
// Given Steps - Tool Setup
// =============================================================================

#[given("a BashExecTool")]
async fn given_bash_exec_tool(w: &mut AlephWorld) {
    let ctx = w.tools.get_or_insert_with(ToolsContext::default);
    let tool = BashExecTool::new();
    ctx.tool_definition = Some(tool.definition());
}

#[given("a SearchTool")]
async fn given_search_tool(w: &mut AlephWorld) {
    let ctx = w.tools.get_or_insert_with(ToolsContext::default);
    let tool = SearchTool::new();
    ctx.tool_definition = Some(tool.definition());
}

#[given("a tool server")]
async fn given_tool_server(w: &mut AlephWorld) {
    let ctx = w.tools.get_or_insert_with(ToolsContext::default);
    ctx.server = Some(AlephToolServer::new());
    ctx.replacement_count = 0;
}

#[given("TestToolV1 is added to the server")]
async fn given_test_tool_v1_added(w: &mut AlephWorld) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    let server = ctx.server.as_ref().expect("Server not initialized");
    server.add_tool(TestToolV1).await;
}

#[given("a server handle")]
async fn given_server_handle(w: &mut AlephWorld) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    let server = ctx.server.as_ref().expect("Server not initialized");
    ctx.handle = Some(server.handle());
}

// =============================================================================
// When Steps - Tool Operations
// =============================================================================

#[when("I get its definition")]
async fn when_get_definition(w: &mut AlephWorld) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    if let Some(ref def) = ctx.tool_definition {
        ctx.llm_context = def.llm_context.clone();
    }
}

#[when("I replace with TestToolV1")]
async fn when_replace_with_v1(w: &mut AlephWorld) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    let server = ctx.server.as_ref().expect("Server not initialized");
    ctx.update_info = Some(server.replace_tool(TestToolV1).await);
    ctx.replacement_count += 1;
}

#[when("I replace with TestToolV2")]
async fn when_replace_with_v2(w: &mut AlephWorld) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    let server = ctx.server.as_ref().expect("Server not initialized");
    ctx.update_info = Some(server.replace_tool(TestToolV2).await);
    ctx.replacement_count += 1;
}

#[when("I replace TestToolV1 via the handle")]
async fn when_replace_v1_via_handle(w: &mut AlephWorld) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    let handle = ctx.handle.as_ref().expect("Handle not initialized");
    ctx.update_info = Some(handle.replace_tool(TestToolV1).await);
    ctx.replacement_count += 1;
}

#[when("I replace TestToolV2 via the handle")]
async fn when_replace_v2_via_handle(w: &mut AlephWorld) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    let handle = ctx.handle.as_ref().expect("Handle not initialized");
    ctx.update_info = Some(handle.replace_tool(TestToolV2).await);
    ctx.replacement_count += 1;
}

#[when(expr = "I call {string} with message {string}")]
async fn when_call_tool(w: &mut AlephWorld, tool_name: String, message: String) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    let server = ctx.server.as_ref().expect("Server not initialized");
    let result = server.call(&tool_name, serde_json::json!({"message": message})).await;
    ctx.call_result = result.ok();
}

// =============================================================================
// Then Steps - Tool Assertions
// =============================================================================

#[then(expr = "the tool name should be {string}")]
async fn then_tool_name_should_be(w: &mut AlephWorld, expected: String) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let def = ctx.tool_definition.as_ref().expect("Tool definition not set");
    assert_eq!(def.name, expected, "Tool name mismatch");
}

#[then(expr = "the llm_context should contain {string}")]
async fn then_llm_context_contains(w: &mut AlephWorld, expected: String) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let context = ctx.llm_context.as_ref().expect("LLM context not set");
    assert!(
        context.contains(&expected),
        "LLM context does not contain '{}'. Context: {}",
        expected,
        context
    );
}

#[then("the update info should indicate a new tool")]
async fn then_update_info_is_new(w: &mut AlephWorld) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let info = ctx.update_info.as_ref().expect("Update info not set");
    assert!(info.is_new(), "Expected is_new() to be true, but was_replaced={}", info.was_replaced);
}

#[then("the update info should indicate a replacement")]
async fn then_update_info_is_replacement(w: &mut AlephWorld) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let info = ctx.update_info.as_ref().expect("Update info not set");
    assert!(info.is_replacement(), "Expected is_replacement() to be true, but was_replaced={}", info.was_replaced);
}

#[then(expr = "the update info tool name should be {string}")]
async fn then_update_info_tool_name(w: &mut AlephWorld, expected: String) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let info = ctx.update_info.as_ref().expect("Update info not set");
    assert_eq!(info.tool_name, expected, "Update info tool name mismatch");
}

#[then(expr = "the update info new description should be {string}")]
async fn then_update_info_new_description(w: &mut AlephWorld, expected: String) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let info = ctx.update_info.as_ref().expect("Update info not set");
    assert_eq!(info.new_description, expected, "Update info new description mismatch");
}

#[then(expr = "the update info old description should be {string}")]
async fn then_update_info_old_description(w: &mut AlephWorld, expected: String) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let info = ctx.update_info.as_ref().expect("Update info not set");
    assert_eq!(
        info.old_description.as_deref(),
        Some(expected.as_str()),
        "Update info old description mismatch"
    );
}

#[then(expr = "the tool {string} should be registered")]
async fn then_tool_registered(w: &mut AlephWorld, tool_name: String) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let server = ctx.server.as_ref().expect("Server not initialized");
    assert!(server.has_tool(&tool_name).await, "Tool '{}' should be registered", tool_name);
}

#[then(expr = "the tool definition description should be {string}")]
async fn then_tool_definition_description(w: &mut AlephWorld, expected: String) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let server = ctx.server.as_ref().expect("Server not initialized");
    let def = server.get_definition("test_tool").await.expect("Tool not found");
    assert_eq!(def.description, expected, "Tool definition description mismatch");
}

#[then(expr = "the call result should be {string}")]
async fn then_call_result(w: &mut AlephWorld, expected: String) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let result = ctx.call_result.as_ref().expect("Call result not set");
    assert_eq!(result["result"], expected, "Call result mismatch");
}
