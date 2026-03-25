//! App lifecycle via NSWorkspace + NSRunningApplication.

use aleph_desktop::system_types::AppInfo;
use aleph_desktop::Result;

pub fn launch_app(_app_name: &str) -> Result<()> {
    todo!("workspace::launch_app — implement with NSWorkspace")
}

pub fn quit_app(_app_name: &str) -> Result<()> {
    todo!("workspace::quit_app — implement with NSRunningApplication")
}

pub fn list_running_apps() -> Result<Vec<AppInfo>> {
    todo!("workspace::list_running_apps — implement with NSWorkspace")
}
