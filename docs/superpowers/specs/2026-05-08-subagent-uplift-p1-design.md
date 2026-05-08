---
title: Subagent Uplift P1 — Design (还债 phase)
status: draft
date: 2026-05-08
authors: ["claude-opus-4-7"]
scope: design-only — 不写代码、不写 plan
follows: 2026-05-08-subagent-uplift-roadmap-design.md
phase: P1 (还债 — Stage A/B/C/D)
---

# Subagent Uplift P1 — Design

> **目标**：把 roadmap § Stage A/B/C/D 四项还债，落成可让 writing-plans 直接消费的设计。
> 单 PR / 4 个 atomic commit / 估 ~625 行（含 ~350 行测试，跨 4 新 integration 文件），零行改动 `src/harness/`。
>
> **非目标**：本设计不冻结字段名 / 函数命名细节（plan 阶段决定），不冻结测试函数体（plan / 实施阶段决定）；不预先实现 Stage A 的 subagent override 字段（按零覆盖原则推迟到 P4）。

## 0. Decisions Locked（来自 brainstorm Q&A）

| ID | Decision | Rationale |
|----|----------|-----------|
| Q1 | **单 PR，4 commit (A→B→C→D)** | 4 项内部独立，bundle review 一次性看完整 P1 picture；rollback 单位 = 整个 P1 |
| Q2 | **`src/orchestrator/deps_builder.rs` 新模块** | 语义对齐（builder 是 orchestrator-scope 装配工具）；不触 R10 `src/harness/` 红线；future 可承接更多 main runner 装配代码 |
| Q3 | **零覆盖：subagent 完全继承主 runner 5 字段** | 当前无具体 subagent override 场景（YAGNI）；AgentDef schema 不增长；future 加 `Option<T>` 字段向后兼容 |
| Q4 | **`AgentDef::is_tool_allowed` mode-aware deny + 字符串字面量 `"subagent"`** | 单点真理：`execute`/`list`/`describe` 三处自动一致；rename refactor 时 `grep '"subagent"'` 即可；当前只一处 deny，无需常量抽象 |
| Q5 | **Lane budget 满时 fail-fast (`ToolError::Execution`)** | 死锁规避（subagent 优先级低，await main 释放可能死锁）；R7 LLM 主权（信任模型处理 busy）；可观测（fail 立即留 trace） |
| Q6a | **`max_concurrent: 4`** | 个人 AI 场景 sweet spot；超过基本是 LLM 失控 fan-out 信号 |
| Q6b | **删 `lane_scheduler.rs:113` TODO** | YAGNI；无消费者；未来真要做 priority boost git log 查得到 |
| Q7 | **3 integration tests (LLM-await / tool-await / turn_timeout cascade) + test-driven fix + 5s timeout wrap** | 覆盖底层 cancel 语义三条核心路径；fix scope 来自实测而非猜测 |

## 1. Shared Constraints

### 1.1 PR 形态

- 单 PR，4 个 atomic commit：A → B → C → D
- 每 commit ≤ 200 行（含测试）；总 PR ≤ 800 行（roadmap 估 ~680，溢出余量 ~120）
- CI 跑一次；rollback 单位是整个 P1 phase

### 1.2 零覆盖原则（影响 Stage A）

- subagent 的 5 字段（`fallback_llm` / `stall_config` / `consecutive_failure_cap` / `turn_timeout` / `trace_sink`）**完全继承**主 runner 装配结果
- AgentDef 不加 override 字段；spawn-time 不传 override 参数
- 显式 deferred 到 P4：未来若发现 subagent 需更紧约束，AgentDef 加 `Option<T>` 字段（向后兼容）即可

### 1.3 R10 红线复核

| 红线 | 影响 | 验证 |
|------|------|------|
| `src/harness/` ≤ 1500 行 / 9 文件 | **零修改**（不进 harness/） | line count + 文件 count check |
| 笨循环 5 个"不"（不判意图 / 不过滤工具 / 不判完成 / 不审内容 / 不选恢复策略） | **零增加** | A 是装配；B 是 allowlist mode-deny；C 是 lane budget；D 是 cancellation；都不是认知 |
| YAGNI / 无消费者抽象立删 | A 共享 builder 有 ≥1 真实消费者（subagent + main runner）；C 新 `try_reserve` 有 ≥1 消费者（spawner） | grep 验证 |
| AgentDef schema 兼容硬约束 | A 不改 schema；B 仅改 `is_tool_allowed` 内部逻辑（API 接口不变）；C 不改；D 不改 | schema test |

