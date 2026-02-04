//! Run Event Bus
//!
//! Provides a dedicated event broadcasting system for agent run lifecycle.
//! This enables multi-subscriber event broadcasting, input queueing during
//! streaming (human-in-the-loop), and waiting for run completion with timeout.
//!
//! # Architecture
//!
//! Each active run has an `ActiveRunHandle` which manages:
//! - Event broadcasting to multiple subscribers via `broadcast::Sender`
//! - Input queueing for human-in-the-loop via `mpsc::Sender`
//! - Cancellation signaling via `oneshot::Sender`
//! - Sequence and chunk counters for ordered event delivery
//!
//! # Example
//!
//! ```ignore
//! use alephcore::gateway::run_event_bus::{ActiveRunHandle, RunEvent, wait_for_run_end};
//! use std::time::Duration;
//!
//! // Create a new run handle
//! let (handle, input_rx, cancel_rx) = ActiveRunHandle::new(
//!     "run-123".to_string(),
//!     SessionKey::main("main"),
//! );
//!
//! // Subscribe to events
//! let mut events = handle.subscribe();
//!
//! // Emit events during execution
//! handle.emit(RunEvent::StatusChanged {
//!     run_id: "run-123".to_string(),
//!     status: RunStatus::Running,
//! });
//!
//! // Wait for completion
//! let result = wait_for_run_end(&mut events, Duration::from_secs(30)).await;
//! ```

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::{broadcast, mpsc, oneshot, Mutex};

use super::router::SessionKey;

/// Default channel capacity for run event broadcasting
const RUN_EVENT_CHANNEL_SIZE: usize = 256;

/// Default channel capacity for input queueing
const INPUT_CHANNEL_SIZE: usize = 16;

// ============================================================================
// Run Status
// ============================================================================

/// Status of an agent run
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    /// Run is queued, waiting to start
    Queued,
    /// Run is actively executing
    Running,
    /// Run completed successfully
    Completed,
    /// Run failed with an error
    Failed,
    /// Run was cancelled by user or system
    Cancelled,
}

impl std::fmt::Display for RunStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunStatus::Queued => write!(f, "queued"),
            RunStatus::Running => write!(f, "running"),
            RunStatus::Completed => write!(f, "completed"),
            RunStatus::Failed => write!(f, "failed"),
            RunStatus::Cancelled => write!(f, "cancelled"),
        }
    }
}

// ============================================================================
// Run Events
// ============================================================================

/// Events emitted during an agent run lifecycle
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunEvent {
    /// Run status changed
    StatusChanged {
        run_id: String,
        status: RunStatus,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },

    /// Token delta from LLM response (streaming)
    TokenDelta {
        run_id: String,
        seq: u64,
        delta: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        role: Option<String>,
    },

    /// Reasoning/thinking content delta (streaming)
    ReasoningDelta {
        run_id: String,
        seq: u64,
        delta: String,
        is_complete: bool,
    },

    /// Tool execution started
    ToolStart {
        run_id: String,
        seq: u64,
        tool_name: String,
        tool_call_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        input: Option<serde_json::Value>,
    },

    /// Tool execution ended
    ToolEnd {
        run_id: String,
        seq: u64,
        tool_name: String,
        tool_call_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        output: Option<serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        duration_ms: u64,
    },

    /// Run completed successfully
    RunCompleted {
        run_id: String,
        seq: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
        total_tokens: u64,
        tool_calls: u32,
        loops: u32,
        duration_ms: u64,
    },

    /// Run failed with an error
    RunFailed {
        run_id: String,
        seq: u64,
        error: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        error_code: Option<String>,
    },

    /// Run was cancelled
    RunCancelled {
        run_id: String,
        seq: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },

    /// Input requested from user (human-in-the-loop)
    InputRequested {
        run_id: String,
        seq: u64,
        prompt: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        input_type: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        timeout_secs: Option<u64>,
    },

    /// Input received from user
    InputReceived {
        run_id: String,
        seq: u64,
        input: String,
    },
}

