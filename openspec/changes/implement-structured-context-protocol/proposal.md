# implement-structured-context-protocol

## SUMMARY

实现基于结构化上下文协议的 Agent 架构升级,替代简单的字符串拼接为类型安全的数据结构,集成 Memory 模块以实现 RAG(检索增强生成)功能。为 Search/MCP/Skills 功能预留标准接口,为未来扩展打下基础。

## STATUS

- **Stage**: Proposal
- **Status**: Draft
- **Created**: 2026-01-04
- **Updated**: 2026-01-04

## MOTIVATION

### Current Problem

当前 Aether 使用简单的字符串拼接来构建 Prompt,存在以下问题:

1. **可维护性差**: 业务逻辑和 Prompt 拼接耦合在一起,难以测试和维护
2. **扩展性受限**: 添加新功能(如搜索、MCP)需要大量修改现有代码
3. **类型不安全**: 字符串拼接容易出错,缺少编译时检查
4. **上下文管理混乱**: 没有统一的上下文数据结构,Memory/Search/MCP 数据格式不一致

### Solution Overview

采用 "Dynamic Context Payload (DCP)" 架构模式:

1. **数据结构层**: 定义 `AgentPayload` 作为统一的内部流转格式
2. **组装层**: `PromptAssembler` 负责将结构化数据渲染为 LLM Prompt
3. **中间件层**: Capability 执行器(Memory/Search/MCP)填充上下文数据
4. **配置层**: 通过配置文件定义 Intent、Capability、Format

### Business Value

- **短期**: 完整的 Memory 功能,支持长期对话记忆和上下文召回
- **中期**: 为 Search 和 MCP 集成铺平道路,接口已预留
- **长期**: 支持复杂的 Skills 工作流和多模态能力扩展

## OBJECTIVES

### MVP Scope (This Implementation)

**实现数据结构和 Memory 集成**:

1. ✅ 核心数据结构 (`AgentPayload`, `Intent`, `Capability`)
2. ✅ Payload 构建器 (`PayloadBuilder`) - 已实施
3. ✅ Prompt 组装器 (`PromptAssembler`) - 已实施
4. 🔧 Memory 能力集成到 Router
5. 🔧 Context 格式化 (Markdown 实现,XML/JSON 预留)
6. 🔧 UniFFI 接口更新 (暴露必要的数据结构)
7. 🔧 配置文件扩展 (支持 capabilities 字段)

**接口预留(不实现逻辑)**:

- Search 接口定义 (`SearchResult`, `Capability::Search`)
- MCP 接口定义 (`Capability::Mcp`, MCP resources 字段)
- Skills 接口定义 (`Intent::Skills`, `WorkflowState`)

### Out of Scope

- Search 实际实现(仅接口)
- MCP 实际实现(仅接口)
- Skills 工作流引擎(仅接口)
- Swift UI 层的大规模修改(仅必要的 UniFFI 绑定更新)

## DESIGN DECISIONS

### Key Architecture Choices

#### 1. Intent 分类设计

将 Intent 分为三类:

- **Built-in Features** (`BuiltinSearch`, `BuiltinMcp`): 硬编码功能,需要特殊处理
- **Custom Commands** (`Custom(name)`): 用户自定义 Prompt 转换
- **Skills Workflows** (`Skills(id)`): 复杂的多步骤工作流(预留)

**Rationale**: 清晰的分类使路由逻辑更简洁,也方便用户理解不同指令的能力边界。

#### 2. Capability 执行顺序

固定顺序: Memory → Search → MCP

**Rationale**:
- Memory 优先级最高,提供对话历史上下文
- Search 次之,基于 Memory 上下文决定是否需要搜索
- MCP 最后,可能需要 Memory 和 Search 的结果

#### 3. Context 注入格式

支持三种格式: Markdown(MVP), XML, JSON

**Rationale**:
- Markdown: 最适合 GPT 系列模型,可读性好
- XML: Claude 系列推荐,结构化强
- JSON: 未来多模态/结构化输出需求

#### 4. Memory 集成策略

采用 "Transparent Integration" 模式:

```rust
Router::route()
  → build AgentPayload
  → execute Capability::Memory (retrieve relevant memories)
  → inject into payload.context.memory_snippets
  → PromptAssembler formats context
  → send to Provider
```

**Rationale**: Memory 检索逻辑对 Provider 透明,保持 Provider 接口简洁。

### Trade-offs

#### 🔺 增加了复杂度

**Cost**: 引入新的抽象层(Payload, Builder, Assembler)
**Benefit**: 类型安全、可测试性、可扩展性

**Decision**: 接受复杂度,因为这是构建复杂 Agent 的必经之路。

#### 🔺 性能开销

**Cost**: Payload 构建和序列化增加了 ~5-10ms 延迟
**Benefit**: 可维护性和扩展性

**Decision**: 可接受,相比 LLM API 调用(通常 500ms+),这个开销微不足道。

### Alternative Considered

**Alternative 1: 保持字符串拼接,添加工具函数**

❌ Rejected: 无法解决类型安全和扩展性问题

**Alternative 2: 直接使用 JSON 作为内部格式**

