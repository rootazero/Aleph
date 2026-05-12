use crate::gateway::handlers::parse_params;
use crate::gateway::handlers::generation_providers::resolve_api_key;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse};
use crate::config::Config;
use crate::gateway::security::SharedTokenManager;
use crate::generation::providers::{ElevenLabsProvider, OpenAiTtsProvider};
use crate::sync_primitives::Arc;
use tokio::sync::RwLock;

/// Get voices for a generation provider
///
/// Resolution order:
/// 1. Try dynamic fetch from provider API (`{base_url}/v1/audio/voices`)
/// 2. Detect model family (MiniMax, OpenAI, etc.) and return known voices
/// 3. Fall back to static list by provider_type
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

    // Resolve API key from vault
    let api_key = resolve_api_key(&params.provider_id, &vault);

    // Step 1: Try dynamic fetch from provider API
    if let (Some(ref key), Some((_, pcfg, _))) = (&api_key, &provider_info) {
        // Use explicit voices_url if configured, otherwise derive from base_url
        let voices_url = if let Some(ref explicit_url) = pcfg.voices_url {
            explicit_url.clone()
        } else if let Some(ref base) = pcfg.base_url {
            let base = base.trim_end_matches('/');
            let base = base
                .strip_suffix("/v1")
                .unwrap_or(base)
                .trim_end_matches('/');
            format!("{}/v1/audio/voices", base)
        } else {
            String::new()
        };

        if !voices_url.is_empty() {
            if let Ok(voices) = fetch_voices_from_api(&voices_url, key).await {
                if !voices.is_empty() {
                    return JsonRpcResponse::success(
                        request.id,
                        serde_json::to_value(voices).unwrap_or_default(),
                    );
                }
            }
        }
    }

    // Step 2: Detect model family from configured models
    let voices = detect_voices_by_model(&models, &provider_type);

    JsonRpcResponse::success(request.id, serde_json::to_value(voices).unwrap_or_default())
}

/// Try to fetch voices from a provider's API endpoint.
async fn fetch_voices_from_api(
    url: &str,
    api_key: &str,
) -> Result<Vec<crate::generation::VoiceInfo>, ()> {
    let client = reqwest::Client::new();
    let resp = client
        .get(url)
        .bearer_auth(api_key)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .map_err(|_| ())?;

    if !resp.status().is_success() {
        return Err(());
    }

    // Try OpenAI-style response: { "voices": [...] } or direct array
    let body: serde_json::Value = resp.json().await.map_err(|_| ())?;

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

    Err(())
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

    // Fall back to static list by provider_type
    match provider_type {
        "openai" | "openai-tts" | "openai_tts" | "openai_compat" | "openai-compat" => {
            OpenAiTtsProvider::static_voice_list()
        }
        "elevenlabs" => ElevenLabsProvider::static_voice_list(),
        _ => vec![],
    }
}

/// MiniMax/Hailuo (海螺) TTS voice list.
fn minimax_voice_list() -> Vec<crate::generation::VoiceInfo> {
    use crate::generation::VoiceInfo;
    vec![
        VoiceInfo {
            id: "male-qn-qingse".into(),
            name: "青涩青年".into(),
            gender: "male".into(),
            description: "清新、年轻的男声".into(),
        },
        VoiceInfo {
            id: "male-qn-jingying".into(),
            name: "精英青年".into(),
            gender: "male".into(),
            description: "自信、专业的男声".into(),
        },
        VoiceInfo {
            id: "male-qn-badao".into(),
            name: "霸道青年".into(),
            gender: "male".into(),
            description: "有力、阳刚的男声".into(),
        },
        VoiceInfo {
            id: "male-qn-daxuesheng".into(),
            name: "大学生青年".into(),
            gender: "male".into(),
            description: "活泼、阳光的男声".into(),
        },
        VoiceInfo {
            id: "female-shaonv".into(),
            name: "少女".into(),
            gender: "female".into(),
            description: "清新、甜美的女声".into(),
        },
        VoiceInfo {
            id: "female-yujie".into(),
            name: "御姐".into(),
            gender: "female".into(),
            description: "成熟、知性的女声".into(),
        },
        VoiceInfo {
            id: "female-chengshu".into(),
            name: "成熟女性".into(),
            gender: "female".into(),
            description: "温和、优雅的女声".into(),
        },
        VoiceInfo {
            id: "female-tianmei".into(),
            name: "甜美女性".into(),
            gender: "female".into(),
            description: "温柔、可爱的女声".into(),
        },
        VoiceInfo {
            id: "presenter_male".into(),
            name: "男性主持人".into(),
            gender: "male".into(),
            description: "标准、清晰的男性播报声".into(),
        },
        VoiceInfo {
            id: "presenter_female".into(),
            name: "女性主持人".into(),
            gender: "female".into(),
            description: "标准、清晰的女性播报声".into(),
        },
        VoiceInfo {
            id: "audiobook_male_1".into(),
            name: "有声书男声1".into(),
            gender: "male".into(),
            description: "沉稳、厚重的男声".into(),
        },
        VoiceInfo {
            id: "audiobook_male_2".into(),
            name: "有声书男声2".into(),
            gender: "male".into(),
            description: "温暖、磁性的男声".into(),
        },
        VoiceInfo {
            id: "audiobook_female_1".into(),
            name: "有声书女声1".into(),
            gender: "female".into(),
            description: "温暖、亲和的女声".into(),
        },
        VoiceInfo {
            id: "audiobook_female_2".into(),
            name: "有声书女声2".into(),
            gender: "female".into(),
            description: "柔和、舒缓的女声".into(),
        },
        VoiceInfo {
            id: "Podcast_girl".into(),
            name: "播客女生".into(),
            gender: "female".into(),
            description: "活泼、自然的播客女声".into(),
        },
        // OpenAI-compatible voices also work through proxies
        VoiceInfo {
            id: "alloy".into(),
            name: "Alloy".into(),
            gender: "neutral".into(),
            description: "中性、平衡 (OpenAI)".into(),
        },
        VoiceInfo {
            id: "echo".into(),
            name: "Echo".into(),
            gender: "male".into(),
            description: "温暖、对话感 (OpenAI)".into(),
        },
        VoiceInfo {
            id: "nova".into(),
            name: "Nova".into(),
            gender: "female".into(),
            description: "友好、活泼 (OpenAI)".into(),
        },
        VoiceInfo {
            id: "shimmer".into(),
            name: "Shimmer".into(),
            gender: "female".into(),
            description: "清晰、专业 (OpenAI)".into(),
        },
    ]
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

    #[test]
    fn test_build_generation_provider_applies_preset_defaults() {
        let cfg = GenerationProviderConfig::new("openai");
        let overrides = crate::config::presets_override::GenerationPresetsOverride::default();
        let _persisted = build_generation_provider_for_persistence("dalle_main", cfg, &overrides);
    }
}