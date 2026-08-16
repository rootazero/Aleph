//! Floating tool switcher for the whiteboard editor — the only writer of
//! `CanvasState::tool` besides the editor's own drop-back-to-Select after a
//! creation gesture.
//!
//! Not in the plan's Task-14 file list: the plan omitted a toolbar entirely,
//! which would have left every creation tool unreachable (a signal with no
//! writer — the "capability wired ≠ capability delivered" failure). Every
//! tool the editor implements gets a button — the full set since Task 15
//! brought Draw and Arrow.
//!
//! Sits inside the editor surface, after the overlay layer in DOM order (so
//! it paints on top), and stops pointer/wheel propagation — a click on a
//! tool must not start a marquee underneath it.

use aleph_protocol::canvas::GeoForm;
use leptos::prelude::*;

use crate::i18n::{t_string, use_i18n};
use crate::state::canvas::{CanvasState, CanvasTool};

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

#[component]
pub(super) fn CanvasToolbar() -> impl IntoView {
    let i18n = use_i18n();
    view! {
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
