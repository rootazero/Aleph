//! ChatGPT OAuth authentication
//!
//! Implements the browser-based OAuth flow for ChatGPT subscription accounts.
//! Opens system browser for user login, captures callback via localhost server.

use crate::error::{AlephError, Result};
use reqwest::Client;
use serde::Deserialize;
use std::time::{Duration, SystemTime};
use tokio::sync::oneshot;
use tracing::{debug, error, info};

/// OpenAI OAuth client ID (web client)
const OPENAI_CLIENT_ID: &str = "DRivsnm2Mu42T3KOpqdtwB3NYviHYzwD";

/// OpenAI auth endpoints
const AUTHORIZE_URL: &str = "https://auth0.openai.com/authorize";
const TOKEN_URL: &str = "https://auth0.openai.com/oauth/token";
const SESSION_URL: &str = "https://chatgpt.com/api/auth/session";

/// OAuth callback timeout (5 minutes)
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(300);

/// Token response from the OAuth token endpoint
#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
}

/// Session response from the ChatGPT session endpoint
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct SessionResponse {
    #[serde(default, rename = "accessToken")]
    access_token: Option<String>,
    #[serde(default)]
    expires: Option<String>,
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

    /// Build the OAuth authorization URL
    pub fn build_authorize_url(port: u16, state: &str) -> String {
        // redirect_uri is always http://localhost:{port}/callback — percent-encode manually
        let redirect_uri = format!("http%3A%2F%2Flocalhost%3A{}%2Fcallback", port);
        // state is a UUID (hex + hyphens) so it's already URL-safe
        format!(
            "{}?client_id={}&redirect_uri={}&response_type=code&scope=openid%20profile%20email&state={}&audience=https%3A%2F%2Fapi.openai.com%2Fv1",
            AUTHORIZE_URL,
            OPENAI_CLIENT_ID,
            redirect_uri,
            state,
        )
    }

    /// Run the full OAuth browser authorization flow.
    ///
    /// 1. Binds a random localhost port
    /// 2. Opens the system browser to the OAuth authorize URL
    /// 3. Waits for the callback with an authorization code (5 min timeout)
    /// 4. Exchanges the code for tokens
    /// 5. Returns a populated `ChatGptAuth`
    pub async fn authorize_via_browser() -> Result<Self> {
        // Bind a random available port
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| AlephError::network(format!("Failed to bind localhost listener: {}", e)))?;
        let port = listener
            .local_addr()
            .map_err(|e| AlephError::network(format!("Failed to get local address: {}", e)))?
            .port();

        let state = uuid::Uuid::new_v4().to_string();
        let authorize_url = Self::build_authorize_url(port, &state);

        info!(port, "Starting OAuth callback server on localhost");

        // Channel to send the authorization code from the callback handler
        let (tx, rx) = oneshot::channel::<std::result::Result<String, String>>();
        let tx = std::sync::Arc::new(std::sync::Mutex::new(Some(tx)));

        let expected_state = state.clone();

        // Build the axum callback router
        let app = axum::Router::new().route(
            "/callback",
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

                    if let Some(ref received_state) = query.state {
                        if received_state != &expected_state {
                            error!("OAuth state mismatch");
                            if let Some(tx) = tx {
                                let _ = tx.send(Err("State parameter mismatch".to_string()));
                            }
                            return axum::response::Html(
                                "<html><body><h1>Authentication Failed</h1><p>State mismatch. You can close this window.</p></body></html>"
                                    .to_string(),
                            );
                        }
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

        debug!("Exchanging authorization code for tokens");
        Self::exchange_code_for_token(&code, port).await
    }

    /// Exchange an authorization code for access and refresh tokens
    async fn exchange_code_for_token(code: &str, port: u16) -> Result<Self> {
        let client = Client::new();
        let redirect_uri = format!("http://localhost:{}/callback", port);

        let response = client
            .post(TOKEN_URL)
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({
                "grant_type": "authorization_code",
                "client_id": OPENAI_CLIENT_ID,
                "code": code,
                "redirect_uri": redirect_uri,
            }))
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

    /// Refresh the access token using the session endpoint
    pub async fn refresh(&mut self) -> Result<()> {
        let client = Client::new();

        let response = client
            .get(SESSION_URL)
            .header("Authorization", format!("Bearer {}", self.access_token))
            .header("Cookie", format!("__Secure-next-auth.session-token={}", self.access_token))
            .send()
            .await
            .map_err(|e| AlephError::network(format!("Session refresh request failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(AlephError::authentication(
                "chatgpt",
                &format!(
                    "Session refresh failed ({}): {}. Re-authentication may be required.",
                    status, body
                ),
            ));
        }

        let session: SessionResponse = response.json().await.map_err(|e| {
            AlephError::provider(format!("Failed to parse session response: {}", e))
        })?;

        if let Some(new_token) = session.access_token {
            self.access_token = new_token;
            // Session tokens from ChatGPT typically last ~30 days,
            // but we conservatively set a shorter window
            self.expires_at = SystemTime::now() + Duration::from_secs(3600);
            debug!("ChatGPT session refreshed successfully");
            Ok(())
        } else {
            Err(AlephError::authentication(
                "chatgpt",
                "Session refresh response did not contain a new access token",
            ))
        }
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
        let url = ChatGptAuth::build_authorize_url(12345, "random_state");
        assert!(url.contains("auth0.openai.com") || url.contains("auth.openai.com"));
        assert!(url.contains("12345"));
        assert!(url.contains("random_state"));
    }
}
