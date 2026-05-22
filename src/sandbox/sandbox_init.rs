//! SP-2 init binary logic — applies landlock + seccomp inside the bwrap
//! mount namespace, then `execvp()`s the target program.
//!
//! Invoked as `aleph-server sandbox-init --policy <json> -- <target>
//! <target-args...>` by `BubblewrapDriver::run`. Lives in a hidden CLI
//! subcommand on the existing aleph-server binary (no separate helper
//! artifact — R3 core minimalism).
//!
//! The init prelude runs inside bwrap's namespace, *after* bwrap finishes
//! its mount/PID/user-ns setup and *before* the untrusted target gets to
//! execute even one instruction. That's the correct LSM hook point: the
//! filesystem is already constrained by mounts, but the kernel attack
//! surface and intra-mount FS ACL are not yet locked down.
//!
//! Cross-platform parts (policy struct, JSON shape, denylist constant,
//! capability→path translation) are not gated so they compile and unit-
//! test on macOS / Windows dev boxes. The actual `apply_*` and `run_init`
//! entry point are `#[cfg(target_os = "linux")]`-gated because they call
//! Linux-only kernel APIs.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::sandbox::capabilities::SandboxCapabilities;

/// Policy passed from `BubblewrapDriver::run` to `sandbox-init` via JSON
/// on argv. Bounded by capability count (typical < 4 KiB serialized;
/// well under `MAX_ARG_STRLEN`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LinuxInitPolicy {
    /// Paths to grant `READ_FILE | READ_DIR | EXECUTE` on (landlock).
    /// Always includes the system minimum (`/usr`, `/lib`, `/lib64`,
    /// `/bin`, `/sbin`, `/etc`) plus every `SandboxCapabilities.fs_read`
    /// entry.
    pub read_paths: Vec<PathBuf>,
    /// Paths to grant `READ_FILE | READ_DIR | WRITE_FILE | REMOVE_FILE |
    /// REMOVE_DIR | MAKE_REG | MAKE_DIR | MAKE_SYM | EXECUTE` on
    /// (landlock). One per `SandboxCapabilities.fs_write` entry; the
    /// workspace cwd is included automatically by the caller.
    pub write_paths: Vec<PathBuf>,
    /// When `true`, `apply_landlock` returns an error if the kernel
    /// does not expose landlock ABI ≥ 1. Default `false` → soft
    /// degrade with a warning.
    #[serde(default)]
    pub require_landlock: bool,

    /// Cycle 3: codex-style defense-in-depth — when `true`, the seccomp
    /// filter denies `socket(AF_INET|AF_INET6|AF_NETLINK)` and
    /// `connect` with EPERM. The bubblewrap `--unshare-net` mount-ns
    /// already removes network interfaces, but a process inside that
    /// netns can still create `AF_INET` sockets that fail at runtime;
    /// seccomp denies the syscall earlier so audit logs surface the
    /// attempt clearly. AF_UNIX sockets stay allowed (needed for IPC).
    ///
    /// Set when [`SandboxCapabilities::network`] is
    /// [`NetworkPolicy::None`]; ignored in `AllowAll` / `AllowHosts`
    /// modes because seccomp cannot filter by IP, only by syscall +
    /// constant args.
    #[serde(default)]
    pub deny_network_sockets: bool,
}

/// System-minimum read paths granted unconditionally. Without these,
/// dynamic linker / libc / shells in `/usr/bin` cannot load.
pub const SYSTEM_READ_PATHS: &[&str] = &["/usr", "/lib", "/lib64", "/bin", "/sbin", "/etc"];

