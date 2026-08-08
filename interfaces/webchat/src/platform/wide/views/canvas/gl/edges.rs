//! Instanced thick-line edge renderer (screen-space Bézier ribbons = smooth star filaments).
//!
//! `gl.lineWidth()` is clamped to 1px on desktop browsers, so edges are drawn as
//! instanced TRIANGLE_STRIP ribbons: one ribbon per edge tessellated into SEGMENTS
//! quads. The Bézier arc and endpoint taper are computed in the vertex shader.
//! Per-instance attributes carry the two endpoints and their colors; a static
//! strip corner buffer (2*(SEGMENTS+1) vertices) is shared.

use web_sys::{WebGl2RenderingContext as Gl, WebGlBuffer, WebGlProgram, WebGlVertexArrayObject};

use super::context::compile_program;
use super::math::Mat4;
use super::nodes::{set_mat4, set_vec2};
use super::shaders;

/// Base edge line width in framebuffer pixels (uploaded as `u_width`). The edge
/// vertex shader lerps it to 1.5x on the per-instance `a_highlight` flag, so an
/// edge incident to the selected node renders at ~4.5 px.
const EDGE_WIDTH_PX: f32 = 3.0;

/// Curve tessellation: segments per edge. 12 = smooth gentle arc.
const SEGMENTS: usize = 12;

/// Triangle-strip corners for a K-segment ribbon: (along ∈ [0,1] in K steps,
/// side ∈ {-1,+1}). The vertex shader evaluates the Bézier at `along` and
/// offsets perpendicular by `side`. Drawn with TRIANGLE_STRIP.
pub(super) fn edge_strip_corners(segments: usize) -> Vec<f32> {
    let mut v = Vec::with_capacity(2 * (segments + 1) * 2);
    for i in 0..=segments {
        let along = i as f32 / segments as f32;
        v.extend_from_slice(&[along, -1.0, along, 1.0]);
    }
    v
}

/// Map a `notes_links.relation` kind string to a compact code.
/// `None`/`"wikilink"` = 0 (plain body wikilink, the structural backbone).
pub fn edge_kind_code(kind: Option<&str>) -> u8 {
    match kind {
        None | Some("wikilink") => 0,
        Some("semantic") => 1,
        Some("related") => 2,
        Some("co_recalled") => 3,
        Some("mention") => 5,
        Some("related_similarity") => 6,
        Some(_) => 4, // keyword/entity verbs and any future kind
                      // 7 (surprising) is not a wire kind string — it is a build-time
                      // override applied on top of the base kind (see `build_galaxy`).
    }
}

/// Tint color for an edge kind, or `None` to keep the endpoint-color gradient
/// (used for `wikilink`, the backbone). Specials get a distinct hue so they
/// stand out against the wikilink filaments.
pub fn edge_kind_color(code: u8) -> Option<[f32; 3]> {
    match code {
        1 => Some([0.133, 0.827, 0.933]), // semantic    → cyan   #22d3ee
        2 => Some([0.655, 0.545, 0.980]), // related     → purple #a78bfa
        3 => Some([0.984, 0.749, 0.141]), // co_recalled → amber  #fbbf24
        4 => Some([0.204, 0.827, 0.600]), // keyword     → green  #34d399
        5 => Some([0.42, 0.45, 0.55]),    // mention     → dark slate
        6 => Some([0.48, 0.40, 0.72]),    // similarity  → violet
        7 => Some([1.35, 1.15, 0.55]),    // surprising  → gold bloom (>1.0)
        _ => None,                        // 0 wikilink / unknown → endpoint gradient
    }
}

pub struct EdgeRenderer {
    prog: WebGlProgram,
    vao: WebGlVertexArrayObject,
    pos_a_buf: WebGlBuffer,
    pos_b_buf: WebGlBuffer,
    col_a_buf: WebGlBuffer,
    col_b_buf: WebGlBuffer,
    /// Per-instance drift phases of the two endpoints (locations 6/7). Let the
    /// edge shader reproduce each node's idle drift so the ribbon never detaches.
    phase_a_buf: WebGlBuffer,
    phase_b_buf: WebGlBuffer,
    /// Per-instance highlight flag buffer (location 5): 1.0 = neighbor, 0.0 = other.
    hl_buf: WebGlBuffer,
    count: i32,
    /// 1.0 when a node is selected (drives non-neighbor dimming in the frag shader).
    select_active: f32,
}

