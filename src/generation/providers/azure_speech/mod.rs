//! Azure Cognitive Services Speech provider (`GenerationType::Speech`).
//!
//! Implements the short-text REST endpoint
//! `POST https://<region>.tts.speech.microsoft.com/cognitiveservices/v1`.
//!
//! Wire shape:
//!
//! * Auth: `Ocp-Apim-Subscription-Key: <key>` header. (Azure also supports
//!   short-lived Bearer tokens but the subscription key path is what
//!   self-hosted users actually configure.)
//! * Content-Type: `application/ssml+xml`.
//! * Required headers: `X-Microsoft-OutputFormat` (mp3/wav/ogg variants) and
//!   `User-Agent` (Azure rejects requests without one).
//! * Body: SSML XML wrapping the input text in a `<voice>` element.
//! * Response: raw audio bytes in the requested output format.
//!
//! Region routing: `base_url` accepts either a full URL
//! (e.g. `https://eastus.tts.speech.microsoft.com`) or a bare region name
//! (`eastus`). A region is **mandatory** — Azure has no global endpoint —
//! so the provider rejects construction if neither is supplied.
//!
//! Streaming (WebSocket) and long-text batch are deliberately out of scope:
//! both require trait/event-loop extensions beyond `GenerationProvider::generate`.

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

pub use types::{AzureErrorDetail, AzureSpeechError};

const DEFAULT_MODEL: &str = "en-US-JennyNeural";
const DEFAULT_TIMEOUT_SECS: u64 = 60;
/// Azure short-text endpoint caps a single request at 10 minutes of audio.
/// 5000 characters is the documented maximum SSML body size (with safety margin).
const MAX_INPUT_CHARS: usize = 5000;
const USER_AGENT: &str = "Aleph";
const DEFAULT_OUTPUT_FORMAT: &str = "audio-24khz-48kbitrate-mono-mp3";

/// Output formats we expose under the canonical `format` field. The values
/// are the human-friendly aliases; `map_output_format()` resolves to Azure's
/// hyphenated long-form names.
pub const AVAILABLE_FORMATS: [&str; 3] = ["mp3", "wav", "opus"];

/// Azure Speech TTS provider.
#[derive(Clone)]
pub struct AzureSpeechProvider {
    client: Client,
    api_key: String,
    /// Fully qualified `/cognitiveservices/v1` URL.
    endpoint: String,
    /// Default voice (Azure calls this the "voice name").
    voice: String,
    /// Per-request total deadline (the client is built with it; kept for the
    /// timeout-error mapping).
    timeout: Duration,
}

impl std::fmt::Debug for AzureSpeechProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AzureSpeechProvider")
            .field("api_key", &"[REDACTED]")
            .field("endpoint", &self.endpoint)
            .field("voice", &self.voice)
            .finish()
    }
}

