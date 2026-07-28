//! macOS platform implementation for Aleph desktop capabilities.

mod automation;
mod ax;
mod escape_listener;
mod permission;
mod pim;
mod screen;
mod sleep_inhibitor;
mod system;

pub use sleep_inhibitor::MacosPower;
// Exposed for `tests/escape_listener_e2e.rs`, which proves the abort key is
// actually observed — the property that silently failed under `NSEvent`.
pub use escape_listener::EscapeListener;

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use aleph_desktop::media_types::{
    AudioDeviceInfo, AudioRecordConfig, AudioRecordResult, CameraClipConfig, CameraClipResult,
    CameraSnapConfig, CameraSnapResult, SpeechToTextConfig, SpeechToTextResult,
};
use aleph_desktop::platform::EscapeAbort;
use aleph_desktop::traits::{
    AccessibilityCapability, AutomationCapability, MediaCapability, PermissionCapability,
    PimCapability, PowerCapability, ScreenCapability, SystemCapability,
};
use aleph_desktop::DesktopPlatform;
use aleph_desktop::Result;
use aleph_desktop::SwiftBridge;
use async_trait::async_trait;
use tracing::debug;

use automation::MacOSAutomation;
use ax::BridgeAccessibility;
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
    power: MacosPower,
    bridge: Arc<SwiftBridge>,
}

/// Process-wide shared Swift bridge client.
///
/// Every `MacOSPlatform` constructed in this process shares one `SwiftBridge`,
/// so the daemon spawns at most a single `AlephBridge` child regardless of how
/// many subsystems (presence reporter, builtin tool registry, voice handler …)
/// build a platform handle. The client is lazy: the child process is not
/// spawned until the first real `desktop.*` call.
static SHARED_BRIDGE: OnceLock<Arc<SwiftBridge>> = OnceLock::new();

fn shared_bridge() -> Arc<SwiftBridge> {
    SHARED_BRIDGE
        .get_or_init(|| Arc::new(SwiftBridge::new(resolve_helper_path())))
        .clone()
}

impl MacOSPlatform {
    /// Create a new `MacOSPlatform` instance.
    ///
    /// Returns a handle backed by the process-wide [`SwiftBridge`] singleton.
    /// Multiple platform handles in the same process share one bridge client,
    /// so the daemon spawns at most a single `AlephBridge` child. The helper
    /// is started lazily on the first real `desktop.*` call — construction
    /// never forks a child process.
    #[must_use]
    pub fn new() -> Self {
        // Shared, process-wide bridge. No construction-time warm-up: the helper
        // is spawned lazily by `ensure_running` on the first real desktop call,
        // so subsystems that never touch the bridge (e.g. the presence reporter,
        // which only uses `system()`) do not fork an `AlephBridge` child.
        let bridge = shared_bridge();

        Self {
            screen: MacOSScreen::new(Arc::clone(&bridge)),
            automation: MacOSAutomation::new(),
            escape: EscapeListener::new(),
            permission: MacOSPermission::new(Arc::clone(&bridge)),
            pim: MacOSPim::new(Arc::clone(&bridge)),
            system: MacOSSystem::new(),
            ax: BridgeAccessibility::new(Arc::clone(&bridge)),
            power: MacosPower::new(),
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
    fn platform_name(&self) -> &'static str {
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

    fn power(&self) -> Option<&dyn PowerCapability> {
        Some(&self.power)
    }

    fn escape_listener(&self) -> Option<&dyn EscapeAbort> {
        Some(&self.escape)
    }
}

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

/// Extra time beyond a capture's requested duration to allow for capture-
/// session warm-up, encoding, and disk I/O before the RPC is treated as hung.
/// Added on top of `duration_secs` for `camera.clip` / `audio.record`, whose
/// recordings outlast [`SwiftBridge::call`]'s default deadline.
const CAPTURE_TIMEOUT_MARGIN_SECS: f64 = 30.0;

/// Deadline for on-device speech transcription. The source file length is not
/// known to the Rust side, so this is bounded generously rather than derived.
const SPEECH_TRANSCRIBE_TIMEOUT: Duration = Duration::from_mins(5);

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
        debug!(
            quality = config.quality,
            "Proxying camera snap to Swift helper"
        );
        let rpc: SnapResult = self
            .bridge
            .call(
                METHOD_CAMERA_SNAP,
                SnapParams {
                    quality: config.quality,
                },
            )
            .await
            .map_err(|e| preserve_typed("media.camera.snap", e))?;
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
        // The helper records for `duration_secs`, which outlasts the default
        // RPC deadline; bound the call by the recording duration plus margin.
        // `duration_secs` is pre-clamped by `.clamped()`, so the sum is finite.
        let rpc: ClipResult = self
            .bridge
            .call_with_timeout(
                METHOD_CAMERA_CLIP,
                ClipParams {
                    duration_secs: config.duration_secs,
                    with_audio: config.with_audio,
                },
                Duration::from_secs_f64(config.duration_secs + CAPTURE_TIMEOUT_MARGIN_SECS),
            )
            .await
            .map_err(|e| preserve_typed("media.camera.clip", e))?;
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
        // Recording outlasts the default RPC deadline; bound the call by the
        // requested duration plus margin. `duration_secs` is pre-clamped.
        let rpc: RecordAudioResult = self
            .bridge
            .call_with_timeout(
                METHOD_AUDIO_RECORD,
                RecordAudioParams {
                    duration_secs: config.duration_secs,
                },
                Duration::from_secs_f64(config.duration_secs + CAPTURE_TIMEOUT_MARGIN_SECS),
            )
            .await
            .map_err(|e| preserve_typed("media.audio.record", e))?;
        Ok(AudioRecordResult {
            file_path: rpc.file_path,
            duration_secs: rpc.duration_secs,
            format: rpc.format,
        })
    }

