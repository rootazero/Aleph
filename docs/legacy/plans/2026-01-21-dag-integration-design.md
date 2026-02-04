# DAG 调度器集成设计文档

**日期**: 2026-01-21
**状态**: 已确认，待实现

## 背景

Aleph 的 Agent Loop 和 DAG 调度器虽然都已实现，但执行路径完全分离。多步骤任务（如"分析文档 → 生成prompt → 调用绘图模型"）被 LLM 直接输出最终答案，跳过了任务分解和 DAG 调度。

### 问题现状

| 组件 | 实现状态 | 使用状态 |
|------|---------|---------|
| observe-think-act-feedback | ✅ 完整 | ✅ 使用中 |
| LlmTaskPlanner (任务分解) | ✅ 完整 | ❌ 未使用 |
| DagScheduler (依赖调度) | ✅ 完整 | ❌ 未使用 |
| CoworkEngine (多步协调) | ✅ 完整 | ❌ 仅FFI桩 |

## 设计决策

| 决策点 | 选择 |
|--------|------|
| 任务分解时机 | 前置分解（LLM 判断） |
| UI 展示方式 | 混合模式（任务卡片 + 流式输出） |
| 失败处理策略 | 重试 2 次 + LLM 决策 |
| 执行确认机制 | 智能判断（低风险自动，高风险确认） |
| 上下文传递 | 混合方式（隐式累积 + 显式引用） |

## 整体架构

```
用户输入
    ↓
IntentRouter.route() [L0-L2 快速判断]
    ↓
┌─────────────────────────────────────────┐
│  TaskAnalyzer (新增)                     │
│  - 调用 LLM 判断：单步 or 多步任务？      │
│  - 单步 → 直接进入 Agent Loop            │
│  - 多步 → 调用 LlmTaskPlanner 生成 TaskGraph │
└─────────────────────────────────────────┘
    ↓ (多步任务)
┌─────────────────────────────────────────┐
│  RiskEvaluator (新增)                    │
│  - 评估 TaskGraph 中每个任务的风险等级    │
│  - 高风险任务标记需要确认                 │
└─────────────────────────────────────────┘
    ↓
┌─────────────────────────────────────────┐
│  DagScheduler (已实现，需集成)            │
│  - 解析任务依赖                          │
│  - 调度并行/串行执行                      │
│  - 每个任务节点调用 Agent Loop 执行       │
└─────────────────────────────────────────┘
    ↓
UI Callback (任务卡片 + 流式输出)
```

## 组件详细设计

### 1. TaskAnalyzer

**位置**: `core/src/dispatcher/analyzer.rs`（新建）

**职责**: 判断用户输入是单步任务还是多步任务

```rust
pub struct TaskAnalyzer {
    llm_client: Arc<dyn LlmClient>,
}

pub enum AnalysisResult {
    /// 单步任务，直接走 Agent Loop
    SingleStep { intent: String },

    /// 多步任务，需要生成 TaskGraph
    MultiStep {
        task_graph: TaskGraph,
        requires_confirmation: bool,
    },
}
```

**LLM Prompt**:
```
分析用户请求，判断是否需要多步骤执行：

用户请求: "{input}"

如果任务可以一步完成（如：回答问题、简单翻译），返回：
{"type": "single", "intent": "..."}

如果任务需要多个步骤（如：分析A然后用结果做B），返回：
{"type": "multi", "tasks": [
  {"id": "t1", "name": "...", "deps": [], "risk": "low|high"},
  {"id": "t2", "name": "...", "deps": ["t1"], "risk": "low|high"}
]}

高风险任务包括：调用外部API、发送网络请求、执行代码、修改文件
```

### 2. RiskEvaluator

**位置**: `core/src/dispatcher/risk.rs`（新建）

```rust
pub enum RiskLevel {
    Low,   // 自动执行
    High,  // 需要用户确认
}

pub struct RiskEvaluator {
    high_risk_patterns: Vec<Regex>,
}

impl RiskEvaluator {
    pub fn new() -> Self {
        Self {
            high_risk_patterns: vec![
                Regex::new(r"(?i)(api|http|request|fetch|curl)").unwrap(),
                Regex::new(r"(?i)(execute|run|eval|shell|command)").unwrap(),
                Regex::new(r"(?i)(write|delete|modify|create)\s+file").unwrap(),
                Regex::new(r"(?i)(send|post|upload|publish)").unwrap(),
                Regex::new(r"(?i)(pay|purchase|transaction|transfer)").unwrap(),
            ],
        }
    }

    pub fn evaluate(&self, task: &Task) -> RiskLevel;
    pub fn evaluate_graph(&self, graph: &TaskGraph) -> bool;
}
```

### 3. TaskContext

**位置**: `core/src/dispatcher/context.rs`（新建）

```rust
pub struct TaskContext {
    /// 隐式累积：所有已完成任务的输出
    history: Vec<TaskOutput>,

    /// 显式变量：任务 ID → 输出值
    variables: HashMap<String, serde_json::Value>,

    /// 原始用户输入
    user_input: String,
}

impl TaskContext {
    pub fn build_prompt_context(&self, task: &Task) -> String;
    pub fn record_output(&mut self, task_id: &str, output: TaskOutput);
}
```

### 4. ExecutionCallback

**位置**: `core/src/dispatcher/callback.rs`（新建）

