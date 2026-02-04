# Agent Loop 架构设计

> 日期: 2026-01-21
> 状态: 设计完成，待实现

## 概述

本文档描述 Aleph 全新的 Agent Loop 架构设计，整合 orchestrator、intent、planner、executor、prompt、dispatcher 六个模块为统一的循环执行架构。

## 设计决策

| 项目 | 决策 |
|------|------|
| 架构风格 | 统一管道式 Agent Loop |
| 思考层 | 纯 LLM 思考，每步调用 |
| 终止机制 | LLM 判断 + 系统保护（双重） |
| 人机协作 | LLM 主动 + 系统强制（两者结合） |
| 上下文管理 | 智能压缩（滑动窗口 + LLM 摘要） |
| intent | 保留 L0-L2 快速路径 |
| planner | 废弃 |
| dispatcher | 拆分到 Thinker（工具过滤 + 模型路由） |
| orchestrator | 重写为 AgentLoop |
| prompt | 整合到 Thinker |
| executor | 简化为单步执行器 |

## 核心理念

Agent Loop 是真正的 Agent 架构：

```
Agent Loop:
  用户请求 → 循环 {
      观察: 当前状态 + 历史结果
      思考: LLM 决定下一步做什么
      行动: 执行一个动作
      反馈: 更新状态
  } → 直到任务完成或需要用户介入
```

### 与预规划架构对比

| 特性 | 全 LLM 规划 | Agent Loop |
|------|-------------|------------|
| 计划时机 | 一次性预先规划 | 每一步动态决策 |
| 错误处理 | 失败即停止 | 可调整、重试、降级 |
| 中间结果 | 不影响后续计划 | 驱动后续决策 |
| 人机协作 | 只能开始/结束时 | 任意时刻可介入 |
| 开放式任务 | 难以处理 | 自然支持 |
| 工具使用 | 预先确定 | 动态发现 |

---

## 整体架构

```
┌─────────────────────────────────────────────────────────────────────────┐
│                              User Request                                │
└─────────────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼
┌─────────────────────────────────────────────────────────────────────────┐
│                         IntentRouter (L0-L2)                             │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐                   │
│  │ L0: Slash    │  │ L1: Pattern  │  │ L2: Context  │                   │
│  │ /screenshot  │→ │ 正则匹配     │→ │ 附件/剪贴板  │                   │
│  └──────┬───────┘  └──────┬───────┘  └──────┬───────┘                   │
│         │                 │                 │                            │
│         ▼                 ▼                 ▼                            │
│    Fast Action       Fast Action        RouteResult::Loop               │
└─────────────────────────────────────────────────────────────────────────┘
            │                                       │
            ▼                                       ▼
     ┌─────────────┐                    ┌─────────────────────────────────┐
     │ 直接执行返回 │                    │          AgentLoop              │
     └─────────────┘                    │                                  │
                                        │  Guards → Compress → Think →    │
                                        │  Decide → Execute → Feedback    │
                                        │           ↑              │      │
                                        │           └──────────────┘      │
                                        └─────────────────────────────────┘
```

---

## 核心数据结构

### Loop 状态

```rust
/// Agent Loop 的完整状态
pub struct LoopState {
    /// 唯一会话 ID
    pub session_id: String,
    /// 用户原始请求
    pub original_request: String,
    /// 请求上下文（附件、选中文件、剪贴板等）
    pub context: RequestContext,
    /// 执行步骤历史
    pub steps: Vec<LoopStep>,
    /// 当前步数
    pub step_count: usize,
    /// 累计 token 使用
    pub total_tokens: usize,
    /// 开始时间
    pub started_at: Instant,
}

/// 单个循环步骤
pub struct LoopStep {
    pub step_id: usize,
    /// 观察阶段的输入摘要
    pub observation: Observation,
    /// LLM 的思考过程
    pub thinking: Thinking,
    /// 执行的动作
    pub action: Action,
    /// 动作的结果
    pub result: ActionResult,
    /// 该步 token 消耗
    pub tokens_used: usize,
}
```

### 观察

```rust
pub struct Observation {
    /// 压缩后的历史摘要
    pub history_summary: String,
    /// 最近 N 步的详细信息（滑动窗口）
    pub recent_steps: Vec<StepSummary>,
    /// 当前可用工具
    pub available_tools: Vec<ToolInfo>,
    /// 上下文附件
    pub attachments: Vec<Attachment>,
}
```

