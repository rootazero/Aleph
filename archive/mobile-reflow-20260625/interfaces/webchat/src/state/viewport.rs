//! Reactive viewport-size signal.
//!
//! Mobile reflow (Phase 0.5) is mostly CSS-driven via Tailwind `max-sm:`
//! utilities, but a few behaviors must branch in logic (e.g. the notification
//! bell opens a full-screen view on mobile vs. a popover on desktop). Those
//! read this `is_mobile` signal, kept in sync with window resizes. The CSS
//! breakpoint and this threshold share the same 640px line.
//!
//! This module also owns `drawer_open` — the mobile nav-drawer toggle. The
//! left sidebar (`ModeSidebar`) is a permanent column on desktop but slides
//! in as an overlay on mobile; the chat top-bar agent pill opens it, a
//! backdrop tap or a route change closes it.

use leptos::prelude::*;

/// Width (px) below which the panel uses its single-column mobile layout.
/// Matches Tailwind's `sm` breakpoint (`max-sm:` = `< 640px`).
pub const MOBILE_BREAKPOINT_PX: f64 = 640.0;

/// Reactive "is the viewport mobile-width" flag, provided at the shell root
/// via context. `Copy` (just a signal handle), so it threads freely.
#[derive(Clone, Copy)]
pub struct ViewportState {
    pub is_mobile: RwSignal<bool>,
    /// Mobile nav-drawer open state. Always `false` on desktop (the sidebar is
    /// a permanent column there); toggled only by mobile affordances.
    pub drawer_open: RwSignal<bool>,
}

impl ViewportState {
    #[must_use]
    pub fn new() -> Self {
        let is_mobile = RwSignal::new(measure_is_mobile());
        let drawer_open = RwSignal::new(false);
        // Keep in sync with window resizes. Fire-and-forget for the app
        // lifetime — mirrors the shell-root keydown listener in app.rs (the
        // handle is intentionally not retained).
        window_event_listener(leptos::ev::resize, move |_| {
            let now = measure_is_mobile();
            if is_mobile.get_untracked() != now {
                is_mobile.set(now);
                // Leaving mobile width must not strand an open drawer (it would
                // otherwise linger as a fixed overlay once the sidebar reverts
                // to a permanent column).
                if !now {
                    drawer_open.set(false);
                }
            }
        });
        Self {
            is_mobile,
            drawer_open,
        }
    }
}

impl Default for ViewportState {
    fn default() -> Self {
        Self::new()
    }
}

/// Read the current window inner width and compare to the breakpoint.
/// Falls back to desktop (`false`) when the width can't be read.
fn measure_is_mobile() -> bool {
    web_sys::window()
        .and_then(|w| w.inner_width().ok())
        .and_then(|v| v.as_f64())
        .is_some_and(|w| w < MOBILE_BREAKPOINT_PX)
}
