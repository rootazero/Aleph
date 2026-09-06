//! Fal.ai aggregator generation provider.
//!
//! Fal hosts hundreds of community models under a uniform queue-based API:
//! `POST {queue}/<model-path>` returns a request id, then poll
//! `GET {queue}/<model-path>/requests/<id>/status` until `status=COMPLETED`,
//! then fetch the result via `GET .../requests/<id>`.
//!
//! Auth is a single header: `Authorization: Key <fal-key>`.
//!
//! A single `FalProvider` instance is configured for **one** Fal endpoint
//! path (e.g. `fal-ai/flux/dev`, `fal-ai/kling-video/v1/pro/text-to-video`).
//! The endpoint path is supplied via `model` in the `GenerationParams` or
//! falls back to the constructor default. This lets us ship one
//! implementation that reaches Flux, Kling, Pika, Luma, Runway, Hailuo,
//! Jimeng and `MusicGen` by varying just the preset/model — exactly the
//! aggregator shape openclaw uses.
//!
//! API reference: <https://fal.ai/docs/model-endpoints>

use crate::generation::{
    GenerationData, GenerationError, GenerationMetadata, GenerationOutput, GenerationProgress,
    GenerationProvider, GenerationRequest, GenerationResult, GenerationType,
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};
use tracing::{debug, error, info, warn};

/// Default queue endpoint — Fal's regional router picks the right backend.
const DEFAULT_ENDPOINT: &str = "https://queue.fal.run";
/// Default model path when caller doesn't specify one.
const DEFAULT_MODEL: &str = "fal-ai/flux/dev";
/// Maximum wall-clock time we wait for a job, including all polls. Bounds
/// the POLL LOOP below, not any single request.
const JOB_DEADLINE_SECS: u64 = 600;
/// Per-request cap when nothing configures one.
///
/// ⚠️ Distinct from [`JOB_DEADLINE_SECS`], and they were ONE constant until
/// 2026-09-05: the same 600 was handed to `Client::builder().timeout()` and to
/// the poll loop's deadline, so the HTTP client carried a ten-minute
/// per-request cap justified by a doc that was describing the loop. A single
/// request here is a submit, one poll, or a result fetch; none of those is a
/// ten-minute operation (判据 §1 — one name, two facts).
const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 120;
/// Initial poll interval; we back off slightly each iteration.
const POLL_INTERVAL_INITIAL_MS: u64 = 1500;
const POLL_INTERVAL_MAX_MS: u64 = 6000;

/// Fal.ai queue-API provider.
///
/// Configured for one model path; capabilities (image/video/audio) are
/// declared at construction time so the factory can route by [`GenerationType`].
#[derive(Clone)]
pub struct FalProvider {
    client: Client,
    api_key: String,
    /// Queue base, e.g. `https://queue.fal.run` (no trailing slash).
    endpoint: String,
    /// Default model path when the request doesn't override it.
    model: String,
    /// Which generation types this preset advertises (image / video / music).
    supported: Vec<GenerationType>,
    color: String,
    name: String,
}

impl std::fmt::Debug for FalProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FalProvider")
            .field("api_key", &"[REDACTED]")
            .field("endpoint", &self.endpoint)
            .field("model", &self.model)
            .field("supported", &self.supported)
            .field("name", &self.name)
            .finish()
    }
}

/// Builder for [`FalProvider`].
///
/// `pub(crate)` because the only consumers are [`FalProvider::builder`] (this
/// module's own factory surface) and the unit tests in this module. The
/// factory at `providers/factory.rs` calls `FalProvider::builder(...).build()`
/// without naming the builder type, so widening this to `pub` only exposed the
/// fluent API to external crates that never use it. (severed-wire audit
/// 2026-09-04, sw-generation-3-1.)
pub(crate) struct FalProviderBuilder {
    name: String,
    api_key: String,
    endpoint: Option<String>,
    model: Option<String>,
    supported: Vec<GenerationType>,
    color: String,
    timeout_secs: u64,
}

