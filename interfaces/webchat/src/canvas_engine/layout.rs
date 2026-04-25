use std::collections::HashMap;

use super::types::{CanvasEdge, CanvasNode, Vec2};

/// FNV-1a 32-bit hash, deterministic across runs and platforms.
fn fnv1a_32(bytes: &[u8]) -> u32 {
    let mut h: u32 = 0x811c9dc5;
    for b in bytes {
        h ^= *b as u32;
        h = h.wrapping_mul(0x01000193);
    }
    h
}

/// Map a relation name to a stable angle in [0, 2π).
pub fn sector_center_angle(relation: &str) -> f32 {
    let h = fnv1a_32(relation.as_bytes());
    ((h as f64 / u32::MAX as f64) * std::f64::consts::TAU) as f32
}

/// Assign each relation an angle so the K relations are evenly spaced around the
/// circle, in `[0, TAU)`. The relative order matches the FNV-1a hash order so
/// spatial memory is consistent across renders. The assigned angles are
/// `i * TAU / K` for `i in 0..K`; on the circle the gap between the last and
/// first wraps naturally.
pub fn assign_sectors(relations: &[String]) -> HashMap<String, f32> {
    let mut sorted: Vec<&String> = relations.iter().collect();
    sorted.sort_by(|a, b| {
        sector_center_angle(a)
            .partial_cmp(&sector_center_angle(b))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let k = sorted.len().max(1) as f32;
    let mut out = HashMap::new();
    for (i, r) in sorted.iter().enumerate() {
        out.insert((*r).clone(), (i as f32) * std::f32::consts::TAU / k);
    }
    out
}

pub struct LayoutConfig {
    pub repulsion_strength: f64,
    pub attraction_strength: f64,
    pub damping: f64,
    pub center_gravity: f64,
    pub max_velocity: f64,
    pub convergence_threshold: f64,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self {
            repulsion_strength: 800.0,
            attraction_strength: 0.015,
            damping: 0.85,
            center_gravity: 0.02,
            max_velocity: 40.0,
            convergence_threshold: 0.5,
        }
    }
}

pub struct ForceLayout {
    pub config: LayoutConfig,
    pub is_settled: bool,
}

impl ForceLayout {
    pub fn new() -> Self {
        Self {
            config: LayoutConfig::default(),
            is_settled: false,
        }
    }

    pub fn tick(&mut self, nodes: &mut [CanvasNode], edges: &[CanvasEdge]) -> f64 {
        let n = nodes.len();
        if n == 0 {
            self.is_settled = true;
            return 0.0;
        }

        let mut forces = vec![Vec2::zero(); n];

        // Repulsion between all pairs of nodes
        for i in 0..n {
            for j in (i + 1)..n {
                let delta = nodes[i].position - nodes[j].position;
                let dist = delta.length().max(1.0);
                let force = delta.normalized() * (self.config.repulsion_strength / (dist * dist));
                forces[i] += force;
                forces[j] = forces[j] - force;
            }
        }

        // Attraction along edges
        for edge in edges {
            if edge.from_idx >= n || edge.to_idx >= n {
                continue;
            }
            let delta = nodes[edge.to_idx].position - nodes[edge.from_idx].position;
            let dist = delta.length().max(1.0);
            let force = delta.normalized() * (dist * self.config.attraction_strength);
            forces[edge.from_idx] += force;
            forces[edge.to_idx] = forces[edge.to_idx] - force;
        }

        // Center gravity pulls all nodes toward the origin
        for i in 0..n {
            forces[i] += (Vec2::zero() - nodes[i].position) * self.config.center_gravity;
        }

        // Apply forces and integrate velocity
        let mut total_energy = 0.0;
        for i in 0..n {
            if nodes[i].pinned {
                nodes[i].velocity = Vec2::zero();
                continue;
            }
            nodes[i].velocity = (nodes[i].velocity + forces[i]) * self.config.damping;
            let speed = nodes[i].velocity.length();
            if speed > self.config.max_velocity {
                nodes[i].velocity = nodes[i].velocity.normalized() * self.config.max_velocity;
            }
            nodes[i].position += nodes[i].velocity;
            total_energy += speed * speed;
        }

        // Continuous drift — never fully settle
        if total_energy < self.config.convergence_threshold * 10.0 {
            for node in nodes.iter_mut() {
                if !node.pinned {
                    let jx = (js_sys::Math::random() - 0.5) * 0.1;
                    let jy = (js_sys::Math::random() - 0.5) * 0.1;
                    node.velocity += Vec2::new(jx, jy);
                    total_energy += 0.1;
                }
            }
        }
        self.is_settled = false; // Never stop animating
        total_energy
    }

    pub fn wake(&mut self) {
        self.is_settled = false;
    }
}

impl Default for ForceLayout {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod radial_tests {
    use super::*;

    #[test]
    fn sector_hash_is_deterministic() {
        let a = sector_center_angle("uses");
        let b = sector_center_angle("uses");
        assert!((a - b).abs() < 1e-6);
    }

    #[test]
    fn sector_hash_in_range() {
        for r in &["uses", "part_of", "references", "is_a", "depends_on", "owned_by"] {
            let a = sector_center_angle(r);
            assert!(a >= 0.0 && a < std::f32::consts::TAU, "{r} -> {a}");
        }
    }

    #[test]
    fn assign_sectors_preserves_relative_hash_order() {
        let relations = vec!["uses".to_string(), "part_of".to_string(), "references".to_string()];
        let assigned = assign_sectors(&relations);

        let mut hash_sorted: Vec<_> = relations.iter().cloned().collect();
        hash_sorted.sort_by(|a, b| {
            sector_center_angle(a).partial_cmp(&sector_center_angle(b)).unwrap()
        });

        // After assignment, the relative order in the result should match hash order
        let mut assigned_order: Vec<_> = assigned.iter().collect();
        assigned_order.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        let final_relations: Vec<String> = assigned_order.into_iter().map(|(r, _)| r.clone()).collect();

        assert_eq!(final_relations, hash_sorted);
    }

    #[test]
    fn assign_sectors_uniform_distribution() {
        let relations = vec!["a".to_string(), "b".to_string(), "c".to_string(), "d".to_string()];
        let assigned = assign_sectors(&relations);
        let mut angles: Vec<_> = assigned.values().copied().collect();
        angles.sort_by(|a, b| a.partial_cmp(b).unwrap());
        for w in angles.windows(2) {
            let gap = w[1] - w[0];
            assert!((gap - std::f32::consts::TAU / 4.0).abs() < 1e-3);
        }
    }
}
