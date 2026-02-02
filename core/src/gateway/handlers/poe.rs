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

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use super::super::event_bus::GatewayEventBus;
use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::error::AetherError;
use crate::poe::{
    // Core types
    CompositeValidator, PoeConfig, PoeManager, PoeOutcome, PoeTask, SuccessManifest,
    ValidationCallback, ValidationEvent, Worker,
    // Contract signing types
    ContractContext, ContractSummary, ManifestBuilder, PendingContract, PendingContractStore,
    PendingResult, PrepareResult, RejectResult, SignRequest, SignResult,
};

// ============================================================================
// RPC Parameter/Result Types
// ============================================================================

/// Parameters for poe.run request
#[derive(Debug, Clone, Deserialize)]
pub struct PoeRunParams {
    /// Success manifest defining success criteria
    pub manifest: SuccessManifest,
    /// Natural language instruction for the worker
    pub instruction: String,
    /// Whether to stream events during execution (default: true)
    #[serde(default = "default_stream")]
    pub stream: bool,
    /// POE configuration overrides
    #[serde(default)]
    pub config: Option<PoeConfigParams>,
}

/// Optional POE configuration overrides
#[derive(Debug, Clone, Deserialize, Default)]
pub struct PoeConfigParams {
    /// Stuck detection window (number of attempts)
    #[serde(default)]
    pub stuck_window: Option<usize>,
    /// Maximum tokens budget
    #[serde(default)]
    pub max_tokens: Option<u32>,
}

fn default_stream() -> bool {
    true
}

/// Result of poe.run request (immediate response)
#[derive(Debug, Clone, Serialize)]
pub struct PoeRunResult {
    /// Unique task identifier (from manifest)
    pub task_id: String,
    /// Session key for event subscription
    pub session_key: String,
    /// Timestamp when task was accepted
    pub accepted_at: String,
}

/// Parameters for poe.status request
#[derive(Debug, Clone, Deserialize)]
pub struct PoeStatusParams {
    /// Task ID to query
    pub task_id: String,
}

/// Result of poe.status request
#[derive(Debug, Clone, Serialize)]
pub struct PoeStatusResult {
    /// Task ID
    pub task_id: String,
    /// Current status
    pub status: String,
    /// Elapsed time in milliseconds
    pub elapsed_ms: u64,
    /// Current attempt number (if running)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_attempt: Option<u8>,
    /// Last distance score (if available)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_distance_score: Option<f32>,
    /// Final outcome (if completed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<PoeOutcome>,
}

/// Parameters for poe.cancel request
#[derive(Debug, Clone, Deserialize)]
pub struct PoeCancelParams {
    /// Task ID to cancel
    pub task_id: String,
}

/// Result of poe.cancel request
#[derive(Debug, Clone, Serialize)]
pub struct PoeCancelResult {
    /// Task ID
    pub task_id: String,
    /// Whether the task was successfully cancelled
    pub cancelled: bool,
    /// Reason if cancellation failed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

// ============================================================================
// Task State
// ============================================================================

/// Status of a POE task
#[derive(Debug, Clone)]
pub enum PoeTaskStatus {
    /// Task is queued/running
    Running {
        current_attempt: u8,
        last_distance_score: Option<f32>,
    },
    /// Task completed with outcome
    Completed(PoeOutcome),
    /// Task was cancelled
    Cancelled,
}

impl PoeTaskStatus {
    /// Get status string for serialization
    pub fn status_str(&self) -> &'static str {
        match self {
            PoeTaskStatus::Running { .. } => "running",
            PoeTaskStatus::Completed(outcome) => match outcome {
                PoeOutcome::Success(_) => "success",
                PoeOutcome::StrategySwitch { .. } => "strategy_switch",
                PoeOutcome::BudgetExhausted { .. } => "budget_exhausted",
            },
            PoeTaskStatus::Cancelled => "cancelled",
        }
    }
}

/// State of a POE task
#[derive(Debug, Clone)]
pub struct PoeTaskState {
    /// Task ID
    pub task_id: String,
    /// Session key for events
    pub session_key: String,
    /// When the task started
    pub started_at: Instant,
    /// Current status
    pub status: PoeTaskStatus,
    /// Whether streaming is enabled
    pub stream: bool,
}

// ============================================================================
// POE Event Types
// ============================================================================

