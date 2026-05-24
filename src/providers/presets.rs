//! Provider presets registry
//!
//! Contains default configurations for known AI providers.

use crate::providers::metadata::{Modality, ProviderMetadata};
use once_cell::sync::Lazy;
use std::collections::HashMap;

/// Provider preset configuration
#[derive(Debug, Clone)]
pub struct ProviderPreset {
    /// Default base URL for the provider
    pub base_url: &'static str,
    /// Protocol to use (e.g., "openai", "anthropic")
    pub protocol: &'static str,
    /// Default color for UI
    pub color: &'static str,
    /// Default model for the provider
    pub default_model: &'static str,
}

/// Registry of known provider presets
pub static PRESETS: Lazy<HashMap<&'static str, ProviderPreset>> = Lazy::new(|| {
    let mut m = HashMap::new();

    // OpenAI official
    m.insert(
        "openai",
        ProviderPreset {
            base_url: "https://api.openai.com/v1",
            protocol: "openai",
            color: "#10a37f",
            default_model: "gpt-4o",
        },
    );

    // ChatGPT subscription (via Codex Responses API, OAuth login)
    m.insert(
        "chatgpt",
        ProviderPreset {
            base_url: "https://chatgpt.com",
            protocol: "codex",
            color: "#10a37f",
            default_model: "gpt-5.4",
        },
    );

    // DeepSeek
    m.insert(
        "deepseek",
        ProviderPreset {
            base_url: "https://api.deepseek.com",
            protocol: "openai",
            color: "#0066cc",
            default_model: "deepseek-chat",
        },
    );

    // Moonshot / Kimi — standard chat API (OpenAI-compatible)
    m.insert(
        "moonshot",
        ProviderPreset {
            base_url: "https://api.moonshot.ai/v1",
            protocol: "openai",
            color: "#6366f1",
            default_model: "kimi-k2-0905-preview",
        },
    );
    m.insert(
        "kimi",
        ProviderPreset {
            base_url: "https://api.moonshot.ai/v1",
            protocol: "openai",
            color: "#6366f1",
            default_model: "kimi-k2-0905-preview",
        },
    );

    // Kimi for Coding — Anthropic-compatible endpoint optimized for IDE/agent
    // tool use (Claude Code, Cline, Roo Code). Outputs tool-call JSON by design,
    // not free-form chat. Use this only when wiring Aleph as a coding agent
    // backend; for general conversation, use the `moonshot` preset above.
    m.insert(
        "kimi-for-coding",
        ProviderPreset {
            base_url: "https://api.kimi.com/coding/v1",
            protocol: "anthropic",
            color: "#6366f1",
            default_model: "Kimi-K2.6",
        },
    );
    m.insert(
        "kimi-coding",
        ProviderPreset {
            base_url: "https://api.kimi.com/coding/v1",
            protocol: "anthropic",
            color: "#6366f1",
            default_model: "Kimi-K2.6",
        },
    );

    // Volcengine Doubao
    m.insert(
        "doubao",
        ProviderPreset {
            base_url: "https://ark.cn-beijing.volces.com/api/v3",
            protocol: "openai",
            color: "#ff6b35",
            default_model: "doubao-1.5-pro-256k",
        },
    );
    m.insert(
        "volcengine",
        ProviderPreset {
            base_url: "https://ark.cn-beijing.volces.com/api/v3",
            protocol: "openai",
            color: "#ff6b35",
            default_model: "doubao-1.5-pro-256k",
        },
    );
    m.insert(
        "ark",
        ProviderPreset {
            base_url: "https://ark.cn-beijing.volces.com/api/v3",
            protocol: "openai",
            color: "#ff6b35",
            default_model: "doubao-1.5-pro-256k",
        },
    );

    // SiliconFlow — Chinese AI cloud platform
    m.insert(
        "siliconflow",
        ProviderPreset {
            base_url: "https://api.siliconflow.cn/v1",
            protocol: "openai",
            color: "#6c5ce7",
            default_model: "deepseek-ai/DeepSeek-V3",
        },
    );

    // Zhipu GLM — Chinese AI research lab
    m.insert(
        "zhipu",
        ProviderPreset {
            base_url: "https://open.bigmodel.cn/api/paas/v4",
            protocol: "openai",
            color: "#3b5998",
            default_model: "GLM-5",
        },
    );
    m.insert(
        "glm",
        ProviderPreset {
            base_url: "https://open.bigmodel.cn/api/paas/v4",
            protocol: "openai",
            color: "#3b5998",
            default_model: "GLM-5",
        },
    );

    // MiniMax — Chinese multimodal AI
    m.insert(
        "minimax",
        ProviderPreset {
            base_url: "https://api.minimax.io/v1",
            protocol: "openai",
            color: "#e84393",
            default_model: "MiniMax-M2.5",
        },
    );

    // T8Star
    m.insert(
        "t8star",
        ProviderPreset {
            base_url: "https://api.t8star.cn/v1",
            protocol: "openai",
            color: "#f59e0b",
            default_model: "",
        },
    );

    // Anthropic Claude
    m.insert(
        "claude",
        ProviderPreset {
            base_url: "https://api.anthropic.com",
            protocol: "anthropic",
            color: "#d97757",
            default_model: "claude-sonnet-4-5-20250514",
        },
    );

    // Google Gemini
    m.insert(
        "gemini",
        ProviderPreset {
            base_url: "https://generativelanguage.googleapis.com",
            protocol: "gemini",
            color: "#4285f4",
            default_model: "gemini-2.5-flash",
        },
    );

    // Groq - Ultra-fast inference
    m.insert(
        "groq",
        ProviderPreset {
            base_url: "https://api.groq.com/openai/v1",
            protocol: "openai",
            color: "#f55036",
            default_model: "llama-3.3-70b-versatile",
        },
    );

    // Together.ai - Open source models
    m.insert(
        "together",
        ProviderPreset {
            base_url: "https://api.together.xyz/v1",
            protocol: "openai",
            color: "#6366f1",
            default_model: "",
        },
    );

    // Perplexity - Search-augmented LLMs
    m.insert(
        "perplexity",
        ProviderPreset {
            base_url: "https://api.perplexity.ai",
            protocol: "openai",
            color: "#20808d",
            default_model: "",
        },
    );

    // Mistral AI - European AI leader
    m.insert(
        "mistral",
        ProviderPreset {
            base_url: "https://api.mistral.ai/v1",
            protocol: "openai",
            color: "#ff7000",
            default_model: "",
        },
    );

    // Cohere - Enterprise focus
    m.insert(
        "cohere",
        ProviderPreset {
            base_url: "https://api.cohere.ai/v1",
            protocol: "openai",
            color: "#39594d",
            default_model: "",
        },
    );

    // Fireworks.ai - Fast API
    m.insert(
        "fireworks",
        ProviderPreset {
            base_url: "https://api.fireworks.ai/inference/v1",
            protocol: "openai",
            color: "#ff6b35",
            default_model: "",
        },
    );

    // Anyscale - Ray ecosystem
    m.insert(
        "anyscale",
        ProviderPreset {
            base_url: "https://api.endpoints.anyscale.com/v1",
            protocol: "openai",
            color: "#00d4aa",
            default_model: "",
        },
    );

    // Replicate - OSS model hosting
    m.insert(
        "replicate",
        ProviderPreset {
            base_url: "https://api.replicate.com/v1",
            protocol: "openai",
            color: "#0c0c0d",
            default_model: "",
        },
    );

    // OpenRouter - Multi-model router (uses Responses API)
    m.insert(
        "openrouter",
        ProviderPreset {
            base_url: "https://openrouter.ai/api",
            protocol: "openai-responses",
            color: "#6467f2",
            default_model: "openai/gpt-4o",
        },
    );

    // Lepton AI - Model deployment
    m.insert(
        "lepton",
        ProviderPreset {
            base_url: "https://api.lepton.ai/api/v1",
            protocol: "openai",
            color: "#4f46e5",
            default_model: "",
        },
    );

    // Hyperbolic - GPU marketplace
    m.insert(
        "hyperbolic",
        ProviderPreset {
            base_url: "https://api.hyperbolic.xyz/v1",
            protocol: "openai",
            color: "#8b5cf6",
            default_model: "",
        },
    );

    // -------------------------------------------------------------------------
    // Phase B (openclaw parity) — additional chat presets. All map onto an
    // existing protocol adapter; no new protocol code required.
    // -------------------------------------------------------------------------

    // Cerebras — ultra-fast Llama inference, OpenAI-compatible
    m.insert(
        "cerebras",
        ProviderPreset {
            base_url: "https://api.cerebras.ai/v1",
            protocol: "openai",
            color: "#f97316",
            default_model: "llama-3.3-70b",
        },
    );

    // Stepfun — Chinese multimodal LLM, OpenAI-compatible
    m.insert(
        "stepfun",
        ProviderPreset {
            base_url: "https://api.stepfun.com/v1",
            protocol: "openai",
            color: "#0ea5e9",
            default_model: "step-1-8k",
        },
    );

    // HuggingFace Router — OpenAI-compatible front for HF Inference API
    m.insert(
        "huggingface",
        ProviderPreset {
            base_url: "https://router.huggingface.co/v1",
            protocol: "openai",
            color: "#ffd21e",
            default_model: "meta-llama/Llama-3.3-70B-Instruct",
        },
    );

    // Vertex AI (Anthropic on GCP) — uses Anthropic protocol, region-specific URL
    // Default region is us-east5; users can override via base_url for other regions.
    m.insert(
        "vertex-anthropic",
        ProviderPreset {
            base_url: "https://us-east5-aiplatform.googleapis.com/v1",
            protocol: "anthropic",
            color: "#4285f4",
            default_model: "claude-sonnet-4@20250514",
        },
    );

    // Azure OpenAI — OpenAI-compatible, but users must override base_url with
    // their resource endpoint (https://<resource>.openai.azure.com). The
    // placeholder is intentional so misconfiguration is obvious.
    m.insert(
        "azure-openai",
        ProviderPreset {
            base_url: "https://YOUR-RESOURCE.openai.azure.com",
            protocol: "openai",
            color: "#0078d4",
            default_model: "gpt-4o",
        },
    );

    // xAI Grok — OpenAI-compatible
    m.insert(
        "xai",
        ProviderPreset {
            base_url: "https://api.x.ai/v1",
            protocol: "openai",
            color: "#000000",
            default_model: "grok-4-0709",
        },
    );
    m.insert(
        "grok",
        ProviderPreset {
            base_url: "https://api.x.ai/v1",
            protocol: "openai",
            color: "#000000",
            default_model: "grok-4-0709",
        },
    );

    // Qwen / DashScope — Alibaba 通义, OpenAI-compatible endpoint
    m.insert(
        "qwen",
        ProviderPreset {
            base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1",
            protocol: "openai",
            color: "#615ced",
            default_model: "qwen-max-2025-01-25",
        },
    );
    m.insert(
        "dashscope",
        ProviderPreset {
            base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1",
            protocol: "openai",
            color: "#615ced",
            default_model: "qwen-max-2025-01-25",
        },
    );

    // Baichuan 百川 — OpenAI-compatible
    m.insert(
        "baichuan",
        ProviderPreset {
            base_url: "https://api.baichuan-ai.com/v1",
            protocol: "openai",
            color: "#e11d48",
            default_model: "Baichuan4",
        },
    );

    // Hunyuan 腾讯混元 — OpenAI-compatible
    m.insert(
        "hunyuan",
        ProviderPreset {
            base_url: "https://api.hunyuan.cloud.tencent.com/v1",
            protocol: "openai",
            color: "#1e40af",
            default_model: "hunyuan-pro",
        },
    );

    // Spark 讯飞星火 — V4 Ultra OpenAI-compatible endpoint
    m.insert(
        "spark",
        ProviderPreset {
            base_url: "https://spark-api-open.xf-yun.com/v1",
            protocol: "openai",
            color: "#ff4d4f",
            default_model: "4.0Ultra",
        },
    );

    m
});

