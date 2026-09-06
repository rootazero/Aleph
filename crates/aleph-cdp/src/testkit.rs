//! `FakeCdpServer` — a scriptable CDP peer on loopback.
//!
//! Every transport test drives a real websocket against this, not a mocked `CdpConnection`: the
//! things that break in a CDP client (id correlation, a reply that never comes, a socket that
//! dies mid-call, a lagging event subscriber) are all properties of the wire, and a fake that
//! skips the wire cannot exhibit any of them.
//!
//! It lives in `src/` behind the `testkit` feature rather than in `tests/support/mod.rs` because
//! `alephcore` drives `CdpBackend` against it (Tasks 9, 12, 13, 19) and a `tests/` module is not
//! reachable from another crate. The feature is what keeps it out of production builds.
//!
//! The same port answers plain HTTP as well as websockets, because the code under test discovers
//! its endpoint through `GET /json/version` before it ever opens a socket (spec §6.2), and a fake
//! that only speaks websockets would leave that half untested.
//!
//! Mirrors the recording-fake shape of `src/browser/testkit.rs` (a `Mutex<Vec<…>>` of what was
//! asked, plus builders that decide what is answered), adapted to frames instead of trait calls.
//!
//! This is a test double, so it panics loudly on a setup failure (a port it cannot bind, a
//! handshake it cannot complete) instead of returning a `Result` every caller would `unwrap`. It
//! never panics on anything a test under it does.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc::{self, UnboundedSender};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;

use crate::connection::{CdpConnection, ConnectOptions};
use crate::ids::{SessionId, TargetId};

/// The websocket path this server serves, and the one its `/json/version` advertises. Shaped like
/// a real browser endpoint so nothing downstream can come to depend on a bare `/`.
const WS_PATH: &str = "/devtools/browser/fake";

/// The target [`FakeCdpServer::connect_and_attach`] attaches to.
pub const FAKE_TARGET_ID: &str = "fake-target-1";
/// The session [`FakeCdpServer::connect_and_attach`] hands back unless the test scripted its own.
pub const FAKE_SESSION_ID: &str = "fake-session-1";

/// Poisoned locks are recovered, never unwrapped: a panic in one test must not turn every later
/// assertion into a second panic that hides it (P7).
pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// What the fake does with one client frame.
///
/// Every variant here is plain data; what actually runs when a frame arrives is the *closure*
/// that produces one (the constructor's closure passed to [`FakeCdpServer::start`], or the one
/// built by [`scripted`]/[`scripted_slow`]). That closure runs SYNCHRONOUSLY inside the
/// connection's own reader-loop task (`resolve`, called from `serve_ws`), on whichever tokio
/// worker thread is currently polling it. **Never call a blocking primitive there** — no
/// `std::thread::sleep`, no blocking I/O, no tight spin loop. Confirmed empirically while writing
/// this crate's fix round: a responder closure that blocks its worker thread can starve
/// [`FakeCdpServer::shutdown`]'s own 2s timeout on that SAME worker, identically at 2 and 8 total
/// worker threads on a 10-core machine — the number of OTHER idle threads does not help, because a
/// blocked thread cannot service the tokio timer/task wakeups tied to it. If a responder needs to
/// wait, express it as an async value instead: [`Responder::Delay`] for "answers later" and
/// [`Responder::Hang`] for "never answers" — both are genuinely-pending futures that cost no
/// thread and cannot starve anything else in the runtime.
#[derive(Clone, Debug)]
pub enum Responder {
    /// Answer `{"id": …, "result": <value>}`.
    Reply(Value),
    /// Answer `{"id": …, "error": {"code": …, "message": …}}`.
    Error { code: i64, message: String },
    /// Wait, then do the inner thing. The wait does not hold up any other frame — that is what
    /// makes the out-of-order correlation test possible.
    Delay(Duration, Box<Responder>),
    /// Drop the socket without answering.
    Drop,
    /// Send this event frame instead of a reply (the request goes unanswered).
    Event(Value),
    /// Receive the frame, record it, and never reply, ever — no error, no event, no close.
    ///
    /// `Delay` says "the peer is slow"; `Hang` says "the peer never answers". They are distinct
    /// facts a caller needs to tell apart: none of the other five variants can represent an
    /// unresponsive peer — `Drop` closes the socket (a `Disconnected`, not a `Timeout`, from a
    /// real client's point of view), and `Delay` always eventually answers, however long the
    /// delay. A real CDP peer that has wedged looks exactly like `Hang`, and `CdpConnection`'s own
    /// per-command timeout (spec §3.4.2, `CdpError::Timeout`) has no other way to be exercised
    /// against this fake — expect Tasks 2/3 to use this for that.
    ///
    /// This is a recorded, deliberate amendment to the union this crate's testkit exposes (added
    /// in the fix round that added `shutdown`'s falsifier test, C1), not a drift: an equivalent
    /// long `Delay(Duration::from_secs(3600), ...)` was considered and rejected, because a magic
    /// duration is a value the next reader can "fix" by shortening it, silently turning a wedge
    /// into a slow reply with the guard still reporting green. `Hang` cannot be tuned into
    /// something else, and it already has a consumer (`shutdown_panics_naming_a_connection_task_
    /// that_will_not_end` in `tests/smoke.rs`), so it is not a zero-consumer abstraction.
    ///
    /// Implemented as a genuinely-pending future (`std::future::pending`), never a blocking sleep:
    /// see this enum's own top-level doc for why a responder must never block its worker thread.
    Hang,
}

