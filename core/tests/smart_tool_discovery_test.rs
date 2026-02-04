//! Smart Tool Discovery Integration Tests
//!
//! Tests for the Smart Tool Discovery System including:
//! - Tool Index generation
//! - Meta tools (list_tools, get_tool_schema)
//! - Two-stage discovery workflow
//! - Performance benchmarks
//! - End-to-end tests with many tools

use std::sync::Arc;
use std::time::Instant;

use serde_json::json;
use tokio::sync::RwLock;

use alephcore::dispatcher::{
    ToolIndex, ToolIndexCategory, ToolIndexEntry, ToolRegistry, ToolSource, UnifiedTool,
};
use alephcore::builtin_tools::meta_tools::{
    GetToolSchemaArgs, GetToolSchemaTool, ListToolsArgs, ListToolsTool,
};
use alephcore::tools::AlephTool;

/// Helper to register a tool in the registry (via Arc<RwLock<>>)
async fn register_tool(registry: &Arc<RwLock<ToolRegistry>>, tool: UnifiedTool) {
    let reg = registry.read().await;
    reg.register_with_conflict_resolution(tool).await;
}

// ============================================================================
// 4.1 Unit Tests for ToolIndex Generation
// ============================================================================

#[test]
fn test_tool_index_generation_basic() {
    let tool = UnifiedTool::new(
        "builtin:search",
        "search",
        "Search the web for information",
        ToolSource::Builtin,
    );

    let entry = tool.to_index_entry(&["search"]);

    assert_eq!(entry.name, "search");
    assert_eq!(entry.category, ToolIndexCategory::Core); // Marked as core
    assert!(entry.is_core);
    assert!(entry.summary.len() <= 50);
}

#[test]
fn test_tool_index_generation_non_core() {
    let tool = UnifiedTool::new(
        "mcp:github:pr_list",
        "github:pr_list",
        "List all pull requests from a GitHub repository with filtering options",
        ToolSource::Mcp {
            server: "github".into(),
        },
    );

    let entry = tool.to_index_entry(&["search", "file_ops"]);

    assert_eq!(entry.name, "github:pr_list");
    assert_eq!(entry.category, ToolIndexCategory::Mcp);
    assert!(!entry.is_core); // Not in core tools list
}

#[test]
fn test_tool_index_truncates_long_descriptions() {
    let long_description = "This is a very long description that definitely exceeds the fifty character limit and should be truncated with ellipsis";
    let tool = UnifiedTool::new(
        "builtin:test",
        "test_tool",
        long_description,
        ToolSource::Builtin,
    );

    let entry = tool.to_index_entry(&[]);

    assert_eq!(entry.summary.len(), 50);
    assert!(entry.summary.ends_with("..."));
}

#[test]
fn test_tool_index_extracts_keywords() {
    let tool = UnifiedTool::new(
        "mcp:github:pr_create",
        "github:pr_create",
        "Create a pull request",
        ToolSource::Mcp {
            server: "github".into(),
        },
    );

    let entry = tool.to_index_entry(&[]);

    // Keywords should be extracted from name parts
    assert!(entry.keywords.contains(&"github".to_string()));
    // Note: 'pr' is only 2 chars, filtered out by len > 2 check
    assert!(entry.keywords.contains(&"create".to_string()));
}

#[test]
fn test_tool_index_category_mapping() {
    // Test all source types map to correct index categories
    let cases = vec![
        (ToolSource::Builtin, ToolIndexCategory::Builtin),
        (ToolSource::Native, ToolIndexCategory::Builtin), // Native treated as builtin
        (
            ToolSource::Mcp {
                server: "test".into(),
            },
            ToolIndexCategory::Mcp,
        ),
        (
            ToolSource::Skill {
                id: "test".into(),
            },
            ToolIndexCategory::Skill,
        ),
        (
            ToolSource::Custom { rule_index: 0 },
            ToolIndexCategory::Custom,
        ),
    ];

    for (source, expected_category) in cases {
        let tool = UnifiedTool::new("test:tool", "test", "Test tool", source);
        let entry = tool.to_index_entry(&[]);
        assert_eq!(
            entry.category, expected_category,
            "Source {:?} should map to {:?}",
            tool.source, expected_category
        );
    }
}

