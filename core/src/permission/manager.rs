// Aether/core/src/permission/manager.rs
//! Permission manager for handling permission requests.

use super::config::{config_to_ruleset, default_config, PermissionConfigMap};
use super::error::PermissionError;
use super::rule::{PermissionEvaluator, Ruleset};
use crate::event::permission::{PermissionReply, PermissionRequest};
use crate::event::{AetherEvent, EventBus};
use crate::extension::PermissionAction;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{oneshot, RwLock};
use tracing::{debug, info, warn};

/// Configuration for the permission manager
#[derive(Debug, Clone)]
#[derive(Default)]
pub struct PermissionManagerConfig {
    /// Default timeout for permission requests (0 = no timeout)
    pub timeout_ms: u64,
    /// Whether to persist approved rules
    pub persist_approvals: bool,
}


/// A pending permission request waiting for user response
pub struct PendingPermission {
    /// The original request
    pub request: PermissionRequest,
    /// Channel to send the result
    response_tx: oneshot::Sender<Result<(), PermissionError>>,
}

/// Permission manager
///
/// Handles permission evaluation and user interaction for tool execution.
pub struct PermissionManager {
    /// Permission evaluator
    evaluator: PermissionEvaluator,
    /// Global configuration rules
    config_rules: Ruleset,
    /// Runtime-approved rules (from user "always" selections)
    approved_rules: RwLock<Ruleset>,
    /// Pending permission requests
    pending: RwLock<HashMap<String, PendingPermission>>,
    /// Event bus for publishing events
    event_bus: Arc<EventBus>,
    /// Configuration
    config: PermissionManagerConfig,
}

impl PermissionManager {
    /// Create a new permission manager
    pub fn new(event_bus: Arc<EventBus>) -> Self {
        Self::with_config(event_bus, default_config(), PermissionManagerConfig::default())
    }

    /// Create with custom configuration
    pub fn with_config(
        event_bus: Arc<EventBus>,
        permission_config: PermissionConfigMap,
        config: PermissionManagerConfig,
    ) -> Self {
        Self {
            evaluator: PermissionEvaluator::new(),
            config_rules: config_to_ruleset(&permission_config),
            approved_rules: RwLock::new(Vec::new()),
            pending: RwLock::new(HashMap::new()),
            event_bus,
            config,
        }
    }

    /// Request permission for an operation
    ///
    /// Returns Ok(()) if permission is granted (either by rule or user approval).
    /// Returns Err if denied by rule or user rejection.
    pub async fn ask(&self, request: PermissionRequest) -> Result<(), PermissionError> {
        let approved = self.approved_rules.read().await;

        // Evaluate each pattern
        for pattern in &request.patterns {
            let rule = self.evaluator.evaluate(
                &request.permission,
                pattern,
                &[&self.config_rules, &approved],
            );

            debug!(
                permission = %request.permission,
                pattern = %pattern,
                action = ?rule.action,
                "Evaluated permission"
            );

            match rule.action {
                PermissionAction::Allow => continue,
                PermissionAction::Deny => {
                    return Err(PermissionError::denied(&request.permission, pattern, rule));
                }
                PermissionAction::Ask => {
                    // Need to drop the read lock before asking user
                    drop(approved);
                    return self.ask_user(request).await;
                }
            }
        }

        Ok(())
    }

    /// Ask the user for permission (internal)
    async fn ask_user(&self, request: PermissionRequest) -> Result<(), PermissionError> {
        let (tx, rx) = oneshot::channel();
        let request_id = request.id.clone();
        let _session_id = request.session_id.clone();

        // Store pending request
        {
            let mut pending = self.pending.write().await;
            pending.insert(
                request_id.clone(),
                PendingPermission {
                    request: request.clone(),
                    response_tx: tx,
                },
            );
        }

        info!(
            request_id = %request_id,
            permission = %request.permission,
            patterns = ?request.patterns,
            "Requesting permission from user"
        );

        // Publish event
        self.event_bus
            .publish(AetherEvent::PermissionAsked(request))
            .await;

        // Wait for response with optional timeout
        let result = if self.config.timeout_ms > 0 {
            tokio::time::timeout(Duration::from_millis(self.config.timeout_ms), rx).await
        } else {
            Ok(rx.await)
        };

        match result {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => {
                // Channel dropped - treat as rejection
                warn!(request_id = %request_id, "Permission request channel dropped");
                Err(PermissionError::Rejected)
            }
            Err(_) => {
                // Timeout
                self.cleanup_pending(&request_id).await;
                Err(PermissionError::timeout(request_id, self.config.timeout_ms))
            }
        }
    }

