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
//! 5. `ensure_valid_token()` - Refresh expired tokens

use crate::sync_primitives::Arc;
use std::time::Duration;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::RngExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{AlephError, Result};
use crate::mcp::auth::storage::{ClientInfo, OAuthStorage, OAuthTokens};

/// OAuth server metadata (from .well-known/oauth-authorization-server)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthServerMetadata {
    /// The authorization server's issuer identifier (RFC 8414).
    ///
    /// Identity of the authorization server itself, as opposed to any of its
    /// endpoints. Client credentials are scoped to it, and RFC 9207 has the
    /// client check an authorization response's `iss` against it. Optional
    /// because not every deployment advertises one.
    #[serde(default)]
    pub issuer: Option<String>,
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
            // Bound every OAuth round-trip (metadata discovery, token exchange,
            // refresh). reqwest's default client has NO timeout, so a hung
            // endpoint would block the caller indefinitely.
            client: Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap_or_else(|_| Client::new()),
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

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| AlephError::IoError(format!("Failed to fetch OAuth metadata: {e}")))?;

        if !response.status().is_success() {
            return Err(AlephError::IoError(format!(
                "OAuth metadata request failed with status {}",
                response.status()
            )));
        }

        let metadata: OAuthServerMetadata = response
            .json()
            .await
            .map_err(|e| AlephError::IoError(format!("Failed to parse OAuth metadata: {e}")))?;

        // RFC 8414 §3.3: every endpoint must be on the same origin as
        // `issuer`. A server that returns an `issuer` pointing at one host
        // but an `authorization_endpoint` pointing at another is hosting a
        // confused-deputy scenario: the user is sent to the attacker's
        // authorization page while the client_id is bound to the legit
        // authorization server. Refuse to proceed.
        validate_metadata_origins(&self.server_name, &metadata)?;

        Ok(metadata)
    }

    /// Register client dynamically (if server supports it)
    ///
    /// Uses OAuth 2.0 Dynamic Client Registration (RFC 7591)
    pub async fn register_client(&self, metadata: &OAuthServerMetadata) -> Result<ClientInfo> {
        let registration_endpoint = metadata.registration_endpoint.as_ref().ok_or_else(|| {
            AlephError::IoError("Server does not support dynamic client registration".to_string())
        })?;

        let request_body = registration_request_body(
            &format!("Aleph MCP Client ({})", self.server_name),
            &self.callback_url,
        );

        let response = self
            .client
            .post(registration_endpoint)
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
            .await
            .map_err(|e| AlephError::IoError(format!("Client registration failed: {e}")))?;

        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_else(|e| {
                tracing::warn!(error = %e, "Failed to read client registration error response body");
                format!("<failed to read body: {e}>")
            });
            return Err(AlephError::IoError(format!(
                "Client registration failed: {body}"
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
            AlephError::IoError(format!("Failed to parse registration response: {e}"))
        })?;

        let client_info = ClientInfo {
            client_id: reg_response.client_id,
            client_secret: reg_response.client_secret,
            client_id_issued_at: reg_response.client_id_issued_at,
            client_secret_expires_at: reg_response.client_secret_expires_at,
            // rust-doctor-disable-next-line excessive-clone
            issuer: metadata.issuer.clone(),
        };

        // Save client info
        self.storage
            .save_client_info(&self.server_name, &client_info)
            .await?;

        Ok(client_info)
    }

    /// Stored client credentials, but only if this authorization server is the
    /// one that issued them.
    ///
    /// Client credentials are bound to their issuer: they must not be presented
    /// to a different authorization server, and a server that has changed
    /// issuers must be re-registered with. Returning `None` on a mismatch is
    /// what makes the caller do that, so the check cannot be skipped by
    /// reaching past it to the storage layer.
    ///
    /// A stored entry with no recorded issuer predates this field. It is reused
    /// only when the current metadata also advertises none — otherwise the
    /// pairing is unverifiable and re-registering is the cheap, safe answer.
    pub async fn client_info_for(
        &self,
        metadata: &OAuthServerMetadata,
    ) -> Result<Option<ClientInfo>> {
        let Some(stored) = self.storage.get_client_info(&self.server_name).await? else {
            return Ok(None);
        };

        if stored.issuer == metadata.issuer {
            return Ok(Some(stored));
        }

        tracing::info!(
            server = %self.server_name,
            stored_issuer = ?stored.issuer,
            current_issuer = ?metadata.issuer,
            "Authorization server issuer changed; re-registering rather than \
             reusing client credentials"
        );
        Ok(None)
    }

    /// Start the authorization flow
    ///
    /// Generates an authorization URL that the user should visit in their
    /// browser. The PKCE code verifier and state are kept in storage for the
    /// token exchange in [`Self::finish_authorization`].
    pub async fn start_authorization(
        &self,
        metadata: &OAuthServerMetadata,
        client_id: &str,
        scope: Option<&str>,
    ) -> Result<String> {
        // Generate PKCE code verifier
        let code_verifier = generate_code_verifier();
        let code_challenge = generate_code_challenge(&code_verifier);

        // Generate state for CSRF protection
        let state = generate_state();

        // Build authorization URL
        let mut url = url::Url::parse(&metadata.authorization_endpoint)
            .map_err(|e| AlephError::IoError(format!("Invalid authorization endpoint: {e}")))?;

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
        // rust-doctor-disable-next-line excessive-clone
        entry.code_verifier = Some(code_verifier.clone());
        // rust-doctor-disable-next-line excessive-clone
        entry.oauth_state = Some(state.clone());
        // Recorded now so the authorization response's `iss` has something
        // trustworthy to be checked against later (RFC 9207).
        // rust-doctor-disable-next-line excessive-clone
        entry.issuer = metadata.issuer.clone();
        self.storage.save_entry(&self.server_name, &entry).await?;

        Ok(url.to_string())
    }

    /// Finish the authorization flow by exchanging the code for tokens
    ///
    /// # Arguments
    ///
    /// * `metadata` - OAuth server metadata
    /// * `client_id` - Client ID
    /// * `code` - Authorization code received from callback
    /// * `received_state` - State parameter received from callback (for verification)
    /// * `received_iss` - The `iss` parameter from the authorization response,
    ///   when the authorization server sent one (RFC 9207). Validated against
    ///   the issuer recorded at [`Self::start_authorization`] *before* the code
    ///   is redeemed, so a code from a different authorization server cannot be
    ///   exchanged at this one's token endpoint.
    pub async fn finish_authorization(
        &self,
        metadata: &OAuthServerMetadata,
        client_id: &str,
        code: &str,
        received_state: &str,
        received_iss: Option<&str>,
    ) -> Result<OAuthTokens> {
        // Get stored state and code verifier
        let entry = self
            .storage
            .get_entry(&self.server_name)
            .await?
            .ok_or_else(|| AlephError::IoError("No pending authorization found".to_string()))?;

        let stored_state = entry
            .oauth_state
            .ok_or_else(|| AlephError::IoError("No stored state found".to_string()))?;

        let code_verifier = entry
            .code_verifier
            .ok_or_else(|| AlephError::IoError("No code verifier found".to_string()))?;

        if !crate::security::secret_equal_bytes(stored_state.as_bytes(), received_state.as_bytes())
        {
            return Err(AlephError::IoError(
                "State mismatch - possible CSRF attack".to_string(),
            ));
        }

        // RFC 9207, checked before the code is redeemed.
        validate_response_issuer(
            &self.server_name,
            entry.issuer.as_deref().or(metadata.issuer.as_deref()),
            received_iss,
        )?;

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
            .map_err(|e| AlephError::IoError(format!("Token exchange failed: {e}")))?;

        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_else(|e| {
                tracing::warn!(error = %e, "Failed to read token exchange error response body");
                format!("<failed to read body: {e}>")
            });
            return Err(AlephError::IoError(format!(
                "Token exchange failed: {body}"
            )));
        }

        let tokens = parse_token_response(response).await?;

        // Save tokens
        self.storage.save_tokens(&self.server_name, &tokens).await?;

        // Clear temporary state
        let mut entry = self
            .storage
            .get_entry(&self.server_name)
            .await?
            .unwrap_or_default();
        entry.code_verifier = None;
        entry.oauth_state = None;
        self.storage.save_entry(&self.server_name, &entry).await?;

        Ok(tokens)
    }

    /// Refresh an expired access token
    ///
    /// Uses the `refresh_token` grant to obtain a new access token.
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
            .map_err(|e| AlephError::IoError(format!("Token refresh failed: {e}")))?;

        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_else(|e| {
                tracing::warn!(error = %e, "Failed to read token refresh error response body");
                format!("<failed to read body: {e}>")
            });
            return Err(AlephError::IoError(format!("Token refresh failed: {body}")));
        }

        let mut tokens = parse_token_response(response).await?;

        // RFC 6749 §6: a refresh response MAY omit `refresh_token`, in which
        // case the client keeps reusing the current one. Without this, saving
        // `refresh_token: None` would wipe the stored refresh token and force a
        // full re-authorization on the next expiry.
        if tokens.refresh_token.is_none() {
            tokens.refresh_token = Some(refresh_token.to_string());
        }

        // Save new tokens
        self.storage.save_tokens(&self.server_name, &tokens).await?;

        tracing::info!(
            server = %self.server_name,
            "OAuth tokens refreshed successfully"
        );

        Ok(tokens)
    }

    /// Check if tokens need refresh and refresh if possible
    ///
    /// Returns new tokens if refreshed, or existing tokens if still valid.
    /// Returns None if no tokens exist or refresh failed without `refresh_token`.
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

