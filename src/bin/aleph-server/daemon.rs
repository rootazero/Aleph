//! Daemon process management for Aleph Gateway
//!
//! This module handles PID file management, process lifecycle,
//! instance locking, and Unix daemonization.

#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;
#[cfg(unix)]
use std::time::SystemTime;

use alephcore::cli::endpoint::{read_endpoint, remove_endpoint, IpcEndpoint};

/// Size cap on the daemon's stdout/stderr redirect file (`gateway.log`) that
/// forces a rotation regardless of age. The redirect fd cannot be rotated while
/// the daemon holds it open (there is no in-flight rotation), so this only bites
/// at the next daemon start; day-boundary rotation (below) is what keeps a
/// low-traffic server's log from accumulating across restarts.
#[cfg(unix)]
const MAX_GATEWAY_LOG_BYTES: u64 = 5 * 1024 * 1024;

/// Decide whether the daemon's redirect log should be rotated out before the
/// next append, and under which `YYYY-MM-DD` archive suffix.
///
/// Returns `None` when the live file should keep being appended (empty, or
/// last written today and still within the size budget). Returns `Some(date)`
/// — the file's own last-write day — when it should be archived, because either:
/// - it was last written on an *earlier* calendar day (so even a low-traffic
///   server that never reaches `max_bytes` still gets one file per day instead
///   of one file that accumulates every boot forever), or
/// - it exceeds `max_bytes`.
///
/// Suffixing with the file's last-write day (not "today") makes the archive
/// name reflect the window it actually covers, and keeps it a 10-char
/// `YYYY-MM-DD` string that `cleanup_old_logs` can later age out. Pure so the
/// rotation decision is unit-testable without touching the filesystem.
#[cfg(unix)]
fn rotation_suffix_date(
    len: u64,
    modified: SystemTime,
    now: SystemTime,
    max_bytes: u64,
) -> Option<chrono::NaiveDate> {
    if len == 0 {
        return None;
    }
    let modified_date = chrono::DateTime::<chrono::Utc>::from(modified).date_naive();
    let today = chrono::DateTime::<chrono::Utc>::from(now).date_naive();
    if modified_date < today || len > max_bytes {
        Some(modified_date)
    } else {
        None
    }
}

/// Rotate `log_path` out of the way when it is stale (last written on an earlier
/// day) or oversized, keeping the daemon's redirect target from accumulating
/// across restarts. The file is renamed to `<file_name>.YYYY-MM-DD` (its own
/// last-write day) — a name `cleanup_old_logs` can later age out — and the next
/// append starts a fresh file. No-op when the file is absent, empty, or a
/// same-day file within budget (same-day restarts keep appending, disambiguated
/// by the per-boot `ALEPH-BOOT` marker). Same-day re-rotation overwrites that
/// day's backup (atomic rename replace), which is acceptable for a catch-all
/// stream whose full history also lives in the timestamped `~/.aleph/logs/`
/// stream.
#[cfg(unix)]
fn rotate_stale_or_oversized_log(log_path: &Path, max_bytes: u64) -> std::io::Result<()> {
    let metadata = match std::fs::metadata(log_path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    // A file whose mtime is unreadable is treated as "today" (only size can
    // then trigger rotation) rather than blocking startup.
    let modified = metadata.modified().unwrap_or_else(|e| {
        tracing::warn!(?e, log_path=%log_path.display(), "unable to read log file mtime; treating as today for rotation");
        SystemTime::now()
    });
    let Some(date) = rotation_suffix_date(metadata.len(), modified, SystemTime::now(), max_bytes)
    else {
        return Ok(());
    };
    let file_name = log_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("gateway.log");
    let suffix = date.format("%Y-%m-%d");
    let rotated = log_path.with_file_name(format!("{file_name}.{suffix}"));
    std::fs::rename(log_path, rotated)
}

/// Expand ~ to home directory.
/// When `dirs::home_dir()` returns `None`, falls back to a per-user scratch
/// dir (`/tmp/.aleph-$uid` on Unix, `%TEMP%\.aleph` on Windows) to avoid
/// collisions between different users on shared systems.
pub fn expand_path(path: &str) -> PathBuf {
    if let Some(stripped) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(stripped);
        }
        // `/tmp` is shared on Unix, so the uid disambiguates users;
        // Windows has no `/tmp` and `temp_dir()` is already per-user.
        #[cfg(unix)]
        {
            // SAFETY: getuid() always succeeds and returns the real user ID
            // of the calling process. It is async-signal-safe.
            let uid = unsafe { libc::getuid() };
            eprintln!(
                "Warning: cannot determine home directory; using /tmp/.aleph-{uid} as fallback"
            );
            return PathBuf::from(format!("/tmp/.aleph-{uid}")).join(stripped);
        }
        #[cfg(not(unix))]
        {
            let fallback = std::env::temp_dir().join(".aleph");
            eprintln!(
                "Warning: cannot determine home directory; using {} as fallback",
                fallback.display()
            );
            return fallback.join(stripped);
        }
    }
    PathBuf::from(path)
}

