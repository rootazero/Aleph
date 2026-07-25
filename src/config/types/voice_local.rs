//! BYO local voice configuration ([voice.local] in config.toml).
//!
//! Aleph does not run a voice server itself (Ollama-style BYO): users point
//! `endpoint` at any OpenAI-compatible speech service (e.g. an mlx-audio
//! server exposing `/v1/audio/speech` and `/v1/audio/transcriptions`).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Configuration for the user-provided local voice endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct VoiceLocalConfig {
    /// Master switch. Off by default — enabling injects a "local" provider
    /// into the generation provider maps at load time (fill-empty-only).
    #[serde(default)]
    pub enabled: bool,
    /// Base URL of the OpenAI-compatible voice server. Providers append
    /// `/audio/speech` / `/audio/transcriptions` to it.
    #[serde(default = "default_endpoint")]
    pub endpoint: String,
    /// Optional bearer token. Most BYO servers run unauthenticated — when
    /// unset, no Authorization header is sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// STT model name. Empty (default) = let the server pick its default.
    #[serde(default)]
    pub stt_model: String,
    /// TTS model name. Empty (default) = let the server pick its default.
    #[serde(default)]
    pub tts_model: String,
    /// TTS voice id. Empty (default) = let the server pick its default.
    #[serde(default)]
    pub tts_voice: String,
    /// TTS output container: "opus" (Telegram-native) or "wav".
    #[serde(default = "default_tts_format")]
    pub tts_format: String,
}

fn default_endpoint() -> String {
    // mlx-audio server's default port.
    "http://127.0.0.1:8000/v1".into()
}
fn default_tts_format() -> String {
    "opus".into()
}

impl Default for VoiceLocalConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: default_endpoint(),
            api_key: None,
            stt_model: String::new(),
            tts_model: String::new(),
            tts_voice: String::new(),
            tts_format: default_tts_format(),
        }
    }
}

/// Real-time streaming STT configuration (`[voice.streaming]`).
///
/// Points at a WebSocket-based streaming ASR endpoint. Two protocol adapters
/// are supported:
/// - `"deepgram"` — Deepgram `/v1/listen` wire protocol (also used by
///   self-hosted WhisperLiveKit, which exposes a compatible API).
/// - `"whisperlive"` — collabora WhisperLive segments protocol.
///
/// `enabled = false` (default) means voice input uses the non-streaming
/// `/v1/audio/transcriptions` path instead.
///
/// ⚠️ **Self-hosted WhisperLiveKit must be started with `--pcm-input`.** Aleph
/// streams raw s16le PCM at 16 kHz; WhisperLiveKit only bypasses FFmpeg for raw
/// PCM when that flag is set, and its Deepgram-compatible endpoint ignores the
/// `encoding=linear16&sample_rate=16000` query parameters Aleph sends. Without
/// the flag the server pipes headerless PCM into FFmpeg, which decodes nothing —
/// the stream connects, no transcript ever arrives, and the Panel silently
/// strikes out to the batch path after two empty utterances.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct StreamingConfig {
    pub enabled: bool,
    /// Protocol adapter: "deepgram" (covers Deepgram cloud + WhisperLiveKit) | "whisperlive".
    pub provider: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub base_url: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub api_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// ASR model requested from the backend. Empty = adapter default
    /// (WhisperLive handshake: "small"; Deepgram dialect: server default) —
    /// set this when your WhisperLive server hosts a larger model, otherwise
    /// it silently loads/serves the small one per client request.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub model: String,
}

impl Default for StreamingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: "deepgram".into(),
            base_url: String::new(),
            api_key: String::new(),
            language: None,
            model: String::new(),
        }
    }
}

/// Post-transcription formatting pass configuration (`[voice.format]`).
///
/// When enabled, a fast LLM pass ("speech refiner") cleans up raw transcription
/// output (punctuation, capitalization, filler-word removal) before the text
/// reaches the main agent loop.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct FormatConfig {
    pub enabled: bool,
    /// Fast model for the "speech refiner" pass, via ModelOverride::from_voice(provider, model).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub provider: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub model: String,
    /// Override the default system prompt (empty → built-in default).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub prompt: String,
}

