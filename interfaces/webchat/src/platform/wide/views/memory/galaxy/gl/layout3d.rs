//! 3D force-directed layout: Coulomb repulsion (O(n²)) + Hooke springs +
//! centering. Deterministic (seed from id hash). Pure — unit-tested on native.

use std::collections::HashMap;

use super::math::Vec3;
use crate::memory_graph::fnv1a::fnv1a_32;

const REPULSION: f32 = 8000.0; // ~k_e
const SPRING_K: f32 = 0.02; // edge stiffness
const REST_LEN: f32 = 60.0; // spring rest length
const CENTER_PULL: f32 = 0.002; // gentle pull to origin
const COMMUNITY_PULL: f32 = 0.015; // gravity toward own community centroid
const DAMPING: f32 = 0.85; // velocity damping
const MAX_STEP: f32 = 30.0; // clamp per-step displacement
const EPS: f32 = 0.5; // convergence threshold (max displacement)

/// Simulated-annealing floor (d3-force's `alphaMin`). Below this the layout is
/// declared settled whatever the residual displacement.
const ALPHA_MIN: f32 = 0.001;
/// Per-step alpha decay = `1 - ALPHA_MIN^(1/400)`, so alpha reaches ALPHA_MIN in
/// exactly the caller's 400-step settle budget (`scene::MAX_SETTLE_STEPS`).
/// Without it the n=500 graph is still expanding when the budget runs out
/// (measured max_disp 6.36, 12.7x EPS) and there is no stationary extent for
/// the camera to frame.
const ALPHA_DECAY: f32 = 0.017_14;

pub struct ForceLayout {
    n: usize,
    edges: Vec<(u32, u32)>,
    vel: Vec<Vec3>,
    last_max_disp: f32,
    /// Per-node community id (`None` = unassigned). Same order/length as nodes.
    communities: Vec<Option<u32>>,
    /// Annealing temperature, 1.0 → ALPHA_MIN over the settle budget. Scales
    /// every force, so the simulation cools instead of oscillating forever.
    alpha: f32,
}

impl ForceLayout {
    pub fn new(
        node_count: usize,
        edges: &[(u32, u32)],
        communities: &[Option<u32>],
    ) -> ForceLayout {
        ForceLayout {
            n: node_count,
            edges: edges.to_vec(),
            vel: vec![Vec3::zero(); node_count],
            last_max_disp: f32::INFINITY,
            communities: communities.to_vec(),
            alpha: 1.0,
        }
    }

    /// Warm-start the anneal at `alpha` instead of the default 1.0. Used when a
    /// graph is REFRESHED rather than replaced: the cloud is already settled, so
    /// it needs to relax the links that changed, not re-expand from cold.
    /// Clamped to `(0, 1]`.
    pub fn set_alpha(&mut self, alpha: f32) {
        self.alpha = alpha.clamp(ALPHA_MIN, 1.0);
    }

    pub fn seed(&self, ids: &[String]) -> Vec<Vec3> {
        ids.iter()
            .map(|id| {
                let h = fnv1a_32(id.as_bytes());
                let theta = (h & 0xffff) as f32 / 65535.0 * std::f32::consts::TAU;
                let phi = ((h >> 16) & 0xffff) as f32 / 65535.0 * std::f32::consts::PI;
                let r = 200.0;
                Vec3::new(
                    r * phi.sin() * theta.cos(),
                    r * phi.sin() * theta.sin(),
                    r * phi.cos(),
                )
            })
            .collect()
    }

