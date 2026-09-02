//! WebSocket client for Aleph Gateway
//!
//! This module provides a JSON-RPC 2.0 client over WebSocket,
//! using only types from `aleph-protocol`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use aleph_protocol::jsonrpc::TOPIC_EVENT_METHOD;
use aleph_protocol::{JsonRpcError, JsonRpcRequest, JsonRpcResponse, StreamEvent};
use futures_util::{SinkExt, StreamExt};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot, Mutex, RwLock};
use tokio_tungstenite::{
    connect_async_tls_with_config, tungstenite::Message, MaybeTlsStream, WebSocketStream,
};
use tracing::{debug, error, info, warn};

use crate::config::CliConfig;
use crate::error::{CliError, CliResult};
use crate::tls;

/// Pending RPC request
struct PendingRequest {
    tx: oneshot::Sender<Result<Value, JsonRpcError>>,
}

/// Type alias for WebSocket write half
type WsWriter = Arc<
    Mutex<futures_util::stream::SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>>,
>;

/// Deadline on one [`AlephClient::reconnect`] attempt's TCP+TLS handshake.
///
/// A bound, not a policy: the caller decides whether to try again, and it
/// cannot decide anything while an attempt that will never return is still
/// outstanding. Ten seconds is long enough for a loaded gateway on a slow link
/// and short enough that a retry cadence stays a retry cadence.
const RECONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// What one inbound WebSocket text frame turned out to be.
///
/// Split out of [`AlephClient::handle_message`] because that function's `write`
/// argument is an `Arc<Mutex<SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, _>>>`
/// — a value nothing can construct without a live socket — so the *parse*, the
/// half that carried the defect below, was unreachable from any test in this
/// repo for as long as it lived inside it.
#[derive(Debug)]
enum Inbound {
    /// A reply to a request this client sent.
    Response {
        id: String,
        result: Result<Value, JsonRpcError>,
    },
    /// A server-initiated request that expects a reply.
    ServerRequest { method: String, id: Value },
    /// A gateway stream event, ready for the caller's receiver.
    Event(Box<StreamEvent>),
    /// A `{"method":"event","params":{"topic":…,"data":…}}` topic-event frame
    /// — the Panel's `events.subscribe` plane, and (since Task 8a) any other
    /// client's. Its own variant rather than falling through to
    /// [`Inbound::UnhandledNotification`]: that arm exists for frames this
    /// client genuinely cannot make sense of, and a topic frame is not
    /// one — it is fully understood, just not a [`StreamEvent`].
    Topic { topic: String, data: Value },
    /// The envelope parsed but the payload is neither a [`StreamEvent`] nor a
    /// recognisable topic-event frame.
    ///
    /// `loud` separates a broken contract from a frame family this client
    /// simply does not consume: every `stream.*` method promises a
    /// `StreamEvent` (`gateway::events::frame_census` asserts that pairing on
    /// the server side).
    UnhandledNotification {
        method: String,
        loud: bool,
        reason: String,
    },
    /// Not a JSON-RPC frame this client understands.
    Unrecognized,
}

/// Classify one inbound text frame. Pure: no I/O, no shared state, one parse.
///
/// ## Why this does not go through `JsonRpcRequest` / `JsonRpcResponse`
///
/// It used to, and that was a total, silent outage of the event plane for every
/// client built on this crate.
///
/// Both of those structs carry a required `jsonrpc: String`. The gateway's
/// event wire form did not send one: `event_bus.rs::publish_frame` hand-built
/// `{"method": "stream.X", "params": {…}}` and `handler.rs::event_wire_form`
/// forwards those bytes verbatim. So `serde_json::from_str::<JsonRpcRequest>`
/// failed with `missing field 'jsonrpc'` on **every** frame, and the caller
/// logged one `debug!` line and moved on. The CLI (`aleph watch`, `aleph ask`)
/// and the whole TUI share this file, so neither had ever received a single
/// `stream.*` frame from a real gateway — `aleph ask` parked forever on a run
/// that had already finished and printed nothing at all, `aleph watch` printed
/// its banner and nothing else. Measured side by side on one socket, a bare
/// `python websockets` client received every frame.
///
/// Two things were wrong and both are now fixed, because either one alone
/// leaves the class open:
///
/// 1. the server now builds that envelope with
///    `aleph_protocol::JsonRpcRequest::notification`, so what goes out is a
///    conformant JSON-RPC 2.0 notification and the two halves of the contract
///    are one type rather than two hand-written shapes; and
/// 2. this reads the three fields it actually needs off a `Value`, so the next
///    producer that forgets the version tag costs a warning line instead of
///    the entire event stream.
///
/// The guard against a repeat is `envelope_without_the_version_tag_still_yields_the_event`
/// below, next to the reconciliation test that feeds this function the exact
/// bytes the shared constructor emits.
fn classify_frame(text: &str) -> Inbound {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return Inbound::Unrecognized;
    };
    // A JSON-RPC notification carries no id, and a response to a request this
    // client never sent is indistinguishable from one it did — so `null` is
    // "absent", not "an id whose value is null".
    let id = value.get("id").filter(|v| !v.is_null());

    // `method` is read first because it is the field only one of the two shapes
    // has. Branching on `id` first would classify a server-initiated *request*
    // (which has both) as a response, and answer it by resolving a pending
    // entry that does not exist — silently, since the map lookup simply misses.
    if let Some(method) = value.get("method").and_then(Value::as_str) {
        if let Some(id) = id {
            return Inbound::ServerRequest {
                method: method.to_string(),
                id: id.clone(),
            };
        }
        let Some(params) = value.get("params") else {
            return Inbound::UnhandledNotification {
                method: method.to_string(),
                loud: method.starts_with(STREAM_METHOD_PREFIX),
                reason: "notification carried no params".to_string(),
            };
        };
        // The Panel's (and now every client's) topic-event envelope:
        // `{"method":"event","params":{"topic":…,"data":…}}` — see
        // `gateway::server::handler::event_wire_form`'s doc for the two shapes
        // that get wrapped into it. Checked before the `StreamEvent` decode
        // attempt below so a topic frame is *recognised*, not merely a
        // `StreamEvent` decode failure that happens to be non-loud — the two
        // used to be indistinguishable here, which is exactly what made this
        // whole family silently undeliverable to any caller (Task 8a).
        if method == TOPIC_EVENT_METHOD {
            return match params.get("topic").and_then(Value::as_str) {
                Some(topic) => Inbound::Topic {
                    topic: topic.to_string(),
                    data: params.get("data").cloned().unwrap_or(Value::Null),
                },
                None => Inbound::UnhandledNotification {
                    method: method.to_string(),
                    // Never loud: `"event"` is not a `stream.*` method, and a
                    // topic envelope missing its own `topic` field is a
                    // producer bug on the SAME frame family this arm exists
                    // to keep quiet, not a broken `stream.*` contract.
                    loud: false,
                    reason: "topic-event envelope missing 'topic' field".to_string(),
                },
            };
        }
        return match serde_json::from_value::<StreamEvent>(params.clone()) {
            Ok(event) => Inbound::Event(Box::new(event)),
            Err(e) => Inbound::UnhandledNotification {
                method: method.to_string(),
                loud: method.starts_with(STREAM_METHOD_PREFIX),
                reason: e.to_string(),
            },
        };
    }

    if let Some(id) = id {
        let id = match id {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            // An id that is neither is not an id this client ever minted
            // (`next_id` produces decimal strings), so nothing is waiting on it.
            _ => return Inbound::Unrecognized,
        };
        let result = match (value.get("error"), value.get("result")) {
            (Some(error), _) => Err(serde_json::from_value::<JsonRpcError>(error.clone())
                .unwrap_or_else(|e| JsonRpcError {
                    code: aleph_protocol::jsonrpc::INVALID_REQUEST,
                    message: format!("Invalid Request: malformed error object: {e}"),
                    data: Some(error.clone()),
                })),
            (None, Some(result)) => Ok(result.clone()),
            (None, None) => Err(JsonRpcError {
                code: aleph_protocol::jsonrpc::INVALID_REQUEST,
                message: "Invalid Request: missing both result and error".into(),
                data: None,
            }),
        };
        return Inbound::Response { id, result };
    }

    Inbound::Unrecognized
}

