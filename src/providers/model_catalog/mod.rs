//! Per-model metadata catalog: alias/vendor normalisation + capabilities.
//!
//! This is the single source of truth for *model-level* (not provider-level,
//! not protocol-level) reference data:
//!
//! * [`alias`] — canonicalise a model id, infer its vendor, normalise a
//!   provider alias. Unifies logic that used to be duplicated in `pricing`
//!   and `presets`.
//! * [`capabilities`] — per-model context window / output cap / vision /
//!   tools / reasoning flags. Previously absent in Aleph.
//! * [`endpoint`] — classify a provider's `base_url` host as on-machine
//!   ([`EndpointKind::Local`]) or a public API ([`EndpointKind::Cloud`]).
//! * [`lifecycle`] — is the vendor still serving this id, or did it retire?
//! * [`discovery`] — ask a configured provider what it actually serves right
//!   now (`GET {base_url}/models`), cached on disk.
//! * [`record`] — **the single join point.** `ModelRecord::resolve` is the one
//!   function that assembles capability + cost + endpoint + lifecycle into one
//!   value; every surface that shows a model consumes it rather than re-doing
//!   the join.
//!
//! Cost rates live in [`crate::pricing`] (consumed by the orchestrator's
//! per-run cost estimate) and reuse this module's canonicalisation so all
//! concerns — alias, cost, capability, lifecycle — share one canonical model
//! id.
//!
//! Design stance (R7 LLM sovereignty): everything here is *data*. It lets
//! callers and the LLM reason about model selection; it never selects a
//! model on its own.

pub mod alias;
pub mod capabilities;
pub mod discovery;
pub mod endpoint;
pub mod lifecycle;
pub mod record;

#[cfg(test)]
mod drift_tests;

pub use alias::{canonical_provider_id, canonicalize_model_id, infer_vendor, prefix_matches};
pub use capabilities::{
    capabilities_for, resolve_context_window, resolve_context_window_with_override,
    ModelCapabilities, CONSERVATIVE_CONTEXT_WINDOW,
};
pub use discovery::{cached_models, refresh_models, DiscoveredModels, DiscoveryError};
pub use endpoint::{endpoint_kind_for_base_url, EndpointKind};
pub use lifecycle::{lifecycle_for, ModelLifecycle, ModelStatus};
pub use record::{ModelRecord, ModelSource};
