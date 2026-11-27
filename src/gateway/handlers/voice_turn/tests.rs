use std::{
    future::Future,
    pin::Pin,
    sync::atomic::{AtomicUsize, Ordering},
};

use crate::gateway::handlers::HandlerRegistry;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse};
use crate::gateway::security::{SecurityStore, SharedTokenManager};
use crate::generation::{
    GenerationData, GenerationError, GenerationOutput, GenerationProvider,
    GenerationProviderRegistry, GenerationRequest, GenerationResult, GenerationType,
};
use aleph_protocol::panel_voice::{MAX_UTTERANCE_SAMPLES, SAMPLE_RATE_HZ};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use tokio::sync::RwLock;

use super::voice_turn::{self, VoiceIngress, VoiceRequestContext};
use crate::{sync_primitives::Arc, Config};

const TURN_ID: &str = "1b63aa6b-0688-4be3-991f-e6994e0f795c";

fn wav(samples: usize) -> Vec<u8> {
    let len = 2 * samples;
    let mut wav = Vec::with_capacity(44 + len);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + len as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt \x10\0\0\0\x01\0\x01\0");
    wav.extend_from_slice(&SAMPLE_RATE_HZ.to_le_bytes());
    wav.extend_from_slice(&(2 * SAMPLE_RATE_HZ).to_le_bytes());
    wav.extend_from_slice(b"\x02\0\x10\0data");
    wav.extend_from_slice(&(len as u32).to_le_bytes());
    wav.resize(44 + len, 0);
    wav
}

pub(crate) fn transcribe_request() -> Value {
    transcribe_request_with_samples(1)
}

pub(crate) fn transcribe_request_with_samples(samples: usize) -> Value {
    json!({"jsonrpc":"2.0","id":1,"method":"voice.turn.transcribe","params":{
        "turn_id":TURN_ID,"audio_base64":STANDARD.encode(wav(samples)),
        "ingress":"desktop_ipc","connection_id":"fake","response_budget":999999999,
        "transport":"desktop_ipc"
    }})
}

pub(crate) struct SttFixture {
    pub(crate) config: Arc<RwLock<Config>>,
    calls: Arc<AtomicUsize>,
    fallback_calls: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
    _root: tempfile::TempDir,
    vault: Arc<SharedTokenManager>,
}

impl SttFixture {
    pub(crate) async fn new() -> Self {
        Self::with_transcript(" fixture ".to_owned()).await
    }

    async fn with_transcript(transcript: String) -> Self {
        use axum::{
            extract::State,
            http::{HeaderMap, StatusCode},
            routing::post,
            Json, Router,
        };
        let calls = Arc::new(AtomicUsize::new(0));
        let fallback_calls = Arc::new(AtomicUsize::new(0));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let router = Router::new()
            .route(
                "/v1/audio/transcriptions",
                post(
                    move |State((calls, _)): State<(Arc<AtomicUsize>, Arc<AtomicUsize>)>,
                          headers: HeaderMap,
                          body: axum::body::Bytes| {
                        let transcript = transcript.clone();
                        async move {
                            assert!(!headers.contains_key("authorization"), "BYO has no key");
                            assert!(body
                                .windows(12)
                                .any(|chunk| &chunk[..4] == b"RIFF" && &chunk[8..] == b"WAVE"));
                            calls.fetch_add(1, Ordering::SeqCst);
                            (StatusCode::OK, Json(json!({"text":transcript})))
                        }
                    },
                ),
            )
            .route(
                "/fail/audio/transcriptions",
                post(
                    |State((calls, _)): State<(Arc<AtomicUsize>, Arc<AtomicUsize>)>| async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                        StatusCode::BAD_GATEWAY
                    },
                ),
            )
            .route(
                "/fallback/v1/audio/transcriptions",
                post(
                    |State((_, fallback)): State<(Arc<AtomicUsize>, Arc<AtomicUsize>)>| async move {
                        fallback.fetch_add(1, Ordering::SeqCst);
                        Json(json!({"text":"configured cloud"}))
                    },
                ),
            )
            .route(
                "/fallback-fail/v1/audio/transcriptions",
                post(
                    |State((_, fallback)): State<(Arc<AtomicUsize>, Arc<AtomicUsize>)>| async move {
                        fallback.fetch_add(1, Ordering::SeqCst);
                        StatusCode::BAD_GATEWAY
                    },
                ),
            )
            .with_state((calls.clone(), fallback_calls.clone()));
        let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let mut config = Config::default();
        config.generation.default_transcription_provider = Some("fixture".to_owned());
        config.generation.transcription_providers.insert(
            "fixture".to_owned(),
            crate::GenerationProviderConfig {
                provider_type: "local".to_owned(),
                base_url: Some(format!("http://{addr}/v1")),
                enabled: true,
                ..Default::default()
            },
        );
        config.generation.transcription_providers.insert(
            "fallback".to_owned(),
            crate::GenerationProviderConfig {
                provider_type: "openai".to_owned(),
                api_key: Some("unused-test-key".to_owned()),
                base_url: Some(format!("http://{addr}/fallback/v1")),
                enabled: true,
                ..Default::default()
            },
        );
        let root = tempfile::tempdir().unwrap();
        let vault = Arc::new(SharedTokenManager::new(
            Arc::new(SecurityStore::in_memory().unwrap()),
            root.path().join("test.vault"),
        ));
        Self {
            config: Arc::new(RwLock::new(config)),
            calls,
            fallback_calls,
            task,
            _root: root,
            vault,
        }
    }

    pub(crate) fn mount(&self, handlers: &mut HandlerRegistry) {
        voice_turn::register_transcription(handlers, self.config.clone(), self.vault.clone(), None);
    }

    fn mount_with_offline(
        &self,
        handlers: &mut HandlerRegistry,
        offline: voice_turn::OfflineSpeechAdapter,
    ) {
        voice_turn::register_transcription(
            handlers,
            self.config.clone(),
            self.vault.clone(),
            Some(offline),
        );
        voice_turn::register_cancel(handlers);
    }

    pub(crate) fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst) + self.fallback_calls.load(Ordering::SeqCst)
    }
}

