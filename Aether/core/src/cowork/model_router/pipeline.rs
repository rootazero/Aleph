//! Pipeline Executor for Multi-Model Task Execution
//!
//! This module provides infrastructure for executing multi-stage pipelines
//! where each stage can be routed to a different AI model based on task requirements.

use super::{ModelProfile, ModelRouter, RoutingError};
use crate::cowork::types::Task;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Error type for pipeline execution
#[derive(Debug, Clone, thiserror::Error)]
pub enum PipelineError {
    #[error("Stage execution failed: {stage_id} - {message}")]
    StageExecutionFailed { stage_id: String, message: String },

    #[error("Routing error: {0}")]
    RoutingError(#[from] RoutingError),

    #[error("Provider error: {provider} - {message}")]
    ProviderError { provider: String, message: String },

    #[error("Pipeline cancelled")]
    Cancelled,

    #[error("Pipeline paused")]
    Paused,

    #[error("Dependency not found: {dependency_id}")]
    DependencyNotFound { dependency_id: String },

    #[error("Context enrichment failed: {message}")]
    ContextEnrichmentFailed { message: String },
}

/// A single stage in the execution pipeline
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineStage {
    /// Unique identifier for this stage
    pub id: String,

    /// The task to execute in this stage
    pub task: Task,

    /// Optional model override (bypasses automatic routing)
    #[serde(default)]
    pub model_override: Option<String>,

    /// IDs of stages this stage depends on
    #[serde(default)]
    pub depends_on: Vec<String>,

    /// Priority for execution ordering (higher = earlier)
    #[serde(default)]
    pub priority: i32,
}

impl PipelineStage {
    /// Create a new pipeline stage
    pub fn new(id: impl Into<String>, task: Task) -> Self {
        Self {
            id: id.into(),
            task,
            model_override: None,
            depends_on: Vec::new(),
            priority: 0,
        }
    }

    /// Builder: set model override
    pub fn with_model(mut self, model_id: impl Into<String>) -> Self {
        self.model_override = Some(model_id.into());
        self
    }

    /// Builder: add dependency
    pub fn depends_on(mut self, stage_id: impl Into<String>) -> Self {
        self.depends_on.push(stage_id.into());
        self
    }

    /// Builder: set priority
    pub fn with_priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }
}

/// Result of executing a single pipeline stage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageResult {
    /// ID of the stage that produced this result
    pub stage_id: String,

    /// Model profile used for execution
    pub model_used: String,

    /// Provider name
    pub provider: String,

    /// Output from the stage execution
    pub output: serde_json::Value,

    /// Number of tokens used (estimated)
    pub tokens_used: u32,

    /// Execution duration
    #[serde(with = "duration_serde")]
    pub duration: Duration,

    /// Whether the stage completed successfully
    pub success: bool,

    /// Error message if failed
    #[serde(default)]
    pub error: Option<String>,
}

impl StageResult {
    /// Create a successful stage result
    pub fn success(
        stage_id: impl Into<String>,
        model_used: impl Into<String>,
        provider: impl Into<String>,
        output: serde_json::Value,
        tokens_used: u32,
        duration: Duration,
    ) -> Self {
        Self {
            stage_id: stage_id.into(),
            model_used: model_used.into(),
            provider: provider.into(),
            output,
            tokens_used,
            duration,
            success: true,
            error: None,
        }
    }

    /// Create a failed stage result
    pub fn failure(
        stage_id: impl Into<String>,
        model_used: impl Into<String>,
        provider: impl Into<String>,
        error: impl Into<String>,
        duration: Duration,
    ) -> Self {
        Self {
            stage_id: stage_id.into(),
            model_used: model_used.into(),
            provider: provider.into(),
            output: serde_json::Value::Null,
            tokens_used: 0,
            duration,
            success: false,
            error: Some(error.into()),
        }
    }
}

/// Context for pipeline execution, accumulating results across stages
#[derive(Debug, Clone, Default)]
pub struct PipelineContext {
    /// Results from completed stages
    pub results: HashMap<String, StageResult>,

    /// Total tokens used across all stages
    pub total_tokens: u32,

    /// Estimated total cost (in arbitrary units)
    pub estimated_cost: f64,

    /// Total execution time
    pub total_duration: Duration,