### 思考（LLM 输出）

```rust
pub struct Thinking {
    /// LLM 的推理过程（可选，用于调试/展示）
    pub reasoning: Option<String>,
    /// 决定的下一步动作
    pub decision: Decision,
}

pub enum Decision {
    /// 执行工具
    UseTool { tool_name: String, arguments: Value },
    /// 请求用户输入
    AskUser { question: String, options: Option<Vec<String>> },
    /// 任务完成
    Complete { summary: String },
    /// 任务失败
    Fail { reason: String },
}
```

### 行动与结果

```rust
pub enum Action {
    ToolCall { tool_name: String, arguments: Value },
    UserInteraction { question: String },
    Completion { summary: String },
    Failure { reason: String },
}

pub enum ActionResult {
    ToolSuccess { output: Value, duration_ms: u64 },
    ToolError { error: String, retryable: bool },
    UserResponse { response: String },
    Completed,
    Failed,
}
```

---

## 模块设计

### 1. IntentRouter（原 intent 模块简化版）

职责：快速路由，决定是否进入 Agent Loop

```rust
pub struct IntentRouter {
    slash_commands: SlashCommandRegistry,  // L0
    patterns: PatternMatcher,               // L1
    context_analyzer: ContextAnalyzer,      // L2
}

impl IntentRouter {
    pub fn route(&self, request: &str, context: &RequestContext) -> RouteResult;
}

pub enum RouteResult {
    Fast(FastAction),  // 快速路径：直接执行
    Loop,              // 进入 Agent Loop
}

pub enum FastAction {
    SlashCommand { cmd: String, args: Vec<String> },
    DirectTool { tool_name: String, args: Value },
    Skill { skill_id: String },
    Mcp { server: String, tool: String, args: Value },
}
```

### 2. AgentLoop（原 orchestrator 重写）

职责：Loop 生命周期管理、状态维护、保护机制

```rust
pub struct AgentLoop {
    thinker: Arc<Thinker>,
    executor: Arc<Executor>,
    compressor: Arc<ContextCompressor>,
    config: LoopConfig,
}

impl AgentLoop {
    pub async fn run(
        &self,
        request: String,
        context: RequestContext,
        tools: Vec<UnifiedTool>,
        callback: impl LoopCallback,
    ) -> LoopResult;
}

pub struct LoopConfig {
    pub max_steps: usize,              // 最大步数，默认 50
    pub max_tokens: usize,             // 最大 token，默认 100k
    pub timeout: Duration,             // 超时，默认 10 分钟
    pub require_confirmation: Vec<String>, // 需确认的工具列表
}
```

### 3. Thinker（整合 prompt + dispatcher 部分能力）

职责：观察构建、工具筛选、模型选择、LLM 调用、决策解析

```rust
pub struct Thinker {
    providers: ProviderRegistry,    // 多模型提供者
    tool_filter: ToolFilter,        // 工具过滤（来自 dispatcher）
    prompt_builder: PromptBuilder,  // 提示构建
    model_router: ModelRouter,      // 模型路由（来自 dispatcher）
}

impl Thinker {
    pub async fn think(
        &self,
        state: &LoopState,
        all_tools: &[UnifiedTool],
    ) -> Result<Thinking> {
        // 1. 构建观察（含压缩历史）
        // 2. 工具过滤（根据当前上下文动态筛选）
        // 3. 模型路由（选择最佳模型）
        // 4. 构建提示
        // 5. 调用 LLM
        // 6. 解析决策
    }
}
```

#### 工具过滤器

```rust
pub struct ToolFilter {
    category_rules: HashMap<TaskCategory, Vec<String>>,
}

impl ToolFilter {
    pub fn filter(
        &self,
        tools: &[UnifiedTool],
        observation: &Observation,
    ) -> Vec<ToolInfo>;
}
```

#### 模型路由器

```rust
pub struct ModelRouter {
    rules: Vec<RoutingRule>,
    default_model: ModelId,
}

pub enum RoutingCondition {
    CodeRelated,
    VisionRequired,
    SimpleChat,
    ComplexReasoning,
}
```

### 4. Executor（原 executor 简化版）

职责：执行单个工具调用

```rust
pub struct Executor {
    tool_registry: Arc<ToolRegistry>,
    runtime_manager: Arc<RuntimeManager>,
}

impl Executor {
    pub async fn execute(&self, action: &Action) -> ActionResult;
}
```

### 5. ContextCompressor（基于 SessionCompactor 演化）

职责：智能压缩历史上下文

```rust
pub struct ContextCompressor {
    provider: Arc<dyn AiProvider>,
    config: CompressorConfig,
}

pub struct CompressorConfig {
    pub compress_after_steps: usize,      // 触发压缩的步数阈值，默认 5
    pub recent_window_size: usize,        // 滑动窗口保留的最近步数，默认 3
    pub target_summary_tokens: usize,     // 压缩目标 token 数，默认 500
    pub preserve_tool_outputs: bool,      // 是否保留关键工具调用详情
}

impl ContextCompressor {
    pub fn should_compress(&self, state: &LoopState) -> bool;
    pub async fn compress(
        &self,
        steps: &[LoopStep],
        current_summary: &str,
    ) -> Result<CompressedHistory>;
}
```

---

## AgentLoop 主流程

```rust
impl AgentLoop {
    pub async fn run(...) -> LoopResult {
        let mut state = LoopState::new(request, context);
        callback.on_loop_start(&state).await;

        loop {
            // ===== 保护检查 =====
            if let Some(violation) = self.check_guards(&state) {
                return LoopResult::GuardTriggered(violation);
            }

            // ===== 1. 观察 + 压缩 =====
            if state.needs_compression() {
                let compressed = self.compressor.compress(&state.steps).await?;
                state.apply_compression(compressed);
            }

            // ===== 2. 思考 =====
            let thinking = self.thinker.think(&state, &tools).await?;

            // ===== 3. 行动前检查 =====
            let action = match &thinking.decision {
                Decision::Complete { summary } => {
                    return LoopResult::Completed { ... };
                }
                Decision::Fail { reason } => {
                    return LoopResult::Failed { ... };
                }
                Decision::AskUser { question, options } => {
                    Action::UserInteraction { question }
                }
                Decision::UseTool { tool_name, arguments } => {
                    // 高风险工具确认
                    if self.requires_confirmation(tool_name) {
                        let confirmed = callback.on_confirmation_required(...).await;
                        if !confirmed { continue; }
                    }
                    Action::ToolCall { tool_name, arguments }
                }
            };

            // ===== 4. 执行 =====
            let result = self.executor.execute(&action).await;

            // ===== 5. 反馈（更新状态）=====
            state.record_step(LoopStep { ... });
        }
    }
}
```

### 保护机制

```rust
pub enum GuardViolation {
    MaxSteps(usize),
    MaxTokens(usize),
    Timeout(Duration),
}

pub enum LoopResult {
    Completed { summary: String, steps: usize, total_tokens: usize },
    Failed { reason: String, steps: usize },
    GuardTriggered(GuardViolation),
    UserAborted,
}
```

---

## Prompt 设计

### System Prompt 模板

```markdown
You are an AI assistant executing tasks step by step.

## Your Role
- Observe the current state and history
- Decide the SINGLE next action to take
- Execute until the task is complete or you need user input

## Available Tools
{tools_list}

## Special Actions
- `complete`: Call when the task is fully done
- `ask_user`: Call when you need clarification or user decision

## Response Format
You must respond with a JSON object:
```json
{
  "reasoning": "Brief explanation of your thinking",
  "action": {
    "type": "tool|ask_user|complete|fail",
    "tool_name": "...",
    "arguments": {...},
    "question": "...",
    "options": [...],
    "summary": "...",
    "reason": "..."
  }
}
```

## Guidelines
1. Take ONE action at a time, observe the result, then decide next
2. Use tool results to inform subsequent decisions
3. Ask user when: multiple valid approaches, unclear requirements, need confirmation
4. Complete when: task is done, or you've provided the requested information
5. Fail when: impossible to proceed, missing critical resources

## Current Context
- Original request: {original_request}
- Steps taken: {step_count}
- Tokens used: {tokens_used}
```

### Messages 构建

