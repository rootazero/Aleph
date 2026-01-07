//! Multi-turn conversation support.
//!
//! This module provides session management for persistent dialogues,
//! enabling users to continue conversations with AI through the Halo overlay.

pub mod session;
pub mod manager;

pub use session::{ConversationSession, ConversationTurn};
pub use manager::ConversationManager;
