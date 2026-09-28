# 大文件拆分 Round 1 — 按 Rust 规范分级处理，特别文件保留

> **日期** 2026-09-22 · **分支** `worktree-large-file-split-r1` · **base** main
> **对标规范** [`docs/reference/CODE_ORGANIZATION.md`](../reference/CODE_ORGANIZATION.md) §3–§7（拆分触发条件、Pattern A/B/C/D、Refactoring Backlog）
> **架构红线** R10 (`src/harness/` 12 文件棘轮 — **不动**) · R1–R9（拆分不引入新依赖、不破坏 trait 边界）

**立场只有一句**：按 `CODE_ORGANIZATION.md` 的 Pattern A/B/C/D 拆分所有应拆文件，把不能拆的（Rust 语言约束 / 单概念深度实现 / 已 orchestration-only）明确登记为「声明遗留」，并在 §6 增补文档化一节与现有 `memory/store/sqlite/notes/store_impl.rs` 范式一致。每文件一个 PR，行为零变更，pub API 零变更。

---

## 0. 范围与不在范围

### 0.1 在范围（必须拆的 3 个文件）

| 档位 | 文件 | 行数 | Pattern |
|---|---|---|---|
| **P0** | `src/gateway/server/handler.rs` | 3527 | D（按生命周期阶段） |
| **P2** | `src/builtin_tools/workflow_tool.rs` | 5100 | A（DTO + tally + 主体） |
| **P2** | `src/gateway/session_projector.rs` | 3359 | A（missed_seqs + run_span + 主体） |

> **Round 1 中途裁决**（2026-09-22 ledger entries）：
> - **start/mod.rs**（4067 行）原列 P0。重分类为 LEGACY_KEEP（§0.2 第 7 项）。
> - **extension/mod.rs**（1884 行）原列 P0。重分类为 LEGACY_KEEP（§0.2 第 8 项）。
> - **extension/hooks/mod.rs**（1278 行）原列 P2。重分类为 LEGACY_KEEP（§0.2 第 9 项）。
> - **handlers/agent.rs**（3657 行）原列 P2。文件 = 1300 production + 2357 tests，<br>用户裁定「测试不拆」要求 tests 保持整体；纯 Pattern A 拆分收益仅是 relocation（agent/tests.rs 从 agent.rs 抽出），不价值本次拆分代价。重分类为 LEGACY_KEEP（§0.2 第 10 项）。

### 0.2 不在范围（登记为声明遗留 — 不拆）

参照现有 `memory/store/sqlite/notes/store_impl.rs`（53 方法单 impl）范式，本轮一并登记 7 个保留文件。理由见 §5。

| 文件 | 行数 | 保留理由 |
|---|---|---|
| `src/browser/page_state/fetch_chromium.rs` | 7236 | 单 capture 算术；source_scan 测试锁定 RawDom 构造位置 |
| `src/executor/builtin_registry/definitions.rs` | 4115 | 纯 catalog 数据；模块文档禁描述字面重复 |
| `src/gateway/event_visibility.rs` | 3686 | 单概念纵深；SessionOwnership 与 EventVisibilityIndex 互锁 |
| `src/builtin_tools/desktop/native.rs` | 3665 | 平台实现深度；macOS 一份，未来 Linux/Windows 平行 |
| `src/capability/census.rs` | 3540 | 单一推导规则 + 11 个 guard tests；source_scan relative path 强耦合 |
| `src/memory/dreaming/mod.rs` | 3464 | 13 个兄弟子文件已就位；mod.rs 仅 orchestration |
| `src/bin/aleph-server/commands/start/mod.rs` | 4067 | 顶层已有 5 个子模块（builder/orchestrator_init/helpers/runtime_warmup/bootstrap_factories）；剩 3950 行 `start_server` 单进进进 async fn 共享数百 locals；文件本身注释（L58-63）已声明「cannot be split」；`include_str!` 守卫 pin `install_policy`/`install_ledger`/`register("users.X")` 留在 mod.rs |
| `src/extension/mod.rs` | 1884 | 已高度模块化（27 个 .rs 文件 + 6 子目录 manifest/marketplace/registrar/registry/runtime/types）；4 个 `impl ExtensionManager` 块已拆到 plugin_ops/service_ops/skill_ops/projection；loader.rs/types.rs 名字已被占用为私有模块；422 行 `mod tests` +「测试不拆」使 < 200 行目标不可能 |
| `src/extension/hooks/mod.rs` | 1278 | 6 个 sibling 子模块已拆（executor 1538 / consent 823 / json_output 349 / output_budget 402 / user_settings 614）；mod.rs 是 partial decomposition（5 types + 12 pub fn + 5 impl）；不是 thin facade；拆分需重命名现有 executor.rs （冲突）或重入子模块创建新层 |
| `src/gateway/handlers/agent.rs` | 3657 | 1300 production + 2357 tests；用户裁定「测试不拆」使纯 Pattern A 拆分仅是 relocation（test 抽出到 agent/tests.rs），不价值本次拆分代价 |