/// Event emitted when a POE task is accepted
#[derive(Debug, Clone, Serialize)]
struct PoeAcceptedEvent {
    pub task_id: String,
    pub session_key: String,
    pub accepted_at: String,
    pub objective: String,
}

/// Event emitted for each POE step (P->O->E iteration)
#[derive(Debug, Clone, Serialize)]
struct PoeStepEvent {
    pub task_id: String,
    pub attempt: u8,
    pub phase: String, // "principle", "operation", "evaluation"
    pub message: String,
}

/// Event emitted after validation
#[derive(Debug, Clone, Serialize)]
struct PoeValidationEvent {
    pub task_id: String,
    pub attempt: u8,
    pub passed: bool,
    pub distance_score: f32,
    pub reason: String,
}

/// Event emitted when POE task completes
#[derive(Debug, Clone, Serialize)]
struct PoeCompletedEvent {
    pub task_id: String,
    pub outcome: PoeOutcome,
    pub duration_ms: u64,
}

/// Event emitted on error
#[derive(Debug, Clone, Serialize)]
struct PoeErrorEvent {
    pub task_id: String,
    pub error: String,
}

// ============================================================================
// PoeRunManager
// ============================================================================

/// Factory function type for creating workers
pub type WorkerFactory<W> = Arc<dyn Fn() -> W + Send + Sync>;

/// Factory function type for creating validators
pub type ValidatorFactory = Arc<dyn Fn() -> CompositeValidator + Send + Sync>;

/// Manager for POE task runs
///
/// Coordinates POE task execution with the Gateway event bus.
/// Maintains active task state and provides status/cancel operations.
///
/// Since PoeManager takes ownership of Worker and Validator,
/// the manager uses factory functions to create fresh instances for each run.
pub struct PoeRunManager<W: Worker + 'static> {
    /// Event bus for publishing events
    event_bus: Arc<GatewayEventBus>,
    /// Active task states
    active_tasks: Arc<RwLock<HashMap<String, PoeTaskState>>>,
    /// Abort signals for cancellation (task_id -> should_abort)
    abort_signals: Arc<RwLock<HashMap<String, Arc<tokio::sync::watch::Sender<bool>>>>>,
    /// Factory for creating worker instances
    worker_factory: WorkerFactory<W>,
    /// Factory for creating validator instances
    validator_factory: ValidatorFactory,
    /// Default POE configuration
    default_config: PoeConfig,
}

