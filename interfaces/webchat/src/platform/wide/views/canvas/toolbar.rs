//! Floating tool switcher and style panel for the whiteboard editor — the
//! only writer of `CanvasState::tool` besides the editor's own
//! drop-back-to-Select after a creation gesture, and the ONLY writer of
//! `CanvasState::style`.
//!
//! Not in the plan's Task-14 file list: the plan omitted a toolbar entirely,
//! which would have left every creation tool unreachable (a signal with no
//! writer — the "capability wired ≠ capability delivered" failure). Every
//! tool the editor implements gets a button; every geo form the contract
//! knows gets one too.
//!
//! # Style panel (2026-09-12)
//!
//! Until this panel existed every human-created shape was born with
//! `ShapeStyle::default()` — the renderer could draw seven colors, fills,
//! three sizes and four stroke kinds, and no person could ask for any of
//! them. The panel is a second floating row above the tool strip: the
//! contract's [`PALETTE_SLOTS`] as swatches (painted with the same theme
//! token the shapes use), a hex picker gated by the contract's
//! [`check_color`], the three sizes, the fill toggle and the four stroke
//! kinds. The editor reads `CanvasState::style` once at the start of each
//! creation gesture (`interaction.rs` carries it through the drag).
//!
//! Both rows sit inside the editor surface, after the overlay layer in DOM
//! order (so they paint on top), and stop pointer/wheel propagation — a
//! click on a button must not start a marquee underneath it.

use aleph_protocol::canvas::{
    check_color, GeoForm, ShapeStyle, SizeKind, StrokeKind, PALETTE_SLOTS,
};
use leptos::prelude::*;

use super::shape_view::palette_var;
use crate::i18n::{t_string, use_i18n};
use crate::state::canvas::{CanvasState, CanvasTool};

/// `style` with `color` replaced — through the contract's gate, so the
/// panel can never mint a color the server would refuse (and every shape
/// drawn afterwards would fail to save).
pub(super) fn style_with_color(style: &ShapeStyle, color: &str) -> Result<ShapeStyle, String> {
    check_color(color)?;
    Ok(ShapeStyle {
        color: color.to_string(),
        ..style.clone()
    })
}

/// Whether a swatch for `slot` reads as selected: the empty wire default
/// and `"default"` are the same slot on every renderer.
#[must_use]
pub(super) fn slot_selected(style: &ShapeStyle, slot: &str) -> bool {
    style.color == slot || (slot == "default" && style.color.is_empty())
}

#[component]
fn ToolButton(
    tool: CanvasTool,
    #[prop(into)] title: Signal<String>,
    children: Children,
) -> impl IntoView {
    let canvas = expect_context::<CanvasState>();
    view! {
        <button
            class=move || {
                let base = "p-2 rounded-lg transition-colors";
                if canvas.tool.get() == tool {
                    format!("{base} bg-primary/15 text-primary")
                } else {
                    format!(
                        "{base} text-text-secondary hover:text-text-primary hover:bg-surface-sunken"
                    )
                }
            }
            title=move || title.get()
            on:click=move |_| canvas.tool.set(tool)
        >
            {children()}
        </button>
    }
}

/// A small toggle in the style panel: `active` paints it selected, the
/// click writes the new style.
#[component]
fn StyleToggle(
    #[prop(into)] active: Signal<bool>,
    #[prop(into)] title: Signal<String>,
    on_pick: Callback<()>,
    children: Children,
) -> impl IntoView {
    view! {
        <button
            class=move || {
                let base = "px-1.5 py-1 rounded-md text-[11px] font-medium transition-colors";
                if active.get() {
                    format!("{base} bg-primary/15 text-primary")
                } else {
                    format!(
                        "{base} text-text-secondary hover:text-text-primary hover:bg-surface-sunken"
                    )
                }
            }
            title=move || title.get()
            on:click=move |_| on_pick.run(())
        >
            {children()}
        </button>
    }
}

/// A 16×16 stroked icon frame — every tool glyph shares it.
macro_rules! icon {
    ($($body:tt)*) => {
        view! {
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                 stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                $($body)*
            </svg>
        }
    };
}

