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
            // size grows with degree; highlighted 0.5x, dimmed base.
            let base = 6.0 + (node.link_count as f32).sqrt() * 4.0;
            let lit = !has_hl || hl.map(|s| s.contains(&(i as u32))).unwrap_or(true);
            sizes.push(if lit { base } else { base * 0.5 });
            let [r, g, b] = node.color;
            if lit {
                // HDR boost so bloom picks up a glow corona (1.2 + brightness*0.8).
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
        if self.count == 0 {
            return;
        }
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