impl FalProviderBuilder {
    pub fn new(name: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            api_key: api_key.into(),
            endpoint: None,
            model: None,
            supported: vec![GenerationType::Image],
            color: "#ff8ad1".into(),
            timeout_secs: DEFAULT_REQUEST_TIMEOUT_SECS,
        }
    }

    /// Set the per-request HTTP timeout. NOT the job deadline -- see
    /// [`JOB_DEADLINE_SECS`].
    #[must_use]
    pub const fn timeout_secs(mut self, secs: Option<u64>) -> Self {
        // `None` = unconfigured. Keep the default this builder chose; the
        // config field cannot express "unset" any other way.
        if let Some(secs) = secs {
            self.timeout_secs = secs;
        }
        self
    }

    pub fn endpoint(mut self, base: impl Into<String>) -> Self {
        self.endpoint = Some(base.into());
        self
    }

    pub fn model(mut self, m: impl Into<String>) -> Self {
        self.model = Some(m.into());
        self
    }

    #[must_use]
    pub fn supported_types(mut self, types: Vec<GenerationType>) -> Self {
        if !types.is_empty() {
            self.supported = types;
        }
        self
    }

    pub fn color(mut self, c: impl Into<String>) -> Self {
        self.color = c.into();
        self
    }

    pub fn build(self) -> GenerationResult<FalProvider> {
        let client = Client::builder()
            .timeout(Duration::from_secs(self.timeout_secs.max(1)))
            .build()
            .map_err(|e| GenerationError::network(format!("Failed to build HTTP client: {e}")))?;

        // Fal's queue URL doesn't fit the OpenAI-style /v1 auto-complete
        // (paths look like `<base>/<owner>/<model>/...`), so we just trim
        // trailing slashes and join the model path verbatim below.
        let endpoint = self
            .endpoint
            .unwrap_or_else(|| DEFAULT_ENDPOINT.to_string())
            .trim_end_matches('/')
            .to_string();

        Ok(FalProvider {
            client,
            api_key: self.api_key,
            endpoint,
            model: self.model.unwrap_or_else(|| DEFAULT_MODEL.to_string()),
            supported: self.supported,
            color: self.color,
            name: self.name,
        })
    }
}

impl FalProvider {
    pub(crate) fn builder(
        name: impl Into<String>,
        api_key: impl Into<String>,
    ) -> FalProviderBuilder {
        FalProviderBuilder::new(name, api_key)
    }

    /// Decide the model path: explicit request override wins, else default.
    fn resolved_model<'a>(&'a self, request: &'a GenerationRequest) -> &'a str {
        request
            .params
            .model
            .as_deref()
            .filter(|m| !m.is_empty())
            .unwrap_or(self.model.as_str())
    }

    /// Convert known `GenerationParams` fields into Fal's input object.
    fn build_input(&self, request: &GenerationRequest) -> Value {
        let mut input = Map::new();
        input.insert("prompt".into(), Value::String(request.prompt.clone()));

        if let Some(neg) = &request.params.negative_prompt {
            input.insert("negative_prompt".into(), Value::String(neg.clone()));
        }
        if let Some(seed) = request.params.seed {
            input.insert("seed".into(), Value::from(seed));
        }
        if let Some(steps) = request.params.steps {
            input.insert("num_inference_steps".into(), Value::from(steps));
        }
        if let Some(cfg) = request.params.guidance_scale {
            input.insert("guidance_scale".into(), Value::from(cfg));
        }
        if let Some(n) = request.params.n {
            input.insert("num_images".into(), Value::from(n));
        }
        if let Some(ar) = &request.params.aspect_ratio {
            input.insert("aspect_ratio".into(), Value::String(ar.clone()));
        }
        if let Some(dur) = request.params.duration_seconds {
            input.insert("duration".into(), Value::from(dur));
        }
        if let Some(fps) = request.params.fps {
            input.insert("fps".into(), Value::from(fps));
        }
        if let Some(ref_img) = &request.params.reference_image {
            input.insert("image_url".into(), Value::String(ref_img.clone()));
        }
        if let (Some(w), Some(h)) = (request.params.width, request.params.height) {
            // Fal models vary in how they accept size; expose all common shapes.
            input.insert(
                "image_size".into(),
                serde_json::json!({"width": w, "height": h}),
            );
        }

        // Bring through any provider-specific overrides verbatim.
        input.extend(
            request
                .params
                .extra
                .iter()
                .map(|(k, v)| (k.clone(), v.clone())),
        );

        Value::Object(input)
    }

