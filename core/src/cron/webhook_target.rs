//! Webhook delivery target for cron job results.
//!
//! Sends job results to external HTTP endpoints.

use async_trait::async_trait;

use crate::cron::config::{CronJob, DeliveryOutcome, DeliveryTargetConfig, JobRun};
use crate::cron::delivery::{DeliveryError, DeliveryTarget};

pub struct WebhookTarget {
    client: reqwest::Client,
}

impl WebhookTarget {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
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
        job: &CronJob,
        run: &JobRun,
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

        let payload = serde_json::json!({
            "job_id": job.id,
            "job_name": job.name,
            "status": run.status.to_string(),
            "response": run.response,
            "error": run.error,
            "duration_ms": run.duration_ms,
            "started_at": run.started_at,
            "ended_at": run.ended_at,
        });

        let method = method.as_deref().unwrap_or("POST");
        let mut request = match method {
            "PUT" => self.client.put(url),
            _ => self.client.post(url),
        };

        request = request
            .header("Content-Type", "application/json")
            .json(&payload);

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
