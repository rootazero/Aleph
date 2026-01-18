# Aether 架构重构设计方案

> **日期**: 2026-01-16
> **状态**: 待审批
> **作者**: Claude (AI Assistant)
> **审查标准**: 低耦合、高内聚、模块化、可扩展

---

## 概述

本文档基于对 Aether 项目的系统性架构审查，提出了 6 项改进方案。项目经过多次迭代、功能添加和代码重组后，存在以下核心问题：

- **uniffi_core.rs** 成为"上帝模块"（1000+ 行，8 种职责）
- **工具系统**分散在 3 个模块，扩展困难
- **内存系统**存在 memory/ 和 store/ 两套实现
- **payload/capability** 存在循环依赖
- **AppDelegate.swift** 职责过重（1230 行）
- **Model Router** 过度设计（7 个文件，5 层 fallback）

### 架构评分

| 标准 | 当前评分 | 目标评分 |
|------|----------|----------|
| 低耦合 | 5.0/10 | 7.5/10 |
| 高内聚 | 5.25/10 | 7.5/10 |
| 模块化 | 5.75/10 | 8.0/10 |
| 可扩展性 | 5.75/10 | 8.0/10 |
| **综合** | **5.44/10** | **7.75/10** |

---

## 架构图

### 整体架构

```
┌─────────────────────────────────────────────────────────────────────────────────────┐
│                                    用户交互层                                         │
│  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐  │
│  │ 全局热键触发     │  │ 选区捕获       │  │ 光标位置检测    │  │ 剪贴板操作      │  │
│  └────────┬────────┘  └───────┬────────┘  └────────┬────────┘  └────────┬────────┘  │
└───────────┼──────────────────┼──────────────────────┼───────────────────┼───────────┘
            └───────────────────┴──────────┬───────────┴───────────────────┘
                                           ↓
┌─────────────────────────────────────────────────────────────────────────────────────┐
│                               Swift UI 层 (原生 macOS)                               │
│  ┌─────────────────────────────────────────────────────────────────────────────────┐│
│  │                           AppDelegate (生命周期中心)                              ││
│  └──────────────────────────────────────┬──────────────────────────────────────────┘│
│  ┌───────────────┐  ┌───────────────┐  ┌─┴────────────┐  ┌───────────────────────┐  │
│  │InputCoordinator│ │OutputCoordinator│ │PermissionCo.│ │ UnifiedInputCoordinator│  │
│  └───────┬───────┘  └───────┬───────┘  └──────────────┘  └───────────────────────┘  │
│  ┌───────┴──────────────────┴───────────────────────────────────────────────────┐   │
│  │                              HaloWindow + HaloView                            │   │
│  └──────────────────────────────────────────────────────────────────────────────┘   │
└──────────────────────────────────────────┼───────────────────────────────────────────┘
                                    ═══════╪═══════  UniFFI FFI 边界
┌──────────────────────────────────────────┼───────────────────────────────────────────┐
│                               Rust Core 层 (核心业务逻辑)                             │
│  ┌───────────────────────────────────────▼───────────────────────────────────────┐  │
│  │                         uniffi_core.rs (AetherCore)                            │  │
│  └───────────────────────────────────────┬───────────────────────────────────────┘  │
│            ┌─────────────────────────────┼─────────────────────────────┐             │
│            ↓                             ↓                             ↓             │
│  ┌─────────────────┐          ┌─────────────────┐          ┌─────────────────┐      │
│  │    agent/       │          │    cowork/      │          │   dispatcher/   │      │
│  │ RigAgentManager │          │  CoworkEngine   │          │  ToolRegistry   │      │
│  └────────┬────────┘          └────────┬────────┘          └────────┬────────┘      │
│  ┌────────┴────────────────────────────┴────────────────────────────┴────────┐      │
│  │                           payload/ (结构化上下文协议)                       │      │
│  └────────────────────────────────────┬───────────────────────────────────────┘      │
│  ┌────────────────────────────────────┼────────────────────────────────────────┐    │
│  │                          capability/ (能力执行层)                            │    │
│  │  ┌───────────────┐  ┌───────────────┐  ┌───────────────┐  ┌──────────────┐  │    │
│  │  │MemoryStrategy │  │  McpStrategy  │  │SkillsStrategy │  │SearchStrategy│  │    │
│  │  └───────┬───────┘  └───────┬───────┘  └───────┬───────┘  └──────┬───────┘  │    │
│  └──────────┼──────────────────┼───────────────────┼─────────────────┼──────────┘    │
│  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐  ┌──────────────┐    │
│  │    memory/      │  │      mcp/       │  │    skills/      │  │   search/    │    │
│  └─────────────────┘  └─────────────────┘  └─────────────────┘  └──────────────┘    │
│  ┌───────────────────────────────────────────────────────────────────────────────┐  │
│  │                           providers/ (AI 提供商抽象)                           │  │
│  │   ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐       │  │
│  │   │ Claude   │  │ OpenAI   │  │ Gemini   │  │ Ollama   │  │ DeepSeek │       │  │
│  │   └──────────┘  └──────────┘  └──────────┘  └──────────┘  └──────────┘       │  │
│  └───────────────────────────────────────────────────────────────────────────────┘  │
│  ┌────────────────────────────────────────────────────────────────────────────────┐ │
│  │                              支撑模块层                                          │ │
│  │  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌─────────┐ │ │
│  │  │ config/  │ │ logging/ │ │ metrics/ │ │services/ │ │ intent/  │ │  video/ │ │ │
│  │  └──────────┘ └──────────┘ └──────────┘ └──────────┘ └──────────┘ └─────────┘ │ │
│  └────────────────────────────────────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────────────────────────────────┘
```

