//! OAuth Provider
//!
//! Implements the OAuth 2.0 authorization code flow with PKCE for MCP servers.
//!
//! # Flow Overview
//!
//! 1. `start_authorization()` - Generate authorization URL with PKCE
//! 2. User visits URL and authorizes in browser
//! 3. Callback server receives authorization code
//! 4. `finish_authorization()` - Exchange code for tokens
//! 5. `refresh_token()` - Refresh expired tokens

use std::sync::Arc;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::Rng;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{AlephError, Result};
use crate::mcp::auth::storage::{ClientInfo, OAuthStorage, OAuthTokens};

/// OAuth server metadata (from .well-known/oauth-authorization-server)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthServerMetadata {
    /// Authorization endpoint URL
    pub authorization_endpoint: String,
    /// Token endpoint URL
    pub token_endpoint: String,
    /// Registration endpoint URL (for dynamic client registration)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registration_endpoint: Option<String>,
    /// Supported response types
    #[serde(default)]
    pub response_types_supported: Vec<String>,
    /// Supported grant types
    #[serde(default)]
    pub grant_types_supported: Vec<String>,
    /// Supported code challenge methods
    #[serde(default)]
    pub code_challenge_methods_supported: Vec<String>,
}

/// Authorization request parameters
#[derive(Debug, Clone)]
pub struct AuthorizationRequest {
    /// The authorization URL to open in browser
    pub authorization_url: String,
    /// The state parameter for CSRF protection
    pub state: String,
    /// The PKCE code verifier (store this for token exchange)
    pub code_verifier: String,
}

/// OAuth provider for MCP server authentication
///
/// Handles the OAuth 2.0 authorization code flow with PKCE.
pub struct OAuthProvider {
    /// HTTP client for making requests
    client: Client,
    /// OAuth credential storage
    storage: Arc<OAuthStorage>,
    /// Server name for identification
    server_name: String,
    /// Server URL
    server_url: String,
    /// Callback URL for authorization code
    callback_url: String,
}

impl OAuthProvider {
    /// Create a new OAuth provider
    ///
    /// # Arguments
    ///
    /// * `storage` - OAuth credential storage
    /// * `server_name` - Name for identifying this server
    /// * `server_url` - The MCP server URL (for discovering OAuth endpoints)
    /// * `callback_url` - URL for receiving authorization code callback
    pub fn new(
        storage: Arc<OAuthStorage>,
        server_name: impl Into<String>,
        server_url: impl Into<String>,
        callback_url: impl Into<String>,
    ) -> Self {
        Self {
            client: Client::new(),
            storage,
            server_name: server_name.into(),
            server_url: server_url.into(),
            callback_url: callback_url.into(),
        }
    }

    /// Discover OAuth server metadata
    ///
    /// Fetches the OAuth configuration from .well-known/oauth-authorization-server
    pub async fn discover_metadata(&self) -> Result<OAuthServerMetadata> {
        let url = format!(
            "{}/.well-known/oauth-authorization-server",
            self.server_url.trim_end_matches('/')
        );

        let response = self.client.get(&url).send().await.map_err(|e| {
            AlephError::IoError(format!("Failed to fetch OAuth metadata: {}", e))
        })?;

        if !response.status().is_success() {
            return Err(AlephError::IoError(format!(
                "OAuth metadata request failed with status {}",
                response.status()
            )));
        }

        response.json::<OAuthServerMetadata>().await.map_err(|e| {
            AlephError::IoError(format!("Failed to parse OAuth metadata: {}", e))
        })
    }