    /// Stages that have been completed
    pub completed_stages: Vec<String>,

    /// Stages that failed
    pub failed_stages: Vec<String>,
}

impl PipelineContext {
    /// Create a new empty context
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a stage result to the context
    pub fn add_result(&mut self, result: StageResult) {
        let stage_id = result.stage_id.clone();

        self.total_tokens += result.tokens_used;
        self.total_duration += result.duration;

        if result.success {
            self.completed_stages.push(stage_id.clone());
        } else {
            self.failed_stages.push(stage_id.clone());
        }

        self.results.insert(stage_id, result);
    }

    /// Get result for a specific stage
    pub fn get_result(&self, stage_id: &str) -> Option<&StageResult> {
        self.results.get(stage_id)
    }

    /// Get output from a specific stage
    pub fn get_output(&self, stage_id: &str) -> Option<&serde_json::Value> {
        self.results.get(stage_id).map(|r| &r.output)
    }

    /// Check if a stage has completed (successfully or not)
    pub fn is_stage_complete(&self, stage_id: &str) -> bool {
        self.results.contains_key(stage_id)
    }

    /// Check if all dependencies are satisfied
    pub fn dependencies_satisfied(&self, dependencies: &[String]) -> bool {
        dependencies.iter().all(|dep| {
            self.results
                .get(dep)
                .map(|r| r.success)
                .unwrap_or(false)
        })
    }

    /// Get outputs from dependency stages
    pub fn get_dependency_outputs(&self, dependencies: &[String]) -> HashMap<String, serde_json::Value> {
        let mut outputs = HashMap::new();
        for dep_id in dependencies {
            if let Some(result) = self.results.get(dep_id) {
                if result.success {
                    outputs.insert(dep_id.clone(), result.output.clone());
                }
            }
        }
        outputs
    }

    /// Get a summary of the pipeline execution
    pub fn summary(&self) -> PipelineSummary {
        PipelineSummary {
            total_stages: self.results.len(),
            completed: self.completed_stages.len(),
            failed: self.failed_stages.len(),
            total_tokens: self.total_tokens,
            estimated_cost: self.estimated_cost,
            total_duration: self.total_duration,
        }
    }
}

/// Summary of pipeline execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineSummary {
    pub total_stages: usize,
    pub completed: usize,
    pub failed: usize,
    pub total_tokens: u32,
    pub estimated_cost: f64,
    #[serde(with = "duration_serde")]
    pub total_duration: Duration,
}

/// Execution state for pipeline control
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineState {
    /// Pipeline is ready to run
    Ready,
    /// Pipeline is currently executing
    Running,
    /// Pipeline is paused
    Paused,
    /// Pipeline completed successfully
    Completed,
    /// Pipeline failed
    Failed,
    /// Pipeline was cancelled
    Cancelled,
}

/// Progress event for pipeline execution
#[derive(Debug, Clone)]
pub enum PipelineEvent {
    /// Pipeline started
    Started { total_stages: usize },
    /// Stage started
    StageStarted { stage_id: String, model: String },
    /// Stage completed
    StageCompleted { stage_id: String, result: StageResult },
    /// Stage failed
    StageFailed { stage_id: String, error: String },
    /// Pipeline progress update
    Progress { completed: usize, total: usize },
    /// Pipeline completed
    Completed { summary: PipelineSummary },
    /// Pipeline failed
    Failed { error: String },
    /// Pipeline paused
    Paused,
    /// Pipeline resumed
    Resumed,
    /// Pipeline cancelled
    Cancelled,
}

/// Trait for receiving pipeline progress events
pub trait PipelineProgressHandler: Send + Sync {
    fn on_event(&self, event: PipelineEvent);
}

/// Provider adapter for executing AI tasks
#[async_trait::async_trait]
pub trait ProviderAdapter: Send + Sync {
    /// Execute a task with the specified model profile
    async fn execute(
        &self,
        task: &Task,
        profile: &ModelProfile,
        context: Option<&str>,
    ) -> Result<ExecutionResult, PipelineError>;
}

/// Result from provider execution
#[derive(Debug, Clone)]
pub struct ExecutionResult {
    pub output: String,
    pub tokens_used: u32,
}