---

## 改进方案

### 优先级矩阵

```
        高影响
          ↑
          │   ┌─────────────────┐         ┌─────────────────┐
          │   │ P1: 拆分        │         │ P2: 统一        │
          │   │ uniffi_core.rs  │         │ 工具系统        │
          │   └─────────────────┘         └─────────────────┘
          │   ┌─────────────────┐         ┌─────────────────┐
          │   │ P3: 合并        │         │ P4: 解耦        │
          │   │ memory/store    │         │ payload/cap.    │
          │   └─────────────────┘         └─────────────────┘
          │   ┌─────────────────┐         ┌─────────────────┐
          │   │ P5: 拆分        │         │ P6: 简化        │
          │   │ AppDelegate     │         │ Model Router    │
          │   └─────────────────┘         └─────────────────┘
        低影响 ──────────────────────────────────────→ 高实施难度
```

---

## P1: 拆分 uniffi_core.rs（最高优先级）

### 问题

- 当前超过 1000 行，承担 8 种职责
- 依赖几乎所有核心模块，成为"上帝模块"
- 任何模块变更都可能影响 FFI 层

### 目标结构

```
src/
├── ffi/                     (新建目录)
│   ├── mod.rs              (AetherCore 主结构 + 核心 API)
│   ├── processing.rs       (process/process_multi_turn)
│   ├── tools.rs            (list_tools/register_mcp_tools)
│   ├── memory.rs           (search_memory/store_memory/get_stats)
│   ├── cowork.rs           (cowork_plan/cowork_execute)
│   ├── config.rs           (reload_config/get_provider_info)
│   ├── skills.rs           (list_skills/match_skill)
│   └── mcp.rs              (list_mcp_servers/get_status)
└── uniffi_core.rs          (仅保留 UniFFI 导出声明)
```

### 实施步骤

1. 创建 `src/ffi/mod.rs`，保留 `AetherCore` 核心结构
2. 创建 `src/ffi/processing.rs`，移入 `process()` 和 `process_multi_turn()`
3. 创建 `src/ffi/tools.rs`，移入工具管理相关方法
4. 创建 `src/ffi/memory.rs`，移入内存操作相关方法
5. 创建 `src/ffi/cowork.rs`，移入 Cowork 相关方法
6. 创建 `src/ffi/config.rs`，移入配置管理相关方法
7. 创建 `src/ffi/skills.rs`，移入 Skills 相关方法
8. 创建 `src/ffi/mcp.rs`，移入 MCP 相关方法
9. 更新 `uniffi_core.rs` 仅保留 `pub use ffi::*` 和 `uniffi::setup_scaffolding!()`

### 预期效果

| 指标 | 改进前 | 改进后 |
|------|--------|--------|
| 单文件行数 | 1000+ | <200 |
| 模块依赖数 | 15+ | 各模块 2-3 |
| 变更影响范围 | 全局 | 局部 |