```rust
fn build_messages(&self, original_request: &str, observation: &Observation) -> Vec<Message> {
    let mut messages = vec![];

    // 1. 用户原始请求
    messages.push(Message::user(format!(
        "Task: {}\n\nContext:\n{}",
        original_request,
        observation.format_attachments()
    )));

    // 2. 压缩历史摘要（如果有）
    if !observation.history_summary.is_empty() {
        messages.push(Message::assistant(format!(
            "[Previous steps summary]\n{}",
            observation.history_summary
        )));
    }

    // 3. 最近步骤详情（滑动窗口）
    for step in &observation.recent_steps {
        messages.push(Message::assistant(...));
        messages.push(Message::tool_result(...));
    }

    // 4. 请求下一步
    messages.push(Message::user("Based on the above, what is your next action?"));

    messages
}
```

---

## 上下文压缩策略

### 触发时机

```
步骤 1-5: 完整保留
步骤 6:   触发压缩 → 步骤 1-3 压缩为摘要，4-6 保留
步骤 9:   触发压缩 → 摘要更新，7-9 保留
...以此类推
```

### 压缩提示模板

```markdown
Summarize the following task execution history concisely.

## Current Summary (if any)
{current_summary}

## New Steps to Compress
{steps_to_compress}

## Instructions
1. Preserve KEY information:
   - Important tool outputs (file paths, search results, errors)
   - User decisions and clarifications
   - State changes (files created, data fetched)

2. Remove redundant details:
   - Verbose tool output formatting
   - Repeated similar operations
   - Intermediate reasoning that led nowhere

3. Format as bullet points, max {target_tokens} tokens
```

---

## FFI 接口

### 事件定义

```rust
#[derive(uniffi::Enum)]
pub enum LoopEvent {
    Started { session_id: String, request: String },
    StepStarted { step: u32 },
    Thinking { reasoning: Option<String> },
    ToolCall { tool_name: String, arguments: String },
    ToolResult { tool_name: String, success: bool, output: String },
    ConfirmationRequired { tool_name: String, description: String, request_id: String },
    UserInputRequired { question: String, options: Vec<String>, request_id: String },
    GuardTriggered { reason: String, suggestion: String },
    Completed { summary: String, steps: u32, tokens: u32 },
    Failed { reason: String, steps: u32 },
}
```

### FFI 入口

```rust
#[uniffi::export]
impl AlephCore {
    pub async fn process(
        &self,
        input: String,
        options: ProcessOptions,
        callback: Box<dyn FfiCallback>,
    ) -> ProcessResult {
        let route = self.intent_router.route(&input, &options.context);

        match route {
            RouteResult::Fast(action) => {
                self.execute_fast_action(action, callback).await
            }
            RouteResult::Loop => {
                self.agent_loop.run(input, options.context, tools, callback).await.into()
            }
        }
    }

    pub fn respond_confirmation(&self, request_id: String, confirmed: bool);
    pub fn respond_user_input(&self, request_id: String, response: String);
    pub fn abort_loop(&self, session_id: String);
}
```

### UI 交互流程

```
Swift UI                         Rust Core
   │                                 │
   │─── process(input) ─────────────►│
   │                                 │
   │◄── Started { session_id } ──────│
   │◄── StepStarted { step: 1 } ─────│
   │◄── Thinking { ... } ────────────│
   │◄── ToolCall { ... } ────────────│
   │◄── ToolResult { ... } ──────────│
   │                                 │
   │◄── ConfirmationRequired ────────│ (高风险操作)
   │                                 │
   │─── respond_confirmation(yes) ──►│
   │                                 │
   │◄── StepStarted { step: 2 } ─────│
   │◄── ... ─────────────────────────│
   │◄── Completed { summary } ───────│
```

---

## 文件结构

### 新建模块

```
core/src/
├── agent_loop/                 # 【新建】Agent Loop 核心
│   ├── mod.rs                  # AgentLoop 主结构
│   ├── state.rs                # LoopState, LoopStep
│   ├── decision.rs             # Decision, Action, ActionResult
│   ├── guards.rs               # 保护机制
│   ├── callback.rs             # LoopCallback trait
│   └── config.rs               # LoopConfig
│
├── thinker/                    # 【新建】思考层
│   ├── mod.rs                  # Thinker 主结构
│   ├── prompt_builder.rs       # 提示构建
│   ├── tool_filter.rs          # 工具过滤
│   ├── model_router.rs         # 模型路由
│   └── decision_parser.rs      # 决策解析
│
├── compressor/                 # 【新建】上下文压缩
│   ├── mod.rs                  # ContextCompressor
│   └── strategy.rs             # 压缩策略
│
├── intent/                     # 【保留简化】仅 L0-L2
│   ├── mod.rs                  # IntentRouter
│   ├── slash_commands.rs       # L0
│   ├── patterns.rs             # L1
│   └── context_signals.rs      # L2
│
├── executor/                   # 【简化】单步执行器
│   ├── mod.rs                  # Executor
│   └── tool_runner.rs          # 工具执行
│
└── ffi/                        # 【更新】FFI 层
    ├── mod.rs
    ├── processing.rs           # 调用 AgentLoop
    ├── events.rs               # LoopEvent 定义
    └── callback_adapter.rs     # FFI 回调适配
```

