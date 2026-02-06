# Tool-as-Resource: 动态工具发现系统

> **状态**: Production Ready (Phase 1-3 完成，Phase 4 部分完成)
> **版本**: 1.0.0
> **最后更新**: 2026-02-06

## 概述

Tool-as-Resource 是 Aleph 的动态工具发现系统，通过语义向量检索实现按需工具加载，解决了"规模-上下文"悖论。

### 核心特性

- ✅ **语义检索**: 基于用户意图的向量匹配，而非关键词匹配
- ✅ **按需加载**: 只将相关工具的完整 Schema 注入上下文
- ✅ **零感延迟**: 检索延迟隐藏在 LLM 推理之前
- ✅ **自我进化**: L2 异步 LLM 优化工具描述质量
- ✅ **事件驱动**: 自动同步 MCP/Skill 工具变更

## 架构

### 三层推理策略

| Level | 来源 | 延迟 | 质量 | Confidence |
|-------|------|------|------|------------|
| **L0** | `structured_meta` | 0ms | 最高 | 0.95 |
| **L1** | 规则引擎模板 | <1ms | 中等 | 0.5-0.85 |
| **L2** | 异步 LLM 补全 | 后台 | 高 | 0.9 |

### 双阈值水合机制

```
Query → Embedding → Vector Search → Threshold Classification
                                    ↓
                    ┌───────────────┼───────────────┐
                    ↓               ↓               ↓
              score >= 0.7    0.6 <= score < 0.7   0.4 <= score < 0.6
                    ↓               ↓               ↓
              Full Schema     Summary Only      Indexed Only
```

## 核心组件

### 1. SemanticPurposeInferrer

生成工具的语义描述，支持三层推理策略。

```rust
use alephcore::dispatcher::tool_index::SemanticPurposeInferrer;

// L0/L1 only
let inferrer = SemanticPurposeInferrer::new();

// With L2 support
let inferrer = SemanticPurposeInferrer::with_llm(llm_provider);

// Infer semantic purpose
let result = inferrer.infer(
    "read_file",
    Some("Read file contents"),
    Some("file"),
    None, // structured_meta
);

// Check if L2 should be triggered
if inferrer.should_trigger_l2(&result) {
    // Schedule async L2 optimization
    let l2_result = inferrer.enhance_with_llm(
        "tool:read_file",
        "read_file",
        Some("Read file contents"),
        Some("file"),
    ).await?;
}
```

### 2. ToolIndexCoordinator

同步工具到 Memory 系统，管理工具索引生命周期。

```rust
use alephcore::dispatcher::tool_index::ToolIndexCoordinator;
use std::sync::Arc;

// Create coordinator
let coordinator = ToolIndexCoordinator::new(db);

// Or with L2 support
let coordinator = ToolIndexCoordinator::with_llm(db, llm_provider);

// Sync a single tool
coordinator.sync_tool(
    "read_file",
    Some("Read file contents"),
    Some("file"),
    None, // structured_meta
    None, // embedding
).await?;

// Sync multiple tools
let tools = vec![
    ToolMeta::new("read_file").with_category("file"),
    ToolMeta::new("write_file").with_category("file"),
];
coordinator.sync_all(tools).await?;

// Remove a tool
coordinator.remove_tool("read_file").await?;
```

### 3. ToolRetrieval

语义检索工具，支持双阈值分类。

```rust
use alephcore::dispatcher::tool_index::{ToolRetrieval, ToolRetrievalConfig};

// Create retrieval service
let config = ToolRetrievalConfig {
    hard_threshold: 0.4,
    soft_threshold: 0.6,
    high_confidence: 0.7,
    top_k: 5,
    ..Default::default()
};

let retrieval = ToolRetrieval::new(db, registry, config);

// Retrieve tools for a query
let result = retrieval.retrieve("read a configuration file").await?;

// Access results
for tool in result.full_schema_tools {
    println!("High confidence: {} (score: {})", tool.name, tool.score);
}

for tool in result.summary_tools {
    println!("Medium confidence: {} (score: {})", tool.name, tool.score);
}
```

