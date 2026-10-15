//! WindowsSandboxDriver — Windows sandbox implementation using
//! Restricted Token + Job Object + ACL.
//!
//! This implementation follows the same architecture as macOS SeatbeltDriver
//! and Linux BubblewrapDriver, mapping SandboxPolicy to Windows-native
//! security mechanisms.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use tracing::debug;

use crate::sandbox::capabilities::SandboxCapabilities;
use crate::sandbox::command::{SandboxError, SandboxOutput};
use crate::sandbox::driver::{OsSandboxDriverTrait, OsSandboxProfile};
use crate::sandbox::platforms::common::run_child_with_drain;
use crate::sandbox::policy::{FsPolicy, NetworkPolicy, SandboxPolicy};
use crate::sandbox::windows_init::WindowsInitPolicy;

/// Driver-side knobs sourced from `WindowsSandboxConfig`.
#[derive(Debug, Clone, Copy)]
pub struct WindowsSandboxOptions {
    pub use_restricted_token: bool,
    pub require_restricted_token: bool,
    pub use_app_container: bool,
    pub require_app_container: bool,
    /// When `false`, the target runs without a Job Object — no
    /// kill-on-close, no UI restrictions, no active-process cap. Honors
    /// `WindowsSandboxConfig.use_job_object`.
    pub use_job_object: bool,
    /// Active-process ceiling for the Job Object when forking is allowed
    /// (`WindowsSandboxConfig.max_active_processes`). A non-forking
    /// command is always pinned to 1 regardless of this value.
    pub max_active_processes: u32,
}

impl Default for WindowsSandboxOptions {
    fn default() -> Self {
        Self {
            use_restricted_token: true,
            require_restricted_token: false,
            use_app_container: true,
            require_app_container: false,
            use_job_object: true,
            max_active_processes: 8,
        }
    }
}

#[derive(Debug, Clone)]
pub struct WindowsSandboxDriver {
    options: WindowsSandboxOptions,
    /// Cycle 7: secret-path globs denied to the sandboxed target's read.
    /// Sourced from `SandboxConfig.deny_read_globs` and threaded into
    /// `WindowsInitPolicy` so the AppContainer init stamps deny-read ACEs.
    /// Kept off `WindowsSandboxOptions` so that struct stays `Copy`.
    deny_read_globs: Vec<String>,
}

impl WindowsSandboxDriver {
    pub fn new() -> Self {
        Self {
            options: WindowsSandboxOptions::default(),
            deny_read_globs: Vec::new(),
        }
    }

    pub fn with_options(options: WindowsSandboxOptions) -> Self {
        Self {
            options,
            deny_read_globs: Vec::new(),
        }
    }

    /// Construct with both the driver knobs and the deny-read glob floor.
    /// Used by `create_platform_driver_with_config` to wire
    /// `SandboxConfig.deny_read_globs` through to the AppContainer init.
    pub fn with_options_and_deny_globs(
        options: WindowsSandboxOptions,
        deny_read_globs: Vec<String>,
    ) -> Self {
        Self {
            options,
            deny_read_globs,
        }
    }

