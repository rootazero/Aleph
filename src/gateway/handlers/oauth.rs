//! OAuth RPC Handlers
//!
//! Handles browser-based OAuth login/logout/status for providers that
//! require OAuth authentication (currently Codex/ChatGPT subscription).
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
use crate::gateway::security::SharedTokenManager;
use crate::providers::codex::auth::CodexAuth;
use crate::providers::presets::get_preset;
use crate::sync_primitives::Arc;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use super::parse_params;

// ─── Types ───────────────────────────────────────────────────────────────────

/// In-memory OAuth token cache with expiry metadata.
///
/// The `access_token` is also persisted as `config.providers["chatgpt"].api_key` (legacy name)
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
    /// Build from a completed `CodexAuth`.
    #[must_use]
    pub fn from_auth(auth: &CodexAuth) -> Self {
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

    /// Reconstruct a `CodexAuth` from this cache.
    #[must_use]
    pub fn to_auth(&self) -> CodexAuth {
        CodexAuth {
            access_token: self.access_token.clone(),
            refresh_token: self.refresh_token.clone(),
            expires_at: UNIX_EPOCH + Duration::from_secs(self.expires_at_unix),
            session_id: self.session_id.clone(),
        }
    }

    /// Whether the cached token has expired.
    #[must_use]
    pub fn is_expired(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs();
        now >= self.expires_at_unix
    }

    /// Seconds remaining until expiry, or None if already expired.
    #[must_use]
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
///
/// Also read by `providers.catalog` to stamp each row's `auth_kind`, so a
/// client never has to keep its own list of which providers log in versus paste
/// a key. The Panel kept one, and it drifted.
pub(crate) fn is_supported_oauth_provider(name: &str) -> bool {
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

/// Build a `ProviderConfig` from preset defaults (used when the provider
/// entry doesn't exist yet in config).
fn new_provider_from_preset(provider_name: &str) -> ProviderConfig {
    let preset = get_preset(provider_name);
    ProviderConfig {
        protocol: preset.map(|p| p.protocol.to_string()),
        api_key: None,
        models: vec![preset.map_or_else(
            || "gpt-5.3-codex".to_string(),
            |p| p.default_model.to_string(),
        )],
        base_url: preset.map(|p| p.base_url.to_string()),
        color: preset.map_or_else(|| "#808080".to_string(), |p| p.color.to_string()),
        timeout_seconds: 300,
        enabled: true,
        max_tokens: None,
        context_window: None,
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
        model_behavior: None,
        verified: false,
        service_tier: None,
        stream_idle_timeout_secs: None,
        cache_retention: None,
        response_format: None,
        parallel_tool_calls: None,
        seed: None,
        logprobs: None,
        top_logprobs: None,
        metadata_user_id: None,
        effort: None,
    }
}

/// Vault key for the full OAuth token blob (access + refresh + expiry).
///
/// The legacy `ai:<provider>` key keeps holding *only* the access token so
/// providers (which read `api_key`) work unchanged. This blob key stores the
/// complete [`OAuthTokenCache`] so a daemon restart can recover the
/// `refresh_token` and real expiry — without it, `restore_from_vault` loses the
/// refresh token and every later refresh fails with "No refresh token
/// available. Please re-login."
fn oauth_blob_key(provider_name: &str) -> String {
    format!("ai:{provider_name}:oauth")
}

/// Persist the full OAuth cache (refresh token + real expiry) as a vault blob.
/// Best-effort: a failure only degrades restart recovery, not the live session.
fn persist_oauth_blob(
    vault: &Arc<SharedTokenManager>,
    provider_name: &str,
    cache: &OAuthTokenCache,
) {
    match serde_json::to_string(cache) {
        Ok(json) => {
            if let Err(e) = vault.store_secret(&oauth_blob_key(provider_name), &json) {
                warn!(error = %e, "Failed to store OAuth token blob in vault");
            }
        }
        Err(e) => warn!(error = %e, "Failed to serialize OAuth token blob"),
    }
}

/// Update config and store OAuth token in vault.
/// Token is stored in vault under "ai:<`provider_name`>" (same key format as other providers).
async fn update_config_api_key(
    config: &Arc<RwLock<Config>>,
    vault: &Arc<SharedTokenManager>,
    provider_name: &str,
    token: Option<&str>,
) {
    // Store/delete token in vault
    let vault_key = format!("ai:{provider_name}");
    if let Some(token) = token {
        if let Err(e) = vault.store_secret(&vault_key, token) {
            warn!(error = %e, "Failed to store OAuth token in vault");
        }
    } else {
        if let Err(e) = vault.delete_secret(&vault_key) {
            warn!(error = %e, "Failed to delete OAuth token from vault");
        }
        // Clear the full blob too so logout fully forgets the refresh token.
        if let Err(e) = vault.delete_secret(&oauth_blob_key(provider_name)) {
            warn!(error = %e, "Failed to delete OAuth token blob from vault");
        }
    }

    let mut cfg = config.write().await;

    if token.is_some() {
        let provider = cfg
            .providers
            .entry(provider_name.to_string())
            .or_insert_with(|| new_provider_from_preset(provider_name));
        provider.api_key = None; // Never persist to config — vault is the source
        provider.enabled = true;
        provider.verified = true;
    } else if let Some(provider) = cfg.providers.get_mut(provider_name) {
        provider.api_key = None;
    }

    if let Err(e) = cfg.save_incremental(&["providers"]) {
        warn!(error = %e, "Failed to persist config after OAuth update");
    }
}

/// Attempt to refresh an expired token. Returns true on success.
///
/// `pub(crate)` so the runtime self-heal path (`codex_token_refresher`) can
/// reuse the exact persistence logic the Panel status-poll uses, instead of
/// duplicating refresh + vault writes.
pub(crate) async fn try_refresh(
    oauth_state: &SharedOAuthState,
    config: &Arc<RwLock<Config>>,
    vault: &Arc<SharedTokenManager>,
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
            update_config_api_key(config, vault, provider_name, Some(&new_cache.access_token))
                .await;
            // Persist refresh_token + real expiry so a restart can recover.
            persist_oauth_blob(vault, provider_name, &new_cache);
            info!("OAuth token refreshed successfully");
            true
        }
        Err(e) => {
            warn!(error = %e, "OAuth token refresh failed");
            false
        }
    }
}