// =============================================================================
// Provider metadata (modality / display name / homepage)
// =============================================================================
//
// Stored in a parallel map rather than embedded in `ProviderPreset` so adding
// it is zero-churn for the 28 existing entries above. Lookups by name are
// case-insensitive at the call site (we lowercase before query).

const CHAT_ONLY: &[Modality] = &[Modality::Chat];

/// Per-preset metadata for chat providers — display name, modalities,
/// homepage. Used by panel/RPC catalog and modality-based routing.
///
/// Not every preset needs an entry; missing names fall back to the
/// default-chat assumption (see [`provider_metadata`] / [`presets_by_modality`]).
pub static PRESET_METADATA: Lazy<HashMap<&'static str, ProviderMetadata>> = Lazy::new(|| {
    let mut m: HashMap<&'static str, ProviderMetadata> = HashMap::new();

    m.insert(
        "openai",
        ProviderMetadata {
            display_name: "OpenAI",
            modalities: CHAT_ONLY,
            homepage: Some("https://platform.openai.com"),
            notes: None,
        },
    );
    m.insert(
        "chatgpt",
        ProviderMetadata {
            display_name: "ChatGPT (Codex Login)",
            modalities: CHAT_ONLY,
            homepage: Some("https://chatgpt.com"),
            notes: Some("OAuth login, Codex Responses protocol"),
        },
    );
    m.insert(
        "deepseek",
        ProviderMetadata {
            display_name: "DeepSeek",
            modalities: CHAT_ONLY,
            homepage: Some("https://platform.deepseek.com"),
            notes: None,
        },
    );
    m.insert(
        "moonshot",
        ProviderMetadata {
            display_name: "Moonshot / Kimi",
            modalities: CHAT_ONLY,
            homepage: Some("https://platform.moonshot.ai"),
            notes: None,
        },
    );
    m.insert(
        "kimi-for-coding",
        ProviderMetadata {
            display_name: "Kimi for Coding",
            modalities: CHAT_ONLY,
            homepage: Some("https://platform.moonshot.ai"),
            notes: Some("Anthropic-protocol endpoint for IDE agents"),
        },
    );
    m.insert(
        "doubao",
        ProviderMetadata {
            display_name: "Volcengine Doubao",
            modalities: CHAT_ONLY,
            homepage: Some("https://www.volcengine.com/product/ark"),
            notes: None,
        },
    );
    m.insert(
        "siliconflow",
        ProviderMetadata {
            display_name: "SiliconFlow",
            modalities: CHAT_ONLY,
            homepage: Some("https://siliconflow.cn"),
            notes: None,
        },
    );
    m.insert(
        "zhipu",
        ProviderMetadata {
            display_name: "Zhipu GLM",
            modalities: CHAT_ONLY,
            homepage: Some("https://open.bigmodel.cn"),
            notes: None,
        },
    );
    m.insert(
        "minimax",
        ProviderMetadata {
            display_name: "MiniMax",
            modalities: CHAT_ONLY,
            homepage: Some("https://www.minimax.io"),
            notes: None,
        },
    );
    m.insert(
        "t8star",
        ProviderMetadata {
            display_name: "T8Star",
            modalities: CHAT_ONLY,
            homepage: Some("https://t8star.cn"),
            notes: Some("OpenAI-compatible aggregator"),
        },
    );
    m.insert(
        "claude",
        ProviderMetadata {
            display_name: "Anthropic Claude",
            modalities: CHAT_ONLY,
            homepage: Some("https://docs.anthropic.com"),
            notes: None,
        },
    );
    m.insert(
        "gemini",
        ProviderMetadata {
            display_name: "Google Gemini",
            modalities: CHAT_ONLY,
            homepage: Some("https://ai.google.dev"),
            notes: None,
        },
    );
    m.insert(
        "groq",
        ProviderMetadata {
            display_name: "Groq",
            modalities: CHAT_ONLY,
            homepage: Some("https://groq.com"),
            notes: Some("Ultra-fast inference"),
        },
    );
    m.insert(
        "together",
        ProviderMetadata {
            display_name: "Together.ai",
            modalities: CHAT_ONLY,
            homepage: Some("https://www.together.ai"),
            notes: None,
        },
    );
    m.insert(
        "perplexity",
        ProviderMetadata {
            display_name: "Perplexity",
            modalities: CHAT_ONLY,
            homepage: Some("https://docs.perplexity.ai"),
            notes: Some("Search-augmented LLMs"),
        },
    );
    m.insert(
        "mistral",
        ProviderMetadata {
            display_name: "Mistral AI",
            modalities: CHAT_ONLY,
            homepage: Some("https://docs.mistral.ai"),
            notes: None,
        },
    );
    m.insert(
        "cohere",
        ProviderMetadata {
            display_name: "Cohere",
            modalities: CHAT_ONLY,
            homepage: Some("https://docs.cohere.com"),
            notes: None,
        },
    );
    m.insert(
        "fireworks",
        ProviderMetadata {
            display_name: "Fireworks.ai",
            modalities: CHAT_ONLY,
            homepage: Some("https://fireworks.ai"),
            notes: None,
        },
    );
    m.insert(
        "anyscale",
        ProviderMetadata {
            display_name: "Anyscale",
            modalities: CHAT_ONLY,
            homepage: Some("https://www.anyscale.com"),
            notes: None,
        },
    );
    m.insert(
        "replicate",
        ProviderMetadata {
            display_name: "Replicate",
            modalities: CHAT_ONLY,
            homepage: Some("https://replicate.com"),
            notes: Some("Hosting; image/video via generation layer"),
        },
    );
    m.insert(
        "openrouter",
        ProviderMetadata {
            display_name: "OpenRouter",
            modalities: CHAT_ONLY,
            homepage: Some("https://openrouter.ai"),
            notes: Some("Multi-model router"),
        },
    );
    m.insert(
        "lepton",
        ProviderMetadata {
            display_name: "Lepton AI",
            modalities: CHAT_ONLY,
            homepage: Some("https://www.lepton.ai"),
            notes: None,
        },
    );
    m.insert(
        "hyperbolic",
        ProviderMetadata {
            display_name: "Hyperbolic",
            modalities: CHAT_ONLY,
            homepage: Some("https://hyperbolic.xyz"),
            notes: None,
        },
    );

    // Phase B chat additions
    m.insert(
        "cerebras",
        ProviderMetadata {
            display_name: "Cerebras",
            modalities: CHAT_ONLY,
            homepage: Some("https://cerebras.ai"),
            notes: Some("Ultra-fast Llama inference"),
        },
    );
    m.insert(
        "stepfun",
        ProviderMetadata {
            display_name: "Stepfun",
            modalities: CHAT_ONLY,
            homepage: Some("https://stepfun.com"),
            notes: None,
        },
    );
    m.insert(
        "huggingface",
        ProviderMetadata {
            display_name: "HuggingFace Inference",
            modalities: CHAT_ONLY,
            homepage: Some("https://huggingface.co/docs/api-inference"),
            notes: Some("Routes to community-hosted open models"),
        },
    );
    m.insert(
        "vertex-anthropic",
        ProviderMetadata {
            display_name: "Vertex AI — Anthropic",
            modalities: CHAT_ONLY,
            homepage: Some(
                "https://cloud.google.com/vertex-ai/generative-ai/docs/partner-models/claude",
            ),
            notes: Some("Claude via GCP Vertex; region-specific base URL"),
        },
    );
    m.insert(
        "azure-openai",
        ProviderMetadata {
            display_name: "Azure OpenAI",
            modalities: CHAT_ONLY,
            homepage: Some("https://learn.microsoft.com/azure/ai-services/openai"),
            notes: Some("Override base_url with your Azure resource"),
        },
    );
    m.insert(
        "xai",
        ProviderMetadata {
            display_name: "xAI Grok",
            modalities: CHAT_ONLY,
            homepage: Some("https://docs.x.ai"),
            notes: None,
        },
    );
    m.insert(
        "qwen",
        ProviderMetadata {
            display_name: "Qwen / 通义",
            modalities: CHAT_ONLY,
            homepage: Some("https://help.aliyun.com/zh/dashscope"),
            notes: Some("Alibaba DashScope compatible endpoint"),
        },
    );
    m.insert(
        "baichuan",
        ProviderMetadata {
            display_name: "Baichuan / 百川",
            modalities: CHAT_ONLY,
            homepage: Some("https://platform.baichuan-ai.com"),
            notes: None,
        },
    );
    m.insert(
        "hunyuan",
        ProviderMetadata {
            display_name: "Hunyuan / 腾讯混元",
            modalities: CHAT_ONLY,
            homepage: Some("https://cloud.tencent.com/document/product/1729"),
            notes: None,
        },
    );
    m.insert(
        "spark",
        ProviderMetadata {
            display_name: "Spark / 讯飞星火",
            modalities: CHAT_ONLY,
            homepage: Some("https://www.xfyun.cn/doc/spark/HTTP%E8%B0%83%E7%94%A8%E6%96%87%E6%A1%A3.html"),
            notes: Some("V3.5+ OpenAI-compatible endpoint"),
        },
    );

    m
});

