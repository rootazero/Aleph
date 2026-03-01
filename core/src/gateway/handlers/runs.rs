//! Run Wait and Queue Message Handlers
//!
//! RPC handlers for waiting on run completion and queueing messages
//! to active runs (human-in-the-loop support).

use std::collections::HashMap;
use crate::sync_primitives::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::RwLock;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, RESOURCE_NOT_FOUND, TIMEOUT};
use super::parse_params;
use super::super::run_event_bus::{ActiveRunHandle, RunEndResult, wait_for_run_end, WaitError};

// ============================================================================
// run.wait
// ============================================================================

/// Default timeout for run.wait (30 seconds)
const DEFAULT_WAIT_TIMEOUT_MS: u64 = 30_000;

/// Maximum timeout for run.wait (5 minutes)
const MAX_WAIT_TIMEOUT_MS: u64 = 300_000;

/// Request parameters for run.wait
#[derive(Debug, Deserialize)]
pub struct RunWaitRequest {
    /// The run ID to wait for
    pub run_id: String,
    /// Timeout in milliseconds (default: 30000)
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
}

fn default_timeout() -> u64 {
    DEFAULT_WAIT_TIMEOUT_MS
}

/// Response for run.wait
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RunWaitResponse {
    /// Run completed successfully
    Completed {
        #[serde(skip_serializing_if = "Option::is_none")]
        output: Option<String>,
        input_tokens: u64,
        output_tokens: u64,
        duration_ms: u64,
    },
    /// Run failed with an error
    Failed {
        error: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        error_code: Option<String>,
    },
    /// Run was cancelled
    Cancelled {
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// Wait timed out
    Timeout,
    /// Run not found
    NotFound,
}

/// Handle run.wait request
///
/// Waits for a run to complete, fail, or be cancelled.
/// Returns immediately if the run is not found.
///
/// # Parameters
///
/// - `run_id`: The ID of the run to wait for
/// - `timeout_ms`: Maximum time to wait in milliseconds (default: 30000, max: 300000)
///
/// # Returns
///
/// A `RunWaitResponse` indicating the final state of the run.
pub async fn handle_run_wait(
    request: JsonRpcRequest,
    active_runs: Arc<RwLock<HashMap<String, ActiveRunHandle>>>,
) -> JsonRpcResponse {
    // Parse parameters
    let params: RunWaitRequest = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Cap timeout at maximum
    let timeout_ms = params.timeout_ms.min(MAX_WAIT_TIMEOUT_MS);
    let timeout = Duration::from_millis(timeout_ms);

    // Get subscriber from active runs
    let mut subscriber = {
        let runs = active_runs.read().await;
        match runs.get(&params.run_id) {
            Some(handle) => handle.subscribe(),
            None => {
                return JsonRpcResponse::success(
                    request.id,
                    json!(RunWaitResponse::NotFound),
                );
            }
        }
    };

    // Wait for run completion
    match wait_for_run_end(&mut subscriber, timeout).await {
        Ok(result) => {
            let response = match result {
                RunEndResult::Completed {
                    summary,
                    total_tokens,
                    duration_ms,
                    ..
                } => RunWaitResponse::Completed {
                    output: summary,
                    // We don't have input/output token split, use total for output
                    input_tokens: 0,
                    output_tokens: total_tokens,
                    duration_ms,
                },
                RunEndResult::Failed { error, error_code } => RunWaitResponse::Failed {
                    error,
                    error_code,
                },
                RunEndResult::Cancelled { reason } => RunWaitResponse::Cancelled { reason },
            };
            JsonRpcResponse::success(request.id, json!(response))
        }
        Err(WaitError::Timeout(_)) => {
            JsonRpcResponse::success(request.id, json!(RunWaitResponse::Timeout))
        }
        Err(WaitError::ChannelClosed) => {
            // Run ended but we missed the event - treat as not found
            JsonRpcResponse::success(request.id, json!(RunWaitResponse::NotFound))
        }
        Err(WaitError::Lagged(n)) => {
            JsonRpcResponse::error(
                request.id,
                TIMEOUT,
                format!("Receiver lagged behind, missed {} events", n),
            )
        }
    }
}

// ============================================================================
// run.queue_message
// ============================================================================

/// Request parameters for run.queue_message
#[derive(Debug, Deserialize)]
pub struct RunQueueMessageRequest {
    /// The run ID to send the message to
    pub run_id: String,
    /// The message to queue
    pub message: String,
}

/// Response for run.queue_message
#[derive(Debug, Clone, Serialize)]
pub struct RunQueueMessageResponse {
    /// Whether the message was queued successfully
    pub success: bool,
    /// Error message if unsuccessful
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Handle run.queue_message request
///
/// Queues a message to an active run for human-in-the-loop scenarios.
/// The message will be delivered to the agent when it next requests input.
///
/// # Parameters
///
/// - `run_id`: The ID of the run to send the message to
/// - `message`: The message content to queue
///
/// # Returns
///
/// A `RunQueueMessageResponse` indicating success or failure.
pub async fn handle_run_queue_message(
    request: JsonRpcRequest,
    active_runs: Arc<RwLock<HashMap<String, ActiveRunHandle>>>,
) -> JsonRpcResponse {
    // Parse parameters
    let params: RunQueueMessageRequest = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Get input sender from active runs
    let input_sender = {
        let runs = active_runs.read().await;
        match runs.get(&params.run_id) {
            Some(handle) => handle.input_sender(),
            None => {
                return JsonRpcResponse::error(
                    request.id,
                    RESOURCE_NOT_FOUND,
                    format!("Run not found: {}", params.run_id),
                );
            }
        }
    };

    // Send the message
    match input_sender.try_send(params.message) {
        Ok(()) => {
            JsonRpcResponse::success(
                request.id,
                json!(RunQueueMessageResponse {
                    success: true,
                    error: None,
                }),
            )
        }
        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
            JsonRpcResponse::success(
                request.id,
                json!(RunQueueMessageResponse {
                    success: false,
                    error: Some("Input queue is full".to_string()),
                }),
            )
        }
        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
            JsonRpcResponse::success(
                request.id,
                json!(RunQueueMessageResponse {
                    success: false,
                    error: Some("Run has ended".to_string()),
                }),
            )
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::protocol::INVALID_PARAMS;
    use crate::gateway::router::SessionKey;
    use serde_json::json;

    #[test]
    fn test_run_wait_request_parsing() {
        // With explicit timeout
        let json = json!({
            "run_id": "run-123",
            "timeout_ms": 5000
        });
        let params: RunWaitRequest = serde_json::from_value(json).unwrap();
        assert_eq!(params.run_id, "run-123");
        assert_eq!(params.timeout_ms, 5000);

        // With default timeout
        let json = json!({
            "run_id": "run-456"
        });
        let params: RunWaitRequest = serde_json::from_value(json).unwrap();
        assert_eq!(params.run_id, "run-456");
        assert_eq!(params.timeout_ms, DEFAULT_WAIT_TIMEOUT_MS);
    }

    #[test]
    fn test_run_queue_message_request_parsing() {
        let json = json!({
            "run_id": "run-123",
            "message": "user response"
        });
        let params: RunQueueMessageRequest = serde_json::from_value(json).unwrap();
        assert_eq!(params.run_id, "run-123");
        assert_eq!(params.message, "user response");
    }

    #[test]
    fn test_run_wait_response_serialization() {
        let completed = RunWaitResponse::Completed {
            output: Some("done".to_string()),
            input_tokens: 100,
            output_tokens: 200,
            duration_ms: 1500,
        };
        let json = serde_json::to_string(&completed).unwrap();
        assert!(json.contains("\"status\":\"completed\""));
        assert!(json.contains("\"output\":\"done\""));

        let failed = RunWaitResponse::Failed {
            error: "something went wrong".to_string(),
            error_code: Some("ERR001".to_string()),
        };
        let json = serde_json::to_string(&failed).unwrap();
        assert!(json.contains("\"status\":\"failed\""));
        assert!(json.contains("\"error\":\"something went wrong\""));

        let cancelled = RunWaitResponse::Cancelled {
            reason: Some("user cancelled".to_string()),
        };
        let json = serde_json::to_string(&cancelled).unwrap();
        assert!(json.contains("\"status\":\"cancelled\""));

        let timeout = RunWaitResponse::Timeout;
        let json = serde_json::to_string(&timeout).unwrap();
        assert!(json.contains("\"status\":\"timeout\""));

        let not_found = RunWaitResponse::NotFound;
        let json = serde_json::to_string(&not_found).unwrap();
        assert!(json.contains("\"status\":\"not_found\""));
    }

    #[test]
    fn test_run_queue_message_response_serialization() {
        let success = RunQueueMessageResponse {
            success: true,
            error: None,
        };
        let json = serde_json::to_string(&success).unwrap();
        assert!(json.contains("\"success\":true"));
        assert!(!json.contains("error"));

        let failure = RunQueueMessageResponse {
            success: false,
            error: Some("Input queue is full".to_string()),
        };
        let json = serde_json::to_string(&failure).unwrap();
        assert!(json.contains("\"success\":false"));
        assert!(json.contains("\"error\":\"Input queue is full\""));
    }

    #[tokio::test]
    async fn test_handle_run_wait_not_found() {
        let active_runs = Arc::new(RwLock::new(HashMap::new()));
        let request = JsonRpcRequest::new(
            "run.wait",
            Some(json!({ "run_id": "nonexistent" })),
            Some(json!(1)),
        );

        let response = handle_run_wait(request, active_runs).await;
        assert!(response.is_success());

        let result = response.result.unwrap();
        assert_eq!(result["status"], "not_found");
    }

    #[tokio::test]
    async fn test_handle_run_wait_missing_params() {
        let active_runs = Arc::new(RwLock::new(HashMap::new()));
        let request = JsonRpcRequest::with_id("run.wait", None, json!(1));

        let response = handle_run_wait(request, active_runs).await;
        assert!(response.is_error());
        assert_eq!(response.error.unwrap().code, INVALID_PARAMS);
    }

    #[tokio::test]
    async fn test_handle_run_queue_message_not_found() {
        let active_runs = Arc::new(RwLock::new(HashMap::new()));
        let request = JsonRpcRequest::new(
            "run.queue_message",
            Some(json!({ "run_id": "nonexistent", "message": "hello" })),
            Some(json!(1)),
        );

        let response = handle_run_queue_message(request, active_runs).await;
        assert!(response.is_error());
        assert_eq!(response.error.unwrap().code, RESOURCE_NOT_FOUND);
    }

    #[tokio::test]
    async fn test_handle_run_queue_message_success() {
        let active_runs = Arc::new(RwLock::new(HashMap::new()));

        // Create an active run
        let (handle, mut input_rx, _cancel_rx) = ActiveRunHandle::new(
            "run-123".to_string(),
            SessionKey::main("main"),
        );

        {
            let mut runs = active_runs.write().await;
            runs.insert("run-123".to_string(), handle);
        }

        let request = JsonRpcRequest::new(
            "run.queue_message",
            Some(json!({ "run_id": "run-123", "message": "user input" })),
            Some(json!(1)),
        );

        let response = handle_run_queue_message(request, active_runs).await;
        assert!(response.is_success());

        let result = response.result.unwrap();
        assert_eq!(result["success"], true);

        // Verify message was received
        let received = input_rx.recv().await.unwrap();
        assert_eq!(received, "user input");
    }

    #[tokio::test]
    async fn test_handle_run_wait_completed() {
        use crate::gateway::run_event_bus::RunEvent;

        let active_runs = Arc::new(RwLock::new(HashMap::new()));

        // Create an active run
        let (handle, _input_rx, _cancel_rx) = ActiveRunHandle::new(
            "run-456".to_string(),
            SessionKey::main("main"),
        );

        let handle_clone = handle.clone();
        {
            let mut runs = active_runs.write().await;
            runs.insert("run-456".to_string(), handle);
        }

        // Spawn task to emit completion after a short delay
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            handle_clone.emit(RunEvent::RunCompleted {
                run_id: "run-456".to_string(),
                seq: 0,
                summary: Some("Task completed successfully".to_string()),
                total_tokens: 150,
                tool_calls: 2,
                loops: 1,
                duration_ms: 500,
            });
        });

        let request = JsonRpcRequest::new(
            "run.wait",
            Some(json!({ "run_id": "run-456", "timeout_ms": 1000 })),
            Some(json!(1)),
        );

        let response = handle_run_wait(request, active_runs).await;
        assert!(response.is_success());

        let result = response.result.unwrap();
        assert_eq!(result["status"], "completed");
        assert_eq!(result["output"], "Task completed successfully");
        assert_eq!(result["output_tokens"], 150);
        assert_eq!(result["duration_ms"], 500);
    }
}
