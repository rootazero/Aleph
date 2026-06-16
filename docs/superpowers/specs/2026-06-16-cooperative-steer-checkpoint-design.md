# 协作式 Steer 检查点 (Cooperative Steer Checkpoint)

**Date:** 2026-06-16
**Branch:** `steer-checkpoint`
**Status:** Design approved — pending implementation plan

---

## 1. 背景与问题

Aleph 在"agent 执行任务时处理新到达的用户消息"这块已有成熟架构（codex/hermes 级别）：

- **Steer**（默认）：把新消息注入 session event log，下一轮 Think 由 LLM 消费。
- **Interrupt**：取消整个 run 再重起。
- **Queue**：FIFO 延迟到 run 结束后作为新 run。
- 配套：`last_prompt_log_len` 水印边界检测、`has_unanswered_user_message` follow-up、steering rescue、`MAX_PENDING_STEERING` 背压、coalescing。

**唯一高价值缺口 = 抢占延迟。** 当用户中途"悔改"（改变需求）时，消息被注入了 log，但**当前 turn 已决定的整批工具会全部执行完**，下一轮 Think 才看到新消息。结果：用户改主意后，agent 仍在为过时需求消耗工具调用与 token，浪费明显。

而"取消进行中工具"的基础设施**已全部存在但未连到 steer 到达事件**：

- `run_cancel.child_token()` — 每个工具调用的子取消令牌（`act.rs:416-419, 804`）
- `in_flight_tool_calls` 注册表 + `tools.cancel_call` RPC（`act.rs:421-432, 805-809`）
- `tools.execute_with_cancel`（`act.rs:429, 809`）

## 2. 目标

当一条**非 synthetic 用户消息**在当前 turn 的 prompt 边界之后落入 session log（即用户在 agent 跑工具时说话），harness **不再派发该批中尚未启动的工具**，而是立刻把控制权交回 Think，让 LLM 看到新消息并自主决定 pivot 或 resume。

**非目标（本轮不做）：**

- 不中止已经在运行的工具（in-flight 工具跑完、结果保留）。硬中止能力由现有 `Interrupt` mode 提供，不在本轮扩展。
- 不在 Panel 上显示 deferred 提示（静默；可作为独立一轮）。
- 不做任何意图分类 / "这是不是悔改"的判断（R7：交给 LLM）。

## 3. 架构红线对齐

| 红线 | 对齐说明 |
|------|----------|
| **R7 LLM 主权** | 系统只决定"别再派发过时工具，把麦克风还给 LLM"。是否 pivot 由 LLM 下一轮推理判断，无规则引擎。 |
| **R9 智慧在 Prompt** | deferred 工具 + 新消息（`<system-reminder>` 包裹）一起进下一轮 prompt，LLM 一次推理覆盖判断。零额外 LLM 调用。 |
| **R10 薄 Harness / 笨循环** | **零新文件**，复用现有水印 + 谓词 + `act.rs`。检查点是机械脚手架，不参与推理。守住 12 文件预算。"加代码前必答 3 问"：①脚手架（是）②模型升级后仍需要（是，纯机械递送优化与模型无关）③真实消费者（是，悔改场景）。 |

## 4. 设计

### 4.1 信号传递：Pull / 水印（不引入新令牌）

选择**复用现有水印 + 谓词**，而非新增 push 信号令牌跨 gateway→harness 边界。理由：水印（`last_prompt_log_len`，`think.rs` 每 turn 取完 events 后设一次）与谓词（`has_unanswered_user_message`，`agent.rs:392`）**已存在**，这是最"连线优先"、最省 R10 预算的选择，零新跨边界状态线。

### 4.2 熵减提取（本设计唯一的"重写"）

把 `has_unanswered_user_message` 从 `agent.rs` 的 `impl` 私有方法提取为**可复用谓词**（同模块自由函数，或挂在能被 `act.rs` 访问的位置），使：

- `Done` 边界 follow-up 检查（现有调用点）
- `act.rs` 工具批检查点（新调用点）

**调用同一份逻辑**，杜绝两份会漂移的副本。提取后 `agent.rs` 原调用点改指共享版。

谓词语义（不变）：读 session events，跳过水印 `last_prompt_log_len` 之前的；若水印之后存在任一**非 synthetic** `UserMessage` 则返回 `true`。这天然排除：run 自己的开场输入、harness 自注入的 nudge/grace turn、已被上一轮消费的消息。

### 4.3 串行路径检查点

位置：`act.rs:263` 的 `for mut call in tool_calls` 批分发循环。

