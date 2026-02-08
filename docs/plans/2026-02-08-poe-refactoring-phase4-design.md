# Phase 4: POE Handlers 重构设计

**日期**: 2026-02-08
**状态**: 设计中
**优先级**: P1 (架构审计 Phase 4)

---

## 📋 背景

根据架构审计，`core/src/gateway/handlers/poe.rs` (1487行) 存在以下问题：

1. **类型定义分散**: 任务状态、事件类型等散落在 handler 文件中 (~400行)
2. **服务层已存在但位置不当**: `PoeRunManager` 和 `PoeContractService` 已经是良好的服务抽象，但嵌入在 handler 文件中 (~600行)
3. **Handler 职责过重**: 8个 RPC handler 包含参数解析、业务逻辑调用、响应构建等多重职责 (~300行)

**好消息**: 与 Phase 3 不同，poe.rs 已经有了良好的服务层抽象，重构主要是"搬家"而非"重新设计"。

---

## 🎯 设计目标

### 核心原则

1. **类型独立**: 将类型定义提取到独立模块，提高复用性
2. **服务分离**: 将服务层移出 handler 文件，明确职责边界
3. **Handler 瘦身**: Handler 只负责 RPC 协议处理，不包含业务逻辑
4. **向后兼容**: 保持现有 API 不变，所有测试继续通过

### 非目标

- **不重新设计服务层**: 现有的 `PoeRunManager` 和 `PoeContractService` 已经是良好的抽象
- **不改变业务逻辑**: 只移动代码位置，不修改实现细节
- **不优化性能**: 本次重构专注于结构，性能优化留待后续

---

## 📊 当前状态分析

### 文件结构

```
core/src/gateway/handlers/poe.rs (1487行)
├── 类型定义 (~400行)
│   ├── PoeTaskStatus, PoeTaskState
│   ├── Event types (PoeRunStartedEvent, PoeRunCompletedEvent, etc.)
│   └── Config types (PoeRunConfig, ContractConfig)
├── 服务层 (~600行)
│   ├── PoeRunManager (运行管理)
│   └── PoeContractService (契约管理)
└── RPC Handlers (~300行)
    ├── handle_run, handle_status, handle_cancel, handle_list
    ├── handle_prepare, handle_sign, handle_reject
    └── handle_pending
```

### 依赖关系

```
handlers/poe.rs
├── 依赖: agent_loop, dispatcher, memory, config
├── 被依赖: gateway/router.rs (注册 RPC handlers)
└── 内部依赖: PoeRunManager ← PoeContractService
```

---

## 🎨 目标架构

### 模块结构

```
core/src/poe/
├── mod.rs                    # 公共接口
├── types/
│   ├── mod.rs
│   ├── task_state.rs         # PoeTaskStatus, PoeTaskState
│   ├── events.rs             # Event types
│   └── config.rs             # PoeRunConfig, ContractConfig
└── services/
    ├── mod.rs
    ├── run_service.rs        # PoeRunManager
    └── contract_service.rs   # PoeContractService

core/src/gateway/handlers/poe.rs (瘦身至 ~300行)
├── 导入: use crate::poe::{types::*, services::*};
└── 8个 RPC handlers (只负责协议处理)
```

### 职责划分

