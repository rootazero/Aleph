//! Hybrid Memory Retrieval Module
//!
//! Provides hybrid search (vector + FTS5), layered retrieval strategies,
//! and dynamic association clustering.
//!
//! ## Architecture
//!
//! This module implements a two-stage retrieval approach:
//! 1. **Candidate Generation**: Fetch candidates using both vector similarity and FTS5 BM25
//! 2. **Score Fusion**: Combine scores with configurable weights (default: 70% vector, 30% text)
//!
//! ## Retrieval Strategies
//!
//! - `FactsOnly`: Fast mode - only search Layer 2 (facts)
//! - `FactsFirst`: Default mode - facts first, fallback to memories if insufficient
//! - `BothLayers`: Deep mode - search both layers simultaneously and merge results

pub mod association;
pub mod hybrid;
pub mod strategy;

pub use association::{AssociationCluster, AssociationConfig, AssociationRetriever};
pub use hybrid::{HybridRetrieval, HybridSearchConfig};
pub use strategy::RetrievalStrategy;
