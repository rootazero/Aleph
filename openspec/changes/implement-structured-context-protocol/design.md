# Design Document: Structured Context Protocol

## Overview

本文档详细描述 "Dynamic Context Payload (DCP)" 架构的技术设计,包括数据流、类型定义、执行流程和扩展策略。

## Architecture Diagram

```
┌─────────────────────────────────────────────────────────────────┐
│                        User Input                                │
│                     "翻译为英文:你好世界"                        │
└──────────────────────┬──────────────────────────────────────────┘
                       │
                       ▼
┌─────────────────────────────────────────────────────────────────┐
│                  Router (Entry Point)                            │
│  1. Match routing rule                                           │
│  2. Extract Intent + Capabilities                                │
│  3. Build AgentPayload                                           │
└──────────────────────┬──────────────────────────────────────────┘
                       │
                       ▼
┌─────────────────────────────────────────────────────────────────┐
│              AgentPayload (Core Data Structure)                  │
│  ┌────────────┬──────────────┬──────────────┬────────────────┐  │
│  │    Meta    │   Config     │   Context    │  User Input    │  │
│  │            │              │              │                │  │
│  │ - Intent   │ - Provider   │ - Memory     │  "你好世界"    │  │
│  │ - Timestamp│ - Temp       │ - Search     │                │  │
│  │ - Anchor   │ - Capabilities│ - MCP       │                │  │
│  └────────────┴──────────────┴──────────────┴────────────────┘  │
└──────────────────────┬──────────────────────────────────────────┘
                       │
                       ▼
┌─────────────────────────────────────────────────────────────────┐
│            Capability Execution Layer                            │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐                      │
│  │ Memory   │→ │ Search   │→ │   MCP    │  (Fixed Order)       │
│  │          │  │          │  │          │                      │
│  │ Retrieve │  │ Query    │  │ Tool     │                      │
│  │ Similar  │  │ Google   │  │ Call     │                      │
│  │ Memories │  │          │  │          │                      │
│  └────┬─────┘  └────┬─────┘  └────┬─────┘                      │
│       │             │             │                             │
│       └─────────────┼─────────────┘                             │
│                     │ Fill context                              │
│                     ▼                                            │
│       payload.context.memory_snippets = [...]                   │
│       payload.context.search_results = [...]                    │
│       payload.context.mcp_resources = {...}                     │
└──────────────────────┬──────────────────────────────────────────┘
                       │
                       ▼
┌─────────────────────────────────────────────────────────────────┐
│              Prompt Assembly Layer                               │
│  PromptAssembler::assemble_system_prompt()                      │
│  ┌────────────────────────────────────────────────────────────┐ │
│  │ Base Prompt: "You are a translator."                       │ │
│  │                                                             │ │
│  │ + ### Context Information                                  │ │
│  │   **Relevant History**:                                    │ │
│  │   1. Conversation at 2024-01-01 10:00:00                   │ │
│  │      App: com.apple.Notes                                  │ │
│  │      User: 翻译为中文:Hello                                │ │
│  │      AI: 你好                                               │ │
│  │      Relevance: 95%                                        │ │
│  └────────────────────────────────────────────────────────────┘ │
└──────────────────────┬──────────────────────────────────────────┘
                       │
                       ▼
┌─────────────────────────────────────────────────────────────────┐
│                   AI Provider Call                               │
│  messages = [                                                    │
│    { role: "system", content: assembled_prompt },               │
│    { role: "user", content: "你好世界" }                         │
│  ]                                                               │
└──────────────────────────────────────────────────────────────────┘
```

## Data Structures

### Core Types

#### AgentPayload

```rust
pub struct AgentPayload {
    pub meta: PayloadMeta,
    pub config: PayloadConfig,
    pub context: AgentContext,
    pub user_input: String,
}
```

**Purpose**: 统一的内部数据流转格式,贯穿整个 Agent 处理流程。

**Lifetime**: 从 Router::route() 创建,到 Provider 调用结束销毁。

**Thread Safety**: 不需要,单个请求在单个 tokio task 中处理。

#### Intent

```rust
pub enum Intent {
    BuiltinSearch,      // 硬编码功能:搜索
    BuiltinMcp,         // 硬编码功能:MCP 工具调用
    Skills(String),     // 🔮 复杂工作流(预留)
    Custom(String),     // 用户自定义 Prompt 转换
    GeneralChat,        // 默认对话
}
```

**Design Rationale**:

- **BuiltinXXX**: 需要 Rust 代码实现的功能,无法通过配置文件定义
- **Custom**: 用户可在配置文件中添加的 Prompt 模板
- **Skills**: 未来支持复杂的多步骤工作流(Claude Code Skills)

**Mapping from Config**:

```toml
intent_type = "search"         → Intent::BuiltinSearch
intent_type = "translation"    → Intent::Custom("translation")
intent_type = "skills:pdf"     → Intent::Skills("pdf")
(no intent_type)               → Intent::GeneralChat
```

#### Capability

```rust
pub enum Capability {
    Memory = 0,  // 优先级最高
    Search = 1,
    Mcp = 2,     // 优先级最低
}
```

**Execution Order**: 通过 `PartialOrd` trait 实现固定顺序。

**Rationale**:
- Memory 最先执行:为后续 Capability 提供对话历史上下文
- Search 次之:可能基于 Memory 上下文判断是否需要搜索
- MCP 最后:可能需要前面的结果作为输入

**Config Syntax**:

```toml
capabilities = ["memory", "search"]  # 字符串数组
```

Parser 将字符串转换为 `Vec<Capability>`,然后自动排序。

### Context Data

#### AgentContext

```rust
pub struct AgentContext {
    pub memory_snippets: Option<Vec<MemoryEntry>>,
    pub search_results: Option<Vec<SearchResult>>,
    pub mcp_resources: Option<HashMap<String, serde_json::Value>>,
    pub workflow_state: Option<WorkflowState>,  // 🔮 Skills
}
```

**Design Principles**:

1. **Optional by Default**: 没有数据时为 `None`,避免空数组/空哈希表占用内存
2. **Strongly Typed**: 每个字段有明确的类型定义,不使用 `serde_json::Value`
3. **Extensible**: 新增上下文源只需添加新字段,不影响现有代码

#### MemoryEntry

```rust
pub struct MemoryEntry {
    pub id: String,
    pub context: ContextAnchor,
    pub user_input: String,
    pub ai_output: String,
    pub embedding: Option<Vec<f32>>,
    pub similarity_score: Option<f32>,
}
```

**Purpose**: 向量检索返回的记忆条目。

**Key Fields**:
- `context`: 记录对话发生时的应用上下文(App, Window, Time)
- `similarity_score`: 与当前查询的相似度(0.0-1.0)

## Execution Flow

### Router::route() Implementation

```rust
pub fn route(&self, input: &str, captured_context: &CapturedContext)
    -> Result<(Arc<dyn AiProvider>, AgentPayload)>
{
    // 1. Match routing rule
    let decision = self.make_decision(input)?;

    // 2. Build AgentPayload
    let anchor = ContextAnchor::from_captured_context(captured_context);
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64;

    let mut payload = PayloadBuilder::new()
        .meta(decision.intent, timestamp, anchor)
        .config(
            decision.provider_name.clone(),
            decision.capabilities.clone(),
            decision.context_format,
        )
        .user_input(decision.processed_input.clone())
        .build()?;

    // 3. Execute capabilities
    self.execute_capabilities(&mut payload)?;

    // 4. Get provider
    let provider = self.providers.get(&decision.provider_name)
        .ok_or_else(|| AetherError::ProviderNotFound(decision.provider_name.clone()))?;

    Ok((provider.clone(), payload))
}
```

### Capability Execution

```rust
fn execute_capabilities(&self, payload: &mut AgentPayload) -> Result<()> {
    // Sort capabilities by priority (Memory → Search → MCP)
    let mut caps = payload.config.capabilities.clone();
    caps.sort();  // Uses PartialOrd

    for cap in caps {
        match cap {
            Capability::Memory => self.execute_memory_capability(payload)?,
            Capability::Search => self.execute_search_capability(payload)?,
            Capability::Mcp => self.execute_mcp_capability(payload)?,
        }
    }

    Ok(())
}
```

**Error Handling Strategy**:

- **Memory 失败**: 记录 warning,继续执行(不阻塞请求)
- **Search 失败**: 记录 warning,继续执行
- **MCP 失败**: 记录 error,可选择中止或继续(根据配置)

### Memory Capability

```rust
fn execute_memory_capability(&self, payload: &mut AgentPayload) -> Result<()> {
    // Get memory config
    let memory_config = &self.config.memory;
    if !memory_config.enabled {
        return Ok(());
    }

    // Retrieve similar memories
    let memories = self.memory_store.search_similar(
        &payload.user_input,
        memory_config.max_context_items,
        memory_config.similarity_threshold,
    )?;

    // Fill context
    if !memories.is_empty() {
        debug!(
            "Retrieved {} memories, avg similarity: {:.2}",
            memories.len(),
            avg_similarity(&memories)
        );
        payload.context.memory_snippets = Some(memories);
    }

    Ok(())
}
```

**Performance Optimization**:

- 使用 HNSW 索引(LanceDB 或 sqlite-vec)
- 批量检索(一次 SQL 查询返回 top-k)
- 延迟加载:只在需要时才初始化 embedding model

### Prompt Assembly

```rust
// In Router, before calling Provider:
let base_prompt = decision.system_prompt
    .unwrap_or_else(|| provider.default_system_prompt());

let assembler = PromptAssembler::new(payload.config.context_format);
let final_prompt = assembler.assemble_system_prompt(&base_prompt, &payload);

// Call provider with final_prompt
provider.generate(final_prompt, payload.user_input).await?;
```

**Context Formatting Logic**:

```rust
impl PromptAssembler {
    fn format_markdown(&self, context: &AgentContext) -> Option<String> {
        let mut sections = Vec::new();

        // Memory section
        if let Some(memories) = &context.memory_snippets {
            sections.push(format!(
                "**Relevant History**:\n{}",
                self.format_memory_entries(memories)
            ));
        }

        // Search section (future)
        if let Some(results) = &context.search_results {
            sections.push(format!(
                "**Search Results**:\n{}",
                self.format_search_results(results)
            ));
        }

        if sections.is_empty() {
            None
        } else {
            Some(format!("### Context Information\n\n{}", sections.join("\n\n")))
        }
    }
}
```

**Format Examples**:

**Markdown** (MVP):
```
### Context Information

**Relevant History**:

1. **Conversation at 2024-01-01 10:00:00 UTC**
   App: com.apple.Notes
   Window: Translation.txt
   User: 翻译为英文:你好
   AI: Hello
   Relevance: 98%
```

**XML** (Future):
```xml
<context>
  <memory>
    <entry timestamp="1704096000" similarity="0.98">
      <app>com.apple.Notes</app>
      <window>Translation.txt</window>
      <user>翻译为英文:你好</user>
      <ai>Hello</ai>
    </entry>
  </memory>
</context>
```

**JSON** (Future):
```json
{
  "context": {
    "memory": [
      {
        "timestamp": 1704096000,
        "app": "com.apple.Notes",
        "user": "翻译为英文:你好",
        "ai": "Hello",
        "similarity": 0.98
      }
    ]
  }
}
```

## Interface Reservations

### Search Interface

```rust
// In payload/mod.rs
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub timestamp: Option<i64>,
    pub relevance_score: Option<f32>,
    pub source_type: Option<String>,  // web/news/academic/image
}
```

**Future Implementation Plan**:

1. Implement `SearchEngine` trait:
   ```rust
   trait SearchEngine {
       async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>>;
   }
   ```

2. Add implementations:
   - `GoogleSearchEngine` (使用 Google Custom Search API)
   - `BingSearchEngine` (使用 Bing Search API)
   - `LocalSearchEngine` (本地文档索引)

3. Call in `execute_search_capability()`:
   ```rust
   let results = self.search_engine.search(&payload.user_input, 5).await?;
   payload.context.search_results = Some(results);
   ```

### MCP Interface

```rust
// In payload/mod.rs
pub struct AgentContext {
    pub mcp_resources: Option<HashMap<String, serde_json::Value>>,
}
```

**Future Implementation Plan**:

1. Implement MCP Client SDK:
   ```rust
   struct McpClient {
       server_url: String,
   }

   impl McpClient {
       async fn list_tools(&self) -> Result<Vec<ToolDefinition>>;
       async fn call_tool(&self, name: &str, args: serde_json::Value) -> Result<serde_json::Value>;
       async fn read_resource(&self, uri: &str) -> Result<String>;
   }
   ```

2. Call in `execute_mcp_capability()`:
   ```rust
   let mut resources = HashMap::new();

   // Example: Read file system resource
   if let Some(mcp_client) = &self.mcp_client {
       let file_content = mcp_client.read_resource("file:///workspace/README.md").await?;
       resources.insert("readme".to_string(), json!(file_content));
   }

   payload.context.mcp_resources = Some(resources);
   ```

### Skills Interface

```rust
// In payload/mod.rs
pub struct WorkflowState {
    pub workflow_id: String,
    pub current_step: usize,
    pub total_steps: usize,
    pub step_results: Vec<serde_json::Value>,
    pub status: WorkflowStatus,
}

pub enum WorkflowStatus {
    Pending,
    Running,
    WaitingForConfirmation,
    Completed,
    Failed,
}
```

**Future Implementation Plan** (Complex):

1. Define Workflow DSL:
   ```yaml
   # skills/build-macos-apps.yaml
   name: build-macos-apps
   steps:
     - id: analyze_requirements
       tool: llm_call
       prompt: "Analyze user requirements..."

     - id: scaffold_project
       tool: mcp_file_system
       action: create_directory
       params:
         path: "./MyApp"

     - id: generate_code
       tool: llm_call
       depends_on: [analyze_requirements]
       prompt: "Generate Swift code..."
   ```

2. Implement Workflow Engine:
   ```rust
   struct WorkflowEngine {
       skills_registry: SkillsRegistry,
       mcp_client: McpClient,
   }

   impl WorkflowEngine {
       async fn execute(&self, skill_id: &str, input: &str) -> Result<WorkflowState>;
   }
   ```

3. Call in Router:
   ```rust
   if payload.meta.intent.is_skills() {
       let workflow = self.workflow_engine.execute(skill_id, &payload.user_input).await?;
       payload.context.workflow_state = Some(workflow);
   }
   ```

## Configuration Schema

### Routing Rule Example

```toml
[[rules]]
# Basic fields (existing)
regex = "^/research"
provider = "claude"
system_prompt = "You are a research assistant."
strip_prefix = true

# New fields
intent_type = "custom:research"  # Determines Intent
capabilities = ["memory", "search"]  # Capabilities to execute
context_format = "markdown"  # Prompt format (markdown/xml/json)
```

### Memory Config Example

```toml
[memory]
enabled = true
embedding_model = "all-MiniLM-L6-v2"
vector_db = "sqlite-vec"
retention_days = 90

# New fields
max_context_items = 5  # Max memory entries to retrieve
similarity_threshold = 0.7  # Minimum similarity score (0.0-1.0)
```

## Extension Points

### Adding New Capabilities

1. Add enum variant:
   ```rust
   pub enum Capability {
       Memory = 0,
       Search = 1,
       Mcp = 2,
       VisionAnalysis = 3,  // NEW
   }
   ```

2. Add context field:
   ```rust
   pub struct AgentContext {
       pub vision_results: Option<Vec<VisionResult>>,  // NEW
   }
   ```

3. Implement executor:
   ```rust
   fn execute_vision_capability(&self, payload: &mut AgentPayload) -> Result<()> {
       // Implementation
   }
   ```

4. Add to executor dispatch:
   ```rust
   Capability::VisionAnalysis => self.execute_vision_capability(payload)?,
   ```

### Adding New Context Formats

1. Add enum variant:
   ```rust
   pub enum ContextFormat {
       Markdown,
       Xml,
       Json,
       Yaml,  // NEW
   }
   ```

2. Implement formatter:
   ```rust
   impl PromptAssembler {
       fn format_yaml(&self, context: &AgentContext) -> Option<String> {
           // Implementation
       }
   }
   ```

3. Add to format dispatch:
   ```rust
   ContextFormat::Yaml => self.format_yaml(context),
   ```

## Performance Considerations

### Payload Construction

**Overhead**: ~2-5ms
- Struct allocation: < 1ms
- String cloning: 1-2ms
- Vec allocation: < 1ms

