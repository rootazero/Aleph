//! OAuth RPC Handlers
//!
//! Handles browser-based OAuth login/logout/status for providers that
//! require OAuth authentication (currently ChatGPT/Codex).
//!
//! Token persistence follows the same pattern as other providers:
//! `api_key` is stored directly in `aleph.toml` (plaintext), consistent
//! with how the Settings UI manages provider credentials.

use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use crate::config::{Config, ProviderConfig};
use crate::providers::chatgpt::auth::ChatGptAuth;
use crate::providers::presets::get_preset;
use crate::sync_primitives::Arc;

use super::parse_params;
use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};

// ─── Types ───────────────────────────────────────────────────────────────────

/// In-memory OAuth token cache with expiry metadata.
///
/// The `access_token` is also persisted as `config.providers["chatgpt"].api_key`
/// in `aleph.toml`, just like any other provider. This struct adds expiry
/// tracking and refresh token support on top.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthTokenCache {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at_unix: u64,
    pub session_id: String,
}

impl OAuthTokenCache {
    /// Build from a completed ChatGptAuth.
    pub fn from_auth(auth: &ChatGptAuth) -> Self {
        let expires_at_unix = auth
            .expires_at
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs();
        Self {
            access_token: auth.access_token.clone(),
            refresh_token: auth.refresh_token.clone(),
            expires_at_unix,
            session_id: auth.session_id.clone(),
        }
    }

    /// Reconstruct a ChatGptAuth from this cache.
    pub fn to_auth(&self) -> ChatGptAuth {
        ChatGptAuth {
            access_token: self.access_token.clone(),
            refresh_token: self.refresh_token.clone(),
            expires_at: UNIX_EPOCH + Duration::from_secs(self.expires_at_unix),
            session_id: self.session_id.clone(),
        }
    }

    /// Whether the cached token has expired.
    pub fn is_expired(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs();
        now >= self.expires_at_unix
    }

    /// Seconds remaining until expiry, or None if already expired.
    pub fn expires_in_seconds(&self) -> Option<u64> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs();
        self.expires_at_unix.checked_sub(now)
    }
}

/// In-memory shared OAuth state (expiry + refresh token metadata).
pub type SharedOAuthState = Arc<RwLock<Option<OAuthTokenCache>>>;

// ─── RPC Param types ─────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct OAuthParams {
    provider: String,
}

/// Supported OAuth provider aliases.
fn is_supported_oauth_provider(name: &str) -> bool {
    matches!(name.to_lowercase().as_str(), "codex" | "chatgpt")
}