/// Frozen seccomp denylist. Each name corresponds to a syscall that gets
/// `SECCOMP_RET_ERRNO(EPERM)`; everything else falls through to allow.
/// `EPERM` (vs `SIGKILL`) keeps the program survivable so it can log /
/// report errors. Argument-aware filters for `clone` / `unshare` /
/// `setns` are not encoded here as strings — they're applied directly
/// in `apply_seccomp` via the `seccompiler` rule builder.
///
/// The unit test `seccomp_denylist_is_frozen` pins this list. Modifying
/// it requires updating that snapshot, which acts as a tripwire for
/// "future contributor silently shrinks the denylist" regressions.
pub const SECCOMP_DENYLIST_SIMPLE: &[&str] = &[
    "mount",
    "umount",
    "umount2",
    "pivot_root",
    "chroot",
    "kexec_load",
    "kexec_file_load",
    "init_module",
    "finit_module",
    "delete_module",
    "bpf",
    "perf_event_open",
    "ptrace",
    "keyctl",
    "add_key",
    "request_key",
    "userfaultfd",
    "io_uring_setup",
    "io_uring_register",
    "io_uring_enter",
    "mknod",
    "mknodat",
    "swapon",
    "swapoff",
    "nfsservctl",
    "syslog",
    "reboot",
    "setns",
];

/// Build a `LinuxInitPolicy` from caller-side `SandboxCapabilities`
/// (cross-platform). The driver calls this on the host before invoking
/// `sandbox-init` so the policy travels as serialized JSON.
pub fn policy_from_capabilities(
    caps: &SandboxCapabilities,
    cwd: &std::path::Path,
    require_landlock: bool,
) -> LinuxInitPolicy {
    let mut read_paths: Vec<PathBuf> = SYSTEM_READ_PATHS.iter().map(PathBuf::from).collect();
    for p in &caps.fs_read {
        read_paths.push(p.clone());
    }
    // Workspace cwd is always writable — it's the whole point of the
    // workspace sandbox. Caller-supplied fs_write paths come after.
    let mut write_paths: Vec<PathBuf> = vec![cwd.to_path_buf()];
    for p in &caps.fs_write {
        write_paths.push(p.clone());
    }
    use crate::sandbox::capabilities::NetworkPolicy;
    let deny_network_sockets = matches!(caps.network, NetworkPolicy::None);
    LinuxInitPolicy {
        read_paths,
        write_paths,
        require_landlock,
        deny_network_sockets,
    }
}

// ---------------------------------------------------------------------------
// Linux-only entry point + LSM application.
// ---------------------------------------------------------------------------

/// Top-level entry point for the `sandbox-init` subcommand. Never
/// returns: either `execvp`s the target on success, or `process::exit`s
/// on policy / kernel failure.
///
/// Exit codes (per spec §6):
/// - 64 → landlock unavailable and `require_landlock=true`
/// - 65 → seccomp filter rejected by the kernel (unrecoverable)
/// - 66 → cannot parse argv (bad policy JSON, missing `--`, etc.)
/// - 67 → `execvp` failed (target binary not found or not executable)
#[cfg(target_os = "linux")]
pub fn run_init(args: Vec<String>) -> ! {
    use std::os::unix::process::CommandExt as _;
    use std::process::Command;

    let parsed = match parse_init_args(&args) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("aleph sandbox-init: argv parse failed: {e}");
            std::process::exit(66);
        }
    };

    // PR_SET_NO_NEW_PRIVS is required before seccomp arms safely (else
    // setuid binaries we exec could regain privileges). Cheap, idempotent.
    if let Err(e) = set_no_new_privs() {
        eprintln!("aleph sandbox-init: prctl(PR_SET_NO_NEW_PRIVS) failed: {e}");
        std::process::exit(65);
    }

    if let Err(e) = apply_landlock(&parsed.policy) {
        if parsed.policy.require_landlock {
            eprintln!("aleph sandbox-init: landlock required but unavailable: {e}");
            std::process::exit(64);
        }
        eprintln!(
            "aleph sandbox-init: landlock unavailable on this kernel ({e}); \
             continuing with bwrap+seccomp only"
        );
    }

    if let Err(e) = apply_seccomp(&parsed.policy) {
        eprintln!("aleph sandbox-init: seccomp filter rejected: {e}");
        std::process::exit(65);
    }

    let mut cmd = Command::new(&parsed.target);
    cmd.args(&parsed.target_args);
    // `exec` replaces the process image; on success this never returns.
    let err = cmd.exec();
    eprintln!(
        "aleph sandbox-init: execvp({:?}) failed: {}",
        parsed.target, err
    );
    std::process::exit(67);
}

#[cfg(not(target_os = "linux"))]
pub fn run_init(_args: Vec<String>) -> ! {
    eprintln!("aleph sandbox-init: only supported on Linux");
    std::process::exit(78); // EX_CONFIG: unsupported on this platform
}

