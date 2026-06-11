use leptos::prelude::*;

/// Visually-anchored edge label, positioned in screen space.
/// Pre-computed `screen_xy` and `tangent_rad_clamped` come from the rAF loop.
#[component]
pub fn EdgeLabel(
    #[prop(into)] text: String,
    screen_xy: ReadSignal<(f32, f32)>,
    /// Tangent in radians, ALREADY clamped to `[-π/4, π/4]` by caller.
    tangent_rad: ReadSignal<f32>,
    /// Visibility: true iff the underlying edge is adjacent to a hovered/selected node
    /// AND zoom ≥ 0.7. Caller computes this.
    visible: ReadSignal<bool>,
) -> impl IntoView {
    let style = move || {
        let (x, y) = screen_xy.get();
        let deg = tangent_rad.get().to_degrees();
        let opacity = if visible.get() { 1.0 } else { 0.0 };
        format!(
            "position:absolute;left:0;top:0;\
             transform:translate3d({x:.1}px,{y:.1}px,0) translate(-50%,-50%) rotate({deg:.1}deg);\
             padding:2px 8px;border-radius:6px;\
             background:rgba(15,23,42,0.85);color:#cbd5e1;\
             font-size:10px;white-space:nowrap;\
             opacity:{opacity:.2};transition:opacity 120ms ease-out;pointer-events:none"
        )
    };

    view! {
        <div style=style>{text}</div>
    }
}