    /// Generate a profile description from the sandbox policy.
    /// On Windows, the profile contains the serialized policy that
    /// will be applied at runtime via Windows APIs.
    fn generate_profile(&self, policy: &SandboxPolicy, cwd: &Path) -> Result<String, SandboxError> {
        let mut lines = Vec::new();

        lines.push("platform=windows/token".to_string());

        match &policy.filesystem {
            FsPolicy::WorkspaceOnly => {
                lines.push("fs=workspace_only".to_string());
                lines.push(format!("cwd={}", cwd.display()));
            }
            FsPolicy::ReadPaths(paths) => {
                lines.push("fs=read_paths".to_string());
                lines.push(format!("cwd={}", cwd.display()));
                for path in paths {
                    lines.push(format!("read={}", path.display()));
                }
            }
            FsPolicy::WritePaths(paths) => {
                lines.push("fs=write_paths".to_string());
                lines.push(format!("cwd={}", cwd.display()));
                for path in paths {
                    lines.push(format!("write={}", path.display()));
                }
            }
            FsPolicy::ReadWritePaths { read, write } => {
                lines.push("fs=read_write_paths".to_string());
                lines.push(format!("cwd={}", cwd.display()));
                for path in read {
                    lines.push(format!("read={}", path.display()));
                }
                for path in write {
                    lines.push(format!("write={}", path.display()));
                }
            }
            FsPolicy::FullRead { exclude } => {
                lines.push("fs=full_read".to_string());
                for path in exclude {
                    lines.push(format!("exclude={}", path.display()));
                }
            }
            FsPolicy::FullWrite { exclude } => {
                lines.push("fs=full_write".to_string());
                for path in exclude {
                    lines.push(format!("exclude={}", path.display()));
                }
            }
        }

        match &policy.network {
            NetworkPolicy::None => {
                lines.push("network=none".to_string());
            }
            NetworkPolicy::AllowAll => {
                lines.push("network=allow_all".to_string());
            }
            NetworkPolicy::AllowHosts(hosts) => {
                // Workspace pre-resolution has already turned hostnames
                // into IP literals (see `src/sandbox/dns.rs`); we expose
                // them in the rejection so callers know which IPs would
                // be allowed under enforcement.
                //
                // Why Cycle 6 Phase A (managed proxy) does NOT help here:
                // AppContainer's default isolation BLOCKS loopback
                // access — that's the well-known "AppContainer loopback
                // isolation" feature. Enabling loopback for the
                // per-execution AppContainer SID requires
                // `CheckNetIsolationEnableLoopback`, which needs admin /
                // SeChangeNotifyPrivilege to add the exemption. Phase A
                // is therefore enabled on macOS only.
                //
                // Path forward: Phase D — WFP filters via a
                // privileged sidecar service (still admin / LocalSystem).
                // Until then, use AllowAll (unfiltered) or None.
                let allowlist = hosts.join(", ");
                return Err(SandboxError::UnsupportedPolicy {
                    platform: "windows/token",
                    feature: "NetworkPolicy::AllowHosts".into(),
                    reason: format!(
                        "per-host egress filtering on Windows is not yet enforced. \
                         Workspace pre-resolved the allowlist to [{allowlist}]. Cycle 6 \
                         Phase A (managed proxy) is macOS-only — AppContainer blocks \
                         loopback to the proxy without admin-level loopback exemption. \
                         Phase D will use WFP filters (admin / LocalSystem). For now, \
                         use AllowAll (unfiltered) or None (no network). Tracked in \
                         docs/reference/SANDBOX.md § Network Filtering."
                    ),
                });
            }
            NetworkPolicy::LoopbackPorts(ports) => {
                // Only the managed launcher can enforce this (per-run WFP
                // confinement keyed by the AppContainer SID); `run()` refuses
                // it. The ports are carried verbatim so `spawn_managed` derives
                // its `NetworkRequest` from the same profile the approval gate
                // saw - never from a second copy.
                lines.push(format!("network={NETWORK_LOOPBACK_PORTS}"));
                for port in ports {
                    lines.push(format!("loopback_port={port}"));
                }
            }
        }

        lines.push(format!("allow_fork={}", policy.process.allow_fork));
        if let Some(max_mem) = policy.process.max_memory_mb {
            lines.push(format!("max_memory_mb={}", max_mem));
        }

        Ok(lines.join("\n"))
    }
}

