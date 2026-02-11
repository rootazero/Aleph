//! Context Injector
//!
//! Implements layered context delivery strategy based on event priority.

use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::bus::AgentMessageBus;
use super::events::{CriticalEvent, EventTier, ImportantEvent};
use crate::error::Result;

/// Maximum number of swarm state entries to keep in context
const DEFAULT_CONTEXT_WINDOW_SIZE: usize = 5;

/// Context Injector
///
/// Manages how swarm events are delivered to agents based on tier:
/// - Tier 1 (Critical): Interrupt current execution
/// - Tier 2 (Important): Inject into next Think phase
/// - Tier 3 (Info): Available via on-demand query
pub struct ContextInjector {
    /// Reference to message bus
    bus: Arc<AgentMessageBus>,
    /// Sliding context viewport
    context_window: Arc<RwLock<ContextWindow>>,
}

/// Sliding context viewport for Tier 2 events
struct ContextWindow {
    max_entries: usize,
    entries: VecDeque<SwarmContextEntry>,
}

/// Entry in the context window
#[derive(Debug, Clone)]
pub struct SwarmContextEntry {
    pub timestamp: u64,
    pub event: ImportantEvent,
    pub summary: String,
}

impl ContextWindow {
    fn new(max_entries: usize) -> Self {
        Self {
            max_entries,
            entries: VecDeque::with_capacity(max_entries),
        }
    }

    fn push(&mut self, entry: SwarmContextEntry) {
        if self.entries.len() >= self.max_entries {
            self.entries.pop_front();
        }
        self.entries.push_back(entry);
    }

    fn get_recent(&self, count: usize) -> Vec<&SwarmContextEntry> {
        self.entries
            .iter()
            .rev()
            .take(count)
            .collect()
    }

    fn clear(&mut self) {
        self.entries.clear();
    }
}

impl ContextInjector {
    /// Create a new context injector
    pub fn new(bus: Arc<AgentMessageBus>) -> Self {
        Self {
            bus,
            context_window: Arc::new(RwLock::new(ContextWindow::new(
                DEFAULT_CONTEXT_WINDOW_SIZE,
            ))),
        }
    }

    /// Create injector with custom window size
    pub fn with_window_size(bus: Arc<AgentMessageBus>, window_size: usize) -> Self {
        Self {
            bus,
            context_window: Arc::new(RwLock::new(ContextWindow::new(window_size))),
        }
    }

    /// Start the context injector background loop
    pub async fn run(self: Arc<Self>) {
        info!("Starting context injector");

        // Subscribe to Important events (Tier 2)
        let mut important_rx = match self.bus.subscribe(EventTier::Important).await {
            Ok(rx) => rx,
            Err(e) => {
                warn!("Failed to subscribe to Important events: {}", e);
                return;
            }
        };

        // Process Important events and add to context window
        loop {
            match important_rx.recv().await {
                Ok(event) => {
                    if let super::events::AgentEvent::Important(important_event) = event {
                        let summary = self.format_important_event(&important_event);
                        let entry = SwarmContextEntry {
                            timestamp: important_event.timestamp(),
                            event: important_event,
                            summary,
                        };

                        let mut window = self.context_window.write().await;
                        window.push(entry);
                        debug!("Added event to context window");
                    }
                }
                Err(e) => {
                    warn!("Error receiving Important event: {}", e);
                    break;
                }
            }
        }

        info!("Context injector stopped");
    }

    /// Inject swarm state into agent context (Tier 2: Passive Injection)
    ///
    /// This should be called before the agent enters Think phase.
    /// Returns a formatted string to be added to the system prompt.
    pub async fn inject_swarm_state(&self, _agent_id: &str) -> String {
        let window = self.context_window.read().await;
        let recent_updates = window.get_recent(DEFAULT_CONTEXT_WINDOW_SIZE);

        if recent_updates.is_empty() {
            return String::new();
        }

        let mut context = String::from("\n## Swarm State (Team Awareness)\n");

        for entry in recent_updates {
            context.push_str(&format!(
                "[{}] {}\n",
                Self::format_timestamp(entry.timestamp),
                entry.summary
            ));
        }

        context.push('\n');
        context
    }

    /// Handle critical event (Tier 1: Interrupt-Driven)
    ///
    /// This is a placeholder for interrupt mechanism.
    /// In a full implementation, this would:
    /// 1. Abort current LLM generation
    /// 2. Inject event as System Feedback
    /// 3. Trigger agent to re-enter Think phase
    pub async fn handle_critical_event(
        &self,
        event: &CriticalEvent,
        _agent_id: &str,
    ) -> Result<String> {
        // Format critical event for immediate attention
        let feedback = format!(
            "[CRITICAL INTERRUPT] {}",
            self.format_critical_event(event)
        );

        info!("Critical event: {}", feedback);

        // TODO: Implement actual interrupt mechanism
        // - Abort current generation
        // - Trigger rethink

        Ok(feedback)
    }

