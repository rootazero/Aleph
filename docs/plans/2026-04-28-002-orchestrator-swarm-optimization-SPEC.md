# SPEC: Aleph Orchestrator & Swarm 优化

> 基于 Symphony (Elixir) vs Aleph (Rust) 架构对比分析
> 目标：非破坏性重构，重点修复错误、优化性能、增强功能

---

## 1. 背景与问题

### 1.1 Symphony 优势特性 Aleph 缺失

| 特性 | Symphony 实现 | Aleph 现状 | 优先级 |
|------|-------------|-----------|--------|
| **Stall Detection** | `last_codex_timestamp` + 300s timeout | ❌ 缺失 | **P0** |
| **生命周期钩子** | after_create/before_run/after_run/before_remove | ❌ 缺失 | **P0** |
| **指数退避重试** | `min(10_000 * 2^n, 300_000)` | ⚠️ 仅 LLM 层 | P1 |
| **FlowSpec 优先级** | Issue 优先级队列 | ⚠️ 部分 | P2 |
| **Orchestrator Metrics** | Slot 监控、stall 计数 | ❌ 缺失 | P2 |

### 1.2 约束

- 非破坏性重构，不删除现有功能
- 利用 Rust 类型安全和 trait 系统
- 与 Aleph 现有架构（Swarm、AgentHarness、Session）无缝融合
- 充分利用现有模块和代码，不重复设计

---

## 2. 研究发现

### 2.1 Aleph 已有实现

| 模块 | 路径 | 已有功能 |
|------|------|----------|
| 指数退避 | `src/providers/llm_retry.rs` | `RetryVerdict` + exponential backoff（MAX_DELAY=30s） |
| Sandbox trait | `src/sandbox/mod.rs` | `execute(command) -> Result<SandboxOutput>` |
| Harness run loop | `src/harness/agent.rs` | `cancel.is_cancelled()` 检查 |
| Cancellation | `src/core/termination.rs` | `CancellationToken` + `is_cancelled()` |
| Session idle timeout | `src/session/actor.rs` | `idle_timeout` 机制 |

### 2.2 Symphony 可借鉴模式

#### Stall Detection（Orchestrator）

```elixir
# Symphony: 基于 last_codex_timestamp 检测
defp stall_elapsed_ms(running_entry, now) do
  last_activity = running_entry.last_codex_timestamp || running_entry.started_at
  DateTime.diff(now, last_activity, :millisecond)
end

# 公式: min(10_000 * 2^min(attempt-1, 10), 300_000)
# Attempt 1: 10s, Attempt 2: 20s, Attempt 3: 40s, ... Attempt 6+: 300s（封顶）
```

#### 生命周期钩子（Workspace）

```elixir
# Hooks 配置
field :after_create, :string   # workspace 创建后
field :before_run, :string      # agent 执行前（失败则停止）
field :after_run, :string       # agent 执行后（失败忽略）
field :before_remove, :string   # workspace 删除前
field :timeout_ms, :integer, default: 60_000
```

#### Rust 等效实现

| Elixir | Rust |
|--------|------|
| `Task.async/1` + `Task.yield/2` | `tokio::spawn` + `tokio::time::timeout` |
| `System.cmd/3` | `tokio::process::Command` |
| `Path.expand/1` | `std::fs::canonicalize` |
| Exponential backoff | `backon` crate 或自定义 |

---

## 3. 实施规范

### 3.1 Unit 1: Stall Detection（优先级 P0）

#### 目标
在 `AgentHarness::run` 循环中添加 activity timeout 检测，防止 agent 卡死

#### 设计

```rust
// src/harness/stall.rs（新建）

/// Stall 配置
#[derive(Debug, Clone)]
pub struct StallConfig {
    /// 无 activity 超时阈值（默认 5 分钟）
    pub timeout: Duration,
    /// 检查间隔（默认 30 秒）
    pub check_interval: Duration,
}

impl Default for StallConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(300),      // 5 分钟
            check_interval: Duration::from_secs(30), // 30 秒
        }
    }
}

/// Stall Tracker
pub struct StallTracker {
    last_activity: Arc<Mutex<Instant>>,
    config: StallConfig,
    cancel: CancellationToken,
}

impl StallTracker {
    pub fn new(config: StallConfig, cancel: CancellationToken) -> Self {
        Self {
            last_activity: Arc::new(Mutex::new(Instant::now())),
            config,
            cancel,
        }
    }

    /// 每次 run_turn 成功后调用
    pub fn tick(&self) {
        *self.last_activity.lock() = Instant::now();
    }

    /// 检查是否 stall
    pub fn is_stalled(&self) -> bool {
        if self.cancel.is_cancelled() {
            return false; // cancellation 优先
        }
        let elapsed = self.last_activity.lock().elapsed();
        elapsed > self.config.timeout
    }
}
```

#### 修改文件

| 文件 | 修改内容 |
|------|----------|
| `src/harness/deps.rs` | 添加 `StallConfig` 到 `HarnessDeps` |
| `src/harness/trait_def.rs` | `HarnessError` 添加 `Stalled` variant |
| `src/harness/agent.rs` | `run` 循环集成 `StallTracker` |

