//! ChatGPT OAuth authentication
//!
//! Implements the browser-based OAuth flow for ChatGPT subscription accounts.
//! Uses PKCE (Proof Key for Code Exchange) with S256 challenge method,
//! matching the OpenAI Codex CLI implementation.

use crate::error::{AlephError, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};

/// Characters that must be percent-encoded in URL query values.
/// RFC 3986 unreserved chars (ALPHA / DIGIT / "-" / "." / "_" / "~") are NOT encoded.
const QUERY_ENCODE_SET: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'<')
    .add(b'>')
    .add(b'`')
    .add(b'?')
    .add(b'{')
    .add(b'}')
    .add(b'%')
    .add(b'/')
    .add(b':')
    .add(b'@')
    .add(b'[')
    .add(b']')
    .add(b'\\')
    .add(b'^')
    .add(b'|')
    .add(b'!')
    .add(b'$')
    .add(b'&')
    .add(b'\'')
    .add(b'(')
    .add(b')')
    .add(b'*')
    .add(b'+')
    .add(b',')
    .add(b';')
    .add(b'=');
use reqwest::Client;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::time::{Duration, SystemTime};
use tokio::sync::oneshot;
use tracing::{debug, error, info};

/// OpenAI OAuth client ID (public client, same as Codex CLI)
const OPENAI_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

/// OpenAI auth issuer
const ISSUER: &str = "https://auth.openai.com";

/// OAuth callback timeout (5 minutes)
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(300);

/// Callback path (must match Codex CLI convention)
const CALLBACK_PATH: &str = "/auth/callback";

/// Fixed callback port (must match OpenAI's registered redirect_uri)
const CALLBACK_PORT: u16 = 1455;

/// PKCE code verifier + challenge pair
struct PkceCodes {
    code_verifier: String,
    code_challenge: String,
}

/// Generate PKCE codes (code_verifier + S256 code_challenge)
fn generate_pkce() -> PkceCodes {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let code_verifier = URL_SAFE_NO_PAD.encode(bytes);

    let mut hasher = Sha256::new();
    hasher.update(code_verifier.as_bytes());
    let code_challenge = URL_SAFE_NO_PAD.encode(hasher.finalize());

    PkceCodes {
        code_verifier,
        code_challenge,
    }
}

/// Generate a random state parameter
fn generate_state() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Token response from the OAuth token endpoint
#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    #[allow(dead_code)]
    id_token: Option<String>,
    expires_in: Option<u64>,
}

/// Query params received on the OAuth callback
#[derive(Debug, Deserialize)]
struct CallbackParams {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

/// ChatGPT authentication state
#[derive(Debug, Clone)]
pub struct ChatGptAuth {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: SystemTime,
    pub session_id: String,
}

impl ChatGptAuth {
    /// Check if the access token has expired
    pub fn is_expired(&self) -> bool {
        SystemTime::now() >= self.expires_at
    }

    /// Get the access token, or error if expired
    pub fn access_token(&self) -> Result<&str> {
        if self.is_expired() {
            return Err(AlephError::authentication(
                "chatgpt",
                "Access token has expired, re-authentication required",
            ));
        }
        Ok(&self.access_token)
    }

    /// Build the OAuth authorization URL with PKCE
    fn build_authorize_url(port: u16, state: &str, pkce: &PkceCodes) -> String {
        let encode = |s: &str| utf8_percent_encode(s, QUERY_ENCODE_SET).to_string();
        let redirect_uri = encode(&format!("http://localhost:{}{}", port, CALLBACK_PATH));
        let scope = encode("openid profile email offline_access");

        format!(
            "{}/oauth/authorize?response_type=code&client_id={}&redirect_uri={}&scope={}&code_challenge={}&code_challenge_method=S256&state={}",
            ISSUER,
            OPENAI_CLIENT_ID,
            redirect_uri,
            scope,
            encode(&pkce.code_challenge),
            encode(state),
        )
    }

