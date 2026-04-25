use std::collections::HashSet;

use js_sys::Array;
use wasm_bindgen::JsValue;
use web_sys::CanvasRenderingContext2d;

use super::types::*;
use super::viewport::Viewport;

pub struct Renderer;

impl Renderer {
    pub fn draw(
        ctx: &CanvasRenderingContext2d,
        viewport: &Viewport,
        nodes: &[CanvasNode],
        edges: &[CanvasEdge],
        selected: Option<&str>,
        hovered: Option<&str>,
        kind_filter: &HashSet<String>,
        highlighted_neighbors: &HashSet<String>,
    ) {
        // Clear background
        ctx.clear_rect(0.0, 0.0, viewport.width, viewport.height);
        ctx.set_fill_style_str("#0a0a0f");
        ctx.fill_rect(0.0, 0.0, viewport.width, viewport.height);

        ctx.save();
        let _ = ctx.translate(viewport.offset.x, viewport.offset.y);
        let _ = ctx.scale(viewport.scale, viewport.scale);

        Self::draw_edges(ctx, nodes, edges, selected, hovered, kind_filter);
        Self::draw_nodes(
            ctx,
            nodes,
            selected,
            hovered,
            kind_filter,
            highlighted_neighbors,
        );

        ctx.restore();
    }

    fn is_node_visible(node: &CanvasNode, kind_filter: &HashSet<String>) -> bool {
        kind_filter.is_empty() || kind_filter.contains(&node.category)
    }

    fn draw_edges(
        ctx: &CanvasRenderingContext2d,
        nodes: &[CanvasNode],
        edges: &[CanvasEdge],
        selected: Option<&str>,
        hovered: Option<&str>,
        kind_filter: &HashSet<String>,
    ) {
        let solid_dash = Array::new();
        let dashed = Array::new();
        dashed.push(&JsValue::from_f64(5.0));
        dashed.push(&JsValue::from_f64(4.0));

        for edge in edges {
            let from = match nodes.get(edge.from_idx) {
                Some(n) => n,
                None => continue,
            };
            let to = match nodes.get(edge.to_idx) {
                Some(n) => n,
                None => continue,
            };

            if !Self::is_node_visible(from, kind_filter) || !Self::is_node_visible(to, kind_filter)
            {
                continue;
            }

            let is_selected_edge = selected
                .map(|s| from.id == s || to.id == s)
                .unwrap_or(false);
            let is_hovered_edge = hovered.map(|h| from.id == h || to.id == h).unwrap_or(false);
            let is_highlighted = is_selected_edge || is_hovered_edge;

            // Dim edges not connected to the hovered node when hovering
            let is_dimmed = hovered.is_some() && !is_hovered_edge && !is_selected_edge;

            let alpha = if is_highlighted {
                0.7
            } else if is_dimmed {
                0.05
            } else {
                0.2
            };
            let color = if edge.is_wikilink {
                format!("rgba(139,92,246,{alpha})")
            } else {
                format!("rgba(100,116,139,{alpha})")
            };

            ctx.set_stroke_style_str(&color);
            ctx.set_line_width(if is_highlighted { 1.5 } else { 0.8 });

            if edge.is_wikilink {
                let _ = ctx.set_line_dash(&dashed);
            } else {
                let _ = ctx.set_line_dash(&solid_dash);
            }

            ctx.begin_path();
            ctx.move_to(from.position.x, from.position.y);
            ctx.line_to(to.position.x, to.position.y);
            ctx.stroke();

            // Draw relation label at midpoint when the edge is highlighted
            if is_highlighted && !edge.relation.is_empty() {
                let mid_x = (from.position.x + to.position.x) / 2.0;
                let mid_y = (from.position.y + to.position.y) / 2.0;
                ctx.set_fill_style_str("rgba(148,163,184,0.9)");
                ctx.set_font("10px sans-serif");
                ctx.set_text_align("center");
                ctx.set_text_baseline("middle");
                let _ = ctx.fill_text(&edge.relation, mid_x, mid_y);
            }
        }

        // Reset line dash to solid
        let _ = ctx.set_line_dash(&solid_dash);
    }

