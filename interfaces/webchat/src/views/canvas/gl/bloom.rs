//! FBO bloom post-processing pipeline: bright-pass → separable gaussian blur → composite.
//!
//! Pure-logic functions (`gaussian_weights`) are unit-tested on native target.
//! GL-bound code (`BloomPipeline`) is verified by WASM compile gate + browser visual check.

use web_sys::{
    WebGl2RenderingContext as Gl, WebGlFramebuffer, WebGlProgram, WebGlTexture,
    WebGlVertexArrayObject,
};

use super::context::compile_program;
use super::shaders::{BLUR_FRAG, BRIGHT_FRAG, COMPOSITE_FRAG, FULLSCREEN_VERT};

// ---------------------------------------------------------------------------
// Pure logic: gaussian weights (unit-tested on native)
// ---------------------------------------------------------------------------

/// Returns a symmetric normalized gaussian kernel of length `2*radius+1`.
/// The kernel sums to 1.0 and the center weight is the heaviest.
pub fn gaussian_weights(radius: usize) -> Vec<f32> {
    let sigma = (radius as f32 / 2.0).max(1.0);
    let mut w: Vec<f32> = (0..=2 * radius)
        .map(|i| {
            let x = i as f32 - radius as f32;
            (-(x * x) / (2.0 * sigma * sigma)).exp()
        })
        .collect();
    let sum: f32 = w.iter().sum();
    for v in &mut w {
        *v /= sum;
    }
    w
}

// ---------------------------------------------------------------------------
// GL-bound: BloomPipeline
// ---------------------------------------------------------------------------

/// One FBO + its color texture, at a given resolution.
struct FboTex {
    fbo: WebGlFramebuffer,
    tex: WebGlTexture,
    w: i32,
    h: i32,
}

impl FboTex {
    /// Allocate an FBO with a single color texture attachment.
    /// Uses RGBA16F when `float_ext` is true, otherwise RGBA8.
    fn new(gl: &Gl, w: i32, h: i32, float_ext: bool) -> Result<FboTex, String> {
        let tex = gl.create_texture().ok_or("create_texture failed")?;
        gl.bind_texture(Gl::TEXTURE_2D, Some(&tex));
        gl.tex_parameteri(Gl::TEXTURE_2D, Gl::TEXTURE_MIN_FILTER, Gl::LINEAR as i32);
        gl.tex_parameteri(Gl::TEXTURE_2D, Gl::TEXTURE_MAG_FILTER, Gl::LINEAR as i32);
        gl.tex_parameteri(Gl::TEXTURE_2D, Gl::TEXTURE_WRAP_S, Gl::CLAMP_TO_EDGE as i32);
        gl.tex_parameteri(Gl::TEXTURE_2D, Gl::TEXTURE_WRAP_T, Gl::CLAMP_TO_EDGE as i32);

        if float_ext {
            // RGBA16F: uses tex_image_2d with type HALF_FLOAT and null data.
            gl.tex_image_2d_with_i32_and_i32_and_i32_and_format_and_type_and_opt_u8_array(
                Gl::TEXTURE_2D,
                0,
                Gl::RGBA16F as i32,
                w,
                h,
                0,
                Gl::RGBA,
                Gl::HALF_FLOAT,
                None,
            )
            .map_err(|e| format!("tex_image_2d RGBA16F: {:?}", e))?;
        } else {
            // RGBA8 fallback: UNSIGNED_BYTE with null data.
            gl.tex_image_2d_with_i32_and_i32_and_i32_and_format_and_type_and_opt_u8_array(
                Gl::TEXTURE_2D,
                0,
                Gl::RGBA as i32,
                w,
                h,
                0,
                Gl::RGBA,
                Gl::UNSIGNED_BYTE,
                None,
            )
            .map_err(|e| format!("tex_image_2d RGBA8: {:?}", e))?;
        }

        let fbo = gl.create_framebuffer().ok_or("create_framebuffer failed")?;
        gl.bind_framebuffer(Gl::FRAMEBUFFER, Some(&fbo));
        gl.framebuffer_texture_2d(
            Gl::FRAMEBUFFER,
            Gl::COLOR_ATTACHMENT0,
            Gl::TEXTURE_2D,
            Some(&tex),
            0,
        );
        gl.bind_framebuffer(Gl::FRAMEBUFFER, None);
        gl.bind_texture(Gl::TEXTURE_2D, None);

        Ok(FboTex { fbo, tex, w, h })
    }
}

