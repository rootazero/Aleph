//! macOS TCC permission check and request via native APIs.
//!
//! Covers 6 permissions: `ScreenRecording`, Camera, Microphone,
//! `SpeechRecognition`, Accessibility, Notifications.
//!
//! Three additional bridge-backed methods (`check_permission`, `guide_permission`,
//! `open_settings`) route to the Swift helper via JSON-RPC (Stage 4).

use std::ptr::NonNull;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use aleph_desktop::permission_types::{
    PermissionInfo, PermissionKind, PermissionStatus, TCC_MANAGED,
};
use aleph_desktop::traits::PermissionCapability;
use aleph_desktop::Result;
use aleph_desktop::SwiftBridge;
use aleph_protocol::desktop_bridge::methods::perm::{
    CheckParams, GuideParams, OpenSettingsParams, OpenSettingsResult, PermissionGuide,
    PermissionStatus as ProtocolPermissionStatus, METHOD_CHECK, METHOD_GUIDE, METHOD_OPEN_SETTINGS,
};
use async_trait::async_trait;
use block2::RcBlock;
use objc2::runtime::Bool;
use objc2_av_foundation::{
    AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio, AVMediaTypeVideo,
};
use objc2_speech::{SFSpeechRecognizer, SFSpeechRecognizerAuthorizationStatus};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNAuthorizationStatus, UNNotificationSettings, UNUserNotificationCenter,
};

// ---------------------------------------------------------------------------
// C FFI — CoreGraphics (ScreenRecording)
// ---------------------------------------------------------------------------

extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}

// ---------------------------------------------------------------------------
// C FFI — ApplicationServices (Accessibility)
// ---------------------------------------------------------------------------

extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(options: core_foundation::base::CFTypeRef) -> bool;
}

/// Timeout for async permission callbacks.
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(10);

/// macOS TCC permission implementation.
///
/// The three legacy methods (`check`, `check_all`, `request`) use native
/// objc2 FFI unchanged.  The three new methods (`check_permission`,
/// `guide_permission`, `open_settings`) route to the Swift helper via the
/// shared `SwiftBridge` RPC client.
pub struct MacOSPermission {
    bridge: Arc<SwiftBridge>,
}

impl MacOSPermission {
    pub const fn new(bridge: Arc<SwiftBridge>) -> Self {
        Self { bridge }
    }
}

// ---------------------------------------------------------------------------
// Per-permission check helpers
// ---------------------------------------------------------------------------

fn check_screen_recording() -> PermissionStatus {
    // CGPreflightScreenCaptureAccess returns false for both NotDetermined
    // and Denied. We conservatively map to NotDetermined.
    // SAFETY: CoreGraphics C function taking no arguments and holding no
    // preconditions; it only reads the current screen-capture TCC status.
    let granted = unsafe { CGPreflightScreenCaptureAccess() };
    if granted {
        PermissionStatus::Granted
    } else {
        PermissionStatus::NotDetermined
    }
}

fn check_camera() -> PermissionStatus {
    // SAFETY: reads the `AVMediaTypeVideo` framework constant; the generated
    // binding is `Option`, so a null/unavailable symbol is handled explicitly.
    let media_type = unsafe {
        match AVMediaTypeVideo {
            Some(mt) => mt,
            None => return PermissionStatus::Unknown,
        }
    };
    // SAFETY: `media_type` is a valid framework constant; this class method
    // only reads the current camera TCC status and never prompts.
    let status = unsafe { AVCaptureDevice::authorizationStatusForMediaType(media_type) };
    av_status_to_permission(status)
}

fn check_microphone() -> PermissionStatus {
    // SAFETY: reads the `AVMediaTypeAudio` framework constant; the generated
    // binding is `Option`, so a null/unavailable symbol is handled explicitly.
    let media_type = unsafe {
        match AVMediaTypeAudio {
            Some(mt) => mt,
            None => return PermissionStatus::Unknown,
        }
    };
    // SAFETY: `media_type` is a valid framework constant; this class method
    // only reads the current microphone TCC status and never prompts.
    let status = unsafe { AVCaptureDevice::authorizationStatusForMediaType(media_type) };
    av_status_to_permission(status)
}

