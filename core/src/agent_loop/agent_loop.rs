//! Agent Loop implementation - Main execution controller

use std::sync::Arc;
use tokio::sync::watch;

use aleph_protocol::IdentityContext;
use crate::agents::thinking::{is_thinking_level_error, ThinkingFallbackState};
use crate::event::{EventBus, StopReason};

use super::callback::LoopCallback;
use super::compaction_trigger::OptionalCompactionTrigger;
use super::config::LoopConfig;
use super::decision::{Action, ActionResult, Decision};
use super::guards::{GuardViolation, LoopGuard};
use super::loop_result::LoopResult;
use super::overflow::OverflowDetector;
use super::session_sync::SessionSync;
use super::state::{LoopState, LoopStep, RequestContext};
use super::traits::{ActionExecutor, CompressorTrait, ThinkerTrait};
use super::swarm_events::AgentLoopEvent;

/// Extract file paths from tool arguments
fn extract_affected_files(arguments: &serde_json::Value) -> Vec<String> {
    let mut files = Vec::new();

    // Check common argument names for file paths
    if let Some(obj) = arguments.as_object() {
        for key in &["path", "file_path", "file", "files", "target"] {
            if let Some(value) = obj.get(*key) {
                if let Some(s) = value.as_str() {
                    files.push(s.to_string());
                } else if let Some(arr) = value.as_array() {
                    for item in arr {
                        if let Some(s) = item.as_str() {
                            files.push(s.to_string());
                        }
                    }
                }
            }
        }
    }

    files
}

/// Context for running an agent loop execution
///
/// This struct encapsulates all the parameters needed to run an agent loop,
/// providing better API ergonomics and making it easier to add new parameters
/// without breaking existing code.
#[derive(Clone)]
pub struct RunContext {
    /// The user's request/query
    pub request: String,
    /// Request context (session info, metadata, etc.)
    pub context: RequestContext,
    /// Available tools for this loop
    pub tools: Vec<crate::dispatcher::UnifiedTool>,
    /// Identity context (user, device, permissions)
    pub identity: IdentityContext,
    /// Optional signal to abort the loop
    pub abort_signal: Option<watch::Receiver<bool>>,
    /// Optional history summary from previous conversations
    pub initial_history: Option<String>,
}

impl RunContext {
    /// Create a new RunContext with required parameters
    pub fn new(
        request: impl Into<String>,
        context: RequestContext,
        tools: Vec<crate::dispatcher::UnifiedTool>,
        identity: IdentityContext,
    ) -> Self {
        Self {
            request: request.into(),
            context,
            tools,
            identity,
            abort_signal: None,
            initial_history: None,
        }
    }

    /// Set the abort signal
    pub fn with_abort_signal(mut self, signal: watch::Receiver<bool>) -> Self {
        self.abort_signal = Some(signal);
        self
    }

