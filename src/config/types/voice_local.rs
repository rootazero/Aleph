//! Local voice sidecar configuration ([voice.local] in config.toml).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Configuration for the aleph-voice local inference sidecar.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct VoiceLocalConfig {
    /// Master switch. Off by default — enabling injects a "local" provider
    /// into the generation provider maps at load time (fill-empty-only).
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_stt_model")]
    pub stt_model: String,
    #[serde(default = "default_tts_model")]
    pub tts_model: String,
    #[serde(default = "default_tts_voice")]
    pub tts_voice: String,
    /// TTS output container: "opus" (Telegram-native) or "wav".
    #[serde(default = "default_tts_format")]
    pub tts_format: String,
    #[serde(default = "default_idle_tts")]
    pub idle_unload_tts_secs: u64,
    #[serde(default = "default_idle_stt")]
    pub idle_unload_stt_secs: u64,
    #[serde(default = "default_idle_exit")]
    pub idle_exit_secs: u64,
    /// auto | github | hf-mirror.
    #[serde(default = "default_download_source")]
    pub download_source: String,
    /// Override the sidecar binary path (default: sibling of aleph-server).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary_path: Option<PathBuf>,
}

fn default_stt_model() -> String {
    "sense-voice-small".into()
}
fn default_tts_model() -> String {
    "kokoro-v1.1-zh".into()
}
fn default_tts_voice() -> String {
    "zf_001".into()
}
fn default_tts_format() -> String {
    "opus".into()
}
const fn default_idle_tts() -> u64 {
    120
}
const fn default_idle_stt() -> u64 {
    600
}
const fn default_idle_exit() -> u64 {
    1800
}
fn default_download_source() -> String {
    "auto".into()
}

impl Default for VoiceLocalConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            stt_model: default_stt_model(),
            tts_model: default_tts_model(),
            tts_voice: default_tts_voice(),
            tts_format: default_tts_format(),
            idle_unload_tts_secs: default_idle_tts(),
            idle_unload_stt_secs: default_idle_stt(),
            idle_exit_secs: default_idle_exit(),
            download_source: default_download_source(),
            binary_path: None,
        }
    }
}

/// `[voice]` config section wrapper (currently only `local`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct VoiceSection {
    #[serde(default)]
    pub local: VoiceLocalConfig,
}

/// Provider name the sidecar registers under.
pub const LOCAL_PROVIDER_NAME: &str = "local";
/// `GenerationProviderConfig.provider_type` for the sidecar.
pub const LOCAL_PROVIDER_TYPE: &str = "local";

/// Load-time normalization: when local voice is enabled, inject a synthetic
/// "local" provider into the speech/transcription maps and point the unset
/// defaults at it. Fill-empty-only — explicit user config (cloud included)
/// always wins. Idempotent: safe across hot reloads.
///
/// Persistence + ownership semantics:
/// - Synthetic state may legitimately end up persisted to disk by any full
///   config save; that is accepted.
/// - When disabled, the cleanup branch removes that state on the next
///   load/patch, so an enable→disable round-trip is clean.
/// - Entries with `provider_type == "local"` are owned by this mechanism:
///   user customizations of them survive enable-time normalization (the
///   `or_insert_with` never overwrites), but they are removed on disable —
///   a "local" provider_type is meaningless without the sidecar.
pub fn normalize_voice_local(cfg: &mut crate::config::structs::Config) {
    if !cfg.local_voice().enabled {
        // Disable-cleanup: remove every sidecar-owned entry, then reset any
        // default that pointed at the now-gone "local" key. Defaults pointing
        // anywhere else are never touched.
        cfg.generation
            .speech_providers
            .retain(|_, p| p.provider_type != LOCAL_PROVIDER_TYPE);
        cfg.generation
            .transcription_providers
            .retain(|_, p| p.provider_type != LOCAL_PROVIDER_TYPE);
        if cfg.generation.default_speech_provider.as_deref() == Some(LOCAL_PROVIDER_NAME)
            && !cfg
                .generation
                .speech_providers
                .contains_key(LOCAL_PROVIDER_NAME)
        {
            cfg.generation.default_speech_provider = None;
        }
        if cfg.generation.default_transcription_provider.as_deref() == Some(LOCAL_PROVIDER_NAME)
            && !cfg
                .generation
                .transcription_providers
                .contains_key(LOCAL_PROVIDER_NAME)
        {
            cfg.generation.default_transcription_provider = None;
        }
        return;
    }
    use crate::generation::GenerationType;

    let tts_model = cfg.voice_local.local.tts_model.clone();
    let stt_model = cfg.voice_local.local.stt_model.clone();

    let synth = |cap: GenerationType, model: &str| {
        let mut p = crate::GenerationProviderConfig::new(LOCAL_PROVIDER_TYPE);
        // Placeholder key keeps existing api_key-presence walks selecting it;
        // the real per-spawn token is injected by the supervisor at call time.
        p.api_key = Some("local-sidecar".into());
        p.capabilities = vec![cap];
        p.models = vec![model.to_string()];
        p
    };

    cfg.generation
        .speech_providers
        .entry(LOCAL_PROVIDER_NAME.into())
        .or_insert_with(|| synth(GenerationType::Speech, &tts_model));
    cfg.generation
        .transcription_providers
        .entry(LOCAL_PROVIDER_NAME.into())
        .or_insert_with(|| synth(GenerationType::Transcription, &stt_model));

    if cfg.generation.default_speech_provider.is_none() {
        cfg.generation.default_speech_provider = Some(LOCAL_PROVIDER_NAME.into());
    }
    if cfg.generation.default_transcription_provider.is_none() {
        cfg.generation.default_transcription_provider = Some(LOCAL_PROVIDER_NAME.into());
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
        assert_eq!(p.models, vec!["kokoro-v1.1-zh".to_string()]);
        // Validation must accept the synthetic reference (spec: validate passes).
        cfg.generation.validate().unwrap();
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
    fn toml_section_parses() {
        let toml = r#"
            [voice.local]
            enabled = true
            tts_voice = "zf_088"
            idle_exit_secs = 900
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
        assert_eq!(w.voice.local.tts_voice, "zf_088");
        assert_eq!(w.voice.local.idle_exit_secs, 900);
        assert_eq!(w.voice.local.idle_unload_tts_secs, 120);
    }
}
