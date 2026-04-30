//! Webhook delivery target for cron job results.
//!
//! Sends job results to external HTTP endpoints with SSRF protection.

use std::time::Duration;

use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderValue};
use reqwest::Method;

use crate::security::ssrf::{safe_fetch, SafeFetchRequest, SsrfPolicy};
use crate::tasks::shared::delivery::{
    DeliveryError, DeliveryOutcome, DeliveryPayload, DeliveryTarget, DeliveryTargetConfig,
};

pub struct WebhookTarget;

impl Default for WebhookTarget {
    fn default() -> Self {
        Self::new()
    }
}

impl WebhookTarget {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl DeliveryTarget for WebhookTarget {
    fn kind(&self) -> &str {
        "webhook"
    }

    async fn deliver(
        &self,
        payload: &DeliveryPayload,
        config: &DeliveryTargetConfig,
    ) -> Result<DeliveryOutcome, DeliveryError> {
        let (url, method, headers) = match config {
            DeliveryTargetConfig::Webhook {
                url,
                method,
                headers,
            } => (url, method, headers),
            _ => {
                return Err(DeliveryError::InvalidConfig(
                    "Expected Webhook config".into(),
                ))
            }
        };

        let body = serde_json::json!({
            "source_type": payload.source_type,
            "task_name": payload.task_name,
            "agent_id": payload.agent_id,
            "output": payload.output,
            "channel_id": payload.channel_id,
            "metadata": payload.metadata,
        });

        let body_bytes =
            serde_json::to_vec(&body).map_err(|e| DeliveryError::Failed(e.to_string()))?;

        // Build headers
        let mut header_map = HeaderMap::new();
        header_map.insert("content-type", HeaderValue::from_static("application/json"));
        if let Some(hdrs) = headers {
            for (key, value) in hdrs {
                if let (Ok(name), Ok(val)) = (
                    key.parse::<reqwest::header::HeaderName>(),
                    HeaderValue::from_str(value),
                ) {
                    header_map.insert(name, val);
                }
            }
        }

        // Determine HTTP method
        let http_method = match method.as_deref().unwrap_or("POST") {
            "GET" => Method::GET,
            "PUT" => Method::PUT,
            "PATCH" => Method::PATCH,
            "DELETE" => Method::DELETE,
            "HEAD" => Method::HEAD,
            "OPTIONS" => Method::OPTIONS,
            _ => Method::POST,
        };

        let fetch_request = SafeFetchRequest::post(body_bytes, Duration::from_secs(30))
            .with_method(http_method)
            .with_headers(header_map);

        match safe_fetch(url, &SsrfPolicy::default(), fetch_request).await {
            Ok(resp) if resp.status.is_success() => Ok(DeliveryOutcome {
                target_kind: "webhook".to_string(),
                success: true,
                message: Some(format!("HTTP {}", resp.status)),
            }),
            Ok(resp) => Err(DeliveryError::Failed(format!(
                "HTTP {} from {}",
                resp.status, url
            ))),
            Err(e) => Err(DeliveryError::Failed(format!("Request failed: {}", e))),
        }
    }
}
