//! Execution Coordinator for Sub-Agent Synchronization
//!
//! Manages the lifecycle of sub-agent executions with synchronous wait capability.
//! Inspired by OpenCode's session-based synchronous execution model.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::{oneshot, RwLock};
use tokio::time::timeout;
use tracing::{debug, info, warn};

use super::traits::{SubAgentResult, ToolCallRecord};

/// Configuration for the ExecutionCoordinator
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoordinatorConfig {
    /// Maximum time to wait for a sub-agent to complete (default: 5 minutes)
    #[serde(default = "default_execution_timeout_ms")]
    pub execution_timeout_ms: u64,

    /// How long to keep completed results before cleanup (default: 1 hour)
    #[serde(default = "default_result_ttl_ms")]
    pub result_ttl_ms: u64,

    /// Maximum concurrent sub-agent executions (default: 5)
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent: usize,

    /// Enable real-time progress events (default: true)
    #[serde(default = "default_progress_events_enabled")]
    pub progress_events_enabled: bool,
}

fn default_execution_timeout_ms() -> u64 {
    300_000 // 5 minutes
}

fn default_result_ttl_ms() -> u64 {
    3_600_000 // 1 hour
}

fn default_max_concurrent() -> usize {
    5
}

fn default_progress_events_enabled() -> bool {
    true
}

impl Default for CoordinatorConfig {
    fn default() -> Self {
        Self {
            execution_timeout_ms: default_execution_timeout_ms(),
            result_ttl_ms: default_result_ttl_ms(),
            max_concurrent: default_max_concurrent(),
            progress_events_enabled: default_progress_events_enabled(),
        }
    }
}

/// Error types for execution coordination
#[derive(Debug, Clone, thiserror::Error)]
pub enum ExecutionError {
    /// Sub-agent did not complete within timeout
    #[error("Execution timeout for request {request_id}: waited {elapsed_ms}ms")]
    Timeout {
        request_id: String,
        elapsed_ms: u64,
        partial_summary: Option<Vec<ToolCallSummary>>,
    },

    /// Sub-agent execution failed
    #[error("Execution failed for request {request_id}: {error}")]
    ExecutionFailed {
        request_id: String,
        error: String,
        tools_completed: Vec<ToolCallSummary>,
    },

    /// No result found (cleaned up or never started)
    #[error("No result found for request {request_id}")]
    NotFound { request_id: String },

    /// Queue timeout - couldn't get execution slot
    #[error("Queue timeout for request {request_id}: no execution slot available")]
    QueueTimeout { request_id: String },

    /// Internal coordination error
    #[error("Internal error: {0}")]
    Internal(String),
}

/// Summary of a tool call (OpenCode-compatible format)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallSummary {
    pub id: String,
    pub tool: String,
    pub state: ToolCallState,
}

/// State of a tool call
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallState {
    pub status: String, // "pending" | "running" | "completed" | "error"
    pub title: Option<String>,
}

impl From<&ToolCallRecord> for ToolCallSummary {
    fn from(record: &ToolCallRecord) -> Self {
        let status = if record.success {
            "completed"
        } else {
            "error"
        };
        Self {
            id: format!("{}_{}", record.name, uuid::Uuid::new_v4().to_string()[..8].to_string()),
            tool: record.name.clone(),
            state: ToolCallState {
                status: status.to_string(),
                title: Some(record.result_summary.clone()),
            },
        }
    }
}

/// Progress update for a tool call
#[derive(Debug, Clone)]
pub struct ToolCallProgress {
    pub call_id: String,
    pub tool_name: String,
    pub status: ToolCallStatus,
    pub timestamp: Instant,
}

/// Status of a tool call during execution
#[derive(Debug, Clone)]
pub enum ToolCallStatus {
    Pending,
    Running,
    Completed { output_preview: String },
    Failed { error: String },
}

impl ToolCallStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Completed { .. } => "completed",
            Self::Failed { .. } => "error",
        }
    }
}

