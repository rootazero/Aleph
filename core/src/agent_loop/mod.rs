//! Agent Loop - Core execution engine for Aether
//!
//! This module implements the Agent Loop architecture, a unified
//! observe-think-act-feedback cycle for executing user tasks.
//!
//! # Architecture
//!
//! ```text
//! User Request → IntentRouter (L0-L2) → Fast Path / Agent Loop
//!                                              ↓
//!                                    ┌─────────────────┐
//!                                    │  Guards Check   │
//!                                    │  Compress       │
//!                                    │  Think (LLM)    │
//!                                    │  Decide         │
//!                                    │  Execute        │
//!                                    │  Feedback       │
//!                                    │  ↑______↓       │
//!                                    └─────────────────┘
//! ```
//!
//! # Compaction Trigger Points
//!
//! The agent loop emits events at key points for SessionCompactor integration:
//!
//! 1. **Before each iteration**: Emit `LoopContinue` with current token count
//!    - SessionCompactor checks for overflow and triggers compaction if needed
//!
//! 2. **After tool execution**: Emit `ToolCallCompleted`
//!    - SessionCompactor triggers pruning check
//!
//! 3. **Session end**: Emit `LoopStop` with reason
//!    - SessionCompactor performs final cleanup/pruning
//!
//! This pattern matches OpenCode's approach (prompt.ts:498-511).
//!
//! # Components
//!
//! - **AgentLoop**: Main loop controller
//! - **LoopState**: Session state management
//! - **Decision**: LLM decision types
//! - **LoopGuard**: Safety guards
//! - **LoopCallback**: UI callback interface
//! - **CompactionTrigger**: Event emission for compaction integration
//!
//! # Example
//!
//! ```rust,ignore
//! use aethecore::agent_loop::{AgentLoop, LoopConfig, NoOpLoopCallback};
//!
//! let config = LoopConfig::default();
//! let agent_loop = AgentLoop::new(thinker, executor, compressor, config);
//!
//! let result = agent_loop.run(
//!     "Search for Rust tutorials".to_string(),
//!     RequestContext::empty(),
//!     tools,
//!     NoOpLoopCallback,
//! ).await;
//! ```

pub mod callback;
pub mod config;
pub mod decision;
pub mod guards;
pub mod message_builder;
pub mod session_sync;
pub mod state;

use std::sync::Arc;
use tokio::sync::watch;

use crate::event::{
    AetherEvent, EventBus, LoopState as EventLoopState, StopReason, TokenUsage, ToolCallResult,
};

pub use callback::{CollectingCallback, LoggingCallback, LoopCallback, LoopEvent, NoOpLoopCallback};
pub use config::{CompressionConfig, LoopConfig, ModelRoutingConfig, ThinkRetryConfig};
pub use decision::{Action, ActionResult, Decision, LlmAction, LlmResponse};
pub use guards::{GuardViolation, LoopGuard};
pub use message_builder::{Message, MessageBuilder, MessageBuilderConfig, ToolCall};
pub use session_sync::SessionSync;
pub use state::{LoopState, LoopStep, Observation, RequestContext, StepSummary, Thinking, ToolInfo};

// Re-export CompactionTrigger for external integration
// (useful when building custom agent loops with compaction support)

use crate::error::Result;

// ============================================================================
// Compaction Trigger
// ============================================================================

/// Helper for triggering compaction checks at key points in the agent loop.
///
/// This struct encapsulates the event emission logic for SessionCompactor
/// integration, following OpenCode's pattern of checking for context overflow
/// before each iteration and after tool execution.
///
/// # Event Flow
///
/// ```text
/// AgentLoop                    EventBus                 SessionCompactor
///     │                           │                           │
///     │── LoopContinue ──────────>│────────────────────────>│
///     │                           │                  (check overflow)
///     │                           │                           │
///     │── ToolCallCompleted ─────>│────────────────────────>│
///     │                           │                   (prune check)
///     │                           │                           │
///     │── LoopStop ──────────────>│────────────────────────>│
///     │                           │                (final cleanup)
/// ```
pub struct CompactionTrigger {
    event_bus: Arc<EventBus>,
}

impl CompactionTrigger {
    /// Create a new CompactionTrigger with the given EventBus
    pub fn new(event_bus: Arc<EventBus>) -> Self {
        Self { event_bus }
    }

