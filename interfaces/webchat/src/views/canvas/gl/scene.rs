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

        // Upload initial state so the first frame has something to draw.
        self.edges.upload(&self.ctx.gl, &data);
        self.nodes.upload(&self.ctx.gl, &data, None);
        self.data = data;
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
                // Pass through the stored highlight so it survives settling.
                self.edges.upload(&self.ctx.gl, &self.data);
                self.nodes
                    .upload(&self.ctx.gl, &self.data, self.highlight.as_ref());
            }
        } else {
            // --- Phase 2: Idle drift ---
            // Build a scratch copy with per-node sine wobble applied.
            // The canonical `self.data.nodes[*].pos` is NEVER mutated here,
            // preserving stable settled positions for picking.
            let drifted: Vec<Vec3> = self.data.nodes.iter().map(|n| {
                drift_offset_3d(t_ms, &n.id, DRIFT_AMPLITUDE, DRIFT_PERIOD_MS, n.pos)
            }).collect();

            // Build a temporary GraphData with drifted positions for the upload.
            let mut scratch = self.data.clone();
            for (node, pos) in scratch.nodes.iter_mut().zip(drifted) {
                node.pos = pos;
            }
            // Re-upload nodes only (edges remain at stable settled positions).
            // Pass through the stored highlight so it survives drift re-uploads.
            self.nodes
                .upload(&self.ctx.gl, &scratch, self.highlight.as_ref());
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
