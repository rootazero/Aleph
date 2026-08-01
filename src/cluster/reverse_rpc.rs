//! Reverse RPC: the server initiates a JSON-RPC request with an id toward a
//! connected WS client and awaits the associated response.
//!
//! Requests and responses are distinguished by **structure** (has `method` =
//! request; has `result` / `error` = response), not by id — so reverse RPC ids
//! and the client's own id space can overlap without routing conflicts.

use std::collections::HashMap;
use std::time::Duration;

use crate::sync_primitives::{Arc, Mutex};
use crate::sync_primitives::{AtomicU64, Ordering};

use serde_json::Value;
use tokio::sync::{mpsc, oneshot, Notify};

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse};

/// Association table: reverse RPC request id → the oneshot sender waiting for
/// its response.
///
/// Thread-safe; lock poisoning handled per P7
/// (`unwrap_or_else(|e| e.into_inner())`).
#[derive(Default)]
pub struct PendingInvokes {
    counter: AtomicU64,
    waiters: Mutex<HashMap<String, oneshot::Sender<JsonRpcResponse>>>,
}

impl PendingInvokes {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Allocate a new reverse RPC id and register a waiter.
    /// Returns `(id, receiver)`: the caller places `id` in the outbound request
    /// frame and awaits `receiver`.
    pub(crate) fn register(&self) -> (String, oneshot::Receiver<JsonRpcResponse>) {
        // Relaxed: id uniqueness only; the Mutex insert below provides the
        // cross-thread ordering for the HashMap.
        let n = self.counter.fetch_add(1, Ordering::Relaxed);
        let id = format!("rpc-{n}");
        let (tx, rx) = oneshot::channel();
        self.waiters
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.clone(), tx);
        (id, rx)
    }

    /// Route a response to the caller waiting on that id.
    /// Returns `true` if an entry existed for this id (even if its receiver was
    /// already dropped, e.g. the caller timed out — still counts as a handled
    /// reverse RPC response); `false` means no such id (unknown / already resolved).
    pub fn resolve(&self, id: &Value, response: JsonRpcResponse) -> bool {
        let Some(key) = id.as_str() else {
            return false;
        };
        let sender = self
            .waiters
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(key);
        match sender {
            Some(tx) => {
                // Best-effort delivery: a dropped receiver (caller timed out)
                // still counts as a known id — return true so callers treat the
                // frame as a handled reverse-RPC response, not an unknown frame.
                let _ = tx.send(response);
                true
            }
            None => false,
        }
    }

    /// Drop a single waiter (used for timeout cleanup). Returns whether an entry
    /// was actually removed.
    pub(crate) fn cancel(&self, id: &str) -> bool {
        self.waiters
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id)
            .is_some()
    }

    /// Drop **all** waiters (used for connection disconnect cleanup). Returns the
    /// number of entries cancelled.
    ///
    /// Drains `waiters` and drops every oneshot `Sender` together — every caller
    /// still awaiting in [`ReverseRpcChannel::call`] immediately receives a
    /// `RecvError`, mapped to [`ReverseRpcError::Cancelled`] **returning at once**
    /// instead of waiting out the full `timeout_ms` (≤130s for a node drop).
    /// Matches openclaw `node-registry.unregister()`'s semantics of immediately
    /// rejecting all pending node invokes on disconnect.
    pub fn cancel_all(&self) -> usize {
        let mut waiters = self.waiters.lock().unwrap_or_else(|e| e.into_inner());
        let n = waiters.len();
        waiters.clear();
        n
    }
}

