//! Per-frame orchestration: camera update → scene FBO → bloom → screen.

use std::collections::HashSet;

use web_sys::{HtmlCanvasElement, WebGl2RenderingContext as Gl};

use super::bloom::BloomPipeline;
use super::camera::OrbitCamera;
use super::context::GlContext;
use super::edges::EdgeRenderer;
use super::layout3d::ForceLayout;
use super::math::{Mat4, Vec3};
use super::nodes::NodeRenderer;
use super::GraphData;
use crate::canvas_engine::fnv1a::fnv1a_32;

/// Maximum force-layout steps per new graph before switching to idle drift.
const MAX_SETTLE_STEPS: u32 = 400;

/// Idle drift: peak displacement from settled position (scene units, not px).
const DRIFT_AMPLITUDE: f32 = 3.0;

/// Idle drift: period of one full oscillation in milliseconds.
const DRIFT_PERIOD_MS: f32 = 5000.0;

pub struct Scene {
    ctx: GlContext,
    nodes: NodeRenderer,
    edges: EdgeRenderer,
    bloom: BloomPipeline,
    pub camera: OrbitCamera,
    data: GraphData,
    width: i32,
    height: i32,
    last_t: f64,
    layout: Option<ForceLayout>,
    settling: bool,
    settle_steps: u32,
    /// Last view-projection matrix, stored each frame for picking.
    last_vp: Mat4,
    /// Current highlight set (selected node index + topological neighbors).
    /// Stored on the struct so settling/drift re-uploads preserve it.
    highlight: Option<HashSet<u32>>,
    /// LOD level in [0, 1]. 0 = show all edges; 1 = show only high-degree backbone.
    /// Stored so all edge-upload sites (set_graph, settling, set_lod) apply it consistently.
    lod: f32,
    /// Pre-filtered edge index list for the current (graph, lod) pair.
    /// Recomputed only when `data` or `lod` changes; used every settling frame
    /// to avoid per-frame clone + O(n log n) sort.
    filtered_edges: Vec<(u32, u32)>,
    /// Reusable scratch buffer for idle-drift drifted positions (C: perf cleanup).
    /// Avoids cloning the full GraphData (incl. String ids) every frame.
    drift_scratch: Vec<Vec3>,
}

impl Scene {
    pub fn new(canvas: &HtmlCanvasElement) -> Result<Scene, String> {
        let ctx = GlContext::from_canvas(canvas)?;
        let nodes = NodeRenderer::new(&ctx.gl)?;
        let edges = EdgeRenderer::new(&ctx.gl)?;
        let w = canvas.width() as i32;
        let h = canvas.height() as i32;
        let bloom = BloomPipeline::new(&ctx.gl, w, h)?;
        Ok(Scene {
            ctx,
            nodes,
            edges,
            bloom,
            camera: OrbitCamera::new(800.0),
            data: GraphData::default(),
            width: w,
            height: h,
            last_t: 0.0,
            layout: None,
            settling: false,
            settle_steps: 0,
            last_vp: Mat4::identity(),
            highlight: None,
            lod: 0.0,
            filtered_edges: Vec::new(),
            drift_scratch: Vec::new(),
        })
    }

    pub fn set_graph(&mut self, data: GraphData) {
        // Build a force layout to animate-settle the incoming positions.
        let n = data.nodes.len();
        let layout = ForceLayout::new(n, &data.edges);
        self.layout = Some(layout);
        self.settling = true;
        self.settle_steps = 0;
        // Clear highlight when the graph changes — indices shift on a new graph.
        self.highlight = None;

        // Assign data and compute the filtered edge list once for (graph, lod).
        self.data = data;
        self.recompute_filtered_edges();
        self.edges.upload_indexed(&self.ctx.gl, &self.data.nodes, &self.filtered_edges);
        self.nodes.upload(&self.ctx.gl, &self.data, None);
    }

    /// Set the LOD level (0.0 = all edges visible, 1.0 = only high-degree backbone).
    /// Recomputes the cached filtered edge list and re-uploads the edge buffer.
    pub fn set_lod(&mut self, lod: f32) {
        self.lod = lod.clamp(0.0, 1.0);
        self.recompute_filtered_edges();
        self.edges.upload_indexed(&self.ctx.gl, &self.data.nodes, &self.filtered_edges);
    }

