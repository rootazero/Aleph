//! Platform-specific sandbox drivers.

use std::sync::Arc;

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "windows")]
pub mod windows;

pub mod common;

/// System directories this platform's OS sandbox grants for executing
/// programs, so `utils::shell` can prefer candidates under them.
///
/// A program resolved elsewhere may be one the sandbox cannot run: seatbelt
/// refuses the exec of a symlink it may not read (exit 71) or dyld refuses the
/// libraries (exit 134). On Linux the list is what bwrap binds into a
/// workspace-only sandbox when `include_platform_defaults` is on (the default);
/// with it off nothing is bound and the list is merely a preference. Each arm
/// returns the list its own driver builds the sandbox from, not a copy of it.
/// Windows has no such list — its resolver walks its own ladder
/// (`utils::shell::resolve`).
#[cfg(not(windows))]
#[must_use]
pub fn exec_read_roots() -> &'static [&'static str] {
    #[cfg(target_os = "macos")]
    {
        macos::seatbelt::EXEC_READ_ROOTS
    }
    #[cfg(target_os = "linux")]
    {
        common::LINUX_PLATFORM_DEFAULT_READ_ROOTS
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        &[]
    }
}

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
        // `deny_read_globs` is threaded into the driver, which fails closed
        // (`UnsupportedPolicy`) when it is non-empty: bwrap has no per-access
        // glob-deny primitive, Landlock read grants are additive, and hiding
        // an enumerated snapshot with mounts is unsound. The unix-socket
        // allowlist remains a macOS-seatbelt widening: bwrap already allows
        // AF_UNIX for local IPC at the seccomp layer (see sandbox_init.rs).
        let _ = (
            windows_config,
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
        Arc::new(BubblewrapDriver::with_options_and_deny_globs(
            options,
            deny_read_globs.to_vec(),
        ))
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

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use crate::sandbox::capabilities::SandboxCapabilities;
    use crate::sandbox::command::SandboxError;
    use crate::sandbox::config::SandboxConfig;

    #[test]
    fn configured_deny_read_globs_fail_closed_through_the_factory() {
        // Catches both "guard removed" and "factory stopped threading the
        // floor": without the wiring the driver would produce a profile and
        // run with the configured deny floor silently dropped.
        let mut config = SandboxConfig::default();
        config.deny_read_globs = vec!["**/.env".into()];
        let driver = create_platform_driver_from_config(&config);
        let ws = tempfile::tempdir().unwrap();

        let err = driver
            .profile_for(&SandboxCapabilities::default(), ws.path())
            .expect_err("a configured non-empty deny floor must refuse a Linux profile");
        match err {
            SandboxError::UnsupportedPolicy {
                platform, feature, ..
            } => {
                assert_eq!(platform, "linux/bwrap");
                assert_eq!(feature, "deny_read_globs");
            }
            other => panic!("expected UnsupportedPolicy, got {other:?}"),
        }
    }

    #[test]
    fn empty_deny_read_globs_still_produce_a_profile() {
        // The factory must keep building a working driver when the floor is
        // off. `include_platform_defaults: false` keeps this independent of
        // which OS trees the host happens to have.
        let mut config = SandboxConfig::default();
        config.linux.include_platform_defaults = false;
        let driver = create_platform_driver_from_config(&config);
        let ws = tempfile::tempdir().unwrap();

        let profile = driver
            .profile_for(&SandboxCapabilities::default(), ws.path())
            .expect("empty deny_read_globs must not refuse a profile");
        assert!(
            profile.contents.contains("--bind"),
            "workspace bind must still be generated, got: {}",
            profile.contents
        );
    }
}