/// Restore OAuth state from vault at startup.
///
/// If vault has an access token for "ai:chatgpt", we know the user
/// previously logged in via OAuth. We build an `OAuthTokenCache` with
/// a conservative 1-hour expiry window so that `oauthStatus` will
/// trigger a refresh if the token is actually stale.
pub fn restore_from_vault(config: &Config, vault: &SharedTokenManager) -> Option<OAuthTokenCache> {
    // Check that chatgpt provider exists and is verified
    let _provider = config.providers.get("chatgpt")?;

    // Prefer the full blob: it carries the real refresh_token + expiry, so the
    // runtime/background refresh can actually renew the token after a restart.
    if let Ok(Some(secret)) = vault.get_secret(&oauth_blob_key("chatgpt")) {
        if let Ok(cache) = serde_json::from_str::<OAuthTokenCache>(secret.expose()) {
            if !cache.access_token.is_empty() {
                debug!("Restored OAuth blob from vault (chatgpt provider)");
                return Some(cache);
            }
        }
    }

    // Legacy fallback: only the access token survived (logged in before the
    // blob existed). Refresh token is unrecoverable; assume a 1h window so
    // `oauthStatus` surfaces a re-login prompt once it lapses.
    let api_key = match vault.get_secret("ai:chatgpt") {
        Ok(Some(secret)) => secret.expose().to_string(),
        _ => return None,
    };

    if api_key.is_empty() {
        return None;
    }

    let cache = OAuthTokenCache {
        access_token: api_key,
        refresh_token: None,
        expires_at_unix: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs()
            + 3600,
        session_id: String::new(),
    };

    debug!("Restored OAuth state from vault (chatgpt provider, legacy access-only)");
    Some(cache)
}