#[async_trait]
impl OsSandboxDriverTrait for WindowsSandboxDriver {
    fn platform(&self) -> &'static str {
        "windows/token"
    }

    fn is_supported(&self) -> bool {
        cfg!(target_os = "windows")
    }

    fn profile_for(
        &self,
        capabilities: &SandboxCapabilities,
        cwd: &Path,
    ) -> Result<OsSandboxProfile, SandboxError> {
        let policy = SandboxPolicy::from(capabilities);
        let contents = self.generate_profile(&policy, cwd)?;
        // SP-3a/SP-6: serialize the WindowsInitPolicy so run() can wrap
        // the target with `sandbox-init-windows`. Skipped only when
        // BOTH restricted-token AND app-container are disabled — falls
        // through to plain CreateProcessW (cycle 1 behavior).
        let windows_init_policy =
            if self.options.use_restricted_token || self.options.use_app_container {
                let p = WindowsInitPolicy {
                    require_restricted_token: self.options.require_restricted_token,
                    use_app_container: self.options.use_app_container,
                    require_app_container: self.options.require_app_container,
                    app_container_capabilities:
                        crate::sandbox::windows_init::capability_names_for_network(
                            &capabilities.network,
                        ),
                    workspace_path: Some(cwd.to_string_lossy().into_owned()),
                    deny_read_globs: self.deny_read_globs.clone(),
                };
                Some(serde_json::to_string(&p).map_err(|e| {
                    SandboxError::ProfileGeneration(format!("WindowsInitPolicy serialize: {e}"))
                })?)
            } else {
                None
            };
        Ok(OsSandboxProfile {
            contents,
            max_memory_mb: policy.process.max_memory_mb,
            linux_init_policy: None,
            windows_init_policy,
        })
    }

    /// A restricted-token / AppContainer refusal surfaces as
    /// ERROR_ACCESS_DENIED, whose formatted message is "Access is denied".
    fn denial_signatures(&self) -> &'static [&'static str] {
        &["access is denied"]
    }

    /// Managed spawn does NOT go through the `sandbox-init-windows` wrapper:
    /// the service is created parent-side, suspended, in a per-run
    /// AppContainer, so its PID/creation time come from its own handle. It is
    /// still refused until per-run WFP confinement is verified and the process
    /// is elevated (typed blockers, see `managed_launch::assess_managed_launch`).
    fn managed_spawn_preflight(&self) -> Result<(), SandboxError> {
        super::managed_launch::managed_spawn_preflight_with(
            &super::managed_launch::HostFacts::observe(),
        )
    }

    async fn spawn_managed(
        &self,
        program: &str,
        args: &[String],
        env: &HashMap<String, String>,
        cwd: &Path,
        profile: &OsSandboxProfile,
    ) -> Result<crate::sandbox::managed::ManagedChild, SandboxError> {
        // Typed refusal first: nothing is created while any blocker applies.
        self.managed_spawn_preflight()?;
        let (spec, req) = managed_launch_inputs(program, args, env, cwd, profile)?;
        let process = tokio::task::spawn_blocking(move || {
            super::launcher::launch(
                spec,
                super::wfp::WfpBackend,
                &req,
                &super::managed_launch::CURRENT_CAPABILITIES,
                MANAGED_CONFIRM_TIMEOUT,
            )
        })
        .await
        .map_err(|e| SandboxError::ExecutionFailed(format!("managed launch task failed: {e}")))??;
        Ok(crate::sandbox::managed::ManagedChild {
            process: Box::new(process),
            keepalive: None,
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn run(
        &self,
        program: &str,
        args: &[String],
        env: &HashMap<String, String>,
        stdin: Option<&[u8]>,
        cwd: &Path,
        profile: &OsSandboxProfile,
        timeout: Duration,
        max_output_bytes: usize,
    ) -> Result<SandboxOutput, SandboxError> {
        let parsed = parse_profile(&profile.contents)?;
        if parsed.network_mode == NETWORK_LOOPBACK_PORTS {
            // The one-shot path has no per-run WFP confinement; running the
            // target here would either be unconfined or silently unreachable.
            return Err(SandboxError::UnsupportedPolicy {
                platform: "windows/token",
                feature: "NetworkPolicy::LoopbackPorts".into(),
                reason: "exact loopback-port confinement is only available through the \
                         managed launcher (spawn_managed), not one-shot run()"
                    .into(),
            });
        }

        debug!("running Windows sandbox for program: {}", program);

        #[cfg(target_os = "windows")]
        {
            use super::job::SandboxJob;
            use windows_sys::Win32::System::Threading::CREATE_NEW_PROCESS_GROUP;

            // SP-3a: when the profile carries a windows_init_policy,
            // wrap the target with `aleph-server sandbox-init-windows
            // --policy <json> -- <program> <args>`. The init child
            // applies the restricted token + Low IL before launching
            // the target via CreateProcessAsUserW (which inherits the
            // JobObject membership we're about to assign below). Skip
            // when policy is absent (use_restricted_token=false) — the
            // target runs directly under the host token.
            let mut cmd = match &profile.windows_init_policy {
                Some(policy_json) => {
                    let aleph_exe = std::env::current_exe()
                        .and_then(std::fs::canonicalize)
                        .map_err(|e| {
                            SandboxError::ExecutionFailed(format!(
                                "cannot determine aleph-server path: {e}"
                            ))
                        })?;
                    let mut c = tokio::process::Command::new(aleph_exe);
                    c.arg("sandbox-init-windows")
                        .arg("--policy")
                        .arg(policy_json)
                        .arg("--")
                        .arg(program)
                        .args(args);
                    c
                }
                None => {
                    let mut c = tokio::process::Command::new(program);
                    c.args(args);
                    c
                }
            };
            cmd.current_dir(cwd)
                .env_clear()
                .envs(env)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .stdin(std::process::Stdio::piped())
                .creation_flags(
                    CREATE_NEW_PROCESS_GROUP | crate::utils::no_window::CREATE_NO_WINDOW,
                )
                // Belt-and-braces: if our future is dropped (e.g. upstream
                // cancellation) the OS terminates the child instead of
                // leaking it. The job object below provides the same
                // guarantee for descendant processes when enabled.
                .kill_on_drop(true);

            // Job Object is optional — honors
            // WindowsSandboxConfig.use_job_object. When enabled, the
            // active-process ceiling is the configured maximum for a
            // forking command and a hard 1 for a non-forking one.
            // `.max(1)` guards against a `0` misconfiguration that would
            // otherwise make the job kill every process immediately.
            let job: Option<SandboxJob> = if self.options.use_job_object {
                let active_limit = if parsed.allow_fork {
                    self.options.max_active_processes.max(1)
                } else {
                    1
                };
                // SAFETY: The returned SandboxJob owns the job-object handle and closes it on Drop.
                Some(
                    // rust-doctor-disable-next-line unsafe-block-audit
                    unsafe { SandboxJob::new(active_limit, profile.max_memory_mb) }.map_err(
                        |e| SandboxError::ExecutionFailed(format!("job creation failed: {e}")),
                    )?,
                )
            } else {
                None
            };

            let child = cmd.spawn().map_err(|e| {
                SandboxError::ExecutionFailed(format!("failed to spawn process: {e}"))
            })?;

            if let Some(ref job) = job {
                // Fail closed: a job object that cannot be attached enforces
                // NONE of its guarantees (memory ceiling, active-process cap,
                // kill-on-close). Letting the child run unsandboxed while
                // reporting success would silently defeat the sandbox, so on
                // any failure we abort the child and surface the error. The
                // child is terminated by `kill_on_drop(true)` (set above) as
                // `child` drops on the early return.
                let pid = child.id().unwrap_or(0);
                let handle = if pid != 0 {
                    child.raw_handle().unwrap_or(std::ptr::null_mut())
                } else {
                    std::ptr::null_mut()
                };
                let assign = if handle.is_null() {
                    Err("child process handle unavailable for job assignment".to_string())
                } else {
                    // SAFETY: `handle` is a valid, non-null child process handle.
                    // rust-doctor-disable-next-line unsafe-block-audit
                    unsafe { job.assign_process(handle as _) }
                };
                if let Err(e) = assign {
                    return Err(SandboxError::ExecutionFailed(format!(
                        "failed to assign process to job object: {e}"
                    )));
                }
            }

            // `_job` is kept alive across the await: its Drop closes the
            // job-object handle (auto-killing any surviving children) only
            // after the helper has reaped the main child process.
            let _job = job;
            run_child_with_drain(child, stdin, timeout, max_output_bytes).await
        }

        #[cfg(not(target_os = "windows"))]
        {
            let _ = (
                program,
                args,
                env,
                stdin,
                cwd,
                timeout,
                max_output_bytes,
                parsed,
            );
            Err(SandboxError::Other(
                "Windows sandbox driver requires Windows platform".into(),
            ))
        }
    }
}

impl Default for WindowsSandboxDriver {
    fn default() -> Self {
        Self::new()
    }
}

/// `network=` value for [`NetworkPolicy::LoopbackPorts`] profiles.
const NETWORK_LOOPBACK_PORTS: &str = "loopback_ports";

/// How long teardown waits for the job to be confirmed empty.
const MANAGED_CONFIRM_TIMEOUT: Duration = Duration::from_secs(10);

fn managed_refusal(reason: impl Into<String>) -> SandboxError {
    SandboxError::UnsupportedPolicy {
        platform: "windows/token",
        feature: "managed_spawn".into(),
        reason: reason.into(),
    }
}

/// Profile -> launch spec + exact loopback request for the managed launcher.
/// Pure (no OS objects). Refuses anything the strict launch cannot honour:
/// no AppContainer, a non-loopback-port network mode, or a forking service.
fn managed_launch_inputs(
    program: &str,
    args: &[String],
    env: &HashMap<String, String>,
    cwd: &Path,
    profile: &OsSandboxProfile,
) -> Result<
    (
        super::launcher::ManagedLaunchSpec,
        super::managed_launch::NetworkRequest,
    ),
    SandboxError,
> {
    let parsed = parse_profile(&profile.contents)?;
    if parsed.network_mode != NETWORK_LOOPBACK_PORTS {
        return Err(managed_refusal(format!(
            "managed spawn needs an exact loopback-port network grant, got network={:?}",
            parsed.network_mode
        )));
    }
    if parsed.allow_fork {
        return Err(managed_refusal(
            "managed spawn is a single-process contract; allow_fork must be false",
        ));
    }
    let init: WindowsInitPolicy = match &profile.windows_init_policy {
        Some(json) => serde_json::from_str(json).map_err(|e| {
            SandboxError::ProfileGeneration(format!("WindowsInitPolicy parse: {e}"))
        })?,
        None => {
            return Err(managed_refusal(
                "managed spawn requires the AppContainer policy; it never runs the service under the host token",
            ))
        }
    };
    if !init.use_app_container {
        return Err(managed_refusal(
            "managed spawn requires use_app_container; per-run confinement is keyed by the AppContainer SID",
        ));
    }
    let spec = super::launcher::ManagedLaunchSpec {
        program: program.to_string(),
        args: args.to_vec(),
        env: env.clone(),
        cwd: cwd.to_path_buf(),
        app_container_capabilities: init.app_container_capabilities,
        workspace: init.workspace_path,
        deny_read_globs: init.deny_read_globs,
        max_memory_mb: profile.max_memory_mb,
    };
    let req = super::managed_launch::NetworkRequest {
        loopback_ports: parsed.loopback_ports,
        allow_hosts: parsed.allowed_hosts,
    };
    Ok((spec, req))
}

/// Parse a profile string back into policy components.
// rust-doctor-disable-next-line high-cyclomatic-complexity
fn parse_profile(contents: &str) -> Result<ParsedProfile, SandboxError> {
    let mut profile = ParsedProfile::default();

    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if let Some((key, value)) = line.split_once('=') {
            match key {
                "platform" => profile.platform = value.to_string(),
                "fs" => profile.fs_mode = value.to_string(),
                "cwd" => profile.cwd = Some(value.to_string()),
                "read" => profile.read_paths.push(value.to_string()),
                "write" => profile.write_paths.push(value.to_string()),
                "exclude" => profile.exclude_paths.push(value.to_string()),
                "network" => profile.network_mode = value.to_string(),
                "host" => profile.allowed_hosts.push(value.to_string()),
                "port" => {
                    if let Ok(port) = value.parse() {
                        profile.proxy_ports.push(port);
                    }
                }
                "loopback_port" => {
                    // Fail closed: a port we cannot parse must not silently
                    // shrink the request into something narrower/emptier.
                    let port = value.parse().map_err(|_| {
                        SandboxError::ProfileGeneration(format!(
                            "invalid loopback_port in windows profile: {value:?}"
                        ))
                    })?;
                    profile.loopback_ports.push(port);
                }
                "allow_fork" => profile.allow_fork = value == "true",
                "max_memory_mb" => {
                    if let Ok(mb) = value.parse() {
                        profile.max_memory_mb = Some(mb);
                    }
                }
                _ => {}
            }
        }
    }

    Ok(profile)
}

#[derive(Debug, Default)]
struct ParsedProfile {
    platform: String,
    fs_mode: String,
    cwd: Option<String>,
    read_paths: Vec<String>,
    write_paths: Vec<String>,
    exclude_paths: Vec<String>,
    network_mode: String,
    allowed_hosts: Vec<String>,
    proxy_ports: Vec<u16>,
    loopback_ports: Vec<u16>,
    allow_fork: bool,
    max_memory_mb: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Launcher, handle-derived identity and WFP confinement are all still
    /// unproven on this host, so preflight refuses (typed) before any spawn.
    #[tokio::test]
    async fn managed_spawn_is_refused_until_wfp_confinement_is_verified() {
        let driver = WindowsSandboxDriver::new();
        match driver.managed_spawn_preflight() {
            Err(SandboxError::UnsupportedPolicy {
                feature, reason, ..
            }) => {
                assert_eq!(feature, "managed_spawn");
                assert!(!reason.contains("wrapper PID"), "{reason}");
                assert!(reason.contains("arbitration"), "{reason}");
            }
            other => panic!("expected UnsupportedPolicy, got {other:?}"),
        }
        let profile = OsSandboxProfile {
            contents: String::new(),
            max_memory_mb: None,
            linux_init_policy: None,
            windows_init_policy: None,
        };
        let spawned = driver
            .spawn_managed(
                "x",
                &[],
                &HashMap::new(),
                std::path::Path::new("."),
                &profile,
            )
            .await;
        assert!(matches!(
            spawned,
            Err(SandboxError::UnsupportedPolicy { .. })
        ));
    }

    #[test]
    fn windows_driver_platform() {
        let driver = WindowsSandboxDriver::new();
        assert_eq!(driver.platform(), "windows/token");
    }

    #[test]
    fn denial_dialect_is_the_tokens_own_and_not_a_union() {
        let sigs = WindowsSandboxDriver::new().denial_signatures();
        assert_eq!(sigs, &["access is denied"]);
        // Borrowing another backend's dialect would let a Windows run be
        // reported as a denial Windows cannot emit.
        for foreign in ["operation not permitted", "read-only file system"] {
            assert!(
                !sigs.contains(&foreign),
                "{foreign} belongs to another backend"
            );
        }
    }

    #[test]
    fn options_default_enables_job_object_with_sane_limit() {
        // BUG-2 regression: the two job-object knobs must default to the
        // production values and `new()` must adopt them.
        let opts = WindowsSandboxOptions::default();
        assert!(opts.use_job_object);
        assert_eq!(opts.max_active_processes, 8);
        let driver = WindowsSandboxDriver::new();
        assert!(driver.options.use_job_object);
        assert_eq!(driver.options.max_active_processes, 8);
    }

    #[test]
    fn with_options_threads_job_object_knobs() {
        let driver = WindowsSandboxDriver::with_options(WindowsSandboxOptions {
            use_job_object: false,
            max_active_processes: 3,
            ..WindowsSandboxOptions::default()
        });
        assert!(!driver.options.use_job_object);
        assert_eq!(driver.options.max_active_processes, 3);
    }

    #[test]
    fn windows_driver_is_supported_on_windows() {
        let driver = WindowsSandboxDriver::new();
        let _ = driver.is_supported();
    }

    #[test]
    fn generate_profile_workspace_only() {
        let driver = WindowsSandboxDriver::new();
        let policy = SandboxPolicy::default();
        let cwd = Path::new("C:\\workspace");
        let profile = driver.generate_profile(&policy, cwd).unwrap();

        assert!(profile.contains("platform=windows/token"));
        assert!(profile.contains("fs=workspace_only"));
        assert!(profile.contains("cwd=C:\\workspace"));
        assert!(profile.contains("network=none"));
        assert!(profile.contains("allow_fork=false"));
    }

    #[test]
    fn generate_profile_with_read_paths() {
        let driver = WindowsSandboxDriver::new();
        let policy = SandboxPolicy {
            filesystem: FsPolicy::ReadPaths(vec![PathBuf::from("C:\\ProgramData")]),
            ..Default::default()
        };
        let cwd = Path::new("C:\\workspace");
        let profile = driver.generate_profile(&policy, cwd).unwrap();

        assert!(profile.contains("fs=read_paths"));
        assert!(profile.contains("read=C:\\ProgramData"));
    }

    #[test]
    fn generate_profile_allow_all_network() {
        let driver = WindowsSandboxDriver::new();
        let policy = SandboxPolicy {
            network: NetworkPolicy::AllowAll,
            ..Default::default()
        };
        let cwd = Path::new("C:\\workspace");
        let profile = driver.generate_profile(&policy, cwd).unwrap();

        assert!(profile.contains("network=allow_all"));
    }

    #[test]
    fn generate_profile_allow_hosts_returns_unsupported() {
        let driver = WindowsSandboxDriver::new();
        // Workspace pre-resolution feeds an IP-only allowlist; mirror
        // that here so the assertion catches IP-bearing rejections.
        let policy = SandboxPolicy {
            network: NetworkPolicy::AllowHosts(vec!["203.0.113.7".into(), "203.0.113.8".into()]),
            ..Default::default()
        };
        let cwd = Path::new("C:\\workspace");
        let err = driver
            .generate_profile(&policy, cwd)
            .expect_err("AllowHosts must hard-fail on windows/token");
        match err {
            SandboxError::UnsupportedPolicy {
                platform,
                feature,
                reason,
            } => {
                assert_eq!(platform, "windows/token");
                assert!(feature.contains("AllowHosts"));
                assert!(reason.contains("203.0.113.7"), "got: {reason}");
                assert!(reason.contains("203.0.113.8"), "got: {reason}");
                assert!(
                    reason.contains("SANDBOX.md")
                        || reason.contains("WFP")
                        || reason.contains("managed proxy"),
                    "rejection must point at the documented gap, got: {reason}"
                );
            }
            other => panic!("expected UnsupportedPolicy, got {other:?}"),
        }
    }

    #[test]
    fn parse_profile_roundtrip() {
        // Uses AllowAll (supported) so the roundtrip exercises memory limit
        // and write-paths plumbing without tripping the unsupported network
        // guard.
        let driver = WindowsSandboxDriver::new();
        let policy = SandboxPolicy {
            filesystem: FsPolicy::WritePaths(vec![PathBuf::from("C:\\temp")]),
            network: NetworkPolicy::AllowAll,
            process: crate::sandbox::policy::ProcessPolicy {
                allow_fork: true,
                max_memory_mb: Some(512),
            },
        };
        let cwd = Path::new("C:\\workspace");
        let profile_str = driver.generate_profile(&policy, cwd).unwrap();

        let parsed = parse_profile(&profile_str).unwrap();
        assert_eq!(parsed.platform, "windows/token");
        assert_eq!(parsed.fs_mode, "write_paths");
        assert_eq!(parsed.cwd, Some("C:\\workspace".to_string()));
        assert_eq!(parsed.write_paths, vec!["C:\\temp"]);
        assert_eq!(parsed.network_mode, "allow_all");
        assert!(parsed.allow_fork);
        assert_eq!(parsed.max_memory_mb, Some(512));
    }

    #[test]
    fn profile_for_from_capabilities() {
        let driver = WindowsSandboxDriver::new();
        let caps = SandboxCapabilities {
            fs_read: vec!["C:\\ProgramData".into()],
            ..Default::default()
        };
        let cwd = Path::new("C:\\workspace");
        let profile = driver.profile_for(&caps, cwd).unwrap();

        assert!(profile.contents.contains("fs=read_paths"));
        assert!(profile.contents.contains("read=C:\\ProgramData"));
    }

    fn managed_profile(network: crate::sandbox::capabilities::NetworkPolicy) -> OsSandboxProfile {
        let caps = SandboxCapabilities {
            network,
            ..SandboxCapabilities::strict()
        };
        WindowsSandboxDriver::new()
            .profile_for(&caps, Path::new(r"C:\ws"))
            .expect("profile")
    }

    /// The exact loopback port reaches the launcher request unchanged and the
    /// spec keeps the AppContainer (no broad capability for loopback ports).
    #[test]
    fn managed_inputs_carry_the_exact_loopback_port_and_the_app_container() {
        use crate::sandbox::capabilities::NetworkPolicy as Net;
        let profile = managed_profile(Net::LoopbackPorts { ports: vec![5173] });
        let (spec, req) = managed_launch_inputs(
            "node.exe",
            &["a".into()],
            &HashMap::new(),
            Path::new(r"C:\ws"),
            &profile,
        )
        .expect("inputs");
        assert_eq!(req.loopback_ports, vec![5173]);
        assert!(req.allow_hosts.is_empty());
        assert!(spec.app_container_capabilities.is_empty());
        assert_eq!(spec.workspace.as_deref(), Some(r"C:\ws"));
        assert_eq!(spec.program, "node.exe");
    }

    #[test]
    fn managed_inputs_refuse_non_loopback_grants_and_missing_app_container() {
        use crate::sandbox::capabilities::NetworkPolicy as Net;
        for net in [Net::None, Net::AllowAll] {
            let profile = managed_profile(net);
            assert!(matches!(
                managed_launch_inputs("x", &[], &HashMap::new(), Path::new("."), &profile),
                Err(SandboxError::UnsupportedPolicy { .. })
            ));
        }
        let mut profile = managed_profile(Net::LoopbackPorts { ports: vec![1] });
        profile.windows_init_policy = None;
        assert!(matches!(
            managed_launch_inputs("x", &[], &HashMap::new(), Path::new("."), &profile),
            Err(SandboxError::UnsupportedPolicy { .. })
        ));
        let mut forking = managed_profile(Net::LoopbackPorts { ports: vec![1] });
        forking.contents.push_str(
            "
allow_fork=true
",
        );
        assert!(matches!(
            managed_launch_inputs("x", &[], &HashMap::new(), Path::new("."), &forking),
            Err(SandboxError::UnsupportedPolicy { .. })
        ));
    }
}
