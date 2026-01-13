# Design: Enable Intelligent Tool Invocation

## Overview

本设计文档详细描述如何实现统一工具执行层，使 Aether 能够像 Claude Code 一样根据用户需求智能调用工具。

## Architecture

### Current State (问题)

```
┌─────────────────────────────────────────────────────────────────────────┐
│                          AetherCore                                      │
├─────────────────────────────────────────────────────────────────────────┤
│  IntentRoutingPipeline                                                   │
│  ├── L1 Regex: /search, /youtube, /chat                                 │
│  ├── L2 Semantic: keyword matching                                      │
│  └── L3 AI: infers tool_name + params                                   │
│                         ↓                                                │
│  execute_matched_tool(tool_name, params)                                │
│  ├── "youtube" → Capability::Video ✓                                    │
│  ├── "search" → Capability::Search ✓                                    │
│  ├── "memory" → Capability::Memory ✓                                    │
│  └── _ → ERROR "Unknown tool" ✗  ← Gap here                             │
├─────────────────────────────────────────────────────────────────────────┤
│  NativeToolRegistry (orphaned)                                           │
│  ├── FileReadTool, FileWriteTool, FileListTool...                       │
│  ├── GitStatusTool, GitDiffTool, GitLogTool...                          │
│  ├── ShellExecuteTool                                                   │
│  ├── WebSearchTool                                                      │
│  └── (NO WebFetchTool)                                                  │
│                                                                          │
│  CapabilityExecutor                                                      │
│  ├── Search → SearchExecutor                                            │
│  ├── Video → VideoStrategy                                              │
│  └── Memory → MemoryDb                                                  │
└─────────────────────────────────────────────────────────────────────────┘
```

### Target State (解决方案)

```
┌─────────────────────────────────────────────────────────────────────────┐
│                          AetherCore                                      │
├─────────────────────────────────────────────────────────────────────────┤
│  IntentRoutingPipeline                                                   │
│  ├── L1 Regex: /search, /youtube, /chat, /fetch                         │
│  ├── L2 Semantic: enhanced keyword matching                             │
│  └── L3 AI: sees ALL tools, infers tool_name + params                   │
│                         ↓                                                │
│  UnifiedToolExecutor.execute(tool_name, params, context)                │
│  ┌─────────────────────────────────────────────────────────────────┐    │
│  │ Tool Resolution Order:                                           │    │
│  │ 1. Builtin Capabilities (search, video, memory)                  │    │
│  │    └── CapabilityExecutor (existing flow, unchanged)             │    │
│  │ 2. Native Tools (AgentTool implementations)                      │    │
│  │    └── NativeToolRegistry.execute()                              │    │
│  │ 3. MCP Tools                                                     │    │
│  │    └── McpClient.call_tool()                                     │    │
│  │ 4. Skills (future)                                               │    │
│  │    └── SkillExecutor.run()                                       │    │
│  └─────────────────────────────────────────────────────────────────┘    │
│                         ↓                                                │
│  ToolExecutionResult { success, content, error }                        │
│                         ↓                                                │
│  PromptAssembler.inject_tool_result(result)                             │
│                         ↓                                                │
│  AI Provider → Final Response                                           │
└─────────────────────────────────────────────────────────────────────────┘
```

## Component Design

### 1. WebFetchTool

**Location**: `Aether/core/src/tools/web/mod.rs`

