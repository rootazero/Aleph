//! Full-pane image viewer.
//!
//! An image row opens here rather than in a new tab. In the desktop shell a new
//! tab is not a tab at all — `desktop/shell/src/external_link.rs` hands
//! `/artifact/` URLs to the OS browser — so leaving the app to glance at a
//! thumbnail is the wrong weight for the gesture. Files and exports still leave
//! (you want those in a real browser or on disk); images stay.
//!
//! The overlay covers the pane, not the window: it is `absolute inset-0` inside
//! the pane's `relative` root, so the chat column keeps streaming underneath.

use leptos::prelude::*;

/// What the viewer is showing. `None` means closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LightboxTarget {
    /// Capability URL of the full-size image.
    pub url: String,
    /// Display name, shown in the caption and used as the `alt` text.
    pub filename: String,
}

/// Image overlay. Renders nothing when `target` is `None`.
///
/// Closes on backdrop click, on the ✕ button, and on Escape. The Escape
/// listener is registered once for the component's lifetime and only acts when
/// the viewer is actually open, so it coexists with the other Escape handlers
/// in the app (the palette's and the sidebar's) the same way those coexist with
/// each other.
#[component]
pub(super) fn Lightbox(target: RwSignal<Option<LightboxTarget>>) -> impl IntoView {
    {
        use leptos::ev::keydown;
        window_event_listener(keydown, move |ev: web_sys::KeyboardEvent| {
            if ev.key() == "Escape" && target.get().is_some() {
                target.set(None);
            }
        });
    }

    move || {
        target.get().map(|t| {
            let alt = t.filename.clone();
            let caption = t.filename.clone();
            let href = t.url.clone();
            view! {
                <div
                    class="absolute inset-0 z-30 flex flex-col bg-surface-base/95 backdrop-blur-sm"
                    on:click=move |_| target.set(None)
                >
                    <div class="flex items-center gap-2 px-3 py-2 border-b border-border shrink-0">
                        <span class="min-w-0 flex-1 truncate text-xs">{caption}</span>
                        // Stop propagation so following the link does not also
                        // run the backdrop's close handler.
                        <a
                            href=href
                            target="_blank"
                            rel="noreferrer"
                            class="px-1.5 py-0.5 rounded text-[11px] text-text-secondary
                                   hover:text-text-primary shrink-0"
                            on:click=|ev: web_sys::MouseEvent| ev.stop_propagation()
                        >"↗"</a>
                        <button
                            class="px-1.5 py-0.5 rounded text-[11px] text-text-secondary
                                   hover:text-text-primary shrink-0"
                            on:click=move |ev: web_sys::MouseEvent| {
                                ev.stop_propagation();
                                target.set(None);
                            }
                        >"✕"</button>
                    </div>
                    <div class="flex-1 min-h-0 flex items-center justify-center p-3">
                        <img
                            src=t.url
                            alt=alt
                            class="max-w-full max-h-full object-contain rounded"
                        />
                    </div>
                </div>
            }
        })
    }
}