    /// Run the full OAuth browser authorization flow with PKCE.
    ///
    /// 1. Generates PKCE codes (code_verifier + code_challenge)
    /// 2. Binds a random localhost port
    /// 3. Opens the system browser to the OAuth authorize URL
    /// 4. Waits for the callback with an authorization code (5 min timeout)
    /// 5. Exchanges the code + code_verifier for tokens
    /// 6. Returns a populated `ChatGptAuth`
    pub async fn authorize_via_browser() -> Result<Self> {
        let pkce = generate_pkce();

        // Bind the fixed callback port (OpenAI only accepts localhost:1455)
        let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{}", CALLBACK_PORT))
            .await
            .map_err(|e| AlephError::network(format!(
                "Failed to bind localhost:{} — is another login in progress? Error: {}",
                CALLBACK_PORT, e
            )))?;
        let port = CALLBACK_PORT;

        let state = generate_state();
        let authorize_url = Self::build_authorize_url(port, &state, &pkce);

        info!(port, "Starting OAuth callback server on localhost");

        // Channel to send the authorization code from the callback handler
        let (tx, rx) = oneshot::channel::<std::result::Result<String, String>>();
        let tx = std::sync::Arc::new(std::sync::Mutex::new(Some(tx)));

        let expected_state = state.clone();

        // Build the axum callback router
        let callback_path = CALLBACK_PATH;
        let app = axum::Router::new().route(
            callback_path,
            axum::routing::get(move |query: axum::extract::Query<CallbackParams>| {
                let tx = tx.lock().unwrap_or_else(|e| e.into_inner()).take();
                async move {
                    if let Some(ref err) = query.error {
                        let desc = query
                            .error_description
                            .as_deref()
                            .unwrap_or("Unknown error");
                        error!(error = err, description = desc, "OAuth callback error");
                        if let Some(tx) = tx {
                            let _ = tx.send(Err(format!("{}: {}", err, desc)));
                        }
                        return axum::response::Html(
                            "<html><body><h1>Authentication Failed</h1><p>You can close this window.</p></body></html>"
                                .to_string(),
                        );
                    }

                    match &query.state {
                        None => {
                            error!("OAuth callback missing state parameter");
                            if let Some(tx) = tx {
                                let _ = tx.send(Err("Missing state parameter".to_string()));
                            }
                            return axum::response::Html(
                                "<html><body><h1>Authentication Failed</h1><p>Missing state parameter. You can close this window.</p></body></html>"
                                    .to_string(),
                            );
                        }
                        Some(received_state) if received_state != &expected_state => {
                            error!("OAuth state mismatch");
                            if let Some(tx) = tx {
                                let _ = tx.send(Err("State parameter mismatch".to_string()));
                            }
                            return axum::response::Html(
                                "<html><body><h1>Authentication Failed</h1><p>State mismatch. You can close this window.</p></body></html>"
                                    .to_string(),
                            );
                        }
                        _ => {} // State matches, proceed
                    }

                    match &query.code {
                        Some(code) => {
                            debug!("Received OAuth authorization code");
                            if let Some(tx) = tx {
                                let _ = tx.send(Ok(code.clone()));
                            }
                            axum::response::Html(
                                "<html><body><h1>Authentication Successful</h1><p>You can close this window and return to Aleph.</p></body></html>"
                                    .to_string(),
                            )
                        }
                        None => {
                            error!("No authorization code in callback");
                            if let Some(tx) = tx {
                                let _ = tx.send(Err("No authorization code received".to_string()));
                            }
                            axum::response::Html(
                                "<html><body><h1>Authentication Failed</h1><p>No code received. You can close this window.</p></body></html>"
                                    .to_string(),
                            )
                        }
                    }
                }
            }),
        );

        // Spawn the server
        let server_handle = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .map_err(|e| AlephError::network(format!("OAuth callback server error: {}", e)))
        });

        // Open the browser
        info!("Opening browser for ChatGPT authentication...");
        if let Err(e) = open::that(&authorize_url) {
            error!(?e, "Failed to open browser");
            return Err(AlephError::provider(format!(
                "Failed to open browser for authentication: {}. Please open this URL manually: {}",
                e, authorize_url
            )));
        }

        // Wait for the callback with timeout
        let code = match tokio::time::timeout(CALLBACK_TIMEOUT, rx).await {
            Ok(Ok(Ok(code))) => code,
            Ok(Ok(Err(err))) => {
                server_handle.abort();
                return Err(AlephError::authentication("chatgpt", &err));
            }
            Ok(Err(_)) => {
                server_handle.abort();
                return Err(AlephError::authentication(
                    "chatgpt",
                    "OAuth callback channel closed unexpectedly",
                ));
            }
            Err(_) => {
                server_handle.abort();
                return Err(AlephError::authentication(
                    "chatgpt",
                    "OAuth authentication timed out (5 minutes). Please try again.",
                ));
            }
        };

        // Abort the server now that we have the code
        server_handle.abort();