❌ Rejected: 失去了 Rust 类型系统的编译时检查

**Alternative 3: 每个 Provider 自己处理 Memory**

❌ Rejected: 导致重复代码,Memory 逻辑应该集中管理

## DEPENDENCIES

### Blocking Dependencies

- ✅ Memory module 已实现 (database, embedding, retrieval)
- ✅ Config 模块支持 TOML 解析
- ✅ Router 模块存在

### Non-Blocking Dependencies

- Search API 集成 (预留接口)
- MCP Client 实现 (预留接口)

## RISKS & MITIGATIONS

### Risk 1: 现有代码迁移成本

**Risk**: 需要修改 Router 和 Provider 的调用方式

**Mitigation**:
- 保持向后兼容:Provider 接口不变,只在 Router 内部使用 Payload
- 分步迁移:先在 Router 中构建 Payload,Provider 暂时继续接收字符串

**Likelihood**: Medium
**Impact**: Low
**Status**: Accepted

### Risk 2: Memory 检索性能

**Risk**: 每次请求都进行向量检索可能影响延迟

**Mitigation**:
- 设置合理的相似度阈值(0.7)
- 限制返回结果数量(max 5 条)
- 未来可添加缓存层

**Likelihood**: Low
**Impact**: Medium
**Status**: Monitoring

### Risk 3: 配置文件格式变更

**Risk**: 添加新字段(capabilities, context_format)可能破坏现有配置

**Mitigation**:
- 所有新字段都是 Optional
- 提供默认值和自动迁移逻辑
- 在 Config 加载时打印 warning 提示用户

**Likelihood**: Low
**Impact**: Low
**Status**: Mitigated

## TESTING STRATEGY

### Unit Tests

- ✅ `PayloadBuilder` - 已有测试
- ✅ `PromptAssembler` - 已有测试
- ✅ `Intent` / `Capability` - 已有测试
- 🔧 Router 集成测试(构建 Payload)
- 🔧 Memory Capability 执行器测试

### Integration Tests

- 🔧 End-to-end: User input → Payload → Memory retrieval → Prompt assembly → Provider call
- 🔧 Config loading with new fields
- 🔧 Context formatting (Markdown output verification)

### Manual Tests

- 🔧 实际对话中验证 Memory 召回是否相关
- 🔧 检查不同 Intent 是否正确触发 Capability
- 🔧 验证 System Prompt 中的 Context 格式是否正确

## SUCCESS CRITERIA

### Functional Requirements

- [x] AgentPayload 数据结构完整定义
- [ ] Router 能够根据配置构建 Payload
- [ ] Memory 检索能够正确填充 payload.context.memory_snippets
- [ ] PromptAssembler 能够将 Memory 上下文格式化为 Markdown
- [ ] Provider 收到的 System Prompt 包含 Memory 上下文
- [ ] 配置文件支持 capabilities 和 context_format 字段

### Non-Functional Requirements

- [ ] 单次请求处理增加的延迟 < 20ms (不含 Memory 检索时间)
- [ ] Memory 检索延迟 < 50ms
- [ ] 所有新代码通过 `cargo clippy`
- [ ] 测试覆盖率 > 80% (针对新增代码)

### Documentation Requirements

- [ ] 更新 CLAUDE.md 中的架构说明
- [ ] 添加 Payload 构建示例到代码注释
- [ ] 更新 config.toml.example 包含新字段
- [ ] 在 agentstructure/ 文档中记录实施细节

## ROLLOUT PLAN

### Phase 1: Core Implementation (This Change)

1. ✅ 数据结构定义完成
2. 🔧 Router 集成 Payload 构建
3. 🔧 Memory Capability 执行
4. 🔧 Config 文件扩展

### Phase 2: Interface Reservation (Included)

1. 🔧 Search 接口定义
2. 🔧 MCP 接口定义
3. 🔧 Skills 接口定义

### Phase 3: Future Work (Out of Scope)

- Search 实际实现
- MCP Client 集成
- Skills 工作流引擎

## MONITORING & METRICS

### Metrics to Track

- Memory 检索延迟 (p50, p95, p99)
- Memory 召回相关性(用户反馈)
- Payload 构建时间
- 内存占用变化

### Logging

- Router 构建 Payload 时记录 Intent 和 Capabilities
- Memory 检索时记录召回数量和相似度分数
- Capability 执行失败时记录详细错误

## OPEN QUESTIONS

1. ✅ **Memory 相似度阈值应该设为多少?**
   → 决定: 0.7 (可通过配置调整)

2. ✅ **是否需要为不同 Provider 使用不同的 Context 格式?**
   → 决定: MVP 统一使用 Markdown,未来通过配置支持 per-provider 格式

3. 🤔 **是否需要在 Swift 层暴露 AgentPayload?**
   → 待定: 暂时仅在 Rust 内部使用,Swift 继续通过现有接口调用

## NOTES

- 本提案基于 agentstructure.md 文档的架构设计
- 大部分数据结构已在代码中实现,需要补充集成逻辑
- 参考了 agentstructure.md 中 ZIV 的建议(Rust Core + UniFFI 架构)
