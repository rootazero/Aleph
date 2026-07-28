//! Linux `SystemCapability` implementation.
//!
//! Uses standard POSIX / freedesktop CLI tools with graceful fallbacks.
//! Tested on Debian/Ubuntu; should work on most distributions.

use aleph_desktop::script_exec::{
    is_spawn_failure, output_capped_blocking, DESKTOP_QUERY_TIMEOUT,
};
use aleph_desktop::system_types::{AppInfo, ClipboardContent, SystemInfo};
use aleph_desktop::traits::SystemCapability;
use aleph_desktop::{DesktopError, Result};
use async_trait::async_trait;

pub struct LinuxSystem;

impl LinuxSystem {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Default for LinuxSystem {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SystemCapability for LinuxSystem {
    async fn launch_app(&self, app_name: &str) -> Result<()> {
        // One launch implementation for both capability surfaces — see
        // `aleph_desktop::linux::app`. This used to be a second, weaker copy
        // (`gtk-launch <human name>` → `xdg-open <human name>`), which meant the
        // `system` tool and the `desktop` tool could disagree about whether an
        // app could be started at all.
        let app_name = app_name.to_string();
        tokio::task::spawn_blocking(move || aleph_desktop::linux::app::launch(&app_name))
            .await
            .map_err(|e| DesktopError::InputFailed(format!("task join error: {e}")))?
    }

    async fn quit_app(&self, app_name: &str) -> Result<()> {
        // Windows-close first, then SIGTERM on an exact executable-name match.
        // The previous `killall` → `pkill -f` pair matched the whole command
        // line, so quitting one app could take unrelated processes with it.
        let app_name = app_name.to_string();
        tokio::task::spawn_blocking(move || aleph_desktop::linux::app::quit(&app_name))
            .await
            .map_err(|e| DesktopError::InputFailed(format!("task join error: {e}")))?
    }

    async fn list_running_apps(&self) -> Result<Vec<AppInfo>> {
        tokio::task::spawn_blocking(|| {
            // `/proc` instead of `ps -eo comm`: the kernel truncates `comm` to 15
            // bytes, so the old listing reported `gnome-calculator` as
            // `gnome-calculato` — a name nothing could then be launched or quit
            // by. `/proc/<pid>/exe` is the real binary.
            let procs = aleph_desktop::linux::proc::snapshot();

            // Which processes own a window, and which one is in front. A GUI
            // application is exactly one that has a window, so this replaces the
            // old hand-maintained deny-list of daemon name prefixes — and it
            // finally lets `is_active` be true for something.
            let windows = aleph_desktop::action::window_linux::window_list().unwrap_or_default();
            let active_pid = aleph_desktop::action::window_linux::active_window()
                .ok()
                .flatten()
                .and_then(|id| windows.iter().find(|w| w.id == id))
                .map(|w| w.pid);
            let windowed: std::collections::HashSet<u64> = windows.iter().map(|w| w.pid).collect();

            Ok(fold_apps(&procs, active_pid, &windowed))
        })
        .await
        .map_err(|e| DesktopError::PlatformError(format!("task join error: {e}")))?
    }

