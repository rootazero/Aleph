//! The desktop side of the TSX Panel's `nsh/host` contract
//! (`interfaces/panel/web/host.js` is the Web side; both answer the same
//! functions with the same shapes):
//!
//! - `aleph.rpc` `{ method, params, voice_binding? }` -> `{ result }` | `{ error: { code, message } }`
//!   Voice RPCs carry a host-only binding to the current authenticated link.
//! - `aleph.pair` -> an error: Desktop IPC is authenticated by the operating
//!   system, so the desktop shell never asks for pairing
//! - `aleph.connect` -> opens a new Desktop IPC connection (after a drop)
//! - `aleph.connection` -> the current connection state
//! - event `aleph.connection` `{ state, endpoint, subject, pairing, voice_binding? }`
//! - `aleph.voice.availability` describes the native mount, not permission or STT setup
//! - event `aleph.voice.visibility` `{ visible }`
//! - event `aleph.event` `{ topic, data }`: a live Gateway event (negotiated
//!   `live_events`), or the host's own `connection.stale` when the client
//!   dropped frames — the Panel then refetches instead of trusting its view
//!
//! The desktop Panel reaches only a local Gateway over Desktop IPC, never a
//! WebSocket: any other target reports `unavailable`.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

use aleph_client::{AlephClient, CliError, CliResult, TopicEvent};
use aleph_protocol::panel::PANEL_HOST_UNAVAILABLE;
use aleph_protocol::system_hello::{ClientHello, Feature, HelloParams, ProtocolVersion};
use aleph_protocol::StreamEvent;
use nsh_engine::{HostCall, HostChannel};
use serde_json::{json, Value};
use tokio::sync::{mpsc, OwnedSemaphorePermit, Semaphore};
use uuid::Uuid;

mod voice;
use voice::{CaptureOwner, VoiceSlots};

/// The Desktop IPC socket of an already-running Gateway (an absolute path).
pub(crate) const IPC_ENV: &str = "ALEPH_SHELL_PANEL_IPC";

/// The endpoint the Panel keys its saved layout by. Not the socket path: the
/// Gateway binds a fresh path on every start, and the layout must survive one.
const ENDPOINT: &str = "desktop-ipc";

const VOICE_AVAILABILITY: &str = "aleph.voice.availability";
const VOICE_VISIBILITY: &str = "aleph.voice.visibility";
const FUNCTIONS: [&str; 5] = [
    "aleph.rpc",
    "aleph.pair",
    "aleph.connect",
    "aleph.connection",
    VOICE_AVAILABILITY,
];

const CALL_TIMEOUT: Duration = Duration::from_secs(30);
// Includes the active ordinary call and callers queued behind the turn lock.
const ORDINARY_RPC_SLOTS: usize = 4;
// The Core capture may run for 45 seconds plus bounded teardown.
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(52);

/// How often an idle connection is checked for loss. Without it a Panel
/// waiting on events would learn of a Gateway restart only at its next call.
const LIVENESS_TICK: Duration = Duration::from_secs(1);

/// Where the desktop Panel's calls go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    Local(PathBuf),
    Unavailable(&'static str),
}

impl Target {
    /// `explicit_connect`: the shell was launched with `--connect`, a
    /// WebSocket target the TSX Panel does not serve.
    pub(crate) fn resolve(explicit_connect: bool, ipc: Option<OsString>) -> Target {
        if explicit_connect {
            return Target::Unavailable(
                "the TSX Panel reaches only a local Gateway over Desktop IPC",
            );
        }
        if !cfg!(unix) {
            return Target::Unavailable("Desktop IPC is unavailable on this platform");
        }
        let Some(path) = ipc.map(PathBuf::from) else {
            return Target::Unavailable("ALEPH_SHELL_PANEL_IPC is not set");
        };
        if !path.is_absolute() || path.file_name().is_none() {
            return Target::Unavailable("ALEPH_SHELL_PANEL_IPC must be an absolute socket path");
        }
        Target::Local(path)
    }
}

/// Serves the Panel's host functions while the surface lives.
pub(crate) struct Handle {
    shared: Arc<Shared>,
    runtime: Option<tokio::runtime::Runtime>,
}

struct Shared {
    host: HostChannel,
    target: Target,
    runtime: tokio::runtime::Handle,
    link: Mutex<Link>,
    playback_admission: Arc<crate::voice_playback::admission::Admission>,
    on_lost: Box<dyn Fn() + Send + Sync>,
    /// Generic calls take turns here. Capture controls must reach Core while
    /// the long capture RPC is in flight.
    turn: tokio::sync::Mutex<()>,
    voice_slots: VoiceSlots,
    ordinary_slots: Arc<Semaphore>,
}