/// The Dynamic Client Registration request body (RFC 7591).
///
/// `application_type` matters more than it looks: Aleph receives the
/// authorization callback on a loopback listener, and OpenID Connect
/// registration defaults to `"web"`, which forbids exactly that redirect shape.
/// Omitting the field is why loopback redirect URIs get rejected by
/// OIDC-backed authorization servers.
fn registration_request_body(client_name: &str, callback_url: &str) -> serde_json::Value {
    serde_json::json!({
        "client_name": client_name,
        "redirect_uris": [callback_url],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
        "application_type": "native"
    })
}

/// Check an authorization response's `iss` against the issuer this flow was
/// started with (RFC 9207).
///
/// Only a *present* `iss` is checked: authorization servers are encouraged, not
/// required, to send one, so its absence cannot be an error without breaking
/// every server that omits it. When it is present it must match — that is the
/// whole point of the parameter, which exists so a code obtained from one
/// authorization server cannot be fed to another's token endpoint (mix-up
/// attack).
///
/// A received issuer with nothing recorded to compare against is also refused:
/// an unverifiable claim is not a weaker version of a verified one.
fn validate_response_issuer(
    server_name: &str,
    recorded: Option<&str>,
    received: Option<&str>,
) -> Result<()> {
    let Some(received) = received else {
        return Ok(());
    };

    match recorded {
        Some(expected) if expected == received => Ok(()),
        Some(expected) => Err(AlephError::IoError(format!(
            "Authorization response issuer mismatch for '{server_name}': expected \
             '{expected}', got '{received}' - refusing to redeem the authorization code"
        ))),
        None => Err(AlephError::IoError(format!(
            "Authorization response for '{server_name}' carried issuer '{received}', but no \
             issuer was recorded to check it against - refusing to redeem the authorization code"
        ))),
    }
}

