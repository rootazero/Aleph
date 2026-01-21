//! Task Context Manager for Pipeline Execution
//!
//! This module provides context management for multi-model pipeline execution,
//! including result storage, dependency resolution, and task enrichment.

use crate::dispatcher::agent_types::{Task, TaskResult, TaskType};
use crate::memory::VectorDatabase;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;

/// Error type for context operations
#[derive(Debug, Clone, thiserror::Error)]
pub enum ContextError {
    #[error("Result not found for task: {task_id}")]
    ResultNotFound { task_id: String },

    #[error("Dependency not satisfied: {dependency_id}")]
    DependencyNotSatisfied { dependency_id: String },

    #[error("Context too large: {current_size} > {max_size}")]
    ContextTooLarge {
        current_size: usize,
        max_size: usize,
    },

    #[error("Serialization error: {message}")]
    SerializationError { message: String },

    #[error("Storage error: {message}")]
    StorageError { message: String },
}

/// Metadata for stored task results
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskResultMetadata {
    /// Task ID
    pub task_id: String,

    /// Graph ID this task belongs to
    pub graph_id: String,

    /// Task type
    pub task_type: String,

    /// Source model used for execution
    pub model_used: Option<String>,

    /// Provider name
    pub provider: Option<String>,

    /// Timestamp when result was stored
    pub timestamp: i64,

    /// Token usage
    pub tokens_used: u32,

    /// Execution duration in milliseconds
    pub duration_ms: u64,
}

/// Stored task result with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredTaskResult {
    /// Result metadata
    pub metadata: TaskResultMetadata,

    /// Task result data
    pub result: TaskResult,
}

impl StoredTaskResult {
    /// Create a new stored result
    pub fn new(
        task_id: impl Into<String>,
        graph_id: impl Into<String>,
        task_type: &TaskType,
        result: TaskResult,
    ) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        Self {
            metadata: TaskResultMetadata {
                task_id: task_id.into(),
                graph_id: graph_id.into(),
                task_type: task_type_to_string(task_type),
                model_used: None,
                provider: None,
                timestamp,
                tokens_used: 0,
                duration_ms: result.duration.as_millis() as u64,
            },
            result,
        }
    }

    /// Set model information
    pub fn with_model(mut self, model: impl Into<String>, provider: impl Into<String>) -> Self {
        self.metadata.model_used = Some(model.into());
        self.metadata.provider = Some(provider.into());
        self
    }

    /// Set token usage
    pub fn with_tokens(mut self, tokens: u32) -> Self {
        self.metadata.tokens_used = tokens;
        self
    }
}

/// Task context containing dependency outputs
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TaskContext {
    /// Outputs from dependency tasks, keyed by task ID
    pub dependency_outputs: HashMap<String, serde_json::Value>,

    /// Summaries from dependency tasks
    pub dependency_summaries: HashMap<String, String>,

    /// Combined context string for prompt injection
    pub combined_context: String,

    /// Total size of context in characters
    pub context_size: usize,
}

impl TaskContext {
    /// Create a new empty context
    pub fn new() -> Self {
        Self::default()
    }

    /// Add dependency output
    pub fn add_dependency(
        &mut self,
        task_id: impl Into<String>,
        output: serde_json::Value,
        summary: Option<String>,
    ) {
        let task_id = task_id.into();

        // Update size estimate
        let output_size = serde_json::to_string(&output).map(|s| s.len()).unwrap_or(0);
        self.context_size += output_size;

        self.dependency_outputs.insert(task_id.clone(), output);

        if let Some(s) = summary {
            self.context_size += s.len();
            self.dependency_summaries.insert(task_id, s);
        }
    }

    /// Build combined context string
    pub fn build_combined_context(&mut self) {
        let mut parts = Vec::new();

        for (task_id, output) in &self.dependency_outputs {
            let output_str = match output {
                serde_json::Value::String(s) => s.clone(),
                _ => serde_json::to_string_pretty(output).unwrap_or_default(),
            };

            if let Some(summary) = self.dependency_summaries.get(task_id) {
                parts.push(format!(
                    "## Result from task '{}'\n### Summary\n{}\n### Output\n{}",
                    task_id, summary, output_str
                ));
            } else {
                parts.push(format!("## Result from task '{}'\n{}", task_id, output_str));
            }
        }

        self.combined_context = parts.join("\n\n");
    }

    /// Truncate context to fit within max size
    pub fn truncate_to_fit(&mut self, max_size: usize) {
        if self.combined_context.len() <= max_size {
            return;
        }

        // Simple truncation with marker
        let truncate_marker = "\n\n... [context truncated] ...";
        let available = max_size.saturating_sub(truncate_marker.len());

        if available > 0 {
            self.combined_context =
                format!("{}{}", &self.combined_context[..available], truncate_marker);
        } else {
            self.combined_context = truncate_marker.to_string();
        }

        self.context_size = self.combined_context.len();
    }

