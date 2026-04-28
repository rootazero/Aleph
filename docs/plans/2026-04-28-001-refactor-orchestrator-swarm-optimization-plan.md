---
title: "feat: Aleph Orchestrator & Swarm Optimization"
type: refactor
status: active
date: 2026-04-28
---

# Aleph Orchestrator & Swarm Optimization

## Overview

基于 Symphony (Elixir) vs Aleph (Rust) 架构对比分析，优化 Aleph 的 Orchestrator、Teams、Sandbox、Error Handling 模块。借鉴 Symphony 成熟特性，融合 Aleph 现有 Rust 架构优势，实施非破坏性重构。

## Problem Frame

**Symphony 优势特性 Aleph 缺失：**
1. **Stall 检测** — agent 卡死超时检测与自动处理
2. **生命周期钩子** — sandbox 预热/清理、metrics 收集
3. **指数退避重试** — transient error 的智能重试
4. **Issue 优先级队列** — 任务调度优先级
5. **并发限制可视化** — 实时 slot 监控

**约束：**
- 非破坏性重构，不删除现有功能
- 利用 Rust 类型安全和 trait 系统
- 与 Aleph 现有架构（Swarm、AgentHarness、Session）无缝融合

## Requirements Trace

- R1. 添加 activity timeout stall 检测机制到 Harness
- R2. 在 SandboxFactory 添加生命周期钩子支持
- R3. 在 Orchestrator 添加指数退避重试
- R4. FlowSpec 添加优先级字段
- R5. Orchestrator 添加可观测性 metrics

## Scope Boundaries

- **不涉及**：Session EventStore 改造、Gateway 改动
- **不涉及**：Swarm 协调器核心逻辑修改
- **不涉及**：多 Provider fallback 机制（已有基础）

## Key Technical Decisions

### 1. Stall Detection 实现位置

**决策**：在 `AgentHarness::run` 循环中添加 activity tracker

**理由**：
- Harness 是所有 flow 的执行入口，stall 检测自然置于此处
- 不影响 SessionService（保持单一职责）
- 与 `CancellationToken` 并行工作，互补不干扰

**方案**：
```rust
// HarnessDeps 添加可选配置
pub struct StallConfig {
    pub timeout: Duration,      // 无 activity 超时阈值
    pub check_interval: Duration, // 检查间隔
}

// run 循环中：
loop {
    if cancel.is_cancelled() { return Err(Cancelled); }
    if stall_tracker.is_stalled() { return Err(HarnessError::Stalled); }
    // ... run_turn
}
```

### 2. 生命周期钩子实现位置

**决策**：在 `SandboxFactory` 添加 hook registry，WorkspaceSandbox 执行时调用

**理由**：
- Sandbox 是资源边界，hook ，自然在 sandbox 层
- 与 `WorkspaceBuilder` 闭包解耦，灵活可插拔
- Symphony 的 before_run/after_run 对应 before_execute/after_execute

**方案**：
```rust
pub trait SandboxHooks: Send + Sync {
    async fn before_execute(&self, session: &SessionId);
    async fn after_execute(&self, session: &SessionId, result: &Result<(), SandboxError>);
}

pub struct SandboxFactory {
    inner: Arc<dyn Fn(SandboxKind, &str) -> Result<Arc<dyn Sandbox>, FlowError>>,
    hooks: Vec<Arc<dyn SandboxHooks>>,
}
```

### 3. 指数退避重试实现位置

**决策**：在 `Orchestrator::dispatch` 返回 `Result<FlowHandle, FlowError>` 后，Gateway 层处理重试；Orchestrator 提供 retry 辅助函数

**理由**：
- Orchestrator 是纯调度，不持有重试状态
- Gateway 已有外层 retry 循环（见 harness_bridge.rs）
- 只需提供重试延迟计算和判断工具

**方案**：
```rust
pub struct RetryConfig {
    pub base_delay: Duration,
    pub max_delay: Duration,
    pub max_attempts: u32,
}

pub fn compute_retry_delay(attempt: u32, config: &RetryConfig) -> Duration {
    let exp = attempt.min(6); // 最多 2^6 = 64
    let delay = config.base_delay * 2u32.pow(exp);
    delay.min(config.max_delay)
}
```

