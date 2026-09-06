use crate::config::Config;
use crate::gateway::handlers::generation_providers::resolve_api_key;
use crate::gateway::handlers::parse_params;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse};
use crate::gateway::security::SharedTokenManager;
use crate::generation::voice_catalog::static_voices_for_provider_type;
use crate::sync_primitives::Arc;
use tokio::sync::RwLock;

/// Get voices for a generation provider
///
/// Resolution order:
/// 1. Try dynamic fetch from provider API (`{base_url}/v1/audio/voices`)
/// 2. Detect model family (`MiniMax`, `OpenAI`, etc.) and return known voices
/// 3. Fall back to the provider's own static catalog
///    ([`static_voices_for_provider_type`])
pub async fn handle_voices(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    vault: Arc<SharedTokenManager>,
) -> JsonRpcResponse {
    #[derive(serde::Deserialize)]
    struct Params {
        provider_id: String,
    }

    let params: Params = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let cfg = config.read().await;

    // Find provider config and type
    let provider_info = cfg
        .generation
        .merged_providers()
        .into_iter()
        .find(|(name, _, _)| name == &params.provider_id);

    let (provider_type, models) = match &provider_info {
        Some((_, pcfg, _)) => (pcfg.provider_type.to_lowercase(), pcfg.models.clone()),
        None => (params.provider_id.to_lowercase(), vec![]),
    };

    // Resolve the key from the in-memory entry FIRST, then the vault. The entry
    // is where it lives for the BYO `local` provider — `normalize_voice_local`
    // writes the (commonly empty) key onto the synthetic entry and never into
    // the vault. A vault-only lookup therefore returned `None` for it, which
    // *skipped the dynamic fetch entirely*: the default local voice setup showed
    // an empty picker even though its server serves `/audio/voices`.
    let api_key = provider_info
        .as_ref()
        .and_then(|(_, pcfg, _)| pcfg.api_key.clone())
        .filter(|k| !k.is_empty())
        .or_else(|| resolve_api_key(&params.provider_id, &vault));

    // Step 1: Try dynamic fetch from provider API. Gated on having a URL, NOT on
    // having a key — unauthenticated BYO endpoints are the common local case.
    if let Some((_, pcfg, _)) = &provider_info {
        // Use explicit voices_url if configured, otherwise derive from base_url
        let voices_url = if let Some(ref explicit_url) = pcfg.voices_url {
            explicit_url.clone()
        } else if let Some(ref base) = pcfg.base_url {
            let base = base.trim_end_matches('/');
            let base = base
                .strip_suffix("/v1")
                .unwrap_or(base)
                .trim_end_matches('/');
            format!("{base}/v1/audio/voices")
        } else {
            String::new()
        };

        if !voices_url.is_empty() {
            match fetch_voices_from_api(&voices_url, api_key.as_deref().unwrap_or_default()).await {
                Ok(voices) if !voices.is_empty() => {
                    return JsonRpcResponse::success(
                        request.id,
                        serde_json::to_value(voices).unwrap_or_default(),
                    );
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::debug!(error = %e, "dynamic voice fetch failed; using static list");
                }
            }
        }
    }

    // Step 2: Detect model family from configured models
    let voices = detect_voices_by_model(&models, &provider_type);

    JsonRpcResponse::success(request.id, serde_json::to_value(voices).unwrap_or_default())
}

/// Try to fetch voices from a provider's API endpoint. An empty `api_key` means
/// an unauthenticated endpoint — send no `Authorization` header at all rather
/// than an empty bearer, which some servers reject outright.
async fn fetch_voices_from_api(
    url: &str,
    api_key: &str,
) -> Result<Vec<crate::generation::VoiceInfo>, String> {
    let client = reqwest::Client::new();
    let mut req = client.get(url).timeout(std::time::Duration::from_secs(5));
    if !api_key.is_empty() {
        req = req.bearer_auth(api_key);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| format!("GET {url} failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        return Err(format!("GET {url} returned HTTP {status}"));
    }

    // Try OpenAI-style response: { "voices": [...] } or direct array
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("GET {url}: failed to parse JSON body: {e}"))?;

    // Format: { "voices": [{ "voice_id": "...", "name": "..." }] }
    if let Some(arr) = body.get("voices").and_then(|v| v.as_array()) {
        let voices: Vec<crate::generation::VoiceInfo> = arr
            .iter()
            .filter_map(|v| {
                let id = v.get("voice_id").or(v.get("id"))?.as_str()?;
                let name = v.get("name").and_then(|n| n.as_str()).unwrap_or(id);
                let gender = v
                    .get("gender")
                    .and_then(|g| g.as_str())
                    .unwrap_or("neutral");
                let desc = v.get("description").and_then(|d| d.as_str()).unwrap_or("");
                Some(crate::generation::VoiceInfo {
                    id: id.to_string(),
                    name: name.to_string(),
                    gender: gender.to_string(),
                    description: desc.to_string(),
                })
            })
            .collect();
        return Ok(voices);
    }

    // Format: direct array [{ "id": "...", "name": "..." }]
    if let Some(arr) = body.as_array() {
        let voices: Vec<crate::generation::VoiceInfo> = arr
            .iter()
            .filter_map(|v| serde_json::from_value(v.clone()).ok())
            .collect();
        if !voices.is_empty() {
            return Ok(voices);
        }
    }

    Err(format!("GET {url}: response contains no voices array"))
}