---

## P2: 统一工具系统

### 问题

- 工具定义分散在 `tools/`、`rig_tools/`、`dispatcher/` 三个位置
- 新增工具类型需修改 5 个文件
- 概念边界模糊

### 目标结构

```
src/
└── tools/                   (统一工具模块)
    ├── mod.rs              (公共导出)
    ├── traits.rs           (AgentTool trait)
    ├── types.rs            (ToolDefinition, ToolSource, ToolPriority)
    ├── registry.rs         (UnifiedToolRegistry - 从 dispatcher 移入)
    ├── executor.rs         (UnifiedToolExecutor - 从 core 移入)
    ├── confirmation.rs     (确认系统 - 从 dispatcher 移入)
    │
    ├── builtin/            (内置命令)
    │   ├── mod.rs
    │   ├── search.rs
    │   ├── youtube.rs
    │   └── webfetch.rs
    │
    ├── native/             (原生 AgentTool)
    │   ├── mod.rs
    │   ├── search_tool.rs
    │   ├── webfetch_tool.rs
    │   ├── youtube_tool.rs
    │   ├── file_read.rs
    │   └── screen_capture.rs
    │
    └── mcp/                (MCP 桥接)
        ├── mod.rs
        └── wrapper.rs
```

### 实施步骤

1. 合并类型定义到 `tools/types.rs`
2. 统一 `AgentTool` trait 定义
3. 迁移 `rig_tools/*` → `tools/native/*`
4. 迁移 `dispatcher/registry.rs` → `tools/registry.rs`
5. 迁移 `dispatcher/confirmation.rs` → `tools/confirmation.rs`
6. 更新所有依赖方的 import 路径
7. 删除空的 `rig_tools/` 和 `dispatcher/` 目录（保留 dispatcher 中其他功能）

### 预期效果

| 指标 | 改进前 | 改进后 |
|------|--------|--------|
| 添加新工具需修改文件数 | 5 | 1-2 |
| 工具相关模块数 | 3 | 1 |

---

## P3: 合并 memory/store 系统

### 问题

- `memory/` - 自研 RAG (sqlite-vec + fastembed)
- `store/` - rig-core MemoryStore (rig-sqlite)
- 两套系统功能重叠

### 决策

**保留 `memory/` 模块，废弃 `store/`**

理由：
1. `memory/` 已实现完整的双层架构
2. `memory/` 支持上下文锚点
3. `store/` 仅是薄封装
4. `memory/` 更符合"本地优先"理念

### 目标结构

```
src/
└── memory/                  (统一内存模块)
    ├── mod.rs
    ├── database.rs
    ├── embedding.rs
    ├── ingestion.rs
    ├── retrieval.rs
    ├── compression/
    └── rig_adapter.rs      (新增: 为 rig-core 提供适配器)
```

### 实施步骤

1. 创建 `memory/rig_adapter.rs`，实现 `rig::memory::MemoryStore` trait
2. 更新 `agent/manager.rs` 使用适配器
3. 删除 `src/store/` 目录
4. 更新 `Cargo.toml` 移除不需要的依赖

### 预期效果

| 指标 | 改进前 | 改进后 |
|------|--------|--------|
| 内存系统数量 | 2 | 1 |
| 维护文件数 | ~15 | ~10 |

---

## P4: 解耦 payload/capability 循环依赖

### 问题

```
payload/mod.rs → AgentPayload { capabilities: Vec<Capability> }
        ↓ 使用
capability/mod.rs → execute(payload: AgentPayload)
        ↓ 定义
payload/capability.rs → enum Capability
```

### 目标结构

```
src/
├── core/                    (核心类型 - 零依赖)
│   ├── mod.rs
│   ├── capability.rs       (Capability enum)
│   ├── intent.rs
│   └── context.rs
│
├── payload/                 (依赖 core)
│   ├── mod.rs
│   ├── builder.rs
│   ├── assembler.rs
│   └── types.rs
│
└── capability/              (依赖 core + payload)
    ├── mod.rs
    ├── executor.rs
    └── strategies/
```

### 实施步骤

