//! Daemon control RPC handlers
//!
//! Provides daemon.status, daemon.shutdown, and daemon.logs methods
//! for monitoring and controlling the Gateway server via WebSocket.

use serde::Deserialize;
use serde_json::json;
use std::path::PathBuf;
use std::time::Instant;

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR};
use crate::logging::get_log_directory;
use crate::sync_primitives::Arc;

/// Handle daemon.status — return server runtime information
pub async fn handle_status(request: JsonRpcRequest, start_time: Instant) -> JsonRpcResponse {
    let uptime = start_time.elapsed().as_secs();

    JsonRpcResponse::success(
        request.id,
        json!({
            "running": true,
            "uptime_secs": uptime,
            "version": env!("CARGO_PKG_VERSION"),
            "platform": format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        }),
    )
}

/// Handle daemon.shutdown — initiate graceful shutdown
pub async fn handle_shutdown(
    request: JsonRpcRequest,
    shutdown_tx: Arc<tokio::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
) -> JsonRpcResponse {
    let mut guard = shutdown_tx.lock().await;
    if let Some(tx) = guard.take() {
        let _ = tx.send(());
        JsonRpcResponse::success(request.id, json!({ "status": "shutting_down" }))
    } else {
        JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            "Shutdown already in progress".to_string(),
        )
    }
}

#[derive(Debug, Deserialize)]
struct LogsParams {
    #[serde(default = "default_lines")]
    lines: usize,
    #[serde(default)]
    level: Option<String>,
}

fn default_lines() -> usize {
    50
}

/// Handle daemon.logs — return recent log lines
pub async fn handle_logs(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: LogsParams = request
        .params
        .as_ref()
        .and_then(|p| serde_json::from_value(p.clone()).ok())
        .unwrap_or(LogsParams {
            lines: 50,
            level: None,
        });

    let log_dir = log_directory();
    let log_file = find_latest_log(&log_dir);

    match log_file {
        Some(path) => match std::fs::read_to_string(&path) {
            Ok(content) => {
                let mut lines: Vec<&str> = content.lines().collect();

                // Filter by level if specified
                if let Some(ref level) = params.level {
                    let level_upper = level.to_uppercase();
                    lines.retain(|line| line.contains(&level_upper));
                }

                // Take last N lines
                let start = lines.len().saturating_sub(params.lines);
                let result: Vec<String> = lines[start..].iter().map(|s| s.to_string()).collect();

                JsonRpcResponse::success(
                    request.id,
                    json!({
                        "logs": result,
                        "file": path.display().to_string(),
                        "total_lines": result.len(),
                    }),
                )
            }
            Err(e) => JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to read log file: {}", e),
            ),
        },
        None => JsonRpcResponse::success(
            request.id,
            json!({
                "logs": [],
                "file": null,
                "total_lines": 0,
            }),
        ),
    }
}

/// Get the log directory path.
///
/// Prefers the canonical `get_log_directory()` from the logging module,
/// falling back to `~/.aleph/logs` if that fails.
fn log_directory() -> PathBuf {
    get_log_directory().unwrap_or_else(|_| {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".aleph")
            .join("logs")
    })
}

/// Find the most recent log file in the directory
fn find_latest_log(dir: &PathBuf) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .map(|ext| ext == "log")
                .unwrap_or(false)
        })
        .max_by_key(|e| e.metadata().ok().and_then(|m| m.modified().ok()))
        .map(|e| e.path())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_directory_is_under_home() {
        let dir = log_directory();
        assert!(dir.to_string_lossy().contains(".aleph"));
    }

    #[test]
    fn find_latest_log_returns_none_for_missing_dir() {
        let result = find_latest_log(&PathBuf::from("/nonexistent/path"));
        assert!(result.is_none());
    }
}
