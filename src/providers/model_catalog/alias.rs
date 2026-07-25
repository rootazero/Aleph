//! Centralised model-id canonicalisation + vendor inference.
//!
//! Single source of truth for three operations that were previously
//! duplicated across the codebase:
//!   1. [`canonicalize_model_id`] — strip a leading vendor tag (`anthropic/`)
//!      and a trailing `YYYYMMDD` date stamp, lower-casing for stable lookup.
//!      (was `pricing::canonicalize_model`)
//!   2. [`infer_vendor`] — map a *bare model name* to its canonical vendor
//!      slug (`claude-sonnet-4` → `anthropic`). (was
//!      `presets::resolve_provider_from_model`, here enriched from 4 vendors
//!      to a hermes-parity prefix table)
//!   3. [`canonical_provider_id`] — normalise a *provider alias* to its
//!      canonical vendor slug (`claude` → `anthropic`). (was
//!      `pricing::canonical_provider`)
//!
//! Mirrors hermes-agent `model_normalize._VENDOR_PREFIXES` (20+ vendors) and
//! openclaw `provider-model-id-normalization`, mapped to Rust as static
//! prefix tables — zero allocation on the lookup path beyond the lowercase
//! copy, no network, no config.

/// Provider/vendor tags stripped from the front of a model id during
/// canonicalisation. Lowercased. Scanned in order; nested tags
/// (`x-ai/openai/…`) peel one layer per matching entry.
const VENDOR_TAGS: &[&str] = &[
    "anthropic/",
    // Bedrock-style dot tag ("anthropic.claude-sonnet-4-6") — stripping it
    // lets the capability catalog resolve Bedrock-hosted Claude ids, and (since
    // `pricing::lookup_rates` gained its vendor-inferred fallback) also lets
    // `amazon-bedrock` be priced at Anthropic's rates instead of `Unknown`.
    // The estimate is flagged `RateBasis::VendorInferred` so the Bedrock
    // margin stays visible rather than being passed off as a quote.
    "anthropic.",
    "openai/",
    "google/",
    "models/",
    "deepseek/",
    "x-ai/",
    "xai/",
    "mistralai/",
    "mistral/",
    "moonshotai/",
    "moonshot/",
    "qwen/",
    "meta-llama/",
    "meta/",
    "z-ai/",
    "zai/",
];

/// `(model-name prefix, canonical vendor slug)`. Scanned in declaration
/// order; the first prefix that matches the canonicalised id wins, so
/// more-specific prefixes must precede broader ones.
///
/// Vendor slugs are the conventional config provider names Aleph already
/// uses. The four historical [`infer_vendor`] outputs
/// (`anthropic`/`openai`/`google`/`deepseek`) are preserved exactly; the
/// rest extend coverage toward hermes' `_VENDOR_PREFIXES`.
const MODEL_VENDOR_PREFIXES: &[(&str, &str)] = &[
    ("claude", "anthropic"),
    ("gpt", "openai"),
    ("chatgpt", "openai"),
    ("o1", "openai"),
    ("o3", "openai"),
    ("o4", "openai"),
    ("gemini", "google"),
    ("gemma", "google"),
    ("deepseek", "deepseek"),
    ("grok", "xai"),
    ("mixtral", "mistral"),
    ("mistral", "mistral"),
    ("magistral", "mistral"),
    ("codestral", "mistral"),
    ("ministral", "mistral"),
    ("kimi", "moonshot"),
    ("moonshot", "moonshot"),
    ("qwen", "qwen"),
    ("qwq", "qwen"),
    ("glm", "zai"),
    ("llama", "meta"),
    // Providers Aleph ships a preset for but whose model families were absent
    // from this table — keeping the model-name path at parity with the
    // provider-alias path ([`canonical_provider_id`]) and the preset roster.
    ("minimax", "minimax"),
    ("doubao", "doubao"),
    ("command", "cohere"),
    ("sonar", "perplexity"),
    ("step", "stepfun"),
];

