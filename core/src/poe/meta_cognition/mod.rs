//! Meta-Cognition layer — behavioral anchors learned from experience.
//! Migrated from core/src/memory/cortex/meta_cognition/.

pub mod anchor_retriever;
pub mod anchor_store;
pub mod reactive;
pub mod tag_extractor;
pub mod types;

pub use anchor_retriever::AnchorRetriever;
pub use anchor_store::AnchorStore;
pub use reactive::{
    FailureSignal, FailureSnapshot, LLMConfig, ReactiveReflector, ReflectionResult, RootCause,
};
pub use tag_extractor::TagExtractor;
pub use types::{AnchorScope, AnchorSource, BehavioralAnchor};