impl<W: Worker + 'static> PoeRunManager<W> {
    /// Create a new PoeRunManager with factory functions
    ///
    /// Factory functions are used because PoeManager takes ownership of
    /// Worker and CompositeValidator, so we need fresh instances for each run.
    ///
    /// # Arguments
    ///
    /// * `event_bus` - Gateway event bus for publishing POE events
    /// * `worker_factory` - Factory function that creates Worker instances
    /// * `validator_factory` - Factory function that creates CompositeValidator instances
    /// * `default_config` - Default POE configuration (can be overridden per-run)
    pub fn new(
        event_bus: Arc<GatewayEventBus>,
        worker_factory: WorkerFactory<W>,
        validator_factory: ValidatorFactory,
        default_config: PoeConfig,
    ) -> Self {
        Self {
            event_bus,
            active_tasks: Arc::new(RwLock::new(HashMap::new())),
            abort_signals: Arc::new(RwLock::new(HashMap::new())),
            worker_factory,
            validator_factory,
            default_config,
        }
    }

    /// Start a new POE run
    pub async fn start_run(&self, params: PoeRunParams) -> Result<PoeRunResult, String> {
        let task_id = params.manifest.task_id.clone();
        let objective = params.manifest.objective.clone();
        let session_key = format!("agent:main:poe:{}", task_id);
        let accepted_at = Utc::now().to_rfc3339();

        // Check for duplicate task
        {
            let tasks = self.active_tasks.read().await;
            if tasks.contains_key(&task_id) {
                return Err(format!("Task {} is already running", task_id));
            }
        }

        // Create abort signal
        let (abort_tx, abort_rx) = tokio::sync::watch::channel(false);

        // Store task state
        let state = PoeTaskState {
            task_id: task_id.clone(),
            session_key: session_key.clone(),
            started_at: Instant::now(),
            status: PoeTaskStatus::Running {
                current_attempt: 0,
                last_distance_score: None,
            },
            stream: params.stream,
        };

        {
            let mut tasks = self.active_tasks.write().await;
            tasks.insert(task_id.clone(), state);
        }

        {
            let mut signals = self.abort_signals.write().await;
            signals.insert(task_id.clone(), Arc::new(abort_tx));
        }

        info!(task_id = %task_id, "Started POE task");

        // Emit accepted event
        if params.stream {
            self.emit_event(
                "poe.accepted",
                &PoeAcceptedEvent {
                    task_id: task_id.clone(),
                    session_key: session_key.clone(),
                    accepted_at: accepted_at.clone(),
                    objective,
                },
            );
        }

        // Build POE config with overrides
        let config = self.build_config(params.config);

        // Create POE task
        let task = PoeTask::new(params.manifest, params.instruction);

        // Create worker and validator instances using factories
        let worker = (self.worker_factory)();
        let validator = (self.validator_factory)();

        // Spawn execution
        let event_bus = self.event_bus.clone();
        let active_tasks = self.active_tasks.clone();
        let abort_signals = self.abort_signals.clone();
        let task_id_clone = task_id.clone();
        let stream = params.stream;

        tokio::spawn(async move {
            let result = execute_poe_task(
                task_id_clone.clone(),
                task,
                worker,
                validator,
                config,
                event_bus.clone(),
                active_tasks.clone(),
                abort_rx,
                stream,
            )
            .await;

            // Handle result
            match result {
                Ok(outcome) => {
                    debug!(task_id = %task_id_clone, "POE task completed: {:?}", outcome);
                }
                Err(e) => {
                    error!(task_id = %task_id_clone, error = %e, "POE task failed");
                }
            }

            // Clean up abort signal
            let mut signals = abort_signals.write().await;
            signals.remove(&task_id_clone);
        });

        Ok(PoeRunResult {
            task_id,
            session_key,
            accepted_at,
        })
    }

    /// Get status of a task
    pub async fn get_status(&self, task_id: &str) -> Option<PoeStatusResult> {
        let tasks = self.active_tasks.read().await;
        tasks.get(task_id).map(|state| {
            let (current_attempt, last_distance_score, outcome) = match &state.status {
                PoeTaskStatus::Running {
                    current_attempt,
                    last_distance_score,
                } => (Some(*current_attempt), *last_distance_score, None),
                PoeTaskStatus::Completed(o) => (None, None, Some(o.clone())),
                PoeTaskStatus::Cancelled => (None, None, None),
            };

            PoeStatusResult {
                task_id: state.task_id.clone(),
                status: state.status.status_str().to_string(),
                elapsed_ms: state.started_at.elapsed().as_millis() as u64,
                current_attempt,
                last_distance_score,
                outcome,
            }
        })
    }

    /// Cancel a running task
    pub async fn cancel(&self, task_id: &str) -> PoeCancelResult {
        // Check if task exists
        let tasks = self.active_tasks.read().await;
        let task_state = tasks.get(task_id);

        match task_state {
            Some(state) => {
                match &state.status {
                    PoeTaskStatus::Running { .. } => {
                        // Send abort signal
                        let signals = self.abort_signals.read().await;
                        if let Some(tx) = signals.get(task_id) {
                            let _ = tx.send(true);
                            PoeCancelResult {
                                task_id: task_id.to_string(),
                                cancelled: true,
                                reason: None,
                            }
                        } else {
                            PoeCancelResult {
                                task_id: task_id.to_string(),
                                cancelled: false,
                                reason: Some("Abort signal not found".to_string()),
                            }
                        }
                    }
                    _ => PoeCancelResult {
                        task_id: task_id.to_string(),
                        cancelled: false,
                        reason: Some("Task is not running".to_string()),
                    },
                }
            }
            None => PoeCancelResult {
                task_id: task_id.to_string(),
                cancelled: false,
                reason: Some("Task not found".to_string()),
            },
        }
    }

    /// List all active tasks
    pub async fn list_tasks(&self) -> Vec<PoeTaskState> {
        self.active_tasks.read().await.values().cloned().collect()
    }

    /// Build POE config with optional overrides
    fn build_config(&self, overrides: Option<PoeConfigParams>) -> PoeConfig {
        let mut config = self.default_config.clone();

        if let Some(params) = overrides {
            if let Some(stuck_window) = params.stuck_window {
                config = config.with_stuck_window(stuck_window);
            }
            if let Some(max_tokens) = params.max_tokens {
                config = config.with_max_tokens(max_tokens);
            }
        }

        config
    }

    /// Emit an event to the event bus
    fn emit_event<T: Serialize>(&self, topic: &str, data: &T) {
        if let Ok(data_value) = serde_json::to_value(data) {
            let notification = JsonRpcRequest::notification(
                topic,
                Some(json!({
                    "topic": topic,
                    "data": data_value,
                    "timestamp": Utc::now().timestamp_millis()
                })),
            );
            if let Ok(json) = serde_json::to_string(&notification) {
                self.event_bus.publish(json);
            }
        }
    }
}