### 1.4 文件改动总清单

| 文件 | 改动类型 | 行数估算 | 来自 stage |
|------|---------|---------|-----------|
| `src/orchestrator/mod.rs` | 新建（pub use） | ~10 | A |
| `src/orchestrator/deps_builder.rs` | 新建（提取 Phase-6 builder） | ~120 | A |
| `src/bin/aleph-server/commands/start/orchestrator_init.rs` | 改：内联 → 调用共享 builder | ~30 删 + ~10 改 | A |
| `src/agents/subagent_spawner.rs` | 改：5 处 None → builder 调用 + lane reserve | ~50 改/加 | A, C |
| `src/agents/types.rs` | 改：`is_tool_allowed` mode-aware deny | ~10 | B |
| `src/agents/subagent_tool.rs` | 改：注释修订 | ~5 | B |
| `src/scheduler/lane_scheduler.rs` | 改：加 `try_reserve` API + 删 line 113 TODO + 加 `LaneBudgetExhausted` 错误 | ~50 | C |
| `src/config/types/scheduler.rs`（或同等位置） | 改：新 `[scheduler.subagent_lane]` section | ~30 | C |
| `tests/integration/cancellation_chain.rs` | 新建：3 个 cancellation 测试 | ~150 | D |
| `tests/integration/recursion_guard.rs` | 新建：1 个递归 guard 端到端 | ~50 | B |
| `tests/integration/lane_budget.rs` | 新建：1 个 lane 满载行为 | ~50 | C |
| `tests/integration/subagent_deps_inherit.rs` | 新建：1 个 5 字段继承断言 | ~50 | A |
| `docs/reference/MULTI_AGENT_SYSTEM.md` | 改：递归保护章节修订 | ~40 | A/B/C ship 时同步 |
| **合计** | | **~625 行** | |

### 1.5 测试整体策略

- **单元测试**：≥3 (A) + ≥3 (B) + ≥5 (C) = 11 个
- **集成测试**：4 个新文件（每文件聚焦一个 stage）
- **总绿条件**：所有新测试 + 现有 6 个 `allowlist_tool_service` 测试 + Phase-6 13 builder unit + 4 init_audit + Stage 4 subagent 测试全绿

## 2. Stage A — Shared `deps_builder` Module

**Status**: ✅ Shipped: 70c3f1480
**Risk class**: low

### 2.1 模块结构

```
src/orchestrator/
├── mod.rs              # pub use deps_builder::*; (~10 行)
└── deps_builder.rs     # Phase-6 builder + StabilityTriple struct (~120 行)
```

### 2.2 公开 API

```rust
//! Shared HarnessDeps builder functions.
//!
//! Used by both the main runner (orchestrator_init.rs) and the subagent
//! spawner (agents/subagent_spawner.rs) to assemble HarnessDeps fields
//! consistently. Subagents inherit identical config; per P1 zero-override
//! decision, no override params are accepted.

pub struct StabilityTriple {
    pub stall_config: Option<StallConfig>,
    pub consecutive_failure_cap: Option<usize>,
    pub turn_timeout: Option<Duration>,
}

pub fn build_fallback_llm(
    config: &HarnessConfig,
    providers: &ProviderRegistry,
) -> Result<Option<Arc<dyn LlmProvider>>, DepsBuilderError>;

pub fn build_stability_triple(config: &HarnessConfig) -> StabilityTriple;
```

**`DepsBuilderError`**：thiserror enum，覆盖 `SelfReferencingFallback` / `UnknownProvider`；语义来自 Phase-6 现有逻辑。

**关键约束**：
- 两个 fn 是 **pure**（同一 config 同一 output），不 hold 任何状态
- 不接受 override 参数 — 严格执行 1.2 零覆盖原则

### 2.3 主 runner wiring

`orchestrator_init.rs`：

- 删（~30 行）：原内联 `build_fallback_llm` + `build_stability_triple` 实现
- 加（~10 行）：直接调用共享 fn `crate::orchestrator::deps_builder::*`
- Phase-6 现有 13 个 builder unit + 4 个 init_audit 测试**全部继续在 orchestrator_init.rs 测试 module 中跑**（测试文件不动，只测试 import 路径变更）

