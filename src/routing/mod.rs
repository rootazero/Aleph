//! Routing module
//!
//! Channel-aware session key, identity links, and hierarchical route
//! resolution (channel/peer → agent + session). Deterministic, config-driven
//! plumbing only — semantic intent classification is the LLM's job (R7), never
//! a regex layer here.

pub mod config;
pub mod experience_store;
pub mod identity_links;
pub mod observer;
pub mod resolve;
pub mod session_key;

pub use experience_store::{RoutingExperienceStore, RoutingOutcome};
pub use observer::{outcome_from_session_completed, OutcomeObserver};

pub use config::{MatchRule, PeerMatchConfig, RouteBinding, SessionConfig};
pub use resolve::{resolve_route, MatchedBy, ResolvedRoute, RouteInput, RoutePeer, RoutePeerKind};
pub use session_key::{
    normalize_agent_id, DmScope, PeerKind, SessionKey, DEFAULT_AGENT_ID, DEFAULT_MAIN_KEY,
};

/// Per-run handle correlating run-start recall (writes `task_emb`) with the
/// completion observer (reads it). One per run; lives in the gateway run loop,
/// outside the harness. `session_id` is read by the observer for trace logging.
///
/// Spec §6 types `task_emb` as `OnceCell`; we use `std::sync::OnceLock`
/// (std, no extra dep) — same write-once semantics. Flagged divergence.
pub struct RoutingAttribution {
    pub session_id: String,
    pub task_emb: std::sync::OnceLock<Vec<f32>>,
}

impl RoutingAttribution {
    #[must_use]
    pub fn new(session_id: String) -> Self {
        Self { session_id, task_emb: std::sync::OnceLock::new() }
    }
}

/// Frozen-model precedence for routing attribution — mirrors the subagent
/// spawn chain `explicit > model_hint > native` (subagent_spawner/mod.rs:297).
#[must_use]
pub fn resolve_routing_model_id(
    explicit: Option<&str>,
    model_hint: Option<&str>,
    native_default: &str,
) -> String {
    explicit.or(model_hint).unwrap_or(native_default).to_string()
}

#[cfg(test)]
mod model_precedence_tests {
    use super::resolve_routing_model_id;

    #[test]
    fn routing_model_precedence_explicit_then_hint_then_native() {
        assert_eq!(resolve_routing_model_id(Some("EXPLICIT"), Some("HINT"), "NATIVE"), "EXPLICIT");
        assert_eq!(resolve_routing_model_id(None, Some("HINT"), "NATIVE"), "HINT");
        assert_eq!(resolve_routing_model_id(None, None, "NATIVE"), "NATIVE");
    }
}
