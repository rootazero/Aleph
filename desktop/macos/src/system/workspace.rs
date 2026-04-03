//! App lifecycle via NSWorkspace + NSRunningApplication.

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
        DesktopError::InputFailed(format!("launch_app: application '{}' not found", app_name))
    })?;

    if !ws.openURL(&url) {
        return Err(DesktopError::InputFailed(format!(
            "launch_app: failed to launch '{}'",
            app_name
        )));
    }
    Ok(())
}

/// Quit a running application by name or bundle identifier.
pub fn quit_app(app_name: &str) -> Result<()> {
    let ws = NSWorkspace::sharedWorkspace();
    let apps = ws.runningApplications();
    let lower_name = app_name.to_lowercase();

    for app in apps.iter() {
        let matches = app
            .bundleIdentifier()
            .map(|b| b.to_string().to_lowercase() == lower_name)
            .unwrap_or(false)
            || app
                .localizedName()
                .map(|n| n.to_string().to_lowercase() == lower_name)
                .unwrap_or(false);

        if matches {
            if app.terminate() {
                return Ok(());
            } else {
                return Err(DesktopError::InputFailed(format!(
                    "quit_app: '{}' refused to terminate",
                    app_name
                )));
            }
        }
    }

    Err(DesktopError::InputFailed(format!(
        "quit_app: no running application matching '{}'",
        app_name
    )))
}

/// List all currently running applications.
pub fn list_running_apps() -> Result<Vec<AppInfo>> {
    let ws = NSWorkspace::sharedWorkspace();
    let apps: objc2::rc::Retained<NSArray<NSRunningApplication>> = ws.runningApplications();

    let mut result = Vec::new();
    for app in apps.iter() {
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
}
