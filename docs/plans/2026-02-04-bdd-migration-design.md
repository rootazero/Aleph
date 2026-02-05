# BDD 测试迁移设计方案

> 日期: 2026-02-04
> 状态: 已批准，待实施

## 概述

将 Aleph 项目的 72 个 Rust 测试文件从传统单元测试全面迁移到 BDD（行为驱动测试），使用 cucumber-rs 框架。

## 决策摘要

| 决策点 | 选择 |
|--------|------|
| 迁移范围 | 全量迁移（72 个测试文件） |
| Feature 组织 | 镜像源码结构 |
| Step Definitions | 分层 + 共享（common.rs + 模块专用） |
| World 设计 | 组合式（AlephWorld + 模块 Context） |
| 异步处理 | 全异步（async steps） |
| 运行器 | 单一入口 + 路径/tag 过滤 |
| 迁移策略 | 6 批次模块化迁移 |

## 目录结构

```
core/
├── Cargo.toml                      # 添加 cucumber 依赖
├── src/                            # 源码保持不变
└── tests/
    ├── cucumber.rs                 # 单一入口 (test harness)
    ├── world/                      # World 定义
    │   ├── mod.rs                  # AlephWorld 主结构
    │   ├── config_ctx.rs           # ConfigContext
    │   ├── agent_ctx.rs            # AgentContext
    │   ├── poe_ctx.rs              # PoeContext
    │   ├── memory_ctx.rs           # MemoryContext
    │   ├── gateway_ctx.rs          # GatewayContext
    │   └── daemon_ctx.rs           # DaemonContext
    ├── steps/                      # Step definitions
    │   ├── mod.rs                  # 导出所有步骤
    │   ├── common.rs               # 共享步骤 (成功/失败断言等)
    │   ├── config_steps.rs
    │   ├── daemon_steps.rs
    │   ├── agent_loop_steps.rs
    │   ├── poe_steps.rs
    │   ├── memory_steps.rs
    │   ├── gateway_steps.rs
    │   └── ...                     # 每模块一个
    └── features/                   # Gherkin 文件 (镜像 src/ 结构)
        ├── config/
        │   ├── basic.feature
        │   ├── validation.feature
        │   └── serialization.feature
        ├── daemon/
        │   ├── service_manager.feature
        │   └── launchd.feature
        ├── agent_loop/
        │   └── state_machine.feature
        ├── poe/
        │   └── cycle.feature
        └── ...
```

## World 结构设计

```rust
// tests/world/mod.rs
use cucumber::World;
use tempfile::TempDir;

mod config_ctx;
mod agent_ctx;
mod poe_ctx;
mod memory_ctx;
mod gateway_ctx;
mod daemon_ctx;

pub use config_ctx::ConfigContext;
pub use agent_ctx::AgentContext;
pub use poe_ctx::PoeContext;
pub use memory_ctx::MemoryContext;
pub use gateway_ctx::GatewayContext;
pub use daemon_ctx::DaemonContext;

#[derive(Debug, Default, World)]
pub struct AlephWorld {
    // ═══ 通用状态 ═══
    pub temp_dir: Option<TempDir>,
    pub last_result: Result<(), String>,
    pub last_error: Option<String>,

    // ═══ 模块上下文（按需初始化）═══
    pub config: Option<ConfigContext>,
    pub daemon: Option<DaemonContext>,
    pub agent: Option<AgentContext>,
    pub poe: Option<PoeContext>,
    pub memory: Option<MemoryContext>,
    pub gateway: Option<GatewayContext>,
}
```

### 模块 Context 示例

```rust
// tests/world/poe_ctx.rs
use alephcore::poe::{PoeOutcome, Budget};

#[derive(Debug, Default)]
pub struct PoeContext {
    pub worker: Option<MockWorker>,
    pub provider: Option<MockProvider>,
    pub outcome: Option<PoeOutcome>,
    pub budget: Budget,
    pub constraints: Vec<Constraint>,
}

impl PoeContext {
    pub fn with_budget(token_limit: usize, attempt_limit: usize) -> Self {
        Self {
            budget: Budget::new(token_limit, attempt_limit),
            ..Default::default()
        }
    }
}
```

