//! SP-5 — Linux cgroups v2 resource limits.
//!
//! Provides `CgroupV2Scope`: an RAII handle to a per-execution sub-cgroup
//! created under the current process's own cgroup. The bwrap driver
//! constructs a scope before `spawn`, writes the child PID into
//! `cgroup.procs` via `pre_exec`, then lets `Drop` `rmdir` the cgroup
//! after `wait()`. Failure at any setup step soft-degrades — `RLIMIT_AS`
//! continues to apply, and the sandbox runs without cgroup containment.
//!
//! Cross-platform parts (parsing `/proc/self/cgroup`, formatting limit
//! file contents) compile on macOS so unit tests run on dev boxes; the
//! kernel-touching `try_create` / `Drop` body is
//! `#[cfg(target_os = "linux")]`-gated.
//!
//! Per spec § 1, this is intentionally cgroups-v2-only. v1 hierarchies
//! return `None` from `try_create` (the v1 line format starts with
//! non-zero hierarchy IDs and is detected by the parser).

#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::path::PathBuf;

/// Standard cgroup v2 unified hierarchy mount point. Mandatory on cgroup
/// v2 systems; absent on cgroup-v1-only systems.
pub const UNIFIED_HIERARCHY: &str = "/sys/fs/cgroup";

/// CPU bandwidth period in microseconds. 100ms is the kernel default and
/// what `cpu.max` expects as the second column.
pub const CPU_PERIOD_US: u64 = 100_000;

/// Parse the first cgroup-v2 line from `/proc/self/cgroup` content.
///
/// cgroup v2 lines look like: `0::/user.slice/.../my.scope\n`
/// cgroup v1 lines look like: `3:freezer:/foo\n` (non-zero hierarchy id,
/// controller name in the middle column)
///
/// Returns `Some(PathBuf)` only for the `0::<path>` v2 form. On a v2-only
/// system, the first (and usually only) line is the v2 line.
pub fn parse_proc_self_cgroup_path(contents: &str) -> Option<PathBuf> {
    for line in contents.lines() {
        let mut parts = line.splitn(3, ':');
        let id = parts.next()?;
        let ctrl = parts.next()?;
        let path = parts.next()?;
        if id == "0" && ctrl.is_empty() {
            let path = PathBuf::from(path);
            // Defense-in-depth: the unified-hierarchy path is later joined
            // under `/sys/fs/cgroup`, and `Path::join` does NOT normalize
            // `..`. A `ParentDir` component would let the join escape the
            // hierarchy root, so reject it (soft-degrade: SP-5 skipped).
            if path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                return None;
            }
            return Some(path);
        }
    }
    None
}

/// Format a `cpu.max` line for `pct`% of one CPU.
/// `cpu.max` accepts `"<quota_us> <period_us>"`. Quota = pct * period / 100.
/// `pct = 100` → "100000 100000" (one full core).
/// `pct > 100` → values > one core (multi-core; kernel accepts).
/// `pct = 0` → "0 100000" (which the kernel interprets as "no CPU at all"
/// — pathological but we don't pre-clamp; caller's choice).
pub fn cpu_quota_max_line(pct: u32) -> String {
    let quota_us = u64::from(pct).saturating_mul(CPU_PERIOD_US) / 100;
    format!("{quota_us} {CPU_PERIOD_US}")
}

/// Convert a megabyte ceiling to a byte count for `memory.max`.
/// Saturating to avoid wrap on absurdly large `mb` inputs.
pub const fn memory_max_bytes(mb: u64) -> u64 {
    mb.saturating_mul(1024 * 1024)
}

/// RAII handle to a per-execution cgroup v2 sub-cgroup. The directory
/// is created in `try_create`, the child PID is written by the bwrap
/// driver's `write_current_pid_to_path` helper (from `pre_exec` in the
/// bwrap spawn), and the directory is `rmdir`-ed on `Drop` after the
/// child has been reaped.
///
/// `path` is the absolute filesystem path to the cgroup directory
/// (e.g. `/sys/fs/cgroup/user.slice/.../aleph-sandbox-12345`). The
/// caller never needs to manipulate it directly — the lifecycle methods
/// cover the full span.
#[derive(Debug)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub struct CgroupV2Scope {
    path: PathBuf,
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
impl CgroupV2Scope {
    /// Convenience: `<path>/cgroup.procs`.
    pub fn procs_path(&self) -> PathBuf {
        self.path.join("cgroup.procs")
    }
}