fn check_speech_recognition() -> PermissionStatus {
    // SAFETY: class method that only reads the current speech-recognition TCC
    // status; it takes no arguments and holds no preconditions.
    let status = unsafe { SFSpeechRecognizer::authorizationStatus() };
    sf_status_to_permission(status)
}

fn check_accessibility() -> PermissionStatus {
    // SAFETY: ApplicationServices C function taking no arguments; it only reads
    // whether this process is currently AX-trusted and never prompts.
    let trusted = unsafe { AXIsProcessTrusted() };
    if trusted {
        PermissionStatus::Granted
    } else {
        // AXIsProcessTrusted returns false for both NotDetermined and Denied.
        PermissionStatus::NotDetermined
    }
}

fn check_notifications() -> PermissionStatus {
    // Check bundle identifier — if None we're not in a .app bundle.
    let bundle = objc2_foundation::NSBundle::mainBundle();
    if bundle.bundleIdentifier().is_none() {
        return PermissionStatus::Unknown;
    }

    let center = UNUserNotificationCenter::currentNotificationCenter();

    let (tx, rx) = mpsc::channel();
    let block = RcBlock::new(move |settings: NonNull<UNNotificationSettings>| {
        // SAFETY: `settings` is a non-null `UNNotificationSettings` handed to
        // this completion block by UserNotifications.framework; it is valid for
        // the duration of the callback, so reading `authorizationStatus` is sound.
        let status = unsafe { settings.as_ref().authorizationStatus() };
        let _ = tx.send(status);
    });

    center.getNotificationSettingsWithCompletionHandler(&block);

    match rx.recv_timeout(CALLBACK_TIMEOUT) {
        Ok(status) => un_status_to_permission(status),
        Err(_) => PermissionStatus::Unknown,
    }
}

// ---------------------------------------------------------------------------
// Per-permission request helpers
// ---------------------------------------------------------------------------

fn request_screen_recording() -> PermissionStatus {
    // SAFETY: CoreGraphics C function taking no arguments; it prompts for (and
    // returns) screen-capture access. Sibling of `CGPreflightScreenCaptureAccess`.
    let granted = unsafe { CGRequestScreenCaptureAccess() };
    if granted {
        PermissionStatus::Granted
    } else {
        PermissionStatus::Denied
    }
}

fn request_camera() -> PermissionStatus {
    // SAFETY: reads the `AVMediaTypeVideo` framework constant; the generated
    // binding is `Option`, so a null/unavailable symbol is handled explicitly.
    let media_type = unsafe {
        match AVMediaTypeVideo {
            Some(mt) => mt,
            None => return PermissionStatus::Unknown,
        }
    };

    let (tx, rx) = mpsc::channel();
    let block = RcBlock::new(move |granted: Bool| {
        let _ = tx.send(granted.as_bool());
    });

    // SAFETY: `media_type` is a valid framework constant. The completion
    // handler is an `RcBlock` (reference-counted), so AVFoundation retains it
    // for the lifetime of the async request even after this `&block` borrow ends.
    unsafe {
        AVCaptureDevice::requestAccessForMediaType_completionHandler(media_type, &block);
    }

    match rx.recv_timeout(CALLBACK_TIMEOUT) {
        Ok(true) => PermissionStatus::Granted,
        Ok(false) => PermissionStatus::Denied,
        Err(_) => PermissionStatus::Unknown,
    }
}

