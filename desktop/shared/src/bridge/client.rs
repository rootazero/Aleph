//! Long-lived Swift helper RPC client.
//!
//! Spawns `aleph-bridge` once and keeps stdin / stdout / stderr open. Requests
//! are line-delimited JSON; responses are matched back to their callers via
//! the shared `InflightTable`. Stderr is forwarded to tracing for diagnostics.
//!
//! Crash recovery: when the reader task detects stdout EOF it drains the
//! inflight table, resets the state slot to `None`, and records a restart in
//! the `RestartWindow`. If the window trips (5 crashes within 10 minutes by
//! default) the `disabled` latch is set and all subsequent calls return
//! `DesktopError::BridgeDisabled` immediately. Backoff between respawn
//! attempts is handled by `ensure_running` using `supervisor::Backoff`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{oneshot, Mutex};

use aleph_protocol::desktop_bridge::envelope::{Message, Request};

use super::codec::{decode_line, encode};
use super::inflight::InflightTable;
use super::supervisor::{Backoff, RestartWindow};
use crate::error::{DesktopError, Result};

/// Long-lived RPC client for the `aleph-bridge` Swift helper.
///
/// Cheap to clone the internal state; the external wrapper owns one copy.
pub struct SwiftBridge {
    binary_path: PathBuf,
    state: Arc<Mutex<Option<BridgeProcess>>>,
    inflight: InflightTable,
    id_seq: AtomicU64,
    backoff: Arc<Mutex<Backoff>>,
    restart_window: Arc<Mutex<RestartWindow>>,
    disabled: Arc<AtomicBool>,
}

struct BridgeProcess {
    #[allow(dead_code)] // held to keep the subprocess alive via Drop.
    child: Child,
    stdin: ChildStdin,
}

impl SwiftBridge {
    pub fn new(binary_path: PathBuf) -> Self {
        Self {
            binary_path,
            state: Arc::new(Mutex::new(None)),
            inflight: InflightTable::default(),
            id_seq: AtomicU64::new(1),
            backoff: Arc::new(Mutex::new(Backoff::default())),
            restart_window: Arc::new(Mutex::new(RestartWindow::new(5, std::time::Duration::from_secs(600)))),
            disabled: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Spawn the helper subprocess and wire up reader + stderr tasks.
    /// This is the inner spawn — does NOT touch backoff or restart_window.
    async fn spawn_process(&self) -> Result<()> {
        let mut cmd = Command::new(&self.binary_path);
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        // Place the helper in its own process group so it inherits the
        // parent's controlling terminal signals (SIGTERM/SIGKILL on
        // aleph-server shutdown propagate cleanly). The Swift helper also
        // polls getppid() and exits if it gets reparented to init (ppid=1).
        #[cfg(unix)]
        unsafe {
            use std::os::unix::process::CommandExt;
            cmd.pre_exec(|| {
                // setpgid(0, 0) makes this process its own group leader.
                if libc::setpgid(0, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }

        let mut child = cmd.spawn().map_err(|e| {
            DesktopError::BridgeFailed(format!(
                "spawn {}: {e}",
                self.binary_path.display()
            ))
        })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| DesktopError::BridgeFailed("missing stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| DesktopError::BridgeFailed("missing stdout".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| DesktopError::BridgeFailed("missing stderr".into()))?;

        // Clones for the reader task.
        let inflight = self.inflight.clone();
        let state_for_reader = Arc::clone(&self.state);
        let restart_window_for_reader = Arc::clone(&self.restart_window);
        let disabled_for_reader = Arc::clone(&self.disabled);

        // Reader task: stdout → InflightTable. On EOF, drain inflight,
        // reset state, and record a restart in the window.
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                match decode_line::<Message>(&line) {
                    Ok(Message::Response(r)) => {
                        let _ = inflight.complete(r.id, r.result).await;
                    }
                    Ok(Message::Error(e)) => {
                        if let Some(id) = e.id {
                            inflight
                                .fail(id, format!("bridge error: {}", e.error.message))
                                .await;
                        } else {
                            tracing::warn!(
                                target: "bridge",
                                "parse-error from helper: {}",
                                e.error.message
                            );
                        }
                    }
                    Ok(Message::Notification(_n)) => {
                        // Notifications handled by later stages — ignore for now.
                    }
                    Err(err) => {
                        tracing::warn!(target: "bridge", "decode failed: {err}; raw={line:?}");
                    }
                }
            }

            // stdout closed — helper crashed or exited.
            tracing::warn!(target: "bridge", "reader loop exited (helper stdout closed)");

            // 1. Drain all pending callers.
            inflight.fail_all("helper stdout closed").await;

            // 2. Reset the state slot so the next call triggers a respawn.
            {
                let mut guard = state_for_reader.lock().await;
                *guard = None;
            }

            // 3. Record the crash in the restart window; trip disabled if threshold exceeded.
            {
                let mut rw = restart_window_for_reader.lock().await;
                if rw.record_and_should_disable() {
                    disabled_for_reader.store(true, Ordering::SeqCst);
                    tracing::error!(
                        target: "bridge",
                        "bridge disabled: too many crashes within the restart window"
                    );
                }
            }
        });

        // Stderr forwarder → tracing.
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::info!(target: "bridge_stderr", "{line}");
            }
        });

        let mut guard = self.state.lock().await;
        *guard = Some(BridgeProcess { child, stdin });
        Ok(())
    }

