//! Sub-agent handler component for managing sub-agent lifecycle.
//!
//! Enhanced to integrate with ExecutionCoordinator and ResultCollector
//! for synchronous result collection and tool call aggregation.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::agents::sub_agents::{
    CollectedToolStatus, ExecutionCoordinator, ResultCollector, ToolCallProgress, ToolCallStatus,
    ToolCallRecord, SubAgentResult as TraitsSubAgentResult,
};
use crate::agents::{AgentDef, AgentRegistry};
use crate::event::{
    AlephEvent, EventContext, EventHandler, EventType, HandlerError, SubAgentRequest,
    SubAgentResult, ToolCallError, ToolCallResult, ToolCallStarted,
};

/// Tracks active sub-agent sessions
#[derive(Debug)]
struct SubAgentSession {
    agent_def: AgentDef,
    parent_session_id: String,
    iteration_count: u32,
    /// When the session started
    started_at: Instant,
}

/// Handler for sub-agent lifecycle events
///
/// This handler now integrates with:
/// - `ExecutionCoordinator`: For synchronous result wait and concurrency control
/// - `ResultCollector`: For aggregating tool calls and artifacts
pub struct SubAgentHandler {
    registry: Arc<AgentRegistry>,
    active_sessions: RwLock<HashMap<String, SubAgentSession>>,
    /// Session ID to Request ID mapping for tool call correlation
    session_to_request: RwLock<HashMap<String, String>>,
    /// Execution coordinator for synchronous wait (optional)
    coordinator: Option<Arc<ExecutionCoordinator>>,
    /// Result collector for tool aggregation (optional)
    collector: Option<Arc<ResultCollector>>,
}

impl SubAgentHandler {
    /// Create a new SubAgentHandler
    pub fn new(registry: Arc<AgentRegistry>) -> Self {
        Self {
            registry,
            active_sessions: RwLock::new(HashMap::new()),
            session_to_request: RwLock::new(HashMap::new()),
            coordinator: None,
            collector: None,
        }
    }

    /// Create a new SubAgentHandler with coordinator and collector
    pub fn with_components(
        registry: Arc<AgentRegistry>,
        coordinator: Arc<ExecutionCoordinator>,
        collector: Arc<ResultCollector>,
    ) -> Self {
        Self {
            registry,
            active_sessions: RwLock::new(HashMap::new()),
            session_to_request: RwLock::new(HashMap::new()),
            coordinator: Some(coordinator),
            collector: Some(collector),
        }
    }

    /// Set the execution coordinator
    pub fn set_coordinator(&mut self, coordinator: Arc<ExecutionCoordinator>) {
        self.coordinator = Some(coordinator);
    }

    /// Set the result collector
    pub fn set_collector(&mut self, collector: Arc<ResultCollector>) {
        self.collector = Some(collector);
    }

    /// Get the agent definition for a sub-agent
    pub fn get_agent(&self, agent_id: &str) -> Option<AgentDef> {
        self.registry.get(agent_id)
    }

    /// Check if a session is active
    pub async fn is_session_active(&self, session_id: &str) -> bool {
        let sessions = self.active_sessions.read().await;
        sessions.contains_key(session_id)
    }

    /// Get the parent session ID for a sub-agent session
    pub async fn get_parent_session(&self, child_session_id: &str) -> Option<String> {
        let sessions = self.active_sessions.read().await;
        sessions
            .get(child_session_id)
            .map(|s| s.parent_session_id.clone())
    }

    /// Get the request ID for a session
    pub async fn get_request_for_session(&self, session_id: &str) -> Option<String> {
        let mapping = self.session_to_request.read().await;
        mapping.get(session_id).cloned()
    }

    /// Get the current iteration count for a sub-agent session
    pub async fn get_iteration_count(&self, session_id: &str) -> Option<u32> {
        let sessions = self.active_sessions.read().await;
        sessions.get(session_id).map(|s| s.iteration_count)
    }

    /// Increment the iteration count for a sub-agent session
    pub async fn increment_iteration(&self, session_id: &str) -> Option<u32> {
        let mut sessions = self.active_sessions.write().await;
        if let Some(session) = sessions.get_mut(session_id) {
            session.iteration_count += 1;
            Some(session.iteration_count)
        } else {
            None
        }
    }