    /// Check if context is empty
    pub fn is_empty(&self) -> bool {
        self.dependency_outputs.is_empty()
    }
}

/// Task Context Manager for pipeline execution
///
/// Manages storage and retrieval of task results within a pipeline execution,
/// with optional persistence to the memory module.
pub struct TaskContextManager {
    /// Current graph ID being executed
    graph_id: String,

    /// In-memory result storage for current execution
    results: Arc<RwLock<HashMap<String, StoredTaskResult>>>,

    /// Optional vector database for persistence
    database: Option<Arc<VectorDatabase>>,

    /// Maximum context size for enrichment (in characters)
    max_context_size: usize,
}

impl TaskContextManager {
    /// Create a new context manager for a graph execution
    pub fn new(graph_id: impl Into<String>) -> Self {
        Self {
            graph_id: graph_id.into(),
            results: Arc::new(RwLock::new(HashMap::new())),
            database: None,
            max_context_size: 100_000, // Default 100K chars
        }
    }

    /// Set the vector database for persistence
    pub fn with_database(mut self, database: Arc<VectorDatabase>) -> Self {
        self.database = Some(database);
        self
    }

    /// Set maximum context size
    pub fn with_max_context_size(mut self, size: usize) -> Self {
        self.max_context_size = size;
        self
    }

    /// Get current graph ID
    pub fn graph_id(&self) -> &str {
        &self.graph_id
    }

    /// Store a task result
    ///
    /// Stores the result in memory and optionally persists to database.
    pub async fn store_result(
        &self,
        task: &Task,
        result: TaskResult,
        model_used: Option<&str>,
        provider: Option<&str>,
        tokens_used: u32,
    ) -> Result<(), ContextError> {
        // Create stored result
        let mut stored = StoredTaskResult::new(&task.id, &self.graph_id, &task.task_type, result)
            .with_tokens(tokens_used);

        if let (Some(model), Some(prov)) = (model_used, provider) {
            stored = stored.with_model(model, prov);
        }

        // Store in memory
        {
            let mut results = self.results.write().await;
            results.insert(task.id.clone(), stored.clone());
        }

        // Optionally persist to database
        if let Some(_db) = &self.database {
            // Future: persist to memory module
            // For now, just log
            tracing::debug!(
                task_id = %task.id,
                graph_id = %self.graph_id,
                "Task result stored (database persistence not yet implemented)"
            );
        }

        Ok(())
    }

    /// Get a stored result by task ID
    pub async fn get_result(&self, task_id: &str) -> Option<StoredTaskResult> {
        let results = self.results.read().await;
        results.get(task_id).cloned()
    }

    /// Check if a result exists for a task
    pub async fn has_result(&self, task_id: &str) -> bool {
        let results = self.results.read().await;
        results.contains_key(task_id)
    }

    /// Get context from dependency tasks
    ///
    /// Builds a TaskContext containing outputs from all specified dependencies.
    pub async fn get_context(&self, dependencies: &[String]) -> Result<TaskContext, ContextError> {
        let results = self.results.read().await;
        let mut context = TaskContext::new();

        for dep_id in dependencies {
            if let Some(stored) = results.get(dep_id) {
                context.add_dependency(
                    dep_id,
                    stored.result.output.clone(),
                    stored.result.summary.clone(),
                );
            }
            // Missing dependencies are not errors - they might be optional
        }

        // Build combined context
        context.build_combined_context();

        // Truncate if needed
        if context.context_size > self.max_context_size {
            context.truncate_to_fit(self.max_context_size);
        }

        Ok(context)
    }

    /// Enrich a task with context from its dependencies
    ///
    /// Injects dependency outputs into the task's parameters.
    pub async fn enrich_task(
        &self,
        task: &Task,
        dependencies: &[String],
    ) -> Result<Task, ContextError> {
        if dependencies.is_empty() {
            return Ok(task.clone());
        }

        let context = self.get_context(dependencies).await?;

        if context.is_empty() {
            return Ok(task.clone());
        }

        // Clone task and inject context
        let mut enriched = task.clone();

        // Add context to parameters
        let mut params = match &task.parameters {
            serde_json::Value::Object(map) => map.clone(),
            serde_json::Value::Null => serde_json::Map::new(),
            _ => {
                let mut map = serde_json::Map::new();
                map.insert("original".to_string(), task.parameters.clone());
                map
            }
        };

        // Inject dependency context
        params.insert(
            "_dependency_context".to_string(),
            serde_json::Value::String(context.combined_context.clone()),
        );

        params.insert(
            "_dependency_outputs".to_string(),
            serde_json::to_value(&context.dependency_outputs).map_err(|e| {
                ContextError::SerializationError {
                    message: e.to_string(),
                }
            })?,
        );

        enriched.parameters = serde_json::Value::Object(params);

        Ok(enriched)
    }

