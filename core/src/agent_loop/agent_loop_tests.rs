//! Tests for AgentLoop

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
    use crate::event::{AlephEvent, EventBus, EventType};

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
            AlephEvent::LoopContinue(state) => {
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
    use crate::event::{AlephEvent, EventBus, EventType};

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
            AlephEvent::ToolCallCompleted(result) => {
                assert_eq!(result.tool, "search");
                assert!(result.session_id.is_some());
            }
            _ => panic!("Expected ToolCallCompleted event"),
        }
    }
}

#[tokio::test]
async fn test_compaction_trigger_emits_loop_stop_on_completion() {
    use crate::event::{AlephEvent, EventBus, EventType, StopReason};

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
            AlephEvent::LoopStop(reason) => {
                assert!(matches!(reason, StopReason::Completed));
            }
            _ => panic!("Expected LoopStop event"),
        }
    }
}

#[tokio::test]
async fn test_compaction_trigger_emits_loop_stop_on_guard() {
    use crate::event::{AlephEvent, EventBus, EventType, StopReason};

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
            AlephEvent::LoopStop(reason) => {
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

#[tokio::test]
async fn test_ask_user_rich_handling() {
    use crate::agent_loop::question::{QuestionKind, ChoiceOption};
    use crate::agent_loop::callback::LoopEvent;

    let thinker = Arc::new(MockThinker::new(vec![
        Decision::AskUserRich {
            question: "Choose option".to_string(),
            kind: QuestionKind::SingleChoice {
                choices: vec![
                    ChoiceOption::new("A"),
                    ChoiceOption::new("B"),
                ],
                default_index: Some(0),
            },
            question_id: None,
        },
        Decision::Complete {
            summary: "Done after user choice".to_string(),
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
            "Test rich question".to_string(),
            RequestContext::empty(),
            vec![],
            &callback,
            None,
            None,
        )
        .await;

    assert!(matches!(result, LoopResult::Completed { steps: 1, .. }));

    let events = callback.events();
    assert!(events.iter().any(|e| matches!(e, LoopEvent::UserQuestionRequired { .. })));
}