    async fn send_notification(&self, title: &str, body: &str) -> Result<()> {
        let title = title.to_string();
        let body = body.to_string();
        tokio::task::spawn_blocking(move || -> Result<()> {
            // `--` before the positional arguments. Title and body are model
            // prose (an R5 summary, a tool result), and glib's option parser
            // treats a leading `-` as a flag — so a summary starting with "-3
            // items left" turned into an unknown-option failure, or worse, an
            // option the caller never meant to pass. Every other platform's
            // notification path learned this the same way: macOS had to escape
            // AppleScript literals, Windows had to move the strings into
            // environment variables.
            let mut cmd = std::process::Command::new("notify-send");
            cmd.arg("--app-name=Aleph").arg("--").arg(&title).arg(&body);

            // Capped: notify-send blocks on the notification daemon's D-Bus
            // reply, and a wedged (or absent-but-activatable) daemon leaves it
            // waiting with no timeout of its own.
            let output = output_capped_blocking(cmd, DESKTOP_QUERY_TIMEOUT, "Sending a notification")
                .map_err(|e| {
                    if is_spawn_failure(&e) {
                        DesktopError::PlatformError(format!(
                            "Failed to send notification (install libnotify-bin): {e}"
                        ))
                    } else {
                        DesktopError::PlatformError(e.to_string())
                    }
                })?;

            if output.status.success() {
                Ok(())
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                Err(DesktopError::PlatformError(format!(
                    "notify-send returned non-zero exit code: {}",
                    stderr.trim().lines().last().unwrap_or("no detail")
                )))
            }
        })
        .await
        .map_err(|e| DesktopError::PlatformError(format!("task join error: {e}")))?
    }

    async fn clipboard_read(&self) -> Result<ClipboardContent> {
        // Delegates to the one Linux clipboard implementation, which reads text
        // plus an optional image (base64 PNG) — see
        // `aleph_desktop::linux::clipboard`.
        tokio::task::spawn_blocking(aleph_desktop::linux::clipboard::read_content)
            .await
            .map_err(|e| DesktopError::InputFailed(format!("task join error: {e}")))?
    }

    async fn clipboard_write(&self, text: &str) -> Result<()> {
        let text = text.to_string();
        tokio::task::spawn_blocking(move || aleph_desktop::linux::clipboard::write_text(&text))
            .await
            .map_err(|e| DesktopError::InputFailed(format!("task join error: {e}")))?
    }

    async fn system_info(&self) -> Result<SystemInfo> {
        tokio::task::spawn_blocking(|| {
            let os_version = read_os_version().unwrap_or_else(|| "unknown".to_string());
            let hostname = read_hostname().unwrap_or_else(|| "unknown".to_string());
            let username = std::env::var("USER")
                .or_else(|_| std::env::var("LOGNAME"))
                .unwrap_or_else(|_| "unknown".to_string());

            Ok(SystemInfo {
                os_name: "Linux".to_string(),
                os_version,
                hostname,
                arch: std::env::consts::ARCH.to_string(),
                username,
            })
        })
        .await
        .map_err(|e| DesktopError::PlatformError(format!("task join error: {e}")))?
    }

