//! Meta-Cognition layer — behavioral anchors learned from experience.
//! Migrated from core/src/memory/cortex/meta_cognition/.

pub mod anchor_retriever;
pub mod anchor_store;
pub mod conflict_detector;
pub mod critic;
pub mod reactive;
pub mod tag_extractor;
pub mod types;

pub use anchor_retriever::AnchorRetriever;
pub use anchor_store::AnchorStore;
pub use conflict_detector::{ConflictDetector, ConflictReport, ConflictType};
pub use critic::{ChainAnalysis, CriticAgent, CriticReport, CriticScanConfig};
pub use reactive::{
    FailureSignal, FailureSnapshot, LLMConfig, ReactiveReflector, ReflectionResult, RootCause,
};
pub use tag_extractor::TagExtractor;
pub use types::{AnchorScope, AnchorSource, BehavioralAnchor};
