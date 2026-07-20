# macOS Review-Follow-Up Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the four live macOS review findings — SCK region crop (#6), recording-completion verification, true per-window focus/bounds targeting, and typed-error preservation on the Swift-bridge media rail.

**Architecture:** Structural fixes over patches. The recorder honors `config.region` via `SCStreamConfiguration::setSourceRect` and verifies its output file. Window ops resolve the exact `CGWindowID` to its `AXUIElement` by **public-API geometry match** (a new `window_ax` module) with a fallback to today's app-activate / osascript behavior. The media rail stops re-flattening the already-typed `DesktopError` the bridge returns.

**Tech Stack:** Rust; macOS crates `aleph-desktop` (`desktop/shared`) and `aleph-desktop-macos` (`desktop/macos`); `objc2-screen-capture-kit 0.3`, `objc2-core-foundation 0.3` (A only), `core-foundation 0.10` + `core-graphics 0.25` (C), raw `extern "C"` to `ApplicationServices`/`CoreFoundation` (C).

## Global Constraints

- **Branch:** all work directly on `main` (single-branch dev mode). One commit per task.
- **Cargo economy (CLAUDE.md 极度节制):** run only **scoped** `cargo check -p <crate>` / `cargo test -p <crate> <filter> --lib` at task boundaries. No full-workspace runs.
- **Redlines:** R1 — direct platform FFI is allowed only inside `desktop/*` (the limb crates). R3 — introduce no new heavy dependency (A's `objc2-core-foundation` is already in the lock tree via SCK; C reuses existing `core-foundation`/`core-graphics`). R2 — no business UI in a bridge. R7/P8 — no semantic pattern-matching; geometry compare only.
- **Coordinate spaces:** SCK `sourceRect`, `SCDisplay.width/height`, AX position/size, and `CGWindowList` bounds are all **top-left-origin points**; SCK `setWidth/Height` are **pixels** (= points × scale). **`ScreenRegion` is in physical pixels** (`lib.rs:58`, and `coord_resolve.rs::resolve_viewport` emits `dim × scale` "because those are what every downstream native bridge expects"; the Linux/Windows sibling recorders `build_x11grab_args`/`build_gdigrab_args` pass region values as raw pixels). **Correction (Task 1 review):** an earlier draft of this line said "points" — that was wrong; the SCK region crop must convert `region` pixels → points (`÷ scale`) for `sourceRect` and use the region pixels **as-is** for `setWidth/Height`. Fixed in Task 1 below.
- **File size:** `window.rs` is already 841 lines — C's AX FFI goes in a **new** `window_ax.rs`, not into `window.rs` (P2 / CLAUDE.md 500-line rule).
- **Commit messages:** English, `<scope>: <description>`.
- **Deferred (NOT in this plan, see spec §范围外):** #7 Linux `webview_perms` audio-only + origin gate — implement on Linux. C's private-API (`_AXUIElementGetWindow`) exactness upgrade — not used; public geometry match only.

---

### Task 1: Crop macOS SCK recording to the requested region (#6 / Group A)

**Files:**
- Modify: `desktop/shared/Cargo.toml` (add `objc2-core-foundation` to the `cfg(target_os = "macos")` deps)
- Modify: `desktop/shared/src/perception/screen_record.rs` (add pure `sck_region_rect` + `SckRegionRect`; wire the region branch into `sc_recording_output_record`, ~lines 237–252)
- Test: `desktop/shared/src/perception/screen_record.rs` (append to the existing `#[cfg(test)] mod tests`)

**Interfaces:**
- Produces: `fn sck_region_rect(region: &crate::ScreenRegion, display_w: u32, display_h: u32, scale: u32) -> Option<SckRegionRect>` where `struct SckRegionRect { x: f64, y: f64, w: f64, h: f64, out_w: usize, out_h: usize }`. `None` = region does not intersect the display.

- [ ] **Step 1: Add the dependency**

In `desktop/shared/Cargo.toml`, under `[target.'cfg(target_os = "macos")'.dependencies]` (after the `objc2-core-media` line), add:

```toml
objc2-core-foundation = { version = "0.3", features = ["objc2"] }
```

- [ ] **Step 2: Write the failing test**

Append to the existing `#[cfg(test)] mod tests` in `screen_record.rs` (it already has `use super::*;` and `ScreenRegion` region tests):

```rust
    #[test]
    fn sck_region_rect_within_bounds_passes_through() {
        let r = crate::ScreenRegion { x: 10, y: 20, width: 100, height: 50 };
        assert_eq!(
            super::sck_region_rect(&r, 1000, 800, 2),
            Some(super::SckRegionRect { x: 10.0, y: 20.0, w: 100.0, h: 50.0, out_w: 200, out_h: 100 })
        );
    }

    #[test]
    fn sck_region_rect_clamps_overflow_to_display() {
        // Far edge exceeds the display: width/height clamp to what remains.
        let r = crate::ScreenRegion { x: 900, y: 700, width: 300, height: 300 };
        assert_eq!(
            super::sck_region_rect(&r, 1000, 800, 2),
            Some(super::SckRegionRect { x: 900.0, y: 700.0, w: 100.0, h: 100.0, out_w: 200, out_h: 200 })
        );
    }

    #[test]
    fn sck_region_rect_origin_outside_is_none() {
        let r = crate::ScreenRegion { x: 1000, y: 0, width: 10, height: 10 };
        assert_eq!(super::sck_region_rect(&r, 1000, 800, 2), None);
    }

    #[test]
    fn sck_region_rect_zero_size_is_none() {
        let r = crate::ScreenRegion { x: 10, y: 10, width: 0, height: 50 };
        assert_eq!(super::sck_region_rect(&r, 1000, 800, 2), None);
    }
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p aleph-desktop sck_region_rect --lib`
Expected: FAIL — `sck_region_rect` / `SckRegionRect` not defined.

- [ ] **Step 4: Implement the pure helper**

Add near the top of `screen_record.rs` (after the `use` block, before `screen_record`):

```rust
/// SCK source rect (display points) + output pixel dims for a recording region.
#[cfg(any(target_os = "macos", test))]
#[derive(Debug, PartialEq)]
struct SckRegionRect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    out_w: usize,
    out_h: usize,
}

/// Clamp `region` (top-left points) to the `display_w`×`display_h` display and
/// return the SCK `sourceRect` (points) plus the output pixel dimensions
/// (clamped size × `scale`). `None` when the region's origin is off-display or
/// the clamped size is zero — the caller maps that to a `ScreenCapture` error.
/// Pure so it is unit-testable without a display (mirrors `build_x11grab_args`).
#[cfg(any(target_os = "macos", test))]
fn sck_region_rect(
    region: &crate::ScreenRegion,
    display_w: u32,
    display_h: u32,
    scale: u32,
) -> Option<SckRegionRect> {
    if region.x >= display_w || region.y >= display_h {
        return None; // origin past the display — no intersection
    }
    let w = region.width.min(display_w - region.x);
    let h = region.height.min(display_h - region.y);
    if w == 0 || h == 0 {
        return None;
    }
    Some(SckRegionRect {
        x: f64::from(region.x),
        y: f64::from(region.y),
        w: f64::from(w),
        h: f64::from(h),
        out_w: (w * scale) as usize,
        out_h: (h * scale) as usize,
    })
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p aleph-desktop sck_region_rect --lib`
Expected: PASS (all four cases).

- [ ] **Step 6: Wire the region branch into the recorder**

In `sc_recording_output_record`, replace the scale + dimension block (currently ~lines 237–252, from `let scale: usize = 2;` through the `stream_config.setCapturesAudio(config.with_audio);` `unsafe` block) with:

```rust
    let scale: u32 = 2;
    // Honor a requested sub-region: crop via `setSourceRect` (display points)
    // and size the output to the region (pixels = points × scale). No region →
    // whole display, exactly as before.
    let (out_w, out_h) = match config.region.as_ref() {
        None => (display_width * scale as usize, display_height * scale as usize),
        Some(region) => {
            let rect = sck_region_rect(region, display_width as u32, display_height as u32, scale)
                .ok_or_else(|| {
                    DesktopError::ScreenCapture(format!(
                        "region {}x{}+{},{} does not intersect the {display_width}x{display_height} display",
                        region.width, region.height, region.x, region.y
                    ))
                })?;
            use objc2_core_foundation::{CGPoint, CGRect, CGSize};
            // SAFETY: `stream_config` is a freshly allocated mutable configuration;
            // `setSourceRect` takes a by-value `CGRect` in display points.
            unsafe {
                stream_config.setSourceRect(CGRect::new(
                    CGPoint::new(rect.x, rect.y),
                    CGSize::new(rect.w, rect.h),
                ));
            }
            (rect.out_w, rect.out_h)
        }
    };

    // SAFETY: `stream_config` is a freshly allocated mutable configuration;
    // these setters are the documented way to populate it.
    unsafe {
        stream_config.setWidth(out_w);
        stream_config.setHeight(out_h);
        stream_config.setMinimumFrameInterval(CMTime {
            value: 1,
            timescale: config.fps as i32,
            flags: CMTimeFlags::Valid,
            epoch: 0,
        });
        stream_config.setShowsCursor(true);
        stream_config.setCapturesAudio(config.with_audio);
    }
```

- [ ] **Step 7: Compile-check**

Run: `cargo check -p aleph-desktop`
Expected: PASS. If `setSourceRect` or the `CGRect` form mismatches this `objc2-screen-capture-kit 0.3` build, adjust to the compiler's suggested method/type form — the invariant is "crop to `region` points, output `region × scale` pixels".

- [ ] **Step 8: Commit**

```bash
git add desktop/shared/Cargo.toml desktop/shared/src/perception/screen_record.rs
git commit -m "desktop: crop macOS screen recording to the requested region (setSourceRect)"
```

---

### Task 2: Verify the macOS recording produced output (Group B)

**Files:**
- Modify: `desktop/shared/src/perception/screen_record.rs` (add pure `verify_recording_output`; wire it + the `timed_out` flag into `sc_recording_output_record`, ~lines 366–392)
- Test: `desktop/shared/src/perception/screen_record.rs` (append to `#[cfg(test)] mod tests`)

**Interfaces:**
- Produces: `fn verify_recording_output(path: &std::path::Path, timed_out: bool) -> Result<()>`.

- [ ] **Step 1: Write the failing test**

Append to `#[cfg(test)] mod tests` in `screen_record.rs`:

```rust
    #[test]
    fn verify_recording_output_ok_for_nonempty_file() {
        let dir = std::env::temp_dir().join(format!("aleph_rec_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("ok.mp4");
        std::fs::write(&f, b"data").unwrap();
        assert!(super::verify_recording_output(&f, false).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn verify_recording_output_err_on_timeout() {
        let f = std::path::Path::new("/nonexistent/whatever.mp4");
        assert!(super::verify_recording_output(f, true).is_err());
    }

    #[test]
    fn verify_recording_output_err_on_missing_or_empty() {
        assert!(super::verify_recording_output(std::path::Path::new("/no/such.mp4"), false).is_err());
        let dir = std::env::temp_dir().join(format!("aleph_rec_empty_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("empty.mp4");
        std::fs::write(&f, b"").unwrap();
        assert!(super::verify_recording_output(&f, false).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p aleph-desktop verify_recording_output --lib`
Expected: FAIL — `verify_recording_output` not defined.

- [ ] **Step 3: Implement the helper**

Add near `sck_region_rect` in `screen_record.rs`:

```rust
/// Confirm a recording actually produced a non-empty file. `timed_out` is the
/// delegate-wait timeout flag. A timeout, a missing file, or a zero-byte file
/// is a failure — the `SCRecordingOutput` path previously returned `Ok` in all
/// three cases (false success). Pure over the filesystem so it is unit-testable.
#[cfg(any(target_os = "macos", test))]
fn verify_recording_output(path: &std::path::Path, timed_out: bool) -> Result<()> {
    if timed_out {
        return Err(DesktopError::ScreenCapture(
            "recording did not signal completion within 15s".into(),
        ));
    }
    match std::fs::metadata(path) {
        Ok(m) if m.len() > 0 => Ok(()),
        Ok(_) => Err(DesktopError::ScreenCapture(
            "recording finished but the output file is empty".into(),
        )),
        Err(e) => Err(DesktopError::ScreenCapture(format!(
            "recording finished but the output file is missing: {e}"
        ))),
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p aleph-desktop verify_recording_output --lib`
Expected: PASS.

- [ ] **Step 5: Capture the timeout flag and verify output**

In `sc_recording_output_record` step 12, replace the wait block (currently ~lines 366–373):

```rust
    // 12. Wait for delegate's didFinishRecording callback
    let (lock, cvar) = &*finished_signal;
    let guard = lock
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let _result = cvar
        .wait_timeout_while(guard, Duration::from_secs(15), |finished| !*finished)
        .unwrap_or_else(std::sync::PoisonError::into_inner);
```

with:

```rust
    // 12. Wait for delegate's didFinishRecording callback
    let (lock, cvar) = &*finished_signal;
    let guard = lock
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (_guard, wait_res) = cvar
        .wait_timeout_while(guard, Duration::from_secs(15), |finished| !*finished)
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let timed_out = wait_res.timed_out();
```

Then, immediately before the final `debug!("Screen recording complete: ...")` / `Ok(...)` (after the existing `error_slot` check block), add:

```rust
    // The delegate never signalling completion, or an absent/empty file, means
    // no usable recording — do not report success (matches the CLI/ffmpeg paths).
    verify_recording_output(output_path, timed_out)?;
```

- [ ] **Step 6: Compile-check**

Run: `cargo check -p aleph-desktop`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add desktop/shared/src/perception/screen_record.rs
git commit -m "desktop: verify macOS recording produced output before reporting success"
```

---

### Task 3: Public-AX window resolver module (Group C, core)

**Files:**
- Create: `desktop/shared/src/action/window_ax.rs`
- Modify: `desktop/shared/src/action/mod.rs` (declare the module after `pub mod window;`, line 19)
- Test: `desktop/shared/src/action/window_ax.rs` (inline `#[cfg(test)] mod tests` — the pure matcher)

**Interfaces:**
- Consumes: `crate::BoundingBox { x, y, w, h: f64 }` (global top-left points).
- Produces: `pub fn raise_window(pid: u64, target: &BoundingBox) -> Result<bool>` and `pub fn set_window_geometry(pid: u64, target: &BoundingBox, position: Option<(i32,i32)>, size: Option<(i32,i32)>) -> Result<bool>`. Both return `Ok(false)` when no AX window matches (caller falls back); `Ok(true)` on success.

- [ ] **Step 1: Declare the module**

In `desktop/shared/src/action/mod.rs`, immediately after `pub mod window;` (line 19), add:

```rust
#[cfg(target_os = "macos")]
mod window_ax;
```

- [ ] **Step 2: Write the failing test (pure matcher)**

Create `desktop/shared/src/action/window_ax.rs` with only the pure matcher + its tests first, so the test can fail before the FFI lands:

```rust
//! macOS per-window targeting via the **public** Accessibility (AX) API.
//!
//! `NSRunningApplication` activates an application; System Events matches a
//! window by *title*. Both miss the specific `CGWindowID` the caller named when
//! an app has several windows (or duplicate titles). This module resolves a
//! `CGWindowID` to its `AXUIElement` window by matching the AX window whose
//! position+size equals that window's global bounds (from
//! `CGWindowListCopyWindowInfo`). AX and CGWindowList both report
//! top-left-origin global points, so the geometry compares directly. **No
//! private symbols** (`_AXUIElementGetWindow`) are used.
//!
//! Entry points return `Ok(false)` when they cannot resolve/authorize the
//! window, so callers fall back to the legacy app-activate / osascript path.

use crate::BoundingBox;

/// Whether an AX window at (`px`,`py`) sized (`sw`,`sh`) is the window described
/// by `target` (global top-left points), within `tol` points to absorb rounding.
/// Pure — unit-testable without AX.
fn bounds_match(px: f64, py: f64, sw: f64, sh: f64, target: &BoundingBox, tol: f64) -> bool {
    (px - target.x).abs() <= tol
        && (py - target.y).abs() <= tol
        && (sw - target.w).abs() <= tol
        && (sh - target.h).abs() <= tol
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bb(x: f64, y: f64, w: f64, h: f64) -> BoundingBox {
        BoundingBox { x, y, w, h }
    }

    #[test]
    fn bounds_match_within_tolerance() {
        assert!(bounds_match(100.0, 200.4, 640.0, 479.6, &bb(100.0, 200.0, 640.0, 480.0), 2.0));
    }

    #[test]
    fn bounds_match_rejects_offset_or_size_drift() {
        assert!(!bounds_match(100.0, 210.0, 640.0, 480.0, &bb(100.0, 200.0, 640.0, 480.0), 2.0));
        assert!(!bounds_match(100.0, 200.0, 640.0, 200.0, &bb(100.0, 200.0, 640.0, 480.0), 2.0));
    }
}
```

- [ ] **Step 3: Run test to verify it fails, then passes for the matcher**

Run: `cargo test -p aleph-desktop bounds_match --lib`
Expected: initially FAIL if the module is not yet compiled in; after Steps 1–2 it PASSES for the two matcher tests. (The FFI in Step 4 does not change these results.)

- [ ] **Step 4: Add the AX FFI + resolver + entry points**

Insert the following **above** the `#[cfg(test)]` block in `window_ax.rs`:

```rust
use std::ffi::c_void;

use core_foundation::base::TCFType;
use core_foundation::string::CFString;
use core_graphics::geometry::{CGPoint, CGSize};

use crate::error::{DesktopError, Result};

/// Opaque CoreFoundation / AX reference. Modeled as a raw pointer; every ref we
/// own is released via `CfOwned`.
type CfRef = *const c_void;
type AXError = i32;
type AXValueType = u32;

const KAXVALUE_CGPOINT: AXValueType = 1;
const KAXVALUE_CGSIZE: AXValueType = 2;
const AX_SUCCESS: AXError = 0;
/// Geometry match tolerance in points (absorbs AX/CG rounding).
const MATCH_TOL: f64 = 2.0;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXUIElementCreateApplication(pid: i32) -> CfRef;
    fn AXUIElementCopyAttributeValue(element: CfRef, attribute: CfRef, value: *mut CfRef) -> AXError;
    fn AXUIElementSetAttributeValue(element: CfRef, attribute: CfRef, value: CfRef) -> AXError;
    fn AXUIElementPerformAction(element: CfRef, action: CfRef) -> AXError;
    fn AXValueCreate(the_type: AXValueType, value_ptr: *const c_void) -> CfRef;
    fn AXValueGetValue(value: CfRef, the_type: AXValueType, value_ptr: *mut c_void) -> bool;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFArrayGetCount(array: CfRef) -> isize;
    fn CFArrayGetValueAtIndex(array: CfRef, idx: isize) -> CfRef;
    fn CFRelease(cf: CfRef);
    fn CFRetain(cf: CfRef) -> CfRef;
}

/// Owns a +1 AX/CF reference and releases it exactly once on drop.
struct CfOwned(CfRef);

impl Drop for CfOwned {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: `self.0` is a +1 reference from a Create/Copy AX call or
            // `CFRetain`; released exactly once here.
            unsafe { CFRelease(self.0) };
        }
    }
}

/// Read an AXValue geometry attribute (`AXPosition`/`AXSize`) of `element` into
/// `out`. Returns false on any AX error or type mismatch.
///
/// # Safety
/// `element` must be a live AX element; `out` must be the struct matching
/// `ax_type` (`CGPoint` for point, `CGSize` for size).
unsafe fn copy_ax_geometry<T>(element: CfRef, attr: &str, ax_type: AXValueType, out: &mut T) -> bool {
    let attr_cf = CFString::new(attr);
    let mut val: CfRef = std::ptr::null();
    if AXUIElementCopyAttributeValue(element, attr_cf.as_concrete_TypeRef().cast(), &mut val) != AX_SUCCESS
        || val.is_null()
    {
        return false;
    }
    let owned = CfOwned(val);
    AXValueGetValue(owned.0, ax_type, (out as *mut T).cast())
}

/// Resolve the AX window of process `pid` whose geometry matches `target`.
/// Returns an owned (+1) AX element, or `None` when AX is unauthorized, the app
/// has no windows, or none matches. All intermediate references are released.
fn matched_window(pid: u64, target: &BoundingBox) -> Option<CfOwned> {
    let pid_i32 = i32::try_from(pid).ok()?;
    // SAFETY: creates the application AX element (+1); released via CfOwned.
    let app = CfOwned(unsafe { AXUIElementCreateApplication(pid_i32) });
    if app.0.is_null() {
        return None;
    }

    let windows_attr = CFString::new("AXWindows");
    let mut windows_ref: CfRef = std::ptr::null();
    // SAFETY: valid app element + attribute string; out-param initialized to null.
    let err = unsafe {
        AXUIElementCopyAttributeValue(app.0, windows_attr.as_concrete_TypeRef().cast(), &mut windows_ref)
    };
    if err != AX_SUCCESS || windows_ref.is_null() {
        return None; // e.g. kAXErrorAPIDisabled — no Accessibility permission
    }
    let windows = CfOwned(windows_ref); // CFArray (+1)

    // SAFETY: `windows.0` is a CFArray of AX elements.
    let count = unsafe { CFArrayGetCount(windows.0) };
    for i in 0..count {
        // SAFETY: `i` in [0, count); the returned element is borrowed (get-rule).
        let win = unsafe { CFArrayGetValueAtIndex(windows.0, i) };
        if win.is_null() {
            continue;
        }
        let mut pos = CGPoint { x: 0.0, y: 0.0 };
        let mut size = CGSize { width: 0.0, height: 0.0 };
        // SAFETY: `win` is a valid AX element; geometry read into stack structs.
        let ok = unsafe {
            copy_ax_geometry(win, "AXPosition", KAXVALUE_CGPOINT, &mut pos)
                && copy_ax_geometry(win, "AXSize", KAXVALUE_CGSIZE, &mut size)
        };
        if ok && bounds_match(pos.x, pos.y, size.width, size.height, target, MATCH_TOL) {
            // Retain the borrowed match so it outlives the array release.
            // SAFETY: `win` is a live element from the still-owned array.
            return Some(CfOwned(unsafe { CFRetain(win) }));
        }
    }
    None
}

/// Raise the specific window identified by `target` bounds within process
/// `pid`. `Ok(true)` = raised; `Ok(false)` = no AX window matched (caller
/// should fall back to app activation).
pub fn raise_window(pid: u64, target: &BoundingBox) -> Result<bool> {
    let Some(win) = matched_window(pid, target) else {
        return Ok(false);
    };
    let action = CFString::new("AXRaise");
    // SAFETY: `win.0` is a valid retained AX element; `action` is a live CFString.
    let err = unsafe { AXUIElementPerformAction(win.0, action.as_concrete_TypeRef().cast()) };
    if err == AX_SUCCESS {
        Ok(true)
    } else {
        Err(DesktopError::WindowFailed(format!("AXRaise failed (AXError {err})")))
    }
}

/// Set the position and/or size of the specific window identified by `target`
/// bounds within `pid`. `Ok(true)` = updated; `Ok(false)` = no AX window
/// matched (caller falls back to osascript).
pub fn set_window_geometry(
    pid: u64,
    target: &BoundingBox,
    position: Option<(i32, i32)>,
    size: Option<(i32, i32)>,
) -> Result<bool> {
    let Some(win) = matched_window(pid, target) else {
        return Ok(false);
    };
    if let Some((x, y)) = position {
        let mut p = CGPoint { x: f64::from(x), y: f64::from(y) };
        // SAFETY: `p` is a valid CGPoint; AXValueCreate copies it.
        let val = CfOwned(unsafe { AXValueCreate(KAXVALUE_CGPOINT, std::ptr::addr_of_mut!(p).cast()) });
        if val.0.is_null() {
            return Err(DesktopError::WindowFailed("AXValueCreate(position) failed".into()));
        }
        let attr = CFString::new("AXPosition");
        // SAFETY: valid window, attribute string, and AXValue.
        let err = unsafe {
            AXUIElementSetAttributeValue(win.0, attr.as_concrete_TypeRef().cast(), val.0)
        };
        if err != AX_SUCCESS {
            return Err(DesktopError::WindowFailed(format!("set AXPosition failed (AXError {err})")));
        }
    }
    if let Some((w, h)) = size {
        let mut s = CGSize { width: f64::from(w), height: f64::from(h) };
        // SAFETY: `s` is a valid CGSize; AXValueCreate copies it.
        let val = CfOwned(unsafe { AXValueCreate(KAXVALUE_CGSIZE, std::ptr::addr_of_mut!(s).cast()) });
        if val.0.is_null() {
            return Err(DesktopError::WindowFailed("AXValueCreate(size) failed".into()));
        }
        let attr = CFString::new("AXSize");
        // SAFETY: valid window, attribute string, and AXValue.
        let err = unsafe {
            AXUIElementSetAttributeValue(win.0, attr.as_concrete_TypeRef().cast(), val.0)
        };
        if err != AX_SUCCESS {
            return Err(DesktopError::WindowFailed(format!("set AXSize failed (AXError {err})")));
        }
    }
    Ok(true)
}
```

- [ ] **Step 5: Compile-check + rerun the matcher test**

Run: `cargo check -p aleph-desktop`
Expected: PASS. If `as_concrete_TypeRef().cast()` or an `extern` signature mismatches the installed `core-foundation 0.10` / SDK forms, adjust to the compiler's suggested pointer form — the invariants are: (1) attribute strings passed as the AX `CFStringRef`, (2) every Create/Copy/Retain ref released once via `CfOwned`, (3) geometry read into `CGPoint`/`CGSize`.
Run: `cargo test -p aleph-desktop bounds_match --lib`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add desktop/shared/src/action/mod.rs desktop/shared/src/action/window_ax.rs
git commit -m "desktop: add public-AX window resolver for macOS (CGWindowID->AX by geometry)"
```

---

### Task 4: Focus the exact window on macOS via AX (Group C, focus)

**Files:**
- Modify: `desktop/shared/src/action/window.rs` (`macos_focus_window`, lines 490–514)
- Test: none unit-testable (AX raise needs a live window); verification is compile + runtime smoke.

**Interfaces:**
- Consumes: `super::window_ax::raise_window(pid: u64, target: &BoundingBox) -> Result<bool>` (Task 3).

- [ ] **Step 1: Replace `macos_focus_window`**

Replace the whole function (lines 490–514) with:

```rust
#[cfg(target_os = "macos")]
fn macos_focus_window(window_id: u64) -> Result<()> {
    // Find the PID (and bounds) for this window by scanning the window list.
    let windows = macos_window_list()?;
    let window = windows.iter().find(|w| w.id == window_id).ok_or_else(|| {
        DesktopError::WindowFailed(format!("No window found with id {window_id}"))
    })?;

    let pid = window.pid as i32;

    use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};
    let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(pid) else {
        return Err(DesktopError::WindowFailed(format!(
            "No application found with PID {pid}"
        )));
    };
    #[allow(deprecated)]
    // ActivateIgnoringOtherApps deprecated in macOS 14 but still functional.
    app.activateWithOptions(NSApplicationActivationOptions::ActivateIgnoringOtherApps);

    // Bring the *specific* window forward, not just whatever the app had
    // frontmost. Best-effort: without a match / Accessibility permission we have
    // still activated the app (the prior behavior), so never hard-fail here.
    match window.bounds.as_ref() {
        Some(bounds) => match super::window_ax::raise_window(window.pid, bounds) {
            Ok(true) => info!(window_id, pid, "Specific window raised via AX (macOS)"),
            Ok(false) => info!(window_id, pid, "No AX window match; app activated only (macOS)"),
            Err(e) => tracing::warn!(window_id, "AX raise error: {e}; app activated only"),
        },
        None => info!(window_id, pid, "No window bounds; app activated only (macOS)"),
    }
    Ok(())
}
```

- [ ] **Step 2: Compile-check**

Run: `cargo check -p aleph-desktop`
Expected: PASS.

- [ ] **Step 3: Runtime smoke (this macOS box)**

Open two windows of the same app (e.g. two TextEdit documents). Note the non-frontmost window's id from `window_list`. Call `focus_window(<that id>)` and confirm **that** window comes forward (not merely the app's frontmost window). Grant Accessibility permission if prompted; without it, the app still activates (fallback verified).

- [ ] **Step 4: Commit**

```bash
git add desktop/shared/src/action/window.rs
git commit -m "desktop: raise the specific target window on macOS focus (AX), not just the app"
```

---

### Task 5: Set macOS window bounds on the exact window via AX (Group C, bounds)

**Files:**
- Modify: `desktop/shared/src/action/window.rs` (`macos_set_window_bounds`, lines 527–599)
- Test: none unit-testable (AX set needs a live window); verification is compile + runtime smoke.

**Interfaces:**
- Consumes: `super::window_ax::set_window_geometry(pid, target, position, size) -> Result<bool>` (Task 3).

- [ ] **Step 1: Insert the AX-first path before the osascript block**

In `macos_set_window_bounds`, after the `let ((a, b), setter) = match (position, size) { ... };` block and **before** the `let script = format!(...)` line, insert:

```rust
    // Precise path: resolve the exact CGWindowID to its AX window (by geometry)
    // and set position/size there. Falls through to the osascript-by-title path
    // below when AX can't resolve/authorize the window (no match / no perm).
    if let Some(bounds) = window.bounds.as_ref() {
        match super::window_ax::set_window_geometry(pid, bounds, position, size) {
            Ok(true) => {
                info!(window_id, pid, "Window bounds updated via AX (macOS)");
                return Ok(());
            }
            Ok(false) => { /* no AX match — fall through to osascript */ }
            Err(e) => {
                tracing::warn!(window_id, "AX set-bounds error: {e}; falling back to osascript");
            }
        }
    }
