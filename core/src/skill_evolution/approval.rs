//! User approval workflow for skill solidification.
//!
//! This module manages the approval lifecycle for skill suggestions:
//! 1. Submit suggestions for user approval
//! 2. List pending approvals
//! 3. Approve/reject suggestions
//! 4. Track approval history
//!
//! ## Usage
//!
//! ```rust,ignore
//! use alephcore::skill_evolution::{ApprovalManager, SolidificationSuggestion};
//!
//! let manager = ApprovalManager::new();
//!
//! // Submit a suggestion for approval
//! manager.submit(suggestion)?;
//!
//! // List pending approvals
//! for request in manager.list_pending()? {
//!     println!("{}: {}", request.id, request.suggestion.suggested_name);
//! }
//!
//! // Approve a suggestion
//! let approved = manager.approve("request-id")?;
//! ```

use std::collections::HashMap;
use crate::sync_primitives::{Arc, RwLock};

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::error::{AlephError, Result};

use super::types::SolidificationSuggestion;

/// Status of an approval request
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalStatus {
    /// Awaiting user decision
    Pending,
    /// User approved the suggestion
    Approved,
    /// User rejected the suggestion
    Rejected,
    /// Suggestion expired (too old)
    Expired,
}

/// A request for user approval of a skill suggestion
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRequest {
    /// Unique request ID
    pub id: String,
    /// The skill suggestion to approve
    pub suggestion: SolidificationSuggestion,
    /// Current approval status
    pub status: ApprovalStatus,
    /// When the request was created (unix timestamp)
    pub created_at: i64,
    /// When the request was last updated (unix timestamp)
    pub updated_at: i64,
    /// Optional user notes
    pub notes: Option<String>,
}

impl ApprovalRequest {
    /// Create a new pending approval request
    pub fn new(suggestion: SolidificationSuggestion) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        Self {
            id: uuid::Uuid::new_v4().to_string(),
            suggestion,
            status: ApprovalStatus::Pending,
            created_at: now,
            updated_at: now,
            notes: None,
        }
    }

    /// Check if the request is still pending
    pub fn is_pending(&self) -> bool {
        self.status == ApprovalStatus::Pending
    }

    /// Check if the request has been approved
    pub fn is_approved(&self) -> bool {
        self.status == ApprovalStatus::Approved
    }

    /// Check if the request is expired (older than max_age_days)
    pub fn is_expired(&self, max_age_days: u32) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let age_days = (now - self.created_at) / 86400;
        age_days > max_age_days as i64
    }
}

/// Configuration for the approval manager
#[derive(Debug, Clone)]
pub struct ApprovalConfig {
    /// Maximum days to keep pending requests before expiring
    pub max_pending_days: u32,
    /// Maximum number of pending requests to keep
    pub max_pending_count: usize,
    /// Auto-reject expired requests
    pub auto_expire: bool,
}

impl Default for ApprovalConfig {
    fn default() -> Self {
        Self {
            max_pending_days: 7,
            max_pending_count: 20,
            auto_expire: true,
        }
    }
}

/// Manager for skill approval workflow.
///
/// Handles the lifecycle of approval requests from submission to resolution.
/// Thread-safe for concurrent access.
pub struct ApprovalManager {
    /// Pending and historical requests
    requests: Arc<RwLock<HashMap<String, ApprovalRequest>>>,
    /// Configuration
    config: ApprovalConfig,
}

impl ApprovalManager {
    /// Create a new approval manager with default config
    pub fn new() -> Self {
        Self {
            requests: Arc::new(RwLock::new(HashMap::new())),
            config: ApprovalConfig::default(),
        }
    }

    /// Create with custom configuration
    pub fn with_config(config: ApprovalConfig) -> Self {
        Self {
            requests: Arc::new(RwLock::new(HashMap::new())),
            config,
        }
    }

