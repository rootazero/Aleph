# Phase 5: Plugins Handlers 重构设计

**日期**: 2026-02-08
**状态**: 设计中
**优先级**: P1 (架构审计 Phase 5)

---

## 📋 背景

根据架构审计，`core/src/gateway/handlers/plugins.rs` (1077行) 存在以下问题：

1. **类型定义分散**: 8 个参数类型散落在 handler 文件中
2. **Handler 职责过重**: 10 个 RPC handler 包含参数解析、业务逻辑调用、错误处理等多重职责
3. **业务逻辑混合**: Handler 中包含大量 ExtensionManager 调用和错误处理逻辑

**模式参考**: 与 Phase 4 (poe.rs) 类似，可以应用相同的重构模式。

---

## 🎯 设计目标

### 核心原则

1. **类型独立**: 将参数类型提取到独立模块，提高复用性
2. **Handler 瘦身**: Handler 只负责 RPC 协议处理，不包含复杂业务逻辑
3. **向后兼容**: 保持现有 API 不变，所有测试继续通过

### 非目标

- **不创建新的服务层**: ExtensionManager 已经是服务层，不需要额外抽象
- **不改变业务逻辑**: 只移动代码位置，不修改实现细节

---

## 📊 当前状态分析

### 文件结构

```
core/src/gateway/handlers/plugins.rs (1077行)
├── 全局状态 (~40行)
│   └── EXTENSION_MANAGER (OnceCell)
├── 参数类型 (~200行)
│   ├── InstallParams
│   ├── InstallFromZipParams
│   ├── UninstallParams
│   ├── ToggleParams
│   ├── CallToolParams
│   ├── LoadPluginParams
│   ├── UnloadPluginParams
│   └── ExecuteCommandParams
├── 辅助类型 (~50行)
│   └── PluginInfoJson
└── RPC Handlers (~787行)
    ├── handle_list (60 lines)
    ├── handle_install (120 lines)
    ├── handle_install_from_zip (100 lines)
    ├── handle_uninstall (80 lines)
    ├── handle_enable (70 lines)
    ├── handle_disable (70 lines)
    ├── handle_call_tool (150 lines)
    ├── handle_execute_command (80 lines)
    ├── handle_load (40 lines)
    └── handle_unload (40 lines)
```

### 依赖关系

```
handlers/plugins.rs
├── 依赖: extension::ExtensionManager
├── 被依赖: gateway/router.rs (注册 RPC handlers)
└── 全局状态: EXTENSION_MANAGER (OnceCell)
```

---

## 🎨 目标架构

### 模块结构

```
core/src/gateway/handlers/
├── plugins.rs                (~300行: RPC handlers + 全局状态)
└── plugins/
    ├── mod.rs                # 模块定义
    └── types.rs              # 参数和结果类型 (~250行)
```

### 职责划分

| 模块 | 职责 | 行数估算 |
|------|------|---------|
| **plugins/types.rs** | 参数类型、辅助类型定义 | ~250 |
| **plugins.rs** | RPC 协议处理、ExtensionManager 调用 | ~300 |

---

## 🔄 重构计划

### Phase 5.1: 类型提取 (P0)

**目标**: 将类型定义移至 `gateway/handlers/plugins/types.rs`

**步骤**:
1. 创建 `gateway/handlers/plugins/` 目录结构
2. 创建 `types.rs` 并提取所有参数类型：
   - InstallParams
   - InstallFromZipParams
   - UninstallParams
   - ToggleParams
   - CallToolParams
   - LoadPluginParams
   - UnloadPluginParams
   - ExecuteCommandParams
   - PluginInfoJson
3. 创建 `mod.rs` 重导出类型
4. 更新 `plugins.rs` 的导入语句
5. 验证编译和测试

**预计变更**:
- 新增文件: 2 个 (plugins/mod.rs, plugins/types.rs)
- 修改文件: 1 个 (plugins.rs)
- 删除代码: ~250 行 (从 plugins.rs)
- 新增代码: ~270 行 (types/ + 导入语句)

**验证标准**:
- ✅ 编译通过 (0 errors)
- ✅ 所有 plugins 相关测试通过
- ✅ 类型可以从 `super::plugins::types` 导入

---

## 🧪 测试策略

### 单元测试

```bash
# 测试 handlers
cargo test --package alephcore --lib gateway::handlers::plugins
```

### 集成测试

```bash
# 测试完整 plugins 流程
cargo test --package alephcore plugins
```

### 回归测试

```bash
# 运行所有测试
cargo test --workspace
```

---

## 🔒 风险评估

### 高风险点

1. **全局状态**: EXTENSION_MANAGER 是全局 OnceCell，需要保持在 plugins.rs
   - **缓解**: 不移动全局状态，只提取类型

2. **错误处理**: Handler 中有大量错误处理逻辑
   - **缓解**: 保持错误处理在 handler 中，只提取类型定义

### 低风险点

1. **类型提取**: 纯类型移动，不涉及逻辑变更
2. **向后兼容**: 保持公共 API 不变

---

## 📦 交付物

### Phase 5.1 (P0)

- [ ] `core/src/gateway/handlers/plugins/mod.rs`
- [ ] `core/src/gateway/handlers/plugins/types.rs`
- [ ] 更新 `core/src/gateway/handlers/plugins.rs`
- [ ] 测试报告

---

## 🔄 回滚计划

### 回滚触发条件

- 编译失败且无法在 30 分钟内修复
- 测试失败率 > 5%
- 发现严重的架构问题

### 回滚步骤

```bash
# 1. 恢复文件
git restore core/src/gateway/handlers/plugins.rs
git clean -fd core/src/gateway/handlers/plugins/

# 2. 验证
cargo check --package alephcore
```

---

## 📚 参考资料

- [Phase 4: POE Handlers 重构设计](./2026-02-08-poe-refactoring-phase4-design.md)
- [Phase 1: Types 重构设计](./2026-02-07-types-refactoring-phase1-design.md)

---

## ✅ 验收标准

### 功能验收

- [ ] 所有 plugins RPC 方法正常工作
- [ ] 插件安装/卸载流程完整
- [ ] 工具调用正常

### 代码质量

- [ ] 编译 0 errors, 0 new warnings
- [ ] 所有测试通过 (100%)
- [ ] 文档更新完整

### 架构验收

- [ ] 类型定义独立可复用
- [ ] Handler 只负责协议处理
- [ ] 模块依赖关系合理

---

**设计完成日期**: 2026-02-08
**审核状态**: 待审核
**下一步**: 开始实施 Phase 5.1 (P0)