impl EdgeRenderer {
    /// # Safety
    ///
    /// This renderer uses `js_sys::Float32Array::view` to upload geometry
    /// without copying. The caller must ensure the source slice is not moved or
    /// dropped until the upload call returns.
    pub fn new(gl: &Gl) -> Result<EdgeRenderer, String> {
        let prog = compile_program(
            gl,
            &shaders::with_drift(shaders::EDGE_VERT),
            shaders::EDGE_FRAG,
        )?;
        let vao = gl.create_vertex_array().ok_or("edge vao")?;
        gl.bind_vertex_array(Some(&vao));

        // a_corner (location 0) — static strip corners, per-vertex (divisor 0).
        let corner_buf = gl.create_buffer().ok_or("edge corner")?;
        gl.bind_buffer(Gl::ARRAY_BUFFER, Some(&corner_buf));
        let corners = edge_strip_corners(SEGMENTS);
        unsafe {
            // SAFETY: `view` is consumed immediately by the buffer upload before
            // any allocation that could move `corners`.
            let view = js_sys::Float32Array::view(&corners);
            gl.buffer_data_with_array_buffer_view(Gl::ARRAY_BUFFER, &view, Gl::STATIC_DRAW);
        }
        gl.enable_vertex_attrib_array(0);
        gl.vertex_attrib_pointer_with_i32(0, 2, Gl::FLOAT, false, 0, 0);

        let pos_a_buf = gl.create_buffer().ok_or("edge pos_a")?;
        let pos_b_buf = gl.create_buffer().ok_or("edge pos_b")?;
        let col_a_buf = gl.create_buffer().ok_or("edge col_a")?;
        let col_b_buf = gl.create_buffer().ok_or("edge col_b")?;
        let phase_a_buf = gl.create_buffer().ok_or("edge phase_a")?;
        let phase_b_buf = gl.create_buffer().ok_or("edge phase_b")?;
        let hl_buf = gl.create_buffer().ok_or("edge hl")?;
        // a_pos_a(1) a_pos_b(2) a_color_a(3) a_color_b(4) — all per-instance.
        Self::setup_instanced(gl, &pos_a_buf, 1, 3);
        Self::setup_instanced(gl, &pos_b_buf, 2, 3);
        Self::setup_instanced(gl, &col_a_buf, 3, 3);
        Self::setup_instanced(gl, &col_b_buf, 4, 3);
        // a_highlight(5) — per-instance flag: 1.0 = neighbor of selected, 0.0 = other.
        Self::setup_instanced(gl, &hl_buf, 5, 1);
        // a_phase_a(6) a_phase_b(7) — per-instance endpoint drift phases.
        Self::setup_instanced(gl, &phase_a_buf, 6, 1);
        Self::setup_instanced(gl, &phase_b_buf, 7, 1);

        gl.bind_vertex_array(None);
        Ok(EdgeRenderer {
            prog,
            vao,
            pos_a_buf,
            pos_b_buf,
            col_a_buf,
            col_b_buf,
            phase_a_buf,
            phase_b_buf,
            hl_buf,
            count: 0,
            select_active: 0.0,
        })
    }

    fn setup_instanced(gl: &Gl, buf: &WebGlBuffer, loc: u32, size: i32) {
        gl.bind_buffer(Gl::ARRAY_BUFFER, Some(buf));
        gl.enable_vertex_attrib_array(loc);
        gl.vertex_attrib_pointer_with_i32(loc, size, Gl::FLOAT, false, 0, 0);
        gl.vertex_attrib_divisor(loc, 1);
    }

    /// Upload edges from an explicit nodes slice + edge-index slice.
    /// Builds one per-instance record (endpoints + colors) per edge; avoids
    /// cloning GraphData.
    pub fn upload_indexed(
        &mut self,
        gl: &Gl,
        nodes: &[super::GalaxyNode],
        edges: &[(u32, u32)],
        edge_kinds: &[u8],
        edge_bright: &[f32],
    ) {
        let mut pos_a = Vec::with_capacity(edges.len() * 3);
        let mut pos_b = Vec::with_capacity(edges.len() * 3);
        let mut col_a = Vec::with_capacity(edges.len() * 3);
        let mut col_b = Vec::with_capacity(edges.len() * 3);
        let mut phase_a = Vec::with_capacity(edges.len());
        let mut phase_b = Vec::with_capacity(edges.len());
        for (i, &(a, b)) in edges.iter().enumerate() {
            let (na, nb) = (&nodes[a as usize], &nodes[b as usize]);
            pos_a.extend_from_slice(&[na.pos.x, na.pos.y, na.pos.z]);
            pos_b.extend_from_slice(&[nb.pos.x, nb.pos.y, nb.pos.z]);
            // Kind tint: specials override both endpoints with a distinct hue;
            // wikilink (or unknown) keeps the endpoint-color gradient. Brightness
            // (confidence-scaled) is multiplied into whichever color wins.
            let bright = edge_bright.get(i).copied().unwrap_or(1.0);
            match edge_kinds.get(i).copied().and_then(edge_kind_color) {
                Some(c) => {
                    let c = [c[0] * bright, c[1] * bright, c[2] * bright];
                    col_a.extend_from_slice(&c);
                    col_b.extend_from_slice(&c);
                }
                None => {
                    col_a.extend_from_slice(&[
                        na.color[0] * bright,
                        na.color[1] * bright,
                        na.color[2] * bright,
                    ]);
                    col_b.extend_from_slice(&[
                        nb.color[0] * bright,
                        nb.color[1] * bright,
                        nb.color[2] * bright,
                    ]);
                }
            }
            // Same phase the node renderer uploads → identical drift per endpoint.
            phase_a.push(super::nodes::node_phase(&na.id));
            phase_b.push(super::nodes::node_phase(&nb.id));
        }
        self.count = edges.len() as i32;
        bind_upload(gl, &self.pos_a_buf, &pos_a);
        bind_upload(gl, &self.pos_b_buf, &pos_b);
        bind_upload(gl, &self.col_a_buf, &col_a);
        bind_upload(gl, &self.col_b_buf, &col_b);
        bind_upload(gl, &self.phase_a_buf, &phase_a);
        bind_upload(gl, &self.phase_b_buf, &phase_b);
        // Initialize a_highlight to all-0 (no highlight); caller re-applies via set_highlight.
        let zeros: Vec<f32> = vec![0.0; edges.len()];
        bind_upload(gl, &self.hl_buf, &zeros);
    }

