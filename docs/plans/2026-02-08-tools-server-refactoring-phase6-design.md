# Phase 6: Tools Server 重构设计

**日期**: 2026-02-08
**状态**: 设计中
**优先级**: P1 (架构审计 Phase 6)

---

## 📋 背景

根据架构审计，`core/src/tools/server.rs` (1091行) 存在以下问题：

1. **Builder 方法过多**: 40+ 个 `with_*` 方法用于注册内置工具
2. **职责混合**: 核心工具服务器逻辑与内置工具注册混在一起
3. **文件过大**: 单文件包含所有工具注册逻辑

**模式**: Builder 模式过度使用，需要提取内置工具注册逻辑。

---

## 🎯 设计目标

### 核心原则

1. **分离关注点**: 核心服务器逻辑与内置工具注册分离
2. **保持 Builder 模式**: 继续使用 builder 模式，但分模块实现
3. **向后兼容**: 保持现有 API 不变

### 非目标

- **不改变 API**: 保持 `AlephToolServer::new().with_bash().with_file_ops()` 等调用方式
- **不重新设计**: 只移动代码位置，不修改实现

---

## 📊 当前状态分析

### 文件结构

```
core/src/tools/server.rs (1091行)
├── 核心类型 (~50行)
│   ├── AlephToolServer
│   ├── AlephToolServerHandle
│   ├── ToolRepairInfo
│   └── ToolUpdateInfo
├── 核心方法 (~300行)
│   ├── new(), tool(), tool_boxed()
│   ├── add_tool(), replace_tool(), remove_tool()
│   ├── get_definition(), list_definitions()
│   ├── call(), call_with_repair()
│   └── handle(), into_handle()
└── 内置工具注册 (~741行)
    ├── with_bash()
    ├── with_file_ops() (read, write, edit, move, delete)
    ├── with_search() (glob, grep)
    ├── with_web_fetch()
    ├── with_web_search()
    ├── with_ask_user()
    ├── with_task_tools() (create, update, get, list)
    ├── with_skill()
    ├── with_enter_plan_mode()
    ├── with_exit_plan_mode()
    ├── with_notebook_edit()
    ├── with_task_output()
    ├── with_task_stop()
    └── ... (40+ methods total)
```

### 依赖关系

```
tools/server.rs
├── 依赖: builtin_tools::* (所有内置工具)
├── 被依赖: executor, gateway, agent_loop
└── 公共 API: AlephToolServer builder 方法
```

---

## 🎨 目标架构

### 模块结构

```
core/src/tools/
├── server.rs              (~350行: 核心服务器)
│   ├── AlephToolServer (核心方法)
│   ├── AlephToolServerHandle
│   └── 工具管理逻辑
├── builtin.rs             (~741行: 内置工具注册)
│   └── impl AlephToolServer (with_* 方法)
└── types.rs               (~50行: 辅助类型)
    ├── ToolRepairInfo
    └── ToolUpdateInfo
```

### 职责划分

| 模块 | 职责 | 行数估算 |
|------|------|---------|
| **server.rs** | 核心工具服务器、工具管理 | ~350 |
| **builtin.rs** | 内置工具注册 builder 方法 | ~741 |
| **types.rs** | 辅助类型定义 | ~50 |

---

## 🔄 重构计划

### Phase 6.1: 提取辅助类型 (P0)

**目标**: 将辅助类型移至 `tools/types.rs`

**步骤**:
1. 创建 `tools/types.rs`
2. 提取 `ToolRepairInfo` 和 `ToolUpdateInfo`
3. 更新 `server.rs` 导入
4. 验证编译

### Phase 6.2: 提取内置工具注册 (P1)

**目标**: 将 `with_*` 方法移至 `tools/builtin.rs`

**步骤**:
1. 创建 `tools/builtin.rs`
2. 移动所有 `with_*` 方法到 `builtin.rs`
3. 在 `builtin.rs` 中实现 `impl AlephToolServer`
4. 更新 `tools/mod.rs` 添加 `mod builtin;`
5. 验证编译和测试

**关键点**:
- 保持 `impl AlephToolServer` 块，只是移到不同文件
- 所有 `with_*` 方法返回 `Self`，支持链式调用
- 需要导入所有 `builtin_tools::*`

**预计变更**:
- 新增文件: 2 个 (types.rs, builtin.rs)
- 修改文件: 2 个 (server.rs, mod.rs)
- server.rs: 1091 → ~350 行 (-741 行)

---

## 🧪 测试策略

### 单元测试

```bash
# 测试核心服务器
cargo test --package alephcore --lib tools::server

# 测试内置工具注册
cargo test --package alephcore --lib tools::builtin
```

### 集成测试

```bash
# 测试完整工具服务器
cargo test --package alephcore tools
```

### Builder 模式验证

确保以下调用方式仍然有效:
```rust
let server = AlephToolServer::new()
    .with_bash()
    .with_file_ops()
    .with_search()
    .with_web_fetch();
```

---

## 🔒 风险评估

### 高风险点

1. **impl 块分离**: 将 `impl AlephToolServer` 分到两个文件
   - **缓解**: Rust 允许多个 impl 块，只要在同一 crate 内

2. **导入依赖**: builtin.rs 需要导入所有 builtin_tools
   - **缓解**: 使用 `use crate::builtin_tools::*;`

### 低风险点

1. **类型提取**: 纯类型移动，不涉及逻辑
2. **向后兼容**: 保持公共 API 不变

---

## 📦 交付物

### Phase 6.1 (P0)

- [ ] `core/src/tools/types.rs`
- [ ] 更新 `core/src/tools/server.rs`
- [ ] 更新 `core/src/tools/mod.rs`
- [ ] 测试报告

### Phase 6.2 (P1)

- [ ] `core/src/tools/builtin.rs`
- [ ] 更新 `core/src/tools/server.rs`
- [ ] 更新 `core/src/tools/mod.rs`
- [ ] 测试报告

---

## 🔄 回滚计划

### 回滚触发条件

- 编译失败且无法在 30 分钟内修复
- 测试失败率 > 5%
- Builder 模式链式调用失效

### 回滚步骤

```bash
# 1. 恢复文件
git restore core/src/tools/

# 2. 验证
cargo check --package alephcore
```

---

## 📚 参考资料

- [Phase 4: POE Handlers 重构](./2026-02-08-poe-refactoring-phase4-design.md)
- [Phase 5: Plugins Handlers 重构](./2026-02-08-plugins-refactoring-phase5-design.md)

---

## ✅ 验收标准

### 功能验收

- [ ] 所有工具注册方法正常工作
- [ ] Builder 模式链式调用有效
- [ ] 工具调用正常

### 代码质量

- [ ] 编译 0 errors, 0 new warnings
- [ ] 所有测试通过 (100%)
- [ ] 文档更新完整

### 架构验收

- [ ] 核心服务器逻辑清晰
- [ ] 内置工具注册独立
- [ ] 模块依赖关系合理

---

**设计完成日期**: 2026-02-08
**审核状态**: 待审核
**下一步**: 开始实施 Phase 6.1 (P0) + 6.2 (P1)
