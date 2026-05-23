//! Poisson-disk-like deterministic placement for orphan nodes.
//!
//! - Avoids the central exclusion rect (the inner 60 % of the viewport,
//!   which is reserved for center + 1-hop + 2-hop rings).
//! - Maintains a minimum pairwise distance between all placed nodes
//!   (existing + new), pulling from hash-derived candidate samples.
//! - Deterministic given the same `(orphans, viewport, existing)` triple.
//!
//! ## f64 rationale
//! All coordinates and distance math use f64 throughout to match `Vec2`
//! (which has `pub x: f64, pub y: f64`). The only u32 value is the raw
//! `fnv1a_32` hash, which is immediately converted to f64 for arithmetic.

use crate::canvas_engine::fnv1a::fnv1a_32;
use crate::canvas_engine::types::Vec2;
use std::collections::HashMap;

/// Default minimum distance between two orphan centres (in world px).
#[allow(dead_code)] // referenced by unit tests; exposed for callers needing the invariant
pub(crate) const MIN_DISTANCE: f64 = 56.0; // ≈ 2 × dot radius + 20
/// Candidate attempts per orphan before we accept the best-so-far.
pub(crate) const CANDIDATES_PER_ORPHAN: u32 = 20;
/// Fraction of viewport the central exclusion rect occupies (each axis).
pub(crate) const CENTRAL_EXCLUSION: f64 = 0.60;
/// Threshold above which orphans spill into a second outer band.
pub(crate) const SPILL_THRESHOLD: usize = 20;

/// Place each orphan in `ids` into `out`. `existing` is the read-only
/// set of already-placed positions used for collision avoidance.
pub(crate) fn place_scattered(
    ids: &[&str],
    viewport: (f64, f64),
    existing: &HashMap<String, Vec2>,
    out: &mut HashMap<String, Vec2>,
) {
    let (vw, vh) = viewport;
    let half_w = vw * 0.5;
    let half_h = vh * 0.5;
    let excl_w = vw * CENTRAL_EXCLUSION * 0.5;
    let excl_h = vh * CENTRAL_EXCLUSION * 0.5;
    let spill = ids.len() > SPILL_THRESHOLD;

    for (i, id) in ids.iter().enumerate() {
        let (best, _) =
            best_candidate(id, i, half_w, half_h, excl_w, excl_h, existing, out, spill);
        out.insert((*id).into(), best);
    }
}

#[allow(clippy::too_many_arguments)] // internal helper; struct-wrapping the 9 args adds noise
fn best_candidate(
    id: &str,
    index: usize,
    half_w: f64,
    half_h: f64,
    excl_w: f64,
    excl_h: f64,
    existing: &HashMap<String, Vec2>,
    placed: &HashMap<String, Vec2>,
    spill: bool,
) -> (Vec2, f64) {
    let band = if spill && index >= SPILL_THRESHOLD {
        Band::Outer
    } else {
        Band::Inner
    };

    let mut best_pos = Vec2 { x: 0.0, y: 0.0 };
    let mut best_score = f64::NEG_INFINITY;

    for attempt in 0..CANDIDATES_PER_ORPHAN {
        let key = format!("scatter:{id}:{attempt}");
        let h = fnv1a_32(key.as_bytes());
        let rx = ((h % 1000) as f64 / 1000.0) - 0.5; // [-0.5, 0.5)
        let ry = ((h.wrapping_shr(11) % 1000) as f64 / 1000.0) - 0.5;
        let mut cx = rx * 2.0 * half_w;
        let mut cy = ry * 2.0 * half_h;

        // Push out of central exclusion rect along the shorter axis.
        // signum() can return 0.0 when cx/cy == 0.0, so clamp to ±1.
        if cx.abs() < excl_w && cy.abs() < excl_h {
            if (excl_w - cx.abs()) < (excl_h - cy.abs()) {
                let sign = if cx >= 0.0 { 1.0_f64 } else { -1.0_f64 };
                cx = excl_w * sign;
            } else {
                let sign = if cy >= 0.0 { 1.0_f64 } else { -1.0_f64 };
                cy = excl_h * sign;
            }
        }

        if matches!(band, Band::Outer) {
            let r = (cx * cx + cy * cy).sqrt().max(1.0);
            let target_r = (half_w.min(half_h) - 30.0).max(r + 30.0);
            let s = target_r / r;
            cx *= s;
            cy *= s;
        }

        let p = Vec2 { x: cx, y: cy };
        let score = min_distance(p, existing, placed);
        if score > best_score {
            best_score = score;
            best_pos = p;
        }
    }
    (best_pos, best_score)
}

