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

/// How the caller came to know about this model id.
///
/// Provenance is not derivable from the id, so the enumerator supplies it. It
/// matters to both audiences: an operator reading the picker wants to know
/// whether a row is something they configured or something Aleph suggested,
/// and the LLM choosing via `list_models` should be able to tell a curated
/// vendor fallback from a raw id scraped off a live `/models` endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelSource {
    /// The provider preset's `default_model`.
    PresetDefault,
    /// One of the preset's curated `fallback_models`.
    PresetFallback,
    /// The preset's cheap `default_aux_model`.
    PresetAux,
    /// Listed by the operator in `[providers.<id>] models`.
    Configured,
    /// Returned by the provider's live `/models` endpoint.
    Discovered,
}

impl ModelSource {
    /// Stable wire string for RPC / tool JSON.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PresetDefault => "preset_default",
            Self::PresetFallback => "preset_fallback",
            Self::PresetAux => "preset_aux",
            Self::Configured => "configured",
            Self::Discovered => "discovered",
        }
    }
}

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
            lifecycle: lifecycle_for(model),
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
        assert_eq!(cost.input_per_mtok, Some(3.0));
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
        assert_eq!(r.lifecycle.successor, Some("deepseek-v4-flash"));
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
