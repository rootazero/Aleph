# Memory Canvas 3D Nebula — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the Canvas2D neighborhood explorer with a WebGL2 whole-graph 3D knowledge nebula (instanced node sprites + fine edges + FBO bloom + orbit camera), rewiring all existing interactions onto it without losing functionality.

**Architecture:** A new pure-Rust WebGL2 renderer lives under `interfaces/webchat/src/views/canvas/gl/`. Pure-logic modules (`math`, `camera`, `layout3d`, `picking`) are `web-sys`-free and unit-tested on the native target. GL-bound modules (`context`, `shaders`, `nodes`, `edges`, `bloom`, `scene`) are verified by `cargo check --target wasm32` + `just wasm` + browser screenshot. The whole graph loads once via the existing `graph.query` RPC; the client computes a 3D force layout and renders it. The existing host (`canvas/mod.rs`) orchestration (agent switch, search, detail panel, list↔graph cross-link) is reused; only the rendering/view-model layer is rewritten.

**Tech Stack:** Rust, Leptos 0.8 (CSR), `wasm-bindgen` / `web-sys` (WebGL2), hand-written GLSL ES 3.00, `serde_json` for RPC. No JS, no `three.js`, no `glam`/`nalgebra` (hand-rolled `Mat4`/`Vec3`).

## Global Constraints

- **Scope:** `interfaces/webchat` only. **Core (`src/`) untouched.** No new RPC; no `serde` contract changes (reuse `GraphQueryResponse` / `NoteNodeDto` / `NoteLinkDto`).
- **Rendering:** Pure Rust WebGL2 via `web-sys`. **No JS injection, no `three.js`, no heavy 3D crate.**
- **Math:** Hand-rolled `Mat4`/`Vec3` in `gl/math.rs`. Do NOT add `glam`/`nalgebra`/`cgmath` unless `math.rs` exceeds ~200 lines (then raise with the user before adding).
- **File size:** Each `gl/` file < 400 lines (P2 high cohesion).
- **Pure-logic modules** (`math`, `camera`, `layout3d`, `picking`) MUST NOT import `web_sys` — keeps them unit-testable on native.
- **Branch:** Work only in worktree `worktree-memory-canvas-3d-nebula`. Never touch `main`.
- **Shell prefix:** Every cargo/just command MUST be preceded by `export PATH="/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:$PATH"` (non-interactive Bash lacks cargo on PATH).
- **Native test cmd:** `cargo test -p aleph-panel --lib <filter>`
- **WASM compile gate:** `cargo check -p aleph-panel --lib --target wasm32-unknown-unknown`
- **Determinism:** Layout/drift randomness derives from `fnv1a` hash of node id (reuse `canvas_engine::fnv1a`), never `Math.random`/`Date.now` — stable across reloads.
- **Commit format:** `<scope>: <description>` (English). No attribution footer (disabled globally).

---

## File Structure

**New (`interfaces/webchat/src/views/canvas/gl/`):**
- `mod.rs` — submodule declarations + shared `GraphData`/`GalaxyNode` types.
- `math.rs` — `Vec3`, `Mat4` (perspective, look_at, mul, rotate). Pure. Unit-tested.
- `camera.rs` — `OrbitCamera` (azimuth/elevation/distance, damping, zoom/pan, fly-to easing, idle auto-rotate). Pure. Unit-tested.
- `layout3d.rs` — `ForceLayout` (3D repulsion + spring + centering, step/energy/converged). Pure. Unit-tested.
- `picking.rs` — `pick_node` (project world→screen, nearest within radius). Pure. Unit-tested.
- `shaders.rs` — GLSL ES 3.00 source constants. Pure data.
- `context.rs` — `GlContext` (acquire `WebGl2RenderingContext`, compile programs, make buffers/VAOs/FBOs/textures). GL-bound.
- `nodes.rs` — `NodeRenderer` (instanced billboard sprites). GL-bound.
- `edges.rs` — `EdgeRenderer` (batched `LINES`). GL-bound.
- `bloom.rs` — `BloomPipeline` (bright-pass → separable gaussian → composite). GL-bound. Kernel weights unit-tested.
- `scene.rs` — `Scene` (owns GL state + renderers + camera + layout; `render(t)` per frame). GL-bound.

**New Leptos host:**
- `views/canvas/galaxy_canvas.rs` — `GalaxyCanvas` component: `<canvas>` node-ref, rAF loop, pointer/wheel events → `Scene` + emits `CanvasEvent`.

**Modified:**
- `interfaces/webchat/Cargo.toml` — add WebGL2 `web-sys` features.
- `views/canvas/mod.rs` — swap `GraphCanvas` → `GalaxyCanvas`, adapt data flow (whole-graph query, no per-hop refetch), reuse interaction Effects.
- `canvas_engine/mod.rs` — drop retired module declarations (Phase 4).

**Retired (Phase 4, after rewire verified):** `canvas_engine/{renderer,edge_curve,drag,tween,viewport,scatter,align_guides,interaction,navigation}.rs`; `views/canvas/{graph_canvas,minimap_view,edge_label}.rs`; `canvas_engine/mini_map.rs` (pending P4 decision).

**Reused unchanged:** `canvas_engine/{category_color,cluster,prefetch,adapter,fnv1a,markdown_excerpt}.rs`; `views/canvas/{node_detail_panel,node_card}.rs`.

---

# Phase 1 — Rendering Foundation

Deliverable: orbit-rotatable 3D scene rendering the whole graph at a mock/random layout. No bloom, no force layout yet.

## Task 1: WebGL2 features + `gl/math.rs`

**Files:**
- Modify: `interfaces/webchat/Cargo.toml` (web-sys features block)
- Create: `interfaces/webchat/src/views/canvas/gl/mod.rs`
- Create: `interfaces/webchat/src/views/canvas/gl/math.rs`
- Modify: `interfaces/webchat/src/views/canvas/mod.rs:1-8` (add `mod gl;`)

**Interfaces:**
- Produces:
  - `gl::math::Vec3 { x: f32, y: f32, z: f32 }` with `new`, `add`, `sub`, `scale`, `dot`, `cross`, `length`, `normalize`, `zero()`.
  - `gl::math::Mat4([f32; 16])` (column-major) with `identity()`, `perspective(fovy_rad, aspect, near, far)`, `look_at(eye: Vec3, center: Vec3, up: Vec3)`, `mul(&self, &Mat4) -> Mat4`, `as_slice(&self) -> &[f32; 16]`.
  - `gl::GraphData { nodes: Vec<GalaxyNode>, edges: Vec<(u32, u32)> }`, `gl::GalaxyNode { id: String, name: String, category: String, link_count: u32, pos: Vec3, color: [f32; 3] }`.

- [ ] **Step 1: Add WebGL2 web-sys features**

In `interfaces/webchat/Cargo.toml`, inside the existing `web-sys = { ... features = [ ... ] }` array, append a new group:

```toml
    # 3D memory nebula — WebGL2 renderer (views/canvas/gl)
    "WebGl2RenderingContext", "WebGlProgram", "WebGlShader", "WebGlBuffer",
    "WebGlVertexArrayObject", "WebGlUniformLocation", "WebGlFramebuffer",
    "WebGlTexture", "WebGlActiveInfo",
```

- [ ] **Step 2: Create `gl/mod.rs` with shared types**

```rust
//! Pure-Rust WebGL2 renderer for the 3D knowledge nebula.
//!
//! Pure-logic submodules (`math`, `camera`, `layout3d`, `picking`) are
//! `web-sys`-free and unit-tested on the native target. GL-bound submodules
//! are verified by wasm compile + browser.
pub mod camera;
pub mod layout3d;
pub mod math;
pub mod picking;

use math::Vec3;

/// One renderable node in the galaxy. `pos` is mutated by the force layout;
/// everything else is derived once from the RPC DTO.
#[derive(Debug, Clone)]
pub struct GalaxyNode {
    pub id: String,
    pub name: String,
    pub category: String,
    pub link_count: u32,
    pub pos: Vec3,
    /// Base RGB in [0,1] (category color, pre-HDR-boost).
    pub color: [f32; 3],
}

/// The whole-graph render input. `edges` index into `nodes` (resolved from ids).
#[derive(Debug, Clone, Default)]
pub struct GraphData {
    pub nodes: Vec<GalaxyNode>,
    pub edges: Vec<(u32, u32)>,
}
```

- [ ] **Step 3: Write failing tests for `math.rs`**

Create `gl/math.rs` with tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) { assert!((a - b).abs() < 1e-4, "{a} vs {b}"); }

    #[test]
    fn vec3_cross_and_normalize() {
        let x = Vec3::new(1.0, 0.0, 0.0);
        let y = Vec3::new(0.0, 1.0, 0.0);
        let z = x.cross(&y);
        approx(z.x, 0.0); approx(z.y, 0.0); approx(z.z, 1.0);
        let n = Vec3::new(3.0, 0.0, 4.0).normalize();
        approx(n.length(), 1.0);
        approx(n.x, 0.6); approx(n.z, 0.8);
    }

    #[test]
    fn mat4_identity_mul_is_identity() {
        let m = Mat4::perspective(1.0, 1.5, 0.1, 100.0);
        let i = Mat4::identity();
        let p = m.mul(&i);
        for k in 0..16 { approx(p.as_slice()[k], m.as_slice()[k]); }
    }

    #[test]
    fn perspective_diagonal_signs() {
        // Standard GL perspective: [0]>0, [5]>0, [10]<0 (z maps to -1..1), [11]==-1.
        let p = Mat4::perspective(std::f32::consts::FRAC_PI_2, 1.0, 0.1, 100.0);
        let s = p.as_slice();
        assert!(s[0] > 0.0 && s[5] > 0.0);
        assert!(s[10] < 0.0);
        approx(s[11], -1.0);
    }

    #[test]
    fn look_at_origin_down_neg_z_is_identity_rotation() {
        // Eye at +z looking at origin with +y up → camera space == world with z flipped.
        let m = Mat4::look_at(Vec3::new(0.0, 0.0, 5.0), Vec3::zero(), Vec3::new(0.0, 1.0, 0.0));
        let s = m.as_slice();
        approx(s[0], 1.0);  // right.x
        approx(s[5], 1.0);  // up.y
        approx(s[10], 1.0); // -forward.z (forward = -z → -(-1)=1)
    }
}
```

- [ ] **Step 4: Run tests to verify they fail**

Run: `export PATH="/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:$PATH" && cargo test -p aleph-panel --lib gl::math 2>&1 | tail -15`
Expected: FAIL (compile error — `Vec3`/`Mat4` undefined).

- [ ] **Step 5: Implement `math.rs`**

Prepend above the test module:

```rust
//! Minimal column-major 3D math. No external deps (Global Constraint).

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub fn new(x: f32, y: f32, z: f32) -> Self { Self { x, y, z } }
    pub fn zero() -> Self { Self { x: 0.0, y: 0.0, z: 0.0 } }
    pub fn add(&self, o: &Vec3) -> Vec3 { Vec3::new(self.x + o.x, self.y + o.y, self.z + o.z) }
    pub fn sub(&self, o: &Vec3) -> Vec3 { Vec3::new(self.x - o.x, self.y - o.y, self.z - o.z) }
    pub fn scale(&self, s: f32) -> Vec3 { Vec3::new(self.x * s, self.y * s, self.z * s) }
    pub fn dot(&self, o: &Vec3) -> f32 { self.x * o.x + self.y * o.y + self.z * o.z }
    pub fn cross(&self, o: &Vec3) -> Vec3 {
        Vec3::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }
    pub fn length(&self) -> f32 { self.dot(self).sqrt() }
    pub fn normalize(&self) -> Vec3 {
        let l = self.length();
        if l > 1e-8 { self.scale(1.0 / l) } else { *self }
    }
}

