//! Memory system probe integration tests.
//!
//! Validates RRF fusion, cross-encoder rerank, query expansion,
//! retrieval trace, tiered decay, access reinforcement, tier
//! promotion, and the reflection system.

mod memory_probe {
    pub mod mock_embedding;
    pub mod harness;
    pub mod extraction_classification;
    pub mod fusion_scoring;
    pub mod rerank;
    pub mod retrieval_trace;
    pub mod decay_promotion;
    pub mod end_to_end;
}
