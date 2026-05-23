use std::collections::HashMap;
use std::f32::consts::TAU;

use super::fnv1a::hash_jitter;
use super::types::{CanvasEdge, CanvasNode, ClusterNode, Vec2, Vec3};

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

/// Place `ids` on a ring around `(0,0)` at base radius `base_r`,
/// with deterministic per-id jitter in angle (±17°) and radius (±15%).
///
/// Writes positions into `out`. Skips ids already present.
pub(crate) fn place_perturbed_ring(
    ids: &[&str],
    base_r: f64,
    out: &mut HashMap<String, Vec2>,
) {
    if ids.is_empty() {
        return;
    }
    let n = ids.len() as f32;
    for (i, id) in ids.iter().enumerate() {
        if out.contains_key(*id) {
            continue;
        }
        let j_angle = 0.30 * hash_jitter(id);              // ±17.2°
        let j_radius = 0.15 * hash_jitter(&format!("r:{id}")); // ±15%, decorrelated
        let angle = ((i as f32 / n) * TAU + j_angle) as f64;
        let radius = base_r * (1.0 + j_radius as f64);
        out.insert((*id).into(), Vec2 {
            x: radius * angle.cos(),
            y: radius * angle.sin(),
        });
    }
}

/// Compute ideal (target) positions for active + neighbors using radial geometry.
///
/// Layout strategy (Task 2.3):
/// - Active centre at world origin.
/// - 1-hop nodes (including cluster representatives) on a perturbed ring at `r_one_hop`.
/// - 2-hop nodes on a perturbed ring at `r_two_hop`.
///
/// Both rings use `place_perturbed_ring` for deterministic per-id jitter.
/// Orphan positions are NOT computed here; call `adapter::populate_orphans` separately.
pub fn compute_target_positions(
    active: &CanvasNode,
    one_hop: &[CanvasNode],
    two_hop: &[CanvasNode],
    clusters: &[ClusterNode],
    edges: &[CanvasEdge],
    viewport_w_px: f32,
) -> HashMap<String, Vec3> {
    // `edges` is retained in the signature for API stability; the perturbed-ring
    // layout no longer uses edge relations for angular placement.
    let _ = edges;

    let mut xy: HashMap<String, super::types::Vec2> = HashMap::new();
    let mut out: HashMap<String, Vec3> = HashMap::new();

    // 1. Centre at origin.
    out.insert(active.id.clone(), Vec3::new(0.0, 0.0, Z_ACTIVE));

    // 2. 1-hop + cluster representatives on the inner ring.
    let one_hop_n = one_hop.len() + clusters.len();
    let r1 = r_one_hop(one_hop_n, viewport_w_px) as f64;
    let mut one_hop_ids: Vec<&str> = one_hop.iter().map(|n| n.id.as_str()).collect();
    for c in clusters {
        one_hop_ids.push(c.id.as_str());
    }
    place_perturbed_ring(&one_hop_ids, r1, &mut xy);
    for id in &one_hop_ids {
        if let Some(p) = xy.remove(*id) {
            out.insert((*id).to_string(), Vec3::new(p.x as f32, p.y as f32, Z_ONE_HOP));
        }
    }

    // 3. 2-hop nodes on the outer ring.
    let r2 = r_two_hop(two_hop.len(), viewport_w_px) as f64;
    let two_hop_ids: Vec<&str> = two_hop.iter().map(|n| n.id.as_str()).collect();
    place_perturbed_ring(&two_hop_ids, r2, &mut xy);
    for id in &two_hop_ids {
        if let Some(p) = xy.remove(*id) {
            out.insert((*id).to_string(), Vec3::new(p.x as f32, p.y as f32, Z_TWO_HOP));
        }
    }

    out
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
        }
    }

    fn e(from: usize, to: usize, relation: &str) -> CanvasEdge {
        CanvasEdge {
            from_idx: from,
            to_idx: to,
            from_id: String::new(),
            to_id: String::new(),
            relation: relation.to_string(),
            label: None,
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
        // perturbed ring places nodes at base_r ± 15% jitter: allow 20% tolerance.
        let base = r_one_hop(1, 800.0);
        assert!(
            (r - base).abs() < base * 0.20,
            "1-hop should be near radius {base} (±20%), got {r}"
        );
    }

    #[test]
    fn compute_targets_two_hop_at_r2() {
        let active = n("a", "concept", 0);
        let one_hop = vec![n("b", "concept", 1)];
        let two_hop = vec![n("c", "concept", 2)];
        let edges = vec![e(0, 1, "uses"), e(1, 2, "part_of")];
        let targets = compute_target_positions(&active, &one_hop, &two_hop, &[], &edges, 800.0);
        let pos_c = targets.get("c").unwrap();
        let r = (pos_c.x.powi(2) + pos_c.y.powi(2)).sqrt();
        // 2-hop ring uses r_two_hop(two_hop.len()=1, 800); allow 20% for jitter.
        let base = r_two_hop(1, 800.0);
        assert!(
            (r - base).abs() < base * 0.20,
            "2-hop should be near radius {base} (±20%), got {r}"
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
    fn compute_targets_one_hop_nodes_all_placed() {
        // Perturbed-ring layout places every 1-hop node in the output map,
        // regardless of edge relation labels.
        let active = n("a", "concept", 0);
        let one_hop = vec![n("b", "concept", 1), n("c", "concept", 1)];
        let edges = vec![e(0, 1, "uses"), e(0, 2, "part_of")];
        let targets = compute_target_positions(&active, &one_hop, &[], &[], &edges, 800.0);
        assert!(targets.contains_key("b"), "b should be in output");
        assert!(targets.contains_key("c"), "c should be in output");
        // Both should be on the 1-hop ring (near r1, allow ±20% jitter).
        let base = r_one_hop(2, 800.0);
        for id in &["b", "c"] {
            let p = targets[*id];
            let r = (p.x.powi(2) + p.y.powi(2)).sqrt();
            assert!(
                (r - base).abs() < base * 0.20,
                "{id} radius {r} should be near {base} (±20%)"
            );
        }
    }

    #[test]
    fn r_one_hop_grows_with_node_count() {
        let small = r_one_hop(20, 800.0);
        let big = r_one_hop(500, 800.0);
        assert!(
            big > small * 1.5,
            "expected bigger n to grow R, got small={small} big={big}"
        );
    }

    #[test]
    fn r_one_hop_clamps_viewport() {
        // Below the lower bound (clamped to 0.6) and above the upper (1.4) should both pin.
        let narrow_low = r_one_hop(50, 100.0);
        let narrow_min = r_one_hop(50, 480.0); // 480/800 = 0.6 exactly
        let wide_max = r_one_hop(50, 1120.0); // 1120/800 = 1.4
        let wide_high = r_one_hop(50, 4000.0);
        let eps = 1e-3;
        assert!(
            (narrow_low - narrow_min).abs() < eps,
            "lower clamp: {narrow_low} vs {narrow_min}"
        );
        assert!(
            (wide_high - wide_max).abs() < eps,
            "upper clamp: {wide_high} vs {wide_max}"
        );
    }

    #[test]
    fn r_two_hop_outside_one_hop() {
        let one = r_one_hop(50, 800.0);
        let two = r_two_hop(50, 800.0);
        assert!(two > one, "R₂ must exceed R₁: one={one} two={two}");
    }

    #[test]
    fn perturbed_ring_is_deterministic() {
        let ids = vec!["a", "b", "c", "d"];
        let mut m1 = HashMap::new();
        let mut m2 = HashMap::new();
        place_perturbed_ring(&ids, 200.0, &mut m1);
        place_perturbed_ring(&ids, 200.0, &mut m2);
        assert_eq!(m1, m2);
    }

    #[test]
    fn perturbed_ring_avoids_collision() {
        let ids: Vec<&str> = (0..8)
            .map(|i| Box::leak(format!("n{i}").into_boxed_str()) as &str)
            .collect();
        let mut m = HashMap::new();
        place_perturbed_ring(&ids, 200.0, &mut m);
        let n = ids.len() as f64;
        let min_sep = (std::f64::consts::TAU / n) * 0.4; // 40% of even spacing
        let mut angles: Vec<f64> = ids
            .iter()
            .map(|id| {
                let p = m[*id];
                p.y.atan2(p.x).rem_euclid(std::f64::consts::TAU)
            })
            .collect();
        angles.sort_by(|a, b| a.partial_cmp(b).unwrap());
        for w in angles.windows(2) {
            assert!(
                (w[1] - w[0]) >= min_sep,
                "adjacent angles {} and {} too close (min_sep={})",
                w[0],
                w[1],
                min_sep
            );
        }
    }

    #[test]
    fn perturbed_ring_skips_existing_ids() {
        let mut m = HashMap::new();
        m.insert("a".into(), Vec2 { x: 999.0, y: 999.0 });
        place_perturbed_ring(&["a", "b"], 200.0, &mut m);
        assert_eq!(m["a"], Vec2 { x: 999.0, y: 999.0 });
        assert!(m.contains_key("b"));
    }

    #[test]
    fn known_seed_layout_matches_baseline() {
        use crate::canvas_engine::adapter::{populate_orphans, to_neighborhood, GraphNeighborsResponse};
        use std::collections::HashMap;

        let json = include_str!("../../tests/fixtures/canvas_30nodes.json");
        let resp: GraphNeighborsResponse = serde_json::from_str(json).unwrap();

        // fold_threshold high enough that no top-K folding kicks in (29 nodes < 99)
        let mut nbhd = to_neighborhood(&resp, 0.0, 99);
        populate_orphans(&mut nbhd, &resp.nodes);

        let baseline_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/layout_baseline_30nodes.json"
        );

        // Bless mode: dump current positions to baseline file
        if std::env::var("BLESS_LAYOUT_SNAPSHOTS").is_ok() {
            let snapshot: HashMap<String, (f32, f32)> = nbhd
                .target_positions
                .iter()
                .map(|(k, v)| (k.clone(), (v.x, v.y)))
                .collect();
            let serialized = serde_json::to_string_pretty(&snapshot).unwrap();
            std::fs::write(baseline_path, serialized).expect("baseline write");
            return;
        }

        // Compare mode: load baseline and assert every position is within tolerance
        let baseline_raw = std::fs::read_to_string(baseline_path)
            .expect("run with BLESS_LAYOUT_SNAPSHOTS=1 to create baseline");
        let baseline: HashMap<String, (f32, f32)> = serde_json::from_str(&baseline_raw).unwrap();

        for (id, expected) in &baseline {
            let actual = nbhd
                .target_positions
                .get(id)
                .unwrap_or_else(|| panic!("layout missing id {id}"));
            assert!(
                (actual.x - expected.0).abs() < 0.01,
                "id={} x drift: actual={} expected={}",
                id, actual.x, expected.0
            );
            assert!(
                (actual.y - expected.1).abs() < 0.01,
                "id={} y drift: actual={} expected={}",
                id, actual.y, expected.1
            );
        }

        // Also assert no UNEXPECTED ids (catches additions)
        assert_eq!(
            nbhd.target_positions.len(),
            baseline.len(),
            "position count drift: expected {}, got {}",
            baseline.len(),
            nbhd.target_positions.len()
        );
    }
}