    pub fn step(&mut self, pos: &mut [Vec3]) {
        // Cool the simulation (d3-force annealing): every force is scaled by
        // alpha, so the layout terminates by construction rather than by luck.
        self.alpha += (0.0 - self.alpha) * ALPHA_DECAY;
        let mut force = vec![Vec3::zero(); self.n];
        // Repulsion (all pairs).
        for i in 0..self.n {
            for j in (i + 1)..self.n {
                let d = pos[i].sub(&pos[j]);
                let dist2 = d.dot(&d).max(1.0);
                let f = REPULSION / dist2;
                let dir = d.scale(1.0 / dist2.sqrt());
                force[i] = force[i].add(&dir.scale(f));
                force[j] = force[j].sub(&dir.scale(f));
            }
        }
        // Springs (edges).
        for &(a, b) in &self.edges {
            let (a, b) = (a as usize, b as usize);
            let d = pos[b].sub(&pos[a]);
            let dist = d.length().max(1e-3);
            let f = SPRING_K * (dist - REST_LEN);
            let dir = d.scale(1.0 / dist);
            force[a] = force[a].add(&dir.scale(f));
            force[b] = force[b].sub(&dir.scale(f));
        }
        // Community centroid gravity: pull each node toward its community's
        // centroid so Louvain communities settle as spatial clusters. No-op
        // when no node carries a community (cold cache).
        if self.communities.iter().any(Option::is_some) {
            let mut sums: HashMap<u32, (Vec3, usize)> = HashMap::new();
            for (i, c) in self.communities.iter().enumerate() {
                if let Some(cid) = c {
                    let e = sums.entry(*cid).or_insert((Vec3::zero(), 0));
                    e.0 = e.0.add(&pos[i]);
                    e.1 += 1;
                }
            }
            for (i, c) in self.communities.iter().enumerate() {
                if let Some(cid) = c {
                    let (sum, count) = sums[cid];
                    let centroid = sum.scale(1.0 / count as f32);
                    let to_centroid = centroid.sub(&pos[i]);
                    force[i] = force[i].add(&to_centroid.scale(COMMUNITY_PULL));
                }
            }
        }

        // Centering + anneal + integrate.
        let mut max_disp = 0.0_f32;
        for i in 0..self.n {
            force[i] = force[i].sub(&pos[i].scale(CENTER_PULL)).scale(self.alpha);
            self.vel[i] = self.vel[i].add(&force[i]).scale(DAMPING);
            let mut disp = self.vel[i];
            let dl = disp.length();
            if dl > MAX_STEP {
                disp = disp.scale(MAX_STEP / dl);
            }
            pos[i] = pos[i].add(&disp);
            max_disp = max_disp.max(disp.length());
        }
        self.last_max_disp = max_disp;
    }

    pub fn energy(&self, pos: &[Vec3]) -> f32 {
        // Spring potential + inverse repulsion sum (proxy; lower = more settled).
        let mut e = 0.0;
        for &(a, b) in &self.edges {
            let d = pos[b as usize].sub(&pos[a as usize]).length() - REST_LEN;
            e += 0.5 * SPRING_K * d * d;
        }
        e
    }

