//! Long-lived Swift helper RPC client.
//!
//! Spawns `aleph-bridge` once and keeps stdin / stdout / stderr open. Requests
//! are line-delimited JSON; responses are matched back to their callers via
//! the shared `InflightTable`. Stderr is forwarded to tracing for diagnostics.
//!
//! Crash recovery: when the reader task detects stdout EOF it drains the
//! inflight table, resets the state slot to `None`, and records the crash in
//! the shared [`SpawnGate`]. If the restart window trips (5 crashes within 10
//! minutes by default) the `disabled` latch is set and all subsequent calls
//! return `DesktopError::BridgeDisabled` immediately.
//!
//! Respawn pacing: every (re)spawn goes through `ensure_running`, which asks the
//! `SpawnGate` whether enough time has elapsed since the last failure. Within
//! the backoff window it returns `DesktopError::BridgeBackoff` *without*
//! spawning, so a crash/spawn-failure loop spaces out (1s→2s→…→30s) instead of
//! hammering — the old code computed this delay and then discarded it, leaving
//! the ladder inert. The state lock is held across the check-and-spawn so two
//! concurrent first-callers cannot both spawn a helper.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{oneshot, Mutex};

use aleph_protocol::desktop_bridge::envelope::{Message, Request, RpcError};
use aleph_protocol::desktop_bridge::errors::{
    ERR_BRIDGE_DISABLED, ERR_HELPER_CRASHED, ERR_NOT_IMPLEMENTED, ERR_PERMISSION_DENIED,
    ERR_PLATFORM, ERR_TIMEOUT,
};
use aleph_protocol::desktop_bridge::methods::perm::PermissionGuide;

use super::codec::{decode_line, encode};
use super::inflight::InflightTable;
use super::supervisor::{SpawnDecision, SpawnGate};
use crate::error::{DesktopError, Result};

/// Convert a bridge `RpcError` into a `DesktopError`.
///
/// The server-defined codes (`errors.rs`) carry semantics the caller needs to
/// distinguish, so each maps to its matching typed variant — most importantly
/// `-32002 NotImplemented` (the method exists, the capability is deliberately
/// absent, e.g. `pim.mail.*` on macOS) vs `-32601 MethodNotFound` (a real
/// wiring gap). `-32001 PermissionDenied` additionally carries a
/// `PermissionGuide` in `data` so the LLM can surface the deep link, steps, and
/// rationale. Codes with no typed home (parse / invalid-request / internal /
/// helper-crashed / anything unknown) fall through to `BridgeFailed`.
fn map_bridge_error(e: RpcError) -> DesktopError {
    match e.code {
        ERR_PERMISSION_DENIED => {
            if let Some(data) = e.data {
                if let Ok(guide) = serde_json::from_value::<PermissionGuide>(data) {
                    return DesktopError::PermissionDenied {
                        kind: guide.kind,
                        guide: Box::new(guide),
                    };
                }
            }
            DesktopError::BridgeFailed(format!("bridge error {}: {}", e.code, e.message))
        }
        ERR_NOT_IMPLEMENTED => DesktopError::NotImplemented(e.message),
        ERR_PLATFORM => DesktopError::PlatformError(e.message),
        ERR_TIMEOUT => DesktopError::BridgeTimeout(e.message),
        ERR_BRIDGE_DISABLED => DesktopError::BridgeDisabled(e.message),
        // `-32005 HelperCrashed` has no dedicated variant; it means the helper
        // died mid-request, which is exactly `BridgeFailed`'s remit.
        ERR_HELPER_CRASHED => DesktopError::BridgeFailed(format!("helper crashed: {}", e.message)),
        _ => DesktopError::BridgeFailed(format!("bridge error {}: {}", e.code, e.message)),
    }
}

/// Default per-call RPC deadline.
///
/// Generous enough for the slowest *interactive* helper operations — OCR,
/// AX tree walks, PIM queries, a single camera snap — yet bounded so a helper
/// that accepts a request and then hangs (without closing stdout, so crash
/// recovery never fires) cannot wedge an agent turn forever.
///
/// Long-running capture operations (`camera.clip`, `audio.record`,
/// `speech.transcribe_file`) outlast this default; they must use
/// [`SwiftBridge::call_with_timeout`] with a deadline derived from the
/// requested duration instead.
pub const DEFAULT_RPC_TIMEOUT: Duration = Duration::from_mins(1);

