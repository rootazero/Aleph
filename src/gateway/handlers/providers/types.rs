//! Provider handler types.
//!
//! # Where the shapes actually live
//!
//! Every request and response shape in the `providers.*` family is defined once
//! in [`aleph_protocol::providers`] and re-exported here. It has to be that way:
//! the family has four clients in four crates, and two of them (`aleph-cli`,
//! `aleph-tui`) are forbidden from depending on `alephcore`. When each side kept
//! its own copy they disagreed — `aleph providers add` sent a flat body the
//! handler could not parse and answered `INVALID_PARAMS` on every invocation
//! ever made, and the Panel's write DTO declared a single `model` where the
//! server declares a `models` ladder, so saving from the settings page silently
//! truncated a configured ladder to its first rung.
//!
//! Only genuinely local glue stays here: the conversions from `alephcore`'s own
//! probe / discovery types into the wire rows.

pub use aleph_protocol::providers::{
    AuthKind, CatalogEntry, CatalogParams, CatalogResult, CatalogView, CreateParams, DeleteParams,
    DiscoveryFailureKind, GetParams, ModelsRefreshParams, ModelsRefreshResult, ModelsRefreshRow,
    OAuthStatus, ProviderConfigJson, ProviderGetResult, ProviderHealthResult, ProviderHealthRow,
    ProviderInfo, ProviderListResult, RosterModel, SetDefaultParams, TestParams, TestResult,
    UpdateParams,
};

impl From<crate::providers::probe::ProbeOutcome> for TestResult {
    fn from(o: crate::providers::probe::ProbeOutcome) -> Self {
        Self {
            success: o.success,
            error: o
                .error
                .map(|error| crate::diagnostics::redact_secrets(&error)),
            latency_ms: o.latency_ms,
        }
    }
}

/// Classify a discovery failure for the wire.
///
/// The classifier already exists — [`crate::providers::model_catalog::DiscoveryError`]
/// distinguishes "this vendor publishes no catalogue endpoint" from "the request
/// timed out". Flattening it to `e.to_string()` threw that away at the last
/// step, leaving every client with one sentence for two situations that call
/// for opposite UI: offer manual entry forever, versus offer a retry.
#[must_use]
pub fn failure_kind(err: &crate::providers::model_catalog::DiscoveryError) -> DiscoveryFailureKind {
    use crate::providers::model_catalog::DiscoveryError as E;
    match err {
        E::Unsupported(_) => DiscoveryFailureKind::Unsupported,
        E::MissingCredential(_) => DiscoveryFailureKind::MissingCredential,
        E::Timeout { .. } => DiscoveryFailureKind::Timeout,
        E::Status { .. } => DiscoveryFailureKind::Status,
        E::Shape { .. } => DiscoveryFailureKind::Shape,
        E::Transport { .. } => DiscoveryFailureKind::Transport,
    }
}