    /// Register client dynamically (if server supports it)
    ///
    /// Uses OAuth 2.0 Dynamic Client Registration (RFC 7591)
    pub async fn register_client(&self, metadata: &OAuthServerMetadata) -> Result<ClientInfo> {
        let registration_endpoint = metadata.registration_endpoint.as_ref().ok_or_else(|| {
            AlephError::IoError(
                "Server does not support dynamic client registration".to_string(),
            )
        })?;

        let request_body = serde_json::json!({
            "client_name": format!("Aether MCP Client ({})", self.server_name),
            "redirect_uris": [&self.callback_url],
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none"
        });

        let response = self
            .client
            .post(registration_endpoint)
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
            .await
            .map_err(|e| AlephError::IoError(format!("Client registration failed: {}", e)))?;

        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(AlephError::IoError(format!(
                "Client registration failed: {}",
                body
            )));
        }

        #[derive(Deserialize)]
        struct RegistrationResponse {
            client_id: String,
            client_secret: Option<String>,
            client_id_issued_at: Option<i64>,
            client_secret_expires_at: Option<i64>,
        }

        let reg_response: RegistrationResponse = response.json().await.map_err(|e| {
            AlephError::IoError(format!("Failed to parse registration response: {}", e))
        })?;

        let client_info = ClientInfo {
            client_id: reg_response.client_id,
            client_secret: reg_response.client_secret,
            client_id_issued_at: reg_response.client_id_issued_at,
            client_secret_expires_at: reg_response.client_secret_expires_at,
        };

        // Save client info
        self.storage
            .save_client_info(&self.server_name, &client_info)
            .await?;

