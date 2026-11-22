#[cfg(target_os = "linux")]
pub(crate) use nsh_platform_linux::{run_app, AppOptions, AppSource, Root};

#[cfg(target_os = "macos")]
pub(crate) use nsh_platform_macos::{run_app, AppOptions, AppSource, Root};

#[cfg(target_os = "windows")]
pub(crate) use nsh_platform_windows::{run_app, AppOptions, AppSource, Root};