#[test]
fn test_tool_index_add_and_count() {
    let mut index = ToolIndex::new();

    index.add(ToolIndexEntry::new(
        "search",
        ToolIndexCategory::Core,
        "Web search",
    ));
    index.add(ToolIndexEntry::new(
        "file_ops",
        ToolIndexCategory::Core,
        "File operations",
    ));
    index.add(ToolIndexEntry::new(
        "github:pr_list",
        ToolIndexCategory::Mcp,
        "List PRs",
    ));
    index.add(ToolIndexEntry::new(
        "code-review",
        ToolIndexCategory::Skill,
        "Review code",
    ));

    assert_eq!(index.total_count(), 4);
    assert_eq!(index.core.len(), 2);
    assert_eq!(index.mcp.len(), 1);
    assert_eq!(index.skill.len(), 1);
}

#[test]
fn test_tool_index_to_prompt_format() {
    let mut index = ToolIndex::new();

    index.add(
        ToolIndexEntry::new("search", ToolIndexCategory::Core, "Web search").with_core(true),
    );
    index.add(ToolIndexEntry::new(
        "github:pr_list",
        ToolIndexCategory::Mcp,
        "List PRs",
    ));

    let prompt = index.to_prompt();

    assert!(prompt.contains("## Available Tools"));
    assert!(prompt.contains("### Core"));
    assert!(prompt.contains("- search: Web search"));
    assert!(prompt.contains("### MCP"));
    assert!(prompt.contains("- github:pr_list: List PRs"));
}

// ============================================================================
// 4.2 Unit Tests for Meta Tools
// ============================================================================

#[tokio::test]
async fn test_list_tools_empty_registry() {
    let registry = Arc::new(RwLock::new(ToolRegistry::new()));
    let tool = ListToolsTool::new(registry);

    let args = ListToolsArgs { category: None };
    let result = tool.call(args).await.unwrap();

    assert_eq!(result.total_count, 0);
    assert!(result.tools.is_empty());
}

#[tokio::test]
async fn test_list_tools_with_tools() {
    let registry = Arc::new(RwLock::new(ToolRegistry::new()));

    // Add some test tools
    register_tool(&registry, UnifiedTool::new(
        "builtin:search",
        "search",
        "Web search",
        ToolSource::Builtin,
    )).await;
    register_tool(&registry, UnifiedTool::new(
        "mcp:github:pr_list",
        "github:pr_list",
        "List PRs",
        ToolSource::Mcp {
            server: "github".into(),
        },
    )).await;

    let tool = ListToolsTool::new(registry);
    let args = ListToolsArgs { category: None };
    let result = tool.call(args).await.unwrap();

    assert_eq!(result.total_count, 2);
    assert!(!result.tools.is_empty());
}

#[tokio::test]
async fn test_list_tools_with_category_filter() {
    let registry = Arc::new(RwLock::new(ToolRegistry::new()));

    register_tool(&registry, UnifiedTool::new(
        "builtin:search",
        "search",
        "Web search",
        ToolSource::Builtin,
    )).await;
    register_tool(&registry, UnifiedTool::new(
        "mcp:github:pr_list",
        "github:pr_list",
        "List PRs",
        ToolSource::Mcp {
            server: "github".into(),
        },
    )).await;

    let tool = ListToolsTool::new(registry);

    // Filter by MCP category
    let args = ListToolsArgs {
        category: Some("mcp".to_string()),
    };
    let result = tool.call(args).await.unwrap();

    // Should only return MCP tools
    for entry in &result.tools {
        assert_eq!(entry.category, ToolIndexCategory::Mcp);
    }
}

