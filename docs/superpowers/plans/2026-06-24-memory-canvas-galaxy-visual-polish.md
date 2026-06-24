# 记忆 Canvas 星系图谱 视觉/性能/交互打磨 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让记忆星系图谱的星点清晰、连线有机、选中链路点亮，且闲置零浪费——治"模糊星 / 死板脱离连线 / 闲置烧 GPU / 高亮链路未连"。

**Architecture:** 纯 Panel WASM 渲染层改造。星点 fragment 改清晰核+柔晕；漂移从每帧 CPU 重传搬进顶点着色器（`u_time` 驱动）；连线由直 quad 改顶点着色器内 K 段贝塞尔条带 + 端点收束；新增每边 `a_highlight` 属性 + 高亮边 intent 通道，选中时邻居链路流光、非邻居调暗。不动数据流 / RPC / 力导引 / LOD / 交互编排骨架。

**Tech Stack:** Rust + Leptos 0.8 (WASM) · WebGL2 / GLSL ES 3.00 · `aleph-panel` crate · 现有 `gl/` 渲染模块。

## Global Constraints

- **crate = `aleph-panel`**；纯逻辑原生单测命令 `cargo test -p aleph-panel --lib <filter>`（gl 子模块已确立原生 `#[cfg(test)]` 模式，见 `bloom.rs`/`camera.rs`/`math.rs`）。
- **极度节制 cargo/just（用户硬约束）**：逻辑 task 用**带 filter 的** `cargo test` 跑**该 task 的测试**，绝不跑全量。GL/shader task 的编译与视觉验证**全部批量集中到 Task 8** 一次 `just wasm` + 一次 `just dev`，不每个 task 跑。整个计划 cargo/just 调用控制在个位数。
- **零新依赖**（R3 / 技术栈禁用清单）：纯 GLSL + 现有 Rust，不引任何 crate。
- **不碰 core / 数据流 / RPC / `ForceLayout` / LOD 语义 / 交互编排骨架**（spec §2.2）。
- **picking 用 canonical `node.pos`**：GPU 漂移后视觉位置与 canonical 略偏（≪18px 容差），与今日 CPU 漂移期行为一致，不得改 `pick`/`screen_pos_of`。
- **星芒硬约束（用户强调）**：弱、hub-only、`alpha ≤ 0.3`、近景淡出，绝不抢连线、密集不乱。
- **代码注释英文**；commit message 英文 `<scope>: <desc>`；单分支直接 main。
- **漂移视觉连续性**：相位与振幅复刻现 `drift_offset_3d`（amp=3.0, period=5000ms, 三轴相位偏移 0 / +0.27 / +0.54 of TAU），只换执行位置（CPU→GPU），不换观感。

---

## File Structure（决策锁定）

| 文件 | 职责 | 本计划改动 |
|------|------|-----------|
| `gl/shaders.rs` | 所有 GLSL 源 | NODE_FRAG 重写、NODE_VERT 加漂移+星芒、EDGE_VERT 贝塞尔+收束、EDGE_FRAG 流光+调暗 |
| `gl/nodes.rs` | 节点实例渲染 + 纯逻辑 helper | `node_phase`、`spike_strength`、`hub_spike_threshold`、`a_phase`/`a_spike` 属性、`u_time`/`u_cam_dist`、上传门控 |
| `gl/edges.rs` | 边实例渲染 + 纯逻辑 helper | `edge_strip_corners`、K 段 strip、`a_highlight` 属性、`u_time`/`u_select_active`、taper |
| `gl/bloom.rs` | Bloom 管线 | `run()` threshold/intensity 参数回调 |
| `gl/mod.rs` | `GraphData`/`GalaxyNode` 定义处 + 纯逻辑 helper | `compute_highlight_edges`（原生可测） |
| `gl/scene.rs` | 每帧编排 | GPU 漂移路径、上传门控、删 `drift_scratch`、下发高亮边集、`u_time`/`u_cam_dist`/`u_select_active` |
| `views/canvas/galaxy_canvas.rs` | canvas 宿主 + rAF | rAF 可见性门控、`highlight_edges` prop + Effect |
| `views/canvas/mod.rs` | 交互编排 | 调 `compute_highlight_edges`、新 intent 通道 `highlight_edges_request` 接线 |

---

## Task 1: 清晰星核 + 柔光晕 + Bloom 回调

**Files:**
- Modify: `interfaces/webchat/src/views/canvas/gl/shaders.rs:26-39` (NODE_FRAG)
- Modify: `interfaces/webchat/src/views/canvas/gl/bloom.rs:190-191,244-245` (threshold/intensity)

**Interfaces:**
- Consumes: 现有 `v_corner`(vec2), `v_color`(vec3) — 不改 NODE_VERT 的 out 集（本 task 只改 frag）。
- Produces: 视觉——清晰实心核 + 柔晕，供后续 task 叠加。

纯 fragment/参数改动，无原生可测逻辑 → 编译与视觉验证集中在 Task 8。

- [ ] **Step 1: 重写 NODE_FRAG（清晰核 + 柔晕）**

把 `shaders.rs` 的 `NODE_FRAG` 常量整体替换为：

