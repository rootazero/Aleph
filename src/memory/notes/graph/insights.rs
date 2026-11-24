//! Graph-health insights: isolated nodes, sparse communities, bridge nodes,
//! surprising cross-community/cross-type connections.

use std::collections::HashSet;

use super::community::Communities;
use super::GraphIndex;

pub const SPARSE_COHESION_MAX: f32 = 0.15;
pub const SPARSE_MIN_SIZE: usize = 3;
pub const BRIDGE_MIN_COMMUNITIES: usize = 3;
pub const SURPRISING_CAP: usize = 20;

#[derive(Debug, Clone)]
pub struct SparseCommunity {
    pub community_id: usize,
    pub size: usize,
    pub cohesion: f32,
    pub exemplar: String,
}
#[derive(Debug, Clone)]
pub struct SurprisingEdge {
    pub from: String,
    pub to: String,
    pub score: f32,
}

#[derive(Debug, Clone, Default)]
pub struct GraphInsights {
    pub isolated: Vec<String>,
    pub sparse_communities: Vec<SparseCommunity>,
    pub bridges: Vec<String>,
    pub surprising: Vec<SurprisingEdge>,
}

#[must_use]
pub fn detect(g: &GraphIndex, c: &Communities) -> GraphInsights {
    if c.of_node.len() < g.len() {
        return GraphInsights::default();
    }

    // Isolated: degree <= 1.
    let isolated = (0..g.len())
        .filter(|&i| g.degree(i) <= 1)
        .filter_map(|i| g.nodes.get(i).map(|n| n.path.clone()))
        .collect();

    // Sparse communities: cohesion below threshold with >= 3 members.
    let num_comms = c
        .cohesion
        .len()
        .max(c.of_node.iter().copied().max().map_or(0, |m| m + 1));
    let mut size = vec![0usize; num_comms];
    for &cid in &c.of_node {
        if let Some(slot) = size.get_mut(cid) {
            *slot += 1;
        }
    }
    let mut sparse = Vec::new();
    for (cid, &cohesion) in c.cohesion.iter().enumerate() {
        let comm_size = size.get(cid).copied().unwrap_or(0);
        if comm_size >= SPARSE_MIN_SIZE && cohesion < SPARSE_COHESION_MAX {
            let exemplar = (0..g.len())
                .find(|&i| c.of_node.get(i).copied() == Some(cid))
                .and_then(|i| g.nodes.get(i).map(|n| n.path.clone()))
                .unwrap_or_default();
            sparse.push(SparseCommunity {
                community_id: cid,
                size: comm_size,
                cohesion,
                exemplar,
            });
        }
    }

    // Bridges: neighbour communities span >= 3 distinct ids.
    let mut bridges = Vec::new();
    for i in 0..g.len() {
        let mut comms: HashSet<usize> = HashSet::new();
        if let Some(&ci) = c.of_node.get(i) {
            comms.insert(ci);
        }
        if let Some(nbrs) = g.adj.get(i) {
            for &nb in nbrs {
                if let Some(&cnb) = c.of_node.get(nb) {
                    comms.insert(cnb);
                }
            }
        }
        if comms.len() >= BRIDGE_MIN_COMMUNITIES {
            if let Some(node) = g.nodes.get(i) {
                bridges.push(node.path.clone());
            }
        }
    }

    // Surprising: cross-community or cross-type edges; peripheral endpoints more surprising.
    let mut surprising = Vec::new();
    for i in 0..g.len() {
        if let Some(nbrs) = g.adj.get(i) {
            for &j in nbrs {
                if i >= j {
                    continue;
                }
                let ci = c.of_node.get(i).copied();
                let cj = c.of_node.get(j).copied();
                let cross_comm = ci != cj;
                let cross_type = match (g.nodes.get(i), g.nodes.get(j)) {
                    (Some(ni), Some(nj)) => ni.category != nj.category,
                    _ => false,
                };
                if cross_comm || cross_type {
                    let di = g.degree(i).max(1) as f32;
                    let dj = g.degree(j).max(1) as f32;
                    let base =
                        if cross_comm { 1.0 } else { 0.0 } + if cross_type { 0.5 } else { 0.0 };
                    let score = base * (1.0 / di + 1.0 / dj);
                    if let (Some(ni), Some(nj)) = (g.nodes.get(i), g.nodes.get(j)) {
                        surprising.push(SurprisingEdge {
                            from: ni.path.clone(),
                            to: nj.path.clone(),
                            score,
                        });
                    }
                }
            }
        }
    }
    surprising.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| (a.from.as_str(), a.to.as_str()).cmp(&(b.from.as_str(), b.to.as_str())))
    });
    surprising.truncate(SURPRISING_CAP);

    GraphInsights {
        isolated,
        sparse_communities: sparse,
        bridges,
        surprising,
    }
}