#[tokio::test]
async fn test_get_tool_schema_found() {
    let registry = Arc::new(RwLock::new(ToolRegistry::new()));

    register_tool(&registry, UnifiedTool::new(
        "builtin:search",
        "search",
        "Search the web for information",
        ToolSource::Builtin,
    )
    .with_parameters_schema(json!({
        "type": "object",
        "properties": {
            "query": { "type": "string" }
        },
        "required": ["query"]
    }))).await;

    let tool = GetToolSchemaTool::new(registry);
    let args = GetToolSchemaArgs {
        tool_name: "search".to_string(),
    };
    let result = tool.call(args).await.unwrap();

    assert!(result.found);
    assert_eq!(result.name, "search");
    assert!(result.description.contains("Search"));
    assert!(result.parameters.get("properties").is_some());
}

#[tokio::test]
async fn test_get_tool_schema_not_found() {
    let registry = Arc::new(RwLock::new(ToolRegistry::new()));
    let tool = GetToolSchemaTool::new(registry);

    let args = GetToolSchemaArgs {
        tool_name: "nonexistent_tool".to_string(),
    };
    let result = tool.call(args).await.unwrap();

    assert!(!result.found);
    assert!(result.error.is_some());
    assert!(result.error.unwrap().contains("not found"));
}

#[tokio::test]
async fn test_get_tool_schema_with_suggestions() {
    let registry = Arc::new(RwLock::new(ToolRegistry::new()));

    register_tool(&registry, UnifiedTool::new(
        "builtin:search",
        "search",
        "Web search",
        ToolSource::Builtin,
    )).await;

    let tool = GetToolSchemaTool::new(registry);

    // Misspelled tool name
    let args = GetToolSchemaArgs {
        tool_name: "serach".to_string(), // typo
    };
    let result = tool.call(args).await.unwrap();

    assert!(!result.found);
    // Should suggest "search" due to fuzzy matching
    assert!(
        result.suggestions.contains(&"search".to_string()),
        "Expected 'search' in suggestions: {:?}",
        result.suggestions
    );
}

// ============================================================================
// 4.3 Integration Tests for Two-Stage Discovery
// ============================================================================

#[tokio::test]
async fn test_two_stage_discovery_workflow() {
    let registry = Arc::new(RwLock::new(ToolRegistry::new()));

    // Register multiple tools
    // Core tools
    register_tool(&registry, UnifiedTool::new("builtin:search", "search", "Web search", ToolSource::Builtin)
        .with_parameters_schema(json!({"type": "object"}))).await;
    register_tool(&registry, UnifiedTool::new("builtin:file_ops", "file_ops", "File ops", ToolSource::Builtin)
        .with_parameters_schema(json!({"type": "object"}))).await;

    // MCP tools
    for i in 0..10 {
        register_tool(&registry, UnifiedTool::new(
            format!("mcp:github:tool_{}", i),
            format!("github:tool_{}", i),
            format!("GitHub tool {}", i),
            ToolSource::Mcp {
                server: "github".into(),
            },
        )).await;
    }

    // Stage 1: List tools to get index
    let list_tool = ListToolsTool::new(Arc::clone(&registry));
    let list_result = list_tool.call(ListToolsArgs { category: None }).await.unwrap();

    assert_eq!(list_result.total_count, 12);
    assert!(!list_result.tools.is_empty());

    // Stage 2: Get specific tool schema
    let schema_tool = GetToolSchemaTool::new(Arc::clone(&registry));
    let schema_result = schema_tool
        .call(GetToolSchemaArgs {
            tool_name: "search".to_string(),
        })
        .await
        .unwrap();

    assert!(schema_result.found);
    assert_eq!(schema_result.name, "search");
}

