//! Platform aggregator trait.

use crate::traits::{
    AccessibilityCapability, AutomationCapability, MediaCapability, PermissionCapability,
    PimCapability, PowerCapability, ScreenCapability, SystemCapability,
};

/// Cross-platform escape abort interface.
///
/// Allows the user to press Escape to abort AI desktop control.
pub trait EscapeAbort: Send + Sync {
    /// Start listening for Escape key presses.
    fn start(&self) -> crate::Result<()>;
    /// Stop listening and clean up.
    fn stop(&self);
    /// Check if the user has pressed Escape since the last reset.
    fn is_aborted(&self) -> bool;
    /// Reset the abort flag (prepare for next action).
    fn reset(&self);
}

/// Aggregator that provides access to all desktop capabilities on a given platform.
///
/// Each capability returns `Option<&dyn XCapability>` because not every platform
/// supports every capability (e.g., PIM is only available on macOS currently).
///
/// # Example
///
/// ```ignore
/// async fn use_platform(platform: &dyn DesktopPlatform) {
///     if let Some(screen) = platform.screen() {
///         let windows = screen.window_list().await.unwrap();
///         println!("{} windows", windows.len());
///     }
/// }
/// ```
pub trait DesktopPlatform: Send + Sync {
    /// Human-readable platform name (e.g., "macOS", "Windows", "Linux").
    fn platform_name(&self) -> &str;

    /// Screen perception and input automation, if available.
    fn screen(&self) -> Option<&dyn ScreenCapability>;

    /// Personal information management (Notes, Calendar, Reminders, Contacts), if available.
    fn pim(&self) -> Option<&dyn PimCapability>;

    /// System-level operations (apps, clipboard, notifications), if available.
    fn system(&self) -> Option<&dyn SystemCapability>;

    /// Automation scripting and Shortcuts, if available.
    fn automation(&self) -> Option<&dyn AutomationCapability>;

    /// TCC permission detection and request, if available.
    fn permission(&self) -> Option<&dyn PermissionCapability>;

    /// Media capture (camera, audio devices), if available.
    fn media(&self) -> Option<&dyn MediaCapability>;

    /// Accessibility (AX) tree queries, if available.
    ///
    /// macOS (Swift bridge), Windows (UI Automation) and Linux (AT-SPI2) all
    /// implement this. A platform returns `None` only when its accessibility
    /// bus is unreachable at runtime (e.g. no AT-SPI registry on a headless
    /// Linux box) — an honest absence is quieter than a capability that errors
    /// on every call.
    fn ax(&self) -> Option<&dyn AccessibilityCapability> {
        None
    }

    /// Power management — sleep inhibition, if available on this platform.
    fn power(&self) -> Option<&dyn PowerCapability> {
        None
    }

    /// Escape key abort listener, if available on this platform.
    fn escape_listener(&self) -> Option<&dyn EscapeAbort> {
        None
    }
}
