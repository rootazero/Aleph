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
}

struct Inner {
    shared: Arc<Shared>,
    outgoing: mpsc::UnboundedSender<Message>,
    next_id: AtomicU64,
    command_timeout: Duration,
    shutdown: watch::Sender<bool>,
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
                    .unwrap_or(CloseReason::LocalClose),
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
                    .unwrap_or(CloseReason::LocalClose),
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
            None => Ok(frame.get("result").cloned().unwrap_or_else(|| json!({}))),
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