/// Method prefix every gateway stream frame carries
/// (`GatewayEventFrame::stream_method`).
const STREAM_METHOD_PREFIX: &str = "stream.";

/// One item delivered on the receiver [`AlephClient::connect`]/[`AlephClient::open`]
/// hands back to the caller.
///
/// A single channel carrying an enum, not two receivers: see
/// [`classify_frame`]'s `TOPIC_EVENT_METHOD` arm for how a frame becomes one
/// variant or the other. Two receivers would give every caller two answers to
/// "what did the server send me, and in what order" — this crate's callers
/// (the CLI's `aleph watch`/`aleph ask`, the TUI's main loop) already select
/// on one receiver and would have to pick an arbitrary priority between two.
#[derive(Debug, Clone)]
pub enum ClientEvent {
    /// A `stream.*` frame — the run/session event plane every existing
    /// caller of this crate already consumes. Boxed: clippy's
    /// `large_enum_variant` flags the unboxed form (`StreamEvent` is far
    /// larger than `Topic`'s two fields), and `Inbound::Event` already hands
    /// one in as a `Box`.
    Stream(Box<StreamEvent>),
    /// A `{"method":"event","params":{"topic":…,"data":…}}` topic frame —
    /// the Panel's `events.subscribe` plane. Quiet on the wire (classifying
    /// one never logs a `warn!`; see `classify_frame`), but no longer
    /// dropped: a caller that has subscribed to a topic can now receive it
    /// here instead of it being swallowed as an "unhandled notification"
    /// indistinguishable from a genuinely undecodable frame.
    Topic { topic: String, data: Value },
}

/// WebSocket client for Aleph Gateway
pub struct AlephClient {
    /// WebSocket write half
    write: WsWriter,
    /// Pending requests waiting for response
    pending: Arc<RwLock<HashMap<String, PendingRequest>>>,
    /// Request ID counter
    id_counter: Arc<std::sync::atomic::AtomicU64>,
    /// Stream event channel.
    ///
    /// Two jobs, and the second is what makes [`AlephClient::reconnect`]
    /// possible at all. It is the ownership anchor that keeps the caller's
    /// `Receiver` alive after a read loop ends — and it is the sender a
    /// REPLACEMENT read loop is handed, so frames off a new socket arrive on
    /// the same receiver the caller has been selecting on since launch.
    /// Nothing downstream is re-plumbed, and no caller has to learn that the
    /// socket underneath changed.
    event_tx: mpsc::Sender<ClientEvent>,
    /// Whether this client is usable — the socket is up AND handshaken.
    ///
    /// Cleared by the read loop when the socket drops, and held low across the
    /// whole of [`AlephClient::reconnect`], where it doubles as the mutual
    /// exclusion keeping other callers off a connection the gateway has not
    /// accepted yet.
    connected: Arc<std::sync::atomic::AtomicBool>,
    /// Which socket generation owns [`Self::connected`].
    ///
    /// A read loop clears that flag on its way out, and after a reconnect two
    /// loops can be alive at once: the superseded one is parked on a socket
    /// nobody writes to any more, and when it finally notices — when the server
    /// times that socket out — it must not report the live connection as dead.
    /// Each loop carries the generation it was spawned for and touches shared
    /// state only while it is still the current one.
    generation: Arc<std::sync::atomic::AtomicU64>,
    /// The gateway URL this client was opened against.
    ///
    /// Kept so `reconnect` needs nothing from the caller but credentials: a
    /// caller that had to re-supply the URL is a caller that could supply a
    /// different one, and this client's identity is the endpoint it opened.
    url: String,
    /// Server-assigned role from the most recent `connect` handshake.
    ///
    /// Behind a lock because there can be more than one handshake — see
    /// [`AlephClient::role`].
    role: Arc<std::sync::Mutex<String>>,
}

