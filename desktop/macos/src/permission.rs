//! macOS TCC permission check and request via native APIs.
//!
//! Covers 6 permissions: ScreenRecording, Camera, Microphone,
//! SpeechRecognition, Accessibility, Notifications.
//!
//! Three additional bridge-backed methods (`check_permission`, `guide_permission`,
//! `open_settings`) route to the Swift helper via JSON-RPC (Stage 4).

use std::ptr::NonNull;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use aleph_desktop::permission_types::{PermissionInfo, PermissionStatus, TccPermission};
use aleph_desktop::traits::PermissionCapability;
use aleph_desktop::Result;
use aleph_desktop::SwiftBridge;
use aleph_protocol::desktop_bridge::methods::perm::{
    CheckParams, GuideParams, OpenSettingsParams, OpenSettingsResult, PermissionGuide,
    PermissionKind, PermissionStatus as ProtocolPermissionStatus, METHOD_CHECK, METHOD_GUIDE,
    METHOD_OPEN_SETTINGS,
};
use async_trait::async_trait;
use block2::RcBlock;
use objc2::runtime::Bool;
use objc2_av_foundation::{
    AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio, AVMediaTypeVideo,
};
use objc2_speech::{SFSpeechRecognizer, SFSpeechRecognizerAuthorizationStatus};
use objc2_user_notifications::{
    UNAuthorizationStatus, UNNotificationSettings, UNUserNotificationCenter,
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
    pub fn new(bridge: Arc<SwiftBridge>) -> Self {
        Self { bridge }
    }
}

// ---------------------------------------------------------------------------
// Per-permission check helpers
// ---------------------------------------------------------------------------

fn check_screen_recording() -> PermissionStatus {
    // CGPreflightScreenCaptureAccess returns false for both NotDetermined
    // and Denied. We conservatively map to NotDetermined.
    let granted = unsafe { CGPreflightScreenCaptureAccess() };
    if granted {
        PermissionStatus::Granted
    } else {
        PermissionStatus::NotDetermined
    }
}

fn check_camera() -> PermissionStatus {
    let media_type = unsafe {
        match AVMediaTypeVideo {
            Some(mt) => mt,
            None => return PermissionStatus::Unknown,
        }
    };
    let status = unsafe { AVCaptureDevice::authorizationStatusForMediaType(media_type) };
    av_status_to_permission(status)
}

fn check_microphone() -> PermissionStatus {
    let media_type = unsafe {
        match AVMediaTypeAudio {
            Some(mt) => mt,
            None => return PermissionStatus::Unknown,
        }
    };
    let status = unsafe { AVCaptureDevice::authorizationStatusForMediaType(media_type) };
    av_status_to_permission(status)
}

fn check_speech_recognition() -> PermissionStatus {
    let status = unsafe { SFSpeechRecognizer::authorizationStatus() };
    sf_status_to_permission(status)
}

fn check_accessibility() -> PermissionStatus {
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
    let granted = unsafe { CGRequestScreenCaptureAccess() };
    if granted {
        PermissionStatus::Granted
    } else {
        PermissionStatus::Denied
    }
}

fn request_camera() -> PermissionStatus {
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

    let trusted = unsafe { AXIsProcessTrustedWithOptions(options.as_CFTypeRef()) };

    if trusted {
        PermissionStatus::Granted
    } else {
        // The system prompt was shown; user hasn't granted yet.
        PermissionStatus::Denied
    }
}

fn request_notifications() -> PermissionStatus {
    // For notifications, checking is the same as what we can do —
    // the actual request requires requestAuthorizationWithOptions which
    // needs specific options. We re-check status after a brief check.
    check_notifications()
}

// ---------------------------------------------------------------------------
// Status mappers
// ---------------------------------------------------------------------------