    async fn record_audio_start(&self) -> Result<()> {
        use aleph_protocol::desktop_bridge::methods::media::{
            RecordStartParams, RecordStartResult, METHOD_AUDIO_RECORD_START,
        };
        debug!("Proxying audio record_start to Swift helper");
        let _: RecordStartResult = self
            .bridge
            .call(METHOD_AUDIO_RECORD_START, RecordStartParams {})
            .await
            .map_err(|e| preserve_typed("media.audio.record_start", e))?;
        Ok(())
    }

    async fn record_audio_stop(&self) -> Result<AudioRecordResult> {
        use aleph_protocol::desktop_bridge::methods::media::{
            RecordAudioResult, RecordStopParams, METHOD_AUDIO_RECORD_STOP,
        };
        debug!("Proxying audio record_stop to Swift helper");
        // The helper finalises the encoder before replying, which is why this
        // gets its own (longer) budget — but that budget is the protocol's to
        // state, not this limb's to re-spell. It used to be a bare
        // `Duration::from_secs(15)` here, i.e. a second copy of a number the
        // protocol had no idea about.
        let rpc: RecordAudioResult = self
            .bridge
            .call(METHOD_AUDIO_RECORD_STOP, RecordStopParams {})
            .await
            .map_err(|e| preserve_typed("media.audio.record_stop", e))?;
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
            .map_err(|e| preserve_typed("media.audio.list_devices", e))?;
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

    async fn mic_level(&self) -> Result<aleph_desktop::traits::media::MicMeterSample> {
        use aleph_protocol::desktop_bridge::methods::media::{
            MicMeterParams, MicMeterResult, METHOD_AUDIO_MIC_METER,
        };
        let rpc: MicMeterResult = self
            .bridge
            .call(METHOD_AUDIO_MIC_METER, MicMeterParams {})
            .await
            .map_err(|e| preserve_typed("media.audio.mic_meter", e))?;
        Ok(aleph_desktop::traits::media::MicMeterSample {
            level: rpc.level,
            active: rpc.active,
            reason: rpc.reason,
        })
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
        // Transcription of a long file can outlast the default deadline;
        // bound it generously since the file length is not known here.
        let rpc: TranscribeFileResult = self
            .bridge
            .call_with_timeout(
                METHOD_SPEECH_TRANSCRIBE_FILE,
                TranscribeFileParams {
                    audio_path: audio_path.to_string(),
                    language: config.language.clone(),
                },
                SPEECH_TRANSCRIBE_TIMEOUT,
            )
            .await
            .map_err(|e| preserve_typed("media.speech.transcribe_file", e))?;
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

    #[test]
    fn platforms_share_one_bridge() {
        let a = MacOSPlatform::new();
        let b = MacOSPlatform::new();
        assert!(
            Arc::ptr_eq(&a.bridge(), &b.bridge()),
            "all platforms must share the process-wide singleton bridge"
        );
    }

    #[tokio::test]
    async fn construction_does_not_spawn_bridge() {
        // No warm-up handshake at construction: the helper stays unspawned
        // until the first real desktop call. Guards against re-introducing an
        // eager warm-up that would fork a child process at construction.
        let platform = MacOSPlatform::new();
        assert!(
            !platform.bridge().is_running(),
            "constructing a platform must not spawn the bridge"
        );
    }

    #[test]
    fn preserve_typed_passes_timeout_through() {
        use aleph_desktop::DesktopError;
        let e = DesktopError::BridgeTimeout("slow".into());
        assert!(matches!(
            preserve_typed("media.audio.record", e),
            DesktopError::BridgeTimeout(_)
        ));
    }

    #[test]
    fn preserve_typed_decorates_bridge_failed_with_method() {
        use aleph_desktop::DesktopError;
        match preserve_typed(
            "media.camera.snap",
            DesktopError::BridgeFailed("boom".into()),
        ) {
            DesktopError::BridgeFailed(m) => assert_eq!(m, "media.camera.snap: boom"),
            other => panic!("unexpected variant: {other:?}"),
        }
    }
}
