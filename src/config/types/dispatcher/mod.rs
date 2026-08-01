//! Team dispatcher / broadcast / message-router configuration types.
//!
//! - `TeamDispatcherConfigToml`: `[team_dispatcher]`
//! - `TeamBroadcastConfigToml`: `[team_broadcast]`
//! - `TeamMessagesConfigToml`: `[team_messages]`

mod core;

// Re-export all types
pub use core::*;
