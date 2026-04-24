//! macOS platform implementation for Aleph desktop capabilities.

mod automation;
mod escape_listener;
pub mod hotkey;
mod media;
mod permission;
mod pim;
mod system;

use std::path::PathBuf;
use std::sync::Arc;

use aleph_desktop::platform::EscapeAbort;
use aleph_desktop::traits::{
    AutomationCapability, MediaCapability, PermissionCapability, PimCapability, ScreenCapability,
    SystemCapability,
};
use aleph_desktop::DesktopPlatform;
use aleph_desktop::NativeScreen;
use aleph_desktop::SwiftBridge;

use automation::MacOSAutomation;
use escape_listener::EscapeListener;
use media::MacOSMedia;
use permission::MacOSPermission;
use pim::MacOSPim;
use system::MacOSSystem;

/// macOS platform with shared `NativeScreen` for screen capabilities.
pub struct MacOSPlatform {
    screen: NativeScreen,
    automation: MacOSAutomation,
    escape: EscapeListener,
    media: MacOSMedia,
    permission: MacOSPermission,
    pim: MacOSPim,
    system: MacOSSystem,
    bridge: Arc<SwiftBridge>,
}

impl MacOSPlatform {
    /// Create a new `MacOSPlatform` instance.
    ///
    /// Constructs the long-lived [`SwiftBridge`] RPC client and schedules a
    /// background handshake against the Swift helper. The helper is spawned
    /// lazily on first use, so startup never blocks on IPC. Handshake results
    /// are logged at `info` on success and `warn` on failure — aleph-server
    /// continues to start even when the helper binary is missing.
    pub fn new() -> Self {
        let helper_path = resolve_helper_path();
        let bridge = Arc::new(SwiftBridge::new(helper_path));

        // Warm the bridge in the background if we are inside a Tokio runtime.
        // Falls through silently (no warm-up) in sync test/CLI contexts; the
        // first real call will still spawn the helper on demand.
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let bridge_warm = Arc::clone(&bridge);
            handle.spawn(async move {
                match bridge_warm.handshake(env!("CARGO_PKG_VERSION")).await {
                    Ok(hs) => tracing::info!(
                        protocol = hs.protocol_version,
                        methods = ?hs.supported_methods,
                        "SwiftBridge handshake complete"
                    ),
                    Err(err) => tracing::warn!(
                        error = %err,
                        "SwiftBridge handshake failed; desktop bridge operations will use disabled mode"
                    ),
                }
            });
        }

        Self {
            screen: NativeScreen::new(),
            automation: MacOSAutomation::new(),
            escape: EscapeListener::new(),
            media: MacOSMedia::new(Arc::clone(&bridge)),
            permission: MacOSPermission::new(),
            pim: MacOSPim::new(),
            system: MacOSSystem::new(),
            bridge,
        }
    }

    /// Expose the warmed bridge to Stage-1+ capabilities that need to issue
    /// RPC calls to the Swift helper.
    pub fn bridge(&self) -> Arc<SwiftBridge> {
        Arc::clone(&self.bridge)
    }
}

/// Locate the `AlephBridge` helper binary at runtime.
///
/// Resolution order:
/// 1. `ALEPH_BRIDGE_PATH` env var (explicit override).
/// 2. `$HOME/.aleph/helpers/AlephBridge` (user-level install).
/// 3. A sibling of the current executable (handy for `cargo run`).
/// 4. Repo-relative dev fallback at `desktop/macos/bridge/.build/release/AlephBridge`.
///
/// The returned path is never validated beyond the `exists()` checks in steps
/// 2 and 3. If no binary is present, `SwiftBridge` will surface a spawn error
/// on first use — the caller is expected to handle that gracefully.
fn resolve_helper_path() -> PathBuf {
    if let Ok(p) = std::env::var("ALEPH_BRIDGE_PATH") {
        return PathBuf::from(p);
    }
    if let Some(home) = dirs::home_dir() {
        let user_path = home.join(".aleph").join("helpers").join("AlephBridge");
        if user_path.exists() {
            return user_path;
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join("AlephBridge");
            if sibling.exists() {
                return sibling;
            }
        }
    }
    PathBuf::from("desktop/macos/bridge/.build/release/AlephBridge")
}

impl Default for MacOSPlatform {
    fn default() -> Self {
        Self::new()
    }
}

impl DesktopPlatform for MacOSPlatform {
    fn platform_name(&self) -> &str {
        "macOS"
    }

    fn screen(&self) -> Option<&dyn ScreenCapability> {
        Some(&self.screen)
    }

    fn pim(&self) -> Option<&dyn PimCapability> {
        Some(&self.pim)
    }

    fn system(&self) -> Option<&dyn SystemCapability> {
        Some(&self.system)
    }

    fn automation(&self) -> Option<&dyn AutomationCapability> {
        Some(&self.automation)
    }

    fn permission(&self) -> Option<&dyn PermissionCapability> {
        Some(&self.permission)
    }

    fn media(&self) -> Option<&dyn MediaCapability> {
        Some(&self.media)
    }

    fn escape_listener(&self) -> Option<&dyn EscapeAbort> {
        Some(&self.escape)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_default() {
        let platform = MacOSPlatform::default();
        assert_eq!(platform.platform_name(), "macOS");
    }

    #[test]
    fn screen_is_some() {
        let platform = MacOSPlatform::new();
        assert!(platform.screen().is_some());
        assert!(platform.pim().is_some());
        assert!(platform.system().is_some());
        assert!(platform.automation().is_some());
    }

    #[tokio::test]
    async fn construct_includes_bridge() {
        let platform = MacOSPlatform::new();
        let bridge = platform.bridge();
        // Arc is shared with the platform; strong_count should be at least 2
        // (one reference owned by `platform.bridge`, one by `bridge`).
        assert!(Arc::strong_count(&bridge) >= 2);
    }
}
