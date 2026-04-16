use crate::gateway::channel::ChannelError;
use crate::gateway::interfaces::matrix::config::MatrixConfig;
use matrix_sdk::Client;
use std::path::Path;

pub struct MatrixSdkClient {
    inner: Client,
    user_id: String,
}

impl MatrixSdkClient {
    pub async fn connect(config: &MatrixConfig) -> Result<Self, ChannelError> {
        let whoami_url = format!("{}/_matrix/client/v3/account/whoami", config.homeserver_url);
        let http = reqwest::Client::new();
        let resp = http
            .get(&whoami_url)
            .bearer_auth(&config.access_token)
            .send()
            .await
            .map_err(|e| ChannelError::AuthFailed(format!("whoami request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(ChannelError::AuthFailed(format!(
                "Matrix auth failed ({status}): {body}"
            )));
        }

        let whoami: serde_json::Value = resp.json().await.map_err(|e| {
            ChannelError::AuthFailed(format!("whoami parse failed: {e}"))
        })?;
        let user_id = whoami["user_id"]
            .as_str()
            .ok_or_else(|| ChannelError::AuthFailed("whoami missing user_id".to_string()))?
            .to_string();

        let state_path = config.state_store_path();
        if let Some(parent) = Path::new(&state_path).parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| ChannelError::Internal(format!("Failed to create state dir: {e}")))?;
        }

        let homeserver = url::Url::parse(&config.homeserver_url)
            .map_err(|e| ChannelError::ConfigError(format!("Invalid homeserver URL: {e}")))?;

        let client = Client::builder()
            .homeserver_url(homeserver)
            .sqlite_store(&state_path, None)
            .build()
            .await
            .map_err(|e| {
                ChannelError::Internal(format!("Failed to build Matrix client: {e}"))
            })?;

        let device_id = whoami["device_id"]
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        let parsed_user_id = user_id
            .parse()
            .map_err(|e| ChannelError::AuthFailed(format!("Invalid user_id: {e}")))?;

        let session = matrix_sdk::authentication::matrix::MatrixSession {
            meta: matrix_sdk::SessionMeta {
                user_id: parsed_user_id,
                device_id: device_id.into(),
            },
            tokens: matrix_sdk::SessionTokens {
                access_token: config.access_token.clone(),
                refresh_token: None,
            },
        };

        client
            .restore_session(matrix_sdk::AuthSession::Matrix(session))
            .await
            .map_err(|e| ChannelError::AuthFailed(format!("Session restore failed: {e}")))?;

        Ok(Self { inner: client, user_id })
    }

    pub fn user_id(&self) -> &str {
        &self.user_id
    }

    pub fn inner(&self) -> &Client {
        &self.inner
    }
}