```rust
/// Node fragment: crisp solid core + soft outer halo (HDR; bloom adds the corona).
/// The core stays sharp (tight smoothstep) so stars read as bright points, not blobs.
pub const NODE_FRAG: &str = r#"#version 300 es
precision highp float;
in vec2 v_corner;
in vec3 v_color;
out vec4 frag;
void main() {
    float r = length(v_corner);
    if (r > 1.0) discard;
    // Crisp core: hard, tight falloff → a defined bright point.
    float core = smoothstep(0.30, 0.0, r);
    core = core * core;                       // sharpen the core profile
    // Soft halo: wide gentle falloff, low weight; bloom turns this into the glow.
    float halo = smoothstep(1.0, 0.0, r) * 0.35;
    vec3  rgb  = v_color * (core * 1.6 + halo);
    float a    = clamp(core + halo * 0.6, 0.0, 1.0);
    frag = vec4(rgb, a);
}
"#;
```

- [ ] **Step 2: Bloom 回调（让清晰核存活、晕不过曝）**

`bloom.rs` `run()` 中 bright-pass 阈值 `0.3 → 0.5`：

```rust
let loc = gl.get_uniform_location(&self.prog_bright, "u_threshold");
gl.uniform1f(loc.as_ref(), 0.5);
```

composite intensity `1.2 → 0.9`：

```rust
let loc = gl.get_uniform_location(&self.prog_composite, "u_intensity");
gl.uniform1f(loc.as_ref(), 0.9);
```

- [ ] **Step 3: 标记待批量验证**

本 task 的视觉验收（清晰核 + 柔晕、不再糊）并入 Task 8 浏览器实测。无独立 cargo 调用。

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/views/canvas/gl/shaders.rs interfaces/webchat/src/views/canvas/gl/bloom.rs
git commit -m "canvas: crisp star core + soft halo, retune bloom"
```

---

## Task 2: 漂移搬进顶点着色器 + 上传门控

**Files:**
- Modify: `interfaces/webchat/src/views/canvas/gl/nodes.rs` (新增 `node_phase`、`a_phase` 属性、`u_time`、上传门控)
- Modify: `interfaces/webchat/src/views/canvas/gl/shaders.rs` (NODE_VERT 加漂移)
- Modify: `interfaces/webchat/src/views/canvas/gl/scene.rs` (闲置走 GPU 漂移、删 `drift_scratch`)

**Interfaces:**
- Consumes: `fnv1a_32(&[u8]) -> u32`（`canvas_engine::fnv1a`，`pub(crate)`）。
- Produces:
  - `pub(super) fn node_phase(id: &str) -> f32` — 节点漂移相位 ∈ [0,1)。
  - `NodeRenderer::upload(gl, data, hl)` 行为不变（仍上传 offset/size/color + **新 phase buffer**），但 scene 闲置帧不再调它。
  - `NodeRenderer::draw(gl, view_proj, viewport, u_time_ms)` — 新增 `u_time_ms: f32` 形参。

- [ ] **Step 1: 写失败测试 `node_phase`**

`nodes.rs` 底部 `#[cfg(test)]`：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_phase_is_stable_and_in_unit_range() {
        for id in ["", "a", "memory-x", "节点42"] {
            let p = node_phase(id);
            assert!((0.0..1.0).contains(&p), "phase out of range for {id}: {p}");
            assert_eq!(p, node_phase(id), "phase not deterministic for {id}");
        }
        assert!(node_phase("a") != node_phase("b"), "distinct ids share phase");
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p aleph-panel --lib node_phase`
Expected: FAIL — `cannot find function node_phase`

- [ ] **Step 3: 实现 `node_phase`（复刻现漂移相位）**

`nodes.rs` 顶部 `use super::{shaders, GraphData};` 之后加：

```rust
use crate::canvas_engine::fnv1a::fnv1a_32;

