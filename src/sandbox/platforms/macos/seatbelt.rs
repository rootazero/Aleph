//! macOS Seatbelt sandbox driver — generates SBPL profiles and executes
//! via `/usr/bin/sandbox-exec`.
//!
//! Inspired by codex's seatbelt implementation but adapted for Aleph's
//! `SandboxPolicy` / `SandboxCapabilities` model.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use tokio::process::Command;
use tracing::debug;

use crate::sandbox::capabilities::SandboxCapabilities;
use crate::sandbox::command::{SandboxError, SandboxOutput};
use crate::sandbox::driver::{OsSandboxDriverTrait, OsSandboxProfile};
use crate::sandbox::platforms::common::run_child_with_drain;
use crate::sandbox::policy::{FsPolicy, NetworkPolicy, ProcessPolicy, SandboxPolicy};

/// Path to the trusted `sandbox-exec` binary.
/// We only trust `/usr/bin/sandbox-exec` to defend against PATH injection.
const SANDBOX_EXEC_PATH: &str = "/usr/bin/sandbox-exec";

/// Base SBPL policy — closed-by-default plus the essential
/// process/sysctl/IOKit/PTY allowances that every macOS binary needs to
/// start. Ported from codex's `seatbelt_base_policy.sbpl` (Chrome-derived).
///
/// Read-only file-system access for system trees lives in
/// [`PLATFORM_DEFAULTS_POLICY`] below; that split mirrors codex's source
/// layout so future updates can be diffed against the upstream files.
const BASE_POLICY: &str = r#"(version 1)

; inspired by Chrome's sandbox policy:
; https://source.chromium.org/chromium/chromium/src/+/main:sandbox/policy/mac/common.sb
; https://source.chromium.org/chromium/chromium/src/+/main:sandbox/policy/mac/renderer.sb

; closed-by-default
(deny default)

; child processes inherit parent's policy
(allow process-exec)
(allow process-fork)
(allow signal (target same-sandbox))
(allow process-info* (target same-sandbox))

(allow file-write-data
  (require-all
    (path "/dev/null")
    (vnode-type CHARACTER-DEVICE)))

; sysctls permitted (CPU detection, memory info, hostname, routing table).
(allow sysctl-read
  (sysctl-name "hw.activecpu")
  (sysctl-name "hw.busfrequency_compat")
  (sysctl-name "hw.byteorder")
  (sysctl-name "hw.cacheconfig")
  (sysctl-name "hw.cachelinesize_compat")
  (sysctl-name "hw.cpufamily")
  (sysctl-name "hw.cpufrequency_compat")
  (sysctl-name "hw.cputype")
  (sysctl-name "hw.l1dcachesize_compat")
  (sysctl-name "hw.l1icachesize_compat")
  (sysctl-name "hw.l2cachesize_compat")
  (sysctl-name "hw.l3cachesize_compat")
  (sysctl-name "hw.logicalcpu_max")
  (sysctl-name "hw.machine")
  (sysctl-name "hw.model")
  (sysctl-name "hw.memsize")
  (sysctl-name "hw.ncpu")
  (sysctl-name "hw.nperflevels")
  (sysctl-name-prefix "hw.optional.arm.")
  (sysctl-name-prefix "hw.optional.armv8_")
  (sysctl-name "hw.packages")
  (sysctl-name "hw.pagesize_compat")
  (sysctl-name "hw.pagesize")
  (sysctl-name "hw.physicalcpu")
  (sysctl-name "hw.physicalcpu_max")
  (sysctl-name "hw.logicalcpu")
  (sysctl-name "hw.cpufrequency")
  (sysctl-name "hw.tbfrequency_compat")
  (sysctl-name "hw.vectorunit")
  (sysctl-name "machdep.cpu.brand_string")
  (sysctl-name "kern.argmax")
  (sysctl-name "kern.hostname")
  (sysctl-name "kern.maxfilesperproc")
  (sysctl-name "kern.maxproc")
  (sysctl-name "kern.osproductversion")
  (sysctl-name "kern.osrelease")
  (sysctl-name "kern.ostype")
  (sysctl-name "kern.osvariant_status")
  (sysctl-name "kern.osversion")
  (sysctl-name "kern.secure_kernel")
  (sysctl-name "kern.usrstack64")
  (sysctl-name "kern.version")
  (sysctl-name "sysctl.proc_cputype")
  (sysctl-name "vm.loadavg")
  (sysctl-name-prefix "hw.perflevel")
  (sysctl-name-prefix "kern.proc.pgrp.")
  (sysctl-name-prefix "kern.proc.pid.")
  (sysctl-name-prefix "net.routetable.")
)

; Allow Java to read some CPU info. This is misclassified as a "write" because
; userspace passes a memory buffer to the sysctl, but conceptually it is a read.
(allow sysctl-write
  (sysctl-name "kern.grade_cputype"))

; IOKit
(allow iokit-open
  (iokit-registry-entry-class "RootDomainUserClient")
)

; needed to look up user info, see https://crbug.com/792228
(allow mach-lookup
  (global-name "com.apple.system.opendirectoryd.libinfo")
)

; Needed for python multiprocessing on MacOS for the SemLock
(allow ipc-posix-sem)