/// Configuration passed into `CgroupV2Scope::try_create`. Each `Option`
/// of `None` means "don't write this controller file" (kernel default
/// applies — usually `max`).
#[derive(Debug, Clone, Copy, Default)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub struct CgroupV2Limits {
    /// `memory.max` in megabytes. `None` → unlimited (kernel default).
    pub memory_mb: Option<u64>,
    /// `cpu.max` quota as a percentage of one CPU. `100` = one full
    /// core; `50` = half a core; `200` = two cores.
    pub cpu_quota_percent: Option<u32>,
    /// `pids.max` ceiling. Defaults to `None` (kernel default = max)
    /// at this layer; callers typically supply a sensible cap like 200.
    pub max_pids: Option<u32>,
}

#[cfg(target_os = "linux")]
impl CgroupV2Scope {
    /// Attempt to create a per-execution sub-cgroup under the current
    /// process's cgroup and apply `limits`. Returns `None` on any
    /// failure (caller's responsibility to interpret as soft-degrade
    /// vs. hard error).
    ///
    /// Failure cases logged at `warn` level (gated by `Once` to avoid
    /// spam):
    /// - cgroup v2 not mounted at `/sys/fs/cgroup`
    /// - `/proc/self/cgroup` missing or unparseable
    /// - parent's `cgroup.subtree_control` write rejected (no delegation)
    /// - `mkdir` rejected (no write permission on parent)
    /// - any of the `memory.max` / `cpu.max` / `pids.max` writes rejected
    pub fn try_create(limits: CgroupV2Limits) -> Option<Self> {
        use std::fs;
        use std::sync::Once;

        static WARN_NO_HIERARCHY: Once = Once::new();
        static WARN_PROC_SELF_CGROUP: Once = Once::new();
        static WARN_SUBTREE_CONTROL: Once = Once::new();
        static WARN_MKDIR: Once = Once::new();

        // Step 1: confirm v2 unified hierarchy is mounted.
        if !std::path::Path::new(UNIFIED_HIERARCHY)
            .join("cgroup.controllers")
            .exists()
        {
            WARN_NO_HIERARCHY.call_once(|| {
                tracing::warn!(
                    "cgroup v2 unified hierarchy not found at {UNIFIED_HIERARCHY}; \
                     SP-5 cgroup containment skipped (RLIMIT_AS still applies)"
                );
            });
            return None;
        }

        // Step 2: discover self cgroup.
        let proc_self = match fs::read_to_string("/proc/self/cgroup") {
            Ok(s) => s,
            Err(e) => {
                WARN_PROC_SELF_CGROUP.call_once(|| {
                    tracing::warn!("read /proc/self/cgroup failed: {e}; SP-5 skipped");
                });
                return None;
            }
        };
        let self_rel = parse_proc_self_cgroup_path(&proc_self)?;
        // self_rel is "/user.slice/.../foo.scope"; strip the leading "/"
        // so the join produces "/sys/fs/cgroup/user.slice/..." not
        // "/user.slice/..." (which would escape UNIFIED_HIERARCHY).
        let parent = std::path::Path::new(UNIFIED_HIERARCHY)
            .join(self_rel.strip_prefix("/").unwrap_or(&self_rel));

        // Step 3: enable controllers in parent. EBUSY = already enabled
        // (benign). EPERM = not delegated (hard fail for cgroup setup;
        // soft-degrade to caller).
        let subtree = parent.join("cgroup.subtree_control");
        if let Err(e) = fs::write(&subtree, b"+memory +cpu +pids") {
            use std::io::ErrorKind;
            if e.kind() != ErrorKind::AlreadyExists && e.raw_os_error() != Some(libc::EBUSY) {
                WARN_SUBTREE_CONTROL.call_once(|| {
                    tracing::warn!("cgroup subtree_control write rejected ({e}); SP-5 skipped");
                });
                return None;
            }
        }

        // Step 4: create child cgroup directory.
        let pid = std::process::id();
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let child = parent.join(format!("aleph-sandbox-{pid}-{nonce}"));
        if let Err(e) = fs::create_dir(&child) {
            WARN_MKDIR.call_once(|| {
                tracing::warn!("mkdir {child:?} failed ({e}); SP-5 skipped");
            });
            return None;
        }

        // Step 5: write limit files. Failures here are unusual but we
        // still soft-degrade (and clean up the partial cgroup) rather
        // than half-apply.
        let scope = CgroupV2Scope { path: child };
        if let Err(e) = scope.write_limits(&limits) {
            tracing::warn!("cgroup limit write failed ({e}); cleaning up partial sub-cgroup");
            // Drop will run rmdir on scope.path.
            return None;
        }

        // Always set memory.swap.max=0 so memory pressure can't escape
        // via swap. Best-effort: if swap controller is unavailable
        // (some hosts disable it), continue silently.
        let _ = std::fs::write(scope.path.join("memory.swap.max"), b"0");

        Some(scope)
    }