/// Bloom post-processing pipeline.
///
/// Usage:
/// 1. Bind `scene_fbo()` and render the scene into it.
/// 2. Call `run(gl)` to execute bright-pass → H-blur → V-blur → composite
///    and write the final image to the default framebuffer.
pub struct BloomPipeline {
    /// Full-resolution scene FBO (render target for the main scene pass).
    scene: FboTex,
    /// Half-resolution ping-pong FBOs for blur passes.
    pp: [FboTex; 2],
    /// bright-pass shader program.
    prog_bright: WebGlProgram,
    /// Separable gaussian blur shader program.
    prog_blur: WebGlProgram,
    /// Composite (scene + bloom) shader program.
    prog_composite: WebGlProgram,
    /// Whether EXT_color_buffer_float was available.
    float_ext: bool,
    /// Reusable empty VAO for gl_VertexID-based fullscreen draws (created once).
    empty_vao: WebGlVertexArrayObject,
}

impl BloomPipeline {
    pub fn new(gl: &Gl, w: i32, h: i32) -> Result<BloomPipeline, String> {
        let float_ext = gl.get_extension("EXT_color_buffer_float").ok().flatten().is_some();
        let hw = (w / 2).max(1);
        let hh = (h / 2).max(1);

        let scene = FboTex::new(gl, w, h, float_ext)?;
        let pp = [FboTex::new(gl, hw, hh, float_ext)?, FboTex::new(gl, hw, hh, float_ext)?];

        let prog_bright = compile_program(gl, FULLSCREEN_VERT, BRIGHT_FRAG)?;
        let prog_blur = compile_program(gl, FULLSCREEN_VERT, BLUR_FRAG)?;
        let prog_composite = compile_program(gl, FULLSCREEN_VERT, COMPOSITE_FRAG)?;

        let empty_vao = gl.create_vertex_array().ok_or("bloom vao")?;

        Ok(BloomPipeline {
            scene,
            pp,
            prog_bright,
            prog_blur,
            prog_composite,
            float_ext,
            empty_vao,
        })
    }

    /// Resize all FBOs. Call this from `Scene::resize`.
    pub fn resize(&mut self, gl: &Gl, w: i32, h: i32) -> Result<(), String> {
        let hw = (w / 2).max(1);
        let hh = (h / 2).max(1);
        self.scene = FboTex::new(gl, w, h, self.float_ext)?;
        self.pp = [
            FboTex::new(gl, hw, hh, self.float_ext)?,
            FboTex::new(gl, hw, hh, self.float_ext)?,
        ];
        Ok(())
    }

    /// The scene FBO that the caller must bind before drawing the scene.
    pub fn scene_fbo(&self) -> &WebGlFramebuffer {
        &self.scene.fbo
    }

