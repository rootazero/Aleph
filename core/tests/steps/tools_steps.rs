//! Step definitions for Tool Server features

use std::sync::Arc;
use std::time::Instant;

use cucumber::{given, when, then};
use serde_json::json;
use tokio::sync::RwLock;

use crate::world::{AlephWorld, ToolsContext};
use alephcore::agents::sub_agents::{
    ArtifactInfo, DelegateResult, ExecutionContextInfo, ResultMerger, SubAgentRequest, ToolCallInfo,
};
use alephcore::builtin_tools::bash_exec::BashExecTool;
use alephcore::builtin_tools::meta_tools::{
    GetToolSchemaArgs, GetToolSchemaTool, ListToolsArgs, ListToolsTool,
};
use alephcore::builtin_tools::search::SearchTool;
#[cfg(feature = "gateway")]
use alephcore::builtin_tools::sessions::{
    SessionsListArgs, SessionsListTool, SessionsSendArgs, SessionsSendStatus, SessionsSendTool,
};
use alephcore::dispatcher::{
    ToolIndex, ToolIndexCategory, ToolIndexEntry, ToolRegistry, ToolSource, UnifiedTool,
};
use alephcore::gateway::a2a_policy::AgentToAgentPolicy;
use alephcore::gateway::router::SessionKey;
use alephcore::tools::{AlephTool, AlephToolServer};
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
// Given Steps - Tool Setup (from server.feature)
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
// Given Steps - Smart Tool Discovery
// =============================================================================

#[given(expr = "a unified tool {string} named {string} with description {string}")]
async fn given_unified_tool(w: &mut AlephWorld, id: String, name: String, description: String) {
    let ctx = w.tools.get_or_insert_with(ToolsContext::default);
    // Default source, will be overridden by next step
    ctx.unified_tool = Some(UnifiedTool::new(&id, &name, &description, ToolSource::Builtin));
}

#[given("the tool source is Builtin")]
async fn given_tool_source_builtin(w: &mut AlephWorld) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    if let Some(ref mut tool) = ctx.unified_tool {
        tool.source = ToolSource::Builtin;
    }
}

#[given("the tool source is Native")]
async fn given_tool_source_native(w: &mut AlephWorld) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    if let Some(ref mut tool) = ctx.unified_tool {
        tool.source = ToolSource::Native;
    }
}

#[given(expr = "the tool source is Mcp with server {string}")]
async fn given_tool_source_mcp(w: &mut AlephWorld, server: String) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    if let Some(ref mut tool) = ctx.unified_tool {
        tool.source = ToolSource::Mcp { server };
    }
}

#[given(expr = "the tool source is Skill with id {string}")]
async fn given_tool_source_skill(w: &mut AlephWorld, id: String) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    if let Some(ref mut tool) = ctx.unified_tool {
        tool.source = ToolSource::Skill { id };
    }
}

#[given(expr = "the tool source is Custom with rule_index {int}")]
async fn given_tool_source_custom(w: &mut AlephWorld, rule_index: usize) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    if let Some(ref mut tool) = ctx.unified_tool {
        tool.source = ToolSource::Custom { rule_index };
    }
}

#[given("an empty tool index")]
async fn given_empty_tool_index(w: &mut AlephWorld) {
    let ctx = w.tools.get_or_insert_with(ToolsContext::default);
    ctx.tool_index = Some(ToolIndex::new());
}

#[given("an empty tool registry")]
async fn given_empty_tool_registry(w: &mut AlephWorld) {
    let ctx = w.tools.get_or_insert_with(ToolsContext::default);
    ctx.tool_registry = Some(Arc::new(RwLock::new(ToolRegistry::new())));
}

#[given(expr = "I register tool {string} named {string} with description {string} and source Builtin")]
async fn given_register_tool_builtin(
    w: &mut AlephWorld,
    id: String,
    name: String,
    description: String,
) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    let registry = ctx.tool_registry.as_ref().expect("Registry not initialized");
    let tool = UnifiedTool::new(&id, &name, &description, ToolSource::Builtin);
    let reg = registry.read().await;
    reg.register_with_conflict_resolution(tool).await;
}

#[given(expr = "I register tool {string} named {string} with description {string} and source Builtin with schema")]
async fn given_register_tool_builtin_with_schema(
    w: &mut AlephWorld,
    id: String,
    name: String,
    description: String,
) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    let registry = ctx.tool_registry.as_ref().expect("Registry not initialized");
    let tool = UnifiedTool::new(&id, &name, &description, ToolSource::Builtin)
        .with_parameters_schema(json!({"type": "object"}));
    let reg = registry.read().await;
    reg.register_with_conflict_resolution(tool).await;
}

#[given(expr = "I register tool {string} named {string} with description {string} and source Mcp with server {string}")]
async fn given_register_tool_mcp(
    w: &mut AlephWorld,
    id: String,
    name: String,
    description: String,
    server: String,
) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    let registry = ctx.tool_registry.as_ref().expect("Registry not initialized");
    let tool = UnifiedTool::new(&id, &name, &description, ToolSource::Mcp { server });
    let reg = registry.read().await;
    reg.register_with_conflict_resolution(tool).await;
}

#[given(expr = "the tool has a parameters schema with property {string} of type {string}")]
async fn given_tool_has_schema(w: &mut AlephWorld, prop_name: String, prop_type: String) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    let registry = ctx.tool_registry.as_ref().expect("Registry not initialized");
    // Update the last registered tool with a schema
    let reg = registry.read().await;
    let tools = reg.list_all().await;
    if let Some(last) = tools.last() {
        let mut properties = serde_json::Map::new();
        properties.insert(prop_name.clone(), json!({ "type": prop_type }));
        let updated = last.clone().with_parameters_schema(json!({
            "type": "object",
            "properties": properties,
            "required": [prop_name]
        }));
        reg.register_with_conflict_resolution(updated).await;
    }
}

#[given(expr = "I register {int} MCP tools for server {string}")]
async fn given_register_mcp_tools(w: &mut AlephWorld, count: usize, server: String) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    let registry = ctx.tool_registry.as_ref().expect("Registry not initialized");
    let reg = registry.read().await;
    for i in 0..count {
        let tool = UnifiedTool::new(
            format!("mcp:{}:tool_{}", server, i),
            format!("{}:tool_{}", server, i),
            format!("{} tool {}", server, i),
            ToolSource::Mcp { server: server.clone() },
        );
        reg.register_with_conflict_resolution(tool).await;
    }
}

#[given(expr = "I register {int} tools with realistic schemas")]
async fn given_register_tools_with_schemas(w: &mut AlephWorld, count: usize) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    let registry = ctx.tool_registry.as_ref().expect("Registry not initialized");
    let reg = registry.read().await;
    for i in 0..count {
        let schema = json!({
            "type": "object",
            "properties": {
                "param1": { "type": "string", "description": format!("Parameter 1 for tool {}", i) },
                "param2": { "type": "integer", "description": "Optional count parameter" },
                "param3": { "type": "boolean", "default": false }
            },
            "required": ["param1"]
        });
        let tool = UnifiedTool::new(
            format!("mcp:server:tool_{}", i),
            format!("tool_{}", i),
            format!("This is tool {} which does something useful for the user. It has multiple parameters and options.", i),
            ToolSource::Mcp { server: "server".into() },
        ).with_parameters_schema(schema);
        reg.register_with_conflict_resolution(tool).await;
    }
}

