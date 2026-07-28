//! Live pointer round-trip on Windows.
//!
//! The unit tests in `win_input` pin the normalization arithmetic; they cannot
//! see whether the resulting `SendInput` actually puts the pointer where it was
//! asked to. This does: move, read back with `GetCursorPos` (through the same
//! `cursor_position` the tool surface uses), compare, restore.
//!
//! That round trip is the whole contract. It is also the thing that was broken:
//! writes were normalized against the primary monitor while reads came back in
//! virtual-desktop coordinates, so on a multi-monitor desktop the two disagreed
//! and every click aimed at a secondary display landed on the primary one.
//!
//! `#[ignore]`d because it moves the user's mouse pointer and needs a real
//! session. Run it deliberately:
//!
//! ```text
//! cargo test -p aleph-desktop --test win_pointer_live -- --ignored --nocapture
//! ```

#![cfg(windows)]

use aleph_desktop::action::cursor_position;
use aleph_desktop::win_input::move_cursor_absolute;

#[test]
#[ignore = "moves the physical mouse pointer; needs an interactive session"]
fn the_pointer_lands_where_it_was_sent() {
    // Settling DPI awareness is what makes these numbers physical pixels rather
    // than the OS's virtualized ones — the same opt-in the platform constructor
    // performs before any coordinate is produced.
    let awareness = aleph_desktop::ensure_process_dpi_aware();
    let (origin_x, origin_y) = cursor_position().expect("cursor_position failed");
    println!("dpi awareness: {awareness:?}, cursor starts at ({origin_x}, {origin_y})");

    // Points spread across the desktop, including the exact corners — the
    // corners are where a normalization off by one row or column shows up.
    let probes: Vec<(i32, i32)> = {
        let (w, h) = screen_size();
        println!("virtual screen: {w}x{h} physical px");
        vec![
            (0, 0),
            (1, 1),
            (w / 2, h / 2),
            (w / 3, (h * 2) / 3),
            (w - 1, h - 1),
        ]
    };

    let mut failures = Vec::new();
    for (x, y) in probes {
        move_cursor_absolute(x, y).expect("move_cursor_absolute failed");
        // The injected move is queued; give the input thread a moment to apply
        // it before reading the position back.
        std::thread::sleep(std::time::Duration::from_millis(30));
        let (gx, gy) = cursor_position().expect("cursor_position failed");
        println!("sent ({x}, {y}) -> read ({gx}, {gy})");
        // One pixel of slack: the 0..=65535 grid cannot address every pixel of a
        // desktop wider than 65536, and rounding costs at most one step.
        if (gx - f64::from(x)).abs() > 1.0 || (gy - f64::from(y)).abs() > 1.0 {
            failures.push(((x, y), (gx, gy)));
        }
    }

    // Put the pointer back where the user left it.
    let _ = move_cursor_absolute(origin_x as i32, origin_y as i32);

    assert!(
        failures.is_empty(),
        "pointer did not land where it was sent: {failures:?}"
    );
}

/// The invisible border is real, and it is what the move/resize path now
/// compensates for.
///
/// The compensation arithmetic is unit-tested; what cannot be unit-tested is
/// whether DWM actually reports a border on this desktop. If it reported none,
/// the compensation would be a no-op and the read/write asymmetry it fixes would
/// be invisible in testing while still shifting real windows.
#[test]
#[ignore = "needs an interactive session with at least one ordinary window open"]
fn ordinary_windows_carry_an_invisible_border() {
    use windows::Win32::Foundation::HWND;

    aleph_desktop::ensure_process_dpi_aware();
    let windows = aleph_desktop::win_window::enumerate_top_level();
    assert!(!windows.is_empty(), "no top-level windows to measure");

    let mut padded = 0usize;
    for w in windows.iter().filter(|w| !w.title.is_empty()) {
        let hwnd = HWND(w.id as usize as *mut core::ffi::c_void);
        let pad = aleph_desktop::win_window::frame_padding(hwnd);
        println!("{:?} -> {pad:?}", w.title);
        // A sane border: never negative (the visible frame is inside the raw
        // rect), never absurd.
        for side in [pad.left, pad.top, pad.right, pad.bottom] {
            assert!(
                (0..=64).contains(&side),
                "implausible border {pad:?} on {:?}",
                w.title
            );
        }
        if pad.left != 0 || pad.right != 0 || pad.bottom != 0 {
            padded += 1;
        }
    }
    assert!(
        padded > 0,
        "no window reported an invisible border — the move/resize compensation \
         would be a silent no-op on this desktop"
    );
}

/// The virtual desktop's physical size, read the same way the input path does.
fn screen_size() -> (i32, i32) {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
    };
    // SAFETY: documented, side-effect-free metric reads.
    unsafe {
        (
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    }
}