    /// Check if a sub-agent has exceeded its max iterations
    pub async fn has_exceeded_max_iterations(&self, session_id: &str) -> bool {
        let sessions = self.active_sessions.read().await;
        if let Some(session) = sessions.get(session_id) {
            if let Some(max) = session.agent_def.max_iterations {
                return session.iteration_count >= max;
            }
        }
        false
    }

    /// Handle SubAgentStarted event
    async fn handle_started(
        &self,
        request: &SubAgentRequest,
        _ctx: &EventContext,
    ) -> Result<Vec<AlephEvent>, HandlerError> {
        // Get the agent definition
        let agent_def = self.registry.get(&request.agent_id).ok_or_else(|| {
            HandlerError::Internal(format!("Agent not found: {}", request.agent_id))
        })?;

        // Generate request ID if not provided (for backwards compatibility)
        let request_id = format!("req_{}", uuid::Uuid::new_v4());

        // Create the sub-agent session tracking
        let session = SubAgentSession {
            agent_def,
            parent_session_id: request.parent_session_id.clone(),
            iteration_count: 0,
            started_at: Instant::now(),
        };

        // Store the session
        {
            let mut sessions = self.active_sessions.write().await;
            sessions.insert(request.child_session_id.clone(), session);
        }

        // Store session -> request mapping
        {
            let mut mapping = self.session_to_request.write().await;
            mapping.insert(request.child_session_id.clone(), request_id.clone());
        }

        // Initialize result collector if available
        if let Some(ref collector) = self.collector {
            collector.init_request(&request_id).await;
        }

        // Start execution tracking if coordinator available
        if let Some(ref coordinator) = self.coordinator {
            coordinator.start_execution(&request_id).await;
        }

        info!(
            agent_id = %request.agent_id,
            child_session_id = %request.child_session_id,
            parent_session_id = %request.parent_session_id,
            request_id = %request_id,
            "Sub-agent started"
        );

        Ok(vec![])
    }

    /// Handle SubAgentCompleted event
    async fn handle_completed(
        &self,
        result: &SubAgentResult,
        _ctx: &EventContext,
    ) -> Result<Vec<AlephEvent>, HandlerError> {
        // Get the request ID before removing the session
        let request_id = self.get_request_for_session(&result.child_session_id).await;

        // Remove the session from tracking
        let session = {
            let mut sessions = self.active_sessions.write().await;
            sessions.remove(&result.child_session_id)
        };

        // Remove session -> request mapping
        {
            let mut mapping = self.session_to_request.write().await;
            mapping.remove(&result.child_session_id);
        }

        if let Some(session) = session {
            let execution_duration_ms = session.started_at.elapsed().as_millis() as u64;

            info!(
                agent_id = %result.agent_id,
                child_session_id = %result.child_session_id,
                success = %result.success,
                iterations = %session.iteration_count,
                duration_ms = %execution_duration_ms,
                "Sub-agent completed"
            );

            // Notify coordinator with aggregated result
            if let Some(ref request_id) = request_id {
                // Get tool summary from collector
                let tools_called = if let Some(ref collector) = self.collector {
                    let summary = collector.get_summary(request_id).await;
                    summary
                        .into_iter()
                        .map(|s| ToolCallRecord {
                            name: s.tool,
                            arguments: serde_json::Value::Null,
                            success: s.state.status == "completed",
                            result_summary: s.state.title.unwrap_or_default(),
                        })
                        .collect()
                } else {
                    Vec::new()
                };

                // Create enhanced result for coordinator
                let enhanced_result =
                    TraitsSubAgentResult::success(request_id, &result.summary)
                        .with_iterations(session.iteration_count)
                        .with_tools_called(tools_called);

                // Notify coordinator
                if let Some(ref coordinator) = self.coordinator {
                    coordinator.on_execution_completed(enhanced_result).await;
                }

                debug!(request_id = %request_id, "Sub-agent result collected");
            }
        }

        Ok(vec![])
    }

    /// Handle ToolCallStarted event
    async fn handle_tool_started(
        &self,
        event: &ToolCallStarted,
        _ctx: &EventContext,
    ) -> Result<Vec<AlephEvent>, HandlerError> {
        // Get request ID from session if available
        let request_id = if let Some(ref session_id) = event.session_id {
            self.get_request_for_session(session_id).await
        } else {
            None
        };

        if let Some(request_id) = request_id {
            // Record in collector
            if let Some(ref collector) = self.collector {
                collector
                    .record_tool_start(&request_id, &event.call_id, &event.tool, event.input.clone())
                    .await;
            }

            // Record progress in coordinator
            if let Some(ref coordinator) = self.coordinator {
                coordinator
                    .on_tool_progress(
                        &request_id,
                        ToolCallProgress {
                            call_id: event.call_id.clone(),
                            tool_name: event.tool.clone(),
                            status: ToolCallStatus::Running,
                            timestamp: Instant::now(),
                        },
                    )
                    .await;
            }

            debug!(
                request_id = %request_id,
                call_id = %event.call_id,
                tool = %event.tool,
                "Tool call started for sub-agent"
            );
        }

        Ok(vec![])
    }