#### run 循环修改（伪代码）

```rust
// src/harness/agent.rs run() 方法
loop {
    // 1. Cancellation 检查
    if self.cancel.is_cancelled() {
        return Err(HarnessError::Cancelled);
    }

    // 2. Stall 检测（新增）
    if self.stall_tracker.is_stalled() {
        return Err(HarnessError::Stalled {
            elapsed: self.stall_tracker.elapsed(),
        });
    }

    // 3. 执行 turn
    match self.run_turn().await? {
        TurnResult::Continue => {
            self.stall_tracker.tick();  // activity 更新
            continue;
        }
        TurnResult::Done => return Ok(()),
    }
}
```

#### 测试场景

| 场景 | 预期 |
|------|------|
| agent 正常完成 | stall tracker 不触发 |
| 超过 timeout 无 activity | 返回 `HarnessError::Stalled` |
| cancel 与 stall 同时发生 | cancel 优先 |

---

### 3.2 Unit 2: Sandbox 生命周期钩子（优先级 P0）

#### 目标
在 `WorkspaceSandbox` 添加 before_execute/after_execute 钩子

#### 设计

```rust
// src/sandbox/hooks.rs（新建）

use async_trait::async_trait;

/// Sandbox 生命周期钩子
#[async_trait]
pub trait SandboxHooks: Send + Sync {
    /// execute 前调用（失败会导致 sandbox error）
    async fn before_execute(&self, ctx: &HookContext) -> Result<(), HookError>;

    /// execute 后调用（失败仅 log，不阻断）
    async fn after_execute(
        &self,
        ctx: &HookContext,
        result: &Result<SandboxOutput, SandboxError>,
    );
}

#[derive(Debug)]
pub struct HookContext {
    pub session_id: SessionId,
    pub workspace_path: PathBuf,
    pub sandbox_kind: SandboxKind,
}

/// 无操作钩子
pub struct NoopSandboxHooks;

#[async_trait]
impl SandboxHooks for NoopSandboxHooks {
    async fn before_execute(&self, _ctx: &HookContext) -> Result<(), HookError> {
        Ok(())
    }
    async fn after_execute(&self, _ctx: &HookContext, _result: &Result<SandboxOutput, SandboxError>) {}
}
```

#### SandboxFactory 修改

```rust
// src/orchestrator/sandbox_factory.rs

pub struct SandboxFactory {
    inner: Arc<dyn Fn(SandboxKind, &str) -> Result<Arc<dyn Sandbox>, FlowError>>,
    hooks: Vec<Arc<dyn SandboxHooks>>,  // 新增
}
```

#### WorkspaceSandbox 修改

```rust
// src/sandbox/workspace.rs execute() 方法

pub async fn execute(
    &self,
    command: SandboxCommand,
) -> Result<SandboxOutput, SandboxError> {
    let ctx = HookContext {
        session_id: self.session_id,
        workspace_path: self.workspace_path.clone(),
        sandbox_kind: self.kind,
    };

    // before_execute 钩子
    for hook in &self.factory.hooks {
        hook.before_execute(&ctx).await?;
    }

    let result = self.inner_execute(command).await;

    // after_execute 钩子（失败仅 log）
    for hook in &self.factory.hooks {
        if let Err(e) = hook.after_execute(&ctx, &result).await {
            tracing::warn!(?e, "after_execute hook failed");
        }
    }

    result
}
```

#### 修改文件

| 文件 | 修改内容 |
|------|----------|
| `src/sandbox/hooks.rs` | 新建 Hook trait + Noop 实现 |
| `src/orchestrator/sandbox_factory.rs` | 添加 `hooks` 字段 |
| `src/sandbox/workspace.rs` | `execute()` 调用钩子 |

#### 测试场景

| 场景 | 预期 |
|------|------|
| hooks 按序执行 | before → execute → after |
| before_execute 失败 | sandbox error 向上传播 |
| after_execute 失败 | 仅 warn log，不影响结果 |

---

### 3.3 Unit 3: 指数退避重试辅助（优先级 P1）

#### 目标
提供 retry delay 计算工具，供 Gateway 层使用（Aleph LLM 层已有，扩展到 Orchestrator 层）

#### 设计

```rust
// src/orchestrator/retry.rs（新建）

use std::time::Duration;

/// Retry 配置
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// 基础延迟（默认 10s）
    pub base_delay: Duration,
    /// 最大延迟（默认 300s = 5 分钟）
    pub max_delay: Duration,
    /// 最大重试次数
    pub max_attempts: u32,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            base_delay: Duration::from_secs(10),
            max_delay: Duration::from_secs(300),
            max_attempts: 6,
        }
    }
}

/// 计算重试延迟
///
/// Formula: min(base_delay * 2^min(attempt-1, 10), max_delay)
/// Attempt 1: base_delay, Attempt 2: 2x, Attempt 3: 4x, ...
pub fn compute_retry_delay(attempt: u32, config: &RetryConfig) -> Duration {
    if attempt == 0 {
        return config.base_delay;
    }
    let exp = attempt.min(10);
    let multiplier = 1u64 << exp; // 2^exp
    let delay = config.base_delay.mul_f64(multiplier as f64);
    config.max_delay.min(delay)
}

/// 判断是否应该重试
pub fn should_retry(attempt: u32, config: &RetryConfig) -> bool {
    attempt < config.max_attempts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_retry_delay() {
        let config = RetryConfig::default();
        assert_eq!(compute_retry_delay(1, &config), Duration::from_secs(10));
        assert_eq!(compute_retry_delay(2, &config), Duration::from_secs(20));
        assert_eq!(compute_retry_delay(3, &config), Duration::from_secs(40));
        assert_eq!(compute_retry_delay(6, &config), Duration::from_secs(320)); // capped
    }
}
```

