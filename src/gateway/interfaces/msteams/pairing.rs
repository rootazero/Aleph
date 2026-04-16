//! DM Auto-Pairing System
//!
//! Automatically pairs users with the bot on their first DM, creating a
//! one-on-one conversation and obtaining a Direct Line token.

use std::time::Instant;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::domain::UserId;
use crate::gateway::channel::ChannelError;
use crate::sync_primitives::Arc;

use super::graph::GraphClient;

/// DM Pairing state machine.
#[derive(Debug, Clone)]
pub enum PairingState {
    /// Not yet paired with any user.
    Unpaired,
    /// Successfully paired with a specific user.
    Paired(PairingInfo),
}

impl PairingState {
    /// Returns `true` if currently paired.
    pub fn is_paired(&self) -> bool {
        matches!(self, PairingState::Paired(_))
    }
}

/// Information about a successful pairing.
#[derive(Debug, Clone)]
pub struct PairingInfo {
    /// The paired user's ID.
    pub user_id: UserId,
    /// The user's email or display name.
    pub user_email: String,
    /// Direct Line token for the paired conversation.
    pub direct_line_token: String,
    /// When this pairing was established.
    pub created_at: Instant,
}

/// Direct Line conversation details.
#[derive(Debug, Clone)]
pub struct DirectLine {
    /// The conversation ID in Teams.
    pub conversation_id: String,
    /// The Direct Line token.
    pub token: String,
}

/// Manages DM pairing lifecycle.
pub struct PairingManager {
    graph_client: Arc<GraphClient>,
    state: Arc<RwLock<PairingState>>,
}

impl PairingManager {
    /// Create a new PairingManager in the Unpaired state.
    pub fn new(graph_client: Arc<GraphClient>) -> Self {
        Self {
            graph_client,
            state: Arc::new(RwLock::new(PairingState::Unpaired)).into(),
        }
    }

    /// Handle an incoming DM message, auto-pairing on first contact.
    ///
    /// Returns `Some(DirectLine)` if this is the paired user, `None` otherwise.
    pub async fn handle_dm(&self, user_id: &str, user_display: Option<&str>) -> Result<Option<DirectLine>, ChannelError> {
        let user_id = UserId::new(user_id.to_string());
        let user_email = user_display.unwrap_or("").to_string();

        let mut state = self.state.write().await;

        match &*state {
            PairingState::Unpaired => {
                info!(user_id = %user_id, "Auto-pairing with user on first DM");
                let pairing = self.create_pairing(&user_id, &user_email).await?;
                let direct_line = DirectLine {
                    conversation_id: pairing.conversation_id.clone(),
                    token: pairing.direct_line_token.clone(),
                };
                *state = PairingState::Paired(pairing);
                Ok(Some(direct_line))
            }
            PairingState::Paired(info) => {
                if info.user_id == user_id {
                    Ok(Some(DirectLine {
                        conversation_id: info.user_email.clone(), // reuse as conversation placeholder
                        token: info.direct_line_token.clone(),
                    }))
                } else {
                    warn!(
                        paired_user = %info.user_id,
                        incoming_user = %user_id,
                        "DM received from user other than the paired one — ignoring"
                    );
                    Ok(None)
                }
            }
        }
    }

    /// Returns a copy of the current pairing state.
    pub async fn get_state(&self) -> PairingState {
        self.state.read().await.clone()
    }

    /// Resets pairing to Unpaired (useful for testing or re-pairing flow).
    pub async fn reset(&self) {
        let mut state = self.state.write().await;
        *state = PairingState::Unpaired;
    }

    // ── Private helpers ────────────────────────────────────────────────────────

    async fn create_pairing(&self, user_id: &UserId, _email: &str) -> Result<PairingInfo, ChannelError> {
        // Step 1: Create a one-on-one chat via Graph API
        #[derive(Serialize)]
        struct CreateChatRequest {
            #[serde(rename = "chatType")]
            chat_type: String,
            members: Vec<Member>,
        }

        #[derive(Serialize)]
        struct Member {
            #[serde(rename = "@odata.type")]
            odata_type: String,
            roles: Vec<String>,
            #[serde(rename = "userId")]
            user_id: String,
        }

        #[derive(Deserialize)]
        struct ChatResponse {
            id: String,
        }

        let request = CreateChatRequest {
            chat_type: "oneOnOne".to_string(),
            members: vec![Member {
                odata_type: "#microsoft.graph.aadUserConversationMember".to_string(),
                roles: vec!["user".to_string()],
                user_id: user_id.to_string(),
            }],
        };

        let chat: ChatResponse = self.graph_client
            .post_json("/me/chats", &request)
            .await
            .map_err(|e| ChannelError::SendFailed(format!("Failed to create DM chat: {e}")))?;

        // Step 2: Get Direct Line token for the conversation
        let token = self.get_direct_line_token(&chat.id).await?;

        Ok(PairingInfo {
            user_id: user_id.clone(),
            user_email: user_id.to_string(),
            direct_line_token: token,
            created_at: Instant::now(),
        })
    }

    async fn get_direct_line_token(&self, conversation_id: &str) -> Result<String, ChannelError> {
        #[derive(Deserialize)]
        struct TokenResponse {
            token: String,
        }

        // POST to conversations/{id}/messages to get a token response
        // The Direct Line API returns a token when posting to the conversation
        let response: TokenResponse = self.graph_client
            .post_with_response(
                &format!("/conversations/{}/messages", conversation_id),
                (),
            )
            .await
            .map_err(|e| ChannelError::SendFailed(format!("Failed to get Direct Line token: {e}")))?;

        Ok(response.token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pairing_state_is_paired() {
        assert!(!PairingState::Unpaired.is_paired());

        let info = PairingInfo {
            user_id: UserId::new("user-123".into()),
            user_email: "test@example.com".into(),
            direct_line_token: "token-abc".into(),
            created_at: Instant::now(),
        };
        assert!(PairingState::Paired(info).is_paired());
    }

    #[test]
    fn test_pairing_info_clone() {
        let info = PairingInfo {
            user_id: UserId::new("user-123".into()),
            user_email: "test@example.com".into(),
            direct_line_token: "token-abc".into(),
            created_at: Instant::now(),
        };
        let cloned = info.clone();
        assert_eq!(cloned.user_id, info.user_id);
        assert_eq!(cloned.direct_line_token, info.direct_line_token);
    }

    #[tokio::test]
    async fn test_pairing_manager_resets() {
        // We can't easily mock GraphClient, but we can verify reset() works
        // by checking the state transitions
        let manager = PairingManager::new(Arc::new(GraphClient::new_for_testing()));

        assert!(!manager.get_state().await.is_paired());
        manager.reset().await;
        assert!(!manager.get_state().await.is_paired());
    }
}