#[given(expr = "I register {int} tools with simple schemas")]
async fn given_register_tools_simple_schemas(w: &mut AlephWorld, count: usize) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    let registry = ctx.tool_registry.as_ref().expect("Registry not initialized");
    let reg = registry.read().await;
    for i in 0..count {
        let tool = UnifiedTool::new(
            format!("mcp:server:tool_{}", i),
            format!("tool_{}", i),
            format!("Tool {} description", i),
            ToolSource::Mcp { server: "server".into() },
        ).with_parameters_schema(json!({
            "type": "object",
            "properties": { "input": { "type": "string" } }
        }));
        reg.register_with_conflict_resolution(tool).await;
    }
}

#[given(expr = "I register a realistic tool mix with {int} builtin, {int} MCP, and {int} skill tools")]
async fn given_register_realistic_mix(
    w: &mut AlephWorld,
    builtin_count: usize,
    mcp_count: usize,
    skill_count: usize,
) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    let registry = ctx.tool_registry.as_ref().expect("Registry not initialized");
    let reg = registry.read().await;

    // Helper function to create a schema
    fn make_schema(params: &[(&str, &str, bool)]) -> serde_json::Value {
        let mut properties = serde_json::Map::new();
        let mut required = Vec::new();
        for (name, desc, req) in params {
            properties.insert(name.to_string(), json!({
                "type": "string",
                "description": desc
            }));
            if *req {
                required.push(serde_json::Value::String(name.to_string()));
            }
        }
        json!({
            "type": "object",
            "properties": properties,
            "required": required
        })
    }

    // Builtin tools
    let builtin_names = ["search", "file_ops", "code_exec", "web_fetch", "youtube"];
    for name in builtin_names.iter().take(builtin_count) {
        let tool = UnifiedTool::new(
            format!("builtin:{}", name),
            *name,
            format!("{} tool for various operations", name),
            ToolSource::Builtin,
        ).with_parameters_schema(make_schema(&[
            ("query", "The search query", true),
            ("limit", "Maximum results", false),
        ]));
        reg.register_with_conflict_resolution(tool).await;
    }

    // MCP tools (GitHub, Notion, Slack)
    let github_tools = [
        ("pr_list", "List pull requests"),
        ("pr_create", "Create a pull request"),
        ("pr_merge", "Merge a pull request"),
        ("issue_list", "List issues"),
        ("issue_create", "Create an issue"),
    ];
    let notion_tools = [
        ("page_read", "Read Notion page"),
        ("page_create", "Create Notion page"),
        ("database_query", "Query Notion database"),
    ];
    let slack_tools = [
        ("message_post", "Post a message"),
        ("channel_list", "List channels"),
    ];

    let mut mcp_registered = 0;
    for (name, desc) in github_tools.iter() {
        if mcp_registered >= mcp_count { break; }
        let tool = UnifiedTool::new(
            format!("mcp:github:{}", name),
            format!("github:{}", name),
            *desc,
            ToolSource::Mcp { server: "github".into() },
        ).with_parameters_schema(make_schema(&[
            ("owner", "Repository owner", true),
            ("repo", "Repository name", true),
        ]));
        reg.register_with_conflict_resolution(tool).await;
        mcp_registered += 1;
    }
    for (name, desc) in notion_tools.iter() {
        if mcp_registered >= mcp_count { break; }
        let tool = UnifiedTool::new(
            format!("mcp:notion:{}", name),
            format!("notion:{}", name),
            *desc,
            ToolSource::Mcp { server: "notion".into() },
        ).with_parameters_schema(make_schema(&[
            ("page_id", "Page ID", false),
        ]));
        reg.register_with_conflict_resolution(tool).await;
        mcp_registered += 1;
    }
    for (name, desc) in slack_tools.iter() {
        if mcp_registered >= mcp_count { break; }
        let tool = UnifiedTool::new(
            format!("mcp:slack:{}", name),
            format!("slack:{}", name),
            *desc,
            ToolSource::Mcp { server: "slack".into() },
        ).with_parameters_schema(make_schema(&[
            ("channel", "Channel ID", false),
        ]));
        reg.register_with_conflict_resolution(tool).await;
        mcp_registered += 1;
    }
    // Fill remaining MCP tools
    for i in mcp_registered..mcp_count {
        let tool = UnifiedTool::new(
            format!("mcp:extra:tool_{}", i),
            format!("extra:tool_{}", i),
            format!("Extra MCP tool {}", i),
            ToolSource::Mcp { server: "extra".into() },
        ).with_parameters_schema(make_schema(&[("input", "Input", true)]));
        reg.register_with_conflict_resolution(tool).await;
    }

    // Skills
    let skills = [
        "code-review", "refine-text", "translate", "summarize",
        "generate-tests", "explain-code", "fix-bugs", "optimize",
        "document", "format", "refactor", "analyze",
    ];
    for name in skills.iter().take(skill_count) {
        let tool = UnifiedTool::new(
            format!("skill:{}", name),
            *name,
            format!("{} skill", name),
            ToolSource::Skill { id: name.to_string() },
        ).with_parameters_schema(make_schema(&[
            ("input", "Input content", true),
        ]));
        reg.register_with_conflict_resolution(tool).await;
    }
}

// =============================================================================
// Given Steps - Sub-Agent Tests
// =============================================================================

#[given(expr = "a delegate result JSON with success true and agent_id {string}")]
async fn given_delegate_json(w: &mut AlephWorld, agent_id: String) {
    let ctx = w.tools.get_or_insert_with(ToolsContext::default);
    ctx.delegate_json = Some(json!({
        "success": true,
        "summary": "Found 3 matching MCP tools",
        "agent_id": agent_id,
        "output": {"tools": []},
        "artifacts": [],
        "tools_called": [],
        "iterations_used": 2,
        "error": null
    }));
}

#[given(expr = "the result has {int} tools and {int} artifact and {int} tool call")]
async fn given_result_contents(w: &mut AlephWorld, _tools: usize, artifacts: usize, tool_calls: usize) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    if let Some(ref mut json) = ctx.delegate_json {
        if let Some(obj) = json.as_object_mut() {
            let mut artifact_list = Vec::new();
            for _ in 0..artifacts {
                artifact_list.push(json!({
                    "artifact_type": "file",
                    "path": "/tmp/tools.json",
                    "mime_type": "application/json"
                }));
            }
            obj.insert("artifacts".to_string(), json!(artifact_list));

            let mut tool_call_list = Vec::new();
            for _ in 0..tool_calls {
                tool_call_list.push(json!({
                    "name": "list_tools",
                    "success": true,
                    "result_summary": "Listed 10 tools"
                }));
            }
            obj.insert("tools_called".to_string(), json!(tool_call_list));
        }
    }
}

#[given(expr = "a delegate result with success true and summary {string}")]
async fn given_delegate_result_struct(w: &mut AlephWorld, summary: String) {
    let ctx = w.tools.get_or_insert_with(ToolsContext::default);
    ctx.delegate_result = Some(DelegateResult {
        success: true,
        summary,
        agent_id: String::new(),
        output: None,
        artifacts: Vec::new(),
        tools_called: Vec::new(),
        iterations_used: 1,
        error: None,
    });
}

#[given(expr = "the delegate result has agent_id {string}")]
async fn given_delegate_agent_id(w: &mut AlephWorld, agent_id: String) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    if let Some(ref mut result) = ctx.delegate_result {
        result.agent_id = agent_id;
    }
}

#[given(expr = "the delegate result has {int} artifact")]
async fn given_delegate_artifacts(w: &mut AlephWorld, count: usize) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    if let Some(ref mut result) = ctx.delegate_result {
        for _ in 0..count {
            result.artifacts.push(ArtifactInfo {
                artifact_type: "file".to_string(),
                path: "/tmp/output.json".to_string(),
                mime_type: Some("application/json".to_string()),
            });
        }
    }
}