/// Failure reason for a reverse RPC call.
#[derive(Debug, thiserror::Error)]
pub enum ReverseRpcError {
    /// Outbound channel is closed (peer connection dropped).
    #[error("reverse-rpc transport closed")]
    TransportClosed,
    /// Frame was **delivered** to the node but no response arrived within budget
    /// — the node is healthy, just slow. Type-level distinction from
    /// [`OutboundWedged`](Self::OutboundWedged): callers can tell this is
    /// "waiting-for-result timeout" rather than "dead socket". Long commands
    /// (large `timeout_ms`) hitting this branch are normal.
    #[error("reverse-rpc call timed out after {0}ms (no response)")]
    Timeout(u64),
    /// Frame could **not be pushed** onto the outbound queue: the writer is
    /// stuck on `send` (peer TCP stopped draining bytes = slow consumer /
    /// half-open connection), the bounded mpsc is full, and `send().await` got
    /// no capacity within the entire budget. This is a socket-level backpressure
    /// failure, mapping openclaw `rejectSlowNodeSocket`'s `bufferedAmount` signal
    /// — distinct from a "slow node" ([`Timeout`](Self::Timeout)): a
    /// center-side channel carrying a close signal will **actively tear down the
    /// stuck connection** because of this (see
    /// [`ReverseRpcChannel::with_close`]).
    #[error("reverse-rpc outbound wedged after {0}ms (node socket not draining)")]
    OutboundWedged(u64),
    /// Waiter dropped: connection cleanup via [`PendingInvokes::cancel_all`]
    /// cancelled all pending. In-flight `node_invoke` / `node_file` / approval
    /// calls immediately receive this error on node drop (fail-fast) instead of
    /// waiting out the full `timeout_ms`.
    #[error("reverse-rpc call cancelled (node disconnected)")]
    Cancelled,
    /// Serializing the request frame failed (should be impossible in practice).
    #[error("failed to serialize JsonRpcRequest: {0}")]
    Serialize(#[from] serde_json::Error),
}

/// A reverse RPC channel bound to a **single connection**: writes request
/// frames into that connection's outbound mpsc and awaits the associated
/// response through a shared [`PendingInvokes`].
#[derive(Clone)]
pub struct ReverseRpcChannel {
    outbound: mpsc::Sender<String>,
    pending: Arc<PendingInvokes>,
    /// Optional "close connection" signal. Channels carrying this will
    /// `notify_one()` on outbound wedge
    /// ([`ReverseRpcError::OutboundWedged`]), causing the owning connection's
    /// read loop to exit and run full cleanup (deregister + `node.disconnected`
    /// event + `cancel_all` + close socket), after which the node backs off,
    /// reconnects, and self-heals. `None` ([`new`](Self::new)) = pure transport
    /// semantics: wedges only return an error, do not close the connection
    /// (node-side / test usage).
    close: Option<Arc<Notify>>,
}

impl ReverseRpcChannel {
    /// Construct a channel from a connection's outbound sender (creates a fresh
    /// independent pending table). No close signal: outbound wedge only returns
    /// [`ReverseRpcError::OutboundWedged`], does not trigger connection teardown
    /// (node-side outbound channels and unit tests use this — reconnection is
    /// managed by each side's own run loop).
    #[must_use]
    pub fn new(outbound: mpsc::Sender<String>) -> Self {
        Self {
            outbound,
            pending: Arc::new(PendingInvokes::new()),
            close: None,
        }
    }

    /// Like [`new`](Self::new), but binds a "close connection" signal (one per
    /// connection on the center side). On outbound wedge,
    /// [`call`](Self::call) returns [`OutboundWedged`](ReverseRpcError::OutboundWedged)
    /// and also `notify_one()` on this signal, causing the connection's read
    /// loop to exit and run full cleanup — removing the half-open / slow-consumer
    /// connection from the fleet (maps openclaw `rejectSlowNodeSocket`). The
    /// idle-watchdog only monitors inbound activity and never fires for a
    /// half-open write-wedge where center→node writes are stuck but node→center
    /// reads are alive, so this active teardown is the only path to reap such
    /// zombie connections.
    #[must_use]
    pub fn with_close(outbound: mpsc::Sender<String>, close: Arc<Notify>) -> Self {
        Self {
            outbound,
            pending: Arc::new(PendingInvokes::new()),
            close: Some(close),
        }
    }