/// Per-node drift phase in [0,1), derived deterministically from the id hash.
/// Replaces the CPU `drift_offset_3d` phase so idle motion moves to the GPU.
pub(super) fn node_phase(id: &str) -> f32 {
    fnv1a_32(id.as_bytes()) as f32 / u32::MAX as f32
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p aleph-panel --lib node_phase`
Expected: PASS

- [ ] **Step 5: NodeRenderer 加 `a_phase` 实例属性**

`NodeRenderer` struct 加字段 `inst_phase: WebGlBuffer,`。`new()` 中创建并 setup（location 4，size 1，divisor 1）：

```rust
let inst_phase = gl.create_buffer().ok_or("phase buf")?;
Self::setup_instanced(gl, &inst_phase, 4, 1);
```

并把 `inst_phase` 纳入返回的 struct。`upload()` 中构建并上传 phase（与 offsets 同循环）：

```rust
let mut phases = Vec::with_capacity(n);
// ...在 for 循环内：
phases.push(node_phase(&node.id));
// ...循环后：
upload_f32(gl, &self.inst_phase, &phases);
```

- [ ] **Step 6: NODE_VERT 加 GPU 漂移**

`shaders.rs` `NODE_VERT` 替换为（新增 `a_phase`、`u_time`，位置 = base + 三轴 sin）：

```rust
pub const NODE_VERT: &str = r#"#version 300 es
precision highp float;
layout(location=0) in vec2 a_corner;
layout(location=1) in vec3 a_offset;
layout(location=2) in float a_size;
layout(location=3) in vec3 a_color;
layout(location=4) in float a_phase;
uniform mat4 u_view_proj;
uniform vec2 u_viewport;
uniform float u_time;        // ms
out vec2 v_corner;
out vec3 v_color;
const float AMP = 3.0;
const float TAU = 6.28318530718;
const float OMEGA = TAU / 5.0;   // period 5000ms → rad/s over t(sec)
void main() {
    float t = u_time / 1000.0;
    float ph = a_phase * TAU;
    vec3 drift = AMP * vec3(
        sin(OMEGA * t + ph),
        sin(OMEGA * t + ph + 0.27 * TAU),
        sin(OMEGA * t + ph + 0.54 * TAU)
    );
    vec4 clip = u_view_proj * vec4(a_offset + drift, 1.0);
    vec2 px = a_corner * a_size / u_viewport * clip.w * 2.0;
    clip.xy += px;
    gl_Position = clip;
    v_corner = a_corner;
    v_color = a_color;
}
"#;
```

- [ ] **Step 7: `draw` 传 `u_time`**

`NodeRenderer::draw` 签名加 `u_time_ms: f32`，函数体内设 uniform：

```rust
pub fn draw(&self, gl: &Gl, view_proj: &Mat4, viewport: (f32, f32), u_time_ms: f32) {
    if self.count == 0 { return; }
    gl.use_program(Some(&self.prog));
    gl.bind_vertex_array(Some(&self.vao));
    set_mat4(gl, &self.prog, "u_view_proj", view_proj);
    set_vec2(gl, &self.prog, "u_viewport", viewport);
    let loc = gl.get_uniform_location(&self.prog, "u_time");
    gl.uniform1f(loc.as_ref(), u_time_ms);
    gl.draw_arrays_instanced(Gl::TRIANGLES, 0, 6, self.count);
    gl.bind_vertex_array(None);
}
```

- [ ] **Step 8: scene.rs 闲置走 GPU 漂移、删 CPU 路径**

`scene.rs` `render()` 的 Phase 2（闲置 else 分支，约 265-286 行）整段替换为 —— 闲置帧**不再上传任何 node buffer**：

```rust
} else {
    // Idle: drift is computed in the vertex shader from u_time; no CPU work,
    // no per-frame buffer re-upload. Canonical node.pos stays authoritative
    // for picking.
}
```

删除 struct 字段 `drift_scratch: Vec<Vec3>,`、`new()` 中其初始化、以及 `drift_offset_3d` 函数 + 其 `use` 的 `fnv1a_32`（若仅此处用）。`settling` 分支末尾的 `self.nodes.upload(...)` 保留（settling 位置真在变）。

`render()` 的 node draw 调用传 `u_time`：

```rust
self.nodes.draw(gl, &vp, (self.width as f32, self.height as f32), t_ms as f32);
```

- [ ] **Step 9: 上传门控——color/size 不再每帧**

确认闲置分支已无 `upload`/`upload_positions` 调用（Step 8 已删）。`set_highlight` 仍调 `upload`（高亮变化时）。settling 仍每帧 `upload`（有界 ≤400 步）。无需额外改动——记录确认即可。

- [ ] **Step 10: Commit**

```bash
git add interfaces/webchat/src/views/canvas/gl/nodes.rs interfaces/webchat/src/views/canvas/gl/shaders.rs interfaces/webchat/src/views/canvas/gl/scene.rs
git commit -m "canvas: move idle drift to vertex shader, drop per-frame node re-upload"
```

> 注：`upload_positions`（settling 用 override 位置）此 task 不删——settling 仍需；但它内部也应上传 phase 以保持 buffer 对齐。在 Step 5 改完 `upload` 后，同步给 `upload_positions` 补 phase 上传（同 `node_phase(&node.id)` 循环），避免两上传路径 buffer 长度不一致。

---

## Task 3: 极弱 hub 星芒（近景淡出）

**Files:**
- Modify: `interfaces/webchat/src/views/canvas/gl/nodes.rs` (`spike_strength`/`hub_spike_threshold`、`a_spike` 属性、`u_cam_dist`)
- Modify: `interfaces/webchat/src/views/canvas/gl/shaders.rs` (NODE_VERT 传 `v_spike`、NODE_FRAG 画十字)
- Modify: `interfaces/webchat/src/views/canvas/gl/scene.rs` (draw 传 `u_cam_dist = camera.distance`)

**Interfaces:**
- Consumes: `GalaxyNode.link_count: u32`。
- Produces:
  - `pub(super) fn hub_spike_threshold(link_counts: &[u32]) -> u32` — 仅 ≥ 此值的节点出星芒（~95 百分位）。
  - `pub(super) fn spike_strength(link_count: u32, threshold: u32) -> f32` — ∈ [0,0.3]，低于阈值=0。
  - `NodeRenderer::draw(.., u_time_ms, u_cam_dist)` — 再加 `u_cam_dist: f32` 形参。

- [ ] **Step 1: 写失败测试**

`nodes.rs` 测试模块加：

```rust
#[test]
fn spikes_only_for_top_hubs_and_capped() {
    let counts = vec![1u32, 1, 2, 2, 3, 3, 4, 50]; // 50 = clear hub
    let th = hub_spike_threshold(&counts);
    assert!(th >= 4, "threshold too low: {th}");
    assert_eq!(spike_strength(1, th), 0.0, "low-degree node must have no spike");
    let s = spike_strength(50, th);
    assert!(s > 0.0 && s <= 0.3, "hub spike must be weak (0,0.3]: {s}");
}