impl AlephClient {
    /// Connect to Aleph Gateway and complete the `connect` handshake.
    ///
    /// The handshake is not optional and is deliberately not a separate public
    /// step: the gateway enforces that the first frame on a connection is
    /// `connect` and *closes the socket* otherwise (`server::handler`), so a
    /// client that skipped it could never issue a single working call. Folding
    /// it in here means a caller cannot hold an un-handshaken `AlephClient` —
    /// twenty of twenty-eight CLI command modules used to skip it because the
    /// two steps were separable, and every one of their subcommands was dead
    /// against a live server.
    pub async fn connect(
        url: &str,
        config: &CliConfig,
    ) -> CliResult<(Self, mpsc::Receiver<ClientEvent>)> {
        let (client, events) = Self::open(url, config).await?;
        client.set_role(client.handshake(config).await?);
        Ok((client, events))
    }

    /// Rebuild the socket under this client, keeping every handle its callers
    /// already hold.
    ///
    /// The caller's `Receiver` is not replaced: a fresh read loop is spawned
    /// onto the SAME `event_tx`, so a client that has been selecting on that
    /// receiver since launch keeps receiving and nothing downstream learns the
    /// socket changed. `write`, `pending` and `connected` are shared through
    /// `Arc`, so this takes `&self` and every outstanding `&AlephClient`
    /// borrow stays valid.
    ///
    /// # What this deliberately does NOT do
    ///
    /// It does not retry, back off, or schedule itself. One attempt; the caller
    /// decides whether there is another. A client that reconnected on its own
    /// would do it invisibly, and the invisible kind is the dangerous kind: a
    /// long-lived UI reconciled state against the connection that died — which
    /// run is in flight, which conversation is live — and coming back without
    /// re-asking leaves it adjudicating on a baseline from a connection that no
    /// longer exists. Whoever owns that state has to learn the socket came
    /// back, so the policy is theirs and only the mechanism is here.
    ///
    /// # Ordering
    ///
    /// `connected` stays false throughout, and that is the mutual exclusion: a
    /// concurrent `call` reads it and fails fast, so nothing but the handshake
    /// can reach a socket the gateway has not accepted yet — it answers a
    /// non-`connect` first frame by closing the connection, which would take
    /// this attempt down with it. The flag goes true only once the handshake
    /// has been answered.
    ///
    /// # Errors
    ///
    /// The socket could not be opened within [`RECONNECT_TIMEOUT`], TLS
    /// negotiation failed, or the handshake was refused. The client is left
    /// disconnected and the caller may try again.
    pub async fn reconnect(&self, config: &CliConfig) -> CliResult<()> {
        if self.is_connected() {
            return Ok(());
        }

        // Opened BEFORE anything shared is touched, and under a deadline: a
        // failure here has to leave the client exactly as it found it rather
        // than half-swapped, and a black-holed TCP connect that never returns
        // would strand the caller's one attempt forever — it is waiting on this
        // future to decide whether to try again.
        let connector = tls::connector_for(&self.url, config.ca_cert.as_deref())?;
        let (ws_stream, _) = tokio::time::timeout(
            RECONNECT_TIMEOUT,
            connect_async_tls_with_config(self.url.as_str(), None, false, connector),
        )
        .await
        .map_err(|_| {
            CliError::Connection(format!(
                "timed out after {}s reconnecting to {}",
                RECONNECT_TIMEOUT.as_secs(),
                self.url
            ))
        })?
        .map_err(|e| CliError::Connection(e.to_string()))?;

        let (write, read) = ws_stream.split();

        // Claimed before the loop is spawned, so the loop it supersedes can
        // never win a race to clear `connected` after this one sets it.
        let my_generation = self
            .generation
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;

        let (read_pending, read_write) = {
            let mut sink = self.write.lock().await;
            *sink = write;
            (self.pending.clone(), self.write.clone())
        };

        // Anything still registered belongs to the socket that died. Normally
        // its own read loop already cleared the map on its way out; this covers
        // the other route to `connected == false`, where a send failed and that
        // loop has not noticed yet. Those callers can never be answered, and
        // dropping their senders tells them so now.
        read_pending.write().await.clear();

        let read_handle = tokio::spawn(Self::read_loop(
            read,
            read_pending,
            self.event_tx.clone(),
            self.connected.clone(),
            read_write,
            self.generation.clone(),
            my_generation,
        ));

        match self.handshake(config).await {
            Ok(role) => {
                self.set_role(role);
                self.connected
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                info!("Reconnected to {}", self.url);
                Ok(())
            }
            Err(e) => {
                // The handshake failed (auth refused, timeout, malformed
                // response). The socket is not usable and `connected` stays
                // false, but the read loop we spawned is still parked on the
                // read half. Without aborting it, every failed reconnect leaves
                // a task behind that may outlive the client and accumulate
                // under repeated attempts. Send a close frame as a courtesy so
                // the gateway can tear its side down promptly.
                read_handle.abort();
                let mut write = self.write.lock().await;
                let _ = write.send(Message::Close(None)).await;
                Err(e)
            }
        }
    }

    /// Open the socket and spawn the read loop, without handshaking.
    async fn open(url: &str, config: &CliConfig) -> CliResult<(Self, mpsc::Receiver<ClientEvent>)> {
        info!("Connecting to {}", url);

        let connector = tls::connector_for(url, config.ca_cert.as_deref())?;
        let (ws_stream, _) = connect_async_tls_with_config(url, None, false, connector)
            .await
            .map_err(|e| CliError::Connection(e.to_string()))?;

        let (write, read) = ws_stream.split();

        let (event_tx, event_rx) = mpsc::channel(100);
        let pending = Arc::new(RwLock::new(HashMap::new()));
        let connected = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let write = Arc::new(Mutex::new(write));
        let generation = Arc::new(std::sync::atomic::AtomicU64::new(0));

        let client = Self {
            write: write.clone(),
            pending: pending.clone(),
            id_counter: Arc::new(std::sync::atomic::AtomicU64::new(1)),
            event_tx: event_tx.clone(),
            connected: connected.clone(),
            url: url.to_string(),
            generation: generation.clone(),
            role: Arc::new(std::sync::Mutex::new(String::new())),
        };

        // Spawn read task with write access for responding to Server requests.
        // Generation 0: the first socket, and the one every later reconnect
        // supersedes.
        tokio::spawn(Self::read_loop(
            read, pending, event_tx, connected, write, generation, 0,
        ));

        info!("Connected to Gateway");
        Ok((client, event_rx))
    }

