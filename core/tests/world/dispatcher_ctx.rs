//! Dispatcher Cortex test context for security pipeline, JSON parsing, decision flow, and DAG scheduling

use alephcore::dispatcher::cortex::{
    parser::{JsonFragment, JsonStreamDetector},
    security::{
        rules::{InstructionOverrideRule, PiiMaskerRule, TagInjectionRule},
        Locale, PipelineResult, SanitizeContext, SecurityConfig, SecurityPipeline,
    },
    DecisionAction, DecisionConfig,
};
use alephcore::dispatcher::{
    DagTaskDisplayStatus, DagTaskInfo, DagTaskPlan, ExecutionCallback, NoOpExecutionCallback,
    RiskEvaluator, RiskLevel, TaskContext, TaskOutput, UserDecision,
};
use alephcore::dispatcher::agent_types::{AiTask, CodeExec, Language, Task, TaskGraph, TaskType};
use alephcore::dispatcher::scheduler::{DagScheduler, ExecutionResult, GraphTaskExecutor};
use alephcore::Result;
use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Mock task executor for testing DAG scheduler
pub struct MockTaskExecutor {
    execution_count: AtomicUsize,
}

impl MockTaskExecutor {
    pub fn new() -> Self {
        Self {
            execution_count: AtomicUsize::new(0),
        }
    }

