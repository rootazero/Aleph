//! Daemon process management for Aleph Gateway
//!
//! This module handles PID file management, process lifecycle,
//! instance locking, and Unix daemonization.

use std::path::PathBuf;

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
/// Uses `kill(pid, 0)` which performs error checking without sending a signal.
/// Returns `true` if the process exists (kill succeeds or EPERM),
/// `false` if the process does not exist (ESRCH).
#[cfg(unix)]
pub fn is_process_running(pid: i32) -> bool {
    // A non-positive PID is never a real target: `kill(0, ..)` signals the
    // caller's whole process group and `kill(-1, ..)` signals every process the
    // user may signal. Treat it as "not running" so callers never escalate to
    // a broadcast SIGTERM/SIGKILL off a corrupted PID file.
    if pid <= 0 {
        return false;
    }
    // SAFETY: kill(pid, 0) performs error checking without sending a signal.
    // It is async-signal-safe and the only way to check process existence on Unix.
    if unsafe { libc::kill(pid, 0) } == 0 { true } else {
        let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
        // EPERM means process exists but we lack permission; ESRCH means it does not exist.
        errno == libc::EPERM
    }
}

#[cfg(not(unix))]
pub fn is_process_running(_pid: i32) -> bool {
    false
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

/// Handle stop command
pub fn handle_stop(pid_file: &str) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(pid) = read_pid_file(pid_file) {
        if is_process_running(pid) {
            #[cfg(unix)]
            {
                println!("Sending SIGTERM to gateway process (PID {pid})");
                // SAFETY: kill() with SIGTERM is the standard way to request graceful
                // process termination on Unix. PID was validated by is_process_running().
                if unsafe { libc::kill(pid, libc::SIGTERM) } != 0 {
                    eprintln!(
                        "Warning: failed to send SIGTERM to PID {}: {}",
                        pid,
                        std::io::Error::last_os_error()
                    );
                }

                // Wait for process to exit (max 5 seconds)
                for _ in 0..50 {
                    if !is_process_running(pid) {
                        println!("Gateway stopped successfully");
                        remove_pid_file(pid_file);
                        return Ok(());
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }

                println!("Gateway did not stop gracefully, sending SIGKILL");
                // SAFETY: kill() with SIGKILL is the standard way to forcefully terminate
                // a process on Unix. PID was validated at the start of this function.
                if unsafe { libc::kill(pid, libc::SIGKILL) } != 0 {
                    return Err(format!(
                        "Failed to send SIGKILL to PID {}: {}",
                        pid,
                        std::io::Error::last_os_error()
                    )
                    .into());
                }

                // Wait for process to exit after SIGKILL (max 2 seconds)
                for _ in 0..20 {
                    if !is_process_running(pid) {
                        println!("Gateway stopped successfully");
                        remove_pid_file(pid_file);
                        return Ok(());
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }

                return Err(format!(
                    "Gateway process (PID {pid}) did not exit even after SIGKILL"
                )
                .into());
            }

            #[cfg(not(unix))]
            {
                eprintln!("Daemon mode is only supported on Unix systems");
                return Err("Unsupported platform".into());
            }
        }
        println!("Gateway is not running (stale PID file)");
        remove_pid_file(pid_file);
    } else {
        println!("No gateway daemon is running (no PID file found)");
    }
    Ok(())
}

/// Handle status command
pub fn handle_status(pid_file: &str, json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let pid = read_pid_file(pid_file);
    let running = pid.is_some_and(is_process_running);

    if json {
        let status = serde_json::json!({
            "running": running,
            "pid": pid,
        });
        println!("{}", serde_json::to_string_pretty(&status)?);
    } else {
        match (pid, running) {
            (Some(p), true) => println!("Gateway is running (PID {p})"),
            (Some(p), false) => println!("Gateway is not running (stale PID file for PID {p})"),
            (None, _) => println!("Gateway is not running (no PID file)"),
        }
    }
    Ok(())
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

    // Write PID file
    write_pid_file(pid_file)?;

    Ok(())
}

#[cfg(not(unix))]
pub fn daemonize(
    _pid_file: &str,
    _log_file: Option<&PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    Err("Daemon mode is only supported on Unix systems".into())
}