## Step Definitions 结构

```rust
// tests/steps/mod.rs
mod common;
mod config_steps;
mod daemon_steps;
mod agent_loop_steps;
mod poe_steps;
mod memory_steps;
mod gateway_steps;
mod dispatcher_steps;
mod tools_steps;
mod extension_steps;

pub use common::*;
pub use config_steps::*;
pub use daemon_steps::*;
pub use agent_loop_steps::*;
pub use poe_steps::*;
pub use memory_steps::*;
pub use gateway_steps::*;
pub use dispatcher_steps::*;
pub use tools_steps::*;
pub use extension_steps::*;
```

### 共享步骤示例

```rust
// tests/steps/common.rs
use cucumber::{given, then};
use crate::world::AlephWorld;
use tempfile::tempdir;

#[given("a temporary directory")]
async fn given_temp_dir(w: &mut AlephWorld) {
    w.temp_dir = Some(tempdir().expect("Failed to create temp dir"));
}

#[then("the operation should succeed")]
async fn then_should_succeed(w: &mut AlephWorld) {
    assert!(w.last_result.is_ok(),
        "Expected success, got error: {:?}", w.last_error);
}

#[then("the operation should fail")]
async fn then_should_fail(w: &mut AlephWorld) {
    assert!(w.last_result.is_err(), "Expected failure, but succeeded");
}

#[then(expr = "the error message should contain {string}")]
async fn then_error_contains(w: &mut AlephWorld, expected: String) {
    let err = w.last_error.as_ref().expect("No error recorded");
    assert!(err.contains(&expected),
        "Error '{}' does not contain '{}'", err, expected);
}
```

### 模块专用步骤示例

```rust
// tests/steps/poe_steps.rs
use cucumber::{given, when, then};
use crate::world::{AlephWorld, PoeContext};
use alephcore::poe::{run_poe_cycle, Budget};

#[given(expr = "a POE context with budget \\({int} tokens, {int} attempts\\)")]
async fn given_poe_budget(w: &mut AlephWorld, tokens: usize, attempts: usize) {
    w.poe = Some(PoeContext::with_budget(tokens, attempts));
}

#[given("a mock worker that always succeeds")]
async fn given_mock_worker_success(w: &mut AlephWorld) {
    let ctx = w.poe.get_or_insert_with(PoeContext::default);
    ctx.worker = Some(MockWorker::always_success());
}

#[when("I run the POE cycle")]
async fn when_run_poe(w: &mut AlephWorld) {
    let ctx = w.poe.as_mut().expect("POE context not initialized");
    let outcome = run_poe_cycle(
        ctx.worker.take().unwrap(),
        ctx.provider.take().unwrap(),
        &ctx.constraints,
        ctx.budget.clone(),
    ).await;
    ctx.outcome = Some(outcome);
}

#[then("the POE outcome should be Success")]
async fn then_poe_success(w: &mut AlephWorld) {
    let ctx = w.poe.as_ref().expect("POE context not initialized");
    assert!(matches!(ctx.outcome, Some(PoeOutcome::Success(_))));
}
```

## 测试运行器配置

```rust
// tests/cucumber.rs
mod world;
mod steps;

use cucumber::{World, WriterExt};
use world::AlephWorld;

#[tokio::main]
async fn main() {
    AlephWorld::cucumber()
        .max_concurrent_scenarios(4)
        .with_writer(
            cucumber::writer::Basic::raw(std::io::stdout(), cucumber::writer::Coloring::Auto, 0)
                .summarized()
                .assert_normalized(),
        )
        .run("tests/features")
        .await;
}
```

### Cargo.toml 配置