    /// Format an important event for display
    fn format_important_event(&self, event: &ImportantEvent) -> String {
        match event {
            ImportantEvent::Hotspot {
                area,
                agent_count,
                activity,
                ..
            } => {
                format!(
                    "Hotspot detected: {} agents working on {} ({})",
                    agent_count, area, activity
                )
            }
            ImportantEvent::ConfirmedInsight {
                symbol,
                confidence,
                sources,
                ..
            } => {
                format!(
                    "Confirmed insight: {} (confidence: {:.0}%, {} sources)",
                    symbol,
                    confidence * 100.0,
                    sources.len()
                )
            }
            ImportantEvent::SwarmStateSummary { summary, .. } => {
                format!("Swarm summary: {}", summary)
            }
            ImportantEvent::ToolExecuted {
                agent_id,
                tool_name,
                duration_ms,
                ..
            } => {
                format!(
                    "Agent {} executed {} ({}ms)",
                    agent_id, tool_name, duration_ms
                )
            }
            ImportantEvent::DecisionBroadcast {
                agent_id,
                decision,
                affected_files,
                ..
            } => {
                if affected_files.is_empty() {
                    format!("Agent {} decided: {}", agent_id, decision)
                } else {
                    format!(
                        "Agent {} decided: {} (affects {} files)",
                        agent_id,
                        decision,
                        affected_files.len()
                    )
                }
            }
        }
    }

    /// Format a critical event for display
    fn format_critical_event(&self, event: &CriticalEvent) -> String {
        match event {
            CriticalEvent::BugRootCauseFound {
                location,
                description,
                ..
            } => {
                format!("Bug root cause found at {}: {}", location, description)
            }
            CriticalEvent::TaskCancelled { task_id, reason, .. } => {
                format!("Task {} cancelled: {}", task_id, reason)
            }
            CriticalEvent::GlobalFailure { error, .. } => {
                format!("Global failure: {}", error)
            }
            CriticalEvent::ErrorDetected {
                agent_id,
                error_message,
                ..
            } => {
                format!("Agent {} error: {}", agent_id, error_message)
            }
        }
    }

    /// Format timestamp for display
    fn format_timestamp(timestamp: u64) -> String {
        // Simple relative time formatting
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let diff = now.saturating_sub(timestamp);

        if diff < 60 {
            format!("{}s ago", diff)
        } else if diff < 3600 {
            format!("{}m ago", diff / 60)
        } else {
            format!("{}h ago", diff / 3600)
        }
    }

    /// Get current context window size
    pub async fn window_size(&self) -> usize {
        let window = self.context_window.read().await;
        window.entries.len()
    }

    /// Clear context window
    pub async fn clear_context(&self) {
        let mut window = self.context_window.write().await;
        window.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_window() {
        let mut window = ContextWindow::new(3);

        let entry1 = SwarmContextEntry {
            timestamp: 1,
            event: ImportantEvent::SwarmStateSummary {
                summary: "Test 1".into(),
                timestamp: 1,
            },
            summary: "Test 1".into(),
        };

        let entry2 = SwarmContextEntry {
            timestamp: 2,
            event: ImportantEvent::SwarmStateSummary {
                summary: "Test 2".into(),
                timestamp: 2,
            },
            summary: "Test 2".into(),
        };

        let entry3 = SwarmContextEntry {
            timestamp: 3,
            event: ImportantEvent::SwarmStateSummary {
                summary: "Test 3".into(),
                timestamp: 3,
            },
            summary: "Test 3".into(),
        };

        let entry4 = SwarmContextEntry {
            timestamp: 4,
            event: ImportantEvent::SwarmStateSummary {
                summary: "Test 4".into(),
                timestamp: 4,
            },
            summary: "Test 4".into(),
        };

        window.push(entry1);
        window.push(entry2);
        window.push(entry3);
        assert_eq!(window.entries.len(), 3);

        // Should evict oldest
        window.push(entry4);
        assert_eq!(window.entries.len(), 3);

        let recent = window.get_recent(2);
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].timestamp, 4);
        assert_eq!(recent[1].timestamp, 3);
    }

    #[tokio::test]
    async fn test_injector_creation() {
        let bus = Arc::new(AgentMessageBus::new());
        let injector = ContextInjector::new(bus);

        assert_eq!(injector.window_size().await, 0);
    }

    #[tokio::test]
    async fn test_inject_empty_state() {
        let bus = Arc::new(AgentMessageBus::new());
        let injector = ContextInjector::new(bus);

        let context = injector.inject_swarm_state("agent_1").await;
        assert_eq!(context, "");
    }

    #[tokio::test]
    async fn test_format_important_event() {
        let bus = Arc::new(AgentMessageBus::new());
        let injector = ContextInjector::new(bus);

        let event = ImportantEvent::Hotspot {
            area: "auth/".into(),
            agent_count: 3,
            activity: "analysis".into(),
            timestamp: 0,
        };

        let formatted = injector.format_important_event(&event);
        assert!(formatted.contains("Hotspot detected"));
        assert!(formatted.contains("3 agents"));
        assert!(formatted.contains("auth/"));
    }

    #[tokio::test]
    async fn test_format_critical_event() {
        let bus = Arc::new(AgentMessageBus::new());
        let injector = ContextInjector::new(bus);

        let event = CriticalEvent::BugRootCauseFound {
            location: "auth/login.rs:42".into(),
            description: "Null pointer dereference".into(),
            timestamp: 0,
        };

        let formatted = injector.format_critical_event(&event);
        assert!(formatted.contains("Bug root cause found"));
        assert!(formatted.contains("auth/login.rs:42"));
    }

    #[tokio::test]
    async fn test_clear_context() {
        let bus = Arc::new(AgentMessageBus::new());
        let injector = ContextInjector::new(bus);

        // Add an entry manually
        {
            let mut window = injector.context_window.write().await;
            window.push(SwarmContextEntry {
                timestamp: 1,
                event: ImportantEvent::SwarmStateSummary {
                    summary: "Test".into(),
                    timestamp: 1,
                },
                summary: "Test".into(),
            });
        }

        assert_eq!(injector.window_size().await, 1);

        injector.clear_context().await;
        assert_eq!(injector.window_size().await, 0);
    }
}