```rust
use async_trait::async_trait;
use scraper::{Html, Selector};
use reqwest::Client;

pub struct WebFetchTool {
    client: Client,
    config: WebFetchConfig,
}

#[derive(Debug, Clone)]
pub struct WebFetchConfig {
    /// Maximum content length to return (default: 50KB)
    pub max_content_bytes: usize,
    /// Request timeout in seconds
    pub timeout_seconds: u64,
    /// User agent string
    pub user_agent: String,
    /// Allowed URL patterns (empty = allow all)
    pub allowed_patterns: Vec<String>,
    /// Blocked domains
    pub blocked_domains: Vec<String>,
}

impl Default for WebFetchConfig {
    fn default() -> Self {
        Self {
            max_content_bytes: 50 * 1024, // 50KB
            timeout_seconds: 30,
            user_agent: "AetherBot/1.0 (+https://github.com/aether)".to_string(),
            allowed_patterns: vec![],
            blocked_domains: vec![],
        }
    }
}

#[async_trait]
impl AgentTool for WebFetchTool {
    fn name(&self) -> &str { "web_fetch" }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "web_fetch",
            "Fetch and extract readable content from a web page URL. Returns the main text content in Markdown format.",
            json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "The URL of the web page to fetch"
                    },
                    "include_links": {
                        "type": "boolean",
                        "description": "Whether to include links in the output (default: false)"
                    }
                },
                "required": ["url"]
            }),
            ToolCategory::Web,
        )
    }

    async fn execute(&self, args: &str) -> Result<ToolResult> {
        let params: WebFetchParams = serde_json::from_str(args)?;

        // Validate URL
        let url = url::Url::parse(&params.url)?;
        self.validate_url(&url)?;

        // Fetch page
        let response = self.client
            .get(url.as_str())
            .header("User-Agent", &self.config.user_agent)
            .timeout(Duration::from_secs(self.config.timeout_seconds))
            .send()
            .await?;

        if !response.status().is_success() {
            return Ok(ToolResult::error(format!(
                "HTTP {} for {}",
                response.status(),
                url
            )));
        }

        // Get content
        let html = response.text().await?;

        // Extract readable content
        let content = self.extract_content(&html, params.include_links.unwrap_or(false))?;

        // Truncate if needed
        let content = if content.len() > self.config.max_content_bytes {
            let truncated: String = content.chars()
                .take(self.config.max_content_bytes)
                .collect();
            format!("{}\n\n[Content truncated at {} bytes]", truncated, self.config.max_content_bytes)
        } else {
            content
        };

        Ok(ToolResult::success(content))
    }
}

impl WebFetchTool {
    fn extract_content(&self, html: &str, include_links: bool) -> Result<String> {
        let document = Html::parse_document(html);

        // Try to find main content
        let content_selectors = [
            "article",
            "main",
            "[role='main']",
            ".post-content",
            ".article-content",
            ".entry-content",
            "#content",
            "body",
        ];

        let mut content = String::new();

        for selector_str in content_selectors {
            if let Ok(selector) = Selector::parse(selector_str) {
                if let Some(element) = document.select(&selector).next() {
                    content = self.element_to_markdown(&element, include_links);
                    if !content.trim().is_empty() {
                        break;
                    }
                }
            }
        }

        // Extract title
        let title = document
            .select(&Selector::parse("title").unwrap())
            .next()
            .map(|e| e.inner_html())
            .unwrap_or_default();

        Ok(format!("# {}\n\n{}", title.trim(), content.trim()))
    }

    fn element_to_markdown(&self, element: &ElementRef, include_links: bool) -> String {
        // Recursive HTML to Markdown conversion
        // Handle: p, h1-h6, ul/ol/li, a, strong, em, blockquote, pre/code
        // Strip: script, style, nav, footer, aside, ads
        // ... implementation details
    }
}
```

### 2. UnifiedToolExecutor

**Location**: `Aether/core/src/core/tool_executor.rs`