    async fn user_idle_seconds(&self) -> Result<f64> {
        tokio::task::spawn_blocking(|| {
            // The session type comes from `aleph_desktop::linux::session`, the
            // single source shared with the input, clipboard, window and
            // permission layers. This function used to read `XDG_SESSION_TYPE`
            // itself — the last of the four private copies the convergence was
            // supposed to remove, and the one that disagreed with the others on
            // a Wayland session that does not export it (WAYLAND_DISPLAY only),
            // where it would read as X11 and go straight to xprintidle, which
            // reports 0 forever there.
            let is_wayland = aleph_desktop::linux::session().kind.is_wayland();

            // Wayland: prefer GNOME Mutter D-Bus IdleMonitor, which works for any
            // logged-in user on a modern GNOME-on-Wayland session and does not
            // depend on running an X server. xprintidle reports 0 forever on a
            // pure Wayland compositor.
            if is_wayland {
                if let Some(secs) = query_mutter_idle_seconds() {
                    return Ok(secs);
                }
            }

            // X11 / XWayland: xprintidle is the standard tool. Some distros ship
            // it by default; others require `apt install xprintidle`.
            if let Ok(output) = output_capped_blocking(
                std::process::Command::new("xprintidle"),
                DESKTOP_QUERY_TIMEOUT,
                "Reading the idle time",
            ) {
                if output.status.success() {
                    let ms = String::from_utf8_lossy(&output.stdout)
                        .trim()
                        .parse::<u64>()
                        .unwrap_or(0);
                    return Ok(ms as f64 / 1000.0);
                }
            }

            // Last-chance Wayland fallback even if XDG_SESSION_TYPE was unset.
            if !is_wayland {
                if let Some(secs) = query_mutter_idle_seconds() {
                    return Ok(secs);
                }
            }

            let hint = if is_wayland {
                "Idle detection on this Wayland session needs GNOME's Mutter \
                 IdleMonitor (gdbus / dbus-send + a running GNOME shell)"
            } else {
                "Idle detection requires xprintidle on X11 \
                 (install: sudo apt install xprintidle) \
                 or GNOME Mutter IdleMonitor on Wayland"
            };
            Err(DesktopError::NotImplemented(hint.into()))
        })
        .await
        .map_err(|e| DesktopError::PlatformError(format!("task join error: {e}")))?
    }
}

/// Collapse a `/proc` snapshot into one row per application.
///
/// Pure so the folding rules can be tested without a desktop session.
///
/// # One row per application, but the *application* owns the flags
///
/// A browser is dozens of processes sharing one executable name, so the listing
/// has to deduplicate. The previous fold did that by keeping the **first** pid
/// it happened to see under each name and reading `is_active` off that one pid
/// — and `read_dir("/proc")` returns entries in whatever order the filesystem
/// gives, which is not pid order and not stable. So for any multi-process
/// application the foreground flag was a coin toss.
///
/// That mattered well beyond a cosmetic listing: `DesktopTool::check_blocked_app`
/// finds the frontmost app with `apps.iter().find(|a| a.is_active)`, so the
/// password-manager hard block was effectively dead against exactly the apps it
/// targets (1Password, Bitwarden and Proton Pass are all Electron, i.e. all
/// multi-process). `observe`'s post-state had the same hole. Windows carried the
/// mirror-image version of this bug until 2026-07-28.
///
/// The fix is to fold the group first and derive the flags from the whole group:
/// **any** pid matching the foreground window makes the application active, and
/// the representative pid prefers a process that actually owns a window (a
/// browser's GPU helper is a poor thing to address by pid).
fn fold_apps(
    procs: &[aleph_desktop::linux::proc::ProcInfo],
    active_pid: Option<u64>,
    windowed: &std::collections::HashSet<u64>,
) -> Vec<AppInfo> {
    struct Group {
        name: String,
        pid: u64,
        /// Rank of `pid` under the precedence below. Stored rather than
        /// recomputed because the *reason* the current representative was
        /// chosen is what decides whether a later sibling may replace it: a
        /// merely-windowed process must not displace the foreground one, and
        /// `/proc` hands them to us in arbitrary order, so either can come
        /// first.
        pid_rank: u8,
        is_active: bool,
    }

    /// Foreground beats windowed beats anything else.
    const fn rank(is_active: bool, is_windowed: bool) -> u8 {
        match (is_active, is_windowed) {
            (true, _) => 2,
            (false, true) => 1,
            (false, false) => 0,
        }
    }

    let mut order: Vec<String> = Vec::new();
    let mut groups: std::collections::HashMap<String, Group> = std::collections::HashMap::new();

    for proc in procs {
        if proc.is_kernel_thread() {
            continue;
        }
        let name = proc.best_name();
        if name.is_empty() {
            continue;
        }
        let pid = u64::from(proc.pid);
        let is_windowed = windowed.contains(&pid);
        let is_active = active_pid == Some(pid);
        let pid_rank = rank(is_active, is_windowed);

        match groups.get_mut(name) {
            Some(group) => {
                // Sticky across the group: one active pid makes the app active.
                group.is_active |= is_active;
                // Prefer a pid the caller can actually address. Strictly
                // greater, so an equal-ranked sibling does not churn the
                // answer — between two windowed processes of the same
                // application there is nothing to choose.
                if pid_rank > group.pid_rank {
                    group.pid = pid;
                    group.pid_rank = pid_rank;
                }
            }
            None => {
                order.push(name.to_string());
                groups.insert(
                    name.to_string(),
                    Group {
                        name: name.to_string(),
                        pid,
                        pid_rank,
                        is_active,
                    },
                );
            }
        }
    }

    let mut apps: Vec<AppInfo> = order
        .into_iter()
        .filter_map(|name| groups.remove(&name))
        .map(|g| AppInfo {
            name: g.name.clone(),
            bundle_id: g.name,
            pid: Some(g.pid),
            is_active: g.is_active,
        })
        .collect();

    // Windowed applications first: when a caller asks "what is running", the
    // things the user can see are the answer they meant.
    apps.sort_by_key(|a| {
        let windowed_rank = u8::from(!a.pid.is_some_and(|p| windowed.contains(&p)));
        (windowed_rank, a.name.to_lowercase())
    });
    apps
}

/// Query GNOME Mutter's `IdleMonitor` over the session bus via `gdbus`.
///
/// Returns the idle time in seconds, or `None` if `gdbus` is unavailable,
/// the `IdleMonitor` object is not present (no GNOME shell), or the reply
/// cannot be parsed. The reply format is `(uint64 N,)` where N is millis.
///
/// Shells out instead of taking a `zbus` dependency: idle detection is a
/// rare query and `gdbus` is part of every GNOME install (it lives in
/// `glib2-tools` / `libglib2.0-bin`).
fn query_mutter_idle_seconds() -> Option<f64> {
    let mut cmd = std::process::Command::new("gdbus");
    cmd.args([
        "call",
        "--session",
        "--dest",
        "org.gnome.Mutter.IdleMonitor",
        "--object-path",
        "/org/gnome/Mutter/IdleMonitor/Core",
        "--method",
        "org.gnome.Mutter.IdleMonitor.GetIdletime",
    ]);
    // Capped: `gdbus call` waits on a D-Bus reply, and this one is issued on a
    // desktop that may not have a Mutter to answer it.
    let output = output_capped_blocking(cmd, DESKTOP_QUERY_TIMEOUT, "Reading the idle time").ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_mutter_idle_reply(&stdout)
}

/// Parse `gdbus`'s textual reply `(uint64 12345,)` into seconds.
fn parse_mutter_idle_reply(reply: &str) -> Option<f64> {
    let trimmed = reply.trim();
    let inner = trimmed.strip_prefix('(')?.strip_suffix(",)")?;
    let inner = inner.trim();
    // Accept the typed form ("uint64 12345") and the bare numeric form ("12345").
    let digits = inner.strip_prefix("uint64").unwrap_or(inner).trim();
    let ms: u64 = digits.parse().ok()?;
    Some(ms as f64 / 1000.0)
}

fn read_os_version() -> Option<String> {
    if let Ok(content) = std::fs::read_to_string("/etc/os-release") {
        for line in content.lines() {
            if let Some(value) = line.strip_prefix("PRETTY_NAME=") {
                return Some(value.trim_matches('"').to_string());
            }
        }
    }

    std::process::Command::new("uname")
        .arg("-r")
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        })
}