```toml
[dev-dependencies]
cucumber = { version = "0.21", features = ["macros"] }
tempfile = "3"
tokio = { version = "1", features = ["full", "test-util"] }

[[test]]
name = "cucumber"
harness = false
```

### 运行命令

```bash
# 运行全部 BDD 测试
cargo test --test cucumber

# 只运行特定模块
cargo test --test cucumber -- tests/features/poe/

# 只运行带特定 tag 的场景
cargo test --test cucumber -- --tags @critical

# 并行 + 详细输出
cargo test --test cucumber -- --concurrent 8 -v
```

## 迁移批次计划

| 批次 | 模块 | 源文件 | 目标 Feature | 预估场景数 |
|------|------|--------|--------------|-----------|
| **1** | config | `src/config/tests/*.rs` (9 files) | `features/config/*.feature` | ~30 |
| | scripting | `tests/scripting_engine_test.rs` | `features/scripting/engine.feature` | ~5 |
| **2** | daemon | `src/daemon/tests/*.rs` (9 files) | `features/daemon/*.feature` | ~25 |
| | perception | `src/daemon/perception/tests/*.rs` (9 files) | `features/perception/*.feature` | ~20 |
| **3** | agent_loop | `src/agent_loop/*_tests.rs` (3 files) | `features/agent_loop/*.feature` | ~15 |
| | poe | `src/poe/tests.rs` | `features/poe/*.feature` | ~40 |
| | thinker | `src/thinker/*_tests.rs` | `features/thinker/*.feature` | ~10 |
| **4** | memory | `src/memory/**/*tests.rs` (2 files) | `features/memory/*.feature` | ~15 |
| | dispatcher | `src/dispatcher/cortex/tests.rs` | `features/dispatcher/*.feature` | ~10 |
| **5** | gateway | `src/gateway/*_tests.rs` | `features/gateway/*.feature` | ~15 |
| | tools | `tests/tool_*.rs` | `features/tools/*.feature` | ~20 |
| | extension | `src/extension/**/tests.rs` | `features/extension/*.feature` | ~10 |
| **6** | 顶层集成 | `tests/*_integration*.rs` (24 files) | `features/integration/*.feature` | ~60 |

### 每批次交付物

1. 新建的 `.feature` 文件
2. 对应的 `*_steps.rs` 文件
3. 更新 `world/*.rs` 添加所需 Context
4. 删除旧的 `*_test.rs` 文件
5. 验证：`cargo test --test cucumber -- tests/features/<module>/`

### 批次间验收标准

- 所有新 BDD 场景通过
- 旧测试文件已删除
- `cargo test` 整体通过（包括未迁移的传统测试）

## 转换原则

将现有测试转换为 BDD 时遵循：

- 一个 `#[test]` 函数 → 一个 `Scenario`
- 测试的 setup 代码 → `Given` 步骤
- 被测行为 → `When` 步骤
- 断言 → `Then` 步骤
- 相关测试共享 setup → `Background`
- 慢测试用 `@slow` tag 标记

### 转换示例

**原始测试：**
```rust
#[tokio::test]
async fn test_poe_budget_exhausted() {
    let worker = MockWorker::always_fail();
    let provider = MockProvider::new();
    let budget = Budget::new(1000, 3);

    let outcome = run_poe_cycle(worker, provider, &[], budget).await;

    assert!(matches!(outcome, PoeOutcome::BudgetExhausted { .. }));
}
```

**转换后：**
```gherkin
Feature: POE Budget Management
  As the system
  I want to enforce budget limits on POE cycles
  So that runaway tasks don't consume unlimited resources

  Background:
    Given a mock provider

  Scenario: Budget exhausted after max attempts
    Given a POE context with budget (1000 tokens, 3 attempts)
    And a mock worker that always fails
    When I run the POE cycle
    Then the POE outcome should be BudgetExhausted
```

## 预估工作量

- ~240 个 Scenario（基于现有测试函数数量）
- ~15 个 Feature 文件目录
- ~12 个 Step 文件
- ~7 个 Context 结构