    fn submit_url(&self, model_path: &str) -> String {
        format!("{}/{}", self.endpoint, model_path.trim_start_matches('/'))
    }

    fn status_url(&self, model_path: &str, request_id: &str) -> String {
        format!(
            "{}/{}/requests/{}/status",
            self.endpoint,
            model_path.trim_start_matches('/'),
            request_id
        )
    }

    fn result_url(&self, model_path: &str, request_id: &str) -> String {
        format!(
            "{}/{}/requests/{}",
            self.endpoint,
            model_path.trim_start_matches('/'),
            request_id
        )
    }

    fn auth_header(&self) -> String {
        format!("Key {}", self.api_key)
    }

    fn map_http_error(&self, status: reqwest::StatusCode, body: &str) -> GenerationError {
        match status.as_u16() {
            401 | 403 => GenerationError::authentication(
                format!(
                    "Fal auth failed: {}",
                    body.chars().take(200).collect::<String>()
                ),
                &self.name,
            ),
            429 => GenerationError::rate_limit("Fal rate limit exceeded", None),
            400..=499 => GenerationError::invalid_parameters(
                format!("Fal client error {}: {}", status.as_u16(), body),
                None,
            ),
            500..=599 => GenerationError::provider(
                format!("Fal server error: {body}"),
                Some(status.as_u16()),
                &self.name,
            ),
            _ => GenerationError::provider(
                format!("Fal unexpected status {}: {}", status.as_u16(), body),
                Some(status.as_u16()),
                &self.name,
            ),
        }
    }

    /// POST → `request_id`.
    async fn submit(&self, model_path: &str, input: &Value) -> GenerationResult<FalSubmitResponse> {
        let url = self.submit_url(model_path);
        debug!(url = %url, "fal submit");
        let resp = self
            .client
            .post(&url)
            .header("Authorization", self.auth_header())
            .header("Content-Type", "application/json")
            .json(input)
            .send()
            .await
            .map_err(|e| GenerationError::network(format!("Fal submit: {e}")))?;

        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| GenerationError::network(format!("Fal submit body: {e}")))?;
        if !status.is_success() {
            error!(%status, %body, "fal submit failed");
            return Err(self.map_http_error(status, &body));
        }
        serde_json::from_str(&body).map_err(|e| {
            GenerationError::serialization(format!("Fal submit parse: {e} (body={body})"))
        })
    }

    async fn poll_status(
        &self,
        model_path: &str,
        request_id: &str,
    ) -> GenerationResult<FalStatusResponse> {
        let url = self.status_url(model_path, request_id);
        let resp = self
            .client
            .get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| GenerationError::network(format!("Fal poll: {e}")))?;
        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| GenerationError::network(format!("Fal poll body: {e}")))?;
        if !status.is_success() {
            return Err(self.map_http_error(status, &body));
        }
        serde_json::from_str(&body).map_err(|e| {
            GenerationError::serialization(format!("Fal poll parse: {e} (body={body})"))
        })
    }

    async fn fetch_result(&self, model_path: &str, request_id: &str) -> GenerationResult<Value> {
        let url = self.result_url(model_path, request_id);
        let resp = self
            .client
            .get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| GenerationError::network(format!("Fal result: {e}")))?;
        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| GenerationError::network(format!("Fal result body: {e}")))?;
        if !status.is_success() {
            return Err(self.map_http_error(status, &body));
        }
        serde_json::from_str(&body).map_err(|e| {
            GenerationError::serialization(format!("Fal result parse: {e} (body={body})"))
        })
    }
}

