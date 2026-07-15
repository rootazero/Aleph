//! Louvain community detection (modularity maximisation) over the undirected
//! note graph, followed by a Leiden-style connectivity refinement. Hand-rolled,
//! no external crate (R3). Deterministic: nodes are visited in index order and
//! ties break to the lower community id, so the same graph always yields the
//! same partition.
//!
//! ## Why the refinement pass
//!
//! Single-level Louvain's aggregate/re-move loop can leave a "community" whose
//! members are split across ≥2 disconnected blobs (a super-node merge that a
//! later local move stranded) — a well-known Louvain defect that Leiden was
//! designed to cure. Such a partition is an artifact, not a cluster, and it
//! fragments everything downstream that trusts a community to be one connected
//! region: `note_graph_query community`, dream synthesis grouping, and the
//! canvas centroid gravity. [`refine_connected`] splits each Louvain community
//! into its connected components so every emitted community is provably
//! internally connected — a no-op on already-connected communities (the common
//! case), so it never worsens a good partition.

use std::collections::HashMap;

use super::GraphIndex;

/// Community assignment + per-community cohesion (intra-edge density).
pub struct Communities {
    /// Dense community id (0..k) per node index.
    pub of_node: Vec<usize>,
    /// Cohesion per community id: actual intra edges / possible intra edges.
    pub cohesion: Vec<f32>,
}

type Adj = Vec<Vec<(usize, f64)>>; // weighted adjacency

#[must_use]
pub fn detect(g: &GraphIndex) -> Communities {
    let n = g.len();
    if n == 0 {
        return Communities {
            of_node: vec![],
            cohesion: vec![],
        };
    }
    // Base weighted adjacency (unit weights) from undirected adjacency.
    let adj: Adj = (0..n)
        .map(|i| g.adj[i].iter().map(|&j| (j, 1.0)).collect())
        .collect();
    let total_w: f64 = adj
        .iter()
        .flat_map(|nbrs| nbrs.iter().map(|(_, w)| *w))
        .sum::<f64>()
        / 2.0;
    if total_w == 0.0 {
        // No edges → each node is its own singleton community.
        return Communities {
            of_node: (0..n).collect(),
            cohesion: vec![0.0; n],
        };
    }
    let raw = louvain(&adj, total_w);
    // Leiden-style refinement: split any Louvain community that is internally
    // disconnected into its connected components. Consumes the raw labels
    // directly (it depends only on same-community equality) and returns dense
    // 0..k labels, so no separate renumber pass is needed.
    let of_node = refine_connected(g, &raw);
    let k = of_node.iter().copied().max().map_or(0, |m| m + 1);
    let cohesion = cohesion_per_community(g, &of_node, k);
    Communities { of_node, cohesion }
}

/// Iterated Louvain: local-moving to a modularity optimum, aggregate, repeat
/// until no further merging. Returns base-node → community id.
fn louvain(adj0: &Adj, total_w: f64) -> Vec<usize> {
    let mut node_comm: Vec<usize> = (0..adj0.len()).collect();
    let mut adj = adj0.clone();
    loop {
        let part = local_moving(&adj, total_w);
        let dense = renumber(&part);
        let k = dense.iter().copied().max().map_or(0, |m| m + 1);
        if k == adj.len() {
            break;
        } // nothing merged → converged
        for c in &mut node_comm {
            *c = dense[*c];
        }
        // Aggregate communities into super-nodes (self-loops carry intra weight).
        let mut acc: Vec<HashMap<usize, f64>> = vec![HashMap::new(); k];
        for (u, nbrs) in adj.iter().enumerate() {
            let cu = dense[u];
            for &(v, w) in nbrs {
                *acc[cu].entry(dense[v]).or_default() += w;
            }
        }
        adj = acc.into_iter().map(|m| m.into_iter().collect()).collect();
        if k == 1 {
            break;
        }
    }
    node_comm
}