/// Output of `parse_init_args`. `target_args` is the slice after `--`.
/// `dead_code` allow: on macOS / Windows no caller exists at build time
/// (run_init is Linux-only), but unit tests reference it cross-platform.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Debug)]
struct ParsedInitArgs {
    policy: LinuxInitPolicy,
    target: String,
    target_args: Vec<String>,
}

/// argv layout: `[--policy <json> -- <target> <target-args...>]`.
/// (The leading `sandbox-init` subcommand name is stripped by the CLI
/// dispatcher before calling `run_init`.)
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_init_args(args: &[String]) -> Result<ParsedInitArgs, String> {
    let mut policy_json: Option<&str> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--policy" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "--policy requires a value".to_string())?;
                policy_json = Some(v.as_str());
                i += 2;
            }
            "--" => {
                i += 1;
                break;
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    let policy_str = policy_json.ok_or_else(|| "missing --policy".to_string())?;
    let policy: LinuxInitPolicy =
        serde_json::from_str(policy_str).map_err(|e| format!("--policy JSON parse error: {e}"))?;

    let target = args
        .get(i)
        .ok_or_else(|| "missing target program after `--`".to_string())?
        .clone();
    let target_args = args[i + 1..].to_vec();

    Ok(ParsedInitArgs {
        policy,
        target,
        target_args,
    })
}