    /// Set the initial history
    pub fn with_initial_history(mut self, history: impl Into<String>) -> Self {
        self.initial_history = Some(history.into());
        self
    }
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
///
/// # Unified Session Model Integration
///
/// When `config.use_realtime_overflow` is enabled, the loop uses
/// `OverflowDetector` to check for context overflow before each iteration.
/// This enables proactive compaction before the context window is exceeded.
pub struct AgentLoop<T, E, C>
where
    T: ThinkerTrait,
    E: ActionExecutor,
    C: CompressorTrait,
{
    thinker: Arc<T>,
    executor: Arc<E>,
    compressor: Arc<C>,
    pub(crate) config: LoopConfig,
    /// Optional EventBus for compaction trigger integration
    compaction_trigger: OptionalCompactionTrigger,
    /// Optional overflow detector for real-time overflow checking
    pub(crate) overflow_detector: Option<Arc<OverflowDetector>>,
    /// Optional SwarmCoordinator for swarm intelligence integration
    swarm_coordinator: Option<Arc<crate::agents::swarm::coordinator::SwarmCoordinator>>,
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
            overflow_detector: None,
            swarm_coordinator: None,
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
            overflow_detector: None,
            swarm_coordinator: None,
        }
    }

    /// Create a new AgentLoop with unified session model features
    ///
    /// This constructor enables the unified session model integration:
    /// - Optional EventBus for compaction trigger events
    /// - Optional OverflowDetector for real-time overflow checking
    ///
    /// # Arguments
    ///
    /// * `thinker` - The thinking layer for LLM decisions
    /// * `executor` - The action executor
    /// * `compressor` - The context compressor
    /// * `config` - Loop configuration (should have `use_realtime_overflow` enabled)
    /// * `event_bus` - Optional EventBus for compaction triggers
    /// * `overflow_detector` - Optional overflow detector for real-time checks
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let config = LoopConfig::default().with_realtime_overflow(true);
    /// let detector = Arc::new(OverflowDetector::default());
    /// let loop = AgentLoop::with_unified_session(
    ///     thinker, executor, compressor, config,
    ///     Some(event_bus), Some(detector),
    /// );
    /// ```
    pub fn with_unified_session(
        thinker: Arc<T>,
        executor: Arc<E>,
        compressor: Arc<C>,
        config: LoopConfig,
        event_bus: Option<Arc<EventBus>>,
        overflow_detector: Option<Arc<OverflowDetector>>,
    ) -> Self {
        Self {
            thinker,
            executor,
            compressor,
            config,
            compaction_trigger: OptionalCompactionTrigger::new(event_bus),
            overflow_detector,
            swarm_coordinator: None,
        }
    }

    /// Check if session is approaching overflow and needs compaction
    ///
    /// This method uses the unified session model to check if the current
    /// session is near the context window limit. Returns `false` if:
    /// - `use_realtime_overflow` is disabled in config
    /// - No overflow detector is configured
    ///
    /// # Arguments
    ///
    /// * `state` - The current loop state to check
    ///
    /// # Returns
    ///
    /// `true` if the session is at or above the threshold percentage of
    /// usable tokens for the configured model.
    pub fn should_compact_unified(&self, state: &LoopState) -> bool {
        if !self.config.use_realtime_overflow {
            return false;
        }
        if let Some(ref detector) = self.overflow_detector {
            let session = SessionSync::to_execution_session(state);
            return detector.is_near_overflow(&session, 85); // 85% threshold
        }
        false
    }

    /// Check if session has overflowed the context window
    ///
    /// Similar to `should_compact_unified`, but checks for actual overflow
    /// rather than approaching overflow.
    ///
    /// # Arguments
    ///
    /// * `state` - The current loop state to check
    ///
    /// # Returns
    ///
    /// `true` if the session has exceeded the usable token limit.
    pub fn is_overflow(&self, state: &LoopState) -> bool {
        if !self.config.use_realtime_overflow {
            return false;
        }
        if let Some(ref detector) = self.overflow_detector {
            let session = SessionSync::to_execution_session(state);
            return detector.is_overflow(&session);
        }
        false
    }

    /// Get the current token usage percentage
    ///
    /// # Arguments
    ///
    /// * `state` - The current loop state
    ///
    /// # Returns
    ///
    /// Usage percentage (0-100+), or 0 if overflow detection is disabled.
    pub fn usage_percent(&self, state: &LoopState) -> u8 {
        if let Some(ref detector) = self.overflow_detector {
            let session = SessionSync::to_execution_session(state);
            return detector.usage_percent(&session);
        }
        0
    }

    /// Set the swarm coordinator for swarm intelligence integration
    ///
    /// This enables event publishing to the swarm for collective intelligence.
    pub fn with_swarm_coordinator(
        mut self,
        coordinator: Arc<crate::agents::swarm::coordinator::SwarmCoordinator>,
    ) -> Self {
        self.swarm_coordinator = Some(coordinator);
        self
    }

    /// Run the Agent Loop
    ///
    /// This is the main entry point that executes the observe-think-act-feedback
    /// cycle until the task is complete or a guard is triggered.
    ///
    /// # Arguments
    /// * `run_context` - Context containing request, tools, identity, and optional parameters
    /// * `callback` - Callback for loop events
    pub async fn run(
        &self,
        run_context: RunContext,
        callback: impl LoopCallback,
    ) -> LoopResult {
        // Extract fields from context
        let RunContext {
            request,
            context,
            tools,
            identity,
            abort_signal,
            initial_history,
        } = run_context;

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

            // ===== UNIFIED SESSION: Overflow Check =====
            // Check for context overflow using the unified session model.
            // This enables proactive compaction recommendation before the
            // context window is exceeded.
            if self.config.use_realtime_overflow && iteration > 0 {
                if let Some(ref detector) = self.overflow_detector {
                    // Create a temporary ExecutionSession for overflow check
                    let session = SessionSync::to_execution_session(&state);
                    if detector.is_overflow(&session) {
                        // Log overflow detection for observability
                        tracing::info!(
                            session_id = %state.session_id,
                            total_tokens = state.total_tokens,
                            iteration = iteration,
                            "Overflow detected, compaction recommended"
                        );
                        // Note: Actual compaction is handled by the existing
                        // compressor.should_compress() logic below.
                        // This is informational for monitoring and debugging.
                    } else if detector.is_near_overflow(&session, 85) {
                        // Warn when approaching overflow (85% threshold)
                        tracing::debug!(
                            session_id = %state.session_id,
                            total_tokens = state.total_tokens,
                            usage_percent = detector.usage_percent(&session),
                            "Session approaching overflow threshold"
                        );
                    }
                }
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

            // ===== Think (with fallback retry) =====
            callback.on_step_start(state.step_count).await;
            callback.on_thinking_start(state.step_count).await;

            // Initialize thinking fallback state with current level
            let initial_level = self.thinker.current_think_level();
            let mut fallback_state = ThinkingFallbackState::new(initial_level);

            let thinking = loop {
                let current_level = fallback_state.current;
                let result = self
                    .thinker
                    .think_with_level(&state, &tools, current_level)
                    .await;

                match result {
                    Ok(t) => break t,
                    Err(e) => {
                        let error_msg = e.to_string();

                        // Check if this is a thinking level error and fallback is enabled
                        if self.config.enable_thinking_fallback
                            && is_thinking_level_error(&error_msg)
                        {
                            // Try to fallback to a lower thinking level
                            if let Some(next_level) = fallback_state.try_fallback(Some(&error_msg))
                            {
                                tracing::info!(
                                    from_level = %current_level,
                                    to_level = %next_level,
                                    error = %error_msg,
                                    "Thinking level not supported, falling back"
                                );
                                // Continue the loop with the new level
                                continue;
                            }
                        }

                        // Either fallback is disabled, not a thinking error, or exhausted
                        let reason = format!("Thinking failed: {}", e);
                        callback.on_failed(&reason).await;
                        return LoopResult::Failed {
                            reason,
                            steps: state.step_count,
                        };
                    }
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
                Decision::AskUserMultigroup { question, groups } => {
                    let response = callback
                        .on_user_multigroup_required(question, groups)
                        .await;

                    // Record user interaction as a step
                    let step = LoopStep {
                        step_id: state.step_count,
                        observation_summary: String::new(),
                        thinking: thinking.clone(),
                        action: Action::UserInteractionMultigroup {
                            question: question.clone(),
                            groups: groups.clone(),
                        },
                        result: ActionResult::UserResponse { response },
                        tokens_used: 0,
                        duration_ms: 0,
                    };
                    state.record_step(step);
                    guard.record_action("ask_user_multigroup");
                    continue;
                }
                Decision::AskUserRich { question, kind, question_id } => {
                    let answer = callback
                        .on_user_question(question, kind)
                        .await;

                    // Record user interaction as a step
                    let step = LoopStep {
                        step_id: state.step_count,
                        observation_summary: String::new(),
                        thinking: thinking.clone(),
                        action: Action::UserInteractionRich {
                            question: question.clone(),
                            kind: kind.clone(),
                            question_id: question_id.clone(),
                        },
                        result: ActionResult::UserResponseRich { response: answer },
                        tokens_used: 0,
                        duration_ms: 0,
                    };
                    state.record_step(step);
                    guard.record_action("ask_user_rich");
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

                    // ===== SWARM EVENT: Decision Made =====
                    // Publish decision event to swarm coordinator (shadow mode)
                    if let Some(ref swarm) = self.swarm_coordinator {
                        let affected_files = extract_affected_files(arguments);
                        swarm
                            .publish_event(AgentLoopEvent::DecisionMade {
                                agent_id: state.session_id.clone(),
                                decision: format!("Using {} to accomplish task", tool_name),
                                affected_files,
                            })
                            .await;
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
                Decision::Silent => {
                    // Silent response - nothing to report to the user
                    callback.on_complete("[silent]").await;
                    self.compaction_trigger
                        .emit_loop_stop(StopReason::Completed)
                        .await;
                    return LoopResult::Completed {
                        summary: "[silent]".to_string(),
                        steps: state.step_count,
                        total_tokens: state.total_tokens,
                    };
                }
                Decision::HeartbeatOk => {
                    // Heartbeat acknowledgment - background task alive
                    callback.on_complete("[heartbeat_ok]").await;
                    self.compaction_trigger
                        .emit_loop_stop(StopReason::Completed)
                        .await;
                    return LoopResult::Completed {
                        summary: "[heartbeat_ok]".to_string(),
                        steps: state.step_count,
                        total_tokens: state.total_tokens,
                    };
                }
            };

            // ===== Execute =====
            callback.on_action_start(&action).await;

            // ===== SWARM EVENT: Action Initiated =====
            // Publish action initiated event to swarm coordinator (shadow mode)
            if let Some(ref swarm) = self.swarm_coordinator {
                if let Action::ToolCall { tool_name, .. } = &action {
                    swarm
                        .publish_event(AgentLoopEvent::ActionInitiated {
                            agent_id: state.session_id.clone(),
                            action_type: action.action_type(),
                            target: Some(tool_name.clone()),
                        })
                        .await;
                }
            }

            let start_time = std::time::Instant::now();
            let started_at = chrono::Utc::now().timestamp_millis();
            let result = self.executor.execute(&action, &identity).await;
            let duration_ms = start_time.elapsed().as_millis() as u64;
            let completed_at = chrono::Utc::now().timestamp_millis();

            callback.on_action_done(&action, &result).await;

            // ===== SWARM EVENT: Action Completed =====
            // Publish action completed event to swarm coordinator (shadow mode)
            if let Some(ref swarm) = self.swarm_coordinator {
                swarm
                    .publish_event(AgentLoopEvent::ActionCompleted {
                        agent_id: state.session_id.clone(),
                        action_type: action.action_type(),
                        result: result.clone(),
                        duration_ms,
                    })
                    .await;
            }

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