enum AdmittedRpc {
    Ordinary(OwnedSemaphorePermit),
    Voice(voice::AdmittedVoice),
}

fn take_rpc_fields(mut data: Value) -> (String, Option<Value>) {
    let fields = data.as_object_mut().expect("admitted RPC envelope");
    let Some(Value::String(method)) = fields.remove("method") else {
        unreachable!("admitted RPC method");
    };
    let params = fields.remove("params").filter(|value| !value.is_null());
    (method, params)
}

struct Link {
    state: &'static str,
    subject: Option<String>,
    client: Option<Arc<AlephClient>>,
    /// Bumped by every connect, so an older attempt's late outcome is ignored.
    attempt: u64,
    closed: bool,
    capture: Option<CaptureOwner>,
    last_capture: u64,
    voice_binding: Option<Uuid>,
}

impl Link {
    fn install_client(&mut self, client: Arc<AlephClient>) {
        self.state = "connected";
        self.subject = client.hello_subject();
        self.client = Some(client);
        self.voice_binding = Some(Uuid::new_v4());
        self.capture = None;
    }
}

impl Handle {
    pub(crate) fn start(
        host: HostChannel,
        target: Target,
        visible: bool,
        playback_admission: Arc<crate::voice_playback::admission::Admission>,
        on_lost: impl Fn() + Send + Sync + 'static,
    ) -> std::io::Result<Handle> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("aleph-panel-host")
            .enable_all()
            .build()?;
        let state = match target {
            Target::Local(_) => "connecting",
            Target::Unavailable(_) => "unavailable",
        };
        playback_admission.set_visible(visible);
        let shared = Arc::new(Shared {
            host,
            target,
            runtime: runtime.handle().clone(),
            link: Mutex::new(Link {
                state,
                subject: None,
                client: None,
                attempt: 0,
                closed: false,
                capture: None,
                voice_binding: None,
                last_capture: 0,
            }),
            playback_admission,
            on_lost: Box::new(on_lost),
            turn: tokio::sync::Mutex::new(()),
            voice_slots: VoiceSlots::default(),
            ordinary_slots: Arc::new(Semaphore::new(ORDINARY_RPC_SLOTS)),
        });
        for name in FUNCTIONS {
            let handler = Arc::downgrade(&shared);
            shared
                .host
                .handle(name, move |call| match handler.upgrade() {
                    Some(shared) => shared.dispatch(call),
                    None => call.reject("the Panel host has stopped"),
                });
        }
        shared.connect();
        Ok(Handle {
            shared,
            runtime: Some(runtime),
        })
    }
}