#[cfg(target_os = "linux")]
fn set_no_new_privs() -> Result<(), std::io::Error> {
    // SAFETY: prctl(PR_SET_NO_NEW_PRIVS, 1) is documented as always-safe;
    // it cannot fail except on impossibly old kernels (< 3.5).
    let rc = unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1u64, 0u64, 0u64, 0u64) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn apply_landlock(policy: &LinuxInitPolicy) -> Result<(), String> {
    use landlock::{
        Access, AccessFs, PathBeneath, PathFd, RestrictionStatus, Ruleset, RulesetAttr,
        RulesetCreatedAttr, RulesetStatus, ABI,
    };

    let abi = ABI::V1;
    let ruleset = Ruleset::default()
        .handle_access(AccessFs::from_all(abi))
        .map_err(|e| format!("Ruleset::handle_access: {e}"))?
        .create()
        .map_err(|e| format!("Ruleset::create (kernel may lack landlock): {e}"))?;

    let mut ruleset = ruleset;

    let read_mask = AccessFs::from_read(abi) | AccessFs::Execute;
    for path in &policy.read_paths {
        if let Ok(fd) = PathFd::new(path) {
            ruleset = ruleset
                .add_rule(PathBeneath::new(fd, read_mask))
                .map_err(|e| format!("add read rule {path:?}: {e}"))?;
        }
    }

    let write_mask = AccessFs::from_all(abi);
    for path in &policy.write_paths {
        if let Ok(fd) = PathFd::new(path) {
            ruleset = ruleset
                .add_rule(PathBeneath::new(fd, write_mask))
                .map_err(|e| format!("add write rule {path:?}: {e}"))?;
        }
    }

    let status: RestrictionStatus = ruleset
        .restrict_self()
        .map_err(|e| format!("restrict_self: {e}"))?;
    if status.ruleset == RulesetStatus::NotEnforced {
        return Err("landlock kernel-side enforcement returned NotEnforced".into());
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn apply_seccomp(policy: &LinuxInitPolicy) -> Result<(), String> {
    use std::collections::BTreeMap;

    use seccompiler::{
        BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompFilter,
        SeccompRule, TargetArch,
    };

    let target_arch: TargetArch = if cfg!(target_arch = "x86_64") {
        TargetArch::x86_64
    } else if cfg!(target_arch = "aarch64") {
        TargetArch::aarch64
    } else {
        return Err(format!(
            "unsupported target_arch for seccomp filter: {}",
            std::env::consts::ARCH
        ));
    };

    let mut rules: BTreeMap<i64, Vec<SeccompRule>> = BTreeMap::new();
    for name in SECCOMP_DENYLIST_SIMPLE {
        let nr =
            syscall_nr(name).ok_or_else(|| format!("unknown syscall name in denylist: {name}"))?;
        // Empty rule vec = unconditional match for this syscall.
        rules.insert(nr, vec![]);
    }

    // clone/unshare with CLONE_NEWUSER → deny (nested user-namespace
    // escape). CLONE_NEWUSER bit = 0x10000000.
    const CLONE_NEWUSER: u64 = 0x1000_0000;
    let nuser_cond = SeccompCondition::new(
        0,
        SeccompCmpArgLen::Qword,
        SeccompCmpOp::MaskedEq(CLONE_NEWUSER),
        CLONE_NEWUSER,
    )
    .map_err(|e| format!("build CLONE_NEWUSER condition: {e}"))?;
    if let Some(nr) = syscall_nr("clone") {
        rules.insert(
            nr,
            vec![SeccompRule::new(vec![nuser_cond.clone()])
                .map_err(|e| format!("build clone rule: {e}"))?],
        );
    }
    if let Some(nr) = syscall_nr("unshare") {
        rules.insert(
            nr,
            vec![SeccompRule::new(vec![nuser_cond])
                .map_err(|e| format!("build unshare rule: {e}"))?],
        );
    }

    // Cycle 3 / codex-aligned: when network access is disabled
    // (NetworkPolicy::None on the host side), block every socket family
    // other than AF_UNIX at the syscall layer. The bwrap `--unshare-net`
    // netns already strips interfaces, but a process inside it can still
    // call `socket(AF_INET, ...)` and `connect()` — they fail at runtime
    // with cryptic errors. Adding the seccomp rules turns those into
    // early EPERM failures with a clear audit trail.
    //
    // We mirror codex's `socket(family != AF_UNIX)` pattern rather than
    // enumerating AF_INET / AF_INET6 / AF_NETLINK individually so the
    // policy also catches AF_PACKET, AF_BLUETOOTH, AF_VSOCK, etc.
    // automatically. Sock-domain args are 32-bit in the syscall ABI, so
    // use `Dword` even though `libc::AF_UNIX` happens to be small.
    if policy.deny_network_sockets {
        let non_unix_rule = SeccompRule::new(vec![SeccompCondition::new(
            0, // first arg of socket / socketpair = domain
            SeccompCmpArgLen::Dword,
            SeccompCmpOp::Ne,
            libc::AF_UNIX as u64,
        )
        .map_err(|e| format!("build AF_UNIX != condition: {e}"))?])
        .map_err(|e| format!("build non-AF_UNIX socket rule: {e}"))?;
        if let Some(nr) = syscall_nr("socket") {
            rules.insert(nr, vec![non_unix_rule.clone()]);
        }
        if let Some(nr) = syscall_nr("socketpair") {
            rules.insert(nr, vec![non_unix_rule]);
        }
        // Deny `connect` unconditionally when network is disabled.
        // codex applies the same gate; AF_UNIX clients that call
        // connect() on a Unix-domain socket will be hit by this too,
        // but sandboxed daemons rarely depend on it and the syscall
        // returns EPERM which clients propagate as a clear error
        // rather than crashing.
        if let Some(nr) = syscall_nr("connect") {
            rules.insert(nr, vec![]);
        }
    }

    let filter = SeccompFilter::new(
        rules,
        // Default: allow everything not on the denylist.
        SeccompAction::Allow,
        // Matched syscalls return EPERM.
        SeccompAction::Errno(libc::EPERM as u32),
        target_arch,
    )
    .map_err(|e| format!("SeccompFilter::new: {e}"))?;

    let prog: BpfProgram = filter
        .try_into()
        .map_err(|e| format!("compile BPF program: {e}"))?;

    seccompiler::apply_filter(&prog).map_err(|e| format!("apply_filter: {e}"))?;
    Ok(())
}

/// Map syscall name → arch-specific syscall number. Returns `None` for
/// names that don't exist on the current architecture (e.g.
/// `nfsservctl` was removed in 5.17).
#[cfg(target_os = "linux")]
fn syscall_nr(name: &str) -> Option<i64> {
    // Translates via libc::SYS_* constants. Only the names actually used
    // by `SECCOMP_DENYLIST_SIMPLE` need to be listed.
    let nr: i64 = match name {
        "mount" => libc::SYS_mount,
        "umount2" => libc::SYS_umount2,
        // umount (no '2') was removed from glibc on most arches; the
        // 2.x form is what userland calls.
        "umount" => return None,
        "pivot_root" => libc::SYS_pivot_root,
        "chroot" => libc::SYS_chroot,
        "kexec_load" => libc::SYS_kexec_load,
        "kexec_file_load" => libc::SYS_kexec_file_load,
        "init_module" => libc::SYS_init_module,
        "finit_module" => libc::SYS_finit_module,
        "delete_module" => libc::SYS_delete_module,
        "bpf" => libc::SYS_bpf,
        "perf_event_open" => libc::SYS_perf_event_open,
        "ptrace" => libc::SYS_ptrace,
        "keyctl" => libc::SYS_keyctl,
        "add_key" => libc::SYS_add_key,
        "request_key" => libc::SYS_request_key,
        "userfaultfd" => libc::SYS_userfaultfd,
        "io_uring_setup" => libc::SYS_io_uring_setup,
        "io_uring_register" => libc::SYS_io_uring_register,
        "io_uring_enter" => libc::SYS_io_uring_enter,
        "mknod" => return None, // arm64 removed mknod in favor of mknodat
        "mknodat" => libc::SYS_mknodat,
        "swapon" => libc::SYS_swapon,
        "swapoff" => libc::SYS_swapoff,
        "nfsservctl" => return None, // removed in 5.17
        "syslog" => libc::SYS_syslog,
        "reboot" => libc::SYS_reboot,
        "setns" => libc::SYS_setns,
        "clone" => libc::SYS_clone,
        "unshare" => libc::SYS_unshare,
        "socket" => libc::SYS_socket,
        "socketpair" => libc::SYS_socketpair,
        "connect" => libc::SYS_connect,
        _ => return None,
    };
    Some(nr)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_includes_system_read_paths() {
        let caps = SandboxCapabilities::strict();
        let cwd = std::path::Path::new("/workspace");
        let p = policy_from_capabilities(&caps, cwd, false);
        for sys in SYSTEM_READ_PATHS {
            assert!(
                p.read_paths.iter().any(|x| x == std::path::Path::new(sys)),
                "missing system read path {sys}"
            );
        }
    }

    #[test]
    fn policy_includes_cwd_as_writable() {
        let caps = SandboxCapabilities::strict();
        let cwd = std::path::Path::new("/workspace/session-abc");
        let p = policy_from_capabilities(&caps, cwd, false);
        assert!(p.write_paths.iter().any(|x| x == cwd));
    }

    #[test]
    fn policy_threads_fs_read_and_fs_write_from_caps() {
        let caps = SandboxCapabilities {
            fs_read: vec![PathBuf::from("/home/user/data")],
            fs_write: vec![PathBuf::from("/tmp/scratch")],
            ..SandboxCapabilities::strict()
        };
        let p = policy_from_capabilities(&caps, std::path::Path::new("/workspace"), false);
        assert!(p
            .read_paths
            .iter()
            .any(|x| x == std::path::Path::new("/home/user/data")));
        assert!(p
            .write_paths
            .iter()
            .any(|x| x == std::path::Path::new("/tmp/scratch")));
    }

    #[test]
    fn policy_threads_require_landlock_flag() {
        let caps = SandboxCapabilities::strict();
        let cwd = std::path::Path::new("/workspace");
        assert!(!policy_from_capabilities(&caps, cwd, false).require_landlock);
        assert!(policy_from_capabilities(&caps, cwd, true).require_landlock);
    }

    #[test]
    fn policy_round_trips_through_json() {
        let original = LinuxInitPolicy {
            read_paths: vec!["/usr".into(), "/lib".into()],
            write_paths: vec!["/workspace".into()],
            require_landlock: true,
            deny_network_sockets: true,
        };
        let json = serde_json::to_string(&original).unwrap();
        let parsed: LinuxInitPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(original, parsed);
    }

    #[test]
    fn deny_network_sockets_field_defaults_false_for_forward_compat() {
        // Existing on-the-wire JSON predates the field; deserialize must
        // succeed and default to false so old aleph-server binaries can
        // still talk to new ones.
        let json = r#"{"read_paths":[],"write_paths":[]}"#;
        let parsed: LinuxInitPolicy = serde_json::from_str(json).unwrap();
        assert!(!parsed.deny_network_sockets);
    }

    #[test]
    fn policy_threads_deny_network_sockets_from_capabilities() {
        use crate::sandbox::capabilities::NetworkPolicy;
        let cwd = std::path::Path::new("/workspace");
        // NetworkPolicy::None → deny_network_sockets = true (defense in
        // depth on top of bwrap --unshare-net).
        let none_caps = SandboxCapabilities {
            network: NetworkPolicy::None,
            ..SandboxCapabilities::strict()
        };
        assert!(policy_from_capabilities(&none_caps, cwd, false).deny_network_sockets);

        // NetworkPolicy::AllowAll → seccomp lets IP sockets through; the
        // kernel namespace + driver-level rules carry policy from there.
        let all_caps = SandboxCapabilities {
            network: NetworkPolicy::AllowAll,
            ..SandboxCapabilities::strict()
        };
        assert!(!policy_from_capabilities(&all_caps, cwd, false).deny_network_sockets);

        // NetworkPolicy::AllowHosts also leaves seccomp permissive —
        // seccomp can't filter by IP, only by socket family. The
        // per-host gate would have to land somewhere else (a future
        // spec: managed proxy or nftables-in-netns).
        let hosts_caps = SandboxCapabilities {
            network: NetworkPolicy::AllowHosts {
                hosts: vec!["10.0.0.1".into()],
            },
            ..SandboxCapabilities::strict()
        };
        assert!(!policy_from_capabilities(&hosts_caps, cwd, false).deny_network_sockets);
    }

    /// Pins the denylist by hash so any addition / removal / reorder is
    /// caught by CI. Snapshot owner: spec SP-2 § 5. If you need to
    /// change the list, also update the spec and bump this hash.
    #[test]
    fn seccomp_denylist_is_frozen() {
        let joined = SECCOMP_DENYLIST_SIMPLE.join(",");
        let expected = "mount,umount,umount2,pivot_root,chroot,kexec_load,kexec_file_load,\
                        init_module,finit_module,delete_module,bpf,perf_event_open,ptrace,\
                        keyctl,add_key,request_key,userfaultfd,io_uring_setup,\
                        io_uring_register,io_uring_enter,mknod,mknodat,swapon,swapoff,\
                        nfsservctl,syslog,reboot,setns";
        assert_eq!(
            joined, expected,
            "seccomp denylist changed — update SP-2 spec"
        );
    }

    #[test]
    fn parse_init_args_extracts_policy_and_target() {
        let policy = LinuxInitPolicy {
            read_paths: vec!["/usr".into()],
            write_paths: vec!["/workspace".into()],
            require_landlock: false,
            deny_network_sockets: false,
        };
        let json = serde_json::to_string(&policy).unwrap();
        let argv = vec![
            "--policy".to_string(),
            json,
            "--".to_string(),
            "/usr/bin/python".to_string(),
            "-c".to_string(),
            "print('hi')".to_string(),
        ];
        let parsed = parse_init_args(&argv).unwrap();
        assert_eq!(parsed.policy, policy);
        assert_eq!(parsed.target, "/usr/bin/python");
        assert_eq!(parsed.target_args, vec!["-c", "print('hi')"]);
    }

    #[test]
    fn parse_init_args_rejects_missing_policy() {
        let argv = vec!["--".to_string(), "/usr/bin/true".to_string()];
        let err = parse_init_args(&argv).unwrap_err();
        assert!(err.contains("missing --policy"), "got: {err}");
    }

    #[test]
    fn parse_init_args_rejects_missing_target() {
        let argv = vec![
            "--policy".to_string(),
            "{\"read_paths\":[],\"write_paths\":[]}".to_string(),
            "--".to_string(),
        ];
        let err = parse_init_args(&argv).unwrap_err();
        assert!(err.contains("missing target"), "got: {err}");
    }

    #[test]
    fn parse_init_args_rejects_bad_json() {
        let argv = vec![
            "--policy".to_string(),
            "not json".to_string(),
            "--".to_string(),
            "/usr/bin/true".to_string(),
        ];
        let err = parse_init_args(&argv).unwrap_err();
        assert!(err.contains("JSON parse error"), "got: {err}");
    }
}