impl RunEvent {
    /// Get the run_id from any event variant
    pub fn run_id(&self) -> &str {
        match self {
            RunEvent::StatusChanged { run_id, .. } => run_id,
            RunEvent::TokenDelta { run_id, .. } => run_id,
            RunEvent::ReasoningDelta { run_id, .. } => run_id,
            RunEvent::ToolStart { run_id, .. } => run_id,
            RunEvent::ToolEnd { run_id, .. } => run_id,
            RunEvent::RunCompleted { run_id, .. } => run_id,
            RunEvent::RunFailed { run_id, .. } => run_id,
            RunEvent::RunCancelled { run_id, .. } => run_id,
            RunEvent::InputRequested { run_id, .. } => run_id,
            RunEvent::InputReceived { run_id, .. } => run_id,
        }
    }

    /// Check if this is a terminal event (run ended)
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            RunEvent::RunCompleted { .. } | RunEvent::RunFailed { .. } | RunEvent::RunCancelled { .. }
        )
    }
}

// ============================================================================
// Run End Result
// ============================================================================

/// Result type for wait_for_run_end
#[derive(Debug, Clone)]
pub enum RunEndResult {
    /// Run completed successfully
    Completed {
        summary: Option<String>,
        total_tokens: u64,
        tool_calls: u32,
        loops: u32,
        duration_ms: u64,
    },

    /// Run failed with an error
    Failed {
        error: String,
        error_code: Option<String>,
    },

    /// Run was cancelled
    Cancelled { reason: Option<String> },
}

impl RunEndResult {
    /// Check if the result is successful
    pub fn is_success(&self) -> bool {
        matches!(self, RunEndResult::Completed { .. })
    }

    /// Get error message if failed
    pub fn error(&self) -> Option<&str> {
        match self {
            RunEndResult::Failed { error, .. } => Some(error),
            _ => None,
        }
    }
}

// ============================================================================
// Errors
// ============================================================================

/// Errors that can occur when waiting for a run to complete
#[derive(Debug, Error)]
pub enum WaitError {
    /// Timeout waiting for run to complete
    #[error("timeout waiting for run to complete after {0:?}")]
    Timeout(Duration),

    /// Channel was closed before run completed
    #[error("event channel closed unexpectedly")]
    ChannelClosed,

    /// Receiver lagged behind and missed events
    #[error("receiver lagged behind, missed {0} events")]
    Lagged(u64),
}

/// Errors that can occur when queueing input
#[derive(Debug, Error)]
pub enum QueueError {
    /// Run has already completed or been cancelled
    #[error("run has already ended")]
    RunEnded,

    /// Input queue is full
    #[error("input queue is full")]
    QueueFull,

    /// Channel was closed
    #[error("input channel closed")]
    ChannelClosed,
}

// ============================================================================
// Active Run Handle
// ============================================================================

/// Handle for managing an active agent run
///
/// This struct provides:
/// - Event broadcasting to multiple subscribers
/// - Input queueing for human-in-the-loop scenarios
/// - Cancellation signaling
/// - Sequence and chunk counters for ordered event delivery
///
/// # Thread Safety
///
/// `ActiveRunHandle` is designed to be shared across tasks via `Arc`.
/// The `cancel_tx` is wrapped in a Mutex to allow safe one-time consumption.
#[derive(Debug)]
pub struct ActiveRunHandle {
    /// Unique run identifier
    pub run_id: String,

    /// Session key for this run
    pub session_key: SessionKey,

    /// When the run was started
    pub started_at: DateTime<Utc>,

    /// Broadcast sender for run events (multi-subscriber)
    event_tx: broadcast::Sender<RunEvent>,

    /// Input sender for human-in-the-loop (mpsc for backpressure)
    input_tx: mpsc::Sender<String>,

    /// One-shot cancel sender (can only be used once)
    cancel_tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,

    /// Monotonically increasing sequence counter
    seq_counter: AtomicU64,

    /// Chunk counter for response streaming
    chunk_counter: AtomicU32,
}

