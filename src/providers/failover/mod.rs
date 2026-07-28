//! Model failover provider — automatic provider/model switching.
//!
//! [`FailoverProvider`] is an [`AiProvider`] *decorator*. It wraps an ordered
//! chain — the live default provider plus a static list of fallbacks, each
//! provider expanded across its configured model list — and transparently
//! walks the chain when a call fails. The harness loop sees a single
//! `Arc<dyn AiProvider>` and never learns failover happened: Redline R10 (the
//! dumb loop performs no error-recovery strategy selection) holds because all
//! of that lives here, in the provider layer.
//!
//! # Failure handling
//!
//! Each failure is classified by [`llm_retry`](crate::providers::llm_retry) —
//! the shared error classifier — into one [`Decision`]:
//!
//! - **transient** (network blip, 529 overloaded) → retried in place a few
//!   times with backoff, then treated as a provider-level failure;
//! - **provider-level** (rate limit, auth, exhausted transient) → the
//!   provider's circuit breaker trips and the walk advances to the next
//!   provider;
//! - **model-level** (404 model not found) → the walk advances to the next
//!   model of the *same* provider;
//! - **fatal** (400 bad request) → returned immediately — switching provider
//!   cannot fix a malformed request;
//! - **context overflow** (413) → returned immediately, since the harness
//!   context-compactor owns that recovery path.
//!
//! # Circuit breaker
//!
//! Per-provider health is a three-state breaker (`Closed → Open → HalfOpen`)
//! keyed by provider name in a [`FailoverHealth`] map. That map is shared
//! (via `Arc`) across the global chain and every per-agent chain, so one
//! provider's outage is visible everywhere.

use std::time::Duration;

use crate::providers::route_policy::EndpointTier;
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;

mod decision;
mod health;
mod provider;

#[cfg(test)]
mod tests;

/// The walk's per-failure verdict, exported so protocol adapters can assert
/// what the chain will actually *do* with an error they construct — rather
/// than re-deriving the rule and drifting from it. Test-only: production has
/// exactly one caller and it lives inside this module.
#[cfg(test)]
pub(crate) use decision::{decide, Decision};
pub use health::{
    CircuitState, FailoverHealth, ModelCooldown, ProviderCooldown, ProviderHealthView,
};
pub(crate) use provider::effective_fallback_names;
pub use provider::{FailoverProvider, RouteStep};

/// Name of the sentinel node that wraps a whole *nested* chain rather than a
/// real endpoint.
///
/// A per-`provider_hint` override chain is `[pinned provider, <the entire global
/// chain>]`; the second slot needs a name, and it must not collide with a real
/// provider (names come from `[providers]` toml keys, which cannot start with
/// `__`). It is not an endpoint, so it takes part in nothing that describes one:
/// no load accounting (its in-flight count and latency EWMA would be the *sum*
/// of the nested dials it delegates to, and its name would surface in
/// `route_status` as a provider the operator never configured), and no health
/// sidelining (the nested chain runs its own breaker per real provider).
pub const NESTED_CHAIN_NODE: &str = "__global_chain__";

/// Consecutive failures at which a provider's circuit breaker opens.
const CIRCUIT_OPEN_THRESHOLD: u32 = 3;
/// Hard ceiling on the circuit-breaker cooldown.
const MAX_COOLDOWN: Duration = Duration::from_secs(600);
/// Backoff used for a bare transient error whose message carried no delay hint.
const DEFAULT_TRANSIENT_DELAY: Duration = Duration::from_millis(300);
/// Ceiling on a single in-place retry wait, applied on top of
/// `llm_retry::backoff_delay`'s exponential growth.
const MAX_RETRY_DELAY: Duration = Duration::from_secs(30);
/// Ceiling on a single in-place wait while *riding out a transient server
/// overload* (429 "please wait a moment", 529). Higher than [`MAX_RETRY_DELAY`]
/// so a large server `Retry-After` is honored in full rather than silently
/// clamped to 30s — a paid primary (e.g. Kimi) that says "wait 60s" should be
/// waited out on, not abandoned to a fallback. Also caps the proactive
/// per-provider cooldown wait. Matches hermes' 120s `Retry-After` cap.
const MAX_OVERLOAD_RETRY_DELAY: Duration = Duration::from_secs(120);
/// In-place retry budget for a *transient server overload* — a 429 whose body
/// says "please wait a moment and try again", or a 529 `overloaded`. In a
/// single-provider setup there is no sibling to fail over to, but endlessly
/// retrying a provider that is consistently overloaded makes the UI appear to
/// hang. One retry gives the server a brief moment to recover (~2s) without
/// abandoning the request entirely; if it still fails we surface the error
/// immediately. Operators who need deeper ride-out for scheduled jobs can
/// configure `[stability] turn_timeout_secs` and/or a secondary provider.
/// A genuine account/quota 429 stays `Fatal` upstream and never reaches this
/// budget.
const OVERLOAD_RETRY_BUDGET: u32 = 1;
/// Default sideline window for a *model-specific* 429 when the server gave no
/// `Retry-After` hint. Short enough that a transient per-model throttle clears
/// quickly; a longer server hint wins (capped by [`MAX_COOLDOWN`]).
const DEFAULT_MODEL_COOLDOWN: Duration = Duration::from_secs(60);

// =============================================================================
// Configuration
// =============================================================================

/// Failover tuning knobs.
///
/// Internal — *not* a TOML type. The operator-facing surface is
/// `[fallback_provider].chain`; see `config::types::phase6_wiring`.
#[derive(Debug, Clone)]
pub struct FailoverConfig {
    /// Same-candidate retries on a transient error before the chain advances.
    pub max_retries: u32,
    /// Initial circuit-breaker cooldown. Doubles on each `HalfOpen` probe
    /// failure, capped at [`MAX_COOLDOWN`].
    pub unhealthy_cooldown: Duration,
}

impl Default for FailoverConfig {
    fn default() -> Self {
        Self {
            max_retries: 2,
            unhealthy_cooldown: Duration::from_secs(300),
        }
    }
}

/// One provider in the failover chain, with the models to try in order.
#[derive(Clone)]
pub struct FailoverNode {
    /// Provider name — the circuit-breaker key.
    pub name: String,
    /// Models to attempt, in order. Empty → a single attempt that lets the
    /// provider pick its own configured default model.
    pub models: Vec<String>,
    /// The underlying provider implementation.
    pub provider: Arc<dyn AiProvider>,
    /// Endpoint locality tier, used by the route policy. Defaults to
    /// [`EndpointTier::Unknown`] so existing literals stay valid and the node
    /// is treated as the operator's configured default (always allowed).
    pub tier: EndpointTier,
}

impl FailoverNode {
    /// Construct a node with an explicit tier.
    pub fn with_tier(
        name: String,
        models: Vec<String>,
        provider: Arc<dyn AiProvider>,
        tier: EndpointTier,
    ) -> Self {
        Self {
            name,
            models,
            provider,
            tier,
        }
    }
}