/// Column-major 4x4 matrix (OpenGL convention). `m[col*4 + row]`.
#[derive(Debug, Clone, Copy)]
pub struct Mat4(pub [f32; 16]);

impl Mat4 {
    pub fn identity() -> Mat4 {
        let mut m = [0.0; 16];
        m[0] = 1.0; m[5] = 1.0; m[10] = 1.0; m[15] = 1.0;
        Mat4(m)
    }
    pub fn as_slice(&self) -> &[f32; 16] { &self.0 }

    pub fn perspective(fovy: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
        let f = 1.0 / (fovy / 2.0).tan();
        let nf = 1.0 / (near - far);
        let mut m = [0.0; 16];
        m[0] = f / aspect;
        m[5] = f;
        m[10] = (far + near) * nf;
        m[11] = -1.0;
        m[14] = 2.0 * far * near * nf;
        Mat4(m)
    }

    pub fn look_at(eye: Vec3, center: Vec3, up: Vec3) -> Mat4 {
        let f = center.sub(&eye).normalize(); // forward
        let s = f.cross(&up).normalize();     // right
        let u = s.cross(&f);                  // true up
        let mut m = [0.0; 16];
        m[0] = s.x; m[4] = s.y; m[8] = s.z;
        m[1] = u.x; m[5] = u.y; m[9] = u.z;
        m[2] = -f.x; m[6] = -f.y; m[10] = -f.z;
        m[12] = -s.dot(&eye);
        m[13] = -u.dot(&eye);
        m[14] = f.dot(&eye);
        m[15] = 1.0;
        Mat4(m)
    }

    /// self * rhs (column-major).
    pub fn mul(&self, rhs: &Mat4) -> Mat4 {
        let a = &self.0;
        let b = &rhs.0;
        let mut out = [0.0; 16];
        for col in 0..4 {
            for row in 0..4 {
                let mut sum = 0.0;
                for k in 0..4 {
                    sum += a[k * 4 + row] * b[col * 4 + k];
                }
                out[col * 4 + row] = sum;
            }
        }
        Mat4(out)
    }
}
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `export PATH="/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:$PATH" && cargo test -p aleph-panel --lib gl::math 2>&1 | tail -15`
Expected: PASS (4 tests). Then add `pub mod gl;` is NOT needed — `mod.rs` declares it; add `mod gl;` to `views/canvas/mod.rs` after line 7 (`mod node_detail_panel;`).

- [ ] **Step 7: WASM compile gate + commit**

Run: `export PATH="/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:$PATH" && cargo check -p aleph-panel --lib --target wasm32-unknown-unknown 2>&1 | tail -5`
Expected: `Finished`.

```bash
git add interfaces/webchat/Cargo.toml interfaces/webchat/src/views/canvas/gl/ interfaces/webchat/src/views/canvas/mod.rs
git commit -m "canvas: add WebGL2 features + gl::math foundation"
```

---

## Task 2: `gl/camera.rs` — orbit camera

**Files:**
- Create: `interfaces/webchat/src/views/canvas/gl/camera.rs`
- Modify: `gl/mod.rs` (already declares `pub mod camera;`)

**Interfaces:**
- Consumes: `gl::math::{Vec3, Mat4}`.
- Produces: `OrbitCamera` with:
  - `new(distance: f32) -> Self`
  - `orbit(&mut self, d_az: f32, d_el: f32)` — drag delta (radians).
  - `zoom(&mut self, factor: f32)` — wheel (clamps distance to `[MIN_DIST, MAX_DIST]`).
  - `fly_to(&mut self, target: Vec3, distance: f32)` — start eased move.
  - `note_interaction(&mut self, t_ms: f64)` / `update(&mut self, t_ms: f64, dt_ms: f32)` — advances damping, fly-to easing, idle auto-rotate.
  - `view_proj(&self, aspect: f32) -> Mat4` — `perspective * look_at`.
  - `eye(&self) -> Vec3`, `target(&self) -> Vec3`.
  - consts `MIN_DIST: f32 = 10.0`, `MAX_DIST: f32 = 50000.0`, `IDLE_MS: f64 = 60_000.0`.

- [ ] **Step 1: Write failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::canvas::gl::math::Vec3;

    #[test]
    fn zoom_clamps_distance() {
        let mut c = OrbitCamera::new(100.0);
        c.zoom(0.0001); // zoom way in
        assert!(c.distance >= OrbitCamera::MIN_DIST);
        c.zoom(100000.0); // zoom way out
        assert!(c.distance <= OrbitCamera::MAX_DIST);
    }

    #[test]
    fn orbit_changes_eye_position() {
        let mut c = OrbitCamera::new(100.0);
        let e0 = c.eye();
        c.orbit(0.5, 0.2);
        // damping means eye moves toward new orbit over update()s
        for _ in 0..120 { c.update(0.0, 16.0); }
        let e1 = c.eye();
        assert!((e0.x - e1.x).abs() + (e0.y - e1.y).abs() + (e0.z - e1.z).abs() > 1.0);
    }

    #[test]
    fn fly_to_converges_target() {
        let mut c = OrbitCamera::new(100.0);
        c.fly_to(Vec3::new(50.0, 0.0, 0.0), 200.0);
        for _ in 0..300 { c.update(0.0, 16.0); }
        let t = c.target();
        assert!((t.x - 50.0).abs() < 1.0, "target.x={}", t.x);
    }

    #[test]
    fn idle_autorotate_after_timeout_only() {
        let mut c = OrbitCamera::new(100.0);
        c.note_interaction(0.0);
        let az_before = c.azimuth;
        c.update(1000.0, 16.0); // 1s < IDLE_MS → no autorotate
        approx_eq(c.azimuth, az_before);
        c.update(OrbitCamera::IDLE_MS + 100.0, 16.0); // past idle → rotates
        assert!(c.azimuth != az_before);
    }

    fn approx_eq(a: f32, b: f32) { assert!((a - b).abs() < 1e-4); }
}
```

- [ ] **Step 2: Run to verify fail**

Run: `export PATH="/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:$PATH" && cargo test -p aleph-panel --lib gl::camera 2>&1 | tail -15`
Expected: FAIL (`OrbitCamera` undefined).

- [ ] **Step 3: Implement `camera.rs`**

```rust
//! Orbit camera with damping, fly-to easing, and idle auto-rotate.
//! Pure (no web-sys) so it unit-tests on native.

use super::math::{Mat4, Vec3};

const DAMPING: f32 = 0.12;          // approach rate per update toward target angles
const AUTOROTATE_RAD_PER_MS: f32 = 0.4 / 1000.0; // ~0.4 rad/s (ref autoRotateSpeed)
const FLY_RATE: f32 = 0.10;         // ease-out approach per update toward fly target

pub struct OrbitCamera {
    // Current (rendered) orbit.
    pub azimuth: f32,
    pub elevation: f32,
    pub distance: f32,
    center: Vec3,
    // Damping targets.
    tgt_azimuth: f32,
    tgt_elevation: f32,
    tgt_distance: f32,
    tgt_center: Vec3,
    last_interaction_ms: f64,
}

impl OrbitCamera {
    pub const MIN_DIST: f32 = 10.0;
    pub const MAX_DIST: f32 = 50000.0;
    pub const IDLE_MS: f64 = 60_000.0;
    const FOVY: f32 = std::f32::consts::PI * 50.0 / 180.0;

    pub fn new(distance: f32) -> Self {
        let d = distance.clamp(Self::MIN_DIST, Self::MAX_DIST);
        Self {
            azimuth: 0.6, elevation: 0.35, distance: d, center: Vec3::zero(),
            tgt_azimuth: 0.6, tgt_elevation: 0.35, tgt_distance: d, tgt_center: Vec3::zero(),
            last_interaction_ms: 0.0,
        }
    }

    pub fn orbit(&mut self, d_az: f32, d_el: f32) {
        self.tgt_azimuth += d_az;
        let lim = std::f32::consts::FRAC_PI_2 - 0.05;
        self.tgt_elevation = (self.tgt_elevation + d_el).clamp(-lim, lim);
    }

    pub fn zoom(&mut self, factor: f32) {
        self.tgt_distance = (self.tgt_distance * factor).clamp(Self::MIN_DIST, Self::MAX_DIST);
    }

    pub fn fly_to(&mut self, target: Vec3, distance: f32) {
        self.tgt_center = target;
        self.tgt_distance = distance.clamp(Self::MIN_DIST, Self::MAX_DIST);
    }

    pub fn note_interaction(&mut self, t_ms: f64) { self.last_interaction_ms = t_ms; }

    pub fn update(&mut self, t_ms: f64, _dt_ms: f32) {
        // Idle auto-rotate (only past timeout; resets damping target).
        if t_ms - self.last_interaction_ms > Self::IDLE_MS {
            self.tgt_azimuth += AUTOROTATE_RAD_PER_MS * 16.0;
        }
        // Critically-ish damped approach.
        self.azimuth += (self.tgt_azimuth - self.azimuth) * DAMPING;
        self.elevation += (self.tgt_elevation - self.elevation) * DAMPING;
        self.distance += (self.tgt_distance - self.distance) * DAMPING;
        self.center = self.center.add(&self.tgt_center.sub(&self.center).scale(FLY_RATE));
    }

    pub fn eye(&self) -> Vec3 {
        let ce = self.elevation.cos();
        Vec3::new(
            self.center.x + self.distance * ce * self.azimuth.sin(),
            self.center.y + self.distance * self.elevation.sin(),
            self.center.z + self.distance * ce * self.azimuth.cos(),
        )
    }

    pub fn target(&self) -> Vec3 { self.center }

    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        let proj = Mat4::perspective(Self::FOVY, aspect.max(0.01), 0.1, 200_000.0);
        let view = Mat4::look_at(self.eye(), self.center, Vec3::new(0.0, 1.0, 0.0));
        proj.mul(&view)
    }
}
```

- [ ] **Step 4: Run to verify pass**

Run: `export PATH="/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:$PATH" && cargo test -p aleph-panel --lib gl::camera 2>&1 | tail -15`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/views/canvas/gl/camera.rs
git commit -m "canvas: gl::camera orbit camera with damping + fly-to + idle rotate"
```

---

## Task 3: `gl/context.rs` — WebGL2 context + resources

**Files:**
- Create: `interfaces/webchat/src/views/canvas/gl/context.rs`
- Modify: `gl/mod.rs` (add `pub mod context; pub mod shaders;`)
- Create: `interfaces/webchat/src/views/canvas/gl/shaders.rs` (empty placeholder consts filled in Tasks 4-5; see below)

