//! Bounded whole-turn RPCs over the configured Core media providers.

use std::io::{self, Write};

use aleph_protocol::panel_voice::{
    VoiceErrorKind, MAX_PCM_BYTES, MAX_WAV_BASE64_BYTES, SAMPLE_RATE_HZ,
    VOICE_TURN_SYNTHESIZE_METHOD, VOICE_TURN_TRANSCRIBE_METHOD, WAV_HEADER_BYTES,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::instrument::WithSubscriber;

use crate::config::Config;
use crate::gateway::handlers::HandlerRegistry;
use crate::gateway::protocol::{
    JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS, PERMISSION_DENIED,
};
use crate::gateway::security::SharedTokenManager;
use crate::gateway::voice::{
    inbound::{resolve_stt_source, transcribe_with_source, SttSource},
    outbound::{generate_tts_turn, TtsOutcome},
};
use crate::generation::GenerationProviderRegistry;
use crate::sync_primitives::Arc;

const PCM_SAMPLE_BYTES: u32 = std::mem::size_of::<i16>() as u32;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum VoiceIngress {
    #[default]
    Legacy,
    DesktopIpc,
    WebV1,
}

/// Minted from live transport authority, never deserialized from the RPC.
#[derive(Clone, Default)]
pub(crate) struct VoiceRequestContext {
    ingress: VoiceIngress,
    connection_id: String,
    response_budget: usize,
    cancellation: CancellationToken,
}

impl VoiceRequestContext {
    pub(crate) fn new(
        ingress: VoiceIngress,
        connection_id: String,
        response_budget: usize,
        cancellation: CancellationToken,
    ) -> Self {
        Self {
            ingress,
            connection_id,
            response_budget,
            cancellation,
        }
    }

    pub(crate) fn reserve_response_bytes(&mut self, bytes: usize) {
        self.response_budget = self.response_budget.saturating_sub(bytes);
    }

    fn check_admission(&self) -> Result<(), VoiceErrorKind> {
        if self.ingress == VoiceIngress::Legacy || self.connection_id.is_empty() {
            return Err(VoiceErrorKind::PermissionDenied);
        }
        if self.cancellation.is_cancelled() {
            return Err(VoiceErrorKind::Cancelled);
        }
        Ok(())
    }
}

/// Accept only the whole-turn wire format, not arbitrary RIFF chunks or legacy media.
fn decode_voice_wav(input: &str) -> Result<Vec<u8>, VoiceErrorKind> {
    if input.len() > MAX_WAV_BASE64_BYTES {
        return Err(VoiceErrorKind::TooLarge);
    }
    let wav = STANDARD
        .decode(input)
        .map_err(|_| VoiceErrorKind::InvalidAudio)?;
    let header = wav
        .get(..WAV_HEADER_BYTES)
        .ok_or(VoiceErrorKind::InvalidAudio)?;
    let read_u32 = |offset| u32::from_le_bytes(header[offset..offset + 4].try_into().unwrap());
    let read_u16 = |offset| u16::from_le_bytes(header[offset..offset + 2].try_into().unwrap());
    let pcm_bytes = wav.len() - WAV_HEADER_BYTES;
    if pcm_bytes > MAX_PCM_BYTES {
        return Err(VoiceErrorKind::TooLarge);
    }
    if &header[..4] != b"RIFF"
        || read_u32(4) as usize != wav.len() - 8
        || &header[8..16] != b"WAVEfmt "
        || read_u32(16) != 16
        || &header[20..24] != b"\x01\0\x01\0" // PCM format, mono
        || read_u32(24) != SAMPLE_RATE_HZ
        || read_u32(28) != SAMPLE_RATE_HZ * PCM_SAMPLE_BYTES
        || u32::from(read_u16(32)) != PCM_SAMPLE_BYTES
        || read_u16(34) != i16::BITS as u16
        || &header[36..40] != b"data"
        || read_u32(40) as usize != pcm_bytes
        || pcm_bytes % PCM_SAMPLE_BYTES as usize != 0
    {
        return Err(VoiceErrorKind::InvalidAudio);
    }
    if pcm_bytes == 0 {
        return Err(VoiceErrorKind::NoSpeech);
    }
    Ok(wav)
}

pub fn register_transcription(
    handlers: &mut HandlerRegistry,
    config: Arc<RwLock<Config>>,
    vault: Arc<SharedTokenManager>,
) {
    handlers.register_with_voice_context(VOICE_TURN_TRANSCRIBE_METHOD, move |request, context| {
        let (config, vault) = (config.clone(), vault.clone());
        bounded_handler(request, context, move |request, context| async move {
            #[derive(Deserialize)]
            struct Params<'a> {
                #[serde(rename = "turn_id")]
                _turn_id: uuid::Uuid,
                audio_base64: &'a str,
                language: Option<&'a str>,
            }
            let params = request
                .params
                .as_ref()
                .and_then(|value| Params::deserialize(value).ok())
                .ok_or(VoiceErrorKind::InvalidAudio)?;
            let wav = decode_voice_wav(params.audio_base64)?;
            let source = {
                let cfg = config.read().await;
                resolve_stt_source(
                    &cfg.generation,
                    &vault,
                    cfg.voice_local.vocabulary_hint().as_deref(),
                )
            };
            // A configured BYO endpoint is a provider, not an installed offline
            // adapter. Do not resend the utterance to any other source on failure.
            let source = match source {
                Some(SttSource::Local { config, .. }) => SttSource::Local {
                    config,
                    fallback: None,
                },
                Some(source) => source,
                None if context.ingress == VoiceIngress::DesktopIpc => {
                    return Err(VoiceErrorKind::OfflineUnavailable)
                }
                None => return Err(VoiceErrorKind::NotConfigured),
            };
            let text = transcribe_with_source(
                wav.into(),
                "turn.wav",
                "audio/wav",
                params.language,
                &source,
            )
            .with_subscriber(tracing::Dispatch::none())
            .await
            .map_err(|_| VoiceErrorKind::ProviderFailed)?;
            let text = text.trim();
            if text.is_empty() {
                return Err(VoiceErrorKind::NoSpeech);
            }
            if text.len() > context.response_budget {
                return Err(VoiceErrorKind::TooLarge);
            }
            let response = JsonRpcResponse::success(request.id, json!({ "text": text }));
            if !response_fits(&response, context.response_budget, 0) {
                return Err(VoiceErrorKind::TooLarge);
            }
            Ok(response)
        })
    });
}

