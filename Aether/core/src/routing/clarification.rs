//! Clarification Flow Integration
//!
//! Manages the clarification flow for parameter collection:
//!
//! - Session creation and storage
//! - Context preservation
//! - Resume handling
//! - Timeout and cleanup

use crate::routing::{
    AggregatedIntent, ClarificationConfig, ClarificationError, ClarificationInputType,
    ClarificationRequest, ResumeResult, RoutingContext,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, warn};
use uuid::Uuid;

/// Pending clarification session
#[derive(Debug, Clone)]
pub struct PendingClarification {
    /// Session ID
    pub session_id: String,

    /// Original routing context
    pub context: RoutingContext,

    /// Intent being clarified
    pub intent: AggregatedIntent,

    /// Parameter being requested
    pub param_name: String,

    /// Creation timestamp
    pub created_at: Instant,

    /// Timeout duration
    pub timeout: Duration,
}

impl PendingClarification {
    /// Create a new pending clarification
    pub fn new(
        context: RoutingContext,
        intent: AggregatedIntent,
        param_name: impl Into<String>,
        timeout: Duration,
    ) -> Self {
        Self {
            session_id: Uuid::new_v4().to_string(),
            context,
            intent,
            param_name: param_name.into(),
            created_at: Instant::now(),
            timeout,
        }
    }

    /// Check if this session has expired
    pub fn is_expired(&self) -> bool {
        self.created_at.elapsed() > self.timeout
    }

    /// Get remaining time before expiration
    pub fn remaining_time(&self) -> Duration {
        self.timeout.saturating_sub(self.created_at.elapsed())
    }

    /// Create a clarification request for UI
    pub fn to_request(&self) -> ClarificationRequest {
        // Get missing param info
        let missing = self.intent.missing_parameters
            .iter()
            .find(|p| p.name == self.param_name);

        let (prompt, suggestions, input_type) = if let Some(param) = missing {
            let input_type = if param.suggestions.is_empty() {
                ClarificationInputType::Text
            } else {
                ClarificationInputType::Select
            };
            (param.clarification_prompt.clone(), param.suggestions.clone(), input_type)
        } else {
            (
                format!("请提供 {}:", self.param_name),
                Vec::new(),
                ClarificationInputType::Text,
            )
        };

        ClarificationRequest::new(&self.session_id, prompt)
            .with_suggestions(suggestions)
            .with_param_name(&self.param_name)
            .with_tool_name(self.intent.tool_name().unwrap_or("unknown"))
            .with_input_type(input_type)
    }
}

/// Clarification Flow Integrator
///
/// Manages pending clarification sessions and handles resume flow.
pub struct ClarificationIntegrator {
    /// Pending clarification sessions
    sessions: Arc<RwLock<HashMap<String, PendingClarification>>>,

    /// Configuration
    config: ClarificationConfig,

    /// Cleanup task handle
    cleanup_handle: Option<tokio::task::JoinHandle<()>>,
}

impl ClarificationIntegrator {
    /// Create a new clarification integrator
    pub fn new(config: ClarificationConfig) -> Self {
        let sessions = Arc::new(RwLock::new(HashMap::new()));

        Self {
            sessions,
            config,
            cleanup_handle: None,
        }
    }