#[given(expr = "the delegate result has {int} tool call")]
async fn given_delegate_tool_calls(w: &mut AlephWorld, count: usize) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    if let Some(ref mut result) = ctx.delegate_result {
        for _ in 0..count {
            result.tools_called.push(ToolCallInfo {
                name: "list_skills".to_string(),
                success: true,
                result_summary: "Found 5 skills".to_string(),
            });
        }
    }
}

#[given(expr = "an execution context with working directory {string}")]
async fn given_execution_context(w: &mut AlephWorld, working_dir: String) {
    let ctx = w.tools.get_or_insert_with(ToolsContext::default);
    ctx.execution_context = Some(ExecutionContextInfo::new().with_working_directory(&working_dir));
}

#[given(expr = "the context has current app {string}")]
async fn given_context_current_app(w: &mut AlephWorld, app: String) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    if let Some(exec_ctx) = ctx.execution_context.take() {
        ctx.execution_context = Some(exec_ctx.with_current_app(&app));
    }
}

#[given(expr = "the context has original request {string}")]
async fn given_context_original_request(w: &mut AlephWorld, request: String) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    if let Some(exec_ctx) = ctx.execution_context.take() {
        ctx.execution_context = Some(exec_ctx.with_original_request(&request));
    }
}

#[given(expr = "the context has history summary {string}")]
async fn given_context_history_summary(w: &mut AlephWorld, summary: String) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    if let Some(exec_ctx) = ctx.execution_context.take() {
        ctx.execution_context = Some(exec_ctx.with_history_summary(&summary));
    }
}

#[given(expr = "the context has metadata {string} with value {string}")]
async fn given_context_metadata(w: &mut AlephWorld, key: String, value: String) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    if let Some(exec_ctx) = ctx.execution_context.take() {
        ctx.execution_context = Some(exec_ctx.with_metadata(&key, &value));
    }
}

// =============================================================================
// Given Steps - Sessions Tools
// =============================================================================

#[given("a permissive gateway context")]
async fn given_permissive_gateway(w: &mut AlephWorld) {
    let ctx = w.tools.get_or_insert_with(ToolsContext::default);
    ctx.init_permissive_gateway();
}

#[given("a permissive gateway context with tracking adapter")]
async fn given_permissive_gateway_tracking(w: &mut AlephWorld) {
    let ctx = w.tools.get_or_insert_with(ToolsContext::default);
    ctx.init_permissive_gateway_with_tracking();
}

#[given("a permissive gateway context with failing adapter")]
async fn given_permissive_gateway_failing(w: &mut AlephWorld) {
    let ctx = w.tools.get_or_insert_with(ToolsContext::default);
    ctx.init_permissive_gateway_with_failing();
}

#[given(expr = "a gateway context with policy allowing only agent {string}")]
async fn given_gateway_restrictive(w: &mut AlephWorld, agent: String) {
    let ctx = w.tools.get_or_insert_with(ToolsContext::default);
    let policy = AgentToAgentPolicy::new(true, vec![agent]);
    ctx.init_gateway_with_policy(policy, false);
}

#[given(expr = "a gateway context with policy allowing pattern {string}")]
async fn given_gateway_pattern(w: &mut AlephWorld, pattern: String) {
    let ctx = w.tools.get_or_insert_with(ToolsContext::default);
    let policy = AgentToAgentPolicy::new(true, vec![pattern]);
    ctx.init_gateway_with_policy(policy, false);
}

#[given(expr = "a gateway context with policy allowing pattern {string} and tracking adapter")]
async fn given_gateway_pattern_tracking(w: &mut AlephWorld, pattern: String) {
    let ctx = w.tools.get_or_insert_with(ToolsContext::default);
    let policy = AgentToAgentPolicy::new(true, vec![pattern]);
    ctx.init_gateway_with_policy(policy, true);
}

#[given("a gateway context with disabled A2A policy")]
async fn given_gateway_disabled(w: &mut AlephWorld) {
    let ctx = w.tools.get_or_insert_with(ToolsContext::default);
    let policy = AgentToAgentPolicy::disabled();
    ctx.init_gateway_with_policy(policy, false);
}

#[given("a gateway context with disabled A2A policy and tracking adapter")]
async fn given_gateway_disabled_tracking(w: &mut AlephWorld) {
    let ctx = w.tools.get_or_insert_with(ToolsContext::default);
    let policy = AgentToAgentPolicy::disabled();
    ctx.init_gateway_with_policy(policy, true);
}

#[given(expr = "a main session for agent {string}")]
async fn given_main_session(w: &mut AlephWorld, agent: String) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    let session_manager = ctx.session_manager.as_ref().expect("Session manager not initialized");
    let key = SessionKey::main(&agent);
    session_manager.get_or_create(&key).await.unwrap();
    ctx.current_session_key = Some(key);
}

#[given(expr = "a task session for agent {string} with kind {string} and id {string}")]
async fn given_task_session(w: &mut AlephWorld, agent: String, kind: String, id: String) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    let session_manager = ctx.session_manager.as_ref().expect("Session manager not initialized");
    let key = SessionKey::task(&agent, &kind, &id);
    session_manager.get_or_create(&key).await.unwrap();
    ctx.current_session_key = Some(key);
}

#[given(expr = "a peer session for agent {string} with peer {string}")]
async fn given_peer_session(w: &mut AlephWorld, agent: String, peer: String) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    let session_manager = ctx.session_manager.as_ref().expect("Session manager not initialized");
    let key = SessionKey::peer(&agent, &peer);
    session_manager.get_or_create(&key).await.unwrap();
    ctx.current_session_key = Some(key);
}

#[given(expr = "an ephemeral session for agent {string}")]
async fn given_ephemeral_session(w: &mut AlephWorld, agent: String) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    let session_manager = ctx.session_manager.as_ref().expect("Session manager not initialized");
    let key = SessionKey::ephemeral(&agent);
    session_manager.get_or_create(&key).await.unwrap();
    ctx.current_session_key = Some(key);
}

#[given(expr = "{int} task sessions for agent {string} with kind {string}")]
async fn given_multiple_task_sessions(w: &mut AlephWorld, count: usize, agent: String, kind: String) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    let session_manager = ctx.session_manager.as_ref().expect("Session manager not initialized");
    for i in 0..count {
        let key = SessionKey::task(&agent, &kind, format!("task-{}", i));
        session_manager.get_or_create(&key).await.unwrap();
    }
}

#[given(expr = "the session has message from {string} with text {string}")]
async fn given_session_message(w: &mut AlephWorld, role: String, text: String) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    let session_manager = ctx.session_manager.as_ref().expect("Session manager not initialized");
    let key = ctx.current_session_key.as_ref().expect("No current session key");
    session_manager.add_message(key, &role, &text).await.unwrap();
}

#[given(expr = "registered agent {string}")]
async fn given_registered_agent(w: &mut AlephWorld, agent_id: String) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    ctx.register_agent(&agent_id).await;
}

#[given("a sessions_send tool without context")]
async fn given_sessions_send_no_context(w: &mut AlephWorld) {
    // No setup needed - the tool will be created without context
    let ctx = w.tools.get_or_insert_with(ToolsContext::default);
    ctx.caller_agent_id = None;
}

#[given(expr = "a sessions_send tool with context for agent {string}")]
async fn given_sessions_send_with_context(w: &mut AlephWorld, agent: String) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    ctx.caller_agent_id = Some(agent);
}

