//! POE (Principle-Operation-Evaluation) RPC Handlers
//!
//! RPC handlers for POE task execution and contract signing workflow.
//!
//! ## Contract Signing Workflow
//!
//! | Method | Description |
//! |--------|-------------|
//! | `poe.prepare` | Generate a contract from instruction, await signature |
//! | `poe.sign` | Sign a contract and start execution |
//! | `poe.reject` | Reject a pending contract |
//! | `poe.pending` | List all pending contracts |
//!
//! ## Direct Execution (Legacy)
//!
//! | Method | Description |
//! |--------|-------------|
//! | `poe.run` | Execute with pre-built manifest (no signing) |
//! | `poe.status` | Query task status |
//! | `poe.cancel` | Cancel a running task |
//! | `poe.list` | List all active tasks |
//!
//! ## Events Emitted
//!
//! | Event | Description |
//! |-------|-------------|
//! | `poe.contract_generated` | Contract generated, awaiting signature |
//! | `poe.signed` | Contract signed, execution starting |
//! | `poe.rejected` | Contract rejected by user |
//! | `poe.accepted` | Task accepted and queued for execution |
//! | `poe.step` | Each P->O->E iteration |
//! | `poe.validation` | Validation result after each attempt |
//! | `poe.completed` | Final outcome (success/failure) |
//! | `poe.error` | Execution error |

use serde_json::{json, Value};
use crate::sync_primitives::Arc;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::poe::{
    // Core types
    Worker,
    // Contract signing types
    SignRequest,
    // Service layer
    PoeRunManager, PoeContractService, PrepareParams, RejectParams,
};
use crate::poe::handler_types::PoeRunParams;

// Re-export types that are used by other modules
pub use crate::poe::handler_types::{
    PoeTaskState, PoeTaskStatus,
    ValidatorFactory, WorkerFactory,
};

// ============================================================================
// RPC Parameter/Result Types (moved to core/src/poe/handler_types/)
// ============================================================================

// ============================================================================
// RPC Handlers
// ============================================================================

/// Handle poe.run RPC request
pub async fn handle_run<W: Worker + 'static>(
    request: JsonRpcRequest,
    manager: Arc<PoeRunManager<W>>,
) -> JsonRpcResponse {
    // Parse params
    let params: PoeRunParams = match &request.params {
        Some(Value::Object(map)) => {
            match serde_json::from_value(Value::Object(map.clone())) {
                Ok(p) => p,
                Err(e) => {
                    return JsonRpcResponse::error(
                        request.id,
                        INVALID_PARAMS,
                        format!("Invalid params: {}", e),
                    );
                }
            }
        }
        _ => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing or invalid params object",
            );
        }
    };

    // Validate manifest
    if params.manifest.task_id.is_empty() {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "manifest.task_id is required");
    }

    if params.manifest.objective.is_empty() {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            "manifest.objective is required",
        );
    }

    if params.instruction.trim().is_empty() {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "instruction cannot be empty");
    }

    // Start the run
    match manager.start_run(params).await {
        Ok(result) => JsonRpcResponse::success(request.id, json!(result)),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e),
    }
}

