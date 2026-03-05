//! Routing module
//!
//! Channel-aware session key, identity links, hierarchical route resolution,
//! and task routing decision layer.

pub mod composite_router;
pub mod config;
pub mod identity_links;
pub mod llm_classifier;
pub mod resolve;
pub mod rules;
pub mod session_key;
pub mod task_router;

pub use composite_router::CompositeRouter;
pub use config::{MatchRule, PeerMatchConfig, RouteBinding, SessionConfig};
pub use resolve::{resolve_route, MatchedBy, ResolvedRoute, RouteInput, RoutePeer, RoutePeerKind};
pub use rules::{RoutingPatternsConfig, RoutingRules};
pub use session_key::{normalize_agent_id, DmScope, PeerKind, SessionKey, DEFAULT_AGENT_ID, DEFAULT_MAIN_KEY};
pub use task_router::{
    CollabStrategy, EscalationContext, EscalationSnapshot, ManifestHints, RouterContext, TaskRoute,
    TaskRouter,
};