**Interfaces:**
- Consumes: `web_sys::{WebGl2RenderingContext, HtmlCanvasElement, WebGlProgram, WebGlShader, WebGlBuffer, WebGlVertexArrayObject}`.
- Produces:
  - `GlContext { pub gl: WebGl2RenderingContext }` with `from_canvas(canvas: &HtmlCanvasElement) -> Result<GlContext, String>`.
  - `compile_program(gl, vert_src: &str, frag_src: &str) -> Result<WebGlProgram, String>` (free fn).
  - `GlContext::resize(&self, w: i32, h: i32)` (sets viewport).
  - GL is GLES 3.00 (WebGL2): request context with `{ antialias: true, alpha: false }`.

- [ ] **Step 1: Create `shaders.rs` stub**

```rust
//! GLSL ES 3.00 shader sources. Filled per-renderer in Tasks 4-6 + Phase 2.
```

- [ ] **Step 2: Implement `context.rs`** (GL-bound — no unit test; compile gate only)

```rust
//! WebGL2 context acquisition + GLSL program compilation.

use wasm_bindgen::JsCast;
use web_sys::{
    HtmlCanvasElement, WebGl2RenderingContext as Gl, WebGlProgram, WebGlShader,
};

pub struct GlContext {
    pub gl: Gl,
}

impl GlContext {
    pub fn from_canvas(canvas: &HtmlCanvasElement) -> Result<GlContext, String> {
        let opts = js_sys::Object::new();
        let set = |k: &str, v: wasm_bindgen::JsValue| {
            let _ = js_sys::Reflect::set(&opts, &k.into(), &v);
        };
        set("antialias", wasm_bindgen::JsValue::TRUE);
        set("alpha", wasm_bindgen::JsValue::FALSE);
        set("depth", wasm_bindgen::JsValue::TRUE);
        let ctx = canvas
            .get_context_with_context_options("webgl2", &opts)
            .map_err(|_| "get_context webgl2 threw".to_string())?
            .ok_or_else(|| "WebGL2 unavailable".to_string())?;
        let gl = ctx
            .dyn_into::<Gl>()
            .map_err(|_| "context is not WebGl2RenderingContext".to_string())?;
        Ok(GlContext { gl })
    }

    pub fn resize(&self, w: i32, h: i32) {
        self.gl.viewport(0, 0, w, h);
    }
}

pub fn compile_program(gl: &Gl, vert_src: &str, frag_src: &str) -> Result<WebGlProgram, String> {
    let vs = compile_shader(gl, Gl::VERTEX_SHADER, vert_src)?;
    let fs = compile_shader(gl, Gl::FRAGMENT_SHADER, frag_src)?;
    let prog = gl.create_program().ok_or("create_program failed")?;
    gl.attach_shader(&prog, &vs);
    gl.attach_shader(&prog, &fs);
    gl.link_program(&prog);
    if gl
        .get_program_parameter(&prog, Gl::LINK_STATUS)
        .as_bool()
        .unwrap_or(false)
    {
        Ok(prog)
    } else {
        Err(gl.get_program_info_log(&prog).unwrap_or_default())
    }
}

fn compile_shader(gl: &Gl, kind: u32, src: &str) -> Result<WebGlShader, String> {
    let sh = gl.create_shader(kind).ok_or("create_shader failed")?;
    gl.shader_source(&sh, src);
    gl.compile_shader(&sh);
    if gl
        .get_shader_parameter(&sh, Gl::COMPILE_STATUS)
        .as_bool()
        .unwrap_or(false)
    {
        Ok(sh)
    } else {
        Err(gl.get_shader_info_log(&sh).unwrap_or_default())
    }
}
```

Add `"HtmlCanvasElement"` is already in web-sys features. Add `pub mod context; pub mod shaders;` to `gl/mod.rs`.

- [ ] **Step 3: WASM compile gate**

Run: `export PATH="/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:$PATH" && cargo check -p aleph-panel --lib --target wasm32-unknown-unknown 2>&1 | tail -8`
Expected: `Finished` (no errors).

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/views/canvas/gl/context.rs interfaces/webchat/src/views/canvas/gl/shaders.rs interfaces/webchat/src/views/canvas/gl/mod.rs
git commit -m "canvas: gl::context WebGL2 acquisition + program compile"
```

---

## Task 4: `gl/nodes.rs` — instanced billboard sprites

**Files:**
- Create: `interfaces/webchat/src/views/canvas/gl/nodes.rs`
- Modify: `gl/shaders.rs` (add node shaders)
- Modify: `gl/mod.rs` (`pub mod nodes;`)

**Interfaces:**
- Consumes: `GlContext`, `compile_program`, `GraphData`, `Mat4`.
- Produces:
  - `NodeRenderer::new(gl: &Gl) -> Result<NodeRenderer, String>`
  - `NodeRenderer::upload(&self, gl: &Gl, data: &GraphData, highlighted: Option<&std::collections::HashSet<u32>>)` — fills per-instance position/size/color buffers (applies HDR boost + highlight dimming).
  - `NodeRenderer::draw(&self, gl: &Gl, view_proj: &Mat4)` — one instanced draw call.

- [ ] **Step 1: Add node shaders to `shaders.rs`**

```rust
/// Node billboard vertex shader. Per-instance: a_offset(vec3), a_size(float),
/// a_color(vec3). Per-vertex: a_corner(vec2 in [-1,1]).
pub const NODE_VERT: &str = r#"#version 300 es
precision highp float;
layout(location=0) in vec2 a_corner;
layout(location=1) in vec3 a_offset;
layout(location=2) in float a_size;
layout(location=3) in vec3 a_color;
uniform mat4 u_view_proj;
uniform vec2 u_viewport;
out vec2 v_corner;
out vec3 v_color;
void main() {
    vec4 clip = u_view_proj * vec4(a_offset, 1.0);
    // Billboard: expand in clip space by pixel size, perspective-correct.
    vec2 px = a_corner * a_size / u_viewport * clip.w * 2.0;
    clip.xy += px;
    gl_Position = clip;
    v_corner = a_corner;
    v_color = a_color;
}
"#;

/// Node fragment shader: soft radial star sprite with HDR color (toneMapped off).
pub const NODE_FRAG: &str = r#"#version 300 es
precision highp float;
in vec2 v_corner;
in vec3 v_color;
out vec4 frag;
void main() {
    float r = length(v_corner);
    if (r > 1.0) discard;
    float a = smoothstep(1.0, 0.0, r);     // soft edge
    float core = smoothstep(0.6, 0.0, r);  // bright core
    frag = vec4(v_color * (0.4 + core), a);
}
"#;
```

- [ ] **Step 2: Implement `nodes.rs`** (GL-bound)

```rust
//! Instanced billboard-sprite node renderer (one draw call for all nodes).

use std::collections::HashSet;
use web_sys::{WebGl2RenderingContext as Gl, WebGlBuffer, WebGlProgram, WebGlVertexArrayObject};

use super::context::compile_program;
use super::math::Mat4;
use super::{shaders, GraphData};

pub struct NodeRenderer {
    prog: WebGlProgram,
    vao: WebGlVertexArrayObject,
    inst_offset: WebGlBuffer,
    inst_size: WebGlBuffer,
    inst_color: WebGlBuffer,
    count: i32,
}

const CORNERS: [f32; 12] = [
    -1.0, -1.0, 1.0, -1.0, -1.0, 1.0, // tri 1
    -1.0, 1.0, 1.0, -1.0, 1.0, 1.0,   // tri 2
];

impl NodeRenderer {
    pub fn new(gl: &Gl) -> Result<NodeRenderer, String> {
        let prog = compile_program(gl, shaders::NODE_VERT, shaders::NODE_FRAG)?;
        let vao = gl.create_vertex_array().ok_or("vao")?;
        gl.bind_vertex_array(Some(&vao));

        // a_corner (location 0) — static quad.
        let corner_buf = gl.create_buffer().ok_or("corner buf")?;
        gl.bind_buffer(Gl::ARRAY_BUFFER, Some(&corner_buf));
        unsafe {
            let view = js_sys::Float32Array::view(&CORNERS);
            gl.buffer_data_with_array_buffer_view(Gl::ARRAY_BUFFER, &view, Gl::STATIC_DRAW);
        }
        gl.enable_vertex_attrib_array(0);
        gl.vertex_attrib_pointer_with_i32(0, 2, Gl::FLOAT, false, 0, 0);

        let inst_offset = gl.create_buffer().ok_or("offset buf")?;
        let inst_size = gl.create_buffer().ok_or("size buf")?;
        let inst_color = gl.create_buffer().ok_or("color buf")?;
        // a_offset (1) vec3, a_size (2) float, a_color (3) vec3 — all per-instance.
        Self::setup_instanced(gl, &inst_offset, 1, 3);
        Self::setup_instanced(gl, &inst_size, 2, 1);
        Self::setup_instanced(gl, &inst_color, 3, 3);

        gl.bind_vertex_array(None);
        Ok(NodeRenderer { prog, vao, inst_offset, inst_size, inst_color, count: 0 })
    }

    fn setup_instanced(gl: &Gl, buf: &WebGlBuffer, loc: u32, size: i32) {
        gl.bind_buffer(Gl::ARRAY_BUFFER, Some(buf));
        gl.enable_vertex_attrib_array(loc);
        gl.vertex_attrib_pointer_with_i32(loc, size, Gl::FLOAT, false, 0, 0);
        gl.vertex_attrib_divisor(loc, 1);
    }

    pub fn upload(&mut self, gl: &Gl, data: &GraphData, hl: Option<&HashSet<u32>>) {
        let n = data.nodes.len();
        let mut offsets = Vec::with_capacity(n * 3);
        let mut sizes = Vec::with_capacity(n);
        let mut colors = Vec::with_capacity(n * 3);
        let has_hl = hl.map(|s| !s.is_empty()).unwrap_or(false);
        for (i, node) in data.nodes.iter().enumerate() {
            offsets.extend_from_slice(&[node.pos.x, node.pos.y, node.pos.z]);
            // size grows with degree; ref: highlighted 0.5x, dimmed 0.2x of base.
            let base = 6.0 + (node.link_count as f32).sqrt() * 4.0;
            let lit = !has_hl || hl.map(|s| s.contains(&(i as u32))).unwrap_or(true);
            sizes.push(if lit { base } else { base * 0.5 });
            let [r, g, b] = node.color;
            if lit {
                // HDR boost so bloom picks up a glow corona (ref: 1.2 + brightness*0.8).
                let boost = 1.2 + ((r + g + b) / 3.0) * 0.8;
                colors.extend_from_slice(&[r * boost, g * boost, b * boost]);
            } else {
                colors.extend_from_slice(&[r * 0.15, g * 0.15, b * 0.15]);
            }
        }
        self.count = n as i32;
        upload_f32(gl, &self.inst_offset, &offsets);
        upload_f32(gl, &self.inst_size, &sizes);
        upload_f32(gl, &self.inst_color, &colors);
    }

    pub fn draw(&self, gl: &Gl, view_proj: &Mat4, viewport: (f32, f32)) {
        if self.count == 0 { return; }
        gl.use_program(Some(&self.prog));
        gl.bind_vertex_array(Some(&self.vao));
        set_mat4(gl, &self.prog, "u_view_proj", view_proj);
        set_vec2(gl, &self.prog, "u_viewport", viewport);
        gl.draw_arrays_instanced(Gl::TRIANGLES, 0, 6, self.count);
        gl.bind_vertex_array(None);
    }
}

