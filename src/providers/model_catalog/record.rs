//! The one place a `(provider, model)` pair becomes a fully-described model.
//!
//! Aleph keeps model reference data in four independent tables — capabilities
//! ([`super::capabilities`]), rates ([`crate::pricing`]), endpoint locality
//! ([`super::endpoint`]) and lifecycle ([`super::lifecycle`]). Every surface
//! that shows a model to a human or to the LLM needs all four, and until now
//! each one re-did the join by hand: `list_models`' `enrich` and the
//! `providers.catalog` RPC twice over (once for presets, once for custom
//! providers). Three hand-written joins is how a fourth table (lifecycle)
//! would have reached two of them and been forgotten in the third — silently,
//! since a missing field is not a compile error.
//!
//! (`route_observe::price_milli_per_mtok` and `failover::price_hint` read
//! [`crate::pricing`] alone and stay that way: they need one scalar to sort on,
//! not a record to display.)
//!
//! opencode solves the same problem by composing its catalog through ordered
//! plugins (`modelsDev → env → account → provider → config → discovery`), each
//! layer mutating one draft record. Aleph does not need a plugin bus for four
//! static tables, but it does need the invariant that layering buys:
//! **exactly one function knows how a model record is assembled.** Adding a
//! dimension is then one edit here, and every consumer gets it.
//!
//! R7 stance unchanged: this joins *data*. It ranks nothing, filters nothing,
//! and picks nothing.

use serde::Serialize;

use super::capabilities::{capabilities_for, ModelCapabilities};
use super::endpoint::{endpoint_kind_for_base_url, EndpointKind};
use super::lifecycle::{lifecycle_for, ModelLifecycle};
use crate::config::types::provider::{ModelCost, ModelDef, ModelInput};
use crate::pricing::{rate_card, RateBasis, RateCard};

// `ModelSource` is the wire's, not this module's: the picker, `list_models` and
// the roster all render it, and two of the crates that do cannot depend on
// `alephcore`. Provenance is not derivable from the id, so every enumerator
// supplies it — an operator wants to know whether a row is something they
// configured or something Aleph suggested, and a raw id scraped off a live
// `/models` endpoint has no curated window or price behind it.
pub use aleph_protocol::providers::ModelSource;

/// Everything the reference tables know about one `(provider, model)` pair.
///
/// `None` on [`capabilities`](Self::capabilities) / [`cost`](Self::cost) means
/// *not recorded* — never zero, never "unsupported". Consumers that must
/// choose a number for an unrecorded model do it explicitly (the context
/// budget falls back to [`CONSERVATIVE_CONTEXT_WINDOW`], cost-aware routing
/// sorts unknown cloud candidates last).
///
/// [`CONSERVATIVE_CONTEXT_WINDOW`]: super::capabilities::CONSERVATIVE_CONTEXT_WINDOW
#[derive(Debug, Clone, Serialize)]
pub struct ModelRecord {
    pub provider: String,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<ModelCapabilities>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<RateCard>,
    pub endpoint: EndpointKind,
    pub lifecycle: ModelLifecycle,
    pub source: ModelSource,
}

impl ModelRecord {
    /// Join the reference tables for one `(provider, model)` pair, then
    /// layer `model_def` (operator-declared, from [`ProviderConfig::model_defs`])
    /// on top as the highest-priority override.
    ///
    /// `base_url` is the provider's configured or preset endpoint; `None`
    /// classifies as [`EndpointKind::Cloud`], matching
    /// [`endpoint_kind_for_base_url`]'s existing contract.
    ///
    /// `model_def` is the operator's override for this model id. When `Some`,
    /// any field it sets wins over the catalog join — context window, max
    /// output tokens, vision / reasoning flags, and the rate card. Tiered
    /// pricing is intentionally not modelled here because [`RateCard`] has no
    /// tier slot; tiers are surfaced through cost-aware routing instead.
    #[must_use]
    pub fn resolve(
        provider: &str,
        model: &str,
        base_url: Option<&str>,
        source: ModelSource,
        model_def: Option<&ModelDef>,
    ) -> Self {
        let base_caps = capabilities_for(model);
        let base_cost = rate_card(provider, model);

        let capabilities = apply_cap_override(base_caps, model_def);
        let cost = merge_cost_override(base_cost, model_def.and_then(|d| d.cost.as_ref()));

        Self {
            provider: provider.to_string(),
            model: model.to_string(),
            capabilities,
            cost,
            endpoint: endpoint_kind_for_base_url(base_url),
            // Retirement is scoped: some rows are one host's word about its own
            // catalog, so the provider has to travel with the id.
            lifecycle: lifecycle_for(Some(provider), model),
            source,
        }
    }

