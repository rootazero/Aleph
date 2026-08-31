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
use crate::tools::budget::REVERSE_RPC_MAX_TIMEOUT_MS;

/// Association table: reverse RPC request id → the oneshot sender waiting for
/// its response.
///
/// Thread-safe; lock poisoning handled per P7
/// (`unwrap_or_else(|e| e.into_inner())`).
#[derive(Debug, Default)]
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
    ///
    /// If no waiter exists for this id (unknown id, the receiver was already
    /// dropped, or the waiter was cancelled), the response is silently dropped
    /// and a `warn!` is logged. The previous `bool` return on this method was
    /// severed: the sole production caller (`gateway/server/handler.rs:699`)
    /// discarded the result, so a misrouted response could be swallowed
    /// without any log line. Moving the diagnostic into the callee makes the
    /// signal observable without requiring callers to opt in.
    pub fn resolve(&self, id: &Value, response: JsonRpcResponse) {
        let Some(key) = id.as_str() else {
            tracing::warn!("reverse-rpc resolve received non-string id; dropping response");
            return;
        };
        let sender = self
            .waiters
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(key);
        match sender {
            Some(tx) => {
                // Best-effort delivery: a dropped receiver (caller timed out)
                // is still a known-id response, not an unknown frame.
                let _ = tx.send(response);
            }
            None => {
                tracing::warn!(
                    id = %key,
                    "reverse-rpc resolve received response for unknown id \
                     (already resolved, cancelled, or never registered)"
                );
            }
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

/// Cap on how long `ReverseRpcChannel::call` may spend pushing the request
/// frame onto the outbound mpsc before it declares the peer a slow consumer
/// (see B3-01). Once the frame is enqueued, the call switches to the
/// response-wait budget (the remainder of the caller's `timeout_ms`).
/// Splitting the two keeps a peer that received and started executing the
/// frame from being told "timed out" by the center.
const OUTBOUND_PUSH_BUDGET_MS: u64 = 500;

/// A reverse RPC channel bound to a **single connection**: writes request
/// frames into that connection's outbound mpsc and awaits the associated
/// response through a shared [`PendingInvokes`].
#[derive(Clone, Debug)]
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
    /// the frame onto the outbound queue" and "wait for the response". It is
    /// clamped to [`REVERSE_RPC_MAX_TIMEOUT_MS`] — a caller may ask for less,
    /// never for more.
    ///
    /// The registered waiter is removed on **every** exit, the dropped-future
    /// one included (see `WaiterGuard` below the impl).
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
        // See `REVERSE_RPC_MAX_TIMEOUT_MS`: an unbounded caller-supplied window
        // is one the harness's per-tool clock preempts, discarding whatever the
        // node had already produced in favour of an opaque overrun.
        let timeout_ms = timeout_ms.min(REVERSE_RPC_MAX_TIMEOUT_MS);
        let (id, rx) = self.pending.register();
        // Registering a waiter and removing it are two halves of one action,
        // and until now only the *error* paths performed the second half. A
        // DROPPED future performed neither: `RegistryToolAdapter::execute`
        // drops exactly this future whenever the harness cancels a tool call,
        // so every cancelled `node_invoke` leaked one `waiters` entry until the
        // connection tore down. The guard makes cancellation a defined state
        // rather than an assumption — which is also the precondition any
        // race-and-drop wake edge (`SteerWatch::race`) would rest on.
        let cleanup = WaiterGuard {
            pending: Arc::clone(&self.pending),
            id: Some(id.clone()),
        };
        let req = JsonRpcRequest::with_id(method, Some(params), Value::String(id.clone()));
        // (B3-03) A serialization failure must cancel the registered waiter
        // before bubbling up — otherwise the id lingers in `waiters` until
        // `cancel_all` on disconnect. `WaiterGuard` now does that for this and
        // every other non-terminal exit, including the one no `match` arm can
        // cover: the future being dropped.
        let frame = match serde_json::to_string(&req) {
            Ok(f) => f,
            Err(e) => return Err(ReverseRpcError::Serialize(e)),
        };

        let budget = Duration::from_millis(timeout_ms);
        // (B3-01) Split the timeout into an outbound-enqueue sub-budget and a
        // response sub-budget. Previously the SAME `deadline` was reused for
        // both phases — if the outbound push consumed most of the budget, the
        // response wait could time out *after* the peer had already received
        // and started executing the frame, leaving the peer to execute a
        // command the center has now told its caller "timed out".
        //
        // `OUTBOUND_PUSH_BUDGET_MS` caps the enqueue half regardless of the
        // caller's `timeout_ms`; the response_deadline is set at call start
        // to `now + timeout_ms` (the *full* caller budget), independent of
        // how much outbound actually consumed. The total call wall-time is
        // therefore bounded by `timeout_ms` for outbound plus a separate
        // `timeout_ms` for the response wait, not by a single shared
        // budget. This is a deliberate split: a slow enqueue is already an
        // error (`OutboundWedged`), so the response half does not need to
        // "subtract" anything from a shared clock.
        let outbound_budget = Duration::from_millis(timeout_ms.min(OUTBOUND_PUSH_BUDGET_MS));
        let outbound_deadline = tokio::time::Instant::now() + outbound_budget;
        let response_deadline = tokio::time::Instant::now() + budget;

        match tokio::time::timeout_at(outbound_deadline, self.outbound.send(frame)).await {
            // Receiver gone: the connection's writer task is finished.
            Ok(Err(_)) => return Err(ReverseRpcError::TransportClosed),
            // Never got the frame queued within the budget — a wedged peer
            // (writer stuck on a socket the peer stopped draining). Distinct from
            // a slow *response*: the frame did not even reach the wire. Ask the
            // owning connection to tear itself down so the zombie is reaped now
            // rather than occupying a registry slot until (or past) the inbound
            // idle-watchdog, which never fires for a half-open write-wedge.
            Err(_) => {
                self.close_connection();
                return Err(ReverseRpcError::OutboundWedged(timeout_ms));
            }
            Ok(Ok(())) => {}
        }

        match tokio::time::timeout_at(response_deadline, rx).await {
            // Resolved: `PendingInvokes::resolve` already removed the entry, so
            // disarm rather than cancel an id that may since have been reused.
            Ok(Ok(resp)) => {
                cleanup.disarm();
                Ok(resp)
            }
            // Sender dropped ⇒ the entry was removed by `resolve` or
            // `cancel_all`; nothing left to clean up.
            Ok(Err(_)) => {
                cleanup.disarm();
                Err(ReverseRpcError::Cancelled)
            }
            Err(_) => Err(ReverseRpcError::Timeout(timeout_ms)),
        }
    }
}