fn upload_f32(gl: &Gl, buf: &WebGlBuffer, data: &[f32]) {
    gl.bind_buffer(Gl::ARRAY_BUFFER, Some(buf));
    unsafe {
        let view = js_sys::Float32Array::view(data);
        gl.buffer_data_with_array_buffer_view(Gl::ARRAY_BUFFER, &view, Gl::DYNAMIC_DRAW);
    }
}

pub(super) fn set_mat4(gl: &Gl, prog: &WebGlProgram, name: &str, m: &Mat4) {
    let loc = gl.get_uniform_location(prog, name);
    gl.uniform_matrix4fv_with_f32_array(loc.as_ref(), false, m.as_slice());
}

pub(super) fn set_vec2(gl: &Gl, prog: &WebGlProgram, name: &str, v: (f32, f32)) {
    let loc = gl.get_uniform_location(prog, name);
    gl.uniform2f(loc.as_ref(), v.0, v.1);
}
```

- [ ] **Step 3: WASM compile gate + commit**

Run: `export PATH="/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:$PATH" && cargo check -p aleph-panel --lib --target wasm32-unknown-unknown 2>&1 | tail -8`
Expected: `Finished`.

```bash
git add interfaces/webchat/src/views/canvas/gl/nodes.rs interfaces/webchat/src/views/canvas/gl/shaders.rs interfaces/webchat/src/views/canvas/gl/mod.rs
git commit -m "canvas: gl::nodes instanced billboard sprite renderer"
```

---

## Task 5: `gl/edges.rs` — batched fine lines

**Files:**
- Create: `interfaces/webchat/src/views/canvas/gl/edges.rs`
- Modify: `gl/shaders.rs` (edge shaders), `gl/mod.rs` (`pub mod edges;`)

**Interfaces:**
- Consumes: `GlContext`, `GraphData`, `Mat4`.
- Produces: `EdgeRenderer::new(gl) -> Result<_, String>`, `upload(&mut self, gl, data)`, `draw(&self, gl, view_proj)`. Uses additive blending (`SRC_ALPHA, ONE`) for star-filament look.

- [ ] **Step 1: Add edge shaders to `shaders.rs`**

```rust
pub const EDGE_VERT: &str = r#"#version 300 es
precision highp float;
layout(location=0) in vec3 a_pos;
layout(location=1) in vec3 a_color;
uniform mat4 u_view_proj;
out vec3 v_color;
out float v_depth;
void main() {
    vec4 clip = u_view_proj * vec4(a_pos, 1.0);
    gl_Position = clip;
    v_color = a_color;
    v_depth = clip.z / clip.w; // [-1,1] for distance fade
}
"#;

pub const EDGE_FRAG: &str = r#"#version 300 es
precision highp float;
in vec3 v_color;
in float v_depth;
out vec4 frag;
void main() {
    float fade = clamp(1.0 - (v_depth * 0.5 + 0.5) * 0.7, 0.15, 1.0);
    frag = vec4(v_color * fade, fade * 0.35); // thin, faint, additive
}
"#;
```

- [ ] **Step 2: Implement `edges.rs`** (GL-bound). Two `f32` buffers: positions (2 verts/edge × 3), colors (endpoint node color). One `gl.draw_arrays(LINES, 0, 2*edge_count)`. Reuse `set_mat4` from `nodes`.

```rust
//! Batched line-segment edge renderer (additive blend = star filaments).

use web_sys::{WebGl2RenderingContext as Gl, WebGlBuffer, WebGlProgram, WebGlVertexArrayObject};

use super::context::compile_program;
use super::math::Mat4;
use super::nodes::set_mat4;
use super::{shaders, GraphData};

pub struct EdgeRenderer {
    prog: WebGlProgram,
    vao: WebGlVertexArrayObject,
    pos_buf: WebGlBuffer,
    col_buf: WebGlBuffer,
    vert_count: i32,
}

impl EdgeRenderer {
    pub fn new(gl: &Gl) -> Result<EdgeRenderer, String> {
        let prog = compile_program(gl, shaders::EDGE_VERT, shaders::EDGE_FRAG)?;
        let vao = gl.create_vertex_array().ok_or("edge vao")?;
        gl.bind_vertex_array(Some(&vao));
        let pos_buf = gl.create_buffer().ok_or("edge pos")?;
        gl.bind_buffer(Gl::ARRAY_BUFFER, Some(&pos_buf));
        gl.enable_vertex_attrib_array(0);
        gl.vertex_attrib_pointer_with_i32(0, 3, Gl::FLOAT, false, 0, 0);
        let col_buf = gl.create_buffer().ok_or("edge col")?;
        gl.bind_buffer(Gl::ARRAY_BUFFER, Some(&col_buf));
        gl.enable_vertex_attrib_array(1);
        gl.vertex_attrib_pointer_with_i32(1, 3, Gl::FLOAT, false, 0, 0);
        gl.bind_vertex_array(None);
        Ok(EdgeRenderer { prog, vao, pos_buf, col_buf, vert_count: 0 })
    }

    pub fn upload(&mut self, gl: &Gl, data: &GraphData) {
        let mut pos = Vec::with_capacity(data.edges.len() * 6);
        let mut col = Vec::with_capacity(data.edges.len() * 6);
        for &(a, b) in &data.edges {
            let (na, nb) = (&data.nodes[a as usize], &data.nodes[b as usize]);
            pos.extend_from_slice(&[na.pos.x, na.pos.y, na.pos.z, nb.pos.x, nb.pos.y, nb.pos.z]);
            col.extend_from_slice(&na.color);
            col.extend_from_slice(&nb.color);
        }
        self.vert_count = (data.edges.len() * 2) as i32;
        bind_upload(gl, &self.pos_buf, &pos);
        bind_upload(gl, &self.col_buf, &col);
    }

    pub fn draw(&self, gl: &Gl, view_proj: &Mat4) {
        if self.vert_count == 0 { return; }
        gl.use_program(Some(&self.prog));
        gl.bind_vertex_array(Some(&self.vao));
        set_mat4(gl, &self.prog, "u_view_proj", view_proj);
        gl.draw_arrays(Gl::LINES, 0, self.vert_count);
        gl.bind_vertex_array(None);
    }
}

fn bind_upload(gl: &Gl, buf: &WebGlBuffer, data: &[f32]) {
    gl.bind_buffer(Gl::ARRAY_BUFFER, Some(buf));
    unsafe {
        let view = js_sys::Float32Array::view(data);
        gl.buffer_data_with_array_buffer_view(Gl::ARRAY_BUFFER, &view, Gl::DYNAMIC_DRAW);
    }
}
```

- [ ] **Step 3: WASM compile gate + commit**

Run: `export PATH="/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:$PATH" && cargo check -p aleph-panel --lib --target wasm32-unknown-unknown 2>&1 | tail -8`
Expected: `Finished`.

```bash
git add interfaces/webchat/src/views/canvas/gl/edges.rs interfaces/webchat/src/views/canvas/gl/shaders.rs interfaces/webchat/src/views/canvas/gl/mod.rs
git commit -m "canvas: gl::edges batched additive line renderer"
```

---

## Task 6: `gl/scene.rs` + `GalaxyCanvas` host — render whole graph (mock layout)

**Files:**
- Create: `interfaces/webchat/src/views/canvas/gl/scene.rs`
- Create: `interfaces/webchat/src/views/canvas/galaxy_canvas.rs`
- Modify: `gl/mod.rs` (`pub mod scene;`), `views/canvas/mod.rs` (`mod galaxy_canvas;` + render `GalaxyCanvas` in `RadialCanvasView` in place of `GraphCanvas` — temporary mock-data path).

**Interfaces:**
- Consumes: all gl renderers + `OrbitCamera` + `GraphData`.
- Produces:
  - `Scene::new(canvas: &HtmlCanvasElement) -> Result<Scene, String>`
  - `Scene::set_graph(&mut self, data: GraphData)`
  - `Scene::on_drag(&mut self, dx: f32, dy: f32, t_ms: f64)`, `on_wheel(&mut self, delta: f32, t_ms: f64)`, `resize(&mut self, w: i32, h: i32)`.
  - `Scene::render(&mut self, t_ms: f64)` — clears, updates camera, draws edges then nodes.
  - `GalaxyCanvas` Leptos `#[component]` taking `graph_state: RwSignal<Option<GraphData>>` + `on_event: Callback<CanvasEvent>`.

- [ ] **Step 1: Implement `scene.rs`** (GL-bound)

```rust
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
            ctx, nodes, edges,
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
        self.width = w; self.height = h;
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
}
```

- [ ] **Step 2: Implement `galaxy_canvas.rs`** (Leptos host with rAF loop)

Mirror the rAF/NodeRef/ResizeObserver pattern in the existing `graph_canvas.rs:1-75` (read it for the exact `request_animation_frame` closure idiom and `IntersectionObserver` visibility pause). Key structure:

```rust
//! WebGL2 galaxy canvas host. Owns the <canvas>, rAF loop, pointer events.

use std::cell::RefCell;
use std::rc::Rc;
use leptos::prelude::*;
use leptos::callback::Callback;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

use crate::canvas_engine::interaction::CanvasEvent;
use super::gl::scene::Scene;
use super::gl::GraphData;

#[component]
#[must_use]
pub fn GalaxyCanvas(
    graph: RwSignal<Option<GraphData>>,
    #[allow(unused)] on_event: Callback<CanvasEvent>,
) -> impl IntoView {
    let canvas_ref = NodeRef::<leptos::html::Canvas>::new();
    let scene: Rc<RefCell<Option<Scene>>> = Rc::new(RefCell::new(None));

    // Init scene once the <canvas> mounts; start rAF loop.
    let scene_init = scene.clone();
    Effect::new(move |_| {
        let Some(canvas) = canvas_ref.get() else { return };
        let el: web_sys::HtmlCanvasElement = canvas.unchecked_into();
        // Size to client box.
        el.set_width(el.client_width().max(1) as u32);
        el.set_height(el.client_height().max(1) as u32);
        match Scene::new(&el) {
            Ok(s) => *scene_init.borrow_mut() = Some(s),
            Err(e) => { web_sys::console::error_1(&format!("GL init failed: {e}").into()); return; }
        }
        start_raf_loop(scene_init.clone());
    });

    // Push graph data into the scene when it changes.
    let scene_data = scene.clone();
    Effect::new(move |_| {
        if let Some(data) = graph.get() {
            if let Some(s) = scene_data.borrow_mut().as_mut() { s.set_graph(data); }
        }
    });

    // Pointer drag → camera orbit; wheel → zoom. (attach on:pointermove/on:wheel
    // to the <canvas>, tracking last pointer pos in an Rc<Cell<(f32,f32)>>; call
    // scene.on_drag / on_wheel with performance.now()).

    view! {
        <canvas node_ref=canvas_ref class="w-full h-full block" />
    }
}

fn start_raf_loop(scene: Rc<RefCell<Option<Scene>>>) {
    let cb = Rc::new(RefCell::new(None::<Closure<dyn FnMut(f64)>>));
    let cb2 = cb.clone();
    *cb.borrow_mut() = Some(Closure::wrap(Box::new(move |t: f64| {
        if let Some(s) = scene.borrow_mut().as_mut() { s.render(t); }
        request_af(cb2.borrow().as_ref().unwrap());
    }) as Box<dyn FnMut(f64)>));
    request_af(cb.borrow().as_ref().unwrap());
}

fn request_af(cb: &Closure<dyn FnMut(f64)>) {
    let _ = web_sys::window()
        .unwrap()
        .request_animation_frame(cb.as_ref().unchecked_ref());
}
```

