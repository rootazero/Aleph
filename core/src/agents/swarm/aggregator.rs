//! Semantic Aggregator
//!
//! Implements "fast reflex + slow thinking" dual-loop control.
//! Transforms low-level events into high-level situational awareness.

use std::collections::VecDeque;
use crate::sync_primitives::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::events::{AgentEvent, ImportantEvent, InfoEvent};
use super::bus::AgentMessageBus;
use super::rules::{AggregationRule, RuleEngine};
use crate::error::Result;

/// Semantic Aggregator
///
/// Combines fast rule-based aggregation with slow LLM-powered summarization.
pub struct SemanticAggregator {
    /// Fast path: rule engine (microsecond-level)
    rule_engine: RuleEngine,
    /// Intelligence path: async summarizer (second-level)
    intelligence_layer: Option<Arc<IntelligenceLayer>>,
    /// Sliding window: cache recent events
    event_window: Arc<RwLock<SlidingWindow>>,
    /// Reference to message bus
    bus: Arc<AgentMessageBus>,
}

/// Sliding window for event buffering
pub struct SlidingWindow {
    max_size: usize,
    events: VecDeque<InfoEvent>,
}

impl SlidingWindow {
    pub fn new(max_size: usize) -> Self {
        Self {
            max_size,
            events: VecDeque::with_capacity(max_size),
        }
    }

    pub fn push(&mut self, event: InfoEvent) {
        if self.events.len() >= self.max_size {
            self.events.pop_front();
        }
        self.events.push_back(event);
    }

    pub fn get_recent(&self, count: usize) -> Vec<&InfoEvent> {
        self.events
            .iter()
            .rev()
            .take(count)
            .collect()
    }

}

impl SemanticAggregator {
    /// Create a new semantic aggregator
    pub fn new(bus: Arc<AgentMessageBus>) -> Self {
        Self {
            rule_engine: RuleEngine::with_default_rules(),
            intelligence_layer: None,
            event_window: Arc::new(RwLock::new(SlidingWindow::new(1000))),
            bus,
        }
    }

    /// Create aggregator with custom configuration
    pub fn with_config(
        bus: Arc<AgentMessageBus>,
        rules: Vec<AggregationRule>,
        window_size: usize,
    ) -> Self {
        Self {
            rule_engine: RuleEngine::new(rules),
            intelligence_layer: None,
            event_window: Arc::new(RwLock::new(SlidingWindow::new(window_size))),
            bus,
        }
    }

    /// Enable intelligence layer with LLM summarization
    pub fn with_intelligence_layer(mut self, layer: Arc<IntelligenceLayer>) -> Self {
        self.intelligence_layer = Some(layer);
        self
    }

    /// Process an info event through the aggregation pipeline
    pub async fn process_event(&self, event: InfoEvent) -> Result<()> {
        // 1. Add to sliding window
        {
            let mut window = self.event_window.write().await;
            window.push(event.clone());
        }

        // 2. Try fast path (rule engine)
        if let Some(aggregated) = self.rule_engine.try_aggregate(&event, &self.event_window).await {
            debug!("Rule engine aggregated event: {:?}", aggregated);
            self.bus.publish(AgentEvent::Important(aggregated)).await?;
        }

        Ok(())
    }

    /// Run the aggregator background loop
    pub async fn run(self: Arc<Self>) {
        info!("Starting semantic aggregator");

        // Subscribe to Info events
        let mut info_rx = match self.bus.subscribe(super::events::EventTier::Info).await {
            Ok(rx) => rx,
            Err(e) => {
                warn!("Failed to subscribe to Info events: {}", e);
                return;
            }
        };

        // Spawn intelligence layer if enabled
        if let Some(ref intelligence) = self.intelligence_layer {
            let intelligence_clone = intelligence.clone();
            let bus_clone = self.bus.clone();
            let window_clone = self.event_window.clone();

            tokio::spawn(async move {
                intelligence_clone.run(bus_clone, window_clone).await;
            });
        }

        // Main event processing loop
        loop {
            match info_rx.recv().await {
                Ok(AgentEvent::Info(event)) => {
                    if let Err(e) = self.process_event(event).await {
                        warn!("Failed to process event: {}", e);
                    }
                }
                Ok(_) => {
                    // Ignore non-Info events (shouldn't happen)
                }
                Err(e) => {
                    warn!("Error receiving event: {}", e);
                    break;
                }
            }
        }

        info!("Semantic aggregator stopped");
    }

