use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use tokio::process::Command;
use tracing::{debug, warn};

use crate::sandbox::capabilities::SandboxCapabilities;
use crate::sandbox::command::{SandboxError, SandboxOutput};
use crate::sandbox::driver::{OsSandboxDriverTrait, OsSandboxProfile};
use crate::sandbox::platforms::common::{is_wsl, wsl_version, LINUX_PLATFORM_DEFAULT_READ_ROOTS};
use crate::sandbox::policy::{FsPolicy, NetworkPolicy, ProcessPolicy, SandboxPolicy};

const BWRAP_CANDIDATES: [&str; 2] = ["/usr/bin/bwrap", "/usr/local/bin/bwrap"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinuxSandboxOptions {
    pub mount_proc: bool,
    pub no_new_privs: bool,
    pub include_platform_defaults: bool,
    /// SP-2: when `true`, sandbox-init exits non-zero if the kernel
    /// does not expose landlock ABI ≥ 1 (kernels < 5.13). Defaults to
    /// `false` → soft-degrade with a warning, keeping bwrap + seccomp +
    /// rlimit active. Hardened production deployments can flip this on.
    pub require_landlock: bool,

    /// SP-5: try to create a cgroup v2 sub-cgroup around the sandboxed
    /// process to enforce RSS-based memory + CPU + pid limits. Disabled
    /// → cgroup is never attempted (RLIMIT_AS still applies).
    pub cgroup_enabled: bool,

    /// SP-5: when `true`, the driver fails (rather than soft-degrades)
    /// if cgroup v2 setup is impossible (e.g. no delegation).
    pub require_cgroups: bool,

    /// SP-5: `cpu.max` quota as a percentage of one CPU. See
    /// [`crate::sandbox::cgroup_v2::cpu_quota_max_line`].
    pub cpu_quota_percent: Option<u32>,

    /// SP-5: `pids.max` ceiling.
    pub max_pids: Option<u32>,
}

impl Default for LinuxSandboxOptions {
    fn default() -> Self {
        Self {
            mount_proc: true,
            no_new_privs: true,
            include_platform_defaults: true,
            require_landlock: false,
            cgroup_enabled: true,
            require_cgroups: false,
            cpu_quota_percent: None,
            max_pids: Some(200),
        }
    }
}

#[derive(Debug, Clone)]
pub struct BubblewrapDriver {
    options: LinuxSandboxOptions,
}

impl BubblewrapDriver {
    pub fn new() -> Self {
        Self {
            options: LinuxSandboxOptions::default(),
        }
    }

    pub fn with_options(options: LinuxSandboxOptions) -> Self {
        Self { options }
    }

    fn find_bwrap(&self) -> Option<PathBuf> {
        for path in &BWRAP_CANDIDATES {
            let p = PathBuf::from(path);
            if p.is_file() {
                return Some(p);
            }
        }

        if let Ok(paths) = std::env::var("PATH") {
            for dir in std::env::split_paths(&paths) {
                let candidate = dir.join("bwrap");
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }

        None
    }

    fn check_wsl(&self) {
        if is_wsl() {
            match wsl_version() {
                Some(1) => {
                    warn!(
                        "WSL1 detected. Bubblewrap sandbox may not work correctly \
                         due to lack of proper Linux namespace support. \
                         Consider upgrading to WSL2."
                    );
                }
                Some(2) => {
                    debug!("WSL2 detected. Bubblewrap sandbox should work correctly.");
                }
                _ => {
                    warn!(
                        "WSL detected but version unknown. Sandbox behavior may be unpredictable."
                    );
                }
            }
        }
    }

    fn generate_args(
        &self,
        policy: &SandboxPolicy,
        cwd: &Path,
    ) -> Result<Vec<String>, SandboxError> {
        self.check_wsl();

        let mut args = Vec::new();

        args.push("--new-session".into());
        args.push("--die-with-parent".into());
        args.push("--unshare-user".into());

        if !policy.process.allow_fork {
            args.push("--unshare-pid".into());
            args.push("--cap-drop".into());
            args.push("ALL".into());
        }

        match &policy.network {
            NetworkPolicy::None => {
                args.push("--unshare-net".into());
            }
            NetworkPolicy::AllowAll => {}
            NetworkPolicy::AllowHosts(hosts) => {
                // Workspace pre-resolution (see `src/sandbox/dns.rs`) has
                // already turned every hostname into an IP literal by the
                // time we get here, so the rejection message can be
                // precise about which IPs *would* be allowed if a
                // future cycle wires per-host enforcement.
                //
                // Why not enforced yet: kernel-level per-IP egress
                // filtering on Linux requires CAP_NET_ADMIN to install
                // nftables rules inside the bwrap netns, which we don't
                // hold in the host process. Landlock-net (kernel 6.7+)
                // only filters by TCP port, not by IP, so it can't
                // express this policy. A managed-proxy mode (with
                // HTTP_PROXY env injection) is the most plausible
                // single-cycle deliverable; tracked as deferred work.
                let allowlist = hosts.join(", ");
                return Err(SandboxError::UnsupportedPolicy {
                    platform: "linux/bwrap",
                    feature: "NetworkPolicy::AllowHosts".into(),
                    reason: format!(
                        "per-host egress filtering on Linux is not yet enforced. \
                         Workspace pre-resolved the allowlist to [{allowlist}]; a future \
                         cycle will land enforcement via a managed proxy or nftables \
                         in a CAP_NET_ADMIN-capable user namespace. For now, use \
                         AllowAll (unfiltered) or None (no network). Tracked in \
                         docs/reference/SANDBOX.md § Network Filtering."
                    ),
                });
            }
            NetworkPolicy::ProxyOnly { .. } => {
                return Err(SandboxError::UnsupportedPolicy {
                    platform: "linux/bwrap",
                    feature: "NetworkPolicy::ProxyOnly".into(),
                    reason: "proxy-only mode requires a managed proxy backend on the host \
                             namespace (deferred to spec SP-4). Use AllowAll or None."
                        .into(),
                });
            }
        }

        self.add_fs_args(&mut args, &policy.filesystem, cwd)?;

        if self.options.mount_proc {
            args.push("--proc".into());
            args.push("/proc".into());
        }
        args.push("--dev".into());
        args.push("/dev".into());

        let cwd_str = cwd.to_str().ok_or_else(|| {
            SandboxError::ProfileGeneration("workspace path contains invalid UTF-8".into())
        })?;
        args.push("--chdir".into());
        args.push(cwd_str.into());

        args.push("--".into());

        Ok(args)
    }

    fn add_fs_args(
        &self,
        args: &mut Vec<String>,
        fs: &FsPolicy,
        cwd: &Path,
    ) -> Result<(), SandboxError> {
        match fs {
            FsPolicy::WorkspaceOnly => {
                args.push("--tmpfs".into());
                args.push("/".into());
                args.push("--dev".into());
                args.push("/dev".into());
                args.push("--dir".into());
                args.push("/tmp".into());
                args.push("--tmpfs".into());
                args.push("/tmp".into());

                if self.options.include_platform_defaults {
                    for root in LINUX_PLATFORM_DEFAULT_READ_ROOTS {
                        if Path::new(root).exists() {
                            args.push("--ro-bind".into());
                            args.push(root.to_string());
                            args.push(root.to_string());
                        }
                    }
                }

                let cwd_str = cwd.to_str().ok_or_else(|| {
                    SandboxError::ProfileGeneration("workspace path contains invalid UTF-8".into())
                })?;
                args.push("--bind".into());
                args.push(cwd_str.into());
                args.push(cwd_str.into());
                push_metadata_protection_args(args, std::iter::once(cwd));
            }
            FsPolicy::ReadPaths(paths) => {
                let cwd_str = cwd.to_str().ok_or_else(|| {
                    SandboxError::ProfileGeneration("workspace path contains invalid UTF-8".into())
                })?;
                args.push("--bind".into());
                args.push(cwd_str.into());
                args.push(cwd_str.into());

                for path in paths {
                    let path_str = path.to_str().ok_or_else(|| {
                        SandboxError::ProfileGeneration(format!(
                            "path contains invalid UTF-8: {}",
                            path.display()
                        ))
                    })?;
                    args.push("--ro-bind".into());
                    args.push(path_str.into());
                    args.push(path_str.into());
                }
                push_metadata_protection_args(args, std::iter::once(cwd));
            }
            FsPolicy::WritePaths(paths) => {
                let cwd_str = cwd.to_str().ok_or_else(|| {
                    SandboxError::ProfileGeneration("workspace path contains invalid UTF-8".into())
                })?;
                args.push("--bind".into());
                args.push(cwd_str.into());
                args.push(cwd_str.into());

                for path in paths {
                    let path_str = path.to_str().ok_or_else(|| {
                        SandboxError::ProfileGeneration(format!(
                            "path contains invalid UTF-8: {}",
                            path.display()
                        ))
                    })?;
                    args.push("--bind".into());
                    args.push(path_str.into());
                    args.push(path_str.into());
                }
                // Protect metadata in cwd AND every additional writable
                // path. bwrap evaluates mounts in order: later mounts
                // shadow earlier ones, so emitting `--ro-bind-try` after
                // the writable `--bind` is what makes these read-only.
                let mut all = vec![cwd];
                all.extend(paths.iter().map(|p| p.as_path()));
                push_metadata_protection_args(args, all.into_iter());
            }
            FsPolicy::FullRead { exclude } => {
                args.push("--ro-bind".into());
                args.push("/".into());
                args.push("/".into());
                args.push("--dev".into());
                args.push("/dev".into());

                for path in exclude {
                    let path_str = path.to_str().ok_or_else(|| {
                        SandboxError::ProfileGeneration(format!(
                            "path contains invalid UTF-8: {}",
                            path.display()
                        ))
                    })?;
                    args.push("--tmpfs".into());
                    args.push(path_str.into());
                }
            }
            FsPolicy::FullWrite { exclude } => {
                args.push("--bind".into());
                args.push("/".into());
                args.push("/".into());
                args.push("--dev".into());
                args.push("/dev".into());

                for path in exclude {
                    let path_str = path.to_str().ok_or_else(|| {
                        SandboxError::ProfileGeneration(format!(
                            "path contains invalid UTF-8: {}",
                            path.display()
                        ))
                    })?;
                    args.push("--tmpfs".into());
                    args.push(path_str.into());
                }
            }
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn build_args_for_test(
        &self,
        policy: &crate::sandbox::policy::SandboxPolicy,
        cwd: &std::path::Path,
    ) -> Result<Vec<String>, SandboxError> {
        // Test entry-point: feeds policy + cwd into the same path the
        // driver uses, so unit tests can assert on the generated
        // argument vector without depending on a real bubblewrap.
        let mut args = Vec::new();
        self.add_fs_args(&mut args, &policy.filesystem, cwd)?;
        Ok(args)
    }

    fn apply_no_new_privs(&self, cmd: &mut Command) {
        if self.options.no_new_privs {
            debug!("Applying PR_SET_NO_NEW_PRIVS via pre-exec");
            unsafe {
                cmd.pre_exec(|| {
                    // SAFETY: prctl with PR_SET_NO_NEW_PRIVS is a well-defined
                    // Linux syscall that cannot fail with valid arguments.
                    let rc = libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0);
                    if rc != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }
    }

    /// Apply `setrlimit(RLIMIT_AS)` on the bwrap process; the limit is
    /// inherited by the eventual target binary via exec.
    fn apply_memory_rlimit(cmd: &mut Command, mb: u64) {
        let bytes = mb.saturating_mul(1024 * 1024);
        unsafe {
            // SAFETY: setrlimit with RLIMIT_AS is async-signal-safe and
            // well-defined. No allocator, mutex, or other handler-unsafe
            // call is made between fork and exec.
            cmd.pre_exec(move || {
                let rlim = libc::rlimit {
                    rlim_cur: bytes as libc::rlim_t,
                    rlim_max: bytes as libc::rlim_t,
                };
                if libc::setrlimit(libc::RLIMIT_AS, &rlim) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
}

/// Append codex-inspired metadata-protection mounts to a bubblewrap
/// argument vector. For every writable root, each protected subpath
/// (e.g. `.git`, `.aleph`) is remounted read-only via `--ro-bind-try`,
/// which silently no-ops when the source does not exist — safe for
/// brand-new workspaces. Must be emitted *after* the writable `--bind`
/// because bwrap mounts override in declaration order.
fn push_metadata_protection_args<'a, I>(args: &mut Vec<String>, writable_roots: I)
where
    I: IntoIterator<Item = &'a std::path::Path>,
{
    let roots: Vec<&std::path::Path> = writable_roots.into_iter().collect();
    let protected =
        crate::sandbox::protected_paths::protected_paths_for(roots.iter().copied());
    for path in protected {
        let Some(path_str) = path.to_str() else { continue };
        args.push("--ro-bind-try".into());
        args.push(path_str.into());
        args.push(path_str.into());
    }
}

#[async_trait]
impl OsSandboxDriverTrait for BubblewrapDriver {
    fn platform(&self) -> &'static str {
        "linux/bwrap"
    }

    fn is_supported(&self) -> bool {
        self.find_bwrap().is_some()
    }

    fn profile_for(
        &self,
        capabilities: &SandboxCapabilities,
        cwd: &Path,
    ) -> Result<OsSandboxProfile, SandboxError> {
        let policy = SandboxPolicy::from(capabilities);
        let args = self.generate_args(&policy, cwd)?;
        let contents = args.join("\n");
        // SP-2: serialize the landlock/seccomp policy for sandbox-init.
        // The init binary applies these inside bwrap's namespace before
        // exec'ing the target.
        let init_policy = crate::sandbox::sandbox_init::policy_from_capabilities(
            capabilities,
            cwd,
            self.options.require_landlock,
        );
        let init_policy_json = serde_json::to_string(&init_policy)
            .map_err(|e| SandboxError::ProfileGeneration(format!("init policy serialize: {e}")))?;
        Ok(OsSandboxProfile {
            contents,
            max_memory_mb: policy.process.max_memory_mb,
            linux_init_policy: Some(init_policy_json),
            windows_init_policy: None,
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
        let bwrap_path = self
            .find_bwrap()
            .ok_or_else(|| SandboxError::ExecutionFailed("bubblewrap (bwrap) not found".into()))?;

        let mut bwrap_args: Vec<String> =
            profile.contents.lines().map(|s| s.to_string()).collect();

        // SP-2: bind-mount the currently-running aleph-server binary
        // read-only inside the bwrap namespace at a fixed path, then
        // launch it as `sandbox-init` to apply landlock + seccomp before
        // exec'ing the target. Skipped when the profile has no init
        // policy (e.g. a future caller using OsSandboxProfile in a
        // non-bwrap path).
        let init_policy_json = profile.linux_init_policy.as_deref();
        if init_policy_json.is_some() {
            let aleph_exe = std::env::current_exe()
                .and_then(std::fs::canonicalize)
                .map_err(|e| SandboxError::ExecutionFailed(
                    format!("cannot determine aleph-server path: {e}"),
                ))?;
            bwrap_args.push("--ro-bind".into());
            bwrap_args.push(aleph_exe.to_string_lossy().into_owned());
            bwrap_args.push("/aleph-sandbox-init".into());
        }

        debug!("running bubblewrap with {} arguments", bwrap_args.len());

        let mut cmd = Command::new(bwrap_path);
        cmd.args(&bwrap_args);

        // Compose the target invocation. With an init policy in hand we
        // exec the target indirectly:
        //   /aleph-sandbox-init sandbox-init --policy <json> -- <program> <args...>
        // Otherwise fall back to direct exec (matches pre-SP-2 behavior).
        match init_policy_json {
            Some(policy) => {
                cmd.arg("/aleph-sandbox-init")
                    .arg("sandbox-init")
                    .arg("--policy")
                    .arg(policy)
                    .arg("--")
                    .arg(program)
                    .args(args);
            }
            None => {
                cmd.arg(program).args(args);
            }
        }

        cmd.current_dir(cwd)
            .env_clear()
            .envs(env)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .stdin(std::process::Stdio::piped());

        self.apply_no_new_privs(&mut cmd);
        if let Some(mb) = profile.max_memory_mb {
            Self::apply_memory_rlimit(&mut cmd, mb);
        }

        // SP-5: build cgroup v2 sub-cgroup before spawn so the child can
        // be attached via pre_exec. The scope is owned by this `run()`
        // frame; its Drop fires after `wait_with_output()` completes and
        // rmdir's the cgroup directory. RLIMIT_AS (above) continues to
        // apply as a fallback when cgroups are unavailable.
        let _cgroup_scope: Option<crate::sandbox::cgroup_v2::CgroupV2Scope> = if self
            .options
            .cgroup_enabled
        {
            let limits = crate::sandbox::cgroup_v2::CgroupV2Limits {
                memory_mb: profile.max_memory_mb,
                cpu_quota_percent: self.options.cpu_quota_percent,
                max_pids: self.options.max_pids,
            };
            let scope = crate::sandbox::cgroup_v2::CgroupV2Scope::try_create(limits);
            if scope.is_none() && self.options.require_cgroups {
                return Err(SandboxError::ExecutionFailed(
                    "cgroup v2 setup failed and require_cgroups=true".into(),
                ));
            }
            // Capture the child's cgroup.procs path by clone so pre_exec
            // doesn't share Arc state with the parent (which would
            // double-leak the ref count across fork+exec).
            if let Some(ref s) = scope {
                let procs_path = s.procs_path();
                unsafe {
                    cmd.pre_exec(move || {
                        let pid = std::process::id();
                        std::fs::write(&procs_path, pid.to_string().as_bytes())
                    });
                }
            }
            scope
        } else {
            None
        };

        let mut child = cmd
            .spawn()
            .map_err(|e| SandboxError::ExecutionFailed(format!("failed to spawn bwrap: {e}")))?;

        if let Some(stdin_data) = stdin {
            if let Some(mut child_stdin) = child.stdin.take() {
                use tokio::io::AsyncWriteExt;
                child_stdin
                    .write_all(stdin_data)
                    .await
                    .map_err(|e| SandboxError::Io(format!("stdin write failed: {e}")))?;
            }
        }

        let start = std::time::Instant::now();
        let result = tokio::time::timeout(timeout, child.wait_with_output()).await;

        let elapsed_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok(Ok(output)) => {
                let stdout_truncated = output.stdout.len() > max_output_bytes;
                let stderr_truncated = output.stderr.len() > max_output_bytes;
                let stdout = if stdout_truncated {
                    output.stdout[..max_output_bytes].to_vec()
                } else {
                    output.stdout
                };
                let stderr = if stderr_truncated {
                    output.stderr[..max_output_bytes].to_vec()
                } else {
                    output.stderr
                };

                Ok(SandboxOutput {
                    stdout,
                    stderr,
                    exit_code: output.status.code(),
                    signal: None,
                    truncated: stdout_truncated || stderr_truncated,
                    duration_ms: elapsed_ms,
                })
            }
            Ok(Err(e)) => Err(SandboxError::ExecutionFailed(format!(
                "bwrap execution error: {e}"
            ))),
            Err(_) => Err(SandboxError::Timeout { elapsed_ms }),
        }
    }
}

impl Default for BubblewrapDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl BubblewrapDriver {
    pub fn options(&self) -> LinuxSandboxOptions {
        self.options
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn bubblewrap_driver_platform() {
        let driver = BubblewrapDriver::new();
        assert_eq!(driver.platform(), "linux/bwrap");
    }

    #[test]
    fn generate_args_workspace_only() {
        let driver = BubblewrapDriver::new();
        let policy = SandboxPolicy::default();
        let cwd = Path::new("/tmp/test-workspace");
        let args = driver.generate_args(&policy, cwd).unwrap();

        assert!(args.contains(&"--new-session".into()));
        assert!(args.contains(&"--die-with-parent".into()));
        assert!(args.contains(&"--unshare-user".into()));
        assert!(args.contains(&"--unshare-net".into()));
        assert!(args.contains(&"--unshare-pid".into()));
        assert!(args.contains(&"--cap-drop".into()));
        assert!(args.contains(&"ALL".into()));
        assert!(args.contains(&"--tmpfs".into()));
        assert!(args.contains(&"/".into()));
        assert!(args.contains(&"--bind".into()));
        assert!(args.contains(&"/tmp/test-workspace".into()));
    }

    #[test]
    fn generate_args_workspace_only_without_platform_defaults() {
        let driver = BubblewrapDriver::with_options(LinuxSandboxOptions {
            mount_proc: true,
            no_new_privs: true,
            include_platform_defaults: false,
        });
        let policy = SandboxPolicy::default();
        let cwd = Path::new("/tmp/test-workspace");
        let args = driver.generate_args(&policy, cwd).unwrap();

        assert!(args.contains(&"--tmpfs".into()));
        assert!(!args.iter().any(|a| a == "/usr"));
    }

    #[test]
    fn workspace_only_adds_ro_bind_try_for_metadata_paths() {
        let driver = BubblewrapDriver::new();
        let policy = SandboxPolicy::default();
        let cwd = Path::new("/tmp/ws");
        let args = driver.generate_args(&policy, cwd).unwrap();

        // The writable --bind comes first, then --ro-bind-try for each
        // protected subpath — bwrap mounts override in order, so the
        // ro-bind must follow.
        let bind_idx = args
            .windows(3)
            .position(|w| w == ["--bind", "/tmp/ws", "/tmp/ws"])
            .expect("workspace --bind must be present");
        let git_idx = args
            .windows(3)
            .position(|w| w == ["--ro-bind-try", "/tmp/ws/.git", "/tmp/ws/.git"])
            .expect(".git --ro-bind-try must be present");
        let aleph_idx = args
            .windows(3)
            .position(|w| w == ["--ro-bind-try", "/tmp/ws/.aleph", "/tmp/ws/.aleph"])
            .expect(".aleph --ro-bind-try must be present");
        assert!(bind_idx < git_idx);
        assert!(bind_idx < aleph_idx);
    }

    #[test]
    fn write_paths_protects_metadata_in_each_writable_root() {
        let driver = BubblewrapDriver::new();
        let policy = SandboxPolicy {
            filesystem: FsPolicy::WritePaths(vec![PathBuf::from("/tmp/extra")]),
            ..Default::default()
        };
        let cwd = Path::new("/tmp/ws");
        let args = driver.generate_args(&policy, cwd).unwrap();

        // Both cwd and the additional writable path get metadata protection.
        for root in ["/tmp/ws", "/tmp/extra"] {
            for sub in [".git", ".aleph", ".codex", ".agents"] {
                let p = format!("{root}/{sub}");
                assert!(
                    args.windows(3)
                        .any(|w| w == ["--ro-bind-try", p.as_str(), p.as_str()]),
                    "missing --ro-bind-try for {p}"
                );
            }
        }
    }

    #[test]
    fn full_write_does_not_auto_protect_metadata() {
        let driver = BubblewrapDriver::new();
        let policy = SandboxPolicy {
            filesystem: FsPolicy::FullWrite { exclude: vec![] },
            ..Default::default()
        };
        let cwd = Path::new("/tmp/ws");
        let args = driver.generate_args(&policy, cwd).unwrap();
        // FullWrite = danger-full-access; explicit caller contract, no
        // auto metadata protection.
        assert!(
            !args
                .windows(3)
                .any(|w| w == ["--ro-bind-try", "/tmp/ws/.git", "/tmp/ws/.git"])
        );
    }

    #[test]
    fn generate_args_with_read_paths() {
        let driver = BubblewrapDriver::new();
        let policy = SandboxPolicy {
            filesystem: FsPolicy::ReadPaths(vec![PathBuf::from("/etc")]),
            ..Default::default()
        };
        let cwd = Path::new("/tmp/ws");
        let args = driver.generate_args(&policy, cwd).unwrap();

        assert!(args.contains(&"--ro-bind".into()));
        assert!(args.contains(&"/etc".into()));
    }

    #[test]
    fn generate_args_allow_all_network() {
        let driver = BubblewrapDriver::new();
        let policy = SandboxPolicy {
            network: NetworkPolicy::AllowAll,
            ..Default::default()
        };
        let cwd = Path::new("/tmp/ws");
        let args = driver.generate_args(&policy, cwd).unwrap();

        assert!(!args.contains(&"--unshare-net".into()));
    }

    #[test]
    fn generate_args_allow_hosts_returns_unsupported() {
        let driver = BubblewrapDriver::new();
        // By the time `generate_args` runs, workspace pre-resolution
        // (`src/sandbox/dns.rs`) has already turned every hostname into
        // an IP literal — we stuff an IP in here directly to match what
        // the production pipeline would produce.
        let policy = SandboxPolicy {
            network: NetworkPolicy::AllowHosts(vec!["10.0.0.1".into(), "10.0.0.2".into()]),
            ..Default::default()
        };
        let cwd = Path::new("/tmp/ws");
        let err = driver
            .generate_args(&policy, cwd)
            .expect_err("AllowHosts must hard-fail on linux/bwrap");
        match err {
            SandboxError::UnsupportedPolicy {
                platform,
                feature,
                reason,
            } => {
                assert_eq!(platform, "linux/bwrap");
                assert!(feature.contains("AllowHosts"));
                // The rejection message must include each pre-resolved
                // IP so callers can see exactly what would be enforced
                // once the deferred work lands.
                assert!(reason.contains("10.0.0.1"), "got: {reason}");
                assert!(reason.contains("10.0.0.2"), "got: {reason}");
                assert!(
                    reason.contains("SANDBOX.md")
                        || reason.contains("managed proxy")
                        || reason.contains("nftables"),
                    "rejection must point at the documented gap, got: {reason}"
                );
            }
            other => panic!("expected UnsupportedPolicy, got {other:?}"),
        }
    }

    #[test]
    fn generate_args_proxy_only_returns_unsupported() {
        let driver = BubblewrapDriver::new();
        let policy = SandboxPolicy {
            network: NetworkPolicy::ProxyOnly { ports: vec![8080] },
            ..Default::default()
        };
        let cwd = Path::new("/tmp/ws");
        let err = driver
            .generate_args(&policy, cwd)
            .expect_err("ProxyOnly must hard-fail on linux/bwrap");
        assert!(matches!(err, SandboxError::UnsupportedPolicy { .. }));
    }

    #[test]
    fn generate_args_allow_fork() {
        let driver = BubblewrapDriver::new();
        let policy = SandboxPolicy {
            process: ProcessPolicy {
                allow_fork: true,
                timeout_secs: 60,
                max_memory_mb: None,
            },
            ..Default::default()
        };
        let cwd = Path::new("/tmp/ws");
        let args = driver.generate_args(&policy, cwd).unwrap();

        assert!(!args.contains(&"--unshare-pid".into()));
        assert!(!args.contains(&"--cap-drop".into()));
    }

    #[test]
    fn profile_for_threads_max_memory_mb() {
        let driver = BubblewrapDriver::new();
        let caps = SandboxCapabilities {
            max_memory_mb: Some(256),
            ..Default::default()
        };
        let cwd = Path::new("/tmp/ws");
        let profile = driver.profile_for(&caps, cwd).unwrap();
        assert_eq!(profile.max_memory_mb, Some(256));
    }
}