/// The style row: the single writer of `CanvasState::style`.
#[component]
fn StylePanel() -> impl IntoView {
    let canvas = expect_context::<CanvasState>();
    let i18n = use_i18n();
    let style = canvas.style;

    let pick_color = move |slot: &str| {
        if let Ok(next) = style_with_color(&style.get_untracked(), slot) {
            style.set(next);
        }
    };

    view! {
        <div
            class="absolute bottom-16 left-1/2 -translate-x-1/2 flex items-center gap-1 \
                   px-2 py-1 rounded-xl border border-border bg-surface-raised shadow-lg"
            on:pointerdown=|ev: web_sys::PointerEvent| ev.stop_propagation()
            on:dblclick=|ev: web_sys::MouseEvent| ev.stop_propagation()
            on:wheel=|ev: web_sys::WheelEvent| ev.stop_propagation()
        >
            {PALETTE_SLOTS
                .iter()
                .map(|slot| {
                    let slot: &'static str = slot;
                    view! {
                        <button
                            class=move || {
                                let base = "w-5 h-5 rounded-full border-2 transition-colors";
                                if style.with(|s| slot_selected(s, slot)) {
                                    format!("{base} border-primary")
                                } else {
                                    format!("{base} border-transparent hover:border-border-strong")
                                }
                            }
                            style=format!("background: {};", palette_var(slot))
                            title=move || t_string!(i18n, canvas.style_color).to_string()
                            on:click=move |_| pick_color(slot)
                        ></button>
                    }
                })
                .collect_view()}
            <input
                type="color"
                class="w-6 h-6 p-0 border-0 bg-transparent cursor-pointer"
                title=move || t_string!(i18n, canvas.style_hex).to_string()
                prop:value=move || {
                    style.with(|s| {
                        if aleph_protocol::canvas::is_hex_color(&s.color) {
                            s.color.clone()
                        } else {
                            "#6e7781".to_string()
                        }
                    })
                }
                on:input=move |ev| pick_color(&event_target_value(&ev))
            />
            <div class="w-px h-5 mx-0.5 bg-border"></div>
            {[
                (SizeKind::Small, "S"),
                (SizeKind::Medium, "M"),
                (SizeKind::Large, "L"),
            ]
                .into_iter()
                .map(|(size, glyph)| {
                    view! {
                        <StyleToggle
                            active=Signal::derive(move || style.with(|s| s.size == size))
                            title=Signal::derive(move || t_string!(i18n, canvas.style_size).to_string())
                            on_pick=Callback::new(move |()| style.update(|s| s.size = size))
                        >
                            {glyph}
                        </StyleToggle>
                    }
                })
                .collect_view()}
            <div class="w-px h-5 mx-0.5 bg-border"></div>
            <StyleToggle
                active=Signal::derive(move || style.with(|s| s.fill))
                title=Signal::derive(move || t_string!(i18n, canvas.style_fill).to_string())
                on_pick=Callback::new(move |()| style.update(|s| s.fill = !s.fill))
            >
                {icon! { <rect x="4" y="4" width="16" height="16" rx="2" fill="currentColor" fill-opacity="0.35" /> }}
            </StyleToggle>
            <div class="w-px h-5 mx-0.5 bg-border"></div>
            {[
                (StrokeKind::Solid, "M3 12h18"),
                (StrokeKind::Sketch, "M3 13c3-3 5 2 8-1s5 2 10-1"),
                (StrokeKind::Dashed, "M3 12h4M10 12h4M17 12h4"),
                (StrokeKind::Dotted, "M4 12h.5M8 12h.5M12 12h.5M16 12h.5M20 12h.5"),
            ]
                .into_iter()
                .map(|(kind, d)| {
                    view! {
                        <StyleToggle
                            active=Signal::derive(move || style.with(|s| s.stroke == kind))
                            title=Signal::derive(move || t_string!(i18n, canvas.style_stroke).to_string())
                            on_pick=Callback::new(move |()| style.update(|s| s.stroke = kind))
                        >
                            {icon! { <path d=d /> }}
                        </StyleToggle>
                    }
                })
                .collect_view()}
        </div>
    }
}

