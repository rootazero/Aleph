//! macOS Seatbelt sandbox driver — generates SBPL profiles and executes
//! via `/usr/bin/sandbox-exec`.
//!
//! Inspired by codex's seatbelt implementation but adapted for Aleph's
//! SandboxPolicy / SandboxCapabilities model.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use tokio::process::Command;
use tracing::debug;

use crate::sandbox::capabilities::SandboxCapabilities;
use crate::sandbox::command::{SandboxError, SandboxOutput};
use crate::sandbox::driver::{OsSandboxDriverTrait, OsSandboxProfile};
use crate::sandbox::policy::{EnvPolicy, FsPolicy, NetworkPolicy, ProcessPolicy, SandboxPolicy};

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

/// Driver for macOS seatbelt sandboxing.
#[derive(Debug, Clone)]
pub struct SeatbeltDriver;

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
    unsafe {
        // SAFETY: setrlimit with RLIMIT_AS is async-signal-safe and well-defined.
        // No allocator, mutex, or other handler-unsafe call is made.
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
    pub fn new() -> Self {
        Self
    }

    /// Check if `sandbox-exec` is available and executable.
    fn check_sandbox_exec(&self) -> bool {
        std::fs::metadata(SANDBOX_EXEC_PATH)
            .map(|m| m.is_file())
            .unwrap_or(false)
    }

    /// Generate SBPL profile from SandboxPolicy.
    fn generate_profile(&self, policy: &SandboxPolicy, cwd: &Path) -> Result<String, SandboxError> {
        let mut profile = String::with_capacity(8192);

        // Base policy — process/sysctl/IOKit/PTY essentials.
        profile.push_str(BASE_POLICY);
        profile.push('\n');

        // Read-only system trees + mach-lookups + scratch space.
        // This is the slab that lets `/bin/echo` and friends actually run.
        profile.push_str(PLATFORM_DEFAULTS_POLICY);
        profile.push('\n');

        // Filesystem policy
        self.add_fs_policy(&mut profile, &policy.filesystem, cwd)?;

        // Network policy
        self.add_network_policy(&mut profile, &policy.network)?;

        // Process policy
        self.add_process_policy(&mut profile, &policy.process);

        // Environment policy
        self.add_env_policy(&mut profile, &policy.environment);

        debug!("generated seatbelt profile ({} bytes)", profile.len());
        Ok(profile)
    }

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
                     (allow file-read* (subpath \"{}\"))\n\
                     (allow file-write* (subpath \"{}\"))\n",
                    cwd_str, cwd_str
                ));
                writable_roots.push(cwd);
            }
            FsPolicy::ReadPaths(paths) => {
                profile.push_str(&format!(
                    "; workspace read/write\n\
                     (allow file-read* (subpath \"{}\"))\n\
                     (allow file-write* (subpath \"{}\"))\n",
                    cwd_str, cwd_str
                ));
                writable_roots.push(cwd);
                for path in paths {
                    let path_str = escape_sbpl(path.to_str().ok_or_else(|| {
                        SandboxError::ProfileGeneration(format!(
                            "path contains invalid UTF-8: {}",
                            path.display()
                        ))
                    })?);
                    profile.push_str(&format!("(allow file-read* (subpath \"{}\"))\n", path_str));
                }
            }
            FsPolicy::WritePaths(paths) => {
                profile.push_str(&format!(
                    "; workspace read/write\n\
                     (allow file-read* (subpath \"{}\"))\n\
                     (allow file-write* (subpath \"{}\"))\n",
                    cwd_str, cwd_str
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
                        "(allow file-read* file-write* (subpath \"{}\"))\n",
                        path_str
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
                    profile.push_str(&format!("(deny file-read* (subpath \"{}\"))\n", path_str));
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
                        "(deny file-read* file-write* (subpath \"{}\"))\n",
                        path_str
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
            let protected =
                crate::sandbox::protected_paths::protected_paths_for(writable_roots.iter().copied());
            if !protected.is_empty() {
                profile.push_str("; protected metadata subpaths (read-only inside writable roots)\n");
                for path in &protected {
                    if let Some(path_str) = path.to_str() {
                        let path_str = escape_sbpl(path_str);
                        profile.push_str(&format!(
                            "(deny file-write* (subpath \"{}\"))\n",
                            path_str
                        ));
                    }
                }
            }
        }
        Ok(())
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
                // hostnames silently never match (silent policy violation). Reject
                // hostnames at profile-generation time so the caller learns
                // immediately rather than discovering at runtime.
                for host in hosts {
                    if !host_is_ip_literal(host) {
                        return Err(SandboxError::UnsupportedPolicy {
                            platform: "macos/seatbelt",
                            feature: "NetworkPolicy::AllowHosts (hostname)".into(),
                            reason: format!(
                                "Seatbelt's `(remote ip ...)` accepts IP literals only; \
                                 '{host}' is not an IP. Pre-resolve hostnames to IPs at \
                                 the call site, or use AllowAll. Hostname-based filtering \
                                 is deferred to spec SP-4."
                            ),
                        });
                    }
                }
                profile.push_str(RESTRICTED_NETWORK_POLICY);
                for host in hosts {
                    let escaped = escape_sbpl(host);
                    profile.push_str(&format!(
                        "(allow network-outbound (remote ip \"{}\"))\n",
                        escaped
                    ));
                }
            }
            NetworkPolicy::ProxyOnly { ports } => {
                profile.push_str(RESTRICTED_NETWORK_POLICY);
                profile.push_str("; proxy-only network access\n");
                for port in ports {
                    profile.push_str(&format!(
                        "(allow network-outbound (remote ip \"localhost:{}\"))\n",
                        port
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

    fn add_env_policy(&self, profile: &mut String, env: &EnvPolicy) {
        // Environment restriction is applied at the `Command::env_clear()`
        // boundary before sandbox-exec is invoked (see the `run` method
        // below); SBPL itself has no `(with environment)` modifier and
        // emitting one silently produces an invalid profile that
        // sandbox-exec rejects at runtime. We therefore only annotate
        // the profile here so the policy intent is visible in dumps.
        match env {
            EnvPolicy::Inherit => {
                // Default — no annotation needed.
            }
            EnvPolicy::Restricted => {
                profile.push_str("; environment policy: restricted (enforced at exec layer)\n");
            }
            EnvPolicy::Minimal => {
                profile.push_str("; environment policy: minimal (enforced at exec layer)\n");
            }
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
        std::fs::write(profile_file.path(), &profile.contents)
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
            .stdin(std::process::Stdio::piped());

        if let Some(mb) = profile.max_memory_mb {
            apply_memory_rlimit(&mut cmd, mb);
        }

        let mut child = cmd.spawn().map_err(|e| {
            SandboxError::ExecutionFailed(format!("failed to spawn sandbox-exec: {e}"))
        })?;

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
                "sandbox-exec execution error: {e}"
            ))),
            Err(_) => Err(SandboxError::Timeout { elapsed_ms }),
        }
    }
}

impl Default for SeatbeltDriver {
    fn default() -> Self {
        Self::new()
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
                profile.contains(&format!(
                    "(deny file-write* (subpath \"{root}/.git\"))"
                )),
                "missing .git protection under {root}"
            );
            assert!(
                profile.contains(&format!(
                    "(deny file-write* (subpath \"{root}/.aleph\"))"
                )),
                "missing .aleph protection under {root}"
            );
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
                timeout_secs: 60,
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
        assert!(profile.contains("(global-name \"com.apple.SystemConfiguration.DNSConfiguration\")"));
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