    /// Read loop for incoming messages.
    ///
    /// `my_generation` is which socket this loop was spawned for; it clears
    /// `connected` on the way out only while `generation` still names it. See
    /// the field's doc — after a reconnect two loops can be alive, and the
    /// abandoned one must not report the live connection as dead.
    async fn read_loop(
        mut read: futures_util::stream::SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>,
        pending: Arc<RwLock<HashMap<String, PendingRequest>>>,
        event_tx: mpsc::Sender<ClientEvent>,
        connected: Arc<std::sync::atomic::AtomicBool>,
        write: WsWriter,
        generation: Arc<std::sync::atomic::AtomicU64>,
        my_generation: u64,
    ) {
        while let Some(msg) = read.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    Self::handle_message(&text, &pending, &event_tx, &write).await;
                }
                Ok(Message::Close(_)) => {
                    info!("Server closed connection");
                    break;
                }
                Ok(Message::Ping(data)) => {
                    debug!("Received ping");
                    // Pong is handled automatically by tungstenite
                    let _ = data;
                }
                Err(e) => {
                    error!("WebSocket error: {}", e);
                    break;
                }
                _ => {}
            }
        }

        // Only the loop that still owns this client may report it dead.
        //
        // After a reconnect there can be two loops alive: the old one is parked
        // on a socket nobody writes to any more, and when it finally notices —
        // seconds or minutes later, when the server times that socket out — it
        // must not clear the flag on the LIVE connection or clear the pending
        // map belonging to it. That would take down a working client with no
        // error anywhere, and the trigger would be an event on a socket that
        // was abandoned on purpose.
        if generation.load(std::sync::atomic::Ordering::SeqCst) != my_generation {
            debug!("Superseded read loop ended (generation {my_generation})");
            return;
        }

        connected.store(false, std::sync::atomic::Ordering::SeqCst);

        // Fail every request still waiting on this socket instead of leaving it
        // to time out. Nothing can answer them — the reader that would route
        // the response is this task, and it is finishing. Clearing the map
        // drops each caller's oneshot sender, which resolves their `await` as
        // `Disconnected` now rather than in thirty seconds' time.
        //
        // Load-bearing for `reconnect`: a socket that dies DURING the new
        // handshake would otherwise park that exchange for the full timeout,
        // and the client would sit there believing it was still connecting.
        let abandoned = {
            let mut map = pending.write().await;
            let n = map.len();
            map.clear();
            n
        };
        if abandoned > 0 {
            warn!("Read loop ended; {abandoned} in-flight request(s) abandoned");
        } else {
            info!("Read loop ended");
        }
    }

    /// Handle incoming message
    async fn handle_message(
        text: &str,
        pending: &Arc<RwLock<HashMap<String, PendingRequest>>>,
        event_tx: &mpsc::Sender<ClientEvent>,
        write: &WsWriter,
    ) {
        // Log all incoming messages for debugging.
        // Truncate on a UTF-8 char boundary — a byte slice (`&text[..n]`) panics
        // when byte `n` lands inside a multi-byte char (common with CJK/emoji),
        // which would kill the read loop and silently hang every pending request.
        let preview_end = text.char_indices().nth(500).map_or(text.len(), |(i, _)| i);
        debug!("Received raw message: {}", &text[..preview_end]);

        match classify_frame(text) {
            Inbound::Response { id, result } => {
                let mut pending_guard = pending.write().await;
                let maybe_req = pending_guard.remove(&id);
                drop(pending_guard);
                if let Some(req) = maybe_req {
                    let _ = req.tx.send(result);
                }
            }
            Inbound::ServerRequest { method, id } => {
                debug!(method = %method, "Received request from Server");
                Self::handle_server_request(&method, id, write).await;
            }
            Inbound::Event(event) => {
                debug!("Parsed event: {:?}", event);
                let _ = event_tx.send(ClientEvent::Stream(event)).await;
            }
            Inbound::Topic { topic, data } => {
                // Deliberately no log line here, loud or quiet: the frame is
                // fully understood and forwarded, which is the "quiet" half
                // of Task 8a's rewritten test — an unlogged frame that is
                // silently DROPPED (the old behaviour) and one that is
                // silently DELIVERED look identical in the log, so the
                // channel send is what the test actually has to observe.
                let _ = event_tx.send(ClientEvent::Topic { topic, data }).await;
            }
            Inbound::UnhandledNotification {
                method,
                loud,
                reason,
            } => {
                if loud {
                    // A `stream.*` method that does not decode is a broken
                    // cross-crate contract, not a frame family this client
                    // declines to consume — see `classify_frame`'s note on why
                    // that distinction is the whole point of this arm.
                    warn!(
                        method = %method,
                        error = %reason,
                        "dropped a stream frame this client could not decode"
                    );
                } else {
                    debug!(method = %method, error = %reason, "ignoring notification");
                }
            }
            Inbound::Unrecognized => {
                debug!("Message is not a JSON-RPC frame, ignoring");
            }
        }
    }

    /// Handle a request from Server
    async fn handle_server_request(method: &str, id: Value, write: &WsWriter) {
        warn!(method = %method, "Unknown method from Server");
        let rpc_response = JsonRpcResponse::error(id, JsonRpcError::method_not_found(method));

        // Send response
        let json = match serde_json::to_string(&rpc_response) {
            Ok(j) => j,
            Err(e) => {
                error!("Failed to serialize response: {}", e);
                return;
            }
        };

        debug!("Sending response to Server: {}", json);
        let mut write_guard = write.lock().await;
        if let Err(e) = write_guard.send(Message::Text(json.into())).await {
            error!("Failed to send response: {}", e);
        }
    }

    /// Generate next request ID
    fn next_id(&self) -> String {
        let id = self
            .id_counter
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        id.to_string()
    }

    /// Send a JSON-RPC request and wait for response
    pub async fn call<P: Serialize, R: DeserializeOwned>(
        &self,
        method: &str,
        params: Option<P>,
    ) -> CliResult<R> {
        self.call_with_timeout(method, params, Duration::from_secs(30))
            .await
    }

    /// Send a JSON-RPC request with custom timeout.
    ///
    /// Refuses before sending when the socket is known dead, so a caller gets
    /// an immediate `Disconnected` instead of waiting out `timeout` for a
    /// response that can never arrive.
    ///
    /// That same flag is what keeps callers off a half-built connection during
    /// [`Self::reconnect`]: it stays false across the socket swap and the new
    /// handshake, so a concurrent call fails fast rather than queueing a frame
    /// onto a socket the gateway has not accepted yet — which the gateway
    /// answers by closing it, taking the reconnect down with it.
    pub async fn call_with_timeout<P: Serialize, R: DeserializeOwned>(
        &self,
        method: &str,
        params: Option<P>,
        timeout: Duration,
    ) -> CliResult<R> {
        if !self.connected.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(CliError::Disconnected(
                "Connection closed by peer".to_string(),
            ));
        }
        self.request(method, params, timeout).await
    }

    /// One request/response exchange, without asking whether this client is
    /// usable.
    ///
    /// Split out for exactly one caller — [`Self::handshake`], which has to go
    /// out while `connected` is false. Everything else goes through
    /// [`Self::call_with_timeout`], which asks first.
    async fn request<P: Serialize, R: DeserializeOwned>(
        &self,
        method: &str,
        params: Option<P>,
        timeout: Duration,
    ) -> CliResult<R> {
        let id = self.next_id();
        let params_value = params.map(|p| serde_json::to_value(p)).transpose()?;

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: method.to_string(),
            params: params_value,
            id: Some(Value::String(id.clone())),
        };

        // Serialise BEFORE registering the pending entry: a serde failure
        // here used to leak the oneshot sender (registered next) for the
        // lifetime of the client because the `?` returned while the entry
        // was still in the map.
        let json = serde_json::to_string(&request)?;
        debug!("Sending: {}", json);

        let (tx, rx) = oneshot::channel();

        // Register pending request (only after serialise has succeeded).
        {
            let mut pending = self.pending.write().await;
            pending.insert(id.clone(), PendingRequest { tx });
        }

        // Send request. Hold the write lock only for the send, then release it
        // before touching `pending` so we never nest `write` -> `pending` and
        // invert the lock order used by `handle_message` (`pending` -> `write`).
        let send_result = {
            let mut write = self.write.lock().await;
            write.send(Message::Text(json.into())).await
        };
        if let Err(e) = send_result {
            // Send failed after the pending entry was registered: drop it so
            // the pending map doesn't leak across repeated failures, and mark
            // the connection dead so subsequent calls fail fast.
            self.pending.write().await.remove(&id);
            self.connected
                .store(false, std::sync::atomic::Ordering::SeqCst);
            return Err(e.into());
        }

        // Wait for response with timeout
        let result = match tokio::time::timeout(timeout, rx).await {
            Ok(result) => result,
            Err(e) => {
                // Remove pending request on timeout so the map doesn't leak
                // across repeated failures.
                self.pending.write().await.remove(&id);
                return Err(CliError::Timeout(e.to_string()));
            }
        }
        .map_err(|e| CliError::Disconnected(e.to_string()))?;

        match result {
            Ok(value) => {
                let result: R = serde_json::from_value(value)?;
                Ok(result)
            }
            Err(error) => Err(CliError::Rpc {
                code: error.code,
                message: error.message,
            }),
        }
    }

    /// Perform the `connect` handshake with the server.
    ///
    /// `connect` carries no credential — it only declares a surface identity
    /// (`device_name`) and receives the session baseline back: `{ role,
    /// state_version, keepalive }`. No token is minted, stored, or replayed.
    ///
    /// That is sufficient on loopback, which the gateway resolves to `operator`
    /// before consulting any credential, and it is the reason this client
    /// cannot reach a REMOTE gateway: a non-loopback connection is walled until
    /// it presents a device token, a bootstrap ticket, or the shared gateway
    /// token, and this crate has a surface for none of the three. The role the
    /// server returns is latched into [`Self::role`] either way, so a walled
    /// connection reports what it actually got rather than assuming operator.
    ///
    /// Private on purpose — see [`Self::connect`].
    async fn handshake(&self, config: &CliConfig) -> CliResult<String> {
        #[derive(Serialize)]
        struct ConnectParams<'a> {
            device_name: &'a str,
        }

        #[derive(serde::Deserialize)]
        struct ConnectResult {
            #[serde(default)]
            role: String,
        }

        let params = ConnectParams {
            device_name: &config.device_name,
        };

        // `request`, not `call`: the guard `call` applies is "is this client
        // usable", and during a reconnect handshake it deliberately is not —
        // `connected` stays false until this very exchange succeeds, so that
        // any concurrent caller fails fast instead of queueing a frame onto a
        // socket the gateway has not accepted yet. The handshake is the one
        // exchange that must go out anyway, and the socket under it is live by
        // construction: it was opened three lines ago.
        let result: ConnectResult = self
            .request("connect", Some(params), Duration::from_secs(30))
            .await?;
        Ok(result.role)
    }

    /// The server-assigned role, as of the most recent handshake.
    ///
    /// `"operator"` on loopback; surfaced so `aleph connect` reports what the
    /// server actually granted rather than what the client hoped for.
    ///
    /// Behind a lock rather than latched into a plain field, because
    /// [`Self::reconnect`] handshakes again: a value latched at first connect
    /// would go on reporting what a socket that no longer exists was granted,
    /// and a reconnect is exactly when the answer can differ (a gateway
    /// restarted with a different posture, or a connection that came back
    /// walled). Both handshake paths write it through [`Self::set_role`], so
    /// there is one writer and no second copy to disagree.
    #[must_use]
    pub fn role(&self) -> String {
        self.role.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Record what a handshake just granted.
    fn set_role(&self, role: String) {
        *self.role.lock().unwrap_or_else(|e| e.into_inner()) = role;
    }

    /// Whether the WebSocket connection is still live.
    ///
    /// Reads the atomic the read loop clears when the socket drops (and that
    /// `call()` checks before sending). A pure read of existing state — no I/O,
    /// no server round-trip. Lets a UI reflect a disconnect even while idle.
    pub fn is_connected(&self) -> bool {
        self.connected.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Close the connection
    pub async fn close(&self) -> CliResult<()> {
        let mut write = self.write.lock().await;
        write.send(Message::Close(None)).await?;
        drop(write);
        self.connected
            .store(false, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    /// The gateway rejects and *closes* any connection whose first frame is
    /// not `connect`, so a client that hands back a live handle before
    /// handshaking is useless — every later call dies on a dead socket. This
    /// pins the wire-level invariant that made twenty CLI command modules
    /// silently non-functional when the handshake was a separate opt-in step.
    #[tokio::test]
    async fn connect_handshakes_before_handing_back_the_client() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            // Bounded so a regression that drops the handshake fails the test
            // instead of hanging it: with nothing sent, `next()` never returns.
            let first = tokio::time::timeout(Duration::from_secs(5), ws.next())
                .await
                .expect("client sent no frame — the handshake was skipped")
                .unwrap()
                .unwrap();
            let req: Value = serde_json::from_str(first.to_text().unwrap()).unwrap();
            let reply = serde_json::json!({
                "jsonrpc": "2.0",
                "id": req["id"],
                "result": { "role": "operator" },
            });
            ws.send(Message::Text(reply.to_string().into()))
                .await
                .unwrap();
            req
        });

        let config = CliConfig {
            device_name: "test-device".to_string(),
            ..CliConfig::default()
        };
        let (client, _events) = AlephClient::connect(&format!("ws://{addr}"), &config)
            .await
            .unwrap();

        let first = server.await.unwrap();
        assert_eq!(
            first["method"], "connect",
            "the first frame on the wire must be the handshake"
        );
        assert_eq!(
            first["params"]["device_name"], "test-device",
            "the handshake must carry the configured device name"
        );
        assert_eq!(
            client.role(),
            "operator",
            "the server-assigned role must be latched for callers to read"
        );
    }

    /// Handshake one connection, answering with `role`.
    async fn serve_handshake(
        ws: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
        role: &str,
    ) -> Value {
        let first = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("client sent no frame — the handshake was skipped")
            .unwrap()
            .unwrap();
        let req: Value = serde_json::from_str(first.to_text().unwrap()).unwrap();
        let reply = serde_json::json!({
            "jsonrpc": "2.0",
            "id": req["id"],
            "result": { "role": role },
        });
        ws.send(Message::Text(reply.to_string().into()))
            .await
            .unwrap();
        req
    }

    /// A reconnect rebuilds the socket UNDER the caller, not beside it.
    ///
    /// The receiver handed out at `connect` is the one a long-lived client has
    /// been selecting on since launch; if a reconnect produced a new one, every
    /// frame after the first drop would land somewhere nobody is reading and
    /// the client would look connected and deaf. The replacement read loop is
    /// therefore spawned onto the SAME sender.
    ///
    /// Also pins that the replacement connection handshakes: the gateway closes
    /// any socket whose first frame is not `connect`, so skipping it would
    /// produce a "reconnect" that dies immediately and silently.
    #[tokio::test]
    async fn a_reconnect_feeds_the_receiver_the_caller_already_holds() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            // First socket: handshake, then vanish the way a restarted gateway
            // does.
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            serve_handshake(&mut ws, "operator").await;
            drop(ws);

            // Second socket: handshake again, then push a frame.
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            let req = serve_handshake(&mut ws, "operator").await;
            assert_eq!(
                req["method"], "connect",
                "the replacement socket must handshake too, or the gateway closes it"
            );
            let frame = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "stream.run_accepted",
                "params": {
                    "type": "run_accepted",
                    "run_id": "run-after-reconnect",
                    "session_key": "agent:main:main:s1",
                    "accepted_at": "2026-08-23T10:00:00Z",
                },
            });
            ws.send(Message::Text(frame.to_string().into()))
                .await
                .unwrap();
            // Hold the socket open past the client's read.
            tokio::time::sleep(Duration::from_secs(3)).await;
        });

        let config = CliConfig {
            device_name: "test-device".to_string(),
            ..CliConfig::default()
        };
        let (client, mut events) = AlephClient::connect(&format!("ws://{addr}"), &config)
            .await
            .unwrap();

        // The drop is observed by the read loop, not by the caller, so wait for
        // it rather than assuming a scheduling order.
        for _ in 0..200 {
            if !client.is_connected() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            !client.is_connected(),
            "a socket the server closed must be reported dead"
        );

        client.reconnect(&config).await.expect("reconnect failed");
        assert!(client.is_connected());
        assert_eq!(
            client.role(),
            "operator",
            "the role is re-latched from the new handshake, not carried over"
        );

        let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("the receiver held since launch must still deliver")
            .expect("the event channel must not close across a reconnect");
        match event {
            ClientEvent::Stream(event) => assert_eq!(event.run_id(), "run-after-reconnect"),
            other => panic!("expected a stream event: {other:?}"),
        }

        server.abort();
    }

    /// The "surfaced" half of Task 8a: a topic-event frame sent over a real
    /// socket reaches the caller's public receiver as `ClientEvent::Topic`,
    /// not the CLI's terminal or a log line.
    ///
    /// `classify_frame`'s own tests (`wire_contract`) prove the frame is
    /// *recognised*; they cannot prove it *arrives*, because `handle_message`
    /// — the function that actually forwards it — takes a `write: &WsWriter`
    /// that nothing can construct without a live socket (see this module's
    /// `Inbound` doc). This is that proof: a real gateway-shaped frame, over a
    /// real loopback socket, landing on the exact receiver
    /// `AlephClient::connect` handed back.
    #[tokio::test]
    async fn a_topic_event_frame_reaches_the_public_receiver() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            serve_handshake(&mut ws, "operator").await;

            // The exact wire form `gateway::server::handler::event_wire_form`
            // emits for a topic event (R8-5's fixture).
            let frame = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "event",
                "params": {
                    "topic": "runtime.agents.changed",
                    "data": {"reason": "sampler_flush"},
                },
            });
            ws.send(Message::Text(frame.to_string().into()))
                .await
                .unwrap();
            // Hold the socket open past the client's read.
            tokio::time::sleep(Duration::from_secs(3)).await;
        });

        let config = CliConfig::default();
        let (_client, mut events) = AlephClient::connect(&format!("ws://{addr}"), &config)
            .await
            .unwrap();

        let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("a topic frame on the wire must reach the receiver")
            .expect("the event channel must not close");

        match event {
            ClientEvent::Topic { topic, data } => {
                assert_eq!(topic, "runtime.agents.changed");
                assert_eq!(data["reason"], "sampler_flush");
            }
            other => panic!("expected a topic event, got {other:?}"),
        }

        server.abort();
    }

    /// A request in flight when the socket dies fails NOW, not at its timeout.
    ///
    /// Nothing can answer it — the task that would route the response is the
    /// one noticing the socket is gone — so leaving the caller parked for the
    /// full timeout is thirty seconds of a UI insisting it is still working.
    /// It also strands a reconnect whose handshake was the request in flight.
    #[tokio::test]
    async fn a_dying_socket_fails_the_request_waiting_on_it() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            serve_handshake(&mut ws, "operator").await;
            // Read the next request — so it is registered as pending on the
            // client — and then die without answering it.
            let _ = ws.next().await;
            drop(ws);
            // Keep the listener from being dropped before the client's socket
            // close is delivered.
            tokio::time::sleep(Duration::from_secs(3)).await;
        });

        let config = CliConfig::default();
        let (client, _events) = AlephClient::connect(&format!("ws://{addr}"), &config)
            .await
            .unwrap();

        let started = std::time::Instant::now();
        let err = client
            .call_with_timeout::<_, Value>("noop", None::<()>, Duration::from_secs(20))
            .await
            .expect_err("a request nobody can answer must not succeed");

        assert!(
            matches!(err, CliError::Disconnected(_)),
            "the caller must be told the connection died, not that it timed out: {err:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the caller waited {:?} — the pending map was not cleared when the \
             read loop ended",
            started.elapsed()
        );
    }

    /// A superseded read loop must not report the LIVE connection as dead.
    ///
    /// After a reconnect two loops can be alive: the old one is parked on a
    /// socket nobody writes to any more, and it ends whenever the server
    /// eventually tears that socket down — seconds or minutes later. Without a
    /// generation check it clears `connected` and empties the pending map on
    /// its way out, killing a working client with no error anywhere, triggered
    /// by an event on a socket that was abandoned on purpose.
    ///
    /// Reachable without contrivance: `close()` marks the client disconnected
    /// while its read loop is still parked, which is exactly this shape.
    #[tokio::test]
    async fn a_superseded_read_loop_does_not_kill_the_live_connection() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let (s1, _) = listener.accept().await.unwrap();
            let mut ws1 = tokio_tungstenite::accept_async(s1).await.unwrap();
            serve_handshake(&mut ws1, "operator").await;

            // Deliberately NOT read again: the client's `close()` frame goes
            // unanswered, so its first read loop stays parked and is still
            // alive when the reconnect below supersedes it.
            let (s2, _) = listener.accept().await.unwrap();
            let mut ws2 = tokio_tungstenite::accept_async(s2).await.unwrap();
            serve_handshake(&mut ws2, "operator").await;

            // Now tear down the FIRST socket. Its read loop ends here, after
            // the replacement is already live.
            drop(ws1);

            let frame = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "stream.run_accepted",
                "params": {
                    "type": "run_accepted",
                    "run_id": "run-on-the-live-socket",
                    "session_key": "agent:main:main:s1",
                    "accepted_at": "2026-08-23T10:00:00Z",
                },
            });
            ws2.send(Message::Text(frame.to_string().into()))
                .await
                .unwrap();

            // Answer whatever the client asks next, so the test can prove the
            // connection is still usable and not merely still flagged.
            while let Some(Ok(msg)) = ws2.next().await {
                let Ok(text) = msg.to_text() else { continue };
                let Ok(req) = serde_json::from_str::<Value>(text) else {
                    continue;
                };
                let reply = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": req["id"],
                    "result": { "ok": true },
                });
                if ws2
                    .send(Message::Text(reply.to_string().into()))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });

        let config = CliConfig::default();
        let (client, mut events) = AlephClient::connect(&format!("ws://{addr}"), &config)
            .await
            .unwrap();

        client.close().await.unwrap();
        assert!(!client.is_connected());

        client.reconnect(&config).await.expect("reconnect failed");
        assert!(client.is_connected());

        // Arrives on the live socket, and its arrival means the server has
        // already dropped the first one.
        let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("the live socket must deliver")
            .expect("channel closed");
        match event {
            ClientEvent::Stream(event) => assert_eq!(event.run_id(), "run-on-the-live-socket"),
            other => panic!("expected a stream event: {other:?}"),
        }

        // Give the superseded loop room to finish observing its dead socket.
        tokio::time::sleep(Duration::from_millis(200)).await;

        let answered: Value = client
            .call_with_timeout("noop", None::<()>, Duration::from_secs(5))
            .await
            .expect("the live connection must still serve calls");
        assert_eq!(answered["ok"], true);
        assert!(client.is_connected());
    }
}

