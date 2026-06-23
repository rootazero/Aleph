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
        self.upload_indexed(gl, &data.nodes, &data.edges);
    }

    /// Upload edges from an explicit nodes slice + edge-index slice.
    /// Avoids cloning GraphData: callers can pass current node positions and a
    /// pre-filtered edge list without allocating a temporary GraphData.
    pub fn upload_indexed(
        &mut self,
        gl: &Gl,
        nodes: &[super::GalaxyNode],
        edges: &[(u32, u32)],
    ) {
        let mut pos = Vec::with_capacity(edges.len() * 6);
        let mut col = Vec::with_capacity(edges.len() * 6);
        for &(a, b) in edges {
            let (na, nb) = (&nodes[a as usize], &nodes[b as usize]);
            pos.extend_from_slice(&[na.pos.x, na.pos.y, na.pos.z, nb.pos.x, nb.pos.y, nb.pos.z]);
            col.extend_from_slice(&na.color);
            col.extend_from_slice(&nb.color);
        }
        self.vert_count = (edges.len() * 2) as i32;
        bind_upload(gl, &self.pos_buf, &pos);
        bind_upload(gl, &self.col_buf, &col);
    }

    pub fn draw(&self, gl: &Gl, view_proj: &Mat4) {
        if self.vert_count == 0 {
            return;
        }
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
        // SAFETY: `view` is consumed immediately by `buffer_data_with_array_buffer_view`
        // before any allocation that could move `data` occurs.
        let view = js_sys::Float32Array::view(data);
        gl.buffer_data_with_array_buffer_view(Gl::ARRAY_BUFFER, &view, Gl::DYNAMIC_DRAW);
    }
}