    /// Get the shared pending table. The connection's inbound loop uses this to
    /// `resolve` response frames back to the caller.
    #[must_use]
    pub fn pending(&self) -> Arc<PendingInvokes> {
        self.pending.clone()
    }

    /// Ask the owning connection to tear itself down, if this channel carries a
    /// close signal ([`with_close`](Self::with_close)); a no-op otherwise
    /// (node-side channels / tests).
    ///
    /// Two producers fire this, for the same reason — the connection must go
    /// away **now**, not at the next idle-watchdog expiry:
    /// * [`call`](Self::call) on an outbound wedge (slow consumer, see
    ///   [`OutboundWedged`](ReverseRpcError::OutboundWedged));
    /// * [`NodeRegistry::forget`](crate::cluster::NodeRegistry::forget) on an
    ///   operator deregister — evicting the session from the registry only stops
    ///   *new* dispatches; without this the revoked node keeps its socket (and
    ///   with it the still-live `node.approval.request` path back to the
    ///   operator) until the ≤90s inbound watchdog fires.
    ///
    /// `notify_one` stores a permit when nobody is waiting yet, so the
    /// connection's `select!` arm cannot miss the wakeup.
    pub fn close_connection(&self) {
        if let Some(close) = &self.close {
            close.notify_one();
        }
    }

