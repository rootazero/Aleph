//! GLSL ES 3.00 shader sources. Filled per-renderer in Tasks 4-6 + Phase 2.

/// Shared idle-drift function injected into BOTH the node and edge vertex
/// shaders via [`with_drift`]. Defining the motion in exactly one place
/// guarantees a node and its connecting edges compute byte-identical drift, so
/// the ribbons can never visually detach from the stars. `phase` ∈ [0,1) is the
/// per-node phase (see `nodes::node_phase`); `time_ms` is `u_time`. All motion
/// is evaluated on the GPU — idle frames upload nothing.
///
/// CONTRACT: `gl/drift.rs` is a Rust twin of this function — CPU-side picking
/// projects the DRIFTED position, so it has to reproduce this motion exactly.
/// The two must be changed together, or the star you click stops being the star
/// you hit. Do not touch DRIFT_AMP, DRIFT_OMEGA or the per-axis phase offsets in
/// isolation.
pub const DRIFT_GLSL: &str = r#"
const float DRIFT_AMP = 3.0;
const float DRIFT_TAU = 6.28318530718;
const float DRIFT_OMEGA = DRIFT_TAU / 5.0;   // period 5000ms
vec3 idle_drift(vec3 base, float phase, float time_ms) {
    float t = time_ms / 1000.0;
    float ph = phase * DRIFT_TAU;
    return base + DRIFT_AMP * vec3(
        sin(DRIFT_OMEGA * t + ph),
        sin(DRIFT_OMEGA * t + ph + 0.27 * DRIFT_TAU),
        sin(DRIFT_OMEGA * t + ph + 0.54 * DRIFT_TAU)
    );
}
"#;