### 4. HydrationPipeline

集成到 Agent Loop，提供端到端的工具水合。

```rust
use alephcore::dispatcher::tool_index::{HydrationPipeline, HydrationPipelineConfig};

// Create pipeline
let config = HydrationPipelineConfig::default()
    .with_max_full_schema(5)
    .with_max_summary(3)
    .with_core_tools(vec!["file_ops".to_string(), "bash".to_string()]);

let pipeline = HydrationPipeline::new(retrieval, config, embedder);

// Hydrate tools for a query
let result = pipeline.hydrate("I need to read a JSON config file").await?;

// Use in prompt building
prompt_builder.append_hydrated_tools(&result);
```

## 事件驱动同步

### MCP Server 事件

```rust
// Start MCP event listener
let coordinator = Arc::new(coordinator);
let handle = coordinator.clone().start_mcp_listener(
    mcp_handle,
    |server_id| {
        // Provide tools for the server
        get_tools_for_server(&server_id)
    }
);

// Events handled:
// - ServerStarted: Sync all tools from the server
// - ToolsChanged: Re-sync tools
// - ServerCrashed: Log warning (tools re-synced on restart)
// - ServerRemoved: Log info
```

### Skill Registry 事件

```rust
// Start Skill event listener
let handle = coordinator.clone().start_skill_listener(
    |skill_id| {
        // Provide tool metadata for the skill
        get_skill_tool_meta(&skill_id)
    }
);

// Events handled:
// - AllReloaded: Re-sync all skill tools
// - SkillLoaded: Sync the single skill
// - SkillRemoved: Invalidate the skill's tool fact
```

## 配置

### ToolRetrievalConfig

```rust
pub struct ToolRetrievalConfig {
    /// Hard threshold for noise filtering (default: 0.4)
    pub hard_threshold: f32,

    /// Soft threshold for confidence boundary (default: 0.6)
    pub soft_threshold: f32,

    /// High confidence threshold (default: 0.7)
    pub high_confidence: f32,

    /// Maximum number of tools to retrieve (default: 5)
    pub top_k: usize,

    /// Core tools always available
    pub core_tools: Vec<String>,
}
```

### HydrationPipelineConfig

```rust
pub struct HydrationPipelineConfig {
    /// Retrieval configuration
    pub retrieval: ToolRetrievalConfig,

    /// Max tools with full schema (default: 5)
    pub max_full_schema: usize,

    /// Max tools with summary only (default: 3)
    pub max_summary: usize,

    /// Core tools to always include
    pub core_tools: Vec<String>,
}
```

## 可观测性

### 日志记录

系统使用 `tracing` 记录所有关键操作：

```rust
// Tool indexing
tracing::info!(
    tool_name = %name,
    optimization_level = "L1",
    confidence = 0.65,
    "Tool indexed with semantic inference"
);

// L2 optimization
tracing::debug!(
    tool_name = %name,
    "Scheduling L2 async optimization"
);

tracing::info!(
    tool_name = %name,
    optimization_level = "L2",
    confidence = 0.9,
    "L2 optimization completed"
);

// Event handling
tracing::info!(
    server_id = %server_id,
    tool_count = %tool_count,
    "MCP server started, syncing tools"
);
```

### 监控指标

建议监控以下指标：

- **L0/L1/L2 分布**: 各优化级别的工具数量
- **L2 成功率**: L2 优化的成功/失败比例
- **检索延迟**: 工具检索的 P50/P95/P99 延迟
- **事件处理延迟**: MCP/Skill 事件的处理时间

## 性能特征

### 检索性能

| 工具数量 | 检索延迟 (P95) | 内存占用 |
|---------|---------------|---------|
| 10 | <10ms | ~1MB |
| 50 | <20ms | ~5MB |
| 200 | <50ms | ~20MB |
| 1000 | <100ms | ~100MB |

