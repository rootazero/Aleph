//! Bot Framework REST API v3 Client

use crate::sync_primitives::Arc;
use reqwest::Client;
use tracing::{debug, warn};

use crate::gateway::channel::ChannelError;
use super::auth::TokenCache;
use super::types::{Activity, ActivityResponse};

pub struct BotFrameworkClient {
    http: Client,
    token_cache: Arc<TokenCache>,
}

impl BotFrameworkClient {
    pub fn new(token_cache: Arc<TokenCache>) -> Self {
        Self {
            http: Client::new(),
            token_cache,
        }
    }

    // Note: Teams conversation IDs contain ':' and '@' (e.g., "19:conv@thread.v2").
    // These are valid URL path characters per RFC 3986 §3.3 (pchar includes ':' and '@').
    // Bot Framework REST API expects raw IDs, not percent-encoded.

    /// POST {serviceUrl}/v3/conversations/{conversationId}/activities
    pub async fn send_activity(
        &self,
        service_url: &str,
        conversation_id: &str,
        activity: &Activity,
    ) -> Result<ActivityResponse, ChannelError> {
        let url = format!(
            "{}v3/conversations/{}/activities",
            ensure_trailing_slash(service_url),
            conversation_id
        );
        self.post_activity(&url, activity).await
    }

    /// POST {serviceUrl}/v3/conversations/{conversationId}/activities/{activityId}
    pub async fn reply_to_activity(
        &self,
        service_url: &str,
        conversation_id: &str,
        activity_id: &str,
        activity: &Activity,
    ) -> Result<ActivityResponse, ChannelError> {
        let url = format!(
            "{}v3/conversations/{}/activities/{}",
            ensure_trailing_slash(service_url),
            conversation_id,
            activity_id
        );
        self.post_activity(&url, activity).await
    }

    /// PUT {serviceUrl}/v3/conversations/{conversationId}/activities/{activityId}
    pub async fn update_activity(
        &self,
        service_url: &str,
        conversation_id: &str,
        activity_id: &str,
        activity: &Activity,
    ) -> Result<(), ChannelError> {
        let url = format!(
            "{}v3/conversations/{}/activities/{}",
            ensure_trailing_slash(service_url),
            conversation_id,
            activity_id
        );
        let token = self.token_cache.get_token().await?;
        let resp = self
            .http
            .put(&url)
            .bearer_auth(&token)
            .json(activity)
            .send()
            .await
            .map_err(|e| ChannelError::SendFailed(format!("HTTP PUT failed: {}", e)))?;

        let status = resp.status();
        if status.is_success() {
            debug!(url = %url, "Activity updated successfully");
            Ok(())
        } else {
            let retry_after = resp.headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok());
            let body = resp.text().await.unwrap_or_default();
            warn!(status = %status, body = %body, "Failed to update activity");
            Err(map_http_error(status.as_u16(), &body, retry_after))
        }
    }

    /// DELETE {serviceUrl}/v3/conversations/{conversationId}/activities/{activityId}
    pub async fn delete_activity(
        &self,
        service_url: &str,
        conversation_id: &str,
        activity_id: &str,
    ) -> Result<(), ChannelError> {
        let url = format!(
            "{}v3/conversations/{}/activities/{}",
            ensure_trailing_slash(service_url),
            conversation_id,
            activity_id
        );
        let token = self.token_cache.get_token().await?;
        let resp = self
            .http
            .delete(&url)
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| ChannelError::SendFailed(format!("HTTP DELETE failed: {}", e)))?;

        let status = resp.status();
        if status.is_success() {
            debug!(url = %url, "Activity deleted successfully");
            Ok(())
        } else {
            let retry_after = resp.headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok());
            let body = resp.text().await.unwrap_or_default();
            warn!(status = %status, body = %body, "Failed to delete activity");
            Err(map_http_error(status.as_u16(), &body, retry_after))
        }
    }

    /// Send typing indicator
    pub async fn send_typing(
        &self,
        service_url: &str,
        conversation_id: &str,
    ) -> Result<(), ChannelError> {
        let activity = Activity {
            activity_type: "typing".into(),
            ..Default::default()
        };
        self.send_activity(service_url, conversation_id, &activity)
            .await
            .map(|_| ())
    }

    async fn post_activity(
        &self,
        url: &str,
        activity: &Activity,
    ) -> Result<ActivityResponse, ChannelError> {
        let token = self.token_cache.get_token().await?;
        let resp = self
            .http
            .post(url)
            .bearer_auth(&token)
            .json(activity)
            .send()
            .await
            .map_err(|e| ChannelError::SendFailed(format!("HTTP POST failed: {}", e)))?;

        let status = resp.status();
        if status.is_success() {
            let response: ActivityResponse = resp
                .json()
                .await
                .map_err(|e| ChannelError::Internal(format!("Failed to parse response: {}", e)))?;
            debug!(url = %url, id = %response.id, "Activity sent successfully");
            Ok(response)
        } else {
            let retry_after = resp.headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok());
            let body = resp.text().await.unwrap_or_default();
            warn!(status = %status, body = %body, "Failed to send activity");
            Err(map_http_error(status.as_u16(), &body, retry_after))
        }
    }
}

fn ensure_trailing_slash(url: &str) -> String {
    if url.ends_with('/') {
        url.to_string()
    } else {
        format!("{}/", url)
    }
}

fn map_http_error(status: u16, body: &str, retry_after: Option<u64>) -> ChannelError {
    match status {
        401 | 403 => ChannelError::AuthFailed(format!("HTTP {}: {}", status, body)),
        429 => ChannelError::RateLimited { retry_after_secs: retry_after.unwrap_or(5) },
        404 => ChannelError::SendFailed(format!("Not found: {}", body)),
        _ => ChannelError::SendFailed(format!("HTTP {}: {}", status, body)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ensure_trailing_slash() {
        assert_eq!(ensure_trailing_slash("https://example.com"), "https://example.com/");
        assert_eq!(ensure_trailing_slash("https://example.com/"), "https://example.com/");
    }

    #[test]
    fn test_map_http_error_auth() {
        let err = map_http_error(401, "unauthorized", None);
        assert!(matches!(err, ChannelError::AuthFailed(_)));
    }

    #[test]
    fn test_map_http_error_rate_limit() {
        let err = map_http_error(429, "too many requests", None);
        assert!(matches!(err, ChannelError::RateLimited { retry_after_secs: 5 }));
    }

    #[test]
    fn test_map_http_error_rate_limit_with_header() {
        let err = map_http_error(429, "too many requests", Some(30));
        assert!(matches!(err, ChannelError::RateLimited { retry_after_secs: 30 }));
    }

    #[test]
    fn test_map_http_error_not_found() {
        let err = map_http_error(404, "not found", None);
        assert!(matches!(err, ChannelError::SendFailed(_)));
    }

    #[test]
    fn test_map_http_error_server() {
        let err = map_http_error(500, "internal server error", None);
        assert!(matches!(err, ChannelError::SendFailed(_)));
    }
}
