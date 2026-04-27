use std::collections::HashMap;

use super::types::{CanvasEdge, CanvasNode, ClusterNode, Vec2, Vec3};

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

pub const R_1: f32 = 180.0;
pub const R_2: f32 = 320.0;
pub const Z_ACTIVE: f32 = 0.0;
pub const Z_ONE_HOP: f32 = 60.0;
pub const Z_TWO_HOP: f32 = 140.0;

/// Compute ideal (target) positions for active + neighbors using radial geometry.
pub fn compute_target_positions(
    active: &CanvasNode,
    one_hop: &[CanvasNode],
    two_hop: &[CanvasNode],
    clusters: &[ClusterNode],
    edges: &[CanvasEdge],
) -> HashMap<String, Vec3> {
    let mut out = HashMap::new();
    out.insert(active.id.clone(), Vec3::new(0.0, 0.0, Z_ACTIVE));

    // Group 1-hop neighbors + clusters by their connecting relation.
    // Canonical index ordering (T11 adapter): active=0, one_hop[i]=i+1.
    let mut by_relation: HashMap<String, Vec<(String, f32)>> = HashMap::new();
    for (i, n) in one_hop.iter().enumerate() {
        let neighbor_idx = i + 1;
        let rel = relation_to_active(neighbor_idx, edges).unwrap_or_else(|| "_default".to_string());
        let w = n.decay_score * n.edge_count.max(1) as f32;
        by_relation.entry(rel).or_default().push((n.id.clone(), w));
    }
    for c in clusters {
        by_relation
            .entry(c.relation.clone())
            .or_default()
            .push((c.id.clone(), c.aggregated_weight));
    }

    // Cluster folding handles dense sectors via fold_threshold; keep R_1 fixed
    // so the 1-hop ring never sprawls past the viewport when nodes get many.
    let r1 = R_1;

    // Assign sector center angles (evenly spaced, hash-ordered)
    let relations: Vec<String> = by_relation.keys().cloned().collect();
    let sector_centers = assign_sectors(&relations);

    // Within each sector sort by weight descending; tiebreak by id for stability.
    for members in by_relation.values_mut() {
        members.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
    }

    let total_n = by_relation.values().map(|v| v.len()).sum::<usize>().max(1) as f32;

    for (rel, members) in &by_relation {
        let center_angle = sector_centers.get(rel).copied().unwrap_or(0.0);
        let n_in_sector = members.len() as f32;
        let sector_width = (std::f32::consts::TAU * (n_in_sector / total_n)).max(0.15);
        let delta = sector_width / (n_in_sector + 1.0);
        for (i, (id, _w)) in members.iter().enumerate() {
            // Alternate offsets: 0, +1, -1, +2, -2, …
            let offset_steps = ((i + 1) / 2) as f32 * if i % 2 == 0 { 1.0 } else { -1.0 };
            let theta = center_angle + offset_steps * delta;
            let x = r1 * theta.cos();
            let y = r1 * theta.sin();
            out.insert(id.clone(), Vec3::new(x, y, Z_ONE_HOP));
        }
    }

    // 2-hop nodes: orbit around their introducing 1-hop parent angle.
    // Canonical index ordering: two_hop[j] = 1 + one_hop.len() + j.
    for (j, n) in two_hop.iter().enumerate() {
        let two_hop_idx = 1 + one_hop.len() + j;
        let parent_id = find_one_hop_parent(two_hop_idx, one_hop, edges);
        let parent_pos = parent_id.as_ref().and_then(|p| out.get(p)).copied();
        let (px, py) = match parent_pos {
            Some(p) => (p.x, p.y),
            None => (R_1, 0.0),
        };
        let parent_angle = py.atan2(px);
        let jitter = (fnv1a_32(n.id.as_bytes()) as f32 / u32::MAX as f32 - 0.5) * 0.6;
        let theta = parent_angle + jitter;
        let x = R_2 * theta.cos();
        let y = R_2 * theta.sin();
        out.insert(n.id.clone(), Vec3::new(x, y, Z_TWO_HOP));
    }

    out
}

/// Returns the relation label of the edge connecting the active node (index 0)
/// to the neighbor at `neighbor_idx` (1..=one_hop.len()). Returns None if no
/// such edge exists.
fn relation_to_active(neighbor_idx: usize, edges: &[CanvasEdge]) -> Option<String> {
    edges
        .iter()
        .find(|e| {
            (e.from_idx == 0 && e.to_idx == neighbor_idx)
                || (e.to_idx == 0 && e.from_idx == neighbor_idx)
        })
        .map(|e| e.relation.clone())
}