impl Drop for SttFixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn invoke(registry: &HandlerRegistry, request: Value, budget: usize) -> JsonRpcResponse {
    invoke_on(
        registry,
        request,
        &TestConnection::new(VoiceIngress::WebV1, "test-conn"),
        budget,
    )
    .await
}

/// One live connection: its teardown token and its turn table.
struct TestConnection {
    ingress: VoiceIngress,
    id: &'static str,
    cancellation: tokio_util::sync::CancellationToken,
    turns: Arc<voice_turn::VoiceTurnScopes>,
}

impl TestConnection {
    fn new(ingress: VoiceIngress, id: &'static str) -> Self {
        Self {
            ingress,
            id,
            cancellation: Default::default(),
            turns: Default::default(),
        }
    }
}

async fn invoke_on(
    registry: &HandlerRegistry,
    request: Value,
    connection: &TestConnection,
    budget: usize,
) -> JsonRpcResponse {
    let request: JsonRpcRequest = serde_json::from_value(request).unwrap();
    registry
        .handle_with_context(
            &request,
            None,
            Some(VoiceRequestContext::new(
                connection.ingress,
                connection.id.to_owned(),
                budget,
                connection.cancellation.child_token(),
                connection.turns.clone(),
            )),
        )
        .await
}

#[tokio::test]
async fn voice_turn_transcript_limited_to_v1_string_bytes() {
    use aleph_client::local_ipc::{parse_payload, MAX_STRING_BYTES};

    // A unique prefix keeps the provider's repetition filter from blanking the fixture.
    let exact = format!("STT {}", "é".repeat((MAX_STRING_BYTES - 4) / 2));
    assert_eq!(exact.len(), 65_536);
    for (text, expected_error) in [(exact.clone(), false), (format!("{exact}a"), true)] {
        let provider = SttFixture::with_transcript(text.clone()).await;
        let mut registry = HandlerRegistry::empty();
        provider.mount(&mut registry);
        let reply = invoke(&registry, transcribe_request(), 1024 * 1024).await;
        let value = parse_payload(&serde_json::to_string(&reply).unwrap()).unwrap();
        assert_eq!(provider.calls(), 1);
        if expected_error {
            assert_eq!(value["error"]["data"]["kind"], "too_large");
            assert!(value.get("result").is_none());
        } else {
            assert_eq!(value.get("error"), None);
            assert!(value["result"]["text"].as_str() == Some(&text));
        }
    }
}

#[tokio::test]
async fn voice_turn_wav_limits_checked_before_dispatch() {
    let provider = SttFixture::new().await;
    let mut registry = HandlerRegistry::empty();
    provider.mount(&mut registry);
    let mut request = transcribe_request();
    let mut riff_past_end = wav(1);
    riff_past_end[4] += 1;
    let mut not_pcm = wav(1);
    not_pcm[20] = 3;
    let mut stereo = wav(1);
    stereo[22] = 2;
    let mut not_16k = wav(1);
    not_16k[24] += 1;
    let mut not_16bit = wav(1);
    not_16bit[34] = 8;
    let mut no_data = wav(1);
    no_data[36..40].copy_from_slice(b"JUNK");
    let cases = [
        (STANDARD.encode(wav(MAX_UTTERANCE_SAMPLES + 1)), "too_large"),
        ("!!!".into(), "invalid_audio"),
        (STANDARD.encode(riff_past_end), "invalid_audio"),
        (STANDARD.encode(not_pcm), "invalid_audio"),
        (STANDARD.encode(stereo), "invalid_audio"),
        (STANDARD.encode(not_16k), "invalid_audio"),
        (STANDARD.encode(not_16bit), "invalid_audio"),
        (STANDARD.encode(no_data), "invalid_audio"),
        (STANDARD.encode(wav(0)), "no_speech"),
    ];
    for (audio, kind) in cases {
        request["params"]["audio_base64"] = json!(audio);
        let response =
            serde_json::to_value(invoke(&registry, request.clone(), 1024 * 1024).await).unwrap();
        assert_eq!(response["error"]["data"]["kind"], kind, "{response}");
        assert_eq!(provider.calls(), 0);
    }
    request["params"]["audio_base64"] = json!(STANDARD.encode(wav(MAX_UTTERANCE_SAMPLES)));
    assert_eq!(
        invoke(&registry, request, 1024 * 1024)
            .await
            .result
            .unwrap()["text"],
        "fixture"
    );
    assert_eq!(provider.calls(), 1);

    let legacy = serde_json::from_value(transcribe_request()).unwrap();
    let response = serde_json::to_value(registry.handle(&legacy).await).unwrap();
    assert_eq!(response["error"]["data"]["kind"], "permission_denied");
    assert_eq!(provider.calls(), 1);

    let mut cfg = provider.config.write().await;
    cfg.generation.transcription_providers.remove("fallback");
    let url = cfg
        .generation
        .transcription_providers
        .get_mut("fixture")
        .unwrap()
        .base_url
        .as_mut()
        .unwrap();
    *url = url.replace("/v1", "/fail");
    drop(cfg);
    let response =
        serde_json::to_value(invoke(&registry, transcribe_request(), 1024 * 1024).await).unwrap();
    assert_eq!(response["error"]["data"]["kind"], "provider_failed");
    assert_eq!(provider.calls(), 2);
}

