//! Black Forest Labs FLUX image provider (`GenerationType::Image`).
//!
//! Implements the BFL async REST API (docs.bfl.ai). Each generation is a
//! submit-then-poll dance:
//!
//! 1. `POST {base}/v1/<model>` with `x-key: <api_key>` and the prompt body.
//!    Returns `{ id, polling_url? }`.
//! 2. `GET {base}/v1/get_result?id=<id>` (or the `polling_url` if present)
//!    until `status == "Ready"`, at which point `result.sample` is a signed
//!    URL pointing at the rendered image. We download the bytes and return
//!    them in `GenerationOutput::data`.
//!
//! Model selection is model-name-in-path: a single provider instance is
//! pinned to one model (`flux-pro-1.1`, `flux-dev`, `flux-pro-1.1-ultra`,
//! `flux-pro-finetuned`, ...). Callers can override per-request via
//! `params.model`.
//!
//! Out of scope here: image-to-image variants (`/v1/<model>-redux`),
//! background removal, and the deprecated `controlnet` family — none of
//! which fit the bare prompt → image flow.

use crate::generation::{
    GenerationData, GenerationError, GenerationMetadata, GenerationOutput, GenerationProvider,
    GenerationRequest, GenerationResult, GenerationType,
};
use reqwest::Client;
use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};
use tokio::time::sleep;
use tracing::{debug, error, info};

#[cfg(test)]
mod tests;
pub mod types;

pub use types::{
    classify_status, BflError, BflGenerateRequest, BflPollResponse, BflResult, BflSubmitResponse,
    StatusClass,
};

const DEFAULT_ENDPOINT: &str = "https://api.bfl.ai";
const DEFAULT_MODEL: &str = "flux-pro-1.1";
const DEFAULT_TIMEOUT_SECS: u64 = 60;
/// Total poll budget: 60 × 2s = 2 minutes. FLUX renders typically finish in
/// 5-20 s; the longer ones (ultra, finetuned) can take up to a minute.
const MAX_POLL_ATTEMPTS: u32 = 60;
const POLL_INTERVAL_SECS: u64 = 2;
const MAX_PROMPT_CHARS: usize = 5000;

/// Output formats accepted by BFL. `"jpeg"` is the BFL default.
pub const AVAILABLE_FORMATS: [&str; 2] = ["jpeg", "png"];

/// BFL FLUX provider.
#[derive(Clone)]
pub struct BflProvider {
    client: Client,
    api_key: String,
    base_url: String,
    model: String,
}

impl std::fmt::Debug for BflProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BflProvider")
            .field("api_key", &"[REDACTED]")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .finish()
    }
}

impl crate::generation::providers::http::WithRequestTimeout for BflProvider {
    fn request_client_mut(&mut self) -> &mut reqwest::Client {
        &mut self.client
    }
}

impl BflProvider {
    pub fn new<S: Into<String>>(
        api_key: S,
        base_url: Option<String>,
        model: Option<String>,
    ) -> GenerationResult<Self> {
        let api_key = api_key.into();
        if api_key.trim().is_empty() {
            return Err(GenerationError::authentication(
                "API key cannot be empty",
                "bfl-flux",
            ));
        }
        let client = Client::builder()
            .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
            .build()
            .map_err(|e| GenerationError::network(format!("Failed to build HTTP client: {e}")))?;
        let base = base_url.unwrap_or_else(|| DEFAULT_ENDPOINT.to_string());
        let base_url = base.trim_end_matches('/').to_string();
        Ok(Self {
            client,
            api_key,
            base_url,
            model: model.unwrap_or_else(|| DEFAULT_MODEL.to_string()),
        })
    }

    fn submit_url(&self, model: &str) -> String {
        format!("{}/v1/{}", self.base_url, model)
    }

    fn get_result_url(&self, id: &str) -> String {
        format!("{}/v1/get_result?id={}", self.base_url, id)
    }

    fn parse_error(&self, status: reqwest::StatusCode, body: &str) -> GenerationError {
        let message = serde_json::from_str::<types::BflError>(body)
            .ok()
            .and_then(|e| e.best_message())
            .unwrap_or_else(|| body.to_string());
        match status.as_u16() {
            401 | 403 => GenerationError::authentication(message, "bfl-flux"),
            429 => GenerationError::rate_limit(message, None),
            400 => GenerationError::invalid_parameters(message, None),
            500..=599 => GenerationError::provider(message, Some(status.as_u16()), "bfl-flux"),
            _ => GenerationError::provider(message, Some(status.as_u16()), "bfl-flux"),
        }
    }

