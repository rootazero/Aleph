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
}

// Note: group mention gating lives in a single source — the inbound router's
// permission layer, driven by `[channels.imessage].require_mention` +
// `bot_name` (see `inbound_router::permission::check_permission`). The old
// transport-level `require_mention` / `mention_patterns` (regex) knobs were a
// second, conflicting gate that only ran on the webhook path (never on
// catch-up poll) and matched wake words by regex — an R7/P8 violation. They
// were removed; existing configs carrying them still parse (unknown fields are
// ignored) and are simply inert.