; Needed for PyTorch/libomp on macOS to register OpenMP runtimes.
(allow ipc-posix-shm-read-data
  ipc-posix-shm-write-create
  ipc-posix-shm-write-unlink
  (ipc-posix-name-regex #"^/__KMP_REGISTERED_LIB_[0-9]+$"))

(allow mach-lookup
  (global-name "com.apple.PowerManagement.control")
)

; allow openpty()
(allow pseudo-tty)
(allow file-read* file-write* file-ioctl (literal "/dev/ptmx"))
(allow file-read* file-write*
  (require-all
    (regex #"^/dev/ttys[0-9]+")
    (extension "com.apple.sandbox.pty")))
; PTYs created before entering seatbelt may lack the extension; allow ioctl
; on those slave ttys so interactive shells detect a TTY and remain functional.
(allow file-ioctl (regex #"^/dev/ttys[0-9]+"))

; allow readonly user preferences
(allow ipc-posix-shm-read* (ipc-posix-name-prefix "apple.cfprefs."))
(allow mach-lookup
  (global-name "com.apple.cfprefsd.daemon")
  (global-name "com.apple.cfprefsd.agent")
  (local-name "com.apple.cfprefsd.agent"))
(allow user-preference-read)
"#;

/// Read-only platform defaults — system trees, frameworks, mach-lookups
/// to logd/trustd/etc., temp scratch space, terminal/device handles.
///
/// Ported verbatim (with light annotations) from codex's
/// `restricted_read_only_platform_defaults.sbpl`. Without these rules,
/// a closed-by-default profile rejects even dyld's mmap of frameworks,
/// causing every binary — including `/bin/echo` — to SIGABRT before
/// producing any output. This block is what makes the sandbox usable
/// for real workloads instead of toy `(allow process-exec)` smoke tests.
///
/// The complete codex file is ~7.6KB; keeping it in source rather than
/// reaching for `include_str!` avoids both the build-time file dependency
/// and the risk of a missing-file fallback at runtime.
const PLATFORM_DEFAULTS_POLICY: &str = r#"
; ---- ported from codex/sandboxing/src/restricted_read_only_platform_defaults.sbpl ----

; Read access to standard system paths
(allow file-read* file-test-existence
  (subpath "/Library/Apple")
  (subpath "/Library/Filesystems/NetFSPlugins")
  (subpath "/Library/Preferences/Logging")
  (subpath "/private/var/db/DarwinDirectory/local/recordStore.data")
  (subpath "/private/var/db/timezone")
  (subpath "/usr/lib")
  (subpath "/usr/share")
  (subpath "/Library/Preferences")
  (subpath "/var/db")
  (subpath "/private/var/db"))

; Map system frameworks + dylibs for loader.
(allow file-map-executable
  (subpath "/Library/Apple/System/Library/Frameworks")
  (subpath "/Library/Apple/System/Library/PrivateFrameworks")
  (subpath "/Library/Apple/usr/lib")
  (subpath "/System/Library/Extensions")
  (subpath "/System/Library/Frameworks")
  (subpath "/System/Library/PrivateFrameworks")
  (subpath "/System/Library/SubFrameworks")
  (subpath "/System/iOSSupport/System/Library/Frameworks")
  (subpath "/System/iOSSupport/System/Library/PrivateFrameworks")
  (subpath "/System/iOSSupport/System/Library/SubFrameworks")
  (subpath "/usr/lib"))

; System Framework and AppKit resources
(allow file-read* file-test-existence
  (subpath "/Library/Apple/System/Library/Frameworks")
  (subpath "/Library/Apple/System/Library/PrivateFrameworks")
  (subpath "/Library/Apple/usr/lib")
  (subpath "/System/Library/Frameworks")
  (subpath "/System/Library/PrivateFrameworks")
  (subpath "/System/Library/SubFrameworks")
  (subpath "/System/iOSSupport/System/Library/Frameworks")
  (subpath "/System/iOSSupport/System/Library/PrivateFrameworks")
  (subpath "/System/iOSSupport/System/Library/SubFrameworks")
  (subpath "/usr/lib"))

; Allow guarded vnodes.
(allow system-mac-syscall (mac-policy-name "vnguard"))

; Determine whether a container is expected.
(allow system-mac-syscall
  (require-all
    (mac-policy-name "Sandbox")
    (mac-syscall-number 67)))

; Allow resolution of standard system symlinks.
(allow file-read-metadata file-test-existence
  (literal "/etc")
  (literal "/tmp")
  (literal "/var")
  (literal "/private/etc/localtime"))

; Allow stat'ing of firmlink parent path components.
(allow file-read-metadata file-test-existence
  (path-ancestors "/System/Volumes/Data/private"))

; Allow processes to get their current working directory.
(allow file-read* file-test-existence
  (literal "/"))

; Allow FSIOC_CAS_BSDFLAGS as alternate chflags.
(allow system-fsctl (fsctl-command FSIOC_CAS_BSDFLAGS))

; Allow access to standard special files.
(allow file-read* file-test-existence
  (literal "/dev/autofs_nowait")
  (literal "/dev/random")
  (literal "/dev/urandom")
  (literal "/private/etc/master.passwd")
  (literal "/private/etc/passwd")
  (literal "/private/etc/protocols")
  (literal "/private/etc/services"))

; Allow null/zero read/write.
(allow file-read* file-test-existence file-write-data
  (literal "/dev/null")
  (literal "/dev/zero"))

; Allow read/write access to the file descriptors.
(allow file-read-data file-test-existence file-write-data
  (subpath "/dev/fd"))

; Provide access to debugger helpers.
(allow file-read* file-test-existence file-write-data file-ioctl
  (literal "/dev/dtracehelper"))

; Scratch space so tools can create temp files. /tmp and friends are
; outside the workspace, but a sandboxed compiler (rustc, clang, cargo)
; routinely creates intermediates in /var/folders/... — the firmlink
; resolution above plus this write grant makes that work.
(allow file-read* file-test-existence file-write* (subpath "/tmp"))
(allow file-read* file-write* (subpath "/private/tmp"))
(allow file-read* file-write* (subpath "/var/tmp"))
(allow file-read* file-write* (subpath "/private/var/tmp"))

; Allow reading standard config directories.
(allow file-read* (subpath "/etc"))
(allow file-read* (subpath "/private/etc"))

(allow file-read* file-test-existence
  (literal "/System/Library/CoreServices")
  (literal "/System/Library/CoreServices/.SystemVersionPlatform.plist")
  (literal "/System/Library/CoreServices/SystemVersion.plist"))

; Some processes read /var metadata during startup.
(allow file-read-metadata (subpath "/var"))
(allow file-read-metadata (subpath "/private/var"))

; IOKit access for root domain services.
(allow iokit-open
  (iokit-registry-entry-class "RootDomainUserClient"))

; macOS Standard library queries opendirectoryd at startup
(allow mach-lookup (global-name "com.apple.system.opendirectoryd.libinfo"))

; Allow IPC to analytics, logging, trust, and other system agents.
(allow mach-lookup
  (global-name "com.apple.analyticsd")
  (global-name "com.apple.analyticsd.messagetracer")
  (global-name "com.apple.appsleep")
  (global-name "com.apple.bsd.dirhelper")
  (global-name "com.apple.cfprefsd.agent")
  (global-name "com.apple.cfprefsd.daemon")
  (global-name "com.apple.diagnosticd")
  (global-name "com.apple.dt.automationmode.reader")
  (global-name "com.apple.espd")
  (global-name "com.apple.logd")
  (global-name "com.apple.logd.events")
  (global-name "com.apple.runningboard")
  (global-name "com.apple.secinitd")
  (global-name "com.apple.system.DirectoryService.libinfo_v1")
  (global-name "com.apple.system.logger")
  (global-name "com.apple.system.notification_center")
  (global-name "com.apple.system.opendirectoryd.membership")
  (global-name "com.apple.trustd")
  (global-name "com.apple.trustd.agent")
  (global-name "com.apple.xpc.activity.unmanaged")
  (local-name "com.apple.cfprefsd.agent"))

; Allow IPC to the syslog socket for logging.
(allow network-outbound (literal "/private/var/run/syslog"))

; macOS Notifications
(allow ipc-posix-shm-read*
  (ipc-posix-name "apple.shm.notification_center"))

; Regulatory domain support.
(allow file-read*
  (literal "/private/var/db/eligibilityd/eligibility.plist"))

; Audio and power management services.
(allow mach-lookup (global-name "com.apple.audio.audiohald"))
(allow mach-lookup (global-name "com.apple.audio.AudioComponentRegistrar"))
(allow mach-lookup (global-name "com.apple.PowerManagement.control"))

; Allow reading the minimum system runtime so exec works.
(allow file-read-data (subpath "/bin"))
(allow file-read-metadata (subpath "/bin"))
(allow file-read-data (subpath "/sbin"))
(allow file-read-metadata (subpath "/sbin"))
(allow file-read-data (subpath "/usr/bin"))
(allow file-read-metadata (subpath "/usr/bin"))
(allow file-read-data (subpath "/usr/sbin"))
(allow file-read-metadata (subpath "/usr/sbin"))
(allow file-read-data (subpath "/usr/libexec"))
(allow file-read-metadata (subpath "/usr/libexec"))

(allow file-read* (subpath "/Library/Preferences"))
(allow file-read* (subpath "/opt/homebrew/lib"))
(allow file-read* (subpath "/usr/local/lib"))
(allow file-read* (subpath "/Applications"))

; Terminal basics and device handles.
(allow file-read* (regex "^/dev/fd/(0|1|2)$"))
(allow file-write* (regex "^/dev/fd/(1|2)$"))
(allow file-read* file-write* (literal "/dev/null"))
(allow file-read* file-write* (literal "/dev/tty"))
(allow file-read-metadata (literal "/dev"))
(allow file-read-metadata (regex "^/dev/.*$"))
(allow file-read-metadata (literal "/dev/stdin"))
(allow file-read-metadata (literal "/dev/stdout"))
(allow file-read-metadata (literal "/dev/stderr"))
(allow file-read-metadata (regex "^/dev/tty[^/]*$"))
(allow file-read-metadata (regex "^/dev/pty[^/]*$"))
(allow file-read* file-write* (regex "^/dev/ttys[0-9]+$"))
(allow file-read* file-write* (literal "/dev/ptmx"))
(allow file-ioctl (regex "^/dev/ttys[0-9]+$"))

; Allow metadata traversal for firmlink parents.
(allow file-read-metadata (literal "/System/Volumes") (vnode-type DIRECTORY))
(allow file-read-metadata (literal "/System/Volumes/Data") (vnode-type DIRECTORY))
(allow file-read-metadata (literal "/System/Volumes/Data/Users") (vnode-type DIRECTORY))

; App sandbox extensions
(allow file-read* (extension "com.apple.app-sandbox.read"))
(allow file-read* file-write* (extension "com.apple.app-sandbox.read-write"))
"#;

/// Header for restricted-network profiles (deny-by-default with the
/// codex-aligned mach-lookups + sysctls that any network-aware program
/// needs to resolve hosts and talk to the platform's networking daemons).
///
/// Ported from codex's `seatbelt_network_policy.sbpl`. The per-host
/// `allow network-outbound (remote ip ...)` lines are appended by the
/// caller after this header.
const RESTRICTED_NETWORK_POLICY: &str = r#"
; deny all network by default — per-host allowlist follows.
(deny network*)

; allow only safe AF_SYSTEM sockets used for local platform services.
(allow system-socket
  (require-all
    (socket-domain AF_SYSTEM)
    (socket-protocol 2)))

(allow mach-lookup
  ; Used by platform helpers that resolve user directory locations.
  (global-name "com.apple.bsd.dirhelper")
  (global-name "com.apple.system.opendirectoryd.membership")
  ; Communicate with the security server for TLS certificate information.
  (global-name "com.apple.SecurityServer")
  (global-name "com.apple.networkd")
  (global-name "com.apple.ocspd")
  (global-name "com.apple.trustd.agent")
  ; Read network configuration.
  (global-name "com.apple.SystemConfiguration.DNSConfiguration")
  (global-name "com.apple.SystemConfiguration.configd"))

(allow sysctl-read
  (sysctl-name-regex #"^net.routetable"))

; allow localhost DNS resolver
(allow network-outbound (remote ip "localhost:53"))
"#;

/// Boot-time tuning for the seatbelt driver. Mirrors Linux's
/// `LinuxSandboxOptions`: sourced from `SandboxConfig` in
/// [`crate::sandbox::platforms::create_platform_driver_with_config`].
#[derive(Debug, Clone, Default)]
pub struct SeatbeltOptions {
    /// Git-style glob patterns denied read (and unlink) inside every
    /// sandboxed command — a security floor over the per-command FS policy.
    /// See [`crate::sandbox::deny_globs`].
    pub deny_read_globs: Vec<String>,

    /// Absolute Unix-domain socket paths a sandboxed command may bind /
    /// connect even under a restricted (or `None`) network policy. A
    /// local-IPC widening over the `(deny network*)` floor — default empty
    /// preserves default-deny. Mirrors codex's `network.allow_unix_sockets`.
    pub allow_unix_sockets: Vec<std::path::PathBuf>,

    /// Permit *any* `AF_UNIX` socket bind/connect (codex's
    /// `dangerously_allow_all_unix_sockets`). Dangerous — a local socket can
    /// reach privileged daemons (e.g. `docker.sock` → root). Prefer the
    /// allowlist. Default `false`.
    pub dangerously_allow_all_unix_sockets: bool,

    /// Permit the sandboxed process to bind and listen on loopback sockets
    /// even under a restricted (or `None`) network policy. The
    /// restricted-network floor otherwise denies `network-bind` /
    /// `network-inbound`, so dev/test workloads that spin up a local server
    /// (`python -m http.server`, a Vite/webpack dev server, a test harness
    /// binding `127.0.0.1:0`, a TCP language server) cannot listen. Grants
    /// loopback binding only — no route to the public internet is opened.
    /// Mirrors codex's `network.allow_local_binding`. Default `false`
    /// preserves the default-deny floor (boot unchanged).
    pub allow_local_binding: bool,
}

/// Driver for macOS seatbelt sandboxing.
#[derive(Debug, Clone, Default)]
pub struct SeatbeltDriver {
    options: SeatbeltOptions,
}

/// Escape a string for safe inclusion in SBPL (Sandbox Profile Language).
/// SBPL uses double quotes for string literals; backslash and quote must be escaped.
fn escape_sbpl(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Recognise the four forms in which a caller may pass an IP-literal host:
///
/// - Bare IPv4: `127.0.0.1`
/// - Bare IPv6: `::1` / `2606:2800:220:1:248:1893:25c8:1946`
/// - IPv4 with port: `127.0.0.1:443`
/// - Bracketed IPv6 (with or without port): `[::1]`, `[::1]:443`
fn host_is_ip_literal(host: &str) -> bool {
    use std::net::IpAddr;
    use std::str::FromStr;
    // Bare IP — catches IPv4 and unbracketed IPv6.
    if IpAddr::from_str(host).is_ok() {
        return true;
    }
    // Bracketed IPv6 with optional `:port`.
    if let Some(rest) = host.strip_prefix('[') {
        if let Some(end) = rest.find(']') {
            return IpAddr::from_str(&rest[..end]).is_ok();
        }
    }
    // IPv4:port (exactly one `:`). Unbracketed IPv6 has multiple colons and
    // was handled by the bare-IP branch above, so this only matches v4:port.
    if host.matches(':').count() == 1 {
        if let Some((addr, _port)) = host.split_once(':') {
            return IpAddr::from_str(addr).is_ok();
        }
    }
    false
}

/// Apply a virtual-address-space ceiling via `setrlimit(RLIMIT_AS)` in the
/// pre-exec hook. The limit is inherited by the eventual target binary that
/// `sandbox-exec` execs. `tokio::process::Command` exposes `pre_exec` as an
/// inherent method, so no `CommandExt` import is needed.
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

impl SeatbeltDriver {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Construct with boot-time options (e.g. the config-driven
    /// `deny_read_globs` security floor).
    #[must_use]
    pub const fn with_options(options: SeatbeltOptions) -> Self {
        Self { options }
    }

    /// Check if `sandbox-exec` is available and executable.
    fn check_sandbox_exec(&self) -> bool {
        std::fs::metadata(SANDBOX_EXEC_PATH).is_ok_and(|m| m.is_file())
    }

    /// Generate SBPL profile from `SandboxPolicy`.
    fn generate_profile(&self, policy: &SandboxPolicy, cwd: &Path) -> Result<String, SandboxError> {
        let mut profile = String::with_capacity(8192);

        // Base policy — process/sysctl/IOKit/PTY essentials.
        profile.push_str(BASE_POLICY);
        profile.push('\n');

        // Read-only system trees + mach-lookups + scratch space.
        // This is the slab that lets `/bin/echo` and friends actually run.
        profile.push_str(PLATFORM_DEFAULTS_POLICY);
        profile.push('\n');

        // The per-user Mach TMPDIR (`/var/folders/<xx>/<yy>/T/`) is NOT under
        // the /tmp / /var/tmp scratch grants above — macOS hands every process
        // its own per-user path, and the grants a profile writes are literal.
        // Without it, anything that touches $TMPDIR dies with EPERM: the Xcode
        // `xcrun` shim's `xcrun_db` cache write is the one that killed every
        // `python3` call (`/usr/bin/python3` is the shim; its cache write to
        // $TMPDIR was denied, exit 71, caught live 2026-08-28). Read from the
        // DAEMON's env — same user, same per-user folder as the child.
        // Read-write, not read-only: scratch exists to be written.
        if let Ok(tmpdir) = std::env::var("TMPDIR") {
            let trimmed = tmpdir.trim_end_matches('/');
            if !trimmed.is_empty() {
                profile.push_str("; per-user Mach TMPDIR scratch (xcrun shims, python cache, …)\n");
                // Seatbelt matches against RESOLVED paths, and `/var` is a
                // firmlink to `/private/var` — grant both spellings, the same
                // pattern the /tmp + /private/tmp pair above already uses.
                let mut spellings = vec![trimmed.to_string()];
                if let Some(rest) = trimmed.strip_prefix("/var/") {
                    spellings.push(format!("/private/var/{rest}"));
                } else if let Some(rest) = trimmed.strip_prefix("/tmp/") {
                    spellings.push(format!("/private/tmp/{rest}"));
                }
                for path in spellings {
                    profile.push_str(&format!(
                        "(allow file-read* file-test-existence file-write* (subpath \"{}\"))\n",
                        escape_sbpl(&path)
                    ));
                }
            }
        }

        // Filesystem policy
        self.add_fs_policy(&mut profile, &policy.filesystem, cwd)?;

        // Unreadable-glob security floor (codex-inspired). Emitted *after*
        // the FS allow rules so last-match-wins makes the denies authoritative
        // over any readable/writable root the policy granted above.
        self.add_deny_read_globs(&mut profile);

        // Network policy
        self.add_network_policy(&mut profile, &policy.network)?;

        // Unix-domain socket allowlist (codex parity). AF_UNIX local IPC is
        // otherwise swept up by the network denies emitted above; emit the
        // re-allow *after* them so SBPL last-match-wins reinstates the
        // approved sockets. Redundant under `AllowAll` (network* already
        // open), so skip there.
        if !matches!(policy.network, NetworkPolicy::AllowAll) {
            self.add_unix_socket_policy(&mut profile);
        }

        // Loopback server binding (codex parity). Like the unix-socket
        // re-allow above, this re-grants loopback bind/listen access that the
        // network denies would otherwise sweep up, so emit it after them
        // (SBPL last-match-wins). Redundant under `AllowAll` (network* already
        // open) and gated off by default, so skip in both cases.
        if self.options.allow_local_binding && !matches!(policy.network, NetworkPolicy::AllowAll) {
            self.add_local_binding_policy(&mut profile);
        }

        // Process policy
        self.add_process_policy(&mut profile, &policy.process);

        debug!("generated seatbelt profile ({} bytes)", profile.len());
        Ok(profile)
    }

    // rust-doctor-disable-next-line high-cyclomatic-complexity
    fn add_fs_policy(
        &self,
        profile: &mut String,
        fs: &FsPolicy,
        cwd: &Path,
    ) -> Result<(), SandboxError> {
        let cwd_str = escape_sbpl(cwd.to_str().ok_or_else(|| {
            SandboxError::ProfileGeneration("workspace path contains invalid UTF-8".into())
        })?);

        // Codex-inspired metadata protection: even in writable workspace
        // modes, certain repository-level dirs (.git, .aleph, .codex,
        // .agents) stay read-only so the agent cannot rewrite its own
        // history / audit trail. SBPL evaluates rules in order; later
        // rules win — emit the deny *after* the workspace allow.
        let mut writable_roots: Vec<&Path> = Vec::new();
        match fs {
            FsPolicy::WorkspaceOnly => {
                profile.push_str(&format!(
                    "; workspace-only filesystem access\n\
                     (allow file-read* (subpath \"{cwd_str}\"))\n\
                     (allow file-write* (subpath \"{cwd_str}\"))\n"
                ));
                writable_roots.push(cwd);
            }
            FsPolicy::ReadPaths(paths) => {
                profile.push_str(&format!(
                    "; workspace read/write\n\
                     (allow file-read* (subpath \"{cwd_str}\"))\n\
                     (allow file-write* (subpath \"{cwd_str}\"))\n"
                ));
                writable_roots.push(cwd);
                for path in paths {
                    let path_str = escape_sbpl(path.to_str().ok_or_else(|| {
                        SandboxError::ProfileGeneration(format!(
                            "path contains invalid UTF-8: {}",
                            path.display()
                        ))
                    })?);
                    profile.push_str(&format!("(allow file-read* (subpath \"{path_str}\"))\n"));
                }
            }
            FsPolicy::WritePaths(paths) => {
                profile.push_str(&format!(
                    "; workspace read/write\n\
                     (allow file-read* (subpath \"{cwd_str}\"))\n\
                     (allow file-write* (subpath \"{cwd_str}\"))\n"
                ));
                writable_roots.push(cwd);
                for path in paths {
                    let path_str = escape_sbpl(path.to_str().ok_or_else(|| {
                        SandboxError::ProfileGeneration(format!(
                            "path contains invalid UTF-8: {}",
                            path.display()
                        ))
                    })?);
                    profile.push_str(&format!(
                        "(allow file-read* file-write* (subpath \"{path_str}\"))\n"
                    ));
                    writable_roots.push(path);
                }
            }
            FsPolicy::ReadWritePaths { read, write } => {
                profile.push_str(&format!(
                    "; workspace read/write\n\
                     (allow file-read* (subpath \"{cwd_str}\"))\n\
                     (allow file-write* (subpath \"{cwd_str}\"))\n"
                ));
                writable_roots.push(cwd);
                for path in read {
                    let path_str = escape_sbpl(path.to_str().ok_or_else(|| {
                        SandboxError::ProfileGeneration(format!(
                            "path contains invalid UTF-8: {}",
                            path.display()
                        ))
                    })?);
                    profile.push_str(&format!("(allow file-read* (subpath \"{path_str}\"))\n"));
                }
                for path in write {
                    let path_str = escape_sbpl(path.to_str().ok_or_else(|| {
                        SandboxError::ProfileGeneration(format!(
                            "path contains invalid UTF-8: {}",
                            path.display()
                        ))
                    })?);
                    profile.push_str(&format!(
                        "(allow file-read* file-write* (subpath \"{path_str}\"))\n"
                    ));
                    writable_roots.push(path);
                }
            }
            FsPolicy::FullRead { exclude } => {
                profile.push_str("; full read access\n(allow file-read*)\n");
                for path in exclude {
                    let path_str = escape_sbpl(path.to_str().ok_or_else(|| {
                        SandboxError::ProfileGeneration(format!(
                            "path contains invalid UTF-8: {}",
                            path.display()
                        ))
                    })?);
                    profile.push_str(&format!("(deny file-read* (subpath \"{path_str}\"))\n"));
                }
            }
            FsPolicy::FullWrite { exclude } => {
                profile.push_str("; full read/write access\n(allow file-read* file-write*)\n");
                for path in exclude {
                    let path_str = escape_sbpl(path.to_str().ok_or_else(|| {
                        SandboxError::ProfileGeneration(format!(
                            "path contains invalid UTF-8: {}",
                            path.display()
                        ))
                    })?);
                    profile.push_str(&format!(
                        "(deny file-read* file-write* (subpath \"{path_str}\"))\n"
                    ));
                }
                // FullWrite is explicit danger-full-access — caller opted out
                // of containment. We do not auto-protect metadata here; the
                // explicit `exclude` list is the user's contract.
            }
        }

        // Append metadata-protection deny rules. Last-match-wins in SBPL,
        // so these override the writable allow above.
        if !writable_roots.is_empty() {
            let protected = crate::sandbox::protected_paths::protected_paths_for(
                writable_roots.iter().copied(),
            );
            if !protected.is_empty() {
                profile
                    .push_str("; protected metadata subpaths (read-only inside writable roots)\n");
                for path in &protected {
                    // TOCTOU guard (codex parity, mirrors the bwrap driver's
                    // `push_metadata_protection_args`): refuse to emit a deny
                    // whose path crosses a writable symlink the sandboxed
                    // process could swap out between profile generation and
                    // use. Seatbelt's `subpath` matches the kernel-resolved
                    // path, so a process-created `.git -> /elsewhere` symlink
                    // would let writes escape the deny rule entirely. A
                    // protection that cannot be enforced must fail closed.
                    if let Some(symlink) =
                        crate::sandbox::protected_paths::first_writable_symlink_component(
                            path,
                            &writable_roots,
                        )
                    {
                        return Err(SandboxError::ProfileGeneration(format!(
                            "TOCTOU: cannot enforce read-only protection for {} because it \
                             crosses writable symlink {}; a sandboxed process could redirect \
                             it. Remove or dereference the symlink before granting \
                             workspace-write.",
                            path.display(),
                            symlink.display()
                        )));
                    }
                    if let Some(path_str) = path.to_str() {
                        let path_str = escape_sbpl(path_str);
                        profile.push_str(&format!("(deny file-write* (subpath \"{path_str}\"))\n"));
                    }
                }
            }
        }
        Ok(())
    }

    /// Emit `(deny file-read* …)` + `(deny file-write-unlink …)` rules for
    /// each configured glob. Translating to an anchored regex lets Seatbelt
    /// deny secrets anywhere inside an otherwise-readable root; denying
    /// `file-write-unlink` too means a blocked path cannot be probed via a
    /// destructive `rm`. Mirrors codex's `build_seatbelt_unreadable_glob_policy`.
    fn add_deny_read_globs(&self, profile: &mut String) {
        if self.options.deny_read_globs.is_empty() {
            return;
        }

        let mut emitted = false;
        for pattern in &self.options.deny_read_globs {
            let Some(regex) = crate::sandbox::deny_globs::glob_to_anchored_regex(pattern) else {
                continue;
            };
            // SBPL `(regex #"…")` is a raw double-quoted literal — only the
            // quote needs escaping (the regex never contains a bare `"`, but
            // be defensive in case a pattern injects one).
            let regex = regex.replace('"', "\\\"");
            if !emitted {
                profile.push_str("; unreadable-glob security floor (read + unlink denied)\n");
                emitted = true;
            }
            profile.push_str(&format!("(deny file-read* (regex #\"{regex}\"))\n"));
            profile.push_str(&format!("(deny file-write-unlink (regex #\"{regex}\"))\n"));
        }
    }

    /// Re-allow `AF_UNIX` local-IPC sockets that the network denies above
    /// would otherwise sweep up. Mirrors codex's `unix_socket_policy`:
    /// `dangerously_allow_all_unix_sockets` opens bind/connect broadly,
    /// otherwise each configured absolute path is granted via `subpath` so
    /// sockets created beneath an approved directory are covered too.
    /// Paths are inline-escaped (Aleph convention; equivalent to codex's
    /// `-D` parameter substitution). Emitted only by `generate_profile`
    /// after the network denies so last-match-wins reinstates access.
    fn add_unix_socket_policy(&self, profile: &mut String) {
        if !self.options.dangerously_allow_all_unix_sockets
            && self.options.allow_unix_sockets.is_empty()
        {
            return;
        }

        // The socket primitive itself must be permitted before bind/connect.
        profile.push_str("; unix-domain socket allowlist (local IPC)\n");
        profile.push_str("(allow system-socket (socket-domain AF_UNIX))\n");

        if self.options.dangerously_allow_all_unix_sockets {
            // Keep the dangerous escape hatch genuinely broad — no path
            // qualifier, matching codex's AllowAll arm.
            profile.push_str("(allow network-bind (local unix-socket))\n");
            profile.push_str("(allow network-outbound (remote unix-socket))\n");
            return;
        }

        for path in &self.options.allow_unix_sockets {
            let Some(path_str) = path.to_str() else {
                continue;
            };
            let escaped = escape_sbpl(path_str);
            profile.push_str(&format!(
                "(allow network-bind (local unix-socket (subpath \"{escaped}\")))\n"
            ));
            profile.push_str(&format!(
                "(allow network-outbound (remote unix-socket (subpath \"{escaped}\")))\n"
            ));
        }
    }

    /// Re-allow loopback `network-bind` / `network-inbound` so a sandboxed
    /// process can run a local server under a restricted network floor.
    /// Mirrors codex's `allow_local_binding` block: binding is permitted on
    /// any local address but inbound/outbound traffic is gated to loopback,
    /// so the public internet stays denied. Emitted only by `generate_profile`
    /// after the network denies (SBPL last-match-wins reinstates loopback).
    fn add_local_binding_policy(&self, profile: &mut String) {
        profile.push_str("; loopback server binding (local-binding capability)\n");
        profile.push_str("(allow network-bind (local ip \"*:*\"))\n");
        profile.push_str("(allow network-inbound (local ip \"localhost:*\"))\n");
        profile.push_str("(allow network-outbound (remote ip \"localhost:*\"))\n");
    }

    fn add_network_policy(
        &self,
        profile: &mut String,
        network: &NetworkPolicy,
    ) -> Result<(), SandboxError> {
        match network {
            NetworkPolicy::None => {
                profile.push_str("; no network access\n(deny network*)\n");
            }
            NetworkPolicy::AllowAll => {
                profile.push_str("; full network access\n(allow network*)\n");
            }
            NetworkPolicy::AllowHosts(hosts) => {
                // Seatbelt's `(remote ip "...")` matcher takes IP literals only;
                // hostnames silently never match (silent policy violation).
                // Two upstream layers feed us IP-only entries:
                //
                // 1. Cycle 6 Phase A: `WorkspaceSandbox::maybe_spawn_proxy`
                //    rewrites `AllowHosts { hosts: [<hostnames>] }` to
                //    `AllowHosts { hosts: ["127.0.0.1"] }` and routes the
                //    sandboxed process through a managed in-process proxy.
                //    The proxy enforces the hostname allowlist; the OS
                //    profile only needs to permit loopback.
                // 2. SP-4 DNS pre-resolution in `WorkspaceSandbox::execute`
                //    converts any remaining hostname inputs to IPs.
                //
                // The rejection branch below is defence-in-depth in case
                // someone bypasses the workspace pipeline and feeds raw
                // hostnames to the driver directly.
                for host in hosts {
                    if !host_is_ip_literal(host) {
                        return Err(SandboxError::UnsupportedPolicy {
                            platform: "macos/seatbelt",
                            feature: "NetworkPolicy::AllowHosts (hostname)".into(),
                            reason: format!(
                                "Seatbelt's `(remote ip ...)` accepts IP literals only; \
                                 '{host}' is not an IP. The WorkspaceSandbox pipeline \
                                 normally routes hostname allowlists through the managed \
                                 proxy (Cycle 6 Phase A) so this driver only sees \
                                 loopback. If you are calling the driver directly, \
                                 pre-resolve hostnames or use AllowAll."
                            ),
                        });
                    }
                }
                profile.push_str(RESTRICTED_NETWORK_POLICY);
                for host in hosts {
                    let escaped = escape_sbpl(host);
                    profile.push_str(&format!(
                        "(allow network-outbound (remote ip \"{escaped}\"))\n"
                    ));
                }
            }
        }
        Ok(())
    }

    fn add_process_policy(&self, profile: &mut String, process: &ProcessPolicy) {
        if !process.allow_fork {
            profile.push_str("; deny subprocess spawning\n(deny process-fork)\n");
        }
    }
}

#[async_trait]
impl OsSandboxDriverTrait for SeatbeltDriver {
    fn platform(&self) -> &'static str {
        "macos/seatbelt"
    }

    fn is_supported(&self) -> bool {
        self.check_sandbox_exec()
    }

    fn profile_for(
        &self,
        capabilities: &SandboxCapabilities,
        cwd: &Path,
    ) -> Result<OsSandboxProfile, SandboxError> {
        let policy = SandboxPolicy::from(capabilities);
        let contents = self.generate_profile(&policy, cwd)?;
        Ok(OsSandboxProfile {
            contents,
            max_memory_mb: policy.process.max_memory_mb,
            linux_init_policy: None,
            windows_init_policy: None,
        })
    }

    /// Seatbelt reports every SBPL refusal to the process as EPERM, so what
    /// reaches stderr is that errno's `strerror` text. File, network and
    /// `process-fork` denials are indistinguishable at this layer — which is
    /// why the consumer names all three escalation parameters rather than
    /// picking one.
    fn denial_signatures(&self) -> &'static [&'static str] {
        &["operation not permitted"]
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
        if !self.is_supported() {
            return Err(SandboxError::ExecutionFailed(
                "sandbox-exec not available".into(),
            ));
        }

        // Write profile to a temporary file
        let profile_file = tempfile::NamedTempFile::new().map_err(|e| {
            SandboxError::Io(format!("failed to create temp file for profile: {e}"))
        })?;
        tokio::fs::write(profile_file.path(), &profile.contents)
            .await
            .map_err(|e| SandboxError::Io(format!("failed to write profile: {e}")))?;

        debug!(
            "running sandbox-exec with profile ({} bytes)",
            profile.contents.len()
        );

        let mut cmd = Command::new(SANDBOX_EXEC_PATH);
        cmd.arg("-f")
            .arg(profile_file.path())
            .arg(program)
            .args(args)
            .current_dir(cwd)
            .env_clear()
            .envs(env)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .stdin(std::process::Stdio::piped())
            // Belt-and-braces: if our future is dropped (e.g. an upstream
            // cancellation) the OS reaps the child instead of leaking it.
            .kill_on_drop(true);

        if let Some(mb) = profile.max_memory_mb {
            apply_memory_rlimit(&mut cmd, mb);
        }

        let child = cmd.spawn().map_err(|e| {
            SandboxError::ExecutionFailed(format!("failed to spawn sandbox-exec: {e}"))
        })?;

        // Helper owns stdin write, output drain, timeout-with-kill, and
        // partial-output capture so the three platform drivers stay in
        // lock-step with codex's IO_DRAIN_TIMEOUT discipline.
        run_child_with_drain(child, stdin, timeout, max_output_bytes).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn seatbelt_driver_platform() {
        let driver = SeatbeltDriver::new();
        assert_eq!(driver.platform(), "macos/seatbelt");
    }

    #[test]
    fn denial_dialect_is_seatbelts_own_and_not_a_union() {
        let sigs = SeatbeltDriver::new().denial_signatures();
        assert_eq!(sigs, &["operation not permitted"]);
        // Borrowing another backend's dialect would let a macOS run be
        // reported as a denial macOS cannot emit.
        for foreign in ["read-only file system", "access is denied"] {
            assert!(
                !sigs.contains(&foreign),
                "{foreign} belongs to another backend"
            );
        }
    }

    /// The per-user Mach TMPDIR grant: present whenever the daemon's own env
    /// carries TMPDIR (always on a real macOS boot), and pointing at that
    /// exact path — the xcrun shim's cache write died here (exit 71) before
    /// this grant existed.
    #[test]
    fn profile_grants_per_user_tmpdir_scratch() {
        let Ok(tmpdir) = std::env::var("TMPDIR") else {
            // No TMPDIR in this environment ⇒ no grant emitted ⇒ nothing to
            // assert (CI sandboxes sometimes strip it).
            return;
        };
        let driver = SeatbeltDriver::new();
        let policy = SandboxPolicy::default();
        let profile = driver
            .generate_profile(&policy, Path::new("/tmp/test-workspace"))
            .unwrap();
        let trimmed = tmpdir.trim_end_matches('/');
        assert!(
            profile.contains(&format!("(subpath \"{trimmed}\")")),
            "profile must grant scratch access to the per-user TMPDIR {trimmed}"
        );
        // Seatbelt matches resolved paths; /var is a firmlink to /private/var,
        // so the firmlink twin must be granted alongside (the /tmp + /private/tmp
        // pattern in PLATFORM_DEFAULTS_POLICY).
        if let Some(rest) = trimmed.strip_prefix("/var/") {
            let twin = format!("/private/var/{rest}");
            assert!(
                profile.contains(&format!("(subpath \"{twin}\")")),
                "profile must grant the firmlink twin {twin}"
            );
        }
    }

    #[test]
    fn generate_profile_workspace_only() {
        let driver = SeatbeltDriver::new();
        let policy = SandboxPolicy::default();
        let cwd = Path::new("/tmp/test-workspace");
        let profile = driver.generate_profile(&policy, cwd).unwrap();

        assert!(profile.contains("(version 1)"));
        assert!(profile.contains("(deny default)"));
        assert!(profile.contains("(subpath \"/tmp/test-workspace\")"));
        assert!(profile.contains("(deny network*)"));
        assert!(profile.contains("(deny process-fork)"));
    }

    #[test]
    fn generate_profile_with_read_paths() {
        let driver = SeatbeltDriver::new();
        let policy = SandboxPolicy {
            filesystem: FsPolicy::ReadPaths(vec![
                PathBuf::from("/etc"),
                PathBuf::from("/usr/share"),
            ]),
            ..Default::default()
        };
        let cwd = Path::new("/tmp/ws");
        let profile = driver.generate_profile(&policy, cwd).unwrap();

        assert!(profile.contains("(subpath \"/tmp/ws\")"));
        assert!(profile.contains("(subpath \"/etc\")"));
        assert!(profile.contains("(subpath \"/usr/share\")"));
        // Read paths should only have file-read*
        assert!(profile.contains("(allow file-read* (subpath \"/etc\"))"));
    }

    #[test]
    fn generate_profile_with_write_paths() {
        let driver = SeatbeltDriver::new();
        let policy = SandboxPolicy {
            filesystem: FsPolicy::WritePaths(vec![PathBuf::from("/tmp/output")]),
            ..Default::default()
        };
        let cwd = Path::new("/tmp/ws");
        let profile = driver.generate_profile(&policy, cwd).unwrap();

        assert!(profile.contains("(allow file-read* file-write* (subpath \"/tmp/output\"))"));
    }

    #[test]
    fn workspace_only_protects_git_and_aleph_subpaths() {
        let driver = SeatbeltDriver::new();
        let policy = SandboxPolicy {
            filesystem: FsPolicy::WorkspaceOnly,
            ..Default::default()
        };
        let cwd = Path::new("/tmp/ws");
        let profile = driver.generate_profile(&policy, cwd).unwrap();

        // Allow comes first.
        let allow_idx = profile
            .find("(allow file-write* (subpath \"/tmp/ws\"))")
            .expect("workspace allow must be present");
        // Deny rules come AFTER allow so last-match-wins makes them read-only.
        let git_idx = profile
            .find("(deny file-write* (subpath \"/tmp/ws/.git\"))")
            .expect("metadata .git must be deny-listed");
        let aleph_idx = profile
            .find("(deny file-write* (subpath \"/tmp/ws/.aleph\"))")
            .expect("metadata .aleph must be deny-listed");
        assert!(allow_idx < git_idx, "deny must come after allow");
        assert!(allow_idx < aleph_idx, "deny must come after allow");
        // And the codex-aligned .codex / .agents are also covered.
        assert!(profile.contains("(deny file-write* (subpath \"/tmp/ws/.codex\"))"));
        assert!(profile.contains("(deny file-write* (subpath \"/tmp/ws/.agents\"))"));
    }

    #[test]
    fn write_paths_protects_metadata_in_each_writable_root() {
        let driver = SeatbeltDriver::new();
        let policy = SandboxPolicy {
            filesystem: FsPolicy::WritePaths(vec![
                PathBuf::from("/tmp/extra1"),
                PathBuf::from("/tmp/extra2"),
            ]),
            ..Default::default()
        };
        let cwd = Path::new("/tmp/ws");
        let profile = driver.generate_profile(&policy, cwd).unwrap();

        // cwd is the first writable root + each WritePaths entry.
        for root in ["/tmp/ws", "/tmp/extra1", "/tmp/extra2"] {
            assert!(
                profile.contains(&format!("(deny file-write* (subpath \"{root}/.git\"))")),
                "missing .git protection under {root}"
            );
            assert!(
                profile.contains(&format!("(deny file-write* (subpath \"{root}/.aleph\"))")),
                "missing .aleph protection under {root}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn protected_metadata_symlink_in_writable_root_fails_closed() {
        // TOCTOU (mirrors the bwrap driver test): the sandboxed process turned
        // the protected `.git` into a symlink. Seatbelt's `subpath` deny rule
        // matches the kernel-resolved path, so writes through the symlink would
        // escape the deny entirely — profile generation must fail closed rather
        // than emit an unenforceable protection.
        let driver = SeatbeltDriver::new();
        let policy = SandboxPolicy::default();
        let ws = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), ws.path().join(".git")).unwrap();

        let err = driver
            .generate_profile(&policy, ws.path())
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
        let driver = SeatbeltDriver::new();
        let policy = SandboxPolicy {
            filesystem: FsPolicy::FullWrite { exclude: vec![] },
            ..Default::default()
        };
        let cwd = Path::new("/tmp/ws");
        let profile = driver.generate_profile(&policy, cwd).unwrap();

        // FullWrite is explicit danger-full-access. We do not auto-protect
        // here; the caller's explicit `exclude` list is the contract.
        assert!(!profile.contains("(deny file-write* (subpath \"/tmp/ws/.git\"))"));
    }

    #[test]
    fn deny_read_globs_emit_read_and_unlink_denies_after_allow() {
        let driver = SeatbeltDriver::with_options(SeatbeltOptions {
            deny_read_globs: vec!["**/.env".into(), "**/*.pem".into()],
            ..Default::default()
        });
        // FullWrite grants broad access; the glob floor must still deny.
        let policy = SandboxPolicy {
            filesystem: FsPolicy::FullWrite { exclude: vec![] },
            ..Default::default()
        };
        let cwd = Path::new("/tmp/ws");
        let profile = driver.generate_profile(&policy, cwd).unwrap();

        let read_deny = r#"(deny file-read* (regex #"^(.*/)?\.env$"))"#;
        let unlink_deny = r#"(deny file-write-unlink (regex #"^(.*/)?\.env$"))"#;
        assert!(profile.contains(read_deny), "missing read deny for .env");
        assert!(
            profile.contains(unlink_deny),
            "missing unlink deny for .env"
        );
        assert!(
            profile.contains(r#"(deny file-read* (regex #"^(.*/)?[^/]*\.pem$"))"#),
            "missing read deny for .pem"
        );

        // Last-match-wins: the deny must come AFTER the broad allow.
        let allow_idx = profile
            .find("(allow file-read* file-write*)")
            .expect("FullWrite allow present");
        let deny_idx = profile.find(read_deny).unwrap();
        assert!(allow_idx < deny_idx, "glob deny must follow the allow rule");
    }

    #[test]
    fn no_deny_globs_emits_no_floor() {
        let driver = SeatbeltDriver::new();
        let policy = SandboxPolicy::default();
        let cwd = Path::new("/tmp/ws");
        let profile = driver.generate_profile(&policy, cwd).unwrap();
        assert!(!profile.contains("unreadable-glob security floor"));
        assert!(!profile.contains("(deny file-write-unlink"));
    }

    #[test]
    fn no_unix_sockets_emits_no_af_unix_allowance() {
        // Default options → default-deny preserved, byte-identical to before.
        let driver = SeatbeltDriver::new();
        let policy = SandboxPolicy::default(); // network: None
        let cwd = Path::new("/tmp/ws");
        let profile = driver.generate_profile(&policy, cwd).unwrap();
        assert!(!profile.contains("unix-domain socket allowlist"));
        assert!(!profile.contains("AF_UNIX"));
        assert!(!profile.contains("unix-socket"));
    }

    #[test]
    fn allow_unix_sockets_reallows_after_network_deny() {
        let driver = SeatbeltDriver::with_options(SeatbeltOptions {
            allow_unix_sockets: vec![PathBuf::from("/tmp/agent.sock")],
            ..Default::default()
        });
        // Strict default network is `None` → emits `(deny network*)`.
        let policy = SandboxPolicy::default();
        let cwd = Path::new("/tmp/ws");
        let profile = driver.generate_profile(&policy, cwd).unwrap();

        assert!(profile.contains("(allow system-socket (socket-domain AF_UNIX))"));
        let bind = r#"(allow network-bind (local unix-socket (subpath "/tmp/agent.sock")))"#;
        let out = r#"(allow network-outbound (remote unix-socket (subpath "/tmp/agent.sock")))"#;
        assert!(profile.contains(bind), "missing unix-socket bind allow");
        assert!(profile.contains(out), "missing unix-socket outbound allow");

        // Last-match-wins: the AF_UNIX re-allow must come AFTER the deny.
        let deny_idx = profile
            .find("(deny network*)")
            .expect("network deny present");
        let allow_idx = profile.find(bind).unwrap();
        assert!(
            deny_idx < allow_idx,
            "unix re-allow must follow network deny"
        );
    }

    #[test]
    fn dangerously_allow_all_unix_sockets_is_broad() {
        let driver = SeatbeltDriver::with_options(SeatbeltOptions {
            dangerously_allow_all_unix_sockets: true,
            // allowlist ignored when allow-all is on.
            allow_unix_sockets: vec![PathBuf::from("/tmp/ignored.sock")],
            ..Default::default()
        });
        let policy = SandboxPolicy::default();
        let cwd = Path::new("/tmp/ws");
        let profile = driver.generate_profile(&policy, cwd).unwrap();

        assert!(profile.contains("(allow system-socket (socket-domain AF_UNIX))"));
        assert!(profile.contains("(allow network-bind (local unix-socket))"));
        assert!(profile.contains("(allow network-outbound (remote unix-socket))"));
        // The broad form omits the per-path subpath qualifier.
        assert!(!profile.contains("(subpath \"/tmp/ignored.sock\")"));
    }

    #[test]
    fn unix_sockets_skipped_under_allow_all_network() {
        // Under AllowAll, network* is already open — no redundant AF_UNIX block.
        let driver = SeatbeltDriver::with_options(SeatbeltOptions {
            allow_unix_sockets: vec![PathBuf::from("/tmp/agent.sock")],
            ..Default::default()
        });
        let policy = SandboxPolicy {
            network: NetworkPolicy::AllowAll,
            ..Default::default()
        };
        let cwd = Path::new("/tmp/ws");
        let profile = driver.generate_profile(&policy, cwd).unwrap();
        assert!(profile.contains("(allow network*)"));
        assert!(!profile.contains("unix-domain socket allowlist"));
    }

    #[test]
    fn unix_sockets_reallowed_under_restricted_hosts() {
        // AllowHosts also emits a network deny floor; the re-allow must apply.
        let driver = SeatbeltDriver::with_options(SeatbeltOptions {
            allow_unix_sockets: vec![PathBuf::from("/var/run/db.sock")],
            ..Default::default()
        });
        let policy = SandboxPolicy {
            network: NetworkPolicy::AllowHosts(vec!["10.0.0.1".into()]),
            ..Default::default()
        };
        let cwd = Path::new("/tmp/ws");
        let profile = driver.generate_profile(&policy, cwd).unwrap();
        let deny_idx = profile
            .find("(deny network*)")
            .expect("restricted deny present");
        let allow_idx = profile
            .find(r#"(local unix-socket (subpath "/var/run/db.sock"))"#)
            .expect("unix re-allow present");
        assert!(deny_idx < allow_idx);
    }

    #[test]
    fn no_local_binding_emits_no_bind_rules() {
        // Default options → default-deny floor preserved, byte-identical to
        // before this capability existed.
        let driver = SeatbeltDriver::new();
        let policy = SandboxPolicy::default(); // network: None
        let cwd = Path::new("/tmp/ws");
        let profile = driver.generate_profile(&policy, cwd).unwrap();
        assert!(!profile.contains("local-binding capability"));
        assert!(!profile.contains("(allow network-bind (local ip"));
        assert!(!profile.contains("(allow network-inbound"));
    }

    #[test]
    fn local_binding_reallows_loopback_after_network_deny() {
        // Under the strict `None` policy the floor is `(deny network*)`; the
        // loopback bind/listen re-allow must follow it (last-match-wins) and
        // must not open any non-loopback outbound route.
        let driver = SeatbeltDriver::with_options(SeatbeltOptions {
            allow_local_binding: true,
            ..Default::default()
        });
        let policy = SandboxPolicy::default();
        let cwd = Path::new("/tmp/ws");
        let profile = driver.generate_profile(&policy, cwd).unwrap();

        let bind = r#"(allow network-bind (local ip "*:*"))"#;
        let inbound = r#"(allow network-inbound (local ip "localhost:*"))"#;
        let outbound = r#"(allow network-outbound (remote ip "localhost:*"))"#;
        assert!(profile.contains(bind), "missing loopback bind allow");
        assert!(profile.contains(inbound), "missing loopback inbound allow");
        assert!(
            profile.contains(outbound),
            "missing loopback outbound allow"
        );

        let deny_idx = profile
            .find("(deny network*)")
            .expect("network deny present");
        let bind_idx = profile.find(bind).unwrap();
        assert!(
            deny_idx < bind_idx,
            "local-binding re-allow must follow network deny"
        );
    }

    #[test]
    fn local_binding_reallowed_under_restricted_hosts() {
        // AllowHosts emits a deny floor + per-host allow; loopback binding
        // must still be reinstated after it.
        let driver = SeatbeltDriver::with_options(SeatbeltOptions {
            allow_local_binding: true,
            ..Default::default()
        });
        let policy = SandboxPolicy {
            network: NetworkPolicy::AllowHosts(vec!["10.0.0.1".into()]),
            ..Default::default()
        };
        let cwd = Path::new("/tmp/ws");
        let profile = driver.generate_profile(&policy, cwd).unwrap();

        let deny_idx = profile
            .find("(deny network*)")
            .expect("restricted deny present");
        let bind_idx = profile
            .find(r#"(allow network-bind (local ip "*:*"))"#)
            .expect("loopback bind re-allow present");
        assert!(deny_idx < bind_idx);
    }

    #[test]
    fn local_binding_skipped_under_allow_all_network() {
        // Under AllowAll, `(allow network*)` already covers binding — no
        // redundant loopback block is emitted.
        let driver = SeatbeltDriver::with_options(SeatbeltOptions {
            allow_local_binding: true,
            ..Default::default()
        });
        let policy = SandboxPolicy {
            network: NetworkPolicy::AllowAll,
            ..Default::default()
        };
        let cwd = Path::new("/tmp/ws");
        let profile = driver.generate_profile(&policy, cwd).unwrap();
        assert!(profile.contains("(allow network*)"));
        assert!(!profile.contains("local-binding capability"));
    }

    #[test]
    fn generate_profile_with_ip_allow_hosts_succeeds() {
        let driver = SeatbeltDriver::new();
        let policy = SandboxPolicy {
            network: NetworkPolicy::AllowHosts(vec![
                "93.184.216.34".into(),
                "2606:2800:220:1:248:1893:25c8:1946".into(),
                "10.0.0.1:443".into(),
            ]),
            ..Default::default()
        };
        let cwd = Path::new("/tmp/ws");
        let profile = driver.generate_profile(&policy, cwd).unwrap();

        assert!(profile.contains("(allow network-outbound (remote ip \"93.184.216.34\"))"));
        assert!(profile.contains("(allow network-outbound (remote ip \"10.0.0.1:443\"))"));
    }

    #[test]
    fn generate_profile_with_hostname_allow_hosts_returns_unsupported() {
        let driver = SeatbeltDriver::new();
        let policy = SandboxPolicy {
            network: NetworkPolicy::AllowHosts(vec!["example.com".into()]),
            ..Default::default()
        };
        let cwd = Path::new("/tmp/ws");
        let err = driver
            .generate_profile(&policy, cwd)
            .expect_err("hostnames must hard-fail on macos/seatbelt");
        match err {
            SandboxError::UnsupportedPolicy {
                platform, reason, ..
            } => {
                assert_eq!(platform, "macos/seatbelt");
                assert!(reason.contains("Pre-resolve") || reason.contains("IP literals"));
            }
            other => panic!("expected UnsupportedPolicy, got {other:?}"),
        }
    }

    #[test]
    fn generate_profile_allow_all_network() {
        let driver = SeatbeltDriver::new();
        let policy = SandboxPolicy {
            network: NetworkPolicy::AllowAll,
            ..Default::default()
        };
        let cwd = Path::new("/tmp/ws");
        let profile = driver.generate_profile(&policy, cwd).unwrap();

        assert!(profile.contains("(allow network*)"));
    }

    #[test]
    fn generate_profile_allow_fork() {
        let driver = SeatbeltDriver::new();
        let policy = SandboxPolicy {
            process: ProcessPolicy {
                allow_fork: true,
                max_memory_mb: None,
            },
            ..Default::default()
        };
        let cwd = Path::new("/tmp/ws");
        let profile = driver.generate_profile(&policy, cwd).unwrap();

        // When fork is allowed, we should NOT see (deny process-fork)
        assert!(!profile.contains("(deny process-fork)"));
    }

    #[test]
    fn generate_profile_full_read_with_exclusions() {
        let driver = SeatbeltDriver::new();
        let policy = SandboxPolicy {
            filesystem: FsPolicy::FullRead {
                exclude: vec![PathBuf::from("/etc/passwd")],
            },
            ..Default::default()
        };
        let cwd = Path::new("/tmp/ws");
        let profile = driver.generate_profile(&policy, cwd).unwrap();

        assert!(profile.contains("(allow file-read*)"));
        assert!(profile.contains("(deny file-read* (subpath \"/etc/passwd\"))"));
    }

    #[test]
    fn profile_for_from_capabilities() {
        let driver = SeatbeltDriver::new();
        let caps = SandboxCapabilities {
            fs_read: vec!["/tmp".into()],
            network: crate::sandbox::capabilities::NetworkPolicy::AllowAll,
            spawn_subprocess: true,
            ..Default::default()
        };
        let cwd = Path::new("/tmp/ws");
        let profile = driver.profile_for(&caps, cwd).unwrap();

        assert!(profile.contents.contains("(allow network*)"));
        assert!(!profile.contents.contains("(deny process-fork)"));
    }

    #[test]
    fn profile_for_threads_max_memory_mb() {
        let driver = SeatbeltDriver::new();
        let caps = SandboxCapabilities {
            max_memory_mb: Some(128),
            ..Default::default()
        };
        let cwd = Path::new("/tmp/ws");
        let profile = driver.profile_for(&caps, cwd).unwrap();
        assert_eq!(profile.max_memory_mb, Some(128));
    }

    #[test]
    fn host_is_ip_literal_accepts_all_canonical_forms() {
        // Bare IPv4
        assert!(host_is_ip_literal("127.0.0.1"));
        assert!(host_is_ip_literal("93.184.216.34"));
        // Bare IPv6
        assert!(host_is_ip_literal("::1"));
        assert!(host_is_ip_literal("2606:2800:220:1:248:1893:25c8:1946"));
        // IPv4 with port
        assert!(host_is_ip_literal("127.0.0.1:443"));
        assert!(host_is_ip_literal("10.0.0.1:8080"));
        // Bracketed IPv6 with/without port
        assert!(host_is_ip_literal("[::1]"));
        assert!(host_is_ip_literal("[::1]:443"));
        assert!(host_is_ip_literal("[2606:2800::1]:80"));
    }

    #[test]
    fn host_is_ip_literal_rejects_hostnames() {
        assert!(!host_is_ip_literal("example.com"));
        assert!(!host_is_ip_literal("api.example.com"));
        assert!(!host_is_ip_literal("api.example.com:443"));
        assert!(!host_is_ip_literal("localhost"));
        assert!(!host_is_ip_literal(""));
        // Malformed brackets
        assert!(!host_is_ip_literal("[::1"));
        assert!(!host_is_ip_literal("[example.com]"));
    }

    #[test]
    fn platform_defaults_present_in_workspace_profile() {
        // Spot-check that the full codex-port content made it in.
        let driver = SeatbeltDriver::new();
        let policy = SandboxPolicy::default();
        let cwd = Path::new("/tmp/ws");
        let profile = driver.generate_profile(&policy, cwd).unwrap();

        // These tokens are unique to PLATFORM_DEFAULTS_POLICY and would
        // disappear if a future refactor accidentally drops the block.
        assert!(
            profile.contains("(allow system-mac-syscall (mac-policy-name \"vnguard\"))"),
            "vnguard mac-syscall missing — dyld cannot resolve guarded vnodes"
        );
        assert!(
            profile.contains("(global-name \"com.apple.logd\")"),
            "logd mach-lookup missing — many CoreFoundation programs SIGABRT without it"
        );
        assert!(
            profile.contains("(allow file-read-data (subpath \"/usr/libexec\"))"),
            "/usr/libexec read-data missing — exec helpers (env, dyld) fail to launch"
        );
        // Codex's network policy mach-lookups appear only when network is
        // restricted, not in the always-on platform defaults.
        assert!(!profile.contains("(global-name \"com.apple.SecurityServer\")"));
    }

    #[test]
    fn restricted_network_includes_codex_resolver_lookups() {
        let driver = SeatbeltDriver::new();
        let policy = SandboxPolicy {
            network: NetworkPolicy::AllowHosts(vec!["10.0.0.1".into()]),
            ..Default::default()
        };
        let cwd = Path::new("/tmp/ws");
        let profile = driver.generate_profile(&policy, cwd).unwrap();

        assert!(profile.contains("(global-name \"com.apple.SecurityServer\")"));
        assert!(
            profile.contains("(global-name \"com.apple.SystemConfiguration.DNSConfiguration\")")
        );
        assert!(profile.contains("(allow network-outbound (remote ip \"10.0.0.1\"))"));
    }

    /// Live smoke test: run `/bin/echo hello` inside a workspace-only
    /// sandbox and verify the binary actually executes. With the
    /// pre-cycle-3 minimum SBPL this SIGABRT'd in dyld; the full codex
    /// platform-defaults port is what lets this pass.
    ///
    /// Gated on `is_supported()` so the test no-ops cleanly on
    /// Linux/Windows dev boxes (and CI without `/usr/bin/sandbox-exec`).
    #[tokio::test]
    async fn echo_runs_inside_workspace_sandbox() {
        let driver = SeatbeltDriver::new();
        if !driver.is_supported() {
            return;
        }

        let tmp = tempfile::tempdir().expect("tempdir");
        let cwd = tmp.path();
        let caps = SandboxCapabilities {
            fs_read: vec![],
            network: crate::sandbox::capabilities::NetworkPolicy::None,
            spawn_subprocess: true,
            ..Default::default()
        };
        let profile = driver.profile_for(&caps, cwd).unwrap();
        let env = HashMap::new();

        let out = driver
            .run(
                "/bin/echo",
                &["hello".to_string()],
                &env,
                None,
                cwd,
                &profile,
                Duration::from_secs(10),
                4096,
            )
            .await
            .expect("/bin/echo must run inside the sandbox now that codex defaults are in place");

        assert_eq!(out.exit_code, Some(0), "echo exit code");
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            "hello",
            "echo stdout"
        );
    }
}