/// Handle poe.status RPC request
pub async fn handle_status<W: Worker + 'static>(
    request: JsonRpcRequest,
    manager: Arc<PoeRunManager<W>>,
) -> JsonRpcResponse {
    // Parse task_id from params
    let task_id = match &request.params {
        Some(Value::Object(map)) => map
            .get("task_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        _ => None,
    };

    match task_id {
        Some(id) => match manager.get_status(&id).await {
            Some(status) => JsonRpcResponse::success(request.id, json!(status)),
            None => {
                JsonRpcResponse::error(request.id, INVALID_PARAMS, format!("Task {} not found", id))
            }
        },
        None => {
            JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing task_id parameter")
        }
    }
}

/// Handle poe.cancel RPC request
pub async fn handle_cancel<W: Worker + 'static>(
    request: JsonRpcRequest,
    manager: Arc<PoeRunManager<W>>,
) -> JsonRpcResponse {
    // Parse task_id from params
    let task_id = match &request.params {
        Some(Value::Object(map)) => map
            .get("task_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        _ => None,
    };

    match task_id {
        Some(id) => {
            let result = manager.cancel(&id).await;
            JsonRpcResponse::success(request.id, json!(result))
        }
        None => {
            JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing task_id parameter")
        }
    }
}

/// Handle poe.list RPC request - list all active POE tasks
pub async fn handle_list<W: Worker + 'static>(
    request: JsonRpcRequest,
    manager: Arc<PoeRunManager<W>>,
) -> JsonRpcResponse {
    let tasks = manager.list_tasks().await;

    let task_summaries: Vec<Value> = tasks
        .iter()
        .map(|t| {
            json!({
                "task_id": t.task_id,
                "session_key": t.session_key,
                "status": t.status.status_str(),
                "elapsed_ms": t.started_at.elapsed().as_millis() as u64,
            })
        })
        .collect();

    JsonRpcResponse::success(
        request.id,
        json!({
            "tasks": task_summaries,
            "count": task_summaries.len(),
        }),
    )
}

// ============================================================================
// Contract Signing RPC Handlers
// ============================================================================

/// Handle poe.prepare RPC request
pub async fn handle_prepare<W: Worker + 'static>(
    request: JsonRpcRequest,
    service: Arc<PoeContractService<W>>,
) -> JsonRpcResponse {
    // Parse params
    let params: PrepareParams = match &request.params {
        Some(Value::Object(map)) => {
            match serde_json::from_value(Value::Object(map.clone())) {
                Ok(p) => p,
                Err(e) => {
                    return JsonRpcResponse::error(
                        request.id,
                        INVALID_PARAMS,
                        format!("Invalid params: {}", e),
                    );
                }
            }
        }
        _ => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing or invalid params object",
            );
        }
    };

    // Validate instruction
    if params.instruction.trim().is_empty() {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "instruction cannot be empty");
    }

    // Prepare the contract
    match service.prepare(params).await {
        Ok(result) => JsonRpcResponse::success(request.id, json!(result)),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e.to_string()),
    }
}

/// Handle poe.sign RPC request
pub async fn handle_sign<W: Worker + 'static>(
    request: JsonRpcRequest,
    service: Arc<PoeContractService<W>>,
) -> JsonRpcResponse {
    // Parse params
    let params: SignRequest = match &request.params {
        Some(Value::Object(map)) => {
            match serde_json::from_value(Value::Object(map.clone())) {
                Ok(p) => p,
                Err(e) => {
                    return JsonRpcResponse::error(
                        request.id,
                        INVALID_PARAMS,
                        format!("Invalid params: {}", e),
                    );
                }
            }
        }
        _ => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing or invalid params object",
            );
        }
    };

    // Validate contract_id
    if params.contract_id.trim().is_empty() {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "contract_id cannot be empty");
    }

    // Sign the contract
    match service.sign(params).await {
        Ok(result) => JsonRpcResponse::success(request.id, json!(result)),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e.to_string()),
    }
}

/// Handle poe.reject RPC request
pub async fn handle_reject<W: Worker + 'static>(
    request: JsonRpcRequest,
    service: Arc<PoeContractService<W>>,
) -> JsonRpcResponse {
    // Parse params
    let params: RejectParams = match &request.params {
        Some(Value::Object(map)) => {
            match serde_json::from_value(Value::Object(map.clone())) {
                Ok(p) => p,
                Err(e) => {
                    return JsonRpcResponse::error(
                        request.id,
                        INVALID_PARAMS,
                        format!("Invalid params: {}", e),
                    );
                }
            }
        }
        _ => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing or invalid params object",
            );
        }
    };

    // Validate contract_id
    if params.contract_id.trim().is_empty() {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "contract_id cannot be empty");
    }

    // Reject the contract
    let result = service.reject(params).await;
    JsonRpcResponse::success(request.id, json!(result))
}

