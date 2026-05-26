//! Static registry of built-in provider presets.
//!
//! Pure data — every entry maps a canonical name (and optional aliases)
//! to the default base URL, wire protocol, brand color, and default model.

use once_cell::sync::Lazy;
use std::collections::HashMap;

use super::ProviderPreset;

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

    // =========================================================================
    // Phase R2 chat additions — extend coverage to cloud LLM gateways, local
    // OpenAI-compatible servers, and remaining commercial vendors. Most are
    // straight OpenAI-protocol entries; Bedrock defaults to anthropic-on-AWS.
    // =========================================================================

    // Amazon Bedrock — Anthropic protocol via AWS sign-v4 proxy. Override
    // `base_url` per region (e.g. `https://bedrock-runtime.us-east-1.amazonaws.com`).
    m.insert(
        "amazon-bedrock",
        ProviderPreset {
            base_url: "https://bedrock-runtime.us-east-1.amazonaws.com",
            protocol: "anthropic",
            color: "#ff9900",
            default_model: "anthropic.claude-3-7-sonnet-20250219-v1:0",
        },
    );
    m.insert(
        "bedrock",
        ProviderPreset {
            base_url: "https://bedrock-runtime.us-east-1.amazonaws.com",
            protocol: "anthropic",
            color: "#ff9900",
            default_model: "anthropic.claude-3-7-sonnet-20250219-v1:0",
        },
    );

    // Cloudflare Workers AI — OpenAI-compatible gateway. Override base_url with
    // your account id: `https://api.cloudflare.com/client/v4/accounts/{id}/ai/v1`.
    m.insert(
        "cloudflare-ai",
        ProviderPreset {
            base_url: "https://api.cloudflare.com/client/v4/accounts/ACCOUNT_ID/ai/v1",
            protocol: "openai",
            color: "#f6821f",
            default_model: "@cf/meta/llama-3.3-70b-instruct-fp8-fast",
        },
    );
    m.insert(
        "workers-ai",
        ProviderPreset {
            base_url: "https://api.cloudflare.com/client/v4/accounts/ACCOUNT_ID/ai/v1",
            protocol: "openai",
            color: "#f6821f",
            default_model: "@cf/meta/llama-3.3-70b-instruct-fp8-fast",
        },
    );

    // DeepInfra — open-model inference, OpenAI-compatible.
    m.insert(
        "deepinfra",
        ProviderPreset {
            base_url: "https://api.deepinfra.com/v1/openai",
            protocol: "openai",
            color: "#5b8def",
            default_model: "meta-llama/Llama-3.3-70B-Instruct",
        },
    );

    // GitHub Copilot Chat — OpenAI-compatible via Copilot API proxy.
    m.insert(
        "github-copilot",
        ProviderPreset {
            base_url: "https://api.githubcopilot.com",
            protocol: "openai",
            color: "#24292f",
            default_model: "gpt-4o-2025-04-09",
        },
    );
    m.insert(
        "copilot",
        ProviderPreset {
            base_url: "https://api.githubcopilot.com",
            protocol: "openai",
            color: "#24292f",
            default_model: "gpt-4o-2025-04-09",
        },
    );

    // LM Studio — local OpenAI-compatible server (default port 1234).
    m.insert(
        "lmstudio",
        ProviderPreset {
            base_url: "http://localhost:1234/v1",
            protocol: "openai",
            color: "#7c3aed",
            default_model: "local-model",
        },
    );

    // LiteLLM proxy — drop-in OpenAI-compatible router for any backend
    // (Bedrock, Vertex, Azure, etc.). Default assumes localhost:4000.
    m.insert(
        "litellm",
        ProviderPreset {
            base_url: "http://localhost:4000",
            protocol: "openai",
            color: "#22c55e",
            default_model: "gpt-4o",
        },
    );

    // NVIDIA NIM — OpenAI-compatible inference catalog (NGC API key).
    m.insert(
        "nvidia-nim",
        ProviderPreset {
            base_url: "https://integrate.api.nvidia.com/v1",
            protocol: "openai",
            color: "#76b900",
            default_model: "meta/llama-3.3-70b-instruct",
        },
    );
    m.insert(
        "nvidia",
        ProviderPreset {
            base_url: "https://integrate.api.nvidia.com/v1",
            protocol: "openai",
            color: "#76b900",
            default_model: "meta/llama-3.3-70b-instruct",
        },
    );

    // Inflection AI — Pi-3.5+, OpenAI-compatible.
    m.insert(
        "inflection",
        ProviderPreset {
            base_url: "https://api.inflection.ai/external/api/inference/openai/v1",
            protocol: "openai",
            color: "#f59e0b",
            default_model: "inflection_3_pi",
        },
    );

    // Novita AI — open-model serverless inference, OpenAI-compatible.
    m.insert(
        "novita",
        ProviderPreset {
            base_url: "https://api.novita.ai/v3/openai",
            protocol: "openai",
            color: "#0ea5e9",
            default_model: "meta-llama/llama-3.3-70b-instruct",
        },
    );

    // Chutes — Bittensor-backed open inference, OpenAI-compatible.
    m.insert(
        "chutes",
        ProviderPreset {
            base_url: "https://llm.chutes.ai/v1",
            protocol: "openai",
            color: "#a855f7",
            default_model: "deepseek-ai/DeepSeek-V3-0324",
        },
    );

    m
});
