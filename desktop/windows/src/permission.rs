//! Windows `PermissionCapability` via the `CapabilityAccessManager` `ConsentStore`.
//!
//! Windows has no monolithic TCC database like macOS. Instead, the small set of
//! genuinely user-gated capabilities (camera, microphone, location) live under
//! `HKCU\…\CapabilityAccessManager\ConsentStore\<capability>\Value` as a `REG_SZ`
//! of `"Allow"` / `"Deny"`. Everything macOS gates through TCC but Windows does
//! *not* gate for a desktop app — screen capture, sending synthetic input
//! (accessibility / input monitoring), and posting toast notifications — is
//! reported as `Granted`, because a Win32 desktop process can do those without
//! any per-app consent prompt.
//!
//! This maps the cross-platform [`PermissionCapability`] contract onto Windows
//! idioms so the already-wired `permission` and `desktop_check_permissions`
//! tools return real answers on Windows instead of "not supported".
//!
//! The `ConsentStore` is read by shelling out to `powershell.exe` (consistent
//! with the rest of this crate, which already drives toasts and shortcuts the
//! same way) — the capability name is always one of a fixed allowlist, never
//! user input, so the query string is injection-free. All pure mapping logic is
//! platform-independent and unit-tested; only the registry read itself is
//! `#[cfg(windows)]`.

use async_trait::async_trait;

use aleph_desktop::permission_types::{
    PermissionInfo, PermissionKind, PermissionStatus, TCC_MANAGED,
};
use aleph_desktop::traits::PermissionCapability;
use aleph_desktop::Result;
use aleph_protocol::desktop_bridge::methods::perm::{
    PermissionGuide, PermissionStatus as ProtocolPermissionStatus,
};

/// Windows permission capability backed by the `CapabilityAccessManager`.
pub struct WindowsPermission {
    _private: (),
}

impl WindowsPermission {
    #[must_use]
    pub const fn new() -> Self {
        Self { _private: () }
    }
}

impl Default for WindowsPermission {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Pure mapping helpers (platform-independent, unit-tested)
// ---------------------------------------------------------------------------

/// The `ConsentStore` capability key for a permission kind, when Windows actually
/// gates it per-app. `None` means Windows does not have a consent prompt for
/// this kind (see [`ungated_status`]).
const fn consent_capability(kind: PermissionKind) -> Option<&'static str> {
    match kind {
        PermissionKind::Camera => Some("webcam"),
        PermissionKind::Microphone => Some("microphone"),
        PermissionKind::Location => Some("location"),
        _ => None,
    }
}

/// Status for kinds Windows does not gate behind a `ConsentStore` entry.
///
/// A Win32 desktop process can capture the screen, send synthetic input, and
/// post toast notifications with no per-app grant, so those report `Granted`.
/// Kinds that simply do not map onto a Windows concept (or that Aleph cannot
/// determine without overreaching) report `Unknown`, mirroring how the macOS
/// native probe returns `Unknown` for bridge-only kinds.
const fn ungated_status(kind: PermissionKind) -> PermissionStatus {
    match kind {
        PermissionKind::ScreenRecording
        | PermissionKind::Accessibility
        | PermissionKind::InputMonitoring
        | PermissionKind::Notifications => PermissionStatus::Granted,
        // Consent-gated kinds are handled before this is reached; list them so
        // the match stays exhaustive and a newly gated kind fails the compile.
        PermissionKind::Camera | PermissionKind::Microphone | PermissionKind::Location => {
            PermissionStatus::Unknown
        }
        PermissionKind::SpeechRecognition
        | PermissionKind::FullDisk
        | PermissionKind::Automation
        | PermissionKind::Contacts
        | PermissionKind::Calendars
        | PermissionKind::Reminders
        | PermissionKind::Photos => PermissionStatus::Unknown,
    }
}

/// Parse a raw `ConsentStore` `Value` into a [`PermissionStatus`].
///
/// Only the `#[cfg(windows)]` registry read calls this in a production build;
/// on other hosts it is exercised solely by unit tests.
#[cfg_attr(not(windows), allow(dead_code))]
fn parse_consent_value(raw: &str) -> PermissionStatus {
    match raw.trim() {
        "Allow" => PermissionStatus::Granted,
        "Deny" => PermissionStatus::Denied,
        // Empty / absent / unexpected → the user has never decided.
        _ => PermissionStatus::NotDetermined,
    }
}

/// `ms-settings:` deep link to the Settings page where the user can grant a
/// kind. Ungated kinds fall back to the top-level privacy page.
const fn settings_uri(kind: PermissionKind) -> &'static str {
    match kind {
        PermissionKind::Camera => "ms-settings:privacy-webcam",
        PermissionKind::Microphone => "ms-settings:privacy-microphone",
        PermissionKind::Location => "ms-settings:privacy-location",
        _ => "ms-settings:privacy",
    }
}