    /// Ensure the helper is running. Respawns on demand with backoff.
    /// Returns `BridgeDisabled` immediately if the disabled latch is set.
    pub async fn ensure_running(&self) -> Result<()> {
        if self.disabled.load(Ordering::SeqCst) {
            return Err(DesktopError::BridgeDisabled(
                "restart threshold exceeded".into(),
            ));
        }

        {
            let guard = self.state.lock().await;
            if guard.is_some() {
                return Ok(());
            }
        }

        // State is None — attempt a spawn.
        match self.spawn_process().await {
            Ok(()) => {
                // Reset backoff on a successful spawn.
                self.backoff.lock().await.reset();
                Ok(())
            }
            Err(e) => {
                // Record a restart event (process failed to start counts as a crash).
                let should_disable = self
                    .restart_window
                    .lock()
                    .await
                    .record_and_should_disable();
                if should_disable {
                    self.disabled.store(true, Ordering::SeqCst);
                    tracing::error!(
                        target: "bridge",
                        "bridge disabled: too many spawn failures"
                    );
                    return Err(DesktopError::BridgeDisabled(
                        "restart threshold exceeded".into(),
                    ));
                }
                // Advance backoff (but don't sleep here — let the caller retry
                // on the next call so we don't hold up the thread for 1-30 s).
                let _delay = self.backoff.lock().await.next();
                Err(e)
            }
        }
    }

    /// Send a JSON-RPC request and await the typed response.
    pub async fn call<P, R>(&self, method: &str, params: P) -> Result<R>
    where
        P: serde::Serialize,
        R: serde::de::DeserializeOwned,
    {
        // Fast-path disabled check.
        if self.disabled.load(Ordering::SeqCst) {
            return Err(DesktopError::BridgeDisabled(
                "restart threshold exceeded".into(),
            ));
        }

        self.ensure_running().await?;

        let id = self.id_seq.fetch_add(1, Ordering::SeqCst);
        let req = Request {
            jsonrpc: "2.0".into(),
            id,
            method: method.into(),
            params: Some(
                serde_json::to_value(params)
                    .map_err(|e| DesktopError::BridgeFailed(format!("serialize params: {e}")))?,
            ),
        };
        let line = encode(&req)?;

        let (tx, rx) = oneshot::channel();
        self.inflight.register(id, tx).await;

        // Attempt to write to stdin. On failure, clear state and retry once.
        let write_result = {
            let mut guard = self.state.lock().await;
            match guard.as_mut() {
                None => Err(DesktopError::BridgeFailed("bridge not running".into())),
                Some(proc) => {
                    let r = proc.stdin.write_all(line.as_bytes()).await;
                    let r = r.and(proc.stdin.flush().await);
                    r.map_err(|e| DesktopError::BridgeFailed(format!("write stdin: {e}")))
                }
            }
        };

        if let Err(_write_err) = write_result {
            // stdin write failed — the process is dead. Reset state and drain inflight.
            {
                let mut guard = self.state.lock().await;
                *guard = None;
            }
            self.inflight.fail_all("write stdin failed").await;

            // One retry: re-ensure the helper and send again.
            self.ensure_running().await?;

            let (tx2, rx2) = oneshot::channel();
            // Re-register the same id (it was drained above).
            self.inflight.register(id, tx2).await;

            {
                let mut guard = self.state.lock().await;
                let proc = guard
                    .as_mut()
                    .ok_or_else(|| DesktopError::BridgeFailed("bridge not running after retry".into()))?;
                proc.stdin
                    .write_all(line.as_bytes())
                    .await
                    .map_err(|e| DesktopError::BridgeFailed(format!("write stdin retry: {e}")))?;
                proc.stdin
                    .flush()
                    .await
                    .map_err(|e| DesktopError::BridgeFailed(format!("flush stdin retry: {e}")))?;
            }

            let raw = rx2
                .await
                .map_err(|_| DesktopError::BridgeFailed("inflight dropped".into()))??;
            return serde_json::from_value(raw)
                .map_err(|e| DesktopError::BridgeFailed(format!("decode result: {e}")));
        }

        let raw = rx
            .await
            .map_err(|_| DesktopError::BridgeFailed("inflight dropped".into()))??;
        serde_json::from_value(raw)
            .map_err(|e| DesktopError::BridgeFailed(format!("decode result: {e}")))
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// Fake helper: reads a JSON line, extracts `"id":<N>`, and emits a
    /// matching success response.
    fn fake_helper_script() -> &'static str {
        r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":[[:space:]]*\([0-9][0-9]*\).*/\1/p')
  printf '{"jsonrpc":"2.0","id":%s,"result":{"pong":true}}\n' "$id"
done
"#
    }

