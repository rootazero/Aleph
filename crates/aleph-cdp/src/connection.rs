//! One websocket, many in-flight commands.
//!
//! Shape: a writer task owns the sink, a reader task owns the source, and both talk to a `Shared`
//! that holds the pending-call table, the event fan-out and the close watch. Neither task holds an
//! `Arc<Inner>`; if they did, the socket would keep the connection alive after the last handle was
//! dropped and nothing would ever close the file descriptor.
//!
//! Three rules the rest of the system leans on:
//! 1. A reply is matched by `id`, never by arrival order. Engines answer out of order as a matter
//!    of course — obscura's global barrier releases a whole batch at once (evidence M12b).
//! 2. A command that does not come back inside its budget returns `Timeout{method, waited}` and is
//!    NOT retried here. When the barrier lifts, a retry would only queue behind the original.
//! 3. A dead socket fails every outstanding call with `Disconnected`. Nothing is left hanging, and
//!    nothing is answered with a default (判据 §8).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio_tungstenite::tungstenite::Message;

use crate::error::{CdpError, CloseReason, Result};
use crate::events::{CdpEvent, EVENT_CHANNEL_CAPACITY};
use crate::ids::SessionId;

/// Per-command budget when the caller does not name one.
///
/// 30s, and it MUST stay under obscura's own 60s guillotine (spec §3.4.2): if ours were the larger
/// number, the engine would kill the page before our wait expired and the model would be told the
/// wrong thing about what happened.
pub const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Debug)]
pub struct ConnectOptions {
    pub command_timeout: Duration,
}

impl Default for ConnectOptions {
    fn default() -> Self {
        Self {
            command_timeout: DEFAULT_COMMAND_TIMEOUT,
        }
    }
}

/// Poisoned locks are recovered, never unwrapped (P7): a panic in one task must not turn every
/// later call into a second panic that hides the first.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The reason reported when we must say `Disconnected` but `fail_all` has not recorded a cause
/// yet — e.g. the writer's own channel died on a half-open socket before the reader noticed, or
/// before `Inner::drop` ran. This must never default to `LocalClose`: that variant is a specific
/// claim that Aleph closed the socket, and asserting it here would blame us for a peer we do not
/// yet know anything about — a claim the engine-failure diagnosis downstream would then read as
/// fact. `TransportError` says plainly that the cause is unknown instead of asserting agency we
/// do not have.
fn unknown_close_reason() -> CloseReason {
    CloseReason::TransportError(
        "close reason unknown: the writer ended before the reader recorded a cause".to_string(),
    )
}

struct Pending {
    /// Kept so the error can name the verb. A `Timeout` that does not say which method is stuck
    /// tells the model nothing it can act on.
    method: String,
    tx: oneshot::Sender<Result<Value>>,
}

pub(crate) struct Shared {
    pending: Mutex<HashMap<u64, Pending>>,
    pub(crate) events_tx: broadcast::Sender<Arc<CdpEvent>>,
    closed: watch::Sender<Option<CloseReason>>,
}

impl Shared {
    /// Record why the socket died and hand that to every outstanding call.
    ///
    /// The FIRST reason wins. A peer that hung up followed by our own handle drop must still read
    /// `PeerClosed`; overwriting would turn "the browser died" into "we closed it" and lose the
    /// only fact worth reporting.
    fn fail_all(&self, reason: CloseReason) {
        // `send_if_modified`, never plain `send`: `watch::Sender::send` does not write the value
        // at all when there are zero active receivers (only `send_if_modified`/`send_replace`
        // do), and at the moment a connection dies nothing may have called `closed()` yet. `send`
        // here would silently drop the reason, `closed()` would keep handing back `None`, and
        // `None` reads downstream as "not closed" — a dead connection reported as live. `alive()`
        // callers (Task 9) act on exactly that value, so a fail-open here is not cosmetic.
        self.closed.send_if_modified(|slot| {
            if slot.is_none() {
                *slot = Some(reason.clone());
                true
            } else {
                false
            }
        });
        let actual = self.closed.borrow().clone().unwrap_or(reason);
        let drained: Vec<Pending> = lock(&self.pending).drain().map(|(_, p)| p).collect();
        // The only line in this crate that logs. Why a socket died, and how many calls were in
        // flight when it did, cannot be reconstructed from any return value: by the time a caller
        // sees `Disconnected`, this has already answered every one of them. The methods those
        // calls named are the useful part — "the engine went away during DOM.getDocument" is a
        // different operator problem from "during Page.navigate".
        tracing::debug!(
            reason = ?actual,
            pending = drained.len(),
            methods = ?drained.iter().map(|p| p.method.as_str()).collect::<Vec<_>>(),
            "cdp socket closed",
        );
        for p in drained {
            let _ = p.tx.send(Err(CdpError::Disconnected(actual.clone())));
        }
    }

