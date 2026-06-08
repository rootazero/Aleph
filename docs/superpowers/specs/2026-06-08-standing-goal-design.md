# Standing Goal 子系统设计

- **日期**: 2026-06-08
- **分支**: `feat/standing-goal`（worktree 隔离，不碰 main）
- **参考项目**: openclaw（`SessionGoal` + `/goal` 工具/命令）、hermes-agent（`goals.py` Ralph loop）、Pi / opensquilla（无 goal 概念）
- **状态**: 设计已批准，待落 plan

## 1. 背景与目标

### 1.1 问题

Aleph 当前只有**轮内**目标推进能力：模型用 `scratchpad` 工具写下 objective + plan checklist，`ScratchpadGoalVerifier` 在模型想 stop 时检查是否还有未勾选项（`- [ ]`），有则否决 stop、逼模型继续。这是一个**单轮**结构看门狗。

缺的是参考项目都有的**常驻目标（standing goal）**层：

- **持久实体**：一个跨轮、跨 session（存活 `/resume`）的用户目标，带生命周期状态与预算。
- **生命周期 / 预算**：active/paused/blocked/complete + token 预算护栏。
- **工具/命令面**：用自然语言创建、查询、更新、清除目标（Aleph 的 R8「一切皆工具」）。
- **跨轮自主推进**：目标未达成时，跨多个轮次持续推进（hermes 的 Ralph loop）。

### 1.2 Gap Analysis（参考项目 ↔ Aleph）

| 能力 | openclaw | hermes-agent | Pi / opensquilla | Aleph（现状） |
|---|---|---|---|---|
| 目标作为持久实体 | ✅ `SessionGoal`（active/paused/blocked/complete + token_budget） | ✅ `goal:<session_id>` 存 SessionDB，存活 `/resume` | ❌ 无 | ⚠️ 仅轮内 scratchpad checklist |
| 用户/LLM 目标面 | ✅ `/goal` 命令 + `get/create/update_goal` 工具 | ✅ `/goal`、`/subgoal`、pause/clear | — | ❌ 无 |
| 跨轮自主推进 | ❌（被动跟踪） | ✅ Ralph loop：每轮重注入 continuation prompt 直到完成/预算耗尽 | — | ⚠️ `ScratchpadGoalVerifier` 仅**轮内** |
| 完成判定 | 模型设状态 | **独立判官 LLM** 每轮调用 | — | 模型自身 prompt 内 `VERDICT:`（R7/R10 禁判官） |
| 预算护栏 | 每目标 token_budget | `max_turns` 兜底 | — | `MAX_VERIFIER_VETOS`（仅轮内） |

### 1.3 架构红线约束（不可违反）

- **R7 LLM 主权**：永禁判官 LLM / POE 目标验证管线。完成判定属于模型（prompt 内），不得用确定性代码替代。hermes 的判官 LLM 在 Aleph 是红线。
- **R10 薄 Harness / 笨循环**：`src/harness/` 不得增加认知；12 文件红线。跨轮推进的「续跑闸门」必须放在 harness 之外。
- **R8 一切皆工具**：目标管理经 `goal` 工具用自然语言完成，不引入新配置文件语法。
- **R5 AI 主动到达 + 不打扰**：自主推进可取，但默认不打扰用户、不抢焦点。
- **R3 核心轻量化 / P6 YAGNI**：能复用就不新增；零消费者抽象立即撤回。

## 2. 关键复用发现

Aleph **已有**自主触发 agent 的基础设施——`src/tasks/cron/`。`build_cron_executor_fn` → `execute_cron_job`（`src/tasks/cron/executor.rs`）通过 `ExecutionAdapter` + `RunRequest`（带 `max_iterations_override` 上限）针对某 `SessionTarget` 跑 agent，用 `prompt` / `prompt_template`（`config.rs:361/408`）注入，再经 `DeliveryEngine`（Gateway/Webhook/Memory 多端，`tasks/shared/delivery`）投递；并支持 chain（`chain.rs::trigger_chain_job`）。

**结论**：hermes 的「Ralph loop」在 Aleph 的现成落点就是 cron executor——它就是 turn-injector。Layer 2（跨轮主动推进）从「净新基建」降级为「连线 cron」，符合「连线优先」。

## 3. 架构总览

