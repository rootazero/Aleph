//! Exec Approval Handler
//!
//! Manages exec command approval workflow with Discord interactions.

use crate::sync_primitives::Arc;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use tokio::sync::RwLock;

/// Pending exec request
#[derive(Debug, Clone)]
pub struct PendingExec {
    /// Unique approval ID
    pub id: String,
    /// User who requested the exec
    pub user_id: u64,
    /// Command to execute
    pub command: String,
    /// When the request was made
    pub created_at: DateTime<Utc>,
    /// Approval status
    pub status: ApprovalStatus,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ApprovalStatus {
    Pending,
    Approved,
    Denied,
    Expired,
}

/// Approval queue errors
#[derive(Debug, thiserror::Error)]
pub enum ApprovalError {
    #[error("approval request not found: {0}")]
    NotFound(String),

    #[error("already {0}: {1}")]
    AlreadyResolved(String, String),

    #[error("expired: {0}")]
    Expired(String),
}

/// Queue for managing exec command approvals
#[derive(Clone)]
pub struct ApprovalQueue {
    /// Pending approvals: approval_id -> PendingExec
    pending: Arc<RwLock<HashMap<String, PendingExec>>>,
    /// User's pending approvals: user_id -> Vec<approval_id>
    user_pending: Arc<RwLock<HashMap<u64, Vec<String>>>>,
    /// TTL in seconds
    ttl_secs: u64,
}

impl ApprovalQueue {
    /// Create a new ApprovalQueue
    pub fn new() -> Self {
        Self {
            pending: Arc::new(RwLock::new(HashMap::new())),
            user_pending: Arc::new(RwLock::new(HashMap::new())),
            ttl_secs: 3600, // 1 hour default
        }
    }

    /// Create a new pending exec request
    pub async fn create(&self, user_id: u64, command: String) -> String {
        let id = format!("exec_{}", uuid::Uuid::new_v4());

        let pending = PendingExec {
            id: id.clone(),
            user_id,
            command,
            created_at: Utc::now(),
            status: ApprovalStatus::Pending,
        };

        {
            let mut pending_guard = self.pending.write().await;
            pending_guard.insert(id.clone(), pending);
        }
        {
            let mut user_pending = self.user_pending.write().await;
            user_pending.entry(user_id).or_default().push(id.clone());
        }

        id
    }

    /// Approve an exec request
    pub async fn approve(&self, approval_id: &str) -> Result<PendingExec, ApprovalError> {
        self.resolve(approval_id, ApprovalStatus::Approved).await
    }

    /// Deny an exec request
    pub async fn deny(&self, approval_id: &str) -> Result<PendingExec, ApprovalError> {
        self.resolve(approval_id, ApprovalStatus::Denied).await
    }

    async fn resolve(
        &self,
        approval_id: &str,
        status: ApprovalStatus,
    ) -> Result<PendingExec, ApprovalError> {
        let mut pending_guard = self.pending.write().await;

        let pending = pending_guard
            .get_mut(approval_id)
            .ok_or_else(|| ApprovalError::NotFound(approval_id.to_string()))?;

        if pending.status != ApprovalStatus::Pending {
            return Err(ApprovalError::AlreadyResolved(
                format!("{:?}", pending.status),
                approval_id.to_string(),
            ));
        }

        let age = Utc::now()
            .signed_duration_since(pending.created_at)
            .num_seconds() as u64;
        if age > self.ttl_secs {
            pending.status = ApprovalStatus::Expired;
            return Err(ApprovalError::Expired(approval_id.to_string()));
        }

        pending.status = status;

        {
            let mut user_pending = self.user_pending.write().await;
            if let Some(ids) = user_pending.get_mut(&pending.user_id) {
                ids.retain(|id| id != approval_id);
            }
        }

        Ok(pending.clone())
    }

    /// Get a pending exec request
    pub async fn get(&self, approval_id: &str) -> Option<PendingExec> {
        let pending = self.pending.read().await;
        pending.get(approval_id).cloned()
    }

    /// List pending requests for a user
    pub async fn list_user_pending(&self, user_id: u64) -> Vec<PendingExec> {
        let pending = self.pending.read().await;
        pending
            .values()
            .filter(|p| p.user_id == user_id && p.status == ApprovalStatus::Pending)
            .cloned()
            .collect()
    }

    /// Clean up expired requests
    pub async fn cleanup_expired(&self) {
        let now = Utc::now();
        let mut pending_guard = self.pending.write().await;

        for pending in pending_guard.values_mut() {
            if pending.status == ApprovalStatus::Pending {
                let age = now.signed_duration_since(pending.created_at).num_seconds() as u64;
                if age > self.ttl_secs {
                    pending.status = ApprovalStatus::Expired;
                }
            }
        }
    }
}

impl Default for ApprovalQueue {
    fn default() -> Self {
        Self::new()
    }
}
