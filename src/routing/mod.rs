//! Routing module
//!
//! Channel-aware session key, identity links, hierarchical route resolution,
//! and task classification value types (consumed by the `gateway_route` tool).

pub mod config;
pub mod identity_links;
pub mod resolve;
pub mod rules;
pub mod session_key;
pub mod task_router;

pub use config::{MatchRule, PeerMatchConfig, RouteBinding, SessionConfig};
pub use resolve::{resolve_route, MatchedBy, ResolvedRoute, RouteInput, RoutePeer, RoutePeerKind};
pub use rules::{RoutingPatternsConfig, RoutingRules};
pub use session_key::{
    normalize_agent_id, DmScope, PeerKind, SessionKey, DEFAULT_AGENT_ID, DEFAULT_MAIN_KEY,
};
pub use task_router::{CollabStrategy, ManifestHints, TaskRoute};
