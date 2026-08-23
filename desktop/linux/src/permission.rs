//! Linux `PermissionCapability` mapped onto freedesktop idioms.
//!
//! Linux has no monolithic permission database like macOS TCC and no per-app
//! `ConsentStore` like Windows. Access is governed by a mix of:
//!
//! - **Display server** — under X11 a desktop process can screen-capture and
//!   inject synthetic input (XTEST) with no prompt, so those report `Granted`.
//!   Under Wayland the compositor denies global input grabs and routes screen
//!   capture through `xdg-desktop-portal` (a request-time dialog, not a
//!   queryable persistent grant), so those report `Unknown`.
//! - **Device groups / udev** — raw camera (`/dev/video*`) and audio
//!   (`/dev/snd`) access is gated by membership in the `video` / `audio` groups
//!   (or per-session udev ACLs). We probe device presence + group membership
//!   without ever *opening* the device (which would grab it).
//! - **D-Bus** — desktop notifications work on both X11 and Wayland, so
//!   `Notifications` reports `Granted`.
//!
//! This maps the cross-platform [`PermissionCapability`] contract onto Linux so
//! the already-wired `permission` and `desktop_check_permissions` tools return
//! real answers on Linux instead of "not supported on Linux".
//!
//! All session/kind mapping is pure and unit-tested on every host; only the
//! device probes and group lookup are `#[cfg(target_os = "linux")]`, with an
//! `Unknown`/`false` fallback elsewhere so the logic stays compilable and
//! testable cross-platform (mirroring `desktop/windows/src/permission.rs`).

use async_trait::async_trait;

use aleph_desktop::permission_types::{
    PermissionInfo, PermissionKind, PermissionStatus, TCC_MANAGED,
};
use aleph_desktop::traits::PermissionCapability;
use aleph_desktop::Result;
use aleph_protocol::desktop_bridge::methods::perm::{
    PermissionGuide, PermissionStatus as ProtocolPermissionStatus,
};

/// Linux permission capability backed by display-server + device-group probes.
pub struct LinuxPermission {
    _private: (),
}

impl LinuxPermission {
    #[must_use]
    pub const fn new() -> Self {
        Self { _private: () }
    }
}

impl Default for LinuxPermission {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Session detection
// ---------------------------------------------------------------------------
//
// The session type comes from `aleph_desktop::linux::session` — the single
// source of truth shared with the input, clipboard and window layers. This
// module used to carry its own `SessionType` enum and its own environment
// rules, which is how three subtly different answers to "is this Wayland?"
// ended up in the tree at once.

use aleph_desktop::linux::SessionKind;

// ---------------------------------------------------------------------------
// Pure mapping helpers (platform-independent, unit-tested)
// ---------------------------------------------------------------------------

/// Status for kinds that are *not* gated by a device group — they depend only
/// on the display server (or not at all). Camera/Microphone are resolved by the
/// device probes before this is reached; they are listed here so the match
/// stays exhaustive and a newly added kind fails the compile.
const fn ungated_status(kind: PermissionKind, session: SessionKind) -> PermissionStatus {
    match kind {
        // D-Bus desktop notifications work regardless of display server.
        PermissionKind::Notifications => PermissionStatus::Granted,

        // Screen capture: free under X11; portal/request-time under Wayland.
        PermissionKind::ScreenRecording => match session {
            SessionKind::X11 => PermissionStatus::Granted,
            SessionKind::Wayland | SessionKind::Unknown => PermissionStatus::Unknown,
        },

        // Synthetic input (XTEST) is free under X11; Wayland denies global
        // input grabs to ordinary clients.
        PermissionKind::Accessibility | PermissionKind::InputMonitoring => match session {
            SessionKind::X11 => PermissionStatus::Granted,
            SessionKind::Wayland | SessionKind::Unknown => PermissionStatus::Unknown,
        },

        // Device-gated kinds are handled by `status_for` before reaching here.
        PermissionKind::Camera | PermissionKind::Microphone => PermissionStatus::Unknown,

        // No Linux concept Aleph can determine without overreaching.
        PermissionKind::SpeechRecognition
        | PermissionKind::FullDisk
        | PermissionKind::Automation
        | PermissionKind::Contacts
        | PermissionKind::Calendars
        | PermissionKind::Reminders
        | PermissionKind::Photos
        | PermissionKind::Location => PermissionStatus::Unknown,
    }
}

/// `true` if a whitespace-separated `id -nG` group listing contains `group`.
///
/// Only the `#[cfg(target_os = "linux")]` group probe calls this in a real
/// build; on other hosts it is exercised solely by unit tests.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn groups_contain(id_output: &str, group: &str) -> bool {
    id_output.split_whitespace().any(|g| g == group)
}