impl Default for FormatConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            provider: String::new(),
            model: String::new(),
            prompt: String::new(),
        }
    }
}

/// `[voice]` config section wrapper — local BYO endpoint, streaming STT, and
/// post-transcription formatting.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct VoiceSection {
    #[serde(default)]
    pub local: VoiceLocalConfig,
    /// Real-time streaming STT (`[voice.streaming]`). Disabled by default;
    /// enable and configure a WebSocket endpoint to use streaming ASR.
    #[serde(default)]
    pub streaming: StreamingConfig,
    /// Post-transcription LLM formatting pass (`[voice.format]`). Enabled by
    /// default with empty provider/model (falls back to global default model).
    #[serde(default)]
    pub format: FormatConfig,
    /// Provider id pinned for voice-mode replies (e.g. a low-TTFT China-edge
    /// model so the spoken reply starts faster than the global default). Empty
    /// = no override; the run falls back to the global default. When
    /// `llm_model` is set but this is empty, the resolver picks the provider by
    /// model-name heuristic (`ModelOverride::Raw`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub llm_provider: String,
    /// Model id pinned for voice-mode replies. Empty = no override. Pairs with
    /// `llm_provider` (both set → pin both; only this set → resolver picks the
    /// provider).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub llm_model: String,
    /// Domain vocabulary biased *into* recognition — proper nouns, product and
    /// agent names, jargon the acoustic model has never seen ("Aleph", "Leptos",
    /// a colleague's name). Empty (default) sends nothing.
    ///
    /// This is upstream *data*, not a downstream text-rewriting pass. Reference
    /// dictation apps ship a post-hoc regex find-and-replace over the transcript;
    /// that is a deterministic rules engine over natural language (violates
    /// R7/P8, and `[voice.format]`'s LLM already owns transcript polish). Biasing
    /// the decoder fixes the recognition instead of patching its output, and it
    /// is exactly the class of thing the ASR cannot know on its own.
    ///
    /// Reaches: batch Whisper (`prompt` form field) and the WhisperLive
    /// handshake (`hotwords` + `initial_prompt`, the two fields WhisperLive's own
    /// client documents as "domain vocabulary or names"). **Not** the Deepgram
    /// dialect — the parameter name there is model-dependent (`keywords` on
    /// nova-2, `keyterm` on nova-3) and guessing wrong is a 400 that kills a
    /// working cloud stream; WhisperLiveKit ignores query parameters anyway.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vocabulary: Vec<String>,
}

/// Character ceiling on the rendered vocabulary hint. Whisper's `initial_prompt`
/// is bounded by the decoder's 224-token context, and a hint that overruns it
/// crowds out the audio itself; terms past the limit are dropped whole rather
/// than truncated mid-word.
const MAX_VOCABULARY_CHARS: usize = 800;

impl VoiceSection {
    /// Render [`Self::vocabulary`] as the single hint string every backend takes
    /// (comma-separated), or `None` when nothing usable is configured.
    ///
    /// One renderer for all three consumers so the batch and streaming paths
    /// cannot bias the decoder differently.
    #[must_use]
    pub fn vocabulary_hint(&self) -> Option<String> {
        let mut out = String::new();
        for term in self
            .vocabulary
            .iter()
            .map(|t| t.trim())
            .filter(|t| !t.is_empty())
        {
            let extra = if out.is_empty() {
                term.chars().count()
            } else {
                term.chars().count() + 2
            };
            if out.chars().count() + extra > MAX_VOCABULARY_CHARS {
                break;
            }
            if !out.is_empty() {
                out.push_str(", ");
            }
            out.push_str(term);
        }
        (!out.is_empty()).then_some(out)
    }
}

/// The BYO endpoint's provider key: used both as the map entry name and as
/// `GenerationProviderConfig.provider_type`.
pub const LOCAL_PROVIDER_TYPE: &str = "local";

