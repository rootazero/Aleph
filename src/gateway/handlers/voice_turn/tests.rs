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
                    |State((calls, _)): State<(Arc<AtomicUsize>, Arc<AtomicUsize>)>,
                     headers: HeaderMap,
                     body: axum::body::Bytes| async move {
                        assert!(!headers.contains_key("authorization"), "BYO has no key");
                        assert!(body
                            .windows(12)
                            .any(|chunk| &chunk[..4] == b"RIFF" && &chunk[8..] == b"WAVE"));
                        calls.fetch_add(1, Ordering::SeqCst);
                        (StatusCode::OK, Json(json!({"text":" fixture "})))
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
                        Json(json!({"text":"should not reach fallback"}))
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
        voice_turn::register_transcription(handlers, self.config.clone(), self.vault.clone());
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
    let request: JsonRpcRequest = serde_json::from_value(request).unwrap();
    registry
        .handle_with_context(
            &request,
            None,
            Some(VoiceRequestContext::new(
                VoiceIngress::WebV1,
                "test-conn".to_owned(),
                budget,
                Default::default(),
            )),
        )
        .await
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
