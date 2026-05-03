use std::collections::HashMap;

use super::types::{CanvasEdge, CanvasNode, ClusterNode, Vec3};

/// FNV-1a 32-bit hash, deterministic across runs and platforms.
pub(crate) fn fnv1a_32(bytes: &[u8]) -> u32 {
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

pub const Z_ACTIVE: f32 = 0.0;
pub const Z_ONE_HOP: f32 = 60.0;
pub const Z_TWO_HOP: f32 = 140.0;

/// Adaptive radius for a hop layer.
///
/// Grows ~ √n so the ring widens as neighborhoods densify, then is multiplied
/// by `hop` (1 for one-hop, 2 for two-hop, 2.4 for orphans) and a viewport
/// scale factor so small windows shrink and large windows expand within
/// reasonable bounds.
fn r_for_hop(hop_factor: f32, n: usize, viewport_w_px: f32) -> f32 {
    let base = 100.0_f32;
    let count_factor = (1.0 + (n as f32) / 16.0).sqrt();
    let vw_factor = (viewport_w_px / 800.0).clamp(0.6, 1.4);
    base * count_factor * vw_factor * hop_factor
}

pub fn r_one_hop(n: usize, viewport_w_px: f32) -> f32 {
    r_for_hop(1.0, n, viewport_w_px)
}

pub fn r_two_hop(n: usize, viewport_w_px: f32) -> f32 {
    r_for_hop(2.0, n, viewport_w_px)
}

pub fn r_orphan(n: usize, viewport_w_px: f32) -> f32 {
    r_for_hop(2.4, n, viewport_w_px)
}

/// Compute ideal (target) positions for active + neighbors using radial geometry.
pub fn compute_target_positions(
    active: &CanvasNode,
    one_hop: &[CanvasNode],
    two_hop: &[CanvasNode],
    clusters: &[ClusterNode],
    edges: &[CanvasEdge],
    viewport_w_px: f32,
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

    // Cluster folding handles dense sectors via fold_threshold; r1/r2 now grow
    // adaptively with neighborhood size and viewport width via r_one_hop/r_two_hop.
    let total_visible = one_hop.len() + clusters.len() + two_hop.len();
    let r1 = r_one_hop(total_visible, viewport_w_px);
    let r2 = r_two_hop(total_visible, viewport_w_px);

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
            None => (r1, 0.0),
        };
        let parent_angle = py.atan2(px);
        let jitter = (fnv1a_32(n.id.as_bytes()) as f32 / u32::MAX as f32 - 0.5) * 0.6;
        let theta = parent_angle + jitter;
        let x = r2 * theta.cos();
        let y = r2 * theta.sin();
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
        let targets = compute_target_positions(&active, &one_hop, &[], &[], &edges, 800.0);
        let pos_a = targets.get("a").unwrap();
        assert_eq!(pos_a.x, 0.0);
        assert_eq!(pos_a.y, 0.0);
    }

    #[test]
    fn compute_targets_one_hop_at_r1() {
        let active = n("a", "concept", 0);
        let one_hop = vec![n("b", "concept", 1)];
        let edges = vec![e(0, 1, "uses")];
        let targets = compute_target_positions(&active, &one_hop, &[], &[], &edges, 800.0);
        let pos_b = targets.get("b").unwrap();
        let r = (pos_b.x.powi(2) + pos_b.y.powi(2)).sqrt();
        // total_visible = 1 (one_hop) + 0 (clusters) + 0 (two_hop) = 1.
        let expected = r_one_hop(1, 800.0);
        assert!(
            (r - expected).abs() < 1.0,
            "1-hop should be at radius {expected}, got {r}"
        );
    }

    #[test]
    fn compute_targets_two_hop_at_r2() {
        let active = n("a", "concept", 0);
        let one_hop = vec![n("b", "concept", 1)];
        let two_hop = vec![n("c", "concept", 2)];
        let edges = vec![e(0, 1, "uses"), e(1, 2, "part_of")];
        let targets =
            compute_target_positions(&active, &one_hop, &two_hop, &[], &edges, 800.0);
        let pos_c = targets.get("c").unwrap();
        let r = (pos_c.x.powi(2) + pos_c.y.powi(2)).sqrt();
        // total_visible = 1 (one_hop) + 0 (clusters) + 1 (two_hop) = 2.
        let expected = r_two_hop(2, 800.0);
        assert!(
            (r - expected).abs() < 5.0,
            "2-hop should be at radius {expected}, got {r}"
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
        let targets = compute_target_positions(&active, &one_hop, &[], &[], &edges, 800.0);
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
    fn r_one_hop_grows_with_node_count() {
        let small = r_one_hop(20, 800.0);
        let big = r_one_hop(500, 800.0);
        assert!(big > small * 1.5, "expected bigger n to grow R, got small={small} big={big}");
    }

    #[test]
    fn r_one_hop_clamps_viewport() {
        // Below the lower bound (clamped to 0.6) and above the upper (1.4) should both pin.
        let narrow_low  = r_one_hop(50, 100.0);
        let narrow_min  = r_one_hop(50, 480.0);  // 480/800 = 0.6 exactly
        let wide_max    = r_one_hop(50, 1120.0); // 1120/800 = 1.4
        let wide_high   = r_one_hop(50, 4000.0);
        let eps = 1e-3;
        assert!((narrow_low - narrow_min).abs() < eps,
            "lower clamp: {narrow_low} vs {narrow_min}");
        assert!((wide_high - wide_max).abs() < eps,
            "upper clamp: {wide_high} vs {wide_max}");
    }

    #[test]
    fn r_two_hop_outside_one_hop() {
        let one = r_one_hop(50, 800.0);
        let two = r_two_hop(50, 800.0);
        assert!(two > one, "R₂ must exceed R₁: one={one} two={two}");
    }

    #[test]
    fn r_orphan_outside_two_hop() {
        let two = r_two_hop(50, 800.0);
        let orphan = r_orphan(50, 800.0);
        assert!(orphan > two, "R_orphan must exceed R₂: two={two} orphan={orphan}");
    }
}