    /// Start the background cleanup task
    pub fn start_cleanup_task(&mut self) {
        if !self.config.auto_cleanup {
            return;
        }

        let sessions = Arc::clone(&self.sessions);
        let interval = self.config.cleanup_interval();

        let handle = tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                Self::cleanup_expired_sessions(&sessions).await;
            }
        });

        self.cleanup_handle = Some(handle);
    }

    /// Stop the cleanup task
    pub fn stop_cleanup_task(&mut self) {
        if let Some(handle) = self.cleanup_handle.take() {
            handle.abort();
        }
    }

    /// Start a new clarification session
    ///
    /// # Arguments
    ///
    /// * `context` - Original routing context
    /// * `intent` - Intent needing clarification
    ///
    /// # Returns
    ///
    /// `ClarificationRequest` to send to UI, or error if session limit reached
    pub async fn start_clarification(
        &self,
        context: RoutingContext,
        intent: AggregatedIntent,
    ) -> Result<ClarificationRequest, ClarificationError> {
        // Check session limit
        let current_count = self.sessions.read().await.len();
        if current_count >= self.config.max_pending {
            warn!(
                current = current_count,
                max = self.config.max_pending,
                "Clarification session limit reached"
            );
            return Err(ClarificationError::Internal(
                "Too many pending clarification sessions".to_string()
            ));
        }

        // Get first missing parameter
        let param_name = intent
            .missing_parameters
            .first()
            .map(|p| p.name.clone())
            .ok_or_else(|| {
                ClarificationError::Internal("No missing parameters to clarify".to_string())
            })?;

        // Create pending session
        let pending = PendingClarification::new(
            context,
            intent,
            &param_name,
            self.config.timeout(),
        );

        let request = pending.to_request();
        let session_id = pending.session_id.clone();

        // Store session
        {
            let mut sessions = self.sessions.write().await;
            sessions.insert(session_id.clone(), pending);
        }

        debug!(
            session_id,
            param_name,
            "Started clarification session"
        );

        Ok(request)
    }

    /// Resume a clarification session with user input
    ///
    /// # Arguments
    ///
    /// * `session_id` - Session ID from the original request
    /// * `user_input` - Value provided by user
    ///
    /// # Returns
    ///
    /// `ResumeResult` with updated context and intent
    pub async fn resume(
        &self,
        session_id: &str,
        user_input: &str,
    ) -> Result<ResumeResult, ClarificationError> {
        // Find and remove session
        let pending = {
            let mut sessions = self.sessions.write().await;
            sessions.remove(session_id)
        };

        let mut pending = pending.ok_or(ClarificationError::SessionNotFound)?;

        // Check expiration
        if pending.is_expired() {
            return Err(ClarificationError::Timeout);
        }

        // Validate user input
        if user_input.trim().is_empty() {
            return Err(ClarificationError::InvalidInput(
                "Empty input provided".to_string()
            ));
        }

        debug!(
            session_id,
            param_name = pending.param_name,
            user_input,
            "Resuming clarification"
        );

        // Update parameters with user input
        let param_name = pending.param_name.clone();
        Self::update_parameters(&mut pending.intent, &param_name, user_input);

        // Remove this parameter from missing list
        pending.intent.missing_parameters.retain(|p| p.name != param_name);

        // Check if more parameters are missing
        if pending.intent.missing_parameters.is_empty() {
            pending.intent.parameters_complete = true;
            // Update action to Execute or RequestConfirmation
            pending.intent.action = crate::routing::IntentAction::Execute;
        }

        Ok(ResumeResult::new(pending.context, pending.intent))
    }

    /// Cancel a clarification session
    pub async fn cancel(&self, session_id: &str) -> Result<(), ClarificationError> {
        let mut sessions = self.sessions.write().await;
        sessions.remove(session_id)
            .ok_or(ClarificationError::SessionNotFound)?;

        debug!(session_id, "Cancelled clarification session");
        Ok(())
    }

    /// Get a pending session
    pub async fn get_session(&self, session_id: &str) -> Option<PendingClarification> {
        self.sessions.read().await.get(session_id).cloned()
    }

    /// Get all pending session IDs
    pub async fn pending_session_ids(&self) -> Vec<String> {
        self.sessions.read().await.keys().cloned().collect()
    }

    /// Get count of pending sessions
    pub async fn pending_count(&self) -> usize {
        self.sessions.read().await.len()
    }

    /// Cleanup expired sessions
    pub async fn cleanup_expired(&self) -> usize {
        Self::cleanup_expired_sessions(&self.sessions).await
    }

    /// Internal cleanup helper
    async fn cleanup_expired_sessions(
        sessions: &Arc<RwLock<HashMap<String, PendingClarification>>>
    ) -> usize {
        let mut sessions = sessions.write().await;
        let expired: Vec<String> = sessions
            .iter()
            .filter(|(_, v)| v.is_expired())
            .map(|(k, _)| k.clone())
            .collect();

        let count = expired.len();
        for session_id in expired {
            sessions.remove(&session_id);
            debug!(session_id, "Cleaned up expired clarification session");
        }

        if count > 0 {
            debug!(count, "Cleaned up expired clarification sessions");
        }

        count
    }

    /// Update intent parameters with user input
    fn update_parameters(intent: &mut AggregatedIntent, param_name: &str, value: &str) {
        // Get mutable access to parameters
        let params = &mut intent.primary.parameters;

        // Ensure it's an object
        if !params.is_object() {
            *params = serde_json::json!({});
        }

        // Insert the new parameter
        if let Some(obj) = params.as_object_mut() {
            obj.insert(param_name.to_string(), serde_json::Value::String(value.to_string()));
        }
    }

    /// Get configuration
    pub fn config(&self) -> &ClarificationConfig {
        &self.config
    }
}