fn read_hostname() -> Option<String> {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .or_else(|| {
            std::process::Command::new("uname")
                .arg("-n")
                .output()
                .ok()
                .and_then(|o| {
                    if o.status.success() {
                        Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
                    } else {
                        None
                    }
                })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_default() {
        let _ = LinuxSystem;
    }

    #[test]
    fn test_system_info() {
        let sys = LinuxSystem::new();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let info = rt.block_on(sys.system_info()).unwrap();
        assert_eq!(info.os_name, "Linux");
        assert!(!info.hostname.is_empty());
        assert!(!info.arch.is_empty());
    }

    // ── fold_apps: one row per application, flags owned by the group ─────────

    fn proc(pid: u32, exe: &str) -> aleph_desktop::linux::proc::ProcInfo {
        aleph_desktop::linux::proc::ProcInfo {
            pid,
            comm: exe.to_string(),
            exe_name: Some(exe.to_string()),
        }
    }

    fn pids(list: &[u64]) -> std::collections::HashSet<u64> {
        list.iter().copied().collect()
    }

    #[test]
    fn a_multi_process_app_is_active_when_any_of_its_pids_is_frontmost() {
        // The regression this guards: `chrome` is dozens of processes, and
        // `/proc` enumeration order is arbitrary — so a fold that reads
        // `is_active` off whichever pid it saw first reports the foreground
        // browser as background most of the time. `check_blocked_app` and
        // `observe`'s post-state both hinge on this flag.
        let procs = [proc(10, "chrome"), proc(11, "chrome"), proc(12, "chrome")];
        let apps = fold_apps(&procs, Some(11), &pids(&[11]));
        assert_eq!(apps.len(), 1, "one row per application");
        assert!(apps[0].is_active, "the group owns the foreground flag");
    }

    #[test]
    fn the_representative_pid_is_one_that_owns_a_window() {
        // Addressing a browser by its GPU helper's pid is useless to every
        // caller that then targets input at it.
        let procs = [proc(10, "chrome"), proc(11, "chrome")];
        let apps = fold_apps(&procs, None, &pids(&[11]));
        assert_eq!(apps[0].pid, Some(11));
    }

    #[test]
    fn the_foreground_pid_outranks_a_merely_windowed_sibling_in_either_order() {
        // `/proc` enumeration order is arbitrary, so the precedence has to hold
        // whichever sibling is folded first. Reading it the other way round is
        // the bug this pins: picking pid 10 because it is frontmost and then
        // letting pid 11 displace it purely for owning a window.
        let forward = [proc(10, "chrome"), proc(11, "chrome")];
        let reverse = [proc(11, "chrome"), proc(10, "chrome")];
        for procs in [&forward, &reverse] {
            let apps = fold_apps(procs, Some(10), &pids(&[11]));
            assert_eq!(apps[0].pid, Some(10), "foreground pid must win");
            assert!(apps[0].is_active);
        }
    }

    #[test]
    fn nothing_frontmost_leaves_every_app_inactive() {
        let procs = [proc(10, "chrome"), proc(20, "firefox")];
        let apps = fold_apps(&procs, None, &pids(&[]));
        assert_eq!(apps.len(), 2);
        assert!(apps.iter().all(|a| !a.is_active));
    }

    #[test]
    fn windowed_apps_sort_ahead_of_background_ones() {
        let procs = [proc(10, "zsh"), proc(20, "firefox")];
        let apps = fold_apps(&procs, None, &pids(&[20]));
        assert_eq!(apps[0].name, "firefox", "what the user can see comes first");
    }

    #[test]
    fn kernel_threads_and_nameless_processes_are_not_applications() {
        let kthread = aleph_desktop::linux::proc::ProcInfo {
            pid: 2,
            comm: "[kthreadd]".to_string(),
            exe_name: None,
        };
        let apps = fold_apps(&[kthread, proc(10, "bash")], None, &pids(&[]));
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].name, "bash");
    }

    #[test]
    fn parses_mutter_idle_typed_reply() {
        // gdbus typically prints the type tag.
        let out = parse_mutter_idle_reply("(uint64 12345,)\n").unwrap();
        assert!((out - 12.345).abs() < 1e-6);
    }

    #[test]
    fn parses_mutter_idle_bare_numeric_reply() {
        // Some glib versions omit the tag.
        let out = parse_mutter_idle_reply("(2500,)").unwrap();
        assert!((out - 2.5).abs() < 1e-6);
    }

    #[test]
    fn parses_mutter_idle_zero() {
        let out = parse_mutter_idle_reply("(uint64 0,)").unwrap();
        assert_eq!(out, 0.0);
    }

    #[test]
    fn rejects_malformed_mutter_idle_reply() {
        assert!(parse_mutter_idle_reply("garbage").is_none());
        assert!(parse_mutter_idle_reply("(uint64 not_a_number,)").is_none());
        assert!(parse_mutter_idle_reply("()").is_none());
    }
}