/// Pipeline executor for multi-model task execution
pub struct PipelineExecutor<R: ModelRouter, P: ProviderAdapter> {
    /// Model router for task-to-model matching
    router: Arc<R>,

    /// Provider adapter for executing tasks
    provider: Arc<P>,

    /// Current execution state
    state: Arc<RwLock<PipelineState>>,

    /// Progress handlers
    handlers: Vec<Arc<dyn PipelineProgressHandler>>,
}

impl<R: ModelRouter, P: ProviderAdapter> PipelineExecutor<R, P> {
    /// Create a new pipeline executor
    pub fn new(router: Arc<R>, provider: Arc<P>) -> Self {
        Self {
            router,
            provider,
            state: Arc::new(RwLock::new(PipelineState::Ready)),
            handlers: Vec::new(),
        }
    }

    /// Add a progress handler
    pub fn add_handler(&mut self, handler: Arc<dyn PipelineProgressHandler>) {
        self.handlers.push(handler);
    }

    /// Get current execution state
    pub async fn state(&self) -> PipelineState {
        *self.state.read().await
    }

    /// Execute a pipeline of stages
    pub async fn execute_pipeline(
        &self,
        stages: Vec<PipelineStage>,
    ) -> Result<PipelineContext, PipelineError> {
        // Set state to running
        {
            let mut state = self.state.write().await;
            *state = PipelineState::Running;
        }

        self.emit_event(PipelineEvent::Started {
            total_stages: stages.len(),
        });

        let mut context = PipelineContext::new();
        let total_stages = stages.len();

        // Sort stages by priority (higher first) and dependencies
        let ordered_stages = self.order_stages(stages)?;

        for stage in ordered_stages {
            // Check if cancelled or paused
            let current_state = *self.state.read().await;
            match current_state {
                PipelineState::Cancelled => {
                    self.emit_event(PipelineEvent::Cancelled);
                    return Err(PipelineError::Cancelled);
                }
                PipelineState::Paused => {
                    self.emit_event(PipelineEvent::Paused);
                    // Wait for resume
                    self.wait_for_resume().await?;
                    self.emit_event(PipelineEvent::Resumed);
                }
                _ => {}
            }

            // Check dependencies
            if !context.dependencies_satisfied(&stage.depends_on) {
                let missing: Vec<_> = stage
                    .depends_on
                    .iter()
                    .filter(|d| !context.is_stage_complete(d))
                    .cloned()
                    .collect();

                if let Some(first_missing) = missing.first() {
                    return Err(PipelineError::DependencyNotFound {
                        dependency_id: first_missing.clone(),
                    });
                }
            }

            // Execute stage
            let result = self.execute_stage(&stage, &context).await;

            match result {
                Ok(stage_result) => {
                    self.emit_event(PipelineEvent::StageCompleted {
                        stage_id: stage.id.clone(),
                        result: stage_result.clone(),
                    });
                    context.add_result(stage_result);
                }
                Err(e) => {
                    let error_msg = e.to_string();
                    self.emit_event(PipelineEvent::StageFailed {
                        stage_id: stage.id.clone(),
                        error: error_msg.clone(),
                    });

                    // Create failure result
                    let fail_result = StageResult::failure(
                        &stage.id,
                        "unknown",
                        "unknown",
                        &error_msg,
                        Duration::ZERO,
                    );
                    context.add_result(fail_result);

                    // Update state and return error
                    {
                        let mut state = self.state.write().await;
                        *state = PipelineState::Failed;
                    }

                    self.emit_event(PipelineEvent::Failed { error: error_msg });
                    return Err(e);
                }
            }

            // Emit progress
            self.emit_event(PipelineEvent::Progress {
                completed: context.completed_stages.len(),
                total: total_stages,
            });
        }

        // Update state to completed
        {
            let mut state = self.state.write().await;
            *state = PipelineState::Completed;
        }

        let summary = context.summary();
        self.emit_event(PipelineEvent::Completed { summary });

        Ok(context)
    }