    fn reason(&self) -> Option<CloseReason> {
        self.closed.borrow().clone()
    }

    /// Call immediately after inserting a pending entry, passing that entry's own id.
    ///
    /// Closes a sub-microsecond TOCTOU: `call_with_timeout` checks `reason()` before inserting
    /// into `pending` so it can fail fast on an already-dead socket, but nothing holds a lock
    /// across that check and the insert. If `fail_all` runs — and finishes draining `pending` —
    /// in that exact window, it drains a table that does not contain this call's entry yet
    /// (the insert hasn't happened), so this entry is never touched by that drain. Nothing else
    /// will ever resolve it: `fail_all` only runs once more, from `Inner::drop`, which may not
    /// happen for as long as this `CdpConnection` handle lives. Without this recheck the call
    /// would sit out its entire timeout budget and report `Timeout` — while `closed()` already
    /// knows, correctly, that the connection is `Disconnected`. That is exactly the wrong-fact
    /// failure mode R63 exists to prevent, reached by interleaving instead of by deleting a guard.
    ///
    /// Returns `Some(reason)` when the race was hit — the entry has already been removed, by this
    /// call, so the caller must not touch `pending` again for `id` and must report `Disconnected`
    /// immediately rather than proceed to send the frame or await a reply that will never come
    /// from `fail_all`'s drain (it already ran) nor from the peer (the caller may not even still
    /// be connected to receive it).
    fn recheck_after_insert(&self, id: u64) -> Option<CloseReason> {
        let reason = self.reason()?;
        lock(&self.pending).remove(&id);
        Some(reason)
    }
}

#[cfg(test)]
mod shared_tests {
    use super::*;

    fn new_shared() -> Shared {
        let (events_tx, _rx) = broadcast::channel(1);
        let (closed_tx, _initial) = watch::channel(None);
        Shared {
            pending: Mutex::new(HashMap::new()),
            events_tx,
            closed: closed_tx,
        }
    }

    /// Proves `recheck_after_insert` is load-bearing without reproducing the real race's exact
    /// (sub-microsecond, multi-thread-runtime-only) timing: it drives the same sequence of state
    /// transitions the race produces — `fail_all` running and draining a table that does not yet
    /// contain this entry, because the entry is inserted only afterward, exactly as it would be
    /// if `call_with_timeout`'s insert landed just after `fail_all`'s drain — and checks that the
    /// recheck catches what the drain could not have.
    #[test]
    fn recheck_after_insert_catches_a_reason_set_before_the_entry_existed() {
        let shared = new_shared();

        // `fail_all` runs (and drains) while the table is still empty — standing in for the
        // window between `call_with_timeout`'s initial guard and its insert.
        shared.fail_all(CloseReason::PeerClosed);

        // The insert that, in the real race, lands just too late to be part of that drain.
        let (tx, rx) = oneshot::channel();
        lock(&shared.pending).insert(
            7,
            Pending {
                method: "Late.op".to_string(),
                tx,
            },
        );

        let caught = shared.recheck_after_insert(7);
        assert_eq!(
            caught,
            Some(CloseReason::PeerClosed),
            "the recheck must surface the reason that was already recorded before this entry existed"
        );
        assert!(
            lock(&shared.pending).get(&7).is_none(),
            "the recheck must remove its own entry — nothing else will, since fail_all already ran"
        );
        // Without the recheck, this oneshot has no other sender left to resolve it: `fail_all`
        // already drained (and cannot see this entry to drain again), so the receiver would sit
        // forever. Dropping it here only stands in for the caller reporting `Disconnected`
        // immediately instead of awaiting it.
        drop(rx);
    }

    /// The recheck must be a true no-op on a live connection: it must not remove an entry, or
    /// report a reason, that was never set.
    #[test]
    fn recheck_after_insert_does_nothing_on_a_live_connection() {
        let shared = new_shared();
        let (tx, _rx) = oneshot::channel();
        lock(&shared.pending).insert(
            1,
            Pending {
                method: "Live.op".to_string(),
                tx,
            },
        );

        assert_eq!(shared.recheck_after_insert(1), None);
        assert!(
            lock(&shared.pending).get(&1).is_some(),
            "a live connection's entry must survive the recheck untouched"
        );
    }
}

struct Inner {
    shared: Arc<Shared>,
    outgoing: mpsc::UnboundedSender<Message>,
    next_id: AtomicU64,
    command_timeout: Duration,
    shutdown: watch::Sender<bool>,
    /// `Some` only on a connection built by the testkit-only
    /// `connect_widening_the_guard_insert_race`. Widens the otherwise sub-microsecond window in
    /// `call_with_timeout` between its disconnected-reason guard and the pending-map insert, so a
    /// race that needs real concurrency to land in production can be landed deterministically in
    /// a test. Ordinary `connect` always leaves this `None`, so this costs one `Option` check —
    /// never a sleep — on every real call.
    race_widen: Option<Duration>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        // The reader is parked on `src.next()` and would sit there forever if the peer never spoke
        // again, so it is nudged explicitly rather than left to notice a dropped channel.
        let _ = self.shutdown.send(true);
        self.shared.fail_all(CloseReason::LocalClose);
    }
}

/// A live CDP connection. Cheap to clone — every clone shares one socket.
#[derive(Clone)]
pub struct CdpConnection {
    inner: Arc<Inner>,
}

// Manual, not derived: `Inner` holds an `mpsc::UnboundedSender<Message>` and a `watch::Sender<bool>`
// that do not implement `Debug`, but `Result<CdpConnection, _>::expect_err` (used by
// `connecting_to_a_dead_port_is_a_transport_error_naming_the_url`) requires `CdpConnection: Debug`
// regardless of which arm is actually printed. A bare type name would satisfy the compiler and
// tell a reader nothing at the one place this actually gets read — a failed test assertion — so
// this prints the one fact worth having: whether the socket is still live, and why not.
impl std::fmt::Debug for CdpConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CdpConnection")
            .field("closed", &self.inner.shared.reason())
            .finish_non_exhaustive()
    }
}

impl CdpConnection {
    pub async fn connect(ws_url: &str, opts: ConnectOptions) -> Result<CdpConnection> {
        Self::connect_inner(ws_url, opts, None).await
    }

    /// Test-only. Otherwise identical to [`connect`](Self::connect), except the connection it
    /// returns widens the guard→insert race window in `call_with_timeout` by `widen` — see
    /// [`Shared::recheck_after_insert`]'s doc for what that window is and why it matters. Exists
    /// because the real window is a sub-microsecond, multi-thread-runtime-only interleaving that
    /// cannot be relied on to land in an ordinary test; this makes it land every time, scoped to
    /// only the one connection a test explicitly opts into, so it cannot affect any other test
    /// running concurrently in the same binary.
    #[cfg(feature = "testkit")]
    pub async fn connect_widening_the_guard_insert_race(
        ws_url: &str,
        opts: ConnectOptions,
        widen: Duration,
    ) -> Result<CdpConnection> {
        Self::connect_inner(ws_url, opts, Some(widen)).await
    }

    /// Test-only. The number of calls currently awaiting a reply. Exists to prove that a call
    /// which times out (or is otherwise resolved) does not leave a dead entry in `pending`
    /// behind it — see `call_with_timeout`'s `Err(_elapsed)` arm.
    #[cfg(feature = "testkit")]
    pub fn pending_len(&self) -> usize {
        lock(&self.inner.shared.pending).len()
    }

