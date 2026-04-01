# Rust 层代码清理规划

> 日期: 2026-01-31
> 状态: 待实施
> 参考: OpenClaw (`/Volumes/TBU4/Workspace/openclaw/`)

## 背景

Aleph 项目经历了多次重构，Rust 层 (`core/src/`) 存在：
- 废弃代码（无引用模块）
- 重复代码（功能重叠模块）
- 死代码（未使用的类型/函数）

本规划按 `src/` 下的文件夹为单位进行逐个清理。

## 清理原则

1. **按依赖顺序** — 先清理被依赖少的，后清理被依赖多的
2. **保持编译通过** — 每个阶段完成后确保 `cargo check` 通过
3. **测试验证** — 每阶段运行 `cargo test` 确保功能正常
4. **参考 OpenClaw** — 架构决策参考 OpenClaw 实现

## 模块分析总结

### 核心执行路径 (保留)

```
Gateway → ExecutionEngine → AgentLoop → Thinker → Providers
                         ↓
                   Dispatcher → Executor (工具执行)
```

| 模块 | 行数 | 引用数 | 状态 |
|------|------|--------|------|
| `gateway/` | 31k | 26 | 核心，保留 |
| `agent_loop/` | 7.3k | 26 | 核心，保留 |
| `thinker/` | 3k | 4 | 核心，保留 |
| `providers/` | 10k | 25 | 核心，保留 |
| `config/` | 16k | 69 | 核心，保留 |

### 工具/执行层 (需重新设计)

| 模块 | 行数 | 引用数 | 状态 |
|------|------|--------|------|
| `dispatcher/` | 40k | 52 | 需重构 |
| `executor/` | 3.4k | 2 | 需重构 |
| `builtin_tools/` | 12k | 10 | 需重构 |
| `exec/` | 4.2k | 7 | 安全层，保留 |
| `tools/` | 2.5k | 30 | 需评估 |
| `mcp/` | 6k | 8 | 保留 |

### 删除/合并模块

| 模块 | 行数 | 原因 | 操作 |
|------|------|------|------|
| `services/` | ~500 | 0 引用 | 删除 |
| `checkpoint/` | ~300 | OpenClaw 无此功能 | 删除 |
| `three_layer/` | ~2000 | 参考 OpenClaw sandbox 重新实现 | 删除 |
| `thinking/` | ~1800 | 与 thinker 功能重叠 | 合并到 thinker |

### 保留但待集成

| 模块 | 行数 | 状态 |
|------|------|------|
| `suggestion/` | ~400 | 保留，待集成到 Gateway 流程 |
| `question/` | ~500 | 保留，待集成到 Gateway 流程 |

---

## Phase 1: 删除无引用模块

**风险**: 低
**预计删除**: ~800 行

### 1.1 删除 `services/`

```bash
# 检查确认无引用
grep -rn "use crate::services" src --include="*.rs" | grep -v "src/services"

# 删除模块
rm -rf src/services/

# 更新 lib.rs - 移除 mod services 声明

# 验证
cargo check
```

**清理内容**:
- `src/services/fs/` - 文件系统服务
- `src/services/git/` - Git 服务
- `src/services/system_info/` - 系统信息服务

### 1.2 删除 `checkpoint/`

```bash
# 检查确认无引用
grep -rn "use crate::checkpoint" src --include="*.rs" | grep -v "src/checkpoint"

# 删除模块
rm -rf src/checkpoint/

# 更新 lib.rs - 移除 mod checkpoint 声明和 pub use

# 验证
cargo check
```

**清理内容**:
- `CheckpointManager` - 检查点管理器
- `FileSnapshot` - 文件快照
- `CheckpointStorage` - 存储实现

---

## Phase 2: 合并重复模块

**风险**: 中
**操作**: 代码移动

### 2.1 合并 `thinking` 到 `thinker`

`thinking/streaming/` 包含 TTS 和流式输出所需的功能：
- `BlockReplyChunker` - 响应分块（对应 OpenClaw `EmbeddedBlockChunker`）
- `BlockCoalescer` - 块合并
- `StreamEvent` - 流事件类型
- `StreamSubscriber` - 流订阅器

**步骤**:

```bash
# 1. 移动 streaming 目录
mv src/thinking/streaming/ src/thinker/streaming/

# 2. 更新 thinker/mod.rs - 添加 pub mod streaming

# 3. 更新 lib.rs 导出路径
# 将 pub use crate::thinking::streaming::* 改为 pub use crate::thinker::streaming::*

# 4. 删除空的 thinking 目录
rm -rf src/thinking/

# 5. 验证
cargo check
cargo test
```

---

## Phase 3: 删除 three_layer

**风险**: 中
**预计删除**: ~2000 行

### 依赖分析

`three_layer` 被以下位置使用：
- `executor/builtin_registry/registry.rs` - 使用 `Capability`, `CapabilityGate`
- `executor/builtin_registry/mod.rs` - 使用 `CapabilityGate`

### 3.1 移除 CapabilityGate 依赖

```rust
// executor/builtin_registry/registry.rs
// 移除:
// use crate::three_layer::{Capability, CapabilityGate};

// 临时替换为简单实现或移除能力检查
// 未来参考 OpenClaw sandbox/tool-policy 重新实现
```

### 3.2 删除 three_layer

```bash
# 删除模块
rm -rf src/three_layer/

# 更新 lib.rs - 移除 mod 声明和 re-exports

# 验证
cargo check
```