    /// Execute a single stage
    async fn execute_stage(
        &self,
        stage: &PipelineStage,
        context: &PipelineContext,
    ) -> Result<StageResult, PipelineError> {
        let start = Instant::now();

        // Determine model to use
        let profile = if let Some(ref override_id) = stage.model_override {
            self.router
                .get_profile(override_id)
                .cloned()
                .ok_or_else(|| RoutingError::ProfileNotFound {
                    profile_id: override_id.clone(),
                })?
        } else {
            self.router.route(&stage.task)?
        };

        self.emit_event(PipelineEvent::StageStarted {
            stage_id: stage.id.clone(),
            model: profile.id.clone(),
        });

        // Build context from dependencies
        let dep_context = if !stage.depends_on.is_empty() {
            let outputs = context.get_dependency_outputs(&stage.depends_on);
            if !outputs.is_empty() {
                Some(serde_json::to_string(&outputs).unwrap_or_default())
            } else {
                None
            }
        } else {
            None
        };

        // Execute with provider
        let exec_result = self
            .provider
            .execute(&stage.task, &profile, dep_context.as_deref())
            .await?;

        let duration = start.elapsed();

        // Parse output as JSON if possible, otherwise wrap as string
        let output = serde_json::from_str(&exec_result.output)
            .unwrap_or_else(|_| serde_json::Value::String(exec_result.output.clone()));

        Ok(StageResult::success(
            &stage.id,
            &profile.id,
            &profile.provider,
            output,
            exec_result.tokens_used,
            duration,
        ))
    }

    /// Order stages by dependencies and priority
    fn order_stages(&self, stages: Vec<PipelineStage>) -> Result<Vec<PipelineStage>, PipelineError> {
        // Simple topological sort
        let mut ordered = Vec::new();
        let mut remaining: Vec<_> = stages.into_iter().collect();
        let mut completed_ids: std::collections::HashSet<String> = std::collections::HashSet::new();

        while !remaining.is_empty() {
            // Sort by priority (descending)
            remaining.sort_by(|a, b| b.priority.cmp(&a.priority));

            // Find stages with all dependencies satisfied
            let ready_idx = remaining.iter().position(|stage| {
                stage
                    .depends_on
                    .iter()
                    .all(|dep| completed_ids.contains(dep))
            });

            match ready_idx {
                Some(idx) => {
                    let stage = remaining.remove(idx);
                    completed_ids.insert(stage.id.clone());
                    ordered.push(stage);
                }
                None => {
                    // Circular dependency or missing dependency
                    let stage = &remaining[0];
                    let missing: Vec<_> = stage
                        .depends_on
                        .iter()
                        .filter(|d| !completed_ids.contains(*d))
                        .cloned()
                        .collect();

                    return Err(PipelineError::DependencyNotFound {
                        dependency_id: missing.first().cloned().unwrap_or_else(|| "unknown".to_string()),
                    });
                }
            }
        }

        Ok(ordered)
    }

    /// Wait for resume after pause
    async fn wait_for_resume(&self) -> Result<(), PipelineError> {
        loop {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let state = *self.state.read().await;
            match state {
                PipelineState::Running => return Ok(()),
                PipelineState::Cancelled => return Err(PipelineError::Cancelled),
                _ => continue,
            }
        }
    }

    /// Emit event to all handlers
    fn emit_event(&self, event: PipelineEvent) {
        for handler in &self.handlers {
            handler.on_event(event.clone());
        }
    }

    /// Cancel the pipeline
    pub async fn cancel(&self) {
        let mut state = self.state.write().await;
        if *state == PipelineState::Running || *state == PipelineState::Paused {
            *state = PipelineState::Cancelled;
        }
    }

    /// Pause the pipeline
    pub async fn pause(&self) {
        let mut state = self.state.write().await;
        if *state == PipelineState::Running {
            *state = PipelineState::Paused;
        }
    }

    /// Resume the pipeline
    pub async fn resume(&self) {
        let mut state = self.state.write().await;
        if *state == PipelineState::Paused {
            *state = PipelineState::Running;
        }
    }
}

/// Serde helper for Duration
mod duration_serde {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::Duration;

    pub fn serialize<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        duration.as_millis().serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        let millis = u64::deserialize(deserializer)?;
        Ok(Duration::from_millis(millis))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cowork::model_router::{Capability, CostTier};
    use crate::cowork::types::{AiTask, TaskType};
    use std::sync::atomic::{AtomicUsize, Ordering};

    // Mock ModelRouter for testing
    struct MockRouter {
        profiles: Vec<ModelProfile>,
    }

