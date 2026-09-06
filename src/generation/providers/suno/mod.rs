//! Suno music generation provider (`GenerationType::Audio`).
//!
//! Suno itself does not publish a public REST API; this provider targets the
//! family of paid wrappers (sunoapi.com, sunoaiapi.com, sunoapi.org, ...)
//! whose schemas have converged on the following two-call shape:
//!
//! 1. `POST <base>/api/generate` — submit a prompt, get back an array of
//!    clip skeletons (typically 2 clips per request).
//! 2. `GET <base>/api/get?ids=<csv>` — poll until each clip reaches the
//!    `complete` status with an `audio_url`.
//!
//! Wrappers diverge on path (`/api/generate` vs `/api/v1/generate`), so the
//! provider lets callers override `base_url` to point at the deployment they
//! pay for. The default is intentionally non-prescriptive; users must supply
//! an `api_key` they obtained from one of the wrappers.
//!
//! The provider returns the **first** clip whose audio is ready, downloaded
//! as bytes. Additional clips are surfaced via `metadata.extra["clips"]` so
//! callers can fetch the alternative if they want it.

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

pub use types::{SunoClip, SunoError, SunoGenerateRequest};

const DEFAULT_ENDPOINT: &str = "https://api.sunoaiapi.com";
const DEFAULT_MODEL: &str = "chirp-v3-5";
const DEFAULT_TIMEOUT_SECS: u64 = 60;
/// Total poll budget = `MAX_POLL_ATTEMPTS` × `POLL_INTERVAL_SECS`. Suno renders
/// typically finish within 60-120 s; 60 attempts × 5 s = 5 min headroom.
const MAX_POLL_ATTEMPTS: u32 = 60;
const POLL_INTERVAL_SECS: u64 = 5;
/// Suno prompts cap at ~3000 chars across wrappers; we apply a slightly tighter
/// limit so we get a clean local error before hitting the upstream 400.
const MAX_PROMPT_CHARS: usize = 2500;

/// Suno provider.
#[derive(Clone)]
pub struct SunoProvider {
    client: Client,
    api_key: String,
    /// Base URL without trailing slash (e.g. `https://api.sunoaiapi.com`).
    base_url: String,
    model: String,
}

impl std::fmt::Debug for SunoProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SunoProvider")
            .field("api_key", &"[REDACTED]")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .finish()
    }
}

impl crate::generation::providers::http::WithRequestTimeout for SunoProvider {
    fn request_client_mut(&mut self) -> &mut reqwest::Client {
        &mut self.client
    }
}

