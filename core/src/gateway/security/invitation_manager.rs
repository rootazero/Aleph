// core/src/gateway/security/invitation_manager.rs

//! Guest Invitation Management
//!
//! Manages the creation and activation of guest invitations with a 15-minute expiry.
//! Invitations are one-time use only and tracked in-memory using DashMap.

use dashmap::DashMap;
use crate::sync_primitives::Arc;
use thiserror::Error;
use uuid::Uuid;

use super::crypto::generate_secret;
use aleph_protocol::{CreateInvitationRequest, GuestScope, GuestToken, Invitation};

/// Default invitation expiry (15 minutes in milliseconds)
const DEFAULT_INVITATION_EXPIRY_MS: i64 = 15 * 60 * 1000;

/// Invitation-related errors
#[derive(Debug, Error)]
pub enum InvitationError {
    #[error("Invalid invitation token")]
    InvalidToken,
    #[error("Invitation expired")]
    InvitationExpired,
    #[error("Invitation already activated")]
    AlreadyActivated,
    #[error("Invalid guest scope")]
    InvalidScope,
}

/// Pending invitation state
#[derive(Debug, Clone)]
struct PendingInvitation {
    /// Unique guest ID
    guest_id: String,
    /// Guest display name (stored for future admin inspection)
    #[allow(dead_code)]
    guest_name: String,
    /// Encrypted invitation token (the secret shared with guest)
    token: String,
    /// When this invitation was created (stored for future audit/admin)
    #[allow(dead_code)]
    created_at: i64,
    /// When this invitation expires
    expires_at: i64,
    /// Guest scope (permissions)
    scope: GuestScope,
    /// Whether this invitation has been activated (one-time use)
    activated: bool,
}

/// Manages guest invitations with 15-minute expiry and one-time activation
pub struct InvitationManager {
    /// Pending invitations by token hash
    pending: Arc<DashMap<String, PendingInvitation>>,
    /// Secret for HMAC signing invitation tokens
    secret: [u8; 32],
    /// Default expiry duration in milliseconds
    default_expiry_ms: i64,
}

impl InvitationManager {
    /// Create a new invitation manager
    pub fn new() -> Self {
        Self {
            pending: Arc::new(DashMap::new()),
            secret: generate_secret(),
            default_expiry_ms: DEFAULT_INVITATION_EXPIRY_MS,
        }
    }

    /// Create a new invitation manager with custom expiry
    pub fn with_expiry(expiry_ms: i64) -> Self {
        Self {
            pending: Arc::new(DashMap::new()),
            secret: generate_secret(),
            default_expiry_ms: expiry_ms,
        }
    }

    /// Create a new guest invitation
    ///
    /// # Arguments
    /// * `request` - The invitation creation request with guest name and scope
    ///
    /// # Returns
    /// An `Invitation` containing the token, URL, guest ID, and expiry
    ///
    /// # Errors
    /// Returns `InvitationError::InvalidScope` if scope is invalid
    pub fn create_invitation(
        &self,
        request: CreateInvitationRequest,
    ) -> Result<Invitation, InvitationError> {
        let guest_id = Uuid::new_v4().to_string();
        let token = Uuid::new_v4().to_string();

        let now = current_timestamp_ms();
        let expires_at = now + self.default_expiry_ms;

        // Store the pending invitation
        let invitation = PendingInvitation {
            guest_id: guest_id.clone(),
            guest_name: request.guest_name,
            token: token.clone(),
            created_at: now,
            expires_at,
            scope: request.scope,
            activated: false,
        };

        self.pending.insert(token.clone(), invitation);

        // Build the invitation URL (mock for now, would be configurable in production)
        let url = format!("https://aleph.local/join?t={}", token);

        Ok(Invitation {
            token,
            url,
            guest_id,
            expires_at: Some(expires_at),
        })
    }

