# Rust 大文件拆分重构设计

> Date: 2026-02-27
> Status: Approved
> Scope: Top 5 largest Rust files — pure module splitting, no logic changes

---

## 目标

在**不改变任何外部行为和功能逻辑**的前提下，将项目中 5 个超过 1000 行的 Rust 文件拆分为职责清晰的子模块。

### 铁律

1. **语义等价**：Public APIs、Error Variants、Send/Sync 行为完全一致
2. **零运行时损失**：不引入额外 clone()、Box<T> 或动态派发
3. **保留防御性约束**：不移除必要的 Option/Result/assert!

### 总体策略

**方案 A：纯模块拆分** — 按职责将大文件拆为子模块，通过 `pub use` 保持导入路径兼容。后续迭代可做 clone 优化和宏消重。

---

## 1. thinker/prompt_builder.rs (1,939 行 → 最大 ~350 行)

### 诊断

- 11 个 `append_*` 方法已迁移至 PromptPipeline 但仍保留
- 39 个测试挤在一个 `mod tests` 中
- 6 个 `build_system_prompt_*` 入口点混在一起

### 拆分结构

```
thinker/
├── prompt_builder/
│   ├── mod.rs              # PromptBuilder struct, new(), 6个 build_* 入口点 (~150行)
│   ├── sections.rs         # 公开的 append_* 方法 (soul, env, security, safety 等) (~350行)
│   ├── messages.rs         # build_messages(), build_observation(), Message struct (~200行)
│   ├── cache.rs            # build_system_prompt_cached(), build_static_header() (~50行)
│   └── tests/
│       ├── mod.rs          # Test utilities, helper fns
│       ├── build_tests.rs  # build_* 入口点测试
│       ├── section_tests.rs # append_* 方法测试
│       └── sanitize_tests.rs # 18个 sanitization 测试
```

### 删除候选

11 个已迁移到 PromptPipeline layers 的私有 append 方法：
- `append_tools()`, `append_runtime_capabilities()`, `append_generation_models()`
- `append_special_actions()`, `append_response_format()`, `append_guidelines()`
- `append_thinking_guidance()`, `append_skill_mode()`, `append_skill_instructions()`
- `append_custom_instructions()`, `append_language_setting()`

**条件**：确认这些方法无外部调用者后删除。如有测试直接调用，将测试改为通过 pipeline 路径验证。

### 语义保证

- `PromptBuilder` struct 不变
- 所有 `pub fn` 签名不变
- 通过 `mod.rs` 的 `pub use` 保持 `use crate::thinker::prompt_builder::*` 兼容

---

## 2. gateway/execution_engine.rs (1,044 行 → 最大 ~350 行)

### 诊断

- 两个并行引擎 (ExecutionEngine + SimpleExecutionEngine) 混在一起
- 死代码：`abort_senders` 字段 + `store_abort_sender()` 方法
- 测试只覆盖 Simple 版本

### 拆分结构

```
gateway/
├── execution_engine/
│   ├── mod.rs              # 公共类型: ExecutionEngineConfig, RunRequest, RunState,
│   │                       #   RunStatus, ActiveRun, ExecutionError, ExecutionAdapter trait (~180行)
│   ├── engine.rs           # ExecutionEngine<P,R> impl + ExecutionAdapter impl (~350行)
│   ├── simple.rs           # SimpleExecutionEngine impl + ExecutionAdapter impl (~200行)
│   └── tests.rs            # TestEmitter + 测试 (~120行)
```

### 删除候选

- `abort_senders: HashMap<String, ...>` 字段 — 存储但从未读取
- `store_abort_sender()` 方法 — 配套死代码
- `ActiveRun::chunk_counter` — 仅 SimpleExecutionEngine 使用，可移至 simple.rs 局部

### 语义保证

- `ExecutionAdapter` trait 签名不变
- 所有 `pub` 类型通过 `mod.rs` 重导出
- `RunState`, `ExecutionError` enum 不变

---

## 3. memory/context.rs (1,302 行 → 最大 ~450 行)

### 诊断

- 纯数据定义文件，无复杂业务逻辑
- 8 个 enum 各有 ~50 行的 as_str/FromStr/Display 样板（未来宏化的完美候选）
- `MemoryFact` 聚合根有 30 字段 + 15 个 builder 方法

### 拆分结构

```
memory/
├── context/
│   ├── mod.rs              # ContextAnchor, MemoryEntry, 常量, pub use 重导出 (~130行)
│   ├── enums.rs            # 8个枚举: FactType, FactSource, MemoryLayer, MemoryCategory,
│   │                       #   MemoryTier, MemoryScope, FactSpecificity, TemporalScope (~450行)
│   ├── fact.rs             # MemoryFact 聚合根 + Entity/AggregateRoot impl + builders (~280行)
│   ├── compression.rs      # CompressionSession, FactStats, CompressionResult (~80行)
│   ├── paths.rs            # PRESET_PATHS, compute_parent_path() (~30行)
│   └── tests/
│       ├── mod.rs
│       ├── enum_tests.rs   # 枚举序列化测试
│       └── fact_tests.rs   # MemoryFact builder 测试
```

