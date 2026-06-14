//! 4-signal relevance: direct-link ×3, source-overlap ×4, Adamic-Adar ×1.5,
//! type-affinity ×1. Concurrency for the full pairwise pass via std threads.

use std::collections::HashSet;

use super::GraphIndex;

/// Tunable weights (defaults mirror the reference protocol).
#[derive(Debug, Clone, Copy)]
pub struct SignalWeights {
    pub direct_link: f32,
    pub source_overlap: f32,
    pub adamic_adar: f32,
    pub type_affinity: f32,
}
impl Default for SignalWeights {
    fn default() -> Self {
        Self { direct_link: 3.0, source_overlap: 4.0, adamic_adar: 1.5, type_affinity: 1.0 }
    }
}

/// Relatedness of nodes `a` and `b` (by index).
#[must_use]
pub fn score_pair(g: &GraphIndex, w: &SignalWeights, a: usize, b: usize) -> f32 {
    if a == b { return 0.0; }
    let mut s = 0.0;
    if g.adj[a].contains(&b) { s += w.direct_link; }
    let overlap = g.sources[a].intersection(&g.sources[b]).count();
    if overlap > 0 { s += w.source_overlap * overlap as f32; }
    let mut aa = 0.0_f32;
    for &c in g.adj[a].intersection(&g.adj[b]) {
        let d = g.degree(c);
        if d > 1 { aa += 1.0 / (d as f32).ln(); }
    }
    s += w.adamic_adar * aa;
    if g.nodes[a].category == g.nodes[b].category { s += w.type_affinity; }
    s
}

/// Top-`k` related nodes for `seed` path, descending score (ties by path).
/// Candidate set is bounded to the local 2-hop neighbourhood + source-sharing
/// nodes, so cost is local, not O(N).
#[must_use]
pub fn related(g: &GraphIndex, w: &SignalWeights, seed: usize, k: usize) -> Vec<(String, f32)> {
    let mut cand: HashSet<usize> = HashSet::new();
    for &n1 in &g.adj[seed] {
        cand.insert(n1);
        for &n2 in &g.adj[n1] { cand.insert(n2); }
    }
    if !g.sources[seed].is_empty() {
        for i in 0..g.len() {
            if i != seed && g.sources[i].intersection(&g.sources[seed]).next().is_some() {
                cand.insert(i);
            }
        }
    }
    cand.remove(&seed);
    let mut scored: Vec<(String, f32)> = cand.into_iter()
        .map(|c| (g.nodes[c].path.clone(), score_pair(g, w, seed, c)))
        .filter(|(_, sc)| *sc > 0.0)
        .collect();
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.0.cmp(&b.0))
    });
    scored.truncate(k);
    scored
}

/// Top-`k` related lists for every node, parallelised across `threads` OS
/// threads via `std::thread::scope` (no external dep). Run inside
/// `tokio::task::spawn_blocking`. Deterministic: output is in node-index order.
#[must_use]
pub fn all_related(g: &GraphIndex, w: &SignalWeights, k: usize, threads: usize)
    -> Vec<(String, Vec<(String, f32)>)>
{
    let n = g.len();
    if n == 0 { return Vec::new(); }
    let threads = threads.clamp(1, n);
    let chunk = n.div_ceil(threads);
    let mut out: Vec<Vec<(String, Vec<(String, f32)>)>> = Vec::new();
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads).map(|t| {
            let start = t * chunk;
            let end = ((t + 1) * chunk).min(n);
            scope.spawn(move || {
                let mut part = Vec::with_capacity(end.saturating_sub(start));
                for i in start..end {
                    part.push((g.nodes[i].path.clone(), related(g, w, i, k)));
                }
                part
            })
        }).collect();
        for h in handles { out.push(h.join().expect("relevance worker panicked")); }
    });
    out.into_iter().flatten().collect()
}