**删除内容**:
- `three_layer/safety/` - Capability, CapabilityGate, PathSandbox, QuotaTracker
- `three_layer/orchestrator/` - OrchestratorState, GuardChecker (未使用)
- `three_layer/skill/` - SkillDefinition, SkillNode (未使用)

---

## Phase 4: 逐目录清理

按代码量从小到大，检查并清理各模块内部死代码。

### 4.1 `routing/` (1.3k 行)

- 检查 `SessionKey` 变体是否都有使用
- 移除未使用的路由逻辑

### 4.2 `permission/` (1.1k 行)

- 评估是否合并到 `exec/`
- 被 `dispatcher/executor` 使用，需与 Phase 5 协调

### 4.3 `suggestion/` (~400 行)

- **保留**，添加 TODO 注释标记待集成点
- 功能: 从 AI 回复解析后续建议

### 4.4 `question/` (~500 行)

- **保留**，添加 TODO 注释标记待集成点
- 功能: 结构化 Q&A 系统

### 4.5 `tools/` (2.5k 行)

- 检查与 `builtin_tools/` 的重复
- `AlephTool` trait 是核心，保留
- 移除重复实现

### 4.6 `thinker/` (3k 行)

- 合并 `thinking` 后整理
- 检查 `decision_parser.rs` 是否仍需要
- 检查 `tool_filter.rs` 与 dispatcher 是否重复

### 4.7 `executor/` (3.4k 行)

- **标记待重新设计**
- 添加 `// TODO: Phase 5 重构` 注释
- 不做大改动，等待 Phase 5

### 4.8 `capability/` (4.4k 行)

- 检查与已删除 `three_layer` 的功能重复
- 评估是否可以简化

### 4.9 `event/` (4.2k 行)

- 检查未使用的事件类型
- 保留 Gateway 使用的事件

### 4.10 `exec/` (4.2k 行)

- 核心安全层，仔细检查
- `ApprovalBridge`, `SecurityKernel` 是核心
- 检查未使用的审批类型

---

## Phase 5: 工具执行层重新设计 (Future)

**风险**: 高
**状态**: 独立 Milestone，不在本次清理中执行

### 当前架构问题

```
dispatcher (40k) + executor (3.4k) + builtin_tools (12k) = 55k+ 行
```

- `dispatcher` 过于庞大，职责不清
- `executor` 与 `dispatcher/executor` 命名混淆
- 工具分散在多个位置

### 目标架构 (参考 OpenClaw)

```
src/
├── tools/                    # 扁平化工具目录
│   ├── mod.rs               # 工具注册和组装
│   ├── policy.rs            # 工具策略 (allow/deny)
│   ├── bash.rs              # Bash 执行
│   ├── file_ops.rs          # 文件操作
│   ├── web_fetch.rs         # Web 获取
│   ├── browser.rs           # 浏览器控制
│   ├── canvas.rs            # Canvas 渲染
│   ├── sessions.rs          # Session 工具
│   └── ...
├── sandbox/                  # 沙箱系统 (参考 OpenClaw)
│   ├── config.rs            # 沙箱配置
│   ├── docker.rs            # Docker 隔离
│   └── context.rs           # 沙箱上下文
```

### OpenClaw 参考文件

- `src/agents/pi-tools.ts` - 工具组装中心
- `src/agents/pi-tools.policy.ts` - 工具策略
- `src/agents/sandbox/` - 沙箱系统
- `src/agents/bash-tools.ts` - Bash 执行
- `src/agents/tools/` - 各类工具实现

---

## 执行检查清单

### Phase 1 检查清单
- [ ] 确认 `services/` 无外部引用
- [ ] 删除 `services/`
- [ ] 更新 `lib.rs`
- [ ] `cargo check` 通过
- [ ] 确认 `checkpoint/` 无外部引用
- [ ] 删除 `checkpoint/`
- [ ] 更新 `lib.rs`
- [ ] `cargo check` 通过

### Phase 2 检查清单
- [ ] 移动 `thinking/streaming/` 到 `thinker/streaming/`
- [ ] 更新 `thinker/mod.rs`
- [ ] 更新 `lib.rs` 导出
- [ ] 删除 `thinking/`
- [ ] `cargo check` 通过
- [ ] `cargo test` 通过

### Phase 3 检查清单
- [ ] 移除 `executor/builtin_registry` 对 `CapabilityGate` 的依赖
- [ ] 删除 `three_layer/`
- [ ] 更新 `lib.rs`
- [ ] `cargo check` 通过
- [ ] `cargo test` 通过

### Phase 4 检查清单
- [ ] 清理 `routing/`
- [ ] 评估 `permission/`
- [ ] 标记 `suggestion/` 待集成
- [ ] 标记 `question/` 待集成
- [ ] 清理 `tools/`
- [ ] 整理 `thinker/`
- [ ] 标记 `executor/` 待重构
- [ ] 清理 `capability/`
- [ ] 清理 `event/`
- [ ] 检查 `exec/`

---

## 预期成果

| 指标 | 清理前 | 清理后 | 变化 |
|------|--------|--------|------|
| 目录数 | 54 | ~50 | -4 |
| 代码行数 | ~200k | ~195k | -5k |
| 编译警告 | 0 | 0 | 维持 |
| 测试通过 | Yes | Yes | 维持 |

## 后续计划

1. **Phase 5 单独规划** — 工具执行层重新设计需要独立设计文档
2. **OpenClaw 对齐** — 持续参考 OpenClaw 架构决策
3. **文档更新** — 清理完成后更新 CLAUDE.md 项目结构