#[tokio::test]
async fn voice_turn_synthesis_rejects_unsupported_and_oversize_output() {
    let request = json!({"jsonrpc":"2.0","id":"escaped \\\" 雪","method":"voice.turn.synthesize","params":{"turn_id":TURN_ID,"text":"**Speak** this."}});
    let exact_budget = serde_json::to_vec(&JsonRpcResponse::success(
        Some(request["id"].clone()),
        json!({"audio_base64": STANDARD.encode([1; 64]), "mime_type": "audio/mpeg"}),
    ))
    .unwrap()
    .len();
    for (data, mime, expected) in [
        (
            GenerationData::bytes(vec![1; 4]),
            "audio/mpeg",
            Ok(("audio/mpeg", 4)),
        ),
        (
            GenerationData::bytes(vec![1; 4]),
            "audio/mp3",
            Ok(("audio/mpeg", 4)),
        ),
        (
            GenerationData::bytes(vec![1; 4]),
            "audio/wav",
            Ok(("audio/wav", 4)),
        ),
        (
            GenerationData::bytes(vec![1; 4]),
            "audio/x-wav",
            Ok(("audio/wav", 4)),
        ),
        (
            GenerationData::bytes(vec![1; 64]),
            "audio/mpeg",
            Ok(("audio/mpeg", 64)),
        ),
        (
            GenerationData::bytes(vec![1; 64]),
            "audio/mpeg",
            Err("too_large"),
        ),
        (
            GenerationData::bytes(vec![1; 4]),
            "audio/flac",
            Err("unsupported_output"),
        ),
        (
            GenerationData::Url("https://example.invalid/contents".into()),
            "audio/mpeg",
            Err("unsupported_output"),
        ),
        (
            GenerationData::LocalPath("should-not-be-read.mp3".into()),
            "audio/mpeg",
            Err("unsupported_output"),
        ),
    ] {
        let mut providers = GenerationProviderRegistry::new();
        let fixture = mount_tts_fixture(&mut providers, "fixture", Ok((data, mime)));
        let mut registry = HandlerRegistry::empty();
        voice_turn::register_synthesis(
            &mut registry,
            Default::default(),
            Some(Arc::new(crate::sync_primitives::RwLock::new(providers))),
        );
        let budget = exact_budget - usize::from(expected == Err("too_large"));
        let response =
            serde_json::to_value(invoke(&registry, request.clone(), budget).await).unwrap();
        assert_eq!(fixture.load(Ordering::SeqCst), 1);
        match expected {
            Err(kind) => {
                assert_eq!(response["error"]["data"]["kind"], kind, "{response}");
                assert!(response.get("result").is_none());
            }
            Ok((mime, byte_count)) => {
                assert_eq!(response["result"]["mime_type"], mime, "{response}");
                assert_eq!(
                    response["result"]["audio_base64"],
                    STANDARD.encode(vec![1; byte_count])
                );
            }
        }
    }
}

struct TtsFixture {
    name: &'static str,
    calls: Arc<AtomicUsize>,
    result: Result<GenerationOutput, ()>,
}

impl GenerationProvider for TtsFixture {
    fn generate(
        &self,
        request: GenerationRequest,
    ) -> Pin<Box<dyn Future<Output = GenerationResult<GenerationOutput>> + Send + '_>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert!(!request.prompt.contains('*'));
        Box::pin(async {
            self.result
                .clone()
                .map_err(|()| GenerationError::network("fixture transient error"))
        })
    }

    fn name(&self) -> &str {
        self.name
    }

    fn supported_types(&self) -> Vec<GenerationType> {
        vec![GenerationType::Speech]
    }
}

