use std::time::Duration;

use aleph_client::{AlephClient, CliConfig};
use aleph_protocol::system_hello::{
    ClientHello, Feature, HelloParams, HelloResult, ProtocolVersion,
};
use serde_json::Value;
use tokio::sync::mpsc;

const DAEMON_URL: &str = "ws://127.0.0.1:18790";
const RECONNECT_DELAY: Duration = Duration::from_secs(2);
const MAX_RECONNECT_DELAY: Duration = Duration::from_secs(30);
const MAX_CONSECUTIVE_FAILURES: usize = 10;
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(40);
const CONNECTION_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Connection state reported to the NSH UI runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellStatus {
    Connecting,
    Connected {
        server_version: String,
        session_count: usize,
    },
    Error(String),
}

/// Build the NSH desktop's system.hello request parameters.
pub fn build_hello_params() -> HelloParams {
    HelloParams {
        protocol: ProtocolVersion { major: 1, minor: 0 },
        client: ClientHello {
            name: "aleph-nsh-desktop".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            platform: std::env::consts::OS.into(),
            ui_runtime: "nsh".into(),
        },
        features: vec![
            Feature::EventReplay,
            Feature::Snapshot,
            Feature::BinaryChannel,
        ],
    }
}

/// Perform the application-level system.hello exchange after the transport
/// client's own `connect` authentication handshake has completed.
///
/// Protocol V1 §18 requires clients to reject `major != 1`.
pub async fn perform_handshake(client: &AlephClient) -> Result<HelloResult, String> {
    let result = client
        .call::<HelloParams, HelloResult>("system.hello", Some(build_hello_params()))
        .await
        .map_err(|error| format!("handshake failed: {error}"))?;

    if result.protocol.major != 1 {
        return Err(format!(
            "incompatible protocol version: expected major=1, got major={}",
            result.protocol.major
        ));
    }

    Ok(result)
}

/// Start the connection lifecycle on the dedicated Tokio worker.
///
/// Each failed cycle reports the error and retries after a fixed delay. A
/// successful cycle remains alive while the event receiver is open; when the
/// transport closes, the cycle returns to the same retry path, which repeats
/// the transport handshake, system.hello, event subscription, and session list.
pub fn spawn_connection_loop(status_tx: mpsc::Sender<ShellStatus>) -> tokio::task::JoinHandle<()> {
    crate::runtime::spawn_tokio_worker(async move {
        let mut consecutive_failures = 0;
        let mut backoff = RECONNECT_DELAY;

        loop {
            match run_connection_cycle(&status_tx).await {
                Ok(()) => {
                    // Successful cycle; reset failure tracking
                    consecutive_failures = 0;
                    backoff = RECONNECT_DELAY;
                }
                Err(error) => {
                    consecutive_failures += 1;
                    let _ = status_tx.send(ShellStatus::Error(error.clone())).await;

                    if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                        let _ = status_tx
                            .send(ShellStatus::Error(format!(
                                "giving up after {MAX_CONSECUTIVE_FAILURES} consecutive failures: {error}"
                            )))
                            .await;
                        break;
                    }

                    tokio::time::sleep(backoff).await;
                    // Exponential backoff with ceiling
                    backoff = std::cmp::min(backoff.saturating_mul(2), MAX_RECONNECT_DELAY);
                }
            }
        }
    })
}

async fn run_connection_cycle(status_tx: &mpsc::Sender<ShellStatus>) -> Result<(), String> {
    let _ = status_tx.send(ShellStatus::Connecting).await;
    crate::supervisor::ensure_ready().await?;
    run_authenticated_cycle(status_tx, DAEMON_URL).await
}

