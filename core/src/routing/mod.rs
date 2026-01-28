//! Routing module
//!
//! Channel-aware session key, identity links, and hierarchical route resolution.

pub mod config;
pub mod identity_links;
pub mod resolve;
pub mod session_key;

pub use config::{MatchRule, PeerMatchConfig, RouteBinding, SessionConfig};
pub use resolve::{resolve_route, MatchedBy, ResolvedRoute, RouteInput, RoutePeer, RoutePeerKind};
pub use session_key::{normalize_agent_id, DmScope, PeerKind, SessionKey, DEFAULT_AGENT_ID, DEFAULT_MAIN_KEY};
