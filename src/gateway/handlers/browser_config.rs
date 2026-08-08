//! Browser configuration RPC handlers
//!
//! Provides RPC methods for managing browser system settings from the Panel UI.

use crate::browser::profile::{BrowserDriver, ProfileConfig};
use crate::config::Config;
use crate::gateway::event_bus::{ConfigChangedEvent, GatewayEvent, GatewayEventBus};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::sync_primitives::Arc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::RwLock;

/// Browser config for the Panel UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct BrowserConfigResponse {
    /// Default profile driver: "managed" (headless) or "`existing_session`" (Chrome `DevTools`)
    pub default_driver: String,
    /// Whether Playwright CLI runs in headless mode
    pub headless: bool,
    /// `DevTools` profile: "user" (user's Chrome) or "managed" (Aleph-managed instance)
    pub devtools_profile: String,
    /// SSRF protection: block private network access
    pub block_private: bool,
    /// SSRF: blocked domain patterns
    pub blocked_domains: Vec<String>,
    /// SSRF: allowed domain patterns (whitelist mode)
    pub allowed_domains: Vec<String>,
    /// Timeout (seconds) for navigate / `wait_for_text`
    pub nav_timeout_secs: u64,
    /// Timeout (seconds) for other actions (click/fill/type/etc)
    pub action_timeout_secs: u64,
}

// =============================================================================
// RPC Handlers
// =============================================================================

/// Get browser configuration
pub async fn handle_get(request: JsonRpcRequest, config: Arc<RwLock<Config>>) -> JsonRpcResponse {
    let cfg = config.read().await;
    let browser = &cfg.general.browser;

    // Find the "default" profile's driver, or fallback to "managed"
    let default_driver = browser
        .profiles
        .get("default")
        .map_or("managed", |p| match p.driver {
            BrowserDriver::Managed => "managed",
            BrowserDriver::ExistingSession => "existing_session",
        })
        .to_string();

    // Headless detection: read from playwright_cli.headless
    let headless = browser.playwright_cli.headless;

    // DevTools profile: "user" (Your Chrome) unless explicitly set to Managed
    let devtools_profile = if browser
        .profiles
        .get("user")
        .is_some_and(|p| p.driver == BrowserDriver::Managed)
    {
        "managed"
    } else {
        "user"
    }
    .to_string();

    let response = BrowserConfigResponse {
        default_driver,
        headless,
        devtools_profile,
        block_private: browser.policy.block_private,
        blocked_domains: browser.policy.blocked_domains.clone(),
        allowed_domains: browser.policy.allowed_domains.clone(),
        nav_timeout_secs: browser.playwright_cli.nav_timeout_secs,
        action_timeout_secs: browser.playwright_cli.action_timeout_secs,
    };

    match serde_json::to_value(&response) {
        Ok(value) => JsonRpcResponse::success(request.id, value),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to serialize config: {e}"),
        ),
    }
}

/// Update browser configuration
pub async fn handle_update(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    let params = match request.params {
        Some(p) => p,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params");
        }
    };

    let update: BrowserConfigResponse = match serde_json::from_value(params) {
        Ok(u) => u,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Invalid params: {e}"),
            );
        }
    };

    {
        let mut cfg = config.write().await;
        let browser = &mut cfg.general.browser;

        // Update default profile driver
        let driver = if update.default_driver == "existing_session" {
            BrowserDriver::ExistingSession
        } else {
            BrowserDriver::Managed
        };

        // Update or create the "default" profile with the chosen driver
        if let Some(profile) = browser.profiles.get_mut("default") {
            profile.driver = driver;
        } else {
            browser.profiles.insert(
                "default".to_string(),
                ProfileConfig {
                    driver,
                    ..Default::default()
                },
            );
        }

        // Update "user" profile based on devtools_profile setting
        let user_driver = if update.devtools_profile == "user" {
            BrowserDriver::ExistingSession
        } else {
            BrowserDriver::Managed
        };
        if let Some(profile) = browser.profiles.get_mut("user") {
            profile.driver = user_driver;
        } else {
            browser.profiles.insert(
                "user".to_string(),
                ProfileConfig {
                    driver: user_driver,
                    browser: crate::browser::profile::BrowserType::Chrome,
                    ..Default::default()
                },
            );
        }

        // Update playwright_cli fields
        browser.playwright_cli.headless = update.headless;
        browser.playwright_cli.nav_timeout_secs = update.nav_timeout_secs;
        browser.playwright_cli.action_timeout_secs = update.action_timeout_secs;

        // Update SSRF policy
        browser.policy.block_private = update.block_private;
        browser.policy.blocked_domains = update.blocked_domains.clone();
        browser.policy.allowed_domains = update.allowed_domains.clone();

        // Save to disk — use precise path to avoid overwriting other general settings
        if let Err(e) = cfg.save_incremental(&["general.browser"]) {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {e}"),
            );
        }
    }

    // Broadcast change event
    let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
        section: Some("browser".to_string()),
        value: serde_json::to_value(&update).unwrap_or(Value::Null),
        timestamp: chrono::Utc::now().timestamp_millis(),
    });
    let _ = event_bus.publish_gateway_event(&event);

    JsonRpcResponse::success(request.id, serde_json::json!({ "success": true }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[tokio::test]
    async fn test_browser_config_exposes_timeouts() {
        let cfg = Arc::new(RwLock::new(Config::default()));
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "browser.get".into(),
            params: None,
            id: Some(serde_json::json!(1)),
        };
        let resp = handle_get(req, cfg).await;
        let v = resp.result.unwrap();
        assert_eq!(v["nav_timeout_secs"], 30);
        assert_eq!(v["action_timeout_secs"], 10);
    }
}