#[tokio::test]
async fn test_tool_index_generation_from_registry() {
    let registry = Arc::new(RwLock::new(ToolRegistry::new()));

    register_tool(&registry, UnifiedTool::new("builtin:search", "search", "Web search", ToolSource::Builtin)).await;
    register_tool(&registry, UnifiedTool::new(
        "mcp:github:pr_list",
        "github:pr_list",
        "List PRs",
        ToolSource::Mcp {
            server: "github".into(),
        },
    )).await;

    let reg = registry.read().await;
    let index = reg.generate_tool_index(&["search"]).await;

    assert_eq!(index.total_count(), 2);
    assert_eq!(index.core.len(), 1);
    assert_eq!(index.mcp.len(), 1);
    assert_eq!(index.core[0].name, "search");
    assert!(index.core[0].is_core);
}

// ============================================================================
// 4.4 Benchmark: Token Consumption Comparison
// ============================================================================

#[tokio::test]
async fn test_token_consumption_comparison() {
    let registry = Arc::new(RwLock::new(ToolRegistry::new()));

    // Create 50 tools with realistic schemas
    for i in 0..50 {
        let schema = json!({
            "type": "object",
            "properties": {
                "param1": { "type": "string", "description": format!("Parameter 1 for tool {}", i) },
                "param2": { "type": "integer", "description": "Optional count parameter" },
                "param3": { "type": "boolean", "default": false }
            },
            "required": ["param1"]
        });

        register_tool(&registry, UnifiedTool::new(
            format!("mcp:server:tool_{}", i),
            format!("tool_{}", i),
            format!("This is tool {} which does something useful for the user. It has multiple parameters and options.", i),
            ToolSource::Mcp {
                server: "server".into(),
            },
        )
        .with_parameters_schema(schema)).await;
    }

    // Measure full schema size
    let full_schema_text = {
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
        text
    };

    // Measure index-only size
    let index_text = {
        let reg = registry.read().await;
        let index = reg.generate_tool_index(&["search", "file_ops"]).await;
        index.to_prompt()
    };

    // Calculate approximate token counts (rough: 1 token ≈ 4 chars)
    let full_tokens = full_schema_text.len() / 4;
    let index_tokens = index_text.len() / 4;
    let savings_percent = ((full_tokens - index_tokens) as f64 / full_tokens as f64) * 100.0;

    println!("=== Token Consumption Comparison ===");
    println!("Full Schema: {} chars (~{} tokens)", full_schema_text.len(), full_tokens);
    println!("Index Only:  {} chars (~{} tokens)", index_text.len(), index_tokens);
    println!("Savings:     {:.1}%", savings_percent);

    // Assert significant savings (should save at least 50% tokens)
    assert!(
        savings_percent > 50.0,
        "Expected >50% token savings, got {:.1}%",
        savings_percent
    );
}

// ============================================================================
// 4.5 Benchmark: Latency Comparison
// ============================================================================