/// Classify camera access from device presence + `video`-group membership.
///
/// - no `/dev/video*` device → `Unknown` (nothing to gate; likely no camera).
/// - device present + in `video` group → `Granted`.
/// - device present + not in group → `Unknown` (per-session udev ACLs may still
///   grant access, so we avoid a false `Denied`; the guide tells the user how
///   to ensure access).
const fn classify_camera(present: bool, in_video_group: bool) -> PermissionStatus {
    match (present, in_video_group) {
        (false, _) => PermissionStatus::Unknown,
        (true, true) => PermissionStatus::Granted,
        (true, false) => PermissionStatus::Unknown,
    }
}

/// Classify microphone access from `/dev/snd` presence. Modern `PipeWire` grants
/// audio to the active session without `audio`-group membership, so device
/// presence is the honest signal; absence is `Unknown` (no sound hardware).
const fn classify_microphone(present: bool) -> PermissionStatus {
    if present {
        PermissionStatus::Granted
    } else {
        PermissionStatus::Unknown
    }
}

/// One-sentence rationale relayed to the user when a permission is missing.
const fn rationale(kind: PermissionKind) -> &'static str {
    match kind {
        PermissionKind::Camera => "Aleph needs camera access to capture photos on your behalf.",
        PermissionKind::Microphone => {
            "Aleph needs microphone access to record and transcribe audio you ask for."
        }
        PermissionKind::ScreenRecording => {
            "Aleph needs screen access to see your desktop and act on what it shows."
        }
        PermissionKind::Accessibility | PermissionKind::InputMonitoring => {
            "Aleph needs input control to move the pointer and type on your behalf."
        }
        _ => "Aleph uses this capability to act on your desktop on your behalf.",
    }
}

/// Step-by-step guidance for granting a kind on Linux.
fn steps(kind: PermissionKind, session: SessionKind) -> Vec<String> {
    match kind {
        PermissionKind::Camera => vec![
            "Add your user to the `video` group: `sudo usermod -aG video $USER`.".to_string(),
            "Log out and back in for the group change to take effect.".to_string(),
            "Return to Aleph and retry.".to_string(),
        ],
        PermissionKind::Microphone => vec![
            "Ensure PipeWire/PulseAudio is running for your session.".to_string(),
            "If using raw ALSA, add your user to the `audio` group: \
             `sudo usermod -aG audio $USER`."
                .to_string(),
            "Return to Aleph and retry.".to_string(),
        ],
        PermissionKind::ScreenRecording => match session {
            SessionKind::Wayland => vec![
                "Wayland routes screen capture through xdg-desktop-portal.".to_string(),
                "When Aleph first captures the screen, approve the portal dialog.".to_string(),
                "Ensure `xdg-desktop-portal` and a backend (e.g. \
                 `xdg-desktop-portal-gnome`/`-kde`/`-wlr`) are installed."
                    .to_string(),
            ],
            _ => vec!["Screen capture is available without a prompt on X11.".to_string()],
        },
        PermissionKind::Accessibility | PermissionKind::InputMonitoring => match session {
            SessionKind::Wayland => vec![
                "Wayland denies global input injection to ordinary clients.".to_string(),
                "Use an X11/XWayland session for full pointer/keyboard control.".to_string(),
            ],
            _ => vec!["Input control is available without a prompt on X11.".to_string()],
        },
        _ => vec![
            "This capability has no per-app permission gate on Linux.".to_string(),
            "Ensure the relevant service/device is available for your session.".to_string(),
        ],
    }
}

/// Build a [`PermissionInfo`]. On Linux access is granted by group membership /
/// session type, never by a programmatic prompt, so `can_request` is always
/// `false` — mirroring the Windows desktop-app contract.
const fn build_info(permission: PermissionKind, status: PermissionStatus) -> PermissionInfo {
    PermissionInfo {
        permission,
        status,
        can_request: false,
    }
}

// ---------------------------------------------------------------------------
// Device / group probes (Linux-only) + cross-platform fallback
// ---------------------------------------------------------------------------

