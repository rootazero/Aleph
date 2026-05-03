use std::collections::HashSet;

use js_sys::Array;
use wasm_bindgen::JsValue;
use web_sys::CanvasRenderingContext2d;

use super::types::*;
use super::viewport::Viewport;
use crate::canvas_engine::drag::DragOverlay;

/// Idle drift amplitude (pixels). Each node oscillates within this radius around its
/// resolved (target) position. Spec §6.1, Q4 = "B / breathing" feel.
pub(crate) const DRIFT_AMPLITUDE_PX: f32 = 5.0;
/// Idle drift period in ms (one full oscillation).
pub(crate) const DRIFT_PERIOD_MS: f32 = 5000.0;

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
// Idle drift (per-node visual wobble)
// ---------------------------------------------------------------------------

/// FNV-1a 32-bit. Stable, allocation-free; used for per-node phase derivation.
fn fnv1a_32_drift(bytes: &[u8]) -> u32 {
    let mut h = 0x811c9dc5_u32;
    for &b in bytes {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// Pure visual drift for a node. Two independent sine components (x, y) using
/// the same period but different phases derived from `node_id` so adjacent nodes
/// move out of sync. Returns an offset to add to the resolved world position
/// before drawing. Never writes to node state.
pub(crate) fn drift_offset(t_ms: f64, node_id: &str, amplitude_px: f32, period_ms: f32) -> Vec2 {
    let phase = fnv1a_32_drift(node_id.as_bytes()) as f32 / u32::MAX as f32; // [0, 1)
    let omega = std::f32::consts::TAU / (period_ms / 1000.0);
    let t = (t_ms as f32) / 1000.0;
    let x = amplitude_px * (omega * t + phase * std::f32::consts::TAU).sin();
    // Use a different phase (+0.27 of full revolution) for y so the motion is not a straight line.
    let y = amplitude_px * (omega * t + (phase + 0.27) * std::f32::consts::TAU).sin();
    Vec2::new(x as f64, y as f64)
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

/// Render a full neighborhood back-to-front (orphans → 2-hop → clusters → 1-hop → active).
pub fn draw_neighborhood(
    ctx: &CanvasRenderingContext2d,
    viewport: &Viewport,
    nbhd: &Neighborhood,
    drag: (f32, f32),
    selected: Option<&str>,
    hovered: Option<&str>,
    node_drag: Option<&DragOverlay>,
) {
    // 1. Clear + bg gradient
    paint_background(ctx, viewport);
    ctx.save();
    let _ = ctx.translate(viewport.offset.x, viewport.offset.y);
    let _ = ctx.scale(viewport.scale, viewport.scale);

    // 2. Layer 0: Orphans (deepest, behind everything else).
    //    Ghost-dot ring around R=ORPHAN_RADIUS for nodes outside the current
    //    connected component. Click to re-center.
    draw_orphan_ring(ctx, &nbhd.orphans, drag, selected, hovered);

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
        // Skip edges for the dragged node — we'll draw a stretched edge instead.
        if node_drag.map(|o| o.node_id == n.id).unwrap_or(false) {
            continue;
        }
        draw_edges_for_node(ctx, n, nbhd, drag);
    }
    for n in &nbhd.one_hop {
        if node_drag.map(|o| o.node_id == n.id).unwrap_or(false) {
            continue;
        }
        draw_node(ctx, n, drag, selected, hovered);
    }

    // Draw the dragged node last (on top of clusters / other 1-hop nodes)
    // with a stretched edge from the center and a glow ring when promote-imminent.
    if let Some(overlay) = node_drag {
        if let Some(n) = nbhd.one_hop.iter().find(|x| x.id == overlay.node_id) {
            draw_dragged_node(ctx, n, &nbhd.center, overlay);
        }
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

/// Draw the ghost-dot ring at `R = ORPHAN_RADIUS`. Each orphan is a small dim
/// dot — no glow, no shadow, no label unless hovered/selected. Hovering brightens
/// and reveals the label so the user can tell what they're about to recenter on.
fn draw_orphan_ring(
    ctx: &CanvasRenderingContext2d,
    orphans: &[CanvasNode],
    drag: (f32, f32),
    selected: Option<&str>,
    hovered: Option<&str>,
) {
    use std::f64::consts::TAU;
    if orphans.is_empty() {
        return;
    }

    // Subtle hint ring so users see there's "more out there" even before pointing
    // at a dot. Drawn first so dots sit on top of it.
    ctx.set_stroke_style_str("rgba(167,139,250,0.06)");
    ctx.set_line_width(1.0);
    ctx.begin_path();
    let _ = ctx.arc(0.0, 0.0, ORPHAN_RADIUS as f64, 0.0, TAU);
    ctx.stroke();

    for n in orphans {
        let off = crate::canvas_engine::viewport::parallax_offset(n.z, drag.0, drag.1);
        let cx = n.position.x + off.0 as f64;
        let cy = n.position.y + off.1 as f64;
        let r = n.radius;

        let is_selected = selected.map(|s| s == n.id).unwrap_or(false);
        let is_hovered = hovered.map(|h| h == n.id).unwrap_or(false);

        // Dim by default, brighten on hover/selection.
        let alpha = if is_selected {
            0.95
        } else if is_hovered {
            0.75
        } else {
            0.30
        };
        ctx.set_fill_style_str(&n.color.to_css_alpha(alpha));
        ctx.begin_path();
        let _ = ctx.arc(cx, cy, r, 0.0, TAU);
        ctx.fill();

        if is_hovered || is_selected {
            // Reveal label only when the user is targeting this orphan.
            let max_chars = 24;
            let label = if n.name.chars().count() > max_chars {
                let truncated: String = n.name.chars().take(max_chars - 1).collect();
                format!("{truncated}\u{2026}")
            } else {
                n.name.clone()
            };
            ctx.set_fill_style_str("rgba(226,232,240,0.95)");
            ctx.set_font("11px sans-serif");
            ctx.set_text_align("center");
            ctx.set_text_baseline("top");
            let _ = ctx.fill_text(&label, cx, cy + r + 4.0);
        }
    }
}

fn draw_node(
    ctx: &CanvasRenderingContext2d,
    n: &CanvasNode,
    drag: (f32, f32),
    selected: Option<&str>,
    hovered: Option<&str>,
) {
    use std::f64::consts::TAU;

    let attrs = depth_attrs(n.z);
    let off = crate::canvas_engine::viewport::parallax_offset(n.z, drag.0, drag.1);
    let cx = n.position.x as f32 + off.0;
    let cy = n.position.y as f32 + off.1;
    let r = (n.radius as f32 * attrs.scale) as f64;
    let cx64 = cx as f64;
    let cy64 = cy as f64;

    let is_selected = selected.map(|s| s == n.id).unwrap_or(false);
    let is_hovered = hovered.map(|h| h == n.id).unwrap_or(false);
    let is_active = n.hop == 0;

    // 1. Shadow ellipse (depth-attenuated, Y-offset varies with depth)
    let shadow_y = cy64 + attrs.shadow_offset_y as f64;
    let shadow_alpha = 0.3 * attrs.opacity as f64;
    ctx.set_fill_style_str(&format!("rgba(0,0,0,{shadow_alpha:.3})"));
    ctx.begin_path();
    let _ = ctx.ellipse(cx64, shadow_y, r * 1.1, r * 0.4, 0.0, 0.0, TAU);
    ctx.fill();

    // 2. Glow ring
    //    Active centre: breathing animation (sin-based pulse)
    //    Hovered/selected: static bright glow
    let glow_alpha = if is_active {
        let t = now_ms_in_seconds();
        let breath = 0.85 + 0.15 * (t * std::f64::consts::TAU / 2.5).sin();
        (attrs.glow_alpha as f64 * breath).clamp(0.0, 1.0)
    } else if is_hovered || is_selected {
        (attrs.glow_alpha as f64 * 1.4).clamp(0.0, 1.0)
    } else {
        attrs.glow_alpha as f64
    };

    if glow_alpha > 0.01 {
        let glow_r = r + if is_selected || is_hovered { 8.0 } else { 4.0 };
        let grad = ctx.create_radial_gradient(cx64, cy64, r * 0.5, cx64, cy64, glow_r);
        let glow_color_inner = format!(
            "rgba({},{},{},{:.3})",
            n.color.r, n.color.g, n.color.b, glow_alpha
        );
        let glow_color_outer = format!("rgba({},{},{},0.0)", n.color.r, n.color.g, n.color.b);
        if let Ok(grad) = grad {
            let _ = grad.add_color_stop(0.0, &glow_color_inner);
            let _ = grad.add_color_stop(1.0, &glow_color_outer);
            ctx.set_fill_style_canvas_gradient(&grad);
            ctx.begin_path();
            let _ = ctx.arc(cx64, cy64, glow_r, 0.0, TAU);
            ctx.fill();
        }
    }

    // 3. Blur filter for 2-hop nodes
    if attrs.blur_px > 0.5 {
        ctx.set_filter(&format!("blur({:.1}px)", attrs.blur_px));
    }

    // 4. Body fill — saturation-attenuated by depth
    let body_color = scale_color_saturation(&n.color, attrs.sat_mul, attrs.opacity);
    ctx.set_fill_style_str(&body_color);
    ctx.begin_path();
    let _ = ctx.arc(cx64, cy64, r, 0.0, TAU);
    ctx.fill();

    // 5. Border
    ctx.set_stroke_style_str("#1a1a2a");
    ctx.set_line_width(1.0);
    ctx.stroke();

    // 6. Reset blur filter
    if attrs.blur_px > 0.5 {
        ctx.set_filter("none");
    }

    // 7. Icon — skipped (CanvasNode has no icon field; future enhancement)

    // 8. Wiki badge — skipped (CanvasNode has no has_wiki field; future enhancement)

    // 9. Label — show by hop layer
    //    hop=0 (active center): always show, larger font
    //    hop=1 (one-hop neighbors): always show
    //    hop=2 (two-hop): only when node is wide enough to read
    let show_label = n.hop <= 1 || r >= 12.0 || is_selected || is_hovered;
    if show_label {
        let max_chars = if is_active { 28 } else { 20 };
        let label = if n.name.chars().count() > max_chars {
            let truncated: String = n.name.chars().take(max_chars - 1).collect();
            format!("{truncated}\u{2026}")
        } else {
            n.name.clone()
        };

        let label_alpha = if is_selected || is_hovered || is_active {
            (attrs.opacity as f64).clamp(0.0, 1.0)
        } else {
            (attrs.opacity as f64 * 0.9).clamp(0.0, 1.0)
        };
        let font_size = if is_active {
            13.0
        } else if is_selected || is_hovered {
            12.0
        } else if n.hop == 1 {
            11.0
        } else {
            10.0
        };
        ctx.set_fill_style_str(&format!("rgba(226,232,240,{label_alpha:.3})"));
        ctx.set_font(&format!("{font_size}px sans-serif"));
        ctx.set_text_align("center");
        ctx.set_text_baseline("top");
        let _ = ctx.fill_text(&label, cx64, cy64 + r + 5.0);
    }
}

fn draw_cluster(
    ctx: &CanvasRenderingContext2d,
    c: &ClusterNode,
    drag: (f32, f32),
    selected: Option<&str>,
    hovered: Option<&str>,
) {
    let attrs = depth_attrs(c.z);
    let off = crate::canvas_engine::viewport::parallax_offset(c.z, drag.0, drag.1);
    let cx = (c.world_pos.x as f32 + off.0) as f64;
    let cy = (c.world_pos.y as f32 + off.1) as f64;
    let r = (c.radius * attrs.scale) as f64;

    // Bounding box: 2.4r wide, 1.6r tall
    let w = r * 2.4;
    let h = r * 1.6;
    let x = cx - w / 2.0;
    let y = cy - h / 2.0;
    let corner_r = 8.0_f64;

    let is_selected = selected.map(|s| s == c.id).unwrap_or(false);
    let is_hovered = hovered.map(|h| h == c.id).unwrap_or(false);

    // 1. Shadow ellipse
    let shadow_alpha = 0.25 * attrs.opacity as f64;
    ctx.set_fill_style_str(&format!("rgba(0,0,0,{shadow_alpha:.3})"));
    ctx.begin_path();
    let shadow_y = cy + attrs.shadow_offset_y as f64;
    let _ = ctx.ellipse(
        cx,
        shadow_y,
        w * 0.5,
        h * 0.25,
        0.0,
        0.0,
        std::f64::consts::TAU,
    );
    ctx.fill();

    // 2. Body: rounded rect with purple tint
    let body_alpha = 0.30 * attrs.opacity as f64;
    ctx.set_fill_style_str(&format!("rgba(124,58,237,{body_alpha:.3})"));
    ctx.begin_path();
    rounded_rect(ctx, x, y, w, h, corner_r);
    ctx.fill();

    // 3. Stroke
    let stroke_alpha = attrs.opacity as f64;
    ctx.set_stroke_style_str(&format!("rgba(167,139,250,{stroke_alpha:.3})"));
    ctx.set_line_width(2.0);
    ctx.begin_path();
    rounded_rect(ctx, x, y, w, h, corner_r);
    ctx.stroke();

    // 4. Highlight outline if hovered or selected
    if is_hovered || is_selected {
        let pad = 3.0;
        ctx.set_stroke_style_str("rgba(255,255,255,0.7)");
        ctx.set_line_width(1.5);
        ctx.begin_path();
        rounded_rect(
            ctx,
            x - pad,
            y - pad,
            w + pad * 2.0,
            h + pad * 2.0,
            corner_r + pad,
        );
        ctx.stroke();
    }

    // 5. Label: "📚 +N kind" centered
    let count = c.member_ids.len();
    let label = format!("\u{1F4DA} +{count} {}", c.kind);
    let label_alpha = attrs.opacity as f64;
    ctx.set_fill_style_str(&format!("rgba(226,232,240,{label_alpha:.3})"));
    ctx.set_font("11px sans-serif");
    ctx.set_text_align("center");
    ctx.set_text_baseline("middle");
    let _ = ctx.fill_text(&label, cx, cy);
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

        let avg_w = if e.is_active_link {
            (2.5 + 1.0) * 0.5
        } else {
            (1.5 + 0.8) * 0.5
        };

        ctx.set_stroke_style_canvas_gradient(&grad);
        ctx.set_line_width(avg_w as f64);
        ctx.begin_path();
        ctx.move_to(from_pos.0 as f64, from_pos.1 as f64);
        ctx.quadratic_curve_to(cx as f64, cy as f64, to_pos.0 as f64, to_pos.1 as f64);
        ctx.stroke();
    }
}

/// Return current time in seconds using `performance.now()`.
fn now_ms_in_seconds() -> f64 {
    web_sys::window()
        .and_then(|w| w.performance())
        .map(|p| p.now() / 1000.0)
        .unwrap_or(0.0)
}

/// Build a CSS rgba() string with saturation attenuated by `sat_mul` and given opacity.
/// Approach: lerp each channel toward its luminance (desaturate) then apply opacity.
fn scale_color_saturation(c: &Color, sat_mul: f32, opacity: f32) -> String {
    // Perceived luminance weights (BT.601)
    let lum = 0.299 * c.r as f32 + 0.587 * c.g as f32 + 0.114 * c.b as f32;
    let r = (lum + (c.r as f32 - lum) * sat_mul).clamp(0.0, 255.0) as u8;
    let g = (lum + (c.g as f32 - lum) * sat_mul).clamp(0.0, 255.0) as u8;
    let b = (lum + (c.b as f32 - lum) * sat_mul).clamp(0.0, 255.0) as u8;
    format!("rgba({r},{g},{b},{opacity:.3})")
}

/// Stroke a rounded rectangle path onto `ctx`.
/// Caller is responsible for calling `fill()` or `stroke()` afterward.
fn rounded_rect(ctx: &CanvasRenderingContext2d, x: f64, y: f64, w: f64, h: f64, r: f64) {
    // Clamp corner radius so it never exceeds half the shorter side
    let r = r.min(w / 2.0).min(h / 2.0);
    ctx.move_to(x + r, y);
    ctx.line_to(x + w - r, y);
    let _ = ctx.arc_to(x + w, y, x + w, y + r, r);
    ctx.line_to(x + w, y + h - r);
    let _ = ctx.arc_to(x + w, y + h, x + w - r, y + h, r);
    ctx.line_to(x + r, y + h);
    let _ = ctx.arc_to(x, y + h, x, y + h - r, r);
    ctx.line_to(x, y + r);
    let _ = ctx.arc_to(x, y, x + r, y, r);
    ctx.close_path();
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

fn draw_dragged_node(
    ctx: &CanvasRenderingContext2d,
    n: &CanvasNode,
    center: &CanvasNode,
    overlay: &DragOverlay,
) {
    use std::f64::consts::TAU;

    // overlay.position is in world space (Task 5 contract). The canvas context
    // already has world transform applied by draw_neighborhood, so draw directly.
    let cx = overlay.position.x;
    let cy = overlay.position.y;

    // Stretched edge: center → dragged node
    ctx.set_stroke_style_str("rgba(167,139,250,0.6)");
    ctx.set_line_width(1.5);
    ctx.begin_path();
    ctx.move_to(center.position.x, center.position.y);
    ctx.line_to(cx, cy);
    let _ = ctx.stroke();

    // Node body — solid violet, slight lift over base color
    let r = n.radius;
    ctx.set_fill_style_str("#a78bfa");
    ctx.begin_path();
    let _ = ctx.arc(cx, cy, r, 0.0, TAU);
    ctx.fill();

    // Glow ring (only when promote-imminent — alpha grows as overlay nears center)
    if overlay.glow_alpha > 0.01 {
        ctx.set_stroke_style_str(&format!("rgba(252,211,77,{:.3})", overlay.glow_alpha));
        ctx.set_line_width(3.0);
        ctx.begin_path();
        let _ = ctx.arc(cx, cy, r + 6.0, 0.0, TAU);
        let _ = ctx.stroke();
    }
}

#[cfg(test)]
mod drift_tests {
    use super::*;

    #[test]
    fn drift_is_periodic_within_epsilon() {
        let id = "node-abc";
        let amp = 5.0_f32;
        let period = 5000.0_f32;
        let a = drift_offset(0.0, id, amp, period);
        let b = drift_offset(period as f64, id, amp, period);
        assert!((a.x - b.x).abs() < 1e-3, "x not periodic: {} vs {}", a.x, b.x);
        assert!((a.y - b.y).abs() < 1e-3, "y not periodic: {} vs {}", a.y, b.y);
    }

    #[test]
    fn drift_amplitude_is_bounded() {
        let amp = 5.0_f32;
        let period = 5000.0_f32;
        for &id in &["a", "b", "long-id-with-dashes", ""] {
            for step in 0..200 {
                let t_ms = step as f64 * 137.0;
                let v = drift_offset(t_ms, id, amp, period);
                let bound = (amp * 1.4143) as f64; // sqrt(2) + ε
                assert!(
                    v.x.abs() <= bound && v.y.abs() <= bound,
                    "drift exceeded amp√2 for id={id} t={t_ms}: ({},{})",
                    v.x, v.y
                );
            }
        }
    }

    #[test]
    fn drift_phases_diverge_per_node() {
        let amp = 5.0_f32;
        let period = 5000.0_f32;
        let a = drift_offset(1234.0, "node-a", amp, period);
        let b = drift_offset(1234.0, "node-z-different", amp, period);
        let dx = (a.x - b.x).abs();
        let dy = (a.y - b.y).abs();
        assert!(dx + dy > 0.5, "drift identical for distinct ids: a={a:?} b={b:?}");
    }
}
