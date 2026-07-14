//! Per-frame orchestration: camera update → scene FBO → bloom → screen.

use std::collections::HashSet;

use wasm_bindgen::JsCast;
use web_sys::{HtmlCanvasElement, WebGl2RenderingContext as Gl};

use super::bloom::BloomPipeline;
use super::camera::OrbitCamera;
use super::context::GlContext;
use super::edges::EdgeRenderer;
use super::layout3d::ForceLayout;
use super::math::{Mat4, Vec3};
use super::nodes::NodeRenderer;
use super::{drift, edge_lod, fit, GraphData};

/// Maximum force-layout steps per new graph before switching to idle drift.
/// Matches the layout's annealing schedule (`layout3d::ALPHA_DECAY`), so a graph
/// of any size is cold — not merely truncated — when this budget runs out.
const MAX_SETTLE_STEPS: u32 = 400;

/// Pick tolerance around a star, in CSS pixels.
const PICK_RADIUS_CSS_PX: f32 = 18.0;

/// Padding used when focusing one node: looser than the whole-graph fit so the
/// star lands inside its 1-hop neighbourhood rather than filling the frame.
const FOCUS_PADDING: f32 = 0.35;

/// Zoom step for a ViewportCmd::ZoomIn / ZoomOut (25% per press).
const ZOOM_STEP: f32 = 1.25;

/// Annealing temperature a REFRESH restarts from (see [`Scene::set_graph`]).
/// Warm enough to relax nodes whose links changed, far too cool to re-expand
/// the settled cloud — which is the whole point: saving a note must not
/// re-run the big bang.
const REFRESH_ALPHA: f32 = 0.3;

/// `u_time` wrap period. The shaders receive `t_ms` as an f32, whose ULP reaches
/// a whole 60 Hz frame after ~2 days of page uptime — the drift, twinkle and
/// edge flow then stall-and-jump. 5,000,000 ms is a whole multiple of both the
/// 5000 ms drift period and the 1666.7 ms edge-flow period, so wrapping there
/// is invisible.
const U_TIME_WRAP_MS: f64 = 5_000_000.0;

/// Viewport commands the host (buttons / hotkeys / pointer) drives the camera
/// with. One enum + one entry point instead of a method per control.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ViewportCmd {
    ZoomIn,
    ZoomOut,
    /// Frame the whole graph and re-arm auto-fit.
    Fit,
    /// Canonical orbit pose + fit (the 3D reading of "zoom to 100%").
    Reset,
    /// Fly to the currently selected node and its 1-hop neighbourhood.
    FocusSelection,
}