```rust
use crate::capability::CapabilityExecutor;
use crate::error::Result;
use crate::mcp::McpClient;
use crate::payload::{AgentPayload, Capability};
use crate::tools::NativeToolRegistry;

/// Result from tool execution
#[derive(Debug, Clone)]
pub struct ToolExecutionResult {
    pub tool_name: String,
    pub success: bool,
    pub content: String,
    pub error: Option<String>,
    pub execution_time_ms: u64,
}

/// Unified executor that routes to the appropriate tool system
pub struct UnifiedToolExecutor {
    /// Builtin capability executor
    capability_executor: CapabilityExecutor,

    /// Native tool registry
    native_registry: Arc<NativeToolRegistry>,

    /// MCP client for MCP tools
    mcp_client: Option<Arc<McpClient>>,

    /// Tool name to source mapping (cached)
    tool_sources: Arc<RwLock<HashMap<String, ToolSource>>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ToolSource {
    Builtin,   // search, video, memory
    Native,    // AgentTool implementations
    Mcp,       // MCP server tools
    Skill,     // Claude Agent Skills (future)
}

impl UnifiedToolExecutor {
    pub fn new(
        capability_executor: CapabilityExecutor,
        native_registry: Arc<NativeToolRegistry>,
        mcp_client: Option<Arc<McpClient>>,
    ) -> Self {
        Self {
            capability_executor,
            native_registry,
            mcp_client,
            tool_sources: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Update tool source cache
    pub async fn refresh_tool_sources(&self) {
        let mut sources = self.tool_sources.write().await;
        sources.clear();

        // Builtins
        for name in ["search", "video", "youtube", "memory"] {
            sources.insert(name.to_string(), ToolSource::Builtin);
        }

        // Native tools
        for tool in self.native_registry.list() {
            sources.insert(tool.name.clone(), ToolSource::Native);
        }

        // MCP tools
        if let Some(ref client) = self.mcp_client {
            for tool in client.list_tools() {
                sources.insert(tool.name.clone(), ToolSource::Mcp);
            }
        }
    }

    /// Execute a tool by name
    pub async fn execute(
        &self,
        tool_name: &str,
        parameters: serde_json::Value,
        context: &CapturedContext,
        provider_name: &str,
    ) -> Result<ToolExecutionResult> {
        let start = Instant::now();

        // Determine tool source
        let source = self.resolve_source(tool_name).await;

        let result = match source {
            Some(ToolSource::Builtin) => {
                self.execute_builtin(tool_name, parameters, context, provider_name).await
            }
            Some(ToolSource::Native) => {
                self.execute_native(tool_name, parameters).await
            }
            Some(ToolSource::Mcp) => {
                self.execute_mcp(tool_name, parameters).await
            }
            Some(ToolSource::Skill) => {
                // Future: skill execution
                Err(AetherError::other("Skills not yet implemented"))
            }
            None => {
                Err(AetherError::tool_not_found(tool_name))
            }
        };

        let execution_time_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok(content) => Ok(ToolExecutionResult {
                tool_name: tool_name.to_string(),
                success: true,
                content,
                error: None,
                execution_time_ms,
            }),
            Err(e) => Ok(ToolExecutionResult {
                tool_name: tool_name.to_string(),
                success: false,
                content: String::new(),
                error: Some(e.to_string()),
                execution_time_ms,
            }),
        }
    }

    async fn resolve_source(&self, tool_name: &str) -> Option<ToolSource> {
        let sources = self.tool_sources.read().await;
        sources.get(tool_name).cloned()
    }

    async fn execute_builtin(
        &self,
        tool_name: &str,
        parameters: serde_json::Value,
        context: &CapturedContext,
        provider_name: &str,
    ) -> Result<String> {
        // Map to Capability enum
        let capability = match tool_name {
            "search" => Capability::Search,
            "video" | "youtube" => Capability::Video,
            "memory" => Capability::Memory,
            _ => return Err(AetherError::tool_not_found(tool_name)),
        };

        // Extract query from parameters
        let query = parameters
            .get("query")
            .or_else(|| parameters.get("url"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        // Build payload with capability
        let payload = self.build_capability_payload(
            query,
            context.clone(),
            provider_name,
            vec![capability],
        ).await?;

        // Extract result based on capability
        match capability {
            Capability::Search => {
                payload.context.search_results
                    .map(|r| format!("Search results:\n{}", serde_json::to_string_pretty(&r).unwrap_or_default()))
                    .ok_or_else(|| AetherError::other("Search returned no results"))
            }
            Capability::Video => {
                payload.context.video_transcript
                    .map(|t| format!("Video transcript:\n{}", t))
                    .ok_or_else(|| AetherError::other("Video transcript not available"))
            }
            Capability::Memory => {
                payload.context.memory_snippets
                    .map(|m| format!("Memory snippets:\n{}", serde_json::to_string_pretty(&m).unwrap_or_default()))
                    .ok_or_else(|| AetherError::other("No memory found"))
            }
            _ => Err(AetherError::other("Unsupported capability")),
        }
    }

    async fn execute_native(
        &self,
        tool_name: &str,
        parameters: serde_json::Value,
    ) -> Result<String> {
        let args = serde_json::to_string(&parameters)?;
        let result = self.native_registry.execute(tool_name, &args).await?;

        if result.success {
            Ok(result.output)
        } else {
            Err(AetherError::other(result.error.unwrap_or_else(|| "Tool execution failed".to_string())))
        }
    }

    async fn execute_mcp(
        &self,
        tool_name: &str,
        parameters: serde_json::Value,
    ) -> Result<String> {
        let client = self.mcp_client.as_ref()
            .ok_or_else(|| AetherError::other("MCP client not available"))?;

        let result = client.call_tool(tool_name, parameters).await?;

        if result.success {
            serde_json::to_string_pretty(&result.content)
                .map_err(|e| AetherError::other(format!("Failed to serialize MCP result: {}", e)))
        } else {
            Err(AetherError::other(result.error.unwrap_or_else(|| "MCP tool failed".to_string())))
        }
    }

    /// Get list of all available tools for prompt building
    pub async fn list_all_tools(&self) -> Vec<ToolInfo> {
        let mut tools = Vec::new();

        // Builtins
        tools.push(ToolInfo {
            name: "search".to_string(),
            description: "Search the web for information".to_string(),
            source: ToolSource::Builtin,
        });
        tools.push(ToolInfo {
            name: "video".to_string(),
            description: "Get transcript from YouTube videos".to_string(),
            source: ToolSource::Builtin,
        });

        // Native tools
        for def in self.native_registry.list() {
            tools.push(ToolInfo {
                name: def.name.clone(),
                description: def.description.clone(),
                source: ToolSource::Native,
            });
        }

        // MCP tools
        if let Some(ref client) = self.mcp_client {
            for tool in client.list_tools() {
                tools.push(ToolInfo {
                    name: tool.name.clone(),
                    description: tool.description.clone(),
                    source: ToolSource::Mcp,
                });
            }
        }

        tools
    }
}
```