async fn run_authenticated_cycle(
    status_tx: &mpsc::Sender<ShellStatus>,
    url: &str,
) -> Result<(), String> {
    let (client, mut events_rx) = tokio::time::timeout(
        CONNECTION_TIMEOUT,
        AlephClient::connect(url, &CliConfig::default()),
    )
    .await
    .map_err(|_| format!("connect timeout after {}s", CONNECTION_TIMEOUT.as_secs()))?
    .map_err(|error| format!("connect failed: {error}"))?;

    let hello = perform_handshake(&client).await?;

    let _: Value = client
        .call(
            "events.subscribe",
            Some(serde_json::json!({"topics": ["**"]})),
        )
        .await
        .map_err(|error| format!("subscribe failed: {error}"))?;

    let sessions: Value = client
        .call("sessions.list", None::<Value>)
        .await
        .map_err(|error| format!("sessions.list failed: {error}"))?;
    let session_count = session_count(&sessions);

    let _ = status_tx
        .send(ShellStatus::Connected {
            server_version: hello.server.version,
            session_count,
        })
        .await;

    loop {
        tokio::select! {
            event = events_rx.recv() => {
                if event.is_none() {
                    break;
                }
                // Phase 2 only keeps the event channel alive. Phase 3 will
                // project events into the NSH session and activity views.
            }
            () = tokio::time::sleep(CONNECTION_POLL_INTERVAL) => {
                if !client.is_connected() {
                    break;
                }
            }
        }
    }

    // Explicit cleanup before reconnect to avoid racing with async shutdown
    client.close().await;

    Err("connection closed".into())
}