    /// Submit a suggestion for user approval.
    ///
    /// Returns the request ID for tracking.
    pub fn submit(&self, suggestion: SolidificationSuggestion) -> Result<String> {
        let mut requests = self.requests.write().map_err(|_| AlephError::Other {
            message: "Failed to acquire lock".to_string(),
            suggestion: None,
        })?;

        // Check if we already have a pending request for this pattern
        for (_, req) in requests.iter() {
            if req.is_pending() && req.suggestion.pattern_id == suggestion.pattern_id {
                debug!(
                    pattern_id = %suggestion.pattern_id,
                    "Duplicate pending request, ignoring"
                );
                return Ok(req.id.clone());
            }
        }

        // Enforce max pending count
        let pending_count = requests.values().filter(|r| r.is_pending()).count();
        if pending_count >= self.config.max_pending_count {
            warn!(
                count = pending_count,
                max = self.config.max_pending_count,
                "Max pending requests reached"
            );
            return Err(AlephError::Other {
                message: format!(
                    "Maximum pending requests ({}) reached. Please approve or reject existing requests.",
                    self.config.max_pending_count
                ),
                suggestion: Some("Use list_pending() to view and resolve existing requests".to_string()),
            });
        }

        let request = ApprovalRequest::new(suggestion);
        let id = request.id.clone();

        info!(
            id = %id,
            name = %request.suggestion.suggested_name,
            "Submitted suggestion for approval"
        );

        requests.insert(id.clone(), request);
        Ok(id)
    }

    /// Submit multiple suggestions for approval.
    ///
    /// Returns the IDs of successfully submitted requests.
    pub fn submit_batch(&self, suggestions: Vec<SolidificationSuggestion>) -> Result<Vec<String>> {
        let mut ids = Vec::new();
        for suggestion in suggestions {
            match self.submit(suggestion) {
                Ok(id) => ids.push(id),
                Err(e) => {
                    warn!(error = %e, "Failed to submit suggestion");
                    // Continue with remaining suggestions
                }
            }
        }
        Ok(ids)
    }

    /// List all pending approval requests.
    pub fn list_pending(&self) -> Result<Vec<ApprovalRequest>> {
        let mut requests = self.requests.write().map_err(|_| AlephError::Other {
            message: "Failed to acquire lock".to_string(),
            suggestion: None,
        })?;

        // Auto-expire old requests if configured
        if self.config.auto_expire {
            let expired_ids: Vec<String> = requests
                .iter()
                .filter(|(_, r)| r.is_pending() && r.is_expired(self.config.max_pending_days))
                .map(|(id, _)| id.clone())
                .collect();

            for id in expired_ids {
                if let Some(req) = requests.get_mut(&id) {
                    req.status = ApprovalStatus::Expired;
                    req.updated_at = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
                        .as_secs() as i64;
                    debug!(id = %id, "Auto-expired pending request");
                }
            }
        }

        let pending: Vec<ApprovalRequest> = requests
            .values()
            .filter(|r| r.is_pending())
            .cloned()
            .collect();

        Ok(pending)
    }

    /// Get a specific request by ID.
    pub fn get(&self, request_id: &str) -> Result<Option<ApprovalRequest>> {
        let requests = self.requests.read().map_err(|_| AlephError::Other {
            message: "Failed to acquire lock".to_string(),
            suggestion: None,
        })?;

        Ok(requests.get(request_id).cloned())
    }

    /// Approve a pending request.
    ///
    /// Returns the approved suggestion for skill generation.
    pub fn approve(&self, request_id: &str) -> Result<SolidificationSuggestion> {
        self.approve_with_notes(request_id, None)
    }

    /// Approve a pending request with optional notes.
    pub fn approve_with_notes(
        &self,
        request_id: &str,
        notes: Option<String>,
    ) -> Result<SolidificationSuggestion> {
        let mut requests = self.requests.write().map_err(|_| AlephError::Other {
            message: "Failed to acquire lock".to_string(),
            suggestion: None,
        })?;

        let request = requests.get_mut(request_id).ok_or_else(|| AlephError::Other {
            message: format!("Request not found: {}", request_id),
            suggestion: None,
        })?;

        if !request.is_pending() {
            return Err(AlephError::Other {
                message: format!(
                    "Request is not pending (status: {:?})",
                    request.status
                ),
                suggestion: None,
            });
        }

        request.status = ApprovalStatus::Approved;
        request.updated_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        request.notes = notes;

        info!(
            id = %request_id,
            name = %request.suggestion.suggested_name,
            "Approved suggestion"
        );

        Ok(request.suggestion.clone())
    }