### 2.4 subagent wiring

`subagent_spawner.rs:200-225` 5 处 `None` → builder 调用：

```rust
let stability = build_stability_triple(&self.harness_cfg);
HarnessDeps {
    // ... 已就绪字段（guardrails inherit 等）...
    fallback_llm: build_fallback_llm(&self.harness_cfg, &self.providers)?,
    stall_config: stability.stall_config,
    consecutive_failure_cap: stability.consecutive_failure_cap,
    turn_timeout: stability.turn_timeout,
    trace_sink: parent_deps.trace_sink.clone(),
    // ... 剩余 5 个 None（A3 范围，本 stage 不动）...
}
```

`harness_cfg` / `providers` 来源：spawner 已持有 parent HarnessDeps 引用；如父级未持有这两个引用，新增 `parent_harness_cfg: Arc<HarnessConfig>` 字段到 spawner 结构（plan 阶段查证决定）。

### 2.5 测试

| 测试 | 位置 | 验证 |
|------|------|------|
| `build_fallback_llm_returns_none_on_missing_config` | `deps_builder.rs` `#[cfg(test)]` | unit |
| `build_fallback_llm_errors_on_self_reference` | 同上 | unit（搬自 Phase-6） |
| `build_stability_triple_independence` | 同上 | unit（3 字段独立 None/Some） |
| `subagent_inherits_5_fields` | `tests/integration/subagent_deps_inherit.rs` | integration: spawn subagent，断言 5 字段值 == 主 runner |

### 2.6 Old code retirement

- `orchestrator_init.rs` 内联 `build_fallback_llm` / `build_stability_triple` 实现 → 删（~30 行）
- `subagent_spawner.rs:200-225` 5 处 `None` 字面量 → 删

### 2.7 Acceptance criteria

- 功能：subagent 装配的 5 字段与主 runner 同等成熟度（同 config 驱动 / 同 self-reference + unknown-name guards）
- 不破坏：Phase-6 13 builder unit + 4 init_audit + Stage 4 subagent 测试全绿；R10 `src/harness/agent.rs` ≤ 1500 行不变
- 测试：3 unit + 1 integration（如上）
- 性能：subagent spawn 延迟 ≤ 1.05× baseline（hyperfine lock 在 plan 阶段）

## 3. Stage B — Recursion Guard

**Status**: ✅ Shipped: 61ce09a96
**Risk class**: low

### 3.1 改动核心：`AgentDef::is_tool_allowed`

`src/agents/types.rs`：

```rust
impl AgentDef {
    pub fn is_tool_allowed(&self, tool_name: &str) -> bool {
        // Recursion guard: SubAgent mode is structurally forbidden from
        // spawning further subagents. Overrides allowlist (even "*" / explicit
        // "subagent") because recursion safety is a system invariant, not a
        // policy knob. Primary mode behavior unchanged.
        if matches!(self.mode, AgentMode::SubAgent) && tool_name == "subagent" {
            return false;
        }
        self.allowed_tools.iter().any(|t| t == "*" || t == tool_name)
    }
}
```

### 3.2 单点真理（Q4-d）

`AllowlistToolService` 零修改 — `execute` / `list` / `describe` 三处行为通过 `is_tool_allowed` 自动统一：
- `execute("subagent", ...)` → `PermissionDenied`
- `list()` → 不返回 `"subagent"`
- `describe("subagent")` → `None`

### 3.3 字符串字面量 vs 常量

按 Q4 决定：用 `"subagent"` 字面量（不引入常量）。当前只一处使用，YAGNI 优先；rename refactor 时 `grep '"subagent"'` 找得到。

### 3.4 注释 / 文档修订

**`src/agents/subagent_tool.rs:6`** —— 重写误导性注释：

> Subagent tool registration. SubAgent-mode agents are denied invocation
> of this tool via `AgentDef::is_tool_allowed` (recursion guard); see
> `agents/types.rs` for the rule.

**`docs/reference/MULTI_AGENT_SYSTEM.md`** —— 递归保护章节对齐为：

> SubAgent-mode agents are structurally denied from invoking the `subagent` tool.
> Enforcement lives in `AgentDef::is_tool_allowed` (`src/agents/types.rs`),
> which overrides any explicit allowlist entry. Primary-mode agents retain
> full subagent-spawning capability. Two additional defense layers exist:
> ChainContext depth guard (`subagent_spawner.rs`) and LaneScheduler
> recursion tracker (`scheduler/lane_scheduler.rs`).