    /// Get all stored results
    pub async fn all_results(&self) -> HashMap<String, StoredTaskResult> {
        let results = self.results.read().await;
        results.clone()
    }

    /// Clear all stored results
    pub async fn clear(&self) {
        let mut results = self.results.write().await;
        results.clear();
    }

    /// Get execution summary
    pub async fn summary(&self) -> ContextSummary {
        let results = self.results.read().await;

        let mut total_tokens = 0u32;
        let mut total_duration = Duration::ZERO;
        let mut successful = 0usize;
        let mut failed = 0usize;

        for stored in results.values() {
            total_tokens += stored.metadata.tokens_used;
            total_duration += stored.result.duration;

            // Check if result has error indicator
            if stored.result.output.get("error").is_some() {
                failed += 1;
            } else {
                successful += 1;
            }
        }

        ContextSummary {
            graph_id: self.graph_id.clone(),
            total_tasks: results.len(),
            successful,
            failed,
            total_tokens,
            total_duration,
        }
    }
}

/// Summary of context manager state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextSummary {
    pub graph_id: String,
    pub total_tasks: usize,
    pub successful: usize,
    pub failed: usize,
    pub total_tokens: u32,
    #[serde(with = "duration_serde")]
    pub total_duration: Duration,
}

/// Helper to convert TaskType to string
fn task_type_to_string(task_type: &TaskType) -> String {
    match task_type {
        TaskType::AiInference(_) => "ai_inference".to_string(),
        TaskType::FileOperation(_) => "file_operation".to_string(),
        TaskType::CodeExecution(_) => "code_execution".to_string(),
        TaskType::AppAutomation(_) => "app_automation".to_string(),
        TaskType::DocumentGeneration(_) => "document_generation".to_string(),
        TaskType::ImageGeneration(_) => "image_generation".to_string(),
        TaskType::VideoGeneration(_) => "video_generation".to_string(),
        TaskType::AudioGeneration(_) => "audio_generation".to_string(),
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
    use crate::dispatcher::agent_types::AiTask;

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

    fn create_test_result(output: &str) -> TaskResult {
        TaskResult {
            output: serde_json::json!({ "result": output }),
            artifacts: Vec::new(),
            duration: Duration::from_millis(100),
            summary: Some(format!("Summary: {}", output)),
        }
    }

    // =========================================================================
    // StoredTaskResult Tests
    // =========================================================================

    #[test]
    fn test_stored_result_creation() {
        let task = create_test_task("t1", "Test Task");
        let result = create_test_result("output1");

        let stored = StoredTaskResult::new("t1", "graph-1", &task.task_type, result)
            .with_model("claude-sonnet", "anthropic")
            .with_tokens(150);

        assert_eq!(stored.metadata.task_id, "t1");
        assert_eq!(stored.metadata.graph_id, "graph-1");
        assert_eq!(
            stored.metadata.model_used,
            Some("claude-sonnet".to_string())
        );
        assert_eq!(stored.metadata.provider, Some("anthropic".to_string()));
        assert_eq!(stored.metadata.tokens_used, 150);
    }

    // =========================================================================
    // TaskContext Tests
    // =========================================================================

    #[test]
    fn test_task_context_add_dependency() {
        let mut context = TaskContext::new();

        context.add_dependency(
            "task1",
            serde_json::json!({"key": "value1"}),
            Some("Summary 1".to_string()),
        );

        context.add_dependency("task2", serde_json::json!({"key": "value2"}), None);

        assert_eq!(context.dependency_outputs.len(), 2);
        assert_eq!(context.dependency_summaries.len(), 1);
        assert!(!context.is_empty());
    }

    #[test]
    fn test_task_context_build_combined() {
        let mut context = TaskContext::new();

        context.add_dependency(
            "task1",
            serde_json::json!("output1"),
            Some("Summary 1".to_string()),
        );

        context.build_combined_context();

        assert!(context.combined_context.contains("task1"));
        assert!(context.combined_context.contains("Summary 1"));
        assert!(context.combined_context.contains("output1"));
    }

    #[test]
    fn test_task_context_truncation() {
        let mut context = TaskContext::new();

        // Add large content
        let large_output = "x".repeat(1000);
        context.add_dependency("task1", serde_json::json!(large_output), None);
        context.build_combined_context();

        // Truncate to small size
        context.truncate_to_fit(100);

        assert!(context.combined_context.len() <= 100);
        assert!(context.combined_context.contains("truncated"));
    }

    // =========================================================================
    // TaskContextManager Tests
    // =========================================================================

    #[tokio::test]
    async fn test_context_manager_store_and_retrieve() {
        let manager = TaskContextManager::new("graph-1");
        let task = create_test_task("t1", "Test Task");
        let result = create_test_result("output1");

        manager
            .store_result(&task, result, Some("model1"), Some("provider1"), 100)
            .await
            .unwrap();

        let stored = manager.get_result("t1").await;
        assert!(stored.is_some());

        let stored = stored.unwrap();
        assert_eq!(stored.metadata.task_id, "t1");
        assert_eq!(stored.metadata.model_used, Some("model1".to_string()));
    }

    #[tokio::test]
    async fn test_context_manager_has_result() {
        let manager = TaskContextManager::new("graph-1");
        let task = create_test_task("t1", "Test Task");
        let result = create_test_result("output1");

        assert!(!manager.has_result("t1").await);

        manager
            .store_result(&task, result, None, None, 0)
            .await
            .unwrap();

        assert!(manager.has_result("t1").await);
    }

    #[tokio::test]
    async fn test_context_manager_get_context() {
        let manager = TaskContextManager::new("graph-1");

        // Store two results
        let task1 = create_test_task("t1", "Task 1");
        let result1 = create_test_result("output1");
        manager
            .store_result(&task1, result1, None, None, 0)
            .await
            .unwrap();

        let task2 = create_test_task("t2", "Task 2");
        let result2 = create_test_result("output2");
        manager
            .store_result(&task2, result2, None, None, 0)
            .await
            .unwrap();

        // Get context for dependencies
        let context = manager
            .get_context(&["t1".to_string(), "t2".to_string()])
            .await
            .unwrap();

        assert_eq!(context.dependency_outputs.len(), 2);
        assert!(!context.combined_context.is_empty());
    }

    #[tokio::test]
    async fn test_context_manager_enrich_task() {
        let manager = TaskContextManager::new("graph-1");

        // Store a dependency result
        let dep_task = create_test_task("dep1", "Dependency");
        let dep_result = create_test_result("dependency output");
        manager
            .store_result(&dep_task, dep_result, None, None, 0)
            .await
            .unwrap();

        // Enrich a task that depends on it
        let task = create_test_task("t1", "Main Task");
        let enriched = manager
            .enrich_task(&task, &["dep1".to_string()])
            .await
            .unwrap();

        // Check that context was injected
        let params = enriched.parameters.as_object().unwrap();
        assert!(params.contains_key("_dependency_context"));
        assert!(params.contains_key("_dependency_outputs"));
    }

    #[tokio::test]
    async fn test_context_manager_enrich_empty_deps() {
        let manager = TaskContextManager::new("graph-1");
        let task = create_test_task("t1", "Test Task");

        let enriched = manager.enrich_task(&task, &[]).await.unwrap();

        // Task should be unchanged
        assert_eq!(enriched.parameters, task.parameters);
    }

    #[tokio::test]
    async fn test_context_manager_summary() {
        let manager = TaskContextManager::new("graph-1");

        let task1 = create_test_task("t1", "Task 1");
        let result1 = create_test_result("output1");
        manager
            .store_result(&task1, result1, None, None, 100)
            .await
            .unwrap();

        let task2 = create_test_task("t2", "Task 2");
        let result2 = create_test_result("output2");
        manager
            .store_result(&task2, result2, None, None, 150)
            .await
            .unwrap();

        let summary = manager.summary().await;

        assert_eq!(summary.graph_id, "graph-1");
        assert_eq!(summary.total_tasks, 2);
        assert_eq!(summary.total_tokens, 250);
    }

    #[tokio::test]
    async fn test_context_manager_clear() {
        let manager = TaskContextManager::new("graph-1");

        let task = create_test_task("t1", "Task 1");
        let result = create_test_result("output1");
        manager
            .store_result(&task, result, None, None, 0)
            .await
            .unwrap();

        assert!(manager.has_result("t1").await);

        manager.clear().await;

        assert!(!manager.has_result("t1").await);
    }

    #[tokio::test]
    async fn test_context_manager_max_context_size() {
        let manager = TaskContextManager::new("graph-1").with_max_context_size(50);

        // Store result with large output
        let task = create_test_task("t1", "Task 1");
        let large_result = TaskResult {
            output: serde_json::json!({ "data": "x".repeat(1000) }),
            artifacts: Vec::new(),
            duration: Duration::from_millis(100),
            summary: None,
        };

        manager
            .store_result(&task, large_result, None, None, 0)
            .await
            .unwrap();

        let context = manager.get_context(&["t1".to_string()]).await.unwrap();

        // Context should be truncated
        assert!(context.combined_context.len() <= 50);
    }
}
