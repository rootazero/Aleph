//! Daemon (Gateway server) management commands

use serde_json::Value;
use std::process::Command;

use crate::client::AlephClient;
use crate::error::CliResult;

/// Show Gateway server status
pub async fn status(server_url: &str) -> CliResult<()> {
    match AlephClient::connect(server_url).await {
        Ok((client, _events)) => {
            let result: Value = client.call("daemon.status", None::<()>).await?;

            println!("Gateway Status");
            println!("──────────────");
            if let Some(true) = result.get("running").and_then(|v| v.as_bool()) {
                println!("Status:      Running");
            } else {
                println!("Status:      Unknown");
            }
            if let Some(uptime) = result.get("uptime_secs").and_then(|v| v.as_u64()) {
                println!("Uptime:      {}", format_uptime(uptime));
            }
            if let Some(version) = result.get("version").and_then(|v| v.as_str()) {
                println!("Version:     {}", version);
            }
            if let Some(platform) = result.get("platform").and_then(|v| v.as_str()) {
                println!("Platform:    {}", platform);
            }

            client.close().await?;
            Ok(())
        }
        Err(_) => {
            println!("Gateway Status");
            println!("──────────────");
            println!("Status:      Not running");
            println!("URL:         {}", server_url);

            // Check for stale PID file
            let pid_file = dirs::home_dir()
                .unwrap_or_default()
                .join(".aleph")
                .join("aleph.pid");
            if pid_file.exists() {
                if let Ok(pid_str) = std::fs::read_to_string(&pid_file) {
                    println!("Stale PID:   {} (process not responding)", pid_str.trim());
                }
            }

            Ok(())
        }
    }
}

/// Start the Gateway server
pub fn start() -> CliResult<()> {
    println!("Starting Aleph Gateway...");

    // Try to start via the aleph binary
    match Command::new("aleph").arg("serve").spawn() {
        Ok(child) => {
            // Write PID file
            let pid_file = dirs::home_dir()
                .unwrap_or_default()
                .join(".aleph")
                .join("aleph.pid");
            if let Some(parent) = pid_file.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&pid_file, child.id().to_string());

            println!("Gateway started (PID: {})", child.id());
            Ok(())
        }
        Err(e) => {
            eprintln!("Failed to start Gateway: {}", e);
            eprintln!("Ensure 'aleph' binary is in your PATH");
            Ok(())
        }
    }
}

/// Stop the Gateway server
pub async fn stop(server_url: &str) -> CliResult<()> {
    // Try graceful shutdown via RPC
    match AlephClient::connect(server_url).await {
        Ok((client, _events)) => {
            let _: Value = client.call("daemon.shutdown", None::<()>).await?;
            println!("Gateway shutdown initiated.");

            // Clean up PID file
            let pid_file = dirs::home_dir()
                .unwrap_or_default()
                .join(".aleph")
                .join("aleph.pid");
            let _ = std::fs::remove_file(&pid_file);

            Ok(())
        }
        Err(_) => {
            // Fallback: try to kill via PID file
            let pid_file = dirs::home_dir()
                .unwrap_or_default()
                .join(".aleph")
                .join("aleph.pid");

            if pid_file.exists() {
                if let Ok(pid_str) = std::fs::read_to_string(&pid_file) {
                    let pid = pid_str.trim();
                    println!("Sending SIGTERM to PID {}...", pid);
                    let _ = Command::new("kill").arg(pid).status();
                    let _ = std::fs::remove_file(&pid_file);
                    println!("Gateway stopped.");
                    return Ok(());
                }
            }

            println!("Gateway is not running.");
            Ok(())
        }
    }
}

/// Restart the Gateway server
pub async fn restart(server_url: &str) -> CliResult<()> {
    stop(server_url).await?;
    // Brief pause for graceful shutdown
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    start()?;
    Ok(())
}

/// View Gateway logs
pub async fn logs(server_url: &str, lines: usize, level: Option<&str>) -> CliResult<()> {
    match AlephClient::connect(server_url).await {
        Ok((client, _events)) => {
            let mut params = serde_json::json!({ "lines": lines });
            if let Some(lvl) = level {
                params["level"] = serde_json::Value::String(lvl.to_string());
            }

            let result: Value = client.call("daemon.logs", Some(params)).await?;

            if let Some(file) = result.get("file").and_then(|v| v.as_str()) {
                println!("Log file: {}", file);
                println!();
            }

            if let Some(log_lines) = result.get("logs").and_then(|v| v.as_array()) {
                for line in log_lines {
                    if let Some(s) = line.as_str() {
                        println!("{}", s);
                    }
                }
                if log_lines.is_empty() {
                    println!("(no log entries found)");
                }
            }

            client.close().await?;
            Ok(())
        }
        Err(_) => {
            // Fallback: read logs directly from filesystem
            let log_dir = dirs::home_dir()
                .unwrap_or_default()
                .join(".aleph")
                .join("logs");

            println!(
                "Gateway not running. Reading logs from: {}",
                log_dir.display()
            );

            if let Ok(entries) = std::fs::read_dir(&log_dir) {
                let mut log_file = None;
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().map(|e| e == "log").unwrap_or(false) {
                        log_file = Some(path);
                    }
                }

                if let Some(path) = log_file {
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        let all_lines: Vec<&str> = content.lines().collect();
                        let start = all_lines.len().saturating_sub(lines);
                        for line in &all_lines[start..] {
                            if let Some(lvl) = level {
                                if line.to_uppercase().contains(&lvl.to_uppercase()) {
                                    println!("{}", line);
                                }
                            } else {
                                println!("{}", line);
                            }
                        }
                    }
                } else {
                    println!("No log files found.");
                }
            } else {
                println!("Log directory not found: {}", log_dir.display());
            }

            Ok(())
        }
    }
}

/// Format seconds into human-readable uptime string
fn format_uptime(secs: u64) -> String {
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let mins = (secs % 3600) / 60;
    let s = secs % 60;

    if days > 0 {
        format!("{}d {}h {}m {}s", days, hours, mins, s)
    } else if hours > 0 {
        format!("{}h {}m {}s", hours, mins, s)
    } else if mins > 0 {
        format!("{}m {}s", mins, s)
    } else {
        format!("{}s", s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_uptime_seconds() {
        assert_eq!(format_uptime(45), "45s");
    }

    #[test]
    fn format_uptime_minutes() {
        assert_eq!(format_uptime(125), "2m 5s");
    }

    #[test]
    fn format_uptime_hours() {
        assert_eq!(format_uptime(3661), "1h 1m 1s");
    }

    #[test]
    fn format_uptime_days() {
        assert_eq!(format_uptime(90061), "1d 1h 1m 1s");
    }
}
