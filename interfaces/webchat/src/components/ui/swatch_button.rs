//! Swatch picker chip — the shared "colour/material face + active ring"
//! primitive behind the appearance pickers (settings page + topbar popover).

use leptos::prelude::*;

/// A swatch chip button for appearance pickers: a colour/gradient face with
/// an active ring, an optional caption below, and `aria-pressed` toggle
/// semantics. The click handler receives the raw `MouseEvent` so popover
/// callers can feed the View-Transition reveal origin; settings callers
/// ignore it.
#[component]
#[must_use]
pub fn SwatchButton(
    /// CSS background of the chip face (any CSS color/gradient value).
    background: &'static str,
    /// Size/shape/hover classes for the face, e.g.
    /// "w-9 h-9 rounded-full transition-transform group-hover:scale-110".
    face: &'static str,
    /// `ring-offset-*` class matching the surface the picker sits on.
    ring_offset: &'static str,
    /// Tooltip + accessible name.
    ///
    /// Reactive, not a snapshot: every caller feeds this from an appearance
    /// enum's localised `label`, and a `&'static str` captured at render time
    /// would keep showing the language the popover was first opened in.
    #[prop(into)]
    title: Signal<String>,
    /// Caption below the chip; omit to render the chip alone.
    #[prop(optional, into)]
    label: Option<Signal<String>>,
    #[prop(into)] active: Signal<bool>,
    on_pick: impl Fn(web_sys::MouseEvent) + 'static,
) -> impl IntoView {
    view! {
        <button
            on:click=move |ev: web_sys::MouseEvent| on_pick(ev)
            title=title
            aria-pressed=move || active.get().to_string()
            class="flex flex-col items-center gap-1.5 group"
        >
            <span
                class=move || {
                    if active.get() {
                        format!("{face} ring-2 ring-offset-2 {ring_offset} ring-text-primary")
                    } else {
                        format!("{face} ring-1 ring-border")
                    }
                }
                style=format!("background: {background}")
            />
            {label.map(|l| view! { <span class="text-xs text-text-secondary">{l}</span> })}
        </button>
    }
}
