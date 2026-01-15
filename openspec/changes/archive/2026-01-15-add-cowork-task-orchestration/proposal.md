# add-cowork-task-orchestration

## SUMMARY

实现 Aether Cowork Mode Phase 1：任务编排基础框架。构建 DAG 任务图结构、LLM 驱动的任务分解器、调度引擎和进度监控系统，为后续的文件操作、代码执行、多模型协作等高级功能奠定基础。

## STATUS

- **Stage**: Proposal
- **Status**: Draft
- **Created**: 2026-01-15
- **Updated**: 2026-01-15

## MOTIVATION

### Current Problem

Claude 推出了 Cowork 功能，允许用户执行复杂的多步骤任务。当前 Aether 缺乏类似能力：

1. **无任务分解能力**: 用户必须手动拆分复杂任务
2. **无并行执行**: 独立子任务无法并行处理
3. **无进度追踪**: 用户无法了解长时任务的执行状态
4. **单模型限制**: 无法为不同子任务选择最优模型

### Solution Overview

实现 **Aether Cowork Mode** - 本地优先的 AI 任务编排系统：

1. **Task Planner**: LLM 驱动的任务分解，生成 DAG 结构
2. **DAG Scheduler**: 基于依赖关系的拓扑排序和并行调度
3. **Task Monitor**: 实时进度追踪和事件广播
4. **Executor Registry**: 可扩展的执行器注册系统

### Business Value

- **短期**: 为用户提供复杂任务自动化能力
- **中期**: 为文件操作、代码执行等功能提供框架
- **长期**: 支持多模型协作，超越 Claude Cowork 的能力

## OBJECTIVES

### Phase 1 Scope (This Implementation)

**核心框架**:

1. TaskGraph 数据结构 (Task, TaskDependency, TaskGraph)
2. Task Planner (LLM 任务分解)
3. DAG Scheduler (拓扑排序 + 并行调度)
4. Task Monitor (进度追踪 + 事件系统)
5. Executor Trait + Registry (执行器抽象)
6. CoworkEngine 统一入口
7. UniFFI 绑定 (Swift 调用)
8. Progress Panel UI (SwiftUI)

**设计原则**:

- **低耦合**: 组件通过 Trait 接口通信
- **高内聚**: 每个模块单一职责
- **模块化**: 清晰的模块边界
- **可扩展**: Registry 模式支持插件

### Out of Scope

- FileOps Executor 实现 (Phase 2)
- Code Executor 实现 (Phase 3)
- Document Generator 实现 (Phase 3)
- Multi-Model Router 实现 (Phase 4)
- App Automation 实现 (Phase 3)

## DESIGN DECISIONS

### Key Architecture Choices

#### 1. Trait-Based Abstraction

所有核心组件都通过 Trait 定义接口：

```rust
pub trait TaskExecutor: Send + Sync {
    fn task_types(&self) -> Vec<TaskType>;
    async fn execute(&self, task: &Task, ctx: &ExecutionContext) -> Result<TaskResult>;
    async fn cancel(&self, task_id: &str) -> Result<()>;
}

pub trait TaskScheduler: Send + Sync {
    fn next_ready(&self, graph: &TaskGraph) -> Vec<&Task>;
    fn mark_completed(&mut self, task_id: &str);
    fn mark_failed(&mut self, task_id: &str);
}

pub trait TaskMonitor: Send + Sync {
    fn on_task_start(&self, task: &Task);
    fn on_progress(&self, task_id: &str, progress: f32);
    fn on_task_complete(&self, task: &Task, result: &TaskResult);
}
```

**Rationale**: 允许替换实现而不影响其他组件，便于测试和扩展。

#### 2. DAG-Based Task Graph

使用有向无环图表示任务依赖关系：

```rust
pub struct TaskGraph {
    pub id: String,
    pub tasks: Vec<Task>,
    pub edges: Vec<TaskDependency>,
}

pub struct TaskDependency {
    pub from: String,  // predecessor
    pub to: String,    // successor
}
```

**Rationale**: DAG 是表示任务依赖的标准方式，支持并行执行无依赖的任务。

#### 3. LLM-Driven Planning

使用 LLM 进行任务分解，而非规则引擎：

**Rationale**:
- 更灵活，能处理自然语言描述的复杂任务
- 可利用现有的 Provider 基础设施
- 用户可通过选择不同模型调整分解质量

#### 4. Event-Based Progress Tracking

使用事件系统而非轮询：

```rust
pub enum ProgressEvent {
    TaskStarted { task_id: String },
    Progress { task_id: String, progress: f32 },
    TaskCompleted { task_id: String, result: TaskResult },
    TaskFailed { task_id: String, error: String },
    GraphCompleted { graph_id: String },
}
```

**Rationale**: 事件驱动更高效，Swift UI 可订阅事件实时更新。

### Trade-offs

#### Complexity vs Flexibility

**Cost**: 引入 Trait 抽象和 Registry 模式增加代码量
**Benefit**: 便于后续扩展（添加新执行器无需修改核心代码）