/// The wire contract this client is the consuming half of.
///
/// These are the tests that did not exist while `aleph ask` and the TUI were
/// blind to every event a real gateway sent: the parse lived inside
/// `handle_message`, whose `write` argument cannot be built without a socket,
/// so the only coverage of the event path went through a fake server in the
/// module above — and a fake server sends what the code expects, which is
/// precisely the shape the real one did not.
#[cfg(test)]
mod wire_contract {
    use super::{classify_frame, Inbound};
    use aleph_protocol::{JsonRpcRequest, StreamEvent};
    use serde_json::json;

    fn run_accepted_params() -> serde_json::Value {
        json!({
            "type": "run_accepted",
            "run_id": "run-1",
            "session_key": "default",
            "accepted_at": "2026-08-29T00:00:00Z",
        })
    }

    /// Reconciliation: the bytes the gateway emits are built by
    /// `JsonRpcRequest::notification` (`event_bus.rs::publish_frame`), and this
    /// feeds *that constructor's* output to the classifier rather than a
    /// hand-written string. The two halves of the envelope contract live in one
    /// type; a change to it moves both sides at once instead of stranding one.
    #[test]
    fn the_shared_notification_constructor_yields_an_event() {
        let text = serde_json::to_string(&JsonRpcRequest::notification(
            "stream.run_accepted",
            Some(run_accepted_params()),
        ))
        .unwrap();

        match classify_frame(&text) {
            Inbound::Event(event) => match *event {
                StreamEvent::RunAccepted { run_id, .. } => assert_eq!(run_id, "run-1"),
                other => panic!("wrong variant: {other:?}"),
            },
            other => panic!("the gateway's own envelope must classify as an event: {other:?}"),
        }
    }