/// Load-time normalization: when local voice is enabled, inject a synthetic
/// "local" provider into the speech/transcription maps and point the unset
/// defaults at it. Fill-empty-only — explicit user config (cloud included)
/// always wins. Idempotent: safe across hot reloads.
///
/// The synthetic entry carries the real connection values (`base_url` =
/// endpoint, `api_key`, model/voice/format) so providers read everything from
/// their own `GenerationProviderConfig`. An empty `api_key` means "no
/// Authorization header".
///
/// Persistence + ownership semantics:
/// - Synthetic state may legitimately end up persisted to disk by any full
///   config save; that is accepted.
/// - When disabled, the cleanup branch removes that state on the next
///   load/patch, so an enable→disable round-trip is clean.
/// - Entries with `provider_type == "local"` are owned by this mechanism:
///   user customizations of them survive enable-time normalization (the
///   `or_insert_with` never overwrites), but they are removed on disable —
///   a "local" provider_type is meaningless without the configured endpoint.
pub fn normalize_voice_local(cfg: &mut crate::config::structs::Config) {
    if !cfg.local_voice().enabled {
        // Disable-cleanup: remove every locally-owned entry, then reset any
        // default that pointed at the now-gone "local" key. Defaults pointing
        // anywhere else are never touched.
        cfg.generation
            .speech_providers
            .retain(|_, p| p.provider_type != LOCAL_PROVIDER_TYPE);
        cfg.generation
            .transcription_providers
            .retain(|_, p| p.provider_type != LOCAL_PROVIDER_TYPE);
        if cfg.generation.default_speech_provider.as_deref() == Some(LOCAL_PROVIDER_TYPE)
            && !cfg
                .generation
                .speech_providers
                .contains_key(LOCAL_PROVIDER_TYPE)
        {
            cfg.generation.default_speech_provider = None;
        }
        if cfg.generation.default_transcription_provider.as_deref() == Some(LOCAL_PROVIDER_TYPE)
            && !cfg
                .generation
                .transcription_providers
                .contains_key(LOCAL_PROVIDER_TYPE)
        {
            cfg.generation.default_transcription_provider = None;
        }
        return;
    }
    use crate::generation::GenerationType;

    let local = cfg.voice_local.local.clone();

    let synth = |cap: GenerationType, model: &str| {
        let mut p = crate::GenerationProviderConfig::new(LOCAL_PROVIDER_TYPE);
        p.base_url = Some(local.endpoint.clone());
        // Empty key = unauthenticated endpoint (no Authorization header).
        p.api_key = Some(local.api_key.clone().unwrap_or_default());
        p.capabilities = vec![cap];
        if !model.is_empty() {
            p.models = vec![model.to_string()];
        }
        if cap == GenerationType::Speech {
            if !local.tts_voice.is_empty() {
                p.defaults.voice = Some(local.tts_voice.clone());
            }
            if !local.tts_format.is_empty() {
                p.defaults.format = Some(local.tts_format.clone());
            }
        }
        p
    };

    cfg.generation
        .speech_providers
        .entry(LOCAL_PROVIDER_TYPE.into())
        .or_insert_with(|| synth(GenerationType::Speech, &local.tts_model));
    cfg.generation
        .transcription_providers
        .entry(LOCAL_PROVIDER_TYPE.into())
        .or_insert_with(|| synth(GenerationType::Transcription, &local.stt_model));

    if cfg.generation.default_speech_provider.is_none() {
        cfg.generation.default_speech_provider = Some(LOCAL_PROVIDER_TYPE.into());
    }
    if cfg.generation.default_transcription_provider.is_none() {
        cfg.generation.default_transcription_provider = Some(LOCAL_PROVIDER_TYPE.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::structs::Config;

    #[test]
    fn disabled_is_a_noop() {
        let mut cfg = Config::default();
        normalize_voice_local(&mut cfg);
        assert!(cfg.generation.speech_providers.is_empty());
        assert!(cfg.generation.default_speech_provider.is_none());
    }

    #[test]
    fn enabled_fills_empty_defaults_and_entries() {
        let mut cfg = Config::default();
        cfg.voice_local.local.enabled = true;
        normalize_voice_local(&mut cfg);
        assert_eq!(
            cfg.generation.default_speech_provider.as_deref(),
            Some("local")
        );
        assert_eq!(
            cfg.generation.default_transcription_provider.as_deref(),
            Some("local")
        );
        let p = &cfg.generation.speech_providers["local"];
        assert_eq!(p.provider_type, "local");
        // Real connection values from [voice.local] land on the entry.
        assert_eq!(p.base_url.as_deref(), Some("http://127.0.0.1:8000/v1"));
        // No api_key configured → empty string = unauthenticated.
        assert_eq!(p.api_key.as_deref(), Some(""));
        // Empty model names are NOT propagated — server default decides.
        assert!(p.models.is_empty());
        assert!(p.defaults.voice.is_none());
        assert_eq!(p.defaults.format.as_deref(), Some("opus"));
        // Validation must accept the synthetic reference (spec: validate passes).
        cfg.generation.validate().unwrap();
    }

    #[test]
    fn enabled_propagates_explicit_models_and_key() {
        let mut cfg = Config::default();
        cfg.voice_local.local.enabled = true;
        cfg.voice_local.local.endpoint = "http://nas.lan:9000/v1".into();
        cfg.voice_local.local.api_key = Some("sk-byo".into());
        cfg.voice_local.local.stt_model = "whisper-large-v3".into();
        cfg.voice_local.local.tts_model = "qwen3-tts".into();
        cfg.voice_local.local.tts_voice = "vivian".into();
        normalize_voice_local(&mut cfg);

        let tts = &cfg.generation.speech_providers["local"];
        assert_eq!(tts.base_url.as_deref(), Some("http://nas.lan:9000/v1"));
        assert_eq!(tts.api_key.as_deref(), Some("sk-byo"));
        assert_eq!(tts.models, vec!["qwen3-tts".to_string()]);
        assert_eq!(tts.defaults.voice.as_deref(), Some("vivian"));

        let stt = &cfg.generation.transcription_providers["local"];
        assert_eq!(stt.models, vec!["whisper-large-v3".to_string()]);
        assert!(stt.defaults.voice.is_none());
    }

    #[test]
    fn explicit_cloud_defaults_win() {
        let mut cfg = Config::default();
        cfg.voice_local.local.enabled = true;
        cfg.generation.default_speech_provider = Some("openai_tts".into());
        cfg.generation.speech_providers.insert(
            "openai_tts".into(),
            crate::GenerationProviderConfig::new("openai_tts"),
        );
        normalize_voice_local(&mut cfg);
        // Cloud default untouched — switching back to cloud is pure config.
        assert_eq!(
            cfg.generation.default_speech_provider.as_deref(),
            Some("openai_tts")
        );
        // Local entry still registered (per-channel override can still pick it).
        assert!(cfg.generation.speech_providers.contains_key("local"));
        // Transcription default was unset → filled with local.
        assert_eq!(
            cfg.generation.default_transcription_provider.as_deref(),
            Some("local")
        );
    }

    #[test]
    fn idempotent_and_preserves_user_local_entry() {
        let mut cfg = Config::default();
        cfg.voice_local.local.enabled = true;
        normalize_voice_local(&mut cfg);
        let mut user_entry = cfg.generation.speech_providers["local"].clone();
        user_entry.models = vec!["custom".into()];
        cfg.generation
            .speech_providers
            .insert("local".into(), user_entry);
        normalize_voice_local(&mut cfg);
        assert_eq!(
            cfg.generation.speech_providers["local"].models,
            vec!["custom".to_string()]
        );
    }

    #[test]
    fn disable_cleans_up_persisted_synthetic_state() {
        // Enable → normalize → disable → normalize: synthetic state is gone.
        let mut cfg = Config::default();
        cfg.voice_local.local.enabled = true;
        normalize_voice_local(&mut cfg);
        assert!(cfg.generation.speech_providers.contains_key("local"));
        assert!(cfg.generation.transcription_providers.contains_key("local"));

        cfg.voice_local.local.enabled = false;
        normalize_voice_local(&mut cfg);
        assert!(!cfg.generation.speech_providers.contains_key("local"));
        assert!(!cfg.generation.transcription_providers.contains_key("local"));
        assert!(cfg.generation.default_speech_provider.is_none());
        assert!(cfg.generation.default_transcription_provider.is_none());

        // An explicit cloud default set before the disable-normalize survives.
        let mut cfg = Config::default();
        cfg.voice_local.local.enabled = true;
        normalize_voice_local(&mut cfg);
        cfg.generation.default_speech_provider = Some("openai_tts".into());
        cfg.generation.speech_providers.insert(
            "openai_tts".into(),
            crate::GenerationProviderConfig::new("openai_tts"),
        );
        cfg.voice_local.local.enabled = false;
        normalize_voice_local(&mut cfg);
        assert_eq!(
            cfg.generation.default_speech_provider.as_deref(),
            Some("openai_tts")
        );
        assert!(cfg.generation.speech_providers.contains_key("openai_tts"));
        assert!(!cfg.generation.speech_providers.contains_key("local"));
    }

    #[test]
    fn vocabulary_hint_is_none_when_unset_or_blank() {
        let mut v = VoiceSection::default();
        assert!(v.vocabulary_hint().is_none());
        v.vocabulary = vec![String::new(), "   ".into()];
        assert!(
            v.vocabulary_hint().is_none(),
            "blank terms must not produce an empty hint that still hits the wire"
        );
    }

    #[test]
    fn vocabulary_hint_joins_and_trims() {
        let v = VoiceSection {
            vocabulary: vec!["  Aleph ".into(), String::new(), "Leptos".into()],
            ..VoiceSection::default()
        };
        assert_eq!(v.vocabulary_hint().as_deref(), Some("Aleph, Leptos"));
    }

    #[test]
    fn vocabulary_hint_is_bounded_by_the_decoder_budget() {
        // Whisper's initial_prompt competes with the audio for decoder context;
        // terms past the ceiling are dropped whole, never truncated mid-word.
        let v = VoiceSection {
            vocabulary: (0..500).map(|i| format!("term{i:04}")).collect(),
            ..VoiceSection::default()
        };
        let hint = v.vocabulary_hint().expect("some terms fit");
        assert!(hint.chars().count() <= MAX_VOCABULARY_CHARS);
        assert!(
            hint.split(", ")
                .all(|t| t.starts_with("term") && t.len() == 8),
            "no partial term survived: {hint}"
        );
    }

    #[test]
    fn streaming_config_defaults_disabled_and_neutral() {
        let c: StreamingConfig = toml::from_str("").unwrap();
        assert!(!c.enabled);
        assert_eq!(c.provider, "deepgram"); // lingua-franca default protocol, NOT a vendor preference
    }

    #[test]
    fn streaming_config_accepts_self_hosted_endpoint() {
        let c: StreamingConfig = toml::from_str(
            "enabled = true\nprovider = \"whisperlive\"\nbase_url = \"ws://192.168.1.50:9090\"\n",
        )
        .unwrap();
        assert!(c.enabled);
        assert_eq!(c.base_url, "ws://192.168.1.50:9090");
    }

    #[test]
    fn toml_section_parses() {
        let toml = r#"
            [voice.local]
            enabled = true
            endpoint = "http://127.0.0.1:9876/v1"
            tts_voice = "zf_088"
        "#;
        #[derive(serde::Deserialize)]
        struct Wrap {
            voice: Voice,
        }
        #[derive(serde::Deserialize)]
        struct Voice {
            local: VoiceLocalConfig,
        }
        let w: Wrap = ::toml::from_str(toml).unwrap();
        assert!(w.voice.local.enabled);
        assert_eq!(w.voice.local.endpoint, "http://127.0.0.1:9876/v1");
        assert_eq!(w.voice.local.tts_voice, "zf_088");
        assert!(w.voice.local.api_key.is_none());
        // Unset model names stay empty = server default.
        assert_eq!(w.voice.local.stt_model, "");
        assert_eq!(w.voice.local.tts_format, "opus");
    }
}