// ─── RPC Handlers ────────────────────────────────────────────────────────────

/// `providers.oauthLogin` — Start browser OAuth flow, store token.
pub async fn handle_oauth_login(
    request: JsonRpcRequest,
    oauth_state: Arc<RwLock<Option<OAuthTokenCache>>>,
    config: Arc<RwLock<Config>>,
    vault: Arc<SharedTokenManager>,
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

    let auth = match CodexAuth::authorize_via_browser().await {
        Ok(auth) => auth,
        Err(e) => {
            error!(error = %e, "OAuth browser login failed");
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("OAuth login failed: {e}"),
            );
        }
    };

    let cache = OAuthTokenCache::from_auth(&auth);
    let expires_in = cache.expires_in_seconds();

    // Store in memory (with expiry + refresh token metadata)
    *oauth_state.write().await = Some(cache.clone());

    // Persist access_token to vault (legacy key read by providers) + the full
    // blob (refresh token + expiry) so a restart can recover and auto-refresh.
    update_config_api_key(&config, &vault, provider_name, Some(&cache.access_token)).await;
    persist_oauth_blob(&vault, provider_name, &cache);

    info!(provider = provider_name, "OAuth login successful");

    JsonRpcResponse::success(
        request.id,
        oauth_status_value(true, provider_name, expires_in, None),
    )
}

/// Build a `providers.oauth{Login,Status}` response from the shared contract
/// type.
///
/// Four call sites used to hand-write the same JSON object, which is four
/// chances for a key to drift from the one the Panel reads. Serialising the
/// contract type makes the key set the contract's rather than each site's.
fn oauth_status_value(
    connected: bool,
    provider: &str,
    expires_in_seconds: Option<u64>,
    error: Option<&str>,
) -> serde_json::Value {
    serde_json::to_value(crate::gateway::handlers::providers::OAuthStatus {
        connected,
        provider: Some(provider.to_string()),
        expires_in_seconds,
        error: error.map(str::to_string),
    })
    .unwrap_or_else(|_| json!({ "connected": connected }))
}

/// `providers.oauthLogout` — Clear stored OAuth token.
pub async fn handle_oauth_logout(
    request: JsonRpcRequest,
    oauth_state: Arc<RwLock<Option<OAuthTokenCache>>>,
    config: Arc<RwLock<Config>>,
    vault: Arc<SharedTokenManager>,
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

    // Clear vault token + config
    update_config_api_key(&config, &vault, provider_name, None).await;

    info!(provider = provider_name, "OAuth logout completed");

    JsonRpcResponse::success(request.id, json!({ "ok": true }))
}

/// `providers.oauthStatus` — Check OAuth connection status, auto-refresh if expired.
pub async fn handle_oauth_status(
    request: JsonRpcRequest,
    oauth_state: Arc<RwLock<Option<OAuthTokenCache>>>,
    config: Arc<RwLock<Config>>,
    vault: Arc<SharedTokenManager>,
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
            oauth_status_value(false, provider_name, None, None),
        );
    }

    // Check expiry and try refresh if needed
    let is_expired = oauth_state
        .read()
        .await
        .as_ref()
        .is_none_or(|c| c.is_expired());

    if is_expired {
        debug!("OAuth token expired, attempting refresh");
        let refreshed = try_refresh(&oauth_state, &config, &vault, provider_name).await;
        if !refreshed {
            *oauth_state.write().await = None;
            return JsonRpcResponse::success(
                request.id,
                oauth_status_value(
                    false,
                    provider_name,
                    None,
                    Some("Token expired and refresh failed. Please re-login."),
                ),
            );
        }
    }

    let expires_in = oauth_state
        .read()
        .await
        .as_ref()
        .and_then(|c| c.expires_in_seconds());

    JsonRpcResponse::success(
        request.id,
        oauth_status_value(true, provider_name, expires_in, None),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_oauth_token_cache_roundtrip() {
        let auth = CodexAuth {
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

    // Tests for restore_from_vault are omitted here because the function
    // requires a full SharedTokenManager with SQLite + vault infrastructure.
    // Integration tests cover this path.
}
