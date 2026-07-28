//! Process enumeration straight off `/proc`.
//!
//! Two Linux limbs needed "which processes are running, and which one is
//! *this* application", and both used to answer it by shelling out:
//!
//! * `list_running_apps` ran `ps -eo comm`, whose `comm` field the kernel
//!   truncates to 15 characters — so `gnome-calculator` was reported as
//!   `gnome-calculato` and could never be matched again.
//! * `quit_app` ran `pkill -f <name>`, which matches the *whole command line*.
//!   Quitting "chrome" would also kill anything whose arguments merely mention
//!   chrome — including, on a bad day, the agent that issued the request.
//!
//! Reading `/proc` directly fixes both: `/proc/<pid>/exe` is a symlink to the
//! real binary (no truncation), and matching is exact by construction, so
//! nothing has to be pattern-matched against a command line at all.

use std::path::Path;

/// One running process, as much as `/proc` will tell us without privileges.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcInfo {
    pub pid: u32,
    /// `/proc/<pid>/comm` — the kernel's name for the task, truncated to 15
    /// bytes. Always present.
    pub comm: String,
    /// File name of `/proc/<pid>/exe`. `None` for kernel threads and for
    /// processes owned by another user (the symlink is unreadable).
    pub exe_name: Option<String>,
}

/// The kernel's hard limit on `comm` (`TASK_COMM_LEN - 1`).
pub const COMM_MAX: usize = 15;

impl ProcInfo {
    /// The best available name: the executable's file name when readable,
    /// otherwise the (possibly truncated) `comm`.
    #[must_use]
    pub fn best_name(&self) -> &str {
        self.exe_name.as_deref().unwrap_or(&self.comm)
    }

    /// `true` for a kernel thread — no executable, and `comm` conventionally
    /// bracketed in `ps` output. These are never "applications".
    #[must_use]
    pub fn is_kernel_thread(&self) -> bool {
        self.exe_name.is_none() && self.comm.starts_with('[')
    }
}

/// Does this process answer to `name`?
///
/// Matches, case-insensitively, against the executable file name and against
/// `comm`. The third arm is the truncation rule: a caller asking for
/// `gnome-calculator` must still match a process whose `comm` the kernel cut to
/// `gnome-calculato`, which is exactly the case `ps`-based matching could never
/// express without falling back to a substring search over the command line.
///
/// Matching is *whole-name only* — never a substring, never the argument
/// vector. That is the property that makes `quit` safe.
#[must_use]
pub fn matches_name(proc: &ProcInfo, name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    if proc
        .exe_name
        .as_deref()
        .is_some_and(|exe| exe.eq_ignore_ascii_case(name))
    {
        return true;
    }
    if proc.comm.eq_ignore_ascii_case(name) {
        return true;
    }
    // comm truncation: compare the request against its own 15-byte prefix, but
    // only when the request is actually longer, so short names stay exact.
    name.len() > COMM_MAX && {
        let truncated = name.get(..COMM_MAX).unwrap_or(name);
        proc.comm.eq_ignore_ascii_case(truncated)
    }
}

/// Every process visible in `/proc`.
///
/// Unreadable entries are skipped rather than failing the whole scan: a process
/// may exit between the directory read and the file read, and another user's
/// process is simply not ours to see.
#[must_use]
pub fn snapshot() -> Vec<ProcInfo> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|n| n.parse::<u32>().ok()) else {
            continue;
        };
        let dir = entry.path();
        let Some(comm) = read_comm(&dir) else {
            continue;
        };
        out.push(ProcInfo {
            pid,
            comm,
            exe_name: read_exe_name(&dir),
        });
    }
    out
}

fn read_comm(dir: &Path) -> Option<String> {
    std::fs::read_to_string(dir.join("comm"))
        .ok()
        .map(|s| s.trim_end_matches('\n').to_string())
        .filter(|s| !s.is_empty())
}

fn read_exe_name(dir: &Path) -> Option<String> {
    std::fs::read_link(dir.join("exe"))
        .ok()?
        .file_name()?
        .to_str()
        .map(|s| {
            // A replaced binary shows up as "/usr/bin/foo (deleted)".
            s.strip_suffix(" (deleted)").unwrap_or(s).to_string()
        })
}

/// Every pid whose process answers to `name`.
#[must_use]
pub fn pids_named(name: &str) -> Vec<u32> {
    snapshot()
        .into_iter()
        .filter(|p| matches_name(p, name))
        .map(|p| p.pid)
        .collect()
}

/// Read one process's name without a full scan — used to check whether a
/// window's owning pid belongs to the application being addressed.
#[must_use]
pub fn info_for(pid: u32) -> Option<ProcInfo> {
    let dir = Path::new("/proc").join(pid.to_string());
    Some(ProcInfo {
        pid,
        comm: read_comm(&dir)?,
        exe_name: read_exe_name(&dir),
    })
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn proc(comm: &str, exe: Option<&str>) -> ProcInfo {
        ProcInfo {
            pid: 1234,
            comm: comm.to_string(),
            exe_name: exe.map(ToString::to_string),
        }
    }

    #[test]
    fn matches_the_executable_name() {
        let p = proc("firefox", Some("firefox"));
        assert!(matches_name(&p, "firefox"));
        assert!(matches_name(&p, "Firefox"), "case-insensitive");
    }

    #[test]
    fn matches_a_truncated_comm_when_the_real_name_is_longer() {
        // The kernel cut `gnome-calculator` (16) to `gnome-calculato` (15), and
        // the exe link is unreadable (another user's process).
        let p = proc("gnome-calculato", None);
        assert!(matches_name(&p, "gnome-calculator"));
    }

    #[test]
    fn never_matches_on_a_substring() {
        // This is the whole reason `pkill -f` was removed.
        let p = proc("chromium", Some("chromium"));
        assert!(!matches_name(&p, "chrome"));
        assert!(!matches_name(&p, "chromium-browser"));

        let agent = proc("aleph-server", Some("aleph-server"));
        assert!(!matches_name(&agent, "aleph"));
    }

    #[test]
    fn short_names_stay_exact_despite_the_truncation_rule() {
        // A 15-or-shorter request must not silently prefix-match.
        let p = proc("bash", Some("bash"));
        assert!(!matches_name(&p, "ba"));
        assert!(matches_name(&p, "bash"));
    }

    #[test]
    fn an_empty_query_matches_nothing() {
        assert!(!matches_name(&proc("bash", Some("bash")), ""));
    }

    #[test]
    fn exe_name_wins_over_comm_for_display() {
        // A process that renamed its own comm (many Electron apps do) is still
        // reported under the binary it actually runs.
        let p = proc("Chrome_ChildIOT", Some("chrome"));
        assert_eq!(p.best_name(), "chrome");
        assert_eq!(proc("kworker/0:1", None).best_name(), "kworker/0:1");
    }

    #[test]
    fn kernel_threads_are_recognisable() {
        assert!(proc("[kthreadd]", None).is_kernel_thread());
        assert!(!proc("kthreadd", None).is_kernel_thread());
        assert!(!proc("bash", Some("bash")).is_kernel_thread());
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn snapshot_sees_this_very_process() {
        let me = std::process::id();
        let snap = snapshot();
        assert!(!snap.is_empty(), "/proc must be readable");
        let mine = snap.iter().find(|p| p.pid == me).expect("own pid missing");
        assert!(!mine.comm.is_empty());
        assert_eq!(info_for(me).map(|p| p.comm), Some(mine.comm.clone()));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn info_for_a_dead_pid_is_none_not_a_panic() {
        // pid 0 never exists as a /proc entry.
        assert!(info_for(0).is_none());
    }
}
