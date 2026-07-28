//! App lifecycle via `NSWorkspace` + `NSRunningApplication`.

use aleph_desktop::system_types::AppInfo;
use aleph_desktop::{DesktopError, Result};
use objc2_app_kit::{NSRunningApplication, NSWorkspace};
use objc2_foundation::{NSArray, NSString, NSURL};

/// Launch an application by name or bundle identifier.
pub fn launch_app(app_name: &str) -> Result<()> {
    let ws = NSWorkspace::sharedWorkspace();
    let ns_name = NSString::from_str(app_name);

    // Try bundle ID (contains dots) or app name
    #[allow(deprecated)] // fullPathForApplication is the only name-based lookup
    let url: Option<objc2::rc::Retained<NSURL>> = if app_name.contains('.') {
        ws.URLForApplicationWithBundleIdentifier(&ns_name)
    } else {
        ws.fullPathForApplication(&ns_name)
            .map(|path| NSURL::fileURLWithPath(&path))
    };

    let url = url.ok_or_else(|| {
        DesktopError::InputFailed(format!("launch_app: application '{app_name}' not found"))
    })?;

    if !ws.openURL(&url) {
        return Err(DesktopError::InputFailed(format!(
            "launch_app: failed to launch '{app_name}'"
        )));
    }
    Ok(())
}

/// Quit a running application by name or bundle identifier.
pub fn quit_app(app_name: &str) -> Result<()> {
    let ws = NSWorkspace::sharedWorkspace();
    let apps = ws.runningApplications();
    let lower_name = app_name.to_lowercase();

    for app in &apps {
        let matches = app
            .bundleIdentifier()
            .is_some_and(|b| b.to_string().to_lowercase() == lower_name)
            || app
                .localizedName()
                .is_some_and(|n| n.to_string().to_lowercase() == lower_name);

        if matches {
            if app.terminate() {
                return Ok(());
            }
            return Err(DesktopError::InputFailed(format!(
                "quit_app: '{app_name}' refused to terminate"
            )));
        }
    }

    Err(DesktopError::InputFailed(format!(
        "quit_app: no running application matching '{app_name}'"
    )))
}

/// List the running applications a user (or the model) could actually address.
///
/// `NSWorkspace.runningApplications` is not a list of apps — it is a list of
/// every process with a LaunchServices connection. Measured on a normal desktop:
/// **121 entries, of which 10 were real apps** (10 `.regular`, 59 `.accessory`,
/// 52 `.prohibited`). The 52 are `LSBackgroundOnly` helpers: XPC services, per-tab
/// renderers, update daemons. They have no UI, cannot be activated, cannot be
/// frontmost, and cannot usefully receive targeted input.
///
/// They are not merely noise — they break app resolution. `native::match_running_app`
/// resolves `app: "chrome"` by unique substring, and on the same machine that
/// query matched **two** entries (Google Chrome, plus a `.prohibited` helper whose
/// name embeds "Google Chrome"), so the tool refused the request as ambiguous.
/// The Windows limb reached the same conclusion from the other side and dedupes
/// by pid; here the discriminator the OS already provides is the activation
/// policy, so that is what is used.
///
/// `.accessory` (menu-bar-only apps) is deliberately **kept**: those do have UI,
/// can become frontmost, and include password managers — which
/// `safety::blocked_app_reason` has to be able to see in the frontmost slot.
pub fn list_running_apps() -> Result<Vec<AppInfo>> {
    use objc2_app_kit::NSApplicationActivationPolicy;

    let ws = NSWorkspace::sharedWorkspace();
    let apps: objc2::rc::Retained<NSArray<NSRunningApplication>> = ws.runningApplications();

    let mut result = Vec::new();
    for app in &apps {
        if app.activationPolicy() == NSApplicationActivationPolicy::Prohibited {
            continue;
        }
        let name = app
            .localizedName()
            .map(|s| s.to_string())
            .unwrap_or_default();
        let bundle_id = app
            .bundleIdentifier()
            .map(|s| s.to_string())
            .unwrap_or_default();
        let pid = u64::try_from(app.processIdentifier()).unwrap_or(0);
        let is_active = app.isActive();

        result.push(AppInfo {
            name,
            bundle_id,
            pid: Some(pid),
            is_active,
        });
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_running_apps_includes_finder() {
        let apps = list_running_apps().unwrap();
        assert!(!apps.is_empty(), "running apps should not be empty");
        let has_finder = apps.iter().any(|a| a.bundle_id == "com.apple.finder");
        assert!(has_finder, "Finder should be in running apps");
    }

    /// The UI-less half of `runningApplications` is dropped, so the list stays
    /// something `match_running_app` can resolve a name against.
    ///
    /// Asserted against the OS rather than a fixed number: the filter's contract
    /// is "no `.prohibited` process survives", and the count on any given machine
    /// is whatever it is.
    #[test]
    fn background_only_processes_are_not_offered_as_apps() {
        use objc2_app_kit::NSApplicationActivationPolicy;

        let ws = NSWorkspace::sharedWorkspace();
        let raw = ws.runningApplications();
        let prohibited: Vec<u64> = raw
            .iter()
            .filter(|a| a.activationPolicy() == NSApplicationActivationPolicy::Prohibited)
            .filter_map(|a| u64::try_from(a.processIdentifier()).ok())
            .collect();

        let listed = list_running_apps().unwrap();
        for pid in &prohibited {
            assert!(
                !listed.iter().any(|a| a.pid == Some(*pid)),
                "pid {pid} is LSBackgroundOnly and must not be offered as an app"
            );
        }
        assert!(
            listed.len() <= raw.len(),
            "the filter may only remove entries"
        );
    }
}
