//! Platform-specific sandbox drivers.

use std::sync::Arc;

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "windows")]
pub mod windows;

pub mod common;

#[must_use]
pub fn create_platform_driver_from_config(
    config: &crate::sandbox::config::SandboxConfig,
) -> Arc<dyn crate::sandbox::driver::OsSandboxDriverTrait> {
    create_platform_driver_with_config(
        &config.linux,
        &config.windows,
        &config.deny_read_globs,
        &config.allow_unix_sockets,
        config.dangerously_allow_all_unix_sockets,
        config.allow_local_binding,
    )
}

#[must_use]
pub fn create_platform_driver_with_config(
    linux_config: &crate::sandbox::config::LinuxSandboxConfig,
    windows_config: &crate::sandbox::config::WindowsSandboxConfig,
    deny_read_globs: &[String],
    allow_unix_sockets: &[std::path::PathBuf],
    dangerously_allow_all_unix_sockets: bool,
    allow_local_binding: bool,
) -> Arc<dyn crate::sandbox::driver::OsSandboxDriverTrait> {
    #[cfg(target_os = "macos")]
    {
        use macos::seatbelt::{SeatbeltDriver, SeatbeltOptions};
        let _ = (linux_config, windows_config);
        Arc::new(SeatbeltDriver::with_options(SeatbeltOptions {
            deny_read_globs: deny_read_globs.to_vec(),
            allow_unix_sockets: allow_unix_sockets.to_vec(),
            dangerously_allow_all_unix_sockets,
            allow_local_binding,
        }))
    }
    #[cfg(target_os = "linux")]
    {
        use linux::bwrap::{BubblewrapDriver, LinuxSandboxOptions};
        // `deny_read_globs` + unix-socket allowlist are macOS-seatbelt
        // floors/widenings today; bwrap allows AF_UNIX for local IPC at the
        // seccomp layer already (see sandbox_init.rs) and has no native
        // glob-deny primitive (landlock enforcement deferred).
        let _ = (
            windows_config,
            deny_read_globs,
            allow_unix_sockets,
            dangerously_allow_all_unix_sockets,
            allow_local_binding,
        );
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
        // deny_read_globs is consumed by with_options_and_deny_globs below;
        // allow_unix_sockets / dangerously_allow_all_unix_sockets are a
        // macOS-seatbelt-only floor (no Windows WFP equivalent yet).
        let _ = (
            linux_config,
            allow_unix_sockets,
            dangerously_allow_all_unix_sockets,
            allow_local_binding,
        );
        let options = WindowsSandboxOptions {
            use_restricted_token: windows_config.use_restricted_token,
            require_restricted_token: windows_config.require_restricted_token,
            use_app_container: windows_config.use_app_container,
            require_app_container: windows_config.require_app_container,
            use_job_object: windows_config.use_job_object,
            max_active_processes: windows_config.max_active_processes,
        };
        // Cycle 7: thread the deny-read glob floor (previously dropped here)
        // into the AppContainer init so secrets inside the workspace get
        // explicit deny-read ACEs — at parity with the macOS seatbelt floor.
        Arc::new(WindowsSandboxDriver::with_options_and_deny_globs(
            options,
            deny_read_globs.to_vec(),
        ))
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = (
            linux_config,
            windows_config,
            deny_read_globs,
            allow_unix_sockets,
            dangerously_allow_all_unix_sockets,
            allow_local_binding,
        );
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
