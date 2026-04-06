//! Context Injector
//!
//! Implements layered context delivery strategy based on event priority.

use crate::sync_primitives::Arc;
use dashmap::DashMap;
use std::collections::VecDeque;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use super::bus::AgentMessageBus;
use super::events::{CriticalEvent, EventTier, ImportantEvent};
use super::tasks::CoordTaskStore;
use crate::error::Result;
use crate::teams::context::InboxContextProvider;

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
    /// Optional task coordination store
    task_store: Option<Arc<dyn CoordTaskStore>>,
    /// Optional inbox context provider for team message awareness
    inbox_provider: Option<Arc<dyn InboxContextProvider>>,
    /// agent_id → CancellationToken for interrupting current LLM call
    agent_tokens: DashMap<String, CancellationToken>,
    /// agent_id → pending interrupt feedback to inject
    pending_interrupts: DashMap<String, String>,
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
        self.entries.iter().rev().take(count).collect()
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
            context_window: Arc::new(RwLock::new(ContextWindow::new(DEFAULT_CONTEXT_WINDOW_SIZE))),
            task_store: None,
            inbox_provider: None,
            agent_tokens: DashMap::new(),
            pending_interrupts: DashMap::new(),
        }
    }

    /// Create injector with custom window size
    pub fn with_window_size(bus: Arc<AgentMessageBus>, window_size: usize) -> Self {
        Self {
            bus,
            context_window: Arc::new(RwLock::new(ContextWindow::new(window_size))),
            task_store: None,
            inbox_provider: None,
            agent_tokens: DashMap::new(),
            pending_interrupts: DashMap::new(),
        }
    }

    /// Attach a task coordination store for injecting task context into prompts
    pub fn with_task_store(mut self, store: Arc<dyn CoordTaskStore>) -> Self {
        self.task_store = Some(store);
        self
    }

    /// Attach an inbox context provider for team message awareness
    pub fn with_inbox_provider(mut self, provider: Arc<dyn InboxContextProvider>) -> Self {
        self.inbox_provider = Some(provider);
        self
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

        // Subscribe to Critical events (Tier 1)
        let mut critical_rx = match self.bus.subscribe(EventTier::Critical).await {
            Ok(rx) => rx,
            Err(e) => {
                warn!("Failed to subscribe to Critical events: {}", e);
                return;
            }
        };

        // Process both Important and Critical events
        loop {
            tokio::select! {
                result = important_rx.recv() => {
                    match result {
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
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            warn!("Context injector lagged on important, skipped {} events", n);
                        }
                        Err(e) => {
                            warn!("Error receiving Important event: {}", e);
                            break;
                        }
                    }
                }
                result = critical_rx.recv() => {
                    match result {
                        Ok(event) => {
                            if let super::events::AgentEvent::Critical(critical_event) = event {
                                // Broadcast interrupt to ALL registered agents
                                let agent_ids: Vec<String> = self.agent_tokens.iter()
                                    .map(|entry| entry.key().clone())
                                    .collect();
                                for agent_id in &agent_ids {
                                    if let Err(e) = self.handle_critical_event(&critical_event, agent_id).await {
                                        warn!("Failed to handle critical event for {}: {}", agent_id, e);
                                    }
                                }
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            warn!("Context injector lagged on critical, skipped {} events", n);
                        }
                        Err(e) => {
                            warn!("Error receiving Critical event: {}", e);
                            break;
                        }
                    }
                }
            }
        }

        info!("Context injector stopped");
    }

    /// Inject swarm state into agent context (Tier 2: Passive Injection)
    ///
    /// This should be called before the agent enters Think phase.
    /// Returns a formatted string to be added to the system prompt.
    pub async fn inject_swarm_state(&self, agent_id: &str) -> String {
        let mut context = String::new();

        // Check for pending critical interrupts first
        if let Some(interrupt) = self.take_pending_interrupt(agent_id) {
            context.push_str(&interrupt);
            context.push('\n');
        }

        let window = self.context_window.read().await;
        let recent_updates = window.get_recent(window.max_entries);

        if !recent_updates.is_empty() {
            tracing::info!(
                subsystem = "swarm",
                event = "context_injected",
                entries = recent_updates.len(),
                "swarm context injector providing team awareness to agent"
            );

            context.push_str("\n## Swarm State (Team Awareness)\n");

            for entry in recent_updates {
                context.push_str(&format!(
                    "[{}] {}\n",
                    Self::format_timestamp(entry.timestamp),
                    entry.summary
                ));
            }

            context.push('\n');
        }

        // Append task coordination context if available
        if let Some(task_ctx) = self.inject_task_context(agent_id).await {
            context.push_str("\n\n");
            context.push_str(&task_ctx);
        }

        // Append inbox context if provider is available
        if let Some(ref provider) = self.inbox_provider {
            let inbox_ctx = provider.get_inbox_context(agent_id).await;
            if let Some(text) = inbox_ctx.to_injection_text() {
                context.push_str("\n\n");
                context.push_str(&text);
            }
        }

        context
    }

    /// Inject task coordination context for an agent
    ///
    /// Queries the task store for tasks owned by the given agent and formats
    /// them into a markdown section suitable for system prompt injection.
    pub async fn inject_task_context(&self, agent_id: &str) -> Option<String> {
        use super::tasks::CoordTaskFilter;

        let store = self.task_store.as_ref()?;

        let filter = CoordTaskFilter {
            owner: Some(agent_id.to_string()),
            ..Default::default()
        };

        let tasks = match store.list_tasks(filter).await {
            Ok(tasks) => tasks,
            Err(e) => {
                warn!(agent_id = agent_id, error = %e, "Failed to load task context for agent");
                return None;
            }
        };

        if tasks.is_empty() {
            return None;
        }

        let mut ctx = String::from("## Your Tasks\n");
        for task in &tasks {
            ctx.push_str(&format!(
                "- [{}] {} (priority: {})\n",
                task.status,
                task.subject,
                task.priority.as_str()
            ));
            if !task.dependencies.is_empty() {
                ctx.push_str(&format!("  blocked by: {}\n", task.dependencies.join(", ")));
            }
        }
        Some(ctx)
    }

    /// Register a CancellationToken for an agent (called at sub-agent spawn).
    pub fn register_agent_token(&self, agent_id: &str, token: CancellationToken) {
        self.agent_tokens.insert(agent_id.to_string(), token);
    }

    /// Remove an agent's token (called when sub-agent completes).
    pub fn unregister_agent_token(&self, agent_id: &str) {
        self.agent_tokens.remove(agent_id);
    }

    /// Take (consume) a pending interrupt message for an agent.
    pub fn take_pending_interrupt(&self, agent_id: &str) -> Option<String> {
        self.pending_interrupts.remove(agent_id).map(|(_, v)| v)
    }

    /// Handle critical event (Tier 1: Interrupt-Driven)
    ///
    /// Cancels the agent's current LLM call (if a token is registered)
    /// and stores a pending interrupt message for injection in the next Think phase.
    pub async fn handle_critical_event(
        &self,
        event: &CriticalEvent,
        agent_id: &str,
    ) -> Result<String> {
        let feedback = format!("[CRITICAL INTERRUPT] {}", self.format_critical_event(event));

        info!("Critical event for agent {}: {}", agent_id, feedback);

        // Cancel the agent's current LLM call if token is registered
        if let Some(token) = self.agent_tokens.get(agent_id) {
            token.cancel();
            tracing::info!(
                agent_id = agent_id,
                "Cancelled agent's CancellationToken due to critical event"
            );
        }

        // Store pending interrupt for injection in next Think phase
        self.pending_interrupts
            .insert(agent_id.to_string(), feedback.clone());

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
            ImportantEvent::ArenaStateUpdate {
                arena_id,
                goal,
                active_agents,
                completed_steps,
                total_steps,
                latest_artifacts,
                ..
            } => {
                let agents_str = active_agents.join(", ");
                let artifacts_str = if latest_artifacts.is_empty() {
                    "none yet".to_string()
                } else {
                    latest_artifacts.join("; ")
                };
                format!(
                    "Arena '{}': {} ({}/{} steps done, agents: [{}], recent: {})",
                    goal, arena_id, completed_steps, total_steps, agents_str, artifacts_str
                )
            }
            ImportantEvent::TaskCompleted {
                task_id, agent_id, ..
            } => {
                format!("Task '{}' completed by agent '{}'", task_id, agent_id)
            }
            ImportantEvent::TaskUnblocked {
                task_id,
                unblocked_by,
                ..
            } => {
                format!("Task '{}' unblocked by '{}'", task_id, unblocked_by)
            }
            ImportantEvent::TaskFailed {
                task_id, reason, ..
            } => {
                format!("Task '{}' failed: {}", task_id, reason)
            }
            ImportantEvent::AllTasksCompleted { team_id, .. } => {
                format!("All tasks completed for team '{}'", team_id)
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
            CriticalEvent::TaskCancelled {
                task_id, reason, ..
            } => {
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

    #[tokio::test]
    async fn test_format_arena_state_update() {
        let bus = Arc::new(AgentMessageBus::new());
        let injector = ContextInjector::new(bus);

        let event = ImportantEvent::ArenaStateUpdate {
            arena_id: "arena-42".into(),
            goal: "Fix auth bugs".into(),
            active_agents: vec!["agent-a".into(), "agent-b".into()],
            completed_steps: 3,
            total_steps: 10,
            latest_artifacts: vec!["Text: art-1".into(), "Code: art-2".into()],
            timestamp: 0,
        };

        let formatted = injector.format_important_event(&event);
        assert!(formatted.contains("Fix auth bugs"));
        assert!(formatted.contains("arena-42"));
        assert!(formatted.contains("3/10 steps done"));
        assert!(formatted.contains("agent-a, agent-b"));
        assert!(formatted.contains("Text: art-1; Code: art-2"));
    }

    // -----------------------------------------------------------------------
    // Mock CoordTaskStore for testing inject_task_context
    // -----------------------------------------------------------------------

    use super::super::tasks::{
        CoordTask, CoordTaskFilter, CoordTaskId, CoordTaskStatus, CoordTaskStore, CoordTaskUpdate,
        NewCoordTask, Priority,
    };
    use async_trait::async_trait;

    struct MockTaskStore {
        tasks: Vec<CoordTask>,
    }

    #[async_trait]
    impl CoordTaskStore for MockTaskStore {
        async fn create_task(&self, _task: NewCoordTask) -> crate::error::Result<CoordTask> {
            unimplemented!()
        }
        async fn get_task(&self, _id: &str) -> crate::error::Result<Option<CoordTask>> {
            unimplemented!()
        }
        async fn update_task(
            &self,
            _id: &str,
            _update: CoordTaskUpdate,
        ) -> crate::error::Result<CoordTask> {
            unimplemented!()
        }
        async fn list_tasks(
            &self,
            filter: CoordTaskFilter,
        ) -> crate::error::Result<Vec<CoordTask>> {
            let results: Vec<CoordTask> = self
                .tasks
                .iter()
                .filter(|t| {
                    filter
                        .owner
                        .as_ref()
                        .map_or(true, |o| t.owner.as_deref() == Some(o.as_str()))
                })
                .cloned()
                .collect();
            Ok(results)
        }
        async fn get_dependencies(&self, _id: &str) -> crate::error::Result<Vec<CoordTaskId>> {
            unimplemented!()
        }
        async fn get_dependents(&self, _id: &str) -> crate::error::Result<Vec<CoordTaskId>> {
            unimplemented!()
        }
        async fn get_newly_unblocked(
            &self,
            _completed_id: &str,
        ) -> crate::error::Result<Vec<CoordTask>> {
            unimplemented!()
        }
    }

    #[tokio::test]
    async fn test_inject_task_context_returns_formatted_list() {
        let store = Arc::new(MockTaskStore {
            tasks: vec![
                CoordTask {
                    id: "t-1".into(),
                    team_id: None,
                    subject: "Implement login".into(),
                    description: String::new(),
                    status: CoordTaskStatus::InProgress,
                    owner: Some("agent-1".into()),
                    priority: Priority::High,
                    result: None,
                    metadata: serde_json::Value::Null,
                    dependencies: vec![],
                    created_at: 0,
                    started_at: None,
                    completed_at: None,
                },
                CoordTask {
                    id: "t-2".into(),
                    team_id: None,
                    subject: "Write tests".into(),
                    description: String::new(),
                    status: CoordTaskStatus::Pending,
                    owner: Some("agent-1".into()),
                    priority: Priority::Normal,
                    result: None,
                    metadata: serde_json::Value::Null,
                    dependencies: vec!["t-1".into()],
                    created_at: 0,
                    started_at: None,
                    completed_at: None,
                },
            ],
        });

        let bus = Arc::new(AgentMessageBus::new());
        let injector = ContextInjector::new(bus).with_task_store(store);

        let ctx = injector.inject_task_context("agent-1").await;
        assert!(ctx.is_some());
        let text = ctx.unwrap();
        assert!(text.contains("## Your Tasks"));
        assert!(text.contains("Implement login"));
        assert!(text.contains("in_progress"));
        assert!(text.contains("high"));
        assert!(text.contains("Write tests"));
        assert!(text.contains("blocked by: t-1"));
    }

    #[tokio::test]
    async fn test_inject_task_context_empty_returns_none() {
        let bus = Arc::new(AgentMessageBus::new());
        let injector = ContextInjector::new(bus);
        let ctx = injector.inject_task_context("agent-1").await;
        assert!(ctx.is_none());
    }

    #[tokio::test]
    async fn test_inject_task_context_no_matching_tasks_returns_none() {
        let store = Arc::new(MockTaskStore { tasks: vec![] });
        let bus = Arc::new(AgentMessageBus::new());
        let injector = ContextInjector::new(bus).with_task_store(store);
        let ctx = injector.inject_task_context("agent-1").await;
        assert!(ctx.is_none());
    }

    #[tokio::test]
    async fn test_format_arena_state_update_no_artifacts() {
        let bus = Arc::new(AgentMessageBus::new());
        let injector = ContextInjector::new(bus);

        let event = ImportantEvent::ArenaStateUpdate {
            arena_id: "arena-1".into(),
            goal: "Research task".into(),
            active_agents: vec!["agent-x".into()],
            completed_steps: 0,
            total_steps: 5,
            latest_artifacts: vec![],
            timestamp: 0,
        };

        let formatted = injector.format_important_event(&event);
        assert!(formatted.contains("none yet"));
        assert!(formatted.contains("0/5 steps done"));
    }

    #[tokio::test]
    async fn test_interrupt_stores_pending_feedback() {
        let bus = Arc::new(AgentMessageBus::new());
        let injector = ContextInjector::new(bus);

        let token = CancellationToken::new();
        injector.register_agent_token("agent-1", token.clone());

        let event = CriticalEvent::ErrorDetected {
            agent_id: "agent-2".into(),
            error_message: "Database connection lost".into(),
            timestamp: 0,
        };

        let feedback = injector
            .handle_critical_event(&event, "agent-1")
            .await
            .unwrap();

        assert!(feedback.contains("CRITICAL INTERRUPT"));
        assert!(feedback.contains("Database connection lost"));
        assert!(token.is_cancelled());

        let pending = injector.take_pending_interrupt("agent-1");
        assert!(pending.is_some());
        assert!(pending.unwrap().contains("Database connection lost"));

        // Second take returns None
        assert!(injector.take_pending_interrupt("agent-1").is_none());
    }

    #[tokio::test]
    async fn test_interrupt_no_token_still_formats() {
        let bus = Arc::new(AgentMessageBus::new());
        let injector = ContextInjector::new(bus);

        let event = CriticalEvent::GlobalFailure {
            error: "Out of memory".into(),
            timestamp: 0,
        };

        let feedback = injector
            .handle_critical_event(&event, "unknown-agent")
            .await
            .unwrap();

        assert!(feedback.contains("CRITICAL INTERRUPT"));
        assert!(feedback.contains("Out of memory"));
    }

    #[test]
    fn test_register_and_unregister_token() {
        let bus_rt = tokio::runtime::Runtime::new().unwrap();
        let bus = bus_rt.block_on(async { Arc::new(AgentMessageBus::new()) });
        let injector = ContextInjector::new(bus);

        let token = CancellationToken::new();
        injector.register_agent_token("agent-1", token);
        injector.unregister_agent_token("agent-1");

        assert!(injector.take_pending_interrupt("agent-1").is_none());
    }
}