### 语义保证

- 所有 `pub struct/enum` 通过 mod.rs 重导出
- `use crate::memory::context::*` 导入路径不变
- `MemoryFact` 的 `Entity + AggregateRoot` 实现不变
- serde derive 属性不变

---

## 4. cron/mod.rs (1,356 行 → 最大 ~260 行)

### 诊断

- CronService 的 21 个方法全部在一个 `impl` 块中
- 职责分层清晰（初始化/CRUD/查询/调度/维护）但物理上未分离

### 拆分结构

```
cron/
├── mod.rs                  # CronService struct, CronError, 类型别名,
│                           #   pub use 重导出, new(), set_executor() (~130行)
├── schema.rs               # init_schema(), migrate_schema() (~120行)
├── crud.rs                 # add_job, update_job, delete_job, enable_job, disable_job (~260行)
├── query.rs                # get_job, list_jobs, get_job_runs, row_to_cron_job,
│                           #   row_to_job_run, JOBS_SELECT, RUNS_SELECT (~160行)
├── executor.rs             # check_and_run_jobs(), finalize_job_sync(), save_run_sync() (~250行)
├── lifecycle.rs            # start(), stop(), startup_catchup(), cleanup_history() (~130行)
├── tests.rs                # 14个测试 (~210行)
├── config.rs               # (已存在，不动)
├── chain.rs                # (已存在，不动)
├── delivery.rs             # (已存在，不动)
├── resource.rs             # (已存在，不动)
├── scheduler.rs            # (已存在，不动)
├── template.rs             # (已存在，不动)
└── webhook_target.rs       # (已存在，不动)
```

### 关键设计决策

所有新文件中的函数仍然是 `impl CronService` 的方法。Rust 允许同一 struct 在多个文件中有 `impl` 块。

### 语义保证

- `CronService` struct 定义不变
- 所有 `pub async fn` 签名不变
- Feature gate (`#[cfg(feature = "cron")]`) 保持在相应函数上

---

## 5. poe/worker.rs (1,128 行 → 最大 ~280 行)

### 诊断

- Worker trait + 3 个实现 + PoeLoopCallback + Gateway 工厂函数全在一个文件
- `#[allow(dead_code)]` 的 workspace 字段（架构预留）
- restore() 仅做验证，未实现完整回滚

### 拆分结构

```
poe/
├── worker/
│   ├── mod.rs              # Worker trait, StateSnapshot, WorkerOutput (~140行)
│   ├── agent_loop_worker.rs # AgentLoopWorker<T,E,C> 实现 (~280行)
│   ├── callback.rs         # PoeLoopCallback (artifact tracking) (~180行)
│   ├── gateway.rs          # GatewayAgentLoopWorker 类型别名 + create_gateway_worker() (~100行)
│   ├── placeholder.rs      # PlaceholderWorker (~80行)
│   └── tests/
│       ├── mod.rs
│       ├── mock_worker.rs   # MockWorker (#[cfg(test)]) (~120行)
│       └── worker_tests.rs  # 12个测试 (~170行)
```

### 语义保证

- `Worker` trait 签名不变
- `StateSnapshot` 不变
- `create_gateway_worker()` 公开签名不变
- `MockWorker` 仅 `#[cfg(test)]` 下可见

---

## 总结

| 文件 | 原始行数 | 拆分后最大文件 | 新文件数 | 删除行数 (预估) |
|------|---------|--------------|---------|---------------|
| prompt_builder.rs | 1,939 | ~350 | 8 | ~200 (死代码) |
| execution_engine.rs | 1,044 | ~350 | 4 | ~30 (死代码) |
| context.rs | 1,302 | ~450 | 8 | 0 |
| cron/mod.rs | 1,356 | ~260 | 6 | 0 |
| poe/worker.rs | 1,128 | ~280 | 8 | 0 |
| **合计** | **6,769** | — | **34** | **~230** |

### 执行顺序

1. prompt_builder.rs — 最大文件，最近有 Pipeline 重构，趁热打铁
2. execution_engine.rs — 有明确死代码可删
3. context.rs — 纯数据，最安全
4. cron/mod.rs — 独立模块，风险低
5. poe/worker.rs — 依赖较多，最后做

### 验证策略

每个文件拆分后：
1. `cargo check` — 编译通过
2. `cargo test` — 原有测试全部通过
3. `cargo clippy` — 无新增警告

---

## 后续迭代（不在本次范围内）

- **宏消重**：为 memory/context/enums.rs 中 8 个 enum 引入 `define_str_enum!` 宏
- **clone 优化**：全局消除不必要的 .clone() (976 处)
- **死代码清理**：清理 49 个 `#[allow(dead_code)]` 文件
- **更多大文件**：推进 extension/loader.rs (1,080行)、memory/store/lance/facts.rs (1,044行) 等