/// Handle poe.pending RPC request
pub async fn handle_pending<W: Worker + 'static>(
    request: JsonRpcRequest,
    service: Arc<PoeContractService<W>>,
) -> JsonRpcResponse {
    let result = service.pending().await;
    JsonRpcResponse::success(request.id, json!(result))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::event_bus::GatewayEventBus;
    use crate::poe::{
        worker::MockWorker,
        ValidationRule, SuccessManifest, PoeOutcome,
        CompositeValidator, PoeConfig,
        WorkerFactory, ValidatorFactory,
    };
    use crate::poe::handler_types::{
        PoeRunParams, PoeConfigParams, PoeTaskStatus,
    };
    use crate::providers::MockProvider;
    use std::path::PathBuf;

    fn create_test_manager() -> PoeRunManager<MockWorker> {
        let event_bus = Arc::new(GatewayEventBus::new());

        // Worker factory - creates a new MockWorker for each run
        let worker_factory: WorkerFactory<MockWorker> = Arc::new(MockWorker::new);

        // Validator factory - creates a new CompositeValidator for each run
        let validator_factory: ValidatorFactory = Arc::new(|| {
            let provider = Arc::new(MockProvider::new(""));
            CompositeValidator::new(provider)
        });

        let config = PoeConfig::default();

        PoeRunManager::new(event_bus, worker_factory, validator_factory, config)
    }

    fn create_test_manifest() -> SuccessManifest {
        SuccessManifest::new("test-task-1", "Complete test objective")
    }

    #[tokio::test]
    async fn test_poe_run_manager_start_run() {
        let manager = Arc::new(create_test_manager());

        let params = PoeRunParams {
            manifest: create_test_manifest(),
            instruction: "Execute test instruction".to_string(),
            stream: false,
            config: None,
        };

        let result = manager.start_run(params).await.unwrap();

        assert_eq!(result.task_id, "test-task-1");
        assert!(result.session_key.contains("poe:test-task-1"));
    }

    #[tokio::test]
    async fn test_poe_run_manager_duplicate_task() {
        let manager = Arc::new(create_test_manager());

        let params = PoeRunParams {
            manifest: create_test_manifest(),
            instruction: "First run".to_string(),
            stream: false,
            config: None,
        };

        // First run should succeed
        let result1 = manager.start_run(params.clone()).await;
        assert!(result1.is_ok());

        // Second run with same task_id should fail
        let result2 = manager.start_run(params).await;
        assert!(result2.is_err());
        assert!(result2.unwrap_err().contains("already running"));
    }

    #[tokio::test]
    async fn test_poe_run_manager_get_status() {
        let manager = Arc::new(create_test_manager());

        let params = PoeRunParams {
            manifest: create_test_manifest(),
            instruction: "Execute test".to_string(),
            stream: false,
            config: None,
        };

        manager.start_run(params).await.unwrap();

        // Wait a bit for task to start
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Should be able to get status
        let status = manager.get_status("test-task-1").await;
        assert!(status.is_some());
        assert_eq!(status.unwrap().task_id, "test-task-1");
    }

    #[tokio::test]
    async fn test_poe_run_manager_cancel() {
        let manager = Arc::new(create_test_manager());

        let params = PoeRunParams {
            manifest: SuccessManifest::new("cancel-test", "Test cancellation")
                .with_hard_constraint(ValidationRule::FileExists {
                    path: PathBuf::from("/nonexistent/file.txt"),
                })
                .with_max_attempts(10),
            instruction: "Long running task".to_string(),
            stream: false,
            config: None,
        };

        manager.start_run(params).await.unwrap();

        // Cancel the task
        let result = manager.cancel("cancel-test").await;
        assert!(result.cancelled);
    }

    #[tokio::test]
    async fn test_poe_run_manager_cancel_nonexistent() {
        let manager = Arc::new(create_test_manager());

        let result = manager.cancel("nonexistent-task").await;
        assert!(!result.cancelled);
        assert!(result.reason.is_some());
    }

    #[tokio::test]
    async fn test_handle_run_invalid_params() {
        let manager = Arc::new(create_test_manager());

        // Missing params
        let request = JsonRpcRequest::with_id("poe.run", None, json!(1));
        let response = handle_run(request, manager.clone()).await;
        assert!(response.is_error());

        // Empty task_id
        let request = JsonRpcRequest::new(
            "poe.run",
            Some(json!({
                "manifest": {
                    "task_id": "",
                    "objective": "Test"
                },
                "instruction": "Test"
            })),
            Some(json!(2)),
        );
        let response = handle_run(request, manager.clone()).await;
        assert!(response.is_error());
        assert!(response.error.unwrap().message.contains("task_id"));

        // Empty instruction
        let request = JsonRpcRequest::new(
            "poe.run",
            Some(json!({
                "manifest": {
                    "task_id": "test",
                    "objective": "Test"
                },
                "instruction": ""
            })),
            Some(json!(3)),
        );
        let response = handle_run(request, manager).await;
        assert!(response.is_error());
        assert!(response.error.unwrap().message.contains("instruction"));
    }

    #[tokio::test]
    async fn test_handle_status() {
        let manager = Arc::new(create_test_manager());

        // Start a task first
        let params = PoeRunParams {
            manifest: SuccessManifest::new("status-test", "Test status"),
            instruction: "Execute".to_string(),
            stream: false,
            config: None,
        };
        manager.start_run(params).await.unwrap();

        // Get status
        let request = JsonRpcRequest::new(
            "poe.status",
            Some(json!({ "task_id": "status-test" })),
            Some(json!(1)),
        );
        let response = handle_status(request, manager.clone()).await;
        assert!(response.is_success());

        // Nonexistent task
        let request = JsonRpcRequest::new(
            "poe.status",
            Some(json!({ "task_id": "nonexistent" })),
            Some(json!(2)),
        );
        let response = handle_status(request, manager).await;
        assert!(response.is_error());
    }

    #[tokio::test]
    async fn test_handle_cancel() {
        let manager = Arc::new(create_test_manager());

        // Start a task
        let params = PoeRunParams {
            manifest: SuccessManifest::new("cancel-handler-test", "Test cancel handler"),
            instruction: "Execute".to_string(),
            stream: false,
            config: None,
        };
        manager.start_run(params).await.unwrap();

        // Cancel via handler
        let request = JsonRpcRequest::new(
            "poe.cancel",
            Some(json!({ "task_id": "cancel-handler-test" })),
            Some(json!(1)),
        );
        let response = handle_cancel(request, manager).await;
        assert!(response.is_success());
    }

    #[tokio::test]
    async fn test_handle_list() {
        let manager = Arc::new(create_test_manager());

        // Start multiple tasks
        for i in 1..=3 {
            let params = PoeRunParams {
                manifest: SuccessManifest::new(format!("list-test-{}", i), "Test"),
                instruction: "Execute".to_string(),
                stream: false,
                config: None,
            };
            manager.start_run(params).await.unwrap();
        }

        let request = JsonRpcRequest::with_id("poe.list", None, json!(1));
        let response = handle_list(request, manager).await;
        assert!(response.is_success());

        let result = response.result.unwrap();
        assert_eq!(result["count"], 3);
    }

    #[test]
    fn test_poe_task_status_str() {
        assert_eq!(
            PoeTaskStatus::Running {
                current_attempt: 1,
                last_distance_score: None
            }
            .status_str(),
            "running"
        );

        assert_eq!(
            PoeTaskStatus::Completed(PoeOutcome::success(crate::poe::Verdict::success("ok"), ""))
                .status_str(),
            "success"
        );

        assert_eq!(PoeTaskStatus::Cancelled.status_str(), "cancelled");
    }

    #[test]
    fn test_poe_config_params_default() {
        let params = PoeConfigParams::default();
        assert!(params.stuck_window.is_none());
        assert!(params.max_tokens.is_none());
    }
}
