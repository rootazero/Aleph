//! Volcengine (ByteDance / Doubao Speech) text-to-speech provider
//! (`GenerationType::Speech`).
//!
//! Targets the native synchronous legacy endpoint
//! `POST https://openspeech.bytedance.com/api/v1/tts`. This is **not**
//! OpenAI-compatible — routing it through the generic `openai_compat` provider
//! produces a silent failure (see [`types`] for the envelope details).
//!
//! ## Credentials (approach A — zero config-schema change)
//!
//! The legacy API needs two credentials plus a cluster:
//!
//! * **token** — the secret, supplied as the provider `api_key`
//!   (`Authorization: Bearer;<token>`, a semicolon, not a space).
//! * **appid** — a non-secret account id, carried in the request body. It is
//!   parsed from the configured `base_url` query string, e.g.
//!   `https://openspeech.bytedance.com?appid=1234567890`. An optional
//!   `&cluster=` overrides the default `volcano_tts`. A request-time
//!   `params.extra["appid"]` overrides the configured one.
//!
//! Streaming (the bidirectional WebSocket surface) and the async
//! BytePlus Seed flow are deliberately out of scope: both require trait/event
//! extensions beyond the single-shot `GenerationProvider::generate` contract —
//! the same boundary `minimax_tts` / `azure_speech` document.

use crate::generation::{
    GenerationData, GenerationError, GenerationMetadata, GenerationOutput, GenerationProvider,
    GenerationRequest, GenerationResult, GenerationType, VoiceInfo,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use reqwest::Client;
use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};
use tracing::{debug, error, info};

#[cfg(test)]
mod tests;
pub mod types;

pub use types::TtsResponse;

/// China-hosted legacy speech endpoint (kept domestic for low latency; the
/// international BytePlus Seed endpoint is deliberately not used).
const DEFAULT_ENDPOINT: &str = "https://openspeech.bytedance.com";
/// Default speaker (a Mandarin female "bigtts" voice). The legacy API has no
/// separate "model" parameter — `voice_type` *is* the model selector.
const DEFAULT_VOICE: &str = "zh_female_cancan_mars_bigtts";
/// Default synthesis cluster for the legacy endpoint.
const DEFAULT_CLUSTER: &str = "volcano_tts";
const DEFAULT_TIMEOUT_SECS: u64 = 60;
/// Conservative upper bound on a single synchronous request. Voice replies are
/// short (and chunked per sentence upstream).
const MAX_INPUT_CHARS: usize = 10_000;
/// Volcengine accepts speaking rates in `[0.2, 3.0]`.
const SPEED_MIN: f32 = 0.2;
const SPEED_MAX: f32 = 3.0;

/// Audio container formats the legacy `audio.encoding` field accepts. `mp3` is
/// the default and what Aleph's voice playback expects.
pub const AVAILABLE_FORMATS: [&str; 4] = ["mp3", "wav", "pcm", "ogg_opus"];

/// Volcengine Text-to-Speech provider.
#[derive(Clone)]
pub struct VolcengineTtsProvider {
    client: Client,
    /// The Volcengine **token** (the secret half of the credential pair).
    token: String,
    /// Non-secret account id parsed from `base_url` (`?appid=…`). Empty when
    /// not configured — `generate()` then fails fast with a clear message.
    appid: String,
    /// Fully qualified `/api/v1/tts` URL (query string stripped).
    endpoint: String,
    /// Synthesis cluster (default `volcano_tts`).
    cluster: String,
    /// Default speaker id (`voice_type`).
    voice: String,
}

impl std::fmt::Debug for VolcengineTtsProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VolcengineTtsProvider")
            .field("token", &"[REDACTED]")
            .field("appid", &self.appid)
            .field("endpoint", &self.endpoint)
            .field("cluster", &self.cluster)
            .field("voice", &self.voice)
            .finish()
    }
}