        Ok(client_info)
    }

    /// Start the authorization flow
    ///
    /// Generates an authorization URL that the user should visit in their browser.
    /// Returns the URL along with the PKCE code verifier and state that should be
    /// stored for the token exchange.
    pub async fn start_authorization(
        &self,
        metadata: &OAuthServerMetadata,
        client_id: &str,
        scope: Option<&str>,
    ) -> Result<AuthorizationRequest> {
        // Generate PKCE code verifier
        let code_verifier = generate_code_verifier();
        let code_challenge = generate_code_challenge(&code_verifier);

        // Generate state for CSRF protection
        let state = generate_state();

        // Build authorization URL
        let mut url = url::Url::parse(&metadata.authorization_endpoint).map_err(|e| {
            AlephError::IoError(format!("Invalid authorization endpoint: {}", e))
        })?;

        url.query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", client_id)
            .append_pair("redirect_uri", &self.callback_url)
            .append_pair("state", &state)
            .append_pair("code_challenge", &code_challenge)
            .append_pair("code_challenge_method", "S256");

        if let Some(scope) = scope {
            url.query_pairs_mut().append_pair("scope", scope);
        }

        // Store state and code verifier for later
        let mut entry = self
            .storage
            .get_entry(&self.server_name)
            .await?
            .unwrap_or_default();
        entry.code_verifier = Some(code_verifier.clone());
        entry.oauth_state = Some(state.clone());
        entry.server_url = Some(self.server_url.clone());
        self.storage.save_entry(&self.server_name, &entry).await?;

        Ok(AuthorizationRequest {
            authorization_url: url.to_string(),
            state,
            code_verifier,
        })
    }

    /// Finish the authorization flow by exchanging the code for tokens
    ///
    /// # Arguments
    ///
    /// * `metadata` - OAuth server metadata
    /// * `client_id` - Client ID
    /// * `code` - Authorization code received from callback
    /// * `received_state` - State parameter received from callback (for verification)
    pub async fn finish_authorization(
        &self,
        metadata: &OAuthServerMetadata,
        client_id: &str,
        code: &str,
        received_state: &str,
    ) -> Result<OAuthTokens> {
        // Get stored state and code verifier
        let entry = self.storage.get_entry(&self.server_name).await?.ok_or_else(|| {
            AlephError::IoError("No pending authorization found".to_string())
        })?;

        let stored_state = entry.oauth_state.ok_or_else(|| {
            AlephError::IoError("No stored state found".to_string())
        })?;

        let code_verifier = entry.code_verifier.ok_or_else(|| {
            AlephError::IoError("No code verifier found".to_string())
        })?;

        // Verify state matches
        if stored_state != received_state {
            return Err(AlephError::IoError(
                "State mismatch - possible CSRF attack".to_string(),
            ));
        }

        // Exchange code for tokens
        let params = [
            ("grant_type", "authorization_code"),
            ("client_id", client_id),
            ("code", code),
            ("redirect_uri", &self.callback_url),
            ("code_verifier", &code_verifier),
        ];

        let response = self
            .client
            .post(&metadata.token_endpoint)
            .form(&params)
            .send()
            .await
            .map_err(|e| AlephError::IoError(format!("Token exchange failed: {}", e)))?;

        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(AlephError::IoError(format!(
                "Token exchange failed: {}",
                body
            )));
        }

        let tokens = parse_token_response(response).await?;

        // Save tokens
        self.storage
            .save_tokens(&self.server_name, &tokens)
            .await?;

        // Clear temporary state
        let mut entry = self.storage.get_entry(&self.server_name).await?.unwrap_or_default();
        entry.code_verifier = None;
        entry.oauth_state = None;
        self.storage.save_entry(&self.server_name, &entry).await?;

        Ok(tokens)
    }

    /// Refresh an expired access token using stored refresh token
    ///
    /// Uses the refresh token from storage to obtain a new access token.
    pub async fn refresh_token(
        &self,
        metadata: &OAuthServerMetadata,
        client_id: &str,
    ) -> Result<OAuthTokens> {
        let current_tokens = self
            .storage
            .get_tokens(&self.server_name)
            .await?
            .ok_or_else(|| AlephError::IoError("No tokens to refresh".to_string()))?;

        let refresh_token = current_tokens.refresh_token.ok_or_else(|| {
            AlephError::IoError("No refresh token available".to_string())
        })?;

        self.refresh_token_with(metadata, client_id, &refresh_token)
            .await
    }

    /// Refresh an expired access token
    ///
    /// Uses the refresh_token grant to obtain a new access token.
    /// Automatically saves the new tokens to storage.
    pub async fn refresh_token_with(
        &self,
        metadata: &OAuthServerMetadata,
        client_id: &str,
        refresh_token: &str,
    ) -> Result<OAuthTokens> {
        let params = [
            ("grant_type", "refresh_token"),
            ("client_id", client_id),
            ("refresh_token", refresh_token),
        ];

        let response = self
            .client
            .post(&metadata.token_endpoint)
            .form(&params)
            .send()
            .await
            .map_err(|e| AlephError::IoError(format!("Token refresh failed: {}", e)))?;

        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(AlephError::IoError(format!(
                "Token refresh failed: {}",
                body
            )));
        }

        let tokens = parse_token_response(response).await?;

        // Save new tokens
        self.storage
            .save_tokens(&self.server_name, &tokens)
            .await?;

        tracing::info!(
            server = %self.server_name,
            "OAuth tokens refreshed successfully"
        );

        Ok(tokens)
    }

    /// Get valid tokens, refreshing if necessary
    ///
    /// Returns cached tokens if still valid, otherwise attempts to refresh.
    pub async fn get_valid_tokens(
        &self,
        metadata: &OAuthServerMetadata,
        client_id: &str,
    ) -> Result<Option<OAuthTokens>> {
        let tokens = match self.storage.get_tokens(&self.server_name).await? {
            Some(t) => t,
            None => return Ok(None),
        };

        if !tokens.is_expired() {
            return Ok(Some(tokens));
        }

        if tokens.can_refresh() {
            match self.refresh_token(metadata, client_id).await {
                Ok(new_tokens) => return Ok(Some(new_tokens)),
                Err(e) => {
                    tracing::warn!(
                        server = %self.server_name,
                        error = %e,
                        "Token refresh failed"
                    );
                }
            }
        }

        // Tokens expired and can't refresh
        Ok(None)
    }

    /// Check if tokens need refresh and refresh if possible
    ///
    /// Returns new tokens if refreshed, or existing tokens if still valid.
    /// Returns None if no tokens exist or refresh failed without refresh_token.
    pub async fn ensure_valid_token(
        &self,
        metadata: &OAuthServerMetadata,
        client_id: &str,
    ) -> Result<Option<OAuthTokens>> {
        let tokens = match self.storage.get_tokens(&self.server_name).await? {
            Some(t) => t,
            None => return Ok(None),
        };

        if !tokens.is_expired() {
            return Ok(Some(tokens));
        }

        // Token is expired, try to refresh
        if let Some(ref refresh) = tokens.refresh_token {
            match self.refresh_token_with(metadata, client_id, refresh).await {
                Ok(new_tokens) => return Ok(Some(new_tokens)),
                Err(e) => {
                    tracing::warn!(
                        server = %self.server_name,
                        error = %e,
                        "Failed to refresh token, will need re-authorization"
                    );
                    return Ok(None);
                }
            }
        }

        // No refresh token, need re-authorization
        tracing::warn!(
            server = %self.server_name,
            "Token expired and no refresh token available"
        );
        Ok(None)
    }
}