/// Detect appropriate voice list based on model names and provider type.
fn detect_voices_by_model(
    models: &[String],
    provider_type: &str,
) -> Vec<crate::generation::VoiceInfo> {
    // Check if any model is a MiniMax/Hailuo model
    let has_minimax = models.iter().any(|m| {
        let lower = m.to_lowercase();
        lower.contains("speech-2")
            || lower.contains("speech-01")
            || lower.contains("speech-02")
            || lower.contains("minimax")
    });

    if has_minimax {
        return minimax_voice_list();
    }

    // Check if any model is a gpt-4o-mini-tts model
    let has_gpt4o_tts = models.iter().any(|m| m.contains("gpt-4o"));
    if has_gpt4o_tts {
        return gpt4o_tts_voice_list();
    }

    // SiliconFlow TTS reuses the OpenAI-compatible provider type, so it can't be
    // told apart by `provider_type` alone — detect it by model family. Its API
    // expects `voice` in `model:voice` form, so each system voice is prefixed
    // with the configured model.
    if let Some(model) = models.iter().find(|m| is_siliconflow_tts_model(m)) {
        return siliconflow_voice_list(model);
    }

    // Fall back to the provider's OWN static catalog, via the single
    // provider_type → voices join point. The hand-maintained match this
    // replaced covered only 4 of the 7 speech provider types, so `cartesia`,
    // `azure_speech` and `deepgram_tts` returned an empty picker despite each
    // shipping a full `static_voice_list()`.
    static_voices_for_provider_type(provider_type)
}

/// True when the model id is a SiliconFlow TTS model family.
fn is_siliconflow_tts_model(model: &str) -> bool {
    let m = model.to_lowercase();
    m.contains("cosyvoice")
        || m.contains("moss-ttsd")
        || m.contains("fish-speech")
        || m.contains("gpt-sovits")
}

/// SiliconFlow system TTS voices. The API expects `voice` in
/// `model:voice` form (e.g. `FunAudioLLM/CosyVoice2-0.5B:alex`), so every id is
/// prefixed with the configured model.
fn siliconflow_voice_list(model: &str) -> Vec<crate::generation::VoiceInfo> {
    use crate::generation::VoiceInfo;
    const VOICES: &[(&str, &str, &str, &str)] = &[
        ("alex", "Alex", "male", "Calm male"),
        ("benjamin", "Benjamin", "male", "Deep male"),
        ("charles", "Charles", "male", "Magnetic male"),
        ("david", "David", "male", "Cheerful male"),
        ("anna", "Anna", "female", "Calm female"),
        ("bella", "Bella", "female", "Passionate female"),
        ("claire", "Claire", "female", "Gentle female"),
        ("diana", "Diana", "female", "Cheerful female"),
    ];
    VOICES
        .iter()
        .map(|(id, name, gender, desc)| VoiceInfo {
            id: format!("{model}:{id}"),
            name: (*name).to_string(),
            gender: (*gender).to_string(),
            description: (*desc).to_string(),
        })
        .collect()
}

/// MiniMax/Hailuo TTS voice list. Delegates to the provider so the
/// picker and the provider share one source of truth — the modern
/// `speech-2.x` / `speech-02` voice taxonomy (the classic `male-qn-*` ids are
/// `speech-01`-only and are rejected by the current default model).
fn minimax_voice_list() -> Vec<crate::generation::VoiceInfo> {
    crate::generation::providers::MinimaxTtsProvider::static_voice_list()
}