/// Removes a registered waiter unless the call reached a terminal state that
/// already removed it.
///
/// The point is the path with no `match` arm at all: a future dropped
/// mid-flight. See the comment at its construction site in
/// [`ReverseRpcChannel::call`].
struct WaiterGuard {
    pending: Arc<PendingInvokes>,
    id: Option<String>,
}

impl WaiterGuard {
    /// The waiter is already gone (resolved, or dropped by `cancel_all`);
    /// cancelling now could remove a *different* call's entry if the id space
    /// ever wrapped.
    fn disarm(mut self) {
        self.id = None;
    }
}

impl Drop for WaiterGuard {
    fn drop(&mut self) {
        if let Some(id) = self.id.take() {
            self.pending.cancel(&id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::protocol::JsonRpcResponse;
    use serde_json::json;

    /// How many waiters the table is holding. Only the guard tests need this,
    /// and only to observe leakage.
    fn waiter_count(pending: &PendingInvokes) -> usize {
        pending
            .waiters
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }

    /// Dropping the `call` future must not leak its waiter.
    ///
    /// `RegistryToolAdapter::execute` drops exactly this future when the
    /// harness cancels a tool call, and every error arm of `call` cleaned up
    /// while the dropped-future path — which has no arm at all — did not. The
    /// entry survived until `cancel_all` at connection teardown, so a session
    /// that cancelled N node invokes carried N dead entries.
    #[tokio::test]
    async fn dropping_the_call_future_releases_its_waiter() {
        // A live receiver that never replies, so `call` parks on the response.
        let (out_tx, _out_rx) = mpsc::channel::<String>(8);
        let channel = ReverseRpcChannel::new(out_tx);
        let pending = channel.pending();

        {
            let fut = channel.call("tool.call", json!({}), 60_000);
            tokio::pin!(fut);
            // Poll once so the waiter is registered and the future is parked.
            tokio::select! {
                _ = &mut fut => panic!("the peer never replies; this must not resolve"),
                () = tokio::time::sleep(Duration::from_millis(20)) => {}
            }
            assert_eq!(waiter_count(&pending), 1, "the call must have registered");
            // `fut` drops here — the cancellation path.
        }

        assert_eq!(
            waiter_count(&pending),
            0,
            "a dropped call future left its waiter behind; every harness-cancelled \
             node_invoke leaks one entry until the connection tears down"
        );
    }

    /// A resolved call leaves nothing behind either — the guard must not be
    /// the only thing keeping the table clean, and it must not cancel an id
    /// that `resolve` already removed.
    #[tokio::test]
    async fn a_resolved_call_leaves_no_waiter() {
        let (out_tx, mut out_rx) = mpsc::channel::<String>(8);
        let channel = ReverseRpcChannel::new(out_tx);
        let pending = channel.pending();
        let replier = Arc::clone(&pending);

        tokio::spawn(async move {
            let frame = out_rx.recv().await.expect("request frame");
            let req: Value = serde_json::from_str(&frame).unwrap();
            let id = req["id"].clone();
            replier.resolve(
                &id,
                JsonRpcResponse::success(Some(id.clone()), json!({"ok": true})),
            );
        });

        let resp = channel
            .call("tool.call", json!({}), 5_000)
            .await
            .expect("the peer replied");
        assert!(resp.is_success());
        assert_eq!(waiter_count(&pending), 0);
    }

    /// A caller may ask for less than the ceiling, never for more.
    ///
    /// Observed through the error the timeout reports, which carries the
    /// budget actually used: `ReverseRpcError::Timeout(ms)`. Asserted against
    /// the constant rather than a literal, so the guard survives the value
    /// moving.
    #[tokio::test(start_paused = true)]
    async fn a_caller_cannot_ask_for_more_than_the_ceiling() {
        let (out_tx, _out_rx) = mpsc::channel::<String>(8);
        let channel = ReverseRpcChannel::new(out_tx);

        let err = channel
            .call("tool.call", json!({}), REVERSE_RPC_MAX_TIMEOUT_MS * 4)
            .await
            .expect_err("the peer never replies");
        match err {
            ReverseRpcError::Timeout(ms) => assert_eq!(
                ms, REVERSE_RPC_MAX_TIMEOUT_MS,
                "an unbounded caller window is one the harness's per-tool clock \
                 preempts, discarding the node's partial work"
            ),
            other => panic!("expected a response timeout, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn register_then_resolve_delivers_response() {
        let pending = PendingInvokes::new();
        let (id, rx) = pending.register();

        // id is the string form of the reverse RPC correlation key
        assert!(id.starts_with("rpc-"));

        let resp = JsonRpcResponse::success(Some(json!(id)), json!({"ok": true}));
        pending.resolve(&json!(id), resp);

        let got = rx.await.expect("sender should not be dropped");
        assert!(got.is_success());
    }

    #[tokio::test]
    async fn resolve_unknown_id_drops_response_silently() {
        // Unknown id: response is dropped (no waiter to deliver to) but no
        // panic, and no observable side effect on the PendingInvokes state.
        let pending = PendingInvokes::new();
        let resp = JsonRpcResponse::success(Some(json!("rpc-999")), json!(null));
        pending.resolve(&json!("rpc-999"), resp);
        // No waiter was registered, so cancel_all returns 0 — the resolve
        // did not synthesize a phantom entry.
        assert_eq!(pending.cancel_all(), 0);
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
