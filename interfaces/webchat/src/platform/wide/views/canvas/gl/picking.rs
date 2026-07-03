//! Screen-space picking: project nodes, return nearest within radius. Pure.

use super::math::Mat4;
use super::GalaxyNode;

pub fn pick_node(
    view_proj: &Mat4,
    nodes: &[GalaxyNode],
    viewport: (f32, f32),
    cursor: (f32, f32),
    radius_px: f32,
) -> Option<u32> {
    let m = view_proj.as_slice();
    let mut best: Option<(u32, f32, f32)> = None; // (idx, dist2, ndc_z)
    for (i, node) in nodes.iter().enumerate() {
        let p = &node.pos;
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
        if d2 <= radius_px * radius_px {
            match best {
                Some((_, _, bz)) if ndc_z >= bz => {}
                _ => best = Some((i as u32, d2, ndc_z)),
            }
        }
    }
    best.map(|(i, _, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::canvas::gl::math::{Mat4, Vec3};
    use crate::views::canvas::gl::GalaxyNode;

    fn node_at(x: f32, y: f32, z: f32) -> GalaxyNode {
        GalaxyNode {
            id: "n".into(),
            name: "n".into(),
            category: "c".into(),
            link_count: 0,
            pos: Vec3::new(x, y, z),
            color: [1.0, 1.0, 1.0],
            community: None,
        }
    }

    #[test]
    fn picks_node_under_cursor() {
        let vp = Mat4::perspective(1.0, 1.0, 0.1, 1000.0).mul(&Mat4::look_at(
            Vec3::new(0.0, 0.0, 300.0),
            Vec3::zero(),
            Vec3::new(0.0, 1.0, 0.0),
        ));
        let nodes = vec![node_at(0.0, 0.0, 0.0), node_at(200.0, 0.0, 0.0)];
        // Center node projects to screen center (400,300) on an 800x600 viewport.
        let hit = pick_node(&vp, &nodes, (800.0, 600.0), (400.0, 300.0), 20.0);
        assert_eq!(hit, Some(0));
    }

    #[test]
    fn returns_none_when_far() {
        let vp = Mat4::perspective(1.0, 1.0, 0.1, 1000.0).mul(&Mat4::look_at(
            Vec3::new(0.0, 0.0, 300.0),
            Vec3::zero(),
            Vec3::new(0.0, 1.0, 0.0),
        ));
        let nodes = vec![node_at(0.0, 0.0, 0.0)];
        let hit = pick_node(&vp, &nodes, (800.0, 600.0), (10.0, 10.0), 20.0);
        assert_eq!(hit, None);
    }
}