    /// Rebuild the per-edge highlight flag aligned to the LAST uploaded edge order,
    /// and flag whether a selection is active (for non-neighbor dimming).
    pub fn set_highlight(
        &mut self,
        gl: &Gl,
        edges_in_order: &[(u32, u32)],
        hl: Option<&std::collections::HashSet<(u32, u32)>>,
    ) {
        let active = hl.map(|s| !s.is_empty()).unwrap_or(false);
        self.select_active = if active { 1.0 } else { 0.0 };
        let flags: Vec<f32> = edges_in_order
            .iter()
            .map(|&(a, b)| {
                let key = (a.min(b), a.max(b));
                if hl.is_some_and(|s| s.contains(&key)) {
                    1.0
                } else {
                    0.0
                }
            })
            .collect();
        bind_upload(gl, &self.hl_buf, &flags);
    }

    pub fn draw(&self, gl: &Gl, view_proj: &Mat4, viewport: (f32, f32), u_time_ms: f32) {
        if self.count == 0 {
            return;
        }
        gl.use_program(Some(&self.prog));
        gl.bind_vertex_array(Some(&self.vao));
        set_mat4(gl, &self.prog, "u_view_proj", view_proj);
        set_vec2(gl, &self.prog, "u_viewport", viewport);
        let loc_width = gl.get_uniform_location(&self.prog, "u_width");
        gl.uniform1f(loc_width.as_ref(), EDGE_WIDTH_PX);
        let loc_time = gl.get_uniform_location(&self.prog, "u_time");
        gl.uniform1f(loc_time.as_ref(), u_time_ms);
        let loc_sa = gl.get_uniform_location(&self.prog, "u_select_active");
        gl.uniform1f(loc_sa.as_ref(), self.select_active);
        let vtx = (2 * (SEGMENTS + 1)) as i32;
        gl.draw_arrays_instanced(Gl::TRIANGLE_STRIP, 0, vtx, self.count);
        gl.bind_vertex_array(None);
    }
}

/// # Safety
///
/// Uses `js_sys::Float32Array::view` on `data`. The view is consumed by the
/// WebGL buffer upload before this function returns, so `data` must remain
/// valid and un-moved for the duration of the call.
fn bind_upload(gl: &Gl, buf: &WebGlBuffer, data: &[f32]) {
    gl.bind_buffer(Gl::ARRAY_BUFFER, Some(buf));
    unsafe {
        // SAFETY: `view` is consumed immediately by `buffer_data_with_array_buffer_view`
        // before any allocation that could move `data` occurs.
        let view = js_sys::Float32Array::view(data);
        gl.buffer_data_with_array_buffer_view(Gl::ARRAY_BUFFER, &view, Gl::DYNAMIC_DRAW);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_corners_shape_and_endpoints() {
        let seg = 12usize;
        let c = edge_strip_corners(seg);
        assert_eq!(c.len(), 2 * (seg + 1) * 2);
        // first pair along=0, sides -1 then +1
        assert_eq!(c[0], 0.0);
        assert_eq!(c[1], -1.0);
        assert_eq!(c[3], 1.0);
        // last pair along=1
        let n = c.len();
        assert!((c[n - 2] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn edge_kind_code_maps_known_relations() {
        assert_eq!(edge_kind_code(None), 0);
        assert_eq!(edge_kind_code(Some("wikilink")), 0);
        assert_eq!(edge_kind_code(Some("semantic")), 1);
        assert_eq!(edge_kind_code(Some("related")), 2);
        assert_eq!(edge_kind_code(Some("co_recalled")), 3);
        assert_eq!(edge_kind_code(Some("keyword-verb-whatever")), 4);
        assert_eq!(edge_kind_code(Some("mention")), 5);
        assert_eq!(edge_kind_code(Some("related_similarity")), 6);
    }

    #[test]
    fn edge_kind_color_backbone_is_none_specials_are_some() {
        assert_eq!(edge_kind_color(0), None); // wikilink → endpoint gradient
        assert!(edge_kind_color(1).is_some()); // semantic tinted
        assert!(edge_kind_color(4).is_some());
        assert_eq!(edge_kind_color(99), None); // out-of-range → None
    }

    #[test]
    fn edge_kind_color_surprising_exceeds_unit_for_bloom() {
        // Surprising (7) is intentionally >1.0 so the bloom bright-pass picks
        // it up even at full brightness multiplier.
        assert!(edge_kind_color(7).unwrap()[0] > 1.0);
    }
}