impl VolcengineTtsProvider {
    /// Construct a provider from a token (`api_key`), optional base URL,
    /// optional model (unused by the legacy API — `voice` is the selector),
    /// and optional default voice.
    ///
    /// `base_url` may be a bare host (`https://openspeech.bytedance.com`), a
    /// host carrying credentials (`…?appid=123&cluster=volcano_tts`), or a full
    /// endpoint (`…/api/v1/tts?appid=123`).
    pub fn new<S: Into<String>>(
        api_key: S,
        base_url: Option<String>,
        _model: Option<String>,
        voice: Option<String>,
    ) -> GenerationResult<Self> {
        let token = api_key.into();
        if token.trim().is_empty() {
            return Err(GenerationError::authentication(
                "Volcengine token (api_key) cannot be empty",
                "volcengine-tts",
            ));
        }

        let client = Client::builder()
            .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
            .build()
            .map_err(|e| GenerationError::network(format!("Failed to build HTTP client: {e}")))?;

        let base = base_url
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| DEFAULT_ENDPOINT.to_string());
        let (endpoint, appid, cluster) = resolve_endpoint(&base);

        Ok(Self {
            client,
            token,
            appid: appid.unwrap_or_default(),
            endpoint,
            cluster: cluster.unwrap_or_else(|| DEFAULT_CLUSTER.to_string()),
            voice: voice
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_VOICE.to_string()),
        })
    }

    /// Representative slice of Volcengine "bigtts" system voices. The full
    /// catalogue lives in the Volcengine console; callers may pass any
    /// `voice_type` via `params.voice` — this list only feeds the UI picker.
    #[must_use]
    pub fn static_voice_list() -> Vec<VoiceInfo> {
        const VOICES: &[(&str, &str, &str, &str)] = &[
            (
                "zh_female_cancan_mars_bigtts",
                "灿灿",
                "female",
                "Mandarin, bright female",
            ),
            (
                "zh_female_qingxinnvsheng_mars_bigtts",
                "清新女声",
                "female",
                "Mandarin, fresh female",
            ),
            (
                "zh_female_linjia_mars_bigtts",
                "邻家女声",
                "female",
                "Mandarin, girl-next-door",
            ),
            (
                "zh_male_wennuanahu_moon_bigtts",
                "温暖阿虎",
                "male",
                "Mandarin, warm male",
            ),
            (
                "zh_male_shaonianzixin_moon_bigtts",
                "少年梓辛",
                "male",
                "Mandarin, youthful male",
            ),
            (
                "zh_female_shuangkuaisisi_moon_bigtts",
                "爽快思思",
                "female",
                "Mandarin, lively female",
            ),
            (
                "en_female_anna_mars_bigtts",
                "Anna",
                "female",
                "English, natural female",
            ),
            (
                "en_male_adam_mars_bigtts",
                "Adam",
                "male",
                "English, natural male",
            ),
        ];
        VOICES
            .iter()
            .map(|(id, name, gender, desc)| VoiceInfo {
                id: (*id).into(),
                name: (*name).into(),
                gender: (*gender).into(),
                description: (*desc).into(),
            })
            .collect()
    }

    fn parse_error(&self, status: reqwest::StatusCode, body: &str) -> GenerationError {
        // Try the envelope first; logical failures arrive as 200 OK with a
        // non-3000 code.
        if let Ok(parsed) = serde_json::from_str::<TtsResponse>(body) {
            if !parsed.is_success() {
                return map_code(parsed.code, parsed.best_message());
            }
        }
        match status.as_u16() {
            401 | 403 => GenerationError::authentication(body.to_string(), "volcengine-tts"),
            429 => GenerationError::rate_limit(body.to_string(), None),
            400 => GenerationError::invalid_parameters(body.to_string(), None),
            _ => {
                GenerationError::provider(body.to_string(), Some(status.as_u16()), "volcengine-tts")
            }
        }
    }
}