/// Check if a process with given PID is running.
///
/// Delegates to the shared cross-platform liveness helper so the daemon
/// lifecycle (`stop` / `status`) and the singleton instance lock agree on
/// what "alive" means. A non-positive PID is rejected by the helper, so a
/// corrupted PID file can never escalate into a broadcast signal on Unix.
pub fn is_process_running(pid: i32) -> bool {
    alephcore::utils::process_alive::is_process_alive(pid)
}

/// Read PID from file
pub fn read_pid_file(pid_file: &str) -> Option<i32> {
    let path = expand_path(pid_file);
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| s.trim().parse::<i32>().ok())
        // Reject 0 / negatives at the boundary so every downstream consumer
        // (handle_stop, handle_status, daemonize) is protected: a corrupted PID
        // file must never become a `kill(0/-1, SIG…)` broadcast.
        .filter(|&pid| pid > 0)
}

/// Write PID to file
#[cfg(unix)]
pub fn write_pid_file(pid_file: &str) -> std::io::Result<()> {
    let path = expand_path(pid_file);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, format!("{}", std::process::id()))
}

/// Remove PID file
pub fn remove_pid_file(pid_file: &str) {
    let path = expand_path(pid_file);
    if let Err(e) = std::fs::remove_file(&path) {
        tracing::debug!("Failed to remove PID file {}: {}", path.display(), e);
    }
}

/// Handle stop command.
///
/// Resolves the live PID from the IPC endpoint file (`.ipc-endpoint.json`,
/// written on every start path) first, falling back to the Unix-only PID file.
/// This makes `stop` work on Windows and for foreground servers, where no PID
/// file is ever written — previously `stop` was a silent no-op there, so the
/// daemon kept running after "Quit & Stop Aleph".
pub fn handle_stop(pid_file: &str) -> Result<(), Box<dyn std::error::Error>> {
    let Some(pid) = read_pid_file(pid_file)
        .or_else(|| read_endpoint_best_effort().and_then(|ep| i32::try_from(ep.pid).ok()))
    else {
        println!("No gateway daemon is running (no PID file or endpoint)");
        return Ok(());
    };

    if !is_process_running(pid) {
        println!("Gateway is not running (stale record for PID {pid})");
        remove_pid_file(pid_file);
        cleanup_endpoint_file();
        return Ok(());
    }

    stop_running_process(pid)?;
    println!("Gateway stopped successfully");
    remove_pid_file(pid_file);
    cleanup_endpoint_file();
    Ok(())
}

