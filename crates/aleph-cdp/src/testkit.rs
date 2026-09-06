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
}

/// Build a responder from a method table. Any method not in the table answers `Reply({})`, which
/// is what a void CDP method returns.
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
    /// instead of rebuilding a whole script per case.
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
    pub fn push_event(&self, event: Value) {
        for c in lock(&self.conns).iter() {
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

    /// Stop accepting, close every socket, and wait until each connection task has ended.
    ///
    /// The wait is what makes this different from letting the value drop: when it returns, the
    /// client has already observed the close, so a test can assert on a pending call's error
    /// without racing the runtime. Bounded at 2s per task so a wedged task fails the test rather
    /// than hanging it.
    pub async fn shutdown(self) {
        self.accept_task.abort();
        for c in lock(&self.conns).iter() {
            let _ = c.ctl_tx.send(());
        }
        let tasks: Vec<JoinHandle<()>> = lock(&self.conn_tasks).drain(..).collect();
        for task in tasks {
            let _ = tokio::time::timeout(Duration::from_secs(2), task).await;
        }
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
                let _ = ctl_tx.send(());
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