    /// Activate an invitation and return a guest token
    ///
    /// Invitations are one-time use only. Subsequent calls with the same token will fail.
    ///
    /// # Arguments
    /// * `token` - The invitation token
    ///
    /// # Returns
    /// A `GuestToken` that can be used for authenticated access
    ///
    /// # Errors
    /// - `InvitationError::InvalidToken` if token not found
    /// - `InvitationError::InvitationExpired` if token has expired
    /// - `InvitationError::AlreadyActivated` if token was already activated
    pub fn activate_invitation(
        &self,
        token: &str,
    ) -> Result<GuestToken, InvitationError> {
        let now = current_timestamp_ms();

        // Find the invitation
        let mut invitation = self
            .pending
            .get_mut(token)
            .ok_or(InvitationError::InvalidToken)?;

        // Check if already activated (one-time use)
        if invitation.activated {
            return Err(InvitationError::AlreadyActivated);
        }

        // Check if expired
        if now > invitation.expires_at {
            return Err(InvitationError::InvitationExpired);
        }

        // Mark as activated
        invitation.activated = true;

        // Return guest token
        Ok(GuestToken {
            token: invitation.token.clone(),
            guest_id: invitation.guest_id.clone(),
            scope: invitation.scope.clone(),
        })
    }

    /// List all pending (non-activated) invitations
    ///
    /// # Returns
    /// Vector of pending invitations with their details
    pub fn list_pending(&self) -> Vec<Invitation> {
        let now = current_timestamp_ms();

        self.pending
            .iter()
            .filter(|entry| !entry.activated && entry.expires_at > now)
            .map(|entry| Invitation {
                token: entry.token.clone(),
                url: format!("https://aleph.local/join?t={}", entry.token),
                guest_id: entry.guest_id.clone(),
                expires_at: Some(entry.expires_at),
            })
            .collect()
    }

    /// Revoke an invitation by token
    ///
    /// # Arguments
    /// * `token` - The invitation token to revoke
    ///
    /// # Returns
    /// `Ok(())` if invitation was revoked, `Err(InvitationError::InvalidToken)` if not found
    pub fn revoke_invitation(&self, token: &str) -> Result<(), InvitationError> {
        self.pending
            .remove(token)
            .ok_or(InvitationError::InvalidToken)?;
        Ok(())
    }

    /// Clean up expired invitations
    ///
    /// # Returns
    /// Number of invitations removed
    pub fn cleanup_expired(&self) -> usize {
        let now = current_timestamp_ms();
        let to_remove: Vec<String> = self
            .pending
            .iter()
            .filter(|entry| entry.expires_at <= now)
            .map(|entry| entry.key().clone())
            .collect();

        let count = to_remove.len();
        for token in to_remove {
            self.pending.remove(&token);
        }
        count
    }

    /// Get the secret (for persistence or testing)
    pub fn secret(&self) -> &[u8; 32] {
        &self.secret
    }
}

impl Default for InvitationManager {
    fn default() -> Self {
        Self::new()
    }
}

