//! Per-model capability metadata.
//!
//! Aleph previously carried capability bits only at the *protocol/endpoint*
//! layer (`AnthropicCapabilities`, `OpenAI` provider policy) — i.e. "what does
//! this wire protocol accept", not "what can this *model* do". Both openclaw
//! (`Model { input[], reasoning, contextWindow, maxTokens }`) and hermes
//! (`ModelInfo { reasoning, tool_call, attachment, context_window,
//! max_output }`) expose per-model capability metadata so callers can answer
//! "does this model see images?" / "how big is its context?".
//!
//! This module closes that gap with a curated static table. It is **data,
//! not routing** (R7 LLM sovereignty): callers and the LLM consult it to
//! reason about model choice; nothing here picks a model automatically.
//!
//! Lookup is prefix-based on [`canonicalize_model_id`], identical in spirit
//! to the `pricing` price table — the first declared prefix that matches the
//! canonicalised id wins, so specific prefixes precede broad ones.

use serde::Serialize;

use super::alias::canonicalize_model_id;

/// Capability metadata for one model family.
///
/// Figures are best-effort reference data (vendor docs as of 2026-07),
/// mirroring `pricing`'s "operators upgrade Aleph to refresh" stance — no
/// runtime config knob, no network lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ModelCapabilities {
    /// Maximum total context window in tokens (input + output budget).
    pub context_window: u32,
    /// Maximum output tokens the model will emit in one response.
    pub max_output_tokens: u32,
    /// Accepts image input (multimodal vision).
    pub supports_vision: bool,
    /// Supports native tool / function calling.
    pub supports_tools: bool,
    /// Has an extended-thinking / reasoning mode.
    pub supports_reasoning: bool,
}

