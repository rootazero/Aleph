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
    ) -> CliResult<(Self, mpsc::Receiver<StreamEvent>)> {
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

        tokio::spawn(Self::read_loop(
            read,
            read_pending,
            self.event_tx.clone(),
            self.connected.clone(),
            read_write,
            self.generation.clone(),
            my_generation,
        ));

        self.set_role(self.handshake(config).await?);
        self.connected
            .store(true, std::sync::atomic::Ordering::SeqCst);
        info!("Reconnected to {}", self.url);
        Ok(())
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
        event_tx: mpsc::Sender<StreamEvent>,
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
        event_tx: &mpsc::Sender<StreamEvent>,
        write: &WsWriter,
    ) {
        // Log all incoming messages for debugging.
        // Truncate on a UTF-8 char boundary — a byte slice (`&text[..n]`) panics
        // when byte `n` lands inside a multi-byte char (common with CJK/emoji),
        // which would kill the read loop and silently hang every pending request.
        let preview_end = text.char_indices().nth(500).map_or(text.len(), |(i, _)| i);
        debug!("Received raw message: {}", &text[..preview_end]);

        // Try to parse as response first (response to our request)
        // Only treat as response if id is a valid string or number (not null)
        if let Ok(response) = serde_json::from_str::<JsonRpcResponse>(text) {
            debug!("Parsed as JsonRpcResponse with id: {:?}", response.id);
            let id = match &response.id {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                Value::Null => {
                    // id is null, this is a notification, not a response
                    debug!("Response has null id, treating as notification");
                    // Fall through to try parsing as request
                    String::new()
                }
                _ => return,
            };

            // Only process as response if we have a valid id
            if !id.is_empty() {
                let mut pending_guard = pending.write().await;
                let maybe_req = pending_guard.remove(&id);
                drop(pending_guard);
                if let Some(req) = maybe_req {
                    let result = if let Some(error) = response.error {
                        Err(error)
                    } else if let Some(value) = response.result {
                        Ok(value)
                    } else {
                        Err(JsonRpcError {
                            code: -32600,
                            message: "Invalid Request: missing both result and error".into(),
                            data: None,
                        })
                    };
                    let _ = req.tx.send(result);
                }
                return;
            }
        } else {
            debug!("Message is not a JsonRpcResponse, trying JsonRpcRequest");
        }

        // Try to parse as request (from Server)
        if let Ok(request) = serde_json::from_str::<JsonRpcRequest>(text) {
            // Check if this is a request (has non-null id) or notification (no id or null id)
            let id = match &request.id {
                Some(Value::Null) | None => None, // null/no id means notification
                Some(id) => Some(id.clone()),     // non-null id means request
            };

            if let Some(id) = id {
                // This is a request from Server that needs a response
                debug!(method = %request.method, "Received request from Server");
                Self::handle_server_request(&request, id, write).await;
                return;
            }

            // This is a notification (no response expected)
            if let Some(params) = request.params {
                debug!(method = %request.method, "Received notification");
                match serde_json::from_value::<StreamEvent>(params.clone()) {
                    Ok(event) => {
                        debug!("Parsed event: {:?}", event);
                        let _ = event_tx.send(event).await;
                    }
                    Err(e) => {
                        debug!("Failed to parse event: {} - params: {}", e, params);
                    }
                }
            }
        } else {
            debug!("Message is not a JsonRpcRequest either, ignoring");
        }
    }

    /// Handle a request from Server
    async fn handle_server_request(request: &JsonRpcRequest, id: Value, write: &WsWriter) {
        warn!(method = %request.method, "Unknown method from Server");
        let rpc_response =
            JsonRpcResponse::error(id, JsonRpcError::method_not_found(&request.method));

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