### L2 优化

- **触发条件**: L1 confidence < 0.7
- **执行方式**: 后台异步，不阻塞
- **失败处理**: 自动回退到 L1，记录警告日志
- **重试策略**: 当前不重试（可扩展）

## 最佳实践

### 1. 提供高质量的 structured_meta

```rust
// Good: Explicit use cases
ToolMeta::new("git_commit")
    .with_structured_meta("Use this tool when you need to save your work to version control")

// Bad: Generic description
ToolMeta::new("git_commit")
    .with_description("Commit changes")
```

### 2. 合理设置阈值

```rust
// Conservative (high precision, low recall)
ToolRetrievalConfig {
    hard_threshold: 0.5,
    soft_threshold: 0.7,
    high_confidence: 0.8,
    ..Default::default()
}

// Aggressive (high recall, lower precision)
ToolRetrievalConfig {
    hard_threshold: 0.3,
    soft_threshold: 0.5,
    high_confidence: 0.6,
    ..Default::default()
}
```

### 3. 启用 L2 优化

```rust
// Only enable L2 if you have LLM access
if let Some(llm_provider) = get_llm_provider() {
    let coordinator = ToolIndexCoordinator::with_llm(db, llm_provider);
} else {
    let coordinator = ToolIndexCoordinator::new(db);
}
```

### 4. 监听事件以保持同步

```rust
// Always start event listeners in production
let mcp_handle = coordinator.clone().start_mcp_listener(mcp_handle, tool_provider);
let skill_handle = coordinator.clone().start_skill_listener(skill_provider);

// Store handles to prevent premature abort
handles.push(mcp_handle);
handles.push(skill_handle);
```

## 故障排查

### 工具检索不到

1. **检查工具是否已索引**
   ```rust
   let facts = coordinator.get_all_tool_facts().await?;
   println!("Indexed tools: {}", facts.len());
   ```

2. **检查 embedding 质量**
   - 确保使用相同的 embedding 模型
   - 验证 embedding 维度 (384-dim for bge-small-zh-v1.5)

3. **调整阈值**
   - 降低 `hard_threshold` 提高召回率
   - 检查日志中的 similarity scores

### L2 优化失败

1. **检查 LLM provider 配置**
   ```rust
   if !inferrer.has_l2_support() {
       println!("L2 not available, check LLM provider");
   }
   ```

2. **查看错误日志**
   ```
   WARN L2 optimization failed, keeping L1 description
   ```

3. **验证 LLM 响应**
   - 确保 LLM 返回有效的描述（>10 字符）
   - 检查 prompt 是否合理

### 事件监听器停止

1. **检查 channel 状态**
   ```
   INFO MCP event channel closed, stopping listener
   ```

2. **处理 lagged 事件**
   ```
   WARN MCP event listener lagged, some events may have been missed
   ```
   - 增加 channel buffer size
   - 优化事件处理速度

## 未来扩展

### 计划中的功能

1. **L2 重试机制**: 失败时自动重试
2. **工具使用统计**: 记录工具调用频率，优化检索排序
3. **A/B 测试框架**: 对比不同阈值配置的效果
4. **配置热重载**: 运行时更新阈值配置
5. **工具分组**: 按类别/服务器分组管理工具

### 性能优化方向

1. **缓存优化**: 缓存常用工具的检索结果
2. **批量处理**: 批量同步工具以减少数据库操作
3. **增量更新**: 只更新变更的工具，而非全量同步
4. **并行检索**: 并行查询多个工具类别

## 相关文档

- [设计文档](plans/2026-02-05-tool-as-resource-design.md)
- [实现计划](plans/2026-02-05-tool-as-resource-implementation.md)
- [Memory System](MEMORY_SYSTEM.md)
- [Tool System](TOOL_SYSTEM.md)

## 贡献者

- Claude Sonnet 4.5
- Claude Opus 4.5
- User (rootazero)

---

**License**: MIT
**Repository**: https://github.com/rootazero/Aleph
