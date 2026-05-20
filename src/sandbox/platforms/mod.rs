//! Platform-specific sandbox drivers.

use std::sync::Arc;

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "windows")]
pub mod windows;

pub mod common;

pub fn create_platform_driver() -> Arc<dyn crate::sandbox::driver::OsSandboxDriverTrait> {
    #[cfg(target_os = "macos")]
    {
        Arc::new(macos::seatbelt::SeatbeltDriver::new())
    }
    #[cfg(target_os = "linux")]
    {
        Arc::new(linux::bwrap::BubblewrapDriver::new())
    }
    #[cfg(target_os = "windows")]
    {
        Arc::new(windows::driver::WindowsSandboxDriver::new())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        Arc::new(UnsupportedDriver)
    }
}

pub fn create_platform_driver_from_config(
    config: &crate::sandbox::config::SandboxConfig,
) -> Arc<dyn crate::sandbox::driver::OsSandboxDriverTrait> {
    create_platform_driver_with_config(&config.linux, &config.windows)
}

pub fn create_platform_driver_with_config(
    linux_config: &crate::sandbox::config::LinuxSandboxConfig,
    windows_config: &crate::sandbox::config::WindowsSandboxConfig,
) -> Arc<dyn crate::sandbox::driver::OsSandboxDriverTrait> {
    #[cfg(target_os = "macos")]
    {
        let _ = (linux_config, windows_config);
        Arc::new(macos::seatbelt::SeatbeltDriver::new())
    }
    #[cfg(target_os = "linux")]
    {
        use linux::bwrap::{BubblewrapDriver, LinuxSandboxOptions};
        let _ = windows_config;
        let options = LinuxSandboxOptions {
            mount_proc: linux_config.mount_proc,
            no_new_privs: linux_config.no_new_privs,
            include_platform_defaults: linux_config.include_platform_defaults,
            require_landlock: linux_config.require_landlock,
            cgroup_enabled: linux_config.cgroup_enabled,
            require_cgroups: linux_config.require_cgroups,
            cpu_quota_percent: linux_config.cpu_quota_percent,
            max_pids: linux_config.max_pids,
        };
        Arc::new(BubblewrapDriver::with_options(options))
    }
    #[cfg(target_os = "windows")]
    {
        use windows::driver::{WindowsSandboxDriver, WindowsSandboxOptions};
        let _ = linux_config;
        let options = WindowsSandboxOptions {
            use_restricted_token: windows_config.use_restricted_token,
            require_restricted_token: windows_config.require_restricted_token,
        };
        Arc::new(WindowsSandboxDriver::with_options(options))
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = (linux_config, _windows_config);
        Arc::new(UnsupportedDriver)
    }
}

pub struct UnsupportedDriver;

#[async_trait::async_trait]
impl crate::sandbox::driver::OsSandboxDriverTrait for UnsupportedDriver {
    fn platform(&self) -> &'static str {
        "unsupported"
    }

    fn is_supported(&self) -> bool {
        false
    }

    fn profile_for(
        &self,
        _capabilities: &crate::sandbox::capabilities::SandboxCapabilities,
        _cwd: &std::path::Path,
    ) -> Result<crate::sandbox::driver::OsSandboxProfile, crate::sandbox::command::SandboxError>
    {
        Err(crate::sandbox::command::SandboxError::Other(
            "sandbox not supported on this platform".into(),
        ))
    }

    async fn run(
        &self,
        _program: &str,
        _args: &[std::string::String],
        _env: &std::collections::HashMap<String, String>,
        _stdin: Option<&[u8]>,
        _cwd: &std::path::Path,
        _profile: &crate::sandbox::driver::OsSandboxProfile,
        _timeout: std::time::Duration,
        _max_output_bytes: usize,
    ) -> Result<crate::sandbox::command::SandboxOutput, crate::sandbox::command::SandboxError> {
        Err(crate::sandbox::command::SandboxError::Other(
            "sandbox not supported on this platform".into(),
        ))
    }
}
