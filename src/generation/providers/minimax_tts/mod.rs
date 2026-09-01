//! `MiniMax` text-to-speech provider (`GenerationType::Speech`).
//!
//! Targets the native synchronous `POST /v1/t2a_v2` endpoint. This is **not**
//! OpenAI-compatible — which is exactly why routing `MiniMax` through the
//! generic `openai_compat` provider produces a silent failure (the connection
//! "test" passes, but no playable audio is ever produced):
//!
//! * The request body nests parameters under `voice_setting` / `audio_setting`
//!   rather than `OpenAI`'s flat `{ input, voice, response_format }`.
//! * The response is a JSON envelope, **not** raw audio bytes. The audio
//!   arrives **hex-encoded** in `data.audio` and must be hex-decoded.
//! * Success is signalled by `base_resp.status_code == 0`, not the HTTP status
//!   — a 200 OK with `status_code != 0` is still a logical failure (matching
//!   every other `MiniMax` surface; see the STT provider).
//!
//! Auth: `Authorization: Bearer <key>`. An optional per-account `GroupId` is
//! appended as a query parameter when supplied via `params.extra["group_id"]`
//! (or pre-embedded in `base_url`); accounts whose key is already group-scoped
//! can omit it.
//!
//! Streaming (`stream:true` SSE and the WebSocket surface) and the async
//! `file_id` batch flow are deliberately out of scope: all three require
//! trait/event-loop extensions beyond the single-shot
//! `GenerationProvider::generate` contract — the same boundary `azure_speech`
//! documents.

use crate::generation::{
    GenerationData, GenerationError, GenerationMetadata, GenerationOutput, GenerationProvider,
    GenerationRequest, GenerationResult, GenerationType, VoiceInfo,
};
use reqwest::Client;
use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};
use tracing::{debug, error, info};

#[cfg(test)]
mod tests;
pub mod types;

pub use types::{MinimaxBaseResp, T2aResponse};

/// `api.minimaxi.com` is the host for the `platform.minimaxi.com` console.
/// Accounts on the global `api.minimax.io` endpoint can override `base_url`.
const DEFAULT_ENDPOINT: &str = "https://api.minimaxi.com";
/// `speech-2.8-turbo` is `MiniMax`'s newest low-latency turbo model — the right
/// default for Aleph voice mode (low latency for conversational replies).
const DEFAULT_MODEL: &str = "speech-2.8-turbo";
/// A voice id from the modern `speech-2.x` / `speech-02` taxonomy. The classic
/// `male-qn-*` ids only work with the `speech-01` family, so they are a poor
/// default against `DEFAULT_MODEL`.
const DEFAULT_VOICE: &str = "Chinese (Mandarin)_Gentle_Boy";
const DEFAULT_TIMEOUT_SECS: u64 = 60;
/// Conservative upper bound on a single synchronous request. Voice replies are
/// short (and chunked per sentence upstream); genuinely long-form text should
/// use the async `file_id` flow, which is out of scope here.
const MAX_INPUT_CHARS: usize = 10_000;
const DEFAULT_SAMPLE_RATE: u32 = 32_000;
/// `MiniMax` accepts speaking rates in `[0.5, 2.0]`.
const SPEED_MIN: f32 = 0.5;
const SPEED_MAX: f32 = 2.0;

/// Audio container formats `MiniMax` `audio_setting.format` accepts. `mp3` is
/// the default and what Aleph's voice playback expects.
pub const AVAILABLE_FORMATS: [&str; 3] = ["mp3", "flac", "pcm"];

/// `MiniMax` Text-to-Speech provider.
#[derive(Clone)]
pub struct MinimaxTtsProvider {
    client: Client,
    api_key: String,
    /// Fully qualified `/v1/t2a_v2` URL (without the `GroupId` query parameter,
    /// which is appended per-request when available).
    endpoint: String,
    model: String,
    /// Default voice id.
    voice: String,
}

impl std::fmt::Debug for MinimaxTtsProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MinimaxTtsProvider")
            .field("api_key", &"[REDACTED]")
            .field("endpoint", &self.endpoint)
            .field("model", &self.model)
            .field("voice", &self.voice)
            .finish()
    }
}

