//! Agent Loop implementation - Main execution controller

use crate::sync_primitives::Arc;
use tokio::sync::watch;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};

use aleph_protocol::IdentityContext;
use crate::agents::thinking::{is_thinking_level_error, ThinkingFallbackState};
use crate::event::{EventBus, StopReason};

use super::callback::LoopCallback;
use super::compaction_trigger::OptionalCompactionTrigger;
use super::config::LoopConfig;
use super::decision::{Action, ActionResult, Decision};
use super::guards::{GuardViolation, LoopGuard};
use super::interrupt;
use super::loop_result::LoopResult;
use super::overflow::OverflowDetector;
use super::session_sync::SessionSync;
use super::state::{LoopState, LoopStep, RequestContext};
use super::traits::{ActionExecutor, CompressorTrait, ThinkerTrait};
use super::events::AgentLoopEvent;
use crate::compressor::PreCompactionPrompt;
use crate::poe::StepDirective;

static FIRST_CYCLE_LOGGED: AtomicBool = AtomicBool::new(false);

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
    /// Optional interrupt receiver for soft steering (new user messages)
    pub interrupt_rx: Option<interrupt::InterruptReceiver>,
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
            interrupt_rx: None,
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

    /// Set the interrupt receiver for soft steering
    pub fn with_interrupt_rx(mut self, rx: interrupt::InterruptReceiver) -> Self {
        self.interrupt_rx = Some(rx);
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
    pub(crate) swarm_coordinator: Option<Arc<crate::agents::swarm::coordinator::SwarmCoordinator>>,
    /// Optional task router for dynamic escalation
    pub(crate) task_router: Option<Arc<dyn crate::routing::TaskRouter>>,
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
            task_router: None,
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
            task_router: None,
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
            task_router: None,
        }
    }

    /// Create a new AgentLoop from builder components
    ///
    /// This is used by `AgentLoopBuilder` to construct a fully configured instance.
    pub(crate) fn from_builder(
        thinker: Arc<T>,
        executor: Arc<E>,
        compressor: Arc<C>,
        config: LoopConfig,
        event_bus: Option<Arc<EventBus>>,
        overflow_detector: Option<Arc<OverflowDetector>>,
        swarm_coordinator: Option<Arc<crate::agents::swarm::coordinator::SwarmCoordinator>>,
    ) -> Self {
        Self {
            thinker,
            executor,
            compressor,
            config,
            compaction_trigger: OptionalCompactionTrigger::new(event_bus),
            overflow_detector,
            swarm_coordinator,
            task_router: None,
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

    /// Set a task router for dynamic escalation checking.
    pub fn with_task_router(mut self, router: Arc<dyn crate::routing::TaskRouter>) -> Self {
        self.task_router = Some(router);
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
            interrupt_rx,
        } = run_context;

        // Extract interrupt receiver as mutable local (similar to abort_signal)
        let mut interrupt_rx = interrupt_rx;

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

        tracing::info!(
            subsystem = "agent_loop",
            event = "initialized",
            session_id = %state.session_id,
            "agent loop initialized"
        );

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
                    tracing::info!(
                        subsystem = "agent_loop",
                        event = "session_completed",
                        session_id = %state.session_id,
                        result = "user_aborted",
                        steps = state.step_count,
                        "agent loop session ended"
                    );
                    return LoopResult::UserAborted;
                }
            }

            if !FIRST_CYCLE_LOGGED.swap(true, AtomicOrdering::Relaxed) {
                tracing::info!(
                    subsystem = "agent_loop",
                    event = "first_cycle_started",
                    session_id = %state.session_id,
                    "agent loop entered first execution cycle"
                );
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
                tracing::info!(
                    subsystem = "agent_loop",
                    event = "session_completed",
                    session_id = %state.session_id,
                    result = "guard_triggered",
                    steps = state.step_count,
                    "agent loop session ended"
                );
                return LoopResult::GuardTriggered(violation);
            }

            // ===== Escalation Check =====
            if let Some(ref router) = self.task_router {
                if !state.escalation_checked {
                    let esc_ctx = crate::routing::EscalationContext {
                        step_count: state.step_count,
                        tools_invoked: state.tools_used(),
                        has_failures: state.has_failures(),
                        original_message: state.original_input().to_string(),
                    };
                    if let Some(route) = router.should_escalate(&esc_ctx).await {
                        state.escalation_checked = true;
                        tracing::info!(
                            subsystem = "task_router",
                            event = "escalation_triggered",
                            route = route.label(),
                            steps = state.step_count,
                            "task router triggered dynamic escalation"
                        );
                        let snapshot = crate::routing::EscalationSnapshot {
                            original_message: state.original_input().to_string(),
                            completed_steps: state.step_count,
                            tools_invoked: state.tools_used(),
                            partial_result: state.last_result_summary(),
                        };
                        return LoopResult::Escalated { route, context: snapshot };
                    }
                }
            }

            // ===== Pre-Compaction Memory Flush + Compression =====
            if self.compressor.should_compress(&state) {
                if !state.silent_mode {
                    // First pass: inject a silent turn to flush memories before compacting.
                    // The agent will process the flush prompt, use memory_store to persist
                    // important information, then we come back here with silent_mode == true
                    // to perform the actual compression.
                    let flush_prompt = PreCompactionPrompt::build();
                    tracing::info!("Pre-compaction: injecting silent memory flush turn");
                    state.original_request = flush_prompt;
                    state.silent_mode = true;
                    continue; // Re-enter loop to process the flush prompt
                }

                // Second pass: silent turn completed, perform actual compression
                state.silent_mode = false;

                match self
                    .compressor
                    .compress(&state.steps, &state.history_summary)
                    .await
                {
                    Ok(compressed) => {
                        let until = state.steps.len().saturating_sub(self.config.compression.recent_window_size);
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
                        tracing::info!(
                            subsystem = "agent_loop",
                            event = "session_completed",
                            session_id = %state.session_id,
                            result = "thinking_failed",
                            steps = state.step_count,
                            "agent loop session ended"
                        );
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
                    // ===== POE Lazy Validation: Check before completing =====
                    if let Some(hint) = callback.on_validate_completion(summary, &state).await {
                        tracing::info!(
                            session_id = %state.session_id,
                            hint = %hint,
                            "POE lazy validation failed at completion, retrying"
                        );
                        state.set_poe_hint(hint);
                        continue; // Retry the loop with the hint
                    }

                    callback.on_complete(summary).await;
                    // ===== COMPACTION TRIGGER: Session End (Completed) =====
                    self.compaction_trigger
                        .emit_loop_stop(StopReason::Completed)
                        .await;
                    tracing::info!(
                        subsystem = "agent_loop",
                        event = "session_completed",
                        session_id = %state.session_id,
                        result = "completed",
                        steps = state.step_count,
                        total_tokens = state.total_tokens,
                        "agent loop session ended"
                    );
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
                    tracing::info!(
                        subsystem = "agent_loop",
                        event = "session_completed",
                        session_id = %state.session_id,
                        result = "decision_failed",
                        steps = state.step_count,
                        "agent loop session ended"
                    );
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
                Decision::UseTools { calls: ref records } => {
                    // For now, take first record for sequential compatibility
                    // (Parallel execution comes in Batch D)
                    if let Some(record) = records.first() {
                        let tool_name = &record.tool_name;
                        let arguments = &record.arguments;
                        let call_id = &record.call_id;

                        // Record tool call for doom loop detection BEFORE checking
                        guard.record_tool_call(tool_name, arguments);

                        // Check for doom loop (exact same tool + arguments repeated)
                        if let Some(violation @ GuardViolation::DoomLoop { .. }) = guard.check(&state)
                        {
                            // Extract fields from the violation for callback use
                            let (doom_tool, repeat_count) = match &violation {
                                GuardViolation::DoomLoop { tool_name: t, repeat_count: r, .. } => (t.clone(), *r),
                                _ => unreachable!(),
                            };

                            // Ask user if they want to continue
                            let should_continue = callback
                                .on_doom_loop_detected(&doom_tool, arguments, repeat_count)
                                .await;

                            if should_continue {
                                // User wants to continue - reset detection and proceed
                                guard.reset_doom_loop_detection();
                            } else {
                                // User doesn't want to continue - trigger guard
                                let final_violation = GuardViolation::DoomLoop {
                                    tool_name: doom_tool,
                                    repeat_count,
                                    arguments_preview: serde_json::to_string(arguments)
                                        .unwrap_or_default()
                                        .chars()
                                        .take(100)
                                        .collect(),
                                };
                                callback.on_guard_triggered(&final_violation).await;
                                // ===== COMPACTION TRIGGER: Session End (Doom Loop) =====
                                self.compaction_trigger
                                    .emit_loop_stop(StopReason::DoomLoopDetected)
                                    .await;
                                tracing::info!(
                                    subsystem = "agent_loop",
                                    event = "session_completed",
                                    session_id = %state.session_id,
                                    result = "doom_loop",
                                    steps = state.step_count,
                                    "agent loop session ended"
                                );
                                return LoopResult::GuardTriggered(final_violation);
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
                                use super::decision::{ToolCallRequest, ToolCallResult, SingleToolResult};
                                let step = LoopStep {
                                    step_id: state.step_count,
                                    observation_summary: String::new(),
                                    thinking: thinking.clone(),
                                    action: Action::ToolCalls { calls: vec![ToolCallRequest {
                                        call_id: call_id.clone(),
                                        tool_name: tool_name.clone(),
                                        arguments: arguments.clone(),
                                    }]},
                                    result: ActionResult::ToolResults { results: vec![ToolCallResult {
                                        call_id: call_id.clone(),
                                        tool_name: tool_name.clone(),
                                        result: SingleToolResult::Error {
                                            error: "User cancelled".to_string(),
                                            retryable: false,
                                        },
                                    }]},
                                    tokens_used: 0,
                                    duration_ms: 0,
                                };
                                state.record_step(step);
                                guard.record_action(&format!("cancelled:{}", tool_name));
                                continue;
                            }
                        }

                        Action::ToolCalls {
                            calls: records
                                .iter()
                                .map(|r| super::decision::ToolCallRequest {
                                    call_id: r.call_id.clone(),
                                    tool_name: r.tool_name.clone(),
                                    arguments: r.arguments.clone(),
                                })
                                .collect(),
                        }
                    } else {
                        // Empty tool call batch — skip
                        continue;
                    }
                }
                Decision::Silent => {
                    // Silent response - nothing to report to the user
                    callback.on_complete("[silent]").await;
                    self.compaction_trigger
                        .emit_loop_stop(StopReason::Completed)
                        .await;
                    tracing::info!(
                        subsystem = "agent_loop",
                        event = "session_completed",
                        session_id = %state.session_id,
                        result = "silent",
                        steps = state.step_count,
                        total_tokens = state.total_tokens,
                        "agent loop session ended"
                    );
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
                    tracing::info!(
                        subsystem = "agent_loop",
                        event = "session_completed",
                        session_id = %state.session_id,
                        result = "heartbeat_ok",
                        steps = state.step_count,
                        total_tokens = state.total_tokens,
                        "agent loop session ended"
                    );
                    return LoopResult::Completed {
                        summary: "[heartbeat_ok]".to_string(),
                        steps: state.step_count,
                        total_tokens: state.total_tokens,
                    };
                }
            };

            // ===== Interrupt Check (before execution) =====
            // Check for steering interrupt before executing the action.
            // Unlike abort (which terminates the loop), interrupt redirects
            // by cancelling the current action and injecting new user input.
            if let Some(ref mut irx) = interrupt_rx {
                if let Some(interrupt::InterruptSignal::NewMessage { content }) = irx.try_recv() {
                    tracing::info!(
                        subsystem = "agent_loop",
                        event = "interrupt_received",
                        session_id = %state.session_id,
                        step = state.step_count,
                        "steering interrupt received, cancelling pending action"
                    );
                    state.record_interrupted_action(&action, &thinking, &content);
                    callback.on_action_cancelled(&action, "interrupted by new user input").await;
                    continue; // Back to top of loop — will re-think with new context
                }
            }

            // ===== Execute =====
            callback.on_action_start(&action).await;

            // ===== SWARM EVENT: Action Initiated =====
            // Publish action initiated event to swarm coordinator (shadow mode)
            if let Some(ref swarm) = self.swarm_coordinator {
                if let Action::ToolCalls { calls: ref requests } = &action {
                    if let Some(req) = requests.first() {
                        swarm
                            .publish_event(AgentLoopEvent::ActionInitiated {
                                agent_id: state.session_id.clone(),
                                action_type: action.action_type(),
                                target: Some(req.tool_name.clone()),
                            })
                            .await;
                    }
                }
            }

            let start_time = std::time::Instant::now();
            let started_at = chrono::Utc::now().timestamp_millis();
            let result = self.executor.execute(&action, &identity).await;
            let duration_ms = start_time.elapsed().as_millis() as u64;
            let completed_at = chrono::Utc::now().timestamp_millis();

            callback.on_action_done(&action, &result).await;

            // ===== Escalate Task Tool Detection =====
            if let Action::ToolCalls { calls: ref requests } = &action {
                if let Some(req) = requests.first() {
                    let tool_name = &req.tool_name;
                    let arguments = &req.arguments;
                    if tool_name == "escalate_task" && result.is_success() {
                        let output_str = result.full_output();
                        if output_str.contains("accepted") {
                            let target = arguments
                                .get("target")
                                .and_then(|v: &serde_json::Value| v.as_str())
                                .unwrap_or("multi_step");
                            let reason = arguments
                                .get("reason")
                                .and_then(|v: &serde_json::Value| v.as_str())
                                .unwrap_or("LLM requested escalation")
                                .to_string();

                            let route = match target {
                                "critical" => crate::routing::TaskRoute::Critical {
                                    reason,
                                    manifest_hints: crate::routing::ManifestHints::default(),
                                },
                                "collaborative" => crate::routing::TaskRoute::Collaborative {
                                    reason,
                                    strategy: crate::routing::CollabStrategy::Parallel,
                                },
                                _ => crate::routing::TaskRoute::MultiStep { reason },
                            };

                            tracing::info!(
                                subsystem = "task_router",
                                event = "llm_escalation",
                                route = route.label(),
                                "LLM self-escalated via escalate_task tool"
                            );

                            let snapshot = crate::routing::EscalationSnapshot {
                                original_message: state.original_input().to_string(),
                                completed_steps: state.step_count,
                                tools_invoked: state.tools_used(),
                                partial_result: state.last_result_summary(),
                            };
                            return LoopResult::Escalated { route, context: snapshot };
                        }
                    }
                }
            }

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
            if let Action::ToolCalls { calls: ref requests } = &action {
                if let Some(req) = requests.first() {
                    let call_id = req.call_id.clone();
                    let tool_name = &req.tool_name;
                    let arguments = &req.arguments;
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
            }

            // ===== POE Interceptor: Evaluate Step =====
            let step_snapshot = LoopStep {
                step_id: state.step_count,
                observation_summary: String::new(),
                thinking: thinking.clone(),
                action: action.clone(),
                result: result.clone(),
                tokens_used: 0,
                duration_ms,
            };
            let directive = callback.on_step_evaluate(&step_snapshot, &state).await;

            match directive {
                StepDirective::Continue => { /* no intervention */ }
                StepDirective::ContinueWithHint { hint } => {
                    state.set_poe_hint(hint);
                }
                StepDirective::SuggestStrategySwitch { reason, suggestion } => {
                    let violation = GuardViolation::PoeStrategySwitch {
                        reason: reason.clone(),
                        suggestion: suggestion.clone(),
                    };
                    callback.on_guard_triggered(&violation).await;
                    self.compaction_trigger
                        .emit_loop_stop(StopReason::Error(reason))
                        .await;
                    tracing::info!(
                        subsystem = "agent_loop",
                        event = "session_completed",
                        session_id = %state.session_id,
                        result = "poe_strategy_switch",
                        steps = state.step_count,
                        "agent loop session ended"
                    );
                    return LoopResult::GuardTriggered(violation);
                }
                StepDirective::Abort { reason } => {
                    callback.on_aborted().await;
                    self.compaction_trigger
                        .emit_loop_stop(StopReason::Error(reason.clone()))
                        .await;
                    tracing::info!(
                        subsystem = "agent_loop",
                        event = "session_completed",
                        session_id = %state.session_id,
                        result = "poe_aborted",
                        steps = state.step_count,
                        "agent loop session ended"
                    );
                    return LoopResult::PoeAborted { reason };
                }
            }

            // ===== Feedback (Update State) =====
            guard.record_action(&action.action_type());

            let step = LoopStep {
                step_id: state.step_count,
                observation_summary: String::new(), // Will be filled by compressor
                tokens_used: thinking.tokens_used.unwrap_or(0),
                thinking,
                action,
                result,
                duration_ms,
            };
            state.record_step(step);

            // Increment iteration counter
            iteration += 1;
        }
    }
}