/// JSON-RPC envelope version Aleph speaks to the Swift helper. Sent in the
/// `bridge.handshake` request and required to match the helper's reply.
pub const BRIDGE_PROTOCOL_VERSION: u32 = 2;

/// Long-lived RPC client for the `aleph-bridge` Swift helper.
///
/// Cheap to clone the internal state; the external wrapper owns one copy.
pub struct SwiftBridge {
    binary_path: PathBuf,
    state: Arc<Mutex<Option<BridgeProcess>>>,
    inflight: InflightTable,
    id_seq: AtomicU64,
    /// Single source of truth for respawn pacing (backoff + restart window).
    gate: Arc<Mutex<SpawnGate>>,
    disabled: Arc<AtomicBool>,
    /// Monotonic helper generation. Bumped once per successful spawn so a stale
    /// reader task can tell whether a newer helper has superseded it before it
    /// runs its EOF cleanup (which would otherwise clobber the live helper).
    generation: Arc<AtomicU64>,
}

struct BridgeProcess {
    #[allow(dead_code)] // held to keep the subprocess alive via Drop.
    child: Child,
    stdin: ChildStdin,
}

impl SwiftBridge {
    #[must_use]
    pub fn new(binary_path: PathBuf) -> Self {
        Self {
            binary_path,
            state: Arc::new(Mutex::new(None)),
            inflight: InflightTable::default(),
            id_seq: AtomicU64::new(1),
            gate: Arc::new(Mutex::new(SpawnGate::new(5, Duration::from_mins(10)))),
            disabled: Arc::new(AtomicBool::new(false)),
            generation: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Returns `true` if the helper subprocess is currently spawned (its
    /// reader loop is live).
    ///
    /// Cheap, non-blocking diagnostic: it `try_lock`s the state slot and never
    /// awaits. Used to verify the bridge stays idle (unspawned) until the first
    /// real `desktop.*` call. A momentary lock contention conservatively
    /// reports `false`.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.state.try_lock().is_ok_and(|g| g.is_some())
    }

    /// Spawn the helper subprocess and wire up reader + stderr tasks.
    ///
    /// This is the inner spawn — it does NOT touch the `SpawnGate` and does NOT
    /// install the process into `self.state`; it returns the live
    /// [`BridgeProcess`] so the caller (`ensure_running`) can store it while
    /// still holding the state lock, closing the double-spawn race.
    fn spawn_process(&self) -> Result<BridgeProcess> {
        let mut cmd = Command::new(&self.binary_path);
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        // Place the helper in its own process group so it inherits the
        // parent's controlling terminal signals (SIGTERM/SIGKILL on
        // aleph-server shutdown propagate cleanly). The Swift helper also
        // polls getppid() and exits if it gets reparented to init (ppid=1).
        #[cfg(unix)]
        // SAFETY: the pre_exec closure only calls async-signal-safe syscalls
        // (setpgid) and returns an error via io::Error, not panic.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setpgid(0, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }

        let mut child = cmd.spawn().map_err(|e| {
            DesktopError::BridgeFailed(format!("spawn {}: {e}", self.binary_path.display()))
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
        let gate_for_reader = Arc::clone(&self.gate);
        let disabled_for_reader = Arc::clone(&self.disabled);
        let generation_for_reader = Arc::clone(&self.generation);
        // Claim this reader's generation now that cmd.spawn() has succeeded, so
        // every live reader owns a unique, monotonically increasing gen aligned
        // with the helper `ensure_running` is about to install.
        let my_gen = self.generation.fetch_add(1, Ordering::SeqCst) + 1;

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
                            let desktop_err = map_bridge_error(e.error);
                            inflight.fail_err(id, desktop_err).await;
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

            // Only the newest reader owns the state slot and the shared inflight
            // table. If a newer helper was installed while this reader's EOF was
            // pending (e.g. the write-failure retry path already respawned),
            // `generation` has advanced past `my_gen` — skip cleanup so we don't
            // drain the live helper's in-flight ids or null its state slot.
            if generation_for_reader.load(Ordering::SeqCst) == my_gen {
                // 1. Drain all pending callers.
                inflight.fail_all("helper stdout closed").await;

                // 2. Reset the state slot so the next call triggers a respawn.
                {
                    let mut guard = state_for_reader.lock().await;
                    *guard = None;
                }

                // 3. Record the crash in the spawn gate: arm the backoff window
                //    so the next call paces its respawn, and trip disabled if the
                //    restart threshold is exceeded.
                {
                    let mut gate = gate_for_reader.lock().await;
                    if gate.record_failure() {
                        disabled_for_reader.store(true, Ordering::SeqCst);
                        tracing::error!(
                            target: "bridge",
                            "bridge disabled: too many crashes within the restart window"
                        );
                    }
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

        Ok(BridgeProcess { child, stdin })
    }

    /// Ensure the helper is running, respawning on demand subject to the
    /// `SpawnGate` backoff.
    ///
    /// Returns `BridgeDisabled` if the disabled latch is set, or `BridgeBackoff`
    /// if a respawn was requested while still inside the cooldown window. The
    /// `state` lock is held across the whole check-and-spawn so two concurrent
    /// first-callers cannot both spawn a helper (which would leave one orphan
    /// reader task polluting the restart window with a phantom crash).
    pub async fn ensure_running(&self) -> Result<()> {
        if self.disabled.load(Ordering::SeqCst) {
            return Err(DesktopError::BridgeDisabled(
                "restart threshold exceeded".into(),
            ));
        }

        let mut guard = self.state.lock().await;
        if guard.is_some() {
            return Ok(());
        }

        // Respawn pacing gate. Holding the state lock here is safe: no other
        // path acquires the gate lock while holding the state lock except this
        // one, and the reader task never nests the two.
        match self.gate.lock().await.poll() {
            SpawnDecision::Go => {}
            SpawnDecision::Backoff { remaining } => {
                return Err(DesktopError::BridgeBackoff(format!(
                    "helper recovering; retry in {}s",
                    remaining.as_secs().max(1)
                )));
            }
        }

        // Gate cleared — attempt a spawn (does not touch state or gate).
        match self.spawn_process() {
            Ok(proc) => {
                *guard = Some(proc);
                self.gate.lock().await.record_spawn();
                Ok(())
            }
            Err(e) => {
                // A failed spawn counts as a crash: arm backoff + restart window.
                if self.gate.lock().await.record_failure() {
                    self.disabled.store(true, Ordering::SeqCst);
                    tracing::error!(
                        target: "bridge",
                        "bridge disabled: too many spawn failures"
                    );
                    return Err(DesktopError::BridgeDisabled(
                        "restart threshold exceeded".into(),
                    ));
                }
                Err(e)
            }
        }
    }

    /// Send a JSON-RPC request and await the typed response, bounded by the
    /// [`DEFAULT_RPC_TIMEOUT`].
    ///
    /// Most callers want this. Operations that can legitimately run longer
    /// than the default (camera/audio capture) must use
    /// [`SwiftBridge::call_with_timeout`] instead.
    pub async fn call<P, R>(&self, method: &str, params: P) -> Result<R>
    where
        P: serde::Serialize,
        R: serde::de::DeserializeOwned,
    {
        self.call_with_timeout(method, params, DEFAULT_RPC_TIMEOUT)
            .await
    }

    /// Await a reply on `rx`, bounded by `timeout`.
    ///
    /// On timeout the dangling inflight slot for `id` is cancelled so it does
    /// not leak, and [`DesktopError::BridgeTimeout`] is returned. The helper
    /// subprocess is left running — only this single call fails. A helper that
    /// is merely slow (not wedged) will have its late response discarded by
    /// the reader loop as an unknown id.
    async fn await_reply(
        &self,
        id: u64,
        rx: oneshot::Receiver<Result<serde_json::Value>>,
        timeout: Duration,
        method: &str,
    ) -> Result<serde_json::Value> {
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_recv)) => Err(DesktopError::BridgeFailed("inflight dropped".into())),
            Err(_elapsed) => {
                self.inflight.cancel(id).await;
                Err(DesktopError::BridgeTimeout(format!(
                    "no reply for '{method}' within {}s",
                    timeout.as_secs()
                )))
            }
        }
    }

    /// Send a JSON-RPC request and await the typed response, bounded by an
    /// explicit per-call `timeout`.
    pub async fn call_with_timeout<P, R>(
        &self,
        method: &str,
        params: P,
        timeout: Duration,
    ) -> Result<R>
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
            // stdin write failed — this helper is likely dead. Reset state so the
            // retry below respawns.
            {
                let mut guard = self.state.lock().await;
                *guard = None;
            }
            // Fail ONLY this caller's request, not every concurrent in-flight one.
            // The bridge (and its InflightTable) is shared process-wide (macOS
            // SHARED_BRIDGE), so a single write failure must not collaterally abort
            // sibling RPCs. If the helper is genuinely dead its stdout also closes
            // and the reader loop's EOF path (`fail_all`) drains the rest.
            self.inflight.fail(id, "write stdin failed").await;

            // One retry: re-ensure the helper and send again.
            self.ensure_running().await?;

            // Use a FRESH id for the retry, not the one just drained. The
            // InflightTable is shared across helper generations; the dead
            // helper's reader task may still run `fail_all` on its stdout EOF,
            // which drains every registered id — so re-registering the original
            // id would let that late EOF fail the retry's own oneshot with
            // "helper stdout closed", turning a recoverable retry into a
            // spurious BridgeFailed. A fresh id is invisible to the old reader.
            let retry_id = self.id_seq.fetch_add(1, Ordering::SeqCst);
            let retry_line = encode(&Request {
                jsonrpc: "2.0".into(),
                id: retry_id,
                method: method.into(),
                params: req.params.clone(),
            })?;

            let (tx2, rx2) = oneshot::channel();
            self.inflight.register(retry_id, tx2).await;

            {
                let mut guard = self.state.lock().await;
                let proc = guard.as_mut().ok_or_else(|| {
                    DesktopError::BridgeFailed("bridge not running after retry".into())
                })?;
                proc.stdin
                    .write_all(retry_line.as_bytes())
                    .await
                    .map_err(|e| DesktopError::BridgeFailed(format!("write stdin retry: {e}")))?;
                proc.stdin
                    .flush()
                    .await
                    .map_err(|e| DesktopError::BridgeFailed(format!("flush stdin retry: {e}")))?;
            }

            let raw = self.await_reply(retry_id, rx2, timeout, method).await?;
            return serde_json::from_value(raw)
                .map_err(|e| DesktopError::BridgeFailed(format!("decode result: {e}")));
        }

        let raw = self.await_reply(id, rx, timeout, method).await?;
        serde_json::from_value(raw)
            .map_err(|e| DesktopError::BridgeFailed(format!("decode result: {e}")))
    }

    /// Send the Stage 0 `bridge.handshake` request and return the helper's
    /// version + supported method list. Mostly used at startup + in e2e
    /// tests; individual capabilities do not need to call this first.
    pub async fn handshake(
        &self,
        rust_version: &str,
    ) -> Result<aleph_protocol::desktop_bridge::methods::bridge::HandshakeResult> {
        use aleph_protocol::desktop_bridge::methods::bridge::{
            HandshakeParams, HandshakeResult, METHOD_HANDSHAKE,
        };
        let result: HandshakeResult = self
            .call(
                METHOD_HANDSHAKE,
                HandshakeParams {
                    rust_version: rust_version.into(),
                    protocol_version: BRIDGE_PROTOCOL_VERSION,
                },
            )
            .await?;
        // Negotiate explicitly: a helper speaking a different envelope version
        // would mis-decode params/results silently, so fail loud and early.
        if result.protocol_version != BRIDGE_PROTOCOL_VERSION {
            return Err(DesktopError::BridgeFailed(format!(
                "protocol mismatch: core speaks v{BRIDGE_PROTOCOL_VERSION}, \
                 helper '{}' speaks v{}",
                result.swift_version, result.protocol_version
            )));
        }
        Ok(result)
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

    /// Fake helper that consumes every request but never replies, simulating
    /// a wedged helper that keeps stdout open (so crash recovery never fires).
    fn silent_helper_script() -> &'static str {
        r#"#!/bin/sh
while IFS= read -r line; do
  :
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

    #[test]
    fn map_bridge_error_routes_server_codes_to_typed_variants() {
        let err = |code: i32| RpcError {
            code,
            message: "detail".into(),
            data: None,
        };
        // -32002 must stay distinguishable from a -32601 wiring gap: it means
        // the capability is deliberately absent (e.g. pim.mail.* on macOS).
        assert!(matches!(
            map_bridge_error(err(ERR_NOT_IMPLEMENTED)),
            DesktopError::NotImplemented(_)
        ));
        assert!(matches!(
            map_bridge_error(err(ERR_PLATFORM)),
            DesktopError::PlatformError(_)
        ));
        assert!(matches!(
            map_bridge_error(err(ERR_TIMEOUT)),
            DesktopError::BridgeTimeout(_)
        ));
        assert!(matches!(
            map_bridge_error(err(ERR_BRIDGE_DISABLED)),
            DesktopError::BridgeDisabled(_)
        ));
        // Codes with no typed home fall through to BridgeFailed.
        assert!(matches!(
            map_bridge_error(err(-32601)),
            DesktopError::BridgeFailed(_)
        ));
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
                let v: serde_json::Value =
                    b.call("bridge.ping", serde_json::json!({})).await.unwrap();
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
        tokio::fs::write(&path, body).await.unwrap();
        tokio::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .await
            .unwrap();

        let bridge = SwiftBridge::new(path);
        // First call races a premature exit: either the call fails fast or
        // ensure_running surfaces a spawn error. Either way the marker is
        // written so the second spawn succeeds.
        let _ = bridge
            .call::<_, serde_json::Value>("bridge.ping", serde_json::json!({}))
            .await;
        // Wait past the first backoff rung (1s): the reader observes stdout EOF,
        // resets state, and arms the spawn gate. A respawn before the window
        // elapses is now (correctly) refused with BridgeBackoff, so the sleep
        // must clear the 1s cooldown (plus reader-task scheduling jitter) before
        // the second call.
        tokio::time::sleep(std::time::Duration::from_millis(1_500)).await;

        let v: serde_json::Value = bridge
            .call("bridge.ping", serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(v["pong"], true);
    }

    #[tokio::test]
    async fn call_with_timeout_trips_when_helper_never_replies() {
        let dir = tempfile::tempdir().unwrap();
        let path = install_fake(&dir, silent_helper_script());

        let bridge = SwiftBridge::new(path);
        bridge.ensure_running().await.unwrap();

        let result: Result<serde_json::Value> = bridge
            .call_with_timeout(
                "bridge.ping",
                serde_json::json!({}),
                Duration::from_millis(200),
            )
            .await;

        let err = result.unwrap_err();
        assert!(
            matches!(err, DesktopError::BridgeTimeout(_)),
            "expected BridgeTimeout, got: {err:?}"
        );
        // The timed-out request must not leak in the inflight table.
        assert_eq!(bridge.inflight.len().await, 0);
    }

    #[tokio::test]
    async fn default_call_succeeds_against_fast_helper() {
        // call() now delegates to call_with_timeout(DEFAULT_RPC_TIMEOUT);
        // a fast helper must still resolve well within the default window.
        let dir = tempfile::tempdir().unwrap();
        let path = install_fake(&dir, fake_helper_script());

        let bridge = SwiftBridge::new(path);
        let v: serde_json::Value = bridge
            .call("bridge.ping", serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(v["pong"], true);
    }

    #[tokio::test]
    async fn spawn_failures_back_off_instead_of_hammering() {
        // Non-existent binary guarantees every spawn fails instantly. The
        // backoff gate must space respawns out rather than burning through the
        // restart window in a tight loop (the old behaviour: 6 rapid calls
        // tripped the disable latch in microseconds). The disable threshold
        // itself is covered by the SpawnGate unit tests, which don't need to
        // wait out real backoff windows.
        let bridge = SwiftBridge::new(std::path::PathBuf::from(
            "/tmp/aleph-nonexistent-helper-for-test-0f3d",
        ));

        // First attempt actually tries to spawn → surfaces the spawn error and
        // arms the 1s backoff.
        let first = bridge
            .call::<_, serde_json::Value>("bridge.ping", serde_json::json!({}))
            .await
            .unwrap_err();
        assert!(
            matches!(first, DesktopError::BridgeFailed(_)),
            "first call should report the spawn failure, got: {first:?}"
        );

        // An immediate retry must be refused by the gate (BridgeBackoff), NOT
        // attempt another spawn and NOT trip the permanent disable latch.
        let second = bridge
            .call::<_, serde_json::Value>("bridge.ping", serde_json::json!({}))
            .await
            .unwrap_err();
        assert!(
            matches!(second, DesktopError::BridgeBackoff(_)),
            "rapid retry should back off, got: {second:?}"
        );
    }

    #[tokio::test]
    async fn is_running_reflects_spawn_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = install_fake(&dir, fake_helper_script());

        let bridge = SwiftBridge::new(path);
        assert!(!bridge.is_running(), "fresh bridge must not be running");
        bridge.ensure_running().await.unwrap();
        assert!(
            bridge.is_running(),
            "bridge must report running after ensure_running"
        );
    }
}