#### 修改文件

| 文件 | 修改内容 |
|------|----------|
| `src/orchestrator/retry.rs` | 新建 RetryConfig + compute_retry_delay |
| `src/orchestrator/mod.rs` | `pub use retry::{RetryConfig, compute_retry_delay, should_retry}` |

---

### 3.4 Unit 4: FlowSpec 优先级（优先级 P2）

#### 目标
FlowSpec 支持 priority 字段用于调度排序

#### 设计

```rust
// src/orchestrator/flow_spec.rs FlowSpec 结构添加

pub struct FlowSpec {
    pub name: SmolStr,
    pub version: Option<Version>,
    pub depth: u8,           // 已有
    pub priority: Option<u8>, // 新增：优先级（值越大优先级越高）
    pub steps: Vec<FlowStep>,
    // ...
}

impl FlowSpec {
    /// 获取优先级（None 当作 0）
    pub fn priority(&self) -> u8 {
        self.priority.unwrap_or(0)
    }
}
```

#### 修改文件

| 文件 | 修改内容 |
|------|----------|
| `src/orchestrator/flow_spec.rs` | 添加 `priority` 字段 |
| `src/orchestrator/dispatch.rs` | dispatch 时按 priority 排序 |

---

### 3.5 Unit 5: Orchestrator Metrics（优先级 P2）

#### 目标
Orchestrator 提供 metrics 接口

#### 设计

```rust
// src/orchestrator/metrics.rs（新建）

use std::sync::Arc;

/// Orchestrator 指标
#[derive(Debug, Clone, Default)]
pub struct OrchestratorMetrics {
    /// 当前运行中的 session 数量
    pub active_sessions: usize,
    /// 总 dispatch 次数
    pub total_dispatches: u64,
    /// Stall 触发次数
    pub stall_count: u64,
    /// 重试次数
    pub retry_count: u64,
}

pub trait MetricsCollector: Send + Sync {
    fn metrics(&self) -> OrchestratorMetrics;
}
```

#### 修改文件

| 文件 | 修改内容 |
|------|----------|
| `src/orchestrator/metrics.rs` | 新建 Metrics structs |
| `src/orchestrator/dispatch.rs` | 添加 metrics 字段和收集逻辑 |

---

## 4. 系统影响

| 模块 | 影响 |
|------|------|
| Harness loop | `run()` 签名不变，新增 stall 检测不影响现有调用方 |
| Sandbox | hooks 失败仅 log，不阻断 execute |
| Gateway | retry 工具供外层 retry 循环使用 |
| FlowSpec | priority 添加为可选字段，向后兼容 |

---

## 5. 风险与缓解

| 风险 | 缓解措施 |
|------|----------|
| Stall 误触发（LLM long think） | 可配置 timeout，默认 5 分钟 |
| Hook 执行失败影响 sandbox | hooks 失败仅 log，不阻断 execute |
| Metrics 性能影响 | 仅在需要时调用，非热路径 |

---

## 6. 实施顺序

```
1. Unit 3 (Retry 辅助) → 最简单，可独立
2. Unit 1 (Stall Detection) → 核心功能
3. Unit 2 (Sandbox Hooks) → 依赖 Unit 1 配置
4. Unit 4 (FlowSpec Priority) → 可选增强
5. Unit 5 (Metrics) → 可选增强
```

---

## 7. 验证

| Unit | 测试命令 |
|------|----------|
| Unit 1 | `cargo test -p alephcore harness::tests::test_stall_detection` |
| Unit 2 | `cargo test -p alephcore sandbox::tests::test_hooks_lifecycle` |
| Unit 3 | `cargo test -p alephcore orchestrator::tests::test_retry_backoff` |
| Unit 4 | `cargo test -p alephcore orchestrator::tests::test_flow_priority` |
| Unit 5 | `cargo test -p alephcore orchestrator::tests::test_metrics` |

---

## 8. 参考文件

| 文件 | 用途 |
|------|------|
| `src/providers/llm_retry.rs` | 指数退避模式参考 |
| `src/sandbox/mod.rs` | Sandbox trait |
| `src/harness/agent.rs` | Harness run loop |
| `src/harness/trait_def.rs` | HarnessError |
| `src/orchestrator/sandbox_factory.rs` | SandboxFactory |
| Symphony `orchestrator.ex` | Stall detection + retry 逻辑 |
| Symphony `workspace.ex` | 生命周期钩子 |