    /// True when the reference tables have nothing on this model at all —
    /// neither a capability row nor a rate. Used by `select_model` to tell
    /// "unknown id, proceed with a caveat" apart from "known id".
    #[must_use]
    pub const fn is_unknown(&self) -> bool {
        self.capabilities.is_none() && self.cost.is_none()
    }
}

/// Apply operator overrides on top of a catalog-derived capability row.
///
/// `None` for any individual field means "don't touch". If the catalog had no
/// row but the operator named at least one field, we materialise a default
/// capability row so the operator's intent actually reaches the surface —
/// `supports_tools` stays `false` because no override exists for it, which
/// is the conservative choice for an unrecorded model.
fn apply_cap_override(
    base: Option<ModelCapabilities>,
    model_def: Option<&ModelDef>,
) -> Option<ModelCapabilities> {
    let Some(def) = model_def else {
        return base;
    };
    let has_any_cap = def.context_window.is_some()
        || def.max_output_tokens.is_some()
        || def.input.is_some()
        || def.reasoning.is_some();
    if !has_any_cap {
        return base;
    }
    let mut caps = base.unwrap_or(ModelCapabilities {
        context_window: 0,
        max_output_tokens: 0,
        supports_vision: false,
        supports_tools: false,
        supports_reasoning: false,
    });
    if let Some(window) = def.context_window {
        caps.context_window = window;
    }
    if let Some(max_output) = def.max_output_tokens {
        caps.max_output_tokens = max_output;
    }
    if let Some(input) = apply_input_override(&caps.supports_vision, def.input.as_ref()) {
        caps.supports_vision = input;
    }
    if let Some(reasoning) = def.reasoning {
        caps.supports_reasoning = reasoning;
    }
    Some(caps)
}

/// `ModelInput` is non-optional inside, but the outer `Option<ModelInput>` is.
/// When the operator omits the whole block, leave the catalog's view alone.
const fn apply_input_override(
    _current: &bool,
    input: Option<&ModelInput>,
) -> Option<bool> {
    match input {
        None => None,
        // `ModelInput`'s only capability flag is `image`; `text` is always
        // supported in the curated catalog and not modelled as a separate
        // flag here.
        Some(ModelInput { image, .. }) => Some(*image),
    }
}

