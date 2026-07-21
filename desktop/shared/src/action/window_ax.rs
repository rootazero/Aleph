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
    fn AXUIElementCopyAttributeValue(
        element: CfRef,
        attribute: CfRef,
        value: *mut CfRef,
    ) -> AXError;
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
unsafe fn copy_ax_geometry<T>(
    element: CfRef,
    attr: &str,
    ax_type: AXValueType,
    out: &mut T,
) -> bool {
    let attr_cf = CFString::new(attr);
    let mut val: CfRef = std::ptr::null();
    if AXUIElementCopyAttributeValue(element, attr_cf.as_concrete_TypeRef().cast(), &mut val)
        != AX_SUCCESS
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
        AXUIElementCopyAttributeValue(
            app.0,
            windows_attr.as_concrete_TypeRef().cast(),
            &mut windows_ref,
        )
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
        let mut size = CGSize {
            width: 0.0,
            height: 0.0,
        };
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
        Err(DesktopError::WindowFailed(format!(
            "AXRaise failed (AXError {err})"
        )))
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
        let mut p = CGPoint {
            x: f64::from(x),
            y: f64::from(y),
        };
        // SAFETY: `p` is a valid CGPoint; AXValueCreate copies it.
        let val =
            CfOwned(unsafe { AXValueCreate(KAXVALUE_CGPOINT, std::ptr::addr_of_mut!(p).cast()) });
        if val.0.is_null() {
            return Err(DesktopError::WindowFailed(
                "AXValueCreate(position) failed".into(),
            ));
        }
        let attr = CFString::new("AXPosition");
        // SAFETY: valid window, attribute string, and AXValue.
        let err = unsafe {
            AXUIElementSetAttributeValue(win.0, attr.as_concrete_TypeRef().cast(), val.0)
        };
        if err != AX_SUCCESS {
            return Err(DesktopError::WindowFailed(format!(
                "set AXPosition failed (AXError {err})"
            )));
        }
    }
    if let Some((w, h)) = size {
        let mut s = CGSize {
            width: f64::from(w),
            height: f64::from(h),
        };
        // SAFETY: `s` is a valid CGSize; AXValueCreate copies it.
        let val =
            CfOwned(unsafe { AXValueCreate(KAXVALUE_CGSIZE, std::ptr::addr_of_mut!(s).cast()) });
        if val.0.is_null() {
            return Err(DesktopError::WindowFailed(
                "AXValueCreate(size) failed".into(),
            ));
        }
        let attr = CFString::new("AXSize");
        // SAFETY: valid window, attribute string, and AXValue.
        let err = unsafe {
            AXUIElementSetAttributeValue(win.0, attr.as_concrete_TypeRef().cast(), val.0)
        };
        if err != AX_SUCCESS {
            return Err(DesktopError::WindowFailed(format!(
                "set AXSize failed (AXError {err})"
            )));
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bb(x: f64, y: f64, w: f64, h: f64) -> BoundingBox {
        BoundingBox { x, y, w, h }
    }

    #[test]
    fn bounds_match_within_tolerance() {
        assert!(bounds_match(
            100.0,
            200.4,
            640.0,
            479.6,
            &bb(100.0, 200.0, 640.0, 480.0),
            2.0
        ));
    }

    #[test]
    fn bounds_match_rejects_offset_or_size_drift() {
        assert!(!bounds_match(
            100.0,
            210.0,
            640.0,
            480.0,
            &bb(100.0, 200.0, 640.0, 480.0),
            2.0
        ));
        assert!(!bounds_match(
            100.0,
            200.0,
            640.0,
            200.0,
            &bb(100.0, 200.0, 640.0, 480.0),
            2.0
        ));
    }
}