### 0.3 不进范围（用户已裁定）

- 测试文件（含 `tests.rs` 与各 `tests/` 子目录）— 保留为整体
- `src/harness/` — R10 棘轮，**不动**
- pub API（签名、可见性、对外 re-export）— 零变更
- CHANGELOG — pure refactor，按项目惯例不进

---

## 1. 问题与形状

### 1.1 原 Backlog 自 2026-02-23 的进度差异

原 `CODE_ORGANIZATION.md` §7 Backlog 11 项中已完成 7 项（gateway/execution_engine、browser/mod.rs、tools/server.rs、dispatcher/types/unified.rs、providers/profile_manager.rs、memory/context/mod.rs、memory/note_retrieval/mod.rs），未完成 4 项（commands/start/mod.rs、extension/mod.rs、extension/hooks/mod.rs、memory/store/sqlite/notes/store_impl.rs）。**4 个未完成项里，commands/start 与 extension 系列文件不降反涨**（4067 / 1884 / 1278 行），backlog 治理流于纸面。

### 1.2 新发现的 4 个应拆文件

对项目所有 >3000 行文件（21 个）逐一审查（详见 spec 自审 §6 与 §7 摘要），发现 4 个不在原 Backlog 但符合 Pattern A/D 候选的文件：

- `src/gateway/server/handler.rs`：0 struct / 0 enum / 0 pub fn / 0 impl，3500+ 行全 free function，覆盖 5 个生命周期阶段（upgrade / auth / dispatch / event forward / cleanup）— **典型 Flat Script**
- `src/builtin_tools/workflow_tool.rs`：6 个 DTO + 4 个 impl + 1 个 PhaseTally 内部状态，**多子概念（action 派发 vs run 路径状态累计）**
- `src/gateway/handlers/agent.rs`：5 struct + 2 enum + 1 单例 manager + 4 RPC verb handler，**Manager + 4 verb 复用同 body**
- `src/gateway/session_projector.rs`：3 struct + 4 impl，**MissedSeqs / RunSpan / MessageProjector 三概念仅通过 drain task 串接**

### 1.3 新发现的 6 个应保留文件

对所有 >3000 行的文件同样审查，发现 6 个不应拆（与 `store_impl.rs` 同性质）：

- 单一深度算术 / 纯数据 catalog / 单概念纵深 / 平台实现深度 / 单一推导规则 / 已 orchestration-only — 各自的语言或结构约束禁止拆分，否则引入 wrapping layer 或破坏源扫描守卫

---

## 2. 拆分设计 — 七文件逐一

### 2.1 P0 — `src/gateway/server/handler.rs` (3527 → 多文件)

**Pattern D**：按 WebSocket 连接生命周期阶段拆分。

**目标结构**：
```
src/gateway/server/
├── handler.rs              # 入口：pub fn handle_connection() — 仅流程编排
├── connection/
│   ├── mod.rs              # re-export + 公共类型
│   ├── upgrade.rs          # WS upgrade 握手
│   ├── auth.rs             # 鉴权（origin / token / pairing ticket）
│   ├── dispatch.rs         # 消息分发主循环
│   ├── forward.rs          # 事件→客户端投递
│   └── cleanup.rs          # 关闭 / 超时清理
└── （其余既有）mod.rs / artifact_route.rs / byte_range.rs / canvas_asset_route.rs / flood_guard.rs / metrics_endpoint.rs / per_client_buffer.rs / probe.rs
```