    fn write_limits(&self, limits: &CgroupV2Limits) -> std::io::Result<()> {
        use std::fs;
        if let Some(mb) = limits.memory_mb {
            fs::write(
                self.path.join("memory.max"),
                memory_max_bytes(mb).to_string().as_bytes(),
            )?;
        }
        if let Some(pct) = limits.cpu_quota_percent {
            fs::write(
                self.path.join("cpu.max"),
                cpu_quota_max_line(pct).as_bytes(),
            )?;
        }
        if let Some(n) = limits.max_pids {
            fs::write(self.path.join("pids.max"), n.to_string().as_bytes())?;
        }
        Ok(())
    }

    /// Async-signal-safe helper: write the current PID into `path`.
    pub(crate) fn write_current_pid_to_path(path: &std::path::Path) -> std::io::Result<()> {
        use std::os::unix::ffi::OsStrExt;
        let path_bytes = path.as_os_str().as_bytes();
        let mut path_buf = [0u8; 512];
        if path_bytes.len() >= path_buf.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "cgroup.procs path too long",
            ));
        }
        path_buf[..path_bytes.len()].copy_from_slice(path_bytes);
        path_buf[path_bytes.len()] = b'\0';

        // SAFETY: `path_buf` is a NUL-terminated byte array with valid open flags.
        // rust-doctor-disable-next-line unsafe-block-audit
        let fd = unsafe {
            libc::open(
                path_buf.as_ptr() as *const i8,
                libc::O_WRONLY | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let pid = std::process::id();
        let mut pid_buf = [0u8; 16];
        // The kernel expects "<pid>\n"; the newline must trail the digits.
        // Reserve index 15 for it and write decimal digits LSB-first from 14.
        pid_buf[15] = b'\n';
        let mut n = pid;
        let mut i = 0;
        loop {
            pid_buf[14 - i] = b'0' + (n % 10) as u8;
            n /= 10;
            i += 1;
            if n == 0 {
                break;
            }
        }
        let pid_slice = &pid_buf[14 - (i - 1)..16];
        let mut written = 0;
        while written < pid_slice.len() {
            // SAFETY: `fd` is valid and `pid_slice` is a contiguous byte buffer.
            // rust-doctor-disable-next-line unsafe-block-audit
            let ret = unsafe {
                libc::write(
                    fd,
                    pid_slice[written..].as_ptr() as *const libc::c_void,
                    pid_slice.len() - written,
                )
            };
            if ret < 0 {
                // SAFETY: `fd` is a valid open file descriptor.
                // rust-doctor-disable-next-line unsafe-block-audit
                let _ = unsafe { libc::close(fd) };
                return Err(std::io::Error::last_os_error());
            }
            written += ret as usize;
        }
        // SAFETY: `fd` is a valid open file descriptor.
        // rust-doctor-disable-next-line unsafe-block-audit
        let _ = unsafe { libc::close(fd) };
        Ok(())
    }
}