    /// Handle ToolCallCompleted event
    async fn handle_tool_completed(
        &self,
        event: &ToolCallResult,
        _ctx: &EventContext,
    ) -> Result<Vec<AlephEvent>, HandlerError> {
        // Get request ID from session if available
        let request_id = if let Some(ref session_id) = event.session_id {
            self.get_request_for_session(session_id).await
        } else {
            None
        };

        if let Some(request_id) = request_id {
            // Truncate output for preview
            let output_preview =
                crate::agents::sub_agents::truncate_for_preview(&event.output);

            // Update collector
            if let Some(ref collector) = self.collector {
                collector
                    .update_tool_status(
                        &request_id,
                        &event.call_id,
                        CollectedToolStatus::Completed {
                            output_preview: output_preview.clone(),
                        },
                        Some(format!("{} completed", event.tool)),
                    )
                    .await;
            }

            // Record progress in coordinator
            if let Some(ref coordinator) = self.coordinator {
                coordinator
                    .on_tool_progress(
                        &request_id,
                        ToolCallProgress {
                            call_id: event.call_id.clone(),
                            tool_name: event.tool.clone(),
                            status: ToolCallStatus::Completed { output_preview },
                            timestamp: Instant::now(),
                        },
                    )
                    .await;
            }

            debug!(
                request_id = %request_id,
                call_id = %event.call_id,
                tool = %event.tool,
                "Tool call completed for sub-agent"
            );
        }

        Ok(vec![])
    }

    /// Handle ToolCallFailed event
    async fn handle_tool_failed(
        &self,
        event: &ToolCallError,
        _ctx: &EventContext,
    ) -> Result<Vec<AlephEvent>, HandlerError> {
        // Get request ID from session if available
        let request_id = if let Some(ref session_id) = event.session_id {
            self.get_request_for_session(session_id).await
        } else {
            None
        };

        if let Some(request_id) = request_id {
            // Update collector
            if let Some(ref collector) = self.collector {
                collector
                    .update_tool_status(
                        &request_id,
                        &event.call_id,
                        CollectedToolStatus::Failed {
                            error: event.error.clone(),
                        },
                        Some(format!("{} failed", event.tool)),
                    )
                    .await;
            }

            // Record progress in coordinator
            if let Some(ref coordinator) = self.coordinator {
                coordinator
                    .on_tool_progress(
                        &request_id,
                        ToolCallProgress {
                            call_id: event.call_id.clone(),
                            tool_name: event.tool.clone(),
                            status: ToolCallStatus::Failed {
                                error: event.error.clone(),
                            },
                            timestamp: Instant::now(),
                        },
                    )
                    .await;
            }

            warn!(
                request_id = %request_id,
                call_id = %event.call_id,
                tool = %event.tool,
                error = %event.error,
                "Tool call failed for sub-agent"
            );
        }

        Ok(vec![])
    }
}