/// A pending execution awaiting result
struct PendingExecution {
    request_id: String,
    created_at: Instant,
    /// Oneshot channel to signal completion
    completion_tx: Option<oneshot::Sender<SubAgentResult>>,
    /// Progress tracking
    tool_calls: Vec<ToolCallProgress>,
}

/// A completed execution with result
struct CompletedExecution {
    request_id: String,
    result: SubAgentResult,
    completed_at: Instant,
    /// Aggregated tool call summary
    tool_summary: Vec<ToolCallSummary>,
}

/// Handle for tracking an execution
#[derive(Debug, Clone)]
pub struct ExecutionHandle {
    pub request_id: String,
    pub started_at: Instant,
}

/// Execution Coordinator
///
/// Manages the lifecycle of sub-agent executions with synchronous wait capability.
pub struct ExecutionCoordinator {
    /// Pending executions awaiting results
    pending: RwLock<HashMap<String, PendingExecution>>,
    /// Completed results with TTL
    completed: RwLock<HashMap<String, CompletedExecution>>,
    /// Configuration
    config: CoordinatorConfig,
    /// Concurrency semaphore
    semaphore: Arc<tokio::sync::Semaphore>,
}

impl ExecutionCoordinator {
    /// Create a new ExecutionCoordinator with default config
    pub fn new(config: CoordinatorConfig) -> Self {
        let semaphore = Arc::new(tokio::sync::Semaphore::new(config.max_concurrent));
        Self {
            pending: RwLock::new(HashMap::new()),
            completed: RwLock::new(HashMap::new()),
            config,
            semaphore,
        }
    }

    /// Start a new execution and get a handle for waiting
    pub async fn start_execution(&self, request_id: &str) -> ExecutionHandle {
        let (tx, _rx) = oneshot::channel();

        let pending = PendingExecution {
            request_id: request_id.to_string(),
            created_at: Instant::now(),
            completion_tx: Some(tx),
            tool_calls: Vec::new(),
        };

        {
            let mut pending_map = self.pending.write().await;
            pending_map.insert(request_id.to_string(), pending);
        }

        debug!(request_id = %request_id, "Started execution tracking");

        ExecutionHandle {
            request_id: request_id.to_string(),
            started_at: Instant::now(),
        }
    }

    /// Wait for a specific execution to complete (with timeout)
    pub async fn wait_for_result(
        &self,
        request_id: &str,
        wait_timeout: Duration,
    ) -> Result<SubAgentResult, ExecutionError> {
        // First check if already completed
        {
            let completed = self.completed.read().await;
            if let Some(exec) = completed.get(request_id) {
                return Ok(exec.result.clone());
            }
        }

        // Get the receiver from pending
        let rx = {
            let mut pending = self.pending.write().await;
            if let Some(exec) = pending.get_mut(request_id) {
                // Create a new channel and swap out the sender
                let (tx, rx) = oneshot::channel();
                exec.completion_tx = Some(tx);
                Some(rx)
            } else {
                None
            }
        };

        match rx {
            Some(rx) => {
                // Wait with timeout
                match timeout(wait_timeout, rx).await {
                    Ok(Ok(result)) => Ok(result),
                    Ok(Err(_)) => {
                        // Channel closed without sending
                        Err(ExecutionError::Internal(
                            "Completion channel closed unexpectedly".to_string(),
                        ))
                    }
                    Err(_) => {
                        // Timeout - get partial results if available
                        let partial = self.get_partial_summary(request_id).await;
                        Err(ExecutionError::Timeout {
                            request_id: request_id.to_string(),
                            elapsed_ms: wait_timeout.as_millis() as u64,
                            partial_summary: partial,
                        })
                    }
                }
            }
            None => {
                // Check completed again (race condition)
                let completed = self.completed.read().await;
                if let Some(exec) = completed.get(request_id) {
                    Ok(exec.result.clone())
                } else {
                    Err(ExecutionError::NotFound {
                        request_id: request_id.to_string(),
                    })
                }
            }
        }
    }