/// One-sentence rationale relayed to the user when a permission is missing.
const fn rationale(kind: PermissionKind) -> &'static str {
    match kind {
        PermissionKind::Camera => "Aleph needs camera access to capture photos on your behalf.",
        PermissionKind::Microphone => {
            "Aleph needs microphone access to record and transcribe audio you ask for."
        }
        PermissionKind::Location => {
            "Aleph needs location access to answer location-aware requests."
        }
        _ => "Aleph uses this capability to act on your desktop on your behalf.",
    }
}

/// Step-by-step instructions for granting a consent-gated kind in Settings.
fn steps(kind: PermissionKind) -> Vec<String> {
    let (page, toggle) = match kind {
        PermissionKind::Camera => ("Privacy & security → Camera", "Camera access"),
        PermissionKind::Microphone => ("Privacy & security → Microphone", "Microphone access"),
        PermissionKind::Location => ("Privacy & security → Location", "Location services"),
        _ => {
            return vec![
                "Open Settings → Privacy & security.".to_string(),
                "Review the relevant capability and ensure access is allowed.".to_string(),
            ];
        }
    };
    vec![
        "Open Settings (Win+I).".to_string(),
        format!("Go to {page}."),
        format!("Turn on \"{toggle}\" and enable access for desktop apps."),
        "Return to Aleph and retry.".to_string(),
    ]
}

/// Build a [`PermissionInfo`]. On Windows a desktop app cannot programmatically
/// trigger a consent prompt, so `can_request` is always `false` — the user must
/// toggle the capability in Settings.
const fn build_info(permission: PermissionKind, status: PermissionStatus) -> PermissionInfo {
    PermissionInfo {
        permission,
        status,
        can_request: false,
    }
}

// ---------------------------------------------------------------------------
// ConsentStore read (Windows-only) + cross-platform fallback
// ---------------------------------------------------------------------------

/// Read a single `ConsentStore` capability value and map it to a status.
async fn query_consent(capability: &str) -> PermissionStatus {
    #[cfg(windows)]
    {
        use tokio::process::Command;

        // `capability` is always a fixed allowlist literal (webcam/microphone/
        // location) from `consent_capability`, so the path is injection-free.
        let script = format!(
            "$v=(Get-ItemProperty -Path \
             'HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\CapabilityAccessManager\\ConsentStore\\{capability}' \
             -Name Value -ErrorAction SilentlyContinue).Value; if($v){{$v}}else{{''}}"
        );

        match Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .output()
            .await
        {
            Ok(out) if out.status.success() => {
                let raw = String::from_utf8_lossy(&out.stdout);
                parse_consent_value(&raw)
            }
            // Spawn failed or non-zero exit: we cannot determine the state.
            _ => PermissionStatus::Unknown,
        }
    }
    #[cfg(not(windows))]
    {
        let _ = capability;
        PermissionStatus::Unknown
    }
}

/// Resolve the status for any kind: consent-gated kinds hit the registry,
/// everything else uses the static [`ungated_status`] mapping.
async fn status_for(kind: PermissionKind) -> PermissionStatus {
    match consent_capability(kind) {
        Some(cap) => query_consent(cap).await,
        None => ungated_status(kind),
    }
}

/// Open the Settings page for a kind via `cmd /C start`. Returns whether the
/// launch was dispatched.
async fn open_settings_uri(kind: PermissionKind) -> bool {
    #[cfg(windows)]
    {
        use tokio::process::Command;
        let uri = settings_uri(kind);
        Command::new("cmd.exe")
            .args(["/C", "start", "", uri])
            .status()
            .await
            .is_ok_and(|s| s.success())
    }
    #[cfg(not(windows))]
    {
        let _ = kind;
        false
    }
}

// ---------------------------------------------------------------------------
// Trait implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl PermissionCapability for WindowsPermission {
    async fn check(&self, permission: PermissionKind) -> Result<PermissionInfo> {
        Ok(build_info(permission, status_for(permission).await))
    }

    async fn check_all(&self) -> Result<Vec<PermissionInfo>> {
        let mut infos = Vec::with_capacity(TCC_MANAGED.len());
        for &kind in TCC_MANAGED {
            infos.push(build_info(kind, status_for(kind).await));
        }
        Ok(infos)
    }

    async fn request(&self, permission: PermissionKind) -> Result<PermissionInfo> {
        // Windows desktop apps cannot raise a consent prompt programmatically.
        // For genuinely gated kinds the helpful analogue to the macOS prompt is
        // to open the Settings page so the user can grant it; then report the
        // current status. Ungated kinds need no action.
        if consent_capability(permission).is_some() {
            let _ = open_settings_uri(permission).await;
        }
        Ok(build_info(permission, status_for(permission).await))
    }

    async fn check_permission(&self, kind: PermissionKind) -> Result<ProtocolPermissionStatus> {
        let status = status_for(kind).await;
        Ok(ProtocolPermissionStatus {
            kind,
            granted: matches!(status, PermissionStatus::Granted),
            // No Win32 API to request a desktop-app capability programmatically.
            can_request_programmatically: false,
            restricted: false,
        })
    }

    async fn guide_permission(&self, kind: PermissionKind) -> Result<PermissionGuide> {
        let status = self.check_permission(kind).await?;
        Ok(PermissionGuide {
            kind,
            status,
            deep_link: settings_uri(kind).to_string(),
            human_readable_steps: steps(kind),
            rationale: rationale(kind).to_string(),
        })
    }

    async fn open_settings(&self, kind: PermissionKind) -> Result<bool> {
        Ok(open_settings_uri(kind).await)
    }
}