/// Generate a cryptographically random code verifier for PKCE
fn generate_code_verifier() -> String {
    let mut rng = rand::thread_rng();
    let bytes: Vec<u8> = (0..32).map(|_| rng.gen()).collect();
    URL_SAFE_NO_PAD.encode(&bytes)
}

/// Generate code challenge from verifier (SHA256 + base64url)
fn generate_code_challenge(verifier: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let hash = hasher.finalize();
    URL_SAFE_NO_PAD.encode(hash)
}

/// Generate a random state string for CSRF protection
fn generate_state() -> String {
    let mut rng = rand::thread_rng();
    let bytes: Vec<u8> = (0..16).map(|_| rng.gen()).collect();
    URL_SAFE_NO_PAD.encode(&bytes)
}

/// Parse token response from OAuth server
async fn parse_token_response(response: reqwest::Response) -> Result<OAuthTokens> {
    #[derive(Deserialize)]
    struct TokenResponse {
        access_token: String,
        refresh_token: Option<String>,
        expires_in: Option<i64>,
        scope: Option<String>,
    }

    let token_response: TokenResponse = response.json().await.map_err(|e| {
        AlephError::IoError(format!("Failed to parse token response: {}", e))
    })?;

    let expires_at = token_response.expires_in.map(|exp| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
            + exp
    });

    Ok(OAuthTokens {
        access_token: token_response.access_token,
        refresh_token: token_response.refresh_token,
        expires_at,
        scope: token_response.scope,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_code_verifier_generation() {
        let verifier = generate_code_verifier();
        // Code verifier should be URL-safe base64 encoded
        assert!(verifier.len() >= 43); // 32 bytes base64 encoded
        assert!(verifier.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    }

    #[test]
    fn test_code_challenge_generation() {
        let verifier = "test_verifier_12345678901234567890";
        let challenge = generate_code_challenge(verifier);
        // Challenge should be URL-safe base64 encoded SHA256
        assert_eq!(challenge.len(), 43); // SHA256 = 32 bytes = 43 base64 chars (no padding)
    }

    #[test]
    fn test_state_generation() {
        let state = generate_state();
        // State should be URL-safe base64 encoded
        assert!(state.len() >= 22); // 16 bytes base64 encoded
        assert!(state.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    }

    #[test]
    fn test_oauth_server_metadata_serialization() {
        let metadata = OAuthServerMetadata {
            authorization_endpoint: "https://example.com/authorize".to_string(),
            token_endpoint: "https://example.com/token".to_string(),
            registration_endpoint: Some("https://example.com/register".to_string()),
            response_types_supported: vec!["code".to_string()],
            grant_types_supported: vec!["authorization_code".to_string(), "refresh_token".to_string()],
            code_challenge_methods_supported: vec!["S256".to_string()],
        };

        let json = serde_json::to_string(&metadata).unwrap();
        assert!(json.contains("authorization_endpoint"));
        assert!(json.contains("token_endpoint"));

        let deserialized: OAuthServerMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.authorization_endpoint, metadata.authorization_endpoint);
    }

    #[tokio::test]
    async fn test_ensure_valid_token_not_expired() {
        use tempfile::tempdir;

        // Create temporary storage
        let dir = tempdir().unwrap();
        let storage = Arc::new(OAuthStorage::new(dir.path().join("mcp-auth.json")));

        // Create a non-expired token (expires far in the future)
        let tokens = OAuthTokens {
            access_token: "valid_token".to_string(),
            refresh_token: Some("refresh_token".to_string()),
            expires_at: Some(9999999999), // Far in the future
            scope: Some("read write".to_string()),
        };

        // Save the token
        storage.save_tokens("test-server", &tokens).await.unwrap();

        // Create the provider
        let provider = OAuthProvider::new(
            storage,
            "test-server",
            "https://example.com",
            "http://localhost:8080/callback",
        );

        // Create metadata (not actually used since token is valid)
        let metadata = OAuthServerMetadata {
            authorization_endpoint: "https://example.com/authorize".to_string(),
            token_endpoint: "https://example.com/token".to_string(),
            registration_endpoint: None,
            response_types_supported: vec!["code".to_string()],
            grant_types_supported: vec!["authorization_code".to_string()],
            code_challenge_methods_supported: vec!["S256".to_string()],
        };

        // Call ensure_valid_token - should return existing token without refresh
        let result = provider
            .ensure_valid_token(&metadata, "client_id")
            .await
            .unwrap();

        assert!(result.is_some());
        let returned_tokens = result.unwrap();
        assert_eq!(returned_tokens.access_token, "valid_token");
    }

    #[tokio::test]
    async fn test_ensure_valid_token_no_tokens() {
        use tempfile::tempdir;

        // Create temporary storage with no tokens
        let dir = tempdir().unwrap();
        let storage = Arc::new(OAuthStorage::new(dir.path().join("mcp-auth.json")));

        // Create the provider
        let provider = OAuthProvider::new(
            storage,
            "test-server",
            "https://example.com",
            "http://localhost:8080/callback",
        );

        let metadata = OAuthServerMetadata {
            authorization_endpoint: "https://example.com/authorize".to_string(),
            token_endpoint: "https://example.com/token".to_string(),
            registration_endpoint: None,
            response_types_supported: vec!["code".to_string()],
            grant_types_supported: vec!["authorization_code".to_string()],
            code_challenge_methods_supported: vec!["S256".to_string()],
        };

        // Call ensure_valid_token - should return None since no tokens exist
        let result = provider
            .ensure_valid_token(&metadata, "client_id")
            .await
            .unwrap();

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_ensure_valid_token_expired_no_refresh() {
        use tempfile::tempdir;

        // Create temporary storage
        let dir = tempdir().unwrap();
        let storage = Arc::new(OAuthStorage::new(dir.path().join("mcp-auth.json")));

        // Create an expired token without refresh token
        let tokens = OAuthTokens {
            access_token: "expired_token".to_string(),
            refresh_token: None, // No refresh token
            expires_at: Some(0), // Already expired (Unix epoch)
            scope: None,
        };

        // Save the token
        storage.save_tokens("test-server", &tokens).await.unwrap();

        // Create the provider
        let provider = OAuthProvider::new(
            storage,
            "test-server",
            "https://example.com",
            "http://localhost:8080/callback",
        );

        let metadata = OAuthServerMetadata {
            authorization_endpoint: "https://example.com/authorize".to_string(),
            token_endpoint: "https://example.com/token".to_string(),
            registration_endpoint: None,
            response_types_supported: vec!["code".to_string()],
            grant_types_supported: vec!["authorization_code".to_string()],
            code_challenge_methods_supported: vec!["S256".to_string()],
        };

        // Call ensure_valid_token - should return None since token is expired and no refresh token
        let result = provider
            .ensure_valid_token(&metadata, "client_id")
            .await
            .unwrap();

        assert!(result.is_none());
    }
}
