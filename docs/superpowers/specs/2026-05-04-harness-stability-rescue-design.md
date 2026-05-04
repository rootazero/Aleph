---
title: Harness Stability Rescue (P0)
status: draft
date: 2026-05-04
authors: ["claude-opus-4-7"]
scope: src/harness/{agent,stall,deps,trait_def}.rs + harness/tests/
---

# Harness Stability Rescue — P0 抢救包

> **目标**：让 `AgentHarness` 在长时运行下不挂死、不无效循环、不因可恢复故障 abort，
> 同时让 `TraceSink` 这条已存在的可观测性管道真正工作。
>
> **非目标**：12 模块全面对齐 / Guardrails / Verification judge / PromptBuilder 抽象 ——
> 留待后续独立 spec。

## 1. 背景与定位

参考输入：
- `Agent Harness：未来 AI 竞争的核心架构` 指出生产级 Harness 的 12 大模块。
- `claude-code` (TS) 的实际实现：工具失败回流模型、per-call timeout、完整 trace 流。
- Aleph 现有架构红线：R10 薄 Harness、R7 LLM 主权、笨循环编排核心。

对比 12 模块 × Aleph 现状（详见 brainstorming 记录），P0 级缺陷集中在 4 处：

| ID | 缺陷 | 文件:行 |
|----|------|---------|
| P0-1 | `act()` 首次工具失败即 abort 整个 session（应回流模型） | `agent.rs:264-336` |
| P0-2 | `StallTracker` 不抢占 hang 中的 turn（`record_activity` 仅 turn 间） | `agent.rs:443-447` + `stall.rs:62-71` |
| P0-3 | `TraceSink` 是死基础设施（schema 全 9 变体在，零 fire 点） | `agent.rs` 全文搜不到 `trace_sink.on_trace` |
| P0-4 | 无 per-turn timeout（hung LLM 调用永久卡住） | — |

修复策略：**原地手术，不引入新 trait/struct，不增加 .rs 文件数**。匹配 R10 薄 Harness。

## 2. 架构边界与不变量

**修改的合法物理范围**

- `src/harness/agent.rs`
- `src/harness/stall.rs`
- `src/harness/deps.rs`
- `src/harness/trait_def.rs`
- `src/harness/tests/` 下新增/扩充 4 个文件中的测试用例

**不动**：`providers/` / `tools/` / `session/` / `context/` / `verification/` / `dispatcher/`。

**不变量保持**

- `harness/` 文件数：10 → 10（不新增 .rs）
- `harness/` 总行数：2066 → ≤ 2300（预算 +234）
- 无新增 trait（emit helper 是 `impl AgentHarness` 内非 pub fn）
- 所有新 `HarnessDeps` 字段为 `Option<...>`，缺省走旧行为
- `HarnessError::Tool` / `Stalled` 变体保留（向后兼容）

## 3. 节 1 — 错误处理改造（P0-1）

### 3.1 当前破口

`act()` (agent.rs:264-336) 当前语义：

```text
for call in tool_calls {
    if first_error.is_some() { skip + emit ToolError("Skipped") + continue }
    match execute {
        Ok  → emit ToolResult
        Err → emit ToolError; first_error = Some(e)
    }
}
if let Some(e) = first_error { return Err(HarnessError::Tool(e)); }
```

两个问题：

1. **首错短路**：批内首工具失败导致后续 N-1 个工具变 `Skipped`，模型本可独立完成的并行工作被牵连。
2. **整 session abort**：`HarnessError::Tool(e)` 一路冒到 `run` → `SessionDriver::drive` → 上层视作不可恢复，session 死。

### 3.2 改造后语义

```text
for call in tool_calls {
    emit ToolCallRequested
    match execute {
        Ok(out)        → emit ToolResult
        Err(tool_err)  → emit ToolError(tool_err.to_string())
    }
}
return Ok((TurnState::Continue, executed_count, false));
```

三处关键决定：

1. **不再首错跳过后续**：批内每 call 独立执行，失败/成功并存。
2. **失败走 ToolError 事件 → 下一 Think 的 `tool_result.is_error=true`**：投影路径 `build_prompt` (agent.rs:652-665) **已存在**，仅因 act 抛 Err 而走不到。
3. **`HarnessError::Tool` 退化为基础设施级**：仅在 `tools.execute` panic / session emit_event 失败时使用。停止从业务路径产生它。

### 3.3 新增安全阀：consecutive_tool_failure_turns

避免"模型死循环调失败工具"无效循环：

- 计数器：本轮 `executed=0` 且至少有 1 个 ToolError → +1；任意成功 → 归零
- 默认上限 8 轮 → 强制 `TurnState::Done` + `hit_limit=true`
- 配置位：`HarnessDeps::consecutive_failure_cap: Option<usize>`，`None` 关闭

