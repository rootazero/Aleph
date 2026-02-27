//! Meta-Cognition layer — behavioral anchors learned from experience.
//! Migrated from core/src/memory/cortex/meta_cognition/.

pub mod anchor_retriever;
pub mod anchor_store;
pub mod tag_extractor;
pub mod types;

pub use anchor_retriever::AnchorRetriever;
pub use anchor_store::AnchorStore;
pub use tag_extractor::TagExtractor;
pub use types::{AnchorScope, AnchorSource, BehavioralAnchor};