    async fn poll_result(
        &self,
        id: &str,
        override_url: Option<&str>,
    ) -> GenerationResult<types::BflResult> {
        let url = override_url.map_or_else(|| self.get_result_url(id), |s| s.to_string());
        for attempt in 1..=MAX_POLL_ATTEMPTS {
            debug!(attempt, max = MAX_POLL_ATTEMPTS, id, "polling BFL result");
            let response = self
                .client
                .get(&url)
                .header("x-key", &self.api_key)
                .header("accept", "application/json")
                .send()
                .await
                .map_err(|e| GenerationError::network(format!("Poll request failed: {e}")))?;
            let status = response.status();
            let body = response
                .text()
                .await
                .map_err(|e| GenerationError::network(format!("Failed to read poll body: {e}")))?;
            if !status.is_success() {
                error!(status = %status, body = %body, "BFL poll failed");
                return Err(self.parse_error(status, &body));
            }

            let poll: types::BflPollResponse = serde_json::from_str(&body).map_err(|e| {
                GenerationError::serialization(format!("Failed to parse BFL poll response: {e}"))
            })?;

            match classify_status(&poll.status) {
                StatusClass::Success => {
                    let result = poll.result.ok_or_else(|| {
                        GenerationError::provider(
                            "BFL reported Ready but result body was empty",
                            None,
                            "bfl-flux",
                        )
                    })?;
                    return Ok(result);
                }
                StatusClass::Pending => {
                    sleep(Duration::from_secs(POLL_INTERVAL_SECS)).await;
                    continue;
                }
                StatusClass::Error => {
                    return Err(GenerationError::provider(
                        format!("BFL render failed for id {id}"),
                        None,
                        "bfl-flux",
                    ));
                }
                StatusClass::Moderated => {
                    return Err(GenerationError::invalid_parameters(
                        format!("BFL moderation blocked the request: {}", poll.status),
                        Some("prompt".to_string()),
                    ));
                }
                StatusClass::NotFound => {
                    return Err(GenerationError::provider(
                        format!("BFL task {id} not found (expired or invalid)"),
                        None,
                        "bfl-flux",
                    ));
                }
                StatusClass::Unknown => {
                    // Unknown but presumed non-terminal — keep polling.
                    debug!(status = %poll.status, "Unknown BFL status, continuing to poll");
                    sleep(Duration::from_secs(POLL_INTERVAL_SECS)).await;
                    continue;
                }
            }
        }

        Err(GenerationError::timeout(Duration::from_secs(
            u64::from(MAX_POLL_ATTEMPTS).saturating_mul(POLL_INTERVAL_SECS),
        )))
    }

    async fn download(&self, url: &str) -> GenerationResult<Vec<u8>> {
        debug!(url, "downloading BFL image");
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| GenerationError::network(format!("Failed to download image: {e}")))?;
        if !response.status().is_success() {
            return Err(GenerationError::network(format!(
                "Image download failed with status: {}",
                response.status()
            )));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|e| GenerationError::network(format!("Failed to read image bytes: {e}")))?;
        Ok(bytes.to_vec())
    }
}