### 3.4 测试

- `tool_failure_recovers_in_next_think`
- `partial_batch_failure_continues`（3 tool，中间失败，后两个仍执行）
- `consecutive_total_failure_caps_loop`（8 轮全失败 → hit_limit=true，不抛 Err）

## 4. 节 2 — Watchdog / Timeout（P0-2 + P0-4）

### 4.1 双层语义切分

| 信号 | 语义 | 触发位置 | 错误变体 |
|------|------|----------|----------|
| **Per-turn timeout** | 单 LLM/tool 调用 hang 死 | wrap 在 `await` 周围 | `HarnessError::StalledTurn { phase, elapsed }` |
| **Cross-turn stall** | 多轮无进展、agent 静默 | 现有 `StallTracker` | `HarnessError::Stalled { elapsed }`（保留） |

切分理由：当前混淆两者→两者都漏检。`is_stalled()` 仅 turn 间检查（hang 中走不到），`record_activity` 仅 turn 完成后调用（hang 永不更新）。

### 4.2 Per-turn 实现

```rust
// deps.rs
pub turn_timeout: Option<Duration>,   // None = 旧行为；推荐 Some(300s)

// agent.rs run_turn_internal
let token = parent_cancel.child_token();
let started = Instant::now();
let think_fut = self.deps.llm.process(payload);
let response = match timeout(turn_timeout, think_fut).await {
    Ok(Ok(r))   => r,
    Ok(Err(e))  => return Err(HarnessError::Llm(e)),
    Err(_)      => {
        token.cancel();
        // elapsed 是实测耗时（≈ turn_timeout，但保留真实值便于诊断）
        return Err(HarnessError::StalledTurn {
            phase: TurnPhase::Think,
            elapsed: started.elapsed(),
        });
    }
};
// act() 内每个 tool call 同样套 timeout，phase = Act { tool_name }
```

**Cancel token 级联**：harness 持有 `parent_cancel`，每 turn 派生 `child_token`；timeout → cancel child → 协作工具 await 退出。parent_cancel 仍直接生效（顶部检查）。

### 4.3 Cross-turn StallTracker 重构（最小动作）

`StallTracker` API 不动；仅扩散 `record_activity()` 调用点：
1. Think 完成后（emit AssistantMessage 之后）
2. 每 tool call 完成后（不论成败）
3. 进入下一 turn 之前（保留现有点）

`is_stalled()` 检查点保持在 turn 顶部。

### 4.4 错误变体

```rust
pub enum TurnPhase {
    Think,
    Act { tool_name: String },
}

pub enum HarnessError {
    // ...existing...
    Stalled { elapsed: Duration },                          // 跨 turn（保留）
    StalledTurn { phase: TurnPhase, elapsed: Duration },    // 新
}
```

### 4.5 测试

- `think_timeout_fires_with_phase_think`
- `act_timeout_fires_with_phase_act_and_tool_name`
- `parent_cancel_takes_precedence_over_timeout`
- `cross_turn_stall_still_works`

## 5. 节 3 — TraceSink 接线（P0-3）

### 5.1 fire 点

| # | 时机 | 事件 |
|---|------|------|
| 1 | run_turn_internal 入口 | `TurnStarted { iteration }` |
| 2 | LLM call 前 | `TurnStateEntered { state: Think }` |
| 3a | response 含 text | `TextEmitted { stream: Final, text }` |
| 3b | tool_calls 非空 | `TurnStateEntered { state: Act }` |
| 4 | act() 内 each tool | `ToolCallStarted` → `ToolCallCompleted` |
| 5 | turn 收尾 | `TurnCompleted { outcome, metrics }` |
| 6 | run 退出循环 | `SessionCompleted { outcome, ... }` |

### 5.2 调用规约

写进 `TraceSink` trait 的 doc comment（不改签名）：

```rust
/// Implementations MUST NOT block. Sink is invoked from async tasks and
/// blocking calls back-pressure the entire harness loop. Production sinks
/// should push events to an mpsc channel and drain elsewhere.
```

### 5.3 惰性构造

```rust
fn emit(&self, build: impl FnOnce() -> LoopTraceEvent) {
    if let Some(ref sink) = self.deps.trace_sink {
        sink.on_trace(&build());
    }
}
```

`trace_sink: None` 时连 event 都不构造（避免 String/Value 分配）。

### 5.4 outcome 映射

| 退出路径 | TurnOutcome | SessionOutcome |
|----------|-------------|----------------|
| 自然 Done | Stop | Completed |
| max_iterations 触顶 | HitLimit | HitLimit |
| FinalReply 短路 | HitLimit | HitLimit |
| `cancel.is_cancelled()` | Cancelled | Cancelled |
| `StalledTurn` | Cancelled | Cancelled |
| `Stalled` (跨 turn) | Cancelled | Cancelled |

