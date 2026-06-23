//! Per-frame orchestration: camera update → clear → edges → nodes.

use web_sys::{HtmlCanvasElement, WebGl2RenderingContext as Gl};

use super::camera::OrbitCamera;
use super::context::GlContext;
use super::edges::EdgeRenderer;
use super::nodes::NodeRenderer;
use super::GraphData;

pub struct Scene {
    ctx: GlContext,
    nodes: NodeRenderer,
    edges: EdgeRenderer,
    pub camera: OrbitCamera,
    data: GraphData,
    width: i32,
    height: i32,
    last_t: f64,
}

impl Scene {
    pub fn new(canvas: &HtmlCanvasElement) -> Result<Scene, String> {
        let ctx = GlContext::from_canvas(canvas)?;
        let nodes = NodeRenderer::new(&ctx.gl)?;
        let edges = EdgeRenderer::new(&ctx.gl)?;
        Ok(Scene {
            ctx,
            nodes,
            edges,
            camera: OrbitCamera::new(800.0),
            data: GraphData::default(),
            width: canvas.width() as i32,
            height: canvas.height() as i32,
            last_t: 0.0,
        })
    }

    pub fn set_graph(&mut self, data: GraphData) {
        self.edges.upload(&self.ctx.gl, &data);
        self.nodes.upload(&self.ctx.gl, &data, None);
        self.data = data;
    }

    pub fn resize(&mut self, w: i32, h: i32) {
        self.width = w;
        self.height = h;
        self.ctx.resize(w, h);
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
        let gl = &self.ctx.gl;
        gl.clear_color(0.024, 0.035, 0.059, 1.0); // #06090f-ish
        gl.enable(Gl::BLEND);
        gl.blend_func(Gl::SRC_ALPHA, Gl::ONE); // additive
        gl.clear(Gl::COLOR_BUFFER_BIT | Gl::DEPTH_BUFFER_BIT);
        let aspect = self.width as f32 / self.height.max(1) as f32;
        let vp = self.camera.view_proj(aspect);
        self.edges.draw(gl, &vp);
        self.nodes.draw(gl, &vp, (self.width as f32, self.height as f32));
    }

    /// Suppress unused-field warning for `data` on non-WASM targets (pure struct holder).
    #[allow(dead_code)]
    fn _data_ref(&self) -> &GraphData {
        &self.data
    }
}
