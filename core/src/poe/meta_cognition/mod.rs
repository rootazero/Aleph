//! Meta-Cognition layer — behavioral anchors learned from experience.
//! Migrated from core/src/memory/cortex/meta_cognition/.

pub mod anchor_store;
pub mod types;

pub use anchor_store::AnchorStore;
pub use types::{AnchorScope, AnchorSource, BehavioralAnchor};