### 3.5 测试

| 测试 | 位置 | 验证 |
|------|------|------|
| `subagent_mode_denies_subagent_tool` | `types.rs` `#[cfg(test)]` | unit: SubAgent + allowlist `["*"]` → false |
| `subagent_mode_denies_explicit_subagent_in_allowlist` | 同上 | unit: SubAgent + allowlist `["subagent","read"]` → false（验证 mode-deny 覆盖 allowlist） |
| `primary_mode_allows_subagent_tool` | 同上 | unit: Primary + allowlist `["subagent"]` → true |
| `recursion_guard_end_to_end` | `tests/integration/recursion_guard.rs` | integration: 父 (Primary) spawn 子 (SubAgent)，子 list 不含 subagent；子 execute → PermissionDenied |

### 3.6 Old code retirement

- `subagent_tool.rs:6` 误导注释 → 重写
- `MULTI_AGENT_SYSTEM.md` 关于递归保护的过期描述 → 更新为现状真理

### 3.7 Acceptance criteria

- 功能：SubAgent mode agent 调 `subagent` tool → `PermissionDenied`；`list` 不暴露；`describe` 返回 None
- 不破坏：Primary mode 仍可调 subagent；ChainContext depth guard 行为不变；现有 `allowlist_tool_service` 6 测试全绿
- 测试：3 unit + 1 integration（如上）
- 文档：`MULTI_AGENT_SYSTEM.md` + `subagent_tool.rs:6` 与代码三方一致

## 4. Stage C — LaneScheduler 接入 Spawner

**Status**: ✅ Shipped: 5f9f155f1
**Risk class**: medium

### 4.1 新 API：`LaneScheduler::try_reserve`

```rust
impl LaneScheduler {
    /// Atomically attempt to reserve a lane slot without queuing.
    ///
    /// Unlike enqueue + try_schedule_next, this is fail-fast: if global
    /// capacity or lane capacity is exhausted, returns
    /// SchedulerError::LaneBudgetExhausted immediately. The caller is
    /// responsible for translating this to a domain-appropriate error.
    ///
    /// On success, returns a ScheduleGuard whose Drop releases permits
    /// via RAII. Caller MUST also invoke on_run_complete on all exit
    /// paths to clear lane state tracking (the guard handles permits;
    /// on_run_complete handles state).
    pub async fn try_reserve(
        &self,
        run_id: String,
        lane: Lane,
    ) -> Result<ScheduleGuard, SchedulerError>;
}
```

新错误变体：`SchedulerError::LaneBudgetExhausted { lane: Lane, max: usize }`（thiserror，~5 行）。

实现要点（详细落到 plan）：
- `global_semaphore.try_acquire` → `lane_semaphore.try_acquire` 双闸；任一失败 release 已得 permit + 返回错误
- 成功路径 `state.mark_running` + 构造 `ScheduleGuard`（与 `try_schedule_next` 共享 RAII 路径）
- 不动 `wait_tracker` / 不动 queue —— 这是"不排队"语义的根本

### 4.2 配置：`[scheduler.subagent_lane]`

```toml
[scheduler.subagent_lane]
max_concurrent = 4
priority = 50    # 主 lane 通常 100；subagent 低于主 lane
```

**default**：若 `aleph.toml` 无此 section，hardcoded default `LaneQuota { max_concurrent: 4, priority: 50 }`。`LaneConfig::default()` 在初始化注入此项。

注：`Lane::Subagent` 是否已在 enum 中需 plan 阶段查证；不存在则 P1.C 内一并加（仅 enum variant 增加，向后兼容）。

### 4.3 Spawner 入口 / 出口 wiring

