//! Daemon (Gateway server) management commands
//!
//! Manages the `aleph-server` binary as an external process: start, stop,
//! status, restart. Uses PID file at `~/.aleph/aleph.pid` for tracking.

use std::path::PathBuf;
use std::process::Command;

use crate::output;
use aleph_client::CliResult;

/// Default binary name for the server
const SERVER_BINARY: &str = "aleph-server";

/// Environment variable to override the server binary path
const SERVER_BINARY_ENV: &str = "ALEPH_SERVER_BIN";

// ── PID file utilities ──────────────────────────────────────────────────

/// Path to the PID file used for daemon process tracking
fn pid_file_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".aleph")
        .join("aleph.pid")
}

/// Read PID from file, returning None if missing or unparseable
fn read_pid_file() -> Option<u32> {
    let path = pid_file_path();
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
}

/// Remove the PID file
fn remove_pid_file() {
    let _ = std::fs::remove_file(pid_file_path());
}

// ── Process utilities ───────────────────────────────────────────────────

/// Check if a process with the given PID is running
#[cfg(unix)]
fn is_process_running(pid: u32) -> bool {
    // kill(pid, 0) checks existence without sending a signal
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[cfg(not(unix))]
fn is_process_running(_pid: u32) -> bool {
    false
}

/// Send a signal to a process (Unix only)
#[cfg(unix)]
fn send_signal(pid: u32, signal: i32) -> bool {
    unsafe { libc::kill(pid as i32, signal) == 0 }
}

/// Locate the `aleph-server` binary.
///
/// Search order:
/// 1. `ALEPH_SERVER_BIN` env var (explicit override)
/// 2. Same directory as the current `aleph` binary
/// 3. Fall back to bare name (resolved via PATH)
fn find_server_binary() -> PathBuf {
    // 1. Env var override
    if let Ok(path) = std::env::var(SERVER_BINARY_ENV) {
        let p = PathBuf::from(&path);
        if p.exists() {
            return p;
        }
        // Still use the env var value even if it doesn't exist yet —
        // the user explicitly asked for it
        return p;
    }

    // 2. Sibling of the current executable
    if let Ok(current_exe) = std::env::current_exe() {
        if let Some(dir) = current_exe.parent() {
            let sibling = dir.join(SERVER_BINARY);
            if sibling.exists() {
                return sibling;
            }
        }
    }

    // 3. Bare name — rely on PATH
    PathBuf::from(SERVER_BINARY)
}

// ── Subcommands ─────────────────────────────────────────────────────────

/// Start the Gateway server
pub fn start(
    port: Option<u16>,
    daemonize: bool,
    config: Option<&str>,
    json: bool,
) -> CliResult<()> {
    // Check if already running
    if let Some(pid) = read_pid_file() {
        if is_process_running(pid) {
            if json {
                output::print_json(&serde_json::json!({
                    "status": "already_running",
                    "pid": pid,
                }));
            } else {
                eprintln!("Gateway is already running (PID {})", pid);
            }
            return Ok(());
        }
        // Stale PID file — clean up
        remove_pid_file();
    }

    let binary = find_server_binary();

    if !json {
        println!("Starting Aleph Gateway...");
        println!("Binary: {}", binary.display());
    }

    let mut cmd = Command::new(&binary);
    cmd.arg("serve");

    if let Some(p) = port {
        cmd.arg("--port").arg(p.to_string());
    }
    if daemonize {
        cmd.arg("--daemon");
    }
    if let Some(c) = config {
        cmd.arg("--config").arg(c);
    }

    // If daemonize, we rely on the server to fork itself.
    // Either way, spawn and don't wait.
    match cmd.spawn() {
        Ok(child) => {
            let child_pid = child.id();

            // Write PID file so we can track it
            let pid_file = pid_file_path();
            if let Some(parent) = pid_file.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&pid_file, child_pid.to_string());

            if json {
                output::print_json(&serde_json::json!({
                    "status": "started",
                    "pid": child_pid,
                    "binary": binary.display().to_string(),
                }));
            } else {
                println!("Gateway started (PID: {})", child_pid);
            }
            Ok(())
        }
        Err(e) => {
            if json {
                output::print_json(&serde_json::json!({
                    "status": "error",
                    "error": e.to_string(),
                    "binary": binary.display().to_string(),
                }));
            } else {
                eprintln!("Failed to start Gateway: {}", e);
                eprintln!("Binary: {}", binary.display());
                eprintln!(
                    "Hint: set {} to override the binary path",
                    SERVER_BINARY_ENV
                );
            }
            Ok(())
        }
    }
}

