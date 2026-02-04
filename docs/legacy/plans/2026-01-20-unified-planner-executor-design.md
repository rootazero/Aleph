# Unified Planner & Executor Architecture Design

> **Date**: 2026-01-20
> **Status**: Approved
> **Goal**: 将 Cowork 功能融入 Aleph 核心，实现 Manus 式的无缝 AI Agent 体验

## 1. 背景与问题

### 当前架构的问题

Aleph 当前存在多个功能重叠的模块：

| 模块 | 职责 | 问题 |
|------|------|------|
| **Intent** 3层分类 | 判断是否为可执行任务 | 与 Command NL 检测重叠 |
| **Command** NL 检测 | 识别自然语言中的命令 | 与 Intent 分类目标相似 |
| **Dispatcher** 3层路由 | 选择哪个工具 | 与 Intent 分类逻辑类似 |
| **Cowork** DAG 调度 | 分解和并行执行复杂任务 | 需要显式 `/agent` 触发，孤立存在 |
| **Agent** 执行引擎 | 实际执行工具 | 只处理单次调用，不与 Cowork 集成 |

**核心问题**：Cowork 是一个"旁路系统"，需要用户主动调用 `/agent` 命令，而不是自动融入主流程。这与 Manus 的"对话即完成一切"理念相悖。

### 目标

- 用户无需区分"对话模式"和"Agent模式"
- 系统自动判断请求复杂度并选择执行策略
- 删除 `/agent` 命令和协作模式配置
- 消除模块间的冗余设计

## 2. 新架构总览

```
┌─────────────────────────────────────────────────────────────┐
│                      用户输入                                │
└─────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────┐
│                  L1: 斜杠命令快速路由                        │
│  ───────────────────────────────────────────────────────── │
│  /search xxx  → SearchAction                                │
│  /youtube xxx → YouTubeAction                               │
│  /skill-name  → SkillAction                                 │
│  保留 command/parser.rs 的斜杠解析能力                       │
└─────────────────────────────────────────────────────────────┘
                    ↓ (非斜杠命令)
┌─────────────────────────────────────────────────────────────┐
│                  L3: AI 统一规划器                           │
│  ───────────────────────────────────────────────────────── │
│  输入: 用户请求 + 可用工具列表 + 上下文                      │
│  输出: ExecutionPlan                                        │
│        - Conversational (纯对话)                            │
│        - SingleAction (单工具)                              │
│        - TaskGraph (多步骤 DAG)                             │
│  新建 planner/ 模块，复用 cowork/planner 的 LLM 规划能力    │
└─────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────┐
│                  统一执行器                                  │
│  ───────────────────────────────────────────────────────── │
│  Conversational → 直接 LLM 对话                             │
│  SingleAction   → Agent 单次工具调用                        │
│  TaskGraph      → DAG 调度器并行执行                        │
│  复用 agent/ 和 cowork/scheduler + executor                 │
└─────────────────────────────────────────────────────────────┘
```

**核心变化**：删除 Intent 和 Dispatcher 模块，用"L1 快速路由 + L3 AI 规划"取代原来的 6 层判断逻辑。

## 3. 模块删除与保留

### 3.1 删除的模块