impl GenerationProvider for VolcengineTtsProvider {
    fn generate(
        &self,
        request: GenerationRequest,
    ) -> Pin<Box<dyn Future<Output = GenerationResult<GenerationOutput>> + Send + '_>> {
        Box::pin(async move {
            if request.generation_type != GenerationType::Speech {
                return Err(GenerationError::unsupported_generation_type(
                    request.generation_type.to_string(),
                    "volcengine-tts",
                ));
            }
            if request.prompt.trim().is_empty() {
                return Err(GenerationError::invalid_parameters(
                    "Input text cannot be empty",
                    Some("input".to_string()),
                ));
            }
            if request.prompt.chars().count() > MAX_INPUT_CHARS {
                return Err(GenerationError::invalid_parameters(
                    format!(
                        "Input too long: {} characters (Volcengine synchronous TTS cap is {} chars)",
                        request.prompt.chars().count(),
                        MAX_INPUT_CHARS
                    ),
                    Some("input".to_string()),
                ));
            }

            // appid: configured via base_url, overridable per-request.
            let appid = request
                .params
                .extra
                .get("appid")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map_or_else(|| self.appid.clone(), ToString::to_string);
            if appid.is_empty() {
                return Err(GenerationError::authentication(
                    "Volcengine appid missing — append `?appid=<your-appid>` to the provider \
                     base_url (e.g. https://openspeech.bytedance.com?appid=1234567890)",
                    "volcengine-tts",
                ));
            }

            let encoding = resolve_format(request.params.format.as_deref())?;
            let voice = request
                .params
                .voice
                .clone()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| self.voice.clone());

            let body = build_request(
                &appid,
                &self.token,
                &self.cluster,
                &voice,
                encoding,
                request.params.speed,
                &request.prompt,
            );

            let started_at = Instant::now();
            debug!(url = %self.endpoint, voice = %voice, encoding = %encoding, "sending Volcengine TTS request");

            let response = self
                .client
                .post(&self.endpoint)
                // NOTE: the semicolon after `Bearer` is required by the API.
                .header("Authorization", format!("Bearer;{}", self.token))
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
            let text = response.text().await.map_err(|e| {
                GenerationError::network(format!("Failed to read response body: {e}"))
            })?;
            if !status.is_success() {
                error!(status = %status, body = %text, "Volcengine TTS HTTP failure");
                return Err(self.parse_error(status, &text));
            }

            let parsed: TtsResponse = serde_json::from_str(&text).map_err(|e| {
                GenerationError::provider(
                    format!("Failed to parse Volcengine TTS response: {e}"),
                    None,
                    "volcengine-tts",
                )
            })?;
            if !parsed.is_success() {
                error!(
                    code = ?parsed.code,
                    msg = %parsed.best_message(),
                    "Volcengine TTS logical failure (HTTP 200, code != 3000)"
                );
                return Err(self.parse_error(status, &text));
            }

            let audio_b64 = parsed
                .data
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    GenerationError::provider(
                        "Volcengine TTS returned no audio payload",
                        None,
                        "volcengine-tts",
                    )
                })?;

            // Volcengine returns audio as base64 (MiniMax uses hex).
            let audio_bytes = BASE64.decode(audio_b64).map_err(|e| {
                GenerationError::provider(
                    format!("Failed to base64-decode Volcengine audio payload: {e}"),
                    None,
                    "volcengine-tts",
                )
            })?;
            if audio_bytes.is_empty() {
                return Err(GenerationError::provider(
                    "Volcengine TTS returned an empty audio payload",
                    None,
                    "volcengine-tts",
                ));
            }

            let metadata = GenerationMetadata::new()
                .with_provider("volcengine-tts")
                .with_duration(started_at.elapsed())
                .with_content_type(content_type_for_format(encoding))
                .with_size_bytes(u64::try_from(audio_bytes.len()).unwrap_or(u64::MAX));

            info!(
                duration_ms = started_at.elapsed().as_millis(),
                voice = %voice,
                size_bytes = audio_bytes.len(),
                "Volcengine TTS generation completed"
            );

            let mut output =
                GenerationOutput::new(GenerationType::Speech, GenerationData::bytes(audio_bytes))
                    .with_metadata(metadata);
            if let Some(id) = request.request_id {
                output = output.with_request_id(id);
            }
            Ok(output)
        })
    }

    fn name(&self) -> &str {
        "volcengine-tts"
    }

    fn supported_types(&self) -> Vec<GenerationType> {
        vec![GenerationType::Speech]
    }

    fn color(&self) -> &str {
        "#0052D9"
    }

    fn list_voices(&self) -> Vec<VoiceInfo> {
        Self::static_voice_list()
    }
}