    /// The regression itself, kept as its own claim.
    ///
    /// `serde_json::from_str::<JsonRpcRequest>` on this exact string fails with
    /// `missing field 'jsonrpc'`, and that failure used to discard the frame
    /// behind one `debug!` line — for `stream.*`, which is to say for the whole
    /// event plane of every client built on this crate. Server conformance is
    /// fixed too, but a client that needs the version tag to route a frame is a
    /// client one careless producer away from going deaf again.
    #[test]
    fn envelope_without_the_version_tag_still_yields_the_event() {
        let text = json!({
            "method": "stream.run_accepted",
            "params": run_accepted_params(),
        })
        .to_string();

        assert!(
            serde_json::from_str::<JsonRpcRequest>(&text).is_err(),
            "this test is only meaningful while the strict parse still rejects \
             these bytes — if it stops, this is asserting nothing"
        );
        assert!(
            matches!(classify_frame(&text), Inbound::Event(_)),
            "a notification missing only the version tag must still route"
        );
    }

    /// A server-initiated request carries BOTH `method` and `id`. Reading `id`
    /// first would file it as a response to a request this client never sent,
    /// which misses the pending map in silence and leaves the server waiting
    /// for a reply that never comes.
    #[test]
    fn a_server_request_is_not_read_as_a_response() {
        let text = json!({
            "jsonrpc": "2.0",
            "method": "sampling/createMessage",
            "id": "srv-1",
            "params": {},
        })
        .to_string();

        match classify_frame(&text) {
            Inbound::ServerRequest { method, id } => {
                assert_eq!(method, "sampling/createMessage");
                assert_eq!(id, json!("srv-1"));
            }
            other => panic!("expected a server request: {other:?}"),
        }
    }