/// Canonical provider name used in config.providers.
fn canonical_provider_name(name: &str) -> &'static str {
    match name.to_lowercase().as_str() {
        "codex" | "chatgpt" => "chatgpt",
        _ => "chatgpt",
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Build a ProviderConfig from preset defaults (used when the provider
/// entry doesn't exist yet in config).
fn new_provider_from_preset(provider_name: &str) -> ProviderConfig {
    let preset = get_preset(provider_name);
    ProviderConfig {
        protocol: preset.map(|p| p.protocol.to_string()),
        api_key: None,
        secret_name: None,
        model: preset
            .map(|p| p.default_model.to_string())
            .unwrap_or_else(|| "gpt-5.3-codex".to_string()),
        base_url: preset.map(|p| p.base_url.to_string()),
        color: preset
            .map(|p| p.color.to_string())
            .unwrap_or_else(|| "#808080".to_string()),
        timeout_seconds: 300,
        enabled: true,
        max_tokens: None,
        temperature: None,
        top_p: None,
        top_k: None,
        frequency_penalty: None,
        presence_penalty: None,
        stop_sequences: None,
        thinking_level: None,
        media_resolution: None,
        repeat_penalty: None,
        system_prompt_mode: None,
        verified: false,
    }
}

/// Update config.providers["chatgpt"].api_key and save to aleph.toml.
/// This is the same persistence path as setting an API key via the Settings UI.
async fn update_config_api_key(
    config: &Arc<RwLock<Config>>,
    provider_name: &str,
    token: Option<&str>,
) {
    let mut cfg = config.write().await;

    if let Some(token) = token {
        let provider = cfg
            .providers
            .entry(provider_name.to_string())
            .or_insert_with(|| new_provider_from_preset(provider_name));
        provider.api_key = Some(token.to_string());
        provider.enabled = true;
        provider.verified = true;
    } else {
        if let Some(provider) = cfg.providers.get_mut(provider_name) {
            provider.api_key = None;
        }
    }

    if let Err(e) = cfg.save() {
        warn!(error = %e, "Failed to persist config after OAuth update");
    }
}

/// Attempt to refresh an expired token. Returns true on success.
async fn try_refresh(
    oauth_state: &SharedOAuthState,
    config: &Arc<RwLock<Config>>,
    provider_name: &str,
) -> bool {
    let cache = {
        let guard = oauth_state.read().await;
        match guard.as_ref() {
            Some(c) => c.clone(),
            None => return false,
        }
    };

    let mut auth = cache.to_auth();
    match auth.refresh().await {
        Ok(()) => {
            let new_cache = OAuthTokenCache::from_auth(&auth);
            *oauth_state.write().await = Some(new_cache.clone());
            update_config_api_key(config, provider_name, Some(&new_cache.access_token)).await;
            info!("OAuth token refreshed successfully");
            true
        }
        Err(e) => {
            warn!(error = %e, "OAuth token refresh failed");
            false
        }
    }
}

/// Restore OAuth state from config at startup.
///
/// If `config.providers["chatgpt"].api_key` is set, we know the user
/// previously logged in via OAuth (or manually). We build an
/// OAuthTokenCache with a conservative 1-hour expiry window so that
/// `oauthStatus` will trigger a refresh if the token is actually stale.
pub fn restore_from_config(config: &Config) -> Option<OAuthTokenCache> {
    let provider = config.providers.get("chatgpt")?;
    let api_key = provider.api_key.as_ref().filter(|k| !k.is_empty())?;

    // We don't know the real expiry from config alone, so assume
    // it might be expired — oauthStatus will auto-refresh.
    let cache = OAuthTokenCache {
        access_token: api_key.clone(),
        refresh_token: None,
        // Set a 1-hour window from now; if the token is actually expired,
        // the first oauthStatus call will detect and refresh it.
        expires_at_unix: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs()
            + 3600,
        session_id: String::new(),
    };

    debug!("Restored OAuth state from config (chatgpt provider)");
    Some(cache)
}

// ─── RPC Handlers ────────────────────────────────────────────────────────────

/// `providers.oauthLogin` — Start browser OAuth flow, store token.
pub async fn handle_oauth_login(
    request: JsonRpcRequest,
    oauth_state: Arc<RwLock<Option<OAuthTokenCache>>>,
    config: Arc<RwLock<Config>>,
) -> JsonRpcResponse {
    let params: OAuthParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    if !is_supported_oauth_provider(&params.provider) {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!(
                "Provider '{}' does not support OAuth login. Supported: codex, chatgpt",
                params.provider
            ),
        );
    }

    let provider_name = canonical_provider_name(&params.provider);

    info!(provider = provider_name, "Starting OAuth browser login");

    let auth = match ChatGptAuth::authorize_via_browser().await {
        Ok(auth) => auth,
        Err(e) => {
            error!(error = %e, "OAuth browser login failed");
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("OAuth login failed: {}", e),
            );
        }
    };

    let cache = OAuthTokenCache::from_auth(&auth);
    let expires_in = cache.expires_in_seconds();

    // Store in memory (with expiry + refresh token metadata)
    *oauth_state.write().await = Some(cache.clone());

    // Persist access_token to aleph.toml (same as setting API key via Settings UI)
    update_config_api_key(&config, provider_name, Some(&cache.access_token)).await;

    info!(provider = provider_name, "OAuth login successful");

    let mut result = json!({
        "connected": true,
        "provider": provider_name,
    });
    if let Some(secs) = expires_in {
        result["expires_in_seconds"] = json!(secs);
    }

    JsonRpcResponse::success(request.id, result)
}

/// `providers.oauthLogout` — Clear stored OAuth token.
pub async fn handle_oauth_logout(
    request: JsonRpcRequest,
    oauth_state: Arc<RwLock<Option<OAuthTokenCache>>>,
    config: Arc<RwLock<Config>>,
) -> JsonRpcResponse {
    let params: OAuthParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    if !is_supported_oauth_provider(&params.provider) {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!(
                "Provider '{}' does not support OAuth. Supported: codex, chatgpt",
                params.provider
            ),
        );
    }

    let provider_name = canonical_provider_name(&params.provider);

    // Clear memory
    *oauth_state.write().await = None;

    // Clear config api_key + save
    update_config_api_key(&config, provider_name, None).await;

    info!(provider = provider_name, "OAuth logout completed");

    JsonRpcResponse::success(request.id, json!({ "ok": true }))
}