impl MinimaxTtsProvider {
    /// Construct a provider from an API key, optional base URL, optional model,
    /// and optional default voice.
    ///
    /// `base_url` may be a bare host (`https://api.minimaxi.com`) or a full
    /// endpoint (`https://api.minimaxi.com/v1/t2a_v2`, optionally with a
    /// `?GroupId=…` query that is preserved verbatim).
    pub fn new<S: Into<String>>(
        api_key: S,
        base_url: Option<String>,
        model: Option<String>,
        voice: Option<String>,
    ) -> GenerationResult<Self> {
        let api_key = api_key.into();
        if api_key.trim().is_empty() {
            return Err(GenerationError::authentication(
                "API key cannot be empty",
                "minimax-tts",
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
        let endpoint = resolve_endpoint(&base);

        Ok(Self {
            client,
            api_key,
            endpoint,
            model: model
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_MODEL.to_string()),
            voice: voice
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_VOICE.to_string()),
        })
    }

    /// Representative slice of `MiniMax` system voices from the modern
    /// `speech-2.x` / `speech-02` taxonomy. The full catalogue lives at
    /// <https://platform.minimaxi.com/docs/faq/system-voice-id>; callers may
    /// pass any voice id via `params.voice` — this list only feeds the UI
    /// picker. (The classic `male-qn-*` ids are intentionally omitted: they are
    /// `speech-01`-only and would mislead users on the default model.)
    #[must_use]
    pub fn static_voice_list() -> Vec<VoiceInfo> {
        const VOICES: &[(&str, &str, &str, &str)] = &[
            (
                "Chinese (Mandarin)_Gentle_Boy",
                "温柔男声",
                "male",
                "Mandarin, gentle male",
            ),
            (
                "Chinese (Mandarin)_Steady_Boy",
                "沉稳男声",
                "male",
                "Mandarin, steady male",
            ),
            (
                "Chinese (Mandarin)_Warm_Girl",
                "温暖女声",
                "female",
                "Mandarin, warm female",
            ),
            (
                "Chinese (Mandarin)_Lively_Girl",
                "活泼女声",
                "female",
                "Mandarin, lively female",
            ),
            (
                "English_expressive_narrator",
                "Expressive Narrator",
                "neutral",
                "English, expressive narrator",
            ),
            (
                "Wise_Woman",
                "Wise Woman",
                "female",
                "English, calm and wise",
            ),
            (
                "Friendly_Person",
                "Friendly Person",
                "neutral",
                "English, warm and friendly",
            ),
            (
                "Deep_Voice_Man",
                "Deep Voice Man",
                "male",
                "English, deep male",
            ),
            ("Calm_Woman", "Calm Woman", "female", "English, calm female"),
            ("Casual_Guy", "Casual Guy", "male", "English, relaxed male"),
            ("Patient_Man", "Patient Man", "male", "English, gentle male"),
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
        // Try the base_resp envelope first; many "errors" come back as 200 OK
        // with status_code != 0.
        if let Ok(parsed) = serde_json::from_str::<T2aResponse>(body) {
            if !parsed.base_resp.is_success() {
                return map_status_code(
                    parsed.base_resp.status_code,
                    parsed.base_resp.best_message(),
                );
            }
        }
        match status.as_u16() {
            401 | 403 => GenerationError::authentication(body.to_string(), "minimax-tts"),
            429 => GenerationError::rate_limit(body.to_string(), None),
            400 => GenerationError::invalid_parameters(body.to_string(), None),
            _ => GenerationError::provider(body.to_string(), Some(status.as_u16()), "minimax-tts"),
        }
    }
}

impl GenerationProvider for MinimaxTtsProvider {
    fn generate(
        &self,
        request: GenerationRequest,
    ) -> Pin<Box<dyn Future<Output = GenerationResult<GenerationOutput>> + Send + '_>> {
        Box::pin(async move {
            if request.generation_type != GenerationType::Speech {
                return Err(GenerationError::unsupported_generation_type(
                    request.generation_type.to_string(),
                    "minimax-tts",
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
                        "Input too long: {} characters (MiniMax synchronous T2A cap is {} chars)",
                        request.prompt.chars().count(),
                        MAX_INPUT_CHARS
                    ),
                    Some("input".to_string()),
                ));
            }

            let format = resolve_format(request.params.format.as_deref())?;

            let model = request
                .params
                .model
                .clone()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| self.model.clone());
            let voice = request
                .params
                .voice
                .clone()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| self.voice.clone());

            let body = build_request(
                &model,
                &request.prompt,
                &voice,
                request.params.speed,
                format,
            );

            // Append the optional per-account GroupId (sourced from params.extra).
            let url = append_group_id(
                &self.endpoint,
                request
                    .params
                    .extra
                    .get("group_id")
                    .and_then(|v| v.as_str()),
            );

            let started_at = Instant::now();
            debug!(url = %self.endpoint, model = %model, voice = %voice, format = %format, "sending MiniMax T2A request");

            let response = self
                .client
                .post(&url)
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
            let text = response.text().await.map_err(|e| {
                GenerationError::network(format!("Failed to read response body: {e}"))
            })?;
            if !status.is_success() {
                error!(status = %status, body = %text, "MiniMax T2A HTTP failure");
                return Err(self.parse_error(status, &text));
            }

            let parsed: T2aResponse = serde_json::from_str(&text).map_err(|e| {
                GenerationError::provider(
                    format!("Failed to parse MiniMax T2A response: {e}"),
                    None,
                    "minimax-tts",
                )
            })?;
            if !parsed.base_resp.is_success() {
                error!(
                    code = parsed.base_resp.status_code,
                    msg = %parsed.base_resp.best_message(),
                    "MiniMax T2A logical failure (HTTP 200, status_code != 0)"
                );
                return Err(self.parse_error(status, &text));
            }

            let audio_hex = parsed
                .data
                .as_ref()
                .and_then(|d| d.audio.as_deref())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    GenerationError::provider(
                        "MiniMax T2A returned no audio payload",
                        None,
                        "minimax-tts",
                    )
                })?;

            // MiniMax returns audio as a HEX string (not base64) — decoding it
            // is the step the openai_compat path was missing.
            let audio_bytes = hex::decode(audio_hex).map_err(|e| {
                GenerationError::provider(
                    format!("Failed to hex-decode MiniMax audio payload: {e}"),
                    None,
                    "minimax-tts",
                )
            })?;
            if audio_bytes.is_empty() {
                return Err(GenerationError::provider(
                    "MiniMax T2A returned an empty audio payload",
                    None,
                    "minimax-tts",
                ));
            }

            let mut metadata = GenerationMetadata::new()
                .with_provider("minimax-tts")
                .with_model(model.clone())
                .with_duration(started_at.elapsed())
                .with_content_type(content_type_for_format(format))
                .with_size_bytes(u64::try_from(audio_bytes.len()).unwrap_or(u64::MAX));
            if let Some(ms) = parsed.extra_info.as_ref().and_then(|e| e.audio_length) {
                #[allow(clippy::cast_precision_loss)]
                let seconds = ms as f32 / 1000.0;
                metadata = metadata.with_duration_seconds(seconds);
            }

            info!(
                duration_ms = started_at.elapsed().as_millis(),
                model = %model,
                voice = %voice,
                size_bytes = audio_bytes.len(),
                "MiniMax T2A generation completed"
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
        "minimax-tts"
    }

    fn supported_types(&self) -> Vec<GenerationType> {
        vec![GenerationType::Speech]
    }

    fn color(&self) -> &str {
        "#1d63ff"
    }

    fn default_model(&self) -> Option<&str> {
        Some(&self.model)
    }

    fn list_voices(&self) -> Vec<VoiceInfo> {
        Self::static_voice_list()
    }
}