/// One pass of local moving to a modularity optimum on weighted `adj`.
fn local_moving(adj: &Adj, total_w: f64) -> Vec<usize> {
    let n = adj.len();
    let mut comm: Vec<usize> = (0..n).collect();
    // Weighted degree (incident weight, self-loops counted once per their weight).
    let k: Vec<f64> = adj
        .iter()
        .map(|nbrs| nbrs.iter().map(|(_, w)| *w).sum())
        .collect();
    let mut sigma_tot = k.clone(); // total incident weight per community
    let two_m = 2.0 * total_w;
    let mut improved = true;
    while improved {
        improved = false;
        for i in 0..n {
            let ci = comm[i];
            sigma_tot[ci] -= k[i]; // remove i from its community
                                   // Weight from i to each neighbouring community.
            let mut w_to: HashMap<usize, f64> = HashMap::new();
            for &(j, w) in &adj[i] {
                if j != i {
                    *w_to.entry(comm[j]).or_default() += w;
                }
            }
            // Best community by ΔQ = w_to(c) - k_i * sigma_tot(c) / (2m).
            let mut best_c = ci;
            let mut best_gain =
                w_to.get(&ci).copied().unwrap_or(0.0) - k[i] * sigma_tot[ci] / two_m;
            for (&c, &wic) in &w_to {
                let gain = wic - k[i] * sigma_tot[c] / two_m;
                if gain > best_gain + 1e-12 || (gain > best_gain - 1e-12 && c < best_c) {
                    best_gain = gain;
                    best_c = c;
                }
            }
            comm[i] = best_c;
            sigma_tot[best_c] += k[i];
            if best_c != ci {
                improved = true;
            }
        }
    }
    comm
}

/// Renumber arbitrary community labels to dense 0..k in first-seen order.
fn renumber(comm: &[usize]) -> Vec<usize> {
    let mut map: HashMap<usize, usize> = HashMap::new();
    let mut next = 0;
    comm.iter()
        .map(|&c| {
            *map.entry(c).or_insert_with(|| {
                let v = next;
                next += 1;
                v
            })
        })
        .collect()
}

/// Leiden-style refinement: guarantee every emitted community's induced
/// subgraph is internally connected, by splitting each input community into its
/// connected components over the *base* graph. Returns dense `0..k` labels.
///
/// Determinism holds regardless of `adj`'s (hash-set) iteration order: the
/// connected components of a fixed graph are invariant, and each component's
/// label is fixed by the index of its lowest-numbered node (the outer loop
/// visits nodes in index order and only mints a new label at a component's
/// first-seen node). Pure graph structure — no content, no LLM (R7).
pub(crate) fn refine_connected(g: &GraphIndex, comm: &[usize]) -> Vec<usize> {
    let n = comm.len();
    let mut out = vec![usize::MAX; n];
    let mut next = 0usize;
    for start in 0..n {
        if out[start] != usize::MAX {
            continue; // already claimed by an earlier component
        }
        let cid = comm[start];
        let label = next;
        next += 1;
        // BFS the connected component of `start`, restricted to its community.
        let mut queue = std::collections::VecDeque::new();
        queue.push_back(start);
        out[start] = label;
        while let Some(u) = queue.pop_front() {
            for &v in &g.adj[u] {
                if comm[v] == cid && out[v] == usize::MAX {
                    out[v] = label;
                    queue.push_back(v);
                }
            }
        }
    }
    out
}

/// Intra-edge density per community over the *base* graph.
fn cohesion_per_community(g: &GraphIndex, of_node: &[usize], k: usize) -> Vec<f32> {
    let mut size = vec![0usize; k];
    for &c in of_node {
        size[c] += 1;
    }
    let mut intra = vec![0usize; k];
    for i in 0..g.len() {
        for &j in &g.adj[i] {
            if i < j && of_node[i] == of_node[j] {
                intra[of_node[i]] += 1;
            }
        }
    }
    (0..k)
        .map(|c| {
            let s = size[c];
            if s < 2 {
                return 0.0;
            }
            let possible = s * (s - 1) / 2;
            intra[c] as f32 / possible as f32
        })
        .collect()
}