> NOTE for implementer: copy the precise pointer-event wiring and ResizeObserver from `graph_canvas.rs` (it already solves the `Send+Sync` Callback + non-Send `Rc` split documented at `canvas/mod.rs:48-54`). Drag deltas feed `scene.on_drag`.

- [ ] **Step 3: Temporary mock-data wiring in `views/canvas/mod.rs`**

Add a temporary mock `GraphData` (random sphere positions from `fnv1a(id)`) fed into a `RwSignal<Option<GraphData>>`, render `<GalaxyCanvas graph=... on_event=.../>` in place of `<GraphCanvas .../>` at `canvas/mod.rs:656-661`. Keep `GraphCanvas` import for now (removed Phase 4). This proves the pipeline before real layout.

```rust
// TEMP (Task 6): mock galaxy from graph.query topology, random-sphere layout.
// Replaced by real force layout in Task 8.
fn mock_galaxy(resp: &GraphQueryResponse) -> super::gl::GraphData {
    use super::gl::{GalaxyNode, GraphData};
    use super::gl::math::Vec3;
    use crate::canvas_engine::fnv1a::fnv1a_32;
    use crate::canvas_engine::category_color::category_rgb; // returns [f32;3]; confirm name
    let mut id_index = std::collections::HashMap::new();
    let nodes: Vec<GalaxyNode> = resp.nodes.iter().enumerate().map(|(i, n)| {
        id_index.insert(n.id.clone(), i as u32);
        let h = fnv1a_32(n.id.as_bytes());
        let theta = (h & 0xffff) as f32 / 65535.0 * std::f32::consts::TAU;
        let phi = ((h >> 16) & 0xffff) as f32 / 65535.0 * std::f32::consts::PI;
        let r = 300.0;
        GalaxyNode {
            id: n.id.clone(), name: n.name.clone(), category: n.category.clone(),
            link_count: n.link_count as u32,
            pos: Vec3::new(r*phi.sin()*theta.cos(), r*phi.sin()*theta.sin(), r*phi.cos()),
            color: category_rgb(&n.category),
        }
    }).collect();
    let edges = resp.edges.iter().filter_map(|e|
        Some((*id_index.get(&e.from)?, *id_index.get(&e.to)?))).collect();
    GraphData { nodes, edges }
}
```

> `category_color` returns a CSS **string** (`var(--cat-*)` / `hsl(...)`), unusable in WebGL. Task 10 adds `category_rgb(&str) -> [f32;3]` with literal RGB mirroring the theme `--cat-*` hex values + an hsl→rgb fallback. For Task 6, add a minimal stub of `category_rgb` now (just the 5 known hex constants + a gray fallback); Task 10 completes the hsl fallback + `hdr_boost`. This is the ONLY allowed touch to a reused module.

- [ ] **Step 4: WASM compile gate**

Run: `export PATH="/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:$PATH" && cargo check -p aleph-panel --lib --target wasm32-unknown-unknown 2>&1 | tail -10`
Expected: `Finished`.

- [ ] **Step 5: Visual gate (Phase 1 milestone)**

Build + serve the panel and open the Memory → Canvas view.

Run: `export PATH="/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:$PATH" && cd interfaces/webchat && (test -d node_modules || npm ci) && cd ../.. && just wasm 2>&1 | tail -15`
Then rebuild+run the server per `docs/reference/DESKTOP_SHELL.md` refresh chain, open the Canvas view.
Expected (browser): a 3D cloud of soft star sprites with faint connecting lines on a near-black background; dragging orbits the camera; wheel zooms.

> If `just wasm` fails on the tailwind step in this worktree, run `npm ci` in `interfaces/webchat` first (memory: fresh worktree lacks node_modules).

- [ ] **Step 6: Commit**

```bash
git add interfaces/webchat/src/views/canvas/gl/scene.rs interfaces/webchat/src/views/canvas/galaxy_canvas.rs interfaces/webchat/src/views/canvas/mod.rs interfaces/webchat/src/views/canvas/gl/mod.rs interfaces/webchat/src/canvas_engine/category_color.rs
git commit -m "canvas: gl::scene + GalaxyCanvas host renders whole graph (mock layout)"
```

---

# Phase 2 — Layout + Nebula

Deliverable: nodes settle via 3D force layout; bloom glow; HDR palette. Looks like a nebula.

## Task 7: `gl/layout3d.rs` — 3D force-directed layout

**Files:**
- Create: `interfaces/webchat/src/views/canvas/gl/layout3d.rs`
- Modify: `gl/mod.rs` (`pub mod layout3d;`)

**Interfaces:**
- Consumes: `GraphData`, `Vec3`, `fnv1a`.
- Produces:
  - `ForceLayout::new(node_count: usize, edges: &[(u32,u32)]) -> ForceLayout`
  - `ForceLayout::seed(&self, ids: &[String]) -> Vec<Vec3>` — deterministic initial sphere from id hash.
  - `ForceLayout::step(&mut self, pos: &mut [Vec3])` — one iteration (repulsion + spring + centering), returns nothing; mutates `pos`.
  - `ForceLayout::energy(&self, pos: &[Vec3]) -> f32` — total kinetic-ish energy (for convergence).
  - `ForceLayout::converged(&self) -> bool` — true once last step's max displacement < `EPS`.

- [ ] **Step 1: Write failing tests** (pure logic, native)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::canvas::gl::math::Vec3;

    fn line_graph(n: usize) -> Vec<(u32, u32)> {
        (0..n as u32 - 1).map(|i| (i, i + 1)).collect()
    }

    #[test]
    fn seed_is_deterministic() {
        let ids: Vec<String> = (0..10).map(|i| format!("n{i}")).collect();
        let l = ForceLayout::new(10, &line_graph(10));
        let a = l.seed(&ids);
        let b = l.seed(&ids);
        assert_eq!(a.len(), 10);
        for i in 0..10 { assert_eq!(a[i], b[i]); }
    }

    #[test]
    fn energy_decreases_over_steps() {
        let ids: Vec<String> = (0..20).map(|i| format!("n{i}")).collect();
        let mut l = ForceLayout::new(20, &line_graph(20));
        let mut pos = l.seed(&ids);
        let e0 = l.energy(&pos);
        for _ in 0..200 { l.step(&mut pos); }
        let e1 = l.energy(&pos);
        assert!(e1 < e0, "energy did not decrease: {e0} -> {e1}");
    }

    #[test]
    fn converges_within_budget() {
        let ids: Vec<String> = (0..15).map(|i| format!("n{i}")).collect();
        let mut l = ForceLayout::new(15, &line_graph(15));
        let mut pos = l.seed(&ids);
        for _ in 0..600 { l.step(&mut pos); if l.converged() { break; } }
        assert!(l.converged(), "did not converge in 600 steps");
    }

    #[test]
    fn connected_nodes_closer_than_unconnected() {
        // 2 disjoint pairs: (0-1) edge, (2,3) no edge. After settling, edge pair
        // sits near spring rest length; non-edge pair drifts apart via repulsion.
        let ids: Vec<String> = (0..4).map(|i| format!("n{i}")).collect();
        let mut l = ForceLayout::new(4, &[(0, 1)]);
        let mut pos = l.seed(&ids);
        for _ in 0..400 { l.step(&mut pos); }
        let d_edge = pos[0].sub(&pos[1]).length();
        let d_free = pos[2].sub(&pos[3]).length();
        assert!(d_edge < d_free, "edge {d_edge} should be < free {d_free}");
    }
}
```

- [ ] **Step 2: Run to verify fail**

Run: `export PATH="/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:$PATH" && cargo test -p aleph-panel --lib gl::layout3d 2>&1 | tail -15`
Expected: FAIL (undefined).

- [ ] **Step 3: Implement `layout3d.rs`** (naive O(n²) repulsion — fine for low-thousands per D4)

```rust
//! 3D force-directed layout: Coulomb repulsion (O(n²)) + Hooke springs +
//! centering. Deterministic (seed from id hash). Pure — unit-tested on native.

use super::math::Vec3;
use crate::canvas_engine::fnv1a::fnv1a_32;

const REPULSION: f32 = 8000.0;   // ~k_e
const SPRING_K: f32 = 0.02;      // edge stiffness
const REST_LEN: f32 = 60.0;      // spring rest length
const CENTER_PULL: f32 = 0.002;  // gentle pull to origin
const DAMPING: f32 = 0.85;       // velocity damping
const MAX_STEP: f32 = 30.0;      // clamp per-step displacement
const EPS: f32 = 0.5;            // convergence threshold (max displacement)

pub struct ForceLayout {
    n: usize,
    edges: Vec<(u32, u32)>,
    vel: Vec<Vec3>,
    last_max_disp: f32,
}

impl ForceLayout {
    pub fn new(node_count: usize, edges: &[(u32, u32)]) -> ForceLayout {
        ForceLayout {
            n: node_count,
            edges: edges.to_vec(),
            vel: vec![Vec3::zero(); node_count],
            last_max_disp: f32::INFINITY,
        }
    }

    pub fn seed(&self, ids: &[String]) -> Vec<Vec3> {
        ids.iter().map(|id| {
            let h = fnv1a_32(id.as_bytes());
            let theta = (h & 0xffff) as f32 / 65535.0 * std::f32::consts::TAU;
            let phi = ((h >> 16) & 0xffff) as f32 / 65535.0 * std::f32::consts::PI;
            let r = 200.0;
            Vec3::new(r * phi.sin() * theta.cos(), r * phi.sin() * theta.sin(), r * phi.cos())
        }).collect()
    }

    pub fn step(&mut self, pos: &mut [Vec3]) {
        let mut force = vec![Vec3::zero(); self.n];
        // Repulsion (all pairs).
        for i in 0..self.n {
            for j in (i + 1)..self.n {
                let d = pos[i].sub(&pos[j]);
                let dist2 = d.dot(&d).max(1.0);
                let f = REPULSION / dist2;
                let dir = d.scale(1.0 / dist2.sqrt());
                force[i] = force[i].add(&dir.scale(f));
                force[j] = force[j].sub(&dir.scale(f));
            }
        }
        // Springs (edges).
        for &(a, b) in &self.edges {
            let (a, b) = (a as usize, b as usize);
            let d = pos[b].sub(&pos[a]);
            let dist = d.length().max(1e-3);
            let f = SPRING_K * (dist - REST_LEN);
            let dir = d.scale(1.0 / dist);
            force[a] = force[a].add(&dir.scale(f));
            force[b] = force[b].sub(&dir.scale(f));
        }
        // Centering + integrate.
        let mut max_disp = 0.0_f32;
        for i in 0..self.n {
            force[i] = force[i].sub(&pos[i].scale(CENTER_PULL));
            self.vel[i] = self.vel[i].add(&force[i]).scale(DAMPING);
            let mut disp = self.vel[i];
            let dl = disp.length();
            if dl > MAX_STEP { disp = disp.scale(MAX_STEP / dl); }
            pos[i] = pos[i].add(&disp);
            max_disp = max_disp.max(disp.length());
        }
        self.last_max_disp = max_disp;
    }