    impl MockRouter {
        fn new() -> Self {
            Self {
                profiles: vec![
                    ModelProfile::new("test-model", "test", "test-model-v1")
                        .with_capabilities(vec![Capability::TextAnalysis])
                        .with_cost_tier(CostTier::Medium),
                ],
            }
        }
    }

    impl ModelRouter for MockRouter {
        fn route(&self, _task: &Task) -> Result<ModelProfile, RoutingError> {
            Ok(self.profiles[0].clone())
        }

        fn get_profile(&self, id: &str) -> Option<&ModelProfile> {
            self.profiles.iter().find(|p| p.id == id)
        }

        fn profiles(&self) -> &[ModelProfile] {
            &self.profiles
        }

        fn supports_capability(&self, profile_id: &str, capability: &Capability) -> bool {
            self.get_profile(profile_id)
                .map(|p| p.has_capability(*capability))
                .unwrap_or(false)
        }

        fn find_best_for(&self, _capability: Capability) -> Option<ModelProfile> {
            self.profiles.first().cloned()
        }

        fn find_balanced(&self) -> Option<ModelProfile> {
            self.profiles.first().cloned()
        }

        fn find_cheapest_with(&self, _capability: Capability) -> Option<ModelProfile> {
            self.profiles.first().cloned()
        }
    }

    // Mock ProviderAdapter for testing
    struct MockProvider {
        call_count: AtomicUsize,
    }

    impl MockProvider {
        fn new() -> Self {
            Self {
                call_count: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait::async_trait]
    impl ProviderAdapter for MockProvider {
        async fn execute(
            &self,
            task: &Task,
            _profile: &ModelProfile,
            _context: Option<&str>,
        ) -> Result<ExecutionResult, PipelineError> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            Ok(ExecutionResult {
                output: format!("Result for task: {}", task.name),
                tokens_used: 100,
            })
        }
    }

    fn create_test_task(id: &str, name: &str) -> Task {
        Task::new(
            id,
            name,
            TaskType::AiInference(AiTask {
                prompt: format!("Test prompt for {}", name),
                requires_privacy: false,
                has_images: false,
                output_format: None,
            }),
        )
    }

    // =========================================================================
    // PipelineStage Tests
    // =========================================================================

    #[test]
    fn test_pipeline_stage_creation() {
        let task = create_test_task("t1", "Test Task");
        let stage = PipelineStage::new("stage1", task)
            .with_model("custom-model")
            .depends_on("stage0")
            .with_priority(10);

        assert_eq!(stage.id, "stage1");
        assert_eq!(stage.model_override, Some("custom-model".to_string()));
        assert_eq!(stage.depends_on, vec!["stage0"]);
        assert_eq!(stage.priority, 10);
    }

    // =========================================================================
    // StageResult Tests
    // =========================================================================

    #[test]
    fn test_stage_result_success() {
        let result = StageResult::success(
            "stage1",
            "test-model",
            "test-provider",
            serde_json::json!({"answer": "42"}),
            150,
            Duration::from_millis(500),
        );

        assert!(result.success);
        assert_eq!(result.stage_id, "stage1");
        assert_eq!(result.tokens_used, 150);
        assert!(result.error.is_none());
    }

    #[test]
    fn test_stage_result_failure() {
        let result = StageResult::failure(
            "stage1",
            "test-model",
            "test-provider",
            "Connection timeout",
            Duration::from_millis(1000),
        );

        assert!(!result.success);
        assert_eq!(result.error, Some("Connection timeout".to_string()));
        assert_eq!(result.tokens_used, 0);
    }

    // =========================================================================
    // PipelineContext Tests
    // =========================================================================

    #[test]
    fn test_pipeline_context_add_result() {
        let mut context = PipelineContext::new();

        let result1 = StageResult::success(
            "stage1",
            "model",
            "provider",
            serde_json::json!("output1"),
            100,
            Duration::from_millis(200),
        );

        let result2 = StageResult::success(
            "stage2",
            "model",
            "provider",
            serde_json::json!("output2"),
            150,
            Duration::from_millis(300),
        );

        context.add_result(result1);
        context.add_result(result2);

        assert_eq!(context.total_tokens, 250);
        assert_eq!(context.completed_stages.len(), 2);
        assert!(context.is_stage_complete("stage1"));
        assert!(context.is_stage_complete("stage2"));
    }