    /// Execute the full bloom pipeline and composite to the default framebuffer.
    ///
    /// Blend state: bloom passes run with BLEND disabled. The caller is
    /// responsible for managing blend state around the scene draw itself.
    pub fn run(&self, gl: &Gl) {
        // Disable blending for all fullscreen bloom passes.
        gl.disable(Gl::BLEND);

        // --- Pass 1: Bright-pass (scene → pp[0]) ---
        let hw = self.pp[0].w;
        let hh = self.pp[0].h;
        gl.bind_framebuffer(Gl::FRAMEBUFFER, Some(&self.pp[0].fbo));
        gl.viewport(0, 0, hw, hh);
        gl.use_program(Some(&self.prog_bright));

        // Bind scene texture to unit 0.
        gl.active_texture(Gl::TEXTURE0);
        gl.bind_texture(Gl::TEXTURE_2D, Some(&self.scene.tex));
        let loc = gl.get_uniform_location(&self.prog_bright, "u_tex");
        gl.uniform1i(loc.as_ref(), 0);
        let loc = gl.get_uniform_location(&self.prog_bright, "u_threshold");
        gl.uniform1f(loc.as_ref(), 0.5);

        self.draw_fullscreen(gl);

        // --- Pass 2: Horizontal blur (pp[0] → pp[1]) ---
        gl.bind_framebuffer(Gl::FRAMEBUFFER, Some(&self.pp[1].fbo));
        gl.use_program(Some(&self.prog_blur));

        gl.active_texture(Gl::TEXTURE0);
        gl.bind_texture(Gl::TEXTURE_2D, Some(&self.pp[0].tex));
        let loc = gl.get_uniform_location(&self.prog_blur, "u_tex");
        gl.uniform1i(loc.as_ref(), 0);
        // Horizontal direction.
        let loc = gl.get_uniform_location(&self.prog_blur, "u_dir");
        gl.uniform2f(loc.as_ref(), 1.0, 0.0);
        let loc = gl.get_uniform_location(&self.prog_blur, "u_texel");
        gl.uniform2f(loc.as_ref(), 1.0 / hw as f32, 1.0 / hh as f32);

        self.draw_fullscreen(gl);

        // --- Pass 3: Vertical blur (pp[1] → pp[0]) ---
        gl.bind_framebuffer(Gl::FRAMEBUFFER, Some(&self.pp[0].fbo));
        gl.use_program(Some(&self.prog_blur));

        gl.active_texture(Gl::TEXTURE0);
        gl.bind_texture(Gl::TEXTURE_2D, Some(&self.pp[1].tex));
        let loc = gl.get_uniform_location(&self.prog_blur, "u_tex");
        gl.uniform1i(loc.as_ref(), 0);
        // Vertical direction.
        let loc = gl.get_uniform_location(&self.prog_blur, "u_dir");
        gl.uniform2f(loc.as_ref(), 0.0, 1.0);
        // u_texel stays the same.
        let loc = gl.get_uniform_location(&self.prog_blur, "u_texel");
        gl.uniform2f(loc.as_ref(), 1.0 / hw as f32, 1.0 / hh as f32);

        self.draw_fullscreen(gl);

        // --- Pass 4: Composite (scene + bloom → default framebuffer) ---
        gl.bind_framebuffer(Gl::FRAMEBUFFER, None);
        gl.viewport(0, 0, self.scene.w, self.scene.h);
        gl.use_program(Some(&self.prog_composite));

        // Scene texture on unit 0, bloom texture on unit 1.
        gl.active_texture(Gl::TEXTURE0);
        gl.bind_texture(Gl::TEXTURE_2D, Some(&self.scene.tex));
        let loc = gl.get_uniform_location(&self.prog_composite, "u_scene");
        gl.uniform1i(loc.as_ref(), 0);

        gl.active_texture(Gl::TEXTURE1);
        gl.bind_texture(Gl::TEXTURE_2D, Some(&self.pp[0].tex));
        let loc = gl.get_uniform_location(&self.prog_composite, "u_bloom");
        gl.uniform1i(loc.as_ref(), 1);

        let loc = gl.get_uniform_location(&self.prog_composite, "u_intensity");
        gl.uniform1f(loc.as_ref(), 0.9);

        self.draw_fullscreen(gl);

        // Clean up texture bindings.
        gl.active_texture(Gl::TEXTURE1);
        gl.bind_texture(Gl::TEXTURE_2D, None);
        gl.active_texture(Gl::TEXTURE0);
        gl.bind_texture(Gl::TEXTURE_2D, None);
    }

    /// Draw a fullscreen triangle via the gl_VertexID trick (no vertex buffer needed).
    /// Binds the shared `empty_vao` (created once in `new`) to satisfy the WebGL2
    /// requirement that a VAO is bound before draw calls.
    fn draw_fullscreen(&self, gl: &Gl) {
        gl.bind_vertex_array(Some(&self.empty_vao));
        gl.draw_arrays(Gl::TRIANGLES, 0, 3);
        gl.bind_vertex_array(None);
    }
}

// ---------------------------------------------------------------------------
// Unit tests (native target only — no WebGL)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights_sum_to_one_and_symmetric() {
        let w = gaussian_weights(4);
        let sum: f32 = w.iter().sum();
        assert!((sum - 1.0).abs() < 1e-3, "sum={sum}");
        assert_eq!(w.len(), 9); // 2*radius+1
        for i in 0..4 {
            assert!((w[i] - w[8 - i]).abs() < 1e-6);
        }
        assert!(w[4] > w[0]); // center heaviest
    }
}