    #[test]
    fn a_result_response_routes_by_id() {
        let text = json!({"jsonrpc": "2.0", "id": "7", "result": {"ok": true}}).to_string();
        match classify_frame(&text) {
            Inbound::Response { id, result } => {
                assert_eq!(id, "7");
                assert_eq!(result.unwrap()["ok"], true);
            }
            other => panic!("expected a response: {other:?}"),
        }
    }

    /// Numeric ids are stringified the same way [`super::AlephClient::next_id`]
    /// mints them, or the reply lands on nothing.
    #[test]
    fn a_numeric_id_matches_the_string_key_the_client_registered() {
        let text = json!({"jsonrpc": "2.0", "id": 7, "error": {"code": -32601, "message": "nope"}})
            .to_string();
        match classify_frame(&text) {
            Inbound::Response { id, result } => {
                assert_eq!(id, "7");
                assert_eq!(result.unwrap_err().code, -32601);
            }
            other => panic!("expected a response: {other:?}"),
        }
    }

    /// A response carrying neither half still has to resolve its caller —
    /// dropping it parks that call until the 30s timeout.
    #[test]
    fn a_response_with_neither_result_nor_error_still_resolves_its_caller() {
        let text = json!({"jsonrpc": "2.0", "id": "9"}).to_string();
        match classify_frame(&text) {
            Inbound::Response { result, .. } => {
                assert!(result.is_err(), "the caller must be told, not left waiting");
            }
            other => panic!("expected a response: {other:?}"),
        }
    }