impl ActiveRunHandle {
    /// Create a new active run handle
    ///
    /// Returns the handle along with receivers for input and cancellation:
    /// - `mpsc::Receiver<String>` for receiving user input
    /// - `oneshot::Receiver<()>` for receiving cancellation signal
    pub fn new(run_id: String, session_key: SessionKey) -> (Self, mpsc::Receiver<String>, oneshot::Receiver<()>) {
        let (event_tx, _) = broadcast::channel(RUN_EVENT_CHANNEL_SIZE);
        let (input_tx, input_rx) = mpsc::channel(INPUT_CHANNEL_SIZE);
        let (cancel_tx, cancel_rx) = oneshot::channel();

        let handle = Self {
            run_id,
            session_key,
            started_at: Utc::now(),
            event_tx,
            input_tx,
            cancel_tx: Arc::new(Mutex::new(Some(cancel_tx))),
            seq_counter: AtomicU64::new(0),
            chunk_counter: AtomicU32::new(0),
        };

        (handle, input_rx, cancel_rx)
    }

    /// Subscribe to run events
    ///
    /// Returns a receiver that will receive all events emitted after this call.
    /// Multiple subscribers can exist simultaneously.
    pub fn subscribe(&self) -> broadcast::Receiver<RunEvent> {
        self.event_tx.subscribe()
    }

    /// Emit an event to all subscribers
    ///
    /// Returns the number of subscribers that received the event.
    /// Returns 0 if there are no active subscribers.
    pub fn emit(&self, event: RunEvent) -> usize {
        self.event_tx.send(event).unwrap_or(0)
    }

    /// Get the next sequence number
    ///
    /// This is monotonically increasing and can be used to order events.
    pub fn next_seq(&self) -> u64 {
        self.seq_counter.fetch_add(1, Ordering::SeqCst)
    }

    /// Get the next chunk index
    ///
    /// This is used for ordering response chunks during streaming.
    pub fn next_chunk(&self) -> u32 {
        self.chunk_counter.fetch_add(1, Ordering::SeqCst)
    }

    /// Get a clone of the input sender
    ///
    /// This can be used to queue input from external sources.
    pub fn input_sender(&self) -> mpsc::Sender<String> {
        self.input_tx.clone()
    }

    /// Take the cancel sender (one-time use)
    ///
    /// Returns `Some(oneshot::Sender)` the first time called, `None` afterwards.
    /// This ensures cancellation can only be triggered once.
    pub async fn take_cancel_tx(&self) -> Option<oneshot::Sender<()>> {
        let mut guard = self.cancel_tx.lock().await;
        guard.take()
    }

    /// Get the current sequence number without incrementing
    pub fn current_seq(&self) -> u64 {
        self.seq_counter.load(Ordering::SeqCst)
    }

    /// Get the current chunk counter without incrementing
    pub fn current_chunk(&self) -> u32 {
        self.chunk_counter.load(Ordering::SeqCst)
    }

    /// Get the number of active subscribers
    pub fn subscriber_count(&self) -> usize {
        self.event_tx.receiver_count()
    }
}

impl Clone for ActiveRunHandle {
    /// Clone the handle
    ///
    /// Note: The cloned handle shares the same event broadcaster and input sender,
    /// but the cancel_tx is shared via Arc<Mutex<Option<...>>> so it can only
    /// be consumed once across all clones.
    fn clone(&self) -> Self {
        Self {
            run_id: self.run_id.clone(),
            session_key: self.session_key.clone(),
            started_at: self.started_at,
            event_tx: self.event_tx.clone(),
            input_tx: self.input_tx.clone(),
            cancel_tx: self.cancel_tx.clone(),
            // Clone atomics by copying their current values
            seq_counter: AtomicU64::new(self.seq_counter.load(Ordering::SeqCst)),
            chunk_counter: AtomicU32::new(self.chunk_counter.load(Ordering::SeqCst)),
        }
    }
}

// ============================================================================
// Wait for Run End
// ============================================================================

