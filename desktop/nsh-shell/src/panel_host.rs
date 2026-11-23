//! The desktop side of the TSX Panel's `nsh/host` contract
//! (`interfaces/panel/web/host.js` is the Web side; both answer the same
//! functions with the same shapes):
//!
//! - `aleph.rpc` `{ method, params }` -> `{ result }` | `{ error: { code, message } }`
//! - `aleph.pair` -> an error: Desktop IPC is authenticated by the operating
//!   system, so the desktop shell never asks for pairing
//! - `aleph.connect` -> opens a new Desktop IPC connection (after a drop)
//! - `aleph.connection` -> the current connection state
//! - event `aleph.connection` `{ state, endpoint, subject, pairing }`
//!
//! The desktop Panel reaches only a local Gateway over Desktop IPC, never a
//! WebSocket: any other target reports `unavailable`.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use aleph_client::{AlephClient, CliError, CliResult};
use aleph_protocol::panel::PANEL_HOST_UNAVAILABLE;
use aleph_protocol::system_hello::{ClientHello, HelloParams, ProtocolVersion};
use nsh_engine::{HostCall, HostChannel};
use serde_json::{json, Value};

/// The Desktop IPC socket of an already-running Gateway (an absolute path).
pub(crate) const IPC_ENV: &str = "ALEPH_SHELL_PANEL_IPC";

/// The endpoint the Panel keys its saved layout by. Not the socket path: the
/// Gateway binds a fresh path on every start, and the layout must survive one.
const ENDPOINT: &str = "desktop-ipc";

const FUNCTIONS: [&str; 4] = [
    "aleph.rpc",
    "aleph.pair",
    "aleph.connect",
    "aleph.connection",
];

const CALL_TIMEOUT: Duration = Duration::from_secs(30);

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
    /// The client refuses calls over the Gateway's in-flight limit instead of
    /// queueing them, so calls take turns here (as the Web host queues them).
    turn: tokio::sync::Mutex<()>,
}

struct Link {
    state: &'static str,
    subject: Option<String>,
    client: Option<Arc<AlephClient>>,
    /// Bumped by every connect, so an older attempt's late outcome is ignored.
    attempt: u64,
    closed: bool,
}

impl Handle {
    pub(crate) fn start(host: HostChannel, target: Target) -> std::io::Result<Handle> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("aleph-panel-host")
            .enable_all()
            .build()?;
        let state = match target {
            Target::Local(_) => "connecting",
            Target::Unavailable(_) => "unavailable",
        };
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
            }),
            turn: tokio::sync::Mutex::new(()),
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

impl Drop for Handle {
    fn drop(&mut self) {
        for name in FUNCTIONS {
            self.shared.host.remove_handler(name);
        }
        {
            let mut link = self.shared.link();
            link.closed = true;
            // Dropping the client stops its socket.
            link.client = None;
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
                let runtime = self.runtime.clone();
                runtime.spawn(async move { call.resolve(self.rpc(data).await) });
            }
            other => {
                let message = format!("no host function {other}");
                call.reject(message);
            }
        }
    }

    fn connection(&self) -> Value {
        let link = self.link();
        connection_state(link.state, link.subject.as_deref())
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
                // The stream events are not used: the Panel only calls.
                Ok((client, _events)) => {
                    link.state = "connected";
                    link.subject = client.hello_subject();
                    link.client = Some(Arc::new(client));
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
        link.client = None;
        link.state = "disconnected";
        drop(link);
        self.publish();
    }

    async fn rpc(&self, data: Value) -> Value {
        let Some(method) = data.get("method").and_then(Value::as_str) else {
            return error_reply(-32602, "aleph.rpc needs { method, params }");
        };
        let params = data
            .get("params")
            .filter(|params| !params.is_null())
            .cloned();
        let _turn = self.turn.lock().await;
        let Some(client) = self.link().client.clone() else {
            return error_reply(PANEL_HOST_UNAVAILABLE, "the Gateway is not connected");
        };
        let result = client
            .call_with_timeout::<Value, Value>(method, params, CALL_TIMEOUT)
            .await;
        if !client.is_connected() {
            self.lost(&client);
        }
        rpc_reply(result)
    }
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
        features: vec![],
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
        Err(CliError::Rpc { code, message }) => error_reply(code, &message),
        Err(other) => error_reply(PANEL_HOST_UNAVAILABLE, &other.to_string()),
    }
}

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
        let handle = Handle::start(host, Target::resolve(true, None)).unwrap();
        assert_eq!(
            handle.shared.connection(),
            json!({ "state": "unavailable", "endpoint": "desktop-ipc", "subject": null, "pairing": false })
        );
    }

    #[test]
    fn rpc_without_a_connection_is_host_unavailable() {
        let handle = Handle::start(HostChannel::default(), Target::resolve(true, None)).unwrap();
        let reply = handle
            .shared
            .runtime
            .block_on(handle.shared.rpc(json!({ "method": "health" })));
        assert_eq!(reply["error"]["code"], json!(PANEL_HOST_UNAVAILABLE));
        let malformed = handle.shared.runtime.block_on(handle.shared.rpc(json!({})));
        assert_eq!(malformed["error"]["code"], json!(-32602));
    }
}
