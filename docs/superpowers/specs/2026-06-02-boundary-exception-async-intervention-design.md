# 维度 3：边界异常反馈与异步干预机制 (Boundary Exception & Asynchronous Intervention)

- **日期**: 2026-06-02
- **范围**: 仅维度 3（真实缺口）。维度 1（状态管理）、2（Goal-Loop Veto）、4（资源治理）已合入 `main` 并验证，本文不重复设计。
- **红线**: R7（LLM 主权）、R10（薄 Harness 笨循环）。

---

## 1. 背景与缺口定位 (Scan)

本设计源自对 `/Volumes/TBU4/goal.md` 执行轨迹的逆向提取。四个 Harness 治理维度中，三个已落地：

| 维度 | goal.md 模式 | Aleph 现状（已验证） |
|---|---|---|
| 1. 状态管理 | scratchpad Objective + `- [ ]` 列表 + complete/clear | ✅ 已合并 `builtin_tools/scratchpad.rs` + `scratchpad_registry.rs`，3 态生命周期（`PlanItemStatus{Pending,InProgress,Done}`），持久化到 `~/.aleph/workspaces/<id>/scratchpad.md` |
| 2. Goal-Loop Veto | `ScratchpadGoalVerifier` veto-until-complete | ✅ 已合并 `verification/scratchpad_goal_verifier.rs`，结构看门狗，零 LLM 调用，`MAX_VERIFIER_VETOS=10` 兜底 |
| 4. 资源治理 | `check_and_run_cargo` 负载门 | ✅ 已合并 `sandbox/resource_governor.rs`，`SandboxBeforeHook`，bounded-wait→Reject |
| **3. 边界异常与异步干预** | 熔断主动反馈 / 用户意图热插拔 | ⚠️ **本设计的真实缺口** |

### 1.1 缺口 A — 熔断后静默终止（无主动反馈）

`src/harness/agent.rs` 有三个非自愿终止点：

- `ConsecutiveFailureCap`（:490）—— **静默** break HitLimit
- `VerifierVeto`（:517）—— **静默** break HitLimit
- `HitMaxIterations`（:529）—— 有 `fire_max_iterations_grace_turn`（:537）救援

只有 `HitMaxIterations` 会给模型一次"收尾"机会产出终端消息。前两者直接 `set_terminate_reason → callback.on_complete() → break HitLimit`，用户收到的是空/半截响应，**不知道卡在哪、缺什么、该如何指示**。

### 1.2 缺口 B — 意图热插拔未触达任务列表

`src/gateway/execution_engine/steering.rs::try_inject_steering` 已合入 `main`（`execute.rs:84` 调用），能把运行中会话的新用户消息作为 `SessionEvent::UserMessage{synthetic:false}` 注入；prompt G2（`agent/prompt.rs:68-80`）会把它包成 `<system-reminder>` 真实插话。

但它**只注入 prompt tail**，没有告诉模型"这是在你执行任务列表期间追加的意图"。模型可能不会去 reconcile scratchpad，导致 Goal-Loop Verifier 永远追踪不到新意图。"如何把新指令更新到 Markdown 任务列表而不撕裂状态"这一问题仍然敞开。

### 1.3 跨切关注点 — scratchpad 写入非原子

`memory/scratchpad/manager.rs::write`（:272）是裸 `fs::write` + 非原子备份拷贝。一旦 steering 能触发运行中 re-plan，中断写入有损坏活跃任务列表的风险。

---

## 2. 设计总纲 (Plan)

**核心论断**: 两个半场都是对三个已合并子系统的**连线**，不是新建认知层。

- **不在 `src/harness/` 新增任何文件**（R10 预算不动）。
- **智慧留在模型**（R7）：3a 让模型自己写"卡住"消息，3b 让模型自己决定 append/插队/重排。Harness 只负责组装结构化上下文 + 投递 framing 文本。