    pub fn energy(&self, pos: &[Vec3]) -> f32 {
        // Spring potential + inverse repulsion sum (proxy; lower = more settled).
        let mut e = 0.0;
        for &(a, b) in &self.edges {
            let d = pos[b as usize].sub(&pos[a as usize]).length() - REST_LEN;
            e += 0.5 * SPRING_K * d * d;
        }
        e
    }

    pub fn converged(&self) -> bool { self.last_max_disp < EPS }
}
```

- [ ] **Step 4: Run to verify pass**

Run: `export PATH="/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:$PATH" && cargo test -p aleph-panel --lib gl::layout3d 2>&1 | tail -15`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/views/canvas/gl/layout3d.rs interfaces/webchat/src/views/canvas/gl/mod.rs
git commit -m "canvas: gl::layout3d 3D force-directed layout"
```

---

## Task 8: Wire layout into scene (animated settling + idle drift)

**Files:**
- Modify: `gl/scene.rs` (own a `ForceLayout`, settle in `render`)
- Modify: `views/canvas/mod.rs` (replace `mock_galaxy` random positions with `ForceLayout::seed`; remove mock layout)

**Interfaces:**
- Consumes: `ForceLayout`.
- Produces: `Scene::set_graph` now seeds positions + creates a `ForceLayout`; `render` advances `MAX_SETTLE_STEPS` budget then applies idle drift; re-uploads node/edge buffers while settling.

- [ ] **Step 1: Extend `Scene`**

In `scene.rs`, add fields `layout: Option<ForceLayout>`, `settling: bool`, `settle_steps: u32`. In `set_graph`: build `ForceLayout::new(n, &edges)`, set `settling = true`. In `render` (before draw):

```rust
const MAX_SETTLE_STEPS: u32 = 400;
if self.settling {
    if let Some(layout) = self.layout.as_mut() {
        let mut pos: Vec<_> = self.data.nodes.iter().map(|n| n.pos).collect();
        layout.step(&mut pos);
        for (n, p) in self.data.nodes.iter_mut().zip(pos) { n.pos = p; }
        self.settle_steps += 1;
        if layout.converged() || self.settle_steps >= MAX_SETTLE_STEPS { self.settling = false; }
        self.edges.upload(&self.ctx.gl, &self.data);
        self.nodes.upload(&self.ctx.gl, &self.data, self.highlight.as_ref());
    }
} else {
    // idle drift: tiny per-node sine wobble around settled pos (port renderer.rs drift).
    // Apply as a transient offset in a scratch copy each frame, re-upload nodes only.
}
```

> Port the `drift_offset` sine logic from `canvas_engine/renderer.rs:33-41` into a 3D variant in `scene.rs` (3 phase-shifted sines). Drift uses a scratch position copy so settled `data.nodes[*].pos` is preserved for picking.

- [ ] **Step 2: Replace mock layout in `mod.rs`** — `mock_galaxy` becomes `build_galaxy`: positions come from `ForceLayout::seed(&ids)` instead of inline random sphere (so scene + host agree on seed). Rename the temp `// TEMP` comment removal.

- [ ] **Step 3: WASM compile gate**

Run: `export PATH="/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:$PATH" && cargo check -p aleph-panel --lib --target wasm32-unknown-unknown 2>&1 | tail -8`
Expected: `Finished`.

- [ ] **Step 4: Visual gate** — rebuild (`just wasm` + server), open Canvas. Expected: nodes start scattered then visibly settle into clustered structure over ~3-5s, then gently drift. Edges pull connected nodes together.

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/views/canvas/gl/scene.rs interfaces/webchat/src/views/canvas/mod.rs
git commit -m "canvas: animated 3D force settling + idle drift in scene"
```

---

## Task 9: `gl/bloom.rs` — FBO bloom pipeline

**Files:**
- Create: `interfaces/webchat/src/views/canvas/gl/bloom.rs`
- Modify: `gl/shaders.rs` (bright-pass, gaussian, composite), `gl/scene.rs` (render scene→FBO, then bloom), `gl/mod.rs` (`pub mod bloom;`)

**Interfaces:**
- Consumes: `GlContext`, `compile_program`.
- Produces:
  - `gaussian_weights(radius: usize) -> Vec<f32>` (free fn, **unit-tested**: sums to ~1.0, symmetric).
  - `BloomPipeline::new(gl, w, h) -> Result<_, String>`, `resize(&mut self, gl, w, h)`.
  - `BloomPipeline::scene_fbo(&self) -> &WebGlFramebuffer` (render target).
  - `BloomPipeline::run(&self, gl)` — bright-pass + separable blur (2 levels) + composite to default framebuffer.

- [ ] **Step 1: Write failing test for gaussian_weights** (pure logic)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn weights_sum_to_one_and_symmetric() {
        let w = gaussian_weights(4);
        let sum: f32 = w.iter().sum();
        assert!((sum - 1.0).abs() < 1e-3, "sum={sum}");
        assert_eq!(w.len(), 9); // 2*radius+1
        for i in 0..4 { assert!((w[i] - w[8 - i]).abs() < 1e-6); }
        assert!(w[4] > w[0]); // center heaviest
    }
}
```

- [ ] **Step 2: Run to verify fail / Step 3: implement**

`gaussian_weights`:

```rust
pub fn gaussian_weights(radius: usize) -> Vec<f32> {
    let sigma = (radius as f32 / 2.0).max(1.0);
    let mut w: Vec<f32> = (0..=2 * radius)
        .map(|i| {
            let x = i as f32 - radius as f32;
            (-(x * x) / (2.0 * sigma * sigma)).exp()
        })
        .collect();
    let sum: f32 = w.iter().sum();
    for v in &mut w { *v /= sum; }
    w
}
```

Run: `export PATH="/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:$PATH" && cargo test -p aleph-panel --lib gl::bloom 2>&1 | tail -10`
Expected: PASS.

- [ ] **Step 4: Add bloom shaders + pipeline** (GL-bound). Shaders:

```rust
// Fullscreen-triangle vertex (no buffer; gl_VertexID trick).
pub const FULLSCREEN_VERT: &str = r#"#version 300 es
precision highp float;
out vec2 v_uv;
void main() {
    vec2 p = vec2((gl_VertexID << 1) & 2, gl_VertexID & 2);
    v_uv = p;
    gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}
"#;

pub const BRIGHT_FRAG: &str = r#"#version 300 es
precision highp float;
in vec2 v_uv; out vec4 frag;
uniform sampler2D u_tex; uniform float u_threshold;
void main() {
    vec3 c = texture(u_tex, v_uv).rgb;
    float l = dot(c, vec3(0.2126, 0.7152, 0.0722));
    frag = vec4(c * max(l - u_threshold, 0.0) / max(l, 1e-4), 1.0);
}
"#;

pub const BLUR_FRAG: &str = r#"#version 300 es
precision highp float;
in vec2 v_uv; out vec4 frag;
uniform sampler2D u_tex; uniform vec2 u_dir; uniform vec2 u_texel;
const float W[5] = float[](0.227, 0.194, 0.121, 0.054, 0.016);
void main() {
    vec3 c = texture(u_tex, v_uv).rgb * W[0];
    for (int i = 1; i < 5; i++) {
        vec2 off = u_dir * u_texel * float(i);
        c += texture(u_tex, v_uv + off).rgb * W[i];
        c += texture(u_tex, v_uv - off).rgb * W[i];
    }
    frag = vec4(c, 1.0);
}
"#;

pub const COMPOSITE_FRAG: &str = r#"#version 300 es
precision highp float;
in vec2 v_uv; out vec4 frag;
uniform sampler2D u_scene; uniform sampler2D u_bloom; uniform float u_intensity;
void main() {
    vec3 s = texture(u_scene, v_uv).rgb;
    vec3 b = texture(u_bloom, v_uv).rgb;
    frag = vec4(s + b * u_intensity, 1.0);
}
"#;
```

`BloomPipeline` (GL-bound, structural): create 1 scene FBO (`RGBA16F` if `EXT_color_buffer_float` present, else `RGBA8`) + 2 half-res ping-pong FBOs. `run`: bright-pass scene→pp[0]; H-blur pp[0]→pp[1]; V-blur pp[1]→pp[0]; composite(scene, pp[0])→default FBO. Draw fullscreen triangle (`gl.draw_arrays(TRIANGLES, 0, 3)`, no VAO attribs). `threshold=0.3`, `intensity=1.2` (ref values).

> Implementer: standard FBO/texture setup. Detect float support: `gl.get_extension("EXT_color_buffer_float")`; on `None`, use `RGBA8` internal format (graceful degrade per spec §10).

- [ ] **Step 5: Hook bloom into `scene.rs`** — in `Scene::new` build `BloomPipeline`; in `render`, bind `bloom.scene_fbo()` before clearing/drawing scene, then call `bloom.run(gl)` to composite to screen. On `resize`, call `bloom.resize`.

- [ ] **Step 6: WASM compile gate + visual gate**

Run: `export PATH="/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:$PATH" && cargo check -p aleph-panel --lib --target wasm32-unknown-unknown 2>&1 | tail -8`
Expected: `Finished`. Then `just wasm` + browser: bright nodes now bloom into soft glowing halos; dense regions read as luminous nebula clouds.

- [ ] **Step 7: Commit**

```bash
git add interfaces/webchat/src/views/canvas/gl/bloom.rs interfaces/webchat/src/views/canvas/gl/shaders.rs interfaces/webchat/src/views/canvas/gl/scene.rs interfaces/webchat/src/views/canvas/gl/mod.rs
git commit -m "canvas: gl::bloom FBO bloom pipeline (nebula glow)"
```

---

## Task 10: HDR palette tuning

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/category_color.rs` (ensure `category_rgb` exists + a `hdr_boost` helper, **unit-tested**)

**Interfaces:**
- Produces:
  - `pub fn category_rgb(category: &str) -> [f32; 3]` — linear-ish RGB in [0,1]; known categories mirror the theme `--cat-*` hex; unknowns use the SAME `hsl(hue, 55%, 65%)` rule as `category_color` (so node color matches the rest of the UI).
  - `pub fn hdr_boost(rgb: [f32; 3]) -> [f32; 3]` (`1.2 + brightness*0.8`, brightness = mean channel).

- [ ] **Step 1: Write failing tests**

```rust
#[cfg(test)]
mod rgb_tests {
    use super::*;
    fn approx(a: f32, b: f32) { assert!((a - b).abs() < 0.01, "{a} vs {b}"); }

    #[test]
    fn known_category_matches_theme_hex() {
        // --cat-feedback: #a78bfa  → (0.655, 0.545, 0.980)
        let c = category_rgb("feedback");
        approx(c[0], 0.655); approx(c[1], 0.545); approx(c[2], 0.980);
    }

    #[test]
    fn unknown_category_is_deterministic_and_in_range() {
        let a = category_rgb("custom-xyz");
        let b = category_rgb("custom-xyz");
        assert_eq!(a, b);
        for ch in a { assert!((0.0..=1.0).contains(&ch)); }
    }

