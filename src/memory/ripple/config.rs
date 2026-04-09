//! Configuration and result types for RippleTask

use crate::memory::MemoryFact;

/// Configuration for ripple exploration
#[derive(Debug, Clone)]
pub struct RippleConfig {
    /// Maximum number of hops to explore (default: 2)
    pub max_hops: usize,

    /// Maximum facts to retrieve per hop (default: 5)
    pub max_facts_per_hop: usize,

    /// Similarity threshold for related facts (default: 0.7)
    pub similarity_threshold: f32,

    /// Enable cross-domain traversal via tunnel edges (default: true).
    pub enable_tunnels: bool,

    /// Maximum tunnel hops per ripple (default: 1).
    pub max_tunnel_hops: usize,
}

impl Default for RippleConfig {
    fn default() -> Self {
        Self {
            max_hops: 2,
            max_facts_per_hop: 5,
            similarity_threshold: 0.7,
            enable_tunnels: true,
            max_tunnel_hops: 1,
        }
    }
}

/// Result of ripple exploration
#[derive(Debug)]
pub struct RippleResult {
    /// Original seed facts that started the exploration
    pub seed_facts: Vec<MemoryFact>,

    /// Expanded facts discovered through graph traversal
    pub expanded_facts: Vec<MemoryFact>,

    /// Total number of hops performed
    pub total_hops: usize,

    /// Facts discovered via tunnel edges (cross-domain).
    pub tunnel_facts: Vec<MemoryFact>,
}
