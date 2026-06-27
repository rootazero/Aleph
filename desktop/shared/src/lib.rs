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
//! - [`AccessibilityCapability`] — AX tree queries (macOS only)
//! - [`PowerCapability`] — sleep inhibition (via the `power()` accessor)
//!
//! Real platform API calls never live here: each platform crate
//! (`desktop-macos`, `desktop-linux`, `desktop-windows`) implements
//! [`DesktopPlatform`] and reaches the OS through the [`bridge`] JSON-RPC IPC
//! layer (R1 brain–limb separation).

pub mod action;
pub mod automation_types;
pub mod bridge;
pub mod coord;
pub mod error;
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

pub use coord::{CoordinateSpace, Point};
pub use error::{DesktopError, Result};
pub use native_screen::NativeScreen;

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

/// A rectangular region on the screen, in physical pixels.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowInfo {
    /// Platform-specific window identifier (HWND on Windows, XID on Linux, etc.).
    pub id: u64,
    /// Window title.
    pub title: String,
    /// Owning application name (may be empty on some platforms).
    pub owner: String,
    /// Process ID of the owning application.
    pub pid: u64,
}