// =============================================================================
// When Steps - Tool Operations (from server.feature)
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
    let result = server.call(&tool_name, json!({"message": message})).await;
    ctx.call_result = result.ok();
}

// =============================================================================
// When Steps - Smart Tool Discovery
// =============================================================================

#[when(expr = "I generate a tool index entry with core tools {string}")]
async fn when_generate_index_entry(w: &mut AlephWorld, core_tools_str: String) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    let tool = ctx.unified_tool.as_ref().expect("Unified tool not set");
    let core_tools: Vec<&str> = if core_tools_str.is_empty() {
        vec![]
    } else {
        core_tools_str.split(',').collect()
    };
    ctx.index_entry = Some(tool.to_index_entry(&core_tools));
}

#[when(expr = "I add entry {string} with category {word} and summary {string}")]
async fn when_add_index_entry(w: &mut AlephWorld, name: String, category: String, summary: String) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    let index = ctx.tool_index.as_mut().expect("Tool index not initialized");
    let cat = match category.as_str() {
        "Core" => ToolIndexCategory::Core,
        "Builtin" => ToolIndexCategory::Builtin,
        "Mcp" => ToolIndexCategory::Mcp,
        "Skill" => ToolIndexCategory::Skill,
        "Custom" => ToolIndexCategory::Custom,
        _ => panic!("Unknown category: {}", category),
    };
    index.add(ToolIndexEntry::new(&name, cat, &summary));
}

#[when(expr = "I add entry {string} with category {word} and summary {string} marked as core")]
async fn when_add_index_entry_core(w: &mut AlephWorld, name: String, category: String, summary: String) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    let index = ctx.tool_index.as_mut().expect("Tool index not initialized");
    let cat = match category.as_str() {
        "Core" => ToolIndexCategory::Core,
        "Builtin" => ToolIndexCategory::Builtin,
        "Mcp" => ToolIndexCategory::Mcp,
        "Skill" => ToolIndexCategory::Skill,
        "Custom" => ToolIndexCategory::Custom,
        _ => panic!("Unknown category: {}", category),
    };
    index.add(ToolIndexEntry::new(&name, cat, &summary).with_core(true));
}

#[when("I generate the prompt")]
async fn when_generate_prompt(w: &mut AlephWorld) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    let index = ctx.tool_index.as_ref().expect("Tool index not initialized");
    ctx.prompt = Some(index.to_prompt());
}

#[when("I call list_tools with no category filter")]
async fn when_call_list_tools(w: &mut AlephWorld) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    let registry = ctx.tool_registry.as_ref().expect("Registry not initialized");
    let tool = ListToolsTool::new(registry.clone());
    let args = ListToolsArgs { category: None };
    ctx.list_result = Some(AlephTool::call(&tool, args).await.unwrap());
}

#[when(expr = "I call list_tools with category filter {string}")]
async fn when_call_list_tools_filtered(w: &mut AlephWorld, category: String) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    let registry = ctx.tool_registry.as_ref().expect("Registry not initialized");
    let tool = ListToolsTool::new(registry.clone());
    let args = ListToolsArgs { category: Some(category) };
    ctx.list_result = Some(AlephTool::call(&tool, args).await.unwrap());
}

#[when(expr = "I call get_tool_schema for {string}")]
async fn when_call_get_tool_schema(w: &mut AlephWorld, tool_name: String) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    let registry = ctx.tool_registry.as_ref().expect("Registry not initialized");
    let tool = GetToolSchemaTool::new(registry.clone());
    let args = GetToolSchemaArgs { tool_name };
    ctx.schema_result = Some(AlephTool::call(&tool, args).await.unwrap());
}

#[when(expr = "I generate tool index from registry with core tools {string}")]
async fn when_generate_index_from_registry(w: &mut AlephWorld, core_tools_str: String) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    let registry = ctx.tool_registry.as_ref().expect("Registry not initialized");
    let core_tools: Vec<&str> = if core_tools_str.is_empty() {
        vec![]
    } else {
        core_tools_str.split(',').collect()
    };
    let reg = registry.read().await;
    ctx.tool_index = Some(reg.generate_tool_index(&core_tools).await);
    ctx.prompt = Some(ctx.tool_index.as_ref().unwrap().to_prompt());
}

#[when("I calculate full schema text size")]
async fn when_calculate_full_schema(w: &mut AlephWorld) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    let registry = ctx.tool_registry.as_ref().expect("Registry not initialized");
    let reg = registry.read().await;
    let tools = reg.list_all().await;
    let mut text = String::new();
    for tool in &tools {
        text.push_str(&format!(
            "Tool: {}\nDescription: {}\nSchema: {}\n\n",
            tool.name,
            tool.description,
            tool.parameters_schema
                .as_ref()
                .map(|s| serde_json::to_string(s).unwrap_or_default())
                .unwrap_or_default()
        ));
    }
    ctx.full_schema_size = Some(text.len());
}

#[when("I calculate index-only text size")]
async fn when_calculate_index_size(w: &mut AlephWorld) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    let registry = ctx.tool_registry.as_ref().expect("Registry not initialized");
    let reg = registry.read().await;
    let index = reg.generate_tool_index(&["search", "file_ops"]).await;
    ctx.index_size = Some(index.to_prompt().len());
}

#[when(expr = "I measure list_tools latency over {int} iterations")]
async fn when_measure_list_tools_latency(w: &mut AlephWorld, iterations: usize) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    let registry = ctx.tool_registry.as_ref().expect("Registry not initialized");
    let tool = ListToolsTool::new(registry.clone());
    let start = Instant::now();
    for _ in 0..iterations {
        let _ = AlephTool::call(&tool, ListToolsArgs { category: None }).await;
    }
    ctx.latencies.list_tools_us = Some(start.elapsed().as_micros() / iterations as u128);
}

#[when(expr = "I measure get_tool_schema latency over {int} iterations")]
async fn when_measure_get_schema_latency(w: &mut AlephWorld, iterations: usize) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    let registry = ctx.tool_registry.as_ref().expect("Registry not initialized");
    let tool = GetToolSchemaTool::new(registry.clone());
    let start = Instant::now();
    for _ in 0..iterations {
        let _ = AlephTool::call(&tool, GetToolSchemaArgs { tool_name: "tool_50".to_string() }).await;
    }
    ctx.latencies.get_tool_schema_us = Some(start.elapsed().as_micros() / iterations as u128);
}

#[when(expr = "I measure generate_index latency over {int} iterations")]
async fn when_measure_index_latency(w: &mut AlephWorld, iterations: usize) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    let registry = ctx.tool_registry.as_ref().expect("Registry not initialized");
    let start = Instant::now();
    for _ in 0..iterations {
        let reg = registry.read().await;
        let _ = reg.generate_tool_index(&["search"]).await;
    }
    ctx.latencies.generate_index_us = Some(start.elapsed().as_micros() / iterations as u128);
}

// =============================================================================
// When Steps - Sub-Agent
// =============================================================================

#[when("I parse the delegate result")]
async fn when_parse_delegate(w: &mut AlephWorld) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    let json = ctx.delegate_json.as_ref().expect("Delegate JSON not set");
    ctx.delegate_result = ResultMerger::parse_delegate_result(json);
}

#[when("I merge the delegate result")]
async fn when_merge_delegate(w: &mut AlephWorld) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    let result = ctx.delegate_result.as_ref().expect("Delegate result not set");
    ctx.merged_result = Some(ResultMerger::merge(result));
}

