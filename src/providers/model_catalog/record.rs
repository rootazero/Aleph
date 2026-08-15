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
use crate::pricing::{rate_card, RateCard};

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
    /// Join the reference tables for one `(provider, model)` pair.
    ///
    /// `base_url` is the provider's configured or preset endpoint; `None`
    /// classifies as [`EndpointKind::Cloud`], matching
    /// [`endpoint_kind_for_base_url`]'s existing contract.
    #[must_use]
    pub fn resolve(
        provider: &str,
        model: &str,
        base_url: Option<&str>,
        source: ModelSource,
    ) -> Self {
        Self {
            provider: provider.to_string(),
            model: model.to_string(),
            capabilities: capabilities_for(model),
            cost: rate_card(provider, model),
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
        );
        assert_eq!(r.endpoint, EndpointKind::Local);
    }

    #[test]
    fn deprecated_id_surfaces_through_the_record() {
        let r = ModelRecord::resolve("deepseek", "deepseek-chat", None, ModelSource::Configured);
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
            let r = ModelRecord::resolve(provider, model, None, ModelSource::PresetDefault);
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
        );
        assert!(r.is_unknown());
        // …but it still gets an endpoint and a lifecycle, so no consumer has
        // to special-case the unknown branch.
        assert_eq!(r.endpoint, EndpointKind::Cloud);
        assert_eq!(r.lifecycle, ModelLifecycle::ACTIVE);
    }
}