```
        ┌─────────────────────── 维度 3 ───────────────────────┐
        │                                                       │
  3a 熔断主动反馈                              3b 意图热插拔     │
  (harness/agent.rs)                          (gateway/steering)│
        │                                            │          │
        │ 复用 fire_max_iterations_grace_turn        │ 复用 scratchpad_registry
        │ 复用 ScratchpadManager::snapshot()         │ 复用 prompt G2 插话包装
        │                                            │          │
        └──────────► 模型一次 LLM 调用 ◄─────────────┘          │
                   (R7：模型产出文本/决策)                       │
        │                                                       │
   跨切：scratchpad write 原子化 (复用 utils/atomic_write)       │
        └───────────────────────────────────────────────────────┘
```

---

## 3. 组件 A — 边界异常救援轮 (3a)

### 3.1 改动

把现有的 `fire_max_iterations_grace_turn` 泛化为统一的边界救援机制：

```rust
// src/harness/agent.rs — 由 fire_max_iterations_grace_turn 泛化而来
//
// Fires ONE final LLM turn so the model produces a user-facing
// "I'm blocked here" summary instead of a silent HitLimit break.
// Structural context only — the harness judges nothing.
async fn fire_boundary_grace_turn(
    &self,
    reason: &TerminateReason,
    scratchpad_ctx: Option<ScratchpadSnapshot>, // 剩余未勾选步骤
    last_blocker: Option<String>,               // 最近一次工具错误/障碍
);
```

在三个非自愿终止点统一调用：

| 终止点 | 现状 | 改动后 |
|---|---|---|
| `ConsecutiveFailureCap`（:490） | 静默 break | 调用 `fire_boundary_grace_turn` |
| `VerifierVeto`（:517） | 静默 break | 调用 `fire_boundary_grace_turn` |
| `HitMaxIterations`（:529） | 已有 grace turn | 改调统一入口（行为不变） |

### 3.2 救援轮内容

救援轮以一段结构化 `<system-reminder>` 喂给模型一次 LLM 调用，载荷包含：

1. 终止原因（veto 上限 / 连续失败 / 迭代上限）；
2. scratchpad 剩余未勾选步骤（来自已合并的 `scratchpad_registry` → `ScratchpadManager::snapshot()`）；
3. 最近一次工具错误 / 障碍。

**模型自己写**"我卡在 X，剩余步骤是 Y，障碍是 Z，请指示如何继续"的用户可读消息。

### 3.3 为什么模型写而非硬编码模板

硬编码"卡住"模板埋进 Harness = 更笨的 Harness + 违反 R7。`fire_max_iterations_grace_turn` 的既有用途正是"把失控轮次救援成终端总结"，泛化它是最小熵增 + 单一机制。

### 3.4 无新死循环风险

救援轮**单次**，绝不循环。若模型本身是坏掉的组件（如空响应触发连续失败上限），救援轮也失败 → 回退到一条最小结构化终止消息。无新会话状态机。

### 3.5 恢复路径

救援消息经正常 callback/event 通道触达用户；用户下一句通过**已合并的 steering** 自然续跑。**零新状态机**（用户拍板的 3a 推荐项）。

### 3.6 挂载点

仅 `src/harness/agent.rs`（改一个函数 + 两个调用点）。上下文采集复用已合并的 scratchpad 基础设施。R10 预算不动（不新增 `src/harness/` 文件）。

---

## 4. 组件 B — Scratchpad 感知的 steering 信封 (3b)

### 4.1 改动

在 `try_inject_steering` 注入前，查 `scratchpad_registry`（已合并）判断目标会话是否有活跃 objective。若有，把 steering 消息渲染成**带 reconcile 前导语的信封**：

> 用户在你执行任务列表期间追加了新意图：‹msg›。请先调用 scratchpad 重规划（append / 插队 / 重排优先级由你判断），再继续。

### 4.2 合并所有权归模型 (R7)

append / 插队 / 重排优先级**由模型决定**。Harness **绝不**替用户 splice `scratchpad.md`（用户拍板的 3b 推荐项）。

### 4.3 无需改 prompt.rs

