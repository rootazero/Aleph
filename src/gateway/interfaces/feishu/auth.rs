use crate::sync_primitives::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

use super::envelope::{read_checked, Envelope};
use super::types::TokenResponse;

const TOKEN_REFRESH_MARGIN_SECS: u64 = 300;
const DEFAULT_TOKEN_EXPIRY_SECS: u64 = 7200;

pub(super) struct TokenState {
    pub(super) access_token: String,
    pub(super) expires_at: Instant,
}

impl TokenState {
    fn needs_refresh(&self) -> bool {
        Instant::now() >= self.expires_at
    }
}

/// Manages Feishu app access token lifecycle.
pub struct TokenManager {
    app_id: String,
    app_secret: String,
    base_url: String,
    http: reqwest::Client,
    token: Arc<RwLock<TokenState>>,
}

impl TokenManager {
    pub fn new(app_id: &str, app_secret: &str, base_url: &str, http: reqwest::Client) -> Self {
        Self {
            app_id: app_id.to_string(),
            app_secret: app_secret.to_string(),
            base_url: base_url.to_string(),
            http,
            token: Arc::new(RwLock::new(TokenState {
                access_token: String::new(),
                expires_at: Instant::now(),
            })),
        }
    }

    /// Force-refresh the access token from Feishu API.
    pub async fn refresh_token(&self) -> Result<(), String> {
        let url = format!(
            "{}/open-apis/auth/v3/app_access_token/internal",
            self.base_url
        );
        let body = serde_json::json!({
            "app_id": self.app_id,
            "app_secret": self.app_secret,
        });

        let resp = self
            .http
            .post(&url)
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Token request failed: {e}"))?;

        let token_resp: TokenResponse =
            read_checked(resp, "Token").await?;

        let access_token = token_resp
            .app_access_token
            .ok_or_else(|| "No access_token in response".to_string())?;
        let expire = token_resp.expire.unwrap_or(DEFAULT_TOKEN_EXPIRY_SECS);

        let expires_at =
            Instant::now() + Duration::from_secs(expire.saturating_sub(TOKEN_REFRESH_MARGIN_SECS));

        let mut state = self.token.write().await;
        state.access_token = access_token;
        state.expires_at = expires_at;

        tracing::debug!("Feishu token refreshed, expires in {}s", expire);
        Ok(())
    }

    /// Get a valid access token, refreshing if expired.
    pub async fn get_token(&self) -> Result<String, String> {
        {
            let state = self.token.read().await;
            if !state.needs_refresh() {
                return Ok(state.access_token.clone());
            }
        }
        self.refresh_token().await?;
        let state = self.token.read().await;
        Ok(state.access_token.clone())
    }

    /// Spawn a background task that refreshes the token before expiry.
    pub fn spawn_token_refresh(&self, shutdown: tokio::sync::watch::Receiver<bool>) {
        let app_id = self.app_id.clone();
        let app_secret = self.app_secret.clone();
        let base_url = self.base_url.clone();
        let http = self.http.clone();
        let token = self.token.clone();

        tokio::spawn(async move {
            let mut shutdown = shutdown;
            loop {
                let sleep_duration = {
                    let state = token.read().await;
                    let now = Instant::now();
                    if state.expires_at > now {
                        state.expires_at.duration_since(now)
                    } else {
                        Duration::from_secs(60)
                    }
                };

                tokio::select! {
                    _ = tokio::time::sleep(sleep_duration) => {}
                    _ = shutdown.changed() => {
                        tracing::debug!("Token refresh task shutting down");
                        return;
                    }
                }

                let url = format!("{base_url}/open-apis/auth/v3/app_access_token/internal");
                let body = serde_json::json!({
                    "app_id": app_id,
                    "app_secret": app_secret,
                });

                match http
                    .post(&url)
                    .header("Content-Type", "application/json; charset=utf-8")
                    .json(&body)
                    .send()
                    .await
                {
                    Ok(resp) => match Envelope::read(resp, "Token refresh")
                        .await
                        .and_then(|env| env.parse::<TokenResponse>("Token refresh"))
                    {
                        Ok(tr) if tr.code == 0 => {
                            if let Some(at) = tr.app_access_token {
                                let expire = tr.expire.unwrap_or(DEFAULT_TOKEN_EXPIRY_SECS);
                                let mut state = token.write().await;
                                state.access_token = at;
                                state.expires_at = Instant::now()
                                    + Duration::from_secs(
                                        expire.saturating_sub(TOKEN_REFRESH_MARGIN_SECS),
                                    );
                                tracing::debug!("Feishu token refreshed (background)");
                            }
                        }
                        Ok(tr) => {
                            tracing::warn!("Token refresh failed: code={}, msg={}", tr.code, tr.msg)
                        }
                        // Already names the status and quotes the body: a 403
                        // from an expired app secret and a 502 from a proxy used
                        // to print the same `error decoding response body`, and
                        // this is a background task nobody is watching.
                        Err(e) => tracing::warn!("Token refresh failed: {e}"),
                    },
                    Err(e) => tracing::warn!("Token refresh request error: {e}"),
                }
            }
        });
    }
}