    /// Settled when either the cloud has stopped moving OR the simulation has
    /// cooled below `ALPHA_MIN` (which happens within the settle budget for any
    /// graph size — the residual forces at that temperature are visually inert).
    pub fn converged(&self) -> bool {
        self.last_max_disp < EPS || self.alpha < ALPHA_MIN
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line_graph(n: usize) -> Vec<(u32, u32)> {
        (0..n as u32 - 1).map(|i| (i, i + 1)).collect()
    }

    #[test]
    fn seed_is_deterministic() {
        let ids: Vec<String> = (0..10).map(|i| format!("n{i}")).collect();
        let l = ForceLayout::new(10, &line_graph(10), &vec![None; 10]);
        let a = l.seed(&ids);
        let b = l.seed(&ids);
        assert_eq!(a.len(), 10);
        for i in 0..10 {
            assert_eq!(a[i], b[i]);
        }
    }

    #[test]
    fn energy_decreases_over_steps() {
        let ids: Vec<String> = (0..20).map(|i| format!("n{i}")).collect();
        let mut l = ForceLayout::new(20, &line_graph(20), &vec![None; 20]);
        let mut pos = l.seed(&ids);
        let e0 = l.energy(&pos);
        for _ in 0..200 {
            l.step(&mut pos);
        }
        let e1 = l.energy(&pos);
        assert!(e1 < e0, "energy did not decrease: {e0} -> {e1}");
    }

    #[test]
    fn converges_within_budget() {
        let ids: Vec<String> = (0..15).map(|i| format!("n{i}")).collect();
        let mut l = ForceLayout::new(15, &line_graph(15), &vec![None; 15]);
        let mut pos = l.seed(&ids);
        for _ in 0..600 {
            l.step(&mut pos);
            if l.converged() {
                break;
            }
        }
        assert!(l.converged(), "did not converge in 600 steps");
    }

    #[test]
    fn connected_nodes_closer_than_unconnected() {
        // 2 disjoint pairs: (0-1) edge, (2,3) no edge. After settling, edge pair
        // sits near spring rest length; non-edge pair drifts apart via repulsion.
        let ids: Vec<String> = (0..4).map(|i| format!("n{i}")).collect();
        let mut l = ForceLayout::new(4, &[(0, 1)], &vec![None; 4]);
        let mut pos = l.seed(&ids);
        for _ in 0..400 {
            l.step(&mut pos);
        }
        let d_edge = pos[0].sub(&pos[1]).length();
        let d_free = pos[2].sub(&pos[3]).length();
        assert!(d_edge < d_free, "edge {d_edge} should be < free {d_free}");
    }

    #[test]
    fn alpha_decays_to_min_within_settle_budget() {
        let ids: Vec<String> = (0..10).map(|i| format!("n{i}")).collect();
        let mut l = ForceLayout::new(10, &line_graph(10), &vec![None; 10]);
        let mut pos = l.seed(&ids);
        for _ in 0..400 {
            l.step(&mut pos);
        }
        assert!(l.alpha < ALPHA_MIN, "alpha={} after 400 steps", l.alpha);
        assert!(l.converged());
    }

    #[test]
    fn converges_at_n_500_within_budget() {
        // The graph size the canvas actually requests (mod.rs caps at 500). On the
        // pre-annealing layout this is the case that never settled: max_disp was
        // still 6.36 (12.7x EPS) when the 400-step budget ran out, so the camera
        // was framing a still-expanding cloud.
        const N: usize = 500;
        let ids: Vec<String> = (0..N).map(|i| format!("n{i}")).collect();
        // A backbone plus chords: a realistic note graph, not a pathological one.
        let mut edges: Vec<(u32, u32)> = (0..N as u32 - 1).map(|i| (i, i + 1)).collect();
        for i in (0..N as u32).step_by(7) {
            edges.push((i, (i + 53) % N as u32));
        }
        let mut l = ForceLayout::new(N, &edges, &vec![None; N]);
        let mut pos = l.seed(&ids);
        let mut steps = 0;
        for _ in 0..400 {
            l.step(&mut pos);
            steps += 1;
            if l.converged() {
                break;
            }
        }
        assert!(
            l.converged(),
            "n=500 did not converge in {steps} steps (max_disp={}, alpha={})",
            l.last_max_disp,
            l.alpha
        );
        // And the settled cloud has a finite extent for the camera to fit.
        let r = pos.iter().map(|p| p.length()).fold(0.0_f32, f32::max);
        assert!(r.is_finite() && r > 1.0, "degenerate extent {r}");
    }

    #[test]
    fn same_community_settles_closer_than_cross_community() {
        // 4 nodes, NO edges. Communities: {0,1}=A, {2,3}=B. Centroid gravity
        // should pull same-community nodes together despite pure repulsion.
        let ids: Vec<String> = (0..4).map(|i| format!("n{i}")).collect();
        let comms = vec![Some(0u32), Some(0), Some(1), Some(1)];
        let mut l = ForceLayout::new(4, &[], &comms);
        let mut pos = l.seed(&ids);
        for _ in 0..400 {
            l.step(&mut pos);
        }
        let same = pos[0].sub(&pos[1]).length();
        let cross = pos[0].sub(&pos[2]).length();
        assert!(
            same < cross,
            "same-community {same} should be < cross {cross}"
        );
    }
}