impl AzureSpeechProvider {
    /// Construct a provider from an API key, region/base URL, and optional voice.
    ///
    /// `base_url` must either be a full URL (with scheme) **or** a bare
    /// region name (e.g. `eastus`, `westeurope`). A missing region is a
    /// configuration error — Azure has no global endpoint.
    pub fn new<S: Into<String>>(
        api_key: S,
        base_url: Option<String>,
        voice: Option<String>,
    ) -> GenerationResult<Self> {
        let api_key = api_key.into();
        if api_key.trim().is_empty() {
            return Err(GenerationError::authentication(
                "API key cannot be empty",
                "azure-speech",
            ));
        }

        let raw_base = base_url
            .as_ref()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                GenerationError::invalid_parameters(
                    "Azure Speech requires base_url (full URL or region name like 'eastus')",
                    Some("base_url".to_string()),
                )
            })?;

        let endpoint = resolve_endpoint(&raw_base);

        // Shared hardened client (`providers::http`): adds the connect-timeout
        // and stale-keep-alive defenses the openai_tts path already had.
        let timeout = Duration::from_secs(DEFAULT_TIMEOUT_SECS);
        let client = super::http::voice_http_client(timeout)
            .map_err(|e| GenerationError::network(format!("Failed to build HTTP client: {e}")))?;

        Ok(Self {
            client,
            api_key,
            endpoint,
            voice: voice.unwrap_or_else(|| DEFAULT_MODEL.to_string()),
            timeout,
        })
    }

    /// Apply the provider's configured `timeout_seconds` (rebuilds the shared
    /// hardened client with the new per-request cap). Factory-only; existing
    /// call sites keep the 60 s default.
    pub fn with_timeout(mut self, secs: Option<u64>) -> GenerationResult<Self> {
        // `None` = unconfigured; keep this provider's own default.
        let Some(secs) = secs else { return Ok(self) };
        self.timeout = Duration::from_secs(secs.max(1));
        self.client = super::http::voice_http_client(self.timeout)
            .map_err(|e| GenerationError::network(format!("Failed to build HTTP client: {e}")))?;
        Ok(self)
    }

    /// Subset of commonly used Neural voices. Azure publishes 400+ voices —
    /// we surface a representative cross-section here; callers can still pass
    /// any voice name via `params.voice`.
    #[must_use]
    pub fn static_voice_list() -> Vec<VoiceInfo> {
        const VOICES: &[(&str, &str, &str, &str)] = &[
            (
                "en-US-JennyNeural",
                "Jenny",
                "female",
                "US English, conversational",
            ),
            ("en-US-GuyNeural", "Guy", "male", "US English, news anchor"),
            ("en-US-AriaNeural", "Aria", "female", "US English, friendly"),
            ("en-US-DavisNeural", "Davis", "male", "US English, neutral"),
            ("en-GB-SoniaNeural", "Sonia", "female", "UK English"),
            ("en-GB-RyanNeural", "Ryan", "male", "UK English"),
            (
                "zh-CN-XiaoxiaoNeural",
                "Xiaoxiao",
                "female",
                "Mandarin Chinese, cheerful",
            ),
            (
                "zh-CN-YunxiNeural",
                "Yunxi",
                "male",
                "Mandarin Chinese, lively",
            ),
            ("ja-JP-NanamiNeural", "Nanami", "female", "Japanese"),
            ("ja-JP-KeitaNeural", "Keita", "male", "Japanese"),
            ("ko-KR-SunHiNeural", "Sun-Hi", "female", "Korean"),
            ("ko-KR-InJoonNeural", "In-Joon", "male", "Korean"),
            ("fr-FR-DeniseNeural", "Denise", "female", "French"),
            ("fr-FR-HenriNeural", "Henri", "male", "French"),
            ("de-DE-KatjaNeural", "Katja", "female", "German"),
            ("de-DE-ConradNeural", "Conrad", "male", "German"),
            ("es-ES-ElviraNeural", "Elvira", "female", "Spanish (Spain)"),
            ("es-MX-DaliaNeural", "Dalia", "female", "Spanish (Mexico)"),
            (
                "pt-BR-FranciscaNeural",
                "Francisca",
                "female",
                "Brazilian Portuguese",
            ),
            ("it-IT-ElsaNeural", "Elsa", "female", "Italian"),
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
        // Azure usually returns plain text; only some 4xx are JSON envelopes.
        let message = serde_json::from_str::<types::AzureSpeechError>(body)
            .ok()
            .and_then(|e| e.best_message())
            .unwrap_or_else(|| body.to_string());
        match status.as_u16() {
            401 | 403 => GenerationError::authentication(message, "azure-speech"),
            429 => GenerationError::rate_limit(message, None),
            400 => GenerationError::invalid_parameters(message, None),
            500..=599 => GenerationError::provider(message, Some(status.as_u16()), "azure-speech"),
            _ => GenerationError::provider(message, Some(status.as_u16()), "azure-speech"),
        }
    }
}