### 4. FlowSpec 优先级字段

**决策**：在 `FlowSpec` 添加 `priority: u8` 字段，Orchestrator dispatch 时作为排序依据

**理由**：
- 最小侵入性添加
- 优先级在 FlowRegistry 解析后直接可用
- 与现有 `depth` 字段正交

### 5. 可观测性 Metrics

**决策**：在 `Orchestrator` 添加 metrics 方法，返回 `OrchestratorMetrics`

**理由**：
- 非侵入性，不改变核心逻辑
- Gateway 可定期拉取或订阅
- 与现有 tracing 结合

## Open Questions

### Resolved During Planning

- **Q: Stall detection 超时阈值？** A: 默认 5 分钟，可通过 `StallConfig` 配置
- **Q: 生命周期钩子是否跨所有 Sandbox 类型？** A: 仅 `WorkspaceSandbox`，`DenyAllSandbox` 无资源需清理

### Deferred to Implementation

- **Q: 优先级队列具体排序算法？** A: 先 depth 升序，再 priority 降序
- **Q: Metrics 暴露格式？** A: JSON 格式 via `OrchestratorMetrics` struct

## Implementation Units

- [ ] **Unit 1: 添加 Stall Detection 到 Harness**

**Goal:** 在 AgentHarness 添加 activity timeout 检测，防止 agent 卡死

**Requirements:** R1

**Dependencies:** None

**Files:**
- Modify: `src/harness/deps.rs` — 添加 `StallConfig` 到 `HarnessDeps`
- Modify: `src/harness/trait_def.rs` — `HarnessError` 添加 `Stalled` variant
- Modify: `src/harness/agent.rs` — `run` 循环添加 stall tracker
- Test: `src/harness/agent_tests.rs` 或新建 `src/harness/tests/`

**Approach:**
1. 在 `deps.rs` 添加 `StallConfig` struct（含 timeout、check_interval）
2. 添加 `StallTracker` struct，内部持有 `Mutex<Option<Instant>>` 和 `CancellationToken`
3. `AgentHarness::run` 每次 `run_turn` 前后调用 `stall_tracker.tick()`
4. stall tracker 后台任务定时检查，超过 timeout 则返回 `HarnessError::Stalled`

**Patterns to follow:**
- `src/harness/deps.rs` HarnessDeps 结构
- `src/harness/trait_def.rs` HarnessError 模式

**Test scenarios:**
- Happy path: agent 正常完成，stall tracker 不触发
- Edge case: activity 超时触发 Stalled 错误
- Error path: cancel 与 stall 同时发生时 cancel 优先

**Verification:**
- `cargo test -p alephcore harness::tests::test_stall_detection`
- 手动测试：模拟长时间 tool execution 触发 stall

---

- [ ] **Unit 2: 添加 Sandbox 生命周期钩子**

**Goal:** 在 SandboxFactory 添加 before_execute/after_execute 钩子

**Requirements:** R2

**Dependencies:** Unit 1

**Files:**
- Create: `src/sandbox/hooks.rs` — `SandboxHooks` trait 定义
- Modify: `src/orchestrator/sandbox_factory.rs` — 添加 hooks registry
- Modify: `src/sandbox/workspace.rs` — 调用 hooks
- Test: `src/sandbox/tests/`

**Approach:**
1. 定义 `SandboxHooks` trait（before_execute, after_execute）
2. `SandboxFactory` 添加 `hooks: Vec<Arc<dyn SandboxHooks>>` 字段
3. `WorkspaceSandbox::execute` 调用 hooks
4. 提供默认空实现 `NoopSandboxHooks`

**Patterns to follow:**
- `src/sandbox/mod.rs` Sandbox trait
- `src/orchestrator/sandbox_factory.rs` factory 模式

**Test scenarios:**
- Happy path: hooks 按序执行
- Edge case: hook 执行失败不影响 sandbox execute
- Integration: metrics hook 在 after_execute 记录执行时长

**Verification:**
- `cargo test -p alephcore sandbox::tests::test_hooks_lifecycle`