```

(The existing `(a, b)` / `setter` derivation and the osascript block remain unchanged as the fallback. `pid` is `window.pid: u64`, which `set_window_geometry` expects.)

- [ ] **Step 2: Compile-check**

Run: `cargo check -p aleph-desktop`
Expected: PASS. (`a`, `b`, `setter` are still used by the retained osascript fallback, so no unused-variable warnings.)

- [ ] **Step 3: Runtime smoke (this macOS box)**

Open two same-titled windows (or two TextEdit docs). `move_window(<id>, x, y)` / `resize_window(<id>, w, h)` on one and confirm the **correct** window (matching that id's bounds) moves/resizes, not a same-titled sibling. Without Accessibility permission it falls back to osascript (prior behavior).

- [ ] **Step 4: Commit**

```bash
git add desktop/shared/src/action/window.rs
git commit -m "desktop: set macOS window bounds on the exact window via AX, osascript fallback"
```

---

### Task 6: Preserve typed bridge errors on the media rail (Group D, media)

**Files:**
- Modify: `desktop/macos/src/lib.rs` (replace `bridge_err` at lines 186–188 with `preserve_typed`; update the 8 media `.map_err` sites at lines 225, 257, 286, 303, 322, 339, 360, 394)
- Test: `desktop/macos/src/lib.rs` (inline `#[cfg(test)] mod tests` — create if absent)

