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
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
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

impl BlueBubblesConfig {
    pub fn validate(&self) -> Result<(), String> {
        let server =
            url::Url::parse(&self.server_url).map_err(|_| "Invalid BlueBubbles server URL")?;
        let local = |host: &str| {
            let host = host.trim_matches(['[', ']']);
            host.eq_ignore_ascii_case("localhost")
                || host
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback() || ip.is_unspecified())
        };
        if !local(
            server
                .host_str()
                .ok_or("BlueBubbles server URL needs a host")?,
        ) && local(&self.webhook_host)
        {
            return Err("Remote BlueBubbles server requires a reachable webhook host, not loopback or unspecified".into());
        }
        if !self.webhook_path.starts_with('/') {
            return Err("BlueBubbles webhook path must start with '/'".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn remote_callback_must_be_reachable_local_callback_is_allowed() {
        let mut config: BlueBubblesConfig = serde_json::from_value(serde_json::json!({
            "server_url": "http://remote.example:1234", "password": "p"
        }))
        .unwrap();
        assert!(config.validate().is_err());
        config.webhook_host = "192.168.1.20".into();
        assert!(config.validate().is_ok());
        config.webhook_host = "::1".into();
        assert!(config.validate().is_err());
        config.server_url = "http://localhost:1234".into();
        assert!(config.validate().is_ok());
    }
}

/// Hand-written `Debug` so the server password never reaches a log line:
/// the derived `Debug` printed the secret verbatim.
impl std::fmt::Debug for BlueBubblesConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlueBubblesConfig")
            .field("server_url", &self.server_url)
            .field("password", &"***")
            .field("webhook_host", &self.webhook_host)
            .field("webhook_port", &self.webhook_port)
            .field("webhook_path", &self.webhook_path)
            .field("poll_interval_secs", &self.poll_interval_secs)
            .field("send_read_receipts", &self.send_read_receipts)
            .finish()
    }
}

// Note: group mention gating lives in a single source — the inbound router's
// permission layer, driven by `[channels.imessage].require_mention` +
// `bot_name` (see `inbound_router::permission::check_permission`). The old
// transport-level `require_mention` / `mention_patterns` (regex) knobs were a
// second, conflicting gate that only ran on the webhook path (never on
// catch-up poll) and matched wake words by regex — an R7/P8 violation. They
// were removed; existing configs carrying them still parse (unknown fields are
// ignored) and are simply inert.