    fn draw_nodes(
        ctx: &CanvasRenderingContext2d,
        nodes: &[CanvasNode],
        selected: Option<&str>,
        hovered: Option<&str>,
        kind_filter: &HashSet<String>,
        highlighted_neighbors: &HashSet<String>,
    ) {
        use std::f64::consts::TAU;

        for node in nodes {
            if !Self::is_node_visible(node, kind_filter) {
                continue;
            }

            let is_selected = selected.map(|s| s == node.id).unwrap_or(false);
            let is_hovered = hovered.map(|h| h == node.id).unwrap_or(false);

            // Dim nodes that are not the hovered node, not selected, and not a neighbor
            let is_dimmed = hovered.is_some()
                && !is_hovered
                && !is_selected
                && !highlighted_neighbors.contains(&node.id);

            let x = node.position.x;
            let y = node.position.y;
            let r = node.radius;

            // Glow (larger, semi-transparent circle behind the dot)
            let glow_alpha = if is_dimmed {
                0.05
            } else if is_selected {
                0.5
            } else if is_hovered {
                0.35
            } else {
                0.15
            };
            let glow_radius = r + if is_selected || is_hovered { 6.0 } else { 3.0 };
            ctx.set_fill_style_str(&node.color.to_css_alpha(glow_alpha));
            ctx.begin_path();
            let _ = ctx.arc(x, y, glow_radius, 0.0, TAU);
            ctx.fill();

            // Main dot
            let dot_alpha = if is_dimmed {
                0.2
            } else if is_selected {
                1.0
            } else {
                0.85
            };
            ctx.set_fill_style_str(&node.color.to_css_alpha(dot_alpha));
            ctx.begin_path();
            let _ = ctx.arc(x, y, r, 0.0, TAU);
            ctx.fill();

            // Label — title is the star
            let label_color = if is_dimmed {
                "rgba(148,163,184,0.2)"
            } else if is_selected || is_hovered {
                "rgba(226,232,240,1.0)"
            } else {
                "rgba(148,163,184,0.85)"
            };
            ctx.set_fill_style_str(label_color);
            let font_size = if is_selected || is_hovered {
                12.0
            } else {
                11.0
            };
            ctx.set_font(&format!("{font_size}px sans-serif"));
            ctx.set_text_align("center");
            ctx.set_text_baseline("top");

            // Truncate with char_indices for UTF-8 safety
            let label = if node.name.chars().count() > 20 {
                let truncated: String = node.name.chars().take(19).collect();
                format!("{truncated}\u{2026}")
            } else {
                node.name.clone()
            };
            let _ = ctx.fill_text(&label, x, y + r + 6.0);
        }
    }
}

// ---------------------------------------------------------------------------
// Depth attribute computation
// ---------------------------------------------------------------------------

/// Compute per-layer visual modifiers based on Z depth.
/// Z=0 → full brightness (active layer); Z=200 → maximum dimming.
pub fn depth_attrs(z: f32) -> DepthAttrs {
    let t = (z / 200.0).clamp(0.0, 1.0);
    DepthAttrs {
        scale: 1.0 - 0.30 * t,
        opacity: 1.0 - 0.45 * t,
        blur_px: 4.0 * t,
        sat_mul: 1.0 - 0.40 * t,
        glow_alpha: (1.0 - t) * 0.6,
        shadow_offset_y: 6.0 + 4.0 * (1.0 - t),
    }
}

#[cfg(test)]
mod depth_tests {
    use super::*;

    #[test]
    fn active_layer_full_brightness() {
        let a = depth_attrs(0.0);
        assert!((a.scale - 1.0).abs() < 1e-6);
        assert!((a.opacity - 1.0).abs() < 1e-6);
        assert!((a.blur_px - 0.0).abs() < 1e-6);
    }

