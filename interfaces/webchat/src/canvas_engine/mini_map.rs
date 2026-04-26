//! Global minimap: deterministic 2D projection of the full graph.
//!
//! Nodes are placed by hashing their id (angle + radius). Connected components
//! are computed via union-find on the edges and used to color-group nodes.
//! Click hit-testing is exposed via `pick_at`.

use crate::canvas_engine::adapter::{NoteLinkDto, NoteNodeDto};
use crate::canvas_engine::types::{Color, Vec2};
use std::collections::HashMap;
use std::f64::consts::TAU;

#[derive(Clone, Debug)]
pub struct MiniPoint {
    pub id: String,
    pub pos: Vec2,
    pub component: u32,
}

pub struct GlobalMiniMap {
    pub size_px: f32,
    pub points: Vec<MiniPoint>,
    pub component_colors: HashMap<u32, Color>,
}

impl GlobalMiniMap {
    pub fn empty(size_px: f32) -> Self {
        Self { size_px, points: Vec::new(), component_colors: HashMap::new() }
    }

    /// Build a deterministic minimap from full-graph DTOs and edges.
    pub fn build(dtos: &[NoteNodeDto], edges: &[NoteLinkDto], size_px: f32) -> Self {
        let component_of = compute_components(dtos, edges);
        let center = (size_px / 2.0) as f64;
        let max_r = (size_px / 2.0 - 6.0).max(1.0) as f64;

        let mut points = Vec::with_capacity(dtos.len());
        for dto in dtos {
            let h1 = hash_to_unit(&dto.id, 0xA5A5_A5A5);
            let h2 = hash_to_unit(&dto.id, 0x5A5A_5A5A);
            let angle = h1 * TAU;
            let radius = h2.sqrt() * max_r;
            let x = center + radius * angle.cos();
            let y = center + radius * angle.sin();
            let component = component_of.get(&dto.id).copied().unwrap_or(0);
            points.push(MiniPoint {
                id: dto.id.clone(),
                pos: Vec2::new(x, y),
                component,
            });
        }

        let component_colors = assign_component_colors(&points);
        Self { size_px, points, component_colors }
    }

    /// Return the id of the closest node within `hit_radius` of `(mx, my)`,
    /// or `None` if no node is close enough.
    pub fn pick_at(&self, mx: f32, my: f32, hit_radius: f32) -> Option<&str> {
        let mx = mx as f64;
        let my = my as f64;
        let r2 = (hit_radius * hit_radius) as f64;
        let mut best: Option<(&str, f64)> = None;
        for p in &self.points {
            let dx = p.pos.x - mx;
            let dy = p.pos.y - my;
            let d2 = dx * dx + dy * dy;
            if d2 <= r2 && best.map(|(_, b)| d2 < b).unwrap_or(true) {
                best = Some((p.id.as_str(), d2));
            }
        }
        best.map(|(id, _)| id)
    }
}

