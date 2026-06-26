//! 4-signal relevance: direct-link ×3, source-overlap ×4 (IDF-damped),
//! Adamic-Adar ×1.5, type-affinity ×1. Concurrency for the full pairwise pass
//! via std threads.
//!
//! Both evidence signals share one rarity principle: a connector linking few
//! notes is stronger than one linking many. Adamic-Adar applies it to shared
//! graph neighbours (`1/ln(degree)`); source-overlap applies it to shared
//! `source_notes` via document frequency (`ln2/ln(df)`), so a source cited by
//! exactly the two notes carries full weight while a ubiquitous source decays.

use std::collections::HashSet;
use std::f32::consts::LN_2;

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
        Self {
            direct_link: 3.0,
            source_overlap: 4.0,
            adamic_adar: 1.5,
            type_affinity: 1.0,
        }
    }
}

/// Relatedness of nodes `a` and `b` (by index).
#[must_use]
pub fn score_pair(g: &GraphIndex, w: &SignalWeights, a: usize, b: usize) -> f32 {
    if a == b {
        return 0.0;
    }
    let mut s = 0.0;
    if g.adj[a].contains(&b) {
        s += w.direct_link;
    }
    // Source-overlap, damped by each shared source's document frequency
    // (Adamic-Adar over the note↔source bipartite graph). df ≥ 2 always holds
    // for a shared source, so `ln2/ln(df)` lands in (0, 1]: df=2 → 1.0 (the
    // rarest possible shared source, exactly the two notes), larger df decays.
    let mut src_signal = 0.0_f32;
    for &src in g.sources[a].intersection(&g.sources[b]) {
        let df = g.source_df(src);
        if df > 1 {
            src_signal += LN_2 / (df as f32).ln();
        }
    }
    if src_signal > 0.0 {
        s += w.source_overlap * src_signal;
    }
    let mut aa = 0.0_f32;
    for &c in g.adj[a].intersection(&g.adj[b]) {
        let d = g.degree(c);
        if d > 1 {
            aa += 1.0 / (d as f32).ln();
        }
    }
    s += w.adamic_adar * aa;
    if g.nodes[a].category == g.nodes[b].category {
        s += w.type_affinity;
    }
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
        for &n2 in &g.adj[n1] {
            cand.insert(n2);
        }
    }
    // Source-sharers from the inverted index (O(postings), not an O(N) scan).
    cand.extend(g.source_sharers(seed));
    cand.remove(&seed);
    let mut scored: Vec<(String, f32)> = cand
        .into_iter()
        .map(|c| (g.nodes[c].path.clone(), score_pair(g, w, seed, c)))
        .filter(|(_, sc)| *sc > 0.0)
        .collect();
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    scored.truncate(k);
    scored
}

/// Top-`k` related lists for every node, parallelised across `threads` OS
/// threads via `std::thread::scope` (no external dep). Run inside
/// `tokio::task::spawn_blocking`. Deterministic: output is in node-index order.
#[must_use]
pub fn all_related(
    g: &GraphIndex,
    w: &SignalWeights,
    k: usize,
    threads: usize,
) -> Vec<(String, Vec<(String, f32)>)> {
    let n = g.len();
    if n == 0 {
        return Vec::new();
    }
    let threads = threads.clamp(1, n);
    let chunk = n.div_ceil(threads);
    let mut out: Vec<Vec<(String, Vec<(String, f32)>)>> = Vec::new();
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|t| {
                let start = t * chunk;
                let end = ((t + 1) * chunk).min(n);
                scope.spawn(move || {
                    let mut part = Vec::with_capacity(end.saturating_sub(start));
                    for i in start..end {
                        part.push((g.nodes[i].path.clone(), related(g, w, i, k)));
                    }
                    part
                })
            })
            .collect();
        for (t, h) in handles.into_iter().enumerate() {
            match h.join() {
                Ok(part) => out.push(part),
                // One partition panicking must not abort the whole offline
                // graph recompute (`GraphRecomputeStage`). Drop this slice and
                // keep the rest; its nodes are re-materialised on the next
                // tick. (P7 graceful degradation — a transient per-node bug
                // should not cost the entire daily relatedness rebuild.)
                Err(_) => tracing::warn!(
                    partition = t,
                    "relevance worker panicked; skipping its slice for this recompute"
                ),
            }
        }
    });
    out.into_iter().flatten().collect()
}