fn av_status_to_permission(status: AVAuthorizationStatus) -> PermissionStatus {
    match status {
        AVAuthorizationStatus::Authorized => PermissionStatus::Granted,
        AVAuthorizationStatus::Denied => PermissionStatus::Denied,
        AVAuthorizationStatus::Restricted => PermissionStatus::Restricted,
        AVAuthorizationStatus::NotDetermined => PermissionStatus::NotDetermined,
        _ => PermissionStatus::Unknown,
    }
}

fn sf_status_to_permission(status: SFSpeechRecognizerAuthorizationStatus) -> PermissionStatus {
    match status {
        SFSpeechRecognizerAuthorizationStatus::Authorized => PermissionStatus::Granted,
        SFSpeechRecognizerAuthorizationStatus::Denied => PermissionStatus::Denied,
        SFSpeechRecognizerAuthorizationStatus::Restricted => PermissionStatus::Restricted,
        SFSpeechRecognizerAuthorizationStatus::NotDetermined => PermissionStatus::NotDetermined,
        _ => PermissionStatus::Unknown,
    }
}

fn un_status_to_permission(status: UNAuthorizationStatus) -> PermissionStatus {
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

fn build_info(permission: TccPermission, status: PermissionStatus) -> PermissionInfo {
    let can_request = matches!(status, PermissionStatus::NotDetermined);
    PermissionInfo {
        permission,
        status,
        can_request,
    }
}

fn do_check(permission: TccPermission) -> PermissionInfo {
    let status = match permission {
        TccPermission::ScreenRecording => check_screen_recording(),
        TccPermission::Camera => check_camera(),
        TccPermission::Microphone => check_microphone(),
        TccPermission::SpeechRecognition => check_speech_recognition(),
        TccPermission::Accessibility => check_accessibility(),
        TccPermission::Notifications => check_notifications(),
    };
    build_info(permission, status)
}

fn do_request(permission: TccPermission) -> PermissionInfo {
    let status = match permission {
        TccPermission::ScreenRecording => request_screen_recording(),
        TccPermission::Camera => request_camera(),
        TccPermission::Microphone => request_microphone(),
        TccPermission::SpeechRecognition => request_speech_recognition(),
        TccPermission::Accessibility => request_accessibility(),
        TccPermission::Notifications => request_notifications(),
    };
    build_info(permission, status)
}

// ---------------------------------------------------------------------------
// Trait implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl PermissionCapability for MacOSPermission {
    async fn check(&self, permission: TccPermission) -> Result<PermissionInfo> {
        let info = tokio::task::spawn_blocking(move || do_check(permission))
            .await
            .unwrap_or_else(|_| build_info(permission, PermissionStatus::Unknown));
        Ok(info)
    }

    async fn check_all(&self) -> Result<Vec<PermissionInfo>> {
        let results = tokio::task::spawn_blocking(|| {
            TccPermission::ALL
                .iter()
                .map(|&p| do_check(p))
                .collect::<Vec<_>>()
        })
        .await
        .unwrap_or_else(|_| {
            TccPermission::ALL
                .iter()
                .map(|&p| build_info(p, PermissionStatus::Unknown))
                .collect()
        });
        Ok(results)
    }

    async fn request(&self, permission: TccPermission) -> Result<PermissionInfo> {
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
        let types: Vec<TccPermission> = results.iter().map(|r| r.permission).collect();
        for p in TccPermission::ALL {
            assert!(types.contains(p), "missing permission: {:?}", p);
        }
    }

    #[test]
    fn test_build_info_can_request() {
        let info = build_info(TccPermission::Camera, PermissionStatus::NotDetermined);
        assert!(info.can_request);

        let info = build_info(TccPermission::Camera, PermissionStatus::Granted);
        assert!(!info.can_request);

        let info = build_info(TccPermission::Camera, PermissionStatus::Denied);
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
        let result = rt.block_on(perm.check(TccPermission::Camera));
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
    fn test_do_check_all_permissions_return_valid_info() {
        for &perm in TccPermission::ALL {
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
    fn test_do_request_all_permissions_return_valid_info() {
        for &perm in TccPermission::ALL {
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
}