fn hash_to_unit(s: &str, salt: u64) -> f64 {
    // FNV-1a 64-bit, then map to [0, 1).
    let mut h: u64 = 0xcbf29ce484222325 ^ salt;
    for byte in s.as_bytes() {
        h ^= *byte as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    (h as f64) / (u64::MAX as f64)
}

fn compute_components(dtos: &[NoteNodeDto], edges: &[NoteLinkDto]) -> HashMap<String, u32> {
    let mut idx: HashMap<&str, usize> = HashMap::new();
    for (i, n) in dtos.iter().enumerate() {
        idx.insert(n.id.as_str(), i);
    }
    let mut parent: Vec<usize> = (0..dtos.len()).collect();

    fn find(parent: &mut [usize], x: usize) -> usize {
        let mut root = x;
        while parent[root] != root {
            root = parent[root];
        }
        let mut cur = x;
        while parent[cur] != root {
            let next = parent[cur];
            parent[cur] = root;
            cur = next;
        }
        root
    }

    for e in edges {
        let (Some(&a), Some(&b)) = (idx.get(e.from.as_str()), idx.get(e.to.as_str())) else {
            continue;
        };
        let ra = find(&mut parent, a);
        let rb = find(&mut parent, b);
        if ra != rb {
            parent[ra] = rb;
        }
    }

    let mut out = HashMap::with_capacity(dtos.len());
    let mut roots: HashMap<usize, u32> = HashMap::new();
    let mut next_id: u32 = 0;
    for (i, n) in dtos.iter().enumerate() {
        let root = find(&mut parent, i);
        let cid = *roots.entry(root).or_insert_with(|| {
            let id = next_id;
            next_id += 1;
            id
        });
        out.insert(n.id.clone(), cid);
    }
    out
}

fn assign_component_colors(points: &[MiniPoint]) -> HashMap<u32, Color> {
    let mut comps: Vec<u32> = points.iter().map(|p| p.component).collect();
    comps.sort_unstable();
    comps.dedup();
    let n = comps.len().max(1);
    let mut out = HashMap::with_capacity(n);
    for (i, c) in comps.iter().enumerate() {
        let hue = (i as f32) * 360.0 / (n as f32);
        let (r, g, b) = hsl_to_rgb(hue, 0.55, 0.55);
        out.insert(*c, Color::new(r, g, b));
    }
    out
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (u8, u8, u8) {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let h_prime = h / 60.0;
    let x = c * (1.0 - (h_prime % 2.0 - 1.0).abs());
    let (r1, g1, b1) = match h_prime as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    (
        ((r1 + m) * 255.0).clamp(0.0, 255.0) as u8,
        ((g1 + m) * 255.0).clamp(0.0, 255.0) as u8,
        ((b1 + m) * 255.0).clamp(0.0, 255.0) as u8,
    )
}

#[cfg(target_arch = "wasm32")]
mod render {
    use super::*;
    use web_sys::CanvasRenderingContext2d;

    impl GlobalMiniMap {
        /// Repaint the minimap into `ctx`. The caller is responsible for
        /// clearing the canvas first if needed.
        ///
        /// `focus_id` is the currently centered Radial node; it gets a thicker
        /// outlined dot.
        /// `focus_neighbor_ids` is the 1-hop set; those points are painted
        /// slightly larger so the user can see the local neighborhood.
        pub fn render(
            &self,
            ctx: &CanvasRenderingContext2d,
            focus_id: Option<&str>,
            focus_neighbor_ids: &[String],
        ) {
            let size = self.size_px as f64;
            let half = size / 2.0;

            // Background circle outline
            ctx.set_stroke_style_str("rgba(255,255,255,0.08)");
            ctx.set_line_width(1.0);
            ctx.begin_path();
            let _ = ctx.arc(half, half, half - 2.0, 0.0, std::f64::consts::TAU);
            ctx.stroke();

            // Node points
            for p in &self.points {
                let is_focus = focus_id.map_or(false, |f| f == p.id);
                let is_neighbor = focus_neighbor_ids.iter().any(|n| n == &p.id);
                let radius = if is_focus { 4.0 } else if is_neighbor { 3.0 } else { 1.6 };

                let color = self
                    .component_colors
                    .get(&p.component)
                    .copied()
                    .unwrap_or(Color::new(180, 180, 180));
                ctx.set_fill_style_str(&color.to_css());
                ctx.begin_path();
                let _ = ctx.arc(p.pos.x, p.pos.y, radius, 0.0, std::f64::consts::TAU);
                ctx.fill();

                if is_focus {
                    ctx.set_stroke_style_str("rgba(255,255,255,0.9)");
                    ctx.set_line_width(1.5);
                    ctx.begin_path();
                    let _ = ctx.arc(p.pos.x, p.pos.y, radius + 1.5, 0.0, std::f64::consts::TAU);
                    ctx.stroke();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dto(id: &str) -> NoteNodeDto {
        NoteNodeDto {
            id: id.to_string(),
            name: id.to_string(),
            path: String::new(),
            category: "concept".to_string(),
            tags: vec![],
            link_count: 0,
        }
    }

    fn link(from: &str, to: &str) -> NoteLinkDto {
        NoteLinkDto { from: from.to_string(), to: to.to_string() }
    }

    #[test]
    fn deterministic_layout() {
        let dtos = vec![dto("a"), dto("b"), dto("c")];
        let edges = vec![link("a", "b")];
        let m1 = GlobalMiniMap::build(&dtos, &edges, 200.0);
        let m2 = GlobalMiniMap::build(&dtos, &edges, 200.0);
        for (p1, p2) in m1.points.iter().zip(m2.points.iter()) {
            assert!((p1.pos.x - p2.pos.x).abs() < 1e-9);
            assert!((p1.pos.y - p2.pos.y).abs() < 1e-9);
        }
    }

    #[test]
    fn pick_at_finds_node() {
        let dtos = vec![dto("a"), dto("b")];
        let m = GlobalMiniMap::build(&dtos, &[], 200.0);
        let target = &m.points[0];
        let hit = m.pick_at(target.pos.x as f32, target.pos.y as f32, 5.0);
        assert_eq!(hit, Some(target.id.as_str()));
    }

    #[test]
    fn pick_at_misses_outside_radius() {
        let dtos = vec![dto("a")];
        let m = GlobalMiniMap::build(&dtos, &[], 200.0);
        let hit = m.pick_at(-100.0, -100.0, 3.0);
        assert!(hit.is_none());
    }

    #[test]
    fn connected_components_share_color() {
        let dtos = vec![dto("a"), dto("b"), dto("c")];
        let edges = vec![link("a", "b")];
        let m = GlobalMiniMap::build(&dtos, &edges, 200.0);
        let comp_a = m.points.iter().find(|p| p.id == "a").unwrap().component;
        let comp_b = m.points.iter().find(|p| p.id == "b").unwrap().component;
        let comp_c = m.points.iter().find(|p| p.id == "c").unwrap().component;
        assert_eq!(comp_a, comp_b);
        assert_ne!(comp_a, comp_c);
    }

    #[test]
    fn empty_minimap_has_no_points() {
        let m = GlobalMiniMap::empty(200.0);
        assert_eq!(m.points.len(), 0);
        assert!(m.pick_at(100.0, 100.0, 5.0).is_none());
    }
}