#[tokio::test]
async fn test_latency_comparison() {
    let registry = Arc::new(RwLock::new(ToolRegistry::new()));

    // Setup: Create 100 tools
    for i in 0..100 {
        register_tool(&registry, UnifiedTool::new(
            format!("mcp:server:tool_{}", i),
            format!("tool_{}", i),
            format!("Tool {} description", i),
            ToolSource::Mcp {
                server: "server".into(),
            },
        )
        .with_parameters_schema(json!({
            "type": "object",
            "properties": {
                "input": { "type": "string" }
            }
        }))).await;
    }

    // Measure: List all tools (index generation)
    let iterations = 100;

    let list_tool = ListToolsTool::new(Arc::clone(&registry));
    let list_start = Instant::now();
    for _ in 0..iterations {
        let _ = list_tool.call(ListToolsArgs { category: None }).await;
    }
    let list_duration = list_start.elapsed();
    let list_avg_us = list_duration.as_micros() / iterations as u128;

    // Measure: Get single tool schema
    let schema_tool = GetToolSchemaTool::new(Arc::clone(&registry));
    let schema_start = Instant::now();
    for _ in 0..iterations {
        let _ = schema_tool
            .call(GetToolSchemaArgs {
                tool_name: "tool_50".to_string(),
            })
            .await;
    }
    let schema_duration = schema_start.elapsed();
    let schema_avg_us = schema_duration.as_micros() / iterations as u128;

    // Measure: Generate tool index
    let index_start = Instant::now();
    for _ in 0..iterations {
        let reg = registry.read().await;
        let _ = reg.generate_tool_index(&["search"]).await;
    }
    let index_duration = index_start.elapsed();
    let index_avg_us = index_duration.as_micros() / iterations as u128;

    println!("=== Latency Benchmark (100 tools, {} iterations) ===", iterations);
    println!("list_tools:      {}µs average", list_avg_us);
    println!("get_tool_schema: {}µs average", schema_avg_us);
    println!("generate_index:  {}µs average", index_avg_us);

    // Assert reasonable latencies (should be sub-millisecond for local operations)
    assert!(
        list_avg_us < 10000, // < 10ms
        "list_tools too slow: {}µs",
        list_avg_us
    );
    assert!(
        schema_avg_us < 5000, // < 5ms
        "get_tool_schema too slow: {}µs",
        schema_avg_us
    );
    assert!(
        index_avg_us < 10000, // < 10ms
        "generate_index too slow: {}µs",
        index_avg_us
    );
}

// ============================================================================
// 4.6 End-to-End Test with 50+ Tools
// ============================================================================

