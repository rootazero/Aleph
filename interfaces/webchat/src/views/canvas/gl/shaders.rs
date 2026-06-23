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