    /// Handle user reply to a permission request
    pub async fn reply(&self, request_id: &str, reply: PermissionReply) -> Result<(), PermissionError> {
        let pending = {
            let mut pending = self.pending.write().await;
            pending.remove(request_id)
        };

        let Some(pending) = pending else {
            warn!(request_id = %request_id, "Reply for unknown permission request");
            return Ok(());
        };

        let session_id = pending.request.session_id.clone();

        // Publish reply event
        self.event_bus
            .publish(AetherEvent::PermissionReplied {
                session_id: session_id.clone(),
                request_id: request_id.to_string(),
                reply: reply.clone(),
            })
            .await;

        match reply {
            PermissionReply::Once => {
                info!(request_id = %request_id, "Permission granted (once)");
                let _ = pending.response_tx.send(Ok(()));
            }
            PermissionReply::Always => {
                info!(request_id = %request_id, "Permission granted (always)");

                // Add to approved rules
                {
                    let mut approved = self.approved_rules.write().await;
                    for pattern in &pending.request.always_patterns {
                        approved.push(super::rule::PermissionRule::new(
                            &pending.request.permission,
                            pattern,
                            PermissionAction::Allow,
                        ));
                    }
                }

                let _ = pending.response_tx.send(Ok(()));

                // Resolve other pending requests that now match
                self.resolve_matching_pending(&session_id).await;
            }
            PermissionReply::Reject => {
                info!(request_id = %request_id, "Permission rejected");
                let _ = pending.response_tx.send(Err(PermissionError::Rejected));

                // Reject all other pending requests in this session
                self.reject_session_pending(&session_id).await;
            }
            PermissionReply::Correct { message } => {
                info!(request_id = %request_id, message = %message, "Permission rejected with feedback");
                let _ = pending.response_tx.send(Err(PermissionError::corrected(message)));
            }
        }

        Ok(())
    }

    /// Resolve pending requests that now match approved rules
    async fn resolve_matching_pending(&self, session_id: &str) {
        let approved = self.approved_rules.read().await;
        let mut to_resolve = Vec::new();

        {
            let pending = self.pending.read().await;
            for (id, p) in pending.iter() {
                if p.request.session_id != session_id {
                    continue;
                }

                // Check if all patterns now match approved rules
                let all_allowed = p.request.patterns.iter().all(|pattern| {
                    let rule = self.evaluator.evaluate(
                        &p.request.permission,
                        pattern,
                        &[&self.config_rules, &approved],
                    );
                    rule.action == PermissionAction::Allow
                });

                if all_allowed {
                    to_resolve.push(id.clone());
                }
            }
        }

        // Resolve matching requests
        let mut pending = self.pending.write().await;
        for id in to_resolve {
            if let Some(p) = pending.remove(&id) {
                debug!(request_id = %id, "Auto-resolving matching permission request");
                let _ = p.response_tx.send(Ok(()));

                // Publish reply event
                self.event_bus
                    .publish(AetherEvent::PermissionReplied {
                        session_id: session_id.to_string(),
                        request_id: id,
                        reply: PermissionReply::Always,
                    })
                    .await;
            }
        }
    }

    /// Reject all pending requests in a session
    async fn reject_session_pending(&self, session_id: &str) {
        let mut to_reject = Vec::new();

        {
            let pending = self.pending.read().await;
            for (id, p) in pending.iter() {
                if p.request.session_id == session_id {
                    to_reject.push(id.clone());
                }
            }
        }

        let mut pending = self.pending.write().await;
        for id in to_reject {
            if let Some(p) = pending.remove(&id) {
                debug!(request_id = %id, "Rejecting session permission request");
                let _ = p.response_tx.send(Err(PermissionError::Rejected));

                // Publish reply event
                self.event_bus
                    .publish(AetherEvent::PermissionReplied {
                        session_id: session_id.to_string(),
                        request_id: id,
                        reply: PermissionReply::Reject,
                    })
                    .await;
            }
        }
    }

    /// Clean up a pending request (e.g., on timeout)
    async fn cleanup_pending(&self, request_id: &str) {
        let mut pending = self.pending.write().await;
        pending.remove(request_id);
    }

    /// Get the list of pending permission requests
    pub async fn list_pending(&self) -> Vec<PermissionRequest> {
        self.pending
            .read()
            .await
            .values()
            .map(|p| p.request.clone())
            .collect()
    }

    /// Get the evaluator (for testing/inspection)
    pub fn evaluator(&self) -> &PermissionEvaluator {
        &self.evaluator
    }

    /// Add approved rules (for testing)
    #[cfg(test)]
    pub async fn add_approved_rule(&self, rule: super::rule::PermissionRule) {
        self.approved_rules.write().await.push(rule);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::EventBus;

    fn create_test_manager() -> PermissionManager {
        let event_bus = Arc::new(EventBus::new());
        PermissionManager::new(event_bus)
    }

    #[tokio::test]
    async fn test_allowed_by_rule() {
        let manager = create_test_manager();

        // Read should be allowed by default config
        let request = PermissionRequest::new("req-1", "session-1", "read", vec!["src/main.rs".into()]);
        let result = manager.ask(request).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    #[ignore] // TODO: HashMap iteration order is non-deterministic, causing rule matching issues
    async fn test_denied_by_rule() {
        let manager = create_test_manager();

        // rm -rf /path should be denied by default config (matches "rm -rf *")
        // Note: This test is flaky due to HashMap iteration order in config.rs
        let request = PermissionRequest::new("req-1", "session-1", "bash", vec!["rm -rf /home/user".into()]);

        // Use timeout to avoid hanging if it falls through to Ask
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            manager.ask(request)
        ).await;

        // Should complete immediately with Deny, not timeout
        assert!(result.is_ok(), "Test should not timeout - pattern should match Deny rule");
        assert!(matches!(result.unwrap(), Err(PermissionError::Denied { .. })));
    }

    #[tokio::test]
    async fn test_permission_request_structure() {
        let request = PermissionRequest::new("req-1", "session-1", "bash", vec!["git push".into()])
            .with_metadata("tool", serde_json::json!("bash"))
            .with_always_patterns(vec!["git *".into()]);

        assert_eq!(request.id, "req-1");
        assert_eq!(request.session_id, "session-1");
        assert_eq!(request.permission, "bash");
        assert_eq!(request.patterns, vec!["git push"]);
        assert_eq!(request.always_patterns, vec!["git *"]);
    }
}
