//! Screen-space picking: project nodes, return nearest within radius. Pure.

use super::math::{Mat4, Vec3};

/// Concentric bands the pick radius is divided into, EQUAL IN PIXELS (so band `k`
/// spans `[k, k+1) * radius / PICK_RINGS` — banding on the squared distance would
/// make the innermost band swallow half the radius). Candidates in the same band
/// count as equally close, and depth decides between them. At the default 18 px
/// radius a band is 4.5 px: tight enough that a clearly-centred star still beats
/// an edge-of-tolerance one, loose enough that the depth term is actually reached.
const PICK_RINGS: u32 = 4;

/// Pick the node nearest the cursor within `radius_px`.
///
/// `pos` are the RENDERED positions (canonical position + `drift::idle_drift`),
/// not the canonical ones — the GPU displaces every node in the vertex shader,
/// so picking the canonical position misses the star the user is looking at.
/// Indices are into `pos`, which the caller keeps aligned with its node list.
///
/// Overlapping candidates are resolved by RING first, then by depth: the pick
/// radius is divided into [`PICK_RINGS`] concentric bands, and among candidates
/// in the same band the frontmost (smallest NDC z) wins.
///
/// The banding is what makes the depth term real. Comparing raw `(dist², z)`
/// pairs looks like "nearest, then frontmost", but two f32 screen distances are
/// essentially never bit-identical — so depth would only ever be consulted on an
/// exact tie, i.e. never, and a background star sitting one pixel closer to the
/// cursor would steal the click from the bright foreground star drawn on top of
/// it. Bands make "about as close" mean something, and let depth break it.
pub fn pick_node(
    view_proj: &Mat4,
    pos: &[Vec3],
    viewport: (f32, f32),
    cursor: (f32, f32),
    radius_px: f32,
) -> Option<u32> {
    let m = view_proj.as_slice();
    let radius = radius_px.max(f32::MIN_POSITIVE);
    let r2 = radius * radius;
    let mut best: Option<(u32, u32, f32)> = None; // (idx, ring, ndc_z)
    for (i, p) in pos.iter().enumerate() {
        let cx = m[0] * p.x + m[4] * p.y + m[8] * p.z + m[12];
        let cy = m[1] * p.x + m[5] * p.y + m[9] * p.z + m[13];
        let cz = m[2] * p.x + m[6] * p.y + m[10] * p.z + m[14];
        let cw = m[3] * p.x + m[7] * p.y + m[11] * p.z + m[15];
        if cw <= 0.0 {
            continue; // behind camera
        }
        let ndc_x = cx / cw;
        let ndc_y = cy / cw;
        let ndc_z = cz / cw;
        let sx = (ndc_x * 0.5 + 0.5) * viewport.0;
        let sy = (1.0 - (ndc_y * 0.5 + 0.5)) * viewport.1;
        let dx = sx - cursor.0;
        let dy = sy - cursor.1;
        let d2 = dx * dx + dy * dy;
        if d2 <= r2 {
            let ring = ((d2.sqrt() / radius) * PICK_RINGS as f32) as u32;
            let better = match best {
                None => true,
                Some((_, bring, bz)) => (ring, ndc_z) < (bring, bz),
            };
            if better {
                best = Some((i as u32, ring, ndc_z));
            }
        }
    }
    best.map(|(i, _, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::memory::galaxy::gl::math::{Mat4, Vec3};

    fn front_vp() -> Mat4 {
        Mat4::perspective(1.0, 1.0, 0.1, 1000.0).mul(&Mat4::look_at(
            Vec3::new(0.0, 0.0, 300.0),
            Vec3::zero(),
            Vec3::new(0.0, 1.0, 0.0),
        ))
    }

    #[test]
    fn picks_node_under_cursor() {
        let vp = front_vp();
        let pos = vec![Vec3::zero(), Vec3::new(200.0, 0.0, 0.0)];
        // Center node projects to screen center (400,300) on an 800x600 viewport.
        let hit = pick_node(&vp, &pos, (800.0, 600.0), (400.0, 300.0), 20.0);
        assert_eq!(hit, Some(0));
    }

    #[test]
    fn returns_none_when_far() {
        let vp = front_vp();
        let pos = vec![Vec3::zero()];
        let hit = pick_node(&vp, &pos, (800.0, 600.0), (10.0, 10.0), 20.0);
        assert_eq!(hit, None);
    }

    #[test]
    fn nearest_candidate_wins_tie() {
        let vp = front_vp();
        // Both discs cover the cursor at screen centre; node 1 sits dead centre,
        // node 0 a few pixels off — far enough to land in an OUTER ring. The
        // nearer-the-cursor node must win even though node 0 is nearer the camera.
        let pos = vec![Vec3::new(6.0, 0.0, 40.0), Vec3::zero()];
        let hit = pick_node(&vp, &pos, (800.0, 600.0), (400.0, 300.0), 40.0);
        assert_eq!(hit, Some(1));
    }

    #[test]
    fn frontmost_wins_within_the_same_ring() {
        let vp = front_vp();
        // Two stars projecting within a fraction of a pixel of each other (and of
        // the cursor) — the same ring — but at very different depths. The one in
        // FRONT is the one the user can see, so it must take the click.
        //
        // This is the case the old `(dist², z)` rule could never handle: the two
        // squared distances differ in the last f32 bits, so it silently degraded
        // to "nearest, full stop" and the depth term was dead code.
        let pos = vec![
            Vec3::new(0.0, 0.0, -250.0), // far behind the origin (bigger NDC z)
            Vec3::new(0.0, 0.0, 100.0),  // in front, toward the camera at z=300
        ];
        let hit = pick_node(&vp, &pos, (800.0, 600.0), (400.0, 300.0), 18.0);
        assert_eq!(hit, Some(1), "the frontmost of two overlapping stars wins");
    }
}