    /// Wait for multiple executions (for parallel dispatch)
    pub async fn wait_for_all(
        &self,
        request_ids: &[String],
        wait_timeout: Duration,
    ) -> Vec<(String, Result<SubAgentResult, ExecutionError>)> {
        let futures: Vec<_> = request_ids
            .iter()
            .map(|id| {
                let id = id.clone();
                let timeout = wait_timeout;
                async move {
                    let result = self.wait_for_result(&id, timeout).await;
                    (id, result)
                }
            })
            .collect();

        futures::future::join_all(futures).await
    }

    /// Called by event handler when execution completes
    pub async fn on_execution_completed(&self, result: SubAgentResult) {
        let request_id = result.request_id.clone();

        // Remove from pending and get the sender
        let sender = {
            let mut pending = self.pending.write().await;
            pending.remove(&request_id).and_then(|mut exec| exec.completion_tx.take())
        };

        // Build tool summary from result
        let tool_summary: Vec<ToolCallSummary> = result
            .tools_called
            .iter()
            .map(ToolCallSummary::from)
            .collect();

        // Store in completed
        {
            let mut completed = self.completed.write().await;
            completed.insert(
                request_id.clone(),
                CompletedExecution {
                    request_id: request_id.clone(),
                    result: result.clone(),
                    completed_at: Instant::now(),
                    tool_summary,
                },
            );
        }

        // Signal completion via channel
        if let Some(tx) = sender {
            if tx.send(result).is_err() {
                debug!(request_id = %request_id, "No receiver waiting for result");
            }
        }

        info!(request_id = %request_id, "Execution completed and stored");
    }

    /// Called for each tool call progress
    pub async fn on_tool_progress(&self, request_id: &str, progress: ToolCallProgress) {
        let mut pending = self.pending.write().await;
        if let Some(exec) = pending.get_mut(request_id) {
            exec.tool_calls.push(progress);
        }
    }

    /// Get partial summary for a pending execution
    async fn get_partial_summary(&self, request_id: &str) -> Option<Vec<ToolCallSummary>> {
        let pending = self.pending.read().await;
        pending.get(request_id).map(|exec| {
            exec.tool_calls
                .iter()
                .map(|p| ToolCallSummary {
                    id: p.call_id.clone(),
                    tool: p.tool_name.clone(),
                    state: ToolCallState {
                        status: p.status.as_str().to_string(),
                        title: None,
                    },
                })
                .collect()
        })
    }

    /// Get the tool summary for a completed execution
    pub async fn get_tool_summary(&self, request_id: &str) -> Option<Vec<ToolCallSummary>> {
        let completed = self.completed.read().await;
        completed.get(request_id).map(|exec| exec.tool_summary.clone())
    }

    /// Acquire an execution slot (for concurrency limiting)
    pub async fn acquire_slot(&self, wait_timeout: Duration) -> Result<ExecutionSlot, ExecutionError> {
        match timeout(wait_timeout, self.semaphore.clone().acquire_owned()).await {
            Ok(Ok(permit)) => Ok(ExecutionSlot { _permit: permit }),
            Ok(Err(_)) => Err(ExecutionError::Internal("Semaphore closed".to_string())),
            Err(_) => Err(ExecutionError::QueueTimeout {
                request_id: "unknown".to_string(),
            }),
        }
    }

    /// Clean up expired completed results
    pub async fn cleanup_expired(&self) {
        let ttl = Duration::from_millis(self.config.result_ttl_ms);
        let now = Instant::now();

        let mut completed = self.completed.write().await;
        let expired: Vec<String> = completed
            .iter()
            .filter(|(_, exec)| now.duration_since(exec.completed_at) > ttl)
            .map(|(id, _)| id.clone())
            .collect();

        for id in &expired {
            completed.remove(id);
        }

        if !expired.is_empty() {
            info!(count = expired.len(), "Cleaned up expired execution results");
        }
    }