/// Walk a Fal result object and pick the first URL that looks like the
/// primary asset. Fal payloads vary by model — some put it under
/// `images[0].url`, others under `video.url` or `audio.url` — so we accept
/// any of these shapes.
fn extract_primary_url(result: &Value) -> Option<String> {
    // images[0].url
    if let Some(url) = result
        .get("images")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|x| x.get("url"))
        .and_then(|u| u.as_str())
    {
        return Some(url.to_string());
    }
    // video.url
    if let Some(url) = result
        .get("video")
        .and_then(|v| v.get("url"))
        .and_then(|s| s.as_str())
    {
        return Some(url.to_string());
    }
    // audio.url
    if let Some(url) = result
        .get("audio")
        .and_then(|v| v.get("url"))
        .and_then(|s| s.as_str())
    {
        return Some(url.to_string());
    }
    // file.url (MusicGen + a few others)
    if let Some(url) = result
        .get("file")
        .and_then(|v| v.get("url"))
        .and_then(|s| s.as_str())
    {
        return Some(url.to_string());
    }
    // Bare top-level "url" field
    result
        .get("url")
        .and_then(|u| u.as_str())
        .map(|s| s.to_string())
}

#[derive(Debug, Deserialize)]
struct FalSubmitResponse {
    request_id: String,
    #[serde(default)]
    #[allow(dead_code)]
    status: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct FalStatusResponse {
    status: String,
    #[serde(default)]
    queue_position: Option<u32>,
}

impl FalStatusResponse {
    fn is_terminal(&self) -> bool {
        matches!(
            self.status.as_str(),
            "COMPLETED" | "ERROR" | "FAILED" | "CANCELED"
        )
    }
    fn is_success(&self) -> bool {
        self.status == "COMPLETED"
    }
}

impl GenerationProvider for FalProvider {
    fn generate(
        &self,
        request: GenerationRequest,
    ) -> Pin<Box<dyn Future<Output = GenerationResult<GenerationOutput>> + Send + '_>> {
        Box::pin(async move {
            if !self.supports(request.generation_type) {
                return Err(GenerationError::unsupported_generation_type(
                    request.generation_type.to_string(),
                    &self.name,
                ));
            }
            let started = Instant::now();
            let model_path = self.resolved_model(&request).to_string();
            let input = self.build_input(&request);

            let submit = self.submit(&model_path, &input).await?;
            info!(name = %self.name, request_id = %submit.request_id, model = %model_path, "fal job submitted");

            // Poll until terminal status or wall-clock timeout.
            let mut interval_ms = POLL_INTERVAL_INITIAL_MS;
            let deadline = started + Duration::from_secs(JOB_DEADLINE_SECS);
            loop {
                if Instant::now() > deadline {
                    return Err(GenerationError::timeout(Duration::from_secs(
                        JOB_DEADLINE_SECS,
                    )));
                }
                tokio::time::sleep(Duration::from_millis(interval_ms)).await;
                let st = self.poll_status(&model_path, &submit.request_id).await?;
                debug!(status = %st.status, qpos = ?st.queue_position, "fal poll");
                if st.is_terminal() {
                    if !st.is_success() {
                        return Err(GenerationError::provider(
                            format!("Fal job ended with status {}", st.status),
                            None,
                            &self.name,
                        ));
                    }
                    break;
                }
                // Linear backoff capped — Fal's queue position decay isn't
                // exponential, so we don't need aggressive backoff.
                interval_ms = (interval_ms + 500).min(POLL_INTERVAL_MAX_MS);
            }

            let result = self.fetch_result(&model_path, &submit.request_id).await?;
            let url = extract_primary_url(&result).ok_or_else(|| {
                warn!(?result, "fal result had no recognised URL field");
                GenerationError::provider(
                    "Fal result did not contain a primary asset URL",
                    None,
                    &self.name,
                )
            })?;

            let metadata = GenerationMetadata::new()
                .with_provider(self.name.clone())
                .with_model(model_path)
                .with_duration(started.elapsed());

            let mut output =
                GenerationOutput::new(request.generation_type, GenerationData::url(url))
                    .with_metadata(metadata);
            if let Some(id) = request.request_id {
                output = output.with_request_id(id);
            }
            Ok(output)
        })
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn supported_types(&self) -> Vec<GenerationType> {
        self.supported.clone()
    }