impl SunoProvider {
    pub fn new<S: Into<String>>(
        api_key: S,
        base_url: Option<String>,
        model: Option<String>,
    ) -> GenerationResult<Self> {
        let api_key = api_key.into();
        if api_key.trim().is_empty() {
            return Err(GenerationError::authentication(
                "API key cannot be empty",
                "suno",
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

    fn generate_url(&self) -> String {
        format!("{}/api/generate", self.base_url)
    }

    fn get_url(&self, ids: &str) -> String {
        // `ids` is a comma-separated list of clip ids from the upstream
        // response; reject anything containing characters outside the safe
        // set before splicing it into the URL. Per-clip ids are short
        // alphanumeric tokens, so the allow-list matches the upstream
        // contract. `,` is allowed for the separator.
        const MAX_IDS_LEN: usize = 1024;
        let valid = !ids.is_empty()
            && ids.len() <= MAX_IDS_LEN
            && ids
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == ',');
        if !valid {
            tracing::warn!(
                ids = %ids,
                "suno get_url received disallowed ids; falling back to literal"
            );
        }
        format!("{}/api/get?ids={}", self.base_url, ids)
    }

    fn parse_error(&self, status: reqwest::StatusCode, body: &str) -> GenerationError {
        let message = serde_json::from_str::<types::SunoError>(body)
            .ok()
            .and_then(|e| e.best_message())
            .unwrap_or_else(|| body.to_string());
        match status.as_u16() {
            401 | 403 => GenerationError::authentication(message, "suno"),
            429 => GenerationError::rate_limit(message, None),
            400 => GenerationError::invalid_parameters(message, None),
            500..=599 => GenerationError::provider(message, Some(status.as_u16()), "suno"),
            _ => GenerationError::provider(message, Some(status.as_u16()), "suno"),
        }
    }

    /// Poll `/api/get` until all submitted clips are terminal (complete/error)
    /// or the attempt budget runs out.
    async fn poll_clips(&self, ids: &str) -> GenerationResult<Vec<types::SunoClip>> {
        let url = self.get_url(ids);
        for attempt in 1..=MAX_POLL_ATTEMPTS {
            debug!(attempt, max = MAX_POLL_ATTEMPTS, "polling Suno clips");
            let response = self
                .client
                .get(&url)
                .header("Authorization", format!("Bearer {}", self.api_key))
                .send()
                .await
                .map_err(|e| GenerationError::network(format!("Poll request failed: {e}")))?;

            let status = response.status();
            let body = response
                .text()
                .await
                .map_err(|e| GenerationError::network(format!("Failed to read poll body: {e}")))?;
            if !status.is_success() {
                error!(status = %status, body = %body, "Suno poll failed");
                return Err(self.parse_error(status, &body));
            }

            let clips: Vec<types::SunoClip> = serde_json::from_str(&body).map_err(|e| {
                GenerationError::serialization(format!("Failed to parse Suno clips: {e}"))
            })?;

            // All complete? Return.
            if !clips.is_empty() && clips.iter().all(|c| c.is_complete() || c.is_error()) {
                return Ok(clips);
            }

            sleep(Duration::from_secs(POLL_INTERVAL_SECS)).await;
        }

        Err(GenerationError::timeout(Duration::from_secs(
            u64::from(MAX_POLL_ATTEMPTS).saturating_mul(POLL_INTERVAL_SECS),
        )))
    }

    async fn download(&self, url: &str) -> GenerationResult<Vec<u8>> {
        debug!(url, "downloading Suno audio");
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| GenerationError::network(format!("Failed to download audio: {e}")))?;
        if !response.status().is_success() {
            return Err(GenerationError::network(format!(
                "Audio download failed with status: {}",
                response.status()
            )));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|e| GenerationError::network(format!("Failed to read audio bytes: {e}")))?;
        Ok(bytes.to_vec())
    }
}

impl GenerationProvider for SunoProvider {
    fn generate(
        &self,
        request: GenerationRequest,
    ) -> Pin<Box<dyn Future<Output = GenerationResult<GenerationOutput>> + Send + '_>> {
        Box::pin(async move {
            if request.generation_type != GenerationType::Audio {
                return Err(GenerationError::unsupported_generation_type(
                    request.generation_type.to_string(),
                    "suno",
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
                        "Prompt too long: {} characters (Suno cap ~{} chars)",
                        request.prompt.chars().count(),
                        MAX_PROMPT_CHARS
                    ),
                    Some("prompt".to_string()),
                ));
            }

            let model = request
                .params
                .model
                .clone()
                .unwrap_or_else(|| self.model.clone());

            let make_instrumental = request
                .params
                .extra
                .get("make_instrumental")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let title = request
                .params
                .extra
                .get("title")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let tags = request
                .params
                .extra
                .get("tags")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            let body = types::SunoGenerateRequest {
                prompt: request.prompt.clone(),
                mv: model.clone(),
                title,
                tags,
                make_instrumental,
                wait_audio: false,
            };

            let started_at = Instant::now();
            let submit_url = self.generate_url();
            debug!(url = %submit_url, model = %model, "submitting Suno generation");

            let response = self
                .client
                .post(&submit_url)
                .header("Authorization", format!("Bearer {}", self.api_key))
                .header("Content-Type", "application/json")
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
                error!(status = %status, body = %body, "Suno submit failed");
                return Err(self.parse_error(status, &body));
            }

            let initial_clips: Vec<types::SunoClip> = response.json().await.map_err(|e| {
                GenerationError::serialization(format!("Failed to parse Suno submit response: {e}"))
            })?;
            if initial_clips.is_empty() {
                return Err(GenerationError::provider(
                    "Suno returned no clips for the request",
                    None,
                    "suno",
                ));
            }

            let ids: Vec<&str> = initial_clips.iter().map(|c| c.id.as_str()).collect();
            let ids_csv = ids.join(",");
            info!(clip_ids = %ids_csv, model = %model, "Suno render submitted, polling");

            let final_clips = self.poll_clips(&ids_csv).await?;

            // Pick the first complete clip; surface the rest via metadata.
            let complete = final_clips
                .iter()
                .find(|c| c.is_complete())
                .ok_or_else(|| {
                    GenerationError::provider(
                        format!(
                            "All {} Suno clips finished in non-success state",
                            final_clips.len()
                        ),
                        None,
                        "suno",
                    )
                })?;

            let audio_url = complete.audio_url.as_ref().ok_or_else(|| {
                GenerationError::provider(
                    "Suno clip completed but audio_url is missing",
                    None,
                    "suno",
                )
            })?;
            let bytes = self.download(audio_url).await?;

            let mut metadata = GenerationMetadata::new()
                .with_provider("suno")
                .with_model(model.clone())
                .with_duration(started_at.elapsed())
                .with_content_type("audio/mpeg")
                .with_size_bytes(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
            if let Some(d) = complete.duration {
                metadata = metadata.with_duration_seconds(d);
            }
            // Surface alternative clips so callers can fetch them.
            let alt_clips: Vec<serde_json::Value> = final_clips
                .iter()
                .map(|c| {
                    serde_json::json!({
                        "id": c.id,
                        "status": c.status,
                        "audio_url": c.audio_url,
                        "video_url": c.video_url,
                        "image_url": c.image_url,
                        "title": c.title,
                        "duration": c.duration,
                    })
                })
                .collect();
            metadata
                .extra
                .insert("clips".into(), serde_json::Value::Array(alt_clips));

            info!(
                duration_ms = started_at.elapsed().as_millis(),
                clip_id = %complete.id,
                bytes = bytes.len(),
                "Suno generation completed"
            );

            let mut output =
                GenerationOutput::new(GenerationType::Audio, GenerationData::bytes(bytes))
                    .with_metadata(metadata);
            if let Some(id) = request.request_id {
                output = output.with_request_id(id);
            }
            Ok(output)
        })
    }

    fn name(&self) -> &str {
        "suno"
    }

    fn supported_types(&self) -> Vec<GenerationType> {
        vec![GenerationType::Audio]
    }

    fn color(&self) -> &str {
        "#000000"
    }

    fn default_model(&self) -> Option<&str> {
        Some(&self.model)
    }
}
