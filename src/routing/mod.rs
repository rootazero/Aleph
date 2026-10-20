//! Routing module
//!
//! Channel-aware session key, identity links, and hierarchical route
//! resolution (channel/peer → agent + session). Deterministic, config-driven
//! plumbing only — semantic intent classification is the LLM's job (R7), never
//! a regex layer here.

pub mod config;
pub mod identity_links;
pub mod overlay;
pub mod resolve;
pub mod session_key;

pub use config::{MatchRule, RouteBinding, SessionConfig};
pub use overlay::{overlay_route, OverlaySource, RuntimeOverlay};
pub use resolve::{resolve_route, ResolvedRoute, RouteInput, RoutePeer, RoutePeerKind};
pub use session_key::{normalize_agent_id, DmScope, PeerKind, SessionKey, DEFAULT_AGENT_ID};
