//! WebSocket client for Aleph Gateway
//!
//! This module provides a JSON-RPC 2.0 client over WebSocket,
//! using only types from `aleph-protocol`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

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
use crate::credential::{ensure_credential_transport, DeviceCredential, VerifiedIdentity};
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
    /// A gateway topic notification, ready for whoever claimed
    /// [`AlephClient::take_topic_events`].
    Topic(TopicEvent),
    /// The envelope parsed but the payload is not a [`StreamEvent`].
    ///
    /// `loud` separates a broken contract from a frame family this client
    /// simply does not consume: every `stream.*` method promises a
    /// `StreamEvent` (`gateway::events::frame_census` asserts that pairing on
    /// the server side). A `{"method":"event",…}` topic frame missing its
    /// `topic`/`data` pair also lands here quietly (well-formed ones are
    /// [`Inbound::Topic`] since the TUI grew a consumer for them).
    UnhandledNotification {
        method: String,
        loud: bool,
        reason: String,
    },
    /// Not a JSON-RPC frame this client understands.
    Unrecognized,
}

/// A gateway topic notification (`{"method":"event","params":{topic,data,…}}`)
/// — the event plane the Panel consumes (`run.subagent_tree`, `team.*`,
/// kanban topics, …), now deliverable to TUI/CLI callers too.
///
/// Deliberately a *view*, not a mirror of the server's carrier: only the two
/// fields every consumer needs, read leniently off the `Value` for the same
/// reason [`classify_frame`] reads its envelope that way — the server-side
/// struct carries fields this client has no use for (`timestamp`,
/// `state_version`), and a strict mirror would zero this plane the day one of
/// them changes shape.
#[derive(Debug, Clone)]
pub struct TopicEvent {
    pub topic: String,
    pub data: Value,
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
/// `{"method": "stream.X", "params": {…}}` and `server/connection/forward.rs::event_wire_form`
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
        if method == TOPIC_EVENT_METHOD {
            if let (Some(topic), Some(data)) = (
                params.get("topic").and_then(Value::as_str),
                params.get("data"),
            ) {
                return Inbound::Topic(TopicEvent {
                    topic: topic.to_string(),
                    data: data.clone(),
                });
            }
            return Inbound::UnhandledNotification {
                method: method.to_string(),
                loud: false,
                reason: "event notification without topic/data".to_string(),
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

/// Method every gateway topic notification carries
/// (`event_bus::TopicEvent::to_notification`).
const TOPIC_EVENT_METHOD: &str = "event";

/// Buffer for the claimed topic receiver. Topic frames are a lossy live layer
/// over snapshot RPCs (the same stance the server takes with its trace
/// mirror), so a slow or absent consumer costs frames, never the socket.
const TOPIC_EVENT_BUFFER: usize = 256;

/// Observable transport continuity. Counters live outside the bounded event
/// channel so a full channel cannot hide the fact that recovery is required.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamContinuity {
    pub generation: u64,
    pub dropped_events: u64,
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
    event_tx: mpsc::Sender<StreamEvent>,
    dropped_events: Arc<std::sync::atomic::AtomicU64>,
    /// Topic-notification sender — same ownership-anchor role as `event_tx`,
    /// for the `{"method":"event"}` plane. Every read loop (initial and each
    /// reconnect replacement) is handed a clone, so a claimed receiver
    /// survives socket swaps exactly like the stream receiver does.
    topic_tx: mpsc::Sender<TopicEvent>,
    /// The unclaimed topic receiver, parked until a caller takes it via
    /// [`AlephClient::take_topic_events`]. Kept alive here on purpose: an
    /// unclaimed plane should cost dropped frames (buffer fills, `try_send`
    /// fails), not a closed-channel error path in the read loop.
    topic_events: Arc<std::sync::Mutex<Option<mpsc::Receiver<TopicEvent>>>>,
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
    /// Opt-in device credential presented on every handshake (initial and
    /// reconnect). `None` for every pre-existing caller. In memory only — see
    /// [`crate::credential`]. After a bootstrap-ticket exchange the one-shot
    /// ticket is replaced here by the device token the Gateway issued, so a
    /// reconnect never replays a consumed ticket.
    credential: Arc<std::sync::Mutex<Option<DeviceCredential>>>,
    /// The identity the Gateway reported for the credential on the most recent
    /// handshake. Cleared on every handshake first, set only when the response
    /// proves the credential path (see [`AlephClient::verified_identity`]).
    identity: Arc<std::sync::Mutex<Option<VerifiedIdentity>>>,
    /// Serialises the entire [`AlephClient::reconnect`] critical section.
    ///
    /// The earlier `is_connected()` early-return and the actual socket swap
    /// were separate steps with a wide window between them: two concurrent
    /// reconnect attempts could each pass the early-return, open a fresh
    /// socket, and install a read loop, leaving the client with two live
    /// generations and the pending map being cleared against the wrong one.
    /// Holding this Tokio mutex from the early-return through the handshake
    /// closes that TOCTOU window.
    reconnect_lock: Arc<Mutex<()>>,
}

impl AlephClient {
    /// Connect to Aleph Gateway and complete the `connect` handshake.
    ///
    /// The handshake is not optional and is deliberately not a separate public
    /// step: the gateway enforces that the first frame on a connection is
    /// `connect` and *closes the socket* otherwise (`server::connection::handle_connection`), so a
    /// client that skipped it could never issue a single working call. Folding
    /// it in here means a caller cannot hold an un-handshaken `AlephClient` —
    /// twenty of twenty-eight CLI command modules used to skip it because the
    /// two steps were separable, and every one of their subcommands was dead
    /// against a live server.
    pub async fn connect(
        url: &str,
        config: &CliConfig,
    ) -> CliResult<(Self, mpsc::Receiver<StreamEvent>)> {
        let (client, events) = Self::open(url, config).await?;
        client.set_role(client.handshake(config).await?);
        Ok((client, events))
    }

    /// [`Self::connect`], presenting `credential` in the handshake.
    ///
    /// The credential is refused before any socket is opened unless the
    /// transport protects it (`wss://`, or `ws://` to a loopback host). The
    /// handshake then fails closed unless the Gateway's response proves the
    /// credential path: `authorized: true` plus a server-reported `device_id`
    /// (a bootstrap exchange must also return the issued `device_token`). A
    /// client-asserted device id is never accepted as proof. The credential is
    /// retained in memory and re-presented by [`Self::reconnect`].
    pub async fn connect_with_credential(
        url: &str,
        config: &CliConfig,
        credential: DeviceCredential,
    ) -> CliResult<(Self, mpsc::Receiver<StreamEvent>)> {
        ensure_credential_transport(url)?;
        let (client, events) = Self::open(url, config).await?;
        *client.credential.lock().unwrap_or_else(|e| e.into_inner()) = Some(credential);
        match client.handshake(config).await {
            Ok(role) => {
                client.set_role(role);
                Ok((client, events))
            }
            Err(e) => {
                // Do not leave a socket (or a retained secret) behind on refusal.
                let _ = client.close().await;
                client
                    .credential
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .take();
                Err(e)
            }
        }
    }

    /// The identity the Gateway reported for this connection's credential, as
    /// of the most recent handshake. `None` when no credential was presented,
    /// or the handshake did not prove one. Server-derived, never client-asserted.
    #[must_use]
    pub fn verified_identity(&self) -> Option<VerifiedIdentity> {
        self.identity
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// The credential this client may persist, as an in-memory clone.
    ///
    /// `Some` only when the credential is a long-lived device token the
    /// Gateway has verified on the most recent handshake (`verified_identity`
    /// names the same device). After a bootstrap-ticket exchange this is the
    /// issued device token, never the consumed ticket; while a ticket is still
    /// held (no successful exchange) it is `None`. Hand the result to
    /// [`crate::DeviceCredentialStore::save`] — never to config or logs.
    #[must_use]
    pub fn credential_for_persistence(&self) -> Option<DeviceCredential> {
        let credential = self
            .credential
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()?;
        credential.persistable_parts()?;
        let identity = self.verified_identity()?;
        (identity.device_id == credential.device_id()).then_some(credential)
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
        // Serialise the entire reconnect critical section: the previous
        // version checked `is_connected()` and then opened the socket in two
        // separate steps, letting two concurrent callers each pass the check
        // and install a fresh read loop. Holding the lock through the whole
        // sequence keeps `is_connected()` and the socket swap atomic with
        // respect to other reconnect callers.
        let _guard = self.reconnect_lock.lock().await;
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
            self.topic_tx.clone(),
            self.dropped_events.clone(),
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
    async fn open(url: &str, config: &CliConfig) -> CliResult<(Self, mpsc::Receiver<StreamEvent>)> {
        info!("Connecting to {}", url);

        let connector = tls::connector_for(url, config.ca_cert.as_deref())?;
        let (ws_stream, _) = connect_async_tls_with_config(url, None, false, connector)
            .await
            .map_err(|e| CliError::Connection(e.to_string()))?;

        let (write, read) = ws_stream.split();

        let (event_tx, event_rx) = mpsc::channel(100);
        let (topic_tx, topic_rx) = mpsc::channel(TOPIC_EVENT_BUFFER);
        let pending = Arc::new(RwLock::new(HashMap::new()));
        let connected = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let write = Arc::new(Mutex::new(write));
        let generation = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let dropped_events = Arc::new(std::sync::atomic::AtomicU64::new(0));

        let client = Self {
            write: write.clone(),
            pending: pending.clone(),
            id_counter: Arc::new(std::sync::atomic::AtomicU64::new(1)),
            event_tx: event_tx.clone(),
            dropped_events: dropped_events.clone(),
            topic_tx: topic_tx.clone(),
            topic_events: Arc::new(std::sync::Mutex::new(Some(topic_rx))),
            connected: connected.clone(),
            url: url.to_string(),
            generation: generation.clone(),
            role: Arc::new(std::sync::Mutex::new(String::new())),
            credential: Arc::new(std::sync::Mutex::new(None)),
            identity: Arc::new(std::sync::Mutex::new(None)),
            reconnect_lock: Arc::new(Mutex::new(())),
        };

        // Spawn read task with write access for responding to Server requests.
        // Generation 0: the first socket, and the one every later reconnect
        // supersedes.
        tokio::spawn(Self::read_loop(
            read,
            pending,
            event_tx,
            topic_tx,
            dropped_events,
            connected,
            write,
            generation,
            0,
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
    // Eight arguments is the loop's full capability set, listed where the two
    // spawn sites can be diffed against it; folding two senders into a struct
    // would trade this lint for a one-consumer type (R10 YAGNI).
    #[allow(clippy::too_many_arguments)]
    async fn read_loop(
        mut read: futures_util::stream::SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>,
        pending: Arc<RwLock<HashMap<String, PendingRequest>>>,
        event_tx: mpsc::Sender<StreamEvent>,
        topic_tx: mpsc::Sender<TopicEvent>,
        dropped_events: Arc<std::sync::atomic::AtomicU64>,
        connected: Arc<std::sync::atomic::AtomicBool>,
        write: WsWriter,
        generation: Arc<std::sync::atomic::AtomicU64>,
        my_generation: u64,
    ) {
        while let Some(msg) = read.next().await {
            if generation.load(std::sync::atomic::Ordering::SeqCst) != my_generation {
                return;
            }
            match msg {
                Ok(Message::Text(text)) => {
                    Self::handle_message(
                        &text,
                        &pending,
                        &event_tx,
                        &topic_tx,
                        &dropped_events,
                        &write,
                    )
                    .await;
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
        event_tx: &mpsc::Sender<StreamEvent>,
        topic_tx: &mpsc::Sender<TopicEvent>,
        dropped_events: &std::sync::atomic::AtomicU64,
        write: &WsWriter,
    ) {
        // Never log the raw frame: the response to `connect` carries the
        // issued `device_token`, and other frames may carry secrets. Size
        // only; the classified arms below log method names.
        debug!(bytes = text.len(), "Received message");

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
                debug!("Parsed stream event");
                // Lossy on backpressure, mirroring the topic channel below:
                // the read loop is the ONLY router of RPC responses on this
                // socket, so it must never `.await` on a downstream consumer
                // that stopped draining — that would wedge every pending
                // request behind a single stalled handler. If the consumer
                // is behind, the event is sacrificed (with a warn) so the
                // connection stays alive; the trade-off is logged loudly
                // enough for a misbehaving consumer to surface in metrics.
                if let Err(e) = event_tx.try_send(*event) {
                    dropped_events.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    warn!(error = %e, "dropping stream event: consumer fell behind, connection stays live");
                }
            }
            Inbound::Topic(event) => {
                // Lossy by design (`try_send`, never `.await`): the topic
                // plane is live decoration over snapshot RPCs, and the read
                // loop must not park behind a consumer that stopped draining
                // — that would stall responses and stream frames too.
                let _ = topic_tx.try_send(event);
            }
            Inbound::UnhandledNotification {
                method,
                loud,
                reason,
            } => {
                if loud {
                    dropped_events.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
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

        debug!(method = %method, "Sending method-not-found response to Server");
        let mut write_guard = write.lock().await;
        if let Err(e) = write_guard.send(Message::Text(json.into())).await {
            error!("Failed to send response: {}", e);
        }
    }

    /// Claim the topic-notification receiver — the `{"method":"event"}` plane
    /// (`run.subagent_tree`, kanban topics, …) that this client used to drop
    /// on the floor with a `debug!` line.
    ///
    /// One claimant per client: the first call returns the receiver, every
    /// later call `None`. The receiver survives [`AlephClient::reconnect`]
    /// (each replacement read loop is handed the same sender). Unclaimed or
    /// lagging, the plane degrades by dropping frames (see `TOPIC_EVENT_BUFFER`)
    /// — consumers own their own cold-start snapshot RPC and treat topics as
    /// live deltas over it.
    pub fn take_topic_events(&self) -> Option<mpsc::Receiver<TopicEvent>> {
        self.topic_events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }

    /// Snapshot transport continuity without waiting on the event queue.
    #[must_use]
    pub fn stream_continuity(&self) -> StreamContinuity {
        StreamContinuity {
            generation: self.generation.load(std::sync::atomic::Ordering::SeqCst),
            dropped_events: self
                .dropped_events
                .load(std::sync::atomic::Ordering::SeqCst),
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
        // Never log the serialised request: `connect` params carry
        // `device_token` / `bootstrap_ticket`. Method + id + size suffice.
        debug!(method = %method, id = %id, bytes = json.len(), "Sending request");

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
    /// By default `connect` carries no credential — it only declares a surface
    /// identity (`device_name`) and receives the session baseline back: `{ role,
    /// state_version, keepalive }`. No token is minted, stored, or replayed.
    /// Only a client built through [`Self::connect_with_credential`] adds a
    /// [`DeviceCredential`]; the response must then prove the credential path
    /// or the handshake fails closed.
    ///
    /// Credential-less is sufficient on loopback, which the gateway resolves to
    /// `operator` when no credential is presented, and it is the reason a
    /// credential-less client cannot reach a REMOTE gateway: a non-loopback
    /// connection is walled until it presents a device token, a bootstrap
    /// ticket, or the shared gateway token. The role the
    /// server returns is latched into [`Self::role`] either way, so a walled
    /// connection reports what it actually got rather than assuming operator.
    ///
    /// Private on purpose — see [`Self::connect`].
    async fn handshake(&self, config: &CliConfig) -> CliResult<String> {
        #[derive(serde::Deserialize)]
        struct ConnectResult {
            #[serde(default)]
            role: String,
            #[serde(default)]
            authorized: Option<bool>,
            #[serde(default)]
            device_id: Option<String>,
            #[serde(default)]
            user_id: Option<String>,
            #[serde(default)]
            device_token: Option<String>,
        }

        // Whatever an earlier handshake proved says nothing about this socket.
        self.identity
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        let credential = self
            .credential
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();

        let params = build_connect_params(&config.device_name, credential.as_ref());

        // `request`, not `call`: the guard `call` applies is "is this client
        // usable", and during a reconnect handshake it deliberately is not —
        // `connected` stays false until this very exchange succeeds, so that
        // any concurrent caller fails fast instead of queueing a frame onto a
        // socket the gateway has not accepted yet. The handshake is the one
        // exchange that must go out anyway, and the socket under it is live by
        // construction: it was opened three lines ago.
        let result: ConnectResult = self
            .request(
                "connect",
                Some(Value::Object(params)),
                Duration::from_secs(30),
            )
            .await?;

        if let Some(credential) = credential {
            // Fail closed: a credential was presented, so the response must
            // prove the Gateway took the credential path. A loopback gateway
            // that ignored it would still answer `authorized: true` with the
            // operator role; the server-reported `device_id` is what tells the
            // two apart, and it must be the device this credential was minted
            // for (a client-asserted id is not proof).
            let verified = verify_credential_response(
                &credential,
                result.authorized,
                result.device_id,
                result.user_id,
                result.device_token.as_deref(),
            )?;
            if credential.is_bootstrap_ticket() {
                // `verify_credential_response` guaranteed a token. The ticket is
                // one-shot and now spent; swap in the issued token (memory only)
                // so reconnects authenticate without replaying it.
                if let Some(token) = result.device_token {
                    *self.credential.lock().unwrap_or_else(|e| e.into_inner()) =
                        Some(DeviceCredential::device_token(&verified.device_id, token)?);
                }
            }
            *self.identity.lock().unwrap_or_else(|e| e.into_inner()) = Some(verified);
        }
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

/// Build the `connect` params: the surface name, plus the credential fields
/// when one is presented.
fn build_connect_params(
    device_name: &str,
    credential: Option<&DeviceCredential>,
) -> serde_json::Map<String, Value> {
    let mut params = serde_json::Map::new();
    params.insert(
        "device_name".to_string(),
        Value::String(device_name.to_string()),
    );
    if let Some(credential) = credential {
        credential.write_connect_params(&mut params);
    }
    params
}

/// Decide whether a connect response proves the presented `credential` was
/// validated by the Gateway. Pure so the fail-closed rules are unit-testable.
fn verify_credential_response(
    credential: &DeviceCredential,
    authorized: Option<bool>,
    device_id: Option<String>,
    user_id: Option<String>,
    issued_device_token: Option<&str>,
) -> CliResult<VerifiedIdentity> {
    if authorized != Some(true) {
        return Err(CliError::Connection(
            "gateway did not authorize the device credential".into(),
        ));
    }
    let Some(device_id) = device_id.filter(|d| !d.is_empty()) else {
        return Err(CliError::Connection(
            "gateway did not confirm a device identity for the credential".into(),
        ));
    };
    if device_id != credential.device_id() {
        return Err(CliError::Connection(
            "gateway resolved the credential to a different device than expected".into(),
        ));
    }
    if credential.is_bootstrap_ticket() && issued_device_token.is_none_or(str::is_empty) {
        return Err(CliError::Connection(
            "gateway accepted the bootstrap ticket but returned no device token".into(),
        ));
    }
    Ok(VerifiedIdentity { device_id, user_id })
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

    /// A full stream queue must report a continuity gap without parking the
    /// only reader that can deliver an RPC response. No files or credentials
    /// are touched: both peers use an ephemeral plaintext loopback socket.
    #[tokio::test]
    async fn full_stream_queue_reports_loss_and_still_delivers_rpc_reply() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            serve_handshake(&mut ws, "operator").await;
            let request = ws.next().await.unwrap().unwrap();
            let request: Value = serde_json::from_str(request.to_text().unwrap()).unwrap();
            for _ in 0..101 {
                let event = serde_json::json!({
                    "method": "stream.response_chunk",
                    "params": { "type": "response_chunk", "run_id": "busy", "seq": 1,
                        "content": "x", "chunk_index": 0, "is_final": false }
                });
                ws.send(Message::Text(event.to_string().into()))
                    .await
                    .unwrap();
            }
            ws.send(Message::Text(
                serde_json::json!({
                    "jsonrpc": "2.0", "id": request["id"], "result": { "alive": true }
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
            // Wait for the client's explicit close; do not make the result
            // depend on the reader winning a race with socket teardown.
            while let Some(Ok(message)) = ws.next().await {
                if message.is_close() {
                    break;
                }
            }
        });
        let (client, mut events) =
            AlephClient::connect(&format!("ws://{addr}"), &CliConfig::default())
                .await
                .unwrap();
        let before = client.stream_continuity();
        let response: Value = client
            .call_with_timeout("probe", None::<()>, Duration::from_secs(5))
            .await
            .expect("a full event queue must not block an RPC response");
        assert_eq!(response["alive"], true);
        let after = client.stream_continuity();
        assert_eq!(after.generation, before.generation);
        assert_eq!(after.dropped_events, before.dropped_events + 1);
        assert_eq!(events.len(), 100);
        assert_eq!(events.recv().await.unwrap().run_id(), "busy");
        client.close().await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
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
        assert_eq!(event.run_id(), "run-after-reconnect");

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
        assert_eq!(event.run_id(), "run-on-the-live-socket");

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

    /// …and the topic-event family decodes into [`Inbound::Topic`] — the
    /// plane the TUI's agents/tasks views consume (`run.subagent_tree`, …).
    /// Before this arm existed, every one of these frames died here as a
    /// quiet `UnhandledNotification` while the server pushed them faithfully.
    #[test]
    fn a_topic_event_notification_decodes_into_topic() {
        let text = json!({
            "method": "event",
            "params": {"topic": "run.subagent_tree", "data": {"kind": "spawned"}, "timestamp": 1},
        })
        .to_string();

        match classify_frame(&text) {
            Inbound::Topic(ev) => {
                assert_eq!(ev.topic, "run.subagent_tree");
                assert_eq!(ev.data["kind"], "spawned");
            }
            other => panic!("expected a topic event: {other:?}"),
        }
    }

    /// A topic frame missing its `topic`/`data` pair stays a QUIET unhandled
    /// notification (it is not a `stream.*` contract break; warning on each
    /// would bury the loud arm above in noise).
    #[test]
    fn a_malformed_topic_event_notification_is_quiet() {
        let text = json!({
            "method": "event",
            "params": {"topic": "connection.warning"},
        })
        .to_string();

        match classify_frame(&text) {
            Inbound::UnhandledNotification { loud, .. } => {
                assert!(
                    !loud,
                    "a malformed topic frame is not a stream contract break"
                )
            }
            other => panic!("expected an unhandled notification: {other:?}"),
        }
    }

    #[test]
    fn a_non_json_frame_is_ignored_rather_than_panicking() {
        assert!(matches!(classify_frame("not json"), Inbound::Unrecognized));
        assert!(matches!(classify_frame("[]"), Inbound::Unrecognized));
    }
}