    #[test]
    fn test_pipeline_context_dependencies_satisfied() {
        let mut context = PipelineContext::new();

        // Add successful stage
        context.add_result(StageResult::success(
            "stage1",
            "model",
            "provider",
            serde_json::Value::Null,
            0,
            Duration::ZERO,
        ));

        // Dependencies satisfied
        assert!(context.dependencies_satisfied(&["stage1".to_string()]));

        // Dependencies not satisfied
        assert!(!context.dependencies_satisfied(&["stage2".to_string()]));
        assert!(!context.dependencies_satisfied(&["stage1".to_string(), "stage2".to_string()]));
    }

    #[test]
    fn test_pipeline_context_get_dependency_outputs() {
        let mut context = PipelineContext::new();

        context.add_result(StageResult::success(
            "stage1",
            "model",
            "provider",
            serde_json::json!({"key": "value1"}),
            0,
            Duration::ZERO,
        ));

        context.add_result(StageResult::success(
            "stage2",
            "model",
            "provider",
            serde_json::json!({"key": "value2"}),
            0,
            Duration::ZERO,
        ));

        let outputs = context.get_dependency_outputs(&["stage1".to_string(), "stage2".to_string()]);
        assert_eq!(outputs.len(), 2);
        assert!(outputs.contains_key("stage1"));
        assert!(outputs.contains_key("stage2"));
    }

    #[test]
    fn test_pipeline_context_summary() {
        let mut context = PipelineContext::new();

        context.add_result(StageResult::success(
            "s1",
            "m",
            "p",
            serde_json::Value::Null,
            100,
            Duration::from_millis(100),
        ));
        context.add_result(StageResult::failure(
            "s2",
            "m",
            "p",
            "error",
            Duration::from_millis(50),
        ));

        let summary = context.summary();
        assert_eq!(summary.total_stages, 2);
        assert_eq!(summary.completed, 1);
        assert_eq!(summary.failed, 1);
        assert_eq!(summary.total_tokens, 100);
    }

    // =========================================================================
    // PipelineExecutor Tests
    // =========================================================================

