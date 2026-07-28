//! Windows process DPI awareness — the one setting that decides what every
//! desktop coordinate on this platform *means*.
//!
//! # Why this exists
//!
//! Aleph's declared native space is "physical pixels, top-left origin"
//! ([`crate::CoordinateSpace::Pixel`]). On Windows, whether a process actually
//! speaks that space is not a property of the API you call — it is a property of
//! the **process**. A DPI-unaware process is lied to by the OS: `GetWindowRect`,
//! `GetCursorPos`, `SendInput`'s absolute space, and UI Automation's
//! `CurrentBoundingRectangle` all come back *virtualized* (divided by the
//! monitor's DPI scale), while a screen capture — which goes through the display
//! driver, not the compatibility shim — comes back in real pixels.
//!
//! On the 150 %-scaled laptop panel that is the Windows default, that is a
//! 1.5× mismatch between "where the model saw the button in the screenshot" and
//! "where a click lands". Nothing in the stack can repair it after the fact,
//! because the two numbers are equally plausible.
//!
//! A Rust binary ships no application manifest by default, so `aleph-server` is
//! DPI-unaware unless it says otherwise. This module is where it says otherwise.
//!
//! # Contract
//!
//! [`ensure_process_dpi_aware`] is called once, from the single per-OS
//! construction point (`WindowsPlatform::new`), before any window or DC exists.
//! It climbs the ladder Per-Monitor-V2 → Per-Monitor → System, then **reads the
//! achieved level back from the OS** rather than trusting the call: the setting
//! is process-wide and one-shot, so an embedder (or a future manifest) may have
//! already pinned it, and reporting an intent as a fact is exactly the class of
//! lie this module exists to prevent.
//!
//! [`effective_scale`] then answers the only question the rest of the stack
//! actually asks: *how many physical pixels are there per unit of the coordinate
//! space this process's input / AX / window APIs speak?* Under per-monitor
//! awareness that is `1.0` — geometry and pixels are the same numbers — and the
//! normalized-coordinate viewport, the Set-of-Marks overlay and the window-space
//! mapping all become correct simultaneously. When the opt-in did not take, it
//! is the monitor's DPI scale, which is the pre-existing behavior.

/// How much of the physical pixel grid this process is allowed to see.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DpiAwareness {
    /// The OS virtualizes coordinates: geometry is reported divided by the
    /// monitor's DPI scale while captures stay at full resolution.
    Unaware,
    /// Real pixels on monitors running at the system DPI; virtualized elsewhere.
    System,
    /// Real pixels on every monitor. The only level at which "physical pixels,
    /// top-left origin" is true across a mixed-DPI desktop.
    PerMonitor,
}

impl DpiAwareness {
    /// Whether geometry this process reads is already in physical pixels
    /// everywhere — i.e. whether points and pixels are the same number.
    #[must_use]
    pub const fn is_physical_everywhere(self) -> bool {
        matches!(self, Self::PerMonitor)
    }
}

/// Physical pixels per unit of the coordinate space this process speaks, on a
/// monitor whose DPI scale is `monitor_scale`.
///
/// Pure so the decision is testable on any host — the FFI below only supplies
/// the `awareness` argument.
///
/// `monitor_scale` is the display's DPI ratio as the OS reports it (1.5 at
/// 150 %). It is deliberately *ignored* under [`DpiAwareness::PerMonitor`]:
/// there, the process already reads physical pixels, so multiplying by the DPI
/// ratio a second time is the double-scale bug this function exists to kill.
/// Non-finite or non-positive input degrades to `1.0` rather than poisoning
/// every downstream coordinate with a NaN.
#[must_use]
pub fn effective_scale(awareness: DpiAwareness, monitor_scale: f64) -> f64 {
    if awareness.is_physical_everywhere() {
        return 1.0;
    }
    if monitor_scale.is_finite() && monitor_scale > 0.0 {
        monitor_scale
    } else {
        1.0
    }
}

// ── Windows FFI ──────────────────────────────────────────────────────────────

#[cfg(windows)]
mod imp {
    use super::DpiAwareness;
    use std::sync::OnceLock;
    use windows::Win32::UI::HiDpi::{
        GetAwarenessFromDpiAwarenessContext, GetThreadDpiAwarenessContext,
        SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE,
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, DPI_AWARENESS_CONTEXT_SYSTEM_AWARE,
        DPI_AWARENESS_PER_MONITOR_AWARE, DPI_AWARENESS_SYSTEM_AWARE,
    };

    /// Latched result of the one-shot opt-in. The Win32 setting is process-wide
    /// and cannot be changed twice, so re-running the ladder would only produce
    /// noise in the log.
    static ACHIEVED: OnceLock<DpiAwareness> = OnceLock::new();