/// Look up rich metadata for a preset by name (case-insensitive).
///
/// Returns `None` if the preset has no metadata entry — callers can still
/// treat it as a chat-only provider for routing purposes.
pub fn provider_metadata(name: &str) -> Option<&'static ProviderMetadata> {
    PRESET_METADATA.get(name.to_lowercase().as_str())
}

/// All preset names (sorted) that serve the requested modality.
///
/// Presets without an explicit metadata entry are treated as `Chat`-only,
/// matching the historical default — this keeps backward compatibility
/// while letting new multimodal entries opt in via [`PRESET_METADATA`].
pub fn presets_by_modality(modality: Modality) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = PRESETS
        .keys()
        .copied()
        .filter(|name| match PRESET_METADATA.get(*name) {
            Some(meta) => meta.supports(modality),
            // Default assumption for entries without metadata: chat-only.
            None => modality == Modality::Chat,
        })
        .collect();
    out.sort_unstable();
    out
}

/// Get a preset by name (case-insensitive)
pub fn get_preset(name: &str) -> Option<&'static ProviderPreset> {
    PRESETS.get(name.to_lowercase().as_str())
}

/// Get a preset with override support.
///
/// Resolution order:
/// 1. If a user override exists for the name (or via alias), merge it onto the built-in preset.
/// 2. If only a built-in preset exists, convert it to owned form.
/// 3. If only a user override exists (new provider), create from partial.
/// 4. Returns `None` if disabled or not found.
pub fn get_merged_preset(
    name: &str,
    overrides: &crate::config::presets_override::PresetsOverride,
) -> Option<crate::config::presets_override::OwnedProviderPreset> {
    let lower = name.to_lowercase();
    let builtin = PRESETS.get(lower.as_str());
    let partial = overrides.providers.get(&lower).or_else(|| {
        // Check aliases in user overrides
        overrides
            .providers
            .values()
            .find(|p| p.aliases.iter().any(|a| a.to_lowercase() == lower))
    });

    match (builtin, partial) {
        (Some(b), Some(p)) => {
            if !p.enabled {
                return None;
            }
            Some(crate::config::presets_override::merge_provider_preset(b, p))
        }
        (Some(b), None) => Some(crate::config::presets_override::OwnedProviderPreset {
            base_url: b.base_url.to_string(),
            protocol: b.protocol.to_string(),
            color: b.color.to_string(),
            default_model: b.default_model.to_string(),
        }),
        (None, Some(p)) => {
            if !p.enabled {
                return None;
            }
            crate::config::presets_override::partial_to_provider_preset(p)
        }
        (None, None) => None,
    }
}

