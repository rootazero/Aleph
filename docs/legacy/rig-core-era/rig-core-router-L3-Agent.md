引入rust rig-core，设计router L3 Agent 增强方案。

  ---
  L3 Agent 增强架构设计

  核心理念

  保持 L1/L2 的速度优势，让 L3 从 "单工具选择器" 进化为 "任务规划器"

  ---
  一、当前 L3 vs 增强后 L3

  现状

  当前 L3:
  ┌─────────────────────────────────────┐
  │  Input: "帮我把这个翻译成英文"        │
  │                                     │
  │  LLM 推理:                          │
  │  → Tool: translate                  │
  │  → Params: {target: "en"}           │
  │  → Confidence: 0.85                 │
  │                                     │
  │  Output: SingleToolCall             │
  └─────────────────────────────────────┘

  局限: 只能选择一个工具，无法处理多步骤任务

  增强后

  增强 L3 (Agent Mode):
  ┌─────────────────────────────────────────────────────────┐
  │  Input: "把昨天的会议笔记翻译成英文，总结要点，发给 John" │
  │                                                         │
  │  LLM 推理 (Task Decomposition):                         │
  │  → 检测: 这是多步骤任务                                  │
  │  → Plan:                                                │
  │     Step 1: search_files(query="会议笔记", date="昨天")  │
  │     Step 2: translate(content=$1, target="en")          │
  │     Step 3: summarize(content=$2, style="bullet")       │
  │     Step 4: send_email(to="john", body=$3)              │
  │  → Confidence: 0.75                                     │
  │  → Dependencies: 1→2→3→4 (sequential)                   │
  │                                                         │
  │  Output: ExecutionPlan                                  │
  └─────────────────────────────────────────────────────────┘

  ---
  二、架构设计

  整体流程

                      用户输入
                         │
             ┌───────────┴───────────┐
             │      L1 (Regex)       │
             │       <10ms           │
             └───────────┬───────────┘
                         │ miss
             ┌───────────┴───────────┐
             │    L2 (Semantic)      │
             │      200-500ms        │
             └───────────┬───────────┘
                         │ miss or low confidence
             ┌───────────┴───────────┐
             │    L3 (LLM Router)    │
             │       500ms-1s        │
             └───────────┬───────────┘
                         │
                ┌────────┴────────┐
                │  Task Analyzer  │
                └────────┬────────┘
                         │
           ┌─────────────┼─────────────┐
           │             │             │
      SingleTool    MultiStep     Clarify
           │             │             │
           ▼             ▼             ▼
      直接执行      AgentPlanner    追问用户
                         │
                         ▼
                ┌────────────────┐
                │ ExecutionPlan  │
                │ (DAG of steps) │
                └───────┬────────┘
                        │
                ┌───────┴────────┐
                │ PlanExecutor   │
                │ (with confirm) │
                └────────────────┘

  核心数据结构

  // L3 的输出类型扩展
  pub enum L3Result {
      /// 单工具调用 (现有)
      SingleTool {
          tool: ToolRef,
          params: Value,
          confidence: f32,
      },

      /// 多步骤执行计划 (新增)
      ExecutionPlan {
          plan: TaskPlan,
          confidence: f32,
          requires_confirmation: bool,
      },

      /// 需要澄清 (现有)
      NeedsClarification {
          question: String,
          options: Vec<String>,
      },

      /// 降级为对话 (现有)
      FallbackToChat,
  }

  /// 执行计划
  pub struct TaskPlan {
      pub id: Uuid,
      pub description: String,           // 计划的自然语言描述
      pub steps: Vec<PlanStep>,          // 执行步骤
      pub dependency_graph: DependencyGraph,  // 步骤间依赖关系
      pub estimated_duration: Duration,  // 预估耗时
      pub rollback_strategy: RollbackStrategy,  // 回滚策略
  }

  /// 单个步骤
  pub struct PlanStep {
      pub id: StepId,
      pub tool: ToolRef,
      pub params: StepParams,            // 可能引用前序步骤的输出
      pub description: String,           // 步骤描述
      pub is_reversible: bool,           // 是否可回滚
      pub timeout: Duration,
  }

  /// 步骤参数 (支持引用)
  pub enum StepParams {
      /// 静态参数
      Static(Value),
      /// 引用前序步骤输出
      Reference {
          step_id: StepId,
          json_path: String,  // e.g., "$.result.content"
      },
      /// 混合 (部分静态，部分引用)
      Mixed(Value),  // 内部包含 "$ref:step_1.output" 占位符
  }

  /// 依赖图
  pub struct DependencyGraph {
      pub edges: Vec<(StepId, StepId)>,  // (前序, 后序)
      pub parallelizable_groups: Vec<Vec<StepId>>,  // 可并行的步骤组
  }

  ---
  三、Task Analyzer 设计

  任务复杂度检测

  pub struct TaskAnalyzer {
      complexity_detector: ComplexityDetector,
      intent_classifier: IntentClassifier,
  }

  impl TaskAnalyzer {
      /// 分析输入，决定是单工具还是多步骤
      pub async fn analyze(&self, input: &str, context: &Context) -> TaskType {
          // 1. 快速启发式检测
          if let Some(task_type) = self.quick_heuristics(input) {
              return task_type;
          }

          // 2. LLM 分类
          let classification = self.intent_classifier.classify(input).await?;

          match classification {
              Classification::SingleAction { .. } => TaskType::SingleTool,
              Classification::MultiStep { steps, .. } => TaskType::MultiStep(steps),
              Classification::Ambiguous { .. } => TaskType::NeedsClarification,
              Classification::Conversational => TaskType::Chat,
          }
      }

      /// 快速启发式 (不调用 LLM)
      fn quick_heuristics(&self, input: &str) -> Option<TaskType> {
          // 多动词检测
          let action_words = ["翻译", "总结", "发送", "保存", "搜索", "分析", "生成"];
          let action_count = action_words.iter()
              .filter(|w| input.contains(*w))
              .count();

          if action_count >= 2 {
              return Some(TaskType::LikelyMultiStep);
          }

          // 连接词检测
          let connectors = ["然后", "接着", "之后", "并且", "同时"];
          if connectors.iter().any(|c| input.contains(c)) {
              return Some(TaskType::LikelyMultiStep);
          }

          None
      }
  }

  LLM Prompt for Task Decomposition

  const TASK_DECOMPOSITION_PROMPT: &str = r#"
  你是一个任务分解专家。分析用户请求，判断是否需要多步骤执行。

  ## 可用工具
  {tools_description}

  ## 分析规则
  1. 如果任务可以用单个工具完成，返回 "single"
  2. 如果任务需要多个步骤，返回执行计划
  3. 如果任务不明确，返回需要澄清的问题

  ## 输出格式 (JSON)
  单工具:
  {"type": "single", "tool": "translate", "params": {"text": "...", "target": "en"}}

  多步骤:
  {
    "type": "multi",
    "plan": {
      "description": "翻译会议笔记并发送给 John",
      "steps": [
        {"id": "1", "tool": "search_files", "params": {"query": "会议笔记"}, "description": "查找会议笔记"},
        {"id": "2", "tool": "translate", "params": {"content": "$ref:1.output", "target": "en"}, "depends_on": ["1"]},
        {"id": "3", "tool": "send_email", "params": {"to": "john", "body": "$ref:2.output"}, "depends_on": ["2"]}
      ]
    }
  }

  需要澄清:
  {"type": "clarify", "question": "你想发给哪个 John？", "options": ["john@work.com", "john@personal.com"]}

  ## 用户请求
  {user_input}

  ## 当前上下文
  {context}
  "#;

  ---
  四、Plan Executor 设计

  执行引擎

  pub struct PlanExecutor {
      tool_registry: Arc<ToolRegistry>,
      event_handler: Arc<dyn AlephEventHandler>,
      config: ExecutorConfig,
  }

  impl PlanExecutor {
      /// 执行计划
      pub async fn execute(&self, plan: TaskPlan) -> Result<ExecutionResult> {
          // 1. 用户确认
          if plan.requires_confirmation() {
              let confirmed = self.request_confirmation(&plan).await?;
              if !confirmed {
                  return Ok(ExecutionResult::Cancelled);
              }
          }

          // 2. 初始化执行上下文
          let mut ctx = ExecutionContext::new(plan.id);

          // 3. 按依赖顺序执行
          for step_group in plan.dependency_graph.topological_order() {
              // 并行执行同一层级的步骤
              let results = self.execute_parallel(step_group, &ctx).await?;

              // 更新上下文
              for (step_id, result) in results {
                  ctx.set_output(step_id, result);

                  // 通知 UI 进度
                  self.event_handler.on_plan_progress(PlanProgress {
                      plan_id: plan.id,
                      completed_step: step_id,
                      total_steps: plan.steps.len(),
                      current_output: result.preview(),
                  }).await;
              }
          }

          // 4. 返回最终结果
          Ok(ExecutionResult::Success {
              final_output: ctx.get_final_output(),
              execution_trace: ctx.trace,
          })
      }

      /// 并行执行一组步骤
      async fn execute_parallel(
          &self,
          steps: Vec<&PlanStep>,
          ctx: &ExecutionContext,
      ) -> Result<Vec<(StepId, StepOutput)>> {
          let futures: Vec<_> = steps.iter()
              .map(|step| self.execute_step(step, ctx))
              .collect();

          futures::future::try_join_all(futures).await
      }

      /// 执行单个步骤
      async fn execute_step(
          &self,
          step: &PlanStep,
          ctx: &ExecutionContext,
      ) -> Result<(StepId, StepOutput)> {
          // 1. 解析参数 (替换引用)
          let resolved_params = self.resolve_params(&step.params, ctx)?;

          // 2. 获取工具
          let tool = self.tool_registry.get(&step.tool)?;

          // 3. 执行 (带超时)
          let output = tokio::time::timeout(
              step.timeout,
              tool.execute(resolved_params),
          ).await??;

          Ok((step.id, output))
      }

      /// 解析参数中的引用
      fn resolve_params(&self, params: &StepParams, ctx: &ExecutionContext) -> Result<Value> {
          match params {
              StepParams::Static(v) => Ok(v.clone()),
              StepParams::Reference { step_id, json_path } => {
                  let output = ctx.get_output(step_id)?;
                  jsonpath::select(&output, json_path)
              }
              StepParams::Mixed(v) => {
                  // 递归替换 "$ref:step_id.path" 占位符
                  self.resolve_refs_in_value(v, ctx)
              }
          }
      }
  }

  执行进度 UI 通知

  // UniFFI callback 扩展
  callback interface AlephEventHandler {
      // ... 现有方法 ...

      /// 计划开始执行
      void on_plan_started(PlanInfo plan);

      /// 步骤进度更新
      void on_plan_progress(PlanProgress progress);

      /// 步骤需要用户输入
      void on_step_requires_input(StepInputRequest request);

      /// 计划执行完成
      void on_plan_completed(PlanResult result);

      /// 计划执行失败
      void on_plan_failed(PlanError error);
  };

  pub struct PlanProgress {
      pub plan_id: Uuid,
      pub current_step: u32,
      pub total_steps: u32,
      pub step_description: String,
      pub step_status: StepStatus,
      pub preview: Option<String>,  // 当前步骤的输出预览
  }

  pub enum StepStatus {
      Pending,
      Running,
      Completed,
      Failed { error: String },
      Skipped { reason: String },
  }

  ---
  五、Swift UI 集成

  Plan Confirmation UI

  // PlanConfirmationView.swift
  struct PlanConfirmationView: View {
      let plan: PlanInfo
      let onConfirm: () -> Void
      let onCancel: () -> Void
      let onEditStep: (Int) -> Void

      var body: some View {
          VStack(alignment: .leading, spacing: 16) {
              // 标题
              HStack {
                  Image(systemName: "list.bullet.clipboard")
                  Text("执行计划")
                      .font(.headline)
                  Spacer()
                  Text("\(plan.steps.count) 步骤")
                      .foregroundColor(.secondary)
              }

              // 步骤列表
              ForEach(Array(plan.steps.enumerated()), id: \.offset) { index, step in
                  PlanStepRow(
                      index: index + 1,
                      step: step,
                      onEdit: { onEditStep(index) }
                  )
              }

              // 预估信息
              HStack {
                  Label("预计耗时: \(plan.estimatedDuration)", systemImage: "clock")
                  Spacer()
                  if plan.hasIrreversibleSteps {
                      Label("包含不可撤销操作", systemImage: "exclamationmark.triangle")
                          .foregroundColor(.orange)
                  }
              }
              .font(.caption)

              // 操作按钮
              HStack {
                  Button("取消", role: .cancel, action: onCancel)
                  Spacer()
                  Button("执行", action: onConfirm)
                      .buttonStyle(.borderedProminent)
              }
          }
          .padding()
          .background(.ultraThinMaterial)
          .cornerRadius(12)
      }
  }

  struct PlanStepRow: View {
      let index: Int
      let step: PlanStepInfo
      let onEdit: () -> Void

      var body: some View {
          HStack {
              // 步骤序号
              Circle()
                  .fill(Color.accentColor.opacity(0.2))
                  .frame(width: 24, height: 24)
                  .overlay(Text("\(index)").font(.caption.bold()))

              // 步骤信息
              VStack(alignment: .leading) {
                  Text(step.description)
                      .font(.subheadline)
                  HStack {
                      Image(systemName: step.toolIcon)
                      Text(step.toolName)
                  }
                  .font(.caption)
                  .foregroundColor(.secondary)
              }

              Spacer()

              // 编辑按钮
              Button(action: onEdit) {
                  Image(systemName: "pencil")
              }
              .buttonStyle(.plain)
          }
          .padding(.vertical, 4)
      }
  }

  Plan Execution Progress UI

  // PlanProgressView.swift
  struct PlanProgressView: View {
      @ObservedObject var viewModel: PlanProgressViewModel

      var body: some View {
          VStack(spacing: 12) {
              // 进度条
              ProgressView(value: viewModel.progress)
                  .progressViewStyle(.linear)

              // 当前步骤
              HStack {
                  StepStatusIcon(status: viewModel.currentStepStatus)
                  Text(viewModel.currentStepDescription)
                      .font(.subheadline)
                  Spacer()
                  Text("\(viewModel.completedSteps)/\(viewModel.totalSteps)")
                      .font(.caption)
                      .foregroundColor(.secondary)
              }

              // 输出预览
              if let preview = viewModel.currentOutput {
                  Text(preview)
                      .font(.caption)
                      .foregroundColor(.secondary)
                      .lineLimit(2)
                      .frame(maxWidth: .infinity, alignment: .leading)
                      .padding(8)
                      .background(Color.secondary.opacity(0.1))
                      .cornerRadius(6)
              }

              // 取消按钮
              if viewModel.canCancel {
                  Button("取消执行", role: .destructive) {
                      viewModel.cancel()
                  }
                  .font(.caption)
              }
          }
          .padding()
          .background(.ultraThinMaterial)
          .cornerRadius(12)
      }
  }

  ---
  六、安全与回滚机制

  工具分类

  pub enum ToolSafetyLevel {
      /// 只读操作，完全安全
      ReadOnly,

      /// 可逆操作 (如：复制文件，有回滚能力)
      Reversible { rollback_fn: RollbackFn },

      /// 不可逆但低风险 (如：发送邮件)
      IrreversibleLowRisk,

      /// 不可逆高风险 (如：删除文件、执行命令)
      IrreversibleHighRisk,
  }

  pub trait AlephTool: Send + Sync {
      fn name(&self) -> &str;
      fn description(&self) -> &str;
      fn safety_level(&self) -> ToolSafetyLevel;

      async fn execute(&self, params: Value) -> Result<ToolOutput>;

      /// 回滚操作 (如果支持)
      async fn rollback(&self, execution_id: Uuid) -> Result<()> {
          Err(AlephError::RollbackNotSupported)
      }
  }

  执行策略

  pub struct ExecutorConfig {
      /// 遇到高风险步骤时的行为
      pub high_risk_policy: HighRiskPolicy,

      /// 步骤失败时的行为
      pub failure_policy: FailurePolicy,

      /// 是否启用回滚
      pub enable_rollback: bool,
  }

  pub enum HighRiskPolicy {
      /// 每个高风险步骤单独确认
      ConfirmEach,
      /// 计划开始时一次性确认
      ConfirmOnce,
      /// 禁止执行高风险步骤
      Block,
  }

  pub enum FailurePolicy {
      /// 失败后停止，尝试回滚已执行的步骤
      StopAndRollback,
      /// 失败后停止，保留已执行的结果
      StopAndKeep,
      /// 跳过失败步骤，继续执行
      SkipAndContinue,
  }

  回滚执行

  impl PlanExecutor {
      async fn rollback(&self, ctx: &ExecutionContext) -> Result<RollbackResult> {
          let mut rollback_errors = Vec::new();

          // 逆序回滚已执行的步骤
          for step_id in ctx.executed_steps.iter().rev() {
              let step = ctx.get_step(step_id)?;
              let tool = self.tool_registry.get(&step.tool)?;

              if let ToolSafetyLevel::Reversible { .. } = tool.safety_level() {
                  if let Err(e) = tool.rollback(ctx.execution_id).await {
                      rollback_errors.push((*step_id, e));
                  }
              }
          }

          if rollback_errors.is_empty() {
              Ok(RollbackResult::Success)
          } else {
              Ok(RollbackResult::PartialFailure { errors: rollback_errors })
          }
      }
  }

  ---
  七、与现有架构的集成

  L3 Router 扩展

  // 修改现有 L3Router
  impl L3Router {
      pub async fn route(&self, input: &str, context: &Context) -> Result<L3Result> {
          // 1. 任务分析
          let task_type = self.task_analyzer.analyze(input, context).await?;

          match task_type {
              TaskType::SingleTool => {
                  // 现有逻辑：单工具选择
                  self.route_single_tool(input, context).await
              }

              TaskType::MultiStep(hints) => {
                  // 新增：多步骤规划
                  self.plan_multi_step(input, context, hints).await
              }

              TaskType::LikelyMultiStep => {
                  // 启发式判断，需要 LLM 确认
                  self.route_with_planning_check(input, context).await
              }

              TaskType::NeedsClarification => {
                  // 需要用户澄清
                  self.generate_clarification(input, context).await
              }

              TaskType::Chat => {
                  Ok(L3Result::FallbackToChat)
              }
          }
      }

      async fn plan_multi_step(
          &self,
          input: &str,
          context: &Context,
          hints: Vec<String>,
      ) -> Result<L3Result> {
          // 调用 LLM 生成执行计划
          let plan = self.planner.generate_plan(input, context, &self.tools).await?;

          // 计算置信度
          let confidence = self.calculate_plan_confidence(&plan);

          // 判断是否需要确认
          let requires_confirmation = confidence < self.config.auto_execute_threshold
              || plan.has_irreversible_steps()
              || plan.steps.len() > 3;

          Ok(L3Result::ExecutionPlan {
              plan,
              confidence,
              requires_confirmation,
          })
      }
  }

  IntentAction 扩展

  // 扩展现有 IntentAction
  pub enum IntentAction {
      /// 直接执行单工具 (现有)
      Execute {
          tool: ToolRef,
          params: Value,
      },

      /// 执行多步骤计划 (新增)
      ExecutePlan {
          plan: TaskPlan,
      },

      /// 请求确认 (现有，扩展)
      RequestConfirmation {
          confirmation_type: ConfirmationType,
      },

      /// 请求澄清 (现有)
      RequestClarification {
          question: String,
          options: Vec<String>,
      },

      /// 降级为对话 (现有)
      GeneralChat,
  }

  pub enum ConfirmationType {
      SingleTool { tool: ToolRef, params: Value },
      ExecutionPlan { plan: TaskPlan },
  }

  ---
  八、配置项

  [dispatcher.agent]
  # 是否启用 Agent 模式
  enabled = true

  # 多步骤任务的最大步骤数
  max_plan_steps = 10

  # 自动执行阈值 (置信度高于此值时自动执行)
  auto_execute_threshold = 0.9

  # 高风险操作策略
  high_risk_policy = "confirm_each"  # confirm_each | confirm_once | block

  # 失败策略
  failure_policy = "stop_and_rollback"  # stop_and_rollback | stop_and_keep | skip_and_continue

  # 单步骤超时
  step_timeout_ms = 30000

  # 整体计划超时
  plan_timeout_ms = 300000

  # 是否允许并行执行
  allow_parallel_execution = true

  ---
  九、示例场景

  场景 1: 简单的多步骤任务

  用户: "把这段代码翻译成英文注释，然后格式化"

  L3 分析:
  - 检测到两个动作: "翻译" + "格式化"
  - 生成计划:
    Step 1: translate(content=$input, target="en", context="code_comment")
    Step 2: format_code(content=$1.output, language="auto")

  执行:
  - 置信度 0.92 > 0.9，自动执行
  - Step 1 完成，输出英文注释
  - Step 2 完成，输出格式化代码
  - 结果粘贴回用户

  场景 2: 需要确认的复杂任务

  用户: "帮我整理下载文件夹，把图片移到图片文件夹，文档移到文档文件夹，删除超过30天的临时文件"

  L3 分析:
  - 检测到多个动作 + 文件操作 + 删除操作
  - 生成计划:
    Step 1: list_files(path="~/Downloads")
    Step 2: classify_files(files=$1.output)  # 分类
    Step 3: move_files(files=$2.images, dest="~/Pictures")
    Step 4: move_files(files=$2.documents, dest="~/Documents")
    Step 5: delete_files(files=$2.temp, older_than="30d")  # 高风险!

  确认 UI:
  ┌─────────────────────────────────────────────┐
  │ 📋 执行计划                                  │
  │                                             │
  │ ① 扫描下载文件夹                             │
  │ ② 分类文件 (图片/文档/临时)                  │
  │ ③ 移动图片到 ~/Pictures                     │
  │ ④ 移动文档到 ~/Documents                    │
  │ ⚠️ ⑤ 删除30天前的临时文件                    │
  │                                             │
  │ ⚠️ 步骤 5 包含不可逆操作                     │
  │                                             │
  │ [取消]                    [查看详情] [执行]  │
  └─────────────────────────────────────────────┘

  场景 3: 需要澄清的任务

  用户: "发给 John"

  L3 分析:
  - 意图不明确: 发什么? 哪个 John?

  返回 NeedsClarification:
  {
    "question": "请补充以下信息",
    "fields": [
      {"name": "content", "question": "要发送什么内容?", "type": "text"},
      {"name": "recipient", "question": "发给哪个 John?", "options": ["john@work.com", "john@personal.com"]}
    ]
  }

  ---
  总结

  这个 L3 Agent 增强方案：
  ┌────────────┬──────────────────────────────────┐
  │    特性    │               描述               │
  ├────────────┼──────────────────────────────────┤
  │ 向后兼容   │ 不改变 L1/L2 的行为，只扩展 L3   │
  ├────────────┼──────────────────────────────────┤
  │ 渐进式信任 │ 简单任务自动执行，复杂任务需确认 │
  ├────────────┼──────────────────────────────────┤
  │ 安全优先   │ 高风险操作强制确认，支持回滚     │
  ├────────────┼──────────────────────────────────┤
  │ 用户可控   │ 配置驱动，可禁用 Agent 模式      │
  ├────────────┼──────────────────────────────────┤
  │ 可视化     │ 计划预览、执行进度、步骤编辑     │
  └────────────┴──────────────────────────────────┘


 ---
  1. LLM Prompt 设计

  核心要点

  目标: 让 LLM 输出结构化的执行计划，而不是自由发挥

  最大的坑

  坑 1: LLM 会"过度规划"

  用户: "翻译成英文"
  期望: 单步骤 translate()
  实际: LLM 可能输出 5 步计划 (检测语言→分析语法→翻译→校对→格式化)

  解决: 在 prompt 中强调 "最小步骤原则"
  "Only decompose into multiple steps if ABSOLUTELY necessary.
   Prefer single tool when possible."

  坑 2: 参数引用格式不一致

  LLM 可能输出:
  - "$1.output"
  - "${step1.result}"
  - "{{step_1}}"
  - "the result from step 1"  // 自然语言！

  解决: 给出严格的格式示例 + JSON Schema 约束

  坑 3: 工具名称幻觉

  LLM 会发明不存在的工具:
  - "analyze_sentiment" (你没这工具)
  - "send_to_john" (太具体了)

  解决:
  1. Prompt 中明确列出所有工具名
  2. 后处理时验证工具是否存在
  3. 模糊匹配 + 确认 (你是想用 send_email 吗?)

  Prompt 模板精华

  const PLANNING_PROMPT: &str = r#"
  ## 规则 (必须遵守)
  1. 只用这些工具: {tool_names}
  2. 参数引用格式: "$ref:step_id.output"
  3. 能一步完成就一步，别画蛇添足
  4. 输出纯 JSON，不要解释

  ## 工具列表
  {tools_json}

  ## 用户请求
  {input}

  ## 输出
  "#;

  ---
  2. 依赖图拓扑排序

  核心要点

  目标: 确定步骤执行顺序，识别可并行的步骤

  最大的坑

  坑 1: 循环依赖

  Step 1 depends on Step 3
  Step 3 depends on Step 1

  解决: Kahn 算法检测环，发现环直接拒绝计划

  fn topological_sort(steps: &[PlanStep]) -> Result<Vec<Vec<StepId>>> {
      let mut in_degree: HashMap<StepId, usize> = HashMap::new();
      let mut graph: HashMap<StepId, Vec<StepId>> = HashMap::new();

      // 建图...

      // 检测环: 如果排序后节点数 != 总节点数，存在环
      if sorted.iter().map(|g| g.len()).sum::<usize>() != steps.len() {
          return Err(AlephError::CyclicDependency);
      }

      Ok(sorted)
  }

  坑 2: 隐式依赖

  Step 1: read_file("config.toml")
  Step 2: write_file("config.toml", ...)  // 没声明依赖 Step 1，但有冲突!

  解决: 资源冲突检测
  - 同一文件的读写必须串行
  - 引入 "资源锁" 概念

  struct ResourceLock {
      file_locks: HashMap<PathBuf, StepId>,
      network_locks: HashMap<String, StepId>,  // e.g., "email:john@..."
  }

  坑 3: 并行度过高导致系统过载

  10 个步骤都可并行 → 同时发起 10 个 API 请求 → Rate Limit / OOM

  解决: 限制并行度

  let semaphore = Arc::new(Semaphore::new(config.max_parallel_steps)); // 默认 3

  for step in parallelizable_steps {
      let permit = semaphore.acquire().await?;
      tokio::spawn(async move {
          let result = execute_step(step).await;
          drop(permit);
          result
      });
  }

  ---
  3. 与 Memory 模块集成

  核心要点

  目标: Agent 规划时能利用历史上下文，执行后记忆结果

  最大的坑

  坑 1: 上下文爆炸

  Memory 返回 10 条相关记录 × 每条 500 tokens = 5000 tokens
  + 工具描述 2000 tokens
  + 用户输入 500 tokens
  = 7500 tokens 仅 prompt，还没算输出

  解决: 分层上下文
  - 规划阶段: 只用摘要 (每条记忆 50 tokens)
  - 执行阶段: 按需加载完整内容

  struct PlanningContext {
      memory_summaries: Vec<MemorySummary>,  // 精简版
      recent_actions: Vec<ActionSummary>,     // 最近 5 个操作
  }

  struct ExecutionContext {
      full_memories: HashMap<MemoryId, Memory>,  // 按需加载
  }

  坑 2: 记忆时机

  问题: 什么时候存入记忆?
  - 计划生成后? (可能取消)
  - 每步执行后? (中途失败怎么办)
  - 全部完成后? (丢失中间状态)

  解决: 分层记忆

  enum MemoryType {
      Intent,      // 用户意图 (计划确认后立即存)
      Execution,   // 执行过程 (成功的步骤存)
      Result,      // 最终结果 (全部完成存)
  }

  // 失败时: 存 Intent + 已成功的 Execution + FailureRecord

  坑 3: 记忆检索干扰规划

  用户: "发邮件给 John"
  Memory: 上次发给 john@work.com
  但这次用户想发给 john@personal.com

  解决: 记忆作为建议，不作为决定
  - 在确认 UI 中显示 "上次发给 john@work.com"
  - 让用户选择，而不是自动使用

  ---
  4. 回滚策略

  核心要点

  目标: 失败时尽可能恢复到执行前状态

  最大的坑

  坑 1: 部分成功的尴尬

  Step 1: 移动文件 A → 成功
  Step 2: 移动文件 B → 成功
  Step 3: 发送邮件 → 失败

  回滚 Step 1/2? 但邮件没发，文件移了也没意义
  不回滚? 用户状态不一致

  解决: 让用户选择

  enum RollbackChoice {
      RollbackAll,           // 全部回滚
      KeepSuccessful,        // 保留成功的
      RollbackSelective(Vec<StepId>),  // 选择性回滚
  }

  // UI 显示:
  // "步骤 3 失败。步骤 1、2 已完成。"
  // [全部撤销] [保留已完成] [选择撤销]

  坑 2: 回滚本身也会失败

  Step 1: delete_file("important.txt") → 成功
  Step 2: 某操作 → 失败
  回滚 Step 1: 文件已删除，无法恢复!

  解决:
  1. 高危操作先备份
  2. 标记真正不可逆的操作
  3. 执行前明确告知用户

  impl DeleteFileTool {
      async fn execute(&self, params: Value) -> Result<ToolOutput> {
          let path = params["path"].as_str()?;

          // 先备份到临时目录
          let backup_path = self.backup_dir.join(uuid());
          fs::copy(path, &backup_path).await?;

          // 再删除
          fs::remove_file(path).await?;

          Ok(ToolOutput {
              result: json!({"deleted": path}),
              rollback_data: Some(json!({
                  "backup_path": backup_path,
                  "original_path": path,
              })),
          })
      }

      async fn rollback(&self, rollback_data: Value) -> Result<()> {
          let backup = rollback_data["backup_path"].as_str()?;
          let original = rollback_data["original_path"].as_str()?;
          fs::rename(backup, original).await?;
          Ok(())
      }
  }

  坑 3: 外部系统无法回滚

  已发送的邮件、已发布的推文、已支付的订单...

  解决: 分类 + 前置警告

  enum Reversibility {
      FullyReversible,              // 文件移动
      ReversibleWithSideEffects,    // 文件复制 (会占空间)
      PartiallyReversible,          // 数据库事务 (可能有触发器)
      Irreversible,                 // 发邮件、外部 API
  }

  // 计划包含 Irreversible 步骤时，强制显示警告:
  // "⚠️ 此计划包含不可撤销的操作: 发送邮件"

  ---
  总结：实现优先级

  Phase 1 (必须做好):
  ├── LLM Prompt 验证 (工具名存在性、参数格式)
  ├── 循环依赖检测 (拓扑排序)
  └── 基础回滚框架 (可逆操作的备份机制)

  Phase 2 (体验优化):
  ├── 并行度控制
  ├── Memory 分层加载
  └── 部分回滚 UI

  Phase 3 (锦上添花):
  ├── 资源冲突检测
  ├── 智能记忆建议
  └── 回滚策略自定义

  最核心的一句话：永远假设 LLM 会出错，永远假设步骤会失败，永远给用户选择权。
