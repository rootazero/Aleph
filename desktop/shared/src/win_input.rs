//! Windows absolute pointer positioning — where a global screen coordinate
//! becomes a physical mouse position.
//!
//! # Why this exists
//!
//! `SendInput`'s absolute mouse space is normalized to `0..=65535`, and *which
//! rectangle* those 65536 steps span is decided by one flag. Without
//! `MOUSEEVENTF_VIRTUALDESK` the span is the **primary monitor**; with it, the
//! whole virtual desktop.
//!
//! enigo's Windows backend normalizes against `SM_CXSCREEN` / `SM_CYSCREEN` and
//! never sets the flag — its source carries the comment `// TODO: Check if we
//! should use MOUSEEVENTF_VIRTUALDESK too`. The consequence is not a rounding
//! difference. On any multi-monitor desktop:
//!
//! * a point on a monitor to the **right** of the primary normalizes past 65535
//!   and the OS clamps it to the primary's right edge;
//! * a point on a monitor to the **left** or **above** has a negative global
//!   coordinate, normalizes negative, and is clamped to the primary's top-left.
//!
//! So every click, drag and hover aimed at a secondary display landed on the
//! primary one — while `window_list`, `display_list` and `screenshot
//! {display_id}` happily reported and captured those very displays, and
//! `cursor_position` (`GetCursorPos`) read back true virtual-desktop
//! coordinates. Read and write disagreed about what a coordinate meant, which is
//! the same class of bug the DPI opt-in in [`crate::win_dpi`] fixed one layer
//! down.
//!
//! # Contract
//!
//! [`move_cursor_absolute`] takes a point in the **global screen pixel space** —
//! the space `window_list` bounds, `display_list` origins and `GetCursorPos` all
//! speak — and puts the pointer there. Button presses stay with enigo: they act
//! at the current pointer position and need no coordinate at all.
//!
//! Points outside the virtual desktop are clamped to it rather than rejected:
//! that is what the OS already does with an out-of-range normalized value, and
//! an off-by-one from bounds arithmetic (`x == width`) is far more common than a
//! genuinely nonsensical target.

/// The rectangle `MOUSEEVENTF_VIRTUALDESK` normalizes against: the bounding box
/// of every display, in global screen pixels. Its origin is negative when a
/// display sits left of or above the primary one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VirtualScreen {
    pub origin_x: i32,
    pub origin_y: i32,
    pub width: i32,
    pub height: i32,
}

/// Map one axis of a global coordinate into `SendInput`'s `0..=65535` space.
///
/// Pure, so the arithmetic is testable on any host — the FFI below only supplies
/// the metrics.
///
/// The formula is deliberately the one enigo already used
/// (`(v * 65535 + span/2) / span` over `span = extent - 1`), so on a
/// single-monitor desktop — where the virtual screen *is* the primary monitor
/// and the origin is zero — this produces byte-identical output to the previous
/// behavior. The change is the origin translation and the virtual-desktop
/// extent, not the rounding.
#[must_use]
pub fn normalize_axis(value: i32, origin: i32, extent: i32) -> i32 {
    let span = i64::from(extent) - 1;
    if span <= 0 {
        // A degenerate extent (metrics unavailable) has one addressable point.
        return 0;
    }
    let local = i64::from(value)
        .saturating_sub(i64::from(origin))
        .clamp(0, span);
    ((local * 65_535 + span / 2) / span) as i32
}

/// Map a global screen point into `SendInput`'s absolute space.
#[must_use]
pub fn normalize_absolute(x: i32, y: i32, screen: &VirtualScreen) -> (i32, i32) {
    (
        normalize_axis(x, screen.origin_x, screen.width),
        normalize_axis(y, screen.origin_y, screen.height),
    )
}

/// Move the pointer to a global screen pixel coordinate.
///
/// # Errors
///
/// [`crate::DesktopError::InputFailed`] when the OS refuses the injection —
/// which on Windows means UIPI blocked it, i.e. a process at a higher integrity
/// level owns the foreground window.
#[cfg(windows)]
pub fn move_cursor_absolute(x: i32, y: i32) -> crate::Result<()> {
    imp::move_cursor_absolute(x, y)
}