/// Resolve provider name from model name using known prefix patterns.
pub fn resolve_provider_from_model(model: &str) -> Option<String> {
    let m = model.to_lowercase();
    if m.starts_with("gpt-") || m.starts_with("o1-") || m.starts_with("o3-") || m.starts_with("o4-")
    {
        Some("openai".into())
    } else if m.starts_with("claude-") {
        Some("anthropic".into())
    } else if m.starts_with("gemini-") {
        Some("google".into())
    } else if m.starts_with("deepseek-") {
        Some("deepseek".into())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_presets_contain_known_vendors() {
        // OpenAI-compatible (original)
        assert!(PRESETS.contains_key("deepseek"));
        assert!(PRESETS.contains_key("moonshot"));
        assert!(PRESETS.contains_key("doubao"));
        assert!(PRESETS.contains_key("siliconflow"));
        assert!(PRESETS.contains_key("zhipu"));
        assert!(PRESETS.contains_key("glm"));
        assert!(PRESETS.contains_key("minimax"));
        assert!(PRESETS.contains_key("openai"));

        // Native protocols
        assert!(PRESETS.contains_key("claude"));
        assert!(PRESETS.contains_key("gemini"));

        // Tier 1: High-priority OpenAI-compatible
        assert!(PRESETS.contains_key("groq"));
        assert!(PRESETS.contains_key("together"));
        assert!(PRESETS.contains_key("perplexity"));
        assert!(PRESETS.contains_key("mistral"));

        // Tier 2: Medium-priority OpenAI-compatible
        assert!(PRESETS.contains_key("cohere"));
        assert!(PRESETS.contains_key("fireworks"));
        assert!(PRESETS.contains_key("anyscale"));
        assert!(PRESETS.contains_key("replicate"));

        // Tier 3: Specialized/Regional OpenAI-compatible
        assert!(PRESETS.contains_key("openrouter"));
        assert!(PRESETS.contains_key("lepton"));
        assert!(PRESETS.contains_key("hyperbolic"));

        // Phase B (openclaw parity)
        assert!(PRESETS.contains_key("cerebras"));
        assert!(PRESETS.contains_key("stepfun"));
        assert!(PRESETS.contains_key("huggingface"));
        assert!(PRESETS.contains_key("vertex-anthropic"));
        assert!(PRESETS.contains_key("azure-openai"));
        assert!(PRESETS.contains_key("xai"));
        assert!(PRESETS.contains_key("grok"));
        assert!(PRESETS.contains_key("qwen"));
        assert!(PRESETS.contains_key("dashscope"));
        assert!(PRESETS.contains_key("baichuan"));
        assert!(PRESETS.contains_key("hunyuan"));
        assert!(PRESETS.contains_key("spark"));
    }

    #[test]
    fn test_phase_b_aliases_share_target() {
        // xai / grok point to the same endpoint
        let xai = get_preset("xai").unwrap();
        let grok = get_preset("grok").unwrap();
        assert_eq!(xai.base_url, grok.base_url);

        // qwen / dashscope point to the same endpoint
        let qwen = get_preset("qwen").unwrap();
        let dashscope = get_preset("dashscope").unwrap();
        assert_eq!(qwen.base_url, dashscope.base_url);
    }

    #[test]
    fn test_vertex_anthropic_uses_anthropic_protocol() {
        let p = get_preset("vertex-anthropic").unwrap();
        assert_eq!(p.protocol, "anthropic");
        assert!(p.base_url.contains("aiplatform.googleapis.com"));
    }

    #[test]
    fn test_presets_have_valid_protocol() {
        let valid_protocols = ["openai", "openai-responses", "anthropic", "gemini", "codex"];
        for (name, preset) in PRESETS.iter() {
            assert!(
                valid_protocols.contains(&preset.protocol),
                "Preset '{}' uses invalid protocol '{}'",
                name,
                preset.protocol
            );
        }
    }

    #[test]
    fn test_get_preset_case_insensitive() {
        assert!(get_preset("DeepSeek").is_some());
        assert!(get_preset("MOONSHOT").is_some());
        assert!(get_preset("doubao").is_some());
    }

    #[test]
    fn test_kimi_alias() {
        let moonshot = get_preset("moonshot").unwrap();
        let kimi = get_preset("kimi").unwrap();
        assert_eq!(moonshot.base_url, kimi.base_url);
        assert_eq!(moonshot.protocol, "openai");
    }

    #[test]
    fn test_kimi_for_coding_preset_distinct_from_moonshot() {
        let coding = get_preset("kimi-for-coding").unwrap();
        let coding_alias = get_preset("kimi-coding").unwrap();
        let moonshot = get_preset("moonshot").unwrap();

        assert_eq!(coding.base_url, "https://api.kimi.com/coding/v1");
        assert_eq!(coding.protocol, "anthropic");
        assert_eq!(coding.base_url, coding_alias.base_url);
        assert_ne!(coding.base_url, moonshot.base_url);
        assert_ne!(coding.protocol, moonshot.protocol);
    }

    #[test]
    fn test_technical_aliases_removed() {
        // These should NOT exist
        assert!(get_preset("anthropic").is_none());
        assert!(get_preset("google").is_none());
    }

    #[test]
    fn test_brand_names_retained() {
        // These should exist
        assert!(get_preset("claude").is_some());
        assert!(get_preset("gemini").is_some());
        assert!(get_preset("kimi").is_some());
        assert!(get_preset("moonshot").is_some());
    }

    // =========================================================================
    // get_merged_preset tests
    // =========================================================================

    #[test]
    fn test_openrouter_preset_uses_responses() {
        let preset = get_preset("openrouter");
        assert!(preset.is_some());
        let p = preset.unwrap();
        assert_eq!(p.protocol, "openai-responses");
        assert_eq!(p.base_url, "https://openrouter.ai/api");
    }

    #[test]
    fn test_get_merged_preset_builtin_only() {
        let overrides = crate::config::presets_override::PresetsOverride::default();
        let preset = get_merged_preset("openai", &overrides).unwrap();
        assert_eq!(preset.base_url, "https://api.openai.com/v1");
        assert_eq!(preset.protocol, "openai");
        assert_eq!(preset.color, "#10a37f");
        assert_eq!(preset.default_model, "gpt-4o");
    }

    #[test]
    fn test_get_merged_preset_with_override() {
        let mut overrides = crate::config::presets_override::PresetsOverride::default();
        overrides.providers.insert(
            "openai".to_string(),
            crate::config::presets_override::PartialProviderPreset {
                base_url: Some("https://custom-openai.example.com/v1".to_string()),
                default_model: Some("gpt-5".to_string()),
                enabled: true,
                ..Default::default()
            },
        );

        let preset = get_merged_preset("openai", &overrides).unwrap();
        assert_eq!(preset.base_url, "https://custom-openai.example.com/v1");
        assert_eq!(preset.default_model, "gpt-5");
        // Non-overridden fields fall back to built-in
        assert_eq!(preset.protocol, "openai");
        assert_eq!(preset.color, "#10a37f");
    }

    #[test]
    fn test_get_merged_preset_disabled() {
        let mut overrides = crate::config::presets_override::PresetsOverride::default();
        overrides.providers.insert(
            "openai".to_string(),
            crate::config::presets_override::PartialProviderPreset {
                enabled: false,
                ..Default::default()
            },
        );

        assert!(get_merged_preset("openai", &overrides).is_none());
    }

    #[test]
    fn test_get_merged_preset_new_provider() {
        let mut overrides = crate::config::presets_override::PresetsOverride::default();
        overrides.providers.insert(
            "my-custom-llm".to_string(),
            crate::config::presets_override::PartialProviderPreset {
                base_url: Some("https://my-llm.example.com/v1".to_string()),
                protocol: Some("openai".to_string()),
                color: Some("#abcdef".to_string()),
                default_model: Some("my-model-v1".to_string()),
                enabled: true,
                ..Default::default()
            },
        );

        let preset = get_merged_preset("my-custom-llm", &overrides).unwrap();
        assert_eq!(preset.base_url, "https://my-llm.example.com/v1");
        assert_eq!(preset.protocol, "openai");
        assert_eq!(preset.color, "#abcdef");
        assert_eq!(preset.default_model, "my-model-v1");
    }

    #[test]
    fn test_get_merged_preset_alias_lookup() {
        let mut overrides = crate::config::presets_override::PresetsOverride::default();
        overrides.providers.insert(
            "my-provider".to_string(),
            crate::config::presets_override::PartialProviderPreset {
                base_url: Some("https://alias-test.example.com/v1".to_string()),
                aliases: vec!["alias-one".to_string(), "alias-two".to_string()],
                enabled: true,
                ..Default::default()
            },
        );

        // Look up by alias — no built-in exists for "alias-one"
        let preset = get_merged_preset("alias-one", &overrides).unwrap();
        assert_eq!(preset.base_url, "https://alias-test.example.com/v1");
    }

    #[test]
    fn test_get_merged_preset_case_insensitive() {
        let overrides = crate::config::presets_override::PresetsOverride::default();
        let preset = get_merged_preset("OpenAI", &overrides).unwrap();
        assert_eq!(preset.base_url, "https://api.openai.com/v1");
    }

    #[test]
    fn test_get_merged_preset_not_found() {
        let overrides = crate::config::presets_override::PresetsOverride::default();
        assert!(get_merged_preset("nonexistent-provider", &overrides).is_none());
    }

    #[test]
    fn test_provider_metadata_lookup() {
        let meta = provider_metadata("DeepSeek").expect("deepseek metadata present");
        assert_eq!(meta.display_name, "DeepSeek");
        assert!(meta.supports(Modality::Chat));
        assert!(!meta.supports(Modality::Image));
        // Missing entries return None.
        assert!(provider_metadata("nonexistent").is_none());
    }

    #[test]
    fn test_presets_by_modality_chat_includes_all() {
        // Every shipped preset is chat-capable today.
        let chat = presets_by_modality(Modality::Chat);
        assert_eq!(chat.len(), PRESETS.len());
        assert!(chat.contains(&"openai"));
        assert!(chat.contains(&"claude"));
        assert!(chat.contains(&"gemini"));
        // Should be sorted.
        let mut sorted = chat.clone();
        sorted.sort_unstable();
        assert_eq!(chat, sorted);
    }

    #[test]
    fn test_presets_by_modality_image_currently_empty() {
        // No chat preset declares Image yet — generation lives in the
        // separate generation layer (see Phase A in generation/presets).
        assert!(presets_by_modality(Modality::Image).is_empty());
        assert!(presets_by_modality(Modality::Video).is_empty());
    }

    #[test]
    fn test_get_merged_preset_new_provider_no_base_url() {
        let mut overrides = crate::config::presets_override::PresetsOverride::default();
        overrides.providers.insert(
            "incomplete-provider".to_string(),
            crate::config::presets_override::PartialProviderPreset {
                // Missing base_url — partial_to_provider_preset returns None
                protocol: Some("openai".to_string()),
                enabled: true,
                ..Default::default()
            },
        );

        assert!(get_merged_preset("incomplete-provider", &overrides).is_none());
    }
}
