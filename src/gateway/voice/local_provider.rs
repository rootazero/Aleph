//! Thin providers for the BYO local voice endpoint (OpenAI-compatible server
//! the user runs themselves, e.g. mlx-audio). All connection values come from
//! the provider's own `GenerationProviderConfig` (populated by
//! `normalize_voice_local`): `base_url` = endpoint, `api_key` (empty = no
//! Authorization header), model/voice/format only sent when configured so the
//! server's own defaults win.

use std::future::Future;
use std::pin::Pin;

use async_trait::async_trait;

use crate::config::GenerationProviderConfig;
use crate::generation::{
    GenerationData, GenerationError, GenerationOutput, GenerationProvider, GenerationRequest,
    GenerationResult, GenerationType,
};
use crate::media::cache::CachedMedia;
use crate::media::transcription::{TranscriptionResult, TranscriptionService};

/// `TranscriptionService` backed by the BYO endpoint (`MediaProcessor` path).
pub struct LocalTranscription {
    cfg: super::inbound::SttConfig,
    /// Reused across requests (connection pooling); per-request timeouts are
    /// set on each request.
    client: reqwest::Client,
}

impl LocalTranscription {
    /// Build from the "local" transcription provider entry.
    #[must_use]
    pub fn from_config(config: &GenerationProviderConfig) -> Self {
        Self {
            cfg: super::inbound::SttConfig {
                api_key: config.api_key.clone().unwrap_or_default(),
                base_url: local_base_url(config),
                model: config.models.first().cloned().unwrap_or_default(),
                // No vocabulary bias here: this is the media-pipeline
                // `TranscriptionService` (attachment transcription), built from a
                // `GenerationProviderConfig` alone with no `[voice]` handle in
                // reach. The voice-conversation paths carry it.
                vocabulary: String::new(),
            },
            client: voice_client(config.timeout_seconds),
        }
    }
}

/// Shared hardened client (`generation::providers::http`) honoring the
/// entry's `timeout_seconds`; a builder failure degrades to the stock client
/// (P7 — never fail construction over an HTTP-client option).
fn voice_client(timeout_seconds: u64) -> reqwest::Client {
    crate::generation::providers::http::voice_http_client(std::time::Duration::from_secs(
        timeout_seconds.max(1),
    ))
    .unwrap_or_default()
}

#[async_trait]
impl TranscriptionService for LocalTranscription {
    async fn transcribe(
        &self,
        audio: &CachedMedia,
        language: Option<&str>,
    ) -> anyhow::Result<TranscriptionResult> {
        let bytes = tokio::fs::read(&audio.local_path).await?;
        let filename = audio
            .local_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("audio.bin")
            .to_string();
        // Reuse the shared whisper-dialect HTTP core (same multipart shape).
        let text = super::inbound::transcribe_bytes(
            &self.client,
            bytes::Bytes::from(bytes),
            &filename,
            &audio.mime_type,
            language,
            &self.cfg,
        )
        .await
        .map_err(|e| anyhow::anyhow!(e))?;
        Ok(TranscriptionResult {
            text,
            language: None,
        })
    }
}

/// Endpoint base URL from the entry, trailing slash trimmed. The configured
/// endpoint already carries its path prefix (e.g. `/v1`) — no URL rewriting.
fn local_base_url(config: &GenerationProviderConfig) -> String {
    config
        .base_url
        .as_deref()
        .unwrap_or("http://127.0.0.1:8000/v1")
        .trim_end_matches('/')
        .to_string()
}

/// `GenerationProvider` backed by the BYO endpoint (TTS path through the registry).
pub struct LocalVoiceProvider {
    capability: GenerationType,
    base_url: String,
    /// Empty = unauthenticated endpoint, no Authorization header sent.
    api_key: String,
    /// Empty = omit "model" from the request (server default).
    model: String,
    /// Empty = omit "voice" from the request (server default).
    voice: String,
    /// Empty = omit "response_format" from the request (server default).
    format: String,
    /// Reused across requests (connection pooling); per-request timeouts are
    /// set on each request.
    client: reqwest::Client,
}