    pub fn get_execution_count(&self) -> usize {
        self.execution_count.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl GraphTaskExecutor for MockTaskExecutor {
    async fn execute(&self, task: &Task, _context: &str) -> Result<TaskOutput> {
        self.execution_count.fetch_add(1, Ordering::SeqCst);
        Ok(TaskOutput::text(format!("Executed: {}", task.name)))
    }
}

/// Collecting callback that records all execution events
pub struct CollectingCallback {
    pub plan_ready_count: AtomicUsize,
    pub task_start_count: AtomicUsize,
    pub task_complete_count: AtomicUsize,
    pub all_complete_count: AtomicUsize,
    pub confirmation_decision: UserDecision,
}

impl CollectingCallback {
    pub fn new() -> Self {
        Self {
            plan_ready_count: AtomicUsize::new(0),
            task_start_count: AtomicUsize::new(0),
            task_complete_count: AtomicUsize::new(0),
            all_complete_count: AtomicUsize::new(0),
            confirmation_decision: UserDecision::Confirmed,
        }
    }
}

#[async_trait]
impl ExecutionCallback for CollectingCallback {
    async fn on_plan_ready(&self, _plan: &DagTaskPlan) {
        self.plan_ready_count.fetch_add(1, Ordering::SeqCst);
    }

    async fn on_confirmation_required(&self, _plan: &DagTaskPlan) -> UserDecision {
        self.confirmation_decision
    }

    async fn on_task_start(&self, _task_id: &str, _task_name: &str) {
        self.task_start_count.fetch_add(1, Ordering::SeqCst);
    }

    async fn on_task_stream(&self, _task_id: &str, _chunk: &str) {}

    async fn on_task_complete(&self, _task_id: &str, _summary: &str) {
        self.task_complete_count.fetch_add(1, Ordering::SeqCst);
    }

    async fn on_task_retry(&self, _task_id: &str, _attempt: u32, _error: &str) {}

    async fn on_task_deciding(&self, _task_id: &str, _error: &str) {}

    async fn on_task_failed(&self, _task_id: &str, _error: &str) {}

    async fn on_all_complete(&self, _summary: &str) {
        self.all_complete_count.fetch_add(1, Ordering::SeqCst);
    }

    async fn on_cancelled(&self) {}
}

/// Dispatcher context for BDD tests
#[derive(Default)]
pub struct DispatcherContext {
    // === Security Pipeline ===
    /// Security pipeline instance
    pub pipeline: Option<SecurityPipeline>,
    /// Sanitize context for pipeline processing
    pub sanitize_ctx: SanitizeContext,
    /// Result from pipeline processing
    pub pipeline_result: Option<PipelineResult>,

    // === JSON Stream Parsing ===
    /// JSON stream detector
    pub json_detector: Option<JsonStreamDetector>,
    /// Collected JSON fragments from streaming
    pub json_fragments: Vec<JsonFragment>,

    // === Decision Flow ===
    /// Decision configuration
    pub decision_config: Option<DecisionConfig>,
    /// Last decision action
    pub decision_action: Option<DecisionAction>,
    /// Test case results for decision thresholds
    pub decision_test_results: Vec<(f32, DecisionAction, bool)>, // (confidence, expected, passed)

    // === DAG Scheduler ===
    /// Risk evaluator for task assessment
    pub risk_evaluator: Option<RiskEvaluator>,
    /// Last evaluated risk level
    pub last_risk_level: Option<RiskLevel>,
    /// Task context for execution
    pub task_context: Option<TaskContext>,
    /// Last built prompt context
    pub last_prompt_context: Option<String>,
    /// Task graph for DAG execution
    pub task_graph: Option<TaskGraph>,
    /// Task plan from graph
    pub task_plan: Option<DagTaskPlan>,
    /// Task info for display
    pub task_info: Option<DagTaskInfo>,
    /// Last task display status
    pub task_display_status: Option<DagTaskDisplayStatus>,
    /// Mock task executor
    pub mock_executor: Option<Arc<MockTaskExecutor>>,
    /// Collecting callback
    pub collecting_callback: Option<Arc<CollectingCallback>>,
    /// Execution result from DAG scheduler
    pub execution_result: Option<ExecutionResult>,
    /// Graph has high risk (from risk evaluator)
    pub graph_has_high_risk: Option<bool>,
    /// NoOp callback test result
    pub noop_callback_completed: bool,
    /// NoOp callback confirmation result
    pub noop_confirmation_result: Option<UserDecision>,
}

impl std::fmt::Debug for DispatcherContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DispatcherContext")
            .field("pipeline", &self.pipeline.is_some())
            .field("sanitize_ctx", &self.sanitize_ctx)
            .field("pipeline_result", &self.pipeline_result.is_some())
            .field("json_detector", &self.json_detector.is_some())
            .field("json_fragments", &self.json_fragments.len())
            .field("decision_config", &self.decision_config)
            .field("decision_action", &self.decision_action)
            .field("decision_test_results", &self.decision_test_results.len())
            .field("risk_evaluator", &self.risk_evaluator.is_some())
            .field("last_risk_level", &self.last_risk_level)
            .field("task_context", &self.task_context.is_some())
            .field("task_graph", &self.task_graph.is_some())
            .field("task_plan", &self.task_plan.is_some())
            .field("task_info", &self.task_info.is_some())
            .field("execution_result", &self.execution_result.is_some())
            .finish()
    }
}


impl DispatcherContext {
    /// Create a security pipeline with all standard rules
    pub fn create_full_pipeline(&mut self) {
        let mut pipeline = SecurityPipeline::new(SecurityConfig::default_enabled());
        pipeline.add_rule(Box::new(InstructionOverrideRule::default()));
        pipeline.add_rule(Box::new(TagInjectionRule::default()));
        pipeline.add_rule(Box::new(PiiMaskerRule::new()));
        self.pipeline = Some(pipeline);
    }

    /// Create a security pipeline with only PII masking
    pub fn create_pii_only_pipeline(&mut self) {
        let mut pipeline = SecurityPipeline::new(SecurityConfig::default_enabled());
        pipeline.add_rule(Box::new(PiiMaskerRule::new()));
        self.pipeline = Some(pipeline);
    }

    /// Set locale on the sanitize context
    pub fn set_locale(&mut self, locale: Locale) {
        self.sanitize_ctx.locale = locale;
    }

    /// Process input through the security pipeline
    pub fn process_input(&mut self, input: &str) {
        if let Some(pipeline) = &self.pipeline {
            let result = pipeline.process(input, &self.sanitize_ctx);
            self.pipeline_result = Some(result);
        }
    }