/// Build a responder from a method table. Any method not in the table answers `Reply({})`, which
/// is what a void CDP method returns. See [`Responder`]'s doc for what belongs in that table and
/// what must never be — in particular, never a closure/entry that blocks its thread.
pub fn scripted(
    entries: Vec<(&'static str, Responder)>,
) -> impl Fn(&Value) -> Responder + Send + Sync + 'static {
    let table: HashMap<String, Responder> = entries
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
    move |frame: &Value| {
        let method = frame
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        table
            .get(method)
            .cloned()
            .unwrap_or(Responder::Reply(json!({})))
    }
}

/// [`scripted`], with every answer — including the `Reply({})` an unlisted method gets — wrapped
/// in a `Delay`. For tests about waiting: a timeout, a stall the model is told about, a snapshot
/// that has to bound its own fetch.
pub fn scripted_slow(
    entries: Vec<(&'static str, Responder)>,
    delay: Duration,
) -> impl Fn(&Value) -> Responder + Send + Sync + 'static {
    let inner = scripted(entries);
    move |frame: &Value| Responder::Delay(delay, Box::new(inner(frame)))
}

type ResponderFn = dyn Fn(&Value) -> Responder + Send + Sync + 'static;

struct ConnHandles {
    /// Frames to write to this connection, in order. Replies and pushed events share it, so a
    /// caller that pushes N events and then makes a call knows the call's reply lands after them.
    out_tx: UnboundedSender<Message>,
    /// A nudge that makes the connection task return, dropping both halves of the socket.
    ctl_tx: UnboundedSender<()>,
}

/// Everything a connection task needs. Bundled so the task takes one argument instead of eight.
#[derive(Clone)]
struct ServerCtx {
    port: u16,
    responder: Arc<ResponderFn>,
    /// Per-method overrides, consulted BEFORE `responder`. Registered by [`FakeCdpServer::on`]
    /// after the server is already running.
    overrides: Arc<Mutex<HashMap<String, Responder>>>,
    received: Arc<Mutex<Vec<Value>>>,
    conns: Arc<Mutex<Vec<ConnHandles>>>,
}

pub struct FakeCdpServer {
    port: u16,
    overrides: Arc<Mutex<HashMap<String, Responder>>>,
    received: Arc<Mutex<Vec<Value>>>,
    conns: Arc<Mutex<Vec<ConnHandles>>>,
    conn_tasks: Arc<Mutex<Vec<JoinHandle<()>>>>,
    accept_task: JoinHandle<()>,
}