    /// Initiate a reverse RPC request on the connection and await the response.
    ///
    /// `timeout_ms` is the budget for the **entire call**, covering both "push
    /// the frame onto the outbound queue" and "wait for the response".
    ///
    /// The outbound is a **bounded** mpsc (drained by the connection's writer
    /// task). If the peer TCP stops draining bytes (slow consumer / half-open
    /// connection), the writer is stuck on `send`, the queue fills up, and
    /// `outbound.send().await` would **block indefinitely** — the old impl had no
    /// timeout here, silently voiding the `timeout_ms` contract, causing callers
    /// (`node_invoke` / approval) to hang forever. Now enqueue is also
    /// budgeted; a full queue at deadline is
    /// [`OutboundWedged`](ReverseRpcError::OutboundWedged) (type-level distinct
    /// from "waiting-for-response timeout" =
    /// [`Timeout`](ReverseRpcError::Timeout)); if this channel was created via
    /// [`with_close`](Self::with_close), it **actively tears down the stuck
    /// connection**, removing it from the fleet and letting the node back off
    /// and reconnect — otherwise a half-open write wedge permanently occupies an
    /// online slot in the registry.
    pub async fn call(
        &self,
        method: &str,
        params: Value,
        timeout_ms: u64,
    ) -> Result<JsonRpcResponse, ReverseRpcError> {
        let (id, rx) = self.pending.register();
        let req = JsonRpcRequest::with_id(method, Some(params), Value::String(id.clone()));
        let frame = serde_json::to_string(&req)?;

        let budget = Duration::from_millis(timeout_ms);
        let deadline = tokio::time::Instant::now() + budget;

        match tokio::time::timeout_at(deadline, self.outbound.send(frame)).await {
            // Receiver gone: the connection's writer task is finished.
            Ok(Err(_)) => {
                self.pending.cancel(&id);
                return Err(ReverseRpcError::TransportClosed);
            }
            // Never got the frame queued within the budget — a wedged peer
            // (writer stuck on a socket the peer stopped draining). Distinct from
            // a slow *response*: the frame did not even reach the wire. Ask the
            // owning connection to tear itself down so the zombie is reaped now
            // rather than occupying a registry slot until (or past) the inbound
            // idle-watchdog, which never fires for a half-open write-wedge.
            Err(_) => {
                self.pending.cancel(&id);
                self.close_connection();
                return Err(ReverseRpcError::OutboundWedged(timeout_ms));
            }
            Ok(Ok(())) => {}
        }

        match tokio::time::timeout_at(deadline, rx).await {
            Ok(Ok(resp)) => Ok(resp),
            Ok(Err(_)) => Err(ReverseRpcError::Cancelled), // sender dropped
            Err(_) => {
                self.pending.cancel(&id);
                Err(ReverseRpcError::Timeout(timeout_ms))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::protocol::JsonRpcResponse;
    use serde_json::json;

    #[tokio::test]
    async fn register_then_resolve_delivers_response() {
        let pending = PendingInvokes::new();
        let (id, rx) = pending.register();

        // id is the string form of the reverse RPC correlation key
        assert!(id.starts_with("rpc-"));

        let resp = JsonRpcResponse::success(Some(json!(id)), json!({"ok": true}));
        let routed = pending.resolve(&json!(id), resp);
        assert!(routed, "resolve should find the pending entry");

        let got = rx.await.expect("sender should not be dropped");
        assert!(got.is_success());
    }

    #[tokio::test]
    async fn resolve_unknown_id_returns_false() {
        let pending = PendingInvokes::new();
        let resp = JsonRpcResponse::success(Some(json!("rpc-999")), json!(null));
        assert!(!pending.resolve(&json!("rpc-999"), resp));
    }

    #[tokio::test]
    async fn channel_call_sends_framed_request_and_returns_response() {
        // Outbound receiver simulates the "write half" of a connection: reads
        // one frame, treats it as the request, extracts the id, constructs a
        // response, and feeds it back via resolve.
        let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<String>(8);
        let channel = ReverseRpcChannel::new(out_tx);
        let pending = channel.pending();

        // Play "client" in the background: receive outbound frame → reply with a
        // success response.
        let bg_pending = pending.clone();
        tokio::spawn(async move {
            let frame = out_rx.recv().await.expect("a frame should be sent");
            let req: serde_json::Value = serde_json::from_str(&frame).unwrap();
            assert_eq!(req["method"], "tool.call");
            let id = req["id"].clone();
            let resp = crate::gateway::protocol::JsonRpcResponse::success(
                Some(id.clone()),
                json!({"echoed": req["params"]["tool"]}),
            );
            bg_pending.resolve(&id, resp);
        });

        let resp = channel
            .call("tool.call", json!({"tool": "bash"}), 1_000)
            .await
            .expect("call should resolve");
        assert!(resp.is_success());
        assert_eq!(resp.result.unwrap()["echoed"], "bash");
    }

    #[tokio::test]
    async fn call_times_out_when_no_response() {
        // Outbound receiver stays alive but never responds → must timeout
        // (not hang forever).
        let (out_tx, _out_rx_keepalive) = tokio::sync::mpsc::channel::<String>(8);
        let channel = ReverseRpcChannel::new(out_tx);

        let err = channel
            .call("tool.call", json!({}), 50)
            .await
            .expect_err("must time out");
        assert!(matches!(err, ReverseRpcError::Timeout(50)));
    }

    #[tokio::test]
    async fn cancel_all_drops_every_waiter_so_receivers_resolve_err() {
        // Simulate connection disconnect cleanup: cancel_all drains every
        // waiter → each receiver resolves immediately with RecvError (call()
        // maps it to Cancelled).
        let pending = PendingInvokes::new();
        let (_id1, rx1) = pending.register();
        let (_id2, rx2) = pending.register();

        assert_eq!(
            pending.cancel_all(),
            2,
            "should report both waiters cancelled"
        );
        assert!(rx1.await.is_err(), "sender dropped → receiver errors");
        assert!(rx2.await.is_err(), "sender dropped → receiver errors");

        // Idempotent: clearing again leaves nothing.
        assert_eq!(pending.cancel_all(), 0);
    }

    #[tokio::test]
    async fn inflight_call_returns_cancelled_after_cancel_all() {
        // Outbound stays alive but never responds; cancel_all must cause the
        // in-flight call to immediately return Cancelled rather than waiting
        // out the full timeout.
        let (out_tx, _out_rx_keepalive) = tokio::sync::mpsc::channel::<String>(8);
        let channel = ReverseRpcChannel::new(out_tx);
        let pending = channel.pending();

        let call = tokio::spawn(async move {
            // Large timeout: if cancel_all isn't wired up correctly, this call
            // would hang until the test itself times out.
            channel.call("tool.call", json!({}), 60_000).await
        });

        // Spin-wait for the call to register its waiter (waiters non-empty after
        // register), then cancel_all. More robust than a fixed sleep, avoids
        // subscribe-before-publish timing races.
        loop {
            if pending.cancel_all() > 0 {
                break;
            }
            tokio::task::yield_now().await;
        }

        let err = call
            .await
            .expect("task joins")
            .expect_err("must be cancelled");
        assert!(matches!(err, ReverseRpcError::Cancelled));
    }

    #[tokio::test]
    async fn call_times_out_instead_of_hanging_on_a_wedged_outbound_queue() {
        // A peer whose socket stopped draining: the writer task never pulls from
        // the queue, so it fills up. `send().await` would block forever without a
        // budget — the whole point of `timeout_ms` is that it cannot.
        let (out_tx, _out_rx_never_drained) = tokio::sync::mpsc::channel::<String>(1);
        // Fill the single slot so the next send must wait for capacity.
        out_tx.send("stale frame".to_string()).await.unwrap();

        let channel = ReverseRpcChannel::new(out_tx);
        let err = channel
            .call("tool.call", json!({}), 50)
            .await
            .expect_err("a wedged queue must time out, not hang");
        // Enqueue-wedge is typed distinctly from a slow *response* (Timeout): the
        // frame never reached the wire. A plain `new` channel just reports it.
        assert!(
            matches!(err, ReverseRpcError::OutboundWedged(50)),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn wedged_outbound_notifies_close_signal_on_with_close_channel() {
        // A with_close channel over a wedged queue must, in addition to returning
        // OutboundWedged, fire its close signal so the owning connection tears
        // down (maps openclaw rejectSlowNodeSocket). A slow *response* would not.
        let (out_tx, _never_drained) = tokio::sync::mpsc::channel::<String>(1);
        out_tx.send("stale frame".to_string()).await.unwrap();
        let close = Arc::new(Notify::new());
        let channel = ReverseRpcChannel::with_close(out_tx, close.clone());

        let err = channel
            .call("tool.call", json!({}), 50)
            .await
            .expect_err("wedged queue must error");
        assert!(
            matches!(err, ReverseRpcError::OutboundWedged(50)),
            "{err:?}"
        );
        // notify_one() before the waiter still leaves a stored permit, so this
        // resolves immediately — no lost wakeup.
        tokio::time::timeout(Duration::from_secs(1), close.notified())
            .await
            .expect("close signal must have been fired by the wedge");
    }

    #[tokio::test]
    async fn slow_response_does_not_fire_close_signal() {
        // Frame enqueues fine (receiver alive, capacity 8) but no response comes:
        // that's a healthy-but-slow node (Timeout), NOT a wedge — the connection
        // must NOT be torn down, so the close signal must stay unfired.
        let (out_tx, _keepalive) = tokio::sync::mpsc::channel::<String>(8);
        let close = Arc::new(Notify::new());
        let channel = ReverseRpcChannel::with_close(out_tx, close.clone());

        let err = channel
            .call("tool.call", json!({}), 50)
            .await
            .expect_err("must time out waiting for response");
        assert!(matches!(err, ReverseRpcError::Timeout(50)), "{err:?}");
        // No permit stored → notified() is still pending → times out here.
        assert!(
            tokio::time::timeout(Duration::from_millis(100), close.notified())
                .await
                .is_err(),
            "a slow response must not tear down the connection"
        );
    }

    #[tokio::test]
    async fn call_fails_when_transport_closed() {
        let (out_tx, out_rx) = tokio::sync::mpsc::channel::<String>(8);
        drop(out_rx); // Immediately close outbound → send fails
        let channel = ReverseRpcChannel::new(out_tx);

        let err = channel
            .call("tool.call", json!({}), 1_000)
            .await
            .expect_err("must fail closed");
        assert!(matches!(err, ReverseRpcError::TransportClosed));
    }
}