type SharedGenerationRegistry = Arc<crate::sync_primitives::RwLock<GenerationProviderRegistry>>;

pub fn register_synthesis(
    handlers: &mut HandlerRegistry,
    config: Arc<RwLock<Config>>,
    generation_registry: Option<SharedGenerationRegistry>,
) {
    handlers.register_with_voice_context(VOICE_TURN_SYNTHESIZE_METHOD, move |request, context| {
        let (config, generation_registry) = (config.clone(), generation_registry.clone());
        bounded_handler(request, context, move |request, context| async move {
            #[derive(Deserialize)]
            struct Params<'a> {
                #[serde(rename = "turn_id")]
                _turn_id: uuid::Uuid,
                text: &'a str,
            }
            let params = request
                .params
                .as_ref()
                .and_then(|value| Params::deserialize(value).ok())
                .ok_or(VoiceErrorKind::NoSpeech)?;
            let registry = generation_registry.ok_or(VoiceErrorKind::NotConfigured)?;
            let registry = super::voice::snapshot_generation_registry(&registry);
            let gen_config = config.read().await.generation.clone();
            // Existing providers may log text from upstream error bodies. The
            // whole-turn path exposes only a typed failure, even in telemetry.
            let outcome = generate_tts_turn(params.text, &registry, &gen_config)
                .with_subscriber(tracing::Dispatch::none())
                .await;
            let attachment = match outcome {
                Some(TtsOutcome::Generated(attachment)) => attachment,
                Some(TtsOutcome::Skipped) => return Err(VoiceErrorKind::NoSpeech),
                Some(TtsOutcome::Failed) => return Err(VoiceErrorKind::ProviderFailed),
                None => return Err(VoiceErrorKind::NotConfigured),
            };
            let bytes = attachment
                .data
                .filter(|bytes| !bytes.is_empty())
                .ok_or(VoiceErrorKind::UnsupportedOutput)?;
            let mime = match attachment.mime_type.as_str() {
                "audio/mpeg" | "audio/mp3" => "audio/mpeg",
                "audio/wav" | "audio/x-wav" => "audio/wav",
                _ => return Err(VoiceErrorKind::UnsupportedOutput),
            };
            let base64_size =
                base64::encoded_len(bytes.len(), true).ok_or(VoiceErrorKind::TooLarge)?;
            let mut response = JsonRpcResponse::success(
                request.id,
                json!({ "audio_base64": "", "mime_type": mime }),
            );
            // Base64 adds no JSON escaping. Size the actual id and result
            // envelope before allocating either the encoded bytes or body.
            if !response_fits(&response, context.response_budget, base64_size) {
                return Err(VoiceErrorKind::TooLarge);
            }
            response.result.as_mut().unwrap()["audio_base64"] = STANDARD.encode(bytes).into();
            Ok(response)
        })
    });
}

async fn bounded_handler<F, Fut>(
    request: JsonRpcRequest,
    context: VoiceRequestContext,
    call: F,
) -> JsonRpcResponse
where
    F: FnOnce(JsonRpcRequest, VoiceRequestContext) -> Fut,
    Fut: std::future::Future<Output = Result<JsonRpcResponse, VoiceErrorKind>>,
{
    let id = request.id.clone();
    let result = match context.check_admission() {
        Ok(()) => tokio::select! {
            biased;
            _ = context.cancellation.cancelled() => Err(VoiceErrorKind::Cancelled),
            outcome = call(request, context.clone()) => outcome,
        },
        Err(kind) => Err(kind),
    };
    result.unwrap_or_else(|kind| {
        let code = match kind {
            VoiceErrorKind::PermissionDenied => PERMISSION_DENIED,
            VoiceErrorKind::InvalidAudio | VoiceErrorKind::NoSpeech | VoiceErrorKind::TooLarge => {
                INVALID_PARAMS
            }
            _ => INTERNAL_ERROR,
        };
        let mut error = JsonRpcResponse::error_with_data(
            id,
            code,
            "Voice turn failed",
            json!({ "kind": kind }),
        );
        if context.ingress != VoiceIngress::Legacy
            && !response_fits(&error, context.response_budget, 0)
        {
            // An enormous id is otherwise enough to make even the error unwritable.
            error.id = None;
        }
        error
    })
}

fn response_fits(response: &JsonRpcResponse, budget: usize, extra_bytes: usize) -> bool {
    struct Budget(usize);
    impl Write for Budget {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0 = self
                .0
                .checked_sub(bytes.len())
                .ok_or_else(|| io::Error::other("response budget exceeded"))?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    budget
        .checked_sub(extra_bytes)
        .is_some_and(|remaining| serde_json::to_writer(Budget(remaining), response).is_ok())
}
