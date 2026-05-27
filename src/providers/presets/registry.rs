//! Static registry of built-in provider presets.
//!
//! Single source of truth is `PROFILES` — one entry per canonical provider,
//! with hermes-style declarative aliases. `PRESETS` is lazily expanded so
//! every alias also resolves through the same `HashMap` shape that older
//! call sites already depend on (catalog, helpers, override merge, RPCs).

use once_cell::sync::Lazy;
use std::collections::HashMap;

use super::ProviderPreset;

/// Canonical provider profiles. Each `aliases` entry will also be inserted
/// into `PRESETS` at lazy init, so `get_preset("kimi")` returns the same
/// data as `get_preset("moonshot")` without a second source-of-truth entry.
const PROFILES: &[(&str, ProviderPreset)] = &[
    // ─── OpenAI family ────────────────────────────────────────────────────────
    (
        "openai",
        ProviderPreset::new("https://api.openai.com/v1", "openai", "#10a37f", "gpt-4o")
            .with_display("OpenAI")
            .with_signup("https://platform.openai.com/api-keys")
            .with_aux_model("gpt-4o-mini")
            .with_fallback_models(&["gpt-4o", "gpt-4o-mini", "o3-mini", "o1-mini"]),
    ),
    (
        "chatgpt",
        ProviderPreset::new("https://chatgpt.com", "codex", "#10a37f", "gpt-5.4")
            .with_display("ChatGPT (Codex Login)")
            .with_signup("https://chatgpt.com")
            .with_description("OAuth login, Codex Responses protocol")
            .no_health_check(),
    ),
    (
        "azure-openai",
        ProviderPreset::new(
            "https://YOUR-RESOURCE.openai.azure.com",
            "openai",
            "#0078d4",
            "gpt-4o",
        )
        .with_display("Azure OpenAI")
        .with_signup("https://portal.azure.com")
        .with_description("Override base_url with your Azure resource")
        .no_health_check(),
    ),
    // ─── Anthropic family ─────────────────────────────────────────────────────
    (
        "claude",
        ProviderPreset::new(
            "https://api.anthropic.com",
            "anthropic",
            "#d97757",
            "claude-sonnet-4-5-20250514",
        )
        .with_display("Anthropic Claude")
        .with_signup("https://console.anthropic.com/settings/keys")
        .with_aux_model("claude-haiku-4-5-20251001")
        .with_models_url("https://api.anthropic.com/v1/models")
        .with_fallback_models(&[
            "claude-sonnet-4-5-20250514",
            "claude-opus-4-5-20250514",
            "claude-haiku-4-5-20251001",
        ]),
    ),
    (
        "amazon-bedrock",
        ProviderPreset::new(
            "https://bedrock-runtime.us-east-1.amazonaws.com",
            "anthropic",
            "#ff9900",
            "anthropic.claude-3-7-sonnet-20250219-v1:0",
        )
        .with_aliases(&["bedrock"])
        .with_display("Amazon Bedrock")
        .with_signup("https://console.aws.amazon.com/bedrock")
        .with_description("Anthropic protocol; override base_url per region")
        .no_health_check(),
    ),
    (
        "vertex-anthropic",
        ProviderPreset::new(
            "https://us-east5-aiplatform.googleapis.com/v1",
            "anthropic",
            "#4285f4",
            "claude-sonnet-4@20250514",
        )
        .with_display("Vertex AI — Anthropic")
        .with_signup("https://console.cloud.google.com")
        .with_description("Claude via GCP Vertex; region-specific base URL")
        .no_health_check(),
    ),
    // ─── Google ───────────────────────────────────────────────────────────────
    (
        "gemini",
        ProviderPreset::new(
            "https://generativelanguage.googleapis.com",
            "gemini",
            "#4285f4",
            "gemini-2.5-flash",
        )
        .with_display("Google Gemini")
        .with_signup("https://aistudio.google.com/app/apikey")
        .with_aux_model("gemini-2.5-flash-lite")
        .with_fallback_models(&[
            "gemini-2.5-pro",
            "gemini-2.5-flash",
            "gemini-2.5-flash-lite",
        ]),
    ),
    // ─── DeepSeek / Moonshot ──────────────────────────────────────────────────
    (
        "deepseek",
        ProviderPreset::new("https://api.deepseek.com", "openai", "#0066cc", "deepseek-chat")
            .with_display("DeepSeek")
            .with_signup("https://platform.deepseek.com/api_keys")
            .with_aux_model("deepseek-chat")
            .with_fallback_models(&["deepseek-chat", "deepseek-reasoner"]),
    ),
    (
        "moonshot",
        ProviderPreset::new(
            "https://api.moonshot.ai/v1",
            "openai",
            "#6366f1",
            "kimi-k2-0905-preview",
        )
        .with_aliases(&["kimi"])
        .with_display("Moonshot / Kimi")
        .with_signup("https://platform.moonshot.ai/console/api-keys")
        .with_fallback_models(&["kimi-k2-0905-preview", "moonshot-v1-128k", "moonshot-v1-32k"]),
    ),
    (
        "kimi-for-coding",
        ProviderPreset::new(
            "https://api.kimi.com/coding/v1",
            "anthropic",
            "#6366f1",
            "Kimi-K2.6",
        )
        .with_aliases(&["kimi-coding"])
        .with_display("Kimi for Coding")
        .with_signup("https://platform.moonshot.ai")
        .with_description("Anthropic-protocol endpoint for IDE agents")
        // Server manages temperature — sending one returns a fixed-value error.
        .with_temperature_policy(super::TemperaturePolicy::Omit),
    ),
    // ─── Chinese commercial LLMs ──────────────────────────────────────────────
    (
        "doubao",
        ProviderPreset::new(
            "https://ark.cn-beijing.volces.com/api/v3",
            "openai",
            "#ff6b35",
            "doubao-1.5-pro-256k",
        )
        .with_aliases(&["volcengine", "ark"])
        .with_display("Volcengine Doubao")
        .with_signup("https://console.volcengine.com/ark"),
    ),
    (
        "siliconflow",
        ProviderPreset::new(
            "https://api.siliconflow.cn/v1",
            "openai",
            "#6c5ce7",
            "deepseek-ai/DeepSeek-V3",
        )
        .with_display("SiliconFlow")
        .with_signup("https://cloud.siliconflow.cn/account/ak"),
    ),
    (
        "zhipu",
        ProviderPreset::new(
            "https://open.bigmodel.cn/api/paas/v4",
            "openai",
            "#3b5998",
            "GLM-5",
        )
        .with_aliases(&["glm"])
        .with_display("Zhipu GLM")
        .with_signup("https://bigmodel.cn/usercenter/apikeys"),
    ),
    (
        "minimax",
        ProviderPreset::new(
            "https://api.minimax.io/v1",
            "openai",
            "#e84393",
            "MiniMax-M2.5",
        )
        .with_display("MiniMax")
        .with_signup("https://www.minimax.io"),
    ),
    (
        "qwen",
        ProviderPreset::new(
            "https://dashscope.aliyuncs.com/compatible-mode/v1",
            "openai",
            "#615ced",
            "qwen-max-2025-01-25",
        )
        .with_aliases(&["dashscope"])
        .with_display("Qwen / 通义")
        .with_signup("https://bailian.console.aliyun.com")
        .with_description("Alibaba DashScope OpenAI-compatible endpoint")
        .with_fallback_models(&["qwen-max-2025-01-25", "qwen-plus", "qwen-turbo"]),
    ),
    (
        "baichuan",
        ProviderPreset::new(
            "https://api.baichuan-ai.com/v1",
            "openai",
            "#e11d48",
            "Baichuan4",
        )
        .with_display("Baichuan / 百川")
        .with_signup("https://platform.baichuan-ai.com"),
    ),
    (
        "hunyuan",
        ProviderPreset::new(
            "https://api.hunyuan.cloud.tencent.com/v1",
            "openai",
            "#1e40af",
            "hunyuan-pro",
        )
        .with_display("Hunyuan / 腾讯混元")
        .with_signup("https://cloud.tencent.com/product/hunyuan"),
    ),
    (
        "spark",
        ProviderPreset::new(
            "https://spark-api-open.xf-yun.com/v1",
            "openai",
            "#ff4d4f",
            "4.0Ultra",
        )
        .with_display("Spark / 讯飞星火")
        .with_description("V4 Ultra OpenAI-compatible endpoint"),
    ),
    (
        "stepfun",
        ProviderPreset::new("https://api.stepfun.com/v1", "openai", "#0ea5e9", "step-1-8k")
            .with_display("Stepfun")
            .with_signup("https://platform.stepfun.com"),
    ),
    (
        "t8star",
        ProviderPreset::new("https://api.t8star.cn/v1", "openai", "#f59e0b", "")
            .with_display("T8Star")
            .with_description("OpenAI-compatible aggregator"),
    ),
    // ─── Western specialty / inference ────────────────────────────────────────
    (
        "groq",
        ProviderPreset::new(
            "https://api.groq.com/openai/v1",
            "openai",
            "#f55036",
            "llama-3.3-70b-versatile",
        )
        .with_display("Groq")
        .with_signup("https://console.groq.com/keys")
        .with_description("Ultra-fast LPU inference")
        .with_fallback_models(&["llama-3.3-70b-versatile", "llama-3.1-8b-instant"]),
    ),
    (
        "cerebras",
        ProviderPreset::new("https://api.cerebras.ai/v1", "openai", "#f97316", "llama-3.3-70b")
            .with_display("Cerebras")
            .with_signup("https://cloud.cerebras.ai")
            .with_description("Ultra-fast Llama inference"),
    ),
    (
        "together",
        ProviderPreset::new("https://api.together.xyz/v1", "openai", "#6366f1", "")
            .with_display("Together.ai")
            .with_signup("https://api.together.xyz/settings/api-keys"),
    ),
    (
        "perplexity",
        ProviderPreset::new("https://api.perplexity.ai", "openai", "#20808d", "")
            .with_display("Perplexity")
            .with_signup("https://www.perplexity.ai/settings/api")
            .with_description("Search-augmented LLMs"),
    ),
    (
        "mistral",
        ProviderPreset::new("https://api.mistral.ai/v1", "openai", "#ff7000", "")
            .with_display("Mistral AI")
            .with_signup("https://console.mistral.ai/api-keys"),
    ),
    (
        "cohere",
        ProviderPreset::new("https://api.cohere.ai/v1", "openai", "#39594d", "")
            .with_display("Cohere")
            .with_signup("https://dashboard.cohere.com/api-keys"),
    ),
    (
        "fireworks",
        ProviderPreset::new("https://api.fireworks.ai/inference/v1", "openai", "#ff6b35", "")
            .with_display("Fireworks.ai")
            .with_signup("https://fireworks.ai/account/api-keys"),
    ),
    (
        "anyscale",
        ProviderPreset::new("https://api.endpoints.anyscale.com/v1", "openai", "#00d4aa", "")
            .with_display("Anyscale"),
    ),
    (
        "replicate",
        ProviderPreset::new("https://api.replicate.com/v1", "openai", "#0c0c0d", "")
            .with_display("Replicate")
            .with_signup("https://replicate.com/account/api-tokens")
            .with_description("Hosting; image/video via generation layer"),
    ),
    (
        "openrouter",
        ProviderPreset::new(
            "https://openrouter.ai/api",
            "openai-responses",
            "#6467f2",
            "openai/gpt-4o",
        )
        .with_aliases(&["or"])
        .with_display("OpenRouter")
        .with_signup("https://openrouter.ai/keys")
        .with_description("Multi-model router")
        .with_fallback_models(&[
            "openai/gpt-4o",
            "anthropic/claude-sonnet-4",
            "google/gemini-2.5-flash",
        ]),
    ),
    (
        "lepton",
        ProviderPreset::new("https://api.lepton.ai/api/v1", "openai", "#4f46e5", "")
            .with_display("Lepton AI"),
    ),
    (
        "hyperbolic",
        ProviderPreset::new("https://api.hyperbolic.xyz/v1", "openai", "#8b5cf6", "")
            .with_display("Hyperbolic"),
    ),
    (
        "huggingface",
        ProviderPreset::new(
            "https://router.huggingface.co/v1",
            "openai",
            "#ffd21e",
            "meta-llama/Llama-3.3-70B-Instruct",
        )
        .with_display("HuggingFace Inference")
        .with_signup("https://huggingface.co/settings/tokens")
        .with_description("Routes to community-hosted open models"),
    ),
    // ─── xAI ──────────────────────────────────────────────────────────────────
    (
        "xai",
        ProviderPreset::new("https://api.x.ai/v1", "openai", "#000000", "grok-4-0709")
            .with_aliases(&["grok"])
            .with_display("xAI Grok")
            .with_signup("https://console.x.ai")
            .with_fallback_models(&["grok-4-0709", "grok-3-mini"]),
    ),
    // ─── Cloud / gateway / local ──────────────────────────────────────────────
    (
        "cloudflare-ai",
        ProviderPreset::new(
            "https://api.cloudflare.com/client/v4/accounts/ACCOUNT_ID/ai/v1",
            "openai",
            "#f6821f",
            "@cf/meta/llama-3.3-70b-instruct-fp8-fast",
        )
        .with_aliases(&["workers-ai"])
        .with_display("Cloudflare Workers AI")
        .with_signup("https://dash.cloudflare.com/?to=/:account/ai")
        .with_description("Set account id in base_url"),
    ),
    (
        "deepinfra",
        ProviderPreset::new(
            "https://api.deepinfra.com/v1/openai",
            "openai",
            "#5b8def",
            "meta-llama/Llama-3.3-70B-Instruct",
        )
        .with_display("DeepInfra")
        .with_signup("https://deepinfra.com/dash/api_keys")
        .with_description("Open-model inference, OpenAI-compatible"),
    ),
    (
        "github-copilot",
        ProviderPreset::new(
            "https://api.githubcopilot.com",
            "openai",
            "#24292f",
            "gpt-4o-2025-04-09",
        )
        .with_aliases(&["copilot"])
        .with_display("GitHub Copilot")
        .with_signup("https://github.com/settings/copilot")
        .with_description("Requires Copilot subscription token"),
    ),
    (
        "lmstudio",
        ProviderPreset::new("http://localhost:1234/v1", "openai", "#7c3aed", "local-model")
            .with_display("LM Studio (Local)")
            .with_signup("https://lmstudio.ai")
            .with_description("Local OpenAI-compatible server (default :1234)"),
    ),
    (
        "litellm",
        ProviderPreset::new("http://localhost:4000", "openai", "#22c55e", "gpt-4o")
            .with_display("LiteLLM Proxy")
            .with_signup("https://docs.litellm.ai")
            .with_description("Drop-in proxy for any LLM backend"),
    ),
    (
        "nvidia-nim",
        ProviderPreset::new(
            "https://integrate.api.nvidia.com/v1",
            "openai",
            "#76b900",
            "meta/llama-3.3-70b-instruct",
        )
        .with_aliases(&["nvidia"])
        .with_display("NVIDIA NIM")
        .with_signup("https://build.nvidia.com")
        .with_description("NGC-hosted inference catalog"),
    ),
    (
        "inflection",
        ProviderPreset::new(
            "https://api.inflection.ai/external/api/inference/openai/v1",
            "openai",
            "#f59e0b",
            "inflection_3_pi",
        )
        .with_display("Inflection AI (Pi)")
        .with_signup("https://developers.inflection.ai"),
    ),
    (
        "novita",
        ProviderPreset::new(
            "https://api.novita.ai/v3/openai",
            "openai",
            "#0ea5e9",
            "meta-llama/llama-3.3-70b-instruct",
        )
        .with_display("Novita AI")
        .with_signup("https://novita.ai/settings/key-management")
        .with_description("Serverless open-model inference"),
    ),
    (
        "chutes",
        ProviderPreset::new(
            "https://llm.chutes.ai/v1",
            "openai",
            "#a855f7",
            "deepseek-ai/DeepSeek-V3-0324",
        )
        .with_display("Chutes")
        .with_signup("https://chutes.ai")
        .with_description("Bittensor-backed open inference"),
    ),
    // ─── New presets brought over from hermes-agent plugins/model-providers ───
    (
        "ai-gateway",
        ProviderPreset::new(
            "https://gateway.ai.cloudflare.com/v1/ACCOUNT_ID/aleph/openai",
            "openai",
            "#f6821f",
            "gpt-4o",
        )
        .with_display("Cloudflare AI Gateway")
        .with_signup("https://dash.cloudflare.com/?to=/:account/ai/ai-gateway")
        .with_description("OpenAI-compatible AI gateway with cache + analytics")
        .no_health_check(),
    ),
    (
        "azure-foundry",
        ProviderPreset::new(
            "https://YOUR-PROJECT.services.ai.azure.com/models",
            "openai",
            "#0078d4",
            "gpt-4o",
        )
        .with_display("Azure AI Foundry")
        .with_signup("https://ai.azure.com")
        .with_description("Azure AI Foundry inference (models endpoint)")
        .no_health_check(),
    ),
    (
        "gmi",
        ProviderPreset::new(
            "https://api.gmi-serving.com/v1",
            "openai",
            "#0ea5e9",
            "deepseek-ai/DeepSeek-V3",
        )
        .with_display("GMI Cloud")
        .with_signup("https://www.gmicloud.ai")
        .with_description("Multi-model direct API"),
    ),
    (
        "nous",
        ProviderPreset::new(
            "https://inference-api.nousresearch.com/v1",
            "openai",
            "#8b5cf6",
            "Hermes-3-Llama-3.1-70B",
        )
        .with_display("Nous Research")
        .with_signup("https://portal.nousresearch.com")
        .with_description("Hermes models from Nous Research"),
    ),
    (
        "zai",
        ProviderPreset::new(
            "https://api.z.ai/api/paas/v4",
            "openai",
            "#3b82f6",
            "glm-4.6",
        )
        .with_display("Z.ai (Zhipu international)")
        .with_signup("https://z.ai")
        .with_description("International gateway for Zhipu / GLM models"),
    ),
    (
        "ollama-cloud",
        ProviderPreset::new(
            "https://ollama.com/api/v1",
            "openai",
            "#0c0c0d",
            "llama3.3:70b",
        )
        .with_display("Ollama Cloud")
        .with_signup("https://ollama.com")
        .with_description("Cloud-hosted Ollama (vs. local default)"),
    ),
];

/// Registry of known provider presets — expanded from `PROFILES` so both
/// canonical names and aliases resolve to the same `ProviderPreset` data.
pub static PRESETS: Lazy<HashMap<&'static str, ProviderPreset>> = Lazy::new(|| {
    let mut m = HashMap::with_capacity(PROFILES.len() * 2);
    for (name, preset) in PROFILES {
        m.insert(*name, *preset);
        for alias in preset.aliases {
            m.insert(*alias, *preset);
        }
    }
    m
});
