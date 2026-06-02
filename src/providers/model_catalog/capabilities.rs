//! Per-model capability metadata.
//!
//! Aleph previously carried capability bits only at the *protocol/endpoint*
//! layer (`AnthropicCapabilities`, OpenAI provider policy) — i.e. "what does
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
/// Figures are best-effort reference data (vendor docs as of 2026-05),
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
    (
        "deepseek-reasoner",
        ModelCapabilities {
            context_window: 128_000,
            max_output_tokens: 8_192,
            supports_vision: false,
            supports_tools: true,
            supports_reasoning: true,
        },
    ),
    (
        "deepseek",
        ModelCapabilities {
            context_window: 128_000,
            max_output_tokens: 8_192,
            supports_vision: false,
            supports_tools: true,
            supports_reasoning: false,
        },
    ),
    // ── xAI ──────────────────────────────────────────────────────────────
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
];

/// Look up capability metadata for a model id (raw or canonical).
///
/// Returns `None` when no family prefix matches — callers treat that as
/// "capabilities unknown" and fall back to their own defaults.
pub fn capabilities_for(model: &str) -> Option<ModelCapabilities> {
    let canon = canonicalize_model_id(model);
    CAPABILITY_TABLE
        .iter()
        .find(|(prefix, _)| canon.starts_with(prefix))
        .map(|(_, caps)| *caps)
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
        assert!(capabilities_for("deepseek-reasoner")
            .unwrap()
            .supports_reasoning);
        assert!(!capabilities_for("deepseek-chat").unwrap().supports_reasoning);
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
}
