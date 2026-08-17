//! Instanced billboard-sprite node renderer (one draw call for all nodes).

use std::collections::HashSet;
use web_sys::{WebGl2RenderingContext as Gl, WebGlBuffer, WebGlProgram, WebGlVertexArrayObject};

use crate::memory_graph::fnv1a::fnv1a_32;

use super::context::compile_program;
use super::math::Mat4;
use super::{shaders, GraphData};

/// Per-node drift phase in [0,1), derived deterministically from the id hash.
/// Replaces the CPU `drift_offset_3d` phase so idle motion moves to the GPU.
pub(super) fn node_phase(id: &str) -> f32 {
    fnv1a_32(id.as_bytes()) as f32 / u32::MAX as f32
}

/// Degree floor above which a node renders a (weak) hub spike: ~95th percentile,
/// but never at or below the median.
///
/// The bare 95th percentile lights EVERY node on a uniform / low-spread graph
/// (e.g. all notes have 2 links): there `p95 == min`, so `link_count >= p95`
/// holds for all and every star grows a spike — defeating the "spike == rare
/// hub" intent. Flooring the threshold just above the median suppresses spikes
/// when no node stands out; on graphs with real spread `p95` already exceeds the
/// median so this floor is a no-op.
pub(super) fn hub_spike_threshold(link_counts: &[u32]) -> u32 {
    if link_counts.is_empty() {
        return 1;
    }
    let mut s: Vec<u32> = link_counts.to_vec();
    s.sort_unstable();
    let idx = ((s.len() as f32 * 0.95) as usize).min(s.len() - 1);
    let p95 = s[idx].max(1);
    let median = s[(s.len() - 1) / 2];
    p95.max(median + 1)
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

pub struct NodeRenderer {
    prog: WebGlProgram,
    vao: WebGlVertexArrayObject,
    inst_offset: WebGlBuffer,
    inst_size: WebGlBuffer,
    inst_color: WebGlBuffer,
    inst_phase: WebGlBuffer,
    inst_spike: WebGlBuffer,
    count: i32,
}

/// Color multiplier for nodes OUTSIDE the highlight set while a node is selected.
/// The node fragment shader HDR-boosts and blooms the lit stars, so a mild
/// de-emphasis reads as "same star, slightly different" against that starfield —
/// the dim has to bite hard enough to recede into the background.
const DIM_COLOR_SCALE: f32 = 0.35;

const CORNERS: [f32; 12] = [
    -1.0, -1.0, 1.0, -1.0, -1.0, 1.0, // tri 1
    -1.0, 1.0, 1.0, -1.0, 1.0, 1.0, // tri 2
];

impl NodeRenderer {
    /// # Safety
    ///
    /// This renderer uses `js_sys::Float32Array::view` to upload geometry
    /// without copying. The caller must ensure the source slice is not moved or
    /// dropped until the upload call returns.
    pub fn new(gl: &Gl) -> Result<NodeRenderer, String> {
        let prog = compile_program(
            gl,
            &shaders::with_drift(shaders::NODE_VERT),
            shaders::NODE_FRAG,
        )?;
        let vao = gl.create_vertex_array().ok_or("vao")?;
        gl.bind_vertex_array(Some(&vao));

        // a_corner (location 0) — static quad.
        let corner_buf = gl.create_buffer().ok_or("corner buf")?;
        gl.bind_buffer(Gl::ARRAY_BUFFER, Some(&corner_buf));
        unsafe {
            // SAFETY: `view` is used immediately inside `buffer_data_with_array_buffer_view`
            // and does not outlive this block. `CORNERS` is a `'static` array so the
            // backing memory is valid for the duration of the call.
            let view = js_sys::Float32Array::view(&CORNERS);
            gl.buffer_data_with_array_buffer_view(Gl::ARRAY_BUFFER, &view, Gl::STATIC_DRAW);
        }
        gl.enable_vertex_attrib_array(0);
        gl.vertex_attrib_pointer_with_i32(0, 2, Gl::FLOAT, false, 0, 0);

        let inst_offset = gl.create_buffer().ok_or("offset buf")?;
        let inst_size = gl.create_buffer().ok_or("size buf")?;
        let inst_color = gl.create_buffer().ok_or("color buf")?;
        let inst_phase = gl.create_buffer().ok_or("phase buf")?;
        let inst_spike = gl.create_buffer().ok_or("spike buf")?;
        // a_offset (1) vec3, a_size (2) float, a_color (3) vec3, a_phase (4) float,
        // a_spike (5) float — per-instance.
        Self::setup_instanced(gl, &inst_offset, 1, 3);
        Self::setup_instanced(gl, &inst_size, 2, 1);
        Self::setup_instanced(gl, &inst_color, 3, 3);
        Self::setup_instanced(gl, &inst_phase, 4, 1);
        Self::setup_instanced(gl, &inst_spike, 5, 1);

        gl.bind_vertex_array(None);
        Ok(NodeRenderer {
            prog,
            vao,
            inst_offset,
            inst_size,
            inst_color,
            inst_phase,
            inst_spike,
            count: 0,
        })
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
        let mut phases = Vec::with_capacity(n);
        let mut spikes = Vec::with_capacity(n);
        let th = hub_spike_threshold(
            &data
                .nodes
                .iter()
                .map(|nd| nd.link_count)
                .collect::<Vec<_>>(),
        );
        let has_hl = hl.map(|s| !s.is_empty()).unwrap_or(false);
        for (i, node) in data.nodes.iter().enumerate() {
            offsets.extend_from_slice(&[node.pos.x, node.pos.y, node.pos.z]);
            // Size grows with degree. Non-highlighted nodes shrink slightly and
            // (mainly) darken — see DIM_COLOR_SCALE.
            let base = 6.0 + (node.link_count as f32).sqrt() * 4.0;
            let lit = !has_hl || hl.map(|s| s.contains(&(i as u32))).unwrap_or(true);
            sizes.push(if lit { base } else { base * 0.9 });
            let [r, g, b] = node.color;
            if lit {
                // HDR boost so bloom picks up a glow corona.
                let [br, bg, bb] = crate::memory_graph::category_color::hdr_boost(node.color);
                colors.extend_from_slice(&[br, bg, bb]);
            } else {
                colors.extend_from_slice(&[
                    r * DIM_COLOR_SCALE,
                    g * DIM_COLOR_SCALE,
                    b * DIM_COLOR_SCALE,
                ]);
            }
            phases.push(node_phase(&node.id));
            spikes.push(spike_strength(node.link_count, th));
        }
        self.count = n as i32;
        upload_f32(gl, &self.inst_offset, &offsets);
        upload_f32(gl, &self.inst_size, &sizes);
        upload_f32(gl, &self.inst_color, &colors);
        upload_f32(gl, &self.inst_phase, &phases);
        upload_f32(gl, &self.inst_spike, &spikes);
    }

    pub fn draw(
        &self,
        gl: &Gl,
        view_proj: &Mat4,
        viewport: (f32, f32),
        u_time_ms: f32,
        u_cam_dist: f32,
    ) {
        if self.count == 0 {
            return;
        }
        gl.use_program(Some(&self.prog));
        gl.bind_vertex_array(Some(&self.vao));
        set_mat4(gl, &self.prog, "u_view_proj", view_proj);
        set_vec2(gl, &self.prog, "u_viewport", viewport);
        let loc = gl.get_uniform_location(&self.prog, "u_time");
        gl.uniform1f(loc.as_ref(), u_time_ms);
        let loc = gl.get_uniform_location(&self.prog, "u_cam_dist");
        gl.uniform1f(loc.as_ref(), u_cam_dist);
        gl.draw_arrays_instanced(Gl::TRIANGLES, 0, 6, self.count);
        gl.bind_vertex_array(None);
    }
}

/// # Safety
///
/// Uses `js_sys::Float32Array::view` on `data`. The view is consumed by the
/// WebGL buffer upload before this function returns, so `data` must remain
/// valid and un-moved for the duration of the call.
fn upload_f32(gl: &Gl, buf: &WebGlBuffer, data: &[f32]) {
    gl.bind_buffer(Gl::ARRAY_BUFFER, Some(buf));
    unsafe {
        // SAFETY: `view` is consumed immediately by `buffer_data_with_array_buffer_view`
        // before any allocation that could move `data` occurs.
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
        assert!(
            node_phase("a") != node_phase("b"),
            "distinct ids share phase"
        );
    }

    #[test]
    fn spikes_only_for_top_hubs_and_capped() {
        let counts = vec![1u32, 1, 2, 2, 3, 3, 4, 50]; // 50 = clear hub
        let th = hub_spike_threshold(&counts);
        assert!(th >= 4, "threshold too low: {th}");
        assert_eq!(
            spike_strength(1, th),
            0.0,
            "low-degree node must have no spike"
        );
        let s = spike_strength(50, th);
        assert!(s > 0.0 && s <= 0.3, "hub spike must be weak (0,0.3]: {s}");
    }

    #[test]
    fn empty_counts_threshold_is_safe() {
        assert!(hub_spike_threshold(&[]) >= 1);
    }

    #[test]
    fn uniform_degrees_suppress_all_spikes() {
        // Every node same degree → no hub stands out → no spikes (regression:
        // the bare 95th-percentile floor lit every node here).
        let uniform = vec![2u32, 2, 2, 2, 2];
        let th = hub_spike_threshold(&uniform);
        for &c in &uniform {
            assert_eq!(
                spike_strength(c, th),
                0.0,
                "uniform graph must have no spikes"
            );
        }
        // A lone dominant hub in an otherwise low-degree graph still spikes.
        let spread = vec![1u32, 1, 1, 1, 20];
        let th2 = hub_spike_threshold(&spread);
        assert!(
            spike_strength(20, th2) > 0.0,
            "true hub must keep its spike"
        );
        assert_eq!(
            spike_strength(1, th2),
            0.0,
            "low-degree node stays spikeless"
        );
    }
}