#[tokio::test]
async fn test_end_to_end_with_many_tools() {
    let registry = Arc::new(RwLock::new(ToolRegistry::new()));

    // Setup: Create a realistic tool mix with realistic schemas
    // Helper function to create complex schema
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

    // Core builtin tools (5) with realistic schemas
    register_tool(&registry, UnifiedTool::new(
        "builtin:search",
        "search",
        "Search the web for information using various search engines and return relevant results",
        ToolSource::Builtin,
    ).with_parameters_schema(make_schema(&[
        ("query", "The search query string", true),
        ("limit", "Maximum number of results to return", false),
        ("source", "Search engine to use: google, bing, duckduckgo", false),
    ]))).await;

    register_tool(&registry, UnifiedTool::new(
        "builtin:file_ops",
        "file_ops",
        "Perform file system operations including read, write, copy, move, and delete files",
        ToolSource::Builtin,
    ).with_parameters_schema(make_schema(&[
        ("operation", "Operation type: read, write, copy, move, delete", true),
        ("path", "Target file path", true),
        ("destination", "Destination path for copy/move operations", false),
        ("content", "Content to write for write operation", false),
    ]))).await;

    register_tool(&registry, UnifiedTool::new(
        "builtin:code_exec",
        "code_exec",
        "Execute code snippets in various programming languages with sandboxed environment",
        ToolSource::Builtin,
    ).with_parameters_schema(make_schema(&[
        ("language", "Programming language: python, javascript, rust, go", true),
        ("code", "Code snippet to execute", true),
        ("timeout", "Maximum execution time in seconds", false),
        ("stdin", "Standard input for the program", false),
    ]))).await;

    register_tool(&registry, UnifiedTool::new(
        "builtin:web_fetch",
        "web_fetch",
        "Fetch content from URLs including web pages, APIs, and file downloads",
        ToolSource::Builtin,
    ).with_parameters_schema(make_schema(&[
        ("url", "URL to fetch content from", true),
        ("method", "HTTP method: GET, POST, PUT, DELETE", false),
        ("headers", "Custom HTTP headers as JSON object", false),
        ("body", "Request body for POST/PUT requests", false),
    ]))).await;

    register_tool(&registry, UnifiedTool::new(
        "builtin:youtube",
        "youtube",
        "Get YouTube video information including metadata, transcripts, and download options",
        ToolSource::Builtin,
    ).with_parameters_schema(make_schema(&[
        ("video_url", "YouTube video URL or ID", true),
        ("include_transcript", "Whether to include video transcript", false),
        ("quality", "Preferred video quality: 360p, 720p, 1080p, 4k", false),
    ]))).await;

    // GitHub MCP tools (15) with realistic schemas
    let github_tools = vec![
        ("pr_list", "List pull requests with filtering by state, author, and labels"),
        ("pr_create", "Create a new pull request with title, description, and reviewers"),
        ("pr_merge", "Merge a pull request with optional merge method and commit message"),
        ("issue_list", "List issues with filtering by state, assignee, and labels"),
        ("issue_create", "Create a new issue with title, body, labels, and assignees"),
        ("issue_close", "Close an issue with optional closing comment"),
        ("repo_info", "Get repository information including stats and settings"),
        ("branch_list", "List branches with protection status and latest commit"),
        ("commit_list", "List commits with filtering by branch, author, and date range"),
        ("file_read", "Read file content from repository at specified ref"),
        ("file_write", "Write or update file content with commit message"),
        ("workflow_list", "List GitHub Actions workflows with status"),
        ("workflow_run", "Trigger a GitHub Actions workflow run with inputs"),
        ("release_list", "List releases with assets and changelog"),
        ("release_create", "Create a new release with tag, name, and assets"),
    ];

    for (name, desc) in github_tools {
        register_tool(&registry, UnifiedTool::new(
            format!("mcp:github:{}", name),
            format!("github:{}", name),
            desc,
            ToolSource::Mcp { server: "github".into() },
        ).with_parameters_schema(make_schema(&[
            ("owner", "Repository owner/organization", true),
            ("repo", "Repository name", true),
            ("state", "Filter by state: open, closed, all", false),
            ("per_page", "Results per page (max 100)", false),
        ]))).await;
    }

    // Notion MCP tools (10) with realistic schemas
    let notion_tools = vec![
        ("page_read", "Read Notion page content including blocks and properties"),
        ("page_create", "Create a new Notion page with content and properties"),
        ("page_update", "Update an existing Notion page properties and content"),
        ("database_query", "Query a Notion database with filters and sorts"),
        ("database_create", "Create a new Notion database with schema"),
        ("block_read", "Read Notion block content and children"),
        ("block_append", "Append new blocks to a page or existing block"),
        ("search", "Search Notion workspace for pages and databases"),
        ("user_list", "List workspace users with access information"),
        ("comment_add", "Add a comment to a page or discussion"),
    ];

    for (name, desc) in notion_tools {
        register_tool(&registry, UnifiedTool::new(
            format!("mcp:notion:{}", name),
            format!("notion:{}", name),
            desc,
            ToolSource::Mcp { server: "notion".into() },
        ).with_parameters_schema(make_schema(&[
            ("page_id", "Notion page or database ID", false),
            ("query", "Search query or filter expression", false),
            ("content", "Content blocks as JSON array", false),
        ]))).await;
    }

    // Slack MCP tools (8) with realistic schemas
    let slack_tools = vec![
        ("message_post", "Post a message to a Slack channel or DM"),
        ("message_update", "Update an existing message in Slack"),
        ("channel_list", "List Slack channels with membership info"),
        ("channel_join", "Join a public Slack channel"),
        ("user_info", "Get Slack user profile and presence"),
        ("file_upload", "Upload a file to Slack with optional message"),
        ("reaction_add", "Add an emoji reaction to a message"),
        ("thread_reply", "Reply to a message in a thread"),
    ];

    for (name, desc) in slack_tools {
        register_tool(&registry, UnifiedTool::new(
            format!("mcp:slack:{}", name),
            format!("slack:{}", name),
            desc,
            ToolSource::Mcp { server: "slack".into() },
        ).with_parameters_schema(make_schema(&[
            ("channel", "Slack channel ID or name", false),
            ("text", "Message text with markdown support", false),
            ("thread_ts", "Thread timestamp for replies", false),
            ("attachments", "Rich message attachments as JSON", false),
        ]))).await;
    }

    // Skills (12) with realistic schemas
    let skills = vec![
        ("code-review", "Review code for bugs, style issues, and best practices"),
        ("refine-text", "Improve text clarity, grammar, and style"),
        ("translate", "Translate text between languages with context"),
        ("summarize", "Create concise summaries of documents or text"),
        ("generate-tests", "Generate unit tests for code functions"),
        ("explain-code", "Explain code functionality in plain language"),
        ("fix-bugs", "Analyze and suggest fixes for code bugs"),
        ("optimize", "Optimize code for performance and readability"),
        ("document", "Generate documentation for code and APIs"),
        ("format", "Format code according to style guidelines"),
        ("refactor", "Suggest code refactoring improvements"),
        ("analyze", "Analyze code structure and dependencies"),
    ];

    for (name, desc) in skills {
        register_tool(&registry, UnifiedTool::new(
            format!("skill:{}", name),
            name,
            desc,
            ToolSource::Skill { id: name.into() },
        ).with_parameters_schema(make_schema(&[
            ("input", "Input content to process", true),
            ("language", "Target language or programming language", false),
            ("style", "Style guide or preferences", false),
            ("options", "Additional processing options", false),
        ]))).await;
    }

    // Verify total count
    let total = {
        let reg = registry.read().await;
        reg.list_all().await.len()
    };

    assert!(
        total >= 50,
        "Expected at least 50 tools, got {}",
        total
    );
    println!("Total tools registered: {}", total);

    // Test 1: List all tools
    let list_tool = ListToolsTool::new(Arc::clone(&registry));
    let list_result = list_tool.call(ListToolsArgs { category: None }).await.unwrap();

    assert_eq!(list_result.total_count, total);
    println!("Listed {} tools", list_result.total_count);

    // Test 2: List by category
    let mcp_result = list_tool
        .call(ListToolsArgs {
            category: Some("mcp".to_string()),
        })
        .await
        .unwrap();

    println!("MCP tools: {}", mcp_result.total_count);
    assert!(mcp_result.total_count > 0);

    // Test 3: Get schema for specific tools
    let schema_tool = GetToolSchemaTool::new(Arc::clone(&registry));

    let search_schema = schema_tool
        .call(GetToolSchemaArgs {
            tool_name: "search".to_string(),
        })
        .await
        .unwrap();
    assert!(search_schema.found);

    let github_schema = schema_tool
        .call(GetToolSchemaArgs {
            tool_name: "github:pr_list".to_string(),
        })
        .await
        .unwrap();
    assert!(github_schema.found);

    // Test 4: Generate tool index
    let index = {
        let reg = registry.read().await;
        reg.generate_tool_index(&["search", "file_ops", "code_exec"]).await
    };

    println!("Index total: {}", index.total_count());
    println!("Core tools: {}", index.core.len());
    println!("Builtin tools: {}", index.builtin.len());
    println!("MCP tools: {}", index.mcp.len());
    println!("Skills: {}", index.skill.len());

    assert_eq!(index.total_count(), total);
    assert_eq!(index.core.len(), 3); // search, file_ops, code_exec

    // Test 5: Verify prompt generation
    let prompt = index.to_prompt();
    assert!(prompt.contains("## Available Tools"));
    assert!(prompt.contains("### Core"));
    assert!(prompt.contains("### MCP"));
    assert!(prompt.contains("### Skills"));

    // Calculate token efficiency
    let full_schema_chars: usize = {
        let reg = registry.read().await;
        let tools = reg.list_all().await;
        tools
            .iter()
            .map(|t| {
                t.name.len()
                    + t.description.len()
                    + t.parameters_schema
                        .as_ref()
                        .map(|s| serde_json::to_string(s).unwrap_or_default().len())
                        .unwrap_or(0)
            })
            .sum()
    };

    let index_chars = prompt.len();
    let efficiency = (1.0 - (index_chars as f64 / full_schema_chars as f64)) * 100.0;

    println!("\n=== Token Efficiency ===");
    println!("Full schemas: ~{} chars", full_schema_chars);
    println!("Index prompt: ~{} chars", index_chars);
    println!("Efficiency: {:.1}% reduction", efficiency);

    assert!(
        efficiency > 30.0,
        "Expected >30% reduction, got {:.1}%",
        efficiency
    );
}

