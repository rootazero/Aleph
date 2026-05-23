use wasm_bindgen::JsValue;
use web_sys::CanvasRenderingContext2d;

use super::types::*;
use super::viewport::Viewport;
use crate::canvas_engine::drag::DragOverlay;
use crate::canvas_engine::edge_curve::{edge_control_point, hop_style, HopLayer, DEFAULT_SAG};

/// Idle drift amplitude (pixels). Each node oscillates within this radius around its
/// resolved (target) position. Spec §6.1, Q4 = "B / breathing" feel.
pub(crate) const DRIFT_AMPLITUDE_PX: f32 = 5.0;
/// Idle drift period in ms (one full oscillation).
pub(crate) const DRIFT_PERIOD_MS: f32 = 5000.0;

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
    //    Dim clustered dots for nodes outside the current connected component.
    //    Click to re-center.
    draw_orphan_ring(ctx, &nbhd.orphans, drag, selected, hovered);

    // 2. Layer A: 2-hop (back) — edges only; node circles are now DOM NodeCard overlays
    for n in &nbhd.two_hop {
        draw_edges_for_node(ctx, n, nbhd, drag, node_drag, selected, hovered);
    }

    // 3. Layer B: 1-hop + clusters — edges only; node circles are DOM NodeCard overlays
    for c in &nbhd.clusters {
        draw_cluster(ctx, c, drag, selected, hovered);
    }
    for n in &nbhd.one_hop {
        draw_edges_for_node(ctx, n, nbhd, drag, node_drag, selected, hovered);
    }

    // Draw the dragged node last (on top of clusters / other 1-hop nodes)
    // with a glow ring when promote-imminent. The centre→dragged edge is drawn
    // by the standard edge pass via drag-aware `endpoints_world_pos`.
    if let Some(overlay) = node_drag {
        if let Some(n) = nbhd.one_hop.iter().find(|x| x.id == overlay.node_id) {
            draw_dragged_node(ctx, n, overlay);
        }
    }

    // Layer C: Active center — edge connections drawn above; node body is a DOM NodeCard overlay

    ctx.restore();
}

fn paint_background(ctx: &CanvasRenderingContext2d, viewport: &Viewport) {
    // Solid dark fill; CanvasGradient feature not enabled in this build.
    // T14/T15 can add a proper radial gradient once the feature is wired.
    ctx.set_fill_style_str("#080818");
    ctx.fill_rect(0.0, 0.0, viewport.width, viewport.height);
}

/// Draw orphan dots in their clustered positions. Each orphan is a small dim
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

    for n in orphans {
        let off = crate::canvas_engine::viewport::parallax_offset(n.z, drag.0, drag.1);
        let drift = drift_offset(
            now_ms_in_seconds() * 1000.0,
            &n.id,
            DRIFT_AMPLITUDE_PX,
            DRIFT_PERIOD_MS,
        );
        let cx = n.position.x + off.0 as f64 + drift.x;
        let cy = n.position.y + off.1 as f64 + drift.y;
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
    node_drag: Option<&DragOverlay>,
    selected: Option<&str>,
    hovered: Option<&str>,
) {
    // Resolve a neighborhood index to the node's string ID.
    let idx_to_id = |idx: usize| -> Option<&str> {
        if idx == 0 {
            Some(nbhd.center.id.as_str())
        } else if idx <= nbhd.one_hop.len() {
            Some(nbhd.one_hop[idx - 1].id.as_str())
        } else {
            let off = idx - 1 - nbhd.one_hop.len();
            nbhd.two_hop.get(off).map(|x| x.id.as_str())
        }
    };

    // Highlight anchor: selected takes priority over hovered.
    let highlight_anchor: Option<&str> = selected.or(hovered);

    // This node's index in the neighborhood (used to filter edges belonging to it).
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

    let t_ms = now_ms_in_seconds() * 1000.0;

    // Two-pass render: dim (non-adjacent) first, bright (adjacent) on top.
    for pass_bright in [false, true] {
        for e in &nbhd.edges {
            // Draw each edge only once — from_idx < to_idx convention.
            if e.from_idx >= e.to_idx {
                continue;
            }
            // Only draw edges that involve this node.
            if e.from_idx != n_idx && e.to_idx != n_idx {
                continue;
            }

            // Determine adjacency to the highlight anchor.
            let is_adjacent = highlight_anchor
                .map(|h| {
                    idx_to_id(e.from_idx).map_or(false, |id| id == h)
                        || idx_to_id(e.to_idx).map_or(false, |id| id == h)
                })
                .unwrap_or(false);

            // Skip this edge in the wrong pass.
            if is_adjacent != pass_bright {
                continue;
            }

            let endpoints = endpoints_world_pos(e, nbhd, drag, node_drag, t_ms);
            let (from_pos, to_pos, _from_z, _to_z) = match endpoints {
                Some(t) => t,
                None => continue,
            };

            // Hop layer: if either endpoint is in two_hop (idx > one_hop.len()), it's Two.
            let layer = if e.from_idx > nbhd.one_hop.len() || e.to_idx > nbhd.one_hop.len() {
                HopLayer::Two
            } else {
                HopLayer::One
            };
            let (max_alpha, line_width) = hop_style(layer);

            // Compute effective color, alpha, and width based on highlight state.
            let (color_rgb, effective_alpha, effective_width) = if highlight_anchor.is_some() {
                if is_adjacent {
                    // Gold, full brightness, wider stroke.
                    ("252,211,77", max_alpha, line_width * 1.5)
                } else {
                    // Existing purple, dimmed to 40%.
                    ("167,139,250", max_alpha * 0.4, line_width)
                }
            } else {
                // No anchor active: standard purple at full alpha.
                ("167,139,250", max_alpha, line_width)
            };

            if e.is_wikilink {
                let dashes = js_sys::Array::new();
                dashes.push(&JsValue::from_f64(5.0));
                dashes.push(&JsValue::from_f64(4.0));
                let _ = ctx.set_line_dash(&dashes);
            } else {
                let solid = js_sys::Array::new();
                let _ = ctx.set_line_dash(&solid);
            }

            let cp = edge_control_point(
                Vec2 { x: from_pos.0 as f64, y: from_pos.1 as f64 },
                Vec2 { x: to_pos.0 as f64,   y: to_pos.1 as f64 },
                DEFAULT_SAG,
            );

            // α-gradient stroke: invisible at endpoints, max at the chord interior.
            let grad = ctx.create_linear_gradient(
                from_pos.0 as f64,
                from_pos.1 as f64,
                to_pos.0 as f64,
                to_pos.1 as f64,
            );
            let _ = grad.add_color_stop(0.00, &format!("rgba({color_rgb},0.000)"));
            let _ = grad.add_color_stop(0.15, &format!("rgba({color_rgb},{effective_alpha:.3})"));
            let _ = grad.add_color_stop(0.85, &format!("rgba({color_rgb},{effective_alpha:.3})"));
            let _ = grad.add_color_stop(1.00, &format!("rgba({color_rgb},0.000)"));
            ctx.set_stroke_style_canvas_gradient(&grad);
            ctx.set_line_width(effective_width);

            ctx.begin_path();
            ctx.move_to(from_pos.0 as f64, from_pos.1 as f64);
            // Quadratic Bézier via degenerate cubic: both control points coincide.
            ctx.bezier_curve_to(
                cp.x, cp.y,
                cp.x, cp.y,
                to_pos.0 as f64, to_pos.1 as f64,
            );
            ctx.stroke();
        }
    }
}