**Interfaces:**
- Produces: `fn preserve_typed(method: &str, e: aleph_desktop::DesktopError) -> aleph_desktop::DesktopError`.

- [ ] **Step 1: Write the failing test**

Append to (or create, with `use super::*;`) the `#[cfg(test)] mod tests` in `lib.rs`:

```rust
    #[test]
    fn preserve_typed_passes_timeout_through() {
        use aleph_desktop::DesktopError;
        let e = DesktopError::BridgeTimeout("slow".into());
        assert!(matches!(preserve_typed("media.audio.record", e), DesktopError::BridgeTimeout(_)));
    }

    #[test]
    fn preserve_typed_decorates_bridge_failed_with_method() {
        use aleph_desktop::DesktopError;
        match preserve_typed("media.camera.snap", DesktopError::BridgeFailed("boom".into())) {
            DesktopError::BridgeFailed(m) => assert_eq!(m, "media.camera.snap: boom"),
            other => panic!("unexpected variant: {other:?}"),
        }
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `CARGO_PROFILE_TEST_DEBUG=line-tables-only cargo test -p aleph-desktop-macos preserve_typed --lib`
Expected: FAIL — `preserve_typed` not defined.

- [ ] **Step 3: Replace `bridge_err` with `preserve_typed`**

In `desktop/macos/src/lib.rs`, replace the helper (lines 186–188):

```rust
fn bridge_err(msg: &str) -> aleph_desktop::DesktopError {
    aleph_desktop::DesktopError::BridgeFailed(msg.to_string())
}
```

with:

```rust
/// Map a bridge-call error while **preserving typed recovery variants**.
///
/// `SwiftBridge::call` already returns a typed `DesktopError` (permission /
/// timeout / platform / …, mapped in `bridge/client.rs::map_bridge_error`). The
/// media rail used to re-flatten every error into `BridgeFailed`, discarding the
/// `PermissionDenied` guide and timeout semantics the caller needs. Keep those
/// variants intact; only add method context to an opaque `BridgeFailed`.
fn preserve_typed(method: &str, e: aleph_desktop::DesktopError) -> aleph_desktop::DesktopError {
    use aleph_desktop::DesktopError;
    match e {
        DesktopError::BridgeFailed(m) => DesktopError::BridgeFailed(format!("{method}: {m}")),
        other => other,
    }
}
```

- [ ] **Step 4: Update the 8 media call sites**

Replace each `.map_err(|e| bridge_err(&format!("<method> RPC: {e}")))` with `.map_err(|e| preserve_typed("<method>", e))`, dropping the `RPC:` suffix. Exact edits:

| Line | From | To |
|------|------|----|
| 225 | `.map_err(\|e\| bridge_err(&format!("media.camera.snap RPC: {e}")))?` | `.map_err(\|e\| preserve_typed("media.camera.snap", e))?` |
| 257 | `...("media.camera.clip RPC: {e}")))?` | `.map_err(\|e\| preserve_typed("media.camera.clip", e))?` |
| 286 | `...("media.audio.record RPC: {e}")))?` | `.map_err(\|e\| preserve_typed("media.audio.record", e))?` |
| 303 | `...("media.audio.record_start RPC: {e}")))?` | `.map_err(\|e\| preserve_typed("media.audio.record_start", e))?` |
| 322 | `...("media.audio.record_stop RPC: {e}")))?` | `.map_err(\|e\| preserve_typed("media.audio.record_stop", e))?` |
| 339 | `...("media.audio.list_devices RPC: {e}")))?` | `.map_err(\|e\| preserve_typed("media.audio.list_devices", e))?` |
| 360 | `...("media.audio.mic_meter RPC: {e}")))?` | `.map_err(\|e\| preserve_typed("media.audio.mic_meter", e))?` |
| 394 | `...("media.speech.transcribe_file RPC: {e}")))?` | `.map_err(\|e\| preserve_typed("media.speech.transcribe_file", e))?` |

- [ ] **Step 5: Run test to verify it passes + compile-check**

Run: `CARGO_PROFILE_TEST_DEBUG=line-tables-only cargo test -p aleph-desktop-macos preserve_typed --lib`
Expected: PASS.
Run: `cargo check -p aleph-desktop-macos`
Expected: PASS (no remaining `bridge_err` references).

- [ ] **Step 6: Commit**

```bash
git add desktop/macos/src/lib.rs
git commit -m "desktop-macos: preserve typed bridge errors on the media rail (drop re-flatten)"
```

---

### Task 7: Preserve typed bridge errors on the input/screenshot rails (Group D, parallel sites)

**Files:**
- Modify: `desktop/macos/src/screen.rs` (`call_input` lines 511–520; `screenshot_via_bridge` map_err at line 547)
- Test: none new (behavior for `BridgeFailed` is unchanged text; the typed pass-through match is identical to Task 6's tested shape). `pim.rs::call_pim` already does this correctly — leave it.

**Interfaces:**
- Consumes: nothing new (local `match` arms, mirroring `pim.rs::call_pim`).

- [ ] **Step 1: Fix `call_input`**

Replace the body of `call_input` (lines 516–519):

```rust
        self.bridge
            .call(method, params)
            .await
            .map_err(|e| DesktopError::InputFailed(format!("bridge {method}: {e}")))