/// Inject [`DRIFT_GLSL`] into a vertex shader source immediately after its
/// `precision` qualifier (keeping `#version` on the first line). The drift
/// function depends only on its parameters, so it is valid anywhere at file
/// scope before `main`. Call only on the two vertex shaders that use drift.
pub fn with_drift(src: &str) -> String {
    const ANCHOR: &str = "precision highp float;";
    src.replacen(ANCHOR, &format!("{ANCHOR}\n{DRIFT_GLSL}"), 1)
}

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
out float v_twinkle;
void main() {
    // Idle drift is the shared GPU function (also used by the edge shader so
    // edges track their nodes exactly — see shaders::DRIFT_GLSL).
    vec4 clip = u_view_proj * vec4(idle_drift(a_offset, a_phase, u_time), 1.0);
    vec2 px = a_corner * a_size / u_viewport * clip.w * 2.0;
    clip.xy += px;
    gl_Position = clip;
    v_corner = a_corner;
    v_color = a_color;
    // Near-fade: spikes only show at distance; vanish when zoomed in / clustered.
    float fade = smoothstep(300.0, 900.0, u_cam_dist);
    v_spike = a_spike * fade;
    // Subtle per-node brightness twinkle: slow sine wave modulated by per-node phase.
    v_twinkle = 0.9 + 0.1 * sin(u_time / 1000.0 * 1.7 + a_phase * 6.2831853);
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
in float v_twinkle;
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
    vec3  rgb  = v_color * (core * 1.6 * v_twinkle + halo + cross);
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
layout(location=5) in float a_highlight; // 1.0 = neighbor of selected, 0.0 = other
layout(location=6) in float a_phase_a;   // drift phase of endpoint a (= node_phase)
layout(location=7) in float a_phase_b;   // drift phase of endpoint b
uniform mat4 u_view_proj;
uniform vec2 u_viewport;
uniform float u_width;
uniform float u_time;        // ms (drives idle_drift here + flow effect in frag)
out vec3 v_color;
out float v_along;
out float v_depth;
out float v_hl;

vec3 bezier(vec3 a, vec3 c, vec3 b, float t) {
    float u = 1.0 - t;
    return u*u*a + 2.0*u*t*c + t*t*b;
}
void main() {
    float t = a_corner.x;
    // Drift both endpoints in lockstep with their nodes (idle_drift is the same
    // GPU function the node shader uses) so the ribbon stays welded to the stars.
    vec3 pa = idle_drift(a_pos_a, a_phase_a, u_time);
    vec3 pb = idle_drift(a_pos_b, a_phase_b, u_time);
    // Control point: midpoint bowed along a world-perp of the chord.
    vec3 chord = pb - pa;
    float len = length(chord);
    vec3 dir = len > 1e-4 ? chord / len : vec3(1.0, 0.0, 0.0);
    vec3 up = abs(dir.y) < 0.95 ? vec3(0.0,1.0,0.0) : vec3(1.0,0.0,0.0);
    vec3 perp = normalize(cross(dir, up));
    vec3 ctrl = (pa + pb) * 0.5 + perp * (len * 0.12);
    // Sample curve point and a neighbor for screen-space tangent.
    vec3 p  = bezier(pa, ctrl, pb, t);
    float dt = 0.01;
    vec3 p2 = bezier(pa, ctrl, pb, clamp(t + dt, 0.0, 1.0));
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
    // Highlighted incident edges are 1.5x thicker, so the selected chain reads as
    // a shape and not only as a hue. a_highlight is 0.0 on every edge when nothing
    // is selected, so this is a no-op in the default view.
    float width = mix(u_width, u_width * 1.5, a_highlight);
    vec2 off_px = nrm * (a_corner.y * width * 0.5 * taper);
    cp.xy += off_px * 2.0 / u_viewport * cp.w;
    gl_Position = cp;
    v_color = mix(a_color_a, a_color_b, t);
    v_along = t;
    v_depth = cp.z / max(cp.w, 1e-4);
    v_hl = a_highlight;
}
"#;

/// Edge fragment shader: endpoint weld brightening + depth fade.
/// When a node is selected (`u_select_active`): neighbor edges (`v_hl`) get an
/// animated energy-flow band; non-neighbor edges dim to focus the selected chain.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression guard for the node/edge detachment bug: the idle-drift motion
    /// must be defined once and injected into BOTH vertex shaders, and each must
    /// actually drift its own geometry (node offset; both edge endpoints). If a
    /// future edit drops drift from the edge shader, edges detach again — caught
    /// here.
    #[test]
    fn drift_injected_into_both_vertex_shaders() {
        let node = with_drift(NODE_VERT);
        let edge = with_drift(EDGE_VERT);
        assert!(
            node.contains("vec3 idle_drift("),
            "node shader missing drift fn"
        );
        assert!(
            edge.contains("vec3 idle_drift("),
            "edge shader missing drift fn"
        );
        assert!(
            node.contains("idle_drift(a_offset"),
            "node must drift its offset"
        );
        assert!(
            edge.contains("idle_drift(a_pos_a"),
            "edge must drift endpoint a"
        );
        assert!(
            edge.contains("idle_drift(a_pos_b"),
            "edge must drift endpoint b"
        );
    }

    /// The selected chain must be legible as a SHAPE, not only as a hue: the edge
    /// vertex shader has to widen on the per-instance highlight flag rather than
    /// use the flat `u_width` everywhere. Guards against a revert to the flat
    /// width (which made a highlighted edge indistinguishable in a dense hairball).
    #[test]
    fn edge_width_lerps_on_highlight_flag() {
        assert!(
            EDGE_VERT.contains("mix(u_width, u_width * 1.5, a_highlight)"),
            "highlighted edges must render thicker than the base width"
        );
        assert!(
            !EDGE_VERT.contains("u_width * 0.5 * taper"),
            "the flat-width ribbon offset must be gone"
        );
    }

    #[test]
    fn with_drift_inserts_once_and_keeps_version_first() {
        let out = with_drift(NODE_VERT);
        assert!(
            out.starts_with("#version 300 es"),
            "#version must stay on line 1"
        );
        assert_eq!(
            out.matches("vec3 idle_drift(").count(),
            1,
            "drift fn definition must appear exactly once"
        );
    }
}