impl Handle {
    pub(crate) fn set_visible(&self, visible: bool) {
        let cancel = {
            let mut link = self.shared.link();
            if link.closed || !self.shared.playback_admission.set_visible(visible) {
                return;
            }
            self.shared.publish_visibility(visible);
            if visible {
                None
            } else {
                link.capture
                    .and_then(|capture| link.start_capture_cleanup(capture))
            }
        };
        if let Some(cleanup) = cancel {
            let shared = self.shared.clone();
            self.shared.runtime.spawn(async move {
                shared.clean_capture(cleanup).await;
            });
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        {
            let mut link = self.shared.link();
            link.closed = true;
            self.shared.playback_admission.disconnected();
            if let Some(client) = link.client.take() {
                client.retire_panel_connection();
            }
        }
        for name in FUNCTIONS {
            self.shared.host.remove_handler(name);
        }
        // The surface drops on the UI thread: stop without waiting. In-flight
        // calls are cancelled, which drops their clones of the client.
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

impl Shared {
    fn link(&self) -> MutexGuard<'_, Link> {
        self.link
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    fn dispatch(self: Arc<Self>, mut call: HostCall) {
        match call.name() {
            VOICE_AVAILABILITY => {
                // Order the new subscriber's snapshot with live visibility edges.
                {
                    let link = self.link();
                    if !link.closed {
                        self.publish_visibility(self.playback_admission.visibility().0);
                    }
                }
                call.resolve(
                    json!({"capture": true, "playback": true, "capture_transport": "desktop"}),
                );
            }
            "aleph.connection" => {
                let state = self.connection();
                call.resolve(state);
            }
            "aleph.connect" => {
                self.connect();
                call.resolve(Value::Null);
            }
            "aleph.pair" => call.resolve(error_reply(
                PANEL_HOST_UNAVAILABLE,
                "the desktop shell is authenticated by the operating system and does not pair",
            )),
            "aleph.rpc" => {
                let data = call.take_data();
                self.start_rpc(data, move |reply| call.resolve(reply));
            }
            other => {
                let message = format!("no host function {other}");
                call.reject(message);
            }
        }
    }

    fn connection(&self) -> Value {
        let link = self.link();
        let mut state = connection_state(link.state, link.subject.as_deref());
        if let Some(binding) = link.voice_binding {
            state["voice_binding"] = json!(binding);
        }
        state
    }

    fn publish_visibility(&self, visible: bool) {
        let _ = self
            .host
            .send(VOICE_VISIBILITY, json!({ "visible": visible }));
    }

    fn publish(&self) {
        let _ = self.host.send("aleph.connection", self.connection());
    }

    fn connect(self: &Arc<Self>) {
        let Target::Local(path) = &self.target else {
            self.publish();
            return;
        };
        let attempt = {
            let mut link = self.link();
            let pending = link.state == "connecting" && link.attempt > 0;
            if link.closed || pending || link.state == "connected" {
                return;
            }
            link.attempt += 1;
            link.state = "connecting";
            link.subject = None;
            link.client = None;
            link.attempt
        };
        self.publish();
        let shared = self.clone();
        let path = path.clone();
        self.runtime.spawn(async move {
            let outcome = AlephClient::connect_local(&path, &hello()).await;
            let mut link = shared.link();
            if link.closed || link.attempt != attempt {
                return;
            }
            match outcome {
                Ok((client, events)) => {
                    let topics = client.take_topic_events();
                    let client = Arc::new(client);
                    link.install_client(client.clone());
                    shared.playback_admission.connected(attempt, client.clone());
                    shared.runtime.spawn(forward_events(
                        Arc::downgrade(&shared),
                        Arc::downgrade(&client),
                        attempt,
                        events,
                        topics,
                    ));
                }
                Err(error) => {
                    eprintln!("NSH TSX Panel Desktop IPC connect failed: {error}");
                    link.state = "disconnected";
                }
            }
            drop(link);
            shared.publish();
        });
    }

    /// Whether `attempt` still owns the link: a replaced or closed connection
    /// must not deliver events into the Panel's current generation.
    fn is_current(&self, attempt: u64) -> bool {
        let link = self.link();
        !link.closed && link.attempt == attempt && link.client.is_some()
    }

    /// The client is gone: report `disconnected` once (unless a newer
    /// connection has replaced it).
    fn lost(&self, client: &Arc<AlephClient>) {
        let mut link = self.link();
        let current = link
            .client
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, client));
        if !current || link.closed {
            return;
        }
        // Invalidate ingress synchronously and enqueue teardown before a
        // newer connection can take ownership.
        self.playback_admission.disconnected();
        link.client = None;
        link.capture = None;
        link.voice_binding = None;
        link.state = "disconnected";
        (self.on_lost)();
        drop(link);
        self.publish();
    }

    fn start_rpc(self: Arc<Self>, data: Value, resolve: impl FnOnce(Value) + Send + 'static) {
        let admission = match self.admit_rpc(&data) {
            Ok(admission) => admission,
            Err(reply) => return resolve(reply),
        };
        // The bridge envelope can contain ignored fields. Keep only the
        // wire-sized method/params and the permit in the spawned task.
        let (method, params) = take_rpc_fields(data);
        let runtime = self.runtime.clone();
        runtime.spawn(async move {
            resolve(self.dispatch_rpc(&method, params.as_ref(), admission).await)
        });
    }

    fn admit_rpc(&self, data: &Value) -> Result<AdmittedRpc, Value> {
        let method = data
            .get("method")
            .and_then(Value::as_str)
            .ok_or_else(|| error_reply(-32602, "aleph.rpc needs { method, params }"))?;
        let binding = data.get("voice_binding");
        let params = data.get("params").filter(|value| !value.is_null());
        if method.starts_with("voice.turn.")
            || (matches!(method, "chat.send" | "chat.abort") && binding.is_some())
        {
            return self
                .admit_voice_rpc(method, params, binding)
                .map(AdmittedRpc::Voice);
        }
        let permit = self
            .ordinary_slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| error_reply(PANEL_HOST_UNAVAILABLE, "the ordinary RPC queue is full"))?;
        if !AlephClient::panel_rpc_request_fits(method, params) {
            return Err(error_reply(
                PANEL_HOST_UNAVAILABLE,
                "ordinary RPC exceeds its wire budget",
            ));
        }
        Ok(AdmittedRpc::Ordinary(permit))
    }