impl FakeCdpServer {
    /// Bind an ephemeral loopback port and start accepting. `responder` answers any method the
    /// per-method override table (see [`on`](FakeCdpServer::on)) does not.
    ///
    /// `responder` is called SYNCHRONOUSLY on the connection's own tokio worker thread for every
    /// frame — see [`Responder`]'s doc for why it must never block that thread.
    pub async fn start(
        responder: impl Fn(&Value) -> Responder + Send + Sync + 'static,
    ) -> FakeCdpServer {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind an ephemeral loopback port");
        let port = listener.local_addr().expect("local addr").port();
        let ctx = ServerCtx {
            port,
            responder: Arc::new(responder),
            overrides: Arc::new(Mutex::new(HashMap::new())),
            received: Arc::new(Mutex::new(Vec::new())),
            conns: Arc::new(Mutex::new(Vec::new())),
        };
        let conn_tasks: Arc<Mutex<Vec<JoinHandle<()>>>> = Arc::new(Mutex::new(Vec::new()));

        let accept_task = {
            let ctx = ctx.clone();
            let conn_tasks = Arc::clone(&conn_tasks);
            tokio::spawn(async move {
                while let Ok((stream, _peer)) = listener.accept().await {
                    let task = tokio::spawn(serve_stream(stream, ctx.clone()));
                    lock(&conn_tasks).push(task);
                }
            })
        };

        FakeCdpServer {
            port,
            overrides: ctx.overrides,
            received: ctx.received,
            conns: ctx.conns,
            conn_tasks,
            accept_task,
        }
    }

    // ---- addresses ----

    pub fn ws_url(&self) -> String {
        format!("ws://127.0.0.1:{}{WS_PATH}", self.port)
    }

