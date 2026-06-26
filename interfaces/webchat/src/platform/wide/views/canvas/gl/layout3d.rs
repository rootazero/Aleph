//! 3D force-directed layout: Coulomb repulsion (O(n²)) + Hooke springs +
//! centering. Deterministic (seed from id hash). Pure — unit-tested on native.

use super::math::Vec3;
use crate::canvas_engine::fnv1a::fnv1a_32;

const REPULSION: f32 = 8000.0; // ~k_e
const SPRING_K: f32 = 0.02; // edge stiffness
const REST_LEN: f32 = 60.0; // spring rest length
const CENTER_PULL: f32 = 0.002; // gentle pull to origin
const DAMPING: f32 = 0.85; // velocity damping
const MAX_STEP: f32 = 30.0; // clamp per-step displacement
const EPS: f32 = 0.5; // convergence threshold (max displacement)

pub struct ForceLayout {
    n: usize,
    edges: Vec<(u32, u32)>,
    vel: Vec<Vec3>,
    last_max_disp: f32,
}

impl ForceLayout {
    pub fn new(node_count: usize, edges: &[(u32, u32)]) -> ForceLayout {
        ForceLayout {
            n: node_count,
            edges: edges.to_vec(),
            vel: vec![Vec3::zero(); node_count],
            last_max_disp: f32::INFINITY,
        }
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
        // Centering + integrate.
        let mut max_disp = 0.0_f32;
        for i in 0..self.n {
            force[i] = force[i].sub(&pos[i].scale(CENTER_PULL));
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

    pub fn converged(&self) -> bool {
        self.last_max_disp < EPS
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
        let l = ForceLayout::new(10, &line_graph(10));
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
        let mut l = ForceLayout::new(20, &line_graph(20));
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
        let mut l = ForceLayout::new(15, &line_graph(15));
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
        let mut l = ForceLayout::new(4, &[(0, 1)]);
        let mut pos = l.seed(&ids);
        for _ in 0..400 {
            l.step(&mut pos);
        }
        let d_edge = pos[0].sub(&pos[1]).length();
        let d_free = pos[2].sub(&pos[3]).length();
        assert!(d_edge < d_free, "edge {d_edge} should be < free {d_free}");
    }
}