### 3. Updated Processing Flow

**Location**: `Aether/core/src/core/processing.rs`

```rust
// Replace execute_matched_tool with:

fn execute_matched_tool(
    &self,
    tool_name: String,
    parameters: serde_json::Value,
    input: String,
    context: CapturedContext,
    start_time: std::time::Instant,
) -> Result<String> {
    info!(
        tool = %tool_name,
        "Executing matched tool via UnifiedToolExecutor"
    );

    // Get provider name for capability execution
    let provider_name = self.get_default_provider_instance()
        .map(|p| p.name().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    // Execute via unified executor
    let result = self.runtime.block_on(
        self.tool_executor.execute(&tool_name, parameters, &context, &provider_name)
    )?;

    if !result.success {
        return Err(AetherError::other(
            result.error.unwrap_or_else(|| "Tool execution failed".to_string())
        ));
    }

    info!(
        tool = %tool_name,
        execution_time_ms = result.execution_time_ms,
        "Tool executed successfully"
    );

    // Now make AI call with tool result
    let provider = self.get_default_provider_instance()
        .ok_or_else(|| AetherError::NoProviderAvailable {
            suggestion: Some("No AI provider available".to_string()),
        })?;

    // Build system prompt with tool result
    let system_prompt = format!(
        "You are a helpful AI assistant. The user requested help and you executed the '{}' tool. \
         Here is the tool output:\n\n<tool_result>\n{}\n</tool_result>\n\n\
         Please synthesize this information to answer the user's request.",
        tool_name,
        result.content
    );

    // Make AI call
    let attachments = context.attachments.as_ref().map(|a| a.as_slice());
    let response = self.runtime.block_on(
        provider.process_with_attachments(&input, attachments, Some(&system_prompt))
    )?;

    info!(
        response_length = response.len(),
        elapsed_ms = start_time.elapsed().as_millis(),
        "Tool execution + AI synthesis completed"
    );

    // Store in memory
    if self.memory_db.is_some() {
        self.store_interaction_async(input, response.clone());
    }

    self.event_handler.on_state_changed(ProcessingState::Success);

    Ok(response)
}
```