    /// Initialize JSON stream detector
    pub fn init_json_detector(&mut self) {
        self.json_detector = Some(JsonStreamDetector::new());
        self.json_fragments.clear();
    }

    /// Push a chunk to the JSON detector
    pub fn push_json_chunk(&mut self, chunk: &str) {
        if let Some(detector) = &mut self.json_detector {
            let fragments = detector.push(chunk);
            self.json_fragments.extend(fragments);
        }
    }

    /// Initialize decision config with defaults
    pub fn init_decision_config(&mut self) {
        self.decision_config = Some(DecisionConfig::default());
    }

    /// Test a confidence value against expected action
    pub fn test_decision(&mut self, confidence: f32, expected: DecisionAction) {
        if let Some(config) = &self.decision_config {
            let actual = config.decide(confidence);
            let passed = actual == expected;
            self.decision_action = Some(actual.clone());
            self.decision_test_results
                .push((confidence, expected, passed));
        }
    }

    /// Get triggered rule names from pipeline result
    pub fn get_triggered_rules(&self) -> Vec<String> {
        self.pipeline_result
            .as_ref()
            .map(|r| r.actions.iter().map(|(name, _)| name.clone()).collect())
            .unwrap_or_default()
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // DAG Scheduler Methods
    // ═══════════════════════════════════════════════════════════════════════════

    /// Create a risk evaluator
    pub fn create_risk_evaluator(&mut self) {
        self.risk_evaluator = Some(RiskEvaluator::new());
    }

    /// Create an AI task
    pub fn create_ai_task(id: &str, name: &str, prompt: &str) -> Task {
        Task::new(
            id,
            name,
            TaskType::AiInference(AiTask {
                prompt: prompt.to_string(),
                requires_privacy: false,
                has_images: false,
                output_format: None,
            }),
        )
    }

    /// Create a code execution task
    pub fn create_code_task(id: &str, name: &str, code: &str) -> Task {
        Task::new(
            id,
            name,
            TaskType::CodeExecution(CodeExec::Script {
                code: code.to_string(),
                language: Language::Python,
            }),
        )
    }

    /// Evaluate risk for a task
    pub fn evaluate_task_risk(&mut self, task: &Task) {
        if let Some(evaluator) = &self.risk_evaluator {
            self.last_risk_level = Some(evaluator.evaluate(task));
        }
    }

    /// Create task context with user input
    pub fn create_task_context(&mut self, user_input: &str) {
        self.task_context = Some(TaskContext::new(user_input));
    }

    /// Record task output
    pub fn record_task_output(&mut self, task_id: &str, output: &str) {
        if let Some(ctx) = &mut self.task_context {
            ctx.record_output(task_id, TaskOutput::text(output));
        }
    }

    /// Record task output with name
    pub fn record_task_output_with_name(&mut self, task_id: &str, name: &str, output: &str) {
        if let Some(ctx) = &mut self.task_context {
            ctx.record_output_with_name(task_id, name, TaskOutput::text(output));
        }
    }

    /// Build prompt context for a task
    pub fn build_prompt_context(&mut self, task_id: &str, dependencies: &[&str]) {
        if let Some(ctx) = &self.task_context {
            let deps: Vec<&str> = dependencies.to_vec();
            self.last_prompt_context = Some(ctx.build_prompt_context(task_id, &deps));
        }
    }

    /// Create a task graph
    pub fn create_task_graph(&mut self, id: &str, title: &str) {
        self.task_graph = Some(TaskGraph::new(id, title));
    }

    /// Add an AI task to the graph
    pub fn add_ai_task_to_graph(&mut self, id: &str, name: &str) {
        if let Some(graph) = &mut self.task_graph {
            let task = Self::create_ai_task(id, name, &format!("{} prompt", name));
            graph.add_task(task);
        }
    }

    /// Add a code task to the graph
    pub fn add_code_task_to_graph(&mut self, id: &str, name: &str, code: &str) {
        if let Some(graph) = &mut self.task_graph {
            let task = Self::create_code_task(id, name, code);
            graph.add_task(task);
        }
    }

    /// Add a dependency to the graph
    pub fn add_graph_dependency(&mut self, from: &str, to: &str) {
        if let Some(graph) = &mut self.task_graph {
            graph.add_dependency(from, to);
        }
    }

    /// Create task plan from graph
    pub fn create_task_plan(&mut self, requires_confirmation: bool) {
        if let Some(graph) = &self.task_graph {
            self.task_plan = Some(DagTaskPlan::from_graph(graph, requires_confirmation));
        }
    }

    /// Create task plan with high risk flag from evaluator
    pub fn create_task_plan_with_risk(&mut self) {
        if let Some(graph) = &self.task_graph {
            let high_risk = self.graph_has_high_risk.unwrap_or(false);
            self.task_plan = Some(DagTaskPlan::from_graph(graph, high_risk));
        }
    }

    /// Evaluate graph for high risk
    pub fn evaluate_graph_risk(&mut self) {
        if let (Some(evaluator), Some(graph)) = (&self.risk_evaluator, &self.task_graph) {
            self.graph_has_high_risk = Some(evaluator.evaluate_graph(graph));
        }
    }

    /// Create task info
    pub fn create_task_info(
        &mut self,
        id: &str,
        name: &str,
        status: DagTaskDisplayStatus,
        risk_level: RiskLevel,
    ) {
        self.task_info = Some(DagTaskInfo::new(id, name, status, risk_level, vec![]));
        self.task_display_status = Some(status);
    }

    /// Add dependency to task info
    pub fn add_task_info_dependency(&mut self, dep: &str) {
        if let Some(info) = &mut self.task_info {
            info.dependencies.push(dep.to_string());
        }
    }

    /// Create mock executor
    pub fn create_mock_executor(&mut self) {
        self.mock_executor = Some(Arc::new(MockTaskExecutor::new()));
    }

    /// Create collecting callback
    pub fn create_collecting_callback(&mut self) {
        self.collecting_callback = Some(Arc::new(CollectingCallback::new()));
    }

    /// Execute the task graph
    pub async fn execute_graph(&mut self) -> Result<()> {
        let graph = self.task_graph.take().ok_or_else(|| {
            alephcore::AlephError::other("No task graph")
        })?;
        let executor = self.mock_executor.clone().ok_or_else(|| {
            alephcore::AlephError::other("No executor")
        })?;
        let callback = self.collecting_callback.clone().ok_or_else(|| {
            alephcore::AlephError::other("No callback")
        })?;
        let context = self.task_context.take().ok_or_else(|| {
            alephcore::AlephError::other("No task context")
        })?;

        let result = DagScheduler::execute_graph(graph, executor, callback, context, None).await?;
        self.execution_result = Some(result);
        Ok(())
    }

    /// Test NoOp callback methods
    pub async fn test_noop_callback(&mut self, plan: &DagTaskPlan) {
        let callback = NoOpExecutionCallback;

        callback.on_plan_ready(plan).await;
        self.noop_confirmation_result = Some(callback.on_confirmation_required(plan).await);
        callback.on_task_start("t1", "Task 1").await;
        callback.on_task_stream("t1", "output").await;
        callback.on_task_complete("t1", "done").await;
        callback.on_task_retry("t1", 1, "error").await;
        callback.on_task_deciding("t1", "error").await;
        callback.on_task_failed("t1", "error").await;
        callback.on_all_complete("done").await;
        callback.on_cancelled().await;

        self.noop_callback_completed = true;
    }

    /// Create empty task plan for testing
    pub fn create_empty_task_plan(&mut self, id: &str, title: &str) {
        self.task_plan = Some(DagTaskPlan {
            id: id.to_string(),
            title: title.to_string(),
            tasks: vec![],
            requires_confirmation: false,
        });
    }
}