    /// Clean up timed-out pending executions
    pub async fn cleanup_timed_out(&self) {
        let execution_timeout = Duration::from_millis(self.config.execution_timeout_ms);
        let now = Instant::now();

        let mut pending = self.pending.write().await;
        let timed_out: Vec<String> = pending
            .iter()
            .filter(|(_, exec)| now.duration_since(exec.created_at) > execution_timeout)
            .map(|(id, _)| id.clone())
            .collect();

        for id in &timed_out {
            if let Some(mut exec) = pending.remove(id) {
                if let Some(tx) = exec.completion_tx.take() {
                    // Send a timeout result
                    let _ = tx.send(SubAgentResult::failure(
                        id.as_str(),
                        format!("Execution timed out after {}ms", self.config.execution_timeout_ms),
                    ));
                }
            }
        }

        if !timed_out.is_empty() {
            warn!(count = timed_out.len(), "Cleaned up timed-out pending executions");
        }
    }

    /// Check if an execution is pending
    pub async fn is_pending(&self, request_id: &str) -> bool {
        let pending = self.pending.read().await;
        pending.contains_key(request_id)
    }

    /// Check if an execution has completed
    pub async fn is_completed(&self, request_id: &str) -> bool {
        let completed = self.completed.read().await;
        completed.contains_key(request_id)
    }

    /// Get statistics about current executions
    pub async fn get_stats(&self) -> CoordinatorStats {
        let pending = self.pending.read().await;
        let completed = self.completed.read().await;

        CoordinatorStats {
            pending_count: pending.len(),
            completed_count: completed.len(),
            available_slots: self.semaphore.available_permits(),
            max_slots: self.config.max_concurrent,
        }
    }
}

/// Execution slot guard (releases on drop)
pub struct ExecutionSlot {
    _permit: tokio::sync::OwnedSemaphorePermit,
}

/// Statistics about coordinator state
#[derive(Debug, Clone, Serialize)]
pub struct CoordinatorStats {
    pub pending_count: usize,
    pub completed_count: usize,
    pub available_slots: usize,
    pub max_slots: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn test_coordinator_creation() {
        let config = CoordinatorConfig::default();
        let coordinator = ExecutionCoordinator::new(config);

        let stats = coordinator.get_stats().await;
        assert_eq!(stats.pending_count, 0);
        assert_eq!(stats.completed_count, 0);
        assert_eq!(stats.available_slots, 5);
    }

    #[tokio::test]
    async fn test_start_execution() {
        let coordinator = ExecutionCoordinator::new(CoordinatorConfig::default());

        let handle = coordinator.start_execution("req-1").await;
        assert_eq!(handle.request_id, "req-1");

        assert!(coordinator.is_pending("req-1").await);
        assert!(!coordinator.is_completed("req-1").await);
    }

    #[tokio::test]
    async fn test_on_execution_completed() {
        let coordinator = ExecutionCoordinator::new(CoordinatorConfig::default());

        // Start execution
        coordinator.start_execution("req-1").await;

        // Complete it
        let result = SubAgentResult::success("req-1", "Done");
        coordinator.on_execution_completed(result).await;

        assert!(!coordinator.is_pending("req-1").await);
        assert!(coordinator.is_completed("req-1").await);
    }

