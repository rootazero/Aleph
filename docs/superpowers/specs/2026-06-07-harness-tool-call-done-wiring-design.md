# Harness `on_tool_call_done` 连线 — 设计规格

**日期**: 2026-06-07
**分支**: `fix/harness-tool-call-done-wiring`（独立 worktree）
**参考项目**: hermes-agent / openclaw / pi（agent loop 生命周期回调对照）

## 1. 背景与问题

Aleph 的 harness（`src/harness/`）已经过两轮 gap 分析且高度成熟（R10 薄 harness 红线）。
上一轮工作（`343053915`）已把 `on_tool_call_start` 连线并合入 main。本轮对照参考项目的
工具生命周期回调后，定位到**唯一一个真实缺口**：`on_tool_call_done` 是一条完整定义、
有完整消费链、但循环里从不触发的死线。

### 完整消费链（已存在，仅缺循环侧 fire）

```
HarnessCallback::on_tool_call_done(id, result, error)   ← callback.rs:39 已定义
  └─ BroadcastCallback::on_tool_call_done               ← harness_bridge/callback.rs:70 已实现
       └─ FlowStreamEvent::ToolCallDone                 ← dispatch.rs:51 已定义
            └─ event_drain.rs:164                        ← 已消费，移除 pending_tools + 发 ToolEnd
                 └─ StreamEvent::ToolEnd                 ← 推给 TUI/Telegram/panel/openai_api
```

### 对称性 bug

在 live `FlowStreamEvent` 广播路径（AgentHarness 运行时**唯一**产出 `StreamEvent::ToolStart`/
`ToolEnd` 的生产路径 —— `event_emitter` 的 `emit_tool_start/end` helper 仅测试调用）：

- `on_tool_call_start` **会触发**（`act.rs` 171/513）→ `ToolCallStart` → `StreamEvent::ToolStart` ✅
- `on_tool_call_done` **从不触发** → `ToolCallDone` 永不发送 → `event_drain.rs:164` 永不执行
  → **`StreamEvent::ToolEnd` 在 AgentHarness 运行时从不发出** ❌

**用户可见影响**：
1. TUI / Telegram / panel / openai_api 客户端在 live 流上看到每个工具"开始但从不结束"。
2. `event_drain` 的 `pending_tools` map **每次工具调用泄漏一个条目**（在 :150 插入，
   只在永不到达的 :167 移除）。

### 与持久化 trace 的区别（确认非重复）

`LoopTraceEvent::ToolCallCompleted`（12 个 construct-site）走的是**持久化 trace 通道**
（`task_traces` + panel rehydrate），与 live `FlowStreamEvent` 通道**不同**。两者互补，
连线 `on_tool_call_done` 不会与 `ToolCallCompleted` 重复，也不会产生重复的 `ToolEnd`
（已确认 `event_emitter` helper 仅测试调用）。

## 2. 调查中被否决的候选项（熵减：不制造无用改动）

| 候选项 | 判定 | 证据 |
|------|------|------|
| terminate_reason last-writer staleness | **不是 bug** | 每个 `set_terminate_reason` 站点后紧跟 `return Done/Err`，每个写入都是终结写入；setter 文档明确"only writes once per run"。 |
| dead trace variants | **无死代码** | 14 个 `LoopTraceEvent` 变体全部有 ≥1 construct-site。 |

## 3. 设计 — Approach A（co-locate）

原则：**每一个发出 `LoopTraceEvent::ToolCallCompleted`（持久化"done"）的站点，
同时触发 `callback.on_tool_call_done(...)`（live "done"）**。由此保证 live "done" 与
持久化 "completed" 完全对称，按构造无法遗漏任何完成路径。

### 4 个连线点

| # | 站点 | 路径 | 触发 |
|---|------|------|------|
| 1 | `act.rs` `emit_tool_success` | 成功 chokepoint（serial + parallel 汇聚） | `on_tool_call_done(&call.id, Some(&output_value), None)` |
| 2 | `act.rs` `emit_tool_error` | 错误 chokepoint（覆盖全部 4 个 error 调用点） | `on_tool_call_done(&call.id, None, Some(&error_msg))` |
| 3 | `act.rs` within-batch dedup cache-hit | 内联成功（不走 helper） | `on_tool_call_done(&call.id, Some(&output_value), None)` |
| 4 | `guardrails.rs` 工具守卫 Block | 守卫拦截完成（`callback` 已在作用域） | `on_tool_call_done(&call.id, None, Some(<block reason>))` |

### 回调穿线

- `emit_tool_success` 与 `emit_tool_error` 新增 `callback: &mut dyn HarnessCallback` 参数；
  其调用点（serial + parallel）传入已有的 `callback`。
- **安全不变量（实施时确认）**：parallel 路径在 join 之后**串行**调用这两个 helper
  （`callback` 是 `&mut`，无法跨 task 共享）—— 不存在独立的 parallel 完成站点这一事实
  已隐含此点。
- 站点 3、4 的 `callback` 已在作用域 → 单行新增。

## 4. 测试（按 TDD 写，但按用户约束不编译/不跑 cargo）

在 `src/harness/tests/` 新增/扩展：一个 `RecordingHarnessCallback`，在 `on_tool_call_done`
中记录 `(id, is_error)`；驱动一个成功工具调用 + 一个失败工具调用；断言两者都产出带正确
id 与 error 标志的 `done` 记录。复用现有 mock-deps 测试脚手架。测试作为回归资产留存。

## 5. 显式排除范围

- `on_tool_summary`：消费者（`event_drain:187` → `ToolUpdate`）已存在，但新 harness 没有
  summary **生成器**，触发它需要额外 LLM 调用 —— 属于新功能而非连线，本轮不做（R10）。

## 6. 流程约束（用户协议）

- 全部工作在独立 worktree 分支 `fix/harness-tool-call-done-wiring`，**不碰 main**。
- **不跑 `cargo check`**，直接提交（用户约束）。
- 接口向后兼容：仅"完成"已定义但未触发的 trait 方法，不改签名语义、不删除任何现有代码。

## 7. 影响面

约 6 处小改动，跨 2 个生产文件（`act.rs`、`guardrails.rs`）+ 1 个测试文件。
每一行都可追溯到这一个被确认的 bug。R10 12 文件边界不变（不新增 harness 文件）。