impl GenerationProvider for BflProvider {
    fn generate(
        &self,
        request: GenerationRequest,
    ) -> Pin<Box<dyn Future<Output = GenerationResult<GenerationOutput>> + Send + '_>> {
        Box::pin(async move {
            if request.generation_type != GenerationType::Image {
                return Err(GenerationError::unsupported_generation_type(
                    request.generation_type.to_string(),
                    "bfl-flux",
                ));
            }
            if request.prompt.trim().is_empty() {
                return Err(GenerationError::invalid_parameters(
                    "Prompt cannot be empty",
                    Some("prompt".to_string()),
                ));
            }
            if request.prompt.chars().count() > MAX_PROMPT_CHARS {
                return Err(GenerationError::invalid_parameters(
                    format!(
                        "Prompt too long: {} characters (BFL cap ~{} chars)",
                        request.prompt.chars().count(),
                        MAX_PROMPT_CHARS
                    ),
                    Some("prompt".to_string()),
                ));
            }

            if let Some(ref format) = request.params.format {
                if !AVAILABLE_FORMATS.contains(&format.as_str()) {
                    return Err(GenerationError::unsupported_format(
                        format.clone(),
                        AVAILABLE_FORMATS.iter().map(|s| s.to_string()).collect(),
                    ));
                }
            }

            let model = request
                .params
                .model
                .clone()
                .unwrap_or_else(|| self.model.clone());

            let body = types::BflGenerateRequest {
                prompt: request.prompt.clone(),
                width: request.params.width,
                height: request.params.height,
                steps: request.params.steps,
                guidance: request
                    .params
                    .extra
                    .get("guidance")
                    .and_then(|v| v.as_f64())
                    .map(|v| v as f32),
                seed: request.params.seed.and_then(|s| u64::try_from(s).ok()),
                safety_tolerance: request
                    .params
                    .extra
                    .get("safety_tolerance")
                    .and_then(|v| v.as_u64())
                    .and_then(|v| u32::try_from(v).ok()),
                output_format: request.params.format.clone(),
                extras: None,
            };

            let started_at = Instant::now();
            let submit_url = self.submit_url(&model);
            debug!(url = %submit_url, model = %model, "submitting BFL render");

            let response = self
                .client
                .post(&submit_url)
                .header("x-key", &self.api_key)
                .header("Content-Type", "application/json")
                .header("accept", "application/json")
                .json(&body)
                .send()
                .await
                .map_err(|e| {
                    if e.is_timeout() {
                        GenerationError::timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
                    } else if e.is_connect() {
                        GenerationError::network(format!("Connection failed: {e}"))
                    } else {
                        GenerationError::network(e.to_string())
                    }
                })?;

            let status = response.status();
            if !status.is_success() {
                let body = response.text().await.map_err(|e| {
                    GenerationError::network(format!("Failed to read error body: {e}"))
                })?;
                error!(status = %status, body = %body, "BFL submit failed");
                return Err(self.parse_error(status, &body));
            }

            let submit: types::BflSubmitResponse = response.json().await.map_err(|e| {
                GenerationError::serialization(format!("Failed to parse BFL submit response: {e}"))
            })?;
            info!(id = %submit.id, model = %model, "BFL render submitted, polling");

            let result = self
                .poll_result(&submit.id, submit.polling_url.as_deref())
                .await?;

            let sample_url = result.sample.clone().ok_or_else(|| {
                GenerationError::provider(
                    "BFL reported Ready but result.sample was empty",
                    None,
                    "bfl-flux",
                )
            })?;
            let bytes = self.download(&sample_url).await?;
            let content_type = match request.params.format.as_deref() {
                Some("png") => "image/png",
                _ => "image/jpeg",
            };

            let mut metadata = GenerationMetadata::new()
                .with_provider("bfl-flux")
                .with_model(model.clone())
                .with_duration(started_at.elapsed())
                .with_content_type(content_type)
                .with_size_bytes(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
            if let Some(seed) = result.seed {
                metadata
                    .extra
                    .insert("seed".into(), serde_json::Value::from(seed));
            }
            if let Some(d) = result.duration {
                metadata.extra.insert(
                    "render_seconds".into(),
                    serde_json::Value::from(f64::from(d)),
                );
            }
            metadata.extra.insert(
                "task_id".into(),
                serde_json::Value::String(submit.id.clone()),
            );

            info!(
                duration_ms = started_at.elapsed().as_millis(),
                id = %submit.id,
                bytes = bytes.len(),
                "BFL FLUX generation completed"
            );

            let mut output =
                GenerationOutput::new(GenerationType::Image, GenerationData::bytes(bytes))
                    .with_metadata(metadata);
            if let Some(id) = request.request_id {
                output = output.with_request_id(id);
            }
            Ok(output)
        })
    }

    fn name(&self) -> &str {
        "bfl-flux"
    }

    fn supported_types(&self) -> Vec<GenerationType> {
        vec![GenerationType::Image]
    }

    fn color(&self) -> &str {
        "#1a1a1a"
    }

    fn default_model(&self) -> Option<&str> {
        Some(&self.model)
    }
}