// ============================================================================
// POE Task Execution
// ============================================================================

/// Execute a POE task with event emission
async fn execute_poe_task<W: Worker + 'static>(
    task_id: String,
    task: PoeTask,
    worker: W,
    validator: CompositeValidator,
    config: PoeConfig,
    event_bus: Arc<GatewayEventBus>,
    active_tasks: Arc<RwLock<HashMap<String, PoeTaskState>>>,
    mut abort_rx: tokio::sync::watch::Receiver<bool>,
    stream: bool,
) -> Result<PoeOutcome, String> {
    let start_time = Instant::now();

    // Helper to emit events
    let emit = |topic: &str, data: Value| {
        if stream {
            let notification = JsonRpcRequest::notification(
                topic,
                Some(json!({
                    "topic": topic,
                    "data": data,
                    "timestamp": Utc::now().timestamp_millis()
                })),
            );
            if let Ok(json) = serde_json::to_string(&notification) {
                event_bus.publish(json);
            }
        }
    };

    // Emit step event: starting principle phase
    emit(
        "poe.step",
        json!(PoeStepEvent {
            task_id: task_id.clone(),
            attempt: 1,
            phase: "principle".to_string(),
            message: format!("Defining success criteria: {}", task.manifest.objective),
        }),
    );

    // Create validation callback for emitting poe.validation events
    let validation_callback: ValidationCallback = {
        let event_bus = event_bus.clone();
        let task_id = task_id.clone();
        Arc::new(move |event: ValidationEvent| {
            if stream {
                let validation_event = PoeValidationEvent {
                    task_id: task_id.clone(),
                    attempt: event.attempt,
                    passed: event.passed,
                    distance_score: event.distance_score,
                    reason: event.reason,
                };
                let notification = JsonRpcRequest::notification(
                    "poe.validation",
                    Some(json!({
                        "topic": "poe.validation",
                        "data": validation_event,
                        "timestamp": Utc::now().timestamp_millis()
                    })),
                );
                if let Ok(json) = serde_json::to_string(&notification) {
                    event_bus.publish(json);
                }
            }
        })
    };

    // Create manager with validation callback and execute
    let manager = PoeManager::new(worker, validator, config)
        .with_validation_callback(validation_callback);

    // Execute with abort check
    let outcome = tokio::select! {
        result = manager.execute(task) => {
            match result {
                Ok(outcome) => outcome,
                Err(e) => {
                    let error_msg = e.to_string();
                    emit("poe.error", json!(PoeErrorEvent {
                        task_id: task_id.clone(),
                        error: error_msg.clone(),
                    }));

                    // Update task state
                    let mut tasks = active_tasks.write().await;
                    if let Some(state) = tasks.get_mut(&task_id) {
                        state.status = PoeTaskStatus::Completed(
                            PoeOutcome::BudgetExhausted {
                                attempts: 0,
                                last_error: error_msg.clone(),
                            }
                        );
                    }

                    return Err(error_msg);
                }
            }
        }
        _ = abort_rx.changed() => {
            if *abort_rx.borrow() {
                warn!(task_id = %task_id, "POE task cancelled");

                // Update task state
                let mut tasks = active_tasks.write().await;
                if let Some(state) = tasks.get_mut(&task_id) {
                    state.status = PoeTaskStatus::Cancelled;
                }

                return Err("Task cancelled".to_string());
            }
            // Spurious wakeup, continue
            return Err("Unexpected abort signal state".to_string());
        }
    };

    let duration_ms = start_time.elapsed().as_millis() as u64;

    // Emit completion event
    emit(
        "poe.completed",
        json!(PoeCompletedEvent {
            task_id: task_id.clone(),
            outcome: outcome.clone(),
            duration_ms,
        }),
    );

    // Update task state
    {
        let mut tasks = active_tasks.write().await;
        if let Some(state) = tasks.get_mut(&task_id) {
            state.status = PoeTaskStatus::Completed(outcome.clone());
        }
    }

    info!(
        task_id = %task_id,
        duration_ms = duration_ms,
        success = outcome.is_success(),
        "POE task completed"
    );

    Ok(outcome)
}

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
// Contract Signing Workflow
// ============================================================================