/// Find the 1-hop node that introduces the given 2-hop node by walking edges.
/// Returns the 1-hop node's id if any edge connects them, else None.
fn find_one_hop_parent(
    two_hop_idx: usize,
    one_hop: &[CanvasNode],
    edges: &[CanvasEdge],
) -> Option<String> {
    edges.iter().find_map(|e| {
        let other_idx = if e.from_idx == two_hop_idx {
            Some(e.to_idx)
        } else if e.to_idx == two_hop_idx {
            Some(e.from_idx)
        } else {
            None
        }?;
        // Map other_idx back to a 1-hop position: 1..=one_hop.len()
        if other_idx >= 1 && other_idx <= one_hop.len() {
            Some(one_hop[other_idx - 1].id.clone())
        } else {
            None
        }
    })
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
    use crate::canvas_engine::types::{CanvasEdge, CanvasNode, Color, Vec2};

    fn n(id: &str, category: &str, hop: u8) -> CanvasNode {
        CanvasNode {
            id: id.to_string(),
            name: id.to_string(),
            category: category.to_string(),
            color: Color::new(0, 0, 0),
            radius: 30.0,
            position: Vec2::new(0.0, 0.0),
            velocity: Vec2::new(0.0, 0.0),
            z: 0.0,
            hop,
            decay_score: 1.0,
            edge_count: 1,
            pinned: false,
        }
    }

    fn e(from: usize, to: usize, relation: &str) -> CanvasEdge {
        CanvasEdge {
            from_idx: from,
            to_idx: to,
            relation: relation.to_string(),
            is_wikilink: false,
            is_active_link: true,
        }
    }

    #[test]
    fn compute_targets_active_at_origin() {
        let active = n("a", "concept", 0);
        let one_hop = vec![n("b", "concept", 1)];
        let edges = vec![e(0, 1, "uses")];
        let targets = compute_target_positions(&active, &one_hop, &[], &[], &edges);
        let pos_a = targets.get("a").unwrap();
        assert_eq!(pos_a.x, 0.0);
        assert_eq!(pos_a.y, 0.0);
    }

    #[test]
    fn compute_targets_one_hop_at_r1() {
        let active = n("a", "concept", 0);
        let one_hop = vec![n("b", "concept", 1)];
        let edges = vec![e(0, 1, "uses")];
        let targets = compute_target_positions(&active, &one_hop, &[], &[], &edges);
        let pos_b = targets.get("b").unwrap();
        let r = (pos_b.x.powi(2) + pos_b.y.powi(2)).sqrt();
        assert!(
            (r - 180.0).abs() < 1.0,
            "1-hop should be at radius 180, got {r}"
        );
    }

    #[test]
    fn compute_targets_two_hop_at_r2() {
        let active = n("a", "concept", 0);
        let one_hop = vec![n("b", "concept", 1)];
        let two_hop = vec![n("c", "concept", 2)];
        let edges = vec![e(0, 1, "uses"), e(1, 2, "part_of")];
        let targets = compute_target_positions(&active, &one_hop, &two_hop, &[], &edges);
        let pos_c = targets.get("c").unwrap();
        let r = (pos_c.x.powi(2) + pos_c.y.powi(2)).sqrt();
        assert!(
            (r - 320.0).abs() < 5.0,
            "2-hop should be at radius 320, got {r}"
        );
    }

    #[test]
    fn sector_hash_is_deterministic() {
        let a = sector_center_angle("uses");
        let b = sector_center_angle("uses");
        assert!((a - b).abs() < 1e-6);
    }

    #[test]
    fn sector_hash_in_range() {
        for r in &[
            "uses",
            "part_of",
            "references",
            "is_a",
            "depends_on",
            "owned_by",
        ] {
            let a = sector_center_angle(r);
            assert!(a >= 0.0 && a < std::f32::consts::TAU, "{r} -> {a}");
        }
    }

    #[test]
    fn assign_sectors_preserves_relative_hash_order() {
        let relations = vec![
            "uses".to_string(),
            "part_of".to_string(),
            "references".to_string(),
        ];
        let assigned = assign_sectors(&relations);

        let mut hash_sorted: Vec<_> = relations.iter().cloned().collect();
        hash_sorted.sort_by(|a, b| {
            sector_center_angle(a)
                .partial_cmp(&sector_center_angle(b))
                .unwrap()
        });

        // After assignment, the relative order in the result should match hash order
        let mut assigned_order: Vec<_> = assigned.iter().collect();
        assigned_order.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        let final_relations: Vec<String> =
            assigned_order.into_iter().map(|(r, _)| r.clone()).collect();

        assert_eq!(final_relations, hash_sorted);
    }

    #[test]
    fn assign_sectors_uniform_distribution() {
        let relations = vec![
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "d".to_string(),
        ];
        let assigned = assign_sectors(&relations);
        let mut angles: Vec<_> = assigned.values().copied().collect();
        angles.sort_by(|a, b| a.partial_cmp(b).unwrap());
        for w in angles.windows(2) {
            let gap = w[1] - w[0];
            assert!((gap - std::f32::consts::TAU / 4.0).abs() < 1e-3);
        }
    }

    #[test]
    fn compute_targets_groups_by_relation() {
        // Active connected to b via "uses" and c via "part_of".
        // Sectors should put b and c in DIFFERENT angular wedges.
        let active = n("a", "concept", 0);
        let one_hop = vec![n("b", "concept", 1), n("c", "concept", 1)];
        let edges = vec![e(0, 1, "uses"), e(0, 2, "part_of")];
        let targets = compute_target_positions(&active, &one_hop, &[], &[], &edges);
        let pos_b = targets.get("b").unwrap();
        let pos_c = targets.get("c").unwrap();
        let angle_b = pos_b.y.atan2(pos_b.x);
        let angle_c = pos_c.y.atan2(pos_c.x);
        // With 2 distinct relations, sectors are TAU/2 = π apart.
        let diff = (angle_b - angle_c).abs();
        let diff = diff.min(std::f32::consts::TAU - diff); // wrap-around
        assert!(
            (diff - std::f32::consts::PI).abs() < 0.5,
            "b and c should be in opposite sectors, got angular diff {diff}"
        );
    }

    #[test]
    fn force_step_converges_within_60_iterations() {
        let active = n("a", "concept", 0);
        let one_hop: Vec<_> = (0..5).map(|i| n(&format!("h{i}"), "concept", 1)).collect();
        let edges: Vec<_> = (0..5).map(|i| e(0, i + 1, "uses")).collect();
        let targets = compute_target_positions(&active, &one_hop, &[], &[], &edges);

        let mut layout = RadialForceLayout::new(targets, ForceConfig::default());
        for _ in 0..60 {
            layout.step(0.016);
        }
        assert!(
            layout.kinetic_energy() < 1.0,
            "expected KE < 1.0 after 60 iters, got {}",
            layout.kinetic_energy()
        );
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ForceConfig {
    pub target_attract: f32,
    pub repulsion: f32,
    pub damping: f32,
    pub max_velocity: f32,
}

impl Default for ForceConfig {
    fn default() -> Self {
        Self {
            target_attract: 0.15,
            repulsion: 800.0,
            damping: 0.85,
            max_velocity: 50.0,
        }
    }
}

pub struct RadialForceLayout {
    pub positions: HashMap<String, Vec3>,
    pub velocities: HashMap<String, (f32, f32)>,
    targets: HashMap<String, Vec3>,
    config: ForceConfig,
    active_id: Option<String>,
}

impl RadialForceLayout {
    pub fn new(targets: HashMap<String, Vec3>, config: ForceConfig) -> Self {
        let positions = targets.clone();
        let velocities = targets
            .keys()
            .map(|k| (k.clone(), (0.0_f32, 0.0_f32)))
            .collect();
        Self {
            positions,
            velocities,
            targets,
            config,
            active_id: None,
        }
    }

    pub fn pin_active(&mut self, id: String) {
        self.active_id = Some(id);
    }

    pub fn step(&mut self, dt: f32) {
        let cfg = self.config;
        let ids: Vec<String> = self.positions.keys().cloned().collect();
        let mut forces: HashMap<String, (f32, f32)> =
            ids.iter().map(|i| (i.clone(), (0.0, 0.0))).collect();

        // Spring force toward target
        for id in &ids {
            let pos = self.positions[id];
            let tgt = self.targets[id];
            let f = forces.get_mut(id).unwrap();
            f.0 += cfg.target_attract * (tgt.x - pos.x);
            f.1 += cfg.target_attract * (tgt.y - pos.y);
        }

        // Pairwise repulsion (O(n²); fine for ≤50 nodes)
        for i in 0..ids.len() {
            for j in (i + 1)..ids.len() {
                let pi = self.positions[&ids[i]];
                let pj = self.positions[&ids[j]];
                let dx = pi.x - pj.x;
                let dy = pi.y - pj.y;
                let d2 = (dx * dx + dy * dy).max(1.0);
                let f_mag = cfg.repulsion / d2;
                let inv_d = 1.0 / d2.sqrt();
                let fx = f_mag * dx * inv_d;
                let fy = f_mag * dy * inv_d;
                {
                    let fi = forces.get_mut(&ids[i]).unwrap();
                    fi.0 += fx;
                    fi.1 += fy;
                }
                {
                    let fj = forces.get_mut(&ids[j]).unwrap();
                    fj.0 -= fx;
                    fj.1 -= fy;
                }
            }
        }

        // Integrate (skip pinned active)
        for id in &ids {
            if Some(id) == self.active_id.as_ref() {
                if let Some(v) = self.velocities.get_mut(id) {
                    *v = (0.0, 0.0);
                }
                if let Some(p) = self.positions.get_mut(id) {
                    p.x = 0.0;
                    p.y = 0.0;
                }
                continue;
            }
            let (fx, fy) = forces[id];
            let v = self.velocities.get_mut(id).unwrap();
            v.0 = (v.0 + fx * dt) * cfg.damping;
            v.1 = (v.1 + fy * dt) * cfg.damping;
            // Clamp velocity
            let speed = (v.0 * v.0 + v.1 * v.1).sqrt();
            if speed > cfg.max_velocity {
                v.0 *= cfg.max_velocity / speed;
                v.1 *= cfg.max_velocity / speed;
            }
            let pos = self.positions.get_mut(id).unwrap();
            pos.x += v.0 * dt;
            pos.y += v.1 * dt;
        }
    }

    pub fn kinetic_energy(&self) -> f32 {
        self.velocities
            .values()
            .map(|(vx, vy)| vx * vx + vy * vy)
            .sum::<f32>()
            * 0.5
    }
}
