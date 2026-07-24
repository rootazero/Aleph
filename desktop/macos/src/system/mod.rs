//! macOS `SystemCapability` implementation using native APIs (objc2).

mod clipboard;
mod notification;
mod sysinfo;
mod workspace;

use aleph_desktop::system_types::{AppInfo, ClipboardContent, SystemInfo};
use aleph_desktop::traits::SystemCapability;
use aleph_desktop::Result;
use async_trait::async_trait;

/// macOS system capability implementation using native Cocoa APIs.
pub struct MacOSSystem {
    _private: (),
}

impl MacOSSystem {
    pub const fn new() -> Self {
        Self { _private: () }
    }
}

impl Default for MacOSSystem {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SystemCapability for MacOSSystem {
    async fn launch_app(&self, app_name: &str) -> Result<()> {
        workspace::launch_app(app_name)
    }

    async fn quit_app(&self, app_name: &str) -> Result<()> {
        workspace::quit_app(app_name)
    }

    async fn list_running_apps(&self) -> Result<Vec<AppInfo>> {
        workspace::list_running_apps()
    }

    async fn send_notification(&self, title: &str, body: &str) -> Result<()> {
        notification::send_notification(title, body).await
    }

    async fn clipboard_read(&self) -> Result<ClipboardContent> {
        clipboard::read()
    }

    async fn clipboard_write(&self, text: &str) -> Result<()> {
        clipboard::write(text)
    }

    async fn system_info(&self) -> Result<SystemInfo> {
        sysinfo::system_info()
    }

    async fn user_idle_seconds(&self) -> Result<f64> {
        Ok(user_idle_seconds())
    }
}

/// Seconds since last keyboard/mouse event via `CGEventSource`.
fn user_idle_seconds() -> f64 {
    extern "C" {
        // CGEventSourceSecondsSinceLastEventType(stateID: CGEventSourceStateID, eventType: CGEventType) -> CFTimeInterval
        fn CGEventSourceSecondsSinceLastEventType(state_id: i32, event_type: u32) -> f64;
    }

    // kCGEventSourceStateCombinedSessionState = 0, kCGAnyInputEventType = ~0u (0xFFFFFFFF)
    // SAFETY: CoreGraphics C function taking two by-value integer arguments and
    // holding no pointer preconditions; both values are the documented sentinels
    // above and the result (a `CFTimeInterval`) is range-checked by the caller.
    let seconds = unsafe { CGEventSourceSecondsSinceLastEventType(0, 0xFFFFFFFF) };

    // Guard against NaN/Infinity (edge case on machines without input devices)
    if seconds.is_nan() || seconds.is_infinite() {
        0.0
    } else {
        seconds
    }
}
