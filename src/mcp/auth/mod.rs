//! OAuth Authentication for MCP
//!
//! Provides OAuth 2.0 authentication for remote MCP servers.
//!
//! # Components
//!
//! - **Storage** ([`OAuthStorage`]): Secure credential storage
//! - **Provider** ([`OAuthProvider`]): OAuth flow implementation
//! - **Callback** ([`CallbackServer`]): Authorization code callback server
//! - **Refresh** ([`TokenRefreshManager`]): Background token refresh
//!
//! Wiring: [`stored_bearer_token`] is consulted by
//! `McpClient::start_remote_server` to inject `Authorization` headers from
//! stored tokens; the interactive flow is driven by the `mcp_login` builtin
//! tool (see `crate::builtin_tools::mcp_login`).
//!
//! # Usage
//!
//! ```ignore
//! use alephcore::mcp::auth::{OAuthStorage, OAuthTokens, OAuthProvider};
//!
//! // Create storage
//! let storage = Arc::new(OAuthStorage::new(OAuthStorage::default_path()));
//!
//! // Create provider
//! let provider = OAuthProvider::new(
//!     storage.clone(),
//!     "my-server",
//!     "https://api.example.com",
//!     "http://localhost:19877/callback",
//! );
//!
//! // Start authorization flow
//! let metadata = provider.discover_metadata().await?;
//! let auth_req = provider.start_authorization(&metadata, "client_id", None).await?;
//! println!("Open in browser: {}", auth_req.authorization_url);
//!
//! // After user authorizes, exchange code for tokens
//! let tokens = provider.finish_authorization(&metadata, "client_id", &code, &state).await?;
//! ```

mod callback;
mod provider;
mod refresh;
mod storage;

pub use callback::{
    CallbackResult, CallbackServer, DEFAULT_CALLBACK_PORT, DEFAULT_CALLBACK_TIMEOUT,
};
pub use provider::{AuthorizationRequest, OAuthProvider, OAuthServerMetadata};
pub use refresh::{TokenRefreshConfig, TokenRefreshManager};
pub use storage::{ClientInfo, OAuthEntry, OAuthStorage, OAuthTokens};

use crate::sync_primitives::Arc;

/// Look up a usable OAuth access token for a remote MCP server.
///
/// Reads the default [`OAuthStorage`]; a still-valid token is returned as-is,
/// an expired-but-refreshable one is refreshed (re-discovering server
/// metadata for the token endpoint). Returns `None` when the server was never
/// authorized or refresh fails — callers fall back to unauthenticated
/// connection, and the `AuthExpired` error guidance points at `mcp_login`.
pub async fn stored_bearer_token(server: &str, server_url: &str) -> Option<String> {
    let storage = oauth_storage_singleton();
    let entry = storage.get_entry(server).await.ok().flatten()?;
    let tokens = entry.tokens.as_ref()?;

    if !tokens.is_expired() {
        return Some(tokens.access_token.clone());
    }
    if !tokens.can_refresh() {
        return None;
    }

    let client_id = entry.client_info.as_ref()?.client_id.clone();
    let provider = OAuthProvider::new(storage, server, server_url, "");
    let metadata = match provider.discover_metadata().await {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(server, error = %e, "OAuth metadata discovery failed during token refresh");
            return None;
        }
    };
    match provider.ensure_valid_token(&metadata, &client_id).await {
        Ok(Some(tokens)) => Some(tokens.access_token),
        Ok(None) => None,
        Err(e) => {
            tracing::warn!(server, error = %e, "OAuth token refresh failed");
            None
        }
    }
}

fn oauth_storage_singleton() -> Arc<OAuthStorage> {
    use std::sync::OnceLock;
    static STORAGE: OnceLock<Arc<OAuthStorage>> = OnceLock::new();
    Arc::clone(STORAGE.get_or_init(|| {
        Arc::new(OAuthStorage::new(OAuthStorage::default_path()))
    }))
}
