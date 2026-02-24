//! Agent loop test context

use alephcore::agent_loop::{
    callback::LoopEvent,
    overflow::{ModelLimit, OverflowConfig, OverflowDetector},
    question::QuestionKind,
    GuardViolation, LoopConfig, LoopResult,
};
use alephcore::event::{EventBus, EventSubscriber, EventType, StopReason, TimestampedEvent};
use std::sync::Arc;

/// Agent loop test context
/// Stores state for BDD scenario execution
#[derive(Default)]
pub struct AgentLoopContext {
    // ═══ Loop Configuration ═══
    /// Mock thinker decision sequence
    pub decision_sequence: Vec<MockDecision>,
    /// Current decision index
    pub decision_index: usize,
    /// Loop configuration override
    pub loop_config: Option<LoopConfig>,

    // ═══ Results ═══
    /// Result of the loop execution
    pub loop_result: Option<LoopResult>,
    /// Collected callback events
    pub events: Vec<LoopEvent>,
    /// Guard violation (if any)
    pub guard_violation: Option<GuardViolation>,

    // ═══ Event Bus ═══
    /// Event bus for compaction trigger tests
    pub event_bus: Option<Arc<EventBus>>,
    /// Event subscriber for filtered events
    pub event_subscriber: Option<EventSubscriber>,
    /// Collected bus events
    pub bus_events: Vec<TimestampedEvent>,

    // ═══ Overflow Detection ═══
    /// Overflow detector for unified session tests
    pub overflow_detector: Option<Arc<OverflowDetector>>,
    /// Simulated token usage
    pub token_usage: usize,
    /// Result of should_compact check
    pub should_compact: Option<bool>,
    /// Result of is_overflow check
    pub is_overflow: Option<bool>,
    /// Result of usage_percent check
    pub usage_percent: Option<u8>,
}

impl std::fmt::Debug for AgentLoopContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentLoopContext")
            .field("decision_sequence_len", &self.decision_sequence.len())
            .field("decision_index", &self.decision_index)
            .field(
                "loop_config",
                &self.loop_config.as_ref().map(|_| "<LoopConfig>"),
            )
            .field("loop_result", &self.loop_result)
            .field("events_count", &self.events.len())
            .field("guard_violation", &self.guard_violation)
            .field("event_bus", &self.event_bus.as_ref().map(|_| "<EventBus>"))
            .field("bus_events_count", &self.bus_events.len())
            .field(
                "overflow_detector",
                &self
                    .overflow_detector
                    .as_ref()
                    .map(|_| "<OverflowDetector>"),
            )
            .field("token_usage", &self.token_usage)
            .field("should_compact", &self.should_compact)
            .field("is_overflow", &self.is_overflow)
            .field("usage_percent", &self.usage_percent)
            .finish()
    }
}

/// Mock decision for test scenarios
#[derive(Debug, Clone)]
pub enum MockDecision {
    /// Complete with summary
    Complete { summary: String },
    /// Use a tool
    UseTool {
        tool_name: String,
        arguments: serde_json::Value,
    },
    /// Ask user a rich question
    AskUserRich {
        question: String,
        kind: QuestionKind,
    },
    /// Fail with reason
    Fail { reason: String },
}

impl AgentLoopContext {
    /// Create a new context with default values
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a decision to the sequence
    pub fn add_decision(&mut self, decision: MockDecision) {
        self.decision_sequence.push(decision);
    }

    /// Set up event bus with filtered subscription
    pub fn setup_event_bus(&mut self, event_types: Vec<EventType>) {
        let bus = Arc::new(EventBus::new());
        self.event_subscriber = Some(bus.subscribe_filtered(event_types));
        self.event_bus = Some(bus);
    }

    /// Set up overflow detector with specified limits
    pub fn setup_overflow_detector(
        &mut self,
        context_limit: usize,
        output_limit: usize,
        reserve: f32,
    ) {
        let config = OverflowConfig {
            default_limit: ModelLimit::new(context_limit as u64, output_limit as u64, reserve),
            ..OverflowConfig::default()
        };
        self.overflow_detector = Some(Arc::new(OverflowDetector::new(config)));
    }

    /// Collect events from subscriber
    pub async fn collect_bus_events(&mut self) {
        if let Some(subscriber) = &mut self.event_subscriber {
            while let Ok(Some(event)) = subscriber.try_recv() {
                self.bus_events.push(event);
            }
        }
    }

    /// Check if a specific StopReason was emitted
    pub fn has_stop_reason(&self, expected: &StopReason) -> bool {
        use alephcore::event::AlephEvent;
        self.bus_events.iter().any(|e| {
            matches!(&e.event, AlephEvent::LoopStop(reason) if std::mem::discriminant(reason) == std::mem::discriminant(expected))
        })
    }

    /// Check if LoopContinue event was emitted
    pub fn has_loop_continue(&self) -> bool {
        use alephcore::event::AlephEvent;
        self.bus_events
            .iter()
            .any(|e| matches!(&e.event, AlephEvent::LoopContinue(_)))
    }

    /// Check if ToolCallCompleted event was emitted for a specific tool
    pub fn has_tool_completed(&self, tool_name: &str) -> bool {
        use alephcore::event::AlephEvent;
        self.bus_events.iter().any(|e| {
            matches!(&e.event, AlephEvent::ToolCallCompleted(result) if result.tool == tool_name)
        })
    }
}
