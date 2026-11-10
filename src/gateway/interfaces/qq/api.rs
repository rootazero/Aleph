use crate::sync_primitives::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

use crate::gateway::interfaces::qq::error::QQError;
use crate::gateway::interfaces::qq::types::SendMessagePayload;

const TOKEN_URL: &str = "https://bots.qq.com/app/getAppAccessToken";
const API_BASE: &str = "https://api.sgroup.qq.com";

#[derive(Debug, Clone)]
struct CachedToken {
    token: String,
    expires_at: Instant,
}

pub struct TokenManager {
    app_id: String,
    client_secret: String,
    cache: Mutex<Option<CachedToken>>,
    fetch_lock: Mutex<()>,
}

impl TokenManager {
    #[must_use]
    pub fn new(app_id: String, client_secret: String) -> Self {
        Self {
            app_id,
            client_secret,
            cache: Mutex::new(None),
            fetch_lock: Mutex::new(()),
        }
    }

    pub async fn get_token(&self, client: &reqwest::Client) -> Result<String, QQError> {
        {
            let guard = self.cache.lock().await;
            if let Some(ref cached) = *guard {
                let remaining = cached.expires_at.saturating_duration_since(Instant::now());
                let refresh_ahead = Duration::from_secs(300).min(remaining / 3);
                if remaining > refresh_ahead {
                    return Ok(cached.token.clone());
                }
            }
        }

        // Keep the lock through fetch + cache update; every waiter rechecks the cache.
        let _fetch = self.fetch_lock.lock().await;

        {
            let guard = self.cache.lock().await;
            if let Some(ref cached) = *guard {
                let remaining = cached.expires_at.saturating_duration_since(Instant::now());
                let refresh_ahead = Duration::from_secs(300).min(remaining / 3);
                if remaining > refresh_ahead {
                    return Ok(cached.token.clone());
                }
            }
        }

        self.fetch_token(client).await
    }

    async fn fetch_token(&self, client: &reqwest::Client) -> Result<String, QQError> {
        let body = serde_json::json!({
            "appId": self.app_id,
            "clientSecret": self.client_secret,
        });

        let resp = client
            .post(TOKEN_URL)
            .json(&body)
            .send()
            .await
            .map_err(|e| QQError::AuthFailed(e.to_string()))?;

        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(QQError::HttpError {
                status: status.as_u16(),
                body: text,
            });
        }

        let data: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| QQError::AuthFailed(e.to_string()))?;

        let token = data["access_token"]
            .as_str()
            .ok_or_else(|| QQError::AuthFailed("Missing access_token".into()))?
            .to_string();

        let expires_in = data["expires_in"].as_u64().unwrap_or(7200);
        let expires_at = Instant::now() + Duration::from_secs(expires_in);

        let mut cache = self.cache.lock().await;
        *cache = Some(CachedToken {
            token: token.clone(),
            expires_at,
        });

        Ok(token)
    }
}

pub struct QQApiClient {
    http: reqwest::Client,
    token_manager: Arc<TokenManager>,
}

impl QQApiClient {
    #[must_use]
    pub fn new(app_id: String, client_secret: String) -> Self {
        Self {
            http: reqwest::Client::new(),
            token_manager: Arc::new(TokenManager::new(app_id, client_secret)),
        }
    }

    pub async fn get_access_token(&self) -> Result<String, QQError> {
        self.token_manager.get_token(&self.http).await
    }

    pub async fn send_c2c_message(
        &self,
        openid: &str,
        payload: SendMessagePayload,
    ) -> Result<String, QQError> {
        let token = self.token_manager.get_token(&self.http).await?;
        let url = format!("{API_BASE}/v2/users/{openid}/messages");
        self.post_message(&url, &token, payload).await
    }

    pub async fn send_group_message(
        &self,
        group_openid: &str,
        payload: SendMessagePayload,
    ) -> Result<String, QQError> {
        let token = self.token_manager.get_token(&self.http).await?;
        let url = format!("{API_BASE}/v2/groups/{group_openid}/messages");
        self.post_message(&url, &token, payload).await
    }

    async fn post_message(
        &self,
        url: &str,
        token: &str,
        payload: SendMessagePayload,
    ) -> Result<String, QQError> {
        let resp = self
            .http
            .post(url)
            .header("Authorization", format!("QQBot {token}"))
            .json(&payload)
            .send()
            .await
            .map_err(|e| QQError::SendFailed(e.to_string()))?;

        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();

        let code = u32::from(status.as_u16());
        if code == 429 || code == 304023 || code == 304024 {
            return Err(QQError::RateLimited {
                retry_after_secs: 5,
            });
        }

        if !status.is_success() {
            return Err(QQError::HttpError {
                status: status.as_u16(),
                body,
            });
        }

        Ok(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_manager_creation() {
        let _tm = TokenManager::new("app".into(), "secret".into());
    }

    #[tokio::test]
    async fn concurrent_token_waiters_recheck_cache_even_after_cancellation() {
        let manager = TokenManager::new("app".into(), "secret".into());
        let client = reqwest::Client::new();
        let fetch = manager.fetch_lock.lock().await;
        let mut waiters: Vec<_> = (0..4)
            .map(|_| Box::pin(manager.get_token(&client)))
            .collect();
        for waiter in &mut waiters {
            assert!(futures_util::poll!(waiter.as_mut()).is_pending());
        }
        // Cancelling a waiter cannot consume another caller's coordination slot.
        drop(waiters.remove(0));
        *manager.cache.lock().await = Some(CachedToken {
            token: "refreshed".into(),
            expires_at: Instant::now() + Duration::from_secs(3600),
        });
        drop(fetch);

        let results = tokio::time::timeout(
            Duration::from_secs(1),
            futures_util::future::join_all(waiters),
        )
        .await
        .expect("all token waiters must complete");
        assert_eq!(results.len(), 3);
        for result in results {
            assert_eq!(result.unwrap(), "refreshed");
        }
    }

    #[tokio::test]
    async fn gateway_identify_reads_the_shared_refreshed_token() {
        use super::super::gateway::{identify_payload, GatewayContext};
        use crate::gateway::channel::ChannelId;

        let api = Arc::new(QQApiClient::new("app".into(), "secret".into()));
        let (event_tx, _event_rx) = tokio::sync::mpsc::channel(4);
        let (_shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let ctx = GatewayContext {
            channel_id: ChannelId::new("qq"),
            api: api.clone(),
            event_tx,
            shutdown_rx,
        };
        *api.token_manager.cache.lock().await = Some(CachedToken {
            token: "startup".into(),
            expires_at: Instant::now() + Duration::from_secs(3600),
        });
        assert_eq!(
            identify_payload(&ctx.api).await.unwrap()["d"]["token"],
            "QQBot startup"
        );

        let fetch = api.token_manager.fetch_lock.lock().await;
        api.token_manager
            .cache
            .lock()
            .await
            .as_mut()
            .unwrap()
            .expires_at = Instant::now();
        let mut reconnect = Box::pin(identify_payload(&ctx.api));
        assert!(futures_util::poll!(reconnect.as_mut()).is_pending());
        // The same manager is refreshed by REST; the gateway owns no token snapshot.
        *api.token_manager.cache.lock().await = Some(CachedToken {
            token: "renewed".into(),
            expires_at: Instant::now() + Duration::from_secs(3600),
        });
        drop(fetch);
        let identify = reconnect.await.unwrap();
        assert_eq!(identify["op"], 2);
        assert_eq!(identify["d"]["token"], "QQBot renewed");
    }
}