    /// Reject a pending request.
    pub fn reject(&self, request_id: &str) -> Result<()> {
        self.reject_with_reason(request_id, None)
    }

    /// Reject a pending request with optional reason.
    pub fn reject_with_reason(&self, request_id: &str, reason: Option<String>) -> Result<()> {
        let mut requests = self.requests.write().map_err(|_| AlephError::Other {
            message: "Failed to acquire lock".to_string(),
            suggestion: None,
        })?;

        let request = requests.get_mut(request_id).ok_or_else(|| AlephError::Other {
            message: format!("Request not found: {}", request_id),
            suggestion: None,
        })?;

        if !request.is_pending() {
            return Err(AlephError::Other {
                message: format!(
                    "Request is not pending (status: {:?})",
                    request.status
                ),
                suggestion: None,
            });
        }

        request.status = ApprovalStatus::Rejected;
        request.updated_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        request.notes = reason;

        info!(
            id = %request_id,
            name = %request.suggestion.suggested_name,
            "Rejected suggestion"
        );

        Ok(())
    }

    /// Get count of pending requests
    pub fn pending_count(&self) -> Result<usize> {
        let requests = self.requests.read().map_err(|_| AlephError::Other {
            message: "Failed to acquire lock".to_string(),
            suggestion: None,
        })?;

        Ok(requests.values().filter(|r| r.is_pending()).count())
    }

    /// Clear all resolved (non-pending) requests to free memory.
    pub fn clear_resolved(&self) -> Result<usize> {
        let mut requests = self.requests.write().map_err(|_| AlephError::Other {
            message: "Failed to acquire lock".to_string(),
            suggestion: None,
        })?;

        let before = requests.len();
        requests.retain(|_, r| r.is_pending());
        let removed = before - requests.len();

        if removed > 0 {
            debug!(count = removed, "Cleared resolved requests");
        }

        Ok(removed)
    }

    /// Export all requests for persistence.
    pub fn export(&self) -> Result<Vec<ApprovalRequest>> {
        let requests = self.requests.read().map_err(|_| AlephError::Other {
            message: "Failed to acquire lock".to_string(),
            suggestion: None,
        })?;

        Ok(requests.values().cloned().collect())
    }

    /// Import requests from persistence.
    pub fn import(&self, requests: Vec<ApprovalRequest>) -> Result<()> {
        let mut store = self.requests.write().map_err(|_| AlephError::Other {
            message: "Failed to acquire lock".to_string(),
            suggestion: None,
        })?;

        for request in requests {
            store.insert(request.id.clone(), request);
        }

        Ok(())
    }
}