/// Normalize a bare host or full endpoint into the canonical `/v1/t2a_v2` URL.
///
/// Preserves an already-complete `/v1/t2a_v2` path (including an embedded
/// `?GroupId=` query). Otherwise strips a trailing `/v1` or `/anthropic`
/// (common when users paste an LLM base URL) before appending the path, so we
/// never produce a doubled `/v1/v1/t2a_v2`.
fn resolve_endpoint(base: &str) -> String {
    let trimmed = base.trim().trim_end_matches('/');
    if trimmed.contains("/v1/t2a_v2") {
        return trimmed.to_string();
    }
    let root = trimmed
        .trim_end_matches("/v1")
        .trim_end_matches("/anthropic")
        .trim_end_matches('/');
    format!("{root}/v1/t2a_v2")
}

/// Append a `GroupId` query parameter when one is supplied and the endpoint
/// doesn't already carry one. `MiniMax` accounts whose API key is already
/// group-scoped can omit it. Pure for unit testing.
///
/// `group_id` originates in `request.params.extra["group_id"]` — an
/// LLM-controlled string. Earlier this was format-interpolated raw into the
/// URL, so a value containing `&` or `#` would either break the URL or
/// silently append attacker-controlled query parameters. Percent-encode the
/// value with a non-userinfo set and additionally validate the charset so a
/// clearly-hostile payload is rejected up front rather than masked by the
/// encoding.
fn append_group_id(endpoint: &str, group_id: Option<&str>) -> String {
    match group_id.map(str::trim).filter(|s| !s.is_empty()) {
        Some(gid) if !endpoint.contains("GroupId=") => {
            if !is_safe_group_id(gid) {
                tracing::warn!(
                    group_id = %gid,
                    "MiniMax group_id contains disallowed characters; ignoring"
                );
                return endpoint.to_string();
            }
            let sep = if endpoint.contains('?') { '&' } else { '?' };
            format!(
                "{endpoint}{sep}GroupId={}",
                percent_encoding::utf8_percent_encode(gid, percent_encoding::NON_ALPHANUMERIC)
            )
        }
        _ => endpoint.to_string(),
    }
}