#[async_trait]
impl EventHandler for SubAgentHandler {
    fn name(&self) -> &'static str {
        "SubAgentHandler"
    }

    fn subscriptions(&self) -> Vec<EventType> {
        vec![
            EventType::SubAgentStarted,
            EventType::SubAgentCompleted,
            // New subscriptions for tool call tracking
            EventType::ToolCallStarted,
            EventType::ToolCallCompleted,
            EventType::ToolCallFailed,
        ]
    }

    async fn handle(
        &self,
        event: &AlephEvent,
        ctx: &EventContext,
    ) -> Result<Vec<AlephEvent>, HandlerError> {
        match event {
            AlephEvent::SubAgentStarted(request) => self.handle_started(request, ctx).await,
            AlephEvent::SubAgentCompleted(result) => self.handle_completed(result, ctx).await,
            AlephEvent::ToolCallStarted(event) => self.handle_tool_started(event, ctx).await,
            AlephEvent::ToolCallCompleted(event) => self.handle_tool_completed(event, ctx).await,
            AlephEvent::ToolCallFailed(event) => self.handle_tool_failed(event, ctx).await,
            _ => Ok(vec![]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::sub_agents::CoordinatorConfig;
    use crate::event::EventBus;
    use std::sync::Arc;

    fn create_test_registry() -> Arc<AgentRegistry> {
        Arc::new(AgentRegistry::with_builtins())
    }

    fn create_test_context() -> EventContext {
        let bus = EventBus::new();
        EventContext::new(bus)
    }

    #[tokio::test]
    async fn test_handler_name() {
        let registry = create_test_registry();
        let handler = SubAgentHandler::new(registry);
        assert_eq!(handler.name(), "SubAgentHandler");
    }

    #[tokio::test]
    async fn test_subscriptions() {
        let registry = create_test_registry();
        let handler = SubAgentHandler::new(registry);
        let events = handler.subscriptions();

        assert!(events.contains(&EventType::SubAgentStarted));
        assert!(events.contains(&EventType::SubAgentCompleted));
        assert!(events.contains(&EventType::ToolCallStarted));
        assert!(events.contains(&EventType::ToolCallCompleted));
        assert!(events.contains(&EventType::ToolCallFailed));
    }

    #[tokio::test]
    async fn test_get_agent() {
        let registry = create_test_registry();
        let handler = SubAgentHandler::new(registry);

        assert!(handler.get_agent("explore").is_some());
        assert!(handler.get_agent("nonexistent").is_none());
    }

    #[tokio::test]
    async fn test_handle_started() {
        let registry = create_test_registry();
        let handler = SubAgentHandler::new(registry);
        let ctx = create_test_context();

        let request = SubAgentRequest {
            agent_id: "explore".into(),
            prompt: "Find files".into(),
            parent_session_id: "parent-1".into(),
            child_session_id: "child-1".into(),
        };

        let event = AlephEvent::SubAgentStarted(request);
        handler.handle(&event, &ctx).await.unwrap();

        assert!(handler.is_session_active("child-1").await);
        assert_eq!(
            handler.get_parent_session("child-1").await,
            Some("parent-1".into())
        );
        // Should have request ID mapping
        assert!(handler.get_request_for_session("child-1").await.is_some());
    }

    #[tokio::test]
    async fn test_handle_started_invalid_agent() {
        let registry = create_test_registry();
        let handler = SubAgentHandler::new(registry);
        let ctx = create_test_context();

        let request = SubAgentRequest {
            agent_id: "nonexistent".into(),
            prompt: "test".into(),
            parent_session_id: "parent-1".into(),
            child_session_id: "child-1".into(),
        };

        let event = AlephEvent::SubAgentStarted(request);
        let result = handler.handle(&event, &ctx).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_completed() {
        let registry = create_test_registry();
        let handler = SubAgentHandler::new(registry);
        let ctx = create_test_context();

        // First start a session
        let start_request = SubAgentRequest {
            agent_id: "explore".into(),
            prompt: "Find files".into(),
            parent_session_id: "parent-1".into(),
            child_session_id: "child-1".into(),
        };
        handler
            .handle(&AlephEvent::SubAgentStarted(start_request), &ctx)
            .await
            .unwrap();

        assert!(handler.is_session_active("child-1").await);

        // Now complete it
        let result = SubAgentResult {
            agent_id: "explore".into(),
            child_session_id: "child-1".into(),
            summary: "Found 5 files".into(),
            success: true,
            error: None,
            request_id: None,
            tools_called: vec![],
            execution_duration_ms: None,
        };
        handler
            .handle(&AlephEvent::SubAgentCompleted(result), &ctx)
            .await
            .unwrap();

        assert!(!handler.is_session_active("child-1").await);
        assert!(handler.get_request_for_session("child-1").await.is_none());
    }

    #[tokio::test]
    async fn test_iteration_tracking() {
        let registry = create_test_registry();
        let handler = SubAgentHandler::new(registry);
        let ctx = create_test_context();

        let request = SubAgentRequest {
            agent_id: "explore".into(),
            prompt: "test".into(),
            parent_session_id: "parent-1".into(),
            child_session_id: "child-1".into(),
        };
        handler
            .handle(&AlephEvent::SubAgentStarted(request), &ctx)
            .await
            .unwrap();

        assert_eq!(handler.get_iteration_count("child-1").await, Some(0));

        handler.increment_iteration("child-1").await;
        assert_eq!(handler.get_iteration_count("child-1").await, Some(1));

        handler.increment_iteration("child-1").await;
        assert_eq!(handler.get_iteration_count("child-1").await, Some(2));
    }

    #[tokio::test]
    async fn test_max_iterations_check() {
        let registry = create_test_registry();
        let handler = SubAgentHandler::new(registry);
        let ctx = create_test_context();

        // explore agent has max_iterations = 20
        let request = SubAgentRequest {
            agent_id: "explore".into(),
            prompt: "test".into(),
            parent_session_id: "parent-1".into(),
            child_session_id: "child-1".into(),
        };
        handler
            .handle(&AlephEvent::SubAgentStarted(request), &ctx)
            .await
            .unwrap();

        assert!(!handler.has_exceeded_max_iterations("child-1").await);

        // Simulate 20 iterations
        for _ in 0..20 {
            handler.increment_iteration("child-1").await;
        }

        assert!(handler.has_exceeded_max_iterations("child-1").await);
    }

    #[tokio::test]
    async fn test_increment_nonexistent_session() {
        let registry = create_test_registry();
        let handler = SubAgentHandler::new(registry);

        assert!(handler.increment_iteration("nonexistent").await.is_none());
    }

    #[tokio::test]
    async fn test_with_components() {
        let registry = create_test_registry();
        let coordinator = Arc::new(ExecutionCoordinator::new(CoordinatorConfig::default()));
        let collector = Arc::new(ResultCollector::new());

        let handler = SubAgentHandler::with_components(registry, coordinator.clone(), collector.clone());
        let ctx = create_test_context();

        // Start a session
        let request = SubAgentRequest {
            agent_id: "explore".into(),
            prompt: "Find files".into(),
            parent_session_id: "parent-1".into(),
            child_session_id: "child-1".into(),
        };
        handler
            .handle(&AlephEvent::SubAgentStarted(request), &ctx)
            .await
            .unwrap();

        // Get the request ID
        let request_id = handler.get_request_for_session("child-1").await.unwrap();

        // Collector should have the request initialized
        assert!(collector.has_request(&request_id).await);

        // Coordinator should be tracking
        assert!(coordinator.is_pending(&request_id).await);
    }

    #[tokio::test]
    async fn test_tool_call_tracking() {
        let registry = create_test_registry();
        let coordinator = Arc::new(ExecutionCoordinator::new(CoordinatorConfig::default()));
        let collector = Arc::new(ResultCollector::new());

        let handler = SubAgentHandler::with_components(registry, coordinator.clone(), collector.clone());
        let ctx = create_test_context();

        // Start a session
        let request = SubAgentRequest {
            agent_id: "explore".into(),
            prompt: "Find files".into(),
            parent_session_id: "parent-1".into(),
            child_session_id: "child-1".into(),
        };
        handler
            .handle(&AlephEvent::SubAgentStarted(request), &ctx)
            .await
            .unwrap();

        let request_id = handler.get_request_for_session("child-1").await.unwrap();

        // Simulate a tool call with session_id
        let tool_started = ToolCallStarted {
            call_id: "call-1".into(),
            tool: "glob".into(),
            input: serde_json::json!({"pattern": "*.rs"}),
            timestamp: chrono::Utc::now().timestamp_millis(),
            session_id: Some("child-1".into()),
        };
        handler
            .handle(&AlephEvent::ToolCallStarted(tool_started), &ctx)
            .await
            .unwrap();

        // Collector should have the tool call
        assert_eq!(collector.get_total_count(&request_id).await, 1);
        assert_eq!(collector.get_running_count(&request_id).await, 1);

        // Complete the tool call
        let tool_completed = ToolCallResult {
            call_id: "call-1".into(),
            tool: "glob".into(),
            input: serde_json::json!({"pattern": "*.rs"}),
            output: "Found 10 files".into(),
            started_at: 1000,
            completed_at: 2000,
            token_usage: crate::event::TokenUsage::default(),
            session_id: Some("child-1".into()),
        };
        handler
            .handle(&AlephEvent::ToolCallCompleted(tool_completed), &ctx)
            .await
            .unwrap();

        // Collector should show completed
        assert_eq!(collector.get_completed_count(&request_id).await, 1);
        assert!(collector.all_completed(&request_id).await);

        // Get summary
        let summary = collector.get_summary(&request_id).await;
        assert_eq!(summary.len(), 1);
        assert_eq!(summary[0].tool, "glob");
        assert_eq!(summary[0].state.status, "completed");
    }
}