```text
分发每个工具前:
  if mid_turn_steering && checkpoint_predicate(session, watermark):
      把"当前及之后所有尚未启动的工具"标记为 deferred
      break 出批循环
  否则: 正常分发该工具
```

命中后 short-circuit，剩余跳过为 O(1) 标记，不重复查 session。

### 4.4 并行路径检查点

位置：`act.rs:841` 的 `stream::iter(live_futs).buffered(parallelism)`。

改为按 `parallelism` 大小**分波**处理；每波分发前查一次谓词。命中则：当前波跑完保留结果，剩余波标记 deferred。

> 两条路径调用**同一谓词函数**，行为一致。

### 4.5 被跳过工具的 API 契约处理（关键正确性点）

Anthropic/codex 协议要求每个 `tool_use` 必有配对 `tool_result`。对每个 deferred 工具：

- 发一条合成 `ToolResult{ call_id: <原 call.id>, content: "deferred: superseded by a new user message; re-issue if still needed" }`。
- 保留原始 `call.id` 与 args 上下文，LLM 下一轮看到后可零成本重发仍需要的工具。
- 走现有 `emit_event` / turn 提交路径，**不新增事件类型**。

### 4.6 turn 返回与循环衔接

批因检查点提前 break 后，turn 返回 `TurnState::Continue`（非 `Done`）。外层 loop 自然进入下一轮 Think：

- `think.rs` 重建 prompt（水印重置），LLM 看到 in-flight 工具结果 + deferred 合成结果 + 新用户消息。
- 不碰 `Done` 边界 follow-up 与 steering rescue（它们在 turn **之后**，本机制在 turn **之内**，互补不重叠）。

## 5. 数据流示例（"悔改"全程）

```text
T1: LLM 决定批量跑 [build, test, deploy]（旧需求）
    串行分发：build 启动…
T1.5: 用户发 "等等，先别 deploy，改成只跑 lint"
    → try_inject_steering 注入 UserMessage(synthetic:false) 到 log（已有路径）
T1: build 完成、结果保留 → 分发 test 前查谓词 → 命中！
    → test、deploy 标记 deferred + 各发一条合成 ToolResult
    → 批循环 break，turn 返回 Continue
T2: 下一轮 Think 重建 prompt（水印重置）→ LLM 看到：
    build 结果 + 两条 deferred + 用户新消息(<system-reminder> 包裹)
    → LLM 自主决定：放弃 test/deploy 改跑 lint (pivot)
       或判断仍需要 → 重发 (resume)
```

对比今天：T1 会把 test + deploy **全跑完**才在 T2 看到悔改——这正是被消除的浪费。

## 6. 守卫与边界条件

- 仅 `mid_turn_steering`（`ExecutionEngineConfig`）开启时启用；关闭 = 逐字节 legacy 全批行为，**无回归**。
- 谓词只认水印之后的非 synthetic 用户消息（见 4.2）。
- 命中后立即 short-circuit，不重复查 session。
- 不碰错误处理语义（`act.rs:501` "一个工具失败不 abort 其他" 是独立关注点，保留）。
- 不碰 in-flight 工具（本轮非目标）。

## 7. 熵减清单

| 动作 | 位置 |
|------|------|
| 提取共享谓词（删私有副本风险） | `has_unanswered_user_message` @ `agent.rs:392` → 共享谓词；原调用点改指 |
| 无其他死代码产生 | 本设计以加法连线为主 |

## 8. 测试计划

1. **谓词单测**：四种输入下布尔正确性 — (无新消息 / synthetic 新消息 / 水印前消息 / 水印后真消息)。
2. **串行检查点**：3 工具批，第 1 个完成后注入 steer → 断言后 2 个得到 deferred 合成 ToolResult、批提前 break、turn 返回 Continue。
3. **并行检查点**：波间命中 → 断言当前波完成、剩余波 deferred。
4. **契约**：每个 deferred `tool_use` 都有配对 `tool_result`（API 不拒绝）。
5. **回归**：`mid_turn_steering=false` → 全批执行，逐字节等价旧行为。

## 9. 受影响文件（预估）

- `src/harness/agent.rs` — 提取共享谓词，改原调用点。
- `src/harness/agent/act.rs` — 串行 + 并行检查点；deferred 合成 ToolResult 生成。
- （可能）`src/harness/agent/think.rs` — 若水印/谓词需要从 think 侧暴露给 act。
- 测试：随上述文件就近添加单测。

**注：** 按项目执行协议，完成后不跑 `cargo check`，直接提交（资源并发治理约束）。