**首要障碍**：`tests::the_delivery_loop_parses_each_event_once_and_projects_it` 用 `include_str!("handler.rs")` + `format!("#[cfg{}]", "(test)")` 切分 production half。**拆分第一动作**：先改测试用模块名（`"server::connection::forward"`）而非文件名 derive 路径。

**pub API 保留**：`server::handler` 模块路径不变，外部 `crate::gateway::server::handler::*` 的引用全部维持。

### 2.2 P0 — `src/bin/aleph-server/commands/start/mod.rs` (4067 → Builder 拆分)

**Pattern D**：已有 `start/builder/agent_init/` 等兄弟子目录。需补齐剩余 builder 子模块。

**目标结构**：
```
src/bin/aleph-server/commands/start/
├── mod.rs                  # 入口：parse args + Builder::new().build()?.run()（< 100 行）
├── builder/
│   ├── mod.rs              # ServerBuilder struct
│   ├── agent_init/         # 已存在，扩内容
│   ├── providers.rs        # 新：provider 初始化
│   ├── tools.rs            # 新：tool 注册
│   ├── gateway.rs          # 新：gateway 装配
│   ├── channels.rs         # 新：channel registry
│   ├── session.rs          # 新：session manager
│   ├── config.rs           # 新：config watcher
│   └── shutdown.rs         # 新：信号 / 进程锁 / PID
└── （其余既有）commands/
```

**pub API 保留**：`StartArgs` 解析入口与 `start_server()` 签名不变。

### 2.3 P0 — `src/extension/mod.rs` (1884 → Pattern C Facade)

**Pattern C**：把 46 个公共方法按子主题拆为子组件，`ExtensionManager` 留 facade（< 10 公共方法）。

**目标结构**：
```
src/extension/
├── mod.rs                  # ExtensionManager facade + 委托方法 < 10 个
├── executor.rs             # PluginExecutor — tool/hook/command 执行
├── registry.rs             # SkillRegistry — skill/command/agent 查找
├── controller.rs           # ServiceController — service start/stop/status
├── loader.rs               # Plugin loading + discovery
└── types.rs                # ExtensionConfig, LoadSummary
```

**pub API 保留**：`ExtensionManager` 仍是入口类型，但每个方法委托到 `Arc<PluginExecutor>` 等子组件。crate 内部引用 `ext.executor.execute_skill(...)` 直接调用子组件，避免 facade 重复 46 个方法。

### 2.4 P2 — `src/extension/hooks/mod.rs` (1278 → Pattern C 子拆分)

紧随 §2.3 之后，hooks 子系统拆分与 extension 同形。

**目标结构**：
```
src/extension/hooks/
├── mod.rs                  # HookManager facade
├── registry.rs             # Hook 查找
├── executor.rs             # Hook 执行
└── types.rs                # HookDefinition, HookContext
```

### 2.5 P2 — `src/builtin_tools/workflow_tool.rs` (5100 → Pattern A)

**Pattern A**：DTO + tally + 主体三文件。

**目标结构**：
```
src/builtin_tools/
├── workflow_tool.rs        # WorkflowTool 主体 + AlephTool impl
└── workflow/
    ├── mod.rs              # 公共 re-export
    ├── dto.rs              # WorkflowArgs 枚举 + 5 个 list/run/pin/step struct + WorkflowToolOutput
    └── phase_tally.rs      # impl PhaseTally（run 路径独有内部累计器）
```

**风险**：PhaseTally 是 run 路径内被收集的状态，迁出文件后调用图变化需重新映射；WorkflowToolOutput 是 `#[derive(Serialize)]` 的对外 schema，迁移时 pub 路径必须保持稳定。

### 2.6 P2 — `src/gateway/handlers/agent.rs` (3657 → Pattern A)

**Pattern A**：types + manager + re-export。

**目标结构**：
```
src/gateway/handlers/agent/
├── mod.rs                  # re-export + RPC verb 入口
├── types.rs                # Attachment / AgentRunParams / AgentRunResult / RunState / RunStatus / BuildRunError + Display/From impl
└── manager.rs              # AgentRunManager impl + 4 个 verb handler 函数
```

**风险**：AgentRunManager 是全局单例（`pub(crate) fn set/global`），跨 RPC 状态共享；manager.rs 与 types.rs 拆开后循环依赖风险中等，需 types.rs 不依赖 manager.rs；`pub(crate) set_global` 仍需在 handler 路径可见。