    #[test]
    fn boost_brighter_for_whiter() {
        let red = hdr_boost([1.0, 0.0, 0.0]);
        let white = hdr_boost([1.0, 1.0, 1.0]);
        assert!(white.iter().sum::<f32>() / 3.0 > red.iter().sum::<f32>());
        assert!(red[0] >= 1.2); // ref floor
    }
}
```

- [ ] **Step 2: Run to verify fail**

Run: `export PATH="/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:$PATH" && cargo test -p aleph-panel --lib category_color 2>&1 | tail -10`
Expected: FAIL (undefined `category_rgb`/`hdr_boost`).

- [ ] **Step 3: Implement in `category_color.rs`**

```rust
/// Theme `--cat-*` hex values mirrored as [0,1] RGB. Keep in sync with
/// `styles/tailwind.css`.
pub fn category_rgb(category: &str) -> [f32; 3] {
    match category {
        "feedback" => hex(0xa7, 0x8b, 0xfa),
        "project" => hex(0x34, 0xd3, 0x99),
        "reference" => hex(0x60, 0xa5, 0xfa),
        "user" => hex(0xfb, 0xbf, 0x24),
        "error" | "broken" | "contradiction" => hex(0xf4, 0x43, 0x36),
        other => {
            // Same rule as category_color: hsl(hue, 55%, 65%).
            let hue = (fnv1a_32(other.as_bytes()) % 360) as f32;
            hsl_to_rgb(hue, 0.55, 0.65)
        }
    }
}

/// HDR boost: brighter base color → stronger bloom corona (ref: 1.2 + brightness*0.8).
pub fn hdr_boost(rgb: [f32; 3]) -> [f32; 3] {
    let b = (rgb[0] + rgb[1] + rgb[2]) / 3.0;
    let k = 1.2 + b * 0.8;
    [rgb[0] * k, rgb[1] * k, rgb[2] * k]
}

fn hex(r: u8, g: u8, b: u8) -> [f32; 3] {
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0]
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> [f32; 3] {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = h / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    [r + m, g + m, b + m]
}
```

- [ ] **Step 4: Run to verify pass; replace inline boost in `nodes.rs`**

Run: `export PATH="/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:$PATH" && cargo test -p aleph-panel --lib category_color 2>&1 | tail -10`
Expected: PASS (3 tests). Then in `nodes.rs::upload`, replace the inline `boost = 1.2 + ...` math with `category_color::hdr_boost(node.color)` (single source of truth).

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/category_color.rs interfaces/webchat/src/views/canvas/gl/nodes.rs
git commit -m "canvas: category_rgb (theme palette) + hdr_boost helper"
```

---

# Phase 3 — Interaction Rewiring

Deliverable: select/hover/detail/search/agent-switch/list-cross-link all work on the 3D view. No functionality lost (spec §5).

## Task 11: `gl/picking.rs` — screen-space picking

**Files:**
- Create: `interfaces/webchat/src/views/canvas/gl/picking.rs`
- Modify: `gl/mod.rs` (`pub mod picking;`)

**Interfaces:**
- Consumes: `Vec3`, `Mat4`, `GraphData`.
- Produces: `pick_node(view_proj: &Mat4, nodes: &[GalaxyNode], viewport: (f32,f32), cursor: (f32,f32), radius_px: f32) -> Option<u32>` — projects each node to screen, returns nearest within `radius_px` (front-most on ties via NDC z).

- [ ] **Step 1: Write failing tests** (pure logic)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::canvas::gl::math::{Mat4, Vec3};
    use crate::views::canvas::gl::{GalaxyNode, GraphData};

    fn node_at(x: f32, y: f32, z: f32) -> GalaxyNode {
        GalaxyNode { id: "n".into(), name: "n".into(), category: "c".into(),
            link_count: 0, pos: Vec3::new(x, y, z), color: [1.0, 1.0, 1.0] }
    }

    #[test]
    fn picks_node_under_cursor() {
        let vp = Mat4::perspective(1.0, 1.0, 0.1, 1000.0)
            .mul(&Mat4::look_at(Vec3::new(0.0, 0.0, 300.0), Vec3::zero(), Vec3::new(0.0,1.0,0.0)));
        let nodes = vec![node_at(0.0, 0.0, 0.0), node_at(200.0, 0.0, 0.0)];
        // Center node projects to screen center (400,300) on an 800x600 viewport.
        let hit = pick_node(&vp, &nodes, (800.0, 600.0), (400.0, 300.0), 20.0);
        assert_eq!(hit, Some(0));
    }

    #[test]
    fn returns_none_when_far() {
        let vp = Mat4::perspective(1.0, 1.0, 0.1, 1000.0)
            .mul(&Mat4::look_at(Vec3::new(0.0, 0.0, 300.0), Vec3::zero(), Vec3::new(0.0,1.0,0.0)));
        let nodes = vec![node_at(0.0, 0.0, 0.0)];
        let hit = pick_node(&vp, &nodes, (800.0, 600.0), (10.0, 10.0), 20.0);
        assert_eq!(hit, None);
    }
}
```

- [ ] **Step 2: Run fail / Step 3: implement**

```rust
//! Screen-space picking: project nodes, return nearest within radius. Pure.

use super::math::{Mat4, Vec3};
use super::GalaxyNode;

pub fn pick_node(
    view_proj: &Mat4,
    nodes: &[GalaxyNode],
    viewport: (f32, f32),
    cursor: (f32, f32),
    radius_px: f32,
) -> Option<u32> {
    let m = view_proj.as_slice();
    let mut best: Option<(u32, f32, f32)> = None; // (idx, dist2, ndc_z)
    for (i, node) in nodes.iter().enumerate() {
        let p = &node.pos;
        let cx = m[0]*p.x + m[4]*p.y + m[8]*p.z + m[12];
        let cy = m[1]*p.x + m[5]*p.y + m[9]*p.z + m[13];
        let cz = m[2]*p.x + m[6]*p.y + m[10]*p.z + m[14];
        let cw = m[3]*p.x + m[7]*p.y + m[11]*p.z + m[15];
        if cw <= 0.0 { continue; } // behind camera
        let ndc_x = cx / cw;
        let ndc_y = cy / cw;
        let ndc_z = cz / cw;
        let sx = (ndc_x * 0.5 + 0.5) * viewport.0;
        let sy = (1.0 - (ndc_y * 0.5 + 0.5)) * viewport.1;
        let dx = sx - cursor.0;
        let dy = sy - cursor.1;
        let d2 = dx * dx + dy * dy;
        if d2 <= radius_px * radius_px {
            match best {
                Some((_, _, bz)) if ndc_z >= bz => {}
                _ => best = Some((i as u32, d2, ndc_z)),
            }
        }
    }
    best.map(|(i, _, _)| i)
}
```

- [ ] **Step 4: Run pass / Step 5: commit**

Run: `export PATH="/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:$PATH" && cargo test -p aleph-panel --lib gl::picking 2>&1 | tail -8`
Expected: PASS (2 tests).

```bash
git add interfaces/webchat/src/views/canvas/gl/picking.rs interfaces/webchat/src/views/canvas/gl/mod.rs
git commit -m "canvas: gl::picking screen-space node picking"
```

---

## Task 12: Wire select/hover → fly-to + highlight + detail panel

**Files:**
- Modify: `gl/scene.rs` (store `view_proj` of last frame + viewport; add `pick(&self, cursor) -> Option<u32>`, `set_highlight(&mut self, Option<HashSet<u32>>)`, `fly_to_node(&mut self, idx)`)
- Modify: `galaxy_canvas.rs` (on `pointerdown` without drag → pick → emit `CanvasEvent::SelectNode(id)`; on `pointermove` hover → emit `HoverNode`)
- Modify: `views/canvas/mod.rs` (reuse existing `on_event`, detail-excerpt Effect, `NodeDetailPanel`; compute highlight set = node + topological neighbors from `GraphData.edges`)

**Interfaces:**
- Consumes: `pick_node`, existing `CanvasEvent`, `NodeDetailPanel`.
- Produces: `Scene::pick`, `Scene::set_highlight`, `Scene::fly_to_node`; host maps selection → highlight neighbors + fly-to + open panel.

- [ ] **Step 1: Scene picking + highlight + fly-to**

Add to `scene.rs`: store `last_vp: Mat4` and `viewport` each `render`. Implement:

```rust
pub fn pick(&self, cursor: (f32, f32)) -> Option<String> {
    super::picking::pick_node(&self.last_vp, &self.data.nodes,
        (self.width as f32, self.height as f32), cursor, 18.0)
        .map(|i| self.data.nodes[i as usize].id.clone())
}
pub fn set_highlight(&mut self, hl: Option<std::collections::HashSet<u32>>) {
    self.highlight = hl;
    self.nodes.upload(&self.ctx.gl, &self.data, self.highlight.as_ref());
}
pub fn fly_to_node(&mut self, id: &str, t_ms: f64) {
    if let Some(n) = self.data.nodes.iter().find(|n| n.id == id) {
        self.camera.fly_to(n.pos, 250.0);
        self.camera.note_interaction(t_ms);
    }
}
```

- [ ] **Step 2: Host event wiring** — In `galaxy_canvas.rs`, distinguish click (pointerdown+up without significant move) → `scene.pick(cursor)` → `on_event.run(CanvasEvent::SelectNode(id))`. Hover (pointermove, no button) → `scene.pick` → emit `HoverNode(Some(id))`/`HoverNode(None)`.

- [ ] **Step 3: mod.rs rewire** — Keep the existing `on_event` match (`canvas/mod.rs:604-629`). For `SelectNode(id)`: set `mem.selected_node`, compute neighbor set from loaded `GraphData.edges`, call `scene.set_highlight(...)` + `scene.fly_to_node(...)`. The detail-excerpt Effect (`canvas/mod.rs:187-239`) is reused **unchanged** (it keys off `selected_node`/`prefetch_request`). Render the existing `<NodeDetailPanel .../>` overlay bound to `selected_node`.

> Highlight/fly-to need a handle to the `Scene`. Add a method channel: `galaxy_canvas` exposes a `RwSignal<Option<String>>` "focus request" + `RwSignal<Option<HashSet<u32>>>` "highlight", and an Effect inside `GalaxyCanvas` applies them to its owned `Scene` (same intent-channel pattern as `canvas/mod.rs:130-136`, required because `Scene` is non-Send `Rc<RefCell>`).

- [ ] **Step 4: WASM compile gate + manual verify**

Run: `export PATH="/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:$PATH" && cargo check -p aleph-panel --lib --target wasm32-unknown-unknown 2>&1 | tail -8`
Expected: `Finished`. Then `just wasm` + browser: clicking a star flies the camera to it, dims non-neighbors, opens the detail panel with the note excerpt; hovering shows the node.

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/views/canvas/gl/scene.rs interfaces/webchat/src/views/canvas/galaxy_canvas.rs interfaces/webchat/src/views/canvas/mod.rs
git commit -m "canvas: wire pick/select/hover → fly-to + highlight + detail panel"
```

---

## Task 13: Wire search, agent-switch, list↔graph cross-link

**Files:**
- Modify: `views/canvas/mod.rs` (search Effect, agent reset Effect, reverse-link Effect adapted to fly-to)