/// Split a configured base URL into `(clean endpoint, appid, cluster)`.
///
/// `appid` and `cluster` ride the query string (approach A — no config-schema
/// change). The returned endpoint always ends in `/api/v1/tts` with the query
/// stripped. Pure for unit testing.
fn resolve_endpoint(base: &str) -> (String, Option<String>, Option<String>) {
    let trimmed = base.trim();
    let (path_part, query) = trimmed
        .split_once('?')
        .map_or((trimmed, None), |(p, q)| (p, Some(q)));

    let mut appid = None;
    let mut cluster = None;
    if let Some(q) = query {
        for kv in q.split('&') {
            if let Some((k, v)) = kv.split_once('=') {
                match k {
                    "appid" => appid = Some(v.trim().to_string()),
                    "cluster" => cluster = Some(v.trim().to_string()),
                    _ => {}
                }
            }
        }
    }

    let path_part = path_part.trim().trim_end_matches('/');
    let endpoint = if path_part.contains("/api/v1/tts") {
        path_part.to_string()
    } else {
        format!("{path_part}/api/v1/tts")
    };
    (
        endpoint,
        appid.filter(|s| !s.is_empty()),
        cluster.filter(|s| !s.is_empty()),
    )
}

/// Build the Volcengine legacy TTS request body. Pure for unit testing.
#[allow(clippy::too_many_arguments)]
fn build_request(
    appid: &str,
    token: &str,
    cluster: &str,
    voice: &str,
    encoding: &str,
    speed: Option<f32>,
    text: &str,
) -> types::TtsRequest {
    types::TtsRequest {
        app: types::AppConfig {
            appid: appid.to_string(),
            token: token.to_string(),
            cluster: cluster.to_string(),
        },
        user: types::UserConfig {
            uid: "aleph".to_string(),
        },
        audio: types::AudioConfig {
            voice_type: voice.to_string(),
            encoding: encoding.to_string(),
            speed_ratio: speed.map(clamp_speed),
        },
        request: types::RequestConfig {
            reqid: uuid::Uuid::new_v4().to_string(),
            text: text.to_string(),
            text_type: "plain".to_string(),
            operation: "query".to_string(),
        },
    }
}

/// Clamp a requested speaking rate into Volcengine's accepted `[0.2, 3.0]`.
fn clamp_speed(speed: f32) -> f32 {
    speed.clamp(SPEED_MIN, SPEED_MAX)
}

/// Validate and resolve the requested audio format. Defaults to `mp3`.
fn resolve_format(format: Option<&str>) -> GenerationResult<&'static str> {
    match format {
        None | Some("mp3") => Ok("mp3"),
        Some("wav") => Ok("wav"),
        Some("pcm") => Ok("pcm"),
        Some("ogg_opus") => Ok("ogg_opus"),
        Some(other) => Err(GenerationError::unsupported_format(
            other.to_string(),
            AVAILABLE_FORMATS.iter().map(|s| (*s).to_string()).collect(),
        )),
    }
}

fn content_type_for_format(format: &str) -> &'static str {
    match format {
        "wav" => "audio/wav",
        "pcm" => "audio/pcm",
        "ogg_opus" => "audio/ogg",
        // "mp3" and any unexpected value fall back to mpeg.
        _ => "audio/mpeg",
    }
}

/// Map a Volcengine response `code` onto a typed `GenerationError`.
fn map_code(code: Option<i32>, msg: String) -> GenerationError {
    match code {
        // 3001 invalid request / 3003 concurrency exceeded handled below by range.
        Some(3003 | 3005) => GenerationError::rate_limit(msg, None),
        Some(3000) => GenerationError::provider(msg, None, "volcengine-tts"),
        Some(c @ 3001..=3099) => {
            GenerationError::invalid_parameters(msg, Some(format!("code={c}")))
        }
        Some(c) => GenerationError::provider(msg, u16::try_from(c).ok(), "volcengine-tts"),
        None => GenerationError::provider(msg, None, "volcengine-tts"),
    }
}
