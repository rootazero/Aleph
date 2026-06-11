use leptos::prelude::*;

use crate::canvas_engine::category_color::category_color;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CardMode {
    Full,
    Mini,
    Dot,
}

/// Choose the render mode for a node given its hop / hover / select / zoom state.
///
/// Only **hover** triggers Full mode (a transient on-canvas preview). The
/// focused / selected center node stays Mini — its rich detail is rendered
/// in the sidebar `NodeDetailPanel`, so the canvas stays readable
/// (no large card occluding 1-hop neighbors and edges).
#[must_use]
pub fn pick_mode(hop: u8, is_hovered: bool, _is_selected: bool, zoom: f32) -> CardMode {
    if zoom < 0.5 {
        return CardMode::Dot;
    }
    if is_hovered {
        return CardMode::Full;
    }
    if hop <= 1 {
        CardMode::Mini
    } else {
        CardMode::Dot
    }
}

/// Three-mode node card overlay for the memory canvas.
///
/// - FULL (280px): stripe + title + optional excerpt + optional tags footer.
///   Breath animation when hop==0 (active center). Class `node-card-full`.
/// - MINI (140px): compact title pill with colored stripe. Class `node-card-mini`.
/// - DOT (10px): round dot colored by category. Class `node-card-dot`.
///
/// All modes are positioned via `transform: translate3d(x,y,0)` using `screen_xy`.
#[component]
#[must_use]
pub fn NodeCard(
    /// Node id (used as click target).
    #[prop(into)]
    id: String,
    /// Node display name (title, never Markdown).
    #[prop(into)]
    name: String,
    /// Free-form category — colors the stripe.
    #[prop(into)]
    category: String,
    /// Tags shown in the meta footer.
    #[prop(default = vec![])]
    tags: Vec<String>,
    /// Pre-rendered HTML excerpt (already sanitized by `render_excerpt`).
    /// Empty string is allowed (renders no body).
    #[prop(into)]
    excerpt_html: String,
    /// Hop distance from the center node; 0 = center.
    hop: u8,
    /// Reactive — current screen position of this node.
    screen_xy: ReadSignal<(f32, f32)>,
    /// Current canvas zoom.
    zoom: ReadSignal<f32>,
    /// Reactive — id of the hovered node (if any).
    hovered_id: ReadSignal<Option<String>>,
    /// Reactive — id of the selected node (if any).
    selected_id: ReadSignal<Option<String>>,
) -> impl IntoView {
    let stripe_color = category_color(&category);

    // IDs cloned for use inside the reactive outer closure and sub-closures.
    // Each one is consumed exactly once (either by `mode` Memo or by one match arm).
    let id_hover = id.clone();
    let id_sel = id.clone();

    let mode = Memo::new(move |_| {
        let is_hovered = hovered_id.with(|h| h.as_deref() == Some(id_hover.as_str()));
        let is_selected = selected_id.with(|s| s.as_deref() == Some(id_sel.as_str()));
        pick_mode(hop, is_hovered, is_selected, zoom.get())
    });

    // Static values: computed once, cloned per invocation of the outer FnMut closure.
    let data_active = hop == 0;
    let has_excerpt = !excerpt_html.is_empty();
    let has_tags = !tags.is_empty();

    // The outer closure is `FnMut` — it must clone any String it consumes each call.
    // We therefore keep `id`, `name`, `excerpt_html`, `tags`, `stripe_color` as owned
    // Strings in the closure environment and clone them into local bindings each arm.
    move || match mode.get() {
        // ------------------------------------------------------------------
        // FULL mode: 280px card with stripe, title, optional excerpt/tags
        // ------------------------------------------------------------------
        CardMode::Full => {
            let stripe = stripe_color.clone();
            let name_v = name.clone();
            let excerpt = excerpt_html.clone();
            let tags_v = tags.clone();
            let data_id = id.clone();
            let sel_id = id.clone();

            // Reactive data-selected attribute (re-evaluated each render tick)
            let is_selected_attr = move || {
                selected_id
                    .with(|s| s.as_deref() == Some(sel_id.as_str()))
                    .to_string()
            };

            // Non-reactive position style (x,y sampled reactively via screen_xy)
            let style_str = move || {
                let (x, y) = screen_xy.get();
                format!(
                    "transform: translate3d({:.1}px, {:.1}px, 0);",
                    x - 140.0,
                    y - 60.0
                )
            };

            let stripe_bar_style = format!(
                "height: 3px; background-color: {stripe}; border-radius: 6px 6px 0 0;"
            );

            view! {
                <div
                    class="node-card-full"
                    style=style_str
                    data-id=data_id
                    data-active=data_active.to_string()
                    data-selected=is_selected_attr
                >
                    // Category stripe (3px top bar)
                    <div style=stripe_bar_style />
                    // Title
                    <div class="node-card-full__title">{name_v}</div>
                    // Optional excerpt (pre-sanitized HTML — safe by design)
                    {has_excerpt.then(move || view! {
                        <div class="node-card-full__excerpt" inner_html=excerpt />
                    })}
                    // Optional tags footer
                    {has_tags.then(move || {
                        let chips = tags_v.iter().map(|t| {
                            let label = format!("#{t}");
                            view! { <span class="node-card-tag">{label}</span> }
                        }).collect::<Vec<_>>();
                        view! {
                            <div class="node-card-full__tags">{chips}</div>
                        }
                    })}
                </div>
            }
            .into_any()
        }

        // ------------------------------------------------------------------
        // MINI mode: 140px compact title pill with inline colored stripe square
        // ------------------------------------------------------------------
        CardMode::Mini => {
            let stripe = stripe_color.clone();
            let name_v = name.clone();
            let data_id = id.clone();
            let sel_id = id.clone();

            // Reactive data-selected attribute — drives the golden halo CSS rule.
            let is_selected_attr = move || {
                selected_id
                    .with(|s| s.as_deref() == Some(sel_id.as_str()))
                    .to_string()
            };

            let style_str = move || {
                let (x, y) = screen_xy.get();
                format!(
                    "transform: translate3d({:.1}px, {:.1}px, 0);",
                    x - 70.0,
                    y - 15.0
                )
            };

            let stripe_sq_style = format!(
                "width: 8px; height: 8px; flex-shrink: 0; border-radius: 2px; background-color: {stripe};"
            );

            view! {
                <div
                    class="node-card-mini"
                    style=style_str
                    data-id=data_id
                    data-active=data_active.to_string()
                    data-selected=is_selected_attr
                >
                    <div style=stripe_sq_style />
                    <span class="node-card-mini__name">{name_v}</span>
                </div>
            }
            .into_any()
        }

        // ------------------------------------------------------------------
        // DOT mode: 10px round dot, colored by category
        // ------------------------------------------------------------------
        CardMode::Dot => {
            let dot_color = stripe_color.clone();
            let data_id = id.clone();

            let style_str = move || {
                let (x, y) = screen_xy.get();
                format!(
                    "transform: translate3d({:.1}px, {:.1}px, 0); background-color: {};",
                    x - 5.0,
                    y - 5.0,
                    dot_color
                )
            };

            view! {
                <div
                    class="node-card-dot"
                    style=style_str
                    data-id=data_id
                />
            }
            .into_any()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_mode_dot_under_low_zoom() {
        assert_eq!(pick_mode(0, false, false, 0.3), CardMode::Dot);
        assert_eq!(pick_mode(1, true, true, 0.3), CardMode::Dot);
    }

    #[test]
    fn pick_mode_center_stays_mini() {
        // hop==0 (the focused center node) no longer promotes to Full —
        // its content is rendered in the sidebar NodeDetailPanel instead,
        // so the canvas stays readable.
        assert_eq!(pick_mode(0, false, false, 1.0), CardMode::Mini);
    }

    #[test]
    fn pick_mode_selected_does_not_promote() {
        // Selection alone no longer promotes to Full; the halo/breath
        // animation on the Mini card itself signals the active focus.
        assert_eq!(pick_mode(1, false, true, 1.0), CardMode::Mini);
        assert_eq!(pick_mode(2, false, true, 1.0), CardMode::Dot);
    }

    #[test]
    fn pick_mode_hover_is_only_full_trigger() {
        assert_eq!(pick_mode(2, false, false, 1.0), CardMode::Dot);
        assert_eq!(pick_mode(2, true, false, 1.0), CardMode::Full);
        assert_eq!(pick_mode(1, false, false, 1.0), CardMode::Mini);
        assert_eq!(pick_mode(1, true, false, 1.0), CardMode::Full);
    }
}
