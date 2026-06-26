//! BlueBubbles transport configuration.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

fn default_webhook_host() -> String {
    "0.0.0.0".to_string()
}
const fn default_webhook_port() -> u16 {
    8645
}
fn default_webhook_path() -> String {
    "/bluebubbles-webhook".to_string()
}
const fn default_poll_interval() -> u64 {
    30
}
const fn default_true() -> bool {
    true
}

/// Configuration for the BlueBubbles iMessage transport.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BlueBubblesConfig {
    /// Base URL of the BlueBubbles server, e.g. `http://192.168.1.50:1234`.
    pub server_url: String,
    /// BlueBubbles server password (resolved from the `{{secret:}}` vault).
    pub password: String,
    /// Host the inbound webhook listener binds to.
    #[serde(default = "default_webhook_host")]
    pub webhook_host: String,
    /// Port the inbound webhook listener binds to.
    #[serde(default = "default_webhook_port")]
    pub webhook_port: u16,
    /// Path the inbound webhook listener serves.
    #[serde(default = "default_webhook_path")]
    pub webhook_path: String,
    /// Catch-up reconciliation poll interval (seconds). Real-time is webhook-driven.
    #[serde(default = "default_poll_interval")]
    pub poll_interval_secs: u64,
    /// Whether to send read receipts for inbound messages.
    #[serde(default = "default_true")]
    pub send_read_receipts: bool,
    /// Require a mention/wake word in group chats before dispatching.
    #[serde(default)]
    pub require_mention: bool,
    /// Group wake-word regex patterns (empty = no gating).
    #[serde(default)]
    pub mention_patterns: Vec<String>,
}