#[when(expr = "I create a sub-agent request with prompt {string}")]
async fn when_create_subagent_request(w: &mut AlephWorld, prompt: String) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    ctx.sub_agent_request = Some(SubAgentRequest::new(&prompt));
}

#[when(expr = "I set target to {string}")]
async fn when_set_target(w: &mut AlephWorld, target: String) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    if let Some(req) = ctx.sub_agent_request.take() {
        ctx.sub_agent_request = Some(req.with_target(&target));
    }
}

#[when(expr = "I set max iterations to {int}")]
async fn when_set_max_iterations(w: &mut AlephWorld, max: i32) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    if let Some(req) = ctx.sub_agent_request.take() {
        ctx.sub_agent_request = Some(req.with_max_iterations(max as u32));
    }
}

#[when(expr = "I set parent session to {string}")]
async fn when_set_parent_session(w: &mut AlephWorld, session: String) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    if let Some(req) = ctx.sub_agent_request.take() {
        ctx.sub_agent_request = Some(req.with_parent_session(&session));
    }
}

#[when("I set the execution context")]
async fn when_set_exec_context(w: &mut AlephWorld) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    let exec_ctx = ctx.execution_context.take().expect("Execution context not set");
    if let Some(req) = ctx.sub_agent_request.take() {
        ctx.sub_agent_request = Some(req.with_execution_context(exec_ctx));
    }
}

// =============================================================================
// When Steps - Sessions Tools
// =============================================================================

#[when(expr = "I call sessions_list with no filters and limit {int}")]
async fn when_call_sessions_list(w: &mut AlephWorld, limit: u32) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    let gateway = ctx.gateway_context.as_ref().expect("Gateway context not initialized");
    let tool = SessionsListTool::new(gateway.clone(), "main");
    let args = SessionsListArgs {
        kinds: None,
        limit: Some(limit),
        active_minutes: None,
        message_limit: None,
    };
    ctx.sessions_list_result = Some(AlephTool::call(&tool, args).await.unwrap());
}

#[when(expr = "I call sessions_list with kind filter {string} and limit {int}")]
async fn when_call_sessions_list_kind_filter(w: &mut AlephWorld, kind: String, limit: u32) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    let gateway = ctx.gateway_context.as_ref().expect("Gateway context not initialized");
    let tool = SessionsListTool::new(gateway.clone(), "main");
    let args = SessionsListArgs {
        kinds: Some(vec![kind]),
        limit: Some(limit),
        active_minutes: None,
        message_limit: None,
    };
    ctx.sessions_list_result = Some(AlephTool::call(&tool, args).await.unwrap());
}

#[when(expr = "I call sessions_list with kind filters {string} and limit {int}")]
async fn when_call_sessions_list_kind_filters(w: &mut AlephWorld, kinds_str: String, limit: u32) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    let gateway = ctx.gateway_context.as_ref().expect("Gateway context not initialized");
    let kinds: Vec<String> = kinds_str.split(',').map(|s| s.to_string()).collect();
    let tool = SessionsListTool::new(gateway.clone(), "main");
    let args = SessionsListArgs {
        kinds: Some(kinds),
        limit: Some(limit),
        active_minutes: None,
        message_limit: None,
    };
    ctx.sessions_list_result = Some(AlephTool::call(&tool, args).await.unwrap());
}

#[when(expr = "I call sessions_list with message limit {int}")]
async fn when_call_sessions_list_message_limit(w: &mut AlephWorld, message_limit: u32) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    let gateway = ctx.gateway_context.as_ref().expect("Gateway context not initialized");
    let tool = SessionsListTool::new(gateway.clone(), "main");
    let args = SessionsListArgs {
        kinds: None,
        limit: Some(50),
        active_minutes: None,
        message_limit: Some(message_limit),
    };
    ctx.sessions_list_result = Some(AlephTool::call(&tool, args).await.unwrap());
}

#[when(expr = "I call sessions_list as agent {string} with no filters and limit {int}")]
async fn when_call_sessions_list_as_agent(w: &mut AlephWorld, agent: String, limit: u32) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    let gateway = ctx.gateway_context.as_ref().expect("Gateway context not initialized");
    let tool = SessionsListTool::new(gateway.clone(), &agent);
    let args = SessionsListArgs {
        kinds: None,
        limit: Some(limit),
        active_minutes: None,
        message_limit: None,
    };
    ctx.sessions_list_result = Some(AlephTool::call(&tool, args).await.unwrap());
}

#[when(expr = "I call sessions_list as agent {string} with kind filter {string} and limit {int}")]
async fn when_call_sessions_list_as_agent_filtered(
    w: &mut AlephWorld,
    agent: String,
    kind: String,
    limit: u32,
) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    let gateway = ctx.gateway_context.as_ref().expect("Gateway context not initialized");
    let tool = SessionsListTool::new(gateway.clone(), &agent);
    let args = SessionsListArgs {
        kinds: Some(vec![kind]),
        limit: Some(limit),
        active_minutes: None,
        message_limit: None,
    };
    ctx.sessions_list_result = Some(AlephTool::call(&tool, args).await.unwrap());
}

#[when(expr = "I call sessions_send to {string} with message {string}")]
async fn when_call_sessions_send(w: &mut AlephWorld, session_key: String, message: String) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    // Use gateway context if available, otherwise create tool without context
    let tool = if let Some(ref gateway) = ctx.gateway_context {
        let caller = ctx.caller_agent_id.as_deref().unwrap_or("main");
        SessionsSendTool::with_context((**gateway).clone(), caller)
    } else {
        SessionsSendTool::new()
    };
    let args = SessionsSendArgs {
        session_key: Some(session_key),
        message,
        timeout_seconds: 0,
    };
    ctx.sessions_send_result = Some(AlephTool::call(&tool, args).await.unwrap());
}

#[when(expr = "I call sessions_send to {string} with message {string} and timeout {int}")]
async fn when_call_sessions_send_timeout(
    w: &mut AlephWorld,
    session_key: String,
    message: String,
    timeout: u64,
) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    if let Some(ref gateway) = ctx.gateway_context {
        let caller = ctx.caller_agent_id.as_deref().unwrap_or("main");
        let tool = SessionsSendTool::with_context((**gateway).clone(), caller);
        let args = SessionsSendArgs {
            session_key: Some(session_key),
            message,
            timeout_seconds: timeout as u32,
        };
        ctx.sessions_send_result = Some(AlephTool::call(&tool, args).await.unwrap());
    } else {
        let tool = SessionsSendTool::new();
        let args = SessionsSendArgs {
            session_key: Some(session_key),
            message,
            timeout_seconds: timeout as u32,
        };
        ctx.sessions_send_result = Some(AlephTool::call(&tool, args).await.unwrap());
    }
}

#[when(expr = "I call sessions_send with no key and message {string} and timeout {int}")]
async fn when_call_sessions_send_no_key(w: &mut AlephWorld, message: String, timeout: u64) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    let gateway = ctx.gateway_context.as_ref().expect("Gateway context not initialized");
    let caller = ctx.caller_agent_id.as_deref().unwrap_or("main");
    let tool = SessionsSendTool::with_context((**gateway).clone(), caller);
    let args = SessionsSendArgs {
        session_key: None,
        message,
        timeout_seconds: timeout as u32,
    };
    ctx.sessions_send_result = Some(AlephTool::call(&tool, args).await.unwrap());
}

#[when(expr = "I find the session with key containing {string}")]
async fn when_find_session_key(w: &mut AlephWorld, pattern: String) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    let result = ctx.sessions_list_result.as_ref().expect("Sessions list result not set");
    let found = result.sessions.iter().find(|s| s.key.contains(&pattern));
    ctx.found_session_key = found.map(|s| s.key.clone());
}