    #[test]
    fn far_layer_dimmed() {
        let a = depth_attrs(200.0);
        assert!((a.scale - 0.7).abs() < 1e-3);
        assert!((a.opacity - 0.55).abs() < 1e-3);
        assert!((a.blur_px - 4.0).abs() < 1e-3);
    }

    #[test]
    fn beyond_z_clamps() {
        let a = depth_attrs(500.0);
        assert!((a.scale - 0.7).abs() < 1e-3);
    }
}

// ---------------------------------------------------------------------------
// Z-layered neighborhood renderer (used by T22 wiring)
// ---------------------------------------------------------------------------

/// Render a full neighborhood back-to-front (2-hop → clusters → 1-hop → active).
pub fn draw_neighborhood(
    ctx: &CanvasRenderingContext2d,
    viewport: &Viewport,
    nbhd: &Neighborhood,
    drag: (f32, f32),
    selected: Option<&str>,
    hovered: Option<&str>,
) {
    // 1. Clear + bg gradient
    paint_background(ctx, viewport);
    ctx.save();
    let _ = ctx.translate(viewport.offset.x, viewport.offset.y);
    let _ = ctx.scale(viewport.scale, viewport.scale);

    // 2. Layer A: 2-hop (back)
    for n in &nbhd.two_hop {
        draw_edges_for_node(ctx, n, nbhd, drag);
    }
    for n in &nbhd.two_hop {
        draw_node(ctx, n, drag, selected, hovered);
    }

    // 3. Layer B: 1-hop + clusters
    for c in &nbhd.clusters {
        draw_cluster(ctx, c, drag, selected, hovered);
    }
    for n in &nbhd.one_hop {
        draw_edges_for_node(ctx, n, nbhd, drag);
    }
    for n in &nbhd.one_hop {
        draw_node(ctx, n, drag, selected, hovered);
    }

    // 4. Layer C: Active (front)
    draw_node(ctx, &nbhd.center, drag, selected, hovered);

    ctx.restore();
}

fn paint_background(ctx: &CanvasRenderingContext2d, viewport: &Viewport) {
    // Solid dark fill; CanvasGradient feature not enabled in this build.
    // T14/T15 can add a proper radial gradient once the feature is wired.
    ctx.set_fill_style_str("#080818");
    ctx.fill_rect(0.0, 0.0, viewport.width, viewport.height);
}

// Stubs — filled in by Tasks 14 (edges) and 15 (nodes/clusters)
fn draw_node(
    _ctx: &CanvasRenderingContext2d,
    _n: &CanvasNode,
    _drag: (f32, f32),
    _selected: Option<&str>,
    _hovered: Option<&str>,
) {
}

fn draw_cluster(
    _ctx: &CanvasRenderingContext2d,
    _c: &ClusterNode,
    _drag: (f32, f32),
    _selected: Option<&str>,
    _hovered: Option<&str>,
) {
}