```rust
#[uniffi::export(callback_interface)]
pub trait ExecutionCallback: Send + Sync {
    fn on_plan_ready(&self, plan: &TaskPlan);
    fn on_confirmation_required(&self, plan: &TaskPlan);
    fn on_task_start(&self, task_id: &str, task_name: &str);
    fn on_task_stream(&self, task_id: &str, chunk: &str);
    fn on_task_complete(&self, task_id: &str, summary: &str);
    fn on_task_retry(&self, task_id: &str, attempt: u32, error: &str);
    fn on_task_deciding(&self, task_id: &str, error: &str);
    fn on_all_complete(&self, summary: &str);
}

#[derive(uniffi::Record)]
pub struct TaskPlan {
    pub tasks: Vec<TaskInfo>,
    pub requires_confirmation: bool,
}

#[derive(uniffi::Record)]
pub struct TaskInfo {
    pub id: String,
    pub name: String,
    pub status: TaskStatus,
    pub dependencies: Vec<String>,
}
```

### 5. DagScheduler 增强

**位置**: `core/src/dispatcher/scheduler/dag.rs`（修改）

```rust
impl DagScheduler {
    pub async fn execute_graph(
        &self,
        graph: TaskGraph,
        executor: Arc<TaskExecutor>,
        callback: Arc<dyn ExecutionCallback>,
    ) -> Result<ExecutionResult> {
        // 1. 通知 UI 显示任务计划卡片
        callback.on_plan_ready(&graph).await;

        // 2. 检查是否需要用户确认
        if graph.requires_confirmation {
            callback.on_confirmation_required(&graph).await;
        }

        // 3. DAG 调度循环
        while let Some(ready_tasks) = self.get_ready_tasks(&graph) {
            let futures = ready_tasks.iter().map(|task| {
                self.execute_single_task(task, executor.clone(), callback.clone())
            });
            let results = join_all(futures).await;
            // 处理结果...
        }

        Ok(ExecutionResult::from(graph))
    }

    async fn execute_single_task(&self, task: &Task, ...) -> TaskResult {
        for attempt in 0..=2 {
            match executor.execute(task).await {
                Ok(output) => return TaskResult::Success { output },
                Err(e) if attempt < 2 => continue,
                Err(e) => {
                    let decision = self.ask_llm_for_decision(task, &e).await;
                    return self.handle_failure_decision(decision, task, e);
                }
            }
        }
    }
}
```

### 6. Swift UI 组件

**位置**: `platforms/macos/Aleph/Sources/Components/TaskPlanCard.swift`（新建）

```swift
struct TaskPlanCard: View {
    let plan: TaskPlan

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("📋 任务计划")
                .font(.headline)

            ForEach(plan.tasks) { task in
                HStack {
                    TaskStatusIcon(status: task.status)
                    Text(task.name)
                    if task.status == .running {
                        ProgressView().scaleEffect(0.7)
                    }
                }
            }
        }
        .padding()
        .background(Color.secondary.opacity(0.1))
        .cornerRadius(12)
    }
}
```

## 文件修改清单

### 新建文件

| 文件 | 职责 |
|------|------|
| `core/src/dispatcher/analyzer.rs` | TaskAnalyzer - 任务分析判断 |
| `core/src/dispatcher/risk.rs` | RiskEvaluator - 风险评估 |
| `core/src/dispatcher/context.rs` | TaskContext - 上下文传递 |
| `core/src/dispatcher/callback.rs` | ExecutionCallback trait 定义 |
| `platforms/macos/.../TaskPlanCard.swift` | 任务计划卡片 UI 组件 |

### 修改文件

| 文件 | 修改内容 |
|------|----------|
| `core/src/ffi/processing.rs` | 在 `process_with_agent_loop()` 中集成 TaskAnalyzer |
| `core/src/dispatcher/mod.rs` | 导出新模块，整合调度流程 |
| `core/src/dispatcher/scheduler/dag.rs` | 增加 `execute_graph()` 方法和回调支持 |
| `core/src/lib.rs` | UniFFI 导出新的回调接口和数据类型 |
| `platforms/macos/Aleph/Sources/Core/AlephBridge.swift` | 实现 ExecutionCallback |

## 集成代码

```rust
// processing.rs 核心修改
pub async fn process_with_agent_loop(...) -> Result<Response> {
    let route_result = router.route(input, None);

    match route_result {
        RouteResult::NeedsThinking(ctx) => {
            let analyzer = TaskAnalyzer::new(llm_client.clone());

            match analyzer.analyze(input).await? {
                AnalysisResult::SingleStep { intent } => {
                    run_agent_loop(ctx, callback).await
                }
                AnalysisResult::MultiStep { task_graph, .. } => {
                    let scheduler = DagScheduler::new();
                    let executor = TaskExecutor::new(llm_client, tools);
                    scheduler.execute_graph(task_graph, executor, callback).await
                }
            }
        }
        RouteResult::DirectRoute(info) => {
            handle_direct_route(info).await
        }
    }
}
```

## 实现顺序

1. **Phase 1**: 新建核心组件（analyzer, risk, context, callback）
2. **Phase 2**: 增强 DagScheduler
3. **Phase 3**: 修改 processing.rs 集成新流程
4. **Phase 4**: UniFFI 导出
5. **Phase 5**: Swift UI 组件
6. **Phase 6**: 集成测试