fn session_count(response: &Value) -> usize {
    match response {
        Value::Array(sessions) => sessions.len(),
        Value::Object(object) => object
            .get("sessions")
            .and_then(Value::as_array)
            .map_or(0, Vec::len),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use serde_json::json;
    use std::sync::Arc;
    use tokio::net::TcpListener;
    use tokio::sync::Mutex;
    use tokio::time::timeout;
    use tokio_tungstenite::{accept_async, tungstenite::Message};

    #[test]
    fn build_hello_params_uses_nsh_desktop_client_name() {
        let params = build_hello_params();
        assert_eq!(params.client.name, "aleph-nsh-desktop");
        assert_eq!(params.protocol.major, 1);
        assert_eq!(params.protocol.minor, 0);
        assert_eq!(
            params.features,
            vec![
                Feature::EventReplay,
                Feature::Snapshot,
                Feature::BinaryChannel
            ]
        );
    }

    #[test]
    fn session_count_reads_the_server_list_envelope() {
        let response = serde_json::json!({
            "sessions": [{"key": "one"}, {"key": "two"}],
            "count": 2,
        });
        assert_eq!(session_count(&response), 2);
    }

    #[test]
    fn session_count_accepts_legacy_array_response() {
        let response = serde_json::json!([{"key": "one"}]);
        assert_eq!(session_count(&response), 1);
    }

    #[tokio::test]
    async fn reconnect_re_issues_hello_then_subscribe_in_order() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        let methods = Arc::new(Mutex::new(Vec::<String>::new()));
        let server_methods = Arc::clone(&methods);

        let server = tokio::spawn(async move {
            for _ in 0..2 {
                let (stream, _) = timeout(Duration::from_secs(5), listener.accept())
                    .await
                    .expect("client did not reconnect")
                    .unwrap();
                let mut socket = accept_async(stream).await.unwrap();
                serve_connection(&mut socket, &server_methods).await;
            }
        });

        let (status_tx, _status_rx) = mpsc::channel(8);
        for _ in 0..2 {
            let error = run_authenticated_cycle(&status_tx, &url)
                .await
                .expect_err("the mock server should close each cycle");
            assert_eq!(error, "connection closed");
        }

        timeout(Duration::from_secs(5), server)
            .await
            .expect("mock server did not finish")
            .unwrap();

        let methods = methods.lock().await.clone();
        let application_methods: Vec<_> = methods
            .iter()
            .filter(|method| method.as_str() != "connect")
            .cloned()
            .collect();
        assert_eq!(
            application_methods,
            vec![
                "system.hello",
                "events.subscribe",
                "sessions.list",
                "system.hello",
                "events.subscribe",
                "sessions.list",
            ],
            "reconnect must replay the full handshake sequence"
        );
    }

    async fn serve_connection(
        socket: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
        methods: &Mutex<Vec<String>>,
    ) {
        for expected_method in [
            "connect",
            "system.hello",
            "events.subscribe",
            "sessions.list",
        ] {
            let message = timeout(Duration::from_secs(5), socket.next())
                .await
                .expect("client did not send the next request")
                .unwrap()
                .unwrap();
            let request: Value = serde_json::from_str(message.to_text().unwrap()).unwrap();
            let method = request["method"].as_str().unwrap().to_string();
            assert_eq!(method, expected_method);
            methods.lock().await.push(method.clone());

            let result = match method.as_str() {
                "connect" => json!({"role": "operator"}),
                "system.hello" => json!({
                    "protocol": {"major": 1, "minor": 0},
                    "server": {"name": "aleph-server", "version": "test"},
                    "connection_id": "conn-test",
                    "features": ["event_replay", "snapshot", "binary_channel"],
                    "limits": {
                        "max_message_bytes": 8_388_608,
                        "max_inflight_requests": 128,
                    },
                }),
                "events.subscribe" => json!({"subscribed": ["**"], "changed": 1}),
                "sessions.list" => json!({"sessions": [], "count": 0}),
                _ => unreachable!("unexpected method: {method}"),
            };
            let response = json!({"jsonrpc": "2.0", "id": request["id"], "result": result});
            socket
                .send(Message::Text(response.to_string().into()))
                .await
                .unwrap();
        }
    }

    #[tokio::test]
    async fn handshake_failure_propagates_error() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_async(stream).await.unwrap();

            // Serve connect successfully
            let msg = socket.next().await.unwrap().unwrap();
            let req: Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
            let response = json!({"jsonrpc": "2.0", "id": req["id"], "result": {"role": "operator"}});
            socket.send(Message::Text(response.to_string().into())).await.unwrap();

            // Inject handshake error
            let msg = socket.next().await.unwrap().unwrap();
            let req: Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
            let error_response = json!({
                "jsonrpc": "2.0",
                "id": req["id"],
                "error": {"code": -32000, "message": "authentication required"}
            });
            socket.send(Message::Text(error_response.to_string().into())).await.unwrap();
        });

        let (status_tx, _status_rx) = mpsc::channel(8);
        let result = run_authenticated_cycle(&status_tx, &url).await;

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("handshake failed"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn subscribe_failure_propagates_error() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_async(stream).await.unwrap();

            // Serve connect and handshake successfully
            for _ in 0..2 {
                let msg = socket.next().await.unwrap().unwrap();
                let req: Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
                let result = if req["method"] == "connect" {
                    json!({"role": "operator"})
                } else {
                    json!({
                        "protocol": {"major": 1, "minor": 0},
                        "server": {"name": "test", "version": "1.0"},
                        "connection_id": "test",
                        "features": [],
                        "limits": {"max_message_bytes": 8388608, "max_inflight_requests": 128}
                    })
                };
                let response = json!({"jsonrpc": "2.0", "id": req["id"], "result": result});
                socket.send(Message::Text(response.to_string().into())).await.unwrap();
            }

            // Inject subscribe error
            let msg = socket.next().await.unwrap().unwrap();
            let req: Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
            let error_response = json!({
                "jsonrpc": "2.0",
                "id": req["id"],
                "error": {"code": -32603, "message": "subscription limit exceeded"}
            });
            socket.send(Message::Text(error_response.to_string().into())).await.unwrap();
        });

        let (status_tx, _status_rx) = mpsc::channel(8);
        let result = run_authenticated_cycle(&status_tx, &url).await;

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("subscribe failed"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn connection_timeout_returns_error() {
        // Bind a port but never accept connections
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());

        let (status_tx, _status_rx) = mpsc::channel(8);
        let result = run_authenticated_cycle(&status_tx, &url).await;

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("timeout"));
    }

    #[tokio::test]
    async fn incompatible_protocol_version_rejected() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_async(stream).await.unwrap();

            // Serve connect successfully
            let msg = socket.next().await.unwrap().unwrap();
            let req: Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
            let response = json!({"jsonrpc": "2.0", "id": req["id"], "result": {"role": "operator"}});
            socket.send(Message::Text(response.to_string().into())).await.unwrap();

            // Return incompatible protocol version
            let msg = socket.next().await.unwrap().unwrap();
            let req: Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
            let result = json!({
                "protocol": {"major": 2, "minor": 0},
                "server": {"name": "aleph-server", "version": "2.0"},
                "connection_id": "conn-test",
                "features": [],
                "limits": {"max_message_bytes": 8388608, "max_inflight_requests": 128}
            });
            let response = json!({"jsonrpc": "2.0", "id": req["id"], "result": result});
            socket.send(Message::Text(response.to_string().into())).await.unwrap();
        });

        let (status_tx, _status_rx) = mpsc::channel(8);
        let result = run_authenticated_cycle(&status_tx, &url).await;

        assert!(result.is_err());
        let error_msg = result.unwrap_err();
        assert!(error_msg.contains("incompatible protocol version"));
        assert!(error_msg.contains("major=2"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn sessions_list_failure_propagates_error() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_async(stream).await.unwrap();

            // Serve connect, handshake, and subscribe successfully
            for call_num in 0..3 {
                let msg = socket.next().await.unwrap().unwrap();
                let req: Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
                let result = match call_num {
                    0 => json!({"role": "operator"}),
                    1 => json!({
                        "protocol": {"major": 1, "minor": 0},
                        "server": {"name": "test", "version": "1.0"},
                        "connection_id": "test",
                        "features": [],
                        "limits": {"max_message_bytes": 8388608, "max_inflight_requests": 128}
                    }),
                    2 => json!({"subscribed": ["**"], "changed": 1}),
                    _ => unreachable!(),
                };
                let response = json!({"jsonrpc": "2.0", "id": req["id"], "result": result});
                socket.send(Message::Text(response.to_string().into())).await.unwrap();
            }

            // Inject sessions.list error
            let msg = socket.next().await.unwrap().unwrap();
            let req: Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
            let error_response = json!({
                "jsonrpc": "2.0",
                "id": req["id"],
                "error": {"code": -32603, "message": "internal database error"}
            });
            socket.send(Message::Text(error_response.to_string().into())).await.unwrap();
        });

        let (status_tx, _status_rx) = mpsc::channel(8);
        let result = run_authenticated_cycle(&status_tx, &url).await;

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("sessions.list failed"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn event_stream_closed_triggers_reconnect() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_async(stream).await.unwrap();

            // Serve full handshake
            for call_num in 0..4 {
                let msg = socket.next().await.unwrap().unwrap();
                let req: Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
                let result = match call_num {
                    0 => json!({"role": "operator"}),
                    1 => json!({
                        "protocol": {"major": 1, "minor": 0},
                        "server": {"name": "test", "version": "1.0"},
                        "connection_id": "test",
                        "features": [],
                        "limits": {"max_message_bytes": 8388608, "max_inflight_requests": 128}
                    }),
                    2 => json!({"subscribed": ["**"], "changed": 1}),
                    3 => json!({"sessions": [], "count": 0}),
                    _ => unreachable!(),
                };
                let response = json!({"jsonrpc": "2.0", "id": req["id"], "result": result});
                socket.send(Message::Text(response.to_string().into())).await.unwrap();
            }

            // Close the socket to simulate event stream closure
            socket.close(None).await.unwrap();
        });

        let (status_tx, _status_rx) = mpsc::channel(8);
        let result = run_authenticated_cycle(&status_tx, &url).await;

        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "connection closed");
        server.await.unwrap();
    }
}