#[component]
pub(super) fn CanvasToolbar() -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <StylePanel />
        <div
            class="absolute bottom-4 left-1/2 -translate-x-1/2 flex items-center gap-0.5 \
                   px-1.5 py-1 rounded-xl border border-border bg-surface-raised shadow-lg"
            on:pointerdown=|ev: web_sys::PointerEvent| ev.stop_propagation()
            on:dblclick=|ev: web_sys::MouseEvent| ev.stop_propagation()
            on:wheel=|ev: web_sys::WheelEvent| ev.stop_propagation()
        >
            <ToolButton
                tool=CanvasTool::Select
                title=Signal::derive(move || t_string!(i18n, canvas.tool_select).to_string())
            >
                {icon! { <path d="M3 3l7.1 17 2.5-7.4L20 10.1z" /> }}
            </ToolButton>
            <ToolButton
                tool=CanvasTool::Pan
                title=Signal::derive(move || t_string!(i18n, canvas.tool_pan).to_string())
            >
                {icon! {
                    <polyline points="5 9 2 12 5 15" />
                    <polyline points="9 5 12 2 15 5" />
                    <polyline points="15 19 12 22 9 19" />
                    <polyline points="19 9 22 12 19 15" />
                    <line x1="2" y1="12" x2="22" y2="12" />
                    <line x1="12" y1="2" x2="12" y2="22" />
                }}
            </ToolButton>
            <div class="w-px h-5 mx-0.5 bg-border"></div>
            <ToolButton
                tool=CanvasTool::Draw
                title=Signal::derive(move || t_string!(i18n, canvas.tool_draw).to_string())
            >
                {icon! {
                    <path d="M12 19l7-7 3 3-7 7-3-3z" />
                    <path d="M18 13l-1.5-7.5L2 2l3.5 14.5L13 18l5-5z" />
                    <path d="M2 2l7.586 7.586" />
                    <circle cx="11" cy="11" r="2" />
                }}
            </ToolButton>
            <ToolButton
                tool=CanvasTool::Geo(GeoForm::Rect)
                title=Signal::derive(move || t_string!(i18n, canvas.tool_rect).to_string())
            >
                {icon! { <rect x="4" y="5" width="16" height="14" rx="1.5" /> }}
            </ToolButton>
            <ToolButton
                tool=CanvasTool::Geo(GeoForm::Ellipse)
                title=Signal::derive(move || t_string!(i18n, canvas.tool_ellipse).to_string())
            >
                {icon! { <ellipse cx="12" cy="12" rx="9" ry="7" /> }}
            </ToolButton>
            <ToolButton
                tool=CanvasTool::Geo(GeoForm::Diamond)
                title=Signal::derive(move || t_string!(i18n, canvas.tool_diamond).to_string())
            >
                {icon! { <path d="M12 3l9 9-9 9-9-9z" /> }}
            </ToolButton>
            <ToolButton
                tool=CanvasTool::Geo(GeoForm::Triangle)
                title=Signal::derive(move || t_string!(i18n, canvas.tool_triangle).to_string())
            >
                {icon! { <path d="M12 4l9 16H3z" /> }}
            </ToolButton>
            <ToolButton
                tool=CanvasTool::Geo(GeoForm::Hexagon)
                title=Signal::derive(move || t_string!(i18n, canvas.tool_hexagon).to_string())
            >
                {icon! { <path d="M7 4h10l4 8-4 8H7l-4-8z" /> }}
            </ToolButton>
            <ToolButton
                tool=CanvasTool::Geo(GeoForm::Pill)
                title=Signal::derive(move || t_string!(i18n, canvas.tool_pill).to_string())
            >
                {icon! { <rect x="3" y="7" width="18" height="10" rx="5" /> }}
            </ToolButton>
            <ToolButton
                tool=CanvasTool::Note
                title=Signal::derive(move || t_string!(i18n, canvas.tool_note).to_string())
            >
                {icon! {
                    <path d="M4 4h16v10l-6 6H4z" />
                    <path d="M14 20v-6h6" />
                }}
            </ToolButton>
            <ToolButton
                tool=CanvasTool::Text
                title=Signal::derive(move || t_string!(i18n, canvas.tool_text).to_string())
            >
                {icon! {
                    <polyline points="4 7 4 4 20 4 20 7" />
                    <line x1="12" y1="4" x2="12" y2="20" />
                    <line x1="8" y1="20" x2="16" y2="20" />
                }}
            </ToolButton>
            <ToolButton
                tool=CanvasTool::Frame
                title=Signal::derive(move || t_string!(i18n, canvas.tool_frame).to_string())
            >
                {icon! {
                    <line x1="7" y1="2" x2="7" y2="22" />
                    <line x1="17" y1="2" x2="17" y2="22" />
                    <line x1="2" y1="7" x2="22" y2="7" />
                    <line x1="2" y1="17" x2="22" y2="17" />
                }}
            </ToolButton>
            <ToolButton
                tool=CanvasTool::Arrow
                title=Signal::derive(move || t_string!(i18n, canvas.tool_arrow).to_string())
            >
                {icon! {
                    <line x1="4" y1="20" x2="20" y2="4" />
                    <polyline points="11 4 20 4 20 13" />
                }}
            </ToolButton>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The panel's color writer goes through the contract's gate: every
    /// palette slot and a hex literal are accepted, anything else leaves
    /// the style untouched — and the rest of the style survives a color
    /// change.
    #[test]
    fn style_with_color_is_gated_by_the_contract_and_keeps_the_rest() {
        let base = ShapeStyle {
            color: "red".to_string(),
            fill: true,
            size: SizeKind::Large,
            stroke: StrokeKind::Dashed,
        };
        for slot in PALETTE_SLOTS {
            let next = style_with_color(&base, slot).expect(slot);
            assert_eq!(next.color, slot);
            assert!(next.fill && next.size == SizeKind::Large && next.stroke == StrokeKind::Dashed);
        }
        assert_eq!(style_with_color(&base, "#0A0b0C").unwrap().color, "#0A0b0C");
        for bad in ["Red", "#abc", "rgb(0,0,0)"] {
            assert!(style_with_color(&base, bad).is_err(), "{bad}");
        }
    }

    /// The empty wire default and `"default"` light the same swatch.
    #[test]
    fn the_default_swatch_covers_the_empty_wire_color() {
        assert!(slot_selected(&ShapeStyle::default(), "default"));
        assert!(!slot_selected(&ShapeStyle::default(), "red"));
        let red = ShapeStyle {
            color: "red".to_string(),
            ..ShapeStyle::default()
        };
        assert!(slot_selected(&red, "red") && !slot_selected(&red, "default"));
    }

    /// Every geo form the contract knows has a tool button — a form the
    /// model can write but no person can draw is the debt this panel
    /// repays. Source-level (the buttons are view markup), through the
    /// crate's one answer to "where does production code end".
    #[test]
    fn every_geo_form_has_a_tool_button() {
        let production: String = crate::i18n_census::production_lines(include_str!("toolbar.rs"))
            .into_iter()
            .map(|(_, line)| line)
            .collect::<Vec<_>>()
            .join("\n");
        for form in ["Rect", "Ellipse", "Diamond", "Triangle", "Hexagon", "Pill"] {
            assert!(
                production.contains(&format!("CanvasTool::Geo(GeoForm::{form})")),
                "no tool button for GeoForm::{form}"
            );
        }
        // …and the list above is the contract's, not a stale copy: adding a
        // variant makes this match non-exhaustive.
        let _ = |f: GeoForm| match f {
            GeoForm::Rect
            | GeoForm::Ellipse
            | GeoForm::Diamond
            | GeoForm::Triangle
            | GeoForm::Hexagon
            | GeoForm::Pill => {}
        };
    }
}
