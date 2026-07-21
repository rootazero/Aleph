use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use tokio::process::Command;
use tracing::{debug, warn};

use crate::sandbox::capabilities::SandboxCapabilities;
use crate::sandbox::command::{SandboxError, SandboxOutput};
use crate::sandbox::driver::{OsSandboxDriverTrait, OsSandboxProfile};
use crate::sandbox::platforms::common::{
    is_wsl, run_child_with_drain, wsl_version, LINUX_PLATFORM_DEFAULT_READ_ROOTS,
};
use crate::sandbox::policy::{FsPolicy, NetworkPolicy, SandboxPolicy};
// Only the test module constructs a ProcessPolicy directly.
#[cfg(test)]
use crate::sandbox::policy::ProcessPolicy;

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

        let mut args = vec![
            "--new-session".into(),
            "--die-with-parent".into(),
            "--unshare-user".into(),
        ];

        // Drop every Linux capability unconditionally. A sandboxed command has
        // no legitimate need for CAP_SYS_ADMIN / CAP_NET_RAW / CAP_DAC_OVERRIDE
        // etc., and capability dropping is orthogonal to fork policy — matching
        // OpenSquilla's unconditional `--cap-drop ALL`. Previously this was
        // gated on `!allow_fork`, so a fork-permitted sandbox silently retained
        // the full capability set. PID-namespace isolation stays fork-gated
        // below, where it can affect a forking workload's process visibility.
        args.push("--cap-drop".into());
        args.push("ALL".into());

        if !policy.process.allow_fork {
            args.push("--unshare-pid".into());
        }

        match &policy.network {
            NetworkPolicy::None => {
                args.push("--unshare-net".into());
            }
            NetworkPolicy::AllowAll => {}
            NetworkPolicy::AllowHosts(hosts) if hosts.iter().all(|h| is_loopback_literal(h)) => {
                // Phase B (netns→UDS→loopback bridge) is live. By the time we
                // get here `WorkspaceSandbox::maybe_spawn_proxy` has spawned
                // the managed proxy + host bridge and collapsed the allowlist
                // to loopback. Unshare the network so the ONLY reachable
                // egress is the netns-side local bridge `sandbox-init` binds
                // on `lo`; it forwards over the bind-mounted UDS to the host
                // proxy, which enforces the real hostname allowlist. The UDS
                // bind-mount itself is added in `run()` (it needs the runtime
                // socket path, which `generate_args` does not see).
                args.push("--unshare-net".into());
            }
            NetworkPolicy::AllowHosts(hosts) => {
                // Raw, non-loopback AllowHosts with no proxy bridge in front.
                // Workspace pre-resolution (`src/sandbox/dns.rs`) has turned
                // hostnames into IP literals, so the message names the IPs
                // that *would* be allowed. True kernel-level per-IP egress
                // still needs nftables under `CAP_NET_ADMIN` (Phase C,
                // deferred). The proxied loopback form above is the supported
                // path; this arm only triggers for a direct driver caller
                // that bypassed `maybe_spawn_proxy`.
                let allowlist = hosts.join(", ");
                return Err(SandboxError::UnsupportedPolicy {
                    platform: "linux/bwrap",
                    feature: "NetworkPolicy::AllowHosts".into(),
                    reason: format!(
                        "per-host egress filtering on Linux is enforced via the managed \
                         proxy + netns→UDS→loopback bridge (Phase B), which \
                         `WorkspaceSandbox` wires by collapsing the allowlist to loopback. \
                         This raw non-loopback allowlist [{allowlist}] reached the driver \
                         without that bridge; true kernel-level per-IP egress (Phase C, \
                         nftables under CAP_NET_ADMIN) remains deferred. Route through \
                         WorkspaceSandbox, or use AllowAll / None. Tracked in \
                         docs/reference/SANDBOX.md § Network Filtering."
                    ),
                });
            }
            NetworkPolicy::ProxyOnly { .. } => {
                return Err(SandboxError::UnsupportedPolicy {
                    platform: "linux/bwrap",
                    feature: "NetworkPolicy::ProxyOnly".into(),
                    reason: "proxy-only (port-list) mode is not wired on Linux; the \
                             supported proxied path is AllowHosts via WorkspaceSandbox. \
                             Use AllowAll or None."
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

    // rust-doctor-disable-next-line high-cyclomatic-complexity
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
                push_metadata_protection_args(args, std::iter::once(cwd))?;
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
                push_metadata_protection_args(args, std::iter::once(cwd))?;
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
                push_metadata_protection_args(args, std::iter::once(cwd))?;
            }
            FsPolicy::ReadWritePaths { read, write } => {
                let cwd_str = cwd.to_str().ok_or_else(|| {
                    SandboxError::ProfileGeneration("workspace path contains invalid UTF-8".into())
                })?;
                args.push("--bind".into());
                args.push(cwd_str.into());
                args.push(cwd_str.into());

                for path in read {
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
                for path in write {
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
                all.extend(write.iter().map(|p| p.as_path()));
                push_metadata_protection_args(args, all)?;
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
            // SAFETY: prctl(PR_SET_NO_NEW_PRIVS) is a well-defined Linux syscall.
            // rust-doctor-disable-next-line unsafe-block-audit
            unsafe {
                cmd.pre_exec(|| {
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
        // SAFETY: setrlimit(RLIMIT_AS) is async-signal-safe and well-defined.
        // rust-doctor-disable-next-line unsafe-block-audit
        unsafe {
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

/// Is `host` an IPv4/IPv6 loopback literal or `localhost`? Used to detect the
/// proxy-collapsed AllowHosts form (`["127.0.0.1"]`) that `WorkspaceSandbox`
/// produces once the managed proxy + host bridge are live, vs a raw
/// non-loopback allowlist that still has no kernel-level enforcement.
fn is_loopback_literal(host: &str) -> bool {
    host == "127.0.0.1"
        || host == "::1"
        || host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false)
}

/// Append codex-inspired metadata-protection mounts to a bubblewrap
/// argument vector. For every writable root, each protected subpath
/// (`.git`, `.aleph`, `.codex`, `.agents`) is shielded so the sandboxed
/// process can neither write into an existing one nor create a missing
/// one:
///
/// - **Existing** path → `--ro-bind-try` remounts it read-only, so tools
///   that read metadata (`git log`, `git status`) keep working.
/// - **Absent** path → a synthetic empty read-only `tmpfs` is mounted
///   (`--perms 555 --tmpfs <p> --remount-ro <p>`). Without this,
///   `--ro-bind-try` silently no-ops for a non-existent source and the
///   sandboxed process can `mkdir .git` inside the writable workspace and
///   write into it. The synthetic mount matches macOS Seatbelt, whose
///   `(deny file-write* (subpath …))` rule applies whether or not the
///   path exists. Mirrors codex's `append_empty_directory_args`.
///
/// Must be emitted *after* the writable `--bind` because bwrap mounts
/// override in declaration order.
///
/// Returns `Err(ProfileGeneration)` when a protected path crosses a
/// **writable symlink** component (TOCTOU): bwrap resolves the symlink
/// target at mount time, so a process that swapped it between policy
/// generation and exec would redirect the read-only protection to an
/// arbitrary target. Such a policy cannot be enforced and must fail
/// closed rather than silently bind the wrong inode.
fn push_metadata_protection_args<'a, I>(
    args: &mut Vec<String>,
    writable_roots: I,
) -> Result<(), SandboxError>
where
    I: IntoIterator<Item = &'a std::path::Path>,
{
    let roots: Vec<&std::path::Path> = writable_roots.into_iter().collect();
    let protected = crate::sandbox::protected_paths::protected_paths_for(roots.iter().copied());
    for path in protected {
        let Some(path_str) = path.to_str() else {
            continue;
        };
        // TOCTOU guard (codex parity): refuse to emit a read-only mount
        // whose path crosses a writable symlink the sandboxed process
        // could swap out from under us.
        if let Some(symlink) =
            crate::sandbox::protected_paths::first_writable_symlink_component(&path, &roots)
        {
            return Err(SandboxError::ProfileGeneration(format!(
                "TOCTOU: cannot enforce read-only protection for {} because it crosses \
                 writable symlink {}; a sandboxed process could redirect it. Remove or \
                 dereference the symlink before granting workspace-write.",
                path.display(),
                symlink.display()
            )));
        }
        if path.exists() {
            // Existing metadata: keep it readable but read-only.
            // `--ro-bind-try` (not `--ro-bind`) tolerates the tiny
            // check→mount race where the dir is removed meanwhile.
            args.push("--ro-bind-try".into());
            args.push(path_str.into());
            args.push(path_str.into());
        } else {
            // Absent metadata: synthesize an empty read-only directory so
            // the process cannot create a real one and write inside it.
            // `--perms 555` → r-x (traversable, not writable); `--tmpfs`
            // then `--remount-ro` pins the whole mount read-only.
            args.push("--perms".into());
            args.push("555".into());
            args.push("--tmpfs".into());
            args.push(path_str.into());
            args.push("--remount-ro".into());
            args.push(path_str.into());
        }
    }
    Ok(())
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

        let mut bwrap_args: Vec<String> = profile.contents.lines().map(|s| s.to_string()).collect();

        // `generate_args` terminates the bwrap option list with `--`. The
        // dynamic mounts appended below (proxy UDS bind, sandbox-init
        // ro-bind) are bwrap OPTIONS and must precede that terminator —
        // otherwise bwrap parses the first one (`--ro-bind`) as the command
        // to exec. Pop it here; it is re-appended just before the target
        // command is composed.
        if bwrap_args.last().map(String::as_str) == Some("--") {
            bwrap_args.pop();
        }

        // Phase B: when a proxy route spec is present (AllowHosts via the
        // managed proxy), bind-mount the host bridge's UDS directory into the
        // namespace at the same path so the netns-side local bridge can
        // `connect()` to it. A read-write `--bind` (not `--ro-bind`) is
        // required because connecting to a Unix socket needs write permission
        // on its inode. The bind must follow the `--tmpfs /` emitted by
        // `generate_args` (it does — those lines are already in `bwrap_args`).
        if let Some(spec_json) = env.get(crate::sandbox::proxy::PROXY_ROUTE_SPEC_ENV) {
            match crate::sandbox::proxy::ProxyRouteSpec::from_env_json(spec_json) {
                Ok(spec) => {
                    if let Some(dir) = spec.uds_path.parent().and_then(|p| p.to_str()) {
                        bwrap_args.push("--bind".into());
                        bwrap_args.push(dir.to_string());
                        bwrap_args.push(dir.to_string());
                    } else {
                        return Err(SandboxError::ProfileGeneration(
                            "proxy route UDS path has no valid parent directory".into(),
                        ));
                    }
                }
                Err(e) => {
                    return Err(SandboxError::ProfileGeneration(format!(
                        "parse proxy route spec for bind-mount: {e}"
                    )));
                }
            }
        }

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
                .map_err(|e| {
                    SandboxError::ExecutionFailed(format!(
                        "cannot determine aleph-server path: {e}"
                    ))
                })?;
            bwrap_args.push("--ro-bind".into());
            bwrap_args.push(aleph_exe.to_string_lossy().into_owned());
            bwrap_args.push("/aleph-sandbox-init".into());
        }

        // Re-append the option terminator popped above, so the target
        // command composed below is not parsed as more bwrap options.
        bwrap_args.push("--".into());

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
            .stdin(std::process::Stdio::piped())
            // Belt-and-braces: kill bwrap if our future is dropped.
            // bwrap already passes `--die-with-parent` so the inner
            // payload follows, but the OS-level reap on drop guards
            // against future code paths that bypass that.
            .kill_on_drop(true);

        self.apply_no_new_privs(&mut cmd);
        if let Some(mb) = profile.max_memory_mb {
            Self::apply_memory_rlimit(&mut cmd, mb);
        }

        // SP-5: build cgroup v2 sub-cgroup before spawn so the child can
        // be attached via pre_exec. The scope is owned by this `run()`
        // frame; its Drop fires after `run_child_with_drain` returns and
        // rmdir's the cgroup directory. RLIMIT_AS (above) continues to
        // apply as a fallback when cgroups are unavailable.
        let _cgroup_scope: Option<crate::sandbox::cgroup_v2::CgroupV2Scope> =
            if self.options.cgroup_enabled {
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
                    // rust-doctor-disable-next-line unsafe-block-audit
                    unsafe {
                        // SAFETY: pre_exec closure only writes the current PID
                        // to the owned cgroup.procs path; it performs no heap
                        // allocation or locking and is safe after fork.
                        cmd.pre_exec(move || {
                            crate::sandbox::cgroup_v2::CgroupV2Scope::write_current_pid_to_path(
                                &procs_path,
                            )
                        });
                    }
                }
                scope
            } else {
                None
            };

        let child = cmd
            .spawn()
            .map_err(|e| SandboxError::ExecutionFailed(format!("failed to spawn bwrap: {e}")))?;

        // `_cgroup_scope` is kept alive across the await so its Drop —
        // which rmdir's the cgroup directory — only fires after the
        // child has been reaped by the helper.
        run_child_with_drain(child, stdin, timeout, max_output_bytes).await
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
            include_platform_defaults: false,
            ..LinuxSandboxOptions::default()
        });
        let policy = SandboxPolicy::default();
        let cwd = Path::new("/tmp/test-workspace");
        let args = driver.generate_args(&policy, cwd).unwrap();

        assert!(args.contains(&"--tmpfs".into()));
        assert!(!args.iter().any(|a| a == "/usr"));
    }

    #[test]
    fn workspace_only_synthesizes_tmpfs_for_absent_metadata() {
        let driver = BubblewrapDriver::new();
        let policy = SandboxPolicy::default();
        let ws = tempfile::tempdir().unwrap();
        let cwd = ws.path();
        // No .git/.aleph/… created → every protected subpath is absent.
        let args = driver.generate_args(&policy, cwd).unwrap();

        let cwd_str = cwd.to_str().unwrap();
        let git = format!("{cwd_str}/.git");
        // The writable --bind comes first; the synthetic tmpfs must
        // follow so it overrides (shadows) the writable mount.
        let bind_idx = args
            .windows(3)
            .position(|w| w == ["--bind", cwd_str, cwd_str])
            .expect("workspace --bind must be present");
        let tmpfs_idx = args
            .windows(2)
            .position(|w| w == ["--tmpfs", git.as_str()])
            .expect("absent .git must get a synthetic --tmpfs");
        assert!(
            bind_idx < tmpfs_idx,
            "tmpfs must follow the writable --bind"
        );
        // The synthetic mount is mode 555 and remounted read-only.
        assert!(args.windows(2).any(|w| w == ["--perms", "555"]));
        assert!(args.windows(2).any(|w| w == ["--remount-ro", git.as_str()]));
        // An absent path must NOT use --ro-bind-try (it would no-op).
        assert!(!args
            .windows(3)
            .any(|w| w == ["--ro-bind-try", git.as_str(), git.as_str()]));
    }

    #[test]
    fn workspace_only_ro_binds_existing_metadata() {
        let driver = BubblewrapDriver::new();
        let policy = SandboxPolicy::default();
        let ws = tempfile::tempdir().unwrap();
        std::fs::create_dir(ws.path().join(".git")).unwrap();
        let cwd = ws.path();
        let args = driver.generate_args(&policy, cwd).unwrap();

        let git = format!("{}/.git", cwd.to_str().unwrap());
        // An existing .git is bound read-only so `git log` still works —
        // not shadowed by a synthetic tmpfs.
        assert!(
            args.windows(3)
                .any(|w| w == ["--ro-bind-try", git.as_str(), git.as_str()]),
            "existing .git must get --ro-bind-try"
        );
        assert!(
            !args.windows(2).any(|w| w == ["--tmpfs", git.as_str()]),
            "existing .git must not be shadowed by a synthetic tmpfs"
        );
    }

    #[test]
    fn write_paths_protects_metadata_in_each_writable_root() {
        let driver = BubblewrapDriver::new();
        let ws = tempfile::tempdir().unwrap();
        let extra = tempfile::tempdir().unwrap();
        // .git exists in the workspace, absent in the extra root — this
        // exercises both protection branches across both writable roots.
        std::fs::create_dir(ws.path().join(".git")).unwrap();
        let policy = SandboxPolicy {
            filesystem: FsPolicy::WritePaths(vec![extra.path().to_path_buf()]),
            ..Default::default()
        };
        let args = driver.generate_args(&policy, ws.path()).unwrap();

        // Every protected subpath under every writable root is shielded
        // by one branch or the other.
        for root in [ws.path(), extra.path()] {
            for sub in [".git", ".aleph", ".codex", ".agents"] {
                let p = format!("{}/{}", root.to_str().unwrap(), sub);
                let ro_bound = args
                    .windows(3)
                    .any(|w| w == ["--ro-bind-try", p.as_str(), p.as_str()]);
                let tmpfs_shadowed = args.windows(2).any(|w| w == ["--tmpfs", p.as_str()]);
                assert!(
                    ro_bound || tmpfs_shadowed,
                    "missing metadata protection for {p}"
                );
            }
        }
        // The one pre-existing dir specifically uses the ro-bind branch.
        let ws_git = format!("{}/.git", ws.path().to_str().unwrap());
        assert!(
            args.windows(3)
                .any(|w| w == ["--ro-bind-try", ws_git.as_str(), ws_git.as_str()]),
            "existing workspace .git must use --ro-bind-try"
        );
    }

    #[test]
    fn protected_metadata_symlink_in_writable_root_fails_closed() {
        // TOCTOU: the sandboxed process turned the protected `.git` into a
        // symlink. bwrap would resolve+bind the target at mount time, so the
        // read-only protection is unenforceable — profile generation must
        // fail closed rather than silently bind the wrong inode.
        let driver = BubblewrapDriver::new();
        let policy = SandboxPolicy::default();
        let ws = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), ws.path().join(".git")).unwrap();

        let err = driver
            .generate_args(&policy, ws.path())
            .expect_err("a writable-symlink protected path must be rejected");
        match err {
            SandboxError::ProfileGeneration(reason) => {
                assert!(
                    reason.contains("TOCTOU") && reason.contains(".git"),
                    "rejection must name the TOCTOU hazard, got: {reason}"
                );
            }
            other => panic!("expected ProfileGeneration, got {other:?}"),
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
        assert!(!args
            .windows(3)
            .any(|w| w == ["--ro-bind-try", "/tmp/ws/.git", "/tmp/ws/.git"]));
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
    fn generate_args_allow_hosts_loopback_unshares_net_for_proxy_bridge() {
        // Phase B: once `WorkspaceSandbox::maybe_spawn_proxy` collapses the
        // allowlist to loopback (proxy + host bridge are live), the driver
        // must NOT hard-fail — it unshares the network so the only egress is
        // the netns-side local bridge on `lo`.
        let driver = BubblewrapDriver::new();
        let policy = SandboxPolicy {
            network: NetworkPolicy::AllowHosts(vec!["127.0.0.1".into()]),
            ..Default::default()
        };
        let cwd = Path::new("/tmp/ws");
        let args = driver
            .generate_args(&policy, cwd)
            .expect("loopback AllowHosts is the supported proxied path");
        assert!(
            args.contains(&"--unshare-net".into()),
            "proxied AllowHosts must isolate the network, got: {args:?}"
        );
    }

    #[test]
    fn is_loopback_literal_matches_loopback_forms_only() {
        assert!(is_loopback_literal("127.0.0.1"));
        assert!(is_loopback_literal("::1"));
        assert!(is_loopback_literal("localhost"));
        assert!(is_loopback_literal("127.5.6.7")); // whole 127/8 is loopback
        assert!(!is_loopback_literal("10.0.0.1"));
        assert!(!is_loopback_literal("example.com"));
    }

    #[test]
    fn generate_args_allow_fork() {
        let driver = BubblewrapDriver::new();
        let policy = SandboxPolicy {
            process: ProcessPolicy {
                allow_fork: true,
                max_memory_mb: None,
            },
            ..Default::default()
        };
        let cwd = Path::new("/tmp/ws");
        let args = driver.generate_args(&policy, cwd).unwrap();

        // Fork is permitted, so the PID namespace is NOT unshared (a forking
        // workload may need shared process visibility)...
        assert!(!args.contains(&"--unshare-pid".into()));
        // ...but capabilities are dropped regardless of fork policy: a
        // sandboxed command never legitimately needs Linux capabilities.
        assert!(args.contains(&"--cap-drop".into()));
        assert!(args.contains(&"ALL".into()));
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