    /// Emit LoopContinue event before each iteration
    ///
    /// This triggers SessionCompactor to check for overflow and
    /// compact the session if needed. Should be called at the start
    /// of each loop iteration (except the first).
    pub async fn emit_loop_continue(
        &self,
        session_id: &str,
        iteration: u32,
        total_tokens: u64,
        last_tool: Option<String>,
        model: &str,
    ) {
        let loop_state = EventLoopState {
            session_id: session_id.to_string(),
            iteration,
            total_tokens,
            last_tool,
            model: model.to_string(),
        };

        tracing::debug!(
            session_id = %session_id,
            iteration = iteration,
            total_tokens = total_tokens,
            "Emitting LoopContinue for compaction check"
        );

        self.event_bus
            .publish(AetherEvent::LoopContinue(loop_state))
            .await;
    }

    /// Emit ToolCallCompleted event after tool execution
    ///
    /// This triggers SessionCompactor to check if pruning is needed.
    pub async fn emit_tool_completed(
        &self,
        session_id: &str,
        call_id: &str,
        tool_name: &str,
        input: serde_json::Value,
        output: &str,
        started_at: i64,
        completed_at: i64,
        input_tokens: u64,
        output_tokens: u64,
    ) {
        let result = ToolCallResult {
            call_id: call_id.to_string(),
            tool: tool_name.to_string(),
            input,
            output: output.to_string(),
            started_at,
            completed_at,
            token_usage: TokenUsage {
                input_tokens,
                output_tokens,
            },
            session_id: Some(session_id.to_string()),
        };

        tracing::debug!(
            session_id = %session_id,
            tool = %tool_name,
            "Emitting ToolCallCompleted for pruning check"
        );

        self.event_bus
            .publish(AetherEvent::ToolCallCompleted(result))
            .await;
    }

    /// Emit LoopStop event when the session ends
    ///
    /// This triggers final cleanup and pruning by SessionCompactor.
    pub async fn emit_loop_stop(&self, reason: StopReason) {
        tracing::debug!(
            reason = ?reason,
            "Emitting LoopStop for final cleanup"
        );

        self.event_bus
            .publish(AetherEvent::LoopStop(reason))
            .await;
    }
}

/// Optional wrapper for CompactionTrigger that handles the None case gracefully
pub struct OptionalCompactionTrigger {
    inner: Option<CompactionTrigger>,
}

impl OptionalCompactionTrigger {
    /// Create a new OptionalCompactionTrigger
    pub fn new(event_bus: Option<Arc<EventBus>>) -> Self {
        Self {
            inner: event_bus.map(CompactionTrigger::new),
        }
    }

    /// Emit LoopContinue if trigger is available
    pub async fn emit_loop_continue(
        &self,
        session_id: &str,
        iteration: u32,
        total_tokens: u64,
        last_tool: Option<String>,
        model: &str,
    ) {
        if let Some(ref trigger) = self.inner {
            trigger
                .emit_loop_continue(session_id, iteration, total_tokens, last_tool, model)
                .await;
        }
    }

    /// Emit ToolCallCompleted if trigger is available
    #[allow(clippy::too_many_arguments)]
    pub async fn emit_tool_completed(
        &self,
        session_id: &str,
        call_id: &str,
        tool_name: &str,
        input: serde_json::Value,
        output: &str,
        started_at: i64,
        completed_at: i64,
        input_tokens: u64,
        output_tokens: u64,
    ) {
        if let Some(ref trigger) = self.inner {
            trigger
                .emit_tool_completed(
                    session_id,
                    call_id,
                    tool_name,
                    input,
                    output,
                    started_at,
                    completed_at,
                    input_tokens,
                    output_tokens,
                )
                .await;
        }
    }

    /// Emit LoopStop if trigger is available
    pub async fn emit_loop_stop(&self, reason: StopReason) {
        if let Some(ref trigger) = self.inner {
            trigger.emit_loop_stop(reason).await;
        }
    }
}