// ============================================================================
// Sub-Agent Integration Tests
// ============================================================================

#[tokio::test]
async fn test_sub_agent_delegate_result_parsing() {
    use alephcore::agents::sub_agents::ResultMerger;

    let json = json!({
        "success": true,
        "summary": "Found 3 matching MCP tools",
        "agent_id": "mcp",
        "output": {"tools": ["github:pr_list", "github:pr_create", "github:pr_merge"]},
        "artifacts": [
            {"artifact_type": "file", "path": "/tmp/tools.json", "mime_type": "application/json"}
        ],
        "tools_called": [
            {"name": "list_tools", "success": true, "result_summary": "Listed 10 tools"}
        ],
        "iterations_used": 2,
        "error": null
    });

    let result = ResultMerger::parse_delegate_result(&json);
    assert!(result.is_some());

    let delegate_result = result.unwrap();
    assert!(delegate_result.success);
    assert_eq!(delegate_result.agent_id, "mcp");
    assert_eq!(delegate_result.artifacts.len(), 1);
    assert_eq!(delegate_result.tools_called.len(), 1);
    assert_eq!(delegate_result.iterations_used, 2);
}

#[tokio::test]
async fn test_sub_agent_result_merging() {
    use alephcore::agents::sub_agents::{ArtifactInfo, DelegateResult, ResultMerger, ToolCallInfo};

    let delegate_result = DelegateResult {
        success: true,
        summary: "Found matching tools".to_string(),
        agent_id: "skill".to_string(),
        output: Some(json!({"skills": ["code-review", "refactor"]})),
        artifacts: vec![
            ArtifactInfo {
                artifact_type: "file".to_string(),
                path: "/tmp/output.json".to_string(),
                mime_type: Some("application/json".to_string()),
            },
        ],
        tools_called: vec![
            ToolCallInfo {
                name: "list_skills".to_string(),
                success: true,
                result_summary: "Found 5 skills".to_string(),
            },
        ],
        iterations_used: 1,
        error: None,
    };

    let merged = ResultMerger::merge(&delegate_result);

    assert!(merged.success);
    assert_eq!(merged.summary, "Found matching tools");
    assert_eq!(merged.artifacts.len(), 1);
    assert_eq!(merged.tool_calls.len(), 1);
    assert!(merged.error.is_none());
}

#[tokio::test]
async fn test_sub_agent_context_passing() {
    use alephcore::agents::sub_agents::{ExecutionContextInfo, SubAgentRequest};

    let context = ExecutionContextInfo::new()
        .with_working_directory("/Users/test/project")
        .with_current_app("VSCode")
        .with_original_request("Help me find available GitHub tools")
        .with_history_summary("User asked about GitHub integration")
        .with_metadata("theme", "dark");

    let request = SubAgentRequest::new("List GitHub tools")
        .with_target("github")
        .with_max_iterations(5)
        .with_parent_session("session-123")
        .with_execution_context(context);

    assert_eq!(request.prompt, "List GitHub tools");
    assert_eq!(request.target, Some("github".to_string()));
    assert_eq!(request.max_iterations, Some(5));
    assert!(request.execution_context.is_some());

    let ctx = request.execution_context.unwrap();
    assert_eq!(ctx.working_directory, Some("/Users/test/project".to_string()));
    assert_eq!(ctx.current_app, Some("VSCode".to_string()));
}
