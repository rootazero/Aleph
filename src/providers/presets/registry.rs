//! Static registry of built-in provider presets.
//!
//! Single source of truth is `PROFILES` — one entry per canonical provider,
//! with hermes-style declarative aliases. `PRESETS` is lazily expanded so
//! every alias also resolves through the same `HashMap` shape that older
//! call sites already depend on (catalog, helpers, override merge, RPCs).
//!
//! Phase 2 entropy reduction: the legacy `PRESET_METADATA` parallel map has
//! been folded into `ProviderPreset` itself (modalities + homepage fields).
//! The `PRESET_METADATA` re-export below is now a derived `Lazy<HashMap>`
//! over `PROFILES` so historical readers keep working.

use once_cell::sync::Lazy;
use std::collections::HashMap;

use super::ProviderPreset;
use crate::providers::metadata::ProviderMetadata;

/// Canonical provider profiles. Each `aliases` entry will also be inserted
/// into `PRESETS` at lazy init, so `get_preset("kimi")` returns the same
/// data as `get_preset("moonshot")` without a second source-of-truth entry.
const PROFILES: &[(&str, ProviderPreset)] = &[
    // ─── OpenAI family ────────────────────────────────────────────────────────
    (
        "openai",
        ProviderPreset::new("https://api.openai.com/v1", "openai", "#10a37f", "gpt-5.6")
            .with_display("OpenAI")
            .with_homepage("https://platform.openai.com")
            .with_signup("https://platform.openai.com/api-keys")
            .with_aux_model("gpt-5.4-mini")
            .with_fallback_models(&["gpt-5.6", "gpt-5.5", "gpt-5.4-mini", "o4-mini"]),
    ),
    (
        "chatgpt",
        ProviderPreset::new("https://chatgpt.com", "codex", "#10a37f", "gpt-5.6")
            .with_display("ChatGPT (Codex Login)")
            .with_homepage("https://chatgpt.com")
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
        .with_homepage("https://learn.microsoft.com/azure/ai-services/openai")
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
            // Date-less aliases are the vendor-recommended form. Sonnet 5 is
            // the current balanced flagship (Claude 5 family); the older
            // dated ids mixed generations and 404'd.
            "claude-sonnet-5",
        )
        .with_display("Anthropic Claude")
        .with_homepage("https://docs.anthropic.com")
        .with_signup("https://console.anthropic.com/settings/keys")
        .with_aux_model("claude-haiku-4-5")
        .with_models_url("https://api.anthropic.com/v1/models")
        .with_fallback_models(&[
            "claude-sonnet-5",
            "claude-opus-4-8",
            "claude-haiku-4-5",
        ]),
    ),
    (
        "amazon-bedrock",
        ProviderPreset::new(
            "https://bedrock-runtime.us-east-1.amazonaws.com",
            "anthropic",
            "#ff9900",
            // Bedrock serves the bare dot-tagged Anthropic id (hermes bedrock
            // pricing table); Sonnet 5 is the current Claude 5 gen.
            "anthropic.claude-sonnet-5",
        )
        .with_aliases(&["bedrock"])
        .with_display("Amazon Bedrock")
        .with_homepage("https://docs.aws.amazon.com/bedrock")
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
            // Vertex serves the date-less id (openclaw
            // ANTHROPIC_VERTEX_DEFAULT_MODEL_ID); Sonnet 5 is the current gen.
            "claude-sonnet-5",
        )
        .with_display("Vertex AI — Anthropic")
        .with_homepage(
            "https://cloud.google.com/vertex-ai/generative-ai/docs/partner-models/claude",
        )
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
            "gemini-3.1-pro-preview",
        )
        .with_display("Google Gemini")
        .with_homepage("https://ai.google.dev")
        .with_signup("https://aistudio.google.com/app/apikey")
        .with_aux_model("gemini-3-flash-preview")
        .with_fallback_models(&[
            "gemini-3.1-pro-preview",
            "gemini-3-flash-preview",
            "gemini-2.5-flash",
        ]),
    ),
    // ─── DeepSeek / Moonshot ──────────────────────────────────────────────────
    (
        "deepseek",
        ProviderPreset::new(
            "https://api.deepseek.com",
            "openai",
            "#0066cc",
            // `deepseek-chat` / `deepseek-reasoner` are legacy aliases the
            // vendor retires 2026-07-24 (both resolve to v4-flash modes).
            // Default to the mainstream V4 tier `deepseek-chat` mapped to;
            // v4-pro is the pricier flagship (fallback).
            "deepseek-v4-flash",
        )
        .with_display("DeepSeek")
        .with_homepage("https://platform.deepseek.com")
        .with_signup("https://platform.deepseek.com/api_keys")
        .with_aux_model("deepseek-v4-flash")
        .with_fallback_models(&["deepseek-v4-flash", "deepseek-v4-pro"]),
    ),
    (
        "moonshot",
        ProviderPreset::new(
            "https://api.moonshot.cn/anthropic",
            "anthropic",
            "#6366f1",
            "kimi-k2.6",
        )
        .with_aliases(&["kimi"])
        .with_display("Moonshot / Kimi")
        .with_homepage("https://platform.moonshot.ai")
        .with_signup("https://platform.moonshot.ai/console/api-keys")
        .with_description("Anthropic-compatible endpoint (recommended)")
        .with_aux_model("kimi-k2.5")
        // Kimi server-manages temperature — sending one returns a fixed-value error.
        .with_temperature_policy(super::TemperaturePolicy::Omit)
        .with_fallback_models(&[
            "kimi-k2.6",
            "kimi-k2.5",
            "kimi-latest",
            "moonshot-v1-128k",
            "moonshot-v1-32k",
            "moonshot-v1-8k",
        ]),
    ),
    (
        "moonshot-openai",
        ProviderPreset::new(
            "https://api.moonshot.ai/v1",
            "openai",
            "#6366f1",
            "kimi-k2.6",
        )
        .with_aliases(&["kimi-openai"])
        .with_display("Moonshot / Kimi (OpenAI endpoint)")
        .with_homepage("https://platform.moonshot.ai")
        .with_signup("https://platform.moonshot.ai/console/api-keys")
        .with_description("OpenAI-compatible Kimi K2 / Moonshot chat models")
        .with_aux_model("kimi-k2.5")
        .with_fallback_models(&[
            "kimi-k2.6",
            "kimi-k2.5",
            "kimi-latest",
            "moonshot-v1-128k",
            "moonshot-v1-32k",
            "moonshot-v1-8k",
        ]),
    ),
    // ─── Moonshot / Kimi (CN region, OpenAI protocol) ─────────────
    (
        "moonshot-cn",
        ProviderPreset::new(
            "https://api.moonshot.cn/v1",
            "openai",
            "#6366f1",
            "kimi-k2.6",
        )
        .with_aliases(&["kimi-cn"])
        .with_display("Moonshot / Kimi (CN)")
        .with_homepage("https://platform.moonshot.cn")
        .with_signup("https://platform.moonshot.cn/console/api-keys")
        .with_description("China-region (api.moonshot.cn) Kimi K2 / Moonshot models")
        .with_aux_model("kimi-k2.5")
        .with_fallback_models(&[
            "kimi-k2.6",
            "kimi-k2.5",
            "kimi-latest",
            "moonshot-v1-128k",
            "moonshot-v1-32k",
            "moonshot-v1-8k",
        ]),
    ),
    (
        "kimi-for-coding",
        ProviderPreset::new(
            "https://api.kimi.com/coding/v1",
            "anthropic",
            "#6366f1",
            "kimi-for-coding",
        )
        .with_aliases(&["kimi-coding"])
        .with_display("Kimi for Coding")
        .with_homepage("https://platform.moonshot.ai")
        .with_signup("https://platform.moonshot.ai")
        .with_description("Anthropic-protocol endpoint for IDE agents")
        // Server manages temperature — sending one returns a fixed-value error.
        .with_temperature_policy(super::TemperaturePolicy::Omit)
        .with_fallback_models(&["kimi-for-coding", "kimi-code", "k2p5"]),
    ),
    // ─── Chinese commercial LLMs ──────────────────────────────────────────────
    (
        "doubao",
        ProviderPreset::new(
            "https://ark.cn-beijing.volces.com/api/v3",
            "openai",
            "#ff6b35",
            // Doubao-Seed 1.8 (256K, multimodal) — current Ark flagship,
            // two generations past the retired doubao-1.5 line.
            "doubao-seed-1-8-251228",
        )
        .with_aliases(&["volcengine", "ark"])
        .with_display("Volcengine Doubao")
        .with_homepage("https://www.volcengine.com/product/ark")
        .with_signup("https://console.volcengine.com/ark")
        .with_fallback_models(&["doubao-seed-1-8-251228", "doubao-1.5-pro-256k"]),
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
        .with_homepage("https://siliconflow.cn")
        .with_signup("https://cloud.siliconflow.cn/account/ak"),
    ),
    (
        "zhipu",
        ProviderPreset::new(
            "https://open.bigmodel.cn/api/paas/v4",
            "openai",
            "#3b5998",
            // GLM-5.2 — current flagship (1M lossless context), served on
            // both bigmodel.cn and z.ai.
            "GLM-5.2",
        )
        .with_aliases(&["glm"])
        .with_display("Zhipu GLM")
        .with_homepage("https://open.bigmodel.cn")
        .with_signup("https://bigmodel.cn/usercenter/apikeys")
        .with_aux_model("glm-4.7")
        .with_fallback_models(&["GLM-5.2", "glm-5.1", "glm-4.7"]),
    ),
    (
        "minimax",
        ProviderPreset::new(
            "https://api.minimaxi.com/anthropic",
            "anthropic",
            "#e84393",
            "MiniMax-M3",
        )
        .with_display("MiniMax")
        .with_description("Anthropic-compatible endpoint (recommended)")
        .with_homepage("https://www.minimax.io")
        .with_signup("https://www.minimax.io")
        .with_aux_model("MiniMax-M2.7")
        .with_fallback_models(&["MiniMax-M3", "MiniMax-M2.7", "MiniMax-M2.7-highspeed"]),
    ),
    (
        "minimax-openai",
        ProviderPreset::new(
            "https://api.minimax.io/v1",
            "openai",
            "#e84393",
            "MiniMax-M3",
        )
        .with_display("MiniMax (OpenAI endpoint)")
        .with_description("OpenAI-compatible endpoint")
        .with_homepage("https://www.minimax.io")
        .with_signup("https://www.minimax.io")
        .with_aux_model("MiniMax-M2.7")
        .with_fallback_models(&["MiniMax-M3", "MiniMax-M2.7", "MiniMax-M2.7-highspeed"]),
    ),
    (
        "qwen",
        ProviderPreset::new(
            "https://dashscope.aliyuncs.com/compatible-mode/v1",
            "openai",
            "#615ced",
            "qwen3-max-2026-01-23",
        )
        .with_aliases(&["dashscope"])
        .with_display("Qwen / Tongyi")
        .with_homepage("https://help.aliyun.com/zh/dashscope")
        .with_signup("https://bailian.console.aliyun.com")
        .with_description("Alibaba DashScope OpenAI-compatible endpoint")
        .with_aux_model("qwen3.6-flash")
        .with_fallback_models(&["qwen3-max-2026-01-23", "qwen3.6-plus", "qwen-plus"]),
    ),
    (
        "baichuan",
        ProviderPreset::new(
            "https://api.baichuan-ai.com/v1",
            "openai",
            "#e11d48",
            "Baichuan4",
        )
        .with_display("Baichuan / Baichuan4")
        .with_homepage("https://platform.baichuan-ai.com")
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
        .with_display("Hunyuan / Tencent Hunyuan")
        .with_homepage("https://cloud.tencent.com/document/product/1729")
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
        .with_display("Spark / iFlytek Spark")
        .with_homepage(
            "https://www.xfyun.cn/doc/spark/HTTP%E8%B0%83%E7%94%A8%E6%96%87%E6%A1%A3.html",
        )
        .with_description("V4 Ultra OpenAI-compatible endpoint"),
    ),
    (
        "stepfun",
        ProviderPreset::new(
            "https://api.stepfun.com/v1",
            "openai",
            "#0ea5e9",
            "step-1-8k",
        )
        .with_display("Stepfun")
        .with_homepage("https://stepfun.com")
        .with_signup("https://platform.stepfun.com"),
    ),
    (
        "t8star",
        ProviderPreset::new("https://api.t8star.cn/v1", "openai", "#f59e0b", "")
            .with_display("T8Star")
            .with_homepage("https://t8star.cn")
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
        .with_homepage("https://groq.com")
        .with_signup("https://console.groq.com/keys")
        .with_description("Ultra-fast LPU inference")
        .with_fallback_models(&["llama-3.3-70b-versatile", "llama-3.1-8b-instant"]),
    ),
    (
        "cerebras",
        ProviderPreset::new(
            "https://api.cerebras.ai/v1",
            "openai",
            "#f97316",
            "llama-3.3-70b",
        )
        .with_display("Cerebras")
        .with_homepage("https://cerebras.ai")
        .with_signup("https://cloud.cerebras.ai")
        .with_description("Ultra-fast Llama inference"),
    ),
    (
        "together",
        ProviderPreset::new(
            "https://api.together.xyz/v1",
            "openai",
            "#6366f1",
            "meta-llama/Llama-3.3-70B-Instruct-Turbo",
        )
        .with_display("Together.ai")
        .with_homepage("https://www.together.ai")
        .with_signup("https://api.together.xyz/settings/api-keys"),
    ),
    (
        "perplexity",
        // Legacy `pplx-*` / `llama-3.1-sonar-*-online` ids were retired; the
        // unified `sonar*` family is the current naming. An empty default sent
        // `model: ""` → 400, so the preset was unusable out of the box.
        ProviderPreset::new("https://api.perplexity.ai", "openai", "#20808d", "sonar")
            .with_display("Perplexity")
            .with_homepage("https://docs.perplexity.ai")
            .with_signup("https://www.perplexity.ai/settings/api")
            .with_description("Search-augmented LLMs")
            .with_fallback_models(&[
                "sonar",
                "sonar-pro",
                "sonar-reasoning",
                "sonar-reasoning-pro",
            ]),
    ),
    (
        "mistral",
        ProviderPreset::new(
            "https://api.mistral.ai/v1",
            "openai",
            "#ff7000",
            "mistral-large-latest",
        )
        .with_display("Mistral AI")
        .with_homepage("https://docs.mistral.ai")
        .with_signup("https://console.mistral.ai/api-keys")
        .with_fallback_models(&[
            "mistral-large-latest",
            "mistral-medium-latest",
            "mistral-small-latest",
        ]),
    ),
    (
        "cohere",
        // Cohere's bare `/v1` is its native (non-OpenAI) API; the
        // OpenAI-compatible Chat Completions surface lives at
        // `/compatibility/v1`. The old base_url 404'd under the `openai`
        // protocol's `/chat/completions` path.
        ProviderPreset::new(
            "https://api.cohere.ai/compatibility/v1",
            "openai",
            "#39594d",
            "command-a-03-2025",
        )
        .with_display("Cohere")
        .with_homepage("https://docs.cohere.com/docs/compatibility-api")
        .with_signup("https://dashboard.cohere.com/api-keys")
        .with_description("OpenAI-compatible endpoint (/compatibility/v1)")
        .with_fallback_models(&["command-a-03-2025", "command-r-plus", "command-r"]),
    ),
    (
        "fireworks",
        ProviderPreset::new(
            "https://api.fireworks.ai/inference/v1",
            "openai",
            "#ff6b35",
            "accounts/fireworks/models/llama-v3p3-70b-instruct",
        )
        .with_display("Fireworks.ai")
        .with_homepage("https://fireworks.ai")
        .with_signup("https://fireworks.ai/account/api-keys"),
    ),
    (
        "anyscale",
        ProviderPreset::new(
            "https://api.endpoints.anyscale.com/v1",
            "openai",
            "#00d4aa",
            "",
        )
        .with_display("Anyscale")
        .with_homepage("https://www.anyscale.com"),
    ),
    (
        "replicate",
        ProviderPreset::new("https://api.replicate.com/v1", "openai", "#0c0c0d", "")
            .with_display("Replicate")
            .with_homepage("https://replicate.com")
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
        .with_homepage("https://openrouter.ai")
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
            .with_display("Lepton AI")
            .with_homepage("https://www.lepton.ai"),
    ),
    (
        "hyperbolic",
        ProviderPreset::new(
            "https://api.hyperbolic.xyz/v1",
            "openai",
            "#8b5cf6",
            "meta-llama/Llama-3.3-70B-Instruct",
        )
        .with_display("Hyperbolic")
        .with_homepage("https://hyperbolic.xyz"),
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
        .with_homepage("https://huggingface.co/docs/api-inference")
        .with_signup("https://huggingface.co/settings/tokens")
        .with_description("Routes to community-hosted open models"),
    ),
    // ─── xAI ──────────────────────────────────────────────────────────────────
    (
        "xai",
        ProviderPreset::new("https://api.x.ai/v1", "openai", "#000000", "grok-4.3")
            .with_aliases(&["grok"])
            .with_display("xAI Grok")
            .with_homepage("https://docs.x.ai")
            .with_signup("https://console.x.ai")
            .with_aux_model("grok-3-mini")
            .with_fallback_models(&["grok-4.3", "grok-4-fast", "grok-3-mini"]),
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
        .with_homepage("https://developers.cloudflare.com/workers-ai")
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
        .with_homepage("https://deepinfra.com/docs")
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
        .with_homepage("https://docs.github.com/copilot")
        .with_signup("https://github.com/settings/copilot")
        .with_description("Requires Copilot subscription token"),
    ),
    (
        "lmstudio",
        ProviderPreset::new(
            "http://localhost:1234/v1",
            "openai",
            "#7c3aed",
            "local-model",
        )
        .with_display("LM Studio (Local)")
        .with_homepage("https://lmstudio.ai")
        .with_signup("https://lmstudio.ai")
        .with_description("Local OpenAI-compatible server (default :1234)"),
    ),
    (
        "litellm",
        ProviderPreset::new("http://localhost:4000", "openai", "#22c55e", "gpt-4o")
            .with_display("LiteLLM Proxy")
            .with_homepage("https://docs.litellm.ai")
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
        .with_homepage("https://docs.nvidia.com/nim")
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
        .with_homepage("https://developers.inflection.ai")
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
        .with_homepage("https://novita.ai/docs")
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
        .with_homepage("https://chutes.ai")
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
        .with_homepage("https://developers.cloudflare.com/ai-gateway")
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
        .with_homepage("https://learn.microsoft.com/azure/ai-foundry")
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
        .with_homepage("https://www.gmicloud.ai")
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
        .with_homepage("https://nousresearch.com")
        .with_signup("https://portal.nousresearch.com")
        .with_description("Hermes models from Nous Research"),
    ),
    (
        "zai",
        ProviderPreset::new(
            "https://api.z.ai/api/paas/v4",
            "openai",
            "#3b82f6",
            "glm-5.2",
        )
        .with_display("Z.ai (Zhipu international)")
        .with_homepage("https://z.ai")
        .with_signup("https://z.ai")
        .with_description("International gateway for Zhipu / GLM models")
        .with_aux_model("glm-4.7")
        .with_fallback_models(&["glm-5.2", "glm-5", "glm-4.6"]),
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
        .with_homepage("https://ollama.com")
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

/// Reverse index: `base_url` → preset.
///
/// `base_url`s in `PROFILES` are unique per canonical entry, so a single
/// hashmap suffices. Used by `temperature_for_base_url()` so adapters can
/// resolve a preset-level policy from `config.base_url` without threading
/// a preset name through the request-building plumbing.
///
/// Users who override `base_url` on a configured provider will miss this
/// lookup — that's intentional: overriding `base_url` opts out of preset
/// assumptions like `temperature_policy`.
pub(crate) static PRESETS_BY_BASE_URL: Lazy<HashMap<&'static str, &'static ProviderPreset>> =
    Lazy::new(|| {
        let mut m = HashMap::with_capacity(PROFILES.len());
        for (_, preset) in PROFILES {
            m.insert(preset.base_url, preset);
        }
        m
    });

/// Backwards-compatible `ProviderMetadata` view derived from `PROFILES`.
///
/// Historically this lived in `metadata.rs` as a hand-maintained parallel
/// map. Phase 2 collapses it: every field comes from the matching preset.
///
/// * `display_name` ← `preset.display_name`, fallback to canonical name
/// * `modalities`   ← `preset.modalities` (defaults to `&[Modality::Chat]`)
/// * `homepage`     ← `preset.homepage`
/// * `notes`        ← `preset.description`
///
/// Both canonical names and aliases are inserted — `PRESETS` exposes alias
/// keys too (catalog iteration / `provider_metadata("kimi")`), so the
/// metadata map must answer for the same key set.
pub static PRESET_METADATA: Lazy<HashMap<&'static str, ProviderMetadata>> = Lazy::new(|| {
    let mut m: HashMap<&'static str, ProviderMetadata> = HashMap::with_capacity(PROFILES.len() * 2);
    for (name, preset) in PROFILES {
        let meta = ProviderMetadata {
            display_name: preset.display_name.unwrap_or(name),
            modalities: preset.modalities,
            homepage: preset.homepage,
            notes: preset.description,
        };
        m.insert(*name, meta);
        for alias in preset.aliases {
            m.insert(*alias, meta);
        }
    }
    m
});
