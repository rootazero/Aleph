//! Agent Loop implementation - Main execution controller

use std::sync::Arc;
use tokio::sync::watch;

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
                        .on_user_multigroup_required(question, &groups)
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
                    // TODO: Implement rich user question handling in Task 4.1
                    // For now, fall back to plain text response
                    let response = callback
                        .on_user_input_required(question, None)
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
                        result: ActionResult::UserResponse { response },
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

    use crate::agent_loop::callback::{CollectingCallback, LoopEvent, NoOpLoopCallback};
    use crate::agent_loop::config::LoopConfig;
    use crate::agent_loop::state::Thinking;
    use crate::error::Result;

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
        ) -> Result<super::super::traits::CompressedHistory> {
            Ok(super::super::traits::CompressedHistory {
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
        use super::super::compaction_trigger::OptionalCompactionTrigger;

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
        use super::super::compaction_trigger::CompactionTrigger;
        use crate::event::EventBus;

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

    // ========================================================================
    // Unified Session Model / Overflow Detector Tests
    // ========================================================================

    #[tokio::test]
    async fn test_agent_loop_with_overflow_detector() {
        use crate::agent_loop::overflow::{OverflowConfig, OverflowDetector};

        // Create an overflow detector with a small limit for testing
        let mut config = OverflowConfig::default();
        config.default_limit = crate::agent_loop::overflow::ModelLimit::new(
            100_000,  // 100K context
            4_000,    // 4K output
            0.2,      // 20% reserve
        );
        let detector = Arc::new(OverflowDetector::new(config));

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

        // Create loop with overflow detector using with_unified_session
        let loop_config = LoopConfig::for_testing().with_realtime_overflow(true);
        let agent_loop = AgentLoop::with_unified_session(
            thinker,
            executor,
            compressor,
            loop_config,
            None, // No EventBus
            Some(detector),
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

        // Verify it doesn't crash and runs correctly
        assert!(result.is_success());
        if let LoopResult::Completed { steps, .. } = result {
            assert_eq!(steps, 1); // One tool call before completion
        }
    }

    #[test]
    fn test_should_compact_unified() {
        use crate::agent_loop::overflow::{ModelLimit, OverflowConfig, OverflowDetector};

        // Create a detector with small limits for testing
        let mut config = OverflowConfig::default();
        config.default_limit = ModelLimit::new(
            10_000,  // 10K context
            1_000,   // 1K output
            0.1,     // 10% reserve
        );
        // Usable tokens: (10000 - 1000) * 0.9 = 8100
        let detector = Arc::new(OverflowDetector::new(config));

        let thinker = Arc::new(MockThinker::new(vec![]));
        let executor = Arc::new(MockExecutor);
        let compressor = Arc::new(MockCompressor);

        // Test with realtime overflow enabled
        let loop_config = LoopConfig::for_testing().with_realtime_overflow(true);
        let agent_loop = AgentLoop::with_unified_session(
            thinker.clone(),
            executor.clone(),
            compressor.clone(),
            loop_config,
            None,
            Some(detector.clone()),
        );

        // Create a state with moderate token usage (below 85%)
        let mut state = LoopState::new(
            "test-session".to_string(),
            "Test request".to_string(),
            RequestContext::empty(),
        );
        state.total_tokens = 4000; // ~49% of 8100, below 85% threshold
        assert!(!agent_loop.should_compact_unified(&state));

        // Create a state with high token usage (above 85%)
        state.total_tokens = 7000; // ~86% of 8100, above 85% threshold
        assert!(agent_loop.should_compact_unified(&state));

        // Test with realtime overflow disabled
        let loop_config_disabled = LoopConfig::for_testing().with_realtime_overflow(false);
        let agent_loop_disabled = AgentLoop::with_unified_session(
            thinker.clone(),
            executor.clone(),
            compressor.clone(),
            loop_config_disabled,
            None,
            Some(detector.clone()),
        );
        // Should return false even with high tokens when disabled
        assert!(!agent_loop_disabled.should_compact_unified(&state));

        // Test without overflow detector
        let agent_loop_no_detector = AgentLoop::with_unified_session(
            thinker,
            executor,
            compressor,
            LoopConfig::for_testing().with_realtime_overflow(true),
            None,
            None, // No detector
        );
        assert!(!agent_loop_no_detector.should_compact_unified(&state));
    }

    #[test]
    fn test_is_overflow() {
        use crate::agent_loop::overflow::{ModelLimit, OverflowConfig, OverflowDetector};

        // Create a detector with small limits for testing
        let mut config = OverflowConfig::default();
        config.default_limit = ModelLimit::new(
            10_000,  // 10K context
            1_000,   // 1K output
            0.1,     // 10% reserve
        );
        // Usable tokens: (10000 - 1000) * 0.9 = 8100
        let detector = Arc::new(OverflowDetector::new(config));

        let thinker = Arc::new(MockThinker::new(vec![]));
        let executor = Arc::new(MockExecutor);
        let compressor = Arc::new(MockCompressor);

        let loop_config = LoopConfig::for_testing().with_realtime_overflow(true);
        let agent_loop = AgentLoop::with_unified_session(
            thinker,
            executor,
            compressor,
            loop_config,
            None,
            Some(detector),
        );

        // Below limit
        let mut state = LoopState::new(
            "test-session".to_string(),
            "Test request".to_string(),
            RequestContext::empty(),
        );
        state.total_tokens = 5000;
        assert!(!agent_loop.is_overflow(&state));

        // Above limit
        state.total_tokens = 9000;
        assert!(agent_loop.is_overflow(&state));
    }

    #[test]
    fn test_usage_percent() {
        use crate::agent_loop::overflow::{ModelLimit, OverflowConfig, OverflowDetector};

        // Create a detector with small limits for testing
        let mut config = OverflowConfig::default();
        config.default_limit = ModelLimit::new(
            10_000,  // 10K context
            1_000,   // 1K output
            0.1,     // 10% reserve
        );
        // Usable tokens: (10000 - 1000) * 0.9 = 8100
        let detector = Arc::new(OverflowDetector::new(config));

        let thinker = Arc::new(MockThinker::new(vec![]));
        let executor = Arc::new(MockExecutor);
        let compressor = Arc::new(MockCompressor);

        let loop_config = LoopConfig::for_testing().with_realtime_overflow(true);
        let agent_loop = AgentLoop::with_unified_session(
            thinker.clone(),
            executor.clone(),
            compressor.clone(),
            loop_config,
            None,
            Some(detector),
        );

        let mut state = LoopState::new(
            "test-session".to_string(),
            "Test request".to_string(),
            RequestContext::empty(),
        );

        // 50% usage
        state.total_tokens = 4050; // ~50% of 8100
        let percent = agent_loop.usage_percent(&state);
        assert_eq!(percent, 50);

        // Test without detector returns 0
        let agent_loop_no_detector = AgentLoop::with_unified_session(
            thinker,
            executor,
            compressor,
            LoopConfig::for_testing(),
            None,
            None,
        );
        assert_eq!(agent_loop_no_detector.usage_percent(&state), 0);
    }

    #[test]
    fn test_with_unified_session_constructor() {
        use crate::agent_loop::overflow::{OverflowConfig, OverflowDetector};
        use crate::event::EventBus;

        let thinker = Arc::new(MockThinker::new(vec![]));
        let executor = Arc::new(MockExecutor);
        let compressor = Arc::new(MockCompressor);

        // Test with both EventBus and OverflowDetector
        let event_bus = Arc::new(EventBus::new());
        let detector = Arc::new(OverflowDetector::new(OverflowConfig::default()));

        let loop_config = LoopConfig::for_testing()
            .with_unified_session(true)
            .with_message_builder(true)
            .with_realtime_overflow(true);

        let agent_loop = AgentLoop::with_unified_session(
            thinker.clone(),
            executor.clone(),
            compressor.clone(),
            loop_config.clone(),
            Some(event_bus),
            Some(detector),
        );

        // Verify config is properly set
        assert!(agent_loop.config.use_unified_session);
        assert!(agent_loop.config.use_message_builder);
        assert!(agent_loop.config.use_realtime_overflow);
        assert!(agent_loop.overflow_detector.is_some());

        // Test with None values
        let agent_loop_minimal = AgentLoop::with_unified_session(
            thinker,
            executor,
            compressor,
            loop_config,
            None,
            None,
        );
        assert!(agent_loop_minimal.overflow_detector.is_none());
    }
}
