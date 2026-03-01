//! POE Run Service
//!
//! Business logic for POE task execution, status tracking, and cancellation.

use chrono::Utc;
use serde::Serialize;
use serde_json::json;
use std::collections::HashMap;
use crate::sync_primitives::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use crate::dispatcher::experience_replay_layer::ExperienceReplayLayer;
use crate::gateway::event_bus::GatewayEventBus;
use crate::gateway::protocol::JsonRpcRequest;
use crate::poe::{
    CompositeValidator, PoeConfig, PoeManager, PoeOutcome, PoeTask, Worker,
};
use crate::poe::crystallization::ExperienceRecorder;
use crate::poe::event_bus::PoeEventBus;
use crate::poe::handler_types::{
    PoeAcceptedEvent, PoeCompletedEvent, PoeErrorEvent, PoeStepEvent, PoeValidationEvent,
    PoeCancelResult, PoeConfigParams, PoeRunParams, PoeRunResult,
    PoeStatusResult, PoeTaskState, PoeTaskStatus,
    ValidatorFactory, WorkerFactory,
};
use crate::poe::{ValidationCallback, ValidationEvent};

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
    /// Optional POE event bus for domain events
    poe_event_bus: Option<Arc<PoeEventBus>>,
    /// Optional experience recorder for crystallization
    recorder: Option<Arc<dyn ExperienceRecorder>>,
    /// Optional experience replay layer for hint injection
    experience_replay: Option<Arc<ExperienceReplayLayer>>,
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
            poe_event_bus: None,
            recorder: None,
            experience_replay: None,
        }
    }

    /// Set the POE event bus for domain event emission.
    pub fn with_poe_event_bus(mut self, bus: Arc<PoeEventBus>) -> Self {
        self.poe_event_bus = Some(bus);
        self
    }

    /// Set the experience recorder for crystallization.
    pub fn with_recorder(mut self, recorder: Arc<dyn ExperienceRecorder>) -> Self {
        self.recorder = Some(recorder);
        self
    }

    /// Set the experience replay layer for hint injection.
    pub fn with_experience_replay(mut self, replay: Arc<ExperienceReplayLayer>) -> Self {
        self.experience_replay = Some(replay);
        self
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
        let poe_event_bus = self.poe_event_bus.clone();
        let recorder = self.recorder.clone();
        let experience_replay = self.experience_replay.clone();

        tokio::spawn(async move {
            let result = execute_poe_task(PoeExecutionContext {
                task_id: task_id_clone.clone(),
                task,
                worker,
                validator,
                config,
                event_bus: event_bus.clone(),
                active_tasks: active_tasks.clone(),
                abort_rx,
                stream,
                poe_event_bus,
                recorder,
                experience_replay,
            })
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

/// Context for a single POE task execution.
struct PoeExecutionContext<W: Worker + 'static> {
    task_id: String,
    task: PoeTask,
    worker: W,
    validator: CompositeValidator,
    config: PoeConfig,
    event_bus: Arc<GatewayEventBus>,
    active_tasks: Arc<RwLock<HashMap<String, PoeTaskState>>>,
    abort_rx: tokio::sync::watch::Receiver<bool>,
    stream: bool,
    poe_event_bus: Option<Arc<PoeEventBus>>,
    recorder: Option<Arc<dyn ExperienceRecorder>>,
    experience_replay: Option<Arc<ExperienceReplayLayer>>,
}

/// Execute a POE task with event emission
async fn execute_poe_task<W: Worker + 'static>(
    ctx: PoeExecutionContext<W>,
) -> Result<PoeOutcome, String> {
    let PoeExecutionContext {
        task_id,
        mut task,
        worker,
        validator,
        config,
        event_bus,
        active_tasks,
        mut abort_rx,
        stream,
        poe_event_bus,
        recorder,
        experience_replay,
    } = ctx;

    let start_time = Instant::now();

    // Query experience replay for hint injection
    if let Some(ref replay) = experience_replay {
        match replay.try_match(&task.manifest.objective).await {
            Ok(Some(match_result)) => {
                info!(
                    task_id = %task_id,
                    confidence = match_result.confidence,
                    "Experience replay match found"
                );
                task.instruction = format_experience_hint(
                    match_result.confidence,
                    &match_result.tool_sequence,
                    &task.instruction,
                );
            }
            Ok(None) => {
                debug!(task_id = %task_id, "No experience replay match found");
            }
            Err(e) => {
                warn!(task_id = %task_id, error = %e, "Experience replay query failed, proceeding without hint");
            }
        }
    }

    // Helper to emit events
    let emit = |topic: &str, data: serde_json::Value| {
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

    // Create manager with validation callback and optional event bus/recorder
    let mut manager = PoeManager::new(worker, validator, config)
        .with_validation_callback(validation_callback);

    if let Some(bus) = poe_event_bus {
        manager = manager.with_event_bus(bus);
    }
    if let Some(rec) = recorder {
        manager = manager.with_recorder(rec);
    }

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

/// Format an experience replay hint to prepend to an instruction.
///
/// Extracted as a standalone function for testability.
fn format_experience_hint(confidence: f64, tool_sequence: &str, instruction: &str) -> String {
    format!(
        "[Experience hint: Similar task matched with {:.0}% confidence. Previous solution: {}]\n\n{}",
        confidence * 100.0,
        tool_sequence,
        instruction,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_experience_hint_prepends_to_instruction() {
        let instruction = "Deploy the service to production";
        let tool_sequence = "build -> test -> deploy";
        let confidence = 0.92;

        let result = format_experience_hint(confidence, tool_sequence, instruction);

        assert!(result.starts_with("[Experience hint:"));
        assert!(result.contains("92%"));
        assert!(result.contains("build -> test -> deploy"));
        assert!(result.ends_with(instruction));
    }

    #[test]
    fn test_format_experience_hint_confidence_rounding() {
        let result = format_experience_hint(0.856, "step1", "do X");
        assert!(result.contains("86%"));

        let result = format_experience_hint(1.0, "step1", "do X");
        assert!(result.contains("100%"));

        let result = format_experience_hint(0.0, "step1", "do X");
        assert!(result.contains("0%"));
    }

    #[test]
    fn test_format_experience_hint_preserves_original_instruction() {
        let instruction = "A multi-line\ninstruction with\nspecial chars: !@#$%";
        let result = format_experience_hint(0.9, "seq", instruction);
        assert!(result.contains(instruction));
    }
}