/// Stop the Gateway server
pub fn stop(json: bool) -> CliResult<()> {
    let pid = match read_pid_file() {
        Some(p) => p,
        None => {
            if json {
                output::print_json(&serde_json::json!({"status": "not_running"}));
            } else {
                println!("Gateway is not running (no PID file)");
            }
            return Ok(());
        }
    };

    if !is_process_running(pid) {
        remove_pid_file();
        if json {
            output::print_json(&serde_json::json!({
                "status": "not_running",
                "stale_pid": pid,
            }));
        } else {
            println!("Gateway is not running (stale PID file for PID {})", pid);
        }
        return Ok(());
    }

    #[cfg(unix)]
    {
        if !json {
            println!("Sending SIGTERM to Gateway (PID {})...", pid);
        }
        send_signal(pid, libc::SIGTERM);

        // Wait up to 5 seconds for graceful exit
        for _ in 0..50 {
            if !is_process_running(pid) {
                remove_pid_file();
                if json {
                    output::print_json(&serde_json::json!({
                        "status": "stopped",
                        "pid": pid,
                        "method": "sigterm",
                    }));
                } else {
                    println!("Gateway stopped successfully");
                }
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }

        // Force kill
        if !json {
            println!("Gateway did not stop gracefully, sending SIGKILL...");
        }
        send_signal(pid, libc::SIGKILL);
        std::thread::sleep(std::time::Duration::from_millis(200));
        remove_pid_file();

        if json {
            output::print_json(&serde_json::json!({
                "status": "killed",
                "pid": pid,
                "method": "sigkill",
            }));
        } else {
            println!("Gateway forcefully killed (PID {})", pid);
        }
    }

    #[cfg(not(unix))]
    {
        if json {
            output::print_json(&serde_json::json!({
                "status": "error",
                "error": "Daemon stop not supported on this platform",
            }));
        } else {
            eprintln!("Daemon stop is only supported on Unix systems");
        }
    }

    Ok(())
}

/// Show Gateway server status
pub fn status(json: bool) -> CliResult<()> {
    let pid = read_pid_file();
    let running = pid.map(is_process_running).unwrap_or(false);

    if json {
        output::print_json(&serde_json::json!({
            "running": running,
            "pid": pid,
            "pid_file": pid_file_path().display().to_string(),
        }));
    } else {
        println!("Gateway Status");
        println!("──────────────");
        match (pid, running) {
            (Some(p), true) => {
                println!("Status:      Running");
                println!("PID:         {}", p);
            }
            (Some(p), false) => {
                println!("Status:      Not running (stale PID {})", p);
            }
            (None, _) => {
                println!("Status:      Not running");
            }
        }
        println!("PID file:    {}", pid_file_path().display());
    }

    Ok(())
}

/// Restart the Gateway server
pub fn restart(
    port: Option<u16>,
    daemonize: bool,
    config: Option<&str>,
    json: bool,
) -> CliResult<()> {
    // Stop if running
    if let Some(pid) = read_pid_file() {
        if is_process_running(pid) {
            if !json {
                println!("Stopping existing Gateway...");
            }
            stop(json)?;
            // Brief pause for cleanup
            std::thread::sleep(std::time::Duration::from_secs(1));
        } else {
            remove_pid_file();
        }
    }

    start(port, daemonize, config, json)
}

/// View Gateway logs (reads from filesystem)
pub fn logs(lines: usize, level: Option<&str>, json: bool) -> CliResult<()> {
    let log_dir = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".aleph")
        .join("logs");

    if json {
        let entries = read_log_entries(&log_dir, lines, level);
        output::print_json(&serde_json::json!({
            "log_dir": log_dir.display().to_string(),
            "lines": entries,
        }));
    } else {
        println!("Log directory: {}", log_dir.display());
        println!();

        let entries = read_log_entries(&log_dir, lines, level);
        if entries.is_empty() {
            println!("(no log entries found)");
        } else {
            for line in &entries {
                println!("{}", line);
            }
        }
    }

    Ok(())
}

/// Read the last N lines from the most recent log file
fn read_log_entries(log_dir: &std::path::Path, lines: usize, level: Option<&str>) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(log_dir) else {
        return vec![];
    };

    // Find the most recent log file
    let mut log_file: Option<PathBuf> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().map(|e| e == "log").unwrap_or(false) {
            log_file = Some(path);
        }
    }

    let Some(path) = log_file else {
        return vec![];
    };

    let Ok(content) = std::fs::read_to_string(&path) else {
        return vec![];
    };

    let all_lines: Vec<&str> = content.lines().collect();
    let start = all_lines.len().saturating_sub(lines);

    all_lines[start..]
        .iter()
        .filter(|line| {
            if let Some(lvl) = level {
                line.to_uppercase().contains(&lvl.to_uppercase())
            } else {
                true
            }
        })
        .map(|s| s.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pid_file_path_under_home() {
        let path = pid_file_path();
        assert!(path.ends_with(".aleph/aleph.pid"));
    }

    #[test]
    fn find_server_binary_returns_something() {
        let binary = find_server_binary();
        // At minimum it should return the bare name
        assert!(binary.to_str().unwrap().contains("aleph-server"));
    }

    #[test]
    fn read_log_entries_empty_dir() {
        let entries = read_log_entries(std::path::Path::new("/nonexistent"), 50, None);
        assert!(entries.is_empty());
    }
}