    /// Recompute `self.filtered_edges` from `self.data` and `self.lod`.
    ///
    /// LOD floor: edges where BOTH endpoints have `link_count < floor` are dropped.
    /// At lod=0 the floor is 0 (all edges pass). At lod=1 the floor equals the
    /// 90th-percentile link_count of the graph, retaining only the structural backbone.
    ///
    /// Call this whenever `self.data` or `self.lod` changes. After this, use
    /// `upload_indexed` with `&self.filtered_edges` to upload to the GPU without
    /// re-sorting or re-filtering every frame.
    fn recompute_filtered_edges(&mut self) {
        if self.lod <= 0.0 || self.data.nodes.is_empty() {
            self.filtered_edges = self.data.edges.clone();
            return;
        }

        // Compute the link_count floor from the LOD level.
        // lod=0.5 → median; lod=1.0 → ~90th percentile (index = 90% of sorted counts).
        let mut counts: Vec<u32> = self.data.nodes.iter().map(|n| n.link_count).collect();
        counts.sort_unstable();
        let idx = ((self.lod * 0.9 * (counts.len().saturating_sub(1)) as f32) as usize)
            .min(counts.len().saturating_sub(1));
        let floor = counts[idx];

        if floor == 0 {
            self.filtered_edges = self.data.edges.clone();
            return;
        }

        // Retain only edges where at least one endpoint is above the floor
        // (weak spokes into strong hubs are still drawn; only weak-to-weak edges
        // are culled, preserving cluster connectivity).
        self.filtered_edges = self.data.edges.iter().copied().filter(|&(a, b)| {
            let lc_a = self.data.nodes.get(a as usize).map_or(0, |n| n.link_count);
            let lc_b = self.data.nodes.get(b as usize).map_or(0, |n| n.link_count);
            lc_a >= floor || lc_b >= floor
        }).collect();
    }

    /// Screen-space picking: project all nodes through the last-frame view-proj
    /// and return the node id nearest the cursor (within 18 px), or `None`.
    pub fn pick(&self, cursor: (f32, f32)) -> Option<String> {
        super::picking::pick_node(
            &self.last_vp,
            &self.data.nodes,
            (self.width as f32, self.height as f32),
            cursor,
            18.0,
        )
        .map(|i| self.data.nodes[i as usize].id.clone())
    }

    /// Project a node's canonical (settled) position to canvas screen coordinates
    /// using the last-frame view-projection matrix. Returns `None` if the node
    /// is behind the camera or not found.
    pub fn screen_pos_of(&self, id: &str) -> Option<(f32, f32)> {
        let node = self.data.nodes.iter().find(|n| n.id == id)?;
        let m = self.last_vp.as_slice();
        let p = &node.pos;
        let cx = m[0] * p.x + m[4] * p.y + m[8] * p.z + m[12];
        let cy = m[1] * p.x + m[5] * p.y + m[9] * p.z + m[13];
        let cw = m[3] * p.x + m[7] * p.y + m[11] * p.z + m[15];
        if cw <= 0.0 {
            return None; // behind camera
        }
        let ndc_x = cx / cw;
        let ndc_y = cy / cw;
        // Clamp to a reasonable on-screen range (don't return extreme off-screen coords).
        if ndc_x < -1.5 || ndc_x > 1.5 || ndc_y < -1.5 || ndc_y > 1.5 {
            return None;
        }
        let sx = (ndc_x * 0.5 + 0.5) * self.width as f32;
        let sy = (1.0 - (ndc_y * 0.5 + 0.5)) * self.height as f32;
        Some((sx, sy))
    }

    /// Look up a node name by its id. Returns `None` if not found.
    pub fn node_name(&self, id: &str) -> Option<&str> {
        self.data.nodes.iter().find(|n| n.id == id).map(|n| n.name.as_str())
    }

    /// Set the highlight set (selected node index + neighbors). Stored so that
    /// settling/drift re-uploads in `render` don't silently clear it.
    pub fn set_highlight(&mut self, hl: Option<HashSet<u32>>) {
        self.highlight = hl;
        self.nodes
            .upload(&self.ctx.gl, &self.data, self.highlight.as_ref());
    }

    /// Fly the camera to the node with the given id, if found.
    pub fn fly_to_node(&mut self, id: &str, t_ms: f64) {
        if let Some(n) = self.data.nodes.iter().find(|n| n.id == id) {
            self.camera.fly_to(n.pos, 250.0);
            self.camera.note_interaction(t_ms);
        }
    }

    pub fn resize(&mut self, w: i32, h: i32) {
        self.width = w;
        self.height = h;
        self.ctx.resize(w, h);
        // Ignore resize errors — if FBO realloc fails the bloom is degraded
        // but the scene still draws (composite falls back to default FBO).
        let _ = self.bloom.resize(&self.ctx.gl, w, h);
    }

    pub fn on_drag(&mut self, dx: f32, dy: f32, t_ms: f64) {
        self.camera.orbit(dx * 0.005, dy * 0.005);
        self.camera.note_interaction(t_ms);
    }

    pub fn on_wheel(&mut self, delta: f32, t_ms: f64) {
        let factor = if delta > 0.0 { 1.1 } else { 0.9 };
        self.camera.zoom(factor);
        self.camera.note_interaction(t_ms);
    }

