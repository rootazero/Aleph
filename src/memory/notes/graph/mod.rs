//! Note knowledge-graph intelligence: 4-signal relevance, Louvain community
//! detection, graph-health insights. Pure functions over an immutable
//! `GraphSnapshot` — zero storage coupling (P4). Consumed by the offline
//! `GraphRecomputeStage` (materialization) and `note_retrieval` (seed
//! expansion). No external graph crate (R3); concurrency via std threads.

pub mod community;
pub mod insights;
pub mod relevance;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};

/// One node in the note graph.
#[derive(Debug, Clone)]
pub struct GraphNode {
    pub path: String,    // "category/filename"
    pub category: String,
    pub sources: Vec<String>, // frontmatter `source_notes`
}

/// Immutable snapshot of the note graph, built once per recompute.
#[derive(Debug, Clone, Default)]
pub struct GraphSnapshot {
    pub nodes: Vec<GraphNode>,
    /// Directed resolved edges (`category/filename` pairs); wikilinks + typed
    /// relations both live in `notes_links`.
    pub edges: Vec<(String, String)>,
}

/// Derived adjacency + lookup, shared by all three algorithms. Built once.
pub struct GraphIndex<'a> {
    pub nodes: &'a [GraphNode],
    idx_of: HashMap<&'a str, usize>,
    /// Undirected, deduped adjacency by node index.
    pub adj: Vec<HashSet<usize>>,
    /// Source-set per node index (from `source_notes`).
    pub sources: Vec<HashSet<&'a str>>,
}

impl<'a> GraphIndex<'a> {
    #[must_use]
    pub fn build(snap: &'a GraphSnapshot) -> Self {
        let mut idx_of = HashMap::with_capacity(snap.nodes.len());
        for (i, n) in snap.nodes.iter().enumerate() {
            idx_of.insert(n.path.as_str(), i);
        }
        let mut adj = vec![HashSet::new(); snap.nodes.len()];
        for (from, to) in &snap.edges {
            if let (Some(&a), Some(&b)) = (idx_of.get(from.as_str()), idx_of.get(to.as_str())) {
                if a != b {
                    adj[a].insert(b);
                    adj[b].insert(a);
                }
            }
        }
        let sources = snap.nodes.iter()
            .map(|n| n.sources.iter().map(String::as_str).collect::<HashSet<_>>())
            .collect();
        Self { nodes: snap.nodes, idx_of, adj, sources }
    }

    #[must_use] pub fn len(&self) -> usize { self.nodes.len() }
    #[must_use] pub fn is_empty(&self) -> bool { self.nodes.is_empty() }
    #[must_use] pub fn degree(&self, i: usize) -> usize { self.adj[i].len() }
    #[must_use] pub fn index_of(&self, path: &str) -> Option<usize> { self.idx_of.get(path).copied() }
}
