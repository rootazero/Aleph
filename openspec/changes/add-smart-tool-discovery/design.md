# Design: Smart Tool Discovery System

## Context

Aether 作为 AI Agent，需要支持大量动态工具（MCP、Skills、Plugins）。但传统的"全量工具传递"方式存在 token 浪费和延迟问题。

**约束**:
- 必须兼容现有 rig-core 工具系统
- 必须支持动态注册/注销
- 不能破坏现有工具调用流程
- 需要支持 100+ 工具场景

**参考**: Claude Code 的设计
- 核心工具模型原生支持
- 子代理模式处理复杂任务
- Skill 是提示词增强，非工具扩展

## Goals / Non-Goals

**Goals**:
- 减少 LLM token 消耗 70%+
- 支持 100+ 工具而不影响性能
- 保持工具调用准确率
- 兼容现有 MCP/Skill/Plugin 系统

**Non-Goals**:
- 不改变工具注册 API
- 不改变工具执行流程
- 不实现工具自动发现（需用户配置）

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                   Smart Tool Discovery Architecture             │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  User Input                                                     │
│       ↓                                                         │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │ Intent Analysis (L1/L2/L3)                                │  │
│  │ Output: intent_type, required_capabilities, categories    │  │
│  └───────────────────────────────────────────────────────────┘  │
│       ↓                                                         │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │ Tool Selection Layer                                      │  │
│  │ ┌─────────────────┐  ┌─────────────────┐                 │  │
│  │ │ Core Tools      │  │ Filtered Tools  │                 │  │
│  │ │ (5-8 always)    │  │ (5-10 by intent)│                 │  │
│  │ │ - search        │  │ - relevance     │                 │  │
│  │ │ - file_ops      │  │ - top-K         │                 │  │
│  │ │ - list_tools    │  │                 │                 │  │
│  │ │ - get_schema    │  │                 │                 │  │
│  │ └─────────────────┘  └─────────────────┘                 │  │
│  └───────────────────────────────────────────────────────────┘  │
│       ↓                                                         │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │ LLM Request (15-20 tools max)                             │  │
│  │ - Core tools: full schema                                 │  │
│  │ - Filtered tools: full schema                             │  │
│  │ - Other tools: index only (name + description)            │  │
│  └───────────────────────────────────────────────────────────┘  │
│       ↓                                                         │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │ Tool Call Flow                                            │  │
│  │                                                           │  │
│  │ LLM wants to call "github_pr_list"                        │  │
│  │   ↓                                                       │  │
│  │ Schema not in context?                                    │  │
│  │   ↓ Yes                                                   │  │
│  │ Call get_tool_schema("github_pr_list")                    │  │
│  │   ↓                                                       │  │
│  │ Schema injected into context                              │  │
│  │   ↓                                                       │  │
│  │ LLM calls tool with correct params                        │  │
│  │                                                           │  │
│  └───────────────────────────────────────────────────────────┘  │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

## Key Decisions

### Decision 1: Tool Index Format

**选择**: 精简的 markdown 格式

```markdown
## Available Tools

### Core (always available)
- search: Web search
- file_ops: File read/write/delete
- shell: Execute shell commands

### MCP (use get_tool_schema to get details)
- github:pr_list: List pull requests
- github:issue_create: Create issue
- notion:page_read: Read Notion page

### Skills (use get_tool_schema to get details)
- code-review: Review code for issues
- refine-text: Improve writing quality
```

**理由**:
- Markdown 是 LLM 原生理解的格式
- 分类展示便于 LLM 选择
- 明确指示如何获取详情

**替代方案**: JSON 格式
- 优点: 结构化，可程序解析
- 缺点: 对 LLM 不够友好，token 效率低

### Decision 2: Meta Tools Design

**选择**: 三个元工具

```rust
// 1. 列出工具（按类别）
pub struct ListToolsTool;
// Input: { category?: string }
// Output: 该类别下的工具列表

// 2. 获取工具 schema
pub struct GetToolSchemaTool;
// Input: { tool_name: string }
// Output: 完整的 JSON Schema

// 3. 请求加载工具（预留，暂不实现）
pub struct RequestToolsTool;
// Input: { category: string }
// Output: 加载成功/失败
```

**理由**:
- 最小化元工具数量
- 覆盖主要使用场景
- 保持简洁

### Decision 3: Core Tools Selection

**选择**: 以下工具作为核心工具（总是有完整 schema）