/// Wheel-zoom sensitivity. The zoom factor is `exp(normalized_delta * this)`,
/// where `normalized_delta = wheel_delta / 100` clamped to ±2. This makes zoom
/// *proportional* to how far the wheel actually moved (a light flick nudges; a
/// firm scroll moves more) instead of a fixed ±10% per event — which compounded
/// rapidly when one physical gesture fires many wheel events (trackpads,
/// smooth-scroll mice). A standard mouse notch (~100px) ≈ 5% zoom.
const ZOOM_SENSITIVITY: f32 = 0.05;

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
    /// Current edge highlight set (normalized (min,max) incident-edge pairs for selected node).
    /// Stored so every upload_indexed re-applies the correct per-edge flags.
    highlight_edges: Option<HashSet<(u32, u32)>>,
    /// LOD level in [0, 1]. 0 = show all edges; 1 = show only high-degree backbone.
    /// Stored so all edge-upload sites (set_graph, settling, set_lod) apply it consistently.
    lod: f32,
    /// Pre-filtered edge index list for the current (graph, lod) pair.
    /// Recomputed only when `data` or `lod` changes; used every settling frame
    /// to avoid per-frame clone + O(n log n) sort.
    filtered_edges: Vec<(u32, u32)>,
    /// Per-edge kind codes aligned with `filtered_edges` (produced in lockstep
    /// by `recompute_filtered_edges`). Consumed by `EdgeRenderer::upload_indexed`
    /// to tint edges by relation kind.
    filtered_edge_kinds: Vec<u8>,
    /// Per-edge brightness scale aligned with `filtered_edges` (produced in
    /// lockstep by `recompute_filtered_edges`). Consumed by
    /// `EdgeRenderer::upload_indexed` to scale edge color by confidence.
    filtered_edge_bright: Vec<f32>,
    /// id → node index, rebuilt on `set_graph`. Turns the per-frame label
    /// lookups (`node_name` / `screen_pos_of`) and `fly_to_node` from O(n)
    /// linear scans into O(1). Positions still come from `data.nodes[idx]`.
    id_index: std::collections::HashMap<String, u32>,
    /// Device pixel ratio. `width`/`height` are the BACKING store; every host
    /// coordinate (cursor, label position, pan delta) is CSS px, so all the
    /// conversion happens inside Scene and the host stays in one unit.
    dpr: f32,
    /// The wrapped `u_time` handed to the shaders last frame. Picking and the
    /// label overlays reproduce the GPU's idle drift from it (see `drift`).
    last_u_time: f32,
    /// True while the camera is allowed to re-frame the graph on its own.
    /// Cleared by the FIRST user orbit/zoom/pan/focus — an auto-fit that keeps
    /// yanking the camera back is worse than one that opens badly. Re-armed only
    /// by a new graph or an explicit Fit/Reset command.
    auto_fit: bool,
    /// Node the camera is flying to, kept until the layout stops moving so the
    /// fly-to tracks the node to its resting place instead of landing where it
    /// used to be.
    focus_id: Option<String>,
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
            // Placeholder pose only: the first `set_graph` fits the camera to the
            // actual node cloud, whose radius is content-dependent.
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
            highlight_edges: None,
            lod: 0.0,
            filtered_edges: Vec::new(),
            filtered_edge_kinds: Vec::new(),
            filtered_edge_bright: Vec::new(),
            id_index: std::collections::HashMap::new(),
            dpr: 1.0,
            last_u_time: 0.0,
            auto_fit: true,
            focus_id: None,
        })
    }

    /// Set the device pixel ratio the host sized the backing store with.
    pub fn set_dpr(&mut self, dpr: f32) {
        self.dpr = dpr.clamp(0.5, 3.0);
    }

    /// Install a new graph.
    ///
    /// Distinguishes a REFRESH (the same agent's graph re-fetched after a note
    /// was saved, renamed or deleted — it shares node ids with what we already
    /// show) from a genuinely NEW graph (an agent switch — no shared ids).
    /// The difference is everything the user sees:
    ///
    /// - **Refresh**: carry the settled positions of every node we already had,
    ///   warm-start the anneal, and leave the camera alone. Re-seeding here would
    ///   collapse the galaxy back to its hash-seeded sphere and re-expand it over
    ///   400 frames — every single time the user saves a note.
    /// - **New**: re-seed, anneal from cold, re-arm auto-fit and re-frame.
    pub fn set_graph(&mut self, mut data: GraphData) {
        // Carry over settled positions for nodes we already know. `data`'s
        // incoming positions are `ForceLayout::seed` output (a hash-derived
        // sphere), which is only the right starting point for a graph we have
        // never laid out.
        let mut carried = 0usize;
        for node in data.nodes.iter_mut() {
            if let Some(&i) = self.id_index.get(&node.id) {
                node.pos = self.data.nodes[i as usize].pos;
                carried += 1;
            }
        }
        let is_refresh = carried > 0;

        // Build a force layout to animate-settle the incoming positions.
        let n = data.nodes.len();
        let communities: Vec<Option<u32>> = data.nodes.iter().map(|node| node.community).collect();
        let mut layout = ForceLayout::new(n, &data.edges, &communities);
        if is_refresh {
            // Warm restart: relax the changed edges without re-expanding the
            // whole cloud from scratch.
            layout.set_alpha(REFRESH_ALPHA);
        }
        self.layout = Some(layout);
        self.settling = true;
        self.settle_steps = 0;
        // Clear highlight when the graph changes — indices shift on a new graph.
        self.highlight = None;
        self.highlight_edges = None;

        // Assign data and compute the filtered edge list once for (graph, lod).
        self.data = data;
        self.id_index = self
            .data
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.id.clone(), i as u32))
            .collect();
        self.recompute_filtered_edges();
        self.edges.upload_indexed(
            &self.ctx.gl,
            &self.data.nodes,
            &self.filtered_edges,
            &self.filtered_edge_kinds,
            &self.filtered_edge_bright,
        );
        self.edges.set_highlight(
            &self.ctx.gl,
            &self.filtered_edges,
            self.highlight_edges.as_ref(),
        );
        self.nodes.upload(&self.ctx.gl, &self.data, None);

        // Only a NEW graph gets a new framing: re-arm auto-fit and frame the seed
        // positions as a first estimate (render() re-fits every settle frame while
        // the cloud is still expanding). An empty graph leaves the camera where it
        // is — there is nothing to fit.
        //
        // A refresh deliberately leaves `auto_fit` and `focus_id` untouched: if the
        // user has not moved the camera, auto-fit is still armed and will re-frame
        // the (barely-changed) cloud on its own; if they HAVE moved it, saving a
        // note must not yank the view back.
        if !is_refresh {
            self.auto_fit = true;
            self.focus_id = None;
            self.fit_to_content();
        }
    }

    /// Set the LOD level (0.0 = all edges visible, 1.0 = only high-degree backbone).
    /// Recomputes the cached filtered edge list and re-uploads the edge buffer.
    pub fn set_lod(&mut self, lod: f32) {
        self.lod = lod.clamp(0.0, 1.0);
        self.recompute_filtered_edges();
        self.edges.upload_indexed(
            &self.ctx.gl,
            &self.data.nodes,
            &self.filtered_edges,
            &self.filtered_edge_kinds,
            &self.filtered_edge_bright,
        );
        self.edges.set_highlight(
            &self.ctx.gl,
            &self.filtered_edges,
            self.highlight_edges.as_ref(),
        );
    }

    /// Recache the drawn edge set for the current `(data, lod)`. Call this
    /// whenever either changes; the settle loop then re-uploads the cached vecs
    /// every frame without re-sorting or re-filtering. See [`edge_lod::filter`]
    /// for the density rule itself.
    fn recompute_filtered_edges(&mut self) {
        let f = edge_lod::filter(&self.data, self.lod);
        self.filtered_edges = f.edges;
        self.filtered_edge_kinds = f.kinds;
        self.filtered_edge_bright = f.bright;
    }

    /// The positions the GPU is actually drawing this frame: canonical position
    /// plus the vertex shader's idle drift. Everything that maps between world
    /// and screen (picking, label overlays) must use THESE — projecting the
    /// canonical position makes clicks miss the star and labels float off it.
    fn rendered_positions(&self) -> Vec<Vec3> {
        self.data
            .nodes
            .iter()
            .map(|n| {
                n.pos.add(&drift::idle_drift(
                    super::nodes::node_phase(&n.id),
                    self.last_u_time,
                ))
            })
            .collect()
    }

    /// Screen-space picking: project the rendered node positions through the
    /// last-frame view-proj and return the node id nearest the cursor (within
    /// 18 CSS px), or `None`. `cursor` is in CSS pixels, canvas-local.
    pub fn pick(&self, cursor: (f32, f32)) -> Option<String> {
        let pos = self.rendered_positions();
        super::picking::pick_node(
            &self.last_vp,
            &pos,
            (self.width as f32, self.height as f32),
            (cursor.0 * self.dpr, cursor.1 * self.dpr),
            PICK_RADIUS_CSS_PX * self.dpr,
        )
        .map(|i| self.data.nodes[i as usize].id.clone())
    }

    /// Project a node's RENDERED position (canonical + idle drift) to canvas CSS
    /// pixels using the last-frame view-projection matrix. Returns `None` if the
    /// node is behind the camera or not found.
    pub fn screen_pos_of(&self, id: &str) -> Option<(f32, f32)> {
        let node = self
            .id_index
            .get(id)
            .map(|&i| &self.data.nodes[i as usize])?;
        let p = node.pos.add(&drift::idle_drift(
            super::nodes::node_phase(&node.id),
            self.last_u_time,
        ));
        let m = self.last_vp.as_slice();
        let cx = m[0] * p.x + m[4] * p.y + m[8] * p.z + m[12];
        let cy = m[1] * p.x + m[5] * p.y + m[9] * p.z + m[13];
        let cw = m[3] * p.x + m[7] * p.y + m[11] * p.z + m[15];
        if cw <= 0.0 {
            return None; // behind camera
        }
        let ndc_x = cx / cw;
        let ndc_y = cy / cw;
        // Clamp to a reasonable on-screen range (don't return extreme off-screen coords).
        if !(-1.5..=1.5).contains(&ndc_x) || !(-1.5..=1.5).contains(&ndc_y) {
            return None;
        }
        // Labels are CSS-px divs: divide the backing-store position by the DPR.
        let sx = (ndc_x * 0.5 + 0.5) * self.width as f32 / self.dpr;
        let sy = (1.0 - (ndc_y * 0.5 + 0.5)) * self.height as f32 / self.dpr;
        Some((sx, sy))
    }

    /// Look up a node name by its id. Returns `None` if not found.
    pub fn node_name(&self, id: &str) -> Option<&str> {
        self.id_index
            .get(id)
            .map(|&i| self.data.nodes[i as usize].name.as_str())
    }

    /// Set the highlight set (selected node index + neighbors). Stored so that
    /// settling/drift re-uploads in `render` don't silently clear it.
    pub fn set_highlight(&mut self, hl: Option<HashSet<u32>>) {
        self.highlight = hl;
        self.nodes
            .upload(&self.ctx.gl, &self.data, self.highlight.as_ref());
    }

    /// Set the edge highlight set (normalized (min,max) incident-edge pairs).
    /// Stored so every upload_indexed re-applies the correct per-edge flags.
    pub fn set_highlight_edges(&mut self, edges: Option<HashSet<(u32, u32)>>) {
        self.edges
            .set_highlight(&self.ctx.gl, &self.filtered_edges, edges.as_ref());
        self.highlight_edges = edges;
    }

    /// Fly the camera to the node with the given id, if found. The camera frames
    /// the node PLUS its 1-hop neighbourhood (a lone star at a fixed distance is
    /// context-free), and keeps tracking it while the layout is still moving.
    pub fn fly_to_node(&mut self, id: &str, t_ms: f64) {
        let Some(&i) = self.id_index.get(id) else {
            return;
        };
        self.take_camera(t_ms);
        self.focus_id = Some(id.to_string());
        self.focus_camera_on(i);
    }

    /// Point the camera at node `i` and its neighbourhood. Re-called every settle
    /// frame while `focus_id` is set, so the fly-to lands where the node ENDS UP,
    /// not where it was when the user clicked.
    fn focus_camera_on(&mut self, i: u32) {
        let pos: Vec<Vec3> = self.data.nodes.iter().map(|n| n.pos).collect();
        let Some(&target) = pos.get(i as usize) else {
            return;
        };
        let graph_r = fit::bounding_sphere(&pos).map_or(1.0, |(_, r)| r);
        let r = fit::focus_radius(i, &pos, &self.data.edges, graph_r);
        let d = OrbitCamera::fit_distance(r, self.aspect(), FOCUS_PADDING);
        self.camera.fly_to(target, d);
    }

    /// Frame the whole node cloud: fit its bounding SPHERE (orbit-invariant — an
    /// AABB fit would clip the moment the user rotates). No-op on an empty graph.
    ///
    /// Eased (damped) — this is the user-facing Fit, and it should read as a move,
    /// not a cut.
    pub fn fit_to_content(&mut self) {
        self.fit_frame(false);
    }

    /// The per-frame auto-fit used while the layout is still expanding.
    ///
    /// `snap_outward` is the difference that matters. The camera's damped approach
    /// closes ~12% of the distance gap per frame, but the cloud expands by up to
    /// `MAX_STEP` (30) world units per node per frame — so an eased fit LOSES the
    /// race for the first ~30 frames and the graph visibly bulges past the edges
    /// of the viewport. Which is precisely the complaint this whole change exists
    /// to fix, just for a shorter time. So while auto-fitting we snap the distance
    /// out instantly when the content demands it, and only ease back IN when the
    /// cloud settles (easing inward is safe: nothing is off-screen while we do it).
    fn fit_frame(&mut self, snap_outward: bool) {
        let pos: Vec<Vec3> = self.data.nodes.iter().map(|n| n.pos).collect();
        let Some((center, radius)) = fit::bounding_sphere(&pos) else {
            return;
        };
        let d = OrbitCamera::fit_distance(radius, self.aspect(), OrbitCamera::FIT_PADDING);
        self.camera.fly_to(center, d);
        if snap_outward && d > self.camera.distance {
            self.camera.set_distance(d);
        }
    }

    /// Apply a viewport command from the host (button, hotkey, pointer).
    /// `selected` is the currently selected node id, used only by FocusSelection.
    pub fn apply_cmd(&mut self, cmd: ViewportCmd, selected: Option<&str>, t_ms: f64) {
        match cmd {
            ViewportCmd::ZoomIn => {
                self.take_camera(t_ms);
                self.camera.zoom(1.0 / ZOOM_STEP);
            }
            ViewportCmd::ZoomOut => {
                self.take_camera(t_ms);
                self.camera.zoom(ZOOM_STEP);
            }
            ViewportCmd::Fit => {
                self.take_camera(t_ms);
                self.fit_to_content();
                self.auto_fit = true; // explicit fit re-arms: keep it framed on resize
            }
            ViewportCmd::Reset => {
                self.take_camera(t_ms);
                self.camera
                    .set_angles(OrbitCamera::HOME_AZIMUTH, OrbitCamera::HOME_ELEVATION);
                self.fit_to_content();
                self.auto_fit = true;
            }
            ViewportCmd::FocusSelection => {
                if let Some(id) = selected {
                    self.fly_to_node(id, t_ms);
                }
            }
        }
    }

    /// The user now owns the camera: stop auto-fitting and stop tracking a focus
    /// target, and defer the idle auto-rotate. Every user-driven camera change
    /// goes through here — an auto-fit that fights the user is worse than a bad
    /// opening framing.
    fn take_camera(&mut self, t_ms: f64) {
        self.auto_fit = false;
        self.focus_id = None;
        self.camera.note_interaction(t_ms);
    }

    fn aspect(&self) -> f32 {
        self.width as f32 / self.height.max(1) as f32
    }

    pub fn resize(&mut self, w: i32, h: i32) {
        if w == self.width && h == self.height {
            return; // ResizeObserver fires on every frame of a window drag
        }
        self.width = w;
        self.height = h;
        self.ctx.resize(w, h);
        // Ignore resize errors — if FBO realloc fails the bloom is degraded
        // but the scene still draws (composite falls back to default FBO).
        let _ = self.bloom.resize(&self.ctx.gl, w, h);
        // The aspect changed, so the limiting fit axis may have too.
        if self.auto_fit {
            self.fit_to_content();
        }
    }

    pub fn on_drag(&mut self, dx: f32, dy: f32, t_ms: f64) {
        self.camera.orbit(dx * 0.005, dy * 0.005);
        self.take_camera(t_ms);
    }

    /// Pan the view by a screen-pixel drag: translate the look-at centre along
    /// the view plane so the point under the cursor stays under the cursor.
    /// Drag right → content follows right (centre moves −right); drag down (screen
    /// y grows downward) → content follows down (centre moves +up).
    /// `dx`/`dy` are CSS pixels.
    pub fn on_pan(&mut self, dx: f32, dy: f32, t_ms: f64) {
        let wpp = self.camera.world_per_pixel(self.css_height());
        let (right, up) = self.camera.screen_basis();
        let delta = right.scale(-dx * wpp).add(&up.scale(dy * wpp));
        self.camera.pan_world(delta);
        self.take_camera(t_ms);
    }

    /// Wheel zoom, anchored on the cursor: the world point under the cursor stays
    /// under the cursor (to first order) instead of sliding off screen as the
    /// camera dollies toward the look-at point. `cursor` is CSS px, canvas-local;
    /// `delta` is a PIXEL-mode wheel delta (the host normalises delta_mode).
    ///
    /// SIGN CONTRACT: **positive `delta` dollies OUT** (distance grows), because
    /// `zoom(factor)` multiplies `tgt_distance` and `factor = exp(delta * k)`.
    /// Every caller must map its gesture onto that: a scroll *down* (deltaY > 0)
    /// and a pinch *close* both pass a positive delta. Getting this backwards is
    /// invisible in a unit test and maddening in the hand — it is why the wheel
    /// zoomed the wrong way for as long as it did.
    pub fn on_wheel(&mut self, delta: f32, cursor: (f32, f32), t_ms: f64) {
        // Zoom proportional to the actual wheel delta (device-normalized) so the
        // camera follows the wheel instead of jumping a fixed step per event.
        // Exponential keeps it smooth and symmetric across zoom in/out; the ±2
        // clamp caps a single outsized delta while letting many small trackpad
        // events accumulate naturally.
        let normalized = (delta / 100.0).clamp(-2.0, 2.0);
        let factor = (normalized * ZOOM_SENSITIVITY).exp();

        // The cursor's world position on the look-at plane, relative to the orbit
        // centre. Keeping it fixed while the distance scales by `factor` means
        // moving the centre by (1 - factor) * that offset.
        let (css_w, css_h) = (self.css_width(), self.css_height());
        let wpp = self.camera.world_per_pixel(css_h);
        let (right, up) = self.camera.screen_basis();
        let offset = right
            .scale((cursor.0 - css_w * 0.5) * wpp)
            .add(&up.scale(-(cursor.1 - css_h * 0.5) * wpp));

        self.camera.zoom(factor);
        self.camera.pan_world(offset.scale(1.0 - factor));
        self.take_camera(t_ms);
    }

    fn css_width(&self) -> f32 {
        self.width as f32 / self.dpr
    }

    fn css_height(&self) -> f32 {
        self.height as f32 / self.dpr
    }

    /// Release every GL object this Scene owns (programs, buffers, FBOs, the
    /// context itself) in one call. The host calls this from `on_cleanup`:
    /// dropping the Rust side alone frees nothing, because web-sys handles are
    /// JsValue wrappers whose GPU allocations outlive them until GC.
    ///
    /// Deletes the bloom pipeline's objects EXPLICITLY first, then loses the
    /// context as a catch-all. Order matters: `WEBGL_lose_context` is an
    /// extension, and `get_extension` is allowed to return `None` — if we relied
    /// on it alone, a browser that withholds it would leak the whole pipeline.
    pub fn dispose(&self) {
        self.bloom.destroy(&self.ctx.gl);

        let Ok(Some(ext)) = self.ctx.gl.get_extension("WEBGL_lose_context") else {
            return;
        };
        let Ok(f) = js_sys::Reflect::get(&ext, &"loseContext".into()) else {
            return;
        };
        if let Ok(f) = f.dyn_into::<js_sys::Function>() {
            let _ = f.call0(&ext);
        }
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
                self.edges.upload_indexed(
                    &self.ctx.gl,
                    &self.data.nodes,
                    &self.filtered_edges,
                    &self.filtered_edge_kinds,
                    &self.filtered_edge_bright,
                );
                self.edges.set_highlight(
                    &self.ctx.gl,
                    &self.filtered_edges,
                    self.highlight_edges.as_ref(),
                );
                self.nodes
                    .upload(&self.ctx.gl, &self.data, self.highlight.as_ref());
            }
            // The cloud is still expanding, so a fit computed once on the seed
            // positions would be stale by the time it lands: track it every frame
            // (O(n) next to the O(n²) force loop) until the user takes over.
            // Snap outward — an eased fit cannot outrun the expansion (see fit_frame).
            if self.auto_fit {
                self.fit_frame(true);
            }
            // Same for a focus target: re-aim at where the node IS, not where it
            // was when the user clicked (it moves up to MAX_STEP per step).
            let focus_idx = self
                .focus_id
                .as_ref()
                .and_then(|id| self.id_index.get(id).copied());
            if let Some(i) = focus_idx {
                self.focus_camera_on(i);
            }
            if !self.settling {
                self.focus_id = None;
            }
        } else {
            // Idle: drift is computed in the vertex shader from u_time; no CPU work,
            // no per-frame buffer re-upload. `pick` / `screen_pos_of` reproduce that
            // drift on the CPU (see `drift`) so they project what the GPU renders.
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
        let vp = self.camera.view_proj(self.aspect());
        // Store for picking, together with the exact u_time the shaders drift by.
        self.last_vp = vp;
        let u_time = (t_ms % U_TIME_WRAP_MS) as f32;
        self.last_u_time = u_time;
        self.edges
            .draw(gl, &vp, (self.width as f32, self.height as f32), u_time);
        self.nodes.draw(
            gl,
            &vp,
            (self.width as f32, self.height as f32),
            u_time,
            self.camera.distance,
        );
        // Restore blend state after scene draw — bloom passes will disable blend.
        gl.disable(Gl::BLEND);

        // --- Bloom pass: bright-pass → blur → composite to default FBO ---
        // bloom.run() manages all blend state internally (BLEND disabled).
        self.bloom.run(gl);
    }
}
