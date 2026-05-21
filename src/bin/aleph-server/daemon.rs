//! Daemon process management for Aleph Gateway
//!
//! This module handles PID file management, process lifecycle,
//! instance locking, and Unix daemonization.

use std::path::PathBuf;

/// Expand ~ to home directory.
/// Falls back to `/tmp/.aleph-$uid/...` when `dirs::home_dir()` returns `None`,
/// to avoid collisions between different users on shared systems.
pub fn expand_path(path: &str) -> PathBuf {
    if let Some(stripped) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(stripped);
        }
        let uid = unsafe { libc::getuid() };
        eprintln!(
            "Warning: cannot determine home directory; using /tmp/.aleph-{} as fallback",
            uid
        );
        return PathBuf::from(format!("/tmp/.aleph-{}", uid)).join(stripped);
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
    match unsafe { libc::kill(pid, 0) } {
        0 => true,
        _ => {
            let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
            // EPERM means process exists but we lack permission; ESRCH means it does not exist.
            errno == libc::EPERM
        }
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
        .and_then(|s| s.trim().parse().ok())
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
    let _ = std::fs::remove_file(&path);
}

/// Handle stop command
pub fn handle_stop(pid_file: &str) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(pid) = read_pid_file(pid_file) {
        if is_process_running(pid) {
            #[cfg(unix)]
            {
                println!("Sending SIGTERM to gateway process (PID {})", pid);
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
                    "Gateway process (PID {}) did not exit even after SIGKILL",
                    pid
                )
                .into());
            }

            #[cfg(not(unix))]
            {
                eprintln!("Daemon mode is only supported on Unix systems");
                return Err("Unsupported platform".into());
            }
        } else {
            println!("Gateway is not running (stale PID file)");
            remove_pid_file(pid_file);
        }
    } else {
        println!("No gateway daemon is running (no PID file found)");
    }
    Ok(())
}

/// Handle status command
pub fn handle_status(pid_file: &str, json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let pid = read_pid_file(pid_file);
    let running = pid.map(is_process_running).unwrap_or(false);

    if json {
        let status = serde_json::json!({
            "running": running,
            "pid": pid,
        });
        println!("{}", serde_json::to_string_pretty(&status)?);
    } else {
        match (pid, running) {
            (Some(p), true) => println!("Gateway is running (PID {})", p),
            (Some(p), false) => println!("Gateway is not running (stale PID file for PID {})", p),
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
            return Err(format!("Gateway already running (PID {})", pid).into());
        }
    }

    // Fork the process
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

    // Create new session
    if unsafe { libc::setsid() } == -1 {
        return Err("setsid failed".into());
    }

    // Fork again to prevent terminal reattachment
    match unsafe { libc::fork() } {
        -1 => return Err("Second fork failed".into()),
        0 => {
            // Child continues
        }
        _ => {
            std::process::exit(0);
        }
    }

    // Set umask so file permissions are deterministic
    unsafe { libc::umask(0o022) };

    // Change to root directory to avoid holding references to mount points
    if unsafe { libc::chdir(b"/\0".as_ptr() as *const libc::c_char) } == -1 {
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

        unsafe {
            if libc::dup2(fd, libc::STDOUT_FILENO) == -1 || libc::dup2(fd, libc::STDERR_FILENO) == -1
            {
                return Err("dup2 failed".into());
            }
        }
    } else {
        // Redirect to /dev/null by default
        use std::os::unix::io::AsRawFd;
        let dev_null = std::fs::File::open("/dev/null")?;
        let fd = dev_null.as_raw_fd();

        unsafe {
            if libc::dup2(fd, libc::STDOUT_FILENO) == -1
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
