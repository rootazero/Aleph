//! Note knowledge-graph intelligence: 4-signal relevance, Louvain community
//! detection, graph-health insights. Pure functions over an immutable
//! `GraphSnapshot` — zero storage coupling (P4). Consumed by the offline
//! `GraphRecomputeStage` (materialization) and `note_retrieval` (seed
//! expansion). No external graph crate (R3); concurrency via std threads.

pub mod community;
pub mod insights;
pub mod minhash;
pub mod relevance;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};

/// Relation label for behavioral co-recall edges in `notes_links`: notes
/// retrieved together by the same query event. Written by the
/// `co_recall_edges` dream stage; scored as a distinct relevance signal
/// (never as a semantic direct link).
pub const CO_RECALLED_RELATION: &str = "co_recalled";

/// One node in the note graph.
#[derive(Debug, Clone)]
pub struct GraphNode {
    pub path: String, // "category/filename"
    pub category: String,
    pub sources: Vec<String>, // frontmatter `source_notes`
}

/// A directed, typed, weighted edge in the note graph.
#[derive(Debug, Clone)]
pub struct GraphEdge {
    pub from: String,
    pub to: String,
    /// LLM-chosen relation verb; `None` for a plain wikilink.
    pub rel_type: Option<String>,
    /// Edge confidence in [0,1]; wikilinks default to 1.0.
    pub confidence: f32,
}

/// Per-target edge metadata in the directed adjacency.
#[derive(Debug, Clone)]
pub struct EdgeMeta {
    pub rel_type: Option<String>,
    pub confidence: f32,
}

/// Immutable snapshot of the note graph, built once per recompute.
#[derive(Debug, Clone, Default)]
pub struct GraphSnapshot {
    pub nodes: Vec<GraphNode>,
    /// Directed, typed, weighted resolved edges (`category/filename` pairs).
    pub edges: Vec<GraphEdge>,
}

/// Derived adjacency + lookup, shared by all three algorithms. Built once.
pub struct GraphIndex<'a> {
    pub nodes: &'a [GraphNode],
    idx_of: HashMap<&'a str, usize>,
    /// Undirected, deduped adjacency by node index.
    pub adj: Vec<HashSet<usize>>,
    /// Directed out-edges by node index, with per-target metadata. Read by
    /// `edge_confidence` to weight the direct-link relevance signal.
    pub out: Vec<HashMap<usize, EdgeMeta>>,
    /// Source-set per node index (from `source_notes`).
    pub sources: Vec<HashSet<&'a str>>,
    /// Inverted index `source_ref -> node indices citing it`. Two uses in the
    /// relevance pass: (1) gather source-sharing candidates in O(postings)
    /// instead of an O(N) per-seed scan, turning `all_related` from O(N²) into
    /// O(E); (2) supply each source's document frequency for IDF-damped
    /// source-overlap weighting (rare sources connect more strongly).
    source_postings: HashMap<&'a str, Vec<usize>>,
}

impl<'a> GraphIndex<'a> {
    #[must_use]
    pub fn build(snap: &'a GraphSnapshot) -> Self {
        let mut idx_of = HashMap::with_capacity(snap.nodes.len());
        for (i, n) in snap.nodes.iter().enumerate() {
            idx_of.insert(n.path.as_str(), i);
        }
        let mut adj = vec![HashSet::new(); snap.nodes.len()];
        let mut out: Vec<HashMap<usize, EdgeMeta>> = vec![HashMap::new(); snap.nodes.len()];
        for e in &snap.edges {
            if let (Some(&a), Some(&b)) = (idx_of.get(e.from.as_str()), idx_of.get(e.to.as_str())) {
                if a != b {
                    adj[a].insert(b);
                    adj[b].insert(a);
                    let meta = EdgeMeta {
                        rel_type: e.rel_type.clone(),
                        confidence: e.confidence,
                    };
                    // Keep the strongest if a pair appears twice.
                    out[a]
                        .entry(b)
                        .and_modify(|m| {
                            if meta.confidence > m.confidence {
                                *m = meta.clone();
                            }
                        })
                        .or_insert(meta);
                }
            }
        }
        let sources: Vec<HashSet<&str>> = snap
            .nodes
            .iter()
            .map(|n| n.sources.iter().map(String::as_str).collect::<HashSet<_>>())
            .collect();
        // Invert the note→source map once so the relevance pass never rescans
        // all nodes to find source-sharers.
        let mut source_postings: HashMap<&str, Vec<usize>> = HashMap::new();
        for (i, set) in sources.iter().enumerate() {
            for &s in set {
                source_postings.entry(s).or_default().push(i);
            }
        }
        Self {
            nodes: &snap.nodes,
            idx_of,
            adj,
            out,
            sources,
            source_postings,
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
    #[must_use]
    pub fn degree(&self, i: usize) -> usize {
        self.adj[i].len()
    }
    #[must_use]
    pub fn index_of(&self, path: &str) -> Option<usize> {
        self.idx_of.get(path).copied()
    }

    /// Document frequency of a source: how many notes cite it. Used to damp the
    /// source-overlap signal — a source shared by many notes is a weak link.
    #[must_use]
    pub fn source_df(&self, source: &str) -> usize {
        self.source_postings.get(source).map_or(0, Vec::len)
    }

    /// Node indices sharing ≥1 source with `seed` (excluding `seed` itself),
    /// gathered from the inverted index. O(sum of posting lengths over the
    /// seed's sources), not O(N).
    #[must_use]
    pub fn source_sharers(&self, seed: usize) -> HashSet<usize> {
        let mut result = HashSet::new();
        for &s in &self.sources[seed] {
            if let Some(posting) = self.source_postings.get(s) {
                for &i in posting {
                    if i != seed {
                        result.insert(i);
                    }
                }
            }
        }
        result
    }

    /// Directed edge confidence from `a` to `b`, split by edge class:
    /// `co_recall = true` matches only behavioral `co_recalled` edges,
    /// `false` only semantic ones (wikilinks / typed relations).
    fn directed_confidence(&self, a: usize, b: usize, co_recall: bool) -> f32 {
        self.out[a].get(&b).map_or(0.0, |m| {
            let is_co = m.rel_type.as_deref() == Some(CO_RECALLED_RELATION);
            if is_co == co_recall {
                m.confidence
            } else {
                0.0
            }
        })
    }

    /// Confidence of the strongest **semantic** edge between `a` and `b` in
    /// either direction; 0.0 if unconnected. Behavioral `co_recalled` edges
    /// are excluded — they feed [`Self::co_recall_confidence`] instead, so
    /// the direct-link signal stays purely semantic.
    #[must_use]
    pub fn edge_confidence(&self, a: usize, b: usize) -> f32 {
        self.directed_confidence(a, b, false)
            .max(self.directed_confidence(b, a, false))
    }

    /// Confidence of the strongest behavioral `co_recalled` edge between `a`
    /// and `b` in either direction; 0.0 if the pair was never co-recalled.
    /// Fifth relevance signal alongside the four semantic/structural ones.
    #[must_use]
    pub fn co_recall_confidence(&self, a: usize, b: usize) -> f32 {
        self.directed_confidence(a, b, true)
            .max(self.directed_confidence(b, a, true))
    }
}