```
用户/LLM ──goal 工具(R8)──▶ GoalStore (新, 小)
                                │ objective+status+budget, session 域, 存活 resume
                                ▼
        ┌──────── 推进机制（复用现有接缝）─────────┐
        │ 轮内:   ScratchpadGoalVerifier  (零改动)   │
        │ 跨轮被动: prompt 注入 active goal (复用)    │
        │ 跨轮主动(opt-in): cron executor 续跑 (复用) │
        └─────────────────────────────────────────┘
            完成判定 = 模型自己 goal(update,complete)  ← R7/R10 无判官
            兜底     = token_budget + max_iterations 结构上限
```

| 角色 | 复用/新增 | 落点 |
|---|---|---|
| 标准目标实体+持久化 | **新**（小） | `src/goal/{mod,types,store}.rs`，`open_sqlite_safe` session 域表 |
| 自然语言管理目标 | **新**（小，R8） | `src/builtin_tools/goal.rs` + core_tools/constructor 注册 |
| 轮内推进 | **复用，零改** | `src/verification/scratchpad_goal_verifier.rs` |
| 跨轮被动注入 | **复用** | prompt/context 组装处加 1 个注入块 |
| 跨轮主动续跑 | **复用 cron** | `src/tasks/cron` executor + `RunRequest` + `DeliveryEngine` |
| 续跑闸门（轮末观察） | **复用，不进 harness** | gateway 执行层 |

## 4. 组件详细设计

### 4.1 Goal 实体 & GoalStore（新，最小）

```rust
// src/goal/types.rs
pub enum GoalStatus { Active, Paused, Blocked, Complete }

pub enum PursuitMode {
    Passive,                          // 默认：仅作常驻上下文
    Active { max_iterations: u32 },   // opt-in：经 cron 自主续跑
}

pub struct Goal {
    pub id: String,
    pub session_id: String,
    pub objective: String,
    pub status: GoalStatus,
    pub token_budget: Option<u64>,    // openclaw 风格软护栏
    pub tokens_at_start: u64,
    pub pursuit: PursuitMode,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub note: Option<String>,
}
```

- **不可变更新**（CLAUDE.md 编码风格）：`with_status()` / `with_note()` 返回新副本，不就地改。
- **持久化**：`src/goal/store.rs` 经 `open_sqlite_safe`（Spec C 进程安全模式）建 session 域表 `goals`。session 域、存活 `/resume`，对齐 openclaw/hermes。
- **一 session 一 active goal**（首版约束）：`set` 替换已有 active goal（对齐 openclaw 单 SessionGoal 语义），简化生命周期。
- **刻意不复用 scratchpad**：scratchpad = 模型「当前任务怎么执行」的工作记忆（agent 域 markdown，`src/memory/scratchpad/`）；goal = 用户「要达成什么」的常驻目标（可跨多个 scratchpad 周期）。二者层级不同，不应合并。

### 4.2 `goal` 工具（R8，新）

`src/builtin_tools/goal.rs`，实现 `AlephTool`（参照 `ScratchpadTool` 范式）：

| action | 语义 | 谁能调 |
|---|---|---|
| `set` | 创建/替换常驻目标（**仅用户显式要求时**，对齐 openclaw "Create only when explicitly requested"） | 用户驱动 |
| `get` | 返回当前目标：objective、status、token 用量/预算 | 任意 |
| `update` | 设状态 `complete` / `blocked`（模型只能这两个）/ `paused` / `active` + note | 模型可标 complete/blocked |
| `clear` | 清除当前目标 | 用户驱动 |

- 工具绑定 active session（参照 `scratchpad_registry::set_active` 的 `Arc<RwLock<String>>` 范式）。
- 注册：`src/executor/builtin_registry/builder/core_tools.rs` 加 `reg(tools, "goal", GoalTool::DESCRIPTION, schema::<GoalArgs>("goal"))`；`constructor.rs` 构造 `GoalTool::new(store)`。

### 4.3 推进机制

1. **轮内（复用，零改）**：`ScratchpadGoalVerifier` 不动——模型 checklist 没勾完不让 stop。
2. **跨轮被动（默认）**：每轮组装 context 时，若存在 `Active` goal，注入一段紧凑文本：
   ```
   Standing goal: <objective>
   (status=active, tokens 1234/5000)
   ```
   模型自然持续推进。**零判官、零新轮触发**。注入点在 prompt/context 组装层（如 `src/harness/agent/prompt.rs` 或 thinker 层），单一注入块。