/// `(canonical model-id prefix, capabilities)`. Declaration order matters —
/// list specific prefixes (`claude-opus-4`) before broad ones (`claude-3`).
const CAPABILITY_TABLE: &[(&str, ModelCapabilities)] = &[
    // ── Anthropic ────────────────────────────────────────────────────────
    // Generation-5 flagship: 1M context, 128K output, adaptive reasoning.
    (
        "claude-fable-5",
        ModelCapabilities {
            context_window: 1_000_000,
            max_output_tokens: 128_000,
            supports_vision: true,
            supports_tools: true,
            supports_reasoning: true,
        },
    ),
    // Opus 4.6+ moved to a 1M window / 128K output; the broad
    // `claude-opus-4` fallback below keeps the 4.0/4.1-era figures, so
    // these specific prefixes must precede it.
    (
        "claude-opus-4-8",
        ModelCapabilities {
            context_window: 1_000_000,
            max_output_tokens: 128_000,
            supports_vision: true,
            supports_tools: true,
            supports_reasoning: true,
        },
    ),
    (
        "claude-opus-4-7",
        ModelCapabilities {
            context_window: 1_000_000,
            max_output_tokens: 128_000,
            supports_vision: true,
            supports_tools: true,
            supports_reasoning: true,
        },
    ),
    (
        "claude-opus-4-6",
        ModelCapabilities {
            context_window: 1_000_000,
            max_output_tokens: 128_000,
            supports_vision: true,
            supports_tools: true,
            supports_reasoning: true,
        },
    ),
    (
        "claude-opus-4-5",
        ModelCapabilities {
            context_window: 200_000,
            max_output_tokens: 64_000,
            supports_vision: true,
            supports_tools: true,
            supports_reasoning: true,
        },
    ),
    (
        "claude-opus-4",
        ModelCapabilities {
            context_window: 200_000,
            max_output_tokens: 32_000,
            supports_vision: true,
            supports_tools: true,
            supports_reasoning: true,
        },
    ),
    // Sonnet 5 (Claude 5 family) is the current balanced flagship: 1M window,
    // 128K output. Must precede the sonnet-4.x prefixes (distinct id, but keep
    // generations grouped newest-first).
    (
        "claude-sonnet-5",
        ModelCapabilities {
            context_window: 1_000_000,
            max_output_tokens: 128_000,
            supports_vision: true,
            supports_tools: true,
            supports_reasoning: true,
        },
    ),
    // Sonnet 4.6 carries the 1M window; older sonnet-4.x stay on 200K via
    // the broad fallback below.
    (
        "claude-sonnet-4-6",
        ModelCapabilities {
            context_window: 1_000_000,
            max_output_tokens: 64_000,
            supports_vision: true,
            supports_tools: true,
            supports_reasoning: true,
        },
    ),
    (
        "claude-sonnet-4",
        ModelCapabilities {
            context_window: 200_000,
            max_output_tokens: 64_000,
            supports_vision: true,
            supports_tools: true,
            supports_reasoning: true,
        },
    ),
    (
        "claude-haiku-4-5",
        ModelCapabilities {
            context_window: 200_000,
            max_output_tokens: 64_000,
            supports_vision: true,
            supports_tools: true,
            supports_reasoning: true,
        },
    ),
    (
        "claude-haiku-4",
        ModelCapabilities {
            context_window: 200_000,
            max_output_tokens: 32_000,
            supports_vision: true,
            supports_tools: true,
            supports_reasoning: true,
        },
    ),
    (
        "claude-3",
        ModelCapabilities {
            context_window: 200_000,
            max_output_tokens: 8_192,
            supports_vision: true,
            supports_tools: true,
            supports_reasoning: false,
        },
    ),
    // ── OpenAI ───────────────────────────────────────────────────────────
    // GPT-5 family (openclaw catalog). Specific dotted prefixes precede the
    // broad `gpt-5` fallback. 5.6 is the current default (openclaw
    // OPENAI_DEFAULT_MODEL): ~1.05M window, 128K output.
    (
        "gpt-5.6",
        ModelCapabilities {
            context_window: 1_050_000,
            max_output_tokens: 128_000,
            supports_vision: true,
            supports_tools: true,
            supports_reasoning: true,
        },
    ),
    (
        "gpt-5.5",
        ModelCapabilities {
            context_window: 1_000_000,
            max_output_tokens: 128_000,
            supports_vision: true,
            supports_tools: true,
            supports_reasoning: true,
        },
    ),
    (
        "gpt-5.4-mini",
        ModelCapabilities {
            context_window: 400_000,
            max_output_tokens: 128_000,
            supports_vision: true,
            supports_tools: true,
            supports_reasoning: true,
        },
    ),
    (
        "gpt-5.4-nano",
        ModelCapabilities {
            context_window: 400_000,
            max_output_tokens: 128_000,
            supports_vision: true,
            supports_tools: true,
            supports_reasoning: true,
        },
    ),
    (
        "gpt-5.4",
        ModelCapabilities {
            context_window: 272_000,
            max_output_tokens: 128_000,
            supports_vision: true,
            supports_tools: true,
            supports_reasoning: true,
        },
    ),
    (
        "gpt-5.3-codex",
        ModelCapabilities {
            context_window: 400_000,
            max_output_tokens: 128_000,
            supports_vision: true,
            supports_tools: true,
            supports_reasoning: true,
        },
    ),
    (
        "gpt-5",
        ModelCapabilities {
            context_window: 272_000,
            max_output_tokens: 128_000,
            supports_vision: true,
            supports_tools: true,
            supports_reasoning: true,
        },
    ),
    (
        "o4-mini",
        ModelCapabilities {
            context_window: 200_000,
            max_output_tokens: 100_000,
            supports_vision: true,
            supports_tools: true,
            supports_reasoning: true,
        },
    ),
    (
        "o3",
        ModelCapabilities {
            context_window: 200_000,
            max_output_tokens: 100_000,
            supports_vision: true,
            supports_tools: true,
            supports_reasoning: true,
        },
    ),
    (
        "o1-mini",
        ModelCapabilities {
            context_window: 128_000,
            max_output_tokens: 65_536,
            supports_vision: false,
            supports_tools: false,
            supports_reasoning: true,
        },
    ),
    (
        "o1",
        ModelCapabilities {
            context_window: 200_000,
            max_output_tokens: 100_000,
            supports_vision: true,
            supports_tools: true,
            supports_reasoning: true,
        },
    ),
    (
        "gpt-4o-mini",
        ModelCapabilities {
            context_window: 128_000,
            max_output_tokens: 16_384,
            supports_vision: true,
            supports_tools: true,
            supports_reasoning: false,
        },
    ),
    (
        "gpt-4o",
        ModelCapabilities {
            context_window: 128_000,
            max_output_tokens: 16_384,
            supports_vision: true,
            supports_tools: true,
            supports_reasoning: false,
        },
    ),
    (
        "gpt-4",
        ModelCapabilities {
            context_window: 128_000,
            max_output_tokens: 4_096,
            supports_vision: false,
            supports_tools: true,
            supports_reasoning: false,
        },
    ),
    // ── Google ───────────────────────────────────────────────────────────
    // Gemini 3.x previews (openclaw catalog) — covers gemini-3-flash and
    // gemini-3.1-* ids.
    (
        "gemini-3",
        ModelCapabilities {
            context_window: 1_048_576,
            max_output_tokens: 65_536,
            supports_vision: true,
            supports_tools: true,
            supports_reasoning: true,
        },
    ),
    (
        "gemini-2.5-pro",
        ModelCapabilities {
            context_window: 1_048_576,
            max_output_tokens: 65_536,
            supports_vision: true,
            supports_tools: true,
            supports_reasoning: true,
        },
    ),
    (
        "gemini-2.5-flash",
        ModelCapabilities {
            context_window: 1_048_576,
            max_output_tokens: 65_536,
            supports_vision: true,
            supports_tools: true,
            supports_reasoning: true,
        },
    ),
    (
        "gemini-2.0-flash",
        ModelCapabilities {
            context_window: 1_048_576,
            max_output_tokens: 8_192,
            supports_vision: true,
            supports_tools: true,
            supports_reasoning: false,
        },
    ),
    (
        "gemini",
        ModelCapabilities {
            context_window: 1_048_576,
            max_output_tokens: 8_192,
            supports_vision: true,
            supports_tools: true,
            supports_reasoning: false,
        },
    ),
    // ── DeepSeek ─────────────────────────────────────────────────────────
    // V4 family: 1M context (official pricing/models pages + openclaw). The
    // legacy `deepseek-chat` / `deepseek-reasoner` names retire 2026-07-24 and
    // now resolve to v4-flash non-thinking / thinking modes — both 1M window.
    //
    // NOTE on max_output: the vendor spec caps output at 384K, but this figure
    // doubles as the compaction *reserve* (`derive_token_budget`:
    // usable = window − max_output). Reserving 384K would shrink usable context
    // by ~38% every turn for output volumes agents almost never emit. We use a
    // realistic 64K reserve (peer-consistent with gemini-3 / gpt-5-mini);
    // callers needing the full 384K set `[providers.*] max_tokens` explicitly.
    (
        "deepseek-v4",
        ModelCapabilities {
            context_window: 1_000_000,
            max_output_tokens: 65_536,
            supports_vision: false,
            supports_tools: true,
            supports_reasoning: true,
        },
    ),
    (
        "deepseek-reasoner",
        ModelCapabilities {
            context_window: 1_000_000,
            max_output_tokens: 65_536,
            supports_vision: false,
            supports_tools: true,
            supports_reasoning: true,
        },
    ),
    (
        "deepseek",
        ModelCapabilities {
            context_window: 1_000_000,
            max_output_tokens: 65_536,
            supports_vision: false,
            supports_tools: true,
            supports_reasoning: false,
        },
    ),
    // ── xAI ──────────────────────────────────────────────────────────────
    // Grok 4.x generations (openclaw model-definitions): specific dotted /
    // suffixed prefixes precede `grok-4`, which precedes the grok-3-era
    // broad fallback.
    (
        "grok-4.3",
        ModelCapabilities {
            context_window: 1_000_000,
            max_output_tokens: 64_000,
            supports_vision: true,
            supports_tools: true,
            supports_reasoning: true,
        },
    ),
    (
        "grok-4-fast",
        ModelCapabilities {
            context_window: 2_000_000,
            max_output_tokens: 30_000,
            supports_vision: true,
            supports_tools: true,
            supports_reasoning: true,
        },
    ),
    (
        "grok-4",
        ModelCapabilities {
            context_window: 256_000,
            max_output_tokens: 64_000,
            supports_vision: true,
            supports_tools: true,
            supports_reasoning: true,
        },
    ),
    (
        "grok",
        ModelCapabilities {
            context_window: 131_072,
            max_output_tokens: 16_384,
            supports_vision: true,
            supports_tools: true,
            supports_reasoning: true,
        },
    ),
    // ── Mistral ──────────────────────────────────────────────────────────
    (
        "mistral-large",
        ModelCapabilities {
            context_window: 131_072,
            max_output_tokens: 8_192,
            supports_vision: false,
            supports_tools: true,
            supports_reasoning: false,
        },
    ),
    (
        // Broad fallback for the rest of the family (medium / small /
        // codestral / ministral) — all 128K-window tool-callers. The `mistral`
        // preset advertises `mistral-medium-latest` and `mistral-small-latest`
        // in its fallback chain, and without this row both sized at the
        // conservative 128K default *by accident* rather than by record.
        "mistral",
        ModelCapabilities {
            context_window: 131_072,
            max_output_tokens: 8_192,
            supports_vision: false,
            supports_tools: true,
            supports_reasoning: false,
        },
    ),
    // ── MiniMax ──────────────────────────────────────────────────────────
    // Aleph ships a `minimax` preset (default MiniMax-M3) with per-model
    // metadata so panel picker / list_models / context budgeting size it
    // correctly. M2.x is a 204K-window text-only reasoning model; M3 widens the
    // window to 1M AND is natively multimodal (image/video). Figures are
    // vendor-doc references (2026-07).
    (
        // M3 is natively multimodal — its Anthropic-compatible endpoint
        // accepts image_url / video_url content blocks (vendor doc + openclaw
        // input=[text,image]). The older M2.x chat family is text-only (below).
        "minimax-m3",
        ModelCapabilities {
            context_window: 1_000_000,
            max_output_tokens: 32_768,
            supports_vision: true,
            supports_tools: true,
            supports_reasoning: true,
        },
    ),
    (
        // M2 / M2.1 / M2.5 / M2.7 chat family — 204K context, tool-calling,
        // interleaved thinking. Broad `minimax` fallback follows.
        "minimax-m2",
        ModelCapabilities {
            context_window: 204_800,
            max_output_tokens: 16_384,
            supports_vision: false,
            supports_tools: true,
            supports_reasoning: true,
        },
    ),
    (
        "minimax",
        ModelCapabilities {
            context_window: 204_800,
            max_output_tokens: 8_192,
            supports_vision: false,
            supports_tools: true,
            supports_reasoning: false,
        },
    ),
    // ── Moonshot / Kimi ──────────────────────────────────────────────────
    // Kimi ships K3 under TWO id namespaces: the open platform serves
    // `kimi-k3` (api.moonshot.ai), the Kimi Code subscription endpoint serves
    // bare `k3` / `k3-256k` (api.kimi.com/coding). Both are the same model;
    // both need a row, because neither shape is reachable from the other's
    // prefix. Max output is the API schema's documented 131072
    // `max_completion_tokens` default for K3.
    //
    // `k3-256k` MUST precede `k3` — `k3` is a prefix of it, and the scan takes
    // the first match.
    (
        "k3-256k",
        ModelCapabilities {
            // Lower-consumption K3 variant: same model, 256K window,
            // images only (no video).
            context_window: 262_144,
            max_output_tokens: 131_072,
            supports_vision: true,
            supports_tools: true,
            supports_reasoning: true,
        },
    ),
    (
        "k3",
        ModelCapabilities {
            context_window: 1_048_576,
            max_output_tokens: 131_072,
            supports_vision: true,
            supports_tools: true,
            supports_reasoning: true,
        },
    ),
    // Open-platform K3. Must precede the broad `kimi` row below, which would
    // size this 1M-window model at 200K and trigger premature compaction.
    (
        "kimi-k3",
        ModelCapabilities {
            context_window: 1_048_576,
            max_output_tokens: 131_072,
            supports_vision: true,
            supports_tools: true,
            supports_reasoning: true,
        },
    ),
    // Kimi-for-coding endpoint model (256K/32K, multimodal). Also covers
    // `kimi-for-coding-highspeed`, which is the same model served faster.
    // Distinct prefix that would otherwise fall to the broad `kimi` 200K/8K
    // row and under-size its window — must precede it.
    (
        "kimi-for-coding",
        ModelCapabilities {
            context_window: 262_144,
            max_output_tokens: 32_768,
            supports_vision: true,
            supports_tools: true,
            supports_reasoning: true,
        },
    ),
    // K2 family (k2.5 / k2.6 / k2.7 / k2-* previews): 256K window, multimodal.
    (
        "kimi-k2",
        ModelCapabilities {
            context_window: 262_144,
            max_output_tokens: 32_768,
            supports_vision: true,
            supports_tools: true,
            supports_reasoning: true,
        },
    ),
    (
        "kimi",
        ModelCapabilities {
            context_window: 200_000,
            max_output_tokens: 8_192,
            supports_vision: false,
            supports_tools: true,
            supports_reasoning: true,
        },
    ),
    (
        "moonshot",
        ModelCapabilities {
            context_window: 131_072,
            max_output_tokens: 8_192,
            supports_vision: false,
            supports_tools: true,
            supports_reasoning: false,
        },
    ),
    // ── Zhipu / GLM ──────────────────────────────────────────────────────
    // GLM-5.2 is the current flagship: 1M lossless context, 128K output. Its
    // `glm-5.2` prefix MUST precede `glm-5` (which it starts with). GLM-5 /
    // GLM-5.1 keep the 200K window per the official bigmodel.cn / z.ai docs.
    (
        "glm-5.2",
        ModelCapabilities {
            context_window: 1_000_000,
            max_output_tokens: 128_000,
            supports_vision: false,
            supports_tools: true,
            supports_reasoning: true,
        },
    ),
    (
        "glm-5",
        ModelCapabilities {
            context_window: 200_000,
            max_output_tokens: 128_000,
            supports_vision: false,
            supports_tools: true,
            supports_reasoning: true,
        },
    ),
    (
        "glm",
        ModelCapabilities {
            context_window: 200_000,
            max_output_tokens: 8_192,
            supports_vision: false,
            supports_tools: true,
            supports_reasoning: true,
        },
    ),
    // ── Alibaba / Qwen ───────────────────────────────────────────────────
    // qwen3-max (current flagship default): 262K window, 64K output. The dated
    // id qwen3-max-2026-01-23 canonicalises (date-stripped) to `qwen3-max`, so
    // this prefix must precede the broad `qwen` 128K row.
    (
        "qwen3-max",
        ModelCapabilities {
            context_window: 262_144,
            max_output_tokens: 65_536,
            supports_vision: false,
            supports_tools: true,
            supports_reasoning: false,
        },
    ),
    (
        "qwen",
        ModelCapabilities {
            context_window: 131_072,
            max_output_tokens: 8_192,
            supports_vision: false,
            supports_tools: true,
            supports_reasoning: false,
        },
    ),
    // ── Volcengine Doubao (Ark) ────────────────────────────────────────────
    // Previously ABSENT — every doubao id fell to the conservative 128K default
    // though the real Seed window is 256K, so the occupancy gauge and the
    // compaction budget under-sized every Doubao run. `doubao-seed` (the Seed
    // 1.x/2.x flagship line, multimodal) precedes the broad `doubao` fallback
    // for legacy non-Seed ids (e.g. the prior default doubao-1.5-pro-256k).
    (
        "doubao-seed",
        ModelCapabilities {
            context_window: 256_000,
            max_output_tokens: 32_768,
            supports_vision: true,
            supports_tools: true,
            supports_reasoning: true,
        },
    ),
    (
        "doubao",
        ModelCapabilities {
            context_window: 256_000,
            max_output_tokens: 16_384,
            supports_vision: false,
            supports_tools: true,
            supports_reasoning: false,
        },
    ),
    // ── Meta Llama (open weights) ──────────────────────────────────────────
    // Served by ~10 presets (Groq, Cerebras, Together, HuggingFace, DeepInfra,
    // NVIDIA NIM, Novita, Hyperbolic, Ollama). `meta-llama/` and `meta/` tags
    // are peeled by `canonicalize_model_id`, so vendor-prefixed ids resolve.
    // Specific generations precede the broad `llama` fallback. Llama is
    // text-only (vision lives in the separate 3.2-Vision / 4 multimodal lines)
    // and has no extended-thinking mode.
    (
        // Llama 4 (Scout/Maverick) is natively multimodal; conservative 1M
        // window (Scout advertises 10M, Maverick 1M — take the lower).
        "llama-4",
        ModelCapabilities {
            context_window: 1_000_000,
            max_output_tokens: 8_192,
            supports_vision: true,
            supports_tools: true,
            supports_reasoning: false,
        },
    ),
    (
        "llama-3.3",
        ModelCapabilities {
            context_window: 131_072,
            max_output_tokens: 8_192,
            supports_vision: false,
            supports_tools: true,
            supports_reasoning: false,
        },
    ),
    (
        // 3.2 Vision (11B/90B) accepts images; the 1B/3B text variants share
        // the 128K window, so the vision flag is the only family difference.
        "llama-3.2",
        ModelCapabilities {
            context_window: 131_072,
            max_output_tokens: 8_192,
            supports_vision: true,
            supports_tools: true,
            supports_reasoning: false,
        },
    ),
    (
        "llama-3.1",
        ModelCapabilities {
            context_window: 131_072,
            max_output_tokens: 8_192,
            supports_vision: false,
            supports_tools: true,
            supports_reasoning: false,
        },
    ),
    (
        // Original Llama 3 (8B/70B) shipped an 8K window — must precede the
        // broad `llama` fallback so it isn't widened to 128K.
        "llama-3",
        ModelCapabilities {
            context_window: 8_192,
            max_output_tokens: 4_096,
            supports_vision: false,
            supports_tools: true,
            supports_reasoning: false,
        },
    ),
    (
        // Broad fallback for dotless / hosted ids (e.g. ollama `llama3.3:70b`).
        "llama",
        ModelCapabilities {
            context_window: 131_072,
            max_output_tokens: 8_192,
            supports_vision: false,
            supports_tools: true,
            supports_reasoning: false,
        },
    ),
    // ── Cohere Command ─────────────────────────────────────────────────────
    // OpenAI-compatible via /compatibility/v1. Command-A (256K) precedes the
    // broad `command` (Command-R/R+ at 128K).
    (
        "command-a",
        ModelCapabilities {
            context_window: 256_000,
            max_output_tokens: 8_192,
            supports_vision: false,
            supports_tools: true,
            supports_reasoning: false,
        },
    ),
    (
        "command",
        ModelCapabilities {
            context_window: 128_000,
            max_output_tokens: 4_096,
            supports_vision: false,
            supports_tools: true,
            supports_reasoning: false,
        },
    ),
    // ── Perplexity Sonar ───────────────────────────────────────────────────
    // Search-augmented. `sonar-reasoning*` exposes CoT (reasoning=true) and
    // must precede `sonar-pro` / broad `sonar`. Sonar has no function calling.
    (
        "sonar-reasoning",
        ModelCapabilities {
            context_window: 128_000,
            max_output_tokens: 8_192,
            supports_vision: false,
            supports_tools: false,
            supports_reasoning: true,
        },
    ),
    (
        "sonar-pro",
        ModelCapabilities {
            context_window: 200_000,
            max_output_tokens: 8_192,
            supports_vision: false,
            supports_tools: false,
            supports_reasoning: false,
        },
    ),
    (
        "sonar",
        ModelCapabilities {
            context_window: 128_000,
            max_output_tokens: 8_192,
            supports_vision: false,
            supports_tools: false,
            supports_reasoning: false,
        },
    ),
    // ── StepFun ────────────────────────────────────────────────────────────
    // StepFun encodes the window in the id itself (`step-1-8k`, `step-1-32k`,
    // `step-1-256k`); the `stepfun` preset ships the 8K variant, which is far
    // below the conservative 128K default it used to inherit — the one
    // direction where a missing row is actively dangerous, since the context
    // budget would have planned for 16x the room the model has.
    (
        "step-1-256k",
        ModelCapabilities {
            context_window: 262_144,
            max_output_tokens: 8_192,
            supports_vision: false,
            supports_tools: true,
            supports_reasoning: false,
        },
    ),
    (
        "step-1-32k",
        ModelCapabilities {
            context_window: 32_768,
            max_output_tokens: 8_192,
            supports_vision: false,
            supports_tools: true,
            supports_reasoning: false,
        },
    ),
    (
        "step-1-8k",
        ModelCapabilities {
            context_window: 8_192,
            max_output_tokens: 4_096,
            supports_vision: false,
            supports_tools: true,
            supports_reasoning: false,
        },
    ),
];