/// Terminate a running gateway process, escalating from a graceful request to a
/// forced kill. Unix: SIGTERM (≤5s) → SIGKILL (≤2s).
#[cfg(unix)]
fn stop_running_process(pid: i32) -> Result<(), Box<dyn std::error::Error>> {
    println!("Sending SIGTERM to gateway process (PID {pid})");
    // SAFETY: kill() with SIGTERM is the standard graceful-termination request
    // on Unix. PID was validated by is_process_running() in the caller.
    if unsafe { libc::kill(pid, libc::SIGTERM) } != 0 {
        eprintln!(
            "Warning: failed to send SIGTERM to PID {pid}: {}",
            std::io::Error::last_os_error()
        );
    }
    if wait_for_exit(pid, 50) {
        return Ok(());
    }

    println!("Gateway did not stop gracefully, sending SIGKILL");
    // SAFETY: kill() with SIGKILL forcefully terminates the process. PID was
    // validated by is_process_running() in the caller.
    if unsafe { libc::kill(pid, libc::SIGKILL) } != 0 {
        return Err(format!(
            "Failed to send SIGKILL to PID {pid}: {}",
            std::io::Error::last_os_error()
        )
        .into());
    }
    if wait_for_exit(pid, 20) {
        return Ok(());
    }
    Err(format!("Gateway process (PID {pid}) did not exit even after SIGKILL").into())
}

/// Terminate a running gateway process on Windows. The bundled daemon is spawned
/// detached with no console (`DETACHED_PROCESS | CREATE_NO_WINDOW`), so it cannot
/// receive a console Ctrl event for graceful shutdown — `taskkill /F /T` is the
/// reliable path, and `/T` also reaps the children the daemon would otherwise
/// orphan (it can't clean them up under a forced kill). The OS releases the
/// singleton flock on process death and SQLite (WAL) is crash-safe, so a forced
/// kill is safe (see PROCESS_MANAGEMENT.md).
#[cfg(windows)]
fn stop_running_process(pid: i32) -> Result<(), Box<dyn std::error::Error>> {
    use std::os::windows::process::CommandExt;
    // Never flash a console window from the windowless daemon / tray context.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    println!("Stopping gateway process (PID {pid})");
    let out = std::process::Command::new("taskkill")
        .args(["/F", "/T", "/PID", &pid.to_string()])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("failed to run taskkill for PID {pid}: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "taskkill failed for PID {pid}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )
        .into());
    }
    if wait_for_exit(pid, 20) {
        return Ok(());
    }
    Err(format!("Gateway process (PID {pid}) did not exit after taskkill /F").into())
}