/// GPT-4o-mini-tts voice list.
fn gpt4o_tts_voice_list() -> Vec<crate::generation::VoiceInfo> {
    use crate::generation::VoiceInfo;
    vec![
        VoiceInfo {
            id: "coral".into(),
            name: "Coral".into(),
            gender: "female".into(),
            description: "Warm, conversational".into(),
        },
        VoiceInfo {
            id: "sage".into(),
            name: "Sage".into(),
            gender: "female".into(),
            description: "Calm, thoughtful".into(),
        },
        VoiceInfo {
            id: "ash".into(),
            name: "Ash".into(),
            gender: "male".into(),
            description: "Confident, direct".into(),
        },
        VoiceInfo {
            id: "ballad".into(),
            name: "Ballad".into(),
            gender: "male".into(),
            description: "Warm, engaging".into(),
        },
        VoiceInfo {
            id: "verse".into(),
            name: "Verse".into(),
            gender: "male".into(),
            description: "Versatile, dynamic".into(),
        },
        VoiceInfo {
            id: "alloy".into(),
            name: "Alloy".into(),
            gender: "neutral".into(),
            description: "Neutral, balanced".into(),
        },
        VoiceInfo {
            id: "echo".into(),
            name: "Echo".into(),
            gender: "male".into(),
            description: "Warm, conversational".into(),
        },
        VoiceInfo {
            id: "fable".into(),
            name: "Fable".into(),
            gender: "neutral".into(),
            description: "Expressive, animated".into(),
        },
        VoiceInfo {
            id: "onyx".into(),
            name: "Onyx".into(),
            gender: "male".into(),
            description: "Deep, authoritative".into(),
        },
        VoiceInfo {
            id: "nova".into(),
            name: "Nova".into(),
            gender: "female".into(),
            description: "Warm, friendly".into(),
        },
        VoiceInfo {
            id: "shimmer".into(),
            name: "Shimmer".into(),
            gender: "female".into(),
            description: "Clear, bright".into(),
        },
    ]
}

#[cfg(test)]
mod tests {

    use crate::config::types::generation::GenerationProviderConfig;
    use crate::gateway::handlers::generation_providers::build_generation_provider_for_persistence;

    /// A config with no base_url and no model takes the preset's.
    ///
    /// It used to bind `_persisted` and assert nothing at all, so it passed for
    /// every possible behaviour of the function it names — including the
    /// behaviour where the preset lookup returns `None` and nothing is applied
    /// (判据 §2: in what case does this go red?). The preset's own existence is
    /// asserted first, or "the preset was empty" and "the merge works" produce
    /// the same green.
    #[test]
    fn test_build_generation_provider_applies_preset_defaults() {
        let overrides = crate::config::presets_override::GenerationPresetsOverride::default();
        let preset = crate::config::types::generation::presets::get_merged_generation_preset(
            "dalle_main",
            "openai",
            &overrides,
        )
        .expect("the 'openai' provider type has a builtin preset to merge from");
        assert!(
            preset.base_url.is_some() && !preset.default_model.is_empty(),
            "the preset carries nothing to apply, so this test could not fail"
        );

        let cfg = GenerationProviderConfig::new("openai");
        assert!(cfg.base_url.is_none() && cfg.models.is_empty());

        let persisted =
            build_generation_provider_for_persistence("dalle_main", cfg, &overrides, None);
        assert_eq!(persisted.base_url, preset.base_url);
        assert_eq!(persisted.models, vec![preset.default_model]);
    }

    #[test]
    fn volcengine_tts_voices_are_wired_through() {
        let voices = super::detect_voices_by_model(&[], "volcengine_tts");
        assert!(!voices.is_empty(), "Volcengine picker must not be empty");
        assert!(voices
            .iter()
            .any(|v| v.id == "zh_female_cancan_mars_bigtts"));
    }

    #[test]
    fn siliconflow_tts_voices_are_model_prefixed() {
        let models = vec!["FunAudioLLM/CosyVoice2-0.5B".to_string()];
        // provider_type collides with OpenAI's; detection is by model family.
        let voices = super::detect_voices_by_model(&models, "openai_tts");
        assert_eq!(voices.len(), 8);
        assert!(voices
            .iter()
            .any(|v| v.id == "FunAudioLLM/CosyVoice2-0.5B:alex"));
        // Must NOT leak the generic OpenAI voices.
        assert!(!voices.iter().any(|v| v.id == "alloy"));
    }

    #[test]
    fn openai_tts_voices_unaffected_by_siliconflow_detection() {
        let models = vec!["tts-1".to_string()];
        let voices = super::detect_voices_by_model(&models, "openai_tts");
        assert!(voices.iter().any(|v| v.id == "alloy"));
    }

    #[test]
    fn previously_empty_provider_types_now_resolve() {
        // Regression: the old hand-maintained match had no arm for these three,
        // so the Settings voice picker was empty even though each provider ships
        // a full static catalog. They now route through the single join point.
        for t in ["cartesia", "azure_speech", "deepgram_tts"] {
            assert!(
                !super::detect_voices_by_model(&[], t).is_empty(),
                "{t} must resolve voices"
            );
        }
    }

    #[test]
    fn local_provider_has_no_static_guess() {
        // A BYO server's voices are whatever IT serves — discovered live from
        // `{endpoint}/audio/voices`, never guessed from a table.
        assert!(super::detect_voices_by_model(&[], "local").is_empty());
    }
}
