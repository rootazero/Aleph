//! GLSL ES 3.00 shader sources. Filled per-renderer in Tasks 4-6 + Phase 2.

/// Node billboard vertex shader. Per-instance: a_offset(vec3), a_size(float),
/// a_color(vec3), a_phase(float), a_spike(float). Per-vertex: a_corner(vec2 in [-1,1]).
/// Idle drift is computed entirely on the GPU from u_time + a_phase so that
/// idle frames upload nothing and do no CPU sine work.
pub const NODE_VERT: &str = r#"#version 300 es
precision highp float;
layout(location=0) in vec2 a_corner;
layout(location=1) in vec3 a_offset;
layout(location=2) in float a_size;
layout(location=3) in vec3 a_color;
layout(location=4) in float a_phase;
layout(location=5) in float a_spike;
uniform mat4 u_view_proj;
uniform vec2 u_viewport;
uniform float u_time;        // ms
uniform float u_cam_dist;    // camera distance for spike near-fade
out vec2 v_corner;
out vec3 v_color;
out float v_spike;
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
    // Near-fade: spikes only show at distance; vanish when zoomed in / clustered.
    float fade = smoothstep(300.0, 900.0, u_cam_dist);
    v_spike = a_spike * fade;
}
"#;

/// Node fragment: crisp solid core + soft outer halo (HDR; bloom adds the corona).
/// Hub nodes additionally draw a weak diffraction cross (v_spike <= 0.3) that fades
/// out at close zoom (v_spike is pre-multiplied by a smoothstep in the vertex shader).
pub const NODE_FRAG: &str = r#"#version 300 es
precision highp float;
in vec2 v_corner;
in vec3 v_color;
in float v_spike;
out vec4 frag;
void main() {
    float r = length(v_corner);
    if (r > 1.0) discard;
    // Crisp core: hard, tight falloff → a defined bright point.
    float core = smoothstep(0.30, 0.0, r);
    core = core * core;                       // sharpen the core profile
    // Soft halo: wide gentle falloff, low weight; bloom turns this into the glow.
    float halo = smoothstep(1.0, 0.0, r) * 0.35;
    // Weak diffraction cross for hubs only. abs() arms along the two axes; very
    // thin, alpha-capped by v_spike (<=0.3). Never drawn when v_spike == 0.
    float cross = 0.0;
    if (v_spike > 0.0) {
        float ax = 1.0 - smoothstep(0.0, 0.06, abs(v_corner.x));
        float ay = 1.0 - smoothstep(0.0, 0.06, abs(v_corner.y));
        float radial = 1.0 - smoothstep(0.0, 1.0, r); // fade arms toward rim
        cross = max(ax, ay) * radial * v_spike;
    }
    vec3  rgb  = v_color * (core * 1.6 + halo + cross);
    float a    = clamp(core + halo * 0.6 + cross, 0.0, 1.0);
    frag = vec4(rgb, a);
}
"#;

/// Edge vertex shader: evaluates a gentle quadratic Bézier arc tessellated into
/// SEGMENTS steps, with endpoint taper so the ribbon visually welds into the star
/// core. Per-instance: the two endpoints (a_pos_a/a_pos_b) and their colors
/// (a_color_a/a_color_b). Per-vertex: a_corner = (along ∈ [0,1], side ∈ {-1,+1}).
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
uniform float u_time;        // ms (used by Task 5 flow; harmless here)
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

/// Edge fragment shader: endpoint weld brightening (visually plugs into the star
/// core) and gentle depth fade. `v_along` is passed through for Task 5 flow effect.
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