fn mount_tts_fixture(
    providers: &mut GenerationProviderRegistry,
    name: &'static str,
    result: Result<(GenerationData, &'static str), ()>,
) -> Arc<AtomicUsize> {
    let calls = Arc::new(AtomicUsize::new(0));
    let result = result.map(|(data, mime)| {
        let mut output = GenerationOutput::new(GenerationType::Speech, data);
        output.metadata = output.metadata.with_content_type(mime);
        output
    });
    providers
        .register(
            name.to_owned(),
            Arc::new(TtsFixture {
                name,
                calls: calls.clone(),
                result,
            }),
        )
        .unwrap();
    calls
}

pub(crate) fn mount_synthesis_bytes(
    handlers: &mut HandlerRegistry,
    bytes: usize,
) -> Arc<AtomicUsize> {
    let mut providers = GenerationProviderRegistry::new();
    let calls = mount_tts_fixture(
        &mut providers,
        "fixture",
        Ok((GenerationData::bytes(vec![1; bytes]), "audio/mpeg")),
    );
    voice_turn::register_synthesis(
        handlers,
        Default::default(),
        Some(Arc::new(crate::sync_primitives::RwLock::new(providers))),
    );
    calls
}

#[tokio::test]
async fn voice_turn_synthesis_attempts_primary_and_secondary_at_most_once() {
    let supported = || (GenerationData::bytes(vec![1; 4]), "audio/wav");
    for secondary_result in [Ok(supported()), Err(())] {
        let secondary_succeeds = secondary_result.is_ok();
        let mut providers = GenerationProviderRegistry::new();
        let primary = mount_tts_fixture(&mut providers, "a-primary", Err(()));
        let secondary = mount_tts_fixture(&mut providers, "b-secondary", secondary_result);
        let untried = mount_tts_fixture(&mut providers, "c-untried", Ok(supported()));
        let mut registry = HandlerRegistry::empty();
        voice_turn::register_synthesis(
            &mut registry,
            Default::default(),
            Some(Arc::new(crate::sync_primitives::RwLock::new(providers))),
        );
        let response = invoke(
            &registry,
            json!({"jsonrpc":"2.0","id":1,"method":"voice.turn.synthesize","params":{
                "turn_id":TURN_ID,"text":"Speak."
            }}),
            1024 * 1024,
        )
        .await;
        let response = serde_json::to_value(response).unwrap();
        if secondary_succeeds {
            assert_eq!(response["result"]["mime_type"], "audio/wav");
        } else {
            assert_eq!(response["error"]["data"]["kind"], "provider_failed");
        }
        assert_eq!(primary.load(Ordering::SeqCst), 1);
        assert_eq!(secondary.load(Ordering::SeqCst), 1);
        assert_eq!(untried.load(Ordering::SeqCst), 0);
    }
}

/// The host's installed recognizer, faked: counts calls and, when `reply` is
/// `None`, holds the clip until cancelled.
#[derive(Default)]
pub(crate) struct FakeOfflineEngine {
    calls: AtomicUsize,
    reply: std::sync::Mutex<Option<String>>,
    started: tokio::sync::Notify,
    clip: std::sync::Mutex<Option<std::path::PathBuf>>,
    /// Whether the clip still existed when the cancel reached the engine.
    clip_present_at_cancel: std::sync::Mutex<Option<bool>>,
    returned: tokio::sync::Notify,
    cancellation_seen: tokio::sync::Notify,
    release_gate: Option<tokio::sync::Semaphore>,
}

impl FakeOfflineEngine {
    pub(crate) async fn wait_started(&self) {
        self.started.notified().await;
    }

    pub(crate) async fn wait_cancelled(&self) {
        self.cancellation_seen.notified().await;
    }

    pub(crate) fn release(&self) {
        self.release_gate.as_ref().unwrap().add_permits(1);
    }

    fn replying(text: &str) -> Arc<Self> {
        let engine = Self::default();
        *engine.reply.lock().unwrap() = Some(text.to_owned());
        Arc::new(engine)
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl aleph_desktop::traits::MediaCapability for FakeOfflineEngine {
    async fn speech_to_text_offline(
        &self,
        audio_path: &str,
        language: Option<&str>,
        cancel: tokio_util::sync::CancellationToken,
    ) -> aleph_desktop::Result<aleph_desktop::media_types::SpeechToTextResult> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let clip = std::path::PathBuf::from(audio_path);
        assert!(clip.is_file(), "the clip is staged before the engine runs");
        *self.clip.lock().unwrap() = Some(clip.clone());
        let reply = self.reply.lock().unwrap().clone();
        let result = match reply {
            Some(text) => Ok(aleph_desktop::media_types::SpeechToTextResult {
                text,
                language: language.unwrap_or("os-locale").to_owned(),
            }),
            None => {
                self.started.notify_one();
                cancel.cancelled().await;
                *self.clip_present_at_cancel.lock().unwrap() = Some(clip.is_file());
                self.cancellation_seen.notify_one();
                if let Some(gate) = &self.release_gate {
                    gate.acquire().await.unwrap().forget();
                }
                assert!(clip.is_file(), "clip released while the engine held it");
                Err(aleph_desktop::DesktopError::PlatformError(
                    "offline speech recognition cancelled".into(),
                ))
            }
        };
        self.returned.notify_one();
        result
    }
}

struct FakePlatform<M>(Arc<M>);

impl<M: aleph_desktop::traits::MediaCapability> aleph_desktop::DesktopPlatform for FakePlatform<M> {
    fn platform_name(&self) -> &str {
        "fake"
    }
    fn screen(&self) -> Option<&dyn aleph_desktop::traits::ScreenCapability> {
        None
    }
    fn pim(&self) -> Option<&dyn aleph_desktop::traits::PimCapability> {
        None
    }
    fn system(&self) -> Option<&dyn aleph_desktop::traits::SystemCapability> {
        None
    }
    fn automation(&self) -> Option<&dyn aleph_desktop::traits::AutomationCapability> {
        None
    }
    fn permission(&self) -> Option<&dyn aleph_desktop::traits::PermissionCapability> {
        None
    }
    fn media(&self) -> Option<&dyn aleph_desktop::traits::MediaCapability> {
        Some(&*self.0)
    }
}

#[derive(Default)]
pub(crate) struct CaptureMediaFixture {
    pub(crate) entered: tokio::sync::Notify,
    pub(crate) permit_samples: tokio::sync::Notify,
    pub(crate) sealing: tokio::sync::Notify,
    pub(crate) seal_cancelled: tokio::sync::Notify,
    pub(crate) permit_release: tokio::sync::Notify,
    pub(crate) released: AtomicUsize,
    pub(crate) release_completed: tokio::sync::Notify,
}

impl CaptureMediaFixture {
    pub(crate) fn mount(self: &Arc<Self>, handlers: &mut HandlerRegistry) {
        let media = Arc::new(FixtureRecording(self.clone()));
        voice_turn::register_capture(handlers, Some(Arc::new(FakePlatform(media))));
    }
}

#[tokio::test]
async fn voice_capture_legacy_dispatch_cannot_acquire_device() {
    use aleph_protocol::panel_voice::{
        VOICE_TURN_CAPTURE_CANCEL_METHOD, VOICE_TURN_CAPTURE_FINISH_METHOD,
        VOICE_TURN_CAPTURE_METHOD, VOICE_TURN_CAPTURE_READY_METHOD,
    };

    let mut handlers = HandlerRegistry::new();
    let media = Arc::new(CaptureMediaFixture::default());
    media.mount(&mut handlers);
    let capture_id = uuid::Uuid::new_v4();
    for method in [
        VOICE_TURN_CAPTURE_METHOD,
        VOICE_TURN_CAPTURE_READY_METHOD,
        VOICE_TURN_CAPTURE_FINISH_METHOD,
        VOICE_TURN_CAPTURE_CANCEL_METHOD,
    ] {
        let request: JsonRpcRequest = serde_json::from_value(serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": method,
            "params": {"capture_id": capture_id}
        }))
        .unwrap();
        let response = handlers.handle(&request).await;
        assert_eq!(
            response.error.unwrap().code,
            crate::gateway::protocol::PERMISSION_DENIED
        );
    }
    let entered = media.entered.notified();
    tokio::pin!(entered);
    assert!(futures::poll!(&mut entered).is_pending());
}