impl GenerationProvider for AzureSpeechProvider {
    fn generate(
        &self,
        request: GenerationRequest,
    ) -> Pin<Box<dyn Future<Output = GenerationResult<GenerationOutput>> + Send + '_>> {
        Box::pin(async move {
            if request.generation_type != GenerationType::Speech {
                return Err(GenerationError::unsupported_generation_type(
                    request.generation_type.to_string(),
                    "azure-speech",
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
                        "Input too long: {} characters (Azure short-text cap is {} chars)",
                        request.prompt.chars().count(),
                        MAX_INPUT_CHARS
                    ),
                    Some("input".to_string()),
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

            // Voice resolution: voice param > model param (some callers route
            // voice via `model`) > provider default.
            let voice = request
                .params
                .voice
                .clone()
                .or_else(|| request.params.model.clone())
                .unwrap_or_else(|| self.voice.clone());

            let output_format = map_output_format(request.params.format.as_deref());
            let content_type = content_type_for_format(request.params.format.as_deref());

            // Treat the input as SSML if it already starts with `<speak`; this
            // lets advanced users send raw SSML with prosody/breaks. Otherwise
            // wrap plain text in a minimal envelope, honoring `params.speed`
            // via a `<prosody rate>` element.
            let body = build_ssml(&request.prompt, &voice, request.params.speed);

            let started_at = Instant::now();
            debug!(url = %self.endpoint, voice = %voice, format = %output_format, "sending Azure Speech request");

            let response = self
                .client
                .post(&self.endpoint)
                .header("Ocp-Apim-Subscription-Key", &self.api_key)
                .header("Content-Type", "application/ssml+xml")
                .header("X-Microsoft-OutputFormat", output_format)
                .header("User-Agent", USER_AGENT)
                .body(body)
                .send()
                .await
                .map_err(|e| {
                    if e.is_timeout() {
                        GenerationError::timeout(self.timeout)
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
                error!(status = %status, body = %body, "Azure Speech request failed");
                return Err(self.parse_error(status, &body));
            }

            let audio_bytes = response.bytes().await.map_err(|e| {
                GenerationError::network(format!("Failed to read audio bytes: {e}"))
            })?;
            if audio_bytes.is_empty() {
                return Err(GenerationError::provider(
                    "Empty audio response from Azure Speech",
                    None,
                    "azure-speech",
                ));
            }

            let metadata = GenerationMetadata::new()
                .with_provider("azure-speech")
                .with_model(voice.clone())
                .with_duration(started_at.elapsed())
                .with_content_type(content_type)
                .with_size_bytes(u64::try_from(audio_bytes.len()).unwrap_or(u64::MAX));

            info!(
                duration_ms = started_at.elapsed().as_millis(),
                voice = %voice,
                size_bytes = audio_bytes.len(),
                "Azure Speech generation completed"
            );

            let mut output = GenerationOutput::new(
                GenerationType::Speech,
                GenerationData::bytes(audio_bytes.to_vec()),
            )
            .with_metadata(metadata);
            if let Some(id) = request.request_id {
                output = output.with_request_id(id);
            }
            Ok(output)
        })
    }

    fn name(&self) -> &str {
        "azure-speech"
    }

    fn supported_types(&self) -> Vec<GenerationType> {
        vec![GenerationType::Speech]
    }

    fn color(&self) -> &str {
        "#0078d4" // Azure blue
    }

    fn default_model(&self) -> Option<&str> {
        Some(&self.voice)
    }

    fn list_voices(&self) -> Vec<VoiceInfo> {
        Self::static_voice_list()
    }
}

/// Resolve a region name or full URL into the canonical
/// `/cognitiveservices/v1` endpoint.
fn resolve_endpoint(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        if trimmed.contains("/cognitiveservices/v1") {
            trimmed.to_string()
        } else {
            format!("{}/cognitiveservices/v1", trimmed.trim_end_matches('/'))
        }
    } else {
        format!("https://{trimmed}.tts.speech.microsoft.com/cognitiveservices/v1")
    }
}

/// Map our canonical `format` alias onto Azure's hyphenated output-format name.
fn map_output_format(format: Option<&str>) -> &'static str {
    match format {
        Some("mp3") | None => DEFAULT_OUTPUT_FORMAT,
        Some("wav") => "riff-24khz-16bit-mono-pcm",
        Some("opus") => "ogg-24khz-16bit-mono-opus",
        // unreachable: validated by AVAILABLE_FORMATS
        _ => DEFAULT_OUTPUT_FORMAT,
    }
}

fn content_type_for_format(format: Option<&str>) -> &'static str {
    match format {
        Some("mp3") | None => "audio/mpeg",
        Some("wav") => "audio/wav",
        Some("opus") => "audio/ogg",
        _ => "audio/mpeg",
    }
}

/// Speed multiplier bounds for `<prosody rate>`. Azure interprets the rate as a
/// multiplier of the default (1.0 = unchanged); we clamp to the canonical
/// `params.speed` range so an out-of-range value can never produce invalid SSML.
const RATE_MIN: f32 = 0.5;
const RATE_MAX: f32 = 2.0;

fn clamp_rate(speed: f32) -> f32 {
    speed.clamp(RATE_MIN, RATE_MAX)
}

/// Build an SSML envelope around the given text. If the caller already passed
/// raw SSML (input starts with `<speak`), pass it through verbatim — this lets
/// advanced users supply prosody/breaks without us re-wrapping it.
///
/// When `speed` is supplied (and the input is plain text), the text is wrapped
/// in a `<prosody rate>` element so the canonical `params.speed` knob actually
/// changes the speaking rate instead of being silently dropped.
fn build_ssml(input: &str, voice: &str, speed: Option<f32>) -> String {
    if input.trim_start().starts_with("<speak") {
        return input.to_string();
    }
    let xml_lang = locale_from_voice(voice);
    let escaped_input = xml_escape(input);
    let escaped_voice = xml_escape(voice);
    let inner = match speed {
        Some(s) => format!(
            r#"<prosody rate="{rate}">{escaped_input}</prosody>"#,
            rate = clamp_rate(s)
        ),
        None => escaped_input,
    };
    format!(
        r#"<speak version="1.0" xmlns="http://www.w3.org/2001/10/synthesis" xml:lang="{xml_lang}"><voice name="{escaped_voice}">{inner}</voice></speak>"#,
    )
}

/// Pull the locale prefix off an Azure voice name (e.g. `en-US-JennyNeural`
/// → `en-US`). Falls back to `en-US` if the name doesn't follow the
/// `<lang>-<region>-<NameNeural>` convention.
fn locale_from_voice(voice: &str) -> String {
    let parts: Vec<&str> = voice.splitn(3, '-').collect();
    if parts.len() >= 2 {
        format!("{}-{}", parts[0], parts[1])
    } else {
        "en-US".to_string()
    }
}

/// Minimal XML escaping for SSML text payloads. We intentionally avoid pulling
/// in a full XML crate for this — SSML payloads only need the five canonical
/// entities escaped, and the heavy lifting (prosody, breaks) is the caller's
/// responsibility when they pass raw SSML.
fn xml_escape(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}