/// Poll until `pid` is no longer alive, up to `max_attempts` × 100ms. Returns
/// true if it exited within the window.
fn wait_for_exit(pid: i32, max_attempts: u32) -> bool {
    for _ in 0..max_attempts {
        if !is_process_running(pid) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    false
}

/// Consolidated daemon liveness, assembled from the PID file AND the IPC
/// endpoint discovery file (`.ipc-endpoint.json`).
///
/// The PID file is written only by `daemonize()` (the Unix daemon path), so it
/// is blind to foreground (`aleph start` without `-d`) and Windows servers.
/// The endpoint file is written on EVERY start path and carries the live PID,
/// URL and start time — consulting it makes `status` correct everywhere and
/// lets it surface where to connect and how long the server has been up.
#[derive(Debug, serde::Serialize, PartialEq, Eq)]
pub struct StatusReport {
    pub running: bool,
    pub pid: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uptime_seconds: Option<u64>,
    pub version: &'static str,
}

/// Merge the PID-file and endpoint-file signals into a single report.
///
/// Pure: process liveness and the wall clock are injected so the resolution
/// logic is unit-testable without spawning real processes. A live endpoint
/// wins over the PID file (it is the authoritative, always-written source);
/// URL/uptime are surfaced only when the endpoint names a LIVE process, so a
/// stale `.ipc-endpoint.json` (server crashed without cleanup) never advertises
/// a dead URL.
fn resolve_status(
    pid_file_pid: Option<i32>,
    endpoint: Option<&IpcEndpoint>,
    now: chrono::DateTime<chrono::Utc>,
    is_alive: impl Fn(i32) -> bool,
) -> StatusReport {
    let endpoint_pid = endpoint.and_then(|e| i32::try_from(e.pid).ok());
    let endpoint_alive = endpoint_pid.is_some_and(&is_alive);
    let pidfile_alive = pid_file_pid.is_some_and(&is_alive);
    let running = endpoint_alive || pidfile_alive;

    // Prefer a live PID for reporting; fall back to whichever stale record
    // exists so the "not running (stale record for PID N)" message can name it.
    let pid = if endpoint_alive {
        endpoint_pid
    } else if pidfile_alive {
        pid_file_pid
    } else {
        endpoint_pid.or(pid_file_pid)
    };

    let (url, started_at, uptime_seconds) = match endpoint.filter(|_| endpoint_alive) {
        Some(ep) => {
            let uptime = chrono::DateTime::parse_from_rfc3339(&ep.started_at)
                .ok()
                .map(|s| (now - s.with_timezone(&chrono::Utc)).num_seconds())
                .filter(|secs| *secs >= 0)
                .map(|secs| secs as u64);
            (Some(ep.url.clone()), Some(ep.started_at.clone()), uptime)
        }
        None => (None, None, None),
    };

    StatusReport {
        running,
        pid,
        url,
        started_at,
        uptime_seconds,
        version: env!("ALEPH_VERSION"),
    }
}

/// Best-effort read of the endpoint discovery file. Returns `None` when the
/// data dir is unresolvable or the file is absent/corrupt — status then
/// degrades gracefully to PID-file-only behavior.
fn read_endpoint_best_effort() -> Option<IpcEndpoint> {
    let dir = alephcore::utils::paths::get_data_dir().ok()?;
    read_endpoint(&dir).ok().flatten()
}

fn print_status_human(report: &StatusReport) {
    match (report.running, report.pid) {
        (true, Some(p)) => {
            println!("Gateway is running (PID {p})");
            if let Some(url) = &report.url {
                println!("  URL:     {url}");
            }
            if let Some(secs) = report.uptime_seconds {
                // Reuse the loop/cadence duration vocabulary so status uptime
                // and loop status render identically (e.g. "1h2m3s").
                let human = alephcore::looping::types::fmt_duration_ms(secs.saturating_mul(1_000));
                println!("  Uptime:  {human}");
            }
            println!("  Version: {}", report.version);
        }
        (_, Some(p)) => println!("Gateway is not running (stale record for PID {p})"),
        (_, None) => println!("Gateway is not running (no PID file or endpoint)"),
    }
}

/// Handle status command
pub fn handle_status(pid_file: &str, json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let pid_file_pid = read_pid_file(pid_file);
    let endpoint = read_endpoint_best_effort();
    let report = resolve_status(
        pid_file_pid,
        endpoint.as_ref(),
        chrono::Utc::now(),
        is_process_running,
    );

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_status_human(&report);
    }
    Ok(())
}

/// Best-effort removal of the IPC endpoint discovery file from the stop path.
/// A foreground / Windows server writes `.ipc-endpoint.json` but no PID file;
/// without this, `stop` would leave a stale endpoint behind (liveness probes
/// still guard `status`, but the lingering file is misleading).
fn cleanup_endpoint_file() {
    if let Ok(dir) = alephcore::utils::paths::get_data_dir() {
        if let Err(e) = remove_endpoint(&dir) {
            tracing::debug!("failed to remove endpoint file during stop: {e}");
        }
    }
}

