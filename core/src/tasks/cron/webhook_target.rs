//! Webhook delivery target for cron job results.
//!
//! Sends job results to external HTTP endpoints.

use async_trait::async_trait;

use crate::tasks::shared::delivery::{
    DeliveryError, DeliveryOutcome, DeliveryPayload, DeliveryTarget, DeliveryTargetConfig,
};

pub struct WebhookTarget {
    client: reqwest::Client,
}

impl Default for WebhookTarget {
    fn default() -> Self {
        Self::new()
    }
}

impl WebhookTarget {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(10))
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
        }
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
            _ => return Err(DeliveryError::InvalidConfig("Expected Webhook config".into())),
        };

        let body = serde_json::json!({
            "source_type": payload.source_type,
            "task_name": payload.task_name,
            "agent_id": payload.agent_id,
            "output": payload.output,
            "channel_id": payload.channel_id,
            "metadata": payload.metadata,
        });

        let method = method.as_deref().unwrap_or("POST");
        let mut request = match method {
            "PUT" => self.client.put(url),
            _ => self.client.post(url),
        };

        request = request
            .header("Content-Type", "application/json")
            .json(&body);

        // Add custom headers
        if let Some(hdrs) = headers {
            for (key, value) in hdrs {
                request = request.header(key.as_str(), value.as_str());
            }
        }

        match request.send().await {
            Ok(resp) if resp.status().is_success() => Ok(DeliveryOutcome {
                target_kind: "webhook".to_string(),
                success: true,
                message: Some(format!("HTTP {}", resp.status())),
            }),
            Ok(resp) => Err(DeliveryError::Failed(format!(
                "HTTP {} from {}",
                resp.status(),
                url
            ))),
            Err(e) => Err(DeliveryError::Failed(format!("Request failed: {}", e))),
        }
    }
}