    async fn connect_inner(
        ws_url: &str,
        opts: ConnectOptions,
        race_widen: Option<Duration>,
    ) -> Result<CdpConnection> {
        let (ws, _resp) = tokio_tungstenite::connect_async(ws_url)
            .await
            // The url is in the message on purpose: "connection refused" on its own does not say
            // which engine was missing, and that is the whole content of the operator's problem.
            .map_err(|e| CdpError::Transport(format!("connect {ws_url}: {e}")))?;
        let (mut sink, mut src) = ws.split();

        let (events_tx, _initial_rx) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        let (closed_tx, _initial_closed) = watch::channel(None);
        let shared = Arc::new(Shared {
            pending: Mutex::new(HashMap::new()),
            events_tx,
            closed: closed_tx,
        });
        let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Message>();
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);

        tokio::spawn(async move {
            while let Some(m) = out_rx.recv().await {
                let closing = matches!(m, Message::Close(_));
                if sink.send(m).await.is_err() {
                    break;
                }
                if closing {
                    break;
                }
            }
            let _ = sink.close().await;
        });

        {
            let shared = Arc::clone(&shared);
            tokio::spawn(async move {
                let reason = loop {
                    tokio::select! {
                        biased;
                        _ = shutdown_rx.changed() => break CloseReason::LocalClose,
                        frame = src.next() => match frame {
                            None => break CloseReason::PeerClosed,
                            Some(Ok(Message::Close(_))) => break CloseReason::PeerClosed,
                            Some(Err(e)) => break CloseReason::TransportError(e.to_string()),
                            Some(Ok(msg)) => handle_frame(&shared, msg),
                        },
                    }
                };
                shared.fail_all(reason);
            });
        }

        Ok(CdpConnection {
            inner: Arc::new(Inner {
                shared,
                outgoing: out_tx,
                next_id: AtomicU64::new(0),
                command_timeout: opts.command_timeout,
                shutdown: shutdown_tx,
                race_widen,
            }),
        })
    }

    pub async fn call(
        &self,
        session: Option<&SessionId>,
        method: &str,
        params: Value,
    ) -> Result<Value> {
        self.call_with_timeout(session, method, params, self.inner.command_timeout)
            .await
    }

    pub async fn call_with_timeout(
        &self,
        session: Option<&SessionId>,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value> {
        // Refuse immediately on a dead socket rather than spending the budget discovering it.
        if let Some(reason) = self.inner.shared.reason() {
            return Err(CdpError::Disconnected(reason));
        }

        // Test-only: `None` on every real connection (see `Inner::race_widen`'s doc), so this is
        // a single `Option` check, never a sleep, outside of the one testkit constructor that
        // opts a connection into it.
        if let Some(widen) = self.inner.race_widen {
            tokio::time::sleep(widen).await;
        }

        // CDP ids start at 1; a 0 id is indistinguishable from "no id" in a hand-written peer.
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = oneshot::channel();
        lock(&self.inner.shared.pending).insert(
            id,
            Pending {
                method: method.to_string(),
                tx,
            },
        );
        // Closes the TOCTOU between the guard above and this insert — see
        // `Shared::recheck_after_insert`'s own doc for why this cannot be skipped.
        if let Some(reason) = self.inner.shared.recheck_after_insert(id) {
            return Err(CdpError::Disconnected(reason));
        }

        let mut frame = json!({
            "id": id,
            "method": method,
            // A null params is normalised to `{}`: some engines reject a literal null where the
            // protocol says object, and "the caller passed nothing" and "the caller passed null"
            // are the same intent.
            "params": if params.is_null() { json!({}) } else { params },
        });
        if let Some(s) = session {
            frame["sessionId"] = json!(s.as_str());
        }

        if self
            .inner
            .outgoing
            .send(Message::text(frame.to_string()))
            .is_err()
        {
            lock(&self.inner.shared.pending).remove(&id);
            return Err(CdpError::Disconnected(
                self.inner
                    .shared
                    .reason()
                    .unwrap_or_else(unknown_close_reason),
            ));
        }

        let started = Instant::now();
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(outcome)) => outcome,
            // The sender was dropped without sending: only reachable if `fail_all` raced the
            // table drain. Report the close, never a success.
            Ok(Err(_recv)) => Err(CdpError::Disconnected(
                self.inner
                    .shared
                    .reason()
                    .unwrap_or_else(unknown_close_reason),
            )),
            Err(_elapsed) => {
                // Forget the slot so a late reply is discarded rather than handed to the next
                // caller that happens to reuse the id space.
                lock(&self.inner.shared.pending).remove(&id);
                Err(CdpError::Timeout {
                    method: method.to_string(),
                    // The MEASURED wait, not the configured budget. The tool layer prints this to
                    // the model as a runtime fact, and a configured number is a different claim.
                    waited: started.elapsed(),
                })
            }
        }
    }

    pub fn closed(&self) -> watch::Receiver<Option<CloseReason>> {
        self.inner.shared.closed.subscribe()
    }

    /// Requests a close; does not wait for the socket to actually go down.
    ///
    /// Two things happen here, both synchronously: a websocket Close frame is queued for the
    /// writer task, and `LocalClose` is recorded as this connection's close reason (so a pending
    /// or later call fails fast — see `call_with_timeout`). Neither one is a wait: the writer may
    /// not have flushed the frame by the time this `async fn` returns (`self.inner.outgoing` is
    /// only queued into, never awaited on here), and the reader task's `select!` is not nudged by
    /// this call — only `Inner::drop`'s `shutdown` watch does that — so the read loop stays
    /// parked on `src.next()` until the peer actually answers the close frame or the last handle
    /// is dropped. A caller needing a guarantee that the socket is fully down when this returns
    /// does not have one from this method; that ordering belongs to whoever owns shutdown
    /// sequencing (Task 9).
    pub async fn close(&self) {
        let _ = self.inner.outgoing.send(Message::Close(None));
        self.inner.shared.fail_all(CloseReason::LocalClose);
    }

    pub fn command_timeout(&self) -> Duration {
        self.inner.command_timeout
    }
}

