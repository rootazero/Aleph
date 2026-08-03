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
            // `gpt-5.5` was retired in favour of 5.6; `gpt-5.6-terra` is the
            // mid-priced 5.6 tier and the natural second rung.
            .with_fallback_models(&["gpt-5.6", "gpt-5.6-terra", "gpt-5.4-mini", "o4-mini"]),
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
        // Opus 5 replaces Opus 4.8, which Anthropic's catalog now marks
        // superseded — it sat in this chain as a retry that could only fail.
        .with_fallback_models(&["claude-sonnet-5", "claude-opus-5", "claude-haiku-4-5"]),
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
        // `gemini-3.6-flash` is the current *stable* flash id; both rungs above
        // it are `-preview` ids that can change without notice, so a stable
        // middle rung is what makes this chain a real recovery path.
        .with_fallback_models(&[
            "gemini-3.1-pro-preview",
            "gemini-3.6-flash",
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
            // K3 is Moonshot's current flagship (their own catalog's
            // default); K2.6 stays the cheap aux tier.
            "kimi-k3",
        )
        .with_aliases(&["kimi"])
        .with_display("Moonshot / Kimi")
        .with_homepage("https://platform.moonshot.ai")
        .with_signup("https://platform.moonshot.ai/console/api-keys")
        .with_description("Anthropic-compatible endpoint (recommended)")
        .with_aux_model("kimi-k2.6")
        // Kimi server-manages temperature — sending one returns a fixed-value error.
        .with_temperature_policy(super::TemperaturePolicy::Omit)
        // `kimi-k3` is the open platform's K3 flagship (1M window, $3/$15) —
        // offered, not defaulted to: it is ~3.5x the K2.6 rate. `kimi-k2.7-code`
        // is the code-tuned K2.7 the price table already anticipates.
        .with_fallback_models(&[
            "kimi-k3",
            "kimi-k2.7-code",
            "kimi-k2.6",
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
            // K3 is Moonshot's current flagship (their own catalog's
            // default); K2.6 stays the cheap aux tier.
            "kimi-k3",
        )
        .with_aliases(&["kimi-openai"])
        .with_display("Moonshot / Kimi (OpenAI endpoint)")
        .with_homepage("https://platform.moonshot.ai")
        .with_signup("https://platform.moonshot.ai/console/api-keys")
        .with_description("OpenAI-compatible Kimi K2 / Moonshot chat models")
        .with_aux_model("kimi-k2.6")
        .with_fallback_models(&[
            "kimi-k3",
            "kimi-k2.7-code",
            "kimi-k2.6",
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
            // K3 is Moonshot's current flagship (their own catalog's
            // default); K2.6 stays the cheap aux tier.
            "kimi-k3",
        )
        .with_aliases(&["kimi-cn"])
        .with_display("Moonshot / Kimi (CN)")
        .with_homepage("https://platform.moonshot.cn")
        .with_signup("https://platform.moonshot.cn/console/api-keys")
        .with_description("China-region (api.moonshot.cn) Kimi K2 / Moonshot models")
        .with_aux_model("kimi-k2.6")
        .with_fallback_models(&[
            "kimi-k3",
            "kimi-k2.7-code",
            "kimi-k2.6",
            "kimi-latest",
            "moonshot-v1-128k",
            "moonshot-v1-32k",
            "moonshot-v1-8k",
        ]),
    ),
    // ─── Kimi for Coding (subscription endpoint, own model-id namespace) ──────
    // The coding endpoint does NOT share the open platform's ids: it serves
    // `k3` / `k3-256k` / `kimi-for-coding` / `kimi-for-coding-highspeed`
    // (www.kimi.com/code/docs/kimi-code/models). `kimi-k3` and `Kimi-K2.*` are
    // open-platform / display ids and are translated on the wire by
    // `anthropic::provider_policy::normalize_kimi_coding_model_id`.
    //
    // Billing is plan quota, not per token — the ids carry no rate rows
    // (see `model_catalog::drift_tests::ENDPOINT_LOCAL_ALIASES`).
    (
        "kimi-for-coding",
        ProviderPreset::new(
            "https://api.kimi.com/coding/v1",
            "anthropic",
            "#6366f1",
            // K3 is the flagship of this endpoint (up to 1M window).
            "k3",
        )
        .with_aliases(&["kimi-coding"])
        .with_display("Kimi for Coding")
        .with_homepage("https://www.kimi.com/code")
        .with_signup("https://www.kimi.com/code")
        .with_description("Anthropic-protocol endpoint for IDE agents")
        // Server manages temperature — sending one returns a fixed-value error.
        .with_temperature_policy(super::TemperaturePolicy::Omit)
        // Order is the picker roster as well as the failover ladder:
        // K3 → the 256K lower-consumption K3 → K2.7 Code → its highspeed
        // variant (5-6x output speed at 3x consumption) → legacy aliases.
        .with_fallback_models(&[
            "k3",
            "k3-256k",
            "kimi-for-coding",
            "kimi-for-coding-highspeed",
            "kimi-code",
            "k2p5",
        ]),
    ),
    // ─── Chinese commercial LLMs ──────────────────────────────────────────────
    (
        "doubao",
        ProviderPreset::new(
            "https://ark.cn-beijing.volces.com/api/v3",
            "openai",
            "#ff6b35",
            // Doubao-Seed 2.1 Pro (256K, multimodal) — current Ark flagship.
            "doubao-seed-2-1-pro-260628",
        )
        .with_aliases(&["volcengine", "ark"])
        .with_display("Volcengine Doubao")
        .with_homepage("https://www.volcengine.com/product/ark")
        .with_signup("https://console.volcengine.com/ark")
        .with_aux_model("doubao-seed-2-1-turbo-260628")
        .with_fallback_models(&[
            "doubao-seed-2-1-pro-260628",
            "doubao-seed-2-1-turbo-260628",
            // The one Seed model on a 1M window.
            "doubao-seed-evolving",
        ]),
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
        // GLM-5.1 is retired in Z.ai's own catalog; GLM-5-Turbo is the live
        // mid tier that replaces it in the chain.
        .with_fallback_models(&["GLM-5.2", "glm-5-turbo", "glm-4.7"]),
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
        .with_fallback_models(&[
            "qwen3-max-2026-01-23",
            "qwen3.6-plus",
            // Cheap last resort. Replaces the legacy rolling `qwen-plus`
            // alias, which is superseded by qwen3.6-plus and carried no rate.
            "qwen3.6-flash",
        ]),
    ),
    (
        "qianfan",
        ProviderPreset::new(
            "https://qianfan.baidubce.com/v2",
            "openai",
            "#2932e1",
            "ernie-5.1",
        )
        .with_aliases(&["ernie", "baidu"])
        .with_display("Baidu Qianfan / ERNIE")
        .with_homepage("https://cloud.baidu.com/product/wenxinworkshop")
        .with_signup("https://console.bce.baidu.com/qianfan")
        .with_description("Baidu Qianfan OpenAI-compatible endpoint")
        .with_aux_model("ernie-5.1")
        // Qianfan also fronts DeepSeek V4, but mixing a priced rung into an
        // otherwise vendor-priced chain is what the rate-coverage guard reads
        // as drift — the ERNIE pair is the vendor's own line.
        .with_fallback_models(&["ernie-5.1", "ernie-5.0"]),
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
            // The Step-3.x Flash line supersedes the Step-1 generation whose
            // window was encoded in the id; `step-1-8k` gave the agent an 8K
            // window to plan against.
            "step-3.7-flash",
        )
        .with_display("Stepfun")
        .with_homepage("https://stepfun.com")
        .with_signup("https://platform.stepfun.com")
        .with_aux_model("step-3.5-flash")
        .with_fallback_models(&["step-3.7-flash", "step-3.5-flash"]),
    ),
    (
        "t8star",
        ProviderPreset::new("https://api.t8star.cn/v1", "openai", "#f59e0b", "")
            .with_display("T8Star")
            .with_homepage("https://t8star.cn")
            .with_description("OpenAI-compatible aggregator; name your model")
            .requires_explicit_model(),
    ),
    // ─── Western specialty / inference ────────────────────────────────────────
    (
        "groq",
        ProviderPreset::new(
            "https://api.groq.com/openai/v1",
            "openai",
            "#f55036",
            // Groq retired both Llama tiers in favour of the gpt-oss pair.
            // The same Llama ids stay current on Together / Cerebras /
            // DeepInfra, which is why the lifecycle rows are Groq-scoped.
            "openai/gpt-oss-120b",
        )
        .with_display("Groq")
        .with_homepage("https://groq.com")
        .with_signup("https://console.groq.com/keys")
        .with_description("Ultra-fast LPU inference")
        .with_fallback_models(&["openai/gpt-oss-120b", "openai/gpt-oss-20b"]),
    ),
    (
        "cerebras",
        ProviderPreset::new(
            "https://api.cerebras.ai/v1",
            "openai",
            "#f97316",
            "gpt-oss-120b",
        )
        .with_display("Cerebras")
        .with_homepage("https://cerebras.ai")
        .with_signup("https://cloud.cerebras.ai")
        .with_description("Ultra-fast open-weight inference")
        .with_fallback_models(&["gpt-oss-120b", "zai-glm-4.7"]),
    ),
    (
        "together",
        ProviderPreset::new(
            "https://api.together.xyz/v1",
            "openai",
            "#6366f1",
            "moonshotai/Kimi-K2.6",
        )
        .with_display("Together.ai")
        .with_homepage("https://www.together.ai")
        .with_signup("https://api.together.xyz/settings/api-keys")
        // Deliberately homogeneous w.r.t. priceability: Together also serves
        // Llama-3.3, but open weights are (correctly) unpriced, and mixing an
        // unpriced rung into a chain whose other rungs price is exactly what
        // `advertised_models_of_priced_vendors_have_rates` reports as drift.
        .with_fallback_models(&[
            "moonshotai/Kimi-K2.6",
            "deepseek-ai/DeepSeek-V4-Pro",
            "zai-org/GLM-5.2",
        ]),
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
            // Cohere folded Command A / A-Reasoning / A-Vision into one
            // flagship; the previous default here was among the retired three.
            "command-a-plus-05-2026",
        )
        .with_display("Cohere")
        .with_homepage("https://docs.cohere.com/docs/compatibility-api")
        .with_signup("https://dashboard.cohere.com/api-keys")
        .with_description("OpenAI-compatible endpoint (/compatibility/v1)")
        .with_fallback_models(&["command-a-plus-05-2026", "north-mini-code-1-0"]),
    ),
    (
        "fireworks",
        ProviderPreset::new(
            "https://api.fireworks.ai/inference/v1",
            "openai",
            "#ff6b35",
            // Fireworks writes the generation separator as `p`
            // (`kimi-k2p6` = Kimi K2.6); `canonicalize_model_id` normalises it
            // so these still reach the curated capability / price rows.
            "accounts/fireworks/models/kimi-k2p6",
        )
        .with_display("Fireworks.ai")
        .with_homepage("https://fireworks.ai")
        .with_signup("https://fireworks.ai/account/api-keys")
        .with_fallback_models(&[
            "accounts/fireworks/models/kimi-k2p6",
            "accounts/fireworks/routers/glm-5p2-fast",
        ]),
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
        .with_homepage("https://www.anyscale.com")
        .with_description("Per-deployment endpoints; name your model")
        .requires_explicit_model(),
    ),
    (
        "replicate",
        ProviderPreset::new("https://api.replicate.com/v1", "openai", "#0c0c0d", "")
            .with_display("Replicate")
            .with_homepage("https://replicate.com")
            .with_signup("https://replicate.com/account/api-tokens")
            .with_description("Hosting; image/video via generation layer")
            .requires_explicit_model(),
    ),
    (
        "openrouter",
        ProviderPreset::new(
            "https://openrouter.ai/api",
            "openai-responses",
            "#6467f2",
            // OpenRouter mirrors each vendor's own id under a `<vendor>/` tag,
            // so this roster is derived from the direct-vendor presets above
            // rather than maintained independently — it had drifted two
            // generations behind them (gpt-4o / sonnet-4 / gemini-2.5-flash).
            // For aggregators generally, the durable answer is on-demand
            // discovery (`model_catalog::discovery`), not a hand-kept list.
            "openai/gpt-5.6",
        )
        .with_aliases(&["or"])
        .with_display("OpenRouter")
        .with_homepage("https://openrouter.ai")
        .with_signup("https://openrouter.ai/keys")
        .with_description("Multi-model router")
        .with_fallback_models(&[
            "openai/gpt-5.6",
            "anthropic/claude-sonnet-5",
            "google/gemini-3.1-pro-preview",
        ]),
    ),
    (
        "lepton",
        ProviderPreset::new("https://api.lepton.ai/api/v1", "openai", "#4f46e5", "")
            .with_display("Lepton AI")
            .with_homepage("https://www.lepton.ai")
            .with_description("Per-deployment hosting; name your model")
            .requires_explicit_model(),
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
            // Aux is the *current* cheap tier, not the previous generation's:
            // grok-4-fast ($0.20/$0.50) beats grok-3-mini ($0.30/$0.50) on both
            // axes and shares the grok-4 window.
            .with_aux_model("grok-4-fast")
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
            "deepseek-ai/DeepSeek-V4-Flash",
        )
        .with_display("DeepInfra")
        .with_homepage("https://deepinfra.com/docs")
        .with_signup("https://deepinfra.com/dash/api_keys")
        .with_description("Open-model inference, OpenAI-compatible")
        .with_fallback_models(&[
            "deepseek-ai/DeepSeek-V4-Flash",
            "deepseek-ai/DeepSeek-V4-Pro",
            "zai-org/GLM-5.2",
            "moonshotai/Kimi-K2.6",
        ]),
    ),
    (
        "github-copilot",
        ProviderPreset::new(
            "https://api.githubcopilot.com",
            "openai",
            "#24292f",
            // Copilot rotates its roster independently of the vendors; the
            // ids it drops are recorded as `github-copilot`-scoped lifecycle
            // rows rather than vendor-wide ones.
            "gpt-5.6-sol",
        )
        .with_aliases(&["copilot"])
        .with_display("GitHub Copilot")
        .with_homepage("https://docs.github.com/copilot")
        .with_signup("https://github.com/settings/copilot")
        .with_description("Requires Copilot subscription token")
        .with_fallback_models(&["gpt-5.6-sol", "claude-sonnet-5", "gemini-3.6-flash"]),
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
            "nvidia/nemotron-3-ultra-550b-a55b",
        )
        .with_aliases(&["nvidia"])
        .with_display("NVIDIA NIM")
        .with_homepage("https://docs.nvidia.com/nim")
        .with_signup("https://build.nvidia.com")
        .with_description("NGC-hosted inference catalog")
        .with_fallback_models(&[
            "nvidia/nemotron-3-ultra-550b-a55b",
            "nvidia/nemotron-3-super-120b-a12b",
        ]),
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
            "deepseek/deepseek-v4-pro",
        )
        .with_display("Novita AI")
        .with_homepage("https://novita.ai/docs")
        .with_signup("https://novita.ai/settings/key-management")
        .with_description("Serverless open-model inference")
        .with_fallback_models(&[
            "deepseek/deepseek-v4-pro",
            "moonshotai/kimi-k3",
            "zai-org/glm-5.2",
            "minimax/minimax-m3",
        ]),
    ),
    (
        "chutes",
        ProviderPreset::new(
            "https://llm.chutes.ai/v1",
            "openai",
            "#a855f7",
            // Chutes serves every model inside a TEE and marks the id `-TEE`.
            "zai-org/GLM-5.2-TEE",
        )
        .with_display("Chutes")
        .with_homepage("https://chutes.ai")
        .with_signup("https://chutes.ai")
        .with_description("Bittensor-backed open inference (TEE)")
        .with_fallback_models(&[
            "zai-org/GLM-5.2-TEE",
            "moonshotai/Kimi-K2.6-TEE",
            "deepseek-ai/DeepSeek-V3.2-TEE",
        ]),
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
            "deepseek-ai/DeepSeek-V4-Pro",
        )
        .with_display("GMI Cloud")
        .with_homepage("https://www.gmicloud.ai")
        .with_signup("https://www.gmicloud.ai")
        .with_description("Multi-model direct API")
        .with_fallback_models(&[
            "deepseek-ai/DeepSeek-V4-Pro",
            "zai-org/GLM-5.2-FP8",
            "openai/gpt-5.6-sol",
        ]),
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
        // Z.ai's roster is 5.2 / 5-Turbo / 5V-Turbo; `glm-5` and `glm-4.6`
        // have both rolled off it.
        .with_fallback_models(&["glm-5.2", "glm-5-turbo", "glm-4.7"]),
    ),
    (
        "baseten",
        ProviderPreset::new(
            "https://inference.baseten.co/v1",
            "openai",
            "#0f172a",
            "moonshotai/Kimi-K2.6",
        )
        .with_display("Baseten")
        .with_homepage("https://www.baseten.co")
        .with_signup("https://app.baseten.co/settings/api_keys")
        .with_description("Dedicated deployments for hosted open models")
        .with_fallback_models(&[
            "moonshotai/Kimi-K2.6",
            "zai-org/GLM-5.2",
            "deepseek-ai/DeepSeek-V4-Pro",
        ]),
    ),
    (
        "xiaomi",
        ProviderPreset::new(
            "https://api.xiaomimimo.com/v1",
            "openai",
            "#ff6900",
            "mimo-v2.5-pro",
        )
        .with_aliases(&["mimo"])
        .with_display("Xiaomi MiMo")
        .with_homepage("https://xiaomimimo.com")
        .with_signup("https://xiaomimimo.com")
        .with_description("MiMo v2.5 — 1M window, multimodal")
        .with_aux_model("mimo-v2.5")
        .with_fallback_models(&["mimo-v2.5-pro", "mimo-v2.5"]),
    ),
    (
        "longcat",
        ProviderPreset::new(
            "https://api.longcat.chat/openai",
            "openai",
            "#f43f5e",
            "LongCat-2.0",
        )
        .with_display("LongCat (Meituan)")
        .with_homepage("https://longcat.chat")
        .with_signup("https://longcat.chat"),
    ),
    (
        "ollama-cloud",
        ProviderPreset::new("https://ollama.com/api/v1", "openai", "#0c0c0d", "glm-5.2")
            .with_display("Ollama Cloud")
            .with_homepage("https://ollama.com")
            .with_signup("https://ollama.com")
            .with_description("Cloud-hosted Ollama (vs. local default)")
            .with_fallback_models(&["glm-5.2", "minimax-m2.7"]),
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