```rust
pub async fn spawn(&self, ..., parent_run_id: &str) -> Result<..., ToolError> {
    let run_id = generate_subagent_run_id();

    // (incidental win) 第三道防线：递归深度
    self.lane_scheduler
        .check_recursion_depth(parent_run_id)
        .await
        .map_err(|e| ToolError::Execution {
            name: "subagent".into(),
            cause: format!("recursion depth exceeded: {e}"),
        })?;

    // 主要：lane budget reserve（fail-fast）
    let guard = self.lane_scheduler
        .try_reserve(run_id.clone(), Lane::Subagent)
        .await
        .map_err(|e| ToolError::Execution {
            name: "subagent".into(),
            cause: format!("subagent lane budget exhausted: {e}"),
        })?;

    // (incidental win) 父-子关系入 recursion tracker
    self.lane_scheduler
        .record_spawn(parent_run_id, &run_id)
        .await;

    // ... existing spawn 内部逻辑 ...
    let result = self.spawn_internal(run_id.clone(), ...).await;

    // 出口（成功/失败/cancel 任一路径）：清 state + RAII 释 permit
    self.lane_scheduler
        .on_run_complete(&run_id, Lane::Subagent, Some(guard))
        .await;

    result
}
```

**RAII 不变量**：`spawn_internal` panic / cancel / Err 路径 — `guard` Drop 自动 release permits；`on_run_complete` 必须在所有路径上调用一次以清 lane state（plan 用 `Result` 解构 + 显式 cleanup，或封装内部 `defer!` 风格 helper；不引 scopeguard crate）。

### 4.4 错误映射汇总

| 来源 | 映射至 |
|------|--------|
| `SchedulerError::LaneBudgetExhausted` | `ToolError::Execution { name: "subagent", cause: "subagent lane budget exhausted (max=N)" }` |
| `SchedulerError::UnknownLane` | `ToolError::Execution { name: "subagent", cause: "scheduler misconfigured" }` |
| `check_recursion_depth` Err | `ToolError::Execution { name: "subagent", cause: "recursion depth exceeded" }` |

不新增 `ToolError::Busy` 变体；`is_retryable()` 自动 false（LLM 看到错误会改策略而非盲重试）。

### 4.5 删除 TODO

`src/scheduler/lane_scheduler.rs:113` 的 `// TODO: In future, we can apply per-run priority boosts here` → 删。

### 4.6 测试

| 测试 | 位置 | 验证 |
|------|------|------|
| `try_reserve_succeeds_with_capacity` | `lane_scheduler.rs` `#[cfg(test)]` | unit: 4 次 reserve 全成功 |
| `try_reserve_fails_when_lane_exhausted` | 同上 | unit: 第 5 次 → `LaneBudgetExhausted` |
| `try_reserve_fails_when_global_exhausted` | 同上 | unit: global cap 满，第 2 次 → 错（即使 lane 有空） |
| `try_reserve_unknown_lane` | 同上 | unit: 未配置 lane → `UnknownLane` |
| `guard_drop_releases_permit` | 同上 | unit: drop guard → 再 reserve 成功 |
| `subagent_spawn_4_ok_5th_busy` | `tests/integration/lane_budget.rs` | integration: 父连续 spawn 4 个 OK，第 5 个 → `ToolError::Execution { cause contains "lane budget" }` |

### 4.7 Old code retirement

- `lane_scheduler.rs:113` TODO 注释 → 删
- 无其他需删；spawner 当前无 lane wiring，是新增不是替换

### 4.8 Acceptance criteria

- 功能：父 spawn 4 个 subagent 并行成功；第 5 个返回 `ToolError::Execution`；任一 subagent 完成后 slot 立即可复用
- 不破坏：scheduler 现有 `enqueue` / `try_schedule_next` 路径行为不变；现有 cucumber tests 全绿
- 测试：5 unit + 1 integration（如上）
- 性能：`try_reserve` 路径 ≤ 100µs（hyperfine lock 在 plan 阶段）

## 5. Stage D — Cancellation Propagation Tests + Fix

**Status**: ✅ Shipped: 7c062b548
**Risk class**: low（测试为主；修补范围 unknown until tests run）

### 5.1 范围声明

- **测试驱动**：先写 3 个 integration tests，跑；测试通过 → 修补 scope = 0；测试失败 → 就地修补 spawner / HarnessDeps token wiring
- **不在范围**：tool service 是否 honor `CancellationToken` mid-execution（tool-side 责任）；LLM provider 是否 honor token mid-stream（provider-side 责任，测试中用 mock 已就位）
- **在范围**：父 `CancellationToken` 是否完整传给子 `HarnessDeps`；子 harness 关键 await 点是否 `select!` 上 token

### 5.2 三个 integration tests

**fixture（共享，新文件 `tests/integration/cancellation_chain.rs`）**：