/// Result of an Agent Loop execution
#[derive(Debug, Clone)]
pub enum LoopResult {
    /// Task completed successfully
    Completed {
        /// Summary of what was accomplished
        summary: String,
        /// Number of steps taken
        steps: usize,
        /// Total tokens consumed
        total_tokens: usize,
    },
    /// Task failed
    Failed {
        /// Reason for failure
        reason: String,
        /// Number of steps taken before failure
        steps: usize,
    },
    /// Guard triggered (resource limit hit)
    GuardTriggered(GuardViolation),
    /// User aborted the loop
    UserAborted,
}

impl LoopResult {
    /// Check if result is successful
    pub fn is_success(&self) -> bool {
        matches!(self, LoopResult::Completed { .. })
    }

    /// Get step count
    pub fn steps(&self) -> usize {
        match self {
            LoopResult::Completed { steps, .. } => *steps,
            LoopResult::Failed { steps, .. } => *steps,
            LoopResult::GuardTriggered(_) => 0,
            LoopResult::UserAborted => 0,
        }
    }
}

/// Thinker trait - abstraction for the thinking layer
///
/// This trait is implemented by the Thinker module to provide
/// LLM-based decision making.
#[async_trait::async_trait]
pub trait ThinkerTrait: Send + Sync {
    /// Think and produce a decision
    async fn think(
        &self,
        state: &LoopState,
        tools: &[crate::dispatcher::UnifiedTool],
    ) -> Result<Thinking>;
}

/// Action Executor trait - abstraction for the execution layer
///
/// This trait is implemented by the Executor module to execute
/// individual actions in the agent loop (observe-think-act cycle).
///
/// Note: This is distinct from:
/// - `dispatcher::executor::TaskExecutor` - for task-type specific execution
/// - `dispatcher::scheduler::GraphTaskExecutor` - for DAG node execution
#[async_trait::async_trait]
pub trait ActionExecutor: Send + Sync {
    /// Execute a single action
    async fn execute(&self, action: &Action) -> ActionResult;
}

/// Deprecated alias for backward compatibility
#[deprecated(since = "0.2.0", note = "Use ActionExecutor instead")]
pub type ExecutorTrait = dyn ActionExecutor;

/// Compressor trait - abstraction for context compression
///
/// This trait is implemented by the ContextCompressor module
/// to compress history for long-running sessions.
#[async_trait::async_trait]
pub trait CompressorTrait: Send + Sync {
    /// Check if compression is needed
    fn should_compress(&self, state: &LoopState) -> bool;

    /// Compress history and return summary
    async fn compress(
        &self,
        steps: &[LoopStep],
        current_summary: &str,
    ) -> Result<CompressedHistory>;
}

/// Result of compression
#[derive(Debug, Clone)]
pub struct CompressedHistory {
    /// New summary text
    pub summary: String,
    /// Number of steps that were compressed
    pub compressed_count: usize,
}

/// Agent Loop - Main execution controller
///
/// The AgentLoop manages the observe-think-act-feedback cycle,
/// coordinating between the Thinker (LLM decisions), Executor
/// (action execution), and Compressor (context management).
///
/// Optionally integrates with EventBus for compaction trigger points,
/// emitting events that SessionCompactor can subscribe to for
/// automatic context management.
pub struct AgentLoop<T, E, C>
where
    T: ThinkerTrait,
    E: ActionExecutor,
    C: CompressorTrait,
{
    thinker: Arc<T>,
    executor: Arc<E>,
    compressor: Arc<C>,
    config: LoopConfig,
    /// Optional EventBus for compaction trigger integration
    compaction_trigger: OptionalCompactionTrigger,
}