struct FixtureRecording(Arc<CaptureMediaFixture>);

#[async_trait::async_trait]
impl aleph_desktop::traits::MediaCapability for FixtureRecording {
    fn supports_voice_capture(&self) -> bool {
        true
    }

    async fn voice_capture_begin(
        &self,
        _cancel: tokio_util::sync::CancellationToken,
    ) -> aleph_desktop::Result<Box<dyn aleph_desktop::traits::VoiceCapture>> {
        self.0.entered.notify_one();
        Ok(Box::new(Self(self.0.clone())))
    }
}

#[async_trait::async_trait]
impl aleph_desktop::traits::VoiceCapture for FixtureRecording {
    async fn poll(
        &mut self,
        after_sample: u32,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> aleph_desktop::Result<Vec<aleph_desktop::media_types::VoiceCaptureObservation>> {
        if after_sample != 0 {
            return Ok(Vec::new());
        }
        tokio::select! {
            () = cancel.cancelled() => Ok(Vec::new()),
            () = self.0.permit_samples.notified() => Ok((1..=8)
                .map(|frame| aleph_desktop::media_types::VoiceCaptureObservation {
                    end_sample: frame * 800,
                    rms: 0.5,
                }).collect()),
        }
    }

    async fn seal(
        self: Box<Self>,
        first_sample: u32,
        end_sample: u32,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> aleph_desktop::Result<Vec<u8>> {
        assert_eq!((first_sample, end_sample), (0, 6400));
        self.0.sealing.notify_one();
        tokio::select! {
            () = self.0.permit_release.notified() => {},
            () = cancel.cancelled() => {
                self.0.seal_cancelled.notify_one();
                self.0.permit_release.notified().await;
            }
        }
        self.0.released.fetch_add(1, Ordering::SeqCst);
        self.0.release_completed.notify_one();
        Ok(wav((end_sample - first_sample) as usize))
    }

    async fn cancel(self: Box<Self>) -> aleph_desktop::Result<()> {
        self.0.released.fetch_add(1, Ordering::SeqCst);
        self.0.release_completed.notify_one();
        Ok(())
    }
}

fn offline_adapter(
    engine: &Arc<FakeOfflineEngine>,
    scratch: &tempfile::TempDir,
) -> voice_turn::OfflineSpeechAdapter {
    voice_turn::OfflineSpeechAdapter::new(
        Arc::new(FakePlatform(engine.clone())),
        scratch.path().to_owned(),
    )
}

pub(crate) async fn mount_gated_offline(
    provider: &SttFixture,
    handlers: &mut HandlerRegistry,
    scratch: &tempfile::TempDir,
) -> Arc<FakeOfflineEngine> {
    remove_stt_providers(provider).await;
    let engine = Arc::new(FakeOfflineEngine {
        release_gate: Some(tokio::sync::Semaphore::new(0)),
        ..Default::default()
    });
    provider.mount_with_offline(handlers, offline_adapter(&engine, scratch));
    engine
}

async fn remove_stt_providers(provider: &SttFixture) {
    let mut cfg = provider.config.write().await;
    cfg.generation.default_transcription_provider = None;
    cfg.generation.transcription_providers.clear();
}

fn cancel_request(turn_id: &str) -> Value {
    json!({"jsonrpc":"2.0","id":"cancel","method":"voice.turn.cancel","params":{"turn_id":turn_id}})
}

fn error_kind(response: JsonRpcResponse) -> Value {
    serde_json::to_value(response).unwrap()["error"]["data"]["kind"].clone()
}

fn scratch_is_empty(scratch: &tempfile::TempDir) -> bool {
    std::fs::read_dir(scratch.path()).unwrap().next().is_none()
}

#[tokio::test]
async fn voice_turn_scratch_survives_offline_recognition_task_abort() {
    for abort in [false, true] {
        let scratch = tempfile::tempdir().unwrap();
        let engine = Arc::new(FakeOfflineEngine {
            release_gate: Some(tokio::sync::Semaphore::new(0)),
            ..Default::default()
        });
        let adapter = offline_adapter(&engine, &scratch);
        let cancel = tokio_util::sync::CancellationToken::new();
        let recognition = {
            let cancel = cancel.clone();
            tokio::spawn(async move { adapter.recognize(&wav(1), None, cancel).await })
        };
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            engine.wait_started().await;
            cancel.cancel();
            engine.wait_cancelled().await;
        })
        .await
        .expect("recognizer did not reach the release gate");
        let clip = engine.clip.lock().unwrap().clone().unwrap();
        assert!(clip.is_file(), "clip missing before engine release");

        if abort {
            // Abort recognition itself, not the RPC waiter, without allowing release.
            recognition.abort();
            assert!(recognition.await.unwrap_err().is_cancelled());
            assert!(clip.is_file(), "task abort deleted an unreleased clip");
        } else {
            engine.release();
            assert_eq!(
                recognition.await.unwrap(),
                Err(aleph_protocol::panel_voice::VoiceErrorKind::ProviderFailed)
            );
            assert!(
                scratch_is_empty(&scratch),
                "clip leaked after normal release"
            );
        }
    }
}