    fn color(&self) -> &str {
        &self.color
    }

    fn default_model(&self) -> Option<&str> {
        Some(&self.model)
    }

    fn check_progress(
        &self,
        job_id: &str,
    ) -> Pin<Box<dyn Future<Output = GenerationResult<GenerationProgress>> + Send + '_>> {
        let model_path = self.model.clone();
        let job_id = job_id.to_string();
        Box::pin(async move {
            let st = self.poll_status(&model_path, &job_id).await?;
            // The Fal queue doesn't expose a numeric percent. Report 100%
            // on success, 0% otherwise; carry the status string so callers
            // can render "IN_QUEUE (pos 3)" type UI.
            let pct: f32 = if st.is_success() { 100.0 } else { 0.0 };
            let step = match st.queue_position {
                Some(p) => format!("{} (pos {})", st.status, p),
                None => st.status.clone(),
            };
            Ok(GenerationProgress::new(pct, step))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generation::GenerationParams;

    fn make_provider(types: Vec<GenerationType>) -> FalProvider {
        FalProvider::builder("fal", "fk-test")
            .model("fal-ai/flux/dev")
            .supported_types(types)
            .build()
            .unwrap()
    }

    /// An unset config knob leaves THIS builder's default in place.
    ///
    /// Four builders carry the same setter shape and each one can drift on its
    /// own, so each one is guarded where its field is visible (判据 §16).
    /// Falsification: make `timeout_secs` assign unconditionally; this reds.
    #[test]
    fn an_unset_timeout_keeps_the_builder_default() {
        let unset = FalProvider::builder("fal", "fk-test").timeout_secs(None);
        assert_eq!(unset.timeout_secs, DEFAULT_REQUEST_TIMEOUT_SECS);

        let set = FalProvider::builder("fal", "fk-test").timeout_secs(Some(7));
        assert_eq!(set.timeout_secs, 7);
    }

    #[test]
    fn builder_defaults() {
        let p = FalProvider::builder("fal", "fk-test").build().unwrap();
        assert_eq!(p.name(), "fal");
        assert_eq!(p.default_model(), Some(DEFAULT_MODEL));
        assert_eq!(p.endpoint, DEFAULT_ENDPOINT);
        assert_eq!(p.supported_types(), vec![GenerationType::Image]);
    }

    #[test]
    fn builder_video_supported() {
        let p = make_provider(vec![GenerationType::Video]);
        assert!(p.supports(GenerationType::Video));
        assert!(!p.supports(GenerationType::Image));
    }

    #[test]
    fn submit_url_joins_correctly() {
        let p = make_provider(vec![GenerationType::Image]);
        assert_eq!(
            p.submit_url("fal-ai/flux/dev"),
            "https://queue.fal.run/fal-ai/flux/dev"
        );
        // Leading slash on model path is tolerated.
        assert_eq!(
            p.submit_url("/fal-ai/flux/dev"),
            "https://queue.fal.run/fal-ai/flux/dev"
        );
    }

    #[test]
    fn status_and_result_urls() {
        let p = make_provider(vec![GenerationType::Image]);
        assert_eq!(
            p.status_url("fal-ai/flux/dev", "abc-123"),
            "https://queue.fal.run/fal-ai/flux/dev/requests/abc-123/status"
        );
        assert_eq!(
            p.result_url("fal-ai/flux/dev", "abc-123"),
            "https://queue.fal.run/fal-ai/flux/dev/requests/abc-123"
        );
    }

    #[test]
    fn build_input_carries_known_params() {
        let p = make_provider(vec![GenerationType::Image]);
        let req = GenerationRequest::image("a fox").with_params(
            GenerationParams::builder()
                .width(768)
                .height(1024)
                .seed(42)
                .steps(28)
                .guidance_scale(3.5)
                .aspect_ratio("9:16")
                .build(),
        );
        let input = p.build_input(&req);
        assert_eq!(input["prompt"], "a fox");
        assert_eq!(input["seed"], 42);
        assert_eq!(input["num_inference_steps"], 28);
        assert_eq!(input["guidance_scale"], 3.5);
        assert_eq!(input["aspect_ratio"], "9:16");
        assert_eq!(
            input["image_size"],
            serde_json::json!({"width": 768, "height": 1024})
        );
    }

    #[test]
    fn build_input_carries_extra() {
        let p = make_provider(vec![GenerationType::Video]);
        let mut params = GenerationParams::default();
        params
            .extra
            .insert("cfg_scale".into(), serde_json::json!(7.5));
        params
            .extra
            .insert("scheduler".into(), serde_json::json!("DDIM"));
        let req = GenerationRequest::video("a sunset").with_params(params);
        let input = p.build_input(&req);
        assert_eq!(input["cfg_scale"], 7.5);
        assert_eq!(input["scheduler"], "DDIM");
    }

    #[test]
    fn resolved_model_prefers_request_override() {
        let p = make_provider(vec![GenerationType::Image]);
        let mut req = GenerationRequest::image("x");
        req.params.model = Some("fal-ai/sdxl".into());
        assert_eq!(p.resolved_model(&req), "fal-ai/sdxl");
        // Empty string falls back to default.
        req.params.model = Some("".into());
        assert_eq!(p.resolved_model(&req), "fal-ai/flux/dev");
    }

    #[test]
    fn status_terminal_detection() {
        let s = FalStatusResponse {
            status: "COMPLETED".into(),
            queue_position: None,
        };
        assert!(s.is_terminal());
        assert!(s.is_success());

        let s = FalStatusResponse {
            status: "IN_QUEUE".into(),
            queue_position: Some(2),
        };
        assert!(!s.is_terminal());

        let s = FalStatusResponse {
            status: "FAILED".into(),
            queue_position: None,
        };
        assert!(s.is_terminal());
        assert!(!s.is_success());
    }

    #[test]
    fn extract_primary_url_handles_image_shape() {
        let v = serde_json::json!({"images": [{"url": "https://x/y.png"}], "seed": 1});
        assert_eq!(extract_primary_url(&v), Some("https://x/y.png".into()));
    }

    #[test]
    fn extract_primary_url_handles_video_shape() {
        let v =
            serde_json::json!({"video": {"url": "https://x/y.mp4", "content_type": "video/mp4"}});
        assert_eq!(extract_primary_url(&v), Some("https://x/y.mp4".into()));
    }

    #[test]
    fn extract_primary_url_handles_audio_shape() {
        let v = serde_json::json!({"audio": {"url": "https://x/y.mp3"}});
        assert_eq!(extract_primary_url(&v), Some("https://x/y.mp3".into()));
    }

    #[test]
    fn extract_primary_url_handles_file_shape() {
        let v = serde_json::json!({"file": {"url": "https://x/y.wav"}});
        assert_eq!(extract_primary_url(&v), Some("https://x/y.wav".into()));
    }

    #[test]
    fn extract_primary_url_handles_bare_url() {
        let v = serde_json::json!({"url": "https://x/y.png", "foo": "bar"});
        assert_eq!(extract_primary_url(&v), Some("https://x/y.png".into()));
    }

    #[test]
    fn extract_primary_url_returns_none_for_unknown_shape() {
        let v = serde_json::json!({"detail": "no asset here"});
        assert!(extract_primary_url(&v).is_none());
    }

    #[test]
    fn provider_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<FalProvider>();
    }
}