impl LocalVoiceProvider {
    /// Build from the "local" speech provider entry.
    #[must_use]
    pub fn from_config(capability: GenerationType, config: &GenerationProviderConfig) -> Self {
        Self {
            capability,
            base_url: local_base_url(config),
            api_key: config.api_key.clone().unwrap_or_default(),
            model: config.models.first().cloned().unwrap_or_default(),
            voice: config.defaults.voice.clone().unwrap_or_default(),
            format: config.defaults.format.clone().unwrap_or_default(),
            client: voice_client(config.timeout_seconds),
        }
    }

    async fn tts(&self, request: GenerationRequest) -> GenerationResult<GenerationOutput> {
        let voice = request
            .params
            .voice
            .clone()
            .unwrap_or_else(|| self.voice.clone());
        let format = request
            .params
            .format
            .clone()
            .unwrap_or_else(|| self.format.clone());

        // Only carry the fields that are actually configured — the BYO server's
        // own defaults decide the rest.
        let mut body = serde_json::json!({ "input": request.prompt });
        if !self.model.is_empty() {
            body["model"] = serde_json::Value::String(self.model.clone());
        }
        if !voice.is_empty() {
            body["voice"] = serde_json::Value::String(voice);
        }
        if !format.is_empty() {
            body["response_format"] = serde_json::Value::String(format.clone());
        }

        // No per-request timeout: the client-level deadline (built from the
        // entry's `timeout_seconds`) governs — a per-request 60 s here would
        // silently override a user-raised cap for slow BYO synth.
        let mut req = self
            .client
            .post(format!("{}/audio/speech", self.base_url))
            .json(&body);
        if !self.api_key.is_empty() {
            req = req.bearer_auth(&self.api_key);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| GenerationError::provider(format!("request: {e}"), None, "local"))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(GenerationError::provider(
                format!("HTTP {status}: {body}"),
                Some(status.as_u16()),
                "local",
            ));
        }
        let content_type = match format.as_str() {
            "wav" => "audio/wav",
            "mp3" => "audio/mpeg",
            _ => "audio/ogg",
        };
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| GenerationError::provider(format!("body: {e}"), None, "local"))?;
        let mut output = GenerationOutput::new(
            GenerationType::Speech,
            GenerationData::Bytes(bytes.to_vec()),
        );
        output.metadata.content_type = Some(content_type.to_string());
        Ok(output)
    }
}

impl GenerationProvider for LocalVoiceProvider {
    fn generate(
        &self,
        request: GenerationRequest,
    ) -> Pin<Box<dyn Future<Output = GenerationResult<GenerationOutput>> + Send + '_>> {
        Box::pin(async move {
            match request.generation_type {
                GenerationType::Speech => self.tts(request).await,
                other => Err(GenerationError::unsupported_feature(
                    "only speech is served by the local voice provider",
                    format!("{other:?}"),
                    "local",
                )),
            }
        })
    }

    fn name(&self) -> &str {
        "local"
    }

    fn supported_types(&self) -> Vec<GenerationType> {
        vec![self.capability]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_config_reads_entry_values() {
        let mut cfg = GenerationProviderConfig::new("local");
        cfg.base_url = Some("http://127.0.0.1:9000/v1/".into());
        cfg.api_key = Some("sk-byo".into());
        cfg.models = vec!["qwen3-tts".into()];
        cfg.defaults.voice = Some("vivian".into());
        cfg.defaults.format = Some("wav".into());

        let p = LocalVoiceProvider::from_config(GenerationType::Speech, &cfg);
        assert_eq!(p.base_url, "http://127.0.0.1:9000/v1");
        assert_eq!(p.api_key, "sk-byo");
        assert_eq!(p.model, "qwen3-tts");
        assert_eq!(p.voice, "vivian");
        assert_eq!(p.format, "wav");
    }

    #[test]
    fn from_config_empty_entry_means_server_defaults() {
        let cfg = GenerationProviderConfig::new("local");
        let p = LocalVoiceProvider::from_config(GenerationType::Speech, &cfg);
        assert_eq!(p.base_url, "http://127.0.0.1:8000/v1");
        assert!(p.api_key.is_empty());
        assert!(p.model.is_empty());
        assert!(p.voice.is_empty());
        assert!(p.format.is_empty());
    }
}