    #[cfg(test)]
    async fn rpc(&self, data: Value) -> Value {
        match self.admit_rpc(&data) {
            Ok(admission) => {
                let (method, params) = take_rpc_fields(data);
                self.dispatch_rpc(&method, params.as_ref(), admission).await
            }
            Err(reply) => reply,
        }
    }

    async fn dispatch_rpc(
        &self,
        method: &str,
        params: Option<&Value>,
        admission: AdmittedRpc,
    ) -> Value {
        let _permit = match admission {
            AdmittedRpc::Voice(voice) => return self.voice_rpc(method, params, voice).await,
            AdmittedRpc::Ordinary(permit) => permit,
        };
        let _turn = self.turn.lock().await;
        let client = self.link().client.clone();
        let Some(client) = client else {
            return error_reply(PANEL_HOST_UNAVAILABLE, "the Gateway is not connected");
        };
        let result = client
            .call_with_timeout::<_, Value>(method, params, CALL_TIMEOUT)
            .await;
        if !client.is_connected() {
            self.lost(&client);
        }
        rpc_reply(result)
    }
}

/// Forwards one connection's live events to the Panel until the connection
/// is replaced, lost, or the host stops. Holds only weak references, so it
/// never keeps a dropped client (and with it the event channels) alive.
async fn forward_events(
    shared: Weak<Shared>,
    client: Weak<AlephClient>,
    attempt: u64,
    mut stream: mpsc::Receiver<StreamEvent>,
    mut topics: Option<mpsc::Receiver<TopicEvent>>,
) {
    let mut reported_drops = 0;
    let mut ticks = tokio::time::interval(LIVENESS_TICK);
    loop {
        let event = tokio::select! {
            Some(event) = stream.recv() => stream_event_payload(&event),
            Some(event) = next_topic(&mut topics) => Some(topic_event_payload(event)),
            _ = ticks.tick() => None,
        };
        let (Some(shared), Some(client)) = (shared.upgrade(), client.upgrade()) else {
            return;
        };
        if !shared.is_current(attempt) {
            return;
        }
        // The client counts frames its full queue refused. Any new loss
        // makes the Panel's view untrustworthy: say so before the next event.
        let drops = client.stream_continuity().dropped_events;
        if drops > reported_drops {
            reported_drops = drops;
            let stale = json!({ "topic": "connection.stale", "data": { "dropped": drops } });
            let _ = shared.host.send("aleph.event", stale);
        }
        if let Some(event) = event {
            let _ = shared.host.send("aleph.event", event);
        }
        if !client.is_connected() {
            shared.lost(&client);
            return;
        }
    }
}

async fn next_topic(topics: &mut Option<mpsc::Receiver<TopicEvent>>) -> Option<TopicEvent> {
    match topics {
        Some(topics) => topics.recv().await,
        None => std::future::pending().await,
    }
}

/// `{ topic, data }` for a stream event: the topic is its wire method
/// (`stream.<type>`) and the data its wire params, including `type`.
fn stream_event_payload(event: &StreamEvent) -> Option<Value> {
    let data = serde_json::to_value(event).ok()?;
    let topic = format!("stream.{}", data.get("type")?.as_str()?);
    Some(json!({ "topic": topic, "data": data }))
}

fn topic_event_payload(event: TopicEvent) -> Value {
    json!({ "topic": event.topic, "data": event.data })
}

fn hello() -> HelloParams {
    HelloParams {
        protocol: ProtocolVersion { major: 1, minor: 0 },
        client: ClientHello {
            name: "aleph-panel".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            platform: std::env::consts::OS.into(),
            ui_runtime: "nsh".into(),
        },
        features: vec![Feature::LiveEvents, Feature::ChatTranscript],
    }
}

fn connection_state(state: &str, subject: Option<&str>) -> Value {
    json!({ "state": state, "endpoint": ENDPOINT, "subject": subject, "pairing": false })
}

fn error_reply(code: i32, message: &str) -> Value {
    json!({ "error": { "code": code, "message": message } })
}