### 5.5 测试

- `recording_sink_captures_full_lifecycle`
- `noop_sink_zero_overhead`（trace_sink=None 时 builder 不调用）
- `outcome_mapping_for_stalled_turn`

## 6. 实施顺序与回滚

```text
Step 1: TraceSink 接线 (P0-3)
        +emit() helper, +5 处 fire 点 — 无行为变化
        提供观测能力以便后续步骤可验证

Step 2: act() 错误吞回 + consecutive_failure cap (P0-1)
        无新 deps 字段；保持向后兼容

Step 3: Per-turn timeout + TurnPhase (P0-4)
        +deps.turn_timeout, +HarnessError::StalledTurn

Step 4: StallTracker record_activity 三处扩散 (P0-2 cross-turn)
        最小 diff，无新 API
```

四步 → 四 commit，单步 revert 不影响后续。Step 1 必须先于 Step 3（否则 timeout 触发缺乏可观测性）。Step 2 与 Step 4 可换序。

## 7. 风险登记

| 风险 | 概率 | 影响 | 缓解 |
|------|------|------|------|
| Step 2 吞掉灾难性 ToolError | 低 | 中 | 保留 `HarnessError::Tool`；只是停止业务路径产生它 |
| Step 3 timeout 误伤合法长 LLM | 中 | 中 | 默认 300s；`Option` 缺省可关 |
| Cancel 未被工具尊重 → 子任务僵尸 | 低 | 低 | tokio Drop 语义自动取消未完成 future |
| TraceSink 阻塞实现拖慢 harness | 中 | 高 | trait doc 强制 "MUST NOT block"；Gateway 端走 mpsc |
| consecutive cap 误杀 debug 长链 | 低 | 低 | 默认 8 已宽；`Option<usize>` 可关 |
| `Stalled` vs `StalledTurn` 双语义混淆 | 中 | 低 | doc 明确区分；后续 spec 统一 |

## 8. 验收闸

- [ ] 4 commit，每个独立 `cargo test -p alephcore --lib harness::` 通过
- [ ] 新增测试 10 个全绿（§3.4×3 + §4.5×4 + §5.5×3）
- [ ] `cargo clippy -p alephcore -- -D warnings` 0 警告
- [ ] `just test-all` 全绿
- [ ] `harness/` 文件数仍为 10
- [ ] `harness/` 总行数 ≤ 2300
- [ ] CHANGELOG.md 写入英文条目
- [ ] 本地长跑：`aleph-server` 起动后跑 30 分钟 looping-tool-error mock，断言不 abort、TraceSink 持续吐事件、内存不漂移

## 9. 面向未来测试（R10 自检）

把 `deps.llm` 换成更强模型（如 Opus 5），harness 性能应**自然提升**——本 spec 零认知逻辑，全部加固在脚手架层（错误回流、超时、可观测）。✅ Pass。

## 10. 后续 spec 候选（不在本范围）

- Guardrails 三层（输入/输出/工具调用）
- Verification & Feedback（judge agent + 计算式回路）
- PromptBuilder 抽象（替代私有 build_prompt）
- ChainContext 流入 AgentHarness（subagent 谱系追溯）
- Tool list 缓存（消除 agent.rs:158-170 每轮重新拉取）
- `run_turn` 重复扫描事件日志（agent.rs:496-502 O(n) 优化）
- `Stalled` / `StalledTurn` 语义统一
- `MAX_STOP_HOOK_VETOS` 配置化

---

## 附录 A — 完整 12 模块对照（参考）

| # | 模块 | Aleph 当前 | 本 spec 触及? | 后续 spec |
|---|------|-----------|--------------|-----------|
| 1 | Orchestration Loop | `harness/agent.rs` | ✅ | — |
| 2 | Tools | `tools/` + `builtin_tools/` + `dispatcher/` | ❌ | 缓存优化 |
| 3 | Memory | `memory/` (Spec A/B/C) | ❌ | — |
| 4 | Context Management | `context/{budget,compact}` | ❌ | — |
| 5 | Prompt Assembly | 私有 `build_prompt` | ❌ | PromptBuilder |
| 6 | Tool Calling/Structured Output | `providers/adapter` | ❌ | — |
| 7 | State & Checkpointing | `session/{events,store}` | ❌ | — |
| 8 | Error Handling | `HarnessError` | ✅ | 错误分类细化 |
| 9 | Guardrails | 仅 stop_hooks | ❌ | 三层 guardrail |
| 10 | Verification & Feedback | `verification/stop_hooks` | ❌ | judge agent |
| 11 | Subagent Orchestration | `agents/` + `ChainContext` | ❌ | ChainContext 流入 |
| 12 | Initialization | `init_unified/` | ❌ | — |