    pub fn http_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    pub fn host(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    // ---- scripting ----

    /// Register or replace the reply for one method, after `start`.
    ///
    /// The override table is consulted before the constructor's closure, so this always wins. That
    /// order is the point: a test builds the server once and then says what THIS case answers,
    /// instead of rebuilding a whole script per case. See [`Responder`]'s doc before reaching for
    /// [`Responder::Delay`]/[`Responder::Hang`] versus a blocking wait of your own.
    pub fn on(&self, method: &str, responder: Responder) {
        lock(&self.overrides).insert(method.to_string(), responder);
    }

    /// [`scripted`] as an associated function, for `FakeCdpServer::scripted(…)` call sites.
    pub fn scripted(
        entries: Vec<(&'static str, Responder)>,
    ) -> impl Fn(&Value) -> Responder + Send + Sync + 'static {
        scripted(entries)
    }

    /// [`scripted_slow`] as an associated function.
    pub fn scripted_slow(
        entries: Vec<(&'static str, Responder)>,
        delay: Duration,
    ) -> impl Fn(&Value) -> Responder + Send + Sync + 'static {
        scripted_slow(entries, delay)
    }

    // ---- observation ----

    /// Every client frame this server has parsed, in arrival order.
    pub fn received(&self) -> Vec<Value> {
        lock(&self.received).clone()
    }

    /// Frames whose `method` matches, in arrival order.
    pub fn received_for(&self, method: &str) -> Vec<Value> {
        lock(&self.received)
            .iter()
            .filter(|f| f.get("method").and_then(Value::as_str) == Some(method))
            .cloned()
            .collect()
    }

    /// The `params` of the last frame for `method`. `None` means the method was never sent —
    /// callers must not read that as "sent with empty params" (判据 §8).
    pub fn last_params(&self, method: &str) -> Option<Value> {
        self.received_for(method)
            .pop()
            .map(|f| f.get("params").cloned().unwrap_or(json!({})))
    }

    // ---- driving ----

    /// Push an unsolicited event frame to every connected client, in line behind everything
    /// already queued for that connection.
    ///
    /// Panics if there are zero connections. A caller can reach this with an empty `conns` list
    /// by racing `connect_async`: `ConnHandles` is only pushed once `accept_async`'s handshake
    /// completes (see `serve_ws`), which happens strictly after the TCP connect a caller's own
    /// `connect_async` call returns from. Pushing an event to nobody and reporting success would
    /// be a report-success no-op inside the very instrument other tests measure with — the danger
    /// is a later NEGATIVE assertion ("no event arrived") that would then pass for entirely the
    /// wrong reason. Fail by name instead: a test that races this finds out immediately, with a
    /// message that says what happened, rather than getting a silently-empty push that looks like
    /// a passing test until someone writes the wrong kind of assertion against it.
    pub fn push_event(&self, event: Value) {
        let conns = lock(&self.conns);
        assert!(
            !conns.is_empty(),
            "FakeCdpServer::push_event: zero connections to push {event} to — either no client \
             has connected yet, or this raced accept_async's registration; await a reply from the \
             client first so this call has somewhere to deliver the event"
        );
        for c in conns.iter() {
            let _ = c.out_tx.send(Message::text(event.to_string()));
        }
    }

    /// Kill every open connection without a close handshake. The listener stays up, so a client
    /// that reconnects is served.
    pub fn drop_socket(&self) {
        for c in lock(&self.conns).iter() {
            let _ = c.ctl_tx.send(());
        }
    }

    /// Stop accepting, close every socket, and wait until each connection task has fully ended.
    ///
    /// "Fully" is a specific, code-level guarantee, not a timing claim: `serve_ws` does
    /// `writer.abort(); writer.await;` before its own task (the one `shutdown` joins here) can
    /// return, so "a connection is closed" gets the same derivation on both of its halves (reader
    /// and writer) instead of one being inferred from the other. By construction, by the time
    /// `shutdown` returns, BOTH split halves of every websocket (`src`, owned by the joined task,
    /// and `sink`, owned by the `writer` task it awaits) have been dropped, and the OS-level close
    /// (the TCP FIN) has already been issued — a fact about Rust's `.await` ordering, not
    /// something that needs a race to observe.
    ///
    /// What this does NOT and CANNOT guarantee: how quickly a REMOTE peer's own runtime notices
    /// that FIN and reports it to a caller of `ws.next()`. Measured directly while writing this
    /// fix round: a client-side check right after `shutdown()` returns, with no wait at all, saw
    /// the socket as "not yet closed" on roughly 90% of 30 runs — identically whether or not
    /// `writer` was awaited (2/30 vs 1/30 "already closed" at 0ms; 30/30 "already closed" for BOTH
    /// at a 1ms bound) — because that observation is dominated by the CLIENT's own reactor
    /// granularity, not by anything the server controls. The guarantee this function makes is
    /// that the server has released everything by the time it returns; it is not, and cannot be,
    /// a guarantee about how fast a peer finds out.
    ///
    /// The wait is what makes this different from letting the value drop: when it returns, the
    /// client has already observed the close, so a test can assert on a pending call's error
    /// without racing the runtime. Each connection task gets a 2s budget. A timeout that expires
    /// is the answer "I do not know whether it stopped" — it must never be read as "it stopped"
    /// (判据 §8), so this panics loudly, naming which connection task is still wedged, rather than
    /// returning as if the wait had succeeded. A connection task that itself panicked is also
    /// surfaced loudly rather than swallowed: this is a test double, so failing loudly on its own
    /// bug is exactly what keeps a wedged or broken fake from silently passing a downstream test.
    ///
    /// This budget catches a connection task that is legitimately, cooperatively stuck (pending
    /// forever on a future that never wakes it — e.g. `Responder::Hang`, which is what
    /// `shutdown_panics_naming_a_connection_task_that_will_not_end` uses to prove this). It does
    /// NOT reliably catch a task that blocks its worker thread synchronously (e.g. calling
    /// `std::thread::sleep` or any non-yielding blocking call from inside a responder closure):
    /// that kind of task can starve the very timer this budget depends on, on the same worker
    /// thread, which is a tokio-wide hazard and not specific to this function. Nothing in this
    /// crate does that, and any responder added later must not either.
    pub async fn shutdown(self) {
        self.accept_task.abort();
        for c in lock(&self.conns).iter() {
            let _ = c.ctl_tx.send(());
        }
        let tasks: Vec<JoinHandle<()>> = lock(&self.conn_tasks).drain(..).collect();
        for (i, task) in tasks.into_iter().enumerate() {
            match tokio::time::timeout(Duration::from_secs(2), task).await {
                Ok(Ok(())) => {}
                Ok(Err(join_err)) => {
                    panic!("FakeCdpServer::shutdown: connection task {i} panicked: {join_err}")
                }
                Err(_elapsed) => panic!(
                    "FakeCdpServer::shutdown: connection task {i} did not end within 2s — a \
                     timeout that expired means we do not know whether it stopped, which must \
                     never be reported as a clean shutdown"
                ),
            }
        }
    }
}

impl FakeCdpServer {
    /// Connect and attach in one line: the shape four other parts' tests start from.
    ///
    /// The command timeout is 5s, deliberately not the 30s product default — a test whose fake
    /// forgot to script a method should fail in five seconds, not thirty.
    ///
    /// The `Target.attachToTarget` reply is filled in ONLY when the override table has no entry
    /// for it, so a test that needs a particular session id calls `on("Target.attachToTarget", …)`
    /// first and that wins. Note the asymmetry: an entry in the CONSTRUCTOR's script does not win,
    /// because the override table is consulted before it — script the session id with `on`, not
    /// with `scripted`.
    pub async fn connect_and_attach(&self) -> (CdpConnection, SessionId) {
        {
            let mut overrides = lock(&self.overrides);
            overrides
                .entry("Target.attachToTarget".to_string())
                .or_insert_with(|| Responder::Reply(json!({ "sessionId": FAKE_SESSION_ID })));
        }
        let conn = CdpConnection::connect(
            self.ws_url().as_str(),
            ConnectOptions {
                command_timeout: Duration::from_secs(5),
            },
        )
        .await
        .expect("connect to the fake server");
        let session = conn
            .attach(&TargetId(FAKE_TARGET_ID.to_string()))
            .await
            .expect("attach to the fake target");
        (conn, session)
    }
}

impl Drop for FakeCdpServer {
    fn drop(&mut self) {
        self.accept_task.abort();
        for c in lock(&self.conns).iter() {
            let _ = c.ctl_tx.send(());
        }
    }
}

/// Decide what this connection is and hand it to the right half.
///
/// The head is PEEKED, not read: `TcpStream::peek` leaves the bytes in the receive queue, so a
/// websocket handshake still reaches `accept_async` whole. Consuming it here and replaying it
/// would mean writing a buffering wrapper around the stream, for no gain.
async fn serve_stream(stream: TcpStream, ctx: ServerCtx) {
    let head = peek_head(&stream).await;
    let head = String::from_utf8_lossy(&head).to_ascii_lowercase();
    if head.contains("upgrade: websocket") {
        serve_ws(stream, ctx).await;
    } else {
        serve_http(stream, ctx.port).await;
    }
}

async fn peek_head(stream: &TcpStream) -> Vec<u8> {
    let mut buf = vec![0u8; 2048];
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let mut seen = Vec::new();
    while tokio::time::Instant::now() < deadline {
        match stream.peek(&mut buf).await {
            Ok(0) => break,
            Ok(n) => {
                seen = buf[..n].to_vec();
                if n == buf.len() || seen.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            Err(_) => break,
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    seen
}

/// The HTTP control plane. Only `/json/version` is real; everything else is a 404 with `{}` so a
/// caller that asks for the wrong path gets a refusal rather than a plausible-looking answer.
async fn serve_http(mut stream: TcpStream, port: u16) {
    let mut buf = vec![0u8; 4096];
    let n = stream.read(&mut buf).await.unwrap_or(0);
    let request = String::from_utf8_lossy(&buf[..n]).to_string();
    let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();
    let (status, body) = if path.starts_with("/json/version") {
        (
            "200 OK",
            json!({
                "Browser": "Aleph-FakeCdp/1.0",
                "Protocol-Version": "1.3",
                "User-Agent": "Aleph-FakeCdp",
                "V8-Version": "0.0.0.0",
                "WebKit-Version": "0.0.0.0",
                // The one field that matters: whatever discovers an endpoint here must be able to
                // connect to what it finds, so this is built from the same port the socket is on.
                "webSocketDebuggerUrl": format!("ws://127.0.0.1:{port}{WS_PATH}"),
            })
            .to_string(),
        )
    } else {
        ("404 Not Found", "{}".to_string())
    };
    let response = format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.flush().await;
}

async fn serve_ws(stream: TcpStream, ctx: ServerCtx) {
    let ws = match tokio_tungstenite::accept_async(stream).await {
        Ok(ws) => ws,
        Err(_) => return,
    };
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Message>();
    let (ctl_tx, mut ctl_rx) = mpsc::unbounded_channel::<()>();
    lock(&ctx.conns).push(ConnHandles {
        out_tx: out_tx.clone(),
        ctl_tx: ctl_tx.clone(),
    });

    let (mut sink, mut src) = ws.split();
    // The writer owns the sink; the reader owns the source. `ctl_rx` is a separate channel from
    // `out_rx` on purpose: `select!` would not compile if both branches borrowed the socket.
    let writer = tokio::spawn(async move {
        while let Some(m) = out_rx.recv().await {
            if sink.send(m).await.is_err() {
                break;
            }
        }
    });

    loop {
        tokio::select! {
            biased;
            _ = ctl_rx.recv() => break,
            msg = src.next() => {
                let Some(Ok(msg)) = msg else { break };
                let Ok(text) = msg.to_text() else { continue };
                let Ok(frame) = serde_json::from_str::<Value>(text) else { continue };
                lock(&ctx.received).push(frame.clone());
                let action = resolve(&ctx, &frame);
                if !dispatch(action, &frame, &out_tx, &ctl_tx).await {
                    break;
                }
            }
        }
    }
    writer.abort();
    // `shutdown` joins the OUTER task that runs `serve_stream` (and, for a websocket, this
    // function) — not `writer` directly. If this returned right after `writer.abort()`, joining
    // the outer task would prove only that the reader loop ended, not that the socket's write
    // half (owned by `writer`, holding `sink`) is actually gone: C3's gap. Awaiting `writer` here
    // makes "the outer task ended" mean "both halves of the socket ended", which is the property
    // `shutdown`'s contract claims and later tasks (2, 3, 4, 9, 12, 13, 17, 19) depend on. An
    // aborted task resolves to a cancelled `JoinError`, which is the expected, not-a-bug outcome
    // of the abort just above — nothing else can legitimately produce it here.
    let _ = writer.await;
}

/// Overrides first, then the constructor's closure. One order, in one place.
fn resolve(ctx: &ServerCtx, frame: &Value) -> Responder {
    let method = frame
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if let Some(r) = lock(&ctx.overrides).get(method).cloned() {
        return r;
    }
    (ctx.responder)(frame)
}

/// Returns `false` when the connection should end.
///
/// Boxed explicitly (`Pin<Box<dyn Future<...> + Send>>`) rather than left as a plain `async fn`:
/// `dispatch` is recursive (a `Delay` may wrap a `Drop`, and the recursive call is itself spawned
/// on the runtime), and a self-recursive `async fn` cannot have its `Send`-ness inferred through a
/// bare `Box::pin` of its own call — the compiler needs the boxed-trait-object return type to break
/// the otherwise-infinite future type before it can prove the whole thing `Send`.
fn dispatch<'a>(
    action: Responder,
    frame: &'a Value,
    out_tx: &'a UnboundedSender<Message>,
    ctl_tx: &'a UnboundedSender<()>,
) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
    Box::pin(async move {
        let id = frame.get("id").and_then(Value::as_i64);
        let session = frame
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::to_string);
        match action {
            Responder::Reply(result) => {
                send_reply(out_tx, id, session, Ok(result));
                true
            }
            Responder::Error { code, message } => {
                send_reply(out_tx, id, session, Err((code, message)));
                true
            }
            Responder::Event(event) => {
                let _ = out_tx.send(Message::text(event.to_string()));
                true
            }
            Responder::Drop => {
                // Returning `false` alone already ends the read loop (`if !dispatch(...).await {
                // break; }` in serve_ws), which then aborts+joins `writer` and drops the socket.
                // A `ctl_tx.send(())` here was dead code — a fix-round review confirmed removing
                // it changes no test's behaviour (`responder_drop_closes_the_socket_for_the_
                // scripted_method` and `fake_server_drop_socket_ends_the_stream` both stay green).
                false
            }
            Responder::Delay(d, inner) => {
                let out_tx = out_tx.clone();
                let ctl_tx = ctl_tx.clone();
                let frame = frame.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(d).await;
                    // `dispatch` already returns a boxed future, so the recursive call needs no
                    // extra `Box::pin` here: a Delay may wrap a Drop, and a delayed Drop still has
                    // to reach the reader loop, which is why `ctl_tx` is threaded here.
                    dispatch(*inner, &frame, &out_tx, &ctl_tx).await;
                });
                true
            }
            Responder::Hang => {
                // Never resolves and never wakes: this parks the connection's own poll (no
                // thread held, unlike a blocking sleep), so the read loop cannot observe `ctl_rx`
                // again until this connection is dropped/aborted from outside (e.g. by
                // `drop_socket` or `shutdown`, both of which act on the socket, not on this task).
                std::future::pending::<()>().await;
                unreachable!("a pending future never resolves")
            }
        }
    })
}

fn send_reply(
    out_tx: &UnboundedSender<Message>,
    id: Option<i64>,
    session: Option<String>,
    body: std::result::Result<Value, (i64, String)>,
) {
    // A frame with no `id` is not a request; there is nothing to answer. Silently returning is
    // correct here and only here — the fake never invents an id.
    let Some(id) = id else { return };
    let mut msg = match body {
        Ok(result) => json!({ "id": id, "result": result }),
        Err((code, message)) => json!({ "id": id, "error": { "code": code, "message": message } }),
    };
    if let Some(s) = session {
        msg["sessionId"] = json!(s);
    }
    let _ = out_tx.send(Message::text(msg.to_string()));
}
