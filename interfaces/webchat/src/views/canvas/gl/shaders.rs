//! GLSL ES 3.00 shader sources. Filled per-renderer in Tasks 4-6 + Phase 2.

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

// ---------------------------------------------------------------------------
// Bloom post-processing shaders (Task 9)
// ---------------------------------------------------------------------------

/// Fullscreen triangle vertex shader. Uses gl_VertexID — no vertex buffer required.
pub const FULLSCREEN_VERT: &str = r#"#version 300 es
precision highp float;
out vec2 v_uv;
void main() {
    vec2 p = vec2((gl_VertexID << 1) & 2, gl_VertexID & 2);
    v_uv = p;
    gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}
"#;

/// Bright-pass fragment shader: extracts luminance above threshold.
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

/// Separable gaussian blur fragment shader. Set `u_dir` to (1,0) or (0,1).
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

/// Composite fragment shader: additively blends bloom over the scene.
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