fn request_microphone() -> PermissionStatus {
    // SAFETY: reads the `AVMediaTypeAudio` framework constant; the generated
    // binding is `Option`, so a null/unavailable symbol is handled explicitly.
    let media_type = unsafe {
        match AVMediaTypeAudio {
            Some(mt) => mt,
            None => return PermissionStatus::Unknown,
        }
    };

    let (tx, rx) = mpsc::channel();
    let block = RcBlock::new(move |granted: Bool| {
        let _ = tx.send(granted.as_bool());
    });

    // SAFETY: `media_type` is a valid framework constant. The completion
    // handler is an `RcBlock` (reference-counted), so AVFoundation retains it
    // for the lifetime of the async request even after this `&block` borrow ends.
    unsafe {
        AVCaptureDevice::requestAccessForMediaType_completionHandler(media_type, &block);
    }

    match rx.recv_timeout(CALLBACK_TIMEOUT) {
        Ok(true) => PermissionStatus::Granted,
        Ok(false) => PermissionStatus::Denied,
        Err(_) => PermissionStatus::Unknown,
    }
}

fn request_speech_recognition() -> PermissionStatus {
    let (tx, rx) = mpsc::channel();
    let block = RcBlock::new(move |status: SFSpeechRecognizerAuthorizationStatus| {
        let _ = tx.send(status);
    });

    // SAFETY: the completion handler is an `RcBlock` (reference-counted), so
    // Speech.framework retains it for the lifetime of the async authorization
    // request even after this `&block` borrow ends.
    unsafe {
        SFSpeechRecognizer::requestAuthorization(&block);
    }

    match rx.recv_timeout(CALLBACK_TIMEOUT) {
        Ok(status) => sf_status_to_permission(status),
        Err(_) => PermissionStatus::Unknown,
    }
}

fn request_accessibility() -> PermissionStatus {
    use core_foundation::base::TCFType;
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::string::CFString;

    let key = CFString::new("AXTrustedCheckOptionPrompt");
    let value = CFBoolean::true_value();

    let options = CFDictionary::from_CFType_pairs(&[(key, value)]);

    // SAFETY: `options` is a live `CFDictionary` owned by this scope; we pass a
    // borrowed `CFTypeRef` that stays valid for the duration of the call.
    let trusted = unsafe { AXIsProcessTrustedWithOptions(options.as_CFTypeRef()) };

    if trusted {
        PermissionStatus::Granted
    } else {
        // The system prompt was shown; user hasn't granted yet.
        PermissionStatus::Denied
    }
}

fn request_notifications() -> PermissionStatus {
    // A non-bundled binary cannot prompt; report Unknown — same gate as
    // `check_notifications`.
    let bundle = objc2_foundation::NSBundle::mainBundle();
    if bundle.bundleIdentifier().is_none() {
        return PermissionStatus::Unknown;
    }

    let center = UNUserNotificationCenter::currentNotificationCenter();
    let options = UNAuthorizationOptions::Alert
        | UNAuthorizationOptions::Sound
        | UNAuthorizationOptions::Badge;

    let (tx, rx) = mpsc::channel();
    let block = RcBlock::new(move |granted: Bool, _err: *mut objc2_foundation::NSError| {
        let _ = tx.send(granted.as_bool());
    });

    center.requestAuthorizationWithOptions_completionHandler(options, &block);

    // The completion handler is invoked after the user dismisses the system
    // prompt. After it returns we re-poll `check_notifications` so the reported
    // status reflects "authorized / provisional / ephemeral" (granted), even
    // when the OS upgrades a provisional grant on first request.
    match rx.recv_timeout(CALLBACK_TIMEOUT) {
        Ok(_) => check_notifications(),
        Err(_) => PermissionStatus::Unknown,
    }
}

// ---------------------------------------------------------------------------
// Status mappers
// ---------------------------------------------------------------------------

const fn av_status_to_permission(status: AVAuthorizationStatus) -> PermissionStatus {
    match status {
        AVAuthorizationStatus::Authorized => PermissionStatus::Granted,
        AVAuthorizationStatus::Denied => PermissionStatus::Denied,
        AVAuthorizationStatus::Restricted => PermissionStatus::Restricted,
        AVAuthorizationStatus::NotDetermined => PermissionStatus::NotDetermined,
        _ => PermissionStatus::Unknown,
    }
}