#[tokio::test]
async fn offline_stt_policy_matrix() {
    const BUDGET: usize = 1024 * 1024;
    let desktop = TestConnection::new(VoiceIngress::DesktopIpc, "desktop-conn");
    let web = TestConnection::new(VoiceIngress::WebV1, "web-conn");
    let scratch = tempfile::tempdir().unwrap();

    // A configured local endpoint is used, and the installed engine is never asked.
    let engine = FakeOfflineEngine::replying("offline");
    let provider = SttFixture::new().await;
    let mut registry = HandlerRegistry::empty();
    provider.mount_with_offline(&mut registry, offline_adapter(&engine, &scratch));
    let reply = invoke_on(&registry, transcribe_request(), &desktop, BUDGET).await;
    assert_eq!(reply.result.unwrap()["text"], "fixture");
    assert_eq!((provider.calls(), engine.calls()), (1, 0));

    // A configured cloud provider likewise.
    provider
        .config
        .write()
        .await
        .generation
        .default_transcription_provider = Some("fallback".to_owned());
    let reply = invoke_on(&registry, transcribe_request(), &desktop, BUDGET).await;
    assert_eq!(reply.result.unwrap()["text"], "configured cloud");
    assert_eq!((provider.calls(), engine.calls()), (2, 0));

    // A failed configured local endpoint retries only the configured cloud.
    {
        let mut cfg = provider.config.write().await;
        cfg.generation.default_transcription_provider = Some("fixture".to_owned());
        let url = cfg
            .generation
            .transcription_providers
            .get_mut("fixture")
            .unwrap()
            .base_url
            .as_mut()
            .unwrap();
        *url = url.replace("/v1", "/fail");
    }
    let reply = invoke_on(&registry, transcribe_request(), &desktop, BUDGET).await;
    assert_eq!(reply.result.unwrap()["text"], "configured cloud");
    assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
    assert_eq!(provider.fallback_calls.load(Ordering::SeqCst), 2);
    assert_eq!(engine.calls(), 0);

    // If both configured providers fail, do not dispatch to the installed engine.
    {
        let mut cfg = provider.config.write().await;
        let url = cfg
            .generation
            .transcription_providers
            .get_mut("fallback")
            .unwrap()
            .base_url
            .as_mut()
            .unwrap();
        *url = url.replace("/fallback/v1", "/fallback-fail/v1");
    }
    let reply = invoke_on(&registry, transcribe_request(), &desktop, BUDGET).await;
    assert_eq!(error_kind(reply), "provider_failed");
    assert_eq!(provider.calls.load(Ordering::SeqCst), 3);
    assert_eq!(provider.fallback_calls.load(Ordering::SeqCst), 3);
    assert_eq!(engine.calls(), 0);

    // No provider at all: owner Desktop IPC asks the installed engine exactly once.
    remove_stt_providers(&provider).await;
    let reply = invoke_on(&registry, transcribe_request(), &desktop, BUDGET).await;
    assert_eq!(reply.result.unwrap()["text"], "offline");
    assert_eq!((provider.calls(), engine.calls()), (6, 1));
    assert!(
        scratch_is_empty(&scratch),
        "clip deleted after the engine returned"
    );

    // Malformed PCM is rejected before any engine.
    let mut malformed = transcribe_request();
    malformed["params"]["audio_base64"] = json!("!!!");
    let reply = invoke_on(&registry, malformed, &desktop, BUDGET).await;
    assert_eq!(error_kind(reply), "invalid_audio");
    assert_eq!(engine.calls(), 1);

    // Web (whatever its socket) and legacy never reach the installed engine.
    let reply = invoke_on(&registry, transcribe_request(), &web, BUDGET).await;
    assert_eq!(error_kind(reply), "not_configured");
    let legacy = serde_json::from_value(transcribe_request()).unwrap();
    let reply = registry.handle(&legacy).await;
    assert_eq!(error_kind(reply), "permission_denied");
    assert_eq!(engine.calls(), 1);

    // No adapter mounted on this host: typed unavailability, not a guess.
    let mut bare = HandlerRegistry::empty();
    provider.mount(&mut bare);
    let reply = invoke_on(&bare, transcribe_request(), &desktop, BUDGET).await;
    assert_eq!(error_kind(reply), "offline_unavailable");

    // An empty offline transcript is no speech.
    let blank = FakeOfflineEngine::replying("  ");
    let mut registry = HandlerRegistry::empty();
    provider.mount_with_offline(&mut registry, offline_adapter(&blank, &scratch));
    let reply = invoke_on(&registry, transcribe_request(), &desktop, BUDGET).await;
    assert_eq!(error_kind(reply), "no_speech");
    assert_eq!(blank.calls(), 1);

    // Cancel stops the engine, which is not retried and releases the clip only
    // after it returned.
    let held = Arc::new(FakeOfflineEngine::default());
    let mut registry = HandlerRegistry::empty();
    provider.mount_with_offline(&mut registry, offline_adapter(&held, &scratch));
    let registry = Arc::new(registry);
    let turn = {
        let registry = registry.clone();
        let desktop = TestConnection {
            cancellation: desktop.cancellation.clone(),
            turns: desktop.turns.clone(),
            ..TestConnection::new(VoiceIngress::DesktopIpc, "desktop-conn")
        };
        tokio::spawn(
            async move { invoke_on(&registry, transcribe_request(), &desktop, BUDGET).await },
        )
    };
    held.started.notified().await;
    let returned = held.returned.notified();
    let reply = invoke_on(&registry, cancel_request(TURN_ID), &desktop, BUDGET).await;
    assert_eq!(reply.result.unwrap()["cancelled"], true);
    assert_eq!(error_kind(turn.await.unwrap()), "cancelled");
    returned.await;
    assert_eq!(*held.clip_present_at_cancel.lock().unwrap(), Some(true));
    wait_until(|| scratch_is_empty(&scratch)).await;
    assert_eq!((provider.calls(), held.calls()), (6, 1));

    // The cancelled turn cannot be started again on this connection.
    let reply = invoke_on(&registry, transcribe_request(), &desktop, BUDGET).await;
    assert_eq!(error_kind(reply), "cancelled");
    assert_eq!(held.calls(), 1);
}