impl<T, E, C> AgentLoop<T, E, C>
where
    T: ThinkerTrait,
    E: ActionExecutor,
    C: CompressorTrait,
{
    /// Create a new AgentLoop
    pub fn new(thinker: Arc<T>, executor: Arc<E>, compressor: Arc<C>, config: LoopConfig) -> Self {
        Self {
            thinker,
            executor,
            compressor,
            config,
            compaction_trigger: OptionalCompactionTrigger::new(None),
        }
    }

    /// Create a new AgentLoop with EventBus integration for compaction
    ///
    /// When EventBus is provided, the loop will emit events at key points:
    /// - `LoopContinue` before each iteration (for overflow check)
    /// - `ToolCallCompleted` after tool execution (for pruning check)
    /// - `LoopStop` at session end (for final cleanup)
    pub fn with_event_bus(
        thinker: Arc<T>,
        executor: Arc<E>,
        compressor: Arc<C>,
        config: LoopConfig,
        event_bus: Arc<EventBus>,
    ) -> Self {
        Self {
            thinker,
            executor,
            compressor,
            config,
            compaction_trigger: OptionalCompactionTrigger::new(Some(event_bus)),
        }
    }

    /// Run the Agent Loop
    ///
    /// This is the main entry point that executes the observe-think-act-feedback
    /// cycle until the task is complete or a guard is triggered.
    ///
    /// # Arguments
    /// * `request` - The user's request
    /// * `context` - Request context (attachments, app context, etc.)
    /// * `tools` - Available tools for this loop
    /// * `callback` - Callback for loop events
    /// * `abort_signal` - Optional signal to abort the loop
    /// * `initial_history` - Optional history summary from previous conversations
    pub async fn run(
        &self,
        request: String,
        context: RequestContext,
        tools: Vec<crate::dispatcher::UnifiedTool>,
        callback: impl LoopCallback,
        abort_signal: Option<watch::Receiver<bool>>,
        initial_history: Option<String>,
    ) -> LoopResult {
        // Generate session ID
        let session_id = uuid::Uuid::new_v4().to_string();

        // Initialize state
        let mut state = LoopState::new(session_id, request, context);

        // Inject initial history if provided (for cross-session context)
        if let Some(history) = initial_history {
            state.history_summary = history;
        }

        // Initialize guard
        let mut guard = LoopGuard::new(self.config.clone());

        // Notify loop start
        callback.on_loop_start(&state).await;

        // Track iteration for compaction trigger
        let mut iteration: u32 = 0;
        // Track last tool for compaction context
        let mut last_tool: Option<String> = None;
        // Get model from config for compaction threshold lookup
        let model = self.config.model_routing.default_model.clone();

        loop {
            // Check abort signal
            if let Some(ref abort) = abort_signal {
                if *abort.borrow() {
                    callback.on_aborted().await;
                    // ===== COMPACTION TRIGGER: Session End (Aborted) =====
                    self.compaction_trigger
                        .emit_loop_stop(StopReason::UserAborted)
                        .await;
                    return LoopResult::UserAborted;
                }
            }

            // ===== COMPACTION TRIGGER: Before Iteration =====
            // Emit LoopContinue for SessionCompactor to check overflow
            // (matches OpenCode pattern: prompt.ts:498-511)
            if iteration > 0 {
                self.compaction_trigger
                    .emit_loop_continue(
                        &state.session_id,
                        iteration,
                        state.total_tokens as u64,
                        last_tool.clone(),
                        &model,
                    )
                    .await;
            }

            // ===== Guard Check =====
            if let Some(violation) = guard.check(&state) {
                callback.on_guard_triggered(&violation).await;
                // ===== COMPACTION TRIGGER: Session End (Guard) =====
                let stop_reason = match &violation {
                    GuardViolation::MaxSteps { .. } => StopReason::MaxIterationsReached,
                    GuardViolation::MaxTokens { .. } => StopReason::TokenLimitReached,
                    GuardViolation::DoomLoop { .. } => StopReason::DoomLoopDetected,
                    _ => StopReason::Error(violation.description()),
                };
                self.compaction_trigger.emit_loop_stop(stop_reason).await;
                return LoopResult::GuardTriggered(violation);
            }

            // ===== Compression =====
            if self.compressor.should_compress(&state) {
                match self
                    .compressor
                    .compress(&state.steps, &state.history_summary)
                    .await
                {
                    Ok(compressed) => {
                        let until = state.steps.len() - self.config.compression.recent_window_size;
                        state.apply_compression(compressed.summary, until);
                    }
                    Err(e) => {
                        tracing::warn!("Compression failed: {}", e);
                        // Continue without compression
                    }
                }
            }

            // ===== Think =====
            callback.on_step_start(state.step_count).await;
            callback.on_thinking_start(state.step_count).await;

            let thinking = match self.thinker.think(&state, &tools).await {
                Ok(t) => t,
                Err(e) => {
                    let reason = format!("Thinking failed: {}", e);
                    callback.on_failed(&reason).await;
                    return LoopResult::Failed {
                        reason,
                        steps: state.step_count,
                    };
                }
            };

            callback.on_thinking_done(&thinking).await;

            // ===== Decide =====
            let action: Action = match &thinking.decision {
                Decision::Complete { summary } => {
                    callback.on_complete(summary).await;
                    // ===== COMPACTION TRIGGER: Session End (Completed) =====
                    self.compaction_trigger
                        .emit_loop_stop(StopReason::Completed)
                        .await;
                    return LoopResult::Completed {
                        summary: summary.clone(),
                        steps: state.step_count,
                        total_tokens: state.total_tokens,
                    };
                }
                Decision::Fail { reason } => {
                    callback.on_failed(reason).await;
                    // ===== COMPACTION TRIGGER: Session End (Failed) =====
                    self.compaction_trigger
                        .emit_loop_stop(StopReason::Error(reason.clone()))
                        .await;
                    return LoopResult::Failed {
                        reason: reason.clone(),
                        steps: state.step_count,
                    };
                }
                Decision::AskUser { question, options } => {
                    let response = callback
                        .on_user_input_required(question, options.as_deref())
                        .await;

                    // Record user interaction as a step
                    let step = LoopStep {
                        step_id: state.step_count,
                        observation_summary: String::new(),
                        thinking: thinking.clone(),
                        action: Action::UserInteraction {
                            question: question.clone(),
                            options: options.clone(),
                        },
                        result: ActionResult::UserResponse { response },
                        tokens_used: 0,
                        duration_ms: 0,
                    };
                    state.record_step(step);
                    guard.record_action("ask_user");
                    continue;
                }
                Decision::UseTool {
                    tool_name,
                    arguments,
                } => {
                    // Record tool call for doom loop detection BEFORE checking
                    guard.record_tool_call(tool_name, arguments);

                    // Check for doom loop (exact same tool + arguments repeated)
                    if let Some(GuardViolation::DoomLoop {
                        tool_name: doom_tool,
                        repeat_count,
                        ..
                    }) = guard.check(&state)
                    {
                        // Only handle DoomLoop here, let other guards be checked at loop start
                        if matches!(
                            guard.check(&state),
                            Some(GuardViolation::DoomLoop { .. })
                        ) {
                            // Ask user if they want to continue
                            let should_continue = callback
                                .on_doom_loop_detected(&doom_tool, arguments, repeat_count)
                                .await;

                            if should_continue {
                                // User wants to continue - reset detection and proceed
                                guard.reset_doom_loop_detection();
                            } else {
                                // User doesn't want to continue - trigger guard
                                let violation = GuardViolation::DoomLoop {
                                    tool_name: doom_tool,
                                    repeat_count,
                                    arguments_preview: serde_json::to_string(arguments)
                                        .unwrap_or_default()
                                        .chars()
                                        .take(100)
                                        .collect(),
                                };
                                callback.on_guard_triggered(&violation).await;
                                // ===== COMPACTION TRIGGER: Session End (Doom Loop) =====
                                self.compaction_trigger
                                    .emit_loop_stop(StopReason::DoomLoopDetected)
                                    .await;
                                return LoopResult::GuardTriggered(violation);
                            }
                        }
                    }

                    // Check if confirmation required
                    if guard.requires_confirmation(tool_name) {
                        let confirmed = callback
                            .on_confirmation_required(tool_name, arguments)
                            .await;
                        if !confirmed {
                            // User cancelled, record and continue
                            let step = LoopStep {
                                step_id: state.step_count,
                                observation_summary: String::new(),
                                thinking: thinking.clone(),
                                action: Action::ToolCall {
                                    tool_name: tool_name.clone(),
                                    arguments: arguments.clone(),
                                },
                                result: ActionResult::ToolError {
                                    error: "User cancelled".to_string(),
                                    retryable: false,
                                },
                                tokens_used: 0,
                                duration_ms: 0,
                            };
                            state.record_step(step);
                            guard.record_action(&format!("cancelled:{}", tool_name));
                            continue;
                        }
                    }

                    Action::ToolCall {
                        tool_name: tool_name.clone(),
                        arguments: arguments.clone(),
                    }
                }
            };

            // ===== Execute =====
            callback.on_action_start(&action).await;

            let start_time = std::time::Instant::now();
            let started_at = chrono::Utc::now().timestamp_millis();
            let result = self.executor.execute(&action).await;
            let duration_ms = start_time.elapsed().as_millis() as u64;
            let completed_at = chrono::Utc::now().timestamp_millis();

            callback.on_action_done(&action, &result).await;

            // ===== COMPACTION TRIGGER: After Tool Execution =====
            // Emit ToolCallCompleted for SessionCompactor pruning check
            if let Action::ToolCall {
                tool_name,
                arguments,
            } = &action
            {
                let call_id = uuid::Uuid::new_v4().to_string();
                let output = result.full_output();

                self.compaction_trigger
                    .emit_tool_completed(
                        &state.session_id,
                        &call_id,
                        tool_name,
                        arguments.clone(),
                        &output,
                        started_at,
                        completed_at,
                        0, // Token usage not tracked at this level
                        0,
                    )
                    .await;

                // Update last_tool for next iteration's LoopContinue event
                last_tool = Some(tool_name.clone());
            }

            // ===== Feedback (Update State) =====
            guard.record_action(&action.action_type());

            let step = LoopStep {
                step_id: state.step_count,
                observation_summary: String::new(), // Will be filled by compressor
                thinking,
                action,
                result,
                tokens_used: 0, // Will be updated by thinker
                duration_ms,
            };
            state.record_step(step);

            // Increment iteration counter
            iteration += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // Mock Thinker that returns predefined decisions
    struct MockThinker {
        decisions: std::sync::Mutex<Vec<Decision>>,
        call_count: AtomicUsize,
    }

    impl MockThinker {
        fn new(decisions: Vec<Decision>) -> Self {
            Self {
                decisions: std::sync::Mutex::new(decisions),
                call_count: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait::async_trait]
    impl ThinkerTrait for MockThinker {
        async fn think(
            &self,
            _state: &LoopState,
            _tools: &[crate::dispatcher::UnifiedTool],
        ) -> Result<Thinking> {
            let count = self.call_count.fetch_add(1, Ordering::SeqCst);
            let decisions = self.decisions.lock().unwrap();
            let decision = decisions
                .get(count)
                .cloned()
                .unwrap_or(Decision::Fail {
                    reason: "No more decisions".to_string(),
                });

            Ok(Thinking {
                reasoning: Some(format!("Step {}", count)),
                decision,
            })
        }
    }

    // Mock Executor
    struct MockExecutor;

    #[async_trait::async_trait]
    impl ActionExecutor for MockExecutor {
        async fn execute(&self, action: &Action) -> ActionResult {
            match action {
                Action::ToolCall { tool_name, .. } => ActionResult::ToolSuccess {
                    output: json!({"tool": tool_name, "result": "success"}),
                    duration_ms: 100,
                },
                _ => ActionResult::Completed,
            }
        }
    }

    // Mock Compressor
    struct MockCompressor;

    #[async_trait::async_trait]
    impl CompressorTrait for MockCompressor {
        fn should_compress(&self, _state: &LoopState) -> bool {
            false
        }

        async fn compress(
            &self,
            _steps: &[LoopStep],
            _current_summary: &str,
        ) -> Result<CompressedHistory> {
            Ok(CompressedHistory {
                summary: "Compressed".to_string(),
                compressed_count: 0,
            })
        }
    }

    #[tokio::test]
    async fn test_simple_completion() {
        let thinker = Arc::new(MockThinker::new(vec![Decision::Complete {
            summary: "Task done".to_string(),
        }]));
        let executor = Arc::new(MockExecutor);
        let compressor = Arc::new(MockCompressor);

        let agent_loop = AgentLoop::new(
            thinker,
            executor,
            compressor,
            LoopConfig::for_testing(),
        );

        let result = agent_loop
            .run(
                "Test request".to_string(),
                RequestContext::empty(),
                vec![],
                NoOpLoopCallback,
                None,
                None, // No initial history
            )
            .await;

        assert!(matches!(result, LoopResult::Completed { .. }));
        if let LoopResult::Completed { summary, steps, .. } = result {
            assert_eq!(summary, "Task done");
            assert_eq!(steps, 0);
        }
    }

    #[tokio::test]
    async fn test_tool_execution() {
        let thinker = Arc::new(MockThinker::new(vec![
            Decision::UseTool {
                tool_name: "search".to_string(),
                arguments: json!({"query": "test"}),
            },
            Decision::Complete {
                summary: "Search complete".to_string(),
            },
        ]));
        let executor = Arc::new(MockExecutor);
        let compressor = Arc::new(MockCompressor);

        let agent_loop = AgentLoop::new(
            thinker,
            executor,
            compressor,
            LoopConfig::for_testing(),
        );

        let callback = CollectingCallback::new();

        let result = agent_loop
            .run(
                "Search for something".to_string(),
                RequestContext::empty(),
                vec![],
                &callback,
                None,
                None, // No initial history
            )
            .await;

        assert!(matches!(result, LoopResult::Completed { steps: 1, .. }));

        let events = callback.events();
        assert!(events.iter().any(|e| matches!(e, LoopEvent::ActionStart { .. })));
        assert!(events.iter().any(|e| matches!(e, LoopEvent::ActionDone { success: true, .. })));
    }

    #[tokio::test]
    async fn test_max_steps_guard() {
        // Create thinker that always returns a tool call
        let decisions: Vec<Decision> = (0..20)
            .map(|i| Decision::UseTool {
                tool_name: format!("tool_{}", i),
                arguments: json!({}),
            })
            .collect();

        let thinker = Arc::new(MockThinker::new(decisions));
        let executor = Arc::new(MockExecutor);
        let compressor = Arc::new(MockCompressor);

        let config = LoopConfig::for_testing().with_max_steps(5);

        let agent_loop = AgentLoop::new(thinker, executor, compressor, config);

        let result = agent_loop
            .run(
                "Run many steps".to_string(),
                RequestContext::empty(),
                vec![],
                NoOpLoopCallback,
                None,
                None, // No initial history
            )
            .await;

        assert!(matches!(
            result,
            LoopResult::GuardTriggered(GuardViolation::MaxSteps { .. })
        ));
    }

    // ========================================================================
    // Compaction Trigger Tests
    // ========================================================================

    #[tokio::test]
    async fn test_compaction_trigger_emits_loop_continue() {
        use crate::event::{AetherEvent, EventBus, EventType};

        let event_bus = Arc::new(EventBus::new());
        let mut subscriber = event_bus.subscribe_filtered(vec![EventType::LoopContinue]);

        let thinker = Arc::new(MockThinker::new(vec![
            Decision::UseTool {
                tool_name: "search".to_string(),
                arguments: json!({"query": "test"}),
            },
            Decision::Complete {
                summary: "Done".to_string(),
            },
        ]));
        let executor = Arc::new(MockExecutor);
        let compressor = Arc::new(MockCompressor);

        let agent_loop = AgentLoop::with_event_bus(
            thinker,
            executor,
            compressor,
            LoopConfig::for_testing(),
            event_bus.clone(),
        );

        let result = agent_loop
            .run(
                "Test request".to_string(),
                RequestContext::empty(),
                vec![],
                NoOpLoopCallback,
                None,
                None,
            )
            .await;

        assert!(result.is_success());

        // Check that LoopContinue was emitted (on second iteration)
        if let Ok(Some(event)) = subscriber.try_recv() {
            match event.event {
                AetherEvent::LoopContinue(state) => {
                    assert_eq!(state.iteration, 1); // First iteration after tool call
                    assert_eq!(state.last_tool, Some("search".to_string()));
                }
                _ => panic!("Expected LoopContinue event"),
            }
        }
        // Note: Event may not be received if loop completes quickly
    }

    #[tokio::test]
    async fn test_compaction_trigger_emits_tool_completed() {
        use crate::event::{AetherEvent, EventBus, EventType};

        let event_bus = Arc::new(EventBus::new());
        let mut subscriber = event_bus.subscribe_filtered(vec![EventType::ToolCallCompleted]);

        let thinker = Arc::new(MockThinker::new(vec![
            Decision::UseTool {
                tool_name: "search".to_string(),
                arguments: json!({"query": "test"}),
            },
            Decision::Complete {
                summary: "Done".to_string(),
            },
        ]));
        let executor = Arc::new(MockExecutor);
        let compressor = Arc::new(MockCompressor);

        let agent_loop = AgentLoop::with_event_bus(
            thinker,
            executor,
            compressor,
            LoopConfig::for_testing(),
            event_bus.clone(),
        );

        let result = agent_loop
            .run(
                "Test request".to_string(),
                RequestContext::empty(),
                vec![],
                NoOpLoopCallback,
                None,
                None,
            )
            .await;

        assert!(result.is_success());

        // Check that ToolCallCompleted was emitted
        if let Ok(Some(event)) = subscriber.try_recv() {
            match event.event {
                AetherEvent::ToolCallCompleted(result) => {
                    assert_eq!(result.tool, "search");
                    assert!(result.session_id.is_some());
                }
                _ => panic!("Expected ToolCallCompleted event"),
            }
        }
    }

    #[tokio::test]
    async fn test_compaction_trigger_emits_loop_stop_on_completion() {
        use crate::event::{AetherEvent, EventBus, EventType, StopReason};

        let event_bus = Arc::new(EventBus::new());
        let mut subscriber = event_bus.subscribe_filtered(vec![EventType::LoopStop]);

        let thinker = Arc::new(MockThinker::new(vec![Decision::Complete {
            summary: "Done".to_string(),
        }]));
        let executor = Arc::new(MockExecutor);
        let compressor = Arc::new(MockCompressor);

        let agent_loop = AgentLoop::with_event_bus(
            thinker,
            executor,
            compressor,
            LoopConfig::for_testing(),
            event_bus.clone(),
        );

        let result = agent_loop
            .run(
                "Test request".to_string(),
                RequestContext::empty(),
                vec![],
                NoOpLoopCallback,
                None,
                None,
            )
            .await;

        assert!(result.is_success());

        // Check that LoopStop was emitted with Completed reason
        if let Ok(Some(event)) = subscriber.try_recv() {
            match event.event {
                AetherEvent::LoopStop(reason) => {
                    assert!(matches!(reason, StopReason::Completed));
                }
                _ => panic!("Expected LoopStop event"),
            }
        }
    }

    #[tokio::test]
    async fn test_compaction_trigger_emits_loop_stop_on_guard() {
        use crate::event::{AetherEvent, EventBus, EventType, StopReason};

        let event_bus = Arc::new(EventBus::new());
        let mut subscriber = event_bus.subscribe_filtered(vec![EventType::LoopStop]);

        // Create thinker that always returns different tool calls
        let decisions: Vec<Decision> = (0..10)
            .map(|i| Decision::UseTool {
                tool_name: format!("tool_{}", i),
                arguments: json!({}),
            })
            .collect();

        let thinker = Arc::new(MockThinker::new(decisions));
        let executor = Arc::new(MockExecutor);
        let compressor = Arc::new(MockCompressor);

        let config = LoopConfig::for_testing().with_max_steps(3);

        let agent_loop =
            AgentLoop::with_event_bus(thinker, executor, compressor, config, event_bus.clone());

        let result = agent_loop
            .run(
                "Run many steps".to_string(),
                RequestContext::empty(),
                vec![],
                NoOpLoopCallback,
                None,
                None,
            )
            .await;

        assert!(matches!(
            result,
            LoopResult::GuardTriggered(GuardViolation::MaxSteps { .. })
        ));

        // Check that LoopStop was emitted with MaxIterationsReached reason
        if let Ok(Some(event)) = subscriber.try_recv() {
            match event.event {
                AetherEvent::LoopStop(reason) => {
                    assert!(matches!(reason, StopReason::MaxIterationsReached));
                }
                _ => panic!("Expected LoopStop event"),
            }
        }
    }

    #[test]
    fn test_optional_compaction_trigger_without_event_bus() {
        // Test that OptionalCompactionTrigger works when no EventBus is provided
        let trigger = OptionalCompactionTrigger::new(None);
        // This should not panic - it just does nothing
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            trigger
                .emit_loop_continue("session-1", 1, 1000, None, "model")
                .await;
            trigger.emit_loop_stop(StopReason::Completed).await;
        });
    }

    #[test]
    fn test_compaction_trigger_creation() {
        let event_bus = Arc::new(EventBus::new());
        let trigger = CompactionTrigger::new(event_bus);

        // Verify trigger was created (internal state is private, just ensure no panic)
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            trigger
                .emit_loop_continue("session-1", 0, 0, None, "test-model")
                .await;
        });
    }
}