### 废弃/删除

| 原模块/文件 | 处理方式 | 原因 |
|-------------|----------|------|
| `planner/` | 删除整个目录 | Agent Loop 不需要预先规划 |
| `orchestrator/` | 删除整个目录 | 被 `agent_loop/` 替代 |
| `dispatcher/scheduler/` | 删除 | DAG 调度不再需要 |
| `dispatcher/mod.rs` | 删除 | 能力已拆分到 thinker |
| `intent/execution_decider.rs` | 删除 | L3-L4 不再需要 |
| `intent/classifier.rs` | 删除 | 旧分类器 |
| `intent/ai_detector.rs` | 删除 | 不再需要 |
| `prompt/` | 删除整个目录 | 整合到 `thinker/` |
| `components/task_planner.rs` | 删除 | 被 AgentLoop 替代 |
| `components/loop_controller.rs` | 删除 | 被 AgentLoop 替代 |

### 保留模块

| 模块 | 原因 |
|------|------|
| `agent/` | RigAgentManager，被 Executor 使用 |
| `tools/` | 工具定义和注册 |
| `memory/` | 会话记忆 |
| `mcp/` | MCP 协议支持 |
| `runtime/` | 运行时管理 |
| `config/` | 配置管理 |
| `components/session_recorder.rs` | 持久化 |
| `components/callback_bridge.rs` | FFI 回调 |

---

## 迁移计划

### Phase 1: 新建核心模块
- 创建 `agent_loop/`、`thinker/`、`compressor/`
- 实现核心数据结构和接口

### Phase 2: 整合现有代码
- 从 dispatcher 迁移工具过滤、模型路由到 thinker
- 从 prompt 迁移提示构建到 thinker
- 简化 intent 为 IntentRouter（仅 L0-L2）

### Phase 3: 更新 FFI 层
- 更新 `processing.rs` 调用 AgentLoop
- 实现新的事件系统

### Phase 4: 清理废弃代码
- 删除 `planner/`、`orchestrator/`、`dispatcher/`
- 删除 intent 中的 L3-L4 相关代码

### Phase 5: 测试验证
- 单元测试
- 集成测试
- 端到端测试

---

## 附录：Agent Loop 优势示例

### 1. 自适应执行

```
用户: "分析这个文档，生成知识图谱"

Agent Loop:
  Step 1: 读取文档 → 发现是加密PDF
  Step 2: 思考: "需要解密，让我尝试OCR"
  Step 3: 用 OCR 提取文本 → 成功
  Step 4: 继续分析...
```

### 2. 中间结果驱动决策

```
用户: "搜索最新的 Rust 教程，整理成文档"

Agent Loop:
  Step 1: 搜索 → 得到 10 个结果
  Step 2: 思考: "有视频、文章、书籍，按类型分组更好"
  Step 3: 生成分类整理的文档
```

### 3. 人机协作

```
用户: "帮我重构这个模块"

Agent Loop:
  Step 1: 分析代码 → 发现 3 种重构方案
  Step 2: 暂停，问用户: "我发现3种方案，你倾向哪个？"
  Step 3: 用户选择后继续执行
```

### 4. 开放式任务

```
用户: "帮我研究一下 WebGPU"

Agent Loop:
  Step 1: 搜索 WebGPU → 找到官方文档
  Step 2: 阅读文档 → 发现需要了解 WGSL
  Step 3: 搜索 WGSL → 继续学习
  Step N: 思考: "已经收集足够信息，可以总结了"
```

### 5. 动态工具组合

```
用户: "帮我分析这张图片里的代码并运行"

Step 1: 识别图片 → 发现是 Python 代码
Step 2: OCR 提取代码
Step 3: 思考: "需要运行 Python，检查是否有 runtime"
Step 4: 检测到有 uv → 使用 uv run
Step 5: 运行代码 → 返回结果
```
