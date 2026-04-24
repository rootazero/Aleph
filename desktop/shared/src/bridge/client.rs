//! Long-lived Swift helper RPC client.
//!
//! Spawns `aleph-bridge` once and keeps stdin / stdout / stderr open. Requests
//! are line-delimited JSON; responses are matched back to their callers via
//! the shared `InflightTable`. Stderr is forwarded to tracing for diagnostics.
//!
//! Crash recovery and backoff live in the supervisor (T0.6) — this module is
//! the bare transport.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{oneshot, Mutex};

use aleph_protocol::desktop_bridge::envelope::{Message, Request};

use super::codec::{decode_line, encode};
use super::inflight::InflightTable;
use crate::error::{DesktopError, Result};

/// Long-lived RPC client for the `aleph-bridge` Swift helper.
///
/// Cheap to clone the internal state; the external wrapper owns one copy.
pub struct SwiftBridge {
    binary_path: PathBuf,
    state: Arc<Mutex<Option<BridgeProcess>>>,
    inflight: InflightTable,
    id_seq: AtomicU64,
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
        }
    }

    /// Spawn the helper if it isn't already running. Safe to call repeatedly.
    pub async fn ensure_running(&self) -> Result<()> {
        let mut guard = self.state.lock().await;
        if guard.is_some() {
            return Ok(());
        }

        let mut child = Command::new(&self.binary_path)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| {
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

        // Reader task: stdout → InflightTable.
        let inflight = self.inflight.clone();
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
                        // Notifications (e.g. ax.mutation, perm.status_changed)
                        // are handled by later stages — ignore for now.
                    }
                    Err(err) => {
                        tracing::warn!(target: "bridge", "decode failed: {err}; raw={line:?}");
                    }
                }
            }
            tracing::warn!(target: "bridge", "reader loop exited (helper stdout closed)");
        });

        // Stderr forwarder → tracing.
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::info!(target: "bridge_stderr", "{line}");
            }
        });

        *guard = Some(BridgeProcess { child, stdin });
        Ok(())
    }

    /// Send a JSON-RPC request and await the typed response.
    pub async fn call<P, R>(&self, method: &str, params: P) -> Result<R>
    where
        P: serde::Serialize,
        R: serde::de::DeserializeOwned,
    {
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

        {
            let mut guard = self.state.lock().await;
            let proc = guard
                .as_mut()
                .ok_or_else(|| DesktopError::BridgeFailed("bridge not running".into()))?;
            proc.stdin
                .write_all(line.as_bytes())
                .await
                .map_err(|e| DesktopError::BridgeFailed(format!("write stdin: {e}")))?;
            proc.stdin
                .flush()
                .await
                .map_err(|e| DesktopError::BridgeFailed(format!("flush stdin: {e}")))?;
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
}
