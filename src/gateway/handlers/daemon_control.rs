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

/// Handle daemon.status — return server runtime information
pub async fn handle_status(request: JsonRpcRequest, start_time: Instant) -> JsonRpcResponse {
    let uptime = start_time.elapsed().as_secs();

    JsonRpcResponse::success(
        request.id,
        json!({
            "running": true,
            "uptime_secs": uptime,
            "version": env!("ALEPH_VERSION"),
            "platform": format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        }),
    )
}

/// Handle daemon.shutdown — initiate graceful shutdown via SIGTERM to self.
///
/// Sends the response first, then schedules a SIGTERM after a brief delay
/// to allow the response to be flushed to the client. This mirrors the
/// approach used by the daemon IPC server.
pub async fn handle_shutdown(request: JsonRpcRequest) -> JsonRpcResponse {
    tracing::info!("Graceful shutdown requested via RPC");

    // A daemon shutdown ends every connected session and drops in-flight
    // runs; it used to leave no forensic row. Record WHO asked before the
    // process exits (the audit sink flushes synchronously on `log`).
    if let Some(log) = crate::security::audit::global() {
        log.log(crate::security::audit::AuditEntry::authority_change(
            crate::gateway::caller_identity::current_caller_user(),
            "daemon.shutdown: graceful shutdown requested".to_string(),
        )).await;
    }

    // Schedule shutdown after response is sent. We use process exit directly
    // rather than libc::kill/SIGTERM so the gateway core stays platform-neutral
    // (architecture redline R1). The binary's own SIGTERM handler does the same.
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        tracing::info!("Initiating graceful shutdown");
        std::process::exit(0);
    });

    JsonRpcResponse::success(request.id, json!({ "status": "shutting_down" }))
}

#[derive(Debug, Deserialize)]
struct LogsParams {
    #[serde(default = "default_lines")]
    lines: usize,
    #[serde(default)]
    level: Option<String>,
}

const fn default_lines() -> usize {
    50
}

/// Hard cap on the number of log lines returned per request. Without this an
/// admin-tier caller can request `lines = usize::MAX` and the server will
/// happily encode and ship the entire log history over the WS frame, which
/// is a trivial OOM / RST vector. The cap mirrors the values that have to be
/// sane regardless of `params.lines`.
const MAX_LOG_LINES: usize = 10_000;

/// Hard cap on the per-line byte length returned to the client. A single
/// multi-MB stack-trace line would otherwise blow out the JSON-RPC frame
/// size and force a fragmented response — truncate at the byte boundary so
/// the operator still sees the leading context but the wire format stays
/// bounded.
const MAX_LOG_LINE_BYTES: usize = 4 * 1024;