#[cfg(windows)]
mod imp {
    use super::{normalize_absolute, VirtualScreen};
    use crate::{DesktopError, Result};

    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_MOVE,
        MOUSEEVENTF_VIRTUALDESK, MOUSEINPUT,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, SM_CXSCREEN, SM_CXVIRTUALSCREEN, SM_CYSCREEN, SM_CYVIRTUALSCREEN,
        SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
    };

    /// Read the virtual desktop's bounding box.
    ///
    /// Falls back to the primary monitor when the virtual metrics come back
    /// empty — an answer of zero is "not told", and a zero-width span would send
    /// every pointer move to the top-left corner.
    fn virtual_screen() -> VirtualScreen {
        // SAFETY: `GetSystemMetrics` is a documented, side-effect-free reader.
        let (ox, oy, w, h) = unsafe {
            (
                GetSystemMetrics(SM_XVIRTUALSCREEN),
                GetSystemMetrics(SM_YVIRTUALSCREEN),
                GetSystemMetrics(SM_CXVIRTUALSCREEN),
                GetSystemMetrics(SM_CYVIRTUALSCREEN),
            )
        };
        if w > 0 && h > 0 {
            return VirtualScreen {
                origin_x: ox,
                origin_y: oy,
                width: w,
                height: h,
            };
        }
        // SAFETY: same documented reader.
        let (pw, ph) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
        VirtualScreen {
            origin_x: 0,
            origin_y: 0,
            width: pw,
            height: ph,
        }
    }

    pub(super) fn move_cursor_absolute(x: i32, y: i32) -> Result<()> {
        let screen = virtual_screen();
        let (dx, dy) = normalize_absolute(x, y, &screen);

        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx,
                    dy,
                    mouseData: 0,
                    // VIRTUALDESK is the whole point: it says the normalized
                    // coordinates span every display, not just the primary one.
                    dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };

        // SAFETY: one fully-initialized `INPUT` of the declared size; `SendInput`
        // copies it and returns the number of events it accepted.
        let sent = unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) };
        if sent == 1 {
            Ok(())
        } else {
            Err(DesktopError::InputFailed(format!(
                "Windows refused the pointer move to ({x}, {y}): SendInput accepted 0 events. \
                 This is UIPI — a process running at a higher integrity level (an installer, or \
                 anything started as administrator) owns the foreground window and blocks \
                 synthetic input. Address the app through set_value / ax_action instead, or run \
                 Aleph elevated."
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The virtual screen of a single 1920×1080 monitor.
    fn single() -> VirtualScreen {
        VirtualScreen {
            origin_x: 0,
            origin_y: 0,
            width: 1920,
            height: 1080,
        }
    }

    /// Primary 1920×1080 with a second 1920×1080 monitor to its *left*, which is
    /// where the negative global coordinates come from.
    fn dual_left() -> VirtualScreen {
        VirtualScreen {
            origin_x: -1920,
            origin_y: 0,
            width: 3840,
            height: 1080,
        }
    }

    #[test]
    fn corners_map_to_the_ends_of_the_normalized_range() {
        let vs = single();
        assert_eq!(normalize_absolute(0, 0, &vs), (0, 0));
        assert_eq!(normalize_absolute(1919, 1079, &vs), (65_535, 65_535));
    }

    #[test]
    fn the_single_monitor_formula_is_unchanged_from_enigo() {
        // The regression guard: enigo computed `(v * 65535 + w/2) / w` with
        // `w = SM_CXSCREEN - 1`. On a single monitor the virtual screen is the
        // primary monitor, so this path must produce the same numbers it always
        // did — the fix is the origin and the extent, not the rounding.
        let vs = single();
        for v in [0, 1, 17, 960, 1234, 1919] {
            let w = i64::from(vs.width) - 1;
            let enigo = ((i64::from(v) * 65_535 + w / 2) / w) as i32;
            assert_eq!(
                normalize_axis(v, 0, vs.width),
                enigo,
                "x={v} must match the previous behavior"
            );
        }
    }

    #[test]
    fn a_point_on_the_second_monitor_is_not_clamped_to_the_primary() {
        // The bug this module exists for: with the primary-only span, x=2500
        // normalized past 65535 and the OS pinned it to the primary's right
        // edge. Over the virtual desktop it lands where it belongs.
        let dual_right = VirtualScreen {
            origin_x: 0,
            origin_y: 0,
            width: 3840,
            height: 1080,
        };
        let (nx, _) = normalize_absolute(2500, 500, &dual_right);
        assert!(nx < 65_535, "must not saturate: got {nx}");
        // 2500 / 3839 ≈ 0.651
        let expected = ((2500i64 * 65_535 + 3839 / 2) / 3839) as i32;
        assert_eq!(nx, expected);
        // …and the same point under the old primary-only span *did* saturate.
        assert_eq!(normalize_axis(2500, 0, 1920), 65_535);
    }

    #[test]
    fn a_negative_global_x_addresses_the_monitor_left_of_primary() {
        // A display placed left of the primary has negative global coordinates.
        // Under the primary-only span those normalized negative; here the origin
        // translation puts the left monitor's left edge at 0.
        let vs = dual_left();
        assert_eq!(normalize_axis(-1920, vs.origin_x, vs.width), 0);
        // The primary's own origin sits halfway across the virtual desktop.
        let mid = normalize_axis(0, vs.origin_x, vs.width);
        assert!(
            (32_000..34_000).contains(&mid),
            "primary origin should land mid-range, got {mid}"
        );
    }

    #[test]
    fn out_of_range_points_clamp_instead_of_wrapping() {
        let vs = single();
        // The classic off-by-one: x == width, from `origin + w` arithmetic.
        assert_eq!(normalize_axis(1920, 0, vs.width), 65_535);
        assert_eq!(normalize_axis(99_999, 0, vs.width), 65_535);
        assert_eq!(normalize_axis(-5, 0, vs.width), 0);
        assert_eq!(normalize_axis(i32::MIN, 0, vs.width), 0);
        assert_eq!(normalize_axis(i32::MAX, i32::MIN, vs.width), 65_535);
    }

    #[test]
    fn a_degenerate_extent_never_divides_by_zero() {
        for extent in [0, 1, -3] {
            assert_eq!(normalize_axis(500, 0, extent), 0);
        }
    }
}
