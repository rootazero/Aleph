//! Sub-agent handler component for managing sub-agent lifecycle.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;

use crate::agents::{AgentDef, AgentRegistry};
use crate::event::{
    AetherEvent, EventContext, EventHandler, EventType, HandlerError, SubAgentRequest,
    SubAgentResult,
};

/// Tracks active sub-agent sessions
#[derive(Debug)]
struct SubAgentSession {
    agent_def: AgentDef,
    parent_session_id: String,
    iteration_count: u32,
}

/// Handler for sub-agent lifecycle events
pub struct SubAgentHandler {
    registry: Arc<AgentRegistry>,
    active_sessions: RwLock<HashMap<String, SubAgentSession>>,
}

impl SubAgentHandler {
    /// Create a new SubAgentHandler
    pub fn new(registry: Arc<AgentRegistry>) -> Self {
        Self {
            registry,
            active_sessions: RwLock::new(HashMap::new()),
        }
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
    ) -> Result<Vec<AetherEvent>, HandlerError> {
        // Get the agent definition
        let agent_def = self.registry.get(&request.agent_id).ok_or_else(|| {
            HandlerError::Internal(format!("Agent not found: {}", request.agent_id))
        })?;

        // Create the sub-agent session tracking
        let session = SubAgentSession {
            agent_def,
            parent_session_id: request.parent_session_id.clone(),
            iteration_count: 0,
        };

        // Store the session
        {
            let mut sessions = self.active_sessions.write().await;
            sessions.insert(request.child_session_id.clone(), session);
        }

        tracing::info!(
            agent_id = %request.agent_id,
            child_session_id = %request.child_session_id,
            parent_session_id = %request.parent_session_id,
            "Sub-agent started"
        );

        Ok(vec![])
    }

    /// Handle SubAgentCompleted event
    async fn handle_completed(
        &self,
        result: &SubAgentResult,
        _ctx: &EventContext,
    ) -> Result<Vec<AetherEvent>, HandlerError> {
        // Remove the session from tracking
        let session = {
            let mut sessions = self.active_sessions.write().await;
            sessions.remove(&result.child_session_id)
        };

        if let Some(session) = session {
            tracing::info!(
                agent_id = %result.agent_id,
                child_session_id = %result.child_session_id,
                success = %result.success,
                iterations = %session.iteration_count,
                "Sub-agent completed"
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
        vec![EventType::SubAgentStarted, EventType::SubAgentCompleted]
    }

    async fn handle(&self, event: &AetherEvent, ctx: &EventContext) -> Result<Vec<AetherEvent>, HandlerError> {
        match event {
            AetherEvent::SubAgentStarted(request) => self.handle_started(request, ctx).await,
            AetherEvent::SubAgentCompleted(result) => self.handle_completed(result, ctx).await,
            _ => Ok(vec![]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

        let event = AetherEvent::SubAgentStarted(request);
        handler.handle(&event, &ctx).await.unwrap();

        assert!(handler.is_session_active("child-1").await);
        assert_eq!(
            handler.get_parent_session("child-1").await,
            Some("parent-1".into())
        );
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

        let event = AetherEvent::SubAgentStarted(request);
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
            .handle(&AetherEvent::SubAgentStarted(start_request), &ctx)
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
        };
        handler
            .handle(&AetherEvent::SubAgentCompleted(result), &ctx)
            .await
            .unwrap();

        assert!(!handler.is_session_active("child-1").await);
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
            .handle(&AetherEvent::SubAgentStarted(request), &ctx)
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
            .handle(&AetherEvent::SubAgentStarted(request), &ctx)
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
}