    #[tokio::test]
    async fn test_wait_for_result_success() {
        let coordinator = Arc::new(ExecutionCoordinator::new(CoordinatorConfig::default()));

        // Start execution
        coordinator.start_execution("req-1").await;

        // Simulate completion in background
        let coord_clone = coordinator.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let result = SubAgentResult::success("req-1", "Done");
            coord_clone.on_execution_completed(result).await;
        });

        // Wait should succeed
        let result = coordinator
            .wait_for_result("req-1", Duration::from_secs(1))
            .await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().summary, "Done");
    }

    #[tokio::test]
    async fn test_wait_for_result_timeout() {
        let coordinator = ExecutionCoordinator::new(CoordinatorConfig::default());

        // Start execution but don't complete it
        coordinator.start_execution("req-1").await;

        // Wait should timeout
        let result = coordinator
            .wait_for_result("req-1", Duration::from_millis(100))
            .await;

        assert!(matches!(result, Err(ExecutionError::Timeout { .. })));
    }

    #[tokio::test]
    async fn test_wait_for_all() {
        let coordinator = Arc::new(ExecutionCoordinator::new(CoordinatorConfig::default()));

        // Start multiple executions
        coordinator.start_execution("req-1").await;
        coordinator.start_execution("req-2").await;
        coordinator.start_execution("req-3").await;

        // Complete them in different order
        let coord_clone = coordinator.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            coord_clone
                .on_execution_completed(SubAgentResult::success("req-2", "Done 2"))
                .await;

            tokio::time::sleep(Duration::from_millis(20)).await;
            coord_clone
                .on_execution_completed(SubAgentResult::success("req-1", "Done 1"))
                .await;

            tokio::time::sleep(Duration::from_millis(10)).await;
            coord_clone
                .on_execution_completed(SubAgentResult::success("req-3", "Done 3"))
                .await;
        });

        // Wait for all
        let results = coordinator
            .wait_for_all(
                &["req-1".to_string(), "req-2".to_string(), "req-3".to_string()],
                Duration::from_secs(1),
            )
            .await;

        // All should succeed and be correlated with request IDs
        assert_eq!(results.len(), 3);
        for (id, result) in results {
            assert!(result.is_ok(), "Request {} failed", id);
        }
    }

    #[tokio::test]
    async fn test_wait_for_already_completed() {
        let coordinator = ExecutionCoordinator::new(CoordinatorConfig::default());

        // Start and immediately complete
        coordinator.start_execution("req-1").await;
        coordinator
            .on_execution_completed(SubAgentResult::success("req-1", "Already done"))
            .await;

        // Wait should return immediately
        let result = coordinator
            .wait_for_result("req-1", Duration::from_secs(1))
            .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap().summary, "Already done");
    }

    #[tokio::test]
    async fn test_concurrency_limiting() {
        let config = CoordinatorConfig {
            max_concurrent: 2,
            ..Default::default()
        };
        let coordinator = ExecutionCoordinator::new(config);

        // Acquire 2 slots
        let slot1 = coordinator.acquire_slot(Duration::from_secs(1)).await;
        let slot2 = coordinator.acquire_slot(Duration::from_secs(1)).await;

        assert!(slot1.is_ok());
        assert!(slot2.is_ok());

        // Third should timeout quickly
        let slot3 = coordinator.acquire_slot(Duration::from_millis(100)).await;
        assert!(matches!(slot3, Err(ExecutionError::QueueTimeout { .. })));

        // Drop one slot
        drop(slot1);

        // Now should succeed
        let slot3 = coordinator.acquire_slot(Duration::from_secs(1)).await;
        assert!(slot3.is_ok());
    }

    #[tokio::test]
    async fn test_cleanup_expired() {
        let config = CoordinatorConfig {
            result_ttl_ms: 50, // Very short TTL for testing
            ..Default::default()
        };
        let coordinator = ExecutionCoordinator::new(config);

        // Start and complete
        coordinator.start_execution("req-1").await;
        coordinator
            .on_execution_completed(SubAgentResult::success("req-1", "Done"))
            .await;

        assert!(coordinator.is_completed("req-1").await);

        // Wait for TTL to expire
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Cleanup
        coordinator.cleanup_expired().await;

        // Should be gone
        assert!(!coordinator.is_completed("req-1").await);
    }

    #[tokio::test]
    async fn test_tool_progress_tracking() {
        let coordinator = ExecutionCoordinator::new(CoordinatorConfig::default());

        // Start execution
        coordinator.start_execution("req-1").await;

        // Add progress
        coordinator
            .on_tool_progress(
                "req-1",
                ToolCallProgress {
                    call_id: "call-1".to_string(),
                    tool_name: "bash".to_string(),
                    status: ToolCallStatus::Running,
                    timestamp: Instant::now(),
                },
            )
            .await;

        // Get partial summary
        let summary = coordinator.get_partial_summary("req-1").await;
        assert!(summary.is_some());
        assert_eq!(summary.unwrap().len(), 1);
    }
}