/// Look up capability metadata for a model id (raw or canonical).
///
/// Returns `None` when no family prefix matches — callers treat that as
/// "capabilities unknown" and fall back to their own defaults.
#[must_use]
pub fn capabilities_for(model: &str) -> Option<ModelCapabilities> {
    let canon = canonicalize_model_id(model);
    CAPABILITY_TABLE
        .iter()
        .find(|(prefix, _)| canon.starts_with(prefix))
        .map(|(_, caps)| *caps)
}

/// Conservative context window (tokens) for models absent from the capability
/// catalogue — keeps the occupancy gauge meaningful for custom / local models
/// instead of failing. Matches the panel's prior unknown-model fallback so the
/// migration to core-authoritative windows is behaviour-preserving for them.
pub const CONSERVATIVE_CONTEXT_WINDOW: u32 = 128_000;

/// Authoritative context-window size for a model id, with a conservative
/// fallback. Display/occupancy consumers use this as the gauge denominator
/// (R7 — the window lookup is business logic and lives in core, not the panel).
#[must_use]
pub fn resolve_context_window(model: &str) -> u32 {
    capabilities_for(model)
        .map(|c| c.context_window)
        .unwrap_or(CONSERVATIVE_CONTEXT_WINDOW)
}

/// Context window honoring an explicit per-provider `context_window` override
/// before falling back to the catalogue. Mirrors the precedence the agent's
/// token budget already applies in `deps_builder::derive_token_budget`
/// (config ▸ catalog ▸ conservative), so the occupancy gauge and the
/// compaction budget agree on the denominator instead of silently diverging
/// when a user pins a custom window in `[providers.*] context_window`.
#[must_use]
pub fn resolve_context_window_with_override(override_window: Option<u32>, model: &str) -> u32 {
    override_window
        .filter(|&w| w > 0)
        .unwrap_or_else(|| resolve_context_window(model))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_up_anthropic_specific_before_broad() {
        let opus = capabilities_for("claude-opus-4-1-20250805").unwrap();
        assert_eq!(opus.max_output_tokens, 32_000);
        assert!(opus.supports_reasoning);

        // claude-3 family is non-reasoning — the broad fallback must not
        // shadow the specific claude-opus-4 entry above.
        let haiku3 = capabilities_for("claude-3-5-haiku-20241022").unwrap();
        assert!(!haiku3.supports_reasoning);
        assert_eq!(haiku3.max_output_tokens, 8_192);
    }

    #[test]
    fn vision_flag_tracks_model_family() {
        assert!(capabilities_for("gpt-4o").unwrap().supports_vision);
        assert!(!capabilities_for("o1-mini").unwrap().supports_vision);
        assert!(!capabilities_for("deepseek-chat").unwrap().supports_vision);
    }

    #[test]
    fn gemini_advertises_million_token_window() {
        assert_eq!(
            capabilities_for("gemini-2.5-pro").unwrap().context_window,
            1_048_576
        );
    }

    #[test]
    fn reasoning_models_flagged() {
        assert!(capabilities_for("o3-mini").unwrap().supports_reasoning);
        assert!(
            capabilities_for("deepseek-reasoner")
                .unwrap()
                .supports_reasoning
        );
        assert!(
            !capabilities_for("deepseek-chat")
                .unwrap()
                .supports_reasoning
        );
    }

    #[test]
    fn handles_vendor_tagged_and_dated_ids() {
        // Canonicalisation runs first, so a tagged + dated id resolves.
        let caps = capabilities_for("anthropic/claude-sonnet-4-6-20250520").unwrap();
        assert_eq!(caps.max_output_tokens, 64_000);
    }

    #[test]
    fn unknown_model_is_none() {
        assert!(capabilities_for("totally-made-up-model").is_none());
    }

    /// Prefix-shadow guard. Lookup takes the first declaration whose prefix
    /// matches, so a broad row above a specific one makes the specific row
    /// unreachable — it still compiles and still reads correctly, it just never
    /// runs. Table order is load-bearing and nothing else enforces it.
    #[test]
    fn no_capability_row_is_shadowed_by_an_earlier_broader_prefix() {
        for (i, (later, _)) in CAPABILITY_TABLE.iter().enumerate() {
            for (earlier, _) in &CAPABILITY_TABLE[..i] {
                assert!(
                    !later.starts_with(earlier),
                    "{later:?} is unreachable — the earlier {earlier:?} row already \
                     prefix-matches it. Move the specific row above the broad one."
                );
            }
        }
    }

    /// Hosted and aggregated ids must reach the same rows their vendor-native
    /// form does. Every scheme below used to miss the table entirely and fall
    /// back to `CONSERVATIVE_CONTEXT_WINDOW`, which over-compresses a
    /// large-window model.
    #[test]
    fn hosted_and_aggregated_ids_resolve_to_their_family() {
        for id in [
            "deepseek-ai/DeepSeek-V3",
            "accounts/fireworks/models/llama-v3p3-70b-instruct",
            "@cf/meta/llama-3.3-70b-instruct-fp8-fast",
            "llama3.3:70b",
            "anthropic.claude-sonnet-5",
            "openai/gpt-5.6",
        ] {
            assert!(
                capabilities_for(id).is_some(),
                "{id} should resolve to a capability row"
            );
        }
    }

    /// StepFun states the window in the id; without these rows `step-1-8k`
    /// inherited the 128K conservative default — planning for 16x the room the
    /// model actually has, the one direction where a missing row is dangerous
    /// rather than merely pessimistic.
    #[test]
    fn stepfun_windows_follow_the_id() {
        assert_eq!(capabilities_for("step-1-8k").unwrap().context_window, 8_192);
        assert_eq!(
            capabilities_for("step-1-32k").unwrap().context_window,
            32_768
        );
        assert_eq!(
            capabilities_for("step-1-256k").unwrap().context_window,
            262_144
        );
    }

    #[test]
    fn current_generation_anthropic_resolves() {
        // Fable 5 + Opus 4.6+ carry the 1M window / 128K output; the broad
        // claude-opus-4 fallback must not shadow them.
        let fable = capabilities_for("claude-fable-5").unwrap();
        assert_eq!(fable.context_window, 1_000_000);
        assert_eq!(fable.max_output_tokens, 128_000);
        assert!(fable.supports_reasoning);

        let opus48 = capabilities_for("claude-opus-4-8").unwrap();
        assert_eq!(opus48.context_window, 1_000_000);
        assert_eq!(opus48.max_output_tokens, 128_000);

        // 4.0/4.1-era ids keep the legacy family figures.
        let opus41 = capabilities_for("claude-opus-4-1-20250805").unwrap();
        assert_eq!(opus41.context_window, 200_000);

        let sonnet46 = capabilities_for("claude-sonnet-4-6").unwrap();
        assert_eq!(sonnet46.context_window, 1_000_000);
        assert_eq!(sonnet46.max_output_tokens, 64_000);

        let haiku45 = capabilities_for("claude-haiku-4-5-20251001").unwrap();
        assert_eq!(haiku45.max_output_tokens, 64_000);
    }

    #[test]
    fn current_generation_cross_vendor_resolves() {
        let gpt55 = capabilities_for("gpt-5.5").unwrap();
        assert_eq!(gpt55.context_window, 1_000_000);

        let gpt54mini = capabilities_for("gpt-5.4-mini").unwrap();
        assert_eq!(gpt54mini.context_window, 400_000);

        assert!(capabilities_for("o4-mini").unwrap().supports_reasoning);
        assert!(
            capabilities_for("gemini-3-flash-preview")
                .unwrap()
                .supports_reasoning
        );
        assert_eq!(
            capabilities_for("grok-4-fast").unwrap().context_window,
            2_000_000
        );
        assert_eq!(capabilities_for("grok-4").unwrap().context_window, 256_000);
        assert!(
            capabilities_for("deepseek-v4-flash")
                .unwrap()
                .supports_reasoning
        );
        assert_eq!(
            capabilities_for("glm-5.1").unwrap().max_output_tokens,
            128_000
        );
    }

    #[test]
    fn minimax_family_resolves() {
        // Shipped `minimax` preset now has metadata so context budgeting can
        // size its window without per-provider config. M2.x chat is text-only
        // (vision lives in the separate VL line); the specific m2/m3 prefixes
        // must precede the broad `minimax` fallback.
        let m25 = capabilities_for("MiniMax-M2.5").unwrap();
        assert_eq!(m25.context_window, 204_800);
        assert!(m25.supports_reasoning);
        assert!(!m25.supports_vision);
        assert_eq!(
            capabilities_for("MiniMax-M3").unwrap().context_window,
            1_000_000
        );
    }

    #[test]
    fn moonshot_zhipu_alibaba_families_resolve() {
        // Newly added families so model-aware context budgeting can size the
        // compaction window without per-provider config.
        assert_eq!(capabilities_for("kimi-k2").unwrap().context_window, 262_144);
        assert_eq!(
            capabilities_for("moonshot-v1-128k").unwrap().context_window,
            131_072
        );
        assert_eq!(capabilities_for("glm-4.6").unwrap().context_window, 200_000);
        assert_eq!(
            capabilities_for("qwen-max").unwrap().context_window,
            131_072
        );
    }

    /// K3 arrives under two id shapes and both must size at 1M. The failure
    /// this guards is silent: `kimi-k3` starts with `kimi`, so without its own
    /// row it lands on the broad 200K row and the context budget compacts a
    /// 1M-window model at a fifth of its capacity.
    #[test]
    fn kimi_k3_sizes_at_one_million_under_both_id_shapes() {
        // Open platform (api.moonshot.ai).
        assert_eq!(
            capabilities_for("kimi-k3").unwrap().context_window,
            1_048_576
        );
        // Kimi Code subscription endpoint (api.kimi.com/coding).
        assert_eq!(capabilities_for("k3").unwrap().context_window, 1_048_576);
        // Both are multimodal reasoning models.
        let k3 = capabilities_for("k3").unwrap();
        assert!(k3.supports_vision && k3.supports_tools && k3.supports_reasoning);
    }

    /// `k3` is a prefix of `k3-256k`; declaration order is the only thing
    /// keeping the 256K variant from being sized at 1M.
    #[test]
    fn k3_256k_wins_over_the_bare_k3_prefix() {
        assert_eq!(
            capabilities_for("k3-256k").unwrap().context_window,
            262_144,
            "k3-256k must precede k3 in the table"
        );
        assert_eq!(resolve_context_window("K3-256K"), 262_144);
    }

    /// The highspeed K2.7 variant is the same model served faster — it must
    /// inherit the 256K `kimi-for-coding` row, not the broad 200K `kimi` row.
    #[test]
    fn kimi_for_coding_highspeed_inherits_the_coding_row() {
        assert_eq!(
            capabilities_for("kimi-for-coding-highspeed")
                .unwrap()
                .context_window,
            262_144
        );
    }

    #[test]
    fn llama_family_resolves_across_hosting_presets() {
        // Vendor-tagged ids (HuggingFace/DeepInfra/Hyperbolic) peel `meta-llama/`.
        assert_eq!(
            capabilities_for("meta-llama/Llama-3.3-70B-Instruct")
                .unwrap()
                .context_window,
            131_072
        );
        // Groq's `-versatile` suffix and NVIDIA's `meta/` tag both resolve.
        assert_eq!(
            capabilities_for("llama-3.3-70b-versatile")
                .unwrap()
                .context_window,
            131_072
        );
        assert_eq!(
            capabilities_for("meta/llama-3.3-70b-instruct")
                .unwrap()
                .context_window,
            131_072
        );
        // Original Llama-3 keeps its 8K window — the broad `llama` fallback and
        // the `llama-3.3` entry must not shadow it in either direction.
        assert_eq!(
            capabilities_for("llama-3-70b-instruct")
                .unwrap()
                .context_window,
            8_192
        );
        // Dotless hosted id (ollama) falls through to the broad entry.
        assert_eq!(
            capabilities_for("llama3.3:70b").unwrap().context_window,
            131_072
        );
        // Llama 4 is multimodal; 3.x text models are not.
        assert!(
            capabilities_for("llama-4-maverick")
                .unwrap()
                .supports_vision
        );
        assert!(!capabilities_for("llama-3.3-70b").unwrap().supports_vision);
    }

    #[test]
    fn cohere_and_perplexity_resolve() {
        // Command-A (256K) must precede the broad `command` (128K).
        assert_eq!(
            capabilities_for("command-a-03-2025")
                .unwrap()
                .context_window,
            256_000
        );
        assert_eq!(
            capabilities_for("command-r-plus").unwrap().context_window,
            128_000
        );
        // sonar-reasoning* exposes CoT; plain sonar does not.
        assert!(
            capabilities_for("sonar-reasoning-pro")
                .unwrap()
                .supports_reasoning
        );
        assert!(!capabilities_for("sonar").unwrap().supports_reasoning);
        assert_eq!(
            capabilities_for("sonar-pro").unwrap().context_window,
            200_000
        );
    }

    #[test]
    fn resolve_context_window_uses_catalog_for_known_models() {
        // claude-opus-4-8 is an exact prefix in the catalog (context_window =
        // 1_000_000, see the "claude-opus-4-8" row in CAPABILITY_TABLE).
        assert_eq!(resolve_context_window("claude-opus-4-8"), 1_000_000);
        assert_ne!(
            resolve_context_window("claude-opus-4-8"),
            CONSERVATIVE_CONTEXT_WINDOW,
            "known model must not hit the fallback"
        );
    }

    #[test]
    fn resolve_context_window_falls_back_for_unknown_models() {
        assert_eq!(
            resolve_context_window("totally-unknown-model"),
            CONSERVATIVE_CONTEXT_WINDOW
        );
        assert_eq!(resolve_context_window(""), CONSERVATIVE_CONTEXT_WINDOW);
    }

    #[test]
    fn override_wins_over_catalog_and_falls_back_when_absent() {
        // Explicit config override takes precedence over the catalog value.
        assert_eq!(
            resolve_context_window_with_override(Some(300_000), "claude-opus-4-8"),
            300_000
        );
        // No override → identical to the catalog lookup.
        assert_eq!(
            resolve_context_window_with_override(None, "claude-opus-4-8"),
            resolve_context_window("claude-opus-4-8")
        );
        // Override on an unknown model still wins (lets users window a custom id).
        assert_eq!(
            resolve_context_window_with_override(Some(64_000), "totally-unknown-model"),
            64_000
        );
        // A zero override is treated as "unset" so a mis-declared 0 can't peg
        // the gauge denominator at 1-token / 100%.
        assert_eq!(
            resolve_context_window_with_override(Some(0), "claude-opus-4-8"),
            resolve_context_window("claude-opus-4-8")
        );
    }

    /// The user's real config: `Kimi-K2.7` must resolve to the 256K K2 window
    /// via the `kimi-k2` prefix (NOT the generic `kimi`=200K row), so the gauge
    /// percentage is honest without needing any override.
    #[test]
    fn kimi_k2_7_resolves_to_256k_window() {
        assert_eq!(resolve_context_window("Kimi-K2.7"), 262_144);
        assert_eq!(resolve_context_window("kimi-k2.7"), 262_144);
        // `kimi-k2.7-code` (open platform) shares the K2 row.
        assert_eq!(resolve_context_window("kimi-k2.7-code"), 262_144);
    }

    #[test]
    fn refreshed_2026_defaults_resolve_windows() {
        // Registry defaults advanced this round must size their windows via the
        // catalog (not the 128K conservative fallback) so the gauge + compaction
        // budget are honest without per-provider config.
        assert_eq!(resolve_context_window("claude-sonnet-5"), 1_000_000);
        assert_eq!(resolve_context_window("gpt-5.6"), 1_050_000);
        assert_eq!(resolve_context_window("GLM-5.2"), 1_000_000);
        assert_eq!(resolve_context_window("MiniMax-M3"), 1_000_000);
        assert_eq!(resolve_context_window("qwen3-max-2026-01-23"), 262_144);
        // DeepSeek V4 window corrected 128K -> 1M (chat/reasoner aliases too).
        assert_eq!(resolve_context_window("deepseek-v4-flash"), 1_000_000);
        assert_eq!(resolve_context_window("deepseek-chat"), 1_000_000);
        assert_eq!(resolve_context_window("deepseek-reasoner"), 1_000_000);
    }

    #[test]
    fn doubao_family_resolves_after_wiring() {
        // Doubao had ZERO capability rows -> every id fell to the 128K default.
        // The new default and the Seed line now resolve to the real 256K window.
        assert_eq!(resolve_context_window("doubao-seed-1-8-251228"), 256_000);
        assert_eq!(resolve_context_window("doubao-1.5-pro-256k"), 256_000);
        let seed = capabilities_for("doubao-seed-1-8-251228").unwrap();
        assert!(seed.supports_vision, "Doubao Seed is multimodal");
        assert!(seed.supports_tools);
    }

    #[test]
    fn minimax_m3_is_multimodal() {
        // Vision flag corrected false -> true: M3's Anthropic-compat endpoint
        // accepts image/video. The text-only M2.x family stays vision=false.
        assert!(capabilities_for("MiniMax-M3").unwrap().supports_vision);
        assert!(!capabilities_for("MiniMax-M2.7").unwrap().supports_vision);
    }

    #[test]
    fn glm_5_2_precedes_glm_5_prefix() {
        // glm-5.2 (1M) must win over the glm-5 (200K) prefix it starts with;
        // glm-5.1 still resolves to the 200K glm-5 row.
        assert_eq!(
            capabilities_for("glm-5.2").unwrap().context_window,
            1_000_000
        );
        assert_eq!(capabilities_for("glm-5.1").unwrap().context_window, 200_000);
    }
}