/// Validate that every endpoint advertised in the metadata is on the same
/// origin as `issuer` (RFC 8414 §3.3).
///
/// Each endpoint is parsed as a URL and compared against the issuer's
/// scheme + host + port. A mismatch is a confused-deputy scenario: the
/// user is sent to one host for authorization while the client_id is
/// bound to another. Refusing the metadata pre-empts the attack before
/// any redirect happens. A missing `issuer` means we have nothing to
/// compare against — refusing the metadata is the safe default.
fn validate_metadata_origins(
    server_name: &str,
    metadata: &OAuthServerMetadata,
) -> Result<()> {
    let issuer = metadata
        .issuer
        .as_deref()
        .ok_or_else(|| {
            AlephError::IoError(format!(
                "OAuth metadata for '{server_name}' omitted the 'issuer' field; \
                 refusing to register against an unverified authorization server"
            ))
        })?;

    let issuer_origin = url_origin_str(issuer).ok_or_else(|| {
        AlephError::IoError(format!(
            "OAuth metadata for '{server_name}' declared an unparseable issuer \
             '{issuer}'; refusing to proceed"
        ))
    })?;

    for endpoint in [
        ("authorization_endpoint", &metadata.authorization_endpoint),
        ("token_endpoint", &metadata.token_endpoint),
    ] {
        let (name, value) = endpoint;
        let origin = url_origin_str(value).ok_or_else(|| {
            AlephError::IoError(format!(
                "OAuth metadata for '{server_name}' declared {name} '{value}' \
                 which is not a valid URL; refusing to proceed"
            ))
        })?;
        if origin != issuer_origin {
            return Err(AlephError::IoError(format!(
                "OAuth metadata for '{server_name}' has {name} '{value}' on a \
                 different origin than issuer '{issuer}'; refusing to proceed"
            )));
        }
    }

    if let Some(reg) = &metadata.registration_endpoint {
        let origin = url_origin_str(reg).ok_or_else(|| {
            AlephError::IoError(format!(
                "OAuth metadata for '{server_name}' declared registration_endpoint \
                 '{reg}' which is not a valid URL; refusing to proceed"
            ))
        })?;
        if origin != issuer_origin {
            return Err(AlephError::IoError(format!(
                "OAuth metadata for '{server_name}' has registration_endpoint '{reg}' \
                 on a different origin than issuer '{issuer}'; refusing to proceed"
            )));
        }
    }

    Ok(())
}