/// Daemonize the current process (Unix only)
#[cfg(unix)]
pub fn daemonize(
    pid_file: &str,
    log_file: Option<&PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::fs::OpenOptions;

    // Check if already running
    if let Some(pid) = read_pid_file(pid_file) {
        if is_process_running(pid) {
            return Err(format!("Gateway already running (PID {pid})").into());
        }
    }

    // SAFETY: fork() is the standard Unix primitive for process creation.
    // We are single-threaded here (called before tokio runtime starts).
    match unsafe { libc::fork() } {
        -1 => return Err("Fork failed".into()),
        0 => {
            // Child process - continue
        }
        _ => {
            // Parent process - exit
            std::process::exit(0);
        }
    }

    // SAFETY: setsid() creates a new session and detaches from the controlling
    // terminal. This is standard Unix daemonization practice.
    if unsafe { libc::setsid() } == -1 {
        return Err("setsid failed".into());
    }

    // SAFETY: Second fork ensures the daemon cannot reacquire a controlling terminal.
    // Standard double-fork daemonization pattern.
    match unsafe { libc::fork() } {
        -1 => return Err("Second fork failed".into()),
        0 => {
            // Child continues
        }
        _ => {
            std::process::exit(0);
        }
    }

    // SAFETY: umask() sets file mode creation mask. 0o022 yields 0o644 files / 0o755 dirs.
    unsafe { libc::umask(0o022) };

    // SAFETY: chdir("/") avoids holding references to mount points.
    // The C string literal is null-terminated.
    if unsafe { libc::chdir(c"/".as_ptr()) } == -1 {
        return Err("chdir to / failed".into());
    }

    // Redirect stdout/stderr to log file if specified
    if let Some(log_path) = log_file {
        let log_path = expand_path(&log_path.to_string_lossy());
        if let Some(parent) = log_path.parent() {
            std::fs::create_dir_all(parent)?;

            // Keep the redirect target from accumulating across restarts: rotate
            // it out when it is from an earlier day or has grown past the cap,
            // then age out old rotations with the same 7-day retention
            // aleph-server.log uses. Best-effort — logging hygiene must never
            // block daemon startup.
            if let Err(e) = rotate_stale_or_oversized_log(&log_path, MAX_GATEWAY_LOG_BYTES) {
                eprintln!("Warning: failed to rotate {}: {e}", log_path.display());
            }
            if let Some(prefix) = log_path
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_suffix(".log"))
            {
                // best-effort filesystem cleanup of aged rotations; rotation
                // already bounded the live file, so a retention failure here is
                // non-fatal and we discard the result.
                let _ = aleph_logging::cleanup_old_logs(parent, 7, Some(prefix));
            }
        }

        let log_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)?;

        use std::os::unix::io::AsRawFd;
        let fd = log_file.as_raw_fd();

        // SAFETY: dup2 duplicates the file descriptor. fd is valid (from open file).
        // Redirect all stdio to detach from terminal.
        unsafe {
            if libc::dup2(fd, libc::STDIN_FILENO) == -1
                || libc::dup2(fd, libc::STDOUT_FILENO) == -1
                || libc::dup2(fd, libc::STDERR_FILENO) == -1
            {
                return Err("dup2 failed".into());
            }
        }
    } else {
        // Redirect to /dev/null by default.
        // Open read+write: the same fd is dup2'd onto STDOUT/STDERR below, and
        // writing to a read-only fd would fail with EBADF (panicking `println!`).
        use std::os::unix::io::AsRawFd;
        let dev_null = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/null")?;
        let fd = dev_null.as_raw_fd();

        // SAFETY: dup2 duplicates the file descriptor. fd is valid (from open file).
        // Also redirect stdin to /dev/null to fully detach from terminal.
        unsafe {
            if libc::dup2(fd, libc::STDIN_FILENO) == -1
                || libc::dup2(fd, libc::STDOUT_FILENO) == -1
                || libc::dup2(fd, libc::STDERR_FILENO) == -1
            {
                return Err("dup2 failed".into());
            }
        }
    }

    // Write PID file. BIN-R4-10: the previous shape used `?` which would
    // propagate the io::Error up — but the daemon has already
    // double-forked and redirected stdio at this point. Returning Err
    // here aborts the parent dispatcher cleanly but leaves the
    // daemon process running invisibly with no PID file: `aleph stop`
    // refuses ("no daemon is running"), `aleph status` says nothing,
    // and a second `start` runs into the singleton lock and exits 64.
    // Warn loudly to stderr (which is still the operator's terminal in
    // foreground mode) and return Ok so the daemon is at least
    // observable through process listing. The lock file held in
    // main() before fork() remains the durable fencing instrument.
    if let Err(e) = write_pid_file(pid_file) {
        eprintln!(
            "Warning: daemon started but PID file '{pid_file}' could not be written: {e}.\n  \
             The process is running; 'aleph stop' will fail to find it. Use 'ps' / 'pgrep' \
             and the singleton lock to recover."
        );
        tracing::warn!(
            pid_file,
            error = %e,
            "daemon: PID file write failed after fork; process is running but unfindable"
        );
    }

    Ok(())
}

