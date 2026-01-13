# Proposal: Enable Intelligent Tool Invocation

## Change ID
`enable-intelligent-tool-invocation`

## Status
Draft

## Problem Statement

用户反馈：当输入 "总结这个网页内容：https://www.bbc.co.uk/news/articles/ce9yv73klx5o" 时，AI 回复表示无法访问网页内容。这暴露了系统的一个核心问题：**意图检测与工具执行之间存在严重脱节**。

### Root Cause Analysis

经过深入分析，问题链如下：

1. **工具缺失**：系统没有 `web_fetch` 工具来获取和解析网页内容
2. **能力映射硬编码**：`execute_matched_tool()` 只支持 3 个硬编码的 Capability（Search, Video, Memory）
3. **动态工具无法执行**：即使 L3 AI 推理出需要 `web_fetch`，由于不在 Capability enum 中，会直接 fallback 到 GeneralChat
4. **Native Tools 未接入执行流**：NativeToolRegistry 中注册的工具（filesystem, git, shell, search 等）无法在意图路由后被实际执行

### 当前架构流程

```
User Input → Pipeline → L1/L2/L3 Routing → ToolMatched
                                              ↓
                               execute_matched_tool()
                                              ↓
                              match tool_name {
                                "youtube" => Capability::Video,
                                "search" => Capability::Search,
                                "memory" => Capability::Memory,
                                _ => ERROR: "Unknown tool, fallback to AI-first"  ← 断点
                              }
```

## Proposed Solution

实现类似 Claude Code 的智能工具调用架构，让 AI 能够根据用户需求动态选择并执行工具。

### 核心设计：Unified Tool Execution Layer

```
User Input
     ↓
┌─────────────────────────────────────────────────────┐
│              IntentRoutingPipeline                   │
│  (L1 Regex → L2 Semantic → L3 AI Inference)         │
└────────────────────┬────────────────────────────────┘
                     ↓
              ToolMatched { tool_name, params }
                     ↓
┌─────────────────────────────────────────────────────┐
│        Unified Tool Executor (新增)                  │
│  ┌─────────────────────────────────────────────┐    │
│  │ Tool Resolution:                             │    │
│  │  - Builtin (search, video, memory) → 内置执行 │    │
│  │  - Native (AgentTool) → NativeToolRegistry   │    │
│  │  - MCP → McpClient                          │    │
│  │  - Skill → SkillExecutor                    │    │
│  └─────────────────────────────────────────────┘    │
└────────────────────┬────────────────────────────────┘
                     ↓
              Tool Result
                     ↓
              AI Synthesizes Response
```

### 关键改进

#### 1. 新增 WebFetch Native Tool

```rust
// tools/web/mod.rs
pub struct WebFetchTool {
    client: reqwest::Client,
    max_content_length: usize,
}

impl AgentTool for WebFetchTool {
    fn name(&self) -> &str { "web_fetch" }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "web_fetch",
            "Fetch and extract content from a URL",
            json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "URL to fetch" }
                },
                "required": ["url"]
            }),
            ToolCategory::Web,
        )
    }

    async fn execute(&self, args: &str) -> Result<ToolResult> {
        // 1. Parse URL from args
        // 2. HTTP GET with timeout
        // 3. HTML to markdown conversion
        // 4. Return content (truncated if too long)
    }
}
```

#### 2. Unified Tool Executor

```rust
// core/tool_executor.rs
pub struct UnifiedToolExecutor {
    // Builtin capabilities (existing)
    capability_executor: CapabilityExecutor,

    // Native tools (AgentTool implementations)
    native_registry: Arc<NativeToolRegistry>,

    // MCP client
    mcp_client: Option<Arc<McpClient>>,

    // Skill executor (future)
    skill_executor: Option<Arc<SkillExecutor>>,
}

impl UnifiedToolExecutor {
    pub async fn execute(
        &self,
        tool_name: &str,
        parameters: serde_json::Value,
        context: &CapturedContext,
    ) -> Result<ToolExecutionResult> {
        // 1. Try builtin capabilities first (backward compatible)
        if let Some(capability) = self.resolve_builtin(tool_name) {
            return self.execute_builtin(capability, parameters, context).await;
        }

        // 2. Try native tools
        if let Some(result) = self.native_registry.execute(tool_name, &parameters).await? {
            return Ok(result);
        }

        // 3. Try MCP tools
        if let Some(ref client) = self.mcp_client {
            if client.has_tool(tool_name) {
                return self.execute_mcp(client, tool_name, parameters).await;
            }
        }

        // 4. Fallback to general chat (no tool found)
        Err(AetherError::tool_not_found(tool_name))
    }
}
```

#### 3. 更新 L3 Prompt Builder

确保 L3 AI 能看到所有可用工具：

```rust
// dispatcher/prompt_builder.rs
fn build_tool_list(&self) -> String {
    let mut tools = Vec::new();

    // Builtin commands
    for cmd in BUILTIN_COMMANDS {
        tools.push(format!("- {} ({}): {}", cmd.name, cmd.category, cmd.description));
    }

    // Native tools from registry
    for tool in self.native_registry.list() {
        tools.push(format!("- {} (native): {}", tool.name, tool.description));
    }

    // MCP tools
    for tool in self.mcp_tools {
        tools.push(format!("- {} (mcp): {}", tool.name, tool.description));
    }

    tools.join("\n")
}
```

## Scope

### In Scope
- 实现 `WebFetchTool` 用于网页内容获取
- 实现 `UnifiedToolExecutor` 统一工具执行层
- 更新 `execute_matched_tool()` 使用新的执行器
- 更新 L3 prompt 包含所有可用工具
- 确保现有 Search/Video/Memory 能力继续工作

### Out of Scope
- Skill 执行器（已有独立提案）
- MCP 工具发现/安装 UI
- 工具权限系统

## Success Criteria

1. 用户输入 "总结这个网页：https://example.com" 时，系统能够：
   - L3 识别意图需要 web_fetch
   - 执行 WebFetchTool 获取网页内容
   - AI 基于内容生成摘要

2. Native tools (file_read, git_status 等) 可通过自然语言触发

3. 现有功能无回退

## Risks & Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| WebFetch 被滥用 | 安全 | 添加 URL 白名单配置，速率限制 |
| 网页内容过大 | 性能 | 限制内容长度（默认 50KB），支持分页 |
| HTML 解析不完整 | 体验 | 使用成熟库（readability, scraper） |
| 破坏现有 Capability 流程 | 稳定性 | UnifiedToolExecutor 优先使用 builtin |

## Dependencies

- `reqwest` - HTTP 客户端（已有）
- `scraper` 或 `html2text` - HTML 解析（新增）
- 现有 NativeToolRegistry 基础设施

## References

- [DISPATCHER.md](../../../docs/DISPATCHER.md) - 当前调度器架构
- [ARCHITECTURE.md](../../../docs/ARCHITECTURE.md) - 系统架构
- Claude Code 的工具调用模式
