//! Session management tools for cross-session communication.
//!
//! This module provides helper functions and tools for managing and interacting
//! with sessions. The main tools (sessions_list, sessions_send) will be added later.
//!
//! # Helper Functions
//!
//! - [`classify_session_kind`] - Classify a session key into its kind
//! - [`resolve_display_key`] - Format a session key for display
//! - [`parse_session_key`] - Parse a session key from its display format
//! - [`derive_channel`] - Extract the channel from a session key

pub mod helpers;

pub use helpers::{
    classify_session_kind, derive_channel, parse_session_key, resolve_display_key, SessionKind,
};