prompt G2 已经把非 synthetic 用户消息包成真实插话，前导语随消息文本一起进入即可。**不新增 `SessionEvent` 变体，不改 `prompt.rs`**。

### 4.4 无状态撕裂

`scratchpad.md` 的唯一写者是循环内的工具调用（每会话单线程）。steering 只往会话日志 append 一个事件（已通过 `session_service` 保证并发安全）。两者不竞争。

### 4.5 挂载点

仅 `src/gateway/execution_engine/steering.rs`。无 Harness 改动。

---

## 5. 跨切 — scratchpad 写入原子化 (P7 防御 + 熵减)

`manager.rs::write`（:272）由裸 `fs::write` 换为复用既有 `utils/atomic_write::atomic_write_file(path, content) -> Result<(), AlephError>`（tmp + rename）。中断写入不再损坏活跃任务列表。外科式一行替换 + 删除非原子备份拷贝逻辑。

**挂载点**: 仅 `src/memory/scratchpad/manager.rs`。

---

## 6. 数据流 (Action)

### 6.1 3a 熔断反馈流

```
循环触 veto 上限
  → set_terminate_reason(VerifierVeto)
  → fire_boundary_grace_turn{reason, snapshot, last_blocker}
  → 一次 LLM 调用（模型写"卡住"消息）
  → callback.on_complete() → break HitLimit
  → 消息经 callback/event 通道触达用户
  → 用户回复
  → (已合并) steering / 新轮次续跑
```

### 6.2 3b 意图热插拔流

```
用户运行中发新消息
  → gateway execute.rs busy 路径
  → try_inject_steering
  → 查 scratchpad_registry：有活跃 objective?
      是 → 组装 reconcile 前导语信封
  → session_service append UserMessage{synthetic:false}
  → 下一 harness 轮次重读日志
  → prompt G2 包成插话
  → 模型调用 scratchpad(set_plan/start_item) 自主合并
  → Goal-Loop Verifier 现在追踪到新增项
```

---

## 7. 红线合规

| 红线 | 如何守住 |
|---|---|
| **R7 (LLM 主权)** | 3a：模型写卡住消息；3b：模型拥有合并决策。Harness 只组装结构化上下文，零语义裁判。 |
| **R10 (薄 Harness)** | 不新增 `src/harness/` 文件，仅泛化一个既有函数；3b 全在 gateway 层。12 文件 / ~4900 行预算不动。 |
| **P7 (防御性设计)** | scratchpad 写入原子化；救援轮单次不循环、有回退。 |

---

## 8. 测试计划（复用既有套件）

| 组件 | 测试 | 复用套件 |
|---|---|---|
| 3a | `fire_boundary_grace_turn` 从 snapshot 正确组装上下文 | 单元（agent.rs） |
| 3a | veto 上限产出**非空**终端 assistant 消息（非静默 HitLimit） | `harness/tests/task10_wiring` |
| 3b | reconcile 前导语**当且仅当**有活跃 objective 时出现 | `steering.rs` tests |
| 3b | 前导语经 prompt G2 正确成插话 | `agent/prompt.rs` tests |
| 跨切 | 中断/并发写入留下合法文件 | `scratchpad/manager.rs` tests（已有 `tempdir` 夹具） |

校验命令（资源门控 `pgrep -x cargo < 3`）：

- `cargo check --bin aleph-server` → exit 0
- `cargo check -p alephcore --tests` → exit 0
- 定向单测全绿

---

## 9. 改动文件清单

| 类型 | 文件 | 作用 |
|---|---|---|
| 改 | `src/harness/agent.rs` | 泛化 `fire_max_iterations_grace_turn` → `fire_boundary_grace_turn`；接入 veto/failure 两个终止点 |
| 改 | `src/gateway/execution_engine/steering.rs` | reconcile 信封；查 `scratchpad_registry` |
| 改 | `src/memory/scratchpad/manager.rs` | `write` 换原子写；删非原子备份拷贝 |

无新增文件。无破坏性接口变更。