        debug!("Exchanging authorization code for tokens (with PKCE verifier)");
        Self::exchange_code_for_token(&code, port, &pkce).await
    }

    /// Exchange an authorization code + PKCE verifier for access and refresh tokens
    async fn exchange_code_for_token(code: &str, port: u16, pkce: &PkceCodes) -> Result<Self> {
        let client = Client::new();
        let redirect_uri = format!("http://localhost:{}{}", port, CALLBACK_PATH);
        let token_url = format!("{}/oauth/token", ISSUER);

        // Use form-encoded body (matching Codex CLI)
        let enc = |s: &str| utf8_percent_encode(s, QUERY_ENCODE_SET).to_string();
        let body = format!(
            "grant_type=authorization_code&code={}&redirect_uri={}&client_id={}&code_verifier={}",
            enc(code),
            enc(&redirect_uri),
            enc(OPENAI_CLIENT_ID),
            enc(&pkce.code_verifier),
        );

        let response = client
            .post(&token_url)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await
            .map_err(|e| AlephError::network(format!("Token exchange request failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(AlephError::authentication(
                "chatgpt",
                &format!("Token exchange failed ({}): {}", status, body),
            ));
        }

        let token_resp: TokenResponse = response.json().await.map_err(|e| {
            AlephError::provider(format!("Failed to parse token response: {}", e))
        })?;

        let expires_in = token_resp.expires_in.unwrap_or(3600);
        let expires_at = SystemTime::now() + Duration::from_secs(expires_in);
        let session_id = uuid::Uuid::new_v4().to_string();

        info!(
            expires_in_secs = expires_in,
            "ChatGPT authentication successful"
        );

        Ok(ChatGptAuth {
            access_token: token_resp.access_token,
            refresh_token: token_resp.refresh_token,
            expires_at,
            session_id,
        })
    }

    /// Refresh the access token using the refresh_token grant
    pub async fn refresh(&mut self) -> Result<()> {
        let refresh_token = self.refresh_token.as_ref().ok_or_else(|| {
            AlephError::authentication(
                "chatgpt",
                "No refresh token available. Please re-login.",
            )
        })?;

        let client = Client::new();
        let token_url = format!("{}/oauth/token", ISSUER);

        let enc = |s: &str| utf8_percent_encode(s, QUERY_ENCODE_SET).to_string();
        let body = format!(
            "grant_type=refresh_token&client_id={}&refresh_token={}",
            enc(OPENAI_CLIENT_ID),
            enc(refresh_token),
        );

        let response = client
            .post(&token_url)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await
            .map_err(|e| AlephError::network(format!("Token refresh request failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let body_text = response.text().await.unwrap_or_default();
            return Err(AlephError::authentication(
                "chatgpt",
                &format!(
                    "Token refresh failed ({}): {}. Re-authentication may be required.",
                    status, body_text
                ),
            ));
        }

        let token_resp: TokenResponse = response.json().await.map_err(|e| {
            AlephError::provider(format!("Failed to parse refresh response: {}", e))
        })?;

        self.access_token = token_resp.access_token;
        if let Some(new_refresh) = token_resp.refresh_token {
            self.refresh_token = Some(new_refresh);
        }
        let expires_in = token_resp.expires_in.unwrap_or(3600);
        self.expires_at = SystemTime::now() + Duration::from_secs(expires_in);

        debug!("ChatGPT token refreshed successfully");
        Ok(())
    }

    /// Ensure the token is valid, refreshing if necessary.
    /// Returns the current access token.
    pub async fn ensure_valid(&mut self) -> Result<&str> {
        if self.is_expired() {
            info!("Access token expired, attempting refresh...");
            self.refresh().await?;
        }
        Ok(&self.access_token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auth_state_token_expired() {
        let auth = ChatGptAuth {
            access_token: "test_token".to_string(),
            refresh_token: None,
            expires_at: SystemTime::UNIX_EPOCH,
            session_id: "test_session".to_string(),
        };
        assert!(auth.is_expired());
    }

    #[test]
    fn test_auth_state_token_valid() {
        let auth = ChatGptAuth {
            access_token: "test_token".to_string(),
            refresh_token: None,
            expires_at: SystemTime::now() + Duration::from_secs(3600),
            session_id: "test_session".to_string(),
        };
        assert!(!auth.is_expired());
    }

    #[test]
    fn test_build_authorize_url() {
        let pkce = PkceCodes {
            code_verifier: "test_verifier".to_string(),
            code_challenge: "test_challenge".to_string(),
        };
        let url = ChatGptAuth::build_authorize_url(12345, "random_state", &pkce);
        assert!(url.contains("auth.openai.com/oauth/authorize"));
        assert!(url.contains("12345"));
        assert!(url.contains("random_state"));
        assert!(url.contains("code_challenge"));
        assert!(url.contains("S256"));
        assert!(url.contains("app_EMoamEEZ73f0CkXaXp7hrann"));
        assert!(url.contains("offline_access"));
    }

    #[test]
    fn test_pkce_generation() {
        let pkce = generate_pkce();
        assert!(!pkce.code_verifier.is_empty());
        assert!(!pkce.code_challenge.is_empty());
        // Verify code_challenge is SHA256 of code_verifier
        let mut hasher = Sha256::new();
        hasher.update(pkce.code_verifier.as_bytes());
        let expected = URL_SAFE_NO_PAD.encode(hasher.finalize());
        assert_eq!(pkce.code_challenge, expected);
    }

    #[test]
    fn test_callback_path() {
        assert_eq!(CALLBACK_PATH, "/auth/callback");
    }
}