impl Default for ApprovalManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill_evolution::types::SkillMetrics;

    fn create_test_suggestion() -> SolidificationSuggestion {
        SolidificationSuggestion {
            pattern_id: "test-pattern".to_string(),
            suggested_name: "test-skill".to_string(),
            suggested_description: "A test skill".to_string(),
            confidence: 0.9,
            metrics: SkillMetrics::new("test-pattern"),
            sample_contexts: vec!["test context".to_string()],
            instructions_preview: "# Instructions\n\nTest instructions.".to_string(),
        }
    }

    #[test]
    fn test_approval_manager_creation() {
        let manager = ApprovalManager::new();
        assert_eq!(manager.pending_count().unwrap(), 0);
    }

    #[test]
    fn test_submit_suggestion() {
        let manager = ApprovalManager::new();
        let suggestion = create_test_suggestion();

        let id = manager.submit(suggestion).unwrap();
        assert!(!id.is_empty());
        assert_eq!(manager.pending_count().unwrap(), 1);
    }

    #[test]
    fn test_list_pending() {
        let manager = ApprovalManager::new();
        manager.submit(create_test_suggestion()).unwrap();

        let pending = manager.list_pending().unwrap();
        assert_eq!(pending.len(), 1);
        assert!(pending[0].is_pending());
    }

    #[test]
    fn test_approve_request() {
        let manager = ApprovalManager::new();
        let id = manager.submit(create_test_suggestion()).unwrap();

        let approved = manager.approve(&id).unwrap();
        assert_eq!(approved.suggested_name, "test-skill");

        let request = manager.get(&id).unwrap().unwrap();
        assert!(request.is_approved());
        assert_eq!(manager.pending_count().unwrap(), 0);
    }

    #[test]
    fn test_reject_request() {
        let manager = ApprovalManager::new();
        let id = manager.submit(create_test_suggestion()).unwrap();

        manager.reject_with_reason(&id, Some("Not useful".to_string())).unwrap();

        let request = manager.get(&id).unwrap().unwrap();
        assert_eq!(request.status, ApprovalStatus::Rejected);
        assert_eq!(request.notes, Some("Not useful".to_string()));
    }

    #[test]
    fn test_cannot_approve_twice() {
        let manager = ApprovalManager::new();
        let id = manager.submit(create_test_suggestion()).unwrap();

        manager.approve(&id).unwrap();
        let result = manager.approve(&id);

        assert!(result.is_err());
    }

    #[test]
    fn test_duplicate_pattern_ignored() {
        let manager = ApprovalManager::new();
        let suggestion = create_test_suggestion();

        let id1 = manager.submit(suggestion.clone()).unwrap();
        let id2 = manager.submit(suggestion).unwrap();

        // Should return the same ID (duplicate detected)
        assert_eq!(id1, id2);
        assert_eq!(manager.pending_count().unwrap(), 1);
    }

    #[test]
    fn test_max_pending_enforced() {
        let config = ApprovalConfig {
            max_pending_count: 2,
            ..Default::default()
        };
        let manager = ApprovalManager::with_config(config);

        // Submit two different patterns
        let mut s1 = create_test_suggestion();
        s1.pattern_id = "pattern-1".to_string();
        manager.submit(s1).unwrap();

        let mut s2 = create_test_suggestion();
        s2.pattern_id = "pattern-2".to_string();
        manager.submit(s2).unwrap();

        // Third should fail
        let mut s3 = create_test_suggestion();
        s3.pattern_id = "pattern-3".to_string();
        let result = manager.submit(s3);

        assert!(result.is_err());
    }

    #[test]
    fn test_clear_resolved() {
        let manager = ApprovalManager::new();
        let id1 = manager.submit(create_test_suggestion()).unwrap();

        let mut s2 = create_test_suggestion();
        s2.pattern_id = "pattern-2".to_string();
        let id2 = manager.submit(s2).unwrap();

        // Approve one
        manager.approve(&id1).unwrap();

        // Clear resolved
        let cleared = manager.clear_resolved().unwrap();
        assert_eq!(cleared, 1);
        assert_eq!(manager.pending_count().unwrap(), 1);

        // Remaining request should still be accessible
        assert!(manager.get(&id2).unwrap().is_some());
    }

    #[test]
    fn test_export_import() {
        let manager1 = ApprovalManager::new();
        manager1.submit(create_test_suggestion()).unwrap();

        let exported = manager1.export().unwrap();
        assert_eq!(exported.len(), 1);

        let manager2 = ApprovalManager::new();
        manager2.import(exported).unwrap();
        assert_eq!(manager2.pending_count().unwrap(), 1);
    }

    #[test]
    fn test_approval_request_is_expired() {
        let suggestion = create_test_suggestion();
        let mut request = ApprovalRequest::new(suggestion);

        // Not expired (just created)
        assert!(!request.is_expired(7));

        // Simulate old request (8 days ago)
        request.created_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
            - (8 * 86400);

        assert!(request.is_expired(7));
    }
}
