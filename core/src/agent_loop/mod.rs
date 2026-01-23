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
//! # Components
//!
//! - **AgentLoop**: Main loop controller
//! - **LoopState**: Session state management
//! - **Decision**: LLM decision types
//! - **LoopGuard**: Safety guards
//! - **LoopCallback**: UI callback interface
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
pub mod state;

use std::sync::Arc;
use tokio::sync::watch;

pub use callback::{CollectingCallback, LoggingCallback, LoopCallback, LoopEvent, NoOpLoopCallback};
pub use config::{CompressionConfig, LoopConfig, ModelRoutingConfig};
pub use decision::{Action, ActionResult, Decision, LlmAction, LlmResponse};
pub use guards::{GuardViolation, LoopGuard};
pub use state::{LoopState, LoopStep, Observation, RequestContext, StepSummary, Thinking, ToolInfo};

use crate::error::Result;

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

        loop {
            // Check abort signal
            if let Some(ref abort) = abort_signal {
                if *abort.borrow() {
                    callback.on_aborted().await;
                    return LoopResult::UserAborted;
                }
            }

            // ===== Guard Check =====
            if let Some(violation) = guard.check(&state) {
                callback.on_guard_triggered(&violation).await;
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
                    return LoopResult::Completed {
                        summary: summary.clone(),
                        steps: state.step_count,
                        total_tokens: state.total_tokens,
                    };
                }
                Decision::Fail { reason } => {
                    callback.on_failed(reason).await;
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
            let result = self.executor.execute(&action).await;
            let duration_ms = start_time.elapsed().as_millis() as u64;

            callback.on_action_done(&action, &result).await;

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
}
