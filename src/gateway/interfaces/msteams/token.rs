use std::time::{Duration, Instant};

use async_trait::async_trait;
use reqwest::Client;
use tracing::debug;

use crate::gateway::channel::ChannelError;
use crate::gateway::interfaces::msteams::auth::{Certificate, FederatedCredential, JwtAssertionGenerator, ManagedIdentityTokenProvider};
use super::graph::GraphTokenSource;

const GRAPH_TOKEN_REFRESH_THRESHOLD: f64 = 0.8;

struct CachedToken {
    token: String,
    refresh_at: Instant,
}

impl CachedToken {
    fn needs_refresh(&self) -> bool {
        Instant::now() >= self.refresh_at
    }
}

enum AuthMethod {
    ClientSecret {
        app_id: String,
        app_password: String,
    },
    Federated {
        credential: FederatedCredential,
        generator: Option<JwtAssertionGenerator>,
        mi_provider: Option<ManagedIdentityTokenProvider>,
    },
}

pub struct GraphTokenManager {
    app_id: String,
    tenant_id: String,
    auth_method: AuthMethod,
    cached: tokio::sync::RwLock<Option<CachedToken>>,
    http: Client,
}

impl GraphTokenManager {
    pub fn new(app_id: String, tenant_id: String, app_password: String) -> Self {
        Self {
            app_id: app_id.clone(),
            tenant_id,
            auth_method: AuthMethod::ClientSecret {
                app_id,
                app_password,
            },
            cached: tokio::sync::RwLock::new(None),
            http: Client::new(),
        }
    }

    pub fn with_federated(app_id: String, tenant_id: String, credential: FederatedCredential) -> Self {
        let mi_provider = Some(ManagedIdentityTokenProvider::new(credential.managed_identity_client_id.clone()));

        Self {
            app_id: app_id.clone(),
            tenant_id,
            auth_method: AuthMethod::Federated {
                credential,
                generator: None,
                mi_provider,
            },
            cached: tokio::sync::RwLock::new(None),
            http: Client::new(),
        }
    }

    pub async fn get_token(&self) -> Result<String, ChannelError> {
        {
            let guard = self.cached.read().await;
            if let Some(ref t) = *guard {
                if !t.needs_refresh() {
                    return Ok(t.token.clone());
                }
            }
        }

        let mut guard = self.cached.write().await;
        if let Some(ref t) = *guard {
            if !t.needs_refresh() {
                return Ok(t.token.clone());
            }
        }

        let token = self.fetch_token().await?;
        let token_str = token.token.clone();
        *guard = Some(token);
        Ok(token_str)
    }

    async fn fetch_token(&self) -> Result<CachedToken, ChannelError> {
        match &self.auth_method {
            AuthMethod::ClientSecret {
                app_id,
                app_password,
            } => self.fetch_token_via_secret(app_id, app_password).await,
            AuthMethod::Federated {
                credential,
                generator,
                mi_provider,
            } => {
                let gen = self.get_or_init_generator(generator, credential).await?;
                self.fetch_token_via_federated(&gen, mi_provider.as_ref()).await
            }
        }
    }

    async fn get_or_init_generator(
        &self,
        generator: &Option<JwtAssertionGenerator>,
        credential: &FederatedCredential,
    ) -> Result<JwtAssertionGenerator, ChannelError> {
        if let Some(ref g) = generator {
            return Ok(g.clone());
        }

        let cert = Certificate::load(credential.certificate_path.clone())
            .await
            .map_err(|e| ChannelError::AuthFailed(format!("Failed to load certificate: {}", e)))?;

        Ok(JwtAssertionGenerator::new(
            cert,
            self.app_id.clone(),
            credential.authority_url.clone(),
        ))
    }

    async fn fetch_token_via_secret(
        &self,
        app_id: &str,
        app_password: &str,
    ) -> Result<CachedToken, ChannelError> {
        let url = format!(
            "https://login.microsoftonline.com/{}/oauth2/v2.0/token",
            self.tenant_id
        );

        let params = [
            ("grant_type", "client_credentials"),
            ("client_id", app_id),
            ("client_secret", app_password),
            ("scope", "https://graph.microsoft.com/.default"),
        ];

        let resp = self
            .http
            .post(&url)
            .form(&params)
            .send()
            .await
            .map_err(|e| ChannelError::AuthFailed(format!("Graph token request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(ChannelError::AuthFailed(format!(
                "Graph token endpoint returned {status}: {body}"
            )));
        }

        #[derive(serde::Deserialize)]
        struct TokenResponse {
            access_token: String,
            expires_in: u64,
        }

        impl HasExpiresIn for TokenResponse {
            fn expires_in(&self) -> u64 {
                self.expires_in
            }
            fn access_token(&self) -> String {
                self.access_token.clone()
            }
        }

        let tr: TokenResponse = resp.json().await.map_err(|e| {
            ChannelError::Internal(format!("Failed to parse Graph token response: {e}"))
        })?;

        self.token_response_to_cached(tr)
    }

    async fn fetch_token_via_federated(
        &self,
        generator: &JwtAssertionGenerator,
        _mi_provider: Option<&ManagedIdentityTokenProvider>,
    ) -> Result<CachedToken, ChannelError> {
        let assertion = generator
            .exchange_for_token(&self.tenant_id, "https://graph.microsoft.com/.default")
            .await
            .map_err(|e| ChannelError::AuthFailed(format!("Federated token exchange failed: {e}")))?;

        let lifetime = Duration::from_secs(3600);
        let refresh_after = lifetime.mul_f64(GRAPH_TOKEN_REFRESH_THRESHOLD);
        let now = Instant::now();

        debug!(refresh_after_secs = refresh_after.as_secs(), "Fetched new Graph API token via federated auth");

        Ok(CachedToken {
            token: assertion,
            refresh_at: now + refresh_after,
        })
    }

    fn token_response_to_cached(&self, tr: impl HasExpiresIn) -> Result<CachedToken, ChannelError> {
        let lifetime = Duration::from_secs(tr.expires_in());
        let refresh_after = lifetime.mul_f64(GRAPH_TOKEN_REFRESH_THRESHOLD);
        let now = Instant::now();

        debug!(
            expires_in = tr.expires_in(),
            refresh_after_secs = refresh_after.as_secs(),
            "Fetched new Graph API token"
        );

        Ok(CachedToken {
            token: tr.access_token(),
            refresh_at: now + refresh_after,
        })
    }

    pub async fn refresh_if_needed(&self) -> Result<(), ChannelError> {
        let needs_refresh = {
            let guard = self.cached.read().await;
            guard.as_ref().map(|t| t.needs_refresh()).unwrap_or(true)
        };

        if needs_refresh {
            let mut guard = self.cached.write().await;
            *guard = None;
        }
        Ok(())
    }
}

#[async_trait]
impl GraphTokenSource for GraphTokenManager {
    async fn get_token(&self) -> Result<String, ChannelError> {
        GraphTokenManager::get_token(self).await
    }
}

trait HasExpiresIn {
    fn expires_in(&self) -> u64;
    fn access_token(&self) -> String;
}

impl HasExpiresIn for serde_json::Value {
    fn expires_in(&self) -> u64 {
        self.get("expires_in")
            .and_then(|v| v.as_u64())
            .unwrap_or(3600)
    }

    fn access_token(&self) -> String {
        self.get("access_token")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_token_cache_fetches_when_empty() {
        let manager = GraphTokenManager::new(
            "app-id".to_string(),
            "tenant".to_string(),
            "secret".to_string(),
        );
        assert!(manager.cached.read().await.is_none());
    }
}
