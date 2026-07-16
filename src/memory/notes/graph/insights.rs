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
    // Isolated: degree <= 1.
    let isolated = (0..g.len())
        .filter(|&i| g.degree(i) <= 1)
        .map(|i| g.nodes[i].path.clone())
        .collect();

    // Sparse communities: cohesion below threshold with >= 3 members.
    let mut size = vec![0usize; c.cohesion.len()];
    for &cid in &c.of_node {
        size[cid] += 1;
    }
    let mut sparse = Vec::new();
    for (cid, &cohesion) in c.cohesion.iter().enumerate() {
        if size[cid] >= SPARSE_MIN_SIZE && cohesion < SPARSE_COHESION_MAX {
            let exemplar = (0..g.len())
                .find(|&i| c.of_node[i] == cid)
                // rust-doctor-disable-next-line excessive-clone
                .map(|i| g.nodes[i].path.clone())
                .unwrap_or_default();
            sparse.push(SparseCommunity {
                community_id: cid,
                size: size[cid],
                cohesion,
                exemplar,
            });
        }
    }

    // Bridges: neighbour communities span >= 3 distinct ids.
    let mut bridges = Vec::new();
    for i in 0..g.len() {
        let mut comms: HashSet<usize> = HashSet::new();
        comms.insert(c.of_node[i]);
        for &nb in &g.adj[i] {
            comms.insert(c.of_node[nb]);
        }
        if comms.len() >= BRIDGE_MIN_COMMUNITIES {
            // rust-doctor-disable-next-line excessive-clone
            bridges.push(g.nodes[i].path.clone());
        }
    }

    // Surprising: cross-community or cross-type edges; peripheral endpoints more surprising.
    let mut surprising = Vec::new();
    for i in 0..g.len() {
        for &j in &g.adj[i] {
            if i >= j {
                continue;
            }
            let cross_comm = c.of_node[i] != c.of_node[j];
            let cross_type = g.nodes[i].category != g.nodes[j].category;
            if cross_comm || cross_type {
                let di = g.degree(i).max(1) as f32;
                let dj = g.degree(j).max(1) as f32;
                let base = if cross_comm { 1.0 } else { 0.0 } + if cross_type { 0.5 } else { 0.0 };
                let score = base * (1.0 / di + 1.0 / dj);
                surprising.push(SurprisingEdge {
                    from: g.nodes[i].path.clone(),
                    to: g.nodes[j].path.clone(),
                    score,
                });
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