#[cfg(not(unix))]
pub fn daemonize(
    _pid_file: &str,
    _log_file: Option<&PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    Err("Daemon mode is only supported on Unix systems".into())
}

#[cfg(all(test, unix))]
mod rotation_tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    fn write_file(path: &Path, bytes: usize) {
        let mut f = std::fs::File::create(path).expect("create temp file");
        f.write_all(&vec![b'x'; bytes]).expect("write temp file");
    }

    fn siblings(dir: &Path, except: &str) -> Vec<String> {
        std::fs::read_dir(dir)
            .expect("read temp dir")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n != except)
            .collect()
    }

    #[test]
    fn under_cap_is_not_rotated() {
        let dir = TempDir::new().unwrap();
        let log = dir.path().join("gateway.log");
        write_file(&log, 100);
        rotate_stale_or_oversized_log(&log, 1024).unwrap();
        assert!(log.exists());
        assert!(
            siblings(dir.path(), "gateway.log").is_empty(),
            "no rotation expected under cap"
        );
    }

    #[test]
    fn over_cap_is_rotated_to_dated_sibling() {
        let dir = TempDir::new().unwrap();
        let log = dir.path().join("gateway.log");
        write_file(&log, 2048);
        rotate_stale_or_oversized_log(&log, 1024).unwrap();
        // Original name freed for a fresh file; exactly one dated backup exists.
        assert!(!log.exists());
        let rotated = siblings(dir.path(), "gateway.log");
        assert_eq!(rotated.len(), 1, "expected one rotation, got {rotated:?}");
        let suffix = rotated[0]
            .strip_prefix("gateway.log.")
            .expect("gateway.log. prefix");
        assert_eq!(suffix.len(), 10, "date suffix must be YYYY-MM-DD");
        assert_eq!(suffix.as_bytes()[4], b'-');
        assert_eq!(suffix.as_bytes()[7], b'-');
    }

    #[test]
    fn missing_file_is_noop() {
        let dir = TempDir::new().unwrap();
        let log = dir.path().join("gateway.log");
        rotate_stale_or_oversized_log(&log, 1024).unwrap();
        assert!(!log.exists());
    }

    // Day-boundary rotation logic. Exercised through the pure decision function
    // because a stale mtime can't easily be forged on a temp file without an
    // extra dev-dependency, and this is where the "one file per day instead of
    // one file forever" behavior lives.
    fn at(y: i32, m: u32, d: u32, hh: u32) -> SystemTime {
        let ts = chrono::NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(hh, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp();
        SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(ts as u64)
    }

    const MAX: u64 = 5 * 1024 * 1024;

    #[test]
    fn empty_file_never_rotates() {
        // A zero-length file is nothing to archive, even if stale/"oversized".
        assert_eq!(
            rotation_suffix_date(0, at(2026, 7, 17, 9), at(2026, 7, 24, 9), MAX),
            None
        );
    }

    #[test]
    fn same_day_within_budget_keeps_appending() {
        // Same-day restarts append to one file; the ALEPH-BOOT marker separates
        // boots within it, so no rotation here.
        assert_eq!(
            rotation_suffix_date(1024, at(2026, 7, 24, 9), at(2026, 7, 24, 15), MAX),
            None
        );
    }

    #[test]
    fn earlier_day_rotates_under_its_own_date() {
        // A file last written on 07-17 archives as gateway.log.2026-07-17 —
        // named for the window it covers, not for today.
        assert_eq!(
            rotation_suffix_date(1024, at(2026, 7, 17, 23), at(2026, 7, 24, 1), MAX),
            chrono::NaiveDate::from_ymd_opt(2026, 7, 17)
        );
    }

    #[test]
    fn oversized_same_day_still_rotates() {
        // Size cap forces a rotation even within the same day.
        assert_eq!(
            rotation_suffix_date(MAX + 1, at(2026, 7, 24, 9), at(2026, 7, 24, 15), MAX),
            chrono::NaiveDate::from_ymd_opt(2026, 7, 24)
        );
    }
}