/// Fold operator-declared rates into the catalog-derived [`RateCard`].
///
/// Field-level override semantics: each `Some` in `cost` replaces the
/// corresponding slot in the base card. When the operator names at least one
/// rate field, the basis becomes [`RateBasis::Direct`] — the operator's quote
/// is the authority, regardless of whether the catalog previously inferred a
/// price. Tiers are silently dropped because [`RateCard`] cannot represent
/// them; tier-aware routing reads them from [`crate::config::types::provider::ModelCost`]
/// directly, not through this surface.
fn merge_cost_override(base: Option<RateCard>, cost: Option<&ModelCost>) -> Option<RateCard> {
    let Some(cost) = cost else {
        return base;
    };
    let has_any_rate = cost.input.is_some()
        || cost.output.is_some()
        || cost.cache_read.is_some()
        || cost.cache_write.is_some()
        || cost.reasoning.is_some();
    if !has_any_rate {
        // Tiers-only overrides cannot be modelled in `RateCard`; leave the
        // existing basis intact so the vendor's catalog price still shows.
        return base;
    }
    let base = base.unwrap_or(RateCard {
        input_per_mtok: None,
        output_per_mtok: None,
        cache_read_per_mtok: None,
        cache_creation_per_mtok: None,
        reasoning_per_mtok: None,
        basis: RateBasis::Direct,
    });
    Some(RateCard {
        input_per_mtok: cost.input.or(base.input_per_mtok),
        output_per_mtok: cost.output.or(base.output_per_mtok),
        cache_read_per_mtok: cost.cache_read.or(base.cache_read_per_mtok),
        // `ModelCost::cache_write` maps to `RateCard::cache_creation_per_mtok` —
        // the protocol field names the rate, not the operator field.
        cache_creation_per_mtok: cost.cache_write.or(base.cache_creation_per_mtok),
        reasoning_per_mtok: cost.reasoning.or(base.reasoning_per_mtok),
        basis: RateBasis::Direct,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pricing::RateBasis;

    #[test]
    fn joins_all_four_tables_for_a_flagship() {
        let r = ModelRecord::resolve(
            "anthropic",
            "claude-sonnet-5",
            Some("https://api.anthropic.com"),
            ModelSource::PresetDefault,
            None,
        );
        assert_eq!(r.capabilities.unwrap().context_window, 1_000_000);
        assert_eq!(r.cost.unwrap().basis, RateBasis::Direct);
        assert_eq!(r.endpoint, EndpointKind::Cloud);
        assert_eq!(r.lifecycle, ModelLifecycle::ACTIVE);
        assert!(!r.is_unknown());
    }

    #[test]
    fn aggregator_gets_vendor_inferred_cost_not_unknown() {
        // The regression this whole round exists for: an OpenRouter-served
        // Claude used to resolve with no cost at all.
        let r = ModelRecord::resolve(
            "openrouter",
            "anthropic/claude-sonnet-5",
            Some("https://openrouter.ai/api/v1"),
            ModelSource::Configured,
            None,
        );
        let cost = r.cost.expect("aggregator model must be priceable");
        assert_eq!(cost.basis, RateBasis::VendorInferred);
        assert_eq!(cost.input_per_mtok, Some(2.0));
        // Capabilities resolve through the same canonicalisation.
        assert_eq!(r.capabilities.unwrap().context_window, 1_000_000);
    }

    #[test]
    fn local_endpoint_is_classified_from_base_url() {
        let r = ModelRecord::resolve(
            "ollama",
            "llama3.3:70b",
            Some("http://localhost:11434"),
            ModelSource::Configured,
            None,
        );
        assert_eq!(r.endpoint, EndpointKind::Local);
    }

    #[test]
    fn deprecated_id_surfaces_through_the_record() {
        let r = ModelRecord::resolve(
            "deepseek",
            "deepseek-chat",
            None,
            ModelSource::Configured,
            None,
        );
        assert!(r.lifecycle.is_deprecated());
        assert_eq!(r.lifecycle.successor.as_deref(), Some("deepseek-v4-flash"));
    }

    /// End-of-pipe check for the 2026-08 refresh: a table row is only worth
    /// something if it arrives at the join point every surface reads. Each of
    /// these is a model Aleph now advertises and previously had nothing on.
    #[test]
    fn refreshed_flagships_resolve_completely() {
        for (provider, model, window) in [
            ("anthropic", "claude-opus-5", 1_000_000),
            ("moonshot", "kimi-k3", 1_048_576),
            ("openai", "gpt-5.6-terra", 1_050_000),
            ("cohere", "command-a-plus-05-2026", 128_000),
            ("qianfan", "ernie-5.1", 128_000),
            ("doubao", "doubao-seed-evolving", 1_024_000),
            ("stepfun", "step-3.7-flash", 256_000),
        ] {
            let r = ModelRecord::resolve(provider, model, None, ModelSource::PresetDefault, None);
            let caps = r
                .capabilities
                .unwrap_or_else(|| panic!("{provider}/{model} has no capability row"));
            assert_eq!(
                caps.context_window, window,
                "{provider}/{model} window drifted"
            );
            assert!(
                !r.lifecycle.is_deprecated(),
                "{provider}/{model} is advertised but recorded as retired"
            );
        }
    }

    /// End-of-pipe check for the 2026-09 round (vs pi.dev/models 2026-09
    /// snapshot). Each of these is a model Aleph now advertises via its
    /// `pi-mono` 2026-09 catalog refresh and previously sized incorrectly
    /// (either compressed to the conservative 128K fallback or to the
    /// broad-family row's stale window).
    #[test]
    fn pi_dev_2026_09_families_resolve_correctly() {
        for (provider, model, window) in [
            // Anthropic 3.7 Sonnet — the only reasoning 3.x model. Used to
            // land on `claude-3` (no reasoning, 8K output).
            ("anthropic", "claude-3-7-sonnet-20250219", 200_000),
            ("anthropic", "claude-3-7-sonnet-latest", 200_000),
            // OpenAI GPT-5.4 Pro — 1.05M (vs the 272K the 5.4 base row
            // reports). codex-mini-latest — new pi.dev id, text-only
            // reasoning at 200K/100K.
            ("openai", "gpt-5.4-pro", 1_050_000),
            ("openai", "codex-mini-latest", 200_000),
            // Google Gemini 2.5 Flash Lite — same shape as the regular Flash
            // (1M/64K) but a distinct prefix so the Lite id stays visible.
            ("gemini", "gemini-2.5-flash-lite", 1_048_576),
            // xAI Grok 3 / Grok 3 Mini — 131K, distinct from the broad 131K
            // / 16K + vision `grok` row. Grok Code Fast 1 — 256K / 10K
            // (not the broad 256K / 64K).
            ("xai", "grok-3", 131_072),
            ("xai", "grok-3-mini", 131_072),
            ("xai", "grok-code-fast-1", 256_000),
            // Mistral reasoning + coding tiers — previously fell to the
            // broad 128K / 8K / no-reasoning row.
            ("mistral", "magistral-small", 128_000),
            // devstral-medium-latest is recorded as superseded by
            // `mistral-medium-3-5` in `lifecycle.rs` and is therefore
            // deliberately excluded here.
            ("mistral", "codestral-latest", 256_000),
            ("mistral", "pixtral-large-latest", 128_000),
            ("mistral", "open-mixtral-8x22b", 64_000),
            ("mistral", "open-mistral-7b", 8_192),
            // MiniMax M2.x — the broad M2 row carried max_output = 16K (vs
            // 131K), reserving 8x less than the real cap. The M2.1 / M2.5 /
            // M2.5-highspeed specific rows pin the right shape.
            ("minimax", "MiniMax-M2.1", 204_800),
            ("minimax", "MiniMax-M2.5", 204_800),
            ("minimax", "MiniMax-M2.5-highspeed", 204_800),
            // ZAI GLM 4.x — distinct from the existing GLM-5.x family.
            ("zai", "glm-4.7", 200_000),
            ("zai", "glm-4.6v", 128_000),
            ("zai", "glm-4.5v", 64_000),
            ("zai", "glm-4.5-air", 131_072),
            // Kimi Coding subscription id `k2p5` — canonicalises to `k2.5` (the
            // Fireworks `p`-separator rewrite), but neither shape reaches
            // the `kimi-for-coding` row; the explicit `k2.5` prefix below
            // is what sizes it.
            ("kimi-for-coding", "k2p5", 262_144),
            // Qwen QwQ reasoning tier — distinct from the broad `qwen` row.
            ("qwen", "qwen-qwq-32b", 131_072),
            // Gemma 2 — 8K, used to land on the 1M `gemini` fallback.
            ("groq", "gemma2-9b-it", 8_192),
        ] {
            let r = ModelRecord::resolve(provider, model, None, ModelSource::PresetDefault, None);
            let caps = r
                .capabilities
                .unwrap_or_else(|| panic!("{provider}/{model} has no capability row"));
            assert_eq!(
                caps.context_window, window,
                "{provider}/{model} window drifted"
            );
            assert!(
                !r.lifecycle.is_deprecated(),
                "{provider}/{model} is advertised but recorded as retired"
            );
        }
    }

    /// Retirement scope, seen from the surface that consumes it rather than
    /// from the table. Groq dropped both Llama tiers; Together did not.
    #[test]
    fn host_scoped_retirement_reaches_the_record() {
        let on_groq = ModelRecord::resolve(
            "groq",
            "llama-3.3-70b-versatile",
            None,
            ModelSource::Configured,
            None,
        );
        assert!(on_groq.lifecycle.is_deprecated());
        assert_eq!(
            on_groq.lifecycle.successor.as_deref(),
            Some("openai/gpt-oss-120b")
        );

        let on_together = ModelRecord::resolve(
            "together",
            "meta-llama/Llama-3.3-70B-Instruct-Turbo",
            None,
            ModelSource::Configured,
            None,
        );
        assert!(!on_together.lifecycle.is_deprecated());
    }

    /// Fireworks' `p` separator is only fixed if it survives the whole join —
    /// both defaults used to arrive here with the wrong family's rate.
    #[test]
    fn fireworks_ids_join_against_their_own_rows() {
        let r = ModelRecord::resolve(
            "fireworks",
            "accounts/fireworks/models/kimi-k2p6",
            Some("https://api.fireworks.ai/inference/v1"),
            ModelSource::PresetDefault,
            None,
        );
        assert_eq!(r.capabilities.unwrap().context_window, 262_144);
        assert_eq!(r.cost.expect("k2p6 prices").input_per_mtok, Some(0.95));
    }

    #[test]
    fn wholly_unrecorded_model_is_flagged_unknown() {
        let r = ModelRecord::resolve(
            "my-relay",
            "internal-model-v7",
            Some("https://relay.example.com"),
            ModelSource::Configured,
            None,
        );
        assert!(r.is_unknown());
        // …but it still gets an endpoint and a lifecycle, so no consumer has
        // to special-case the unknown branch.
        assert_eq!(r.endpoint, EndpointKind::Cloud);
        assert_eq!(r.lifecycle, ModelLifecycle::ACTIVE);
    }

    /// Operator override layer, end-to-end: a `ModelDef` that names a
    /// capability and a rate must replace the catalog's view of both, even
    /// when the catalog already has a row. The union of "different context
    /// window" + "different cost" + "vision toggled" is the smallest shape
    /// that exercises every code path in `apply_cap_override` and
    /// `merge_cost_override` simultaneously.
    #[test]
    fn model_def_overrides_catalog_caps_and_cost() {
        let def = ModelDef {
            id: "claude-sonnet-5".into(),
            name: None,
            base_url: None,
            context_window: Some(512_000),
            max_output_tokens: Some(64_000),
            input: Some(crate::config::types::provider::ModelInput {
                text: true,
                image: false,
            }),
            reasoning: Some(false),
            thinking_level_map: None,
            cost: Some(crate::config::types::provider::ModelCost {
                input: Some(99.0),
                output: Some(199.0),
                cache_read: Some(9.0),
                cache_write: Some(19.0),
                reasoning: Some(299.0),
                tiers: Vec::new(),
            }),
            compat: None,
        };

        let r = ModelRecord::resolve(
            "anthropic",
            "claude-sonnet-5",
            Some("https://api.anthropic.com"),
            ModelSource::PresetDefault,
            Some(&def),
        );

        let caps = r.capabilities.expect("def named caps, row must exist");
        assert_eq!(caps.context_window, 512_000);
        assert_eq!(caps.max_output_tokens, 64_000);
        assert!(!caps.supports_vision, "ModelInput::image=false must clear vision");
        assert!(!caps.supports_reasoning, "def.reasoning=false must clear reasoning");
        // Catalog's own `supports_tools` must survive — the operator has no
        // field for it and the override layer does not zero it out.
        let bare = ModelRecord::resolve(
            "anthropic",
            "claude-sonnet-5",
            Some("https://api.anthropic.com"),
            ModelSource::PresetDefault,
            None,
        );
        assert_eq!(caps.supports_tools, bare.capabilities.unwrap().supports_tools);

        let cost = r.cost.expect("def named rates, card must exist");
        assert_eq!(cost.input_per_mtok, Some(99.0));
        assert_eq!(cost.output_per_mtok, Some(199.0));
        assert_eq!(cost.cache_read_per_mtok, Some(9.0));
        // `ModelCost::cache_write` lands on the protocol's
        // `cache_creation_per_mtok` slot.
        assert_eq!(cost.cache_creation_per_mtok, Some(19.0));
        assert_eq!(cost.reasoning_per_mtok, Some(299.0));
        // Operator's quote is the authority — basis must flip to Direct
        // regardless of what the catalog had.
        assert_eq!(cost.basis, RateBasis::Direct);
    }

    /// An operator can resurrect an entirely unrecorded model by naming it.
    /// Without `model_def`, `internal-model-v7` is unknown; with one, it
    /// carries a usable capability row.
    #[test]
    fn model_def_creates_caps_for_an_unknown_id() {
        let def = ModelDef {
            id: "internal-model-v7".into(),
            name: None,
            base_url: None,
            context_window: Some(32_000),
            max_output_tokens: Some(4_000),
            input: None,
            reasoning: None,
            thinking_level_map: None,
            cost: None,
            compat: None,
        };

        let r = ModelRecord::resolve(
            "my-relay",
            "internal-model-v7",
            Some("https://relay.example.com"),
            ModelSource::Configured,
            Some(&def),
        );

        assert!(!r.is_unknown());
        let caps = r.capabilities.unwrap();
        assert_eq!(caps.context_window, 32_000);
        assert_eq!(caps.max_output_tokens, 4_000);
        // No `supports_tools` named by the operator → conservative default.
        assert!(!caps.supports_tools);
    }

    /// A `ModelDef` that names only `cost.input` must override just that one
    /// slot, leave the catalog's other rates intact, and flip the basis to
    /// Direct (the operator is the authority for any field they touch).
    #[test]
    fn model_def_partial_cost_override_keeps_other_rates() {
        let def = ModelDef {
            id: "claude-sonnet-5".into(),
            name: None,
            base_url: None,
            context_window: None,
            max_output_tokens: None,
            input: None,
            reasoning: None,
            thinking_level_map: None,
            cost: Some(crate::config::types::provider::ModelCost {
                input: Some(7.5),
                output: None,
                cache_read: None,
                cache_write: None,
                reasoning: None,
                tiers: Vec::new(),
            }),
            compat: None,
        };

        let r = ModelRecord::resolve(
            "anthropic",
            "claude-sonnet-5",
            Some("https://api.anthropic.com"),
            ModelSource::PresetDefault,
            Some(&def),
        );
        let bare = ModelRecord::resolve(
            "anthropic",
            "claude-sonnet-5",
            Some("https://api.anthropic.com"),
            ModelSource::PresetDefault,
            None,
        );
        let cost = r.cost.unwrap();
        let base = bare.cost.unwrap();
        assert_eq!(cost.input_per_mtok, Some(7.5));
        assert_eq!(cost.output_per_mtok, base.output_per_mtok);
        assert_eq!(cost.basis, RateBasis::Direct);
    }
}