**Optimization**: 使用 `Arc<str>` 替代 `String` for shared data (future)

### Memory Retrieval

**Target**: < 50ms
- Embedding inference: 20-30ms (cached model)
- Vector search: 10-20ms (HNSW index)
- SQL query: 5-10ms

**Optimization**:
- Lazy load embedding model
- Use connection pool for DB
- Cache frequent queries

### Prompt Assembly

**Overhead**: ~1-3ms
- String formatting: 1-2ms
- Markdown rendering: < 1ms

**Optimization**: Pre-allocate string buffer with estimated capacity

### Total Added Latency

**Target**: < 20ms (excluding Memory retrieval)
**Actual** (estimated):
- Payload build: 5ms
- Capability dispatch: 2ms
- Prompt assembly: 3ms
- **Total**: ~10ms ✅

## Error Handling

### Capability Execution Errors

**Strategy**: Continue on Error

```rust
for cap in caps {
    if let Err(e) = execute_capability(cap, payload) {
        warn!("Capability {:?} failed: {}, continuing...", cap, e);
        // Don't propagate error
    }
}
```

**Rationale**: 单个 Capability 失败不应阻塞整个请求。例如:Memory 检索失败时,仍然可以发送请求给 LLM,只是没有历史上下文。

### Memory Retrieval Errors

**Types**:
- DB connection error → Log error, return empty vec
- Embedding inference error → Log error, return empty vec
- Invalid data error → Skip invalid entries, return valid ones

**Fallback**: 总是返回 `Ok(vec![])`,确保流程继续。

### Config Parsing Errors

**Types**:
- Invalid capability name → Log warning, skip
- Invalid context_format → Use default (Markdown)
- Invalid intent_type → Use GeneralChat

**Principle**: 尽量兼容,不因配置错误而拒绝服务。

## Security Considerations

### PII in Memory

**Risk**: Memory 中可能包含敏感信息(如手机号、邮箱)

**Mitigation** (Future):
1. 在存储前 scrub PII (使用现有的 PII scrubber)
2. 在检索时二次过滤
3. 提供用户删除 Memory 的接口

### Context Injection Attacks

**Risk**: 恶意构造的 Memory 内容可能影响 LLM 行为

**Mitigation**:
1. Markdown escape:转义特殊字符
2. Length limit:截断过长的内容
3. Sanitization:移除可疑的 Prompt injection 模式

### Config Injection

**Risk**: 用户修改配置文件注入恶意 System Prompt

**Mitigation**:
- 配置文件在本地,用户对自己的配置负责
- 不提供远程配置下载功能
- 记录所有 System Prompt 更改到日志

## Testing Strategy

### Unit Tests

- Payload construction
- Capability sorting
- Intent parsing
- Config parsing
- Prompt formatting

### Integration Tests

- Router → Payload → Capability → Assembly → Provider (mock)
- Memory retrieval with real database
- Config loading with new fields

### Property-Based Tests (Future)

- Fuzz testing for Config parser
- Property: `parse(serialize(config)) == config`

### Performance Tests

- Benchmark Payload construction
- Benchmark Memory retrieval (10k entries)
- Benchmark Prompt assembly (various context sizes)

## Rollback Plan

如果发现严重问题,回滚策略:

1. **Feature Flag**: 添加配置项 `enable_structured_context = false`
2. **Fallback Path**: Router 检测到 flag 为 false 时,使用旧的字符串拼接逻辑
3. **Data Compatibility**: Payload 数据结构向后兼容,可以从旧格式转换

## Future Enhancements

1. **Caching Layer**: 缓存 Payload 构建结果(基于 input hash)
2. **Streaming Context**: 支持增量更新 Context(用于长对话)
3. **Context Compression**: 自动总结过长的 Memory 上下文
4. **Multi-modal Context**: 支持图片、文件等上下文类型
5. **Context Prioritization**: 根据相关性动态调整 Context 顺序

## References

- [agentstructure.md](../../../agentstructure.md) - 原始架构设计
- Memory Module Design - `Aether/core/src/memory/mod.rs`
- Router Implementation - `Aether/core/src/router/mod.rs`
- Config Schema - `config.toml.example`