**Interfaces:**
- Consumes: existing `mem.search_query`/`search_nonce`, `mem.agent_id`, `mem.selected_node`/`mem.highlight_note_id`, `GraphApi::search`.
- Produces: search → `scene.fly_to_node` + highlight; agent switch → reload `graph.query` + rebuild galaxy + reseed layout; reverse-link → fly-to.

- [ ] **Step 1: Search** — Adapt the existing search Effect (`canvas/mod.rs:633-652`): `graph.search` → first result id → push to the focus-request signal (fly-to + highlight) instead of `active_request` re-fetch.
- [ ] **Step 2: Agent switch** — Adapt the reset Effect (`canvas/mod.rs:271-314`): on agent change, clear galaxy state and re-run the whole-graph `graph.query` → `build_galaxy` → push new `GraphData` into the scene signal (re-seeds layout, settling restarts).
- [ ] **Step 3: Reverse-link** — The list view sets `mem.selected_node` / `mem.highlight_note_id` (see `views/memory/mod.rs:339-342` `on_locate`). Add/adapt an Effect: when `mem.selected_node` changes from the list (memory_view flips to Graph), push it to the focus-request signal → fly-to + highlight + open panel.
- [ ] **Step 4: WASM compile gate + manual verify** — search box flies to a match; switching agent in the sidebar reloads a different galaxy; clicking "view in graph" from the Memory table flies to that node.

Run: `export PATH="/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:$PATH" && cargo check -p aleph-panel --lib --target wasm32-unknown-unknown 2>&1 | tail -8`
Expected: `Finished`.

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/views/canvas/mod.rs
git commit -m "canvas: rewire search + agent-switch + list cross-link to 3D galaxy"
```

---

## Task 14: `fold_threshold` → LOD / visual density

**Files:**
- Modify: `gl/scene.rs` (accept `lod: f32`; hide labels + fade weak edges beyond threshold), `views/canvas/mod.rs` (feed `mem.fold_threshold` → scene LOD signal)

**Interfaces:**
- Produces: `Scene::set_lod(&mut self, lod: f32)` — controls edge alpha cutoff + (later) label density.

- [ ] **Step 1:** Add `lod` to `Scene`; in `edges` upload, drop edges whose both endpoints have `link_count` below an LOD-derived floor (keeps strong structure at high density). Feed `mem.fold_threshold` (1..1000) normalized into `set_lod`.
- [ ] **Step 2: WASM compile gate + manual verify** — the existing sidebar slider now thins/thickens the visible edge web.

Run: `export PATH="/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:$PATH" && cargo check -p aleph-panel --lib --target wasm32-unknown-unknown 2>&1 | tail -8`
Expected: `Finished`.

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/src/views/canvas/gl/scene.rs interfaces/webchat/src/views/canvas/mod.rs
git commit -m "canvas: fold_threshold drives LOD edge density"
```

---

# Phase 4 — Polish + Retirement

Deliverable: production polish; dead Canvas2D code removed; clean build.

## Task 15: Polish — fly-to easing, idle auto-rotate, optional labels

**Files:**
- Modify: `gl/camera.rs` (tune damping/fly rates if needed), `gl/scene.rs` (label overlay for hovered/selected node only)

- [ ] **Step 1:** Render the hovered/selected node's `name` as an HTML overlay positioned via the projected screen coord (reuse `pick_node`'s projection; emit screen pos through a signal to a positioned `<div>` in `galaxy_canvas`). Avoids GL text. Only 1-2 labels at a time → no clutter at scale.
- [ ] **Step 2:** Verify idle auto-rotate kicks in after 60s of no interaction and stops on any pointer/wheel event.
- [ ] **Step 3: WASM compile gate + visual gate + commit**

Run: `export PATH="/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:$PATH" && cargo check -p aleph-panel --lib --target wasm32-unknown-unknown 2>&1 | tail -8`

```bash
git add interfaces/webchat/src/views/canvas/gl/camera.rs interfaces/webchat/src/views/canvas/gl/scene.rs interfaces/webchat/src/views/canvas/galaxy_canvas.rs
git commit -m "canvas: polish labels + idle auto-rotate"
```

## Task 16: MiniMap decision

**Files:**
- Modify: `views/canvas/mod.rs` (remove `MiniMapOverlay` usage), possibly delete `views/canvas/minimap_view.rs` + `canvas_engine/mini_map.rs`

- [ ] **Step 1:** Remove the 2D `MiniMapOverlay` render block (`canvas/mod.rs:662-679`). Decision: **remove** the 2D minimap (it assumed Canvas2D viewport coords). Do NOT build a 3D overview this iteration (YAGNI; spec §11). Delete `minimap_view.rs`; mark `mini_map.rs` for retirement in Task 17 if no other consumer.
- [ ] **Step 2: grep for other consumers**

Run: `grep -rn "mini_map\|MiniMap\|GlobalMiniMap" interfaces/webchat/src | grep -v "minimap_view.rs\|mini_map.rs"`
Expected: no remaining consumers → safe to retire in Task 17.

- [ ] **Step 3: WASM compile gate + commit**

Run: `export PATH="/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:$PATH" && cargo check -p aleph-panel --lib --target wasm32-unknown-unknown 2>&1 | tail -8`

```bash
git add interfaces/webchat/src/views/canvas/mod.rs
git rm interfaces/webchat/src/views/canvas/minimap_view.rs
git commit -m "canvas: remove 2D minimap (superseded by 3D galaxy)"
```

## Task 17: Retire Canvas2D modules

**Files:**
- Delete: `canvas_engine/{renderer,edge_curve,drag,tween,viewport,scatter,align_guides,interaction,navigation,mini_map}.rs`
- Delete: `views/canvas/{graph_canvas,edge_label}.rs`
- Modify: `canvas_engine/mod.rs` (drop module declarations), `views/canvas/mod.rs` (drop `GraphCanvas`/`GraphState` imports + retired imports)

- [ ] **Step 1: For EACH file, grep for consumers before deleting**

Run (example): `grep -rn "canvas_engine::renderer\|draw_neighborhood\|GraphCanvas\|edge_curve\|::drag::\|::tween::\|::viewport::\|::scatter::\|align_guides\|::interaction::\|::navigation::" interfaces/webchat/src | grep -v "/gl/"`
Expected: only references inside the to-be-deleted files themselves. Any external consumer → port or keep that file (note in commit).

> Before deleting `renderer.rs`, confirm its `drift_offset` was ported to `scene.rs` (Task 8). Before deleting `navigation.rs`, confirm no breadcrumb feature is still wanted (spec §6 says port-or-delete; default delete).

- [ ] **Step 2: Delete + clean module declarations**

```bash
git rm interfaces/webchat/src/canvas_engine/renderer.rs interfaces/webchat/src/canvas_engine/edge_curve.rs interfaces/webchat/src/canvas_engine/drag.rs interfaces/webchat/src/canvas_engine/tween.rs interfaces/webchat/src/canvas_engine/viewport.rs interfaces/webchat/src/canvas_engine/scatter.rs interfaces/webchat/src/canvas_engine/align_guides.rs interfaces/webchat/src/canvas_engine/interaction.rs interfaces/webchat/src/canvas_engine/navigation.rs interfaces/webchat/src/canvas_engine/mini_map.rs interfaces/webchat/src/views/canvas/graph_canvas.rs interfaces/webchat/src/views/canvas/edge_label.rs
```

Remove the corresponding `pub mod ...;` lines from `canvas_engine/mod.rs` and the retired `use`/`mod` lines from `views/canvas/mod.rs`.

- [ ] **Step 3: WASM compile gate (must be clean — proves no dangling refs)**

Run: `export PATH="/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:$PATH" && cargo check -p aleph-panel --lib --target wasm32-unknown-unknown 2>&1 | tail -12`
Expected: `Finished` with zero errors. Fix any dangling references by porting/removing.

- [ ] **Step 4: Native test sweep (ensure ported logic + reused modules still pass)**

Run: `export PATH="/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:$PATH" && cargo test -p aleph-panel --lib 2>&1 | tail -15`
Expected: all tests pass (gl::* + reused adapter/prefetch + others).

- [ ] **Step 5: Commit**

```bash
git add -A interfaces/webchat/src
git commit -m "canvas: retire Canvas2D renderer + radial engine (dead code removal)"
```

## Task 18: Final verification

- [ ] **Step 1: Full clippy + check (cargo exemption granted this task)**

Run: `export PATH="/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:$PATH" && cargo clippy -p aleph-panel --lib --target wasm32-unknown-unknown -- -D warnings 2>&1 | tail -20`
Expected: no warnings. Fix any.

- [ ] **Step 2: Full WASM build + serve**

Run: `export PATH="/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:$PATH" && just wasm 2>&1 | tail -10` then rebuild server + replace binary per `docs/reference/DESKTOP_SHELL.md`.

- [ ] **Step 3: Manual acceptance checklist** (spec §12 + §5 table) — verify EACH: 3D orbit/zoom; nebula glow; settling; select→fly-to+highlight+panel; hover; search→fly-to; agent switch reload; list "view in graph" cross-link; note edit; fold slider LOD; idle auto-rotate. Screenshot the nebula.

- [ ] **Step 4: Final commit (if any fixes)**

```bash
git add -A interfaces/webchat/src
git commit -m "canvas: final polish + clippy clean for 3D nebula"
```

---

## Self-Review

**Spec coverage:** D1 replace (T6/T12/T17) ✓ · D2 pure-Rust WebGL2 (all gl/ tasks, no JS) ✓ · D3 client layout (T7/T8) ✓ · D4 scale/instancing, no Barnes-Hut (T4 instanced, T7 O(n²) noted) ✓ · D5 sprites (T4) ✓ · D6 FBO bloom (T9) ✓ · D7 settling+drift (T8) ✓ · D8 four phases ✓ · D9 rewire all (T12/T13/T14) ✓. Spec §5 interaction table → T12/T13/T14 each row ✓. §6 retirement → T16/T17 ✓. §7 phasing → phases ✓. §8 testing → native unit tests (T1/T2/T7/T9/T10/T11) + visual gates ✓. §9 redlines → core untouched (scope), no JS, serde unchanged ✓.

**Placeholder scan:** No "TBD/TODO/implement later". GL-bound tasks (T3/T4/T5/T6/T9) give full shaders + concrete Rust with real signatures; structural notes ("standard FBO setup") are bounded mechanical steps with the key calls + extension-detection spelled out, not vague placeholders. Pure-logic tasks have complete test + impl code.

**Type consistency:** `GalaxyNode`/`GraphData` (T1) used consistently in T4/T5/T7/T11. `Mat4::as_slice`/`mul`/`perspective`/`look_at` (T1) used in T2/T11. `OrbitCamera::{orbit,zoom,fly_to,note_interaction,update,view_proj,eye,target}` (T2) used in T6/T12. `ForceLayout::{new,seed,step,energy,converged}` (T7) used in T8. `pick_node` signature (T11) used in T12. `set_mat4`/`set_vec2` defined in `nodes.rs` (T4), reused in `edges.rs` (T5) — consistent. `category_rgb`/`hdr_boost` (T10) referenced in T6/T4 — T10 formalizes them; T6 note flags creating `category_rgb` if absent (resolved in T10).

**One gap fixed inline:** T6 references `category_rgb` which T10 formalizes — T6 step 3 note already instructs creating it if absent, T10 makes it canonical. Consistent.