    #[tokio::test]
    async fn test_pipeline_executor_single_stage() {
        let router = Arc::new(MockRouter::new());
        let provider = Arc::new(MockProvider::new());
        let executor = PipelineExecutor::new(router, provider.clone());

        let stages = vec![PipelineStage::new("stage1", create_test_task("t1", "Task 1"))];

        let result = executor.execute_pipeline(stages).await;
        assert!(result.is_ok());

        let context = result.unwrap();
        assert_eq!(context.completed_stages.len(), 1);
        assert_eq!(provider.call_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_pipeline_executor_multiple_stages() {
        let router = Arc::new(MockRouter::new());
        let provider = Arc::new(MockProvider::new());
        let executor = PipelineExecutor::new(router, provider.clone());

        let stages = vec![
            PipelineStage::new("stage1", create_test_task("t1", "Task 1")),
            PipelineStage::new("stage2", create_test_task("t2", "Task 2")),
            PipelineStage::new("stage3", create_test_task("t3", "Task 3")),
        ];

        let result = executor.execute_pipeline(stages).await;
        assert!(result.is_ok());

        let context = result.unwrap();
        assert_eq!(context.completed_stages.len(), 3);
        assert_eq!(provider.call_count.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_pipeline_executor_with_dependencies() {
        let router = Arc::new(MockRouter::new());
        let provider = Arc::new(MockProvider::new());
        let executor = PipelineExecutor::new(router, provider);

        let stages = vec![
            PipelineStage::new("stage1", create_test_task("t1", "Task 1")),
            PipelineStage::new("stage2", create_test_task("t2", "Task 2")).depends_on("stage1"),
            PipelineStage::new("stage3", create_test_task("t3", "Task 3"))
                .depends_on("stage1")
                .depends_on("stage2"),
        ];

        let result = executor.execute_pipeline(stages).await;
        assert!(result.is_ok());

        let context = result.unwrap();
        assert_eq!(context.completed_stages.len(), 3);

        // Verify execution order
        let stage1_idx = context.completed_stages.iter().position(|s| s == "stage1").unwrap();
        let stage2_idx = context.completed_stages.iter().position(|s| s == "stage2").unwrap();
        let stage3_idx = context.completed_stages.iter().position(|s| s == "stage3").unwrap();

        assert!(stage1_idx < stage2_idx);
        assert!(stage2_idx < stage3_idx);
    }

    #[tokio::test]
    async fn test_pipeline_executor_priority_ordering() {
        let router = Arc::new(MockRouter::new());
        let provider = Arc::new(MockProvider::new());
        let executor = PipelineExecutor::new(router, provider);

        // stage3 has highest priority but depends on stage1
        let stages = vec![
            PipelineStage::new("stage1", create_test_task("t1", "Task 1")).with_priority(1),
            PipelineStage::new("stage2", create_test_task("t2", "Task 2")).with_priority(2),
            PipelineStage::new("stage3", create_test_task("t3", "Task 3"))
                .with_priority(10)
                .depends_on("stage1"),
        ];

        let result = executor.execute_pipeline(stages).await;
        assert!(result.is_ok());

        let context = result.unwrap();
        // stage1 must be before stage3 due to dependency
        let stage1_idx = context.completed_stages.iter().position(|s| s == "stage1").unwrap();
        let stage3_idx = context.completed_stages.iter().position(|s| s == "stage3").unwrap();
        assert!(stage1_idx < stage3_idx);
    }

    #[tokio::test]
    async fn test_pipeline_executor_empty_pipeline() {
        let router = Arc::new(MockRouter::new());
        let provider = Arc::new(MockProvider::new());
        let executor = PipelineExecutor::new(router, provider);

        let result = executor.execute_pipeline(vec![]).await;
        assert!(result.is_ok());

        let context = result.unwrap();
        assert_eq!(context.completed_stages.len(), 0);
    }

    #[tokio::test]
    async fn test_pipeline_state_transitions() {
        let router = Arc::new(MockRouter::new());
        let provider = Arc::new(MockProvider::new());
        let executor = PipelineExecutor::new(router, provider);

        assert_eq!(executor.state().await, PipelineState::Ready);

        let stages = vec![PipelineStage::new("stage1", create_test_task("t1", "Task 1"))];

        let _ = executor.execute_pipeline(stages).await;

        assert_eq!(executor.state().await, PipelineState::Completed);
    }

    // =========================================================================
    // Progress Handler Tests
    // =========================================================================

    struct TestProgressHandler {
        events: Arc<RwLock<Vec<String>>>,
    }

    impl TestProgressHandler {
        fn new() -> Self {
            Self {
                events: Arc::new(RwLock::new(Vec::new())),
            }
        }

        async fn event_count(&self) -> usize {
            self.events.read().await.len()
        }
    }

    impl PipelineProgressHandler for TestProgressHandler {
        fn on_event(&self, event: PipelineEvent) {
            let events = self.events.clone();
            let event_name = match event {
                PipelineEvent::Started { .. } => "Started",
                PipelineEvent::StageStarted { .. } => "StageStarted",
                PipelineEvent::StageCompleted { .. } => "StageCompleted",
                PipelineEvent::StageFailed { .. } => "StageFailed",
                PipelineEvent::Progress { .. } => "Progress",
                PipelineEvent::Completed { .. } => "Completed",
                PipelineEvent::Failed { .. } => "Failed",
                PipelineEvent::Paused => "Paused",
                PipelineEvent::Resumed => "Resumed",
                PipelineEvent::Cancelled => "Cancelled",
            };
            // Use blocking lock for test
            tokio::spawn(async move {
                events.write().await.push(event_name.to_string());
            });
        }
    }

    #[tokio::test]
    async fn test_pipeline_progress_events() {
        let router = Arc::new(MockRouter::new());
        let provider = Arc::new(MockProvider::new());
        let mut executor = PipelineExecutor::new(router, provider);

        let handler = Arc::new(TestProgressHandler::new());
        executor.add_handler(handler.clone());

        let stages = vec![
            PipelineStage::new("stage1", create_test_task("t1", "Task 1")),
            PipelineStage::new("stage2", create_test_task("t2", "Task 2")),
        ];

        let _ = executor.execute_pipeline(stages).await;

        // Give async handlers time to process
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Should have: Started, StageStarted*2, StageCompleted*2, Progress*2, Completed
        assert!(handler.event_count().await >= 4);
    }
}