1. 移动 `Capability` enum 到 `core/capability.rs`
2. 更新 `payload/` 使用 `crate::core::Capability`
3. 更新 `capability/` 使用 `crate::core::Capability`
4. 验证编译顺序：`core ← payload ← capability`

### 预期效果

| 指标 | 改进前 | 改进后 |
|------|--------|--------|
| 循环依赖 | 存在 | 消除 |
| 编译顺序 | 不确定 | 明确 |

---

## P5: 拆分 AppDelegate.swift

### 问题

- 超过 1230 行，承担 6 种职责
- 难以测试和维护

### 目标结构

```
Aether/Sources/
├── AppDelegate.swift        (~200 行, 仅生命周期)
├── AppServices/
│   ├── MenuBarService.swift
│   ├── PermissionService.swift
│   ├── CoreService.swift
│   ├── HotkeyService.swift
│   └── ServiceContainer.swift
└── Coordinator/
    ├── InputCoordinator.swift
    ├── OutputCoordinator.swift
    └── PermissionCoordinator.swift
```

### 实施步骤

1. 创建 `AppServices/ServiceContainer.swift` 依赖注入容器
2. 提取 `MenuBarService.swift`
3. 提取 `PermissionService.swift`
4. 提取 `CoreService.swift`
5. 提取 `HotkeyService.swift`
6. 简化 `AppDelegate.swift` 仅保留生命周期逻辑

### 预期效果

| 指标 | 改进前 | 改进后 |
|------|--------|--------|
| AppDelegate 行数 | 1230 | ~200 |
| 职责数 | 6 | 1 |
| 可测试性 | 困难 | 各 Service 可独立测试 |

---

## P6: 简化 Model Router

### 问题

- 7 个文件，实现 5 层 fallback 链
- 复杂度过高，与 config.toml 的 rules 功能重叠
- 大多数用户只需简单的 provider 选择

### 目标结构

```
src/cowork/
├── engine.rs
├── planner/
├── scheduler/
├── executor/
├── monitor/
└── router/                  (简化后)
    ├── mod.rs              (RouterService)
    ├── rules.rs            (RuleBasedRouter)
    └── intent.rs           (TaskIntent - 简化版)

删除:
- model_router/context.rs
- model_router/matcher.rs
- model_router/pipeline.rs
- model_router/profiles.rs
```

### 简化后的 Fallback 链

```
当前 5 层:
L1: RoutingRuleConfig (regex/keyword)
L2: TaskIntent (Coding/Research/Creative)
L3: ModelMatcher (能力匹配)
L4: 模型配置默认值
L5: 全局默认 Provider

简化为 3 层:
L1: RoutingRuleConfig (配置文件规则)
L2: TaskIntent (仅作为 hint)
L3: 全局默认 Provider
```

### 实施步骤

1. 创建简化版 `RouterService`
2. 简化 `TaskIntent` enum
3. 删除过度设计的文件
4. 更新 `CoworkEngine` 使用简化版路由

### 预期效果

| 指标 | 改进前 | 改进后 |
|------|--------|--------|
| 文件数 | 7 | 3 |
| 代码行数 | ~1500 | ~400 |
| 理解难度 | 高 | 低 |

---

## 实施计划

### Phase 1: 核心解耦 (P1 + P4)

**目标**: 消除上帝模块和循环依赖

**范围**:
- uniffi_core.rs 拆分
- payload/capability 解耦

**风险**: 中等（FFI 边界变更需要同步更新 Swift）

**验证**:
```bash
cd Aether/core && cargo build
xcodegen generate && xcodebuild -project Aether.xcodeproj -scheme Aether build
```

### Phase 2: 工具系统统一 (P2)

**目标**: 统一工具概念，简化扩展

**范围**: 合并 tools/rig_tools/dispatcher

**风险**: 中等（影响所有工具调用路径）

**验证**: 所有工具类型正常工作
```bash
cargo test tools
# 手动测试 /search, /youtube, /webfetch, MCP tools
```

### Phase 3: 内存系统合并 (P3)

**目标**: 消除重复，统一内存管理

**范围**: 废弃 store/，增强 memory/

**风险**: 低（store/ 使用较少）

**验证**:
```bash
cargo test memory
# 手动测试内存存储和检索
```

### Phase 4: Swift 层重构 (P5)

