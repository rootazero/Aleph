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
    ) {
        // Clear background
        ctx.clear_rect(0.0, 0.0, viewport.width, viewport.height);
        ctx.set_fill_style_str("#0a0a0f");
        ctx.fill_rect(0.0, 0.0, viewport.width, viewport.height);

        ctx.save();
        let _ = ctx.translate(viewport.offset.x, viewport.offset.y);
        let _ = ctx.scale(viewport.scale, viewport.scale);

        Self::draw_edges(ctx, nodes, edges, selected, kind_filter);
        Self::draw_nodes(ctx, nodes, selected, hovered, kind_filter);

        ctx.restore();
    }

    fn is_node_visible(node: &CanvasNode, kind_filter: &HashSet<String>) -> bool {
        kind_filter.is_empty() || kind_filter.contains(&node.kind)
    }

    fn draw_edges(
        ctx: &CanvasRenderingContext2d,
        nodes: &[CanvasNode],
        edges: &[CanvasEdge],
        selected: Option<&str>,
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

            if !Self::is_node_visible(from, kind_filter)
                || !Self::is_node_visible(to, kind_filter)
            {
                continue;
            }

            let is_highlighted = selected
                .map(|s| from.id == s || to.id == s)
                .unwrap_or(false);

            let alpha = if is_highlighted { 0.7 } else { 0.2 };
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
    ) {
        for node in nodes {
            if !Self::is_node_visible(node, kind_filter) {
                continue;
            }

            let is_selected = selected.map(|s| s == node.id).unwrap_or(false);
            let is_hovered = hovered.map(|h| h == node.id).unwrap_or(false);

            let x = node.position.x;
            let y = node.position.y;
            let r = node.radius;

            // Outer glow ring for selected / hovered nodes
            if is_selected || is_hovered {
                let glow_color = node.color.to_css_alpha(0.25);
                ctx.set_fill_style_str(&glow_color);
                ctx.begin_path();
                let _ = ctx.arc(x, y, r + 8.0, 0.0, std::f64::consts::TAU);
                ctx.fill();
            }

            // Main node circle
            let fill_color = node.color.to_css_alpha(if is_selected { 1.0 } else { 0.85 });
            ctx.set_fill_style_str(&fill_color);
            ctx.begin_path();
            let _ = ctx.arc(x, y, r, 0.0, std::f64::consts::TAU);
            ctx.fill();

            // Border ring
            let border_color = if is_selected {
                "rgba(255,255,255,0.9)".to_string()
            } else if is_hovered {
                "rgba(255,255,255,0.6)".to_string()
            } else {
                node.color.to_css_alpha(0.4)
            };
            ctx.set_stroke_style_str(&border_color);
            ctx.set_line_width(if is_selected { 2.0 } else { 1.0 });
            ctx.begin_path();
            let _ = ctx.arc(x, y, r, 0.0, std::f64::consts::TAU);
            ctx.stroke();

            // Small yellow dot for nodes that have a wiki page
            if node.has_wiki {
                ctx.set_fill_style_str("rgba(250,204,21,0.9)");
                ctx.begin_path();
                let _ = ctx.arc(x + r * 0.65, y - r * 0.65, 3.0, 0.0, std::f64::consts::TAU);
                ctx.fill();
            }

            // Icon emoji, only when the node is large enough to show it
            if r >= 12.0 {
                ctx.set_fill_style_str("rgba(255,255,255,0.9)");
                ctx.set_font(&format!("{}px sans-serif", (r * 0.85).min(16.0)));
                ctx.set_text_align("center");
                ctx.set_text_baseline("middle");
                let _ = ctx.fill_text(node.icon, x, y);
            }

            // Node name label below the circle
            let label_color = if is_selected || is_hovered {
                "rgba(241,245,249,1.0)"
            } else {
                "rgba(148,163,184,0.85)"
            };
            ctx.set_fill_style_str(label_color);
            ctx.set_font("11px sans-serif");
            ctx.set_text_align("center");
            ctx.set_text_baseline("top");
            let label = if node.name.len() > 20 {
                format!("{}…", &node.name[..19])
            } else {
                node.name.clone()
            };
            let _ = ctx.fill_text(&label, x, y + r + 12.0);
        }
    }
}