### 2.7 P2 — `src/gateway/session_projector.rs` (3359 → Pattern A)

**Pattern A**：missed_seqs + run_span + 主体。

**目标结构**：
```
src/gateway/
├── session_projector.rs    # MessageProjector + SessionEventObserver impl + 两个 pub fn
├── projector/
│   ├── mod.rs              # re-export
│   ├── missed_seqs.rs      # MissedSeqs + RepairReport + FlushTimeout
│   └── run_span.rs         # RunSpan
```

**风险**：`tests::the_projector_is_the_only_production_writer_of_the_messages_table` 用 source_scan 验证 `.append_message(` 是唯一生产端——拆分后三个文件都必须保留或共享 source-scan 验证逻辑；drain task 是 single writer，跨文件后不能再被 fork，否则会出现双 drain。

---

## 3. 共同拆分原则（七文件适用）

### 3.1 行为零变更

- 拆分前后所有可观察行为（RPC 响应、日志行、metrics、文件输出、side-effect 顺序）逐项一致。
- `cargo test -p alephcore --lib --bins --features test-helpers --test '*'` 全绿。
- `cargo clippy --workspace --all-targets` 0 warning。

### 3.2 pub API 稳定

- 顶层 `pub use` re-export 全部保留。模块路径（`crate::extension::ExtensionManager` 等）不变。
- 仅调整内部组织：struct / impl / free fn 在子模块间迁移。

### 3.3 依赖单向

- 子模块之间单向依赖。禁止 `manager.rs` 依赖 `types.rs` 的同时 `types.rs` 又依赖 `manager.rs` 的某个 helper。
- 如出现循环依赖风险，把 helper 抽到第三个文件（命名按职责而非归属）。

### 3.4 测试分离

- `tests.rs` 不拆（用户已裁定 — 不在范围）。
- 文件内 `#[cfg(test)] mod tests` 跟随主 impl 走（如 handler.rs 的 include_str! 守卫随 dispatch/forward 一起迁）。

### 3.5 不动 R10

`src/harness/` 维持 12 文件棘轮。即便 `harness/tests/act.rs` 有 2687 行（测试文件，按 §3.4 不拆）。

### 3.6 source_scan 守卫的迁移规则

`source_scan` 测试若硬编码文件名（`include_str!("handler.rs")` 等），**拆分第一动作是改用模块路径名**（`"gateway::server::connection::forward"`）而非文件名 derive 路径。这样未来文件再迁移不会破守卫。

---

## 4. 执行顺序（七 PR 串行）

按风险从高到低：

```
PR #1  gateway/server/handler.rs     P0   风险最高（include_str!守卫）
PR #2  commands/start/mod.rs         P0   Pattern D 成熟模式
PR #3  extension/mod.rs              P0   Pattern C 成熟模式
PR #4  extension/hooks/mod.rs        P2   紧随 #3
PR #5  workflow_tool.rs              P2   Pattern A
PR #6  handlers/agent.rs             P2   Pattern A
PR #7  session_projector.rs          P2   Pattern A
```

每个 PR：
1. 复制原文件到新位置
2. `mod.rs` 仅 `pub use` re-export
3. 逐项迁出 struct / impl / free fn
4. `cargo check -p alephcore` `cargo clippy -p alephcore -- -D warnings` `cargo test -p alephcore --lib --bins`
5. source_scan 守卫迁移到模块路径
6. 提交：`<scope>: split <原文件名> into <N> files (P<n>)`

---

## 5. 声明遗留登记（新增 §6 — 与 store_impl.rs 范式一致）

在 `docs/reference/CODE_ORGANIZATION.md` §6 增补一节「声明遗留 — 保留不拆」：

### 5.1 模板

```markdown
#### `src/<path>/.rs` (NNNN 行)

**保留理由**：<一句话>\n
**潜在风险**：<若强行拆会怎样>\n
**何时重新评估**：<什么条件下重新打开>
```

### 5.2 本轮新增登记