    pub fn render(&mut self, t_ms: f64) {
        let dt = (t_ms - self.last_t) as f32;
        self.last_t = t_ms;
        self.camera.update(t_ms, dt);

        // --- Phase 1: Animated settling ---
        if self.settling {
            if let Some(layout) = self.layout.as_mut() {
                // Step layout over live node positions.
                let mut pos: Vec<Vec3> = self.data.nodes.iter().map(|n| n.pos).collect();
                layout.step(&mut pos);
                for (n, p) in self.data.nodes.iter_mut().zip(pos) {
                    n.pos = p;
                }
                self.settle_steps += 1;
                if layout.converged() || self.settle_steps >= MAX_SETTLE_STEPS {
                    self.settling = false;
                }
                // Re-upload both edges and nodes (positions changed).
                // Use the cached filtered_edges list — no clone, no re-sort.
                // Pass through the stored highlight so it survives settling.
                self.edges.upload_indexed(&self.ctx.gl, &self.data.nodes, &self.filtered_edges);
                self.nodes
                    .upload(&self.ctx.gl, &self.data, self.highlight.as_ref());
            }
        } else {
            // --- Phase 2: Idle drift ---
            // Reuse `drift_scratch` (Vec<Vec3>) to avoid cloning the full GraphData
            // (incl. String ids) every frame. Canonical `self.data.nodes[*].pos` is
            // NEVER mutated here, preserving stable settled positions for picking.
            let n = self.data.nodes.len();
            self.drift_scratch.clear();
            self.drift_scratch.reserve(n);
            for node in &self.data.nodes {
                self.drift_scratch.push(
                    drift_offset_3d(t_ms, &node.id, DRIFT_AMPLITUDE, DRIFT_PERIOD_MS, node.pos)
                );
            }
            // Re-upload nodes only (edges remain at stable settled positions).
            // Pass through the stored highlight so it survives drift re-uploads.
            self.nodes.upload_positions(
                &self.ctx.gl,
                &self.drift_scratch,
                &self.data,
                self.highlight.as_ref(),
            );
        }

        let gl = &self.ctx.gl;

        // --- Scene pass: render into the bloom scene FBO ---
        // Bind the scene FBO before clearing so we draw into it, not the screen.
        gl.bind_framebuffer(Gl::FRAMEBUFFER, Some(self.bloom.scene_fbo()));
        gl.viewport(0, 0, self.width, self.height);
        gl.clear_color(0.024, 0.035, 0.059, 1.0); // #06090f-ish
        // Enable additive blend for the scene geometry (edges + nodes).
        gl.enable(Gl::BLEND);
        gl.blend_func(Gl::SRC_ALPHA, Gl::ONE); // additive
        gl.clear(Gl::COLOR_BUFFER_BIT | Gl::DEPTH_BUFFER_BIT);
        let aspect = self.width as f32 / self.height.max(1) as f32;
        let vp = self.camera.view_proj(aspect);
        // Store for picking (uses stable canonical positions, not drifted).
        self.last_vp = vp;
        self.edges.draw(gl, &vp);
        self.nodes.draw(gl, &vp, (self.width as f32, self.height as f32));
        // Restore blend state after scene draw — bloom passes will disable blend.
        gl.disable(Gl::BLEND);

        // --- Bloom pass: bright-pass → blur → composite to default FBO ---
        // bloom.run() manages all blend state internally (BLEND disabled).
        self.bloom.run(gl);
    }

}

// ---------------------------------------------------------------------------
// 3D idle drift helper
// ---------------------------------------------------------------------------

/// Returns `base_pos` offset by three independent phase-shifted sine components
/// (x, y, z) derived from the node id hash. The `base_pos` argument is passed
/// by value; this function never touches `data.nodes[*].pos`.
fn drift_offset_3d(t_ms: f64, node_id: &str, amplitude: f32, period_ms: f32, base: Vec3) -> Vec3 {
    let h = fnv1a_32(node_id.as_bytes());
    let phase = h as f32 / u32::MAX as f32; // [0, 1)
    let omega = std::f32::consts::TAU / (period_ms / 1000.0);
    let t = (t_ms as f32) / 1000.0;
    // Three axes with phase offsets (0, +0.27, +0.54 of TAU) so motion is
    // non-planar and adjacent nodes move out of sync.
    let dx = amplitude * (omega * t + phase * std::f32::consts::TAU).sin();
    let dy = amplitude * (omega * t + (phase + 0.27) * std::f32::consts::TAU).sin();
    let dz = amplitude * (omega * t + (phase + 0.54) * std::f32::consts::TAU).sin();
    Vec3::new(base.x + dx, base.y + dy, base.z + dz)
}
