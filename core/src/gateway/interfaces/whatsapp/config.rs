//! WhatsApp Channel Configuration
//!
//! Configuration for the WhatsApp channel adapter.

use serde::{Deserialize, Serialize};

/// WhatsApp channel configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WhatsAppConfig {
    /// Phone number (optional, used for login if available)
    pub phone_number: Option<String>,
    /// Session data (base64 encoded JSON)
    pub session_data: Option<String>,
    /// Whether to send typing indicators
    #[serde(default = "default_true")]
    pub send_typing: bool,
    /// Whether to mark messages as read
    #[serde(default = "default_true")]
    pub mark_read: bool,
    /// List of allowed users/groups (empty = all allowed)
    #[serde(default)]
    pub allowed_chats: Vec<String>,
    /// Path to the whatsapp-bridge binary (auto-detected if not set)
    #[serde(default)]
    pub bridge_binary: Option<String>,
    /// Max restart attempts for the bridge process
    #[serde(default = "default_max_restarts")]
    pub max_restarts: u32,
}

fn default_true() -> bool {
    true
}

fn default_max_restarts() -> u32 {
    5
}

impl WhatsAppConfig {
    /// Validate the configuration
    pub fn validate(&self) -> Result<(), String> {
        // No strict requirements for now, but we can add validation for phone numbers if needed
        Ok(())
    }

    /// Check if a chat ID is allowed
    pub fn is_chat_allowed(&self, chat_id: &str) -> bool {
        if self.allowed_chats.is_empty() {
            return true;
        }
        self.allowed_chats.iter().any(|c| c == chat_id)
    }
}