| 模块 | 路径 | 删除原因 |
|------|------|----------|
| **intent/** | `core/src/intent/` | 整个模块删除。L1/L2 本地匹配被 AI 规划取代，L3 AI 检测合并到统一规划器 |
| **dispatcher/** | `core/src/dispatcher/` | 整个模块删除。3层路由逻辑被统一规划器取代 |
| **command/nl_detector.rs** | `core/src/command/` | NL 命令检测被 AI 规划取代 |
| **command/unified_index.rs** | `core/src/command/` | 关键词索引不再需要 |

### 3.2 保留并复用的模块

| 模块 | 路径 | 保留内容 |
|------|------|----------|
| **command/** | `core/src/command/` | 保留 `parser.rs` 的斜杠命令解析、`registry.rs` 命令注册 |
| **cowork/** | `core/src/cowork/` | 保留 `scheduler/` DAG 调度、`executor/` 执行器框架、`types/` 数据结构 |
| **agent/** | `core/src/agent/` | 保留 `manager.rs` rig-core 集成、工具调用能力 |

### 3.3 新建的模块

| 模块 | 路径 | 职责 |
|------|------|------|
| **planner/** | `core/src/planner/` | 统一规划器，生成 ExecutionPlan |
| **executor/** | `core/src/executor/` | 统一执行器，执行任何类型的 Plan |

### 3.4 文件数量变化估算

```
删除: ~25 个文件 (intent/ 全部, dispatcher/ 全部, command 部分)
新建: ~8 个文件 (planner/, executor/)
修改: ~10 个文件 (ffi/processing.rs, cowork/, agent/)
```

## 4. 统一规划器 (UnifiedPlanner) 设计

### 4.1 核心数据结构

```rust
// core/src/planner/types.rs

/// 执行计划 - AI 规划器的输出
pub enum ExecutionPlan {
    /// 纯对话，无需工具
    Conversational {
        enhanced_prompt: Option<String>,  // 可选的提示增强
    },

    /// 单一动作（工具调用或简单任务）
    SingleAction {
        tool_name: String,
        parameters: serde_json::Value,
        requires_confirmation: bool,
    },

    /// 复杂任务图（多步骤）
    TaskGraph {
        tasks: Vec<PlannedTask>,
        dependencies: Vec<(usize, usize)>,  // (前置, 后继)
        requires_confirmation: bool,
    },
}

/// 规划的任务
pub struct PlannedTask {
    pub id: usize,
    pub description: String,
    pub task_type: TaskType,      // 复用 cowork/types
    pub tool_hint: Option<String>, // 建议使用的工具
    pub parameters: serde_json::Value,
}
```

### 4.2 规划器接口

```rust
// core/src/planner/mod.rs

pub struct UnifiedPlanner {
    llm_client: Arc<dyn CompletionModel>,
    available_tools: Vec<ToolMetadata>,
    planning_prompt: String,
}

impl UnifiedPlanner {
    /// 核心方法：分析用户输入，生成执行计划
    pub async fn plan(
        &self,
        user_input: &str,
        context: &ConversationContext,
    ) -> Result<ExecutionPlan, PlannerError>;
}
```

### 4.3 AI 规划 Prompt 策略

规划器向 LLM 发送结构化请求，要求返回 JSON 格式的计划：

```
你是一个任务规划器。分析用户请求，决定执行策略。

可用工具: [工具列表]

用户请求: "{user_input}"

请返回 JSON:
- type: "conversational" | "single_action" | "task_graph"
- 如果是 task_graph，列出任务和依赖关系
```

## 5. 统一执行器 (UnifiedExecutor) 设计

### 5.1 核心架构

```rust
// core/src/executor/mod.rs

pub struct UnifiedExecutor {
    /// 对话处理 - 复用现有 agent
    agent_manager: Arc<RigAgentManager>,

    /// DAG 调度 - 复用 cowork/scheduler
    dag_scheduler: Arc<DagScheduler>,

    /// 任务执行器注册表 - 复用 cowork/executor
    executor_registry: Arc<ExecutorRegistry>,

    /// 事件回调
    event_handler: Arc<dyn AlephEventHandler>,
}

impl UnifiedExecutor {
    /// 执行任何类型的计划
    pub async fn execute(
        &self,
        plan: ExecutionPlan,
        context: &mut ExecutionContext,
    ) -> Result<ExecutionResult, ExecutorError> {
        match plan {
            ExecutionPlan::Conversational { enhanced_prompt } => {
                self.execute_conversation(enhanced_prompt, context).await
            }
            ExecutionPlan::SingleAction { tool_name, parameters, .. } => {
                self.execute_single_action(&tool_name, parameters, context).await
            }
            ExecutionPlan::TaskGraph { tasks, dependencies, .. } => {
                self.execute_task_graph(tasks, dependencies, context).await
            }
        }
    }
}
```

### 5.2 三种执行路径

```
ExecutionPlan::Conversational
    ↓
    agent_manager.chat() → 流式响应 → 完成

ExecutionPlan::SingleAction
    ↓
    确认（如需要）→ agent_manager.execute_tool() → 结果

ExecutionPlan::TaskGraph
    ↓
    确认（如需要）→ dag_scheduler.schedule(tasks)
        ↓
    并行执行（max_parallelism）
        ↓
    每个任务 → executor_registry.execute(task)
        ↓
    汇总结果 → 生成最终响应
```

### 5.3 执行结果

```rust
// core/src/executor/types.rs

pub struct ExecutionResult {
    pub content: String,              // 最终响应文本
    pub tool_calls: Vec<ToolCallInfo>, // 工具调用记录
    pub task_results: Option<Vec<TaskResult>>, // TaskGraph 的各任务结果
    pub execution_time_ms: u64,
}
```

### 5.4 复用关系

| 新组件方法 | 复用的现有代码 |
|-----------|---------------|
| `execute_conversation()` | `agent/manager.rs` → `RigAgentManager::process()` |
| `execute_single_action()` | `agent/manager.rs` → 工具调用逻辑 |
| `execute_task_graph()` | `cowork/scheduler/` → `DagScheduler` |
| 任务执行 | `cowork/executor/` → `FileOpsExecutor`, `CodeExecExecutor` |

## 6. 处理流程集成 (ffi/processing.rs)

### 6.1 流程对比

```
【当前流程 - 6层判断】
用户输入
    → CommandParser (斜杠 + NL检测)
    → IntentClassifier (L1→L2→L3)
    → Dispatcher (L1→L2→L3)
    → Agent 或 手动触发 Cowork

【新流程 - 2层判断】
用户输入
    → L1: 斜杠命令检测 (CommandParser.parse_slash_only)
    → L3: AI 统一规划 (UnifiedPlanner.plan)
    → 统一执行 (UnifiedExecutor.execute)
```

### 6.2 新的 process() 函数

```rust
// core/src/ffi/processing.rs

pub async fn process(
    input: &str,
    options: &ProcessOptions,
    handler: Arc<dyn AlephEventHandler>,
) -> Result<ProcessResult, ProcessError> {

    // L1: 斜杠命令快速路由
    if let Some(slash_cmd) = command_parser.parse_slash_only(input) {
        return execute_slash_command(slash_cmd, handler).await;
    }

    // L3: AI 统一规划
    handler.on_thinking();  // 通知 UI "正在理解..."

    let plan = unified_planner.plan(input, &context).await?;

    // 需要确认时通知 UI
    if plan.requires_confirmation() {
        handler.on_plan_created(&plan);
        // 等待用户确认...
    }

    // 统一执行
    let result = unified_executor.execute(plan, &mut context).await?;

    // 存储到记忆
    memory_ingestion.store(&result).await?;

    handler.on_complete(&result.content);
    Ok(result.into())
}
```

### 6.3 回调变化

| 删除的回调 | 新增/修改的回调 |
|-----------|----------------|
| `on_agent_mode_detected()` | `on_plan_created(plan: &ExecutionPlan)` |
| - | `on_task_started(task_id, description)` |
| - | `on_task_completed(task_id, result)` |

## 7. UI 层修改

### 7.1 删除的 UI 元素

**macOS (Swift)**

| 文件 | 删除内容 |
|------|----------|
| `Settings/` | 移除"协作模式"配置选项（如果存在） |
| `HaloView.swift` | 移除 Agent 模式切换指示器 |
| 命令提示 | 移除 `/agent` 命令的自动补全 |

**Windows (C#) - 未来**

| 文件 | 删除内容 |
|------|----------|
| `Settings/` | 移除"协作模式"配置选项 |

### 7.2 新增/修改的 UI 元素

**计划确认视图 (PlanConfirmationView)**

```swift
// platforms/macos/Aether/Sources/Views/PlanConfirmationView.swift

struct PlanConfirmationView: View {
    let plan: ExecutionPlan
    let onConfirm: () -> Void
    let onCancel: () -> Void

    var body: some View {
        VStack {
            // 显示计划类型
            planTypeHeader

            // 如果是 TaskGraph，显示任务列表
            if case .taskGraph(let tasks, _) = plan {
                taskListView(tasks)
            }

            // 确认/取消按钮
            actionButtons
        }
    }
}
```

**进度显示 (融合现有 CoworkProgressView)**

```swift
// 复用并简化现有的 CoworkProgressView
// 重命名为 ExecutionProgressView

struct ExecutionProgressView: View {
    @ObservedObject var progress: ExecutionProgress

    // 显示当前执行的任务
    // 显示整体进度
    // 支持 SingleAction 和 TaskGraph 两种模式
}
```

### 7.3 回调桥接修改

```swift
// platforms/macos/Aether/Sources/Bridge/AlephEventHandler.swift

protocol AlephEventHandler {
    // 删除
    // func onAgentModeDetected(task: ExecutableTask)

    // 新增
    func onPlanCreated(plan: ExecutionPlan)
    func onTaskStarted(taskId: String, description: String)
    func onTaskCompleted(taskId: String, result: String)
}
```

### 7.4 用户体验变化

```
【之前】
用户: "整理我的下载文件夹"
系统: [需要用户先输入 /agent 或开启协作模式]

【之后】
用户: "整理我的下载文件夹"
系统: [自动规划] → [显示计划] → [用户确认] → [执行]
```

## 8. 配置文件变化

### 8.1 config.toml 修改

```toml
# ============ 删除的配置 ============

# [intent]                    # 整个 section 删除
# keyword_policy = "relaxed"
# ai_classification = true
# confidence_threshold = 0.7

# [dispatcher]                # 整个 section 删除
# confirmation_threshold = 0.7
# l3_enabled = true

# [cowork]                    # 部分字段移动
# enabled = true              # 删除 - 不再需要开关
# require_confirmation = true # 移动到 [execution]

# ============ 新增的配置 ============

[planner]
# 规划器使用的模型（建议用快速模型降低延迟）
model = "claude-haiku"
# 规划超时
timeout_seconds = 10

[execution]
# 执行前是否需要用户确认
require_confirmation = true
# 确认策略: "always" | "destructive_only" | "never"
confirmation_policy = "destructive_only"
# 最大并行任务数
max_parallelism = 4
# 单任务超时
task_timeout_seconds = 300

[execution.file_ops]
enabled = true
allowed_paths = ["~/Downloads/**", "~/Documents/**"]
# 删除操作始终需要确认
require_confirmation_for_delete = true

[execution.code_exec]
enabled = false
allowed_runtimes = ["shell", "python"]
sandbox_enabled = true
```

### 8.2 配置迁移

| 旧配置 | 新配置 |
|--------|--------|
| `cowork.max_parallelism` | `execution.max_parallelism` |
| `cowork.task_timeout_seconds` | `execution.task_timeout_seconds` |
| `cowork.require_confirmation` | `execution.require_confirmation` |
| `cowork.file_ops.*` | `execution.file_ops.*` |
| `cowork.code_exec.*` | `execution.code_exec.*` |
| `intent.*` | 删除 |
| `dispatcher.*` | 删除 |

### 8.3 向后兼容

```rust
// core/src/config/migration.rs

/// 配置迁移：自动将旧配置转换为新格式
pub fn migrate_config(old: &OldConfig) -> NewConfig {
    // 检测旧配置格式
    // 自动迁移到新格式
    // 首次启动时执行一次
}
```

## 9. 实施步骤

### Phase 1: 新建核心模块

```
1.1 创建 planner/ 模块
    - types.rs      (ExecutionPlan, PlannedTask)
    - prompt.rs     (规划 Prompt 模板)
    - mod.rs        (UnifiedPlanner)

1.2 创建 executor/ 模块
    - types.rs      (ExecutionResult, ExecutionContext)
    - mod.rs        (UnifiedExecutor)

1.3 单元测试
    - planner 输出正确的 Plan 类型
    - executor 能执行三种 Plan
```

### Phase 2: 集成到处理流程

```
2.1 修改 ffi/processing.rs
    - 引入 UnifiedPlanner + UnifiedExecutor
    - 新的 process() 流程
    - 保留旧代码作为 fallback（可选）

2.2 修改回调接口
    - 更新 AlephEventHandler trait
    - 新增 on_plan_created 等回调

2.3 集成测试
    - 端到端测试新流程
```

### Phase 3: 删除旧模块

```
3.1 删除 intent/ 目录
3.2 删除 dispatcher/ 目录
3.3 删除 command/nl_detector.rs, unified_index.rs
3.4 清理 cowork/ 中不再需要的入口
3.5 更新 lib.rs 导出
```

### Phase 4: UI 适配

```
4.1 macOS Swift
    - 删除协作模式设置
    - 删除 /agent 命令支持
    - 新增 PlanConfirmationView
    - 修改 ExecutionProgressView

4.2 更新 UniFFI 绑定
    - 重新生成 Swift 绑定
```

### Phase 5: 配置与文档

```
5.1 更新 config.toml 结构
5.2 实现配置迁移逻辑
5.3 更新 docs/ARCHITECTURE.md
5.4 更新 docs/COWORK.md → docs/EXECUTION.md
5.5 删除 docs/DISPATCHER.md
```

### 依赖顺序

```
Phase 1 → Phase 2 → Phase 3 → Phase 4 → Phase 5
   ↓         ↓         ↓         ↓         ↓
 可独立    依赖1     依赖2     依赖3     依赖4
 开发      集成      清理      UI适配    收尾
```

## 10. 风险与缓解

| 风险 | 缓解措施 |
|------|----------|
| AI 规划增加延迟 | 使用快速模型 (haiku)，流式反馈掩盖延迟 |
| 规划器判断错误 | 保留用户确认机制，可手动修正 |
| 大规模重构引入 bug | 分阶段实施，每阶段完整测试 |
| 配置迁移失败 | 自动迁移 + 手动回退机制 |

## 11. 成功指标

- [ ] 用户无需使用 `/agent` 命令即可完成复杂任务
- [ ] 简单对话响应延迟 < 3秒（规划 + 首 token）
- [ ] 复杂任务自动分解并并行执行
- [ ] 代码量净减少（删除 > 新增）
- [ ] 所有现有测试通过