#[when(expr = "I call sessions_send to the found session with message {string} and timeout {int}")]
async fn when_call_sessions_send_found(w: &mut AlephWorld, message: String, timeout: u64) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    let session_key = ctx.found_session_key.clone().expect("No found session key");
    let gateway = ctx.gateway_context.as_ref().expect("Gateway context not initialized");
    let caller = ctx.caller_agent_id.as_deref().unwrap_or("main");
    let tool = SessionsSendTool::with_context((**gateway).clone(), caller);
    let args = SessionsSendArgs {
        session_key: Some(session_key),
        message,
        timeout_seconds: timeout as u32,
    };
    ctx.sessions_send_result = Some(AlephTool::call(&tool, args).await.unwrap());
}

#[when(expr = "I setup sessions_send tool with context for agent {string}")]
async fn when_setup_sessions_send(w: &mut AlephWorld, agent: String) {
    let ctx = w.tools.as_mut().expect("Tools context not initialized");
    ctx.caller_agent_id = Some(agent);
}

// =============================================================================
// Then Steps - Tool Assertions (from server.feature)
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

// =============================================================================
// Then Steps - Smart Tool Discovery
// =============================================================================

#[then(expr = "the index entry name should be {string}")]
async fn then_index_entry_name(w: &mut AlephWorld, expected: String) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let entry = ctx.index_entry.as_ref().expect("Index entry not set");
    assert_eq!(entry.name, expected);
}

#[then(expr = "the index entry category should be {word}")]
async fn then_index_entry_category(w: &mut AlephWorld, expected: String) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let entry = ctx.index_entry.as_ref().expect("Index entry not set");
    let expected_cat = match expected.as_str() {
        "Core" => ToolIndexCategory::Core,
        "Builtin" => ToolIndexCategory::Builtin,
        "Mcp" => ToolIndexCategory::Mcp,
        "Skill" => ToolIndexCategory::Skill,
        "Custom" => ToolIndexCategory::Custom,
        _ => panic!("Unknown category: {}", expected),
    };
    assert_eq!(entry.category, expected_cat, "Category mismatch");
}

#[then("the index entry should be marked as core")]
async fn then_index_entry_is_core(w: &mut AlephWorld) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let entry = ctx.index_entry.as_ref().expect("Index entry not set");
    assert!(entry.is_core, "Expected entry to be marked as core");
}

#[then("the index entry should not be marked as core")]
async fn then_index_entry_not_core(w: &mut AlephWorld) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let entry = ctx.index_entry.as_ref().expect("Index entry not set");
    assert!(!entry.is_core, "Expected entry not to be marked as core");
}

#[then("the index entry summary should be at most 50 characters")]
async fn then_index_entry_summary_max_50(w: &mut AlephWorld) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let entry = ctx.index_entry.as_ref().expect("Index entry not set");
    assert!(entry.summary.len() <= 50, "Summary too long: {}", entry.summary.len());
}

#[then("the index entry summary should be exactly 50 characters")]
async fn then_index_entry_summary_exactly_50(w: &mut AlephWorld) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let entry = ctx.index_entry.as_ref().expect("Index entry not set");
    assert_eq!(entry.summary.len(), 50, "Summary length should be 50, got {}", entry.summary.len());
}

#[then(expr = "the index entry summary should end with {string}")]
async fn then_index_entry_summary_ends_with(w: &mut AlephWorld, suffix: String) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let entry = ctx.index_entry.as_ref().expect("Index entry not set");
    assert!(entry.summary.ends_with(&suffix), "Summary should end with '{}': {}", suffix, entry.summary);
}

#[then(expr = "the index entry keywords should contain {string}")]
async fn then_index_entry_keywords_contain(w: &mut AlephWorld, keyword: String) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let entry = ctx.index_entry.as_ref().expect("Index entry not set");
    assert!(
        entry.keywords.contains(&keyword),
        "Keywords should contain '{}': {:?}",
        keyword,
        entry.keywords
    );
}

#[then(expr = "the tool index total count should be {int}")]
async fn then_tool_index_total_count(w: &mut AlephWorld, expected: usize) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let index = ctx.tool_index.as_ref().expect("Tool index not set");
    assert_eq!(index.total_count(), expected, "Tool index total count mismatch");
}

#[then(expr = "the tool index core count should be {int}")]
async fn then_tool_index_core_count(w: &mut AlephWorld, expected: usize) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let index = ctx.tool_index.as_ref().expect("Tool index not set");
    assert_eq!(index.core.len(), expected, "Tool index core count mismatch");
}

#[then(expr = "the tool index mcp count should be {int}")]
async fn then_tool_index_mcp_count(w: &mut AlephWorld, expected: usize) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let index = ctx.tool_index.as_ref().expect("Tool index not set");
    assert_eq!(index.mcp.len(), expected, "Tool index MCP count mismatch");
}

#[then(expr = "the tool index skill count should be {int}")]
async fn then_tool_index_skill_count(w: &mut AlephWorld, expected: usize) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let index = ctx.tool_index.as_ref().expect("Tool index not set");
    assert_eq!(index.skill.len(), expected, "Tool index skill count mismatch");
}

#[then(expr = "the tool prompt should contain {string}")]
async fn then_tool_prompt_contains(w: &mut AlephWorld, expected: String) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let prompt = ctx.prompt.as_ref().expect("Prompt not set");
    assert!(prompt.contains(&expected), "Prompt should contain '{}': {}", expected, prompt);
}

#[then(expr = "the list result total count should be {int}")]
async fn then_list_result_count(w: &mut AlephWorld, expected: usize) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let result = ctx.list_result.as_ref().expect("List result not set");
    assert_eq!(result.total_count, expected, "List result count mismatch");
}

#[then(expr = "the list result total count should be at least {int}")]
async fn then_list_result_count_at_least(w: &mut AlephWorld, expected: usize) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let result = ctx.list_result.as_ref().expect("List result not set");
    assert!(result.total_count >= expected, "Expected at least {}, got {}", expected, result.total_count);
}

#[then(expr = "the list result total count should be greater than {int}")]
async fn then_list_result_count_greater_than(w: &mut AlephWorld, expected: usize) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let result = ctx.list_result.as_ref().expect("List result not set");
    assert!(result.total_count > expected, "Expected > {}, got {}", expected, result.total_count);
}

#[then("the list result tools should be empty")]
async fn then_list_result_empty(w: &mut AlephWorld) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let result = ctx.list_result.as_ref().expect("List result not set");
    assert!(result.tools.is_empty(), "List result should be empty");
}

#[then("the list result tools should not be empty")]
async fn then_list_result_not_empty(w: &mut AlephWorld) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let result = ctx.list_result.as_ref().expect("List result not set");
    assert!(!result.tools.is_empty(), "List result should not be empty");
}

#[then(expr = "all list result entries should have category {word}")]
async fn then_all_list_result_category(w: &mut AlephWorld, category: String) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let result = ctx.list_result.as_ref().expect("List result not set");
    let expected_cat = match category.as_str() {
        "Core" => ToolIndexCategory::Core,
        "Builtin" => ToolIndexCategory::Builtin,
        "Mcp" => ToolIndexCategory::Mcp,
        "Skill" => ToolIndexCategory::Skill,
        "Custom" => ToolIndexCategory::Custom,
        _ => panic!("Unknown category: {}", category),
    };
    for entry in &result.tools {
        assert_eq!(entry.category, expected_cat, "Entry {} has wrong category", entry.name);
    }
}