```rust
struct HangingLlmProvider { cancel_token: CancellationToken }
// LLM await token.cancelled() 才返回；模拟"长时 LLM 调用"

struct HangingTool;
// execute() 进入即 await token.cancelled() 才返回；模拟"长时工具"

fn spawn_test_parent_with_subagent() -> (parent_handle, parent_token, child_completion_signal);
```

每个测试用 `tokio::time::timeout(Duration::from_secs(5), async { ... }).await.expect("test must complete")` 强制 5s 上限。

#### Test 1 — `parent_cancel_stops_child_at_llm_await`

父 spawn 子；子用 `HangingLlmProvider` 进入 LLM await；父发 cancel；断言子在 ~10ms 内 `HarnessExitStatus::Cancelled`；timeout 5s。

#### Test 2 — `parent_cancel_stops_child_at_tool_await`

同 fixture，但子在 `HangingTool::execute` 中 await token.cancelled()；验证子 harness 在 tool 返回 cancelled 后正确结束 turn / 退出 loop。

#### Test 3 — `parent_turn_timeout_cascades_to_child`

父 `turn_timeout = 1s`；父 spawn 长时子（`HangingLlmProvider`）；父 turn_timeout 触发后，父 harness loop cancel → 父 token cancel → 子终止；timeout 5s。

### 5.3 修补范围预测

按概率排序：

1. **subagent_spawner 没把父 token 传给子 HarnessDeps** —— 修补：spawner 入口处 `child_token = parent_token.child_token()` 或 `Arc::clone`，写入 `HarnessDeps.cancellation_token`。预算 ≤ 30 行 in spawner 内。
2. **HarnessDeps `cancellation_token` 字段虽存在但子 harness loop 未 `select!` 它** —— 修补：`src/harness/agent.rs` 关键 await 点用 `tokio::select!`。**触碰 `src/harness/`，违反 R10 红线** → 若 fix 真落到 `agent.rs`，**必须**在 plan 阶段额外加 R10 影响声明 + 行数审核（agent.rs 仍 ≤ 1500 行）。
3. **turn_timeout 触发后没 cancel parent token** —— 修补：harness loop 内 turn_timeout fires → token.cancel()；同样可能落 `src/harness/agent.rs`，同样 R10 复核。
4. **(无失败)** —— spawner 已正确 wire token；3 测试全绿；修补 scope = 0。

预期最常见：(1) 或 (4)。Phase-6 已 wire turn_timeout（假定包含 cancel 路径），但**没人测过**，所以 D 的价值就是发现真相。

### 5.4 R10 风险标注

如果修补需触碰 `src/harness/agent.rs`：

- 必须在 plan / commit message 显式声明 R10 影响
- 行数审核：修补后 agent.rs 仍 ≤ 1500 行（plan 时锁 baseline）
- "笨循环 5 个不" 复核：cancel `select!` 是工程纪律护栏（不是认知判断），通过

如果修补只在 `subagent_spawner.rs`：零 R10 风险。

### 5.5 Acceptance criteria

- 功能：3 测试在 5s 内全绿（cancel 真的传播）
- 修补：若 (1) 触发，spawner token wiring fix ≤ 30 行；若 (2)/(3) 触发，触 R10 复核并显式声明
- 不破坏：现有 Stage 4 subagent 测试 + Phase-6 13 builder unit + 4 init_audit 全绿
- 测试覆盖：3 path（LLM-await / tool-await / turn_timeout-cascade）全部覆盖

## 6. PR Order & Verification

### 6.1 Commit 顺序

A → B → C → D。每 commit 独立编译 + 测试通过（atomic）：

| Commit | 主要改动 | 预算行数 |
|--------|---------|---------|
| 1. Stage A — extract shared deps_builder | 新建 orchestrator/deps_builder.rs；orchestrator_init.rs forward；spawner 5 字段 wiring；4 测试 | ~210 |
| 2. Stage B — recursion guard via is_tool_allowed | types.rs mode-deny；subagent_tool.rs 注释；MULTI_AGENT_SYSTEM.md 修订；4 测试 | ~100 |
| 3. Stage C — LaneScheduler integration | lane_scheduler.rs::try_reserve + LaneBudgetExhausted；scheduler config section；spawner reserve wiring；6 测试；删 TODO | ~165 |
| 4. Stage D — cancellation chain tests + fix | 3 cancellation tests；如有 fix 单独 sub-commit 或同 commit（plan 决定） | ~150 + fix 预算 |
| **总计** | | **~625 行** |

