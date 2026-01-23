# Change: Smart Tool Discovery System

## Why

当前 Aether 将所有注册工具一股脑传递给 LLM，存在以下问题：
1. **Token 浪费**: 50+ 工具的完整 schema 可能消耗 10K+ tokens
2. **延迟增加**: 更多 tokens = 更长的首次响应时间
3. **LLM 混淆**: 过多工具选择降低调用准确性
4. **不可扩展**: 用户动态添加 MCP/Skills/Plugins 会导致工具数量失控

借鉴 Claude Code 的设计理念：
- 核心工具模型原生支持，无需每次传递完整 schema
- 复杂任务通过子代理委托，而非堆工具
- Skill 系统是提示词增强，不是新工具

## What Changes

### Phase 1: Two-Stage Tool Discovery (两阶段工具发现)

1. **轻量工具索引 (Tool Index)**
   - 总是传递给 LLM 的精简工具列表
   - 仅包含: name, category, one-line description
   - Token 消耗: ~500 tokens (vs 10K+)

2. **元工具 (Meta Tools)**
   - `list_tools(category?)` - 列出可用工具类别/工具
   - `get_tool_schema(tool_name)` - 获取工具完整定义
   - `request_tools(category)` - 请求加载某类工具

3. **按需 Schema 加载**
   - LLM 决定使用某工具时，才获取完整参数 schema
   - 使用缓存避免重复加载

### Phase 2: Intent-Based Pre-filtering (意图驱动预筛选)

1. **意图分析增强**
   - 利用现有 L1/L2/L3 意图检测
   - 提取 `required_capabilities` 和 `tool_categories`

2. **工具预筛选**
   - 根据意图预选 5-10 个相关工具
   - 传递预选工具的完整定义（非索引）

3. **核心工具集**
   - 定义 5-8 个核心工具（总是可用）
   - 包含元工具和基础能力

### Phase 3: Sub-Agent Delegation (子代理委托)

1. **专门子代理**
   - MCP Agent: 处理 MCP 工具调用
   - Skill Agent: 处理 Skill 执行
   - Code Agent: 处理代码相关任务

2. **委托机制**
   - 主代理识别任务需要专门能力
   - 委托给子代理（带独立工具集）
   - 子代理返回结果给主代理

## Impact

- Affected specs: `dispatcher/registry`, `agent_loop`, `intent`
- Affected code:
  - `core/src/dispatcher/registry.rs` - 新增 ToolIndex, MetaTool
  - `core/src/dispatcher/types.rs` - 新增 ToolIndexEntry
  - `core/src/intent/` - 增强意图分析输出
  - `core/src/agent_loop/` - 支持两阶段工具发现
  - `core/src/agents/` - 子代理系统

## Success Metrics

- Token 消耗减少 70%+
- 首次响应延迟减少 30%+
- 工具调用准确率保持或提升
- 支持 100+ 工具而不影响性能