### 4. L3 Prompt Enhancement

**Location**: `Aether/core/src/routing/l3_enhanced.rs`

```rust
fn build_routing_prompt(&self, input: &str, tools: &[UnifiedTool]) -> String {
    // Build comprehensive tool list
    let tool_descriptions: Vec<String> = tools.iter().map(|t| {
        format!(
            "- **{}** ({}): {}",
            t.name,
            match t.source {
                ToolSource::Builtin => "builtin",
                ToolSource::Native => "native",
                ToolSource::Mcp => "mcp",
                ToolSource::Skill => "skill",
            },
            t.description
        )
    }).collect();

    format!(r#"You are an AI assistant that helps route user requests to the appropriate tool.

## Available Tools

{tools}

## Instructions

1. Analyze the user's request
2. If a tool can help, respond with JSON:
   ```json
   {{"tool": "<tool_name>", "parameters": {{...}}, "reasoning": "..."}}
   ```
3. If no tool is needed, respond normally

## User Request

{input}
"#,
        tools = tool_descriptions.join("\n"),
        input = input
    )
}
```

## Data Flow

```
1. User: "总结这个网页 https://bbc.com/article"

2. Pipeline.process(input)
   ├── L1 Regex: No match (no /fetch prefix)
   ├── L2 Semantic: Matches "summarize" + URL pattern → tool=web_fetch, confidence=0.7
   └── L3 AI (if L2 < threshold): Confirms web_fetch with params={url: "..."}

3. PipelineResult::ToolMatched { tool_name: "web_fetch", parameters: {url: "..."} }

4. AetherCore.execute_matched_tool("web_fetch", params)
   ├── UnifiedToolExecutor.execute("web_fetch", params)
   │   ├── resolve_source("web_fetch") → Native
   │   └── native_registry.execute("web_fetch", args)
   │       └── WebFetchTool.execute(args)
   │           ├── HTTP GET https://bbc.com/article
   │           ├── HTML → Markdown conversion
   │           └── Return content
   └── ToolExecutionResult { success: true, content: "# Article Title\n..." }

5. AI Provider synthesizes response with tool result
   └── "这是BBC新闻文章的摘要：..."

6. Return to user
```

## Configuration

**config.toml additions:**

```toml
[tools]
enabled = true

[tools.web_fetch]
enabled = true
max_content_bytes = 51200  # 50KB
timeout_seconds = 30
user_agent = "AetherBot/1.0"
# Optional: URL restrictions
allowed_patterns = []
blocked_domains = ["localhost", "127.0.0.1", "0.0.0.0"]
```

## Testing Strategy

### Unit Tests

```rust
#[tokio::test]
async fn test_web_fetch_tool_basic() {
    let tool = WebFetchTool::new(WebFetchConfig::default());
    let result = tool.execute(r#"{"url": "https://example.com"}"#).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_unified_executor_routing() {
    let executor = create_test_executor();

    // Test builtin routing
    let result = executor.execute("search", json!({"query": "test"}), &ctx, "test").await;
    assert!(result.is_ok());

    // Test native routing
    let result = executor.execute("web_fetch", json!({"url": "https://example.com"}), &ctx, "test").await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_tool_source_resolution() {
    let executor = create_test_executor();
    executor.refresh_tool_sources().await;

    assert_eq!(executor.resolve_source("search").await, Some(ToolSource::Builtin));
    assert_eq!(executor.resolve_source("web_fetch").await, Some(ToolSource::Native));
}
```

### Integration Tests

```rust
#[tokio::test]
async fn test_summarize_webpage_flow() {
    let core = create_test_core();

    let input = "总结这个网页 https://example.com";
    let context = CapturedContext::default();

    let result = core.process_input(input.to_string(), context);
    assert!(result.is_ok());

    let response = result.unwrap();
    // Response should contain summary content
    assert!(response.contains("Example Domain") || response.len() > 100);
}
```

## Migration Notes

1. **Backward Compatibility**: Existing Capability-based flow remains as fallback
2. **Gradual Rollout**: Feature flag `tools.enabled` controls new executor
3. **No Breaking Changes**: Current API unchanged, new functionality is additive