/// Parameters for poe.prepare request
#[derive(Debug, Clone, Deserialize)]
pub struct PrepareParams {
    /// Natural language instruction
    pub instruction: String,
    /// Optional context for manifest generation
    #[serde(default)]
    pub context: Option<PrepareContext>,
}

/// Context for poe.prepare request
#[derive(Debug, Clone, Deserialize, Default)]
pub struct PrepareContext {
    /// Working directory
    #[serde(default)]
    pub working_dir: Option<String>,
    /// Related files
    #[serde(default)]
    pub files: Vec<String>,
    /// Session key for events
    #[serde(default)]
    pub session_key: Option<String>,
}

impl From<PrepareContext> for ContractContext {
    fn from(ctx: PrepareContext) -> Self {
        ContractContext {
            working_dir: ctx.working_dir,
            files: ctx.files,
            session_key: ctx.session_key,
        }
    }
}

/// Parameters for poe.reject request
#[derive(Debug, Clone, Deserialize)]
pub struct RejectParams {
    /// Contract ID to reject
    pub contract_id: String,
    /// Optional rejection reason
    #[serde(default)]
    pub reason: Option<String>,
}

// ============================================================================
// PoeContractService
// ============================================================================

/// Service for managing POE contract signing workflow.
///
/// Handles the full lifecycle: prepare → sign/reject → execute
pub struct PoeContractService<W: Worker + 'static> {
    /// Manifest builder for generating contracts
    manifest_builder: Arc<ManifestBuilder>,
    /// Store for pending contracts
    contract_store: Arc<PendingContractStore>,
    /// Run manager for executing signed contracts
    run_manager: Arc<PoeRunManager<W>>,
    /// Event bus for publishing events
    event_bus: Arc<GatewayEventBus>,
}

