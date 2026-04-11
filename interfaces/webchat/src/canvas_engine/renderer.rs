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
        Self::draw_nodes(ctx, nodes, selected, hovered, kind_filter, highlighted_neighbors);

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

            if !Self::is_node_visible(from, kind_filter)
                || !Self::is_node_visible(to, kind_filter)
            {
                continue;
            }

            let is_selected_edge = selected
                .map(|s| from.id == s || to.id == s)
                .unwrap_or(false);
            let is_hovered_edge = hovered
                .map(|h| from.id == h || to.id == h)
                .unwrap_or(false);
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
