# Loop Engineering: 把客观闸门接进自主 goal 循环

**日期**: 2026-06-11
**范围**: A — 客观闸门接进自主循环（最锋利单点连线）
**架构层**: loop（`prompt → context → harness → **loop**` 中的最外层）

---

## 1. 背景与问题

参考文章《Loop engineering: the 14-step roadmap》把"loop engineering"拆为
5 个积木（Automations / Worktrees / Skills / Connectors / Sub-agents）+
状态文件 + **客观闸门(gate)** + 失败模式防护。Aleph 几乎每块都已有底座，
本任务是**连线为主**，不是从零造引擎。

文章核心论点（§09 / §12）：**maker 不能给自己批改作业**。自主循环在宣告
"完成"前，必须有一个**独立、客观**的 checker 守住停止条件，否则就是
**Ralph Wiggum loop**——模型提前发出完成信号，循环在半成品上静默退出。

### 1.1 Aleph 现状（已扫描确认）

| 文章积木 | Aleph 底座 | 连线状态 |
|---|---|---|
| Automations（心跳 + `/goal` until-condition） | `tasks/cron/`、`goal/`、`tasks/goal_pursuit.rs` | ✅ 已连 |
| Worktrees / Skills / Connectors(MCP) | `sandbox/`、`skill/`、`mcp/` | ✅ 已连 |
| Sub-agents（maker ≠ checker） | `teams/`、`verification/`（stop-hook / tool-loop / scratchpad-goal verifier） | ⚠️ 半连 |
| 客观闸门（test/build/lint 退出码） | `verification/stop_hooks.rs`（`ShellStopHook`：exit 0=allow, exit 2=block） | 🔴 **未接进自主循环** |

### 1.2 精确缺口

- `src/builtin_tools/goal.rs:200`：`goal(action='update', status='complete')`
  **无条件**把状态写成 `Complete`。没有任何客观闸门。
- `src/verification/stop_hook_verifier.rs` 的 `StopHookVerifier`（红线合规的
  **结构化**退出码闸门）**已接线**——但只接进 **within-turn** 的 harness
  verifier 链（`orchestrator_init.rs:147`）。它守的是单 run 的停止；一次
  veto 只会重跑那一 turn，**永远到不了 cross-run 自主循环的终止决策**。
- `src/gateway/execution_engine/execute.rs:623` 的续跑钩子纯粹以
  `goal.status != Active` 为终止条件，**对闸门 handler 零访问**。

→ 红线合规的客观闸门**存在**，自主循环**存在**，但二者在完成决策点
**没有连接**。本任务即纯 **连线**。

### 1.3 红线约束（决定映射方式，非照抄）

`src/verification/mod.rs` **永久禁止 `JudgeVerifier` / 认知判断在 Rust**
（R7 LLM 主权 + R10 笨循环 5 个不 #3）。因此文章的"独立 checker 模型"
**不能**照搬为一个语义判断器。映射后的 checker 只能是：
**结构化退出码闸门**（`ShellStopHook`，deterministic exit code，零 LLM 调用）。
这正好是文章 §11/§12 强调的 *"a test that passes or fails, not an opinion"*。

---

## 2. 核心设计：Maker/Checker 的类型态分离

模型调用 `goal(complete)` 是一个**主张(claim)**；闸门退出码 0 才是
**确认(confirmation)**。把这个区分**编码进类型系统**——这是用 Rust 类型
安全实现并超越参考项目松散 `bool`/`string` flag 的地方。

全部改动落在 **loop 层**，**不碰 `src/harness/`**（R10 12 文件预算零增长）。
within-turn 的 `StopHookVerifier` 保持不动；本轮补的是 **cross-run** 自主
循环的完成决策。

---

## 3. 改动清单

### 改动 1 — 类型：`src/goal/types.rs`

新增 2 态枚举字段（`#[serde(default)]` 向后兼容旧持久化）：

```rust
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GateOutcome {
    /// 尚无闸门确认完成（默认；也是 Active/Paused/Blocked 的静息态）。
    #[default]
    Unchecked,
    /// 客观闸门确认了模型的 `Complete` 主张。
    Passed,
}

// Goal 新增字段：
//   #[serde(default)]
//   pub gate_outcome: GateOutcome,
```

状态转换：

- 模型设 `Complete`（goal 工具）→ `gate_outcome` 保持 `Unchecked`（仅 claim）。
- 闸门退出 0 → `Passed`（确认，循环真正终止）。
- 闸门退出 2 → **不持久化失败态**；把 goal 退回 `Active`（`gate_outcome`
  复位 `Unchecked`），失败原因写入 `note`。

新增不可变 mutator（遵循 §不可变性：返回新 `Goal`）：

```rust
#[must_use]
pub fn with_gate_outcome(mut self, outcome: GateOutcome, now_ms: u64) -> Self { ... }
```

**"真正终止"判定**：`status == Complete && (无闸门 || gate_outcome == Passed)`。
没有配置 `stop_hooks` 的用户：`gate.is_none()` → claim 立即终止 →
**行为零变化**（关键回归保护）。

### 改动 2 — 纯决策函数：`src/tasks/goal_pursuit.rs`

零副作用、可单测（Rust 纯函数测试优势）：