/// Strip a leading vendor tag and a trailing `YYYYMMDD` date stamp from a
/// model id, lower-casing for case-insensitive prefix matching.
///
/// ```text
/// "anthropic/Claude-Sonnet-4-6-20250520" -> "claude-sonnet-4-6"
/// "gpt-4o-2024-11-20"                     -> "gpt-4o-2024-11-20" (non-8-digit tail kept)
/// "deepseek-ai/DeepSeek-V3"               -> "deepseek-v3"       (org path collapsed)
/// "accounts/fireworks/models/llama-v3p3"  -> "llama-v3p3"        (host path collapsed)
/// "llama3.3:70b"                          -> "llama3.3"          (ollama size tag)
/// ```
///
/// The result is a **table-lookup key only** — pricing, capabilities and
/// [`infer_vendor`] all consume it, and none of them ever puts it back on the
/// wire. The outgoing request always carries the operator's raw model id, so
/// collapsing a host path here can never mis-address a provider.
#[must_use]
pub fn canonicalize_model_id(model: &str) -> String {
    let mut m = model.trim().to_ascii_lowercase();
    // Peel vendor tags until none match. A single in-order pass would miss
    // nested aggregator tags whose inner tag precedes the outer in the table
    // (e.g. "x-ai/openai/…" — "openai/" is scanned before "x-ai/"). Each peel
    // strictly shortens the string, so the loop terminates.
    loop {
        let before = m.len();
        for tag in VENDOR_TAGS {
            if let Some(rest) = m.strip_prefix(tag) {
                m = rest.to_string();
            }
        }
        if m.len() == before {
            break;
        }
    }
    // Collapse any remaining host/org path to its last segment. [`VENDOR_TAGS`]
    // only knows the tags aggregators had when it was written; hosts keep
    // inventing new shapes ("deepseek-ai/…", "accounts/fireworks/models/…",
    // "@cf/meta/…"), and every unlisted shape used to miss BOTH the capability
    // table (→ conservative 128K window → premature compression) and the price
    // table (→ `CostStatus::Unknown` → `u64::MAX` under `cost_aware`). The
    // trailing segment is the vendor's own model id in every hosting scheme we
    // ship a preset for, so this generalises the fixed table instead of
    // chasing it.
    if let Some(idx) = m.rfind('/') {
        m = m[idx + 1..].to_string();
    }
    // Drop an Ollama-style `:tag` (size / quantisation variant): "llama3.3:70b",
    // "qwen3:8b-q4_K_M". The tag picks a *weight file*, not a different model
    // family, so it must not defeat the family lookup.
    if let Some(idx) = m.find(':') {
        m.truncate(idx);
    }
    // Drop a trailing 8-digit date stamp (e.g. "-20250520"). Arbitrary
    // dash-separated dates with non-8-digit tails are left intact.
    if let Some(idx) = m.rfind('-') {
        let tail = &m[idx + 1..];
        if tail.len() == 8 && tail.bytes().all(|b| b.is_ascii_digit()) {
            m.truncate(idx);
        }
    }
    m
}

/// Infer the canonical vendor slug for a *bare model name*.
///
/// Returns `None` when no prefix matches — callers should treat that as
/// "unknown vendor" and fall back to their default. Superset of the legacy
/// 4-vendor `resolve_provider_from_model`; the original outputs are
/// byte-for-byte preserved.
#[must_use]
pub fn infer_vendor(model: &str) -> Option<&'static str> {
    let canon = canonicalize_model_id(model);
    MODEL_VENDOR_PREFIXES
        .iter()
        .find(|(prefix, _)| canon.starts_with(prefix))
        .map(|(_, vendor)| *vendor)
}