#[then("the schema result should be found")]
async fn then_schema_found(w: &mut AlephWorld) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let result = ctx.schema_result.as_ref().expect("Schema result not set");
    assert!(result.found, "Schema should be found");
}

#[then("the schema result should not be found")]
async fn then_schema_not_found(w: &mut AlephWorld) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let result = ctx.schema_result.as_ref().expect("Schema result not set");
    assert!(!result.found, "Schema should not be found");
}

#[then(expr = "the schema result name should be {string}")]
async fn then_schema_name(w: &mut AlephWorld, expected: String) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let result = ctx.schema_result.as_ref().expect("Schema result not set");
    assert_eq!(result.name, expected, "Schema name mismatch");
}

#[then(expr = "the schema result description should contain {string}")]
async fn then_schema_description_contains(w: &mut AlephWorld, expected: String) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let result = ctx.schema_result.as_ref().expect("Schema result not set");
    assert!(result.description.contains(&expected), "Schema description should contain '{}'", expected);
}

#[then(expr = "the schema result parameters should have {string}")]
async fn then_schema_parameters_has(w: &mut AlephWorld, key: String) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let result = ctx.schema_result.as_ref().expect("Schema result not set");
    assert!(result.parameters.get(&key).is_some(), "Schema parameters should have '{}'", key);
}

#[then(expr = "the schema result error should contain {string}")]
async fn then_schema_error_contains(w: &mut AlephWorld, expected: String) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let result = ctx.schema_result.as_ref().expect("Schema result not set");
    let error = result.error.as_ref().expect("Schema error not set");
    assert!(error.contains(&expected), "Schema error should contain '{}': {}", expected, error);
}

#[then(expr = "the schema result suggestions should contain {string}")]
async fn then_schema_suggestions_contain(w: &mut AlephWorld, expected: String) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let result = ctx.schema_result.as_ref().expect("Schema result not set");
    assert!(
        result.suggestions.contains(&expected),
        "Schema suggestions should contain '{}': {:?}",
        expected,
        result.suggestions
    );
}

#[then(expr = "the first core tool should be {string}")]
async fn then_first_core_tool(w: &mut AlephWorld, expected: String) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let index = ctx.tool_index.as_ref().expect("Tool index not set");
    assert!(!index.core.is_empty(), "Core tools should not be empty");
    assert_eq!(index.core[0].name, expected, "First core tool mismatch");
}

#[then("the first core tool should be marked as core")]
async fn then_first_core_tool_marked(w: &mut AlephWorld) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let index = ctx.tool_index.as_ref().expect("Tool index not set");
    assert!(!index.core.is_empty(), "Core tools should not be empty");
    assert!(index.core[0].is_core, "First core tool should be marked as core");
}

#[then(expr = "the token savings should be greater than {int} percent")]
async fn then_token_savings(w: &mut AlephWorld, expected: usize) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let full = ctx.full_schema_size.expect("Full schema size not set");
    let index = ctx.index_size.expect("Index size not set");
    let savings = ((full - index) as f64 / full as f64) * 100.0;
    assert!(savings > expected as f64, "Expected >{}% savings, got {:.1}%", expected, savings);
}

#[then(expr = "the token efficiency should be greater than {int} percent")]
async fn then_token_efficiency(w: &mut AlephWorld, expected: usize) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    // Calculate efficiency from registry vs prompt
    if let (Some(registry), Some(prompt)) = (&ctx.tool_registry, &ctx.prompt) {
        let rt = tokio::runtime::Handle::current();
        let full_size: usize = rt.block_on(async {
            let reg = registry.read().await;
            let tools = reg.list_all().await;
            tools.iter().map(|t| {
                t.name.len() + t.description.len() +
                t.parameters_schema.as_ref()
                    .map(|s| serde_json::to_string(s).unwrap_or_default().len())
                    .unwrap_or(0)
            }).sum()
        });
        let efficiency = (1.0 - (prompt.len() as f64 / full_size as f64)) * 100.0;
        assert!(efficiency > expected as f64, "Expected >{}% efficiency, got {:.1}%", expected, efficiency);
    }
}

#[then(expr = "the average list_tools latency should be under {int} microseconds")]
async fn then_list_tools_latency(w: &mut AlephWorld, max_us: u128) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let latency = ctx.latencies.list_tools_us.expect("Latency not measured");
    assert!(latency < max_us, "list_tools too slow: {}us", latency);
}

#[then(expr = "the average get_tool_schema latency should be under {int} microseconds")]
async fn then_get_schema_latency(w: &mut AlephWorld, max_us: u128) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let latency = ctx.latencies.get_tool_schema_us.expect("Latency not measured");
    assert!(latency < max_us, "get_tool_schema too slow: {}us", latency);
}

#[then(expr = "the average generate_index latency should be under {int} microseconds")]
async fn then_generate_index_latency(w: &mut AlephWorld, max_us: u128) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let latency = ctx.latencies.generate_index_us.expect("Latency not measured");
    assert!(latency < max_us, "generate_index too slow: {}us", latency);
}

// =============================================================================
// Then Steps - Sub-Agent
// =============================================================================

#[then("the parsed result should be successful")]
async fn then_parsed_result_success(w: &mut AlephWorld) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let result = ctx.delegate_result.as_ref().expect("Delegate result not set");
    assert!(result.success, "Parsed result should be successful");
}

#[then(expr = "the parsed result agent_id should be {string}")]
async fn then_parsed_agent_id(w: &mut AlephWorld, expected: String) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let result = ctx.delegate_result.as_ref().expect("Delegate result not set");
    assert_eq!(result.agent_id, expected, "Agent ID mismatch");
}

#[then(expr = "the parsed result should have {int} artifact")]
async fn then_parsed_artifacts(w: &mut AlephWorld, expected: usize) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let result = ctx.delegate_result.as_ref().expect("Delegate result not set");
    assert_eq!(result.artifacts.len(), expected, "Artifact count mismatch");
}

#[then(expr = "the parsed result should have {int} tool call")]
async fn then_parsed_tool_calls(w: &mut AlephWorld, expected: usize) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let result = ctx.delegate_result.as_ref().expect("Delegate result not set");
    assert_eq!(result.tools_called.len(), expected, "Tool call count mismatch");
}

#[then(expr = "the parsed result iterations used should be {int}")]
async fn then_parsed_iterations(w: &mut AlephWorld, expected: usize) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let result = ctx.delegate_result.as_ref().expect("Delegate result not set");
    assert_eq!(result.iterations_used, expected as u32, "Iterations mismatch");
}

#[then("the merged result should be successful")]
async fn then_merged_success(w: &mut AlephWorld) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let result = ctx.merged_result.as_ref().expect("Merged result not set");
    assert!(result.success, "Merged result should be successful");
}

#[then(expr = "the merged result summary should be {string}")]
async fn then_merged_summary(w: &mut AlephWorld, expected: String) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let result = ctx.merged_result.as_ref().expect("Merged result not set");
    assert_eq!(result.summary, expected, "Merged summary mismatch");
}

#[then(expr = "the merged result should have {int} artifact")]
async fn then_merged_artifacts(w: &mut AlephWorld, expected: usize) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let result = ctx.merged_result.as_ref().expect("Merged result not set");
    assert_eq!(result.artifacts.len(), expected, "Merged artifact count mismatch");
}

#[then(expr = "the merged result should have {int} tool call")]
async fn then_merged_tool_calls(w: &mut AlephWorld, expected: usize) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let result = ctx.merged_result.as_ref().expect("Merged result not set");
    assert_eq!(result.tool_calls.len(), expected, "Merged tool call count mismatch");
}