#[cfg(target_os = "linux")]
impl Drop for CgroupV2Scope {
    fn drop(&mut self) {
        // Best-effort rmdir. ENOTEMPTY means a child is still alive —
        // shouldn't happen if the caller waited, but log so it's
        // discoverable. ENOENT means the cgroup was already removed
        // (likely by an admin); silent.
        if let Err(e) = std::fs::remove_dir(&self.path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(
                    "cgroup cleanup: rmdir {:?} failed: {e} \
                     (leaked cgroup directory — kernel will reap empty cgroups eventually)",
                    self.path
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Cross-platform stubs so callers compile on macOS / Windows. On those
// platforms `try_create` always returns `None` (no cgroup mechanism
// exists), making the cgroup path a no-op without forcing every caller
// to add `#[cfg(target_os = "linux")]`.
// ---------------------------------------------------------------------------

#[cfg(not(target_os = "linux"))]
impl CgroupV2Scope {
    pub const fn try_create(_limits: CgroupV2Limits) -> Option<Self> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- parse_proc_self_cgroup_path ---------------------------------

    #[test]
    fn parse_proc_self_cgroup_v2_canonical() {
        let input = "0::/user.slice/user-1000.slice/user@1000.service/app.scope\n";
        let got = parse_proc_self_cgroup_path(input);
        assert_eq!(
            got,
            Some(PathBuf::from(
                "/user.slice/user-1000.slice/user@1000.service/app.scope"
            ))
        );
    }

    #[test]
    fn parse_proc_self_cgroup_root() {
        let input = "0::/\n";
        assert_eq!(parse_proc_self_cgroup_path(input), Some(PathBuf::from("/")));
    }

    #[test]
    fn parse_proc_self_cgroup_v1_only_returns_none() {
        // v1 hierarchies have a non-zero numeric ID and a non-empty
        // controller name in the second column. v2 has `0::<path>`.
        let input = "12:freezer:/\n11:cpuset:/\n";
        assert_eq!(parse_proc_self_cgroup_path(input), None);
    }

    #[test]
    fn parse_proc_self_cgroup_v2_first_among_mixed_lines() {
        // Hybrid systems list v1 controllers above the unified v2 line.
        let input = "12:freezer:/\n0::/user.slice/foo.scope\n";
        let got = parse_proc_self_cgroup_path(input);
        assert_eq!(got, Some(PathBuf::from("/user.slice/foo.scope")));
    }

    #[test]
    fn parse_proc_self_cgroup_empty_input() {
        assert_eq!(parse_proc_self_cgroup_path(""), None);
    }

    #[test]
    fn parse_proc_self_cgroup_rejects_parent_dir_escape() {
        // A `..` component would let the later `/sys/fs/cgroup`-join escape the
        // hierarchy root; such a path must be rejected (soft-degrade to None).
        let input = "0::/user.slice/../../../etc\n";
        assert_eq!(parse_proc_self_cgroup_path(input), None);
    }

    // ---- cpu_quota_max_line -----------------------------------------

    #[test]
    fn cpu_quota_format_renders_50pct() {
        assert_eq!(cpu_quota_max_line(50), "50000 100000");
    }

    #[test]
    fn cpu_quota_format_renders_100pct() {
        assert_eq!(cpu_quota_max_line(100), "100000 100000");
    }

    #[test]
    fn cpu_quota_format_renders_multicore() {
        assert_eq!(cpu_quota_max_line(250), "250000 100000");
    }

    // ---- memory_max_bytes -------------------------------------------

    #[test]
    fn memory_max_bytes_64mb() {
        assert_eq!(memory_max_bytes(64), 64 * 1024 * 1024);
    }

    #[test]
    fn memory_max_bytes_saturates_on_overflow() {
        assert_eq!(memory_max_bytes(u64::MAX), u64::MAX);
    }
}