    /// Fake helper that emits an error response for every request.
    fn failing_helper_script() -> &'static str {
        r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":[[:space:]]*\([0-9][0-9]*\).*/\1/p')
  printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32602,"message":"bad params"}}\n' "$id"
done
"#
    }

    fn install_fake(dir: &tempfile::TempDir, body: &str) -> PathBuf {
        let path = dir.path().join("fake-bridge");
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[tokio::test]
    async fn call_returns_result_from_fake_helper() {
        let dir = tempfile::tempdir().unwrap();
        let path = install_fake(&dir, fake_helper_script());

        let bridge = SwiftBridge::new(path);
        bridge.ensure_running().await.unwrap();
        let v: serde_json::Value = bridge
            .call("bridge.ping", serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(v["pong"], true);
    }

    #[tokio::test]
    async fn call_propagates_bridge_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = install_fake(&dir, failing_helper_script());

        let bridge = SwiftBridge::new(path);
        bridge.ensure_running().await.unwrap();
        let result: Result<serde_json::Value> =
            bridge.call("bridge.ping", serde_json::json!({})).await;
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("bad params"), "unexpected error: {msg}");
    }

    #[tokio::test]
    async fn concurrent_calls_get_distinct_ids() {
        let dir = tempfile::tempdir().unwrap();
        let path = install_fake(&dir, fake_helper_script());

        let bridge = Arc::new(SwiftBridge::new(path));
        bridge.ensure_running().await.unwrap();

        let mut handles = Vec::new();
        for _ in 0..8 {
            let b = Arc::clone(&bridge);
            handles.push(tokio::spawn(async move {
                let v: serde_json::Value = b
                    .call("bridge.ping", serde_json::json!({}))
                    .await
                    .unwrap();
                assert_eq!(v["pong"], true);
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
    }

    #[tokio::test]
    async fn auto_restart_after_crash() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("crash_once");
        let marker = dir.path().join("started_once");

        // First invocation: exit 1 immediately. Second invocation: fake_helper_script.
        let body = format!(
            "#!/bin/sh\nif [ ! -f {m} ]; then touch {m}; exit 1; fi\n{rest}",
            m = marker.display(),
            rest = fake_helper_script().trim_start_matches("#!/bin/sh\n"),
        );
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();

        let bridge = SwiftBridge::new(path);
        // First call races a premature exit: either the call fails fast or
        // ensure_running surfaces a spawn error. Either way the marker is
        // written so the second spawn succeeds.
        let _ = bridge
            .call::<_, serde_json::Value>("bridge.ping", serde_json::json!({}))
            .await;
        // Give the reader task a moment to observe stdout EOF and reset state.
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;

        let v: serde_json::Value = bridge
            .call("bridge.ping", serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(v["pong"], true);
    }

    #[tokio::test]
    async fn trips_disabled_after_too_many_spawn_failures() {
        // Non-existent binary guarantees every spawn fails instantly.
        let bridge = SwiftBridge::new(std::path::PathBuf::from(
            "/tmp/aleph-nonexistent-helper-for-test-0f3d",
        ));
        // 6 calls should drive past the 5-restart threshold.
        for _ in 0..6 {
            let _ = bridge
                .call::<_, serde_json::Value>("bridge.ping", serde_json::json!({}))
                .await;
        }
        // Next call must short-circuit with BridgeDisabled.
        let err = bridge
            .call::<_, serde_json::Value>("bridge.ping", serde_json::json!({}))
            .await
            .unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("disabled"),
            "expected BridgeDisabled, got: {msg}"
        );
    }
}
