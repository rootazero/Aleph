//! Guest invitation types

use crate::auth::GuestScope;
use serde::{Deserialize, Serialize};

/// Request to create guest invitation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateInvitationRequest {
    /// Guest display name
    pub guest_name: String,
    /// Permission scope
    pub scope: GuestScope,
}

/// Created invitation result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Invitation {
    /// Encrypted invitation token
    pub token: String,
    /// Invitation URL
    pub url: String,
    /// Guest ID
    pub guest_id: String,
    /// Expiry timestamp
    pub expires_at: Option<i64>,
}

/// Request to activate invitation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivateInvitationRequest {
    /// Invitation token
    pub token: String,
}

/// Activated guest token
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuestToken {
    /// JWT-style token
    pub token: String,
    /// Guest ID
    pub guest_id: String,
    /// Scope
    pub scope: GuestScope,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_invitation_request_serde() {
        let req = CreateInvitationRequest {
            guest_name: "Mom".to_string(),
            scope: GuestScope {
                allowed_tools: vec!["translate".to_string()],
                expires_at: Some(1735689600),
                display_name: Some("Mom".to_string()),
            },
        };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: CreateInvitationRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.guest_name, "Mom");
    }

    #[test]
    fn test_invitation_serde() {
        let inv = Invitation {
            token: "encrypted_token".to_string(),
            url: "https://aleph.local/join?t=xxx".to_string(),
            guest_id: "guest1".to_string(),
            expires_at: Some(1735689600),
        };
        let json = serde_json::to_string(&inv).unwrap();
        let parsed: Invitation = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.guest_id, "guest1");
    }
}