---

- [ ] **Unit 3: 添加指数退避重试辅助**

**Goal:** 提供 retry delay 计算工具，供 Gateway 层使用

**Requirements:** R3

**Dependencies:** None

**Files:**
- Create: `src/orchestrator/retry.rs` — RetryConfig, compute_retry_delay
- Modify: `src/orchestrator/mod.rs` — pub use retry module
- Test: `src/orchestrator/tests/`

**Approach:**
1. 定义 `RetryConfig` struct
2. 实现 `compute_retry_delay(attempt, config) -> Duration`
3. 实现 `should_retry(error, attempt, config) -> bool`

**Patterns to follow:**
- `src/orchestrator/errors.rs` FlowError::is_retryable()

**Test scenarios:**
- Happy path: 指数退避 delay 计算正确
- Edge case: attempt 超过 max_attempts 返回 false
- Error path: non-transient error 直接返回 false

**Verification:**
- `cargo test -p alephcore orchestrator::tests::test_retry_backoff`

---

- [ ] **Unit 4: 添加 FlowSpec 优先级字段**

**Goal:** FlowSpec 支持 priority 字段用于调度排序

**Requirements:** R4

**Dependencies:** None

**Files:**
- Modify: `src/orchestrator/flow_spec.rs` — FlowSpec 添加 `priority: u8`
- Modify: `src/orchestrator/dispatch.rs` — dispatch 时考虑优先级
- Test: `src/orchestrator/tests/`

**Approach:**
1. `FlowSpec` 添加 `priority: Option<u8>`，默认 None (=0)
2. `Orchestrator::dispatch` 返回前可按 priority 排序（如果需要）

**Patterns to follow:**
- `src/orchestrator/flow_spec.rs` FlowSpec 结构

**Test scenarios:**
- Happy path: priority 字段正确序列化/反序列化
- Edge case: None priority 当作 0 处理

**Verification:**
- `cargo test -p alephcore orchestrator::tests::test_flow_priority`

---

- [ ] **Unit 5: 添加 Orchestrator Metrics 可观测性**

**Goal:** Orchestrator 提供 metrics 接口

**Requirements:** R5

**Dependencies:** None

**Files:**
- Create: `src/orchestrator/metrics.rs` — OrchestratorMetrics struct
- Modify: `src/orchestrator/dispatch.rs` — Orchestrator 添加 metrics 方法
- Test: `src/orchestrator/tests/`

**Approach:**
1. 定义 `OrchestratorMetrics` struct（含 active_sessions count, total_dispatches, stall_count 等）
2. `Orchestrator` 添加 `async fn metrics(&self) -> OrchestratorMetrics`
3. `active_sessions` Mutex 添加 `len()` 方法

**Patterns to follow:**
- `src/session/service.rs` metrics 模式

**Test scenarios:**
- Happy path: metrics 返回正确计数
- Edge case: 并发访问 metrics 不 panic

**Verification:**
- `cargo test -p alephcore orchestrator::tests::test_metrics`

---

## System-Wide Impact

- **Harness loop**: run() 签名不变，新增 stall 检测不影响现有调用方
- **Sandbox**: hooks 失败不影响 execute 结果（log only）
- **Gateway**: retry 工具供外层 retry 循环使用
- **FlowSpec**: priority 添加为可选字段，向后兼容

## Risks & Dependencies

| Risk | Mitigation |
|------|------------|
| Stall detection 误触发（LLM long think） | 可配置 timeout，默认 5min |
| Hook 执行失败影响 sandbox | hooks 失败仅 log，不阻断 execute |
| Metrics 性能影响 | 仅在需要时调用，非热路径 |

## Documentation / Operational Notes

- 新增配置项：`stall_timeout_secs`, `stall_check_interval_secs`
- 新增 tracing span: `stall_detection`, `sandbox_hook`

## Sources & References

- Symphony orchestrator.ex (1655 行) — stall detection, retry logic
- Symphony workspace.ex — lifecycle hooks
- Aleph src/harness/agent.rs — 当前 Think→Act loop
- Aleph src/orchestrator/sandbox_factory.rs — 当前 SandboxFactory
