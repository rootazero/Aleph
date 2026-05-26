//! Per-preset rich metadata — display name, modalities, homepage.
//!
//! Stored in a parallel map rather than embedded in `ProviderPreset` so adding
//! it is zero-churn for the existing entries. Lookups by name are
//! case-insensitive at the call site (we lowercase before query).

use once_cell::sync::Lazy;
use std::collections::HashMap;

use crate::providers::metadata::{Modality, ProviderMetadata};

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

    // Phase R2 metadata
    m.insert(
        "amazon-bedrock",
        ProviderMetadata {
            display_name: "Amazon Bedrock",
            modalities: CHAT_ONLY,
            homepage: Some("https://docs.aws.amazon.com/bedrock"),
            notes: Some("Anthropic protocol; override base_url per region"),
        },
    );
    m.insert(
        "cloudflare-ai",
        ProviderMetadata {
            display_name: "Cloudflare Workers AI",
            modalities: CHAT_ONLY,
            homepage: Some("https://developers.cloudflare.com/workers-ai"),
            notes: Some("Set account id in base_url"),
        },
    );
    m.insert(
        "deepinfra",
        ProviderMetadata {
            display_name: "DeepInfra",
            modalities: CHAT_ONLY,
            homepage: Some("https://deepinfra.com/docs"),
            notes: Some("Open-model inference, OpenAI-compatible"),
        },
    );
    m.insert(
        "github-copilot",
        ProviderMetadata {
            display_name: "GitHub Copilot",
            modalities: CHAT_ONLY,
            homepage: Some("https://docs.github.com/copilot"),
            notes: Some("Requires Copilot subscription token"),
        },
    );
    m.insert(
        "lmstudio",
        ProviderMetadata {
            display_name: "LM Studio (Local)",
            modalities: CHAT_ONLY,
            homepage: Some("https://lmstudio.ai"),
            notes: Some("Local OpenAI-compatible server (default :1234)"),
        },
    );
    m.insert(
        "litellm",
        ProviderMetadata {
            display_name: "LiteLLM Proxy",
            modalities: CHAT_ONLY,
            homepage: Some("https://docs.litellm.ai"),
            notes: Some("Drop-in proxy for any LLM backend"),
        },
    );
    m.insert(
        "nvidia-nim",
        ProviderMetadata {
            display_name: "NVIDIA NIM",
            modalities: CHAT_ONLY,
            homepage: Some("https://docs.nvidia.com/nim"),
            notes: Some("NGC-hosted inference catalog"),
        },
    );
    m.insert(
        "inflection",
        ProviderMetadata {
            display_name: "Inflection AI (Pi)",
            modalities: CHAT_ONLY,
            homepage: Some("https://developers.inflection.ai"),
            notes: None,
        },
    );
    m.insert(
        "novita",
        ProviderMetadata {
            display_name: "Novita AI",
            modalities: CHAT_ONLY,
            homepage: Some("https://novita.ai/docs"),
            notes: Some("Serverless open-model inference"),
        },
    );
    m.insert(
        "chutes",
        ProviderMetadata {
            display_name: "Chutes",
            modalities: CHAT_ONLY,
            homepage: Some("https://chutes.ai"),
            notes: Some("Bittensor-backed open inference"),
        },
    );

    m
});