async fn wait_until(mut done: impl FnMut() -> bool) {
    for _ in 0..200 {
        if done() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("condition not reached");
}

#[tokio::test]
async fn voice_turn_cancel_is_denied_to_other_connections() {
    const BUDGET: usize = 1024 * 1024;
    let scratch = tempfile::tempdir().unwrap();
    let provider = SttFixture::new().await;
    remove_stt_providers(&provider).await;
    let held = Arc::new(FakeOfflineEngine::default());
    let mut registry = HandlerRegistry::empty();
    provider.mount_with_offline(&mut registry, offline_adapter(&held, &scratch));
    let registry = Arc::new(registry);
    let owner = TestConnection::new(VoiceIngress::DesktopIpc, "owner-conn");
    let other = TestConnection::new(VoiceIngress::DesktopIpc, "other-conn");

    let turn = {
        let registry = registry.clone();
        let owner = TestConnection {
            cancellation: owner.cancellation.clone(),
            turns: owner.turns.clone(),
            ..TestConnection::new(VoiceIngress::DesktopIpc, "owner-conn")
        };
        tokio::spawn(
            async move { invoke_on(&registry, transcribe_request(), &owner, BUDGET).await },
        )
    };
    held.started.notified().await;

    // Same turn id, another connection: nothing of its own to cancel.
    let reply = invoke_on(&registry, cancel_request(TURN_ID), &other, BUDGET).await;
    assert_eq!(reply.result.unwrap()["cancelled"], false);
    assert!(!turn.is_finished());
    assert_eq!(*held.clip_present_at_cancel.lock().unwrap(), None);
    // Legacy has no connection identity to cancel with.
    let legacy = serde_json::from_value(cancel_request(TURN_ID)).unwrap();
    assert_eq!(
        error_kind(registry.handle(&legacy).await),
        "permission_denied"
    );
    assert!(!turn.is_finished());

    // Connection teardown cancels its turns through the same engine path.
    let returned = held.returned.notified();
    owner.cancellation.cancel();
    assert_eq!(error_kind(turn.await.unwrap()), "cancelled");
    returned.await;
    assert_eq!(*held.clip_present_at_cancel.lock().unwrap(), Some(true));
    wait_until(|| scratch_is_empty(&scratch)).await;
    assert_eq!(held.calls(), 1);

    // A cancel that arrives first refuses the late start without the engine.
    let late = "2b63aa6b-0688-4be3-991f-e6994e0f795c";
    let reply = invoke_on(&registry, cancel_request(late), &other, BUDGET).await;
    assert_eq!(reply.result.unwrap()["cancelled"], false);
    let mut request = transcribe_request();
    request["params"]["turn_id"] = json!(late);
    assert_eq!(
        error_kind(invoke_on(&registry, request, &other, BUDGET).await),
        "cancelled"
    );
    assert_eq!(held.calls(), 1);
}

pub(crate) struct BlockedSttFixture {
    _provider: SttFixture,
    entered: Arc<tokio::sync::Semaphore>,
    exited: Arc<tokio::sync::Semaphore>,
    server: tokio::task::JoinHandle<()>,
}

impl BlockedSttFixture {
    pub(crate) async fn new(handlers: &mut HandlerRegistry) -> Self {
        use axum::{extract::State, routing::post, Router};
        struct Exit(Arc<tokio::sync::Semaphore>);
        impl Drop for Exit {
            fn drop(&mut self) {
                self.0.add_permits(1);
            }
        }
        let entered = Arc::new(tokio::sync::Semaphore::new(0));
        let exited = Arc::new(tokio::sync::Semaphore::new(0));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new()
            .route(
                "/v1/audio/transcriptions",
                post(
                    |State((entered, exited)): State<(
                        Arc<tokio::sync::Semaphore>,
                        Arc<tokio::sync::Semaphore>,
                    )>,
                     _body: axum::body::Bytes| async move {
                        let _exit = Exit(exited);
                        entered.add_permits(1);
                        std::future::pending::<axum::http::StatusCode>().await
                    },
                ),
            )
            .with_state((entered.clone(), exited.clone()));
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let provider = SttFixture::new().await;
        provider
            .config
            .write()
            .await
            .generation
            .transcription_providers
            .get_mut("fixture")
            .unwrap()
            .base_url = Some(format!("http://{addr}/v1"));
        provider.mount(handlers);
        voice_turn::register_cancel(handlers);
        Self {
            _provider: provider,
            entered,
            exited,
            server,
        }
    }

    pub(crate) async fn entered(&self, count: u32) {
        self.entered.acquire_many(count).await.unwrap().forget();
    }

    pub(crate) async fn exited(&self, count: u32) {
        self.exited.acquire_many(count).await.unwrap().forget();
    }
}

impl Drop for BlockedSttFixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

pub(crate) async fn fill_query_lane(
    lanes: &crate::gateway::lane::LaneManager,
    class: crate::gateway::lane::ChannelClass,
) -> Vec<tokio::sync::OwnedSemaphorePermit> {
    use crate::gateway::lane::{ChannelClass, Lane};
    let query = lanes
        .snapshot()
        .into_iter()
        .find(|row| row.lane == Lane::Query.to_string())
        .unwrap();
    let count = query.shared_available
        + if class == ChannelClass::Desktop {
            query.desktop_available.unwrap_or_default()
        } else {
            0
        };
    let mut permits = Vec::with_capacity(count);
    for _ in 0..count {
        permits.push(lanes.acquire("voice.turn.transcribe", class).await.unwrap());
    }
    permits
}

pub(crate) fn blocked_stt_turn(index: u32) -> Value {
    let mut request = transcribe_request();
    request["id"] = json!(index);
    request["params"]["turn_id"] = json!(uuid::Uuid::from_u128(u128::from(index) + 1));
    request
}

pub(crate) fn blocked_stt_cancel(index: u32) -> Value {
    let mut request = cancel_request(&uuid::Uuid::from_u128(u128::from(index) + 1).to_string());
    request["id"] = json!(1000 + index);
    request
}

#[tokio::test]
async fn voice_turn_early_cancel_survives_duplicate_and_history_overflow() {
    const BUDGET: usize = 1024 * 1024;
    let scratch = tempfile::tempdir().unwrap();
    let provider = SttFixture::new().await;
    remove_stt_providers(&provider).await;
    let engine = FakeOfflineEngine::replying("unused");
    let mut registry = HandlerRegistry::empty();
    provider.mount_with_offline(&mut registry, offline_adapter(&engine, &scratch));
    let desktop = TestConnection::new(VoiceIngress::DesktopIpc, "desktop-conn");

    for _ in 0..64 {
        let reply = invoke_on(&registry, blocked_stt_cancel(1), &desktop, BUDGET).await;
        assert_eq!(reply.result.unwrap()["cancelled"], false);
    }
    let reply = invoke_on(&registry, blocked_stt_turn(1), &desktop, BUDGET).await;
    assert_eq!(error_kind(reply), "cancelled");
    assert_eq!(engine.calls(), 0);
    assert!(scratch_is_empty(&scratch));

    // Repeated cancellation of one id must not exhaust the distinct-id budget.
    let reply = invoke_on(&registry, blocked_stt_turn(2), &desktop, BUDGET).await;
    assert_eq!(reply.result.unwrap()["text"], "unused");

    for index in 2..=34 {
        let reply = invoke_on(&registry, blocked_stt_cancel(index), &desktop, BUDGET).await;
        assert_eq!(reply.result.unwrap()["cancelled"], false);
    }
    // Retained oldest id and unknown ids both fail closed after overflow.
    // Malformed PCM must not win over the pre-start cancellation check.
    for index in [1, 34, 35] {
        let mut request = blocked_stt_turn(index);
        request["params"]["audio_base64"] = json!("!!!");
        assert_eq!(
            error_kind(invoke_on(&registry, request, &desktop, BUDGET).await),
            "cancelled"
        );
    }
    assert_eq!(engine.calls(), 1);
    assert_eq!(provider.calls(), 0);
    assert!(scratch_is_empty(&scratch));
}