**目标**: 提高 Swift 代码可维护性

**范围**: 拆分 AppDelegate

**风险**: 低（不影响 Rust 核心）

**验证**:
```bash
xcodebuild -project Aether.xcodeproj -scheme Aether build
# 手动测试应用启动和热键功能
```

### Phase 5: 简化 Cowork (P6)

**目标**: 降低复杂度，提高可理解性

**范围**: 简化 Model Router

**风险**: 低（功能降级但更实用）

**验证**:
```bash
cargo test cowork
# 手动测试 Cowork 任务执行
```

---

## 预期改进效果

| 维度 | 改进前 | 改进后 | 提升 |
|------|--------|--------|------|
| 低耦合评分 | 5.0/10 | 7.5/10 | +50% |
| 高内聚评分 | 5.25/10 | 7.5/10 | +43% |
| 模块化评分 | 5.75/10 | 8.0/10 | +39% |
| 可扩展性评分 | 5.75/10 | 8.0/10 | +39% |
| **综合评分** | **5.44/10** | **7.75/10** | **+42%** |

### 代码指标

| 指标 | 改进前 | 改进后 | 变化 |
|------|--------|--------|------|
| uniffi_core.rs 行数 | 1000+ | <200 | -80% |
| AppDelegate.swift 行数 | 1230 | ~200 | -84% |
| 工具模块数 | 3 | 1 | -67% |
| 内存系统数 | 2 | 1 | -50% |
| Model Router 文件数 | 7 | 3 | -57% |

---

## 风险评估

| 改进项 | 风险等级 | 主要风险 | 缓解措施 |
|--------|----------|----------|----------|
| P1 | 中 | FFI 边界变更 | 逐步迁移，保持兼容 |
| P2 | 中 | 工具调用中断 | 完善测试覆盖 |
| P3 | 低 | 数据迁移 | store/ 使用较少 |
| P4 | 低 | 编译问题 | 类型提取简单 |
| P5 | 低 | UI 回归 | 独立于 Rust |
| P6 | 低 | 功能降级 | 保留核心路由 |

---

## 附录

### A. 当前依赖关系热力图

```
被依赖模块 →      config  payload  providers  memory  dispatcher  mcp  agent
依赖方 ↓         ──────  ───────  ─────────  ──────  ──────────  ───  ─────
uniffi_core        ●●●     ●●●      ●●●       ●●●       ●●●      ●●    ●●●
agent              ●●      ●●       ●●●        ●        ●●       ●●     -
cowork             ●●      ●●       ●●         ●        ●●        -     ●
capability         ●●      ●●●       ●        ●●         ●       ●●     -
payload            ●●       -        ●         -         ●        -     -
dispatcher         ●●      ●●        -         -         -       ●●     -

●●● = 强依赖 (>5)    ●● = 中等依赖 (2-5)    ● = 弱依赖 (1)
```

### B. 模块边界清晰度分析

| 清晰边界 ✅ | 模糊边界 ⚠️ |
|------------|------------|
| providers/ (AiProvider trait) | tools/ vs rig_tools/ vs dispatcher/ |
| search/ (SearchProvider) | memory/ vs store/ |
| mcp/ (McpClient) | payload/ vs capability/ |
| memory/ (VectorDatabase) | agent/ vs cowork/ |
| config/ (Config struct) | |

### C. 扩展点清单

| 扩展类型 | 位置 | 扩展方式 | 难度 |
|----------|------|----------|------|
| AI Provider | providers/ | 实现 AiProvider trait | 简单 |
| 搜索引擎 | search/providers/ | 实现 SearchProvider | 简单 |
| MCP Server | config.toml | 配置 [mcp.servers.x] | 简单 |
| Skills | ~/.config/aether/skills | 创建 SKILL.md | 简单 |
| Cowork Executor | cowork/executor/ | 实现 TaskExecutor | 中等 |
| Capability | capability/strategies/ | 实现 CapabilityStrategy | 中等 |
| 新工具类型 | tools/ + dispatcher/ | 修改多处 | 困难 |
| 新 FFI API | uniffi_core.rs | 修改上帝模块 | 困难 |

---

## 变更历史

| 日期 | 版本 | 变更内容 |
|------|------|----------|
| 2026-01-16 | 1.0 | 初始版本 |