| Tool | Reason |
|------|--------|
| search | 最常用，信息检索基础 |
| file_ops | 文件操作基础 |
| shell | 命令执行基础 |
| list_tools | 元工具：发现其他工具 |
| get_tool_schema | 元工具：获取工具详情 |
| generate_image | 高频生成能力 |

**理由**:
- 覆盖 80% 的基础场景
- 保持核心集小（6-8 个）
- 高频工具避免二次查询

### Decision 4: Intent → Tool Category Mapping

**选择**: 静态映射 + 动态扩展

```rust
pub fn intent_to_categories(intent: &IntentAnalysis) -> Vec<ToolCategory> {
    let mut categories = vec![];

    // 静态映射
    match intent.intent_type.as_str() {
        "code" | "programming" => categories.push(ToolCategory::Mcp), // github, etc.
        "media" | "image" | "video" => categories.push(ToolCategory::Builtin),
        "knowledge" | "search" => categories.push(ToolCategory::Builtin),
        "file" => categories.push(ToolCategory::Builtin),
        _ => {}
    }

    // 从 capabilities 推断
    for cap in &intent.capabilities {
        match cap.as_str() {
            "github" | "git" => categories.push(ToolCategory::Mcp),
            "notion" | "slack" => categories.push(ToolCategory::Mcp),
            _ => {}
        }
    }

    categories
}
```

**理由**:
- 简单高效
- 可配置扩展
- 不需要额外 LLM 调用

## Data Flow

### Two-Stage Discovery Flow

```
1. 用户输入 "帮我创建一个 GitHub PR"

2. Intent Analysis:
   - intent_type: "code"
   - capabilities: ["github", "git"]
   - categories: [Mcp]

3. Tool Selection:
   - Core: search, file_ops, shell, list_tools, get_schema
   - Filtered: github:pr_create, github:pr_list, github:branch_list
   - Index: 其他 40+ 工具仅展示名称

4. LLM Request:
   - tools: [Core + Filtered] (10 tools with full schema)
   - tool_index: [Other tools] (40 tools, index only)
   - Total tokens: ~2000 (vs ~10000)

5. LLM Response:
   - tool_call: github:pr_create(...)
   - Schema 已在 context，直接执行
```

### On-Demand Schema Flow

```
1. LLM 想调用 "notion:page_create"（不在 filtered 集合中）

2. LLM 调用 get_tool_schema("notion:page_create")

3. System 返回完整 schema:
   {
     "name": "notion:page_create",
     "description": "...",
     "parameters": {...}
   }

4. LLM 使用 schema 构造正确的调用

5. System 执行工具，返回结果
```

## Risks / Trade-offs

### Risk 1: 多轮交互增加延迟
- **风险**: 两阶段发现需要额外 LLM 轮次
- **缓解**:
  - 核心工具避免二次查询
  - 意图预筛选提高命中率
  - 预加载高频工具 schema

### Risk 2: LLM 可能选择错误工具
- **风险**: 仅有 index 时，LLM 可能误选
- **缓解**:
  - index 包含足够描述信息
  - list_tools 可获取类别详情
  - 允许 LLM 请求更多信息

### Risk 3: 复杂度增加
- **风险**: 新增元工具和索引逻辑
- **缓解**:
  - 保持架构简单（3 个元工具）
  - 渐进式实现（Phase 1 优先）

## Migration Plan

### Phase 1 (MVP)
1. 实现 Tool Index 生成
2. 实现 list_tools, get_tool_schema 元工具
3. 修改 agent loop 支持两阶段
4. 保持现有工具调用兼容

### Phase 2
1. 集成意图分析
2. 实现工具预筛选
3. 配置核心工具集

### Phase 3
1. 子代理框架设计
2. 专门子代理实现
3. 委托机制集成

### Rollback
- 所有变更可通过配置开关回退
- 保持 `to_prompt_block()` 原有逻辑
- 新增 `to_smart_prompt()` 作为替代

## Open Questions

1. **核心工具集是否应该用户可配置？**
   - 倾向: 是，通过 config.toml

2. **Tool Index 更新频率？**
   - 倾向: 每次 session 开始时，MCP/Skill 变更时

3. **是否需要工具使用统计来优化预筛选？**
   - 倾向: Phase 2+ 考虑

4. **子代理的独立 context 如何管理？**
   - 倾向: Phase 3 详细设计
