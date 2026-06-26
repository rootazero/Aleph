//! Reactive form-factor signal (Wide / Phone / Tablet).
//!
//! Phone (<640px) is currently the only factor that changes rendering — it
//! swaps the wide `/settings` page for the iOS-native `PhoneSettings` screen
//! (see `crate::platform::phone::settings`). Tablet is reserved for future
//! iPad screens and renders identically to Wide for now. The 640px line
//! matches Tailwind's `sm` breakpoint, so CSS and logic agree.

use leptos::prelude::*;

/// Upper bound (exclusive) of the Phone band. Matches Tailwind `sm`.
pub const PHONE_MAX_PX: f64 = 640.0;
/// Upper bound (exclusive) of the Tablet band.
pub const TABLET_MAX_PX: f64 = 1024.0;

/// Viewport class. Only `Phone` diverges in rendering today.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FormFactor {
    Wide,
    Phone,
    Tablet,
}

impl FormFactor {
    /// Classify a viewport width. `<640 → Phone`, `<1024 → Tablet`, else `Wide`.
    #[must_use]
    pub fn from_width(width: f64) -> Self {
        if width < PHONE_MAX_PX {
            FormFactor::Phone
        } else if width < TABLET_MAX_PX {
            FormFactor::Tablet
        } else {
            FormFactor::Wide
        }
    }
}

/// Reactive form-factor, provided at the shell root via context. `Copy` (just a
/// signal handle), so it threads freely into router closures.
#[derive(Clone, Copy)]
pub struct FormFactorState {
    pub form_factor: RwSignal<FormFactor>,
}

impl FormFactorState {
    #[must_use]
    pub fn new() -> Self {
        let form_factor = RwSignal::new(FormFactor::from_width(measure_width()));
        // Keep in sync with resizes. Fire-and-forget for the app lifetime —
        // mirrors the shell-root listeners in app.rs (handle not retained).
        window_event_listener(leptos::ev::resize, move |_| {
            let now = FormFactor::from_width(measure_width());
            if form_factor.get_untracked() != now {
                form_factor.set(now);
            }
        });
        Self { form_factor }
    }
}

impl Default for FormFactorState {
    fn default() -> Self {
        Self::new()
    }
}

/// Current window inner width; falls back to a wide width when unreadable
/// (e.g. during SSR / host-target tests where there is no `window`).
fn measure_width() -> f64 {
    web_sys::window()
        .and_then(|w| w.inner_width().ok())
        .and_then(|v| v.as_f64())
        .unwrap_or(1280.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_widths_at_band_boundaries() {
        assert_eq!(FormFactor::from_width(0.0), FormFactor::Phone);
        assert_eq!(FormFactor::from_width(639.9), FormFactor::Phone);
        assert_eq!(FormFactor::from_width(640.0), FormFactor::Tablet);
        assert_eq!(FormFactor::from_width(1023.9), FormFactor::Tablet);
        assert_eq!(FormFactor::from_width(1024.0), FormFactor::Wide);
        assert_eq!(FormFactor::from_width(1920.0), FormFactor::Wide);
    }
}