/// A Gateway error keeps its code; a transport failure is the host's own
/// `PANEL_HOST_UNAVAILABLE`, as on the Web.
fn rpc_reply(result: CliResult<Value>) -> Value {
    match result {
        Ok(result) => json!({ "result": result }),
        Err(CliError::Rpc {
            code,
            message,
            data,
        }) => {
            let mut reply = error_reply(code, &message);
            if let Some(data) = data {
                reply["error"]["data"] = data;
            }
            reply
        }
        Err(other) => error_reply(PANEL_HOST_UNAVAILABLE, &other.to_string()),
    }
}

#[cfg(test)]
mod voice_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rpc_handler_maps_client_error_to_error_reply() {
        assert_eq!(
            rpc_reply(Ok(json!({ "status": "healthy" }))),
            json!({ "result": { "status": "healthy" } })
        );
        assert_eq!(rpc_reply(Ok(Value::Null)), json!({ "result": null }));
        assert_eq!(
            rpc_reply(Err(CliError::Rpc {
                data: None,
                code: -32601,
                message: "method unavailable".into(),
            })),
            json!({ "error": { "code": -32601, "message": "method unavailable" } })
        );
        let lost = rpc_reply(Err(CliError::Disconnected(
            "Connection closed by peer".into(),
        )));
        assert_eq!(lost["error"]["code"], json!(PANEL_HOST_UNAVAILABLE));
    }

    /// The Panel sees the same `{ topic, data }` for a stream frame as the
    /// Web host builds from the wire: topic `stream.<type>`, data the params.
    #[test]
    fn live_events_reach_the_panel_as_topic_and_wire_params() {
        let chunk: StreamEvent = serde_json::from_value(json!({
            "type": "run_accepted", "run_id": "r-1", "session_key": "k", "accepted_at": "t"
        }))
        .unwrap();
        assert_eq!(
            stream_event_payload(&chunk).unwrap(),
            json!({ "topic": "stream.run_accepted", "data": {
                "type": "run_accepted", "run_id": "r-1", "session_key": "k", "accepted_at": "t"
            } })
        );
        let warning = TopicEvent {
            topic: "connection.warning".into(),
            data: json!({ "reason": "events_overflow" }),
        };
        assert_eq!(
            topic_event_payload(warning),
            json!({ "topic": "connection.warning", "data": { "reason": "events_overflow" } })
        );
        assert!(hello().features.contains(&Feature::LiveEvents));
    }

    #[test]
    fn rich_tools_use_the_same_typed_notification_as_text() {
        let offered = serde_json::to_value(hello()).unwrap();
        assert!(offered["features"]
            .as_array()
            .unwrap()
            .contains(&json!("chat_transcript")));
        let payload = json!({
            "type": "chat_tool", "run_id": "r-1", "seq": 2,
            "tool_call_id": "call-1", "change": {"kind": "working"}
        });
        let event: StreamEvent = serde_json::from_value(payload.clone()).unwrap();
        assert_eq!(
            stream_event_payload(&event).unwrap(),
            json!({
                "topic": "stream.chat_tool", "data": payload
            })
        );
    }

    #[test]
    fn remote_target_reports_unavailable() {
        let socket = std::env::temp_dir().join("aleph.sock");
        assert!(matches!(
            Target::resolve(true, Some(socket.clone().into())),
            Target::Unavailable(_)
        ));
        assert!(matches!(
            Target::resolve(false, None),
            Target::Unavailable(_)
        ));
        assert!(matches!(
            Target::resolve(false, Some("aleph.sock".into())),
            Target::Unavailable(_)
        ));
        if cfg!(unix) {
            assert_eq!(
                Target::resolve(false, Some(socket.clone().into())),
                Target::Local(socket)
            );
        }

        let host = HostChannel::default();
        let handle = Handle::start(
            host,
            Target::resolve(true, None),
            true,
            Arc::default(),
            || {},
        )
        .unwrap();
        assert_eq!(
            handle.shared.connection(),
            json!({ "state": "unavailable", "endpoint": "desktop-ipc", "subject": null, "pairing": false })
        );
    }

    #[test]
    fn rpc_without_a_connection_is_host_unavailable() {
        let handle = Handle::start(
            HostChannel::default(),
            Target::resolve(true, None),
            true,
            Arc::default(),
            || {},
        )
        .unwrap();
        let reply = handle
            .shared
            .runtime
            .block_on(handle.shared.rpc(json!({ "method": "health" })));
        assert_eq!(reply["error"]["code"], json!(PANEL_HOST_UNAVAILABLE));
        let malformed = handle.shared.runtime.block_on(handle.shared.rpc(json!({})));
        assert_eq!(malformed["error"]["code"], json!(-32602));
    }
}