fn draw_edges_for_node(
    ctx: &CanvasRenderingContext2d,
    n: &CanvasNode,
    nbhd: &Neighborhood,
    drag: (f32, f32),
) {
    for e in &nbhd.edges {
        let endpoints = endpoints_world_pos(e, nbhd, drag);
        let (from_pos, to_pos, from_z, to_z) = match endpoints {
            Some(t) => t,
            None => continue,
        };
        // Draw each edge only once — from_idx < to_idx convention
        if e.from_idx >= e.to_idx {
            continue;
        }
        // Only draw edges that involve this node (by index in the neighborhood)
        let n_idx = nbhd
            .one_hop
            .iter()
            .position(|x| x.id == n.id)
            .map(|i| i + 1)
            .or_else(|| {
                nbhd.two_hop
                    .iter()
                    .position(|x| x.id == n.id)
                    .map(|i| i + 1 + nbhd.one_hop.len())
            })
            .unwrap_or(0);
        if e.from_idx != n_idx && e.to_idx != n_idx {
            continue;
        }

        let attrs_from = depth_attrs(from_z);
        let attrs_to = depth_attrs(to_z);
        let stroke_alpha = if e.is_active_link { 0.85_f32 } else { 0.25_f32 };

        if e.is_wikilink {
            let dashes = js_sys::Array::new();
            dashes.push(&JsValue::from_f64(5.0));
            dashes.push(&JsValue::from_f64(4.0));
            let _ = ctx.set_line_dash(&dashes);
        } else {
            let solid = js_sys::Array::new();
            let _ = ctx.set_line_dash(&solid);
        }

        // Bezier control point: perpendicular offset from midpoint
        let mid_x = (from_pos.0 + to_pos.0) * 0.5;
        let mid_y = (from_pos.1 + to_pos.1) * 0.5;
        let dx = to_pos.0 - from_pos.0;
        let dy = to_pos.1 - from_pos.1;
        let len = (dx * dx + dy * dy).sqrt().max(1.0);
        let nx = -dy / len; // perpendicular unit vector
        let ny = dx / len;
        let curve_amt = 30.0_f32;
        let cx = mid_x + nx * curve_amt;
        let cy = mid_y + ny * curve_amt;

        let grad = ctx.create_linear_gradient(
            from_pos.0 as f64,
            from_pos.1 as f64,
            to_pos.0 as f64,
            to_pos.1 as f64,
        );
        if e.is_active_link {
            let _ = grad.add_color_stop(
                0.0,
                &format!("rgba(167,139,250,{:.3})", stroke_alpha * attrs_from.opacity),
            );
            let _ = grad.add_color_stop(
                1.0,
                &format!("rgba(76,29,149,{:.3})", stroke_alpha * attrs_to.opacity),
            );
        } else {
            let _ = grad.add_color_stop(
                0.0,
                &format!("rgba(107,107,138,{:.3})", stroke_alpha * attrs_from.opacity),
            );
            let _ = grad.add_color_stop(
                1.0,
                &format!("rgba(42,42,58,{:.3})", stroke_alpha * attrs_to.opacity),
            );
        }

        let avg_w = if e.is_active_link { (2.5 + 1.0) * 0.5 } else { (1.5 + 0.8) * 0.5 };

        ctx.set_stroke_style_canvas_gradient(&grad);
        ctx.set_line_width(avg_w as f64);
        ctx.begin_path();
        ctx.move_to(from_pos.0 as f64, from_pos.1 as f64);
        ctx.quadratic_curve_to(cx as f64, cy as f64, to_pos.0 as f64, to_pos.1 as f64);
        ctx.stroke();
    }
}

fn endpoints_world_pos(
    e: &CanvasEdge,
    nbhd: &Neighborhood,
    drag: (f32, f32),
) -> Option<((f32, f32), (f32, f32), f32, f32)> {
    let resolve = |idx: usize| -> Option<Vec3> {
        if idx == 0 {
            nbhd.target_positions.get(&nbhd.center.id).copied()
        } else if idx <= nbhd.one_hop.len() {
            let n = &nbhd.one_hop[idx - 1];
            nbhd.target_positions.get(&n.id).copied()
        } else {
            let off = idx - 1 - nbhd.one_hop.len();
            let n = nbhd.two_hop.get(off)?;
            nbhd.target_positions.get(&n.id).copied()
        }
    };
    let p1 = resolve(e.from_idx)?;
    let p2 = resolve(e.to_idx)?;
    let off1 = crate::canvas_engine::viewport::parallax_offset(p1.z, drag.0, drag.1);
    let off2 = crate::canvas_engine::viewport::parallax_offset(p2.z, drag.0, drag.1);
    Some((
        (p1.x + off1.0, p1.y + off1.1),
        (p2.x + off2.0, p2.y + off2.1),
        p1.z,
        p2.z,
    ))
}