/// Normalise a *provider alias* to its canonical vendor slug.
///
/// Substring-based (a provider name may embed the vendor, e.g.
/// `vertex-anthropic`). Returns `None` for unrecognised providers.
#[must_use]
// rust-doctor-disable-next-line high-cyclomatic-complexity
pub fn canonical_provider_id(provider: &str) -> Option<&'static str> {
    let p = provider.trim().to_ascii_lowercase();
    // Vendor-native OpenAI-compatible endpoints (MiniMax / Moonshot ship both
    // an anthropic-protocol primary and an OpenAI-protocol secondary). The
    // `-openai` suffix here marks the wire PROTOCOL, not the billing vendor, so
    // these must short-circuit before the generic "openai" substring branch
    // below — which would otherwise shadow them and mis-price the run as OpenAI.
    match p.as_str() {
        "minimax-openai" => return Some("minimax"),
        "moonshot-openai" | "kimi-openai" => return Some("moonshot"),
        _ => {}
    }
    if p.contains("anthropic") || p.contains("claude") {
        Some("anthropic")
    } else if p.contains("openai")
        || p.contains("gpt")
        || p.contains("chatgpt")
        || p.starts_with("o1")
        || p.starts_with("o3")
    {
        Some("openai")
    } else if p.contains("google") || p.contains("gemini") || p.contains("vertex") {
        Some("google")
    } else if p.contains("deepseek") {
        Some("deepseek")
    } else if p.contains("grok") || p == "xai" || p == "x-ai" {
        Some("xai")
    } else if p.contains("mistral") {
        Some("mistral")
    } else if p.contains("moonshot") || p.contains("kimi") {
        Some("moonshot")
    } else if p.contains("qwen") || p.contains("dashscope") {
        Some("qwen")
    } else if p.contains("zhipu") || p.contains("glm") || p.contains("z-ai") || p.contains("zai") {
        Some("zai")
    } else if p.contains("minimax") {
        Some("minimax")
    } else if p.contains("doubao") || p.contains("volcengine") || p == "ark" {
        // Volcengine Ark serves the Doubao family. Match the preset name and
        // its aliases (`volcengine`, `ark`). `ark` is compared EXACTLY, never
        // as a substring — `spark` (iFlytek Spark) contains "ark" and must not be
        // misrouted here.
        Some("doubao")
    } else if p.contains("cohere") || p.contains("command") {
        Some("cohere")
    } else if p.contains("perplexity") || p.contains("sonar") {
        Some("perplexity")
    } else if p.contains("stepfun") || p.contains("step") {
        Some("stepfun")
    } else if p.contains("meta") || p.contains("llama") {
        // Parity with [`infer_vendor`]'s `llama -> meta` row. Open-weight
        // Llama is multi-hosted (Groq/Together/…), so the *provider* alias
        // rarely embeds "meta"; this exists so the two paths agree.
        Some("meta")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalize_strips_vendor_tag_and_date() {
        assert_eq!(
            canonicalize_model_id("anthropic/Claude-Sonnet-4-6-20250520"),
            "claude-sonnet-4-6"
        );
        assert_eq!(canonicalize_model_id("gpt-4o-20241120"), "gpt-4o");
        // Non-8-digit dash tail is left intact.
        assert_eq!(
            canonicalize_model_id("openai/gpt-4o-2024-11-20"),
            "gpt-4o-2024-11-20"
        );
    }

    #[test]
    fn canonicalize_peels_nested_aggregator_tags() {
        assert_eq!(canonicalize_model_id("x-ai/grok-4"), "grok-4");
    }

    #[test]
    fn infer_vendor_preserves_legacy_four() {
        // Exact parity with the old resolve_provider_from_model outputs.
        assert_eq!(infer_vendor("gpt-4o"), Some("openai"));
        assert_eq!(infer_vendor("o1-mini"), Some("openai"));
        assert_eq!(infer_vendor("o3-mini"), Some("openai"));
        assert_eq!(infer_vendor("claude-sonnet-4-6"), Some("anthropic"));
        assert_eq!(infer_vendor("gemini-2.5-pro"), Some("google"));
        assert_eq!(infer_vendor("deepseek-chat"), Some("deepseek"));
    }

    #[test]
    fn infer_vendor_extends_to_new_vendors() {
        assert_eq!(infer_vendor("grok-4"), Some("xai"));
        assert_eq!(infer_vendor("mistral-large-latest"), Some("mistral"));
        assert_eq!(infer_vendor("kimi-k2"), Some("moonshot"));
        assert_eq!(infer_vendor("qwen-2.5-72b"), Some("qwen"));
        assert_eq!(infer_vendor("glm-4.6"), Some("zai"));
        assert_eq!(infer_vendor("llama-3.3-70b"), Some("meta"));
    }

    #[test]
    fn infer_vendor_handles_vendor_tagged_input() {
        assert_eq!(infer_vendor("anthropic/claude-opus-4"), Some("anthropic"));
        assert_eq!(infer_vendor("x-ai/grok-4-fast"), Some("xai"));
    }

    #[test]
    fn infer_vendor_unknown_is_none() {
        assert_eq!(infer_vendor("some-unknown-model"), None);
    }

    #[test]
    fn canonical_provider_resolves_aliases() {
        assert_eq!(canonical_provider_id("claude"), Some("anthropic"));
        assert_eq!(canonical_provider_id("vertex-anthropic"), Some("anthropic"));
        assert_eq!(canonical_provider_id("OpenAI"), Some("openai"));
        assert_eq!(canonical_provider_id("gemini"), Some("google"));
        assert_eq!(canonical_provider_id("xai"), Some("xai"));
        assert_eq!(canonical_provider_id("nonexistent"), None);
    }

    #[test]
    fn canonical_provider_resolves_zai_and_dashscope() {
        assert_eq!(canonical_provider_id("zhipu"), Some("zai"));
        assert_eq!(canonical_provider_id("z-ai"), Some("zai"));
        assert_eq!(canonical_provider_id("zai"), Some("zai"));
        assert_eq!(canonical_provider_id("glm"), Some("zai"));
        assert_eq!(canonical_provider_id("dashscope"), Some("qwen"));
    }

    #[test]
    fn infer_vendor_covers_shipped_preset_families() {
        // Newly added rows: these providers all ship a built-in preset but their
        // model families were previously unrecognised by the model-name path.
        assert_eq!(infer_vendor("MiniMax-M2.5"), Some("minimax"));
        assert_eq!(infer_vendor("command-a-03-2025"), Some("cohere"));
        assert_eq!(infer_vendor("sonar-reasoning-pro"), Some("perplexity"));
        assert_eq!(infer_vendor("step-1-8k"), Some("stepfun"));
    }

    #[test]
    fn canonical_provider_at_parity_with_infer_vendor() {
        // The provider-alias path must recognise every vendor the model-name
        // path does — the two tables previously drifted (table had llama→meta,
        // the if/else chain did not; both lacked minimax/cohere/perplexity).
        assert_eq!(canonical_provider_id("minimax"), Some("minimax"));
        assert_eq!(canonical_provider_id("cohere"), Some("cohere"));
        assert_eq!(canonical_provider_id("perplexity"), Some("perplexity"));
        assert_eq!(canonical_provider_id("stepfun"), Some("stepfun"));
        assert_eq!(canonical_provider_id("groq-llama"), Some("meta"));
    }

    #[test]
    fn vendor_native_openai_secondaries_keep_their_billing_vendor() {
        // MiniMax / Moonshot ship an OpenAI-protocol secondary alongside their
        // anthropic primary. The "-openai" suffix is the wire protocol, not the
        // billing vendor — these must NOT be shadowed by the generic "openai"
        // branch (which would mis-price the run as OpenAI).
        assert_eq!(canonical_provider_id("minimax-openai"), Some("minimax"));
        assert_eq!(canonical_provider_id("moonshot-openai"), Some("moonshot"));
        assert_eq!(canonical_provider_id("kimi-openai"), Some("moonshot"));
        // Regression guard: genuine OpenAI / vertex-anthropic semantics intact.
        assert_eq!(canonical_provider_id("openai"), Some("openai"));
        assert_eq!(canonical_provider_id("vertex-anthropic"), Some("anthropic"));
    }

    #[test]
    fn doubao_wired_on_both_paths_without_spark_collision() {
        // Provider-alias path: preset name + aliases (volcengine, ark) -> doubao.
        assert_eq!(canonical_provider_id("doubao"), Some("doubao"));
        assert_eq!(canonical_provider_id("volcengine"), Some("doubao"));
        assert_eq!(canonical_provider_id("ark"), Some("doubao"));
        // `spark` (iFlytek Spark) contains the substring "ark" but must NOT be
        // misrouted to doubao — `ark` is matched exactly.
        assert_ne!(canonical_provider_id("spark"), Some("doubao"));
        // Model-name path: doubao ids -> doubao (parity with the alias path).
        assert_eq!(infer_vendor("doubao-seed-1-8-251228"), Some("doubao"));
        assert_eq!(infer_vendor("doubao-1.5-pro-256k"), Some("doubao"));
    }

    #[test]
    fn canonicalize_collapses_unlisted_host_paths() {
        // Hosting schemes absent from VENDOR_TAGS used to miss both lookup
        // tables entirely; the trailing segment is the vendor id in all of them.
        assert_eq!(
            canonicalize_model_id("deepseek-ai/DeepSeek-V3"),
            "deepseek-v3"
        );
        assert_eq!(
            canonicalize_model_id("accounts/fireworks/models/llama-v3p3-70b-instruct"),
            "llama-v3p3-70b-instruct"
        );
        assert_eq!(
            canonicalize_model_id("@cf/meta/llama-3.3-70b-instruct-fp8-fast"),
            "llama-3.3-70b-instruct-fp8-fast"
        );
        // Listed tags keep working (peeled before the collapse, same result).
        assert_eq!(canonicalize_model_id("openai/gpt-4o"), "gpt-4o");
    }

    #[test]
    fn canonicalize_strips_ollama_size_tag() {
        assert_eq!(canonicalize_model_id("llama3.3:70b"), "llama3.3");
        assert_eq!(canonicalize_model_id("qwen3:8b-q4_K_M"), "qwen3");
        // A colon-free id is untouched.
        assert_eq!(canonicalize_model_id("gpt-4o"), "gpt-4o");
    }

    #[test]
    fn infer_vendor_resolves_hosted_ids() {
        // The whole point of the collapse: hosted/aggregated ids now reach the
        // vendor table, which is what the pricing fallback keys on.
        assert_eq!(infer_vendor("deepseek-ai/DeepSeek-V3"), Some("deepseek"));
        assert_eq!(
            infer_vendor("accounts/fireworks/models/llama-v3p3-70b-instruct"),
            Some("meta")
        );
        assert_eq!(
            infer_vendor("@cf/meta/llama-3.3-70b-instruct-fp8-fast"),
            Some("meta")
        );
        assert_eq!(infer_vendor("llama3.3:70b"), Some("meta"));
    }

    #[test]
    fn canonicalize_strips_bedrock_dot_tag() {
        assert_eq!(
            canonicalize_model_id("anthropic.claude-sonnet-4-6"),
            "claude-sonnet-4-6"
        );
        assert_eq!(infer_vendor("anthropic.claude-opus-4-8"), Some("anthropic"));
    }
}
