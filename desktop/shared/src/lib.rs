//! # aleph-desktop
//!
//! Cross-platform desktop capabilities for the Aleph AI assistant.
//!
//! ## Architecture
//!
//! [`DesktopPlatform`] aggregator exposing eight capability traits, each an
//! `Option<&dyn …>` so a platform returns `None` for what it can't do:
//! - [`ScreenCapability`] — screenshot, OCR, mouse/keyboard, window management
//! - [`PimCapability`] — Notes, Calendar, Reminders, Contacts
//! - [`SystemCapability`] — app management, notifications, clipboard, system info
//! - [`AutomationCapability`] — AppleScript/JXA, Shortcuts
//! - [`PermissionCapability`] — TCC permission detection and request
//! - [`MediaCapability`] — camera / audio device capture
//! - [`AccessibilityCapability`] — AX tree queries (macOS AX, Windows UIA, Linux AT-SPI2)
//! - [`PowerCapability`] — sleep inhibition (via the `power()` accessor)
//!
//! Real platform API calls never live here: each platform crate
//! (`desktop-macos`, `desktop-linux`, `desktop-windows`) implements
//! [`DesktopPlatform`] and reaches the OS through the [`bridge`] JSON-RPC IPC
//! layer (R1 brain–limb separation).

pub mod action;
pub mod automation_types;
pub mod ax_rank;
pub mod ax_secure;
pub mod bridge;
pub mod coord;
pub mod error;
// Linux session/tool detection plus the Linux-only clipboard and app-launch
// implementations. Compiled everywhere so its pure logic stays host-testable;
// its consumers are all Linux-gated.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub mod linux;
pub mod media_types;
pub mod native_screen;
pub mod perception;
pub mod permission_types;
pub mod pim_types;
pub mod platform;
pub mod screen_types;
pub mod script_exec;
pub mod system_types;
pub mod traits;
pub mod win_dpi;
// Absolute pointer positioning. Like `win_dpi`, the arithmetic compiles (and is
// unit-tested) everywhere; only the `SendInput` call is Windows-gated.
pub mod win_input;
#[cfg(windows)]
pub mod win_registry;
#[cfg(windows)]
pub mod win_window;

pub use ax_rank::{rank_candidates, RankCandidate};
pub use ax_secure::is_password_like;
pub use coord::{CoordinateSpace, Point};
pub use error::{DesktopError, Result};
pub use native_screen::NativeScreen;
// Re-exported at the root because it is the return type of
// `ensure_process_dpi_aware`, which the Windows platform crate calls by path.
pub use win_dpi::{ensure_process_dpi_aware, DpiAwareness};

// Re-export the long-lived Swift helper RPC client at crate root so callers
// can write `aleph_desktop::SwiftBridge` without reaching into the submodule.
pub use bridge::client::SwiftBridge;

// Re-export new capability traits.
pub use platform::{DesktopPlatform, EscapeAbort};
pub use traits::{
    AccessibilityCapability, AutomationCapability, MediaCapability, PermissionCapability,
    PimCapability, ScreenCapability, SystemCapability,
};

use serde::{Deserialize, Serialize};

// ── Types ────────────────────────────────────────────────────────

/// A rectangular region on the screen, in physical pixels of the **global**
/// screen space (the space clicks are issued in), top-left origin.
///
/// The fields are unsigned, so a display placed left of or above the primary one
/// — which has negative global coordinates — cannot be addressed by a region.
/// That is a known limitation of this shape, not a silent remapping: a region is
/// captured from the display it overlaps, never re-interpreted as an offset into
/// a different one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenRegion {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// A captured screenshot, encoded as PNG.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Screenshot {
    /// Base64-encoded PNG image data.
    pub image_base64: String,
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// Image format (always "png").
    pub format: String,
    /// Display backing-scale factor (e.g. 2.0 for Retina). Lets the model
    /// reason in logical points or normalized coords without losing pixel
    /// fidelity. `serde(default)` keeps the wire format back-compatible:
    /// older bridges that omit the field deserialize to `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale_factor: Option<f64>,
}

/// A screenshot cropped to a single window, plus the geometry needed to turn
/// its pixels back into global click coordinates.
///
/// Carrying the mapping alongside the image is what keeps a window-cropped
/// capture from becoming a targeting regression: the model sees pixels relative
/// to the window, but every click is issued in the global point space, so the
/// window's origin and the backing scale must travel with the image rather than
/// be re-derived (and mis-derived) downstream:
///
/// ```text
/// global_point = window_bounds.origin + pixel / image.scale_factor
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowShot {
    /// The captured image. `image.scale_factor` is the backing scale of the
    /// captured surface (2.0 on Retina): `pixels = points * scale_factor`.
    pub image: Screenshot,
    /// The window's frame in the GLOBAL screen POINT space, top-left origin.
    /// `None` when the platform could not report it — in which case the pixels
    /// cannot be mapped back and the caller must not pretend otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_bounds: Option<BoundingBox>,
}

/// Result of an OCR operation on a screenshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrResult {
    /// Full recognized text, concatenated from all lines.
    pub full_text: String,
    /// Individual lines with optional bounding boxes.
    pub lines: Vec<OcrLine>,
}

/// A single line of text recognized by OCR.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrLine {
    /// The recognized text content.
    pub text: String,
    /// Optional bounding box for this line.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bounding_box: Option<BoundingBox>,
    /// Optional confidence score (0.0 to 1.0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
}

/// A bounding box in screen coordinates.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct BoundingBox {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Mouse button identifiers.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// Mouse/keyboard press action type.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PressAction {
    /// Hold button/key down without releasing.
    Press,
    /// Release a previously pressed button/key.
    Release,
    /// Press and immediately release (standard click).
    Click,
}

/// Information about a connected display.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayInfo {
    /// Platform-specific display identifier.
    pub id: u32,
    /// Display name (may be empty on some platforms).
    pub name: String,
    /// Logical width in pixels.
    pub width: u32,
    /// Logical height in pixels.
    pub height: u32,
    /// Scale factor (e.g., 2.0 for Retina).
    pub scale_factor: f64,
    /// Whether this is the primary display.
    pub is_primary: bool,
    /// X origin in the global coordinate space.
    pub origin_x: i32,
    /// Y origin in the global coordinate space.
    pub origin_y: i32,
}

/// Information about an on-screen window.
///
/// The three trailing fields are `Option` because `None` means *"this
/// platform's window query does not tell us"* — never "zero" or "false". A
/// platform that cannot report geometry must leave `bounds` absent rather than
/// invent one, and every consumer must treat unknown as unknown.
///
/// `Default` exists so a limb can spell only the fields its platform query
/// actually yields via `..Default::default()`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WindowInfo {
    /// Platform-specific window identifier (HWND on Windows, XID on Linux, etc.).
    pub id: u64,
    /// Window title.
    pub title: String,
    /// Owning application name (may be empty on some platforms).
    pub owner: String,
    /// Process ID of the owning application.
    pub pid: u64,
    /// Window frame in the GLOBAL screen POINT space, top-left origin — the
    /// same space clicks are issued in. Without it nothing can crop a capture
    /// to this window, nor map that crop's pixels back to click coordinates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bounds: Option<BoundingBox>,
    /// Window level. `0` is a normal application window; menu extras, docks and
    /// overlays sit above it. Lets a caller skip chrome by a mechanical rule
    /// instead of guessing from the title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer: Option<i32>,
    /// Whether the window is currently on screen (not minimized, not parked on
    /// another Space/desktop).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_screen: Option<bool>,
}
