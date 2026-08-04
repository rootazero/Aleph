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
    ///
    /// Width alone. On a touch device use [`FormFactor::from_viewport`] instead:
    /// a rotated phone is still a phone, and this function cannot know that.
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

    /// Classify a viewport, honouring the product law that **an iPhone-class
    /// screen is never split** — it is always the full-bleed drill-down shell.
    ///
    /// Width alone breaks that law the moment the device is rotated: an iPhone
    /// in landscape is `844 × 390`, and 844 lands in the Tablet band, which
    /// brings back the desktop two-column shell (sidebar + main). Rotating the
    /// phone was enough to produce exactly the split layout that must never
    /// appear on it.
    ///
    /// The **short edge** is what separates the two iOS classes, and it is
    /// orientation-invariant: every iPhone is under 640 on its short edge, every
    /// iPad is at least 640 on both (iPad mini portrait is 744). So an iPhone
    /// stays Phone in both orientations and an iPad stays Tablet/Wide in both —
    /// no user-agent sniffing, no device table to maintain.
    ///
    /// Gated on `coarse_pointer` so this cannot touch the desktop: a mouse user
    /// who drags a window short-and-wide (e.g. `900 × 500`) keeps the
    /// width-only classification they have today. The gate is the primary
    /// pointer, so a trackpad laptop with a touchscreen still reads as fine.
    #[must_use]
    pub fn from_viewport(width: f64, height: f64, coarse_pointer: bool) -> Self {
        if coarse_pointer && width.min(height) < PHONE_MAX_PX {
            return FormFactor::Phone;
        }
        Self::from_width(width)
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
        let form_factor = RwSignal::new(measure_form_factor());
        // Keep in sync with resizes — which is also how a rotation arrives.
        // Fire-and-forget for the app lifetime — mirrors the shell-root
        // listeners in app.rs (handle not retained).
        window_event_listener(leptos::ev::resize, move |_| {
            let now = measure_form_factor();
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

/// Current window inner height, same fallback contract as [`measure_width`].
fn measure_height() -> f64 {
    web_sys::window()
        .and_then(|w| w.inner_height().ok())
        .and_then(|v| v.as_f64())
        .unwrap_or(1024.0)
}

/// Whether the primary pointer is coarse (finger / stylus) rather than a mouse.
/// Unreadable ⇒ `false`, which falls back to the width-only classification —
/// the conservative direction, since the extra rule only ever *adds* Phone.
fn has_coarse_pointer() -> bool {
    web_sys::window()
        .and_then(|w| w.match_media("(pointer: coarse)").ok().flatten())
        .is_some_and(|m| m.matches())
}

/// Read the live viewport and classify it. One place, so the boot read and the
/// resize listener can never disagree about which inputs matter.
fn measure_form_factor() -> FormFactor {
    FormFactor::from_viewport(measure_width(), measure_height(), has_coarse_pointer())
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

    /// The law: an iPhone-class screen is never split. Rotation must not change
    /// that, and width alone said otherwise — 844 landed in Tablet, which is the
    /// desktop two-column shell.
    #[test]
    fn a_rotated_phone_is_still_a_phone() {
        // iPhone 14/15 Pro, both orientations.
        assert_eq!(
            FormFactor::from_viewport(390.0, 844.0, true),
            FormFactor::Phone
        );
        assert_eq!(
            FormFactor::from_viewport(844.0, 390.0, true),
            FormFactor::Phone,
            "landscape iPhone fell into the Tablet band — the split shell is back"
        );
        // iPhone SE / mini and the largest Pro Max, landscape.
        assert_eq!(
            FormFactor::from_viewport(667.0, 375.0, true),
            FormFactor::Phone
        );
        assert_eq!(
            FormFactor::from_viewport(932.0, 430.0, true),
            FormFactor::Phone
        );
    }

    /// …and an iPad is NOT dragged into the phone shell by the same rule. Its
    /// short edge clears 640 in both orientations (iPad mini portrait is 744).
    #[test]
    fn an_ipad_stays_out_of_the_phone_band_in_both_orientations() {
        for (w, h) in [
            (744.0, 1133.0), // iPad mini portrait
            (1133.0, 744.0), // iPad mini landscape
            (820.0, 1180.0), // iPad Air portrait
            (1180.0, 820.0), // iPad Air landscape
        ] {
            assert_ne!(
                FormFactor::from_viewport(w, h, true),
                FormFactor::Phone,
                "{w}x{h} was pulled into the phone shell"
            );
        }
    }

    /// A mouse user is untouched: the short-edge rule is gated on a coarse
    /// pointer, so a short-and-wide desktop window classifies exactly as it does
    /// today. Without the gate this window would flip to the phone shell.
    #[test]
    fn a_short_wide_desktop_window_is_unaffected() {
        assert_eq!(
            FormFactor::from_viewport(900.0, 500.0, false),
            FormFactor::from_width(900.0)
        );
        assert_eq!(
            FormFactor::from_viewport(1440.0, 400.0, false),
            FormFactor::Wide
        );
        // The gate is what makes the difference — same window, coarse pointer.
        assert_eq!(
            FormFactor::from_viewport(900.0, 500.0, true),
            FormFactor::Phone
        );
    }

    /// With a fine pointer the new entry point is the old one, verbatim, across
    /// the whole band range. Proves the desktop path cannot have drifted.
    #[test]
    fn fine_pointer_matches_width_only_classification() {
        for w in [
            0.0, 320.0, 639.9, 640.0, 800.0, 1023.9, 1024.0, 1440.0, 2560.0,
        ] {
            for h in [300.0, 500.0, 900.0, 1440.0] {
                assert_eq!(
                    FormFactor::from_viewport(w, h, false),
                    FormFactor::from_width(w),
                    "fine-pointer {w}x{h} diverged from the width-only rule"
                );
            }
        }
    }
}