/// Extract the scheme + host + port (`origin`) of a URL string, or `None`
/// when the URL does not parse.
fn url_origin_str(s: &str) -> Option<String> {
    let url = url::Url::parse(s).ok()?;
    Some(url.origin().ascii_serialization())
}

/// Generate a cryptographically random code verifier for PKCE
fn generate_code_verifier() -> String {
    let mut rng = rand::rng();
    let bytes: Vec<u8> = (0..32).map(|_| rng.random::<u8>()).collect();
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
    let mut rng = rand::rng();
    let bytes: Vec<u8> = (0..16).map(|_| rng.random::<u8>()).collect();
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

    let token_response: TokenResponse = response
        .json()
        .await
        .map_err(|e| AlephError::IoError(format!("Failed to parse token response: {e}")))?;

    let expires_at = token_response.expires_in.map(|exp| {
        let now: i64 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .try_into()
            .unwrap_or(i64::MAX);
        now.saturating_add(exp)
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

    const ISSUER: &str = "https://issuer.example.com";
    const OTHER_ISSUER: &str = "https://attacker.example.com";

    #[test]
    fn absent_issuer_in_the_response_is_accepted() {
        // Authorization servers only SHOULD send `iss`; requiring it would lock
        // out every server that predates RFC 9207.
        assert!(validate_response_issuer("srv", Some(ISSUER), None).is_ok());
        assert!(validate_response_issuer("srv", None, None).is_ok());
    }

    #[test]
    fn matching_issuer_is_accepted() {
        assert!(validate_response_issuer("srv", Some(ISSUER), Some(ISSUER)).is_ok());
    }

    #[test]
    fn mismatched_issuer_refuses_to_redeem_the_code() {
        // The mix-up attack this parameter exists to stop: a code minted by one
        // authorization server presented at another's token endpoint.
        let err = validate_response_issuer("srv", Some(ISSUER), Some(OTHER_ISSUER))
            .unwrap_err()
            .to_string();

        assert!(err.contains(ISSUER), "{err}");
        assert!(err.contains(OTHER_ISSUER), "{err}");
        assert!(err.contains("refusing to redeem"), "{err}");
    }

    #[test]
    fn unverifiable_issuer_is_refused_rather_than_trusted() {
        // A claim with nothing to check it against is not a weaker version of a
        // verified one.
        let err = validate_response_issuer("srv", None, Some(ISSUER))
            .unwrap_err()
            .to_string();

        assert!(err.contains("no issuer was recorded"), "{err}");
    }

    #[test]
    fn issuer_comparison_is_exact() {
        // Issuer identifiers compare exactly (RFC 8414); a trailing slash or a
        // case change is a different issuer, not the same one.
        assert!(validate_response_issuer(
            "srv",
            Some("https://issuer.example.com"),
            Some("https://issuer.example.com/")
        )
        .is_err());
        assert!(validate_response_issuer(
            "srv",
            Some("https://issuer.example.com"),
            Some("https://Issuer.Example.com")
        )
        .is_err());
    }

    #[test]
    fn metadata_origins_must_match_issuer() {
        // RFC 8414 §3.3: every endpoint must be on the same origin as the
        // issuer. A server that hands out an issuer pointing at one host
        // and an authorization_endpoint pointing at another is hosting a
        // confused-deputy scenario; the metadata is rejected outright.
        let cross = OAuthServerMetadata {
            issuer: Some("https://api.example.com".to_string()),
            authorization_endpoint: "https://evil.example.com/auth".to_string(),
            token_endpoint: "https://api.example.com/token".to_string(),
            registration_endpoint: None,
            response_types_supported: vec![],
            grant_types_supported: vec![],
            code_challenge_methods_supported: vec![],
        };
        let err = validate_metadata_origins("srv", &cross).unwrap_err().to_string();
        assert!(err.contains("evil.example.com"), "{err}");
        assert!(err.contains("authorization_endpoint"), "{err}");

        // An absent issuer is also refused: nothing to compare against.
        let no_issuer = OAuthServerMetadata {
            issuer: None,
            authorization_endpoint: "https://api.example.com/auth".to_string(),
            token_endpoint: "https://api.example.com/token".to_string(),
            registration_endpoint: None,
            response_types_supported: vec![],
            grant_types_supported: vec![],
            code_challenge_methods_supported: vec![],
        };
        assert!(validate_metadata_origins("srv", &no_issuer).is_err());

        // A well-formed metadata with same-origin endpoints passes.
        let ok = OAuthServerMetadata {
            issuer: Some("https://api.example.com".to_string()),
            authorization_endpoint: "https://api.example.com/auth".to_string(),
            token_endpoint: "https://api.example.com/token".to_string(),
            registration_endpoint: Some("https://api.example.com/register".to_string()),
            response_types_supported: vec![],
            grant_types_supported: vec![],
            code_challenge_methods_supported: vec![],
        };
        assert!(validate_metadata_origins("srv", &ok).is_ok());
    }

    #[tokio::test]
    async fn client_credentials_are_reused_only_for_their_own_issuer() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(OAuthStorage::new(dir.path().join("auth.json")));
        storage
            .save_client_info(
                "srv",
                &ClientInfo {
                    client_id: "client-from-issuer-a".to_string(),
                    client_secret: None,
                    client_id_issued_at: None,
                    client_secret_expires_at: None,
                    issuer: Some(ISSUER.to_string()),
                },
            )
            .await
            .unwrap();

        let provider = OAuthProvider::new(
            Arc::clone(&storage),
            "srv",
            "https://mcp.example.com",
            "http://127.0.0.1:8899/callback",
        );

        let same_issuer = OAuthServerMetadata {
            issuer: Some(ISSUER.to_string()),
            authorization_endpoint: "https://issuer.example.com/authorize".to_string(),
            token_endpoint: "https://issuer.example.com/token".to_string(),
            registration_endpoint: None,
            response_types_supported: vec![],
            grant_types_supported: vec![],
            code_challenge_methods_supported: vec![],
        };
        let reused = provider.client_info_for(&same_issuer).await.unwrap();
        assert_eq!(reused.unwrap().client_id, "client-from-issuer-a");

        // A different authorization server must not be handed this client
        // identity; returning None is what drives a fresh registration.
        let different_issuer = OAuthServerMetadata {
            issuer: Some(OTHER_ISSUER.to_string()),
            ..same_issuer
        };
        assert!(provider
            .client_info_for(&different_issuer)
            .await
            .unwrap()
            .is_none());
    }

    #[test]
    fn dynamic_registration_declares_a_native_application() {
        // Aleph's redirect URI is a loopback listener, which the OIDC default
        // application_type of "web" forbids.
        let body = registration_request_body("Aleph MCP Client (srv)", "http://127.0.0.1:8899/cb");

        assert_eq!(body["application_type"], "native");
        assert_eq!(body["redirect_uris"][0], "http://127.0.0.1:8899/cb");
        assert_eq!(body["token_endpoint_auth_method"], "none");
    }

    #[test]
    fn test_code_verifier_generation() {
        let verifier = generate_code_verifier();
        // Code verifier should be URL-safe base64 encoded
        assert!(verifier.len() >= 43); // 32 bytes base64 encoded
        assert!(verifier
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
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
        assert!(state
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    }

    #[test]
    fn test_oauth_server_metadata_serialization() {
        let metadata = OAuthServerMetadata {
            issuer: Some("https://example.com".to_string()),
            authorization_endpoint: "https://example.com/authorize".to_string(),
            token_endpoint: "https://example.com/token".to_string(),
            registration_endpoint: Some("https://example.com/register".to_string()),
            response_types_supported: vec!["code".to_string()],
            grant_types_supported: vec![
                "authorization_code".to_string(),
                "refresh_token".to_string(),
            ],
            code_challenge_methods_supported: vec!["S256".to_string()],
        };

        let json = serde_json::to_string(&metadata).unwrap();
        assert!(json.contains("authorization_endpoint"));
        assert!(json.contains("token_endpoint"));

        let deserialized: OAuthServerMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(
            deserialized.authorization_endpoint,
            metadata.authorization_endpoint
        );
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
            issuer: Some("https://example.com".to_string()),
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
            issuer: Some("https://example.com".to_string()),
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
            issuer: Some("https://example.com".to_string()),
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