#[then("the merged result error should be none")]
async fn then_merged_no_error(w: &mut AlephWorld) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let result = ctx.merged_result.as_ref().expect("Merged result not set");
    assert!(result.error.is_none(), "Merged result should have no error");
}

#[then(expr = "the request prompt should be {string}")]
async fn then_request_prompt(w: &mut AlephWorld, expected: String) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let request = ctx.sub_agent_request.as_ref().expect("Sub-agent request not set");
    assert_eq!(request.prompt, expected, "Request prompt mismatch");
}

#[then(expr = "the request target should be {string}")]
async fn then_request_target(w: &mut AlephWorld, expected: String) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let request = ctx.sub_agent_request.as_ref().expect("Sub-agent request not set");
    assert_eq!(request.target, Some(expected), "Request target mismatch");
}

#[then(expr = "the request max iterations should be {int}")]
async fn then_request_max_iterations(w: &mut AlephWorld, expected: usize) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let request = ctx.sub_agent_request.as_ref().expect("Sub-agent request not set");
    assert_eq!(request.max_iterations, Some(expected as u32), "Request max iterations mismatch");
}

#[then(expr = "the request execution context working directory should be {string}")]
async fn then_request_exec_ctx_workdir(w: &mut AlephWorld, expected: String) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let request = ctx.sub_agent_request.as_ref().expect("Sub-agent request not set");
    let exec_ctx = request.execution_context.as_ref().expect("Execution context not set");
    assert_eq!(exec_ctx.working_directory, Some(expected), "Working directory mismatch");
}

#[then(expr = "the request execution context current app should be {string}")]
async fn then_request_exec_ctx_app(w: &mut AlephWorld, expected: String) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let request = ctx.sub_agent_request.as_ref().expect("Sub-agent request not set");
    let exec_ctx = request.execution_context.as_ref().expect("Execution context not set");
    assert_eq!(exec_ctx.current_app, Some(expected), "Current app mismatch");
}

// =============================================================================
// Then Steps - Sessions Tools
// =============================================================================

#[then(expr = "the sessions list count should be {int}")]
async fn then_sessions_list_count(w: &mut AlephWorld, expected: usize) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let result = ctx.sessions_list_result.as_ref().expect("Sessions list result not set");
    assert_eq!(result.count, expected, "Sessions list count mismatch");
}

#[then(expr = "the sessions list count should be at least {int}")]
async fn then_sessions_list_count_at_least(w: &mut AlephWorld, expected: usize) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let result = ctx.sessions_list_result.as_ref().expect("Sessions list result not set");
    assert!(result.count >= expected, "Expected at least {}, got {}", expected, result.count);
}

#[then("the sessions list should be empty")]
async fn then_sessions_list_empty(w: &mut AlephWorld) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let result = ctx.sessions_list_result.as_ref().expect("Sessions list result not set");
    assert!(result.sessions.is_empty(), "Sessions list should be empty");
}

#[then(expr = "all sessions should have kind {string}")]
async fn then_all_sessions_kind(w: &mut AlephWorld, expected: String) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let result = ctx.sessions_list_result.as_ref().expect("Sessions list result not set");
    for session in &result.sessions {
        assert_eq!(session.kind, expected, "Session kind mismatch");
    }
}

#[then(expr = "sessions should include kind {string}")]
async fn then_sessions_include_kind(w: &mut AlephWorld, expected: String) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let result = ctx.sessions_list_result.as_ref().expect("Sessions list result not set");
    let has_kind = result.sessions.iter().any(|s| s.kind == expected);
    assert!(has_kind, "Sessions should include kind '{}'", expected);
}

#[then(expr = "sessions should not include kind {string}")]
async fn then_sessions_not_include_kind(w: &mut AlephWorld, expected: String) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let result = ctx.sessions_list_result.as_ref().expect("Sessions list result not set");
    let has_kind = result.sessions.iter().any(|s| s.kind == expected);
    assert!(!has_kind, "Sessions should not include kind '{}'", expected);
}

#[then("the first session should have messages")]
async fn then_first_session_has_messages(w: &mut AlephWorld) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let result = ctx.sessions_list_result.as_ref().expect("Sessions list result not set");
    assert!(!result.sessions.is_empty(), "Sessions list should not be empty");
    assert!(result.sessions[0].messages.is_some(), "First session should have messages");
}

#[then(expr = "the first session should have {int} messages")]
async fn then_first_session_message_count(w: &mut AlephWorld, expected: usize) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let result = ctx.sessions_list_result.as_ref().expect("Sessions list result not set");
    let messages = result.sessions[0].messages.as_ref().expect("Messages not set");
    assert_eq!(messages.len(), expected, "Message count mismatch");
}

#[then(expr = "the first session key should contain {string}")]
async fn then_first_session_key_contains(w: &mut AlephWorld, expected: String) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let result = ctx.sessions_list_result.as_ref().expect("Sessions list result not set");
    assert!(!result.sessions.is_empty(), "Sessions list should not be empty");
    assert!(result.sessions[0].key.contains(&expected), "First session key should contain '{}'", expected);
}

#[then(expr = "all session keys should contain {string}")]
async fn then_all_session_keys_contain(w: &mut AlephWorld, expected: String) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let result = ctx.sessions_list_result.as_ref().expect("Sessions list result not set");
    for session in &result.sessions {
        assert!(session.key.contains(&expected), "Session key should contain '{}': {}", expected, session.key);
    }
}

#[then(expr = "the send status should be {word}")]
async fn then_send_status(w: &mut AlephWorld, expected: String) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let result = ctx.sessions_send_result.as_ref().expect("Sessions send result not set");
    let expected_status = match expected.as_str() {
        "Accepted" => SessionsSendStatus::Accepted,
        "Error" => SessionsSendStatus::Error,
        "Forbidden" => SessionsSendStatus::Forbidden,
        "Completed" => SessionsSendStatus::Ok,
        "TimedOut" => SessionsSendStatus::Timeout,
        _ => panic!("Unknown status: {}", expected),
    };
    assert_eq!(result.status, expected_status, "Send status mismatch");
}

#[then(expr = "the send error should contain {string}")]
async fn then_send_error_contains(w: &mut AlephWorld, expected: String) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let result = ctx.sessions_send_result.as_ref().expect("Sessions send result not set");
    let error = result.error.as_ref().expect("Error not set");
    assert!(error.contains(&expected), "Error should contain '{}': {}", expected, error);
}

#[then("the send result should have a session key")]
async fn then_send_has_session_key(w: &mut AlephWorld) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let result = ctx.sessions_send_result.as_ref().expect("Sessions send result not set");
    assert!(result.session_key.is_some(), "Send result should have session key");
}

#[then("the send result should not have a reply")]
async fn then_send_no_reply(w: &mut AlephWorld) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    let result = ctx.sessions_send_result.as_ref().expect("Sessions send result not set");
    assert!(result.reply.is_none(), "Send result should not have reply");
}

#[then(expr = "after {int}ms the adapter should have been called at least {int} time")]
async fn then_adapter_called(w: &mut AlephWorld, delay_ms: u64, expected: usize) {
    let ctx = w.tools.as_ref().expect("Tools context not initialized");
    tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
    let adapter = ctx.tracking_adapter.as_ref().expect("Tracking adapter not set");
    assert!(
        adapter.call_count() >= expected,
        "Adapter should have been called at least {} time(s), got {}",
        expected,
        adapter.call_count()
    );
}