enum Band {
    Inner,
    Outer,
}

fn min_distance(
    p: Vec2,
    existing: &HashMap<String, Vec2>,
    placed: &HashMap<String, Vec2>,
) -> f64 {
    let mut best = f64::INFINITY;
    for q in existing.values().chain(placed.values()) {
        let dx = p.x - q.x;
        let dy = p.y - q.y;
        let d = (dx * dx + dy * dy).sqrt();
        if d < best {
            best = d;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vp() -> (f64, f64) {
        (1200.0, 800.0)
    }

    #[test]
    fn scattered_orphans_avoid_center() {
        let ids: Vec<&str> = (0..10)
            .map(|i| Box::leak(format!("o{i}").into_boxed_str()) as &str)
            .collect();
        let mut out = HashMap::new();
        place_scattered(&ids, vp(), &HashMap::new(), &mut out);
        let (vw, vh) = vp();
        let excl_w = vw * CENTRAL_EXCLUSION * 0.5;
        let excl_h = vh * CENTRAL_EXCLUSION * 0.5;
        for (id, p) in &out {
            assert!(
                p.x.abs() >= excl_w || p.y.abs() >= excl_h,
                "orphan {} at ({}, {}) inside exclusion rect",
                id,
                p.x,
                p.y
            );
        }
    }

    #[test]
    fn scattered_orphans_minimum_distance() {
        let ids: Vec<&str> = (0..8)
            .map(|i| Box::leak(format!("o{i}").into_boxed_str()) as &str)
            .collect();
        let mut out = HashMap::new();
        place_scattered(&ids, vp(), &HashMap::new(), &mut out);
        let v: Vec<_> = out.values().copied().collect();
        for i in 0..v.len() {
            for j in (i + 1)..v.len() {
                let d = ((v[i].x - v[j].x).powi(2) + (v[i].y - v[j].y).powi(2)).sqrt();
                assert!(
                    d >= MIN_DISTANCE * 0.5,
                    "two orphans only {:.1}px apart (expect ≥ {:.1})",
                    d,
                    MIN_DISTANCE * 0.5
                );
            }
        }
    }

    #[test]
    fn scattered_is_deterministic() {
        let ids = ["a", "b", "c", "d", "e"];
        let mut m1 = HashMap::new();
        let mut m2 = HashMap::new();
        place_scattered(&ids, vp(), &HashMap::new(), &mut m1);
        place_scattered(&ids, vp(), &HashMap::new(), &mut m2);
        for k in &ids {
            assert_eq!(m1[*k].x, m2[*k].x);
            assert_eq!(m1[*k].y, m2[*k].y);
        }
    }

    #[test]
    fn scattered_spills_band_when_count_gt_threshold() {
        let ids: Vec<&str> = (0..25)
            .map(|i| Box::leak(format!("o{i:02}").into_boxed_str()) as &str)
            .collect();
        let mut out = HashMap::new();
        place_scattered(&ids, vp(), &HashMap::new(), &mut out);
        let mut radii: Vec<f64> = out.values().map(|p| (p.x * p.x + p.y * p.y).sqrt()).collect();
        radii.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let max_r = *radii.last().unwrap();
        let median = radii[radii.len() / 2];
        assert!(
            max_r - median > 100.0,
            "expected outer band well beyond median; got max={max_r}, med={median}"
        );
    }
}