### 6.2 PR 级 verification

1. `cargo build --release` 全绿
2. `cargo test --workspace` 全绿
3. `cargo clippy --workspace -- -D warnings` 全绿
4. R10 hard checks：
   - `wc -l src/harness/*.rs | tail -1` ≤ 1500
   - `ls src/harness/*.rs | wc -l` 无变化（仍 9 文件）
5. 文档代码一致性：`MULTI_AGENT_SYSTEM.md` 描述与 `agents/types.rs::is_tool_allowed` 实际行为一致

### 6.3 Roadmap 更新

PR ship 后，在 roadmap (`2026-05-08-subagent-uplift-roadmap-design.md`) 的 Stage A/B/C/D 条目下追加：
```
✅ Shipped: <commit hash> on 2026-05-08
```

并在文件头部追加一行：
```
✅ P1 Shipped: <PR url> on 2026-05-08
```

## 7. Risk Register

| ID | Risk | Likelihood | Impact | Mitigation |
|----|------|-----------|--------|-----------|
| R1 | Stage D 修补落到 `src/harness/agent.rs` 触 R10 | 30% | medium | plan 阶段 line baseline；fix ≤ 30 行约束；`agent.rs` 仍 ≤ 1500 行硬上限 |
| R2 | `Lane::Subagent` 已在 enum 中但 default LaneConfig 未注入 | 50% | low | plan 阶段查证；如缺注入 LaneConfig::default() |
| R3 | spawner 当前未持 `harness_cfg` / `providers` 引用 | 40% | low | 加 `parent_harness_cfg: Arc<HarnessConfig>` 字段（minor struct 改动） |
| R4 | `try_reserve` 与 `try_schedule_next` 现有 RAII 路径冲突 | 10% | medium | plan 阶段共享 ScheduleGuard 构造逻辑 helper；测试 `guard_drop_releases_permit` 锁不变量 |
| R5 | 子 harness 开始执行后再 cancel，已发出 LLM 调用 token 漏传 | 20% | low | Test 1 直接覆盖此 case；如失败按 5.3 路径 (1) 修补 |
| R6 | Stage A `DepsBuilderError` 与 `orchestrator_init.rs` 现有错误类型不兼容 | 20% | low | plan 阶段统一为 `anyhow::Error` 或新 enum；不破坏现有 init_audit 测试 |
| R7 | `[scheduler.subagent_lane]` config schema 与现有 LaneConfig 序列化不兼容 | 15% | low | plan 阶段 schema test；新 section 用 `#[serde(default)]` 兜底 |

## 8. Out-of-scope（显式不做）

| 项 | 推迟到 | 理由 |
|----|-------|------|
| Subagent 可在 AgentDef 内 override 5 字段 | P4 | 当前无具体场景需要；YAGNI |
| `ToolError::Busy` 新变体 | P4（若多 busy-case 出现） | 当前只一处 lane busy；三次法则 |
| LaneScheduler `apply per-run priority boosts` | 永不（除非新需求） | YAGNI；line 113 TODO 删 |
| Subagent 多层 cascade cancel（父 → 子 → 孙） | P3.H 必要时 | 当前 ChainContext depth guard 默认禁深嵌套；P3.H worktree isolation 时一并搞 |
| 流式进度事件（subagent 中间状态） | P2.F | 依赖 Stage A trace_sink |
| 文件系统 agent 加载 | P2.E | 依赖 Stage B recursion guard |

## 9. References

- Roadmap master: `docs/superpowers/specs/2026-05-08-subagent-uplift-roadmap-design.md`
- Phase-6 master: `docs/superpowers/specs/2026-05-08-phase6-config-wiring-design.md`
- Phase-6 commit: `4aa1c0f6d`（2026-05-08）
- 12-module roadmap: `docs/superpowers/specs/2026-05-05-harness-12-module-roadmap-design.md`
- R10 哲学: `docs/reference/HARNESS_PHILOSOPHY.md`
- 现状文件：`src/agents/subagent_spawner.rs`, `src/agents/allowlist_tool_service.rs`, `src/agents/types.rs`, `src/scheduler/lane_scheduler.rs`, `src/tools/service.rs`, `src/bin/aleph-server/commands/start/orchestrator_init.rs`
