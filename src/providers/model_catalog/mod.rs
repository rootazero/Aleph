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
//!
//! Cost rates live in [`crate::pricing`] (consumed by the orchestrator's
//! per-run cost estimate) and reuse this module's canonicalisation so all
//! three concerns — alias, cost, capability — share one canonical model id.
//!
//! Design stance (R7 LLM sovereignty): everything here is *data*. It lets
//! callers and the LLM reason about model selection; it never selects a
//! model on its own.

pub mod alias;
pub mod capabilities;
pub mod endpoint;

pub use alias::{canonical_provider_id, canonicalize_model_id, infer_vendor};
pub use capabilities::{
    capabilities_for, resolve_context_window, ModelCapabilities, CONSERVATIVE_CONTEXT_WINDOW,
};
pub use endpoint::{endpoint_kind_for_base_url, EndpointKind};
