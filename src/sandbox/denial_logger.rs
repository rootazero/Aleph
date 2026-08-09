//! macOS sandbox denial logger (codex-inspired observability).
//!
//! macOS's `sandbox-exec` emits structured violation messages through the
//! system log when a sandboxed process attempts an action the SBPL profile
//! denies. The denials are otherwise invisible — the sandboxed process
//! just sees EPERM or ENOENT and may not surface a useful error.
//!
//! `DenialLogger` spawns `log stream` with a sandbox-scoped predicate,
//! captures every emitted line, and stops cleanly when its handle is
//! dropped (`stop()` joins the child).
//!
//! Used by the `aleph-server sandbox-debug --log-denials` command and
//! available as a building block for future audit integrations.
//!
//! Linux / Windows: there is no equivalent unified log stream, so the
//! type compiles to a no-op stub. Callers can use the same API on all
//! platforms.

use std::sync::Arc;

use tokio::sync::Mutex;

/// Captures sandbox denial lines from the macOS unified log stream.
///
/// Spawn with [`DenialLogger::start`]; collect lines with
/// [`DenialLogger::take_lines`]; tear down with [`DenialLogger::stop`].
/// Cloning the handle is cheap (internal `Arc`).
#[derive(Clone)]
pub struct DenialLogger {
    inner: Arc<Mutex<LoggerInner>>,
}

struct LoggerInner {
    lines: Vec<String>,
    #[cfg(target_os = "macos")]
    child: Option<tokio::process::Child>,
}

impl Default for DenialLogger {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(LoggerInner {
                lines: Vec::new(),
                #[cfg(target_os = "macos")]
                child: None,
            })),
        }
    }
}

impl DenialLogger {
    /// Construct a stopped logger; call [`start`] to begin streaming.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// True where a unified denial log stream exists. False on Linux /
    /// Windows, where [`DenialLogger`] is a no-op — callers should surface
    /// that honestly rather than claim a logger is running.
    #[must_use]
    pub const fn supported() -> bool {
        cfg!(target_os = "macos")
    }

    /// Start `log stream`. On non-macOS platforms this is a no-op that
    /// still returns `Ok(())`, so callers can run the same code path
    /// across OSes without `#[cfg]` gates.
    #[cfg(target_os = "macos")]
    pub async fn start(&self) -> Result<(), String> {
        use std::process::Stdio;
        use tokio::io::{AsyncBufReadExt, BufReader};

        let mut inner = self.inner.lock().await;
        if inner.child.is_some() {
            return Err("DenialLogger already started".into());
        }
        // Predicate filters to messages that mention "sandbox" — broad
        // enough to catch both `sandboxd` denials and `sandbox-exec`
        // policy violations without picking up unrelated subsystems.
        let mut cmd = tokio::process::Command::new("/usr/bin/log");
        cmd.args([
            "stream",
            "--style",
            "compact",
            "--predicate",
            "(sender == \"sandboxd\") || (eventMessage CONTAINS[c] \"deny(\") || (subsystem == \"com.apple.sandbox\")",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(Stdio::null());
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("failed to spawn `log stream`: {e}"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "log stream produced no stdout pipe".to_string())?;
        let sink = self.inner.clone();
        tokio::spawn(async move {
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();
            // `next_line()` returns Ok(None) when the child closes
            // stdout — that's our stop signal.
            while let Ok(Some(line)) = lines.next_line().await {
                let mut guard = sink.lock().await;
                guard.lines.push(line);
            }
        });
        inner.child = Some(child);
        Ok(())
    }

    /// Linux/Windows: no equivalent unified log stream — no-op.
    #[cfg(not(target_os = "macos"))]
    pub async fn start(&self) -> Result<(), String> {
        Ok(())
    }

    /// Take any captured lines so far, leaving the buffer empty. Safe
    /// to call repeatedly.
    pub async fn take_lines(&self) -> Vec<String> {
        let mut inner = self.inner.lock().await;
        std::mem::take(&mut inner.lines)
    }

    /// Stop the underlying `log stream` process. On non-macOS this is a
    /// no-op. Safe to call even if `start` was never called.
    pub async fn stop(&self) {
        // `mut` is only needed on macOS, where the child handle is taken below.
        #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
        let mut inner = self.inner.lock().await;
        #[cfg(target_os = "macos")]
        if let Some(mut child) = inner.child.take() {
            // Best-effort: SIGTERM, then wait briefly. Ignore errors — the
            // logger is observational, not safety-critical.
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        // Linux/Windows: no child to stop.
        let _ = &inner;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn start_and_stop_are_idempotent_on_unsupported_platforms() {
        let logger = DenialLogger::new();
        logger.start().await.expect("start");
        logger.stop().await;
        // take_lines starts empty.
        assert!(logger.take_lines().await.is_empty());
    }

    #[tokio::test]
    async fn double_start_errors_on_macos() {
        // On non-macOS the second start is also a no-op (Ok(())) — but
        // that's not a contract worth pinning; we only assert structural
        // behavior on the platform the logger actually runs on.
        let logger = DenialLogger::new();
        logger.start().await.expect("first start");
        #[cfg(target_os = "macos")]
        {
            let err = logger.start().await.expect_err("double start must error");
            assert!(err.contains("already"));
        }
        logger.stop().await;
    }

    #[tokio::test]
    async fn take_lines_clears_buffer() {
        let logger = DenialLogger::new();
        {
            // Inject a fake line directly through the inner buffer to
            // exercise take_lines without depending on `log stream`.
            let mut inner = logger.inner.lock().await;
            inner.lines.push("fake denial".to_string());
        }
        let first = logger.take_lines().await;
        assert_eq!(first, vec!["fake denial".to_string()]);
        let second = logger.take_lines().await;
        assert!(second.is_empty(), "second take must be empty");
    }
}