    /// Read the level the OS actually has for this process.
    ///
    /// Uses the *thread* context because that is what every subsequent Win32
    /// call in this process resolves against; with no per-thread override in
    /// play (Aleph never sets one) it is the process default.
    pub(super) fn current() -> DpiAwareness {
        // SAFETY: both calls are documented, side-effect-free readers of the
        // calling thread's DPI context.
        let awareness = unsafe {
            let ctx = GetThreadDpiAwarenessContext();
            GetAwarenessFromDpiAwarenessContext(ctx)
        };
        if awareness == DPI_AWARENESS_PER_MONITOR_AWARE {
            DpiAwareness::PerMonitor
        } else if awareness == DPI_AWARENESS_SYSTEM_AWARE {
            DpiAwareness::System
        } else {
            // Covers DPI_AWARENESS_UNAWARE and DPI_AWARENESS_INVALID. An
            // invalid context is not knowledge that we are aware, so it lands
            // on the conservative side.
            DpiAwareness::Unaware
        }
    }

    pub(super) fn ensure() -> DpiAwareness {
        *ACHIEVED.get_or_init(|| {
            for ctx in [
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE,
                DPI_AWARENESS_CONTEXT_SYSTEM_AWARE,
            ] {
                // SAFETY: documented process-wide setter. It fails when the
                // level is already pinned (by a manifest or an embedder) or the
                // context is unsupported on this build — both are handled by
                // reading the real level back below rather than by trusting the
                // return value.
                if unsafe { SetProcessDpiAwarenessContext(ctx) }.is_ok() {
                    break;
                }
            }

            let achieved = current();
            match achieved {
                DpiAwareness::PerMonitor => tracing::debug!(
                    "desktop: process is per-monitor DPI aware; screen geometry is physical pixels"
                ),
                DpiAwareness::System => tracing::warn!(
                    "desktop: process is only system-DPI aware; coordinates on monitors whose \
                     scale differs from the system DPI will be off by that ratio"
                ),
                DpiAwareness::Unaware => tracing::warn!(
                    "desktop: process is DPI-unaware and the opt-in did not take; window and \
                     accessibility geometry is virtualized while screen captures are not, so \
                     clicks derived from a screenshot will be off by the display scale"
                ),
            }
            achieved
        })
    }
}

#[cfg(not(windows))]
mod imp {
    use super::DpiAwareness;

    /// Off-Windows there is no process-wide virtualization to opt out of, and
    /// no caller: the two public entry points are `cfg(windows)`-gated at every
    /// call site. `PerMonitor` is the value that means "geometry is already
    /// physical", which is the correct non-Windows answer had anyone asked.
    pub(super) const fn current() -> DpiAwareness {
        DpiAwareness::PerMonitor
    }

    pub(super) const fn ensure() -> DpiAwareness {
        DpiAwareness::PerMonitor
    }
}

/// Opt this process into the highest DPI awareness the OS will grant, once.
///
/// Idempotent and cheap after the first call. Returns the level the OS reports
/// afterwards — which may be lower than requested if something already pinned
/// it, and is never merely the level we asked for.
pub fn ensure_process_dpi_aware() -> DpiAwareness {
    imp::ensure()
}

/// The DPI awareness the OS currently reports for this process.
#[must_use]
pub fn process_dpi_awareness() -> DpiAwareness {
    imp::current()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_monitor_awareness_collapses_the_scale_to_one() {
        // The whole point: once geometry is physical, multiplying by the display
        // DPI ratio a second time is the bug, not the fix.
        assert!((effective_scale(DpiAwareness::PerMonitor, 1.5) - 1.0).abs() < f64::EPSILON);
        assert!((effective_scale(DpiAwareness::PerMonitor, 3.0) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn unaware_process_keeps_the_monitor_scale() {
        // Legacy behavior is preserved exactly when the opt-in did not take.
        assert!((effective_scale(DpiAwareness::Unaware, 1.5) - 1.5).abs() < f64::EPSILON);
        assert!((effective_scale(DpiAwareness::System, 2.0) - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn degenerate_monitor_scale_degrades_to_identity() {
        // A NaN here would propagate into every click coordinate.
        for bad in [f64::NAN, f64::INFINITY, 0.0, -1.0] {
            assert!((effective_scale(DpiAwareness::Unaware, bad) - 1.0).abs() < f64::EPSILON);
        }
    }

    #[test]
    fn only_per_monitor_claims_physical_everywhere() {
        assert!(DpiAwareness::PerMonitor.is_physical_everywhere());
        assert!(!DpiAwareness::System.is_physical_everywhere());
        assert!(!DpiAwareness::Unaware.is_physical_everywhere());
    }

    #[cfg(windows)]
    #[test]
    fn opt_in_is_idempotent_and_reports_the_real_level() {
        let first = ensure_process_dpi_aware();
        let second = ensure_process_dpi_aware();
        assert_eq!(first, second, "the latched level must not drift");
        // Whatever the CI host granted, the live query must agree with the
        // latched value — that agreement is the honesty property.
        assert_eq!(first, process_dpi_awareness());
    }
}