/// Return current time in seconds using `performance.now()`.
fn now_ms_in_seconds() -> f64 {
    web_sys::window()
        .and_then(|w| w.performance())
        .map(|p| p.now() / 1000.0)
        .unwrap_or(0.0)
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
    node_drag: Option<&DragOverlay>,
    t_ms: f64,
) -> Option<((f32, f32), (f32, f32), f32, f32)> {
    let resolve = |idx: usize| -> Option<(&str, Vec3)> {
        if idx == 0 {
            let id = nbhd.center.id.as_str();
            nbhd.target_positions.get(id).copied().map(|p| (id, p))
        } else if idx <= nbhd.one_hop.len() {
            let n = &nbhd.one_hop[idx - 1];
            nbhd.target_positions
                .get(n.id.as_str())
                .copied()
                .map(|p| (n.id.as_str(), p))
        } else {
            let off = idx - 1 - nbhd.one_hop.len();
            let n = nbhd.two_hop.get(off)?;
            nbhd.target_positions
                .get(n.id.as_str())
                .copied()
                .map(|p| (n.id.as_str(), p))
        }
    };
    let (id1, p1) = resolve(e.from_idx)?;
    let (id2, p2) = resolve(e.to_idx)?;

    let resolve_endpoint = |id: &str, p: Vec3| -> (f32, f32, f32) {
        if let Some(o) = node_drag {
            if o.node_id == id {
                return (o.position.x as f32, o.position.y as f32, p.z);
            }
        }
        let off = crate::canvas_engine::viewport::parallax_offset(p.z, drag.0, drag.1);
        let d = drift_offset(t_ms, id, DRIFT_AMPLITUDE_PX, DRIFT_PERIOD_MS);
        (p.x + off.0 + d.x as f32, p.y + off.1 + d.y as f32, p.z)
    };

    let (x1, y1, z1) = resolve_endpoint(id1, p1);
    let (x2, y2, z2) = resolve_endpoint(id2, p2);
    Some(((x1, y1), (x2, y2), z1, z2))
}

fn draw_dragged_node(ctx: &CanvasRenderingContext2d, n: &CanvasNode, overlay: &DragOverlay) {
    use std::f64::consts::TAU;

    // overlay.position is in world space (Task 5 contract). The canvas context
    // already has world transform applied by draw_neighborhood, so draw directly.
    // The centre→dragged-node edge is rendered by the standard edge pass
    // (`endpoints_world_pos` is drag-aware), so we only draw the node body and
    // promote-glow here. Drawing the edge a second time produces visual doubling.
    let cx = overlay.position.x;
    let cy = overlay.position.y;

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
        assert!(
            (a.x - b.x).abs() < 1e-3,
            "x not periodic: {} vs {}",
            a.x,
            b.x
        );
        assert!(
            (a.y - b.y).abs() < 1e-3,
            "y not periodic: {} vs {}",
            a.y,
            b.y
        );
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
                    v.x,
                    v.y
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
        assert!(
            dx + dy > 0.5,
            "drift identical for distinct ids: a={a:?} b={b:?}"
        );
    }
}
