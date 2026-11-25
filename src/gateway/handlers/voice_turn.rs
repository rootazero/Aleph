//! Bounded whole-turn RPCs over the configured Core media providers.

use std::collections::{HashMap, HashSet};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use aleph_client::local_ipc::MAX_STRING_BYTES;
use aleph_desktop::{DesktopError, DesktopPlatform};
use aleph_protocol::panel_voice::{
    VoiceErrorKind, MAX_PCM_BYTES, MAX_WAV_BASE64_BYTES, SAMPLE_RATE_HZ, VOICE_TURN_CANCEL_METHOD,
    VOICE_TURN_SYNTHESIZE_METHOD, VOICE_TURN_TRANSCRIBE_METHOD, WAV_HEADER_BYTES,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::{OwnedSemaphorePermit, RwLock};
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use tracing::instrument::WithSubscriber;

use crate::config::Config;
use crate::gateway::handlers::HandlerRegistry;
use crate::gateway::protocol::{
    JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS, PERMISSION_DENIED,
};
use crate::gateway::security::SharedTokenManager;
use crate::gateway::voice::{
    inbound::{resolve_stt_source, transcribe_with_source},
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
    turns: Arc<VoiceTurnScopes>,
    query_permit: Option<Arc<OwnedSemaphorePermit>>,
}

impl VoiceRequestContext {
    pub(crate) fn new(
        ingress: VoiceIngress,
        connection_id: String,
        response_budget: usize,
        cancellation: CancellationToken,
        turns: Arc<VoiceTurnScopes>,
    ) -> Self {
        Self {
            ingress,
            connection_id,
            response_budget,
            cancellation,
            turns,
            query_permit: None,
        }
    }

    pub(crate) fn with_query_permit(mut self, permit: Option<OwnedSemaphorePermit>) -> Self {
        self.query_permit = permit.map(Arc::new);
        self
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

/// Bound cancellation history; overflow closes voice admission for this peer.
const MAX_CANCELLED_TURNS: usize = 32;

/// The provider work of one connection's whole turns, keyed by `turn_id`.
///
/// Each connection owns one table, so the key is `(connection_id, turn_id)`:
/// another connection can neither see nor cancel its turns, and teardown
/// cancels them through the connection token every turn token descends from.
#[derive(Default)]
pub(crate) struct VoiceTurnScopes {
    table: Mutex<TurnTable>,
    offline: TaskTracker,
}

#[derive(Default)]
struct TurnTable {
    active: HashMap<uuid::Uuid, ActiveTurn>,
    cancelled: HashSet<uuid::Uuid>,
    cancellation_overflow: bool,
}

struct ActiveTurn {
    token: CancellationToken,
    /// Admitted requests sharing the token.
    admitted: usize,
}

/// An admitted request's share of its turn. Leaves the table on drop.
struct TurnScope {
    turns: Arc<VoiceTurnScopes>,
    turn_id: uuid::Uuid,
    token: CancellationToken,
}

impl VoiceTurnScopes {
    /// Admission and cancel take the same lock, so a request arriving after
    /// `voice.turn.cancel` cannot start the cancelled turn again.
    fn admit(
        self: &Arc<Self>,
        turn_id: uuid::Uuid,
        connection: &CancellationToken,
    ) -> Result<TurnScope, VoiceErrorKind> {
        let mut table = self.table.lock().unwrap_or_else(PoisonError::into_inner);
        if connection.is_cancelled()
            || table.cancellation_overflow
            || table.cancelled.contains(&turn_id)
        {
            return Err(VoiceErrorKind::Cancelled);
        }
        let turn = table.active.entry(turn_id).or_insert_with(|| ActiveTurn {
            token: connection.child_token(),
            admitted: 0,
        });
        turn.admitted += 1;
        Ok(TurnScope {
            turns: self.clone(),
            turn_id,
            token: turn.token.clone(),
        })
    }

    /// Cancel this connection's work for `turn_id` and refuse it from now on.
    /// `true` only when admitted work was in flight. Remote provider work that
    /// already left this process is not claimed to have stopped.
    fn cancel(&self, turn_id: uuid::Uuid) -> bool {
        let mut table = self.table.lock().unwrap_or_else(PoisonError::into_inner);
        if !table.cancelled.contains(&turn_id) {
            if table.cancelled.len() == MAX_CANCELLED_TURNS {
                // Do not evict an id that a delayed request can still use.
                table.cancellation_overflow = true;
            } else {
                table.cancelled.insert(turn_id);
            }
        }
        if let Some(turn) = table.active.get(&turn_id) {
            if !turn.token.is_cancelled() {
                turn.token.cancel();
                return true;
            }
        }
        false
    }

    #[cfg(windows)]
    pub(crate) fn has_offline(&self) -> bool {
        !self.offline.is_empty()
    }

    #[cfg(any(unix, windows))]
    pub(crate) async fn drain_offline(&self) {
        self.offline.close();
        self.offline.wait().await;
    }
}

impl Drop for TurnScope {
    fn drop(&mut self) {
        let mut table = self
            .turns
            .table
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let Some(turn) = table.active.get_mut(&self.turn_id) else {
            return;
        };
        turn.admitted -= 1;
        if turn.admitted == 0 {
            table.active.remove(&self.turn_id);
        }
    }
}

/// The host's installed on-device recognizer. Reached only when no STT provider
/// is configured and the turn arrived over owner Desktop IPC.
#[derive(Clone)]
pub struct OfflineSpeechAdapter {
    platform: Arc<dyn DesktopPlatform>,
    /// Directory the platform recognizer is permitted to read from.
    scratch_root: PathBuf,
}

impl OfflineSpeechAdapter {
    pub fn new(platform: Arc<dyn DesktopPlatform>, scratch_root: PathBuf) -> Self {
        Self {
            platform,
            scratch_root,
        }
    }

    // A dropped connection waiter must not drop the adapter or its Query slot.
    // The connection cancels the token and drains its tracked work on teardown.
    async fn transcribe(
        &self,
        wav: Vec<u8>,
        language: Option<String>,
        context: &VoiceRequestContext,
    ) -> Result<String, VoiceErrorKind> {
        let cancel = context.cancellation.clone();
        if cancel.is_cancelled() {
            return Err(VoiceErrorKind::Cancelled);
        }
        let adapter = self.clone();
        let query_permit = context.query_permit.clone();
        let result = context
            .turns
            .offline
            .spawn(
                async move {
                    let _query_permit = query_permit;
                    adapter.recognize(&wav, language.as_deref(), cancel).await
                }
                .with_subscriber(tracing::Dispatch::none()),
            )
            .await
            .unwrap_or(Err(VoiceErrorKind::ProviderFailed));
        // Keep the IPC job and its active request slot through adapter release,
        // even when the separate cancel control reply has already been sent.
        if context.cancellation.is_cancelled() {
            return Err(VoiceErrorKind::Cancelled);
        }
        result
    }

    async fn recognize(
        &self,
        wav: &[u8],
        language: Option<&str>,
        cancel: CancellationToken,
    ) -> Result<String, VoiceErrorKind> {
        if cancel.is_cancelled() {
            return Err(VoiceErrorKind::Cancelled);
        }
        let media = self
            .platform
            .media()
            .ok_or(VoiceErrorKind::OfflineUnavailable)?;
        let clip = ScratchClip::stage(&self.scratch_root, wav)
            .map_err(|_| VoiceErrorKind::OfflineUnavailable)?;
        if cancel.is_cancelled() {
            return Err(VoiceErrorKind::Cancelled);
        }
        let result = media
            .speech_to_text_offline(clip.path_str()?, language, cancel)
            .await;
        // Only now has the engine let go of the clip.
        drop(clip);
        result
            .map(|result| result.text)
            .map_err(|error| offline_error_kind(&error))
    }
}

fn offline_error_kind(error: &DesktopError) -> VoiceErrorKind {
    match error {
        DesktopError::NotImplemented(_)
        | DesktopError::NotAvailable(_)
        | DesktopError::BridgeBackoff(_)
        | DesktopError::BridgeDisabled(_) => VoiceErrorKind::OfflineUnavailable,
        DesktopError::UnsupportedLanguage(_) => VoiceErrorKind::UnsupportedLanguage,
        DesktopError::PermissionDenied { .. } => VoiceErrorKind::PermissionDenied,
        _ => VoiceErrorKind::ProviderFailed,
    }
}

/// One private, exclusively created WAV for the offline recognizer; removed on drop.
struct ScratchClip(PathBuf);

impl ScratchClip {
    fn stage(root: &Path, wav: &[u8]) -> io::Result<Self> {
        std::fs::create_dir_all(root)?;
        let path = root.join(format!("voice-turn-{}.wav", uuid::Uuid::new_v4().simple()));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&path)?;
        let clip = Self(path);
        file.write_all(wav)?;
        Ok(clip)
    }

    fn path_str(&self) -> Result<&str, VoiceErrorKind> {
        self.0.to_str().ok_or(VoiceErrorKind::OfflineUnavailable)
    }
}

impl Drop for ScratchClip {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
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
        || !pcm_bytes.is_multiple_of(PCM_SAMPLE_BYTES as usize)
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
    offline: Option<OfflineSpeechAdapter>,
) {
    handlers.register_with_voice_context(VOICE_TURN_TRANSCRIBE_METHOD, move |request, context| {
        let (config, vault, offline) = (config.clone(), vault.clone(), offline.clone());
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
                let cfg = until_cancelled(&context.cancellation, config.read()).await?;
                resolve_stt_source(
                    &cfg.generation,
                    &vault,
                    cfg.voice_local.vocabulary_hint().as_deref(),
                )
            };
            let text = match source {
                // Preserve the resolver's explicitly configured local→cloud
                // sequence; neither failure can enter the installed adapter.
                Some(source) => until_cancelled(
                    &context.cancellation,
                    transcribe_with_source(
                        wav.into(),
                        "turn.wav",
                        "audio/wav",
                        params.language,
                        &source,
                    )
                    .with_subscriber(tracing::Dispatch::none()),
                )
                .await?
                .map_err(|_| VoiceErrorKind::ProviderFailed)?,
                None if context.ingress == VoiceIngress::DesktopIpc => {
                    offline
                        .ok_or(VoiceErrorKind::OfflineUnavailable)?
                        .transcribe(wav, params.language.map(str::to_owned), &context)
                        .await?
                }
                None => return Err(VoiceErrorKind::NotConfigured),
            };
            let text = text.trim();
            if text.is_empty() {
                return Err(VoiceErrorKind::NoSpeech);
            }
            if text.len() > MAX_STRING_BYTES || text.len() > context.response_budget {
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

/// `voice.turn.cancel { turn_id } -> { cancelled }`, for the calling connection's turns only.
pub fn register_cancel(handlers: &mut HandlerRegistry) {
    handlers.register_with_voice_context(VOICE_TURN_CANCEL_METHOD, |request, context| async move {
        #[derive(Deserialize)]
        struct Params {
            turn_id: uuid::Uuid,
        }
        if let Err(kind) = context.check_admission() {
            return voice_error(request.id, kind, &context);
        }
        let Some(params) = request
            .params
            .as_ref()
            .and_then(|value| Params::deserialize(value).ok())
        else {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Invalid turn_id");
        };
        let cancelled = context.turns.cancel(params.turn_id);
        JsonRpcResponse::success(request.id, json!({ "cancelled": cancelled }))
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
            let gen_config = until_cancelled(&context.cancellation, config.read())
                .await?
                .generation
                .clone();
            // Existing providers may log text from upstream error bodies. The
            // whole-turn path exposes only a typed failure, even in telemetry.
            let outcome = until_cancelled(
                &context.cancellation,
                generate_tts_turn(params.text, &registry, &gen_config)
                    .with_subscriber(tracing::Dispatch::none()),
            )
            .await?;
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
    let result = match admit_turn(&request, &context) {
        Ok(scope) => {
            // Work runs under the turn token, which `voice.turn.cancel` and
            // connection teardown both fire.
            let mut turn_context = context.clone();
            if let Some(scope) = &scope {
                turn_context.cancellation = scope.token.clone();
            }
            if turn_context.cancellation.is_cancelled() {
                Err(VoiceErrorKind::Cancelled)
            } else {
                call(request, turn_context).await
            }
        }
        Err(kind) => Err(kind),
    };
    result.unwrap_or_else(|kind| voice_error(id, kind, &context))
}

async fn until_cancelled<F: std::future::Future>(
    cancel: &CancellationToken,
    work: F,
) -> Result<F::Output, VoiceErrorKind> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(VoiceErrorKind::Cancelled),
        result = work => Ok(result),
    }
}

/// `None` for a missing or malformed turn id; the handler's own parse rejects it.
fn admit_turn(
    request: &JsonRpcRequest,
    context: &VoiceRequestContext,
) -> Result<Option<TurnScope>, VoiceErrorKind> {
    context.check_admission()?;
    let Some(turn_id) = request
        .params
        .as_ref()
        .and_then(|params| params.get("turn_id"))
        .and_then(|value| uuid::Uuid::deserialize(value).ok())
    else {
        return Ok(None);
    };
    context
        .turns
        .admit(turn_id, &context.cancellation)
        .map(Some)
}

fn voice_error(
    id: Option<serde_json::Value>,
    kind: VoiceErrorKind,
    context: &VoiceRequestContext,
) -> JsonRpcResponse {
    let code = match kind {
        VoiceErrorKind::PermissionDenied => PERMISSION_DENIED,
        VoiceErrorKind::InvalidAudio | VoiceErrorKind::NoSpeech | VoiceErrorKind::TooLarge => {
            INVALID_PARAMS
        }
        _ => INTERNAL_ERROR,
    };
    let mut error =
        JsonRpcResponse::error_with_data(id, code, "Voice turn failed", json!({ "kind": kind }));
    if context.ingress != VoiceIngress::Legacy && !response_fits(&error, context.response_budget, 0)
    {
        // An enormous id is otherwise enough to make even the error unwritable.
        error.id = None;
    }
    error
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
