//! macOS platform implementation for Aleph desktop capabilities.

mod automation;
mod ax;
mod escape_listener;
pub mod hotkey;
mod permission;
mod pim;
mod screen;
mod system;

use std::path::PathBuf;
use std::sync::Arc;

use aleph_desktop::media_types::{
    AudioDeviceInfo, AudioRecordConfig, AudioRecordResult, CameraClipConfig, CameraClipResult,
    CameraSnapConfig, CameraSnapResult, SpeechToTextConfig, SpeechToTextResult,
};
use aleph_desktop::platform::EscapeAbort;
use aleph_desktop::traits::{
    AccessibilityCapability, AutomationCapability, MediaCapability, PermissionCapability,
    PimCapability, ScreenCapability, SystemCapability,
};
use aleph_desktop::DesktopPlatform;
use aleph_desktop::Result;
use aleph_desktop::SwiftBridge;
use async_trait::async_trait;
use tracing::debug;

use automation::MacOSAutomation;
use ax::BridgeAccessibility;
use escape_listener::EscapeListener;
use permission::MacOSPermission;
use pim::MacOSPim;
use screen::MacOSScreen;
use system::MacOSSystem;

/// macOS platform with bridge-connected `MacOSScreen` for screen capabilities.
pub struct MacOSPlatform {
    screen: MacOSScreen,
    automation: MacOSAutomation,
    escape: EscapeListener,
    permission: MacOSPermission,
    pim: MacOSPim,
    system: MacOSSystem,
    ax: BridgeAccessibility,
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
            screen: MacOSScreen::new(Arc::clone(&bridge)),
            automation: MacOSAutomation::new(),
            escape: EscapeListener::new(),
            permission: MacOSPermission::new(),
            pim: MacOSPim::new(),
            system: MacOSSystem::new(),
            ax: BridgeAccessibility::new(Arc::clone(&bridge)),
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
        Some(self)
    }

    fn ax(&self) -> Option<&dyn AccessibilityCapability> {
        Some(&self.ax)
    }

    fn escape_listener(&self) -> Option<&dyn EscapeAbort> {
        Some(&self.escape)
    }
}

/// Shorthand for creating a BridgeFailed error.
fn bridge_err(msg: &str) -> aleph_desktop::DesktopError {
    aleph_desktop::DesktopError::BridgeFailed(msg.to_string())
}

// ---------------------------------------------------------------------------
// MediaCapability: camera + audio + speech proxied via the Swift helper.
// All method bodies are thin RPC forwarders — no native AVFoundation /
// SFSpeechRecognizer code lives on the Rust side as of Stage 1c.
// ---------------------------------------------------------------------------
#[async_trait]
impl MediaCapability for MacOSPlatform {
    async fn camera_snap(&self, config: CameraSnapConfig) -> Result<CameraSnapResult> {
        use aleph_protocol::desktop_bridge::methods::media::{
            SnapParams, SnapResult, METHOD_CAMERA_SNAP,
        };
        let config = config.clamped();
        debug!(quality = config.quality, "Proxying camera snap to Swift helper");
        let rpc: SnapResult = self
            .bridge
            .call(
                METHOD_CAMERA_SNAP,
                SnapParams {
                    quality: config.quality,
                },
            )
            .await
            .map_err(|e| bridge_err(&format!("media.camera.snap RPC: {e}")))?;
        Ok(CameraSnapResult {
            image_base64: rpc.image_base64,
            width: rpc.width,
            height: rpc.height,
        })
    }

    async fn camera_clip(&self, config: CameraClipConfig) -> Result<CameraClipResult> {
        use aleph_protocol::desktop_bridge::methods::media::{
            ClipParams, ClipResult, METHOD_CAMERA_CLIP,
        };
        let config = config.clamped();
        debug!(
            duration = config.duration_secs,
            audio = config.with_audio,
            "Proxying camera clip to Swift helper"
        );
        let rpc: ClipResult = self
            .bridge
            .call(
                METHOD_CAMERA_CLIP,
                ClipParams {
                    duration_secs: config.duration_secs,
                    with_audio: config.with_audio,
                },
            )
            .await
            .map_err(|e| bridge_err(&format!("media.camera.clip RPC: {e}")))?;
        Ok(CameraClipResult {
            file_path: rpc.file_path,
            duration_secs: rpc.duration_secs,
            has_audio: rpc.has_audio,
        })
    }

    async fn record_audio(&self, config: AudioRecordConfig) -> Result<AudioRecordResult> {
        use aleph_protocol::desktop_bridge::methods::media::{
            RecordAudioParams, RecordAudioResult, METHOD_AUDIO_RECORD,
        };
        let config = config.clamped();
        debug!(
            duration = config.duration_secs,
            "Proxying audio record to Swift helper"
        );
        let rpc: RecordAudioResult = self
            .bridge
            .call(
                METHOD_AUDIO_RECORD,
                RecordAudioParams {
                    duration_secs: config.duration_secs,
                },
            )
            .await
            .map_err(|e| bridge_err(&format!("media.audio.record RPC: {e}")))?;
        Ok(AudioRecordResult {
            file_path: rpc.file_path,
            duration_secs: rpc.duration_secs,
            format: rpc.format,
        })
    }

    async fn list_audio_devices(&self) -> Result<Vec<AudioDeviceInfo>> {
        use aleph_protocol::desktop_bridge::methods::media::{
            ListAudioDevicesParams, ListAudioDevicesResult, METHOD_AUDIO_LIST_DEVICES,
        };
        debug!("Proxying audio list_devices to Swift helper");
        let rpc: ListAudioDevicesResult = self
            .bridge
            .call(METHOD_AUDIO_LIST_DEVICES, ListAudioDevicesParams {})
            .await
            .map_err(|e| bridge_err(&format!("media.audio.list_devices RPC: {e}")))?;
        Ok(rpc
            .devices
            .into_iter()
            .map(|d| AudioDeviceInfo {
                uid: d.uid,
                name: d.name,
                is_input: d.is_input,
                is_default: d.is_default,
            })
            .collect())
    }

    async fn speech_to_text(
        &self,
        audio_path: &str,
        config: SpeechToTextConfig,
    ) -> Result<SpeechToTextResult> {
        use aleph_protocol::desktop_bridge::methods::media::{
            TranscribeFileParams, TranscribeFileResult, METHOD_SPEECH_TRANSCRIBE_FILE,
        };
        debug!(
            path = audio_path,
            lang = %config.language,
            "Proxying speech transcribe_file to Swift helper"
        );
        let rpc: TranscribeFileResult = self
            .bridge
            .call(
                METHOD_SPEECH_TRANSCRIBE_FILE,
                TranscribeFileParams {
                    audio_path: audio_path.to_string(),
                    language: config.language.clone(),
                },
            )
            .await
            .map_err(|e| bridge_err(&format!("media.speech.transcribe_file RPC: {e}")))?;
        Ok(SpeechToTextResult {
            text: rpc.text,
            language: rpc.language,
        })
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