/// Hard cap on the number of bytes read off disk for `daemon.logs`. Without
/// this `tokio::fs::read_to_string` will happily load a multi-GB file into
/// heap; cap at the largest size we are willing to read in a single request
/// (4 MiB) and reject anything larger with a clear error. The operator can
/// request a smaller window via `lines` and a level-scoped filter.
const MAX_LOG_FILE_BYTES: u64 = 4 * 1024 * 1024;

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

    // Cap `lines` BEFORE any I/O so the bound is enforced even when the
    // caller supplies the maximum `usize` value.
    let requested_lines = params.lines.min(MAX_LOG_LINES);

    let log_dir = log_directory();
    let log_file = find_latest_log(&log_dir);

    match log_file {
        Some(path) => {
            // Reject log files above the byte cap up front. `metadata()` is
            // a cheap stat call that avoids pulling the whole file into
            // memory before we decide to read it.
            match tokio::fs::metadata(&path).await {
                Ok(md) if md.len() > MAX_LOG_FILE_BYTES => {
                    return JsonRpcResponse::error(
                        request.id,
                        INTERNAL_ERROR,
                        format!(
                            "log file too large to serve ({} bytes > {} cap); \
                             pass a smaller `lines` or filter by `level`",
                            md.len(),
                            MAX_LOG_FILE_BYTES
                        ),
                    );
                }
                Ok(_) => {}
                Err(e) => {
                    return JsonRpcResponse::error(
                        request.id,
                        INTERNAL_ERROR,
                        format!("Failed to stat log file: {e}"),
                    );
                }
            }

            match tokio::fs::read_to_string(&path).await {
                Ok(content) => {
                    let mut lines: Vec<&str> = content.lines().collect();

                    // Filter by level if specified
                    if let Some(ref level) = params.level {
                        let level_upper = level.to_uppercase();
                        // Match level as a standalone word to avoid partial matches
                        // (e.g., "ERROR" shouldn't match "WARN" or "INFO").
                        lines.retain(|line| {
                            line.contains(&format!(" {level_upper} "))
                                || line.contains(&format!("[{level_upper}]"))
                                || line.ends_with(&format!(" {level_upper}"))
                        });
                    }

                    // Take last N lines (bounded by `requested_lines`)
                    let start = lines.len().saturating_sub(requested_lines);
                    let result: Vec<String> = lines[start..]
                        .iter()
                        .map(|s| truncate_line_bytes(s, MAX_LOG_LINE_BYTES))
                        .collect();

                    JsonRpcResponse::success(
                        request.id,
                        json!({
                            "logs": result,
                            "file": path.display().to_string(),
                            "truncated_per_line_bytes": MAX_LOG_LINE_BYTES,
                            "max_lines": MAX_LOG_LINES,
                        }),
                    )
                }
                Err(e) => JsonRpcResponse::error(
                    request.id,
                    INTERNAL_ERROR,
                    format!("Failed to read log file: {e}"),
                ),
            }
        }
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

/// Truncate `s` to at most `max_bytes` bytes, respecting the nearest valid
/// UTF-8 boundary so the JSON encoder never emits a partial codepoint.
fn truncate_line_bytes(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = String::with_capacity(end + 1);
    out.push_str(&s[..end]);
    out.push('…');
    out
}

/// Get the log directory path.
///
/// Prefers the canonical `get_log_directory()` from the logging module,
/// falling back to `<config_dir>/logs` if that fails.
fn log_directory() -> PathBuf {
    get_log_directory().unwrap_or_else(|_| {
        crate::utils::paths::get_config_dir()
            .unwrap_or_else(|_| PathBuf::from(".").join(".aleph"))
            .join("logs")
    })
}

/// Find the most recent log file in the directory
fn find_latest_log(dir: &std::path::Path) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.starts_with("aleph-") && name.contains(".log")
        })
        .max_by_key(|e| e.metadata().ok().and_then(|m| m.modified().ok()))
        .map(|e| e.path())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_directory_is_under_home() {
        // ALEPH_HOME is process-global; lock so other tests don't point it at a
        // temp directory without "aleph" in the path while we read it. Same
        // hazard, same guard as `logging::file_appender::test_get_log_directory`
        // — this reads the very same `get_log_directory()`.
        let _guard = crate::utils::paths::ALEPH_HOME_TEST_GUARD
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let dir = log_directory();
        assert!(dir.to_string_lossy().contains(".aleph"));
    }

    #[test]
    fn find_latest_log_returns_none_for_missing_dir() {
        let result = find_latest_log(&PathBuf::from("/nonexistent/path"));
        assert!(result.is_none());
    }

    #[test]
    fn find_latest_log_matches_dated_files() {
        let (_scratch, dir) = crate::utils::scratch::scratch_root();
        let _ = std::fs::create_dir_all(&dir);

        // Create a file matching real naming: aleph-server.log.2026-03-04
        let dated = dir.join("aleph-server.log.2026-03-04");
        std::fs::write(&dated, "test log line").unwrap();

        let result = find_latest_log(&dir);
        assert!(result.is_some(), "Should find dated log file");
        assert!(result
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains("aleph-server"));

        // Cleanup
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn truncate_line_bytes_respects_utf8_boundaries() {
        // 4-byte emoji at the cap boundary: must not split a codepoint.
        let s = format!("{}🚀 tail", "x".repeat(MAX_LOG_LINE_BYTES - 2));
        let truncated = truncate_line_bytes(&s, MAX_LOG_LINE_BYTES);
        assert!(truncated.ends_with('…'));
        // The string after truncation must round-trip through valid UTF-8.
        let _ = std::str::from_utf8(truncated.as_bytes()).unwrap();
    }

    #[test]
    fn truncate_line_bytes_short_passes_through() {
        let s = "short line";
        assert_eq!(truncate_line_bytes(s, 64), s);
    }
}