/// MiniMax group ids are short alphanumeric tokens; reject anything that
/// could double as a query-string fragment (`&`, `=`, `#`) or path separator
/// (`/`) before percent-encoding. This is belt-and-suspenders against a
/// caller passing the raw extra map through.
fn is_safe_group_id(gid: &str) -> bool {
    !gid.is_empty()
        && gid.len() <= 64
        && gid
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Build the `MiniMax` T2A request body. Pure for unit testing.
fn build_request(
    model: &str,
    text: &str,
    voice: &str,
    speed: Option<f32>,
    format: &str,
) -> types::T2aRequest {
    types::T2aRequest {
        model: model.to_string(),
        text: text.to_string(),
        stream: false,
        voice_setting: types::VoiceSetting {
            voice_id: voice.to_string(),
            speed: speed.map(clamp_speed),
        },
        audio_setting: types::AudioSetting {
            format: format.to_string(),
            sample_rate: DEFAULT_SAMPLE_RATE,
        },
    }
}

/// Clamp a requested speaking rate into `MiniMax`'s accepted `[0.5, 2.0]` range.
fn clamp_speed(speed: f32) -> f32 {
    speed.clamp(SPEED_MIN, SPEED_MAX)
}

/// Validate and resolve the requested audio format. Defaults to `mp3`.
fn resolve_format(format: Option<&str>) -> GenerationResult<&'static str> {
    match format {
        None => Ok("mp3"),
        Some("mp3") => Ok("mp3"),
        Some("flac") => Ok("flac"),
        Some("pcm") => Ok("pcm"),
        Some(other) => Err(GenerationError::unsupported_format(
            other.to_string(),
            AVAILABLE_FORMATS.iter().map(|s| (*s).to_string()).collect(),
        )),
    }
}

fn content_type_for_format(format: &str) -> &'static str {
    match format {
        "flac" => "audio/flac",
        "pcm" => "audio/pcm",
        // "mp3" and any unexpected value fall back to mpeg.
        _ => "audio/mpeg",
    }
}

/// Map a `MiniMax` `base_resp.status_code` onto a typed `GenerationError`.
/// Codes mirror the STT provider's mapping (shared `MiniMax` error space).
fn map_status_code(code: i32, msg: String) -> GenerationError {
    match code {
        1004 | 1042 => GenerationError::authentication(msg, "minimax-tts"),
        1008 | 1030 | 1039 => GenerationError::rate_limit(msg, None),
        1000..=1999 => GenerationError::invalid_parameters(msg, None),
        _ => GenerationError::provider(msg, u16::try_from(code).ok(), "minimax-tts"),
    }
}