| 模块 | 职责 | 行数估算 |
|------|------|---------|
| **poe/types/** | 类型定义、序列化/反序列化 | ~400 |
| **poe/services/** | 业务逻辑、状态管理、事件发布 | ~600 |
| **handlers/poe.rs** | RPC 参数解析、服务调用、响应构建 | ~300 |

---

## 🔄 重构优先级

### P0: 类型提取 (最安全)

**目标**: 将类型定义移至 `core/src/poe/types/`

**优势**:
- 最安全的重构，只涉及类型移动
- 提高类型复用性
- 为后续步骤奠定基础

**步骤**:
1. 创建 `poe/types/` 目录结构
2. 提取 `task_state.rs` (PoeTaskStatus, PoeTaskState)
3. 提取 `events.rs` (所有 Event 类型)
4. 提取 `config.rs` (PoeRunConfig, ContractConfig)
5. 更新 `handlers/poe.rs` 的导入语句
6. 验证编译和测试

### P1: 服务层迁移 (中等风险)

**目标**: 将服务层移至 `core/src/poe/services/`

**优势**:
- 明确服务层边界
- 提高服务复用性
- 便于独立测试

**步骤**:
1. 创建 `poe/services/` 目录结构
2. 移动 `PoeRunManager` → `run_service.rs`
3. 移动 `PoeContractService` → `contract_service.rs`
4. 更新 `handlers/poe.rs` 的导入和初始化
5. 验证编译和测试

### P2: Handler 简化 (可选)

**目标**: 简化 handler 代码，提取公共模式

**优势**:
- 减少重复代码
- 提高可维护性

**步骤**:
1. 识别 8 个 handler 的公共模式
2. 提取参数解析辅助函数
3. 提取响应构建辅助函数
4. 验证编译和测试

---

## 📝 实施计划

### Phase 4.1: 类型提取 (P0)

**预计变更**:
- 新增文件: 4 个 (poe/types/*.rs)
- 修改文件: 2 个 (poe/mod.rs, handlers/poe.rs)
- 删除代码: ~400 行 (从 handlers/poe.rs)
- 新增代码: ~420 行 (types/ + 导入语句)

**验证标准**:
- ✅ 编译通过 (0 errors)
- ✅ 所有 POE 相关测试通过
- ✅ 类型可以从 `crate::poe::types` 导入

### Phase 4.2: 服务层迁移 (P1)

**预计变更**:
- 新增文件: 3 个 (poe/services/*.rs)
- 修改文件: 2 个 (poe/mod.rs, handlers/poe.rs)
- 删除代码: ~600 行 (从 handlers/poe.rs)
- 新增代码: ~620 行 (services/ + 导入语句)

**验证标准**:
- ✅ 编译通过 (0 errors)
- ✅ 所有 POE 相关测试通过
- ✅ 服务可以从 `crate::poe::services` 导入

### Phase 4.3: Handler 简化 (P2, 可选)

**预计变更**:
- 修改文件: 1 个 (handlers/poe.rs)
- 删除代码: ~50 行 (重复代码)
- 新增代码: ~30 行 (辅助函数)

**验证标准**:
- ✅ 编译通过 (0 errors)
- ✅ 所有 POE 相关测试通过
- ✅ Handler 代码更简洁

---

## 🧪 测试策略

### 单元测试

```bash
# 测试类型定义
cargo test --package alephcore --lib poe::types

# 测试服务层
cargo test --package alephcore --lib poe::services

# 测试 handlers
cargo test --package alephcore --lib gateway::handlers::poe
```

### 集成测试

```bash
# 测试完整 POE 流程
cargo test --package alephcore poe_integration

# 测试 Gateway RPC
cargo test --package alephcore gateway_poe
```

### 回归测试

```bash
# 运行所有测试
cargo test --workspace
```

---

## 🔒 风险评估

### 高风险点

1. **服务层依赖**: `PoeRunManager` 和 `PoeContractService` 之间有依赖关系
   - **缓解**: 先提取类型 (P0)，再整体移动服务 (P1)

2. **事件发布**: 服务层需要访问 `EventBus`
   - **缓解**: 保持服务初始化方式不变，只移动代码位置

3. **测试覆盖**: POE 系统可能有集成测试依赖具体实现
   - **缓解**: 每个阶段都运行完整测试套件

### 低风险点

1. **类型提取**: 纯类型移动，不涉及逻辑变更
2. **向后兼容**: 保持公共 API 不变
3. **增量重构**: 分 3 个阶段，每个阶段独立验证

---

## 📦 交付物

### Phase 4.1 (P0)

- [ ] `core/src/poe/types/mod.rs`
- [ ] `core/src/poe/types/task_state.rs`
- [ ] `core/src/poe/types/events.rs`
- [ ] `core/src/poe/types/config.rs`
- [ ] 更新 `core/src/poe/mod.rs`
- [ ] 更新 `core/src/gateway/handlers/poe.rs`
- [ ] 测试报告

### Phase 4.2 (P1)

- [ ] `core/src/poe/services/mod.rs`
- [ ] `core/src/poe/services/run_service.rs`
- [ ] `core/src/poe/services/contract_service.rs`
- [ ] 更新 `core/src/poe/mod.rs`
- [ ] 更新 `core/src/gateway/handlers/poe.rs`
- [ ] 测试报告

### Phase 4.3 (P2, 可选)

- [ ] 简化后的 `core/src/gateway/handlers/poe.rs`
- [ ] 测试报告

---

## 🔄 回滚计划

### 回滚触发条件

- 编译失败且无法在 30 分钟内修复
- 测试失败率 > 5%
- 发现严重的架构问题

### 回滚步骤

```bash
# 1. 切换回 main 分支
git checkout main

# 2. 删除 worktree
git worktree remove phase4-poe-refactoring

# 3. 删除远程分支 (如果已推送)
git push origin --delete phase4-poe-refactoring
```

---

## 📚 参考资料

- [Phase 1: Types 重构设计](./2026-02-07-types-refactoring-phase1-design.md)
- [Phase 2: Atomic Executor 重构设计](./2026-02-07-atomic-executor-refactoring-phase2-design.md)
- [Phase 3: Browser 重构设计](./2026-02-08-browser-refactoring-phase3-design.md)
- [POE 架构设计](./2026-02-01-poe-architecture-design.md)
- [DDD+BDD 双轮驱动设计](./2026-02-06-ddd-bdd-dual-wheel-design.md)

---

## ✅ 验收标准

### 功能验收

- [ ] 所有 POE RPC 方法正常工作
- [ ] 事件发布和订阅正常
- [ ] 任务状态管理正确
- [ ] 契约签署流程完整

### 代码质量

- [ ] 编译 0 errors, 0 new warnings
- [ ] 所有测试通过 (100%)
- [ ] 代码覆盖率不降低
- [ ] 文档更新完整

### 架构验收

- [ ] 类型定义独立可复用
- [ ] 服务层职责清晰
- [ ] Handler 只负责协议处理
- [ ] 模块依赖关系合理

---

**设计完成日期**: 2026-02-08
**审核状态**: 待审核
**下一步**: 用户确认设计方案，开始实施 Phase 4.1 (P0)