#[cfg(test)]
mod status_tests {
    use super::*;

    fn endpoint(pid: u32, started_at: &str) -> IpcEndpoint {
        IpcEndpoint {
            version: 1,
            url: "http://127.0.0.1:18790".to_string(),
            pid,
            started_at: started_at.to_string(),
        }
    }

    fn at(rfc3339: &str) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339(rfc3339)
            .unwrap()
            .with_timezone(&chrono::Utc)
    }

    #[test]
    fn live_endpoint_reports_url_and_uptime() {
        let ep = endpoint(4321, "2026-06-17T00:00:00Z");
        let report = resolve_status(None, Some(&ep), at("2026-06-17T01:01:01Z"), |p| p == 4321);
        assert!(report.running);
        assert_eq!(report.pid, Some(4321));
        assert_eq!(report.url.as_deref(), Some("http://127.0.0.1:18790"));
        assert_eq!(report.uptime_seconds, Some(3_661));
        assert_eq!(report.started_at.as_deref(), Some("2026-06-17T00:00:00Z"));
    }

    #[test]
    fn stale_endpoint_is_down_and_hides_url() {
        let ep = endpoint(4321, "2026-06-17T00:00:00Z");
        let report = resolve_status(None, Some(&ep), at("2026-06-17T01:00:00Z"), |_| false);
        assert!(!report.running);
        // Stale PID is still surfaced for the diagnostic message...
        assert_eq!(report.pid, Some(4321));
        // ...but a dead server must not advertise a URL or uptime.
        assert!(report.url.is_none());
        assert!(report.uptime_seconds.is_none());
    }

    #[test]
    fn daemon_pid_file_only_runs_without_endpoint_fields() {
        // Unix daemon path: PID file present, endpoint somehow absent.
        let report = resolve_status(Some(99), None, chrono::Utc::now(), |p| p == 99);
        assert!(report.running);
        assert_eq!(report.pid, Some(99));
        assert!(report.url.is_none());
    }

    #[test]
    fn live_endpoint_wins_over_dead_pid_file() {
        let ep = endpoint(4321, "2026-06-17T00:00:00Z");
        // PID file names a dead process; the endpoint names the live one.
        let report = resolve_status(Some(7), Some(&ep), at("2026-06-17T00:00:30Z"), |p| {
            p == 4321
        });
        assert!(report.running);
        assert_eq!(report.pid, Some(4321));
        assert_eq!(report.uptime_seconds, Some(30));
    }

    #[test]
    fn nothing_present_reports_down() {
        let report = resolve_status(None, None, chrono::Utc::now(), |_| true);
        assert!(!report.running);
        assert_eq!(report.pid, None);
        assert!(report.url.is_none());
    }

    #[test]
    fn down_report_omits_optional_fields_in_json() {
        let report = resolve_status(None, None, chrono::Utc::now(), |_| false);
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["running"], serde_json::json!(false));
        assert_eq!(json["pid"], serde_json::Value::Null);
        // skip_serializing_if keeps the down-state envelope minimal.
        assert!(json.get("url").is_none());
        assert!(json.get("uptime_seconds").is_none());
        assert!(json.get("version").is_some());
    }
}