    /// Get recent events from window
    pub async fn get_recent_events(&self, count: usize) -> Vec<InfoEvent> {
        let window = self.event_window.read().await;
        window.get_recent(count).into_iter().cloned().collect()
    }
}

/// Intelligence Layer for LLM-powered summarization
pub struct IntelligenceLayer {
    /// Interval between summaries
    summary_interval: Duration,
    /// Minimum events before summarizing
    min_events: usize,
}

impl IntelligenceLayer {
    /// Create a new intelligence layer
    pub fn new(summary_interval: Duration, min_events: usize) -> Self {
        Self {
            summary_interval,
            min_events,
        }
    }

    /// Run the intelligence layer background loop
    pub async fn run(
        &self,
        bus: Arc<AgentMessageBus>,
        event_window: Arc<RwLock<SlidingWindow>>,
    ) {
        info!("Starting intelligence layer");

        let mut interval = tokio::time::interval(self.summary_interval);

        loop {
            interval.tick().await;

            // Collect recent events
            let events = {
                let window = event_window.read().await;
                window.get_recent(100).into_iter().cloned().collect::<Vec<_>>()
            };

            if events.len() < self.min_events {
                debug!("Not enough events for summarization: {}", events.len());
                continue;
            }

            // Generate summary (placeholder - will integrate LLM later)
            if let Some(summary) = self.summarize_swarm_behavior(&events).await {
                debug!("Generated swarm summary: {}", summary);

                let event = AgentEvent::Important(ImportantEvent::SwarmStateSummary {
                    summary,
                    timestamp: current_timestamp(),
                });

                if let Err(e) = bus.publish(event).await {
                    warn!("Failed to publish swarm summary: {}", e);
                }
            }
        }
    }

    /// Summarize swarm behavior (placeholder implementation)
    async fn summarize_swarm_behavior(&self, events: &[InfoEvent]) -> Option<String> {
        if events.is_empty() {
            return None;
        }

        // TODO: Integrate with LLM for intelligent summarization
        // For now, provide a simple statistical summary

        let tool_count = events.iter()
            .filter(|e| matches!(e, InfoEvent::ToolExecuted { .. }))
            .count();

        let file_count = events.iter()
            .filter(|e| matches!(e, InfoEvent::FileAccessed { .. }))
            .count();

        let search_count = events.iter()
            .filter(|e| matches!(e, InfoEvent::SymbolSearched { .. }))
            .count();

        Some(format!(
            "Swarm activity: {} tool executions, {} file accesses, {} symbol searches in last {} seconds",
            tool_count,
            file_count,
            search_count,
            self.summary_interval.as_secs()
        ))
    }
}

fn current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::swarm::events::FileOperation;

    #[test]
    fn test_sliding_window() {
        let mut window = SlidingWindow::new(3);

        let event1 = InfoEvent::FileAccessed {
            agent_id: "agent_1".into(),
            path: "/test1".into(),
            operation: FileOperation::Read,
            timestamp: 1,
        };

        let event2 = InfoEvent::FileAccessed {
            agent_id: "agent_2".into(),
            path: "/test2".into(),
            operation: FileOperation::Read,
            timestamp: 2,
        };

        let event3 = InfoEvent::FileAccessed {
            agent_id: "agent_3".into(),
            path: "/test3".into(),
            operation: FileOperation::Read,
            timestamp: 3,
        };

        let event4 = InfoEvent::FileAccessed {
            agent_id: "agent_4".into(),
            path: "/test4".into(),
            operation: FileOperation::Read,
            timestamp: 4,
        };

        window.push(event1);
        window.push(event2);
        window.push(event3);
        assert_eq!(window.events.len(), 3);

        // Should evict oldest
        window.push(event4);
        assert_eq!(window.events.len(), 3);

        let recent = window.get_recent(2);
        assert_eq!(recent.len(), 2);
    }

    #[tokio::test]
    async fn test_aggregator_creation() {
        let bus = Arc::new(AgentMessageBus::new());
        let aggregator = SemanticAggregator::new(bus);

        let events = aggregator.get_recent_events(10).await;
        assert_eq!(events.len(), 0);
    }

    #[tokio::test]
    async fn test_intelligence_layer_creation() {
        let layer = IntelligenceLayer::new(Duration::from_secs(5), 10);
        assert_eq!(layer.summary_interval, Duration::from_secs(5));
        assert_eq!(layer.min_events, 10);
    }
}