/// `true` if at least one `/dev/video*` capture device exists.
fn camera_present() -> bool {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_dir("/dev")
            .map(|rd| {
                rd.flatten()
                    .any(|e| e.file_name().to_string_lossy().starts_with("video"))
            })
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

/// `true` if the ALSA `/dev/snd` device tree exists.
fn microphone_present() -> bool {
    #[cfg(target_os = "linux")]
    {
        std::path::Path::new("/dev/snd").exists()
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

/// `true` if the current user belongs to `group`, via `id -nG`.
///
/// Shells out (consistent with the rest of this crate); `group` is always a
/// fixed literal (`video`), never user input, so the command is injection-free.
async fn in_group(group: &str) -> bool {
    #[cfg(target_os = "linux")]
    {
        let mut cmd = tokio::process::Command::new("id");
        cmd.arg("-nG");
        match aleph_desktop::script_exec::output_capped(
            cmd,
            aleph_desktop::script_exec::DESKTOP_QUERY_TIMEOUT,
        )
        .await
        {
            Ok(out) if out.status.success() => {
                groups_contain(&String::from_utf8_lossy(&out.stdout), group)
            }
            _ => false,
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = group;
        false
    }
}

/// Resolve the status for any kind: device-gated kinds probe the device tree +
/// groups, everything else uses the session-aware [`ungated_status`] mapping.
async fn status_for(kind: PermissionKind, session: SessionKind) -> PermissionStatus {
    match kind {
        PermissionKind::Camera => classify_camera(camera_present(), in_group("video").await),
        PermissionKind::Microphone => classify_microphone(microphone_present()),
        _ => ungated_status(kind, session),
    }
}

/// Try to open a desktop settings/privacy panel. Best-effort across the major
/// desktops; returns whether a launch was dispatched.
///
/// **Spawned, never waited on.** This used to call `.status()`, which blocks
/// until the settings *window is closed* — so `permission(request)` and
/// `permission(open_settings)` hung the entire agent turn until the user
/// noticed and closed a window they may not have seen open. A settings panel
/// outlives the tool call by definition.
///
/// "Dispatched" is therefore the honest claim: we know the binary started, not
/// that the user did anything with it.
async fn open_settings_panel() -> bool {
    #[cfg(target_os = "linux")]
    {
        use std::process::Stdio;
        use tokio::process::Command;
        for (bin, args) in [
            ("gnome-control-center", &["privacy"][..]),
            ("systemsettings", &[][..]),
            ("systemsettings5", &[][..]),
            ("xfce4-settings-manager", &[][..]),
            ("cinnamon-settings", &[][..]),
            ("mate-control-center", &[][..]),
        ] {
            if Command::new(bin)
                .args(args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .is_ok()
            {
                return true;
            }
        }
        false
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

// ---------------------------------------------------------------------------
// Trait implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl PermissionCapability for LinuxPermission {
    async fn check(&self, permission: PermissionKind) -> Result<PermissionInfo> {
        let session = aleph_desktop::linux::session().kind;
        Ok(build_info(
            permission,
            status_for(permission, session).await,
        ))
    }

    async fn check_all(&self) -> Result<Vec<PermissionInfo>> {
        let session = aleph_desktop::linux::session().kind;
        let mut infos = Vec::with_capacity(TCC_MANAGED.len());
        for &kind in TCC_MANAGED {
            infos.push(build_info(kind, status_for(kind, session).await));
        }
        Ok(infos)
    }

    async fn request(&self, permission: PermissionKind) -> Result<PermissionInfo> {
        // Linux access is granted by group membership / session type, not by a
        // programmatic prompt. The helpful analogue is to open the settings
        // panel so the user can review access; then report current status.
        let session = aleph_desktop::linux::session().kind;
        let status = status_for(permission, session).await;
        if !matches!(status, PermissionStatus::Granted) {
            let _ = open_settings_panel().await;
        }
        Ok(build_info(permission, status))
    }

    async fn check_permission(&self, kind: PermissionKind) -> Result<ProtocolPermissionStatus> {
        let session = aleph_desktop::linux::session().kind;
        let status = status_for(kind, session).await;
        Ok(ProtocolPermissionStatus {
            kind,
            granted: matches!(status, PermissionStatus::Granted),
            // No Linux API raises a per-app permission prompt programmatically.
            can_request_programmatically: false,
            restricted: false,
        })
    }

    async fn guide_permission(&self, kind: PermissionKind) -> Result<PermissionGuide> {
        let session = aleph_desktop::linux::session().kind;
        let status = self.check_permission(kind).await?;
        Ok(PermissionGuide {
            kind,
            status,
            // Linux has no universal settings deep-link URI scheme.
            deep_link: String::new(),
            human_readable_steps: steps(kind, session),
            rationale: rationale(kind).to_string(),
        })
    }

    async fn open_settings(&self, _kind: PermissionKind) -> Result<bool> {
        Ok(open_settings_panel().await)
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
        let _p = LinuxPermission::default();
    }

    #[test]
    fn notifications_always_granted() {
        for session in [SessionKind::X11, SessionKind::Wayland, SessionKind::Unknown] {
            assert_eq!(
                ungated_status(PermissionKind::Notifications, session),
                PermissionStatus::Granted
            );
        }
    }

    #[test]
    fn screen_and_input_granted_on_x11_only() {
        for kind in [
            PermissionKind::ScreenRecording,
            PermissionKind::Accessibility,
            PermissionKind::InputMonitoring,
        ] {
            assert_eq!(
                ungated_status(kind, SessionKind::X11),
                PermissionStatus::Granted,
                "{kind:?} should be Granted on X11"
            );
            assert_eq!(
                ungated_status(kind, SessionKind::Wayland),
                PermissionStatus::Unknown,
                "{kind:?} should be Unknown on Wayland"
            );
            assert_eq!(
                ungated_status(kind, SessionKind::Unknown),
                PermissionStatus::Unknown
            );
        }
    }

    #[test]
    fn unmappable_kinds_are_unknown() {
        for kind in [
            PermissionKind::SpeechRecognition,
            PermissionKind::FullDisk,
            PermissionKind::Automation,
            PermissionKind::Contacts,
            PermissionKind::Calendars,
            PermissionKind::Reminders,
            PermissionKind::Photos,
            PermissionKind::Location,
        ] {
            assert_eq!(
                ungated_status(kind, SessionKind::X11),
                PermissionStatus::Unknown
            );
        }
    }

    #[test]
    fn groups_contain_matches_whole_token() {
        assert!(groups_contain("user adm video sudo", "video"));
        assert!(groups_contain("video", "video"));
        assert!(!groups_contain("audiovideox novideo", "video"));
        assert!(!groups_contain("user adm sudo", "video"));
        assert!(!groups_contain("", "video"));
    }

    #[test]
    fn camera_classification() {
        assert_eq!(classify_camera(false, false), PermissionStatus::Unknown);
        assert_eq!(classify_camera(false, true), PermissionStatus::Unknown);
        assert_eq!(classify_camera(true, true), PermissionStatus::Granted);
        // Present but not in group: avoid a false Denied (udev ACLs may grant).
        assert_eq!(classify_camera(true, false), PermissionStatus::Unknown);
    }

    #[test]
    fn microphone_classification() {
        assert_eq!(classify_microphone(true), PermissionStatus::Granted);
        assert_eq!(classify_microphone(false), PermissionStatus::Unknown);
    }

    #[test]
    fn build_info_never_requestable_on_linux() {
        for status in [
            PermissionStatus::Granted,
            PermissionStatus::Denied,
            PermissionStatus::NotDetermined,
            PermissionStatus::Unknown,
        ] {
            assert!(!build_info(PermissionKind::Camera, status).can_request);
        }
    }

    #[test]
    fn steps_are_actionable() {
        assert!(!steps(PermissionKind::Camera, SessionKind::X11).is_empty());
        assert!(!steps(PermissionKind::Microphone, SessionKind::Wayland).is_empty());
        // Wayland screen-record guidance mentions the portal.
        let sr = steps(PermissionKind::ScreenRecording, SessionKind::Wayland);
        assert!(sr.iter().any(|s| s.contains("portal")));
        // X11 screen-record guidance is the no-prompt path.
        let sr_x11 = steps(PermissionKind::ScreenRecording, SessionKind::X11);
        assert!(sr_x11.iter().any(|s| s.contains("X11")));
    }

    #[test]
    fn rationale_non_empty_for_all_managed() {
        for &kind in TCC_MANAGED {
            assert!(
                !rationale(kind).is_empty(),
                "missing rationale for {kind:?}"
            );
        }
    }

    #[tokio::test]
    async fn check_all_returns_all_managed_kinds() {
        let perm = LinuxPermission::new();
        let infos = perm.check_all().await.unwrap();
        assert_eq!(infos.len(), TCC_MANAGED.len());
        let kinds: Vec<PermissionKind> = infos.iter().map(|i| i.permission).collect();
        for k in TCC_MANAGED {
            assert!(kinds.contains(k), "missing {k:?}");
        }
    }

    #[tokio::test]
    async fn check_notifications_is_granted_and_not_requestable() {
        let perm = LinuxPermission::new();
        let info = perm.check(PermissionKind::Notifications).await.unwrap();
        assert_eq!(info.status, PermissionStatus::Granted);
        assert!(!info.can_request);
    }

    #[tokio::test]
    async fn guide_permission_carries_steps_and_rationale() {
        let perm = LinuxPermission::new();
        let guide = perm.guide_permission(PermissionKind::Camera).await.unwrap();
        assert!(guide.deep_link.is_empty()); // no universal scheme on Linux
        assert!(!guide.human_readable_steps.is_empty());
        assert!(!guide.rationale.is_empty());
        assert_eq!(guide.kind, PermissionKind::Camera);
    }
}