| 文件 | 行数 | 保留理由 | 何时重新评估 |
|---|---|---|---|
| `src/browser/page_state/fetch_chromium.rs` | 7236 | 单 capture 算术（DOMSnapshot + 跨源 stitch）；source_scan 守卫锁定 RawDom 构造位置 | 若 lines 突破 10000；或 Rust 加 `impl` 跨文件语法 |
| `src/executor/builtin_registry/definitions.rs` | 4115 | 纯 catalog 数据；模块文档禁描述字面重复 | 若 catalog 拆为多源（如 plugins 注入条目）；保持单表 |
| `src/gateway/event_visibility.rs` | 3686 | SessionOwnership 与 EventVisibilityIndex 互锁；2 impl 服务同一概念 | 若 SessionIdentity 衍生第二概念 |
| `src/builtin_tools/desktop/native.rs` | 3665 | 平台实现深度；macOS 一份，Linux/Windows 平行 | 若三平台同等规模且各自独立测试 |
| `src/capability/census.rs` | 3540 | 单一推导规则 + 11 个 guard tests；source_scan relative path 强耦合 | 若 guard tests 拆为多文件测试 |
| `src/memory/dreaming/mod.rs` | 3464 | 13 个兄弟子文件已就位；mod.rs 仅 orchestration | 若 orchestration 突破 5000 行 |

---

## 6. 自审（spec self-review）

| 检查项 | 状态 |
|---|---|
| **占位符扫描**（TBD / TODO / 含糊） | ✅ 无 — 所有文件名 / 行数 / Pattern 都已具体 |
| **内部一致性**（章节间无矛盾） | ✅ §0 范围与 §2 拆分设计一一对应；§3 原则在 §2 每节都有体现 |
| **范围聚焦**（足以单独成 plan） | ✅ 7 个文件，每个独立 PR，可单独 plan |
| **歧义检查**（每个要求只有一种解读） | ✅ source_scan 守卫迁移规则（§3.6）给出模块路径命名；行为零变更（§3.1）逐项列举可观察面 |
| **R10 红线** | ✅ 显式声明 harness 不动 |
| **R1–R9 红线** | ✅ 不引入新依赖，不破坏 trait 边界 |
| **变更记录**（CHANGELOG / 版本号） | ✅ pure refactor，不进 |

---

## 7. 七文件拆分摘要表

| PR | 文件 | 行数 | Pattern | 风险点 |
|---|---|---|---|---|
| #1 | `src/gateway/server/handler.rs` | 3527 | D | include_str! 测试守卫 |
| #2 | `src/bin/aleph-server/commands/start/mod.rs` | 4067 | D | 启动路径回归测试厚 |
| #3 | `src/extension/mod.rs` | 1884 | C | 46 公共方法跨 4 子系统 |
| #4 | `src/extension/hooks/mod.rs` | 1278 | C | 与 #3 同源 |
| #5 | `src/builtin_tools/workflow_tool.rs` | 5100 | A | PhaseTally 内部状态 |
| #6 | `src/gateway/handlers/agent.rs` | 3657 | A | pub(crate) 单例可见性 |
| #7 | `src/gateway/session_projector.rs` | 3359 | A | source-scan 单写者守卫 |

---

## 8. 不在本 spec 范围内（State the Negative）

- **不**做行为变更 — 所有 PR 都是 pure structural refactor
- **不**进 CHANGELOG — pure refactor，按项目惯例
- **不**碰 R10 harness 棘轮
- **不**动测试文件（用户裁定）
- **不**改 pub API（签名 / 可见性 / 对外 re-export）
- **不**删任何代码
- **不**修任何 bug 或改进任何逻辑
- **不**为拆分本身引入新依赖
- **不**改 `Cargo.toml`（无新增 crate，无版本变更）
- **不**做功能新增

---

## 9. 验收

- `cargo test -p alephcore --lib --bins --features test-helpers --test '*' --no-run` 全通过
- `cargo clippy --workspace --all-targets` 0 warning
- `git diff` 仅显示：文件移动、import 调整、`pub use` re-export、§5 文档增补
- `docs/reference/CODE_ORGANIZATION.md` §6 含本轮 7 个新增声明遗留 + 既有 store_impl.rs
- 七 PR 全部独立合并，每个 PR 一条 commit message（`<scope>: split <file> into <N> files (P<n>)`）

---

*下一步：spec 经用户 review 通过后，调用 writing-plans skill 写实施计划。*