#[test]
fn empty_counts_threshold_is_safe() {
    assert!(hub_spike_threshold(&[]) >= 1);
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p aleph-panel --lib spike`
Expected: FAIL — `cannot find function hub_spike_threshold`

- [ ] **Step 3: 实现 helper**

`nodes.rs` 加：

```rust
/// Degree floor above which a node renders a (weak) hub spike: ~95th percentile.
pub(super) fn hub_spike_threshold(link_counts: &[u32]) -> u32 {
    if link_counts.is_empty() {
        return 1;
    }
    let mut s: Vec<u32> = link_counts.to_vec();
    s.sort_unstable();
    let idx = ((s.len() as f32 * 0.95) as usize).min(s.len() - 1);
    s[idx].max(1)
}

/// Weak spike strength in [0, 0.3]; 0 below the hub threshold. Caps so spikes
/// never dominate (user hard constraint: must not fight the edges).
pub(super) fn spike_strength(link_count: u32, threshold: u32) -> f32 {
    if link_count < threshold {
        return 0.0;
    }
    let over = (link_count - threshold) as f32;
    (0.15 + (over.sqrt() * 0.03)).min(0.3)
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p aleph-panel --lib spike`
Expected: PASS

- [ ] **Step 5: `a_spike` 实例属性**

`NodeRenderer` 加 `inst_spike: WebGlBuffer`，`new()` 创建 + `setup_instanced(gl, &inst_spike, 5, 1)`。`upload()`/`upload_positions()` 中：循环前 `let th = hub_spike_threshold(&data.nodes.iter().map(|n| n.link_count).collect::<Vec<_>>());`，循环内 `spikes.push(spike_strength(node.link_count, th));`，循环后 `upload_f32(gl, &self.inst_spike, &spikes);`。

- [ ] **Step 6: NODE_VERT 透传 `v_spike`**

NODE_VERT 加 `layout(location=5) in float a_spike;`、`uniform float u_cam_dist;`、`out float v_spike;`，并在 `main` 末尾：

```glsl
// Near-fade: spikes only show at distance; vanish when zoomed in / clustered.
float fade = smoothstep(300.0, 900.0, u_cam_dist);
v_spike = a_spike * fade;
```

（`v_corner = a_corner; v_color = a_color;` 保留）

- [ ] **Step 7: NODE_FRAG 画弱十字星芒**

NODE_FRAG 加 `in float v_spike;`，在 `if (r > 1.0) discard;` 之后、组装 frag 之前加：

```glsl
// Weak diffraction cross for hubs only. abs() arms along the two axes; very
// thin, alpha-capped by v_spike (<=0.3). Never drawn when v_spike == 0.
float cross = 0.0;
if (v_spike > 0.0) {
    float ax = 1.0 - smoothstep(0.0, 0.06, abs(v_corner.x));
    float ay = 1.0 - smoothstep(0.0, 0.06, abs(v_corner.y));
    float radial = 1.0 - smoothstep(0.0, 1.0, r); // fade arms toward rim
    cross = max(ax, ay) * radial * v_spike;
}
```

并把组装行改为把 cross 叠到 rgb/alpha（不抬非 hub 节点）：

```glsl
vec3  rgb  = v_color * (core * 1.6 + halo + cross);
float a    = clamp(core + halo * 0.6 + cross, 0.0, 1.0);
frag = vec4(rgb, a);
```

- [ ] **Step 8: scene.rs 传 `u_cam_dist`**

`render()` node draw 调用改为：

```rust
self.nodes.draw(gl, &vp, (self.width as f32, self.height as f32), t_ms as f32, self.camera.distance);
```

`NodeRenderer::draw` 加 `u_cam_dist: f32` 形参并设 uniform `u_cam_dist`（同 Task 2 Step 7 模式）。

- [ ] **Step 9: Commit**

```bash
git add interfaces/webchat/src/views/canvas/gl/nodes.rs interfaces/webchat/src/views/canvas/gl/shaders.rs interfaces/webchat/src/views/canvas/gl/scene.rs
git commit -m "canvas: weak hub-only diffraction spikes with near-fade"
```

---

## Task 4: 微弧 + 端点收束连线

**Files:**
- Modify: `interfaces/webchat/src/views/canvas/gl/edges.rs` (`edge_strip_corners`、K 段 strip、draw 改 TRIANGLE_STRIP + `u_time`)
- Modify: `interfaces/webchat/src/views/canvas/gl/shaders.rs` (EDGE_VERT 贝塞尔 + taper、EDGE_FRAG 透传 `v_along`)

**Interfaces:**
- Consumes: 现有 `a_pos_a/a_pos_b/a_color_a/a_color_b` 实例属性。
- Produces:
  - `pub(super) fn edge_strip_corners(segments: usize) -> Vec<f32>` — 长度 `2*(segments+1)*2` 的 `[along,side,...]`，`along` 0→1 均分，`side` 交替 -1/+1。
  - `EdgeRenderer::draw(.., u_time_ms)` — 新增 `u_time_ms`。

- [ ] **Step 1: 写失败测试 `edge_strip_corners`**

`edges.rs` 底部 `#[cfg(test)]`：

```rust
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
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p aleph-panel --lib edge_strip_corners`
Expected: FAIL — `cannot find function edge_strip_corners`

- [ ] **Step 3: 实现 + 替换静态 corner buffer**

`edges.rs` 加（并删除旧的 `const CORNERS: [f32;12]`，改用动态生成 + `SEGMENTS`）：

```rust
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
```

`new()` 中上传 corner buffer 改为：

```rust
let corners = edge_strip_corners(SEGMENTS);
unsafe {
    let view = js_sys::Float32Array::view(&corners);
    gl.buffer_data_with_array_buffer_view(Gl::ARRAY_BUFFER, &view, Gl::STATIC_DRAW);
}
```

`draw()` 的 `draw_arrays_instanced` 改为 TRIANGLE_STRIP + 顶点数：

```rust
let vtx = (2 * (SEGMENTS + 1)) as i32;
gl.draw_arrays_instanced(Gl::TRIANGLE_STRIP, 0, vtx, self.count);
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p aleph-panel --lib edge_strip_corners`
Expected: PASS

- [ ] **Step 5: EDGE_VERT 贝塞尔 + taper**

`shaders.rs` `EDGE_VERT` 替换为（沿曲线采样切线求屏幕法向，端点收束 taper，输出 `v_along`）：

```rust
pub const EDGE_VERT: &str = r#"#version 300 es
precision highp float;
layout(location=0) in vec2 a_corner;   // (along 0..1, side -1/+1)
layout(location=1) in vec3 a_pos_a;
layout(location=2) in vec3 a_pos_b;
layout(location=3) in vec3 a_color_a;
layout(location=4) in vec3 a_color_b;
uniform mat4 u_view_proj;
uniform vec2 u_viewport;
uniform float u_width;
out vec3 v_color;
out float v_along;
out float v_depth;

vec3 bezier(vec3 a, vec3 c, vec3 b, float t) {
    float u = 1.0 - t;
    return u*u*a + 2.0*u*t*c + t*t*b;
}
void main() {
    float t = a_corner.x;
    // Control point: midpoint bowed along a world-perp of the chord.
    vec3 chord = a_pos_b - a_pos_a;
    float len = length(chord);
    vec3 dir = len > 1e-4 ? chord / len : vec3(1.0, 0.0, 0.0);
    vec3 up = abs(dir.y) < 0.95 ? vec3(0.0,1.0,0.0) : vec3(1.0,0.0,0.0);
    vec3 perp = normalize(cross(dir, up));
    vec3 ctrl = (a_pos_a + a_pos_b) * 0.5 + perp * (len * 0.12);
    // Sample curve point and a neighbor for screen-space tangent.
    vec3 p  = bezier(a_pos_a, ctrl, a_pos_b, t);
    float dt = 0.01;
    vec3 p2 = bezier(a_pos_a, ctrl, a_pos_b, clamp(t + dt, 0.0, 1.0));
    vec4 cp  = u_view_proj * vec4(p, 1.0);
    vec4 cp2 = u_view_proj * vec4(p2, 1.0);
    vec2 sp  = cp.xy  / max(cp.w, 1e-4)  * u_viewport;
    vec2 sp2 = cp2.xy / max(cp2.w, 1e-4) * u_viewport;
    vec2 tdir = sp2 - sp;
    tdir = length(tdir) > 1e-4 ? normalize(tdir) : vec2(1.0, 0.0);
    vec2 nrm = vec2(-tdir.y, tdir.x);
    // Endpoint taper: thinner near both ends so the line welds into the star.
    float edge = min(t, 1.0 - t);
    float taper = mix(0.55, 1.0, smoothstep(0.0, 0.12, edge));
    vec2 off_px = nrm * (a_corner.y * u_width * 0.5 * taper);
    cp.xy += off_px * 2.0 / u_viewport * cp.w;
    gl_Position = cp;
    v_color = mix(a_color_a, a_color_b, t);
    v_along = t;
    v_depth = cp.z / max(cp.w, 1e-4);
}
"#;
```

- [ ] **Step 6: EDGE_FRAG 透传 along（流光在 Task 5 接）**

`shaders.rs` `EDGE_FRAG` 暂改为接收 `v_along`、保持安静底色（flow 在 Task 5 加）：

```rust
pub const EDGE_FRAG: &str = r#"#version 300 es
precision highp float;
in vec3 v_color;
in float v_along;
in float v_depth;
out vec4 frag;
void main() {
    // Brighter near endpoints → visually plugs into the star core.
    float edge = min(v_along, 1.0 - v_along);
    float weld = mix(1.4, 1.0, smoothstep(0.0, 0.12, edge));
    float fade = clamp(1.0 - (v_depth * 0.5 + 0.5) * 0.5, 0.4, 1.0);
    frag = vec4(v_color * (1.0 * weld * fade), fade * 0.55);
}
"#;
```

> 注：原 EDGE_FRAG 的 `v_side`（rim AA）随单段 quad 废弃——现宽度由 taper 控制，rim 简化掉；`u_width` 仍由 `EDGE_WIDTH_PX` 提供。`draw()` 加 `u_time` uniform 形参（本 task 先传，Task 5 用）：`EdgeRenderer::draw(.., u_time_ms: f32)`，设 `u_time` uniform（同 Task 2 模式）；`scene.rs` 两处 `edges.draw(...)` 加 `t_ms as f32`。

- [ ] **Step 7: Commit**

```bash
git add interfaces/webchat/src/views/canvas/gl/edges.rs interfaces/webchat/src/views/canvas/gl/shaders.rs interfaces/webchat/src/views/canvas/gl/scene.rs
git commit -m "canvas: curved bezier edges with endpoint taper + weld"
```

---

## Task 5: 高亮链路连通（修脱节 + 选中流光 + 调暗）

**Files:**
- Modify: `interfaces/webchat/src/views/canvas/gl/mod.rs` (`compute_highlight_edges`)
- Modify: `interfaces/webchat/src/views/canvas/gl/edges.rs` (`a_highlight` 属性、`upload_indexed` 接 highlight、`u_select_active`)
- Modify: `interfaces/webchat/src/views/canvas/gl/shaders.rs` (EDGE_VERT 透传 `v_hl`、EDGE_FRAG 流光 + 调暗)
- Modify: `interfaces/webchat/src/views/canvas/gl/scene.rs` (`set_highlight_edges`)
- Modify: `interfaces/webchat/src/views/canvas/galaxy_canvas.rs` (`highlight_edges` prop + Effect)
- Modify: `interfaces/webchat/src/views/canvas/mod.rs` (调 `compute_highlight_edges`、新 intent 通道接线)

**Interfaces:**
- Consumes: `GraphData { nodes: Vec<GalaxyNode>, edges: Vec<(u32,u32)> }`（`gl/mod.rs`）。
- Produces:
  - `pub fn compute_highlight_edges(data: &GraphData, selected_id: &str) -> std::collections::HashSet<(u32,u32)>` — 选中节点的邻接边，归一化 `(min,max)` 节点索引对。
  - `Scene::set_highlight_edges(&mut self, edges: Option<HashSet<(u32,u32)>>)`。
  - `EdgeRenderer::set_highlight(&mut self, gl, edges_in_order: &[(u32,u32)], hl_set: Option<&HashSet<(u32,u32)>>)` — 按当前已上传边顺序重建 `a_highlight` + 设 `u_select_active`。

- [ ] **Step 1: 写失败测试 `compute_highlight_edges`**

`gl/mod.rs` 底部（若无 test 模块则新增）：

```rust
#[cfg(test)]
mod highlight_tests {
    use super::*;
    use crate::views::canvas::gl::math::Vec3;

    fn node(id: &str) -> GalaxyNode {
        GalaxyNode { id: id.into(), name: id.into(), category: "x".into(),
            link_count: 0, pos: Vec3::zero(), color: [1.0,1.0,1.0] }
    }

    #[test]
    fn highlight_edges_are_neighbor_links_normalized() {
        let data = GraphData {
            nodes: vec![node("a"), node("b"), node("c"), node("d")],
            edges: vec![(0,1), (2,0), (2,3)], // a-b, c-a, c-d
        };
        let hl = compute_highlight_edges(&data, "a");
        assert!(hl.contains(&(0,1)));   // a-b
        assert!(hl.contains(&(0,2)));   // c-a normalized → (0,2)
        assert!(!hl.contains(&(2,3)));  // c-d not incident to a
    }

    #[test]
    fn unknown_id_yields_empty() {
        let data = GraphData { nodes: vec![node("a")], edges: vec![] };
        assert!(compute_highlight_edges(&data, "zzz").is_empty());
    }
}
```

> 若 `GalaxyNode` 字段集与上不符，以 `gl/mod.rs` 实际定义为准，仅补齐构造字段（不改类型）。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p aleph-panel --lib highlight_edges`
Expected: FAIL — `cannot find function compute_highlight_edges`

- [ ] **Step 3: 实现 `compute_highlight_edges`**

`gl/mod.rs`：

```rust
/// Edges incident to the selected node, as normalized (min,max) index pairs.
/// Drives edge highlight (flow) + non-neighbor dimming.
pub fn compute_highlight_edges(
    data: &GraphData,
    selected_id: &str,
) -> std::collections::HashSet<(u32, u32)> {
    let mut out = std::collections::HashSet::new();
    let Some(sel) = data.nodes.iter().position(|n| n.id == selected_id) else {
        return out;
    };
    let sel = sel as u32;
    for &(a, b) in &data.edges {
        if a == sel || b == sel {
            out.insert((a.min(b), a.max(b)));
        }
    }
    out
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p aleph-panel --lib highlight_edges`
Expected: PASS

- [ ] **Step 5: EdgeRenderer 加 `a_highlight` + `u_select_active`**

`edges.rs`：struct 加 `hl_buf: WebGlBuffer`，`new()` 创建 + `setup_instanced(gl, &hl_buf, 5, 1)`。`upload_indexed()` 末尾把 `a_highlight` 初始化为全 0（长度 = edges.len()），上传 `hl_buf`。新增：

```rust
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
    let flags: Vec<f32> = edges_in_order.iter().map(|&(a, b)| {
        let key = (a.min(b), a.max(b));
        if active && hl.unwrap().contains(&key) { 1.0 } else { 0.0 }
    }).collect();
    bind_upload(gl, &self.hl_buf, &flags);
}
```

struct 加 `select_active: f32`（`new()` 初始化 0.0），`draw()` 设 uniform `u_select_active`。

- [ ] **Step 6: EDGE_VERT/FRAG 流光 + 调暗**

EDGE_VERT 加 `layout(location=5) in float a_highlight;` + `out float v_hl;`，`main` 末尾 `v_hl = a_highlight;`。

EDGE_FRAG 替换为：

```rust
pub const EDGE_FRAG: &str = r#"#version 300 es
precision highp float;
in vec3 v_color;
in float v_along;
in float v_depth;
in float v_hl;
out vec4 frag;
uniform float u_time;          // ms
uniform float u_select_active; // 1.0 when a node is selected
void main() {
    float edge = min(v_along, 1.0 - v_along);
    float weld = mix(1.4, 1.0, smoothstep(0.0, 0.12, edge));
    float fade = clamp(1.0 - (v_depth * 0.5 + 0.5) * 0.5, 0.4, 1.0);
    vec3  rgb = v_color * (weld * fade);
    float a   = fade * 0.55;
    if (u_select_active > 0.5) {
        if (v_hl > 0.5) {
            // Energy flow: a bright band travels along the highlighted link.
            float pulse = fract(v_along * 1.5 - u_time * 0.0006);
            float band = smoothstep(0.0, 0.06, pulse) * (1.0 - smoothstep(0.10, 0.45, pulse));
            rgb += v_color * band * 1.8;
            a = clamp(a + band * 0.5, 0.0, 1.0);
        } else {
            // Non-neighbor edge: dim to focus the selected chain.
            rgb *= 0.25;
            a   *= 0.35;
        }
    }
    frag = vec4(rgb, a);
}
"#;
```

- [ ] **Step 7: Scene::set_highlight_edges**

`scene.rs`：struct 加字段 `highlight_edges: Option<HashSet<(u32,u32)>>`（`new()` 初始化 None）。新增：

```rust
pub fn set_highlight_edges(&mut self, edges: Option<std::collections::HashSet<(u32, u32)>>) {
    self.edges.set_highlight(&self.ctx.gl, &self.filtered_edges, edges.as_ref());
    self.highlight_edges = edges;
}
```

并在每次 `edges.upload_indexed(...)` 之后（set_graph / set_lod / settling 帧）重应用高亮，保证 `a_highlight` 与新边顺序对齐：

```rust
self.edges.set_highlight(&self.ctx.gl, &self.filtered_edges, self.highlight_edges.as_ref());
```

- [ ] **Step 8: galaxy_canvas.rs 加 prop + Effect**

`GalaxyCanvas` 组件签名加：

```rust
/// Intent channel: edges incident to the selected node (normalized index pairs).
highlight_edges_request: RwSignal<Option<std::collections::HashSet<(u32, u32)>>>,
```

加 Effect（仿 highlight Effect，galaxy_canvas.rs:164-170）：

```rust
let scene_hle = scene.clone();
Effect::new(move |_| {
    let hle = highlight_edges_request.get();
    if let Some(s) = scene_hle.borrow_mut().as_mut() {
        s.set_highlight_edges(hle);
    }
});
```

- [ ] **Step 9: mod.rs 接线**

`mod.rs`：新 intent 通道（仿 highlight_request, mod.rs:116）：

```rust
let highlight_edges_request: RwSignal<Option<std::collections::HashSet<(u32, u32)>>> = RwSignal::new(None);
```

`<GalaxyCanvas>` 传 `highlight_edges_request=highlight_edges_request`。

三处现有 `highlight_request.set(Some(hl))` 调用点（`on_event::SelectNode` mod.rs:194、search Effect:236、reverse-link Effect:290）后各加一行：

```rust
highlight_edges_request.set(Some(crate::views::canvas::gl::compute_highlight_edges(&data, &id)));
```

（`data` 是同作用域已 `get_untracked()` 的 `galaxy_data`；`id`/`node_id` 用各处已有变量名。）

`DeselectNode`（mod.rs:199）与 agent-switch reset（mod.rs:153 附近）加 `highlight_edges_request.set(None);`。

- [ ] **Step 10: Commit**

```bash
git add interfaces/webchat/src/views/canvas/gl/mod.rs interfaces/webchat/src/views/canvas/gl/edges.rs interfaces/webchat/src/views/canvas/gl/shaders.rs interfaces/webchat/src/views/canvas/gl/scene.rs interfaces/webchat/src/views/canvas/galaxy_canvas.rs interfaces/webchat/src/views/canvas/mod.rs
git commit -m "canvas: wire highlight chain — neighbor-edge flow + non-neighbor dim on select"
```

---

## Task 6: rAF 可见性门控

**Files:**
- Modify: `interfaces/webchat/src/views/canvas/galaxy_canvas.rs` (`start_raf_loop` 跳过隐藏帧)

**Interfaces:**
- Consumes: `scene` 的 `Rc<RefCell<Option<Scene>>>`、canvas 元素。
- Produces: 隐藏（`display:none` keep-alive）或零尺寸时跳过 `render`，仍保活 rAF。

无原生可测纯逻辑（DOM 可见性）→ 行为验证并入 Task 8。

- [ ] **Step 1: 可见性判断 + 跳过 render**

`start_raf_loop` 的闭包内，把 `s.render(t)` 包在可见性判断里。用 canvas 的 `offset_parent()`（`display:none` 时为 `None`）+ 尺寸 > 0 判断。需要在 `start_raf_loop` 捕获 canvas 句柄——改签名传入 `canvas_el: web_sys::HtmlCanvasElement`，调用处（galaxy_canvas.rs:139）传 `el.clone()`。闭包内：

```rust
let visible = canvas_el.offset_parent().is_some()
    && canvas_el.client_width() > 0
    && canvas_el.client_height() > 0;
if visible {
    if let Some(s) = scene.borrow_mut().as_mut() {
        s.render(t);
    }
}
```

label 覆盖层的投影更新（galaxy_canvas.rs:357-371）同样移入 `if visible` 块（隐藏时无需更新）。`request_af(...)` 始终调用（保活）。

- [ ] **Step 2: 行为验证并入 Task 8**

验收：面板切到非 Graph 视图（隐藏 canvas）后，performance trace 显示 GPU/CPU 帧活动停止；切回即恢复。

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/src/views/canvas/galaxy_canvas.rs
git commit -m "canvas: skip render while galaxy canvas is hidden (keep-alive rAF)"
```

---

## Task 7: 可选打磨——背景星尘 + 节点微闪（可砍）

> **可选**：spec §3.5。若 review 时决定砍掉，跳过此 task，主线不受影响。

**Files:**
- Modify: `interfaces/webchat/src/views/canvas/gl/shaders.rs` (NODE_FRAG/VERT 微闪)

**Interfaces:**
- Consumes: 已有 `a_phase`(Task 2)、`u_time`。
- Produces: 节点核亮度极轻微闪烁（着色器内，零额外开销/上传）。

- [ ] **Step 1: 节点微闪（复用 a_phase + u_time）**

NODE_VERT 已有 `a_phase`/`u_time`；加 `out float v_twinkle;`，`main` 末尾：

```glsl
v_twinkle = 0.9 + 0.1 * sin(u_time / 1000.0 * 1.7 + a_phase * 6.2831853);
```

NODE_FRAG 加 `in float v_twinkle;`，核亮度乘上：把 `core * 1.6` 改为 `core * 1.6 * v_twinkle`。

> 背景星尘（远景静态暗点层）若也要：作为独立后续 task 评估——需新 renderer，较重，**默认仅做零开销微闪**，星尘留 review 决定。

- [ ] **Step 2: 验证并入 Task 8**

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/src/views/canvas/gl/shaders.rs
git commit -m "canvas: subtle per-node twinkle (shader-side, zero cost)"
```

---

## Task 8: 批量编译 + 浏览器验收（GL/视觉/性能）

> 所有 GL/shader 改动的编译门与视觉/性能验证集中在此，遵守 cargo/just 节制约束。

**Files:** 无新改动——验证 Task 1–7 成果。

- [ ] **Step 1: WASM 编译门（一次）**

Run: `just wasm`
Expected: 编译通过，无 GLSL link 报错（运行时 shader 编译失败会在浏览器 console 报，Step 3 验）。

- [ ] **Step 2: 起 dev 服务**

Run: `just dev`
Expected: server 起，panel 由重建的 WASM 提供。

- [ ] **Step 3: 浏览器视觉验收（chrome-devtools MCP）**

打开 panel 记忆 Graph 视图，逐项核对：
- 星点：清晰实心核（非软糊球）+ 柔光晕。
- hub 星芒：仅极少数高度数节点可见、弱、近景（zoom in）自动消失，不与连线混。
- 连线：微弧、端点收束融入星核，无脱离感。
- 选中节点：邻居链路流光（移动亮带）、非邻居边调暗、非邻居节点调暗；取消选中复位。
- console 无 shader 编译/link 报错。

- [ ] **Step 4: 性能验收**

`performance_start_trace` → 闲置 5s → `performance_stop_trace`：
- 闲置帧无 node/edge buffer 重传（仅 uniform + draw）。
- 切到非 Graph 视图（隐藏 canvas）：帧活动停止；切回恢复。
- 目标：闲置 ~60fps、隐藏时零渲染。

- [ ] **Step 5: 收尾（如有 clippy 噪声）**

仅当 Step 1 报 warning 影响合并时，至多一次 `cargo check -p aleph-panel --lib` 修净（用户约束：高风险至多一次 check）。否则跳过。

- [ ] **Step 6: 最终 commit（若 Step 5 有修动）**

```bash
git add -A
git commit -m "canvas: lint clean for galaxy polish"
```

---

## Self-Review 结论

- **Spec 覆盖**：§3.1 星核→T1/T3；§3.2 连线→T4；§3.3 高亮链路→T5；§3.4 性能(GPU 漂移/上传门控/可见性)→T2/T5(上传门控)/T6；§3.5 可选打磨→T7；§4 测试→各 task 原生单测 + T8 批量验收。无遗漏。
- **类型一致**：`compute_highlight_edges -> HashSet<(u32,u32)>` 在 T5 定义并被 mod.rs/scene/edges 一致消费；`node_phase`/`spike_strength`/`hub_spike_threshold`/`edge_strip_corners` 签名跨 task 一致；`draw` 形参演进（T2 加 `u_time`，T3 加 `u_cam_dist`）已在各自 task 标注，最终签名 `NodeRenderer::draw(gl, vp, viewport, u_time_ms, u_cam_dist)` / `EdgeRenderer::draw(gl, vp, viewport, u_time_ms)`。
- **Placeholder 扫描**：无 TBD/TODO；所有 code step 给出完整 GLSL/Rust。
- **cargo 节制**：纯逻辑 4 处带 filter 单测；GL 验证 1×`just wasm`+1×`just dev`+浏览器；至多 1× 兜底 `cargo check`。符合用户硬约束。