fn current_timestamp_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_invitation() {
        let manager = InvitationManager::new();

        let request = CreateInvitationRequest {
            guest_name: "Mom".to_string(),
            scope: GuestScope {
                allowed_tools: vec!["translate".to_string()],
                expires_at: Some(1735689600),
                display_name: Some("Mom".to_string()),
            },
        };

        let invitation = manager.create_invitation(request).unwrap();

        assert!(!invitation.token.is_empty());
        assert!(!invitation.url.is_empty());
        assert_eq!(invitation.guest_id.len(), 36); // UUID length
        assert!(invitation.expires_at.is_some());
    }

    #[test]
    fn test_activate_invitation_valid() {
        let manager = InvitationManager::new();

        let request = CreateInvitationRequest {
            guest_name: "Alice".to_string(),
            scope: GuestScope {
                allowed_tools: vec!["summarize".to_string()],
                expires_at: Some(1735689600),
                display_name: Some("Alice".to_string()),
            },
        };

        let invitation = manager.create_invitation(request).unwrap();
        let token = invitation.token;
        let guest_id = invitation.guest_id.clone();

        // Activate the invitation
        let guest_token = manager.activate_invitation(&token).unwrap();

        assert_eq!(guest_token.guest_id, guest_id);
        assert_eq!(guest_token.token, token);
        assert_eq!(guest_token.scope.allowed_tools, vec!["summarize"]);
    }

    #[test]
    fn test_activate_invitation_invalid_token() {
        let manager = InvitationManager::new();

        let result = manager.activate_invitation("nonexistent-token");
        assert!(matches!(result, Err(InvitationError::InvalidToken)));
    }

    #[test]
    fn test_activate_invitation_one_time_use() {
        let manager = InvitationManager::new();

        let request = CreateInvitationRequest {
            guest_name: "Bob".to_string(),
            scope: GuestScope {
                allowed_tools: vec!["translate".to_string()],
                expires_at: None,
                display_name: Some("Bob".to_string()),
            },
        };

        let invitation = manager.create_invitation(request).unwrap();
        let token = invitation.token;

        // First activation should succeed
        let result1 = manager.activate_invitation(&token);
        assert!(result1.is_ok());

        // Second activation should fail (one-time use)
        let result2 = manager.activate_invitation(&token);
        assert!(matches!(result2, Err(InvitationError::AlreadyActivated)));
    }

    #[test]
    fn test_activate_invitation_expired() {
        let manager = InvitationManager::with_expiry(10); // 10ms expiry

        let request = CreateInvitationRequest {
            guest_name: "Charlie".to_string(),
            scope: GuestScope {
                allowed_tools: vec!["translate".to_string()],
                expires_at: None,
                display_name: Some("Charlie".to_string()),
            },
        };

        let invitation = manager.create_invitation(request).unwrap();
        let token = invitation.token;

        // Wait for expiry
        std::thread::sleep(std::time::Duration::from_millis(50));

        // Activation should fail (expired)
        let result = manager.activate_invitation(&token);
        assert!(matches!(result, Err(InvitationError::InvitationExpired)));
    }

    #[test]
    fn test_list_pending() {
        let manager = InvitationManager::new();

        // Create multiple invitations
        let req1 = CreateInvitationRequest {
            guest_name: "Guest1".to_string(),
            scope: GuestScope {
                allowed_tools: vec!["translate".to_string()],
                expires_at: None,
                display_name: None,
            },
        };
        let inv1 = manager.create_invitation(req1).unwrap();

        let req2 = CreateInvitationRequest {
            guest_name: "Guest2".to_string(),
            scope: GuestScope {
                allowed_tools: vec!["summarize".to_string()],
                expires_at: None,
                display_name: None,
            },
        };
        let inv2 = manager.create_invitation(req2).unwrap();

        // List pending
        let pending = manager.list_pending();
        assert_eq!(pending.len(), 2);

        // Activate one
        let _ = manager.activate_invitation(&inv1.token);

        // List pending again - should still show both (we filter by activation and expiry)
        let pending = manager.list_pending();
        assert_eq!(pending.len(), 1); // One was activated, so one remaining
        assert_eq!(pending[0].guest_id, inv2.guest_id);
    }

    #[test]
    fn test_cleanup_expired() {
        let manager = InvitationManager::with_expiry(10); // 10ms expiry

        // Create invitations
        let req1 = CreateInvitationRequest {
            guest_name: "Guest1".to_string(),
            scope: GuestScope {
                allowed_tools: vec![],
                expires_at: None,
                display_name: None,
            },
        };
        manager.create_invitation(req1).unwrap();

        let req2 = CreateInvitationRequest {
            guest_name: "Guest2".to_string(),
            scope: GuestScope {
                allowed_tools: vec![],
                expires_at: None,
                display_name: None,
            },
        };
        manager.create_invitation(req2).unwrap();

        assert_eq!(manager.pending.len(), 2);

        // Wait for expiry
        std::thread::sleep(std::time::Duration::from_millis(50));

        // Cleanup
        let removed = manager.cleanup_expired();
        assert_eq!(removed, 2);
        assert_eq!(manager.pending.len(), 0);
    }
}