    /// `stream.*` promises a `StreamEvent` — the server-side census
    /// (`gateway::events::frame_census`) exists to keep that promise — so one
    /// that fails to decode is a broken contract and must be audible. The
    /// `debug!` this replaced is why `stream.clarification_ended` reached the
    /// Panel for months while the TUI rendered a card for a question that had
    /// already ended.
    #[test]
    fn an_undecodable_stream_frame_is_loud() {
        let text = json!({
            "jsonrpc": "2.0",
            "method": "stream.something_new",
            "params": {"type": "something_new"},
        })
        .to_string();

        match classify_frame(&text) {
            Inbound::UnhandledNotification { loud, method, .. } => {
                assert!(loud, "a stream frame this client cannot decode is a defect");
                assert_eq!(method, "stream.something_new");
            }
            other => panic!("expected an unhandled notification: {other:?}"),
        }
    }

    /// …and the topic-event family is not undecodable at all — it is a
    /// recognised, understood frame shape.
    ///
    /// Before Task 8a this classified identically to a frame this client
    /// genuinely could not make sense of: both fell into
    /// `Inbound::UnhandledNotification { loud: false, .. }`, and
    /// `handle_message` dropped both on the floor after one `debug!` line.
    /// "Quiet" and "swallowed" were the same code path, so this test could
    /// not have told them apart. Now a topic frame gets its own `Inbound`
    /// variant carrying the topic/data it actually parsed — the "not a
    /// broken contract" half of this test's original purpose is kept (it is
    /// still never `loud`, because `Inbound::Topic` has no `loud` field to
    /// set), and "…so drop it" is what changes: `handle_message` forwards it
    /// instead (see `a_topic_event_frame_reaches_the_public_receiver` in
    /// `mod tests` for the "reaches the caller" half — proving that needs a
    /// live socket, the same reason `classify_frame` was split out of
    /// `handle_message` in the first place; see this function's own doc).
    #[test]
    fn a_topic_event_notification_is_recognised_and_quiet() {
        let text = json!({
            "method": "event",
            "params": {"topic": "connection.warning", "data": {"reason": "events_overflow"}},
        })
        .to_string();

        match classify_frame(&text) {
            Inbound::Topic { topic, data } => {
                assert_eq!(topic, "connection.warning");
                assert_eq!(data["reason"], "events_overflow");
            }
            other => panic!(
                "a topic event must be recognised as one, not folded into the \
                 generic 'unhandled' bucket a truly undecodable frame falls \
                 into: {other:?}"
            ),
        }
    }

    #[test]
    fn a_non_json_frame_is_ignored_rather_than_panicking() {
        assert!(matches!(classify_frame("not json"), Inbound::Unrecognized));
        assert!(matches!(classify_frame("[]"), Inbound::Unrecognized));
    }
}