3. **跨轮主动（opt-in，默认关）**：当 goal 为 `Active { max_iterations }` 时：
   - 轮末闸门在 **gateway 执行层**（cron executor 同层，**不进 `src/harness/`**）读 goal 状态——纯结构判断。
   - 若 goal 仍 `Active` 且未撞预算 → 经 cron executor 对**同一 session** 续跑一个 continuation prompt（"Continue pursuing your standing goal: …"）。
   - 终止条件（任一）：模型调 `goal(update, complete)`（R7 主权，模型显式信号）、撞 `token_budget`、撞 `max_iterations`。后两者为结构兜底，对齐 hermes `max_turns`。
   - 投递走现成 `DeliveryEngine`，不抢焦点（R5 不打扰）。

#### R7/R10 合规论证

- 完成判定**永远**是模型的显式 `goal(update, complete)` 调用，不是任何 Rust 代码的语义判断。
- 续跑闸门只读 `GoalStatus` 枚举 + 比较 token 计数/迭代数——与 `ScratchpadGoalVerifier` 读 checkbox、`StopHookVerifier` 读 exit code 同形，是结构信号、零 LLM 调用，通过 R10「面向未来测试」。
- 闸门**不在** `src/harness/`，守 12 文件红线。

## 5. 数据流

### 5.1 创建 + 被动推进（默认路径）
```
用户「帮我把整个 auth 模块迁移到新 API」
 → 模型 goal(set, objective="迁移 auth 到新 API")
 → GoalStore 写入 Active/Passive, tokens_at_start=当前
 → 每轮 prompt 注入 "Standing goal: 迁移 auth …(tokens X/—)"
 → 模型用 scratchpad 分解 + 逐步执行（轮内 verifier 守 checklist）
 → 完成时模型 goal(update, complete) → GoalStore 标 Complete → 停止注入
```

### 5.2 主动续跑（opt-in）
```
用户「持续推进直到完成，预算 5 万 token」
 → 模型 goal(set, …, pursuit=Active{max_iterations=N}, token_budget=50000)
 → 轮末 gateway 闸门: goal.Active && tokens<50000 && iter<N?
     是 → cron executor 对同 session 续跑 continuation prompt
     否 → 停（complete / 撞预算 / 撞迭代）→ DeliveryEngine 投递结果
```

## 6. 错误处理 & 防御性设计（P7）

- GoalStore SQLite 损坏：`open_sqlite_safe` 已处理；读失败 fail-safe 视为「无 goal」（被动注入跳过，不 panic）。
- 续跑闸门读 goal 失败：**fail-closed = 停止续跑**。与 hermes「判官失败 fail-open（继续）」相反——hermes 的兜底是 turn budget，空转代价低；Aleph 主动续跑直接烧 token 预算，读不到状态时应停而非空转。
- 锁中毒：`.lock().unwrap_or_else(|e| e.into_inner())`（P7）。
- UTF-8：objective 注入截断用 `char_indices()`，不用 `&s[..n]`。
- 预算计数溢出：`saturating_sub`。

## 7. 测试计划

- **单元**：GoalStore CRUD + 不可变更新；`with_status` 状态机；token 用量计算；PursuitMode 序列化。
- **集成**：`goal` 工具四 action 端到端（含「模型只能 complete/blocked」门控）；被动注入在 Active 时出现、Complete 后消失。
- **续跑闸门**：Active+预算内 → 续跑；complete/撞预算/撞迭代 → 停（用假 cron executor / 注入桩）。
- **回归**：`ScratchpadGoalVerifier` 行为不变。
- 目标覆盖率 ≥ 80%（新增模块）。

## 8. 熵减 / 清理点

- 更新 `src/verification/scratchpad_goal_verifier.rs` 顶部 doc 注释：它现在只是**轮内**补充，跨轮归 goal 子系统（文档准确性=熵减；当前注释声称「闭合 hermes goals.py gap」已不全面）。
- 落地前 grep 确认无半成品 goal 脚手架（现状仅该 verifier + `components/types/context.rs` 一处无关引用，干净）。

## 9. 不做（YAGNI 边界）

- ❌ 判官 LLM / POE 目标验证管线（R7 永禁）。
- ❌ Panel 目标面板（参考方案三，过早前端，留后续独立 spec）。
- ❌ `/subgoal` 多条件（hermes 有；首版单 objective + scratchpad checklist 已覆盖分解）。
- ❌ 多 active goal / 目标依赖图（首版一 session 一 active goal）。
- ❌ 在 harness 内加任何认知（R10）。

## 10. 落地约束（来自任务协议）

- 全程在 `feat/standing-goal` worktree，**不碰 main**。
- 重构同步清理过期代码（见 §8），不留死代码。
- 完成后**不做 cargo check**，直接提交（任务方强制约束，避免系统负担）。
- 集成回 main 用 `--no-ff` 合并（隔离开发≠禁止集成）。