impl<W: Worker + 'static> PoeContractService<W> {
    /// Create a new contract service.
    pub fn new(
        manifest_builder: Arc<ManifestBuilder>,
        run_manager: Arc<PoeRunManager<W>>,
        event_bus: Arc<GatewayEventBus>,
    ) -> Self {
        Self {
            manifest_builder,
            contract_store: Arc::new(PendingContractStore::new()),
            run_manager,
            event_bus,
        }
    }

    /// Get access to the contract store.
    pub fn contract_store(&self) -> &Arc<PendingContractStore> {
        &self.contract_store
    }

    /// Get access to the run manager.
    pub fn run_manager(&self) -> &Arc<PoeRunManager<W>> {
        &self.run_manager
    }

    /// Prepare a new contract from instruction.
    ///
    /// Generates a SuccessManifest using ManifestBuilder and stores it
    /// in the pending contracts store awaiting signature.
    pub async fn prepare(&self, params: PrepareParams) -> Result<PrepareResult, AetherError> {
        // 1. Build context string for ManifestBuilder
        let context_str = params.context.as_ref().and_then(|ctx| {
            let contract_ctx: ContractContext = ctx.clone().into();
            contract_ctx.to_context_string()
        });

        // 2. Generate manifest using ManifestBuilder
        let manifest = self
            .manifest_builder
            .build(&params.instruction, context_str.as_deref())
            .await?;

        // 3. Generate contract ID
        let contract_id = format!(
            "contract-{}",
            &uuid::Uuid::new_v4().to_string()[..8]
        );

        // 4. Create pending contract
        let mut contract = PendingContract::new(
            contract_id.clone(),
            params.instruction.clone(),
            manifest.clone(),
        );

        if let Some(ctx) = params.context {
            contract = contract.with_context(ctx.into());
        }

        // 5. Store in pending contracts
        self.contract_store.insert(contract).await;

        info!(contract_id = %contract_id, "Contract prepared, awaiting signature");

        // 6. Emit event
        self.emit_event(
            "poe.contract_generated",
            &json!({
                "contract_id": contract_id,
                "objective": manifest.objective,
                "constraint_count": manifest.hard_constraints.len(),
                "metric_count": manifest.soft_metrics.len(),
            }),
        );

        Ok(PrepareResult {
            contract_id,
            manifest,
            created_at: Utc::now().to_rfc3339(),
            instruction: params.instruction,
        })
    }

    /// Sign a pending contract and start execution.
    ///
    /// Optionally applies amendments before execution.
    pub async fn sign(&self, params: SignRequest) -> Result<SignResult, AetherError> {
        // 1. Take contract from store (atomic remove + return)
        let contract = self
            .contract_store
            .take(&params.contract_id)
            .await
            .ok_or_else(|| {
                AetherError::NotFound(format!(
                    "Contract {} not found or already signed",
                    params.contract_id
                ))
            })?;

        // 2. Apply amendments if provided
        let final_manifest = match (&params.amendments, &params.manifest_override) {
            // Natural language amendment
            (Some(amendment), None) => {
                self.manifest_builder
                    .amend(&contract.manifest, amendment)
                    .await?
            }
            // JSON override
            (None, Some(override_manifest)) => {
                ManifestBuilder::merge_override(&contract.manifest, override_manifest)
            }
            // Both: merge first, then amend
            (Some(amendment), Some(override_manifest)) => {
                let merged = ManifestBuilder::merge_override(&contract.manifest, override_manifest);
                self.manifest_builder.amend(&merged, amendment).await?
            }
            // No modifications
            (None, None) => contract.manifest.clone(),
        };

        info!(
            contract_id = %params.contract_id,
            task_id = %final_manifest.task_id,
            amendments = params.amendments.is_some(),
            "Contract signed, starting execution"
        );

        // 3. Emit signed event
        self.emit_event(
            "poe.signed",
            &json!({
                "contract_id": params.contract_id,
                "task_id": final_manifest.task_id,
                "amendments_applied": params.amendments.is_some() || params.manifest_override.is_some(),
                "signed_at": Utc::now().to_rfc3339(),
            }),
        );

        // 4. Start POE execution via run manager
        let run_params = PoeRunParams {
            manifest: final_manifest.clone(),
            instruction: contract.instruction,
            stream: params.stream,
            config: None,
        };

        let run_result = self
            .run_manager
            .start_run(run_params)
            .await
            .map_err(|e| AetherError::other(e))?;

        Ok(SignResult {
            task_id: run_result.task_id,
            session_key: run_result.session_key,
            signed_at: Utc::now().to_rfc3339(),
            final_manifest,
        })
    }

    /// Reject a pending contract.
    pub async fn reject(&self, params: RejectParams) -> RejectResult {
        let removed = self.contract_store.remove(&params.contract_id).await;

        if removed {
            info!(
                contract_id = %params.contract_id,
                reason = ?params.reason,
                "Contract rejected"
            );

            self.emit_event(
                "poe.rejected",
                &json!({
                    "contract_id": params.contract_id,
                    "reason": params.reason.as_deref().unwrap_or("User cancelled"),
                    "rejected_at": Utc::now().to_rfc3339(),
                }),
            );
        }

        RejectResult {
            contract_id: params.contract_id,
            rejected: removed,
        }
    }

    /// List all pending contracts.
    pub async fn pending(&self) -> PendingResult {
        let contracts = self.contract_store.list().await;
        let count = contracts.len();

        PendingResult {
            contracts: contracts.into_iter().map(ContractSummary::from).collect(),
            count,
        }
    }

    /// Emit an event to the event bus.
    fn emit_event<T: Serialize>(&self, topic: &str, data: &T) {
        if let Ok(data_value) = serde_json::to_value(data) {
            let notification = JsonRpcRequest::notification(
                topic,
                Some(json!({
                    "topic": topic,
                    "data": data_value,
                    "timestamp": Utc::now().timestamp_millis()
                })),
            );
            if let Ok(json) = serde_json::to_string(&notification) {
                self.event_bus.publish(json);
            }
        }
    }
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
    use crate::poe::worker::MockWorker;
    use crate::poe::ValidationRule;
    use crate::providers::MockProvider;
    use std::path::PathBuf;

    fn create_test_manager() -> PoeRunManager<MockWorker> {
        let event_bus = Arc::new(GatewayEventBus::new());

        // Worker factory - creates a new MockWorker for each run
        let worker_factory: WorkerFactory<MockWorker> = Arc::new(|| MockWorker::new());

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
        let request = JsonRpcRequest::new("poe.run", None, Some(json!(1)));
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

        let request = JsonRpcRequest::new("poe.list", None, Some(json!(1)));
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
            PoeTaskStatus::Completed(PoeOutcome::success(crate::poe::Verdict::success("ok")))
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