/// `providers.oauthStatus` — Check OAuth connection status, auto-refresh if expired.
pub async fn handle_oauth_status(
    request: JsonRpcRequest,
    oauth_state: Arc<RwLock<Option<OAuthTokenCache>>>,
    config: Arc<RwLock<Config>>,
) -> JsonRpcResponse {
    let params: OAuthParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    if !is_supported_oauth_provider(&params.provider) {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!(
                "Provider '{}' does not support OAuth. Supported: codex, chatgpt",
                params.provider
            ),
        );
    }

    let provider_name = canonical_provider_name(&params.provider);

    let has_token = oauth_state.read().await.is_some();
    if !has_token {
        return JsonRpcResponse::success(
            request.id,
            json!({
                "connected": false,
                "provider": provider_name,
            }),
        );
    }

    // Check expiry and try refresh if needed
    let is_expired = oauth_state
        .read()
        .await
        .as_ref()
        .map(|c| c.is_expired())
        .unwrap_or(true);

    if is_expired {
        debug!("OAuth token expired, attempting refresh");
        let refreshed = try_refresh(&oauth_state, &config, provider_name).await;
        if !refreshed {
            *oauth_state.write().await = None;
            return JsonRpcResponse::success(
                request.id,
                json!({
                    "connected": false,
                    "provider": provider_name,
                    "error": "Token expired and refresh failed. Please re-login.",
                }),
            );
        }
    }

    let expires_in = oauth_state
        .read()
        .await
        .as_ref()
        .and_then(|c| c.expires_in_seconds());

    let mut result = json!({
        "connected": true,
        "provider": provider_name,
    });
    if let Some(secs) = expires_in {
        result["expires_in_seconds"] = json!(secs);
    }

    JsonRpcResponse::success(request.id, result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_oauth_token_cache_roundtrip() {
        let auth = ChatGptAuth {
            access_token: "test_token".to_string(),
            refresh_token: Some("refresh_tok".to_string()),
            expires_at: SystemTime::now() + Duration::from_secs(3600),
            session_id: "session_123".to_string(),
        };

        let cache = OAuthTokenCache::from_auth(&auth);
        assert!(!cache.is_expired());
        assert!(cache.expires_in_seconds().unwrap() > 3500);

        let roundtripped = cache.to_auth();
        assert_eq!(roundtripped.access_token, "test_token");
        assert_eq!(roundtripped.refresh_token, Some("refresh_tok".to_string()));
        assert_eq!(roundtripped.session_id, "session_123");
        assert!(!roundtripped.is_expired());
    }

    #[test]
    fn test_oauth_token_cache_expired() {
        let cache = OAuthTokenCache {
            access_token: "old".to_string(),
            refresh_token: None,
            expires_at_unix: 0,
            session_id: "s".to_string(),
        };
        assert!(cache.is_expired());
        assert_eq!(cache.expires_in_seconds(), None);
    }

    #[test]
    fn test_supported_providers() {
        assert!(is_supported_oauth_provider("codex"));
        assert!(is_supported_oauth_provider("chatgpt"));
        assert!(is_supported_oauth_provider("Codex"));
        assert!(is_supported_oauth_provider("ChatGPT"));
        assert!(!is_supported_oauth_provider("openai"));
        assert!(!is_supported_oauth_provider("claude"));
    }

    #[test]
    fn test_canonical_name() {
        assert_eq!(canonical_provider_name("codex"), "chatgpt");
        assert_eq!(canonical_provider_name("chatgpt"), "chatgpt");
        assert_eq!(canonical_provider_name("Codex"), "chatgpt");
    }

    #[test]
    fn test_serialization() {
        let cache = OAuthTokenCache {
            access_token: "tok".to_string(),
            refresh_token: Some("ref".to_string()),
            expires_at_unix: 1700000000,
            session_id: "sid".to_string(),
        };
        let json = serde_json::to_string(&cache).unwrap();
        let deserialized: OAuthTokenCache = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.access_token, "tok");
        assert_eq!(deserialized.expires_at_unix, 1700000000);
    }

    #[test]
    fn test_restore_from_config_present() {
        let mut config = Config::new();
        let mut provider = new_provider_from_preset("chatgpt");
        provider.api_key = Some("my-oauth-token".to_string());
        config.providers.insert("chatgpt".to_string(), provider);

        let cache = restore_from_config(&config).unwrap();
        assert_eq!(cache.access_token, "my-oauth-token");
        assert!(!cache.is_expired()); // 1-hour window
    }

    #[test]
    fn test_restore_from_config_absent() {
        let config = Config::new();
        assert!(restore_from_config(&config).is_none());
    }

    #[test]
    fn test_restore_from_config_empty_key() {
        let mut config = Config::new();
        let mut provider = new_provider_from_preset("chatgpt");
        provider.api_key = Some(String::new());
        config.providers.insert("chatgpt".to_string(), provider);

        assert!(restore_from_config(&config).is_none());
    }
}