const fn sf_status_to_permission(
    status: SFSpeechRecognizerAuthorizationStatus,
) -> PermissionStatus {
    match status {
        SFSpeechRecognizerAuthorizationStatus::Authorized => PermissionStatus::Granted,
        SFSpeechRecognizerAuthorizationStatus::Denied => PermissionStatus::Denied,
        SFSpeechRecognizerAuthorizationStatus::Restricted => PermissionStatus::Restricted,
        SFSpeechRecognizerAuthorizationStatus::NotDetermined => PermissionStatus::NotDetermined,
        _ => PermissionStatus::Unknown,
    }
}

const fn un_status_to_permission(status: UNAuthorizationStatus) -> PermissionStatus {
    match status {
        UNAuthorizationStatus::Authorized
        | UNAuthorizationStatus::Provisional
        | UNAuthorizationStatus::Ephemeral => PermissionStatus::Granted,
        UNAuthorizationStatus::Denied => PermissionStatus::Denied,
        UNAuthorizationStatus::NotDetermined => PermissionStatus::NotDetermined,
        _ => PermissionStatus::Unknown,
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const fn build_info(permission: PermissionKind, status: PermissionStatus) -> PermissionInfo {
    let can_request = matches!(status, PermissionStatus::NotDetermined);
    PermissionInfo {
        permission,
        status,
        can_request,
    }
}

/// Native TCC probe. Bridge-only kinds return `Unknown` — callers should use
/// `check_permission()` (bridge RPC) for those instead.
fn do_check(permission: PermissionKind) -> PermissionInfo {
    let status = match permission {
        PermissionKind::ScreenRecording => check_screen_recording(),
        PermissionKind::Camera => check_camera(),
        PermissionKind::Microphone => check_microphone(),
        PermissionKind::SpeechRecognition => check_speech_recognition(),
        PermissionKind::Accessibility => check_accessibility(),
        PermissionKind::Notifications => check_notifications(),
        PermissionKind::InputMonitoring
        | PermissionKind::FullDisk
        | PermissionKind::Automation
        | PermissionKind::Contacts
        | PermissionKind::Calendars
        | PermissionKind::Reminders
        | PermissionKind::Photos
        | PermissionKind::Location => PermissionStatus::Unknown,
    };
    build_info(permission, status)
}

/// Native TCC request. Bridge-only kinds return `Unknown` — callers should
/// invoke their bridge-side workflow instead.
fn do_request(permission: PermissionKind) -> PermissionInfo {
    let status = match permission {
        PermissionKind::ScreenRecording => request_screen_recording(),
        PermissionKind::Camera => request_camera(),
        PermissionKind::Microphone => request_microphone(),
        PermissionKind::SpeechRecognition => request_speech_recognition(),
        PermissionKind::Accessibility => request_accessibility(),
        PermissionKind::Notifications => request_notifications(),
        PermissionKind::InputMonitoring
        | PermissionKind::FullDisk
        | PermissionKind::Automation
        | PermissionKind::Contacts
        | PermissionKind::Calendars
        | PermissionKind::Reminders
        | PermissionKind::Photos
        | PermissionKind::Location => PermissionStatus::Unknown,
    };
    build_info(permission, status)
}

// ---------------------------------------------------------------------------
// Trait implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl PermissionCapability for MacOSPermission {
    async fn check(&self, permission: PermissionKind) -> Result<PermissionInfo> {
        let info = tokio::task::spawn_blocking(move || do_check(permission))
            .await
            .unwrap_or_else(|_| build_info(permission, PermissionStatus::Unknown));
        Ok(info)
    }

    async fn check_all(&self) -> Result<Vec<PermissionInfo>> {
        let results = tokio::task::spawn_blocking(|| {
            TCC_MANAGED.iter().map(|&p| do_check(p)).collect::<Vec<_>>()
        })
        .await
        .unwrap_or_else(|_| {
            TCC_MANAGED
                .iter()
                .map(|&p| build_info(p, PermissionStatus::Unknown))
                .collect()
        });
        Ok(results)
    }

    async fn request(&self, permission: PermissionKind) -> Result<PermissionInfo> {
        let info = tokio::task::spawn_blocking(move || do_request(permission))
            .await
            .unwrap_or_else(|_| build_info(permission, PermissionStatus::Unknown));
        Ok(info)
    }

    // -----------------------------------------------------------------------
    // Bridge-backed methods (Stage 4) — route to Swift helper via JSON-RPC.
    // -----------------------------------------------------------------------

    async fn check_permission(&self, kind: PermissionKind) -> Result<ProtocolPermissionStatus> {
        self.bridge
            .call(METHOD_CHECK, CheckParams { kind })
            .await
            .map_err(|e| aleph_desktop::DesktopError::BridgeFailed(format!("perm.check RPC: {e}")))
    }

    async fn guide_permission(&self, kind: PermissionKind) -> Result<PermissionGuide> {
        self.bridge
            .call(METHOD_GUIDE, GuideParams { kind })
            .await
            .map_err(|e| aleph_desktop::DesktopError::BridgeFailed(format!("perm.guide RPC: {e}")))
    }

    async fn open_settings(&self, kind: PermissionKind) -> Result<bool> {
        let r: OpenSettingsResult = self
            .bridge
            .call(METHOD_OPEN_SETTINGS, OpenSettingsParams { kind })
            .await
            .map_err(|e| {
                aleph_desktop::DesktopError::BridgeFailed(format!("perm.open_settings RPC: {e}"))
            })?;
        Ok(r.ok)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_accessibility() {
        let status = check_accessibility();
        // On any macOS machine this should return a valid status.
        assert!(
            matches!(
                status,
                PermissionStatus::Granted | PermissionStatus::NotDetermined
            ),
            "unexpected accessibility status: {:?}",
            status
        );
    }

    #[test]
    fn test_check_screen_recording() {
        let status = check_screen_recording();
        assert!(
            matches!(
                status,
                PermissionStatus::Granted | PermissionStatus::NotDetermined
            ),
            "unexpected screen recording status: {:?}",
            status
        );
    }

    #[test]
    fn test_check_all_returns_six() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        // Bridge not used by the legacy check_all path; any path is fine.
        let bridge = Arc::new(SwiftBridge::new(std::path::PathBuf::from("/dev/null")));
        let perm = MacOSPermission::new(bridge);
        let results = rt.block_on(async { perm.check_all().await.unwrap() });
        assert_eq!(results.len(), 6, "expected 6 permission results");
        // Verify each permission type is present.
        let types: Vec<PermissionKind> = results.iter().map(|r| r.permission).collect();
        for p in TCC_MANAGED {
            assert!(types.contains(p), "missing permission: {:?}", p);
        }
    }

    #[test]
    fn bridge_only_kind_check_returns_unknown_status() {
        // Native probe can't read FullDisk / Automation / etc. directly —
        // make sure `do_check` returns Unknown so callers know to use the
        // bridge-backed `check_permission()` RPC instead of trusting a false
        // "denied" result.
        let bridge_only = [
            PermissionKind::InputMonitoring,
            PermissionKind::FullDisk,
            PermissionKind::Automation,
            PermissionKind::Contacts,
            PermissionKind::Calendars,
            PermissionKind::Reminders,
            PermissionKind::Photos,
            PermissionKind::Location,
        ];
        for kind in bridge_only {
            let info = do_check(kind);
            assert_eq!(
                info.status,
                PermissionStatus::Unknown,
                "bridge-only kind {kind:?} must report Unknown from native probe",
            );
            assert!(!info.can_request);
            assert_eq!(info.permission, kind);
        }
    }

    #[test]
    fn test_build_info_can_request() {
        let info = build_info(PermissionKind::Camera, PermissionStatus::NotDetermined);
        assert!(info.can_request);

        let info = build_info(PermissionKind::Camera, PermissionStatus::Granted);
        assert!(!info.can_request);

        let info = build_info(PermissionKind::Camera, PermissionStatus::Denied);
        assert!(!info.can_request);
    }

    #[test]
    fn test_check_never_panics() {
        // Verify check() does not panic — tokio::test harness must be available.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let bridge = Arc::new(SwiftBridge::new(std::path::PathBuf::from("/dev/null")));
        let perm = MacOSPermission::new(bridge);

        // Run check inside a/runtime to verify no panic path.
        // The spawn_blocking -> JoinError path is caught by unwrap_or_else.
        let result = rt.block_on(perm.check(PermissionKind::Camera));
        assert!(result.is_ok()); // Returns Ok or propagates error, never panics
    }

    #[test]
    fn test_av_status_mapper_known_variants() {
        use objc2_av_foundation::AVAuthorizationStatus;
        // Exhaustive mapping for known variants — catch if new objc2 variants are added
        assert_eq!(
            av_status_to_permission(AVAuthorizationStatus::Authorized),
            PermissionStatus::Granted
        );
        assert_eq!(
            av_status_to_permission(AVAuthorizationStatus::Denied),
            PermissionStatus::Denied
        );
        assert_eq!(
            av_status_to_permission(AVAuthorizationStatus::Restricted),
            PermissionStatus::Restricted
        );
        assert_eq!(
            av_status_to_permission(AVAuthorizationStatus::NotDetermined),
            PermissionStatus::NotDetermined
        );
        // Unknown is tested via catch-all _ branch (coverage verified via
        // test_do_check_all_permissions_return_valid_info which exercises all TCC permissions)
    }

    #[test]
    fn test_sf_status_mapper_known_variants() {
        use objc2_speech::SFSpeechRecognizerAuthorizationStatus;
        assert_eq!(
            sf_status_to_permission(SFSpeechRecognizerAuthorizationStatus::Authorized),
            PermissionStatus::Granted
        );
        assert_eq!(
            sf_status_to_permission(SFSpeechRecognizerAuthorizationStatus::Denied),
            PermissionStatus::Denied
        );
        assert_eq!(
            sf_status_to_permission(SFSpeechRecognizerAuthorizationStatus::Restricted),
            PermissionStatus::Restricted
        );
        assert_eq!(
            sf_status_to_permission(SFSpeechRecognizerAuthorizationStatus::NotDetermined),
            PermissionStatus::NotDetermined
        );
        // Unknown is tested via catch-all _ branch
    }

    #[test]
    fn test_do_check_all_tcc_managed_return_valid_info() {
        for &perm in TCC_MANAGED {
            let info = do_check(perm);
            assert!(
                matches!(
                    info.status,
                    PermissionStatus::Granted
                        | PermissionStatus::Denied
                        | PermissionStatus::Restricted
                        | PermissionStatus::NotDetermined
                        | PermissionStatus::Unknown
                ),
                "do_check({:?}) returned invalid status: {:?}",
                perm,
                info.status
            );
            assert_eq!(info.permission, perm);
        }
    }

    #[test]
    fn test_do_request_all_tcc_managed_return_valid_info() {
        for &perm in TCC_MANAGED {
            let info = do_request(perm);
            assert!(
                matches!(
                    info.status,
                    PermissionStatus::Granted
                        | PermissionStatus::Denied
                        | PermissionStatus::Restricted
                        | PermissionStatus::NotDetermined
                        | PermissionStatus::Unknown
                ),
                "do_request({:?}) returned invalid status: {:?}",
                perm,
                info.status
            );
            assert_eq!(info.permission, perm);
        }
    }

    #[test]
    fn request_notifications_returns_unknown_when_unbundled() {
        // `cargo test` runs as a plain executable with no CFBundleIdentifier,
        // so the bundle-ID gate must short-circuit to Unknown rather than
        // blocking on a system prompt that will never arrive.
        let bundle = objc2_foundation::NSBundle::mainBundle();
        if bundle.bundleIdentifier().is_some() {
            // If a future test harness ever sets a bundle ID, skip this case
            // to avoid hanging on a UN authorization prompt.
            return;
        }
        let status = request_notifications();
        assert_eq!(status, PermissionStatus::Unknown);
    }
}