fn handle_frame(shared: &Shared, msg: Message) {
    // tokio-tungstenite answers pings itself; a control frame is not a CDP frame.
    if matches!(msg, Message::Ping(_) | Message::Pong(_)) {
        return;
    }
    let Ok(text) = msg.to_text() else { return };
    if text.is_empty() {
        return;
    }
    let Ok(frame) = serde_json::from_str::<Value>(text) else {
        return;
    };

    if let Some(id) = frame.get("id").and_then(Value::as_u64) {
        // `None` here means the call was already timed out and forgotten. Dropping the reply is
        // the whole point: handing it to whoever comes next would answer one question with
        // another question's answer.
        let Some(pending) = lock(&shared.pending).remove(&id) else {
            return;
        };
        let outcome = match frame.get("error") {
            Some(err) => match (
                err.get("code").and_then(Value::as_i64),
                err.get("message").and_then(Value::as_str),
            ) {
                (Some(code), Some(message)) => Err(CdpError::Protocol {
                    method: pending.method.clone(),
                    code,
                    message: message.to_string(),
                    // Kept verbatim: some engines put the only actionable detail in `data`, and a
                    // dropped field is a message that says less than the wire did.
                    data: err.get("data").cloned(),
                }),
                // An error frame we cannot read is not a success and not a known refusal.
                _ => Err(CdpError::Decode(format!(
                    "{}: error frame without a numeric code and a message: {err}",
                    pending.method
                ))),
            },
            // A present `result` key answers the call, even an empty `{}` — that is a real CDP
            // shape (Task 0's `<engine>-void.json` fixtures) for a method with nothing to return.
            // But a frame with NEITHER `result` NOR `error` is not that: it is a protocol
            // violation, and treating it as `Ok({})` would hand Task 4's typed wrappers a
            // fabricated success for a reply the peer never actually gave. Fail closed instead:
            // an ambiguous frame is "we do not know", never "it worked" (判据 §8).
            None => match frame.get("result") {
                Some(result) => Ok(result.clone()),
                None => Err(CdpError::Decode(format!(
                    "{}: reply id {id} carries neither `result` nor `error`: {frame}",
                    pending.method
                ))),
            },
        };
        let _ = pending.tx.send(outcome);
        return;
    }

    if let Some(method) = frame.get("method").and_then(Value::as_str) {
        let event = CdpEvent {
            session: frame
                .get("sessionId")
                .and_then(Value::as_str)
                .map(|s| SessionId(s.to_string())),
            method: method.to_string(),
            params: frame.get("params").cloned().unwrap_or_else(|| json!({})),
        };
        // No subscribers is not an error: events happen whether or not anyone is listening, and a
        // subscriber that attaches later starts from the next one.
        let _ = shared.events_tx.send(Arc::new(event));
    }
}