impl Drop for ClarificationIntegrator {
    fn drop(&mut self) {
        self.stop_cleanup_task();
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::{ToolSource, UnifiedTool};
    use crate::routing::{IntentAction, IntentSignal, ParameterRequirement, RoutingLayerType};

    fn create_test_intent_with_missing_params() -> AggregatedIntent {
        let tool = UnifiedTool::new("search", "search", "Search tool", ToolSource::Native);
        let signal = IntentSignal::with_tool(RoutingLayerType::L1Regex, tool, 0.9);

        let mut intent = AggregatedIntent::new(signal, IntentAction::Execute);
        intent.parameters_complete = false;
        intent.missing_parameters = vec![
            ParameterRequirement::new("location", "位置")
                .with_suggestions(vec!["北京".to_string(), "上海".to_string()]),
        ];

        intent
    }

    #[tokio::test]
    async fn test_integrator_creation() {
        let config = ClarificationConfig::default();
        let integrator = ClarificationIntegrator::new(config);

        assert_eq!(integrator.pending_count().await, 0);
    }

    #[tokio::test]
    async fn test_start_clarification() {
        let config = ClarificationConfig::default();
        let integrator = ClarificationIntegrator::new(config);

        let ctx = RoutingContext::new("test input");
        let intent = create_test_intent_with_missing_params();

        let result = integrator.start_clarification(ctx, intent).await;
        assert!(result.is_ok());

        let request = result.unwrap();
        assert!(!request.session_id.is_empty());
        assert_eq!(request.param_name, "location");
        assert!(!request.suggestions.is_empty());

        assert_eq!(integrator.pending_count().await, 1);
    }

    #[tokio::test]
    async fn test_resume_clarification() {
        let config = ClarificationConfig::default();
        let integrator = ClarificationIntegrator::new(config);

        let ctx = RoutingContext::new("search weather");
        let intent = create_test_intent_with_missing_params();

        let request = integrator.start_clarification(ctx, intent).await.unwrap();
        let session_id = request.session_id.clone();

        // Resume with user input
        let result = integrator.resume(&session_id, "北京").await;
        assert!(result.is_ok());

        let resume_result = result.unwrap();
        assert!(resume_result.is_complete());
        assert!(matches!(resume_result.intent.action, IntentAction::Execute));

        // Session should be removed
        assert_eq!(integrator.pending_count().await, 0);
    }

    #[tokio::test]
    async fn test_resume_not_found() {
        let config = ClarificationConfig::default();
        let integrator = ClarificationIntegrator::new(config);

        let result = integrator.resume("non-existent", "value").await;
        assert!(matches!(result, Err(ClarificationError::SessionNotFound)));
    }

    #[tokio::test]
    async fn test_resume_empty_input() {
        let config = ClarificationConfig::default();
        let integrator = ClarificationIntegrator::new(config);

        let ctx = RoutingContext::new("test");
        let intent = create_test_intent_with_missing_params();
        let request = integrator.start_clarification(ctx, intent).await.unwrap();

        let result = integrator.resume(&request.session_id, "  ").await;
        assert!(matches!(result, Err(ClarificationError::InvalidInput(_))));
    }

    #[tokio::test]
    async fn test_cancel_clarification() {
        let config = ClarificationConfig::default();
        let integrator = ClarificationIntegrator::new(config);

        let ctx = RoutingContext::new("test");
        let intent = create_test_intent_with_missing_params();
        let request = integrator.start_clarification(ctx, intent).await.unwrap();

        assert_eq!(integrator.pending_count().await, 1);

        integrator.cancel(&request.session_id).await.unwrap();
        assert_eq!(integrator.pending_count().await, 0);
    }

    #[tokio::test]
    async fn test_session_limit() {
        let mut config = ClarificationConfig::default();
        config.max_pending = 2;
        let integrator = ClarificationIntegrator::new(config);

        // Fill to limit
        for _ in 0..2 {
            let ctx = RoutingContext::new("test");
            let intent = create_test_intent_with_missing_params();
            integrator.start_clarification(ctx, intent).await.unwrap();
        }

        // Third should fail
        let ctx = RoutingContext::new("test");
        let intent = create_test_intent_with_missing_params();
        let result = integrator.start_clarification(ctx, intent).await;

        assert!(matches!(result, Err(ClarificationError::Internal(_))));
    }

    #[test]
    fn test_pending_clarification_expiration() {
        let ctx = RoutingContext::new("test");
        let intent = create_test_intent_with_missing_params();
        let pending = PendingClarification::new(
            ctx,
            intent,
            "location",
            Duration::from_millis(10), // Very short timeout
        );

        // Wait for expiration
        std::thread::sleep(Duration::from_millis(20));
        assert!(pending.is_expired());
    }

    #[test]
    fn test_pending_clarification_to_request() {
        let ctx = RoutingContext::new("test");
        let intent = create_test_intent_with_missing_params();
        let pending = PendingClarification::new(ctx, intent, "location", Duration::from_secs(60));

        let request = pending.to_request();
        assert_eq!(request.param_name, "location");
        assert_eq!(request.tool_name, Some("search".to_string()));
        assert!(request.suggestions.contains(&"北京".to_string()));
    }

    #[tokio::test]
    async fn test_cleanup_expired() {
        let mut config = ClarificationConfig::default();
        config.timeout_seconds = 0; // Immediate expiration
        let integrator = ClarificationIntegrator::new(config);

        let ctx = RoutingContext::new("test");
        let intent = create_test_intent_with_missing_params();
        integrator.start_clarification(ctx, intent).await.unwrap();

        // Wait a bit for expiration
        tokio::time::sleep(Duration::from_millis(10)).await;

        let cleaned = integrator.cleanup_expired().await;
        assert_eq!(cleaned, 1);
        assert_eq!(integrator.pending_count().await, 0);
    }

    #[tokio::test]
    async fn test_parameter_update() {
        let tool = UnifiedTool::new("search", "search", "Search tool", ToolSource::Native);
        let signal = IntentSignal::with_tool(RoutingLayerType::L1Regex, tool, 0.9)
            .with_parameters(serde_json::json!({"query": "weather"}));

        let mut intent = AggregatedIntent::new(signal, IntentAction::Execute);

        ClarificationIntegrator::update_parameters(&mut intent, "location", "Beijing");

        let params = &intent.primary.parameters;
        assert!(params.get("query").is_some());
        assert!(params.get("location").is_some());
        assert_eq!(params["location"].as_str(), Some("Beijing"));
    }
}