/// Wait for a run to complete, fail, or be cancelled
///
/// This function consumes events from the receiver until a terminal event
/// (RunCompleted, RunFailed, or RunCancelled) is received, or timeout occurs.
///
/// # Arguments
///
/// * `receiver` - Broadcast receiver for run events
/// * `timeout` - Maximum time to wait for run completion
///
/// # Returns
///
/// * `Ok(RunEndResult)` - The run completed with this result
/// * `Err(WaitError)` - Timeout, channel closed, or lagged behind
///
/// # Example
///
/// ```ignore
/// let mut events = handle.subscribe();
/// match wait_for_run_end(&mut events, Duration::from_secs(60)).await {
///     Ok(RunEndResult::Completed { summary, .. }) => {
///         println!("Run completed: {:?}", summary);
///     }
///     Ok(RunEndResult::Failed { error, .. }) => {
///         eprintln!("Run failed: {}", error);
///     }
///     Ok(RunEndResult::Cancelled { reason }) => {
///         println!("Run cancelled: {:?}", reason);
///     }
///     Err(WaitError::Timeout(duration)) => {
///         eprintln!("Timed out after {:?}", duration);
///     }
///     Err(e) => {
///         eprintln!("Error: {}", e);
///     }
/// }
/// ```
pub async fn wait_for_run_end(
    receiver: &mut broadcast::Receiver<RunEvent>,
    timeout: Duration,
) -> Result<RunEndResult, WaitError> {
    let deadline = tokio::time::Instant::now() + timeout;

    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(WaitError::Timeout(timeout));
        }

        let result = tokio::time::timeout(remaining, receiver.recv()).await;

        match result {
            Ok(Ok(event)) => {
                match event {
                    RunEvent::RunCompleted {
                        summary,
                        total_tokens,
                        tool_calls,
                        loops,
                        duration_ms,
                        ..
                    } => {
                        return Ok(RunEndResult::Completed {
                            summary,
                            total_tokens,
                            tool_calls,
                            loops,
                            duration_ms,
                        });
                    }
                    RunEvent::RunFailed { error, error_code, .. } => {
                        return Ok(RunEndResult::Failed { error, error_code });
                    }
                    RunEvent::RunCancelled { reason, .. } => {
                        return Ok(RunEndResult::Cancelled { reason });
                    }
                    // Continue waiting for terminal event
                    _ => continue,
                }
            }
            Ok(Err(broadcast::error::RecvError::Closed)) => {
                return Err(WaitError::ChannelClosed);
            }
            Ok(Err(broadcast::error::RecvError::Lagged(n))) => {
                return Err(WaitError::Lagged(n));
            }
            Err(_) => {
                return Err(WaitError::Timeout(timeout));
            }
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_run_status_display() {
        assert_eq!(RunStatus::Queued.to_string(), "queued");
        assert_eq!(RunStatus::Running.to_string(), "running");
        assert_eq!(RunStatus::Completed.to_string(), "completed");
        assert_eq!(RunStatus::Failed.to_string(), "failed");
        assert_eq!(RunStatus::Cancelled.to_string(), "cancelled");
    }

    #[test]
    fn test_run_event_is_terminal() {
        let completed = RunEvent::RunCompleted {
            run_id: "test".into(),
            seq: 0,
            summary: None,
            total_tokens: 0,
            tool_calls: 0,
            loops: 0,
            duration_ms: 0,
        };
        assert!(completed.is_terminal());

        let failed = RunEvent::RunFailed {
            run_id: "test".into(),
            seq: 0,
            error: "error".into(),
            error_code: None,
        };
        assert!(failed.is_terminal());

        let cancelled = RunEvent::RunCancelled {
            run_id: "test".into(),
            seq: 0,
            reason: None,
        };
        assert!(cancelled.is_terminal());

        let token_delta = RunEvent::TokenDelta {
            run_id: "test".into(),
            seq: 0,
            delta: "hello".into(),
            role: None,
        };
        assert!(!token_delta.is_terminal());
    }

    #[test]
    fn test_run_end_result() {
        let completed = RunEndResult::Completed {
            summary: Some("done".into()),
            total_tokens: 100,
            tool_calls: 2,
            loops: 3,
            duration_ms: 1000,
        };
        assert!(completed.is_success());
        assert!(completed.error().is_none());

        let failed = RunEndResult::Failed {
            error: "something went wrong".into(),
            error_code: Some("ERR001".into()),
        };
        assert!(!failed.is_success());
        assert_eq!(failed.error(), Some("something went wrong"));

        let cancelled = RunEndResult::Cancelled {
            reason: Some("user cancelled".into()),
        };
        assert!(!cancelled.is_success());
        assert!(cancelled.error().is_none());
    }

    #[tokio::test]
    async fn test_active_run_handle_new() {
        let (handle, _input_rx, _cancel_rx) = ActiveRunHandle::new(
            "test-run-1".to_string(),
            SessionKey::main("main"),
        );

        assert_eq!(handle.run_id, "test-run-1");
        assert_eq!(handle.current_seq(), 0);
        assert_eq!(handle.current_chunk(), 0);
    }

    #[tokio::test]
    async fn test_active_run_handle_subscribe_and_emit() {
        let (handle, _input_rx, _cancel_rx) = ActiveRunHandle::new(
            "test-run-2".to_string(),
            SessionKey::main("main"),
        );

        let mut rx1 = handle.subscribe();
        let mut rx2 = handle.subscribe();

        let event = RunEvent::StatusChanged {
            run_id: "test-run-2".to_string(),
            status: RunStatus::Running,
            reason: None,
        };

        let count = handle.emit(event.clone());
        assert_eq!(count, 2);

        let received1 = rx1.recv().await.unwrap();
        let received2 = rx2.recv().await.unwrap();

        assert!(matches!(received1, RunEvent::StatusChanged { status: RunStatus::Running, .. }));
        assert!(matches!(received2, RunEvent::StatusChanged { status: RunStatus::Running, .. }));
    }

    #[tokio::test]
    async fn test_active_run_handle_seq_counter() {
        let (handle, _input_rx, _cancel_rx) = ActiveRunHandle::new(
            "test-run-3".to_string(),
            SessionKey::main("main"),
        );

        assert_eq!(handle.next_seq(), 0);
        assert_eq!(handle.next_seq(), 1);
        assert_eq!(handle.next_seq(), 2);
        assert_eq!(handle.current_seq(), 3);
    }

    #[tokio::test]
    async fn test_active_run_handle_chunk_counter() {
        let (handle, _input_rx, _cancel_rx) = ActiveRunHandle::new(
            "test-run-4".to_string(),
            SessionKey::main("main"),
        );

        assert_eq!(handle.next_chunk(), 0);
        assert_eq!(handle.next_chunk(), 1);
        assert_eq!(handle.current_chunk(), 2);
    }

    #[tokio::test]
    async fn test_active_run_handle_take_cancel_tx() {
        let (handle, _input_rx, cancel_rx) = ActiveRunHandle::new(
            "test-run-5".to_string(),
            SessionKey::main("main"),
        );

        // First take should succeed
        let tx = handle.take_cancel_tx().await;
        assert!(tx.is_some());

        // Second take should return None
        let tx2 = handle.take_cancel_tx().await;
        assert!(tx2.is_none());

        // Send cancellation
        tx.unwrap().send(()).unwrap();

        // Verify receiver got the signal
        assert!(cancel_rx.await.is_ok());
    }

    #[tokio::test]
    async fn test_active_run_handle_clone() {
        let (handle, _input_rx, _cancel_rx) = ActiveRunHandle::new(
            "test-run-6".to_string(),
            SessionKey::main("main"),
        );

        handle.next_seq();
        handle.next_seq();

        let cloned = handle.clone();

        assert_eq!(cloned.run_id, handle.run_id);
        assert_eq!(cloned.current_seq(), handle.current_seq());

        // Both handles share the same event broadcaster
        let mut rx = handle.subscribe();
        cloned.emit(RunEvent::StatusChanged {
            run_id: "test-run-6".to_string(),
            status: RunStatus::Running,
            reason: None,
        });

        let event = rx.recv().await.unwrap();
        assert!(matches!(event, RunEvent::StatusChanged { .. }));
    }

    #[tokio::test]
    async fn test_active_run_handle_input_sender() {
        let (handle, mut input_rx, _cancel_rx) = ActiveRunHandle::new(
            "test-run-7".to_string(),
            SessionKey::main("main"),
        );

        let input_tx = handle.input_sender();
        input_tx.send("user input".to_string()).await.unwrap();

        let received = input_rx.recv().await.unwrap();
        assert_eq!(received, "user input");
    }

    #[tokio::test]
    async fn test_wait_for_run_end_completed() {
        let (handle, _input_rx, _cancel_rx) = ActiveRunHandle::new(
            "test-run-8".to_string(),
            SessionKey::main("main"),
        );

        let mut rx = handle.subscribe();

        // Spawn task to emit completion after a short delay
        let handle_clone = handle.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            handle_clone.emit(RunEvent::RunCompleted {
                run_id: "test-run-8".to_string(),
                seq: 0,
                summary: Some("done".to_string()),
                total_tokens: 100,
                tool_calls: 2,
                loops: 3,
                duration_ms: 1000,
            });
        });

        let result = wait_for_run_end(&mut rx, Duration::from_secs(1)).await;
        assert!(result.is_ok());

        match result.unwrap() {
            RunEndResult::Completed { summary, total_tokens, .. } => {
                assert_eq!(summary, Some("done".to_string()));
                assert_eq!(total_tokens, 100);
            }
            _ => panic!("Expected Completed result"),
        }
    }

    #[tokio::test]
    async fn test_wait_for_run_end_failed() {
        let (handle, _input_rx, _cancel_rx) = ActiveRunHandle::new(
            "test-run-9".to_string(),
            SessionKey::main("main"),
        );

        let mut rx = handle.subscribe();

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            handle.emit(RunEvent::RunFailed {
                run_id: "test-run-9".to_string(),
                seq: 0,
                error: "something went wrong".to_string(),
                error_code: Some("ERR001".to_string()),
            });
        });

        let result = wait_for_run_end(&mut rx, Duration::from_secs(1)).await;
        assert!(result.is_ok());

        match result.unwrap() {
            RunEndResult::Failed { error, error_code } => {
                assert_eq!(error, "something went wrong");
                assert_eq!(error_code, Some("ERR001".to_string()));
            }
            _ => panic!("Expected Failed result"),
        }
    }

    #[tokio::test]
    async fn test_wait_for_run_end_timeout() {
        let (handle, _input_rx, _cancel_rx) = ActiveRunHandle::new(
            "test-run-10".to_string(),
            SessionKey::main("main"),
        );

        let mut rx = handle.subscribe();

        // Don't emit any terminal event
        let result = wait_for_run_end(&mut rx, Duration::from_millis(50)).await;
        assert!(matches!(result, Err(WaitError::Timeout(_))));
    }

    #[tokio::test]
    async fn test_wait_for_run_end_ignores_non_terminal() {
        let (handle, _input_rx, _cancel_rx) = ActiveRunHandle::new(
            "test-run-11".to_string(),
            SessionKey::main("main"),
        );

        let mut rx = handle.subscribe();

        let handle_clone = handle.clone();
        tokio::spawn(async move {
            // Emit non-terminal events first
            handle_clone.emit(RunEvent::TokenDelta {
                run_id: "test-run-11".to_string(),
                seq: 0,
                delta: "hello".to_string(),
                role: None,
            });

            handle_clone.emit(RunEvent::ToolStart {
                run_id: "test-run-11".to_string(),
                seq: 1,
                tool_name: "search".to_string(),
                tool_call_id: "call-1".to_string(),
                input: None,
            });

            tokio::time::sleep(Duration::from_millis(10)).await;

            // Finally emit terminal event
            handle_clone.emit(RunEvent::RunCompleted {
                run_id: "test-run-11".to_string(),
                seq: 2,
                summary: Some("done".to_string()),
                total_tokens: 50,
                tool_calls: 1,
                loops: 1,
                duration_ms: 500,
            });
        });

        let result = wait_for_run_end(&mut rx, Duration::from_secs(1)).await;
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), RunEndResult::Completed { .. }));
    }

    #[test]
    fn test_run_event_serialization() {
        let event = RunEvent::TokenDelta {
            run_id: "test".to_string(),
            seq: 42,
            delta: "hello world".to_string(),
            role: Some("assistant".to_string()),
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("token_delta"));
        assert!(json.contains("hello world"));

        let parsed: RunEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, RunEvent::TokenDelta { seq: 42, .. }));
    }

    #[test]
    fn test_run_status_serialization() {
        let status = RunStatus::Running;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"running\"");

        let parsed: RunStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, RunStatus::Running);
    }
}
