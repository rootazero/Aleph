//! Thin providers that bridge core's existing voice seams to the aleph-voice
//! sidecar. Ports are dynamic per spawn, so these resolve (base_url, token)
//! from the supervisor at call time instead of static config.

use std::future::Future;
use std::pin::Pin;

use async_trait::async_trait;

use crate::generation::{
    GenerationData, GenerationError, GenerationOutput, GenerationProvider, GenerationRequest,
    GenerationResult, GenerationType,
};
use crate::media::cache::CachedMedia;
use crate::media::transcription::{TranscriptionResult, TranscriptionService};

use super::sidecar;

/// `TranscriptionService` backed by the sidecar (`MediaProcessor` path).
pub struct LocalTranscription;

#[async_trait]
impl TranscriptionService for LocalTranscription {
    async fn transcribe(
        &self,
        audio: &CachedMedia,
        language: Option<&str>,
    ) -> anyhow::Result<TranscriptionResult> {
        let sup = sidecar::global().ok_or_else(|| {
            anyhow::anyhow!("local voice not initialized (voice.local.enabled?)")
        })?;
        let ep = sup.ensure_endpoint().await?;
        let bytes = tokio::fs::read(&audio.local_path).await?;
        let filename = audio
            .local_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("audio.bin")
            .to_string();
        // Reuse the shared whisper-dialect HTTP core (same multipart shape).
        let cfg = super::inbound::SttConfig {
            api_key: ep.token,
            base_url: ep.base_url,
            model: sup.config().stt_model.clone(),
        };
        let text =
            super::inbound::transcribe_bytes(bytes, &filename, &audio.mime_type, language, &cfg)
                .await
                .map_err(|e| anyhow::anyhow!(e))?;
        Ok(TranscriptionResult {
            text,
            language: None,
        })
    }
}

/// `GenerationProvider` backed by the sidecar (TTS path through the registry).
pub struct LocalVoiceProvider {
    capability: GenerationType,
}

impl LocalVoiceProvider {
    #[must_use]
    pub const fn new(capability: GenerationType) -> Self {
        Self { capability }
    }

    async fn tts(&self, request: GenerationRequest) -> GenerationResult<GenerationOutput> {
        let sup = sidecar::global().ok_or_else(|| {
            GenerationError::provider("local voice not initialized", None, "local")
        })?;
        let ep = sup
            .ensure_endpoint()
            .await
            .map_err(|e| GenerationError::provider(format!("{e:#}"), None, "local"))?;
        let cfg = sup.config();
        let voice = request
            .params
            .voice
            .clone()
            .unwrap_or_else(|| cfg.tts_voice.clone());
        let body = serde_json::json!({
            "model": cfg.tts_model,
            "input": request.prompt,
            "voice": voice,
            "response_format": cfg.tts_format,
        });
        let client = reqwest::Client::new();
        let resp = client
            .post(format!("{}/audio/speech", ep.base_url))
            .bearer_auth(&ep.token)
            .json(&body)
            .timeout(std::time::Duration::from_secs(60))
            .send()
            .await
            .map_err(|e| GenerationError::provider(format!("request: {e}"), None, "local"))?;

        let status = resp.status();
        if status == reqwest::StatusCode::SERVICE_UNAVAILABLE {
            let v: serde_json::Value = resp.json().await.unwrap_or_default();
            let pct = v["percent"].as_u64().unwrap_or(0);
            return Err(GenerationError::provider(
                format!("model downloading ({pct}%)"),
                Some(status.as_u16()),
                "local",
            ));
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(GenerationError::provider(
                format!("HTTP {status}: {body}"),
                Some(status.as_u16()),
                "local",
            ));
        }
        let content_type = match cfg.tts_format.as_str() {
            "wav" => "audio/wav",
            _ => "audio/ogg",
        };
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| GenerationError::provider(format!("body: {e}"), None, "local"))?;
        let mut output =
            GenerationOutput::new(GenerationType::Speech, GenerationData::Bytes(bytes.to_vec()));
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
                    "only speech is served by the local sidecar provider",
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