```

with:

```rust
        self.bridge.call(method, params).await.map_err(|e| match e {
            // Keep typed recovery variants (permission / timeout / …); only an
            // opaque BridgeFailed becomes an InputFailed with rail context.
            DesktopError::BridgeFailed(m) => DesktopError::InputFailed(format!("bridge {method}: {m}")),
            other => other,
        })
```

- [ ] **Step 2: Fix `screenshot_via_bridge`**

Replace the map_err at line 547:

```rust
            .map_err(|e| DesktopError::ScreenCapture(format!("bridge screen.capture: {e}")))?;
```

with:

```rust
            .map_err(|e| match e {
                DesktopError::BridgeFailed(m) => {
                    DesktopError::ScreenCapture(format!("bridge screen.capture: {m}"))
                }
                other => other,
            })?;
```

- [ ] **Step 3: Compile-check**

Run: `cargo check -p aleph-desktop-macos`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add desktop/macos/src/screen.rs
git commit -m "desktop-macos: preserve typed bridge errors on input/screenshot rails"
```

---

### Final verification

- [ ] **Step 1: Consolidated compile-checks**

Run: `cargo check -p aleph-desktop`
Expected: PASS (Tasks 1–5).
Run: `cargo check -p aleph-desktop-macos`
Expected: PASS (Tasks 6–7).

- [ ] **Step 2: Scoped test runs**

Run: `cargo test -p aleph-desktop sck_region_rect --lib && cargo test -p aleph-desktop verify_recording_output --lib && cargo test -p aleph-desktop bounds_match --lib`
Run: `CARGO_PROFILE_TEST_DEBUG=line-tables-only cargo test -p aleph-desktop-macos preserve_typed --lib`
Expected: all PASS.

- [ ] **Step 3: Runtime smoke summary (this macOS box)**

- A: record a known sub-region; confirm the output pixel dims == region × scale and the content is the cropped area (not full screen).
- B: a normal short recording still returns `Ok` with a non-empty file.
- C: two windows of one app — `focus_window` raises the correct one; `move_window`/`resize_window` targets the correct one.

- [ ] **Step 4: Confirm deferred items remain recorded**

Verify `docs/superpowers/specs/2026-07-20-macos-review-followup-fixes-design.md` §范围外 still documents #7 (Linux) and the C private-API exactness upgrade as not-in-this-cycle. No code for those lands here.