// ---------------------------------------------------------------------------
// Tests (pure mapping logic — runs on every platform)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_default() {
        let _p = WindowsPermission::default();
    }

    #[test]
    fn consent_capability_maps_three_gated_kinds() {
        assert_eq!(consent_capability(PermissionKind::Camera), Some("webcam"));
        assert_eq!(
            consent_capability(PermissionKind::Microphone),
            Some("microphone")
        );
        assert_eq!(
            consent_capability(PermissionKind::Location),
            Some("location")
        );
        assert_eq!(consent_capability(PermissionKind::ScreenRecording), None);
        assert_eq!(consent_capability(PermissionKind::Accessibility), None);
    }

    #[test]
    fn ungated_screen_and_input_are_granted() {
        for kind in [
            PermissionKind::ScreenRecording,
            PermissionKind::Accessibility,
            PermissionKind::InputMonitoring,
            PermissionKind::Notifications,
        ] {
            assert_eq!(
                ungated_status(kind),
                PermissionStatus::Granted,
                "{kind:?} should be Granted on Windows"
            );
        }
    }

    #[test]
    fn ungated_unmappable_kinds_are_unknown() {
        for kind in [
            PermissionKind::SpeechRecognition,
            PermissionKind::FullDisk,
            PermissionKind::Automation,
            PermissionKind::Contacts,
            PermissionKind::Calendars,
            PermissionKind::Reminders,
            PermissionKind::Photos,
        ] {
            assert_eq!(ungated_status(kind), PermissionStatus::Unknown);
        }
    }

    #[test]
    fn parse_consent_value_maps_allow_deny() {
        assert_eq!(parse_consent_value("Allow"), PermissionStatus::Granted);
        assert_eq!(parse_consent_value("Deny"), PermissionStatus::Denied);
        assert_eq!(parse_consent_value(" Allow \n"), PermissionStatus::Granted);
        assert_eq!(parse_consent_value(""), PermissionStatus::NotDetermined);
        assert_eq!(
            parse_consent_value("Prompt"),
            PermissionStatus::NotDetermined
        );
    }

    #[test]
    fn settings_uri_for_gated_kinds() {
        assert_eq!(
            settings_uri(PermissionKind::Camera),
            "ms-settings:privacy-webcam"
        );
        assert_eq!(
            settings_uri(PermissionKind::Microphone),
            "ms-settings:privacy-microphone"
        );
        assert_eq!(
            settings_uri(PermissionKind::Location),
            "ms-settings:privacy-location"
        );
        assert_eq!(
            settings_uri(PermissionKind::ScreenRecording),
            "ms-settings:privacy"
        );
    }

    #[test]
    fn build_info_never_requestable_on_windows() {
        for status in [
            PermissionStatus::Granted,
            PermissionStatus::Denied,
            PermissionStatus::NotDetermined,
        ] {
            assert!(!build_info(PermissionKind::Camera, status).can_request);
        }
    }

    #[test]
    fn steps_gated_kinds_have_four_steps() {
        assert_eq!(steps(PermissionKind::Camera).len(), 4);
        assert_eq!(steps(PermissionKind::Microphone).len(), 4);
        assert_eq!(steps(PermissionKind::Location).len(), 4);
        // Ungated fallback still produces actionable guidance.
        assert!(!steps(PermissionKind::ScreenRecording).is_empty());
    }

    #[tokio::test]
    async fn check_all_returns_six_managed_kinds() {
        let perm = WindowsPermission::new();
        let infos = perm.check_all().await.unwrap();
        assert_eq!(infos.len(), TCC_MANAGED.len());
        let kinds: Vec<PermissionKind> = infos.iter().map(|i| i.permission).collect();
        for k in TCC_MANAGED {
            assert!(kinds.contains(k), "missing {k:?}");
        }
    }

    #[tokio::test]
    async fn check_permission_screen_recording_granted() {
        let perm = WindowsPermission::new();
        let status = perm
            .check_permission(PermissionKind::ScreenRecording)
            .await
            .unwrap();
        assert!(status.granted);
        assert!(!status.can_request_programmatically);
        assert_eq!(status.kind, PermissionKind::ScreenRecording);
    }

    #[tokio::test]
    async fn guide_permission_carries_deep_link_and_steps() {
        let perm = WindowsPermission::new();
        let guide = perm.guide_permission(PermissionKind::Camera).await.unwrap();
        assert_eq!(guide.deep_link, "ms-settings:privacy-webcam");
        assert!(!guide.human_readable_steps.is_empty());
        assert!(!guide.rationale.is_empty());
    }
}