```rust
/// 模型在 Active 续跑下自报 Complete，但闸门尚未确认。调用方应跑闸门。
pub fn awaiting_gate(goal: &Goal, gate_configured: bool) -> bool {
    matches!(goal.pursuit, PursuitMode::Active { .. })
        && goal.status == GoalStatus::Complete
        && goal.gate_outcome == GateOutcome::Unchecked
        && gate_configured
}

/// 闸门通过：完成被确认，循环终止。
pub fn confirm_complete(goal: &Goal, now_ms: u64) -> Goal {
    goal.clone().with_gate_outcome(GateOutcome::Passed, now_ms)
}

/// 闸门失败（Ralph Wiggum 营救）：退回 Active + 失败原因入 note；
/// 若迭代上限已耗尽 → Blocked（复用 cap_reached_note，不复制 Blocked 逻辑）。
pub fn reopen_after_gate_failure(goal: &Goal, reason: &str, now_ms: u64) -> Goal { ... }

/// 闸门失败后的续跑 prompt——注入客观失败信号（R9 智慧在 prompt）。
pub fn gate_failure_prompt(goal: &Goal, reason: &str) -> String { ... }
```

Ralph Wiggum 营救 = `reopen_after_gate_failure` + `gate_failure_prompt`，
仍受现有迭代上限（`PursuitMode::Active { max_iterations }`）硬约束；上限
耗尽则复用现成 `cap_reached_note` 转 `Blocked`。

### 改动 3 — 连线：`execute.rs` + `engine.rs` + `orchestrator_init.rs`

- **`engine.rs`**：`continuation_deps` 元组扩一项
  `Option<Arc<Vec<Arc<dyn StopHookHandler>>>>`（同一份闸门 handler）。
- **`orchestrator_init.rs`**：`build_stop_hooks(...)` 结果 `.clone()`（Arc
  廉价）**同时**喂给 `StopHookVerifier` 和续跑 deps——一份闸门，两个消费者。
- **`execute.rs`** 续跑块：在 `should_continue` 检查**之前**加一个分支
  `awaiting_gate(&goal, gate.is_some())`：
  - 用现成 `execute_stop_hooks(&boxed, &hctx, cancel)` 跑闸门（**Tokio
    并发**已内建：多 hook 并行、各自 timeout；闸门跑在 post-run
    `tokio::spawn` 内，**不阻塞网关**）。
  - 构造 `StopHookContext`：`final_text` = goal objective / 最后文本；
    `iterations` = `continuations_used`；`stop_reason` = `"goal_complete_claim"`。
  - **通过**（无 blocking / halt）→ `store.put(confirm_complete(...))` +
    `GoalVerified` info 日志 → 终止。
  - **否决** → `reopen_after_gate_failure` → `store.put`：
    - 回到 Active → `spent_continuation` + enqueue `gate_failure_prompt` 续跑。
    - Blocked（cap 已满）→ 仅持久化 + 日志。

### 数据流（cross-run）

```
run 结束 → post-run hook
  ├─ awaiting_gate? ──no──→ 现有 should_continue / exhausted 逻辑（不变）
  └─ yes → execute_stop_hooks (Tokio 并行)
            ├─ exit 0 → confirm_complete → gate_outcome=Passed → 终止 ✅
            └─ exit 2 → reopen_after_gate_failure
                        ├─ cap 未满 → Active + note + 续跑(gate_failure_prompt)
                        └─ cap 已满 → Blocked + cap_reached_note
```

---

## 4. 熵减清单（实施时核实并标注）

- 复用 `cap_reached_note` / `continuation_prompt` / `execute_stop_hooks` /
  `GoalStore`，不新建并行机制。
- 合并 `exhausted_while_active` 的 Blocked 转换与 gate-failure-cap-spent 的
  Blocked 转换为单一 helper，避免重复 Blocked 逻辑。
- 扫描有无遗留的 goal-verification 死桩；有则删除。

---

## 5. 测试（Rust 单测，纯函数易测）

`src/tasks/goal_pursuit.rs` 单测：

- `awaiting_gate` 四态真值表（Active+Complete+Unchecked+gate → true；
  其余组合 → false）。
- `confirm_complete` 幂等 + 设 `Passed`。
- `reopen_after_gate_failure`：cap 未满 → Active + note；cap 满 → Blocked。
- **回归保护**：无闸门时 claim 立即终止（`gate.is_none()` 路径行为不变）。

`src/goal/types.rs` 单测：

- `with_gate_outcome` 返回新 Goal、bump `updated_at_ms`、不改其它字段。
- 旧持久化（缺 `gate_outcome`）反序列化为 `Unchecked`。

---

## 6. 红线合规核对

- **R1/R3**：无平台 API、无重型依赖；纯复用现有 `ShellStopHook`。
- **R7 LLM 主权**：闸门是**结构化退出码**，零 LLM 调用，零语义判断。
- **R10 薄 Harness**：改动落在 `goal/` + `tasks/` + `gateway/execution_engine/`，
  **`src/harness/` 零改动**，12 文件预算不动。
- **§不可变性**：`Goal` 全部 mutator 返回新值。
- **失败模式（文章 §12）**：直接修复 Ralph Wiggum loop——模型提前发出完成
  主张 → 客观退出码闸门否决 → 循环带失败信号继续；硬停止（迭代上限 →
  Blocked）保留。

---

## 7. 非目标（Out of Scope）

- per-goal 闸门命令（文章每个 /goal 独立条件）——留作后续；本轮纯复用全局
  `stop_hooks`，零新配置面。
- 任何认知判断器 / `JudgeVerifier`——永久红线禁止。
- harness 层 / within-turn verifier 链改动——本轮不碰。
