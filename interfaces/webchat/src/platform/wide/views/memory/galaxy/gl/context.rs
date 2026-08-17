//! WebGL2 context acquisition + GLSL program compilation.

use wasm_bindgen::JsCast;
use web_sys::{HtmlCanvasElement, WebGl2RenderingContext as Gl, WebGlProgram, WebGlShader};

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