**Decision**: 接受复杂度，因为 Cowork 是需要持续扩展的大型功能。

#### LLM Planning vs Rule-Based

**Cost**: LLM 调用有延迟和成本
**Benefit**: 更智能的任务分解，用户体验更好

**Decision**: 使用 LLM，同时支持用户跳过规划直接执行预定义任务。

### Alternatives Considered

**Alternative 1: 使用现有 Skills 系统**

❌ Rejected: Skills 是静态工作流，Cowork 需要动态任务分解。

**Alternative 2: 简单的线性任务队列**

❌ Rejected: 无法利用并行执行优化性能。

**Alternative 3: 依赖外部工作流引擎（如 Temporal）**

❌ Rejected: 过于重量级，违反本地优先原则。

## DEPENDENCIES

### Blocking Dependencies

- ✅ Provider 系统 (用于 LLM 调用)
- ✅ UniFFI 绑定框架
- ✅ tokio 异步运行时

### Non-Blocking Dependencies

- FileOps Executor (Phase 2)
- Code Executor (Phase 3)
- Model Router (Phase 4)

## RISKS & MITIGATIONS

### Risk 1: LLM 任务分解质量不稳定

**Risk**: LLM 可能生成不合理的任务图

**Mitigation**:
- 提供任务确认步骤，用户可修改计划
- 使用结构化 JSON 输出，验证格式正确性
- 设置 fallback：分解失败时提示用户手动拆分

**Likelihood**: Medium
**Impact**: Medium
**Status**: Mitigated

### Risk 2: 并行执行资源竞争

**Risk**: 多任务并行可能导致资源冲突

**Mitigation**:
- 设置最大并行度限制
- 执行器内部使用 Mutex 保护共享资源
- 任务级别的资源锁定机制

**Likelihood**: Low
**Impact**: Medium
**Status**: Mitigated

### Risk 3: 长时任务中断恢复

**Risk**: 应用关闭后任务状态丢失

**Mitigation**:
- Phase 1 暂不支持持久化（用户需重新执行）
- Phase 2 考虑添加检查点机制

**Likelihood**: Medium
**Impact**: Low (MVP acceptable)
**Status**: Accepted for MVP

## TESTING STRATEGY

### Unit Tests

- TaskGraph 构建和验证
- DAG Scheduler 拓扑排序算法
- Task Monitor 事件分发
- Executor Registry 注册/查找

### Integration Tests

- LLM Planner 生成 TaskGraph
- CoworkEngine 端到端执行
- Swift-Rust UniFFI 调用

### Manual Tests

- 实际任务分解效果验证
- Progress Panel UI 交互
- 取消/暂停功能

## SUCCESS CRITERIA

### Functional Requirements

- [ ] TaskGraph 数据结构完整定义
- [ ] Task Planner 能够通过 LLM 生成任务图
- [ ] DAG Scheduler 正确计算执行顺序
- [ ] Task Monitor 能够追踪进度并广播事件
- [ ] Executor Registry 支持注册和查找执行器
- [ ] CoworkEngine 提供统一 API
- [ ] UniFFI 绑定能够被 Swift 调用
- [ ] Progress Panel 显示任务进度

### Non-Functional Requirements

- [ ] 任务分解延迟 < 5s (取决于 LLM)
- [ ] 调度开销 < 10ms per task
- [ ] 代码通过 `cargo clippy`
- [ ] 测试覆盖率 > 80%

### Documentation Requirements

- [ ] 更新 CLAUDE.md 添加 Cowork 说明
- [ ] 添加 Cowork 架构文档
- [ ] 更新 docs/plans/ 设计文档

## ROLLOUT PLAN

### Phase 1: Core Framework (This Change)

1. 数据结构定义 (Task, TaskGraph, TaskStatus)
2. Task Planner 实现
3. DAG Scheduler 实现
4. Task Monitor 实现
5. Executor Registry 实现
6. CoworkEngine 统一入口
7. UniFFI 绑定
8. Progress Panel UI

### Future Phases

- Phase 2: FileOps Executor
- Phase 3: Code Executor + Document Generator
- Phase 4: Multi-Model Router

## MONITORING & METRICS

### Metrics to Track

- 任务分解成功率
- 平均任务执行时间
- 并行执行利用率
- 用户取消率

### Logging

- Planner 输出的 TaskGraph 结构
- Scheduler 调度决策
- Executor 执行结果
- 错误详情和堆栈

## OPEN QUESTIONS

1. ✅ **任务确认是否默认开启？**
   → 决定: 默认开启，可通过配置关闭

2. ✅ **最大并行度应该设为多少？**
   → 决定: 默认 4，可配置

3. 🤔 **是否需要支持任务优先级？**
   → 待定: Phase 1 暂不支持，后续根据需求添加

## NOTES

- 本提案基于 `docs/plans/2026-01-15-cowork-mode-design.md` 设计文档
- 采用渐进式实现，Phase 1 只实现核心框架
- 后续 Phase 将添加具体执行器和多模型支持
