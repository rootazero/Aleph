use super::local_ipc::{read_frame, write_frame, HELLO_TIMEOUT, MAX_FRAME_BYTES};

#[tokio::test]
async fn local_client_uses_hello_and_correlates_out_of_order() {
    use crate::{AlephClient, CliConfig, CliError};
    use aleph_protocol::{JsonRpcRequest, JsonRpcResponse};
    use serde_json::{json, Value};
    use std::time::Duration;
    use tokio::{sync::oneshot, time::timeout};

    #[cfg(unix)]
    let (_dir, endpoint, listener) = {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let endpoint = dir.path().join("ipc");
        let listener = tokio::net::UnixListener::bind(&endpoint).unwrap();
        (dir, endpoint, listener)
    };
    #[cfg(windows)]
    let (_dir, endpoint, account, mut stream) = {
        let dir = tempfile::tempdir().unwrap();
        let name = dir.path().file_name().unwrap().to_str().unwrap();
        let endpoint = std::path::PathBuf::from(format!(
            r"\\.\pipe\aleph-client-test-{}-{name}",
            std::process::id()
        ));
        let account = super::local_ipc::windows::PipeAccount::current().unwrap();
        let stream = account.create(&endpoint, true, 1).unwrap();
        (dir, endpoint, account, stream)
    };
    let (received, observed) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let peer = async {
        #[cfg(unix)]
        let mut stream = {
            let (stream, _) = listener.accept().await.unwrap();
            super::local_ipc::unix::require_same_user(&stream).unwrap();
            stream
        };
        #[cfg(windows)]
        {
            stream.connect().await.unwrap();
            account.authenticate_client(&mut stream).await.unwrap();
        }
        let hello: JsonRpcRequest =
            serde_json::from_str(&read_frame(&mut stream).await.unwrap().unwrap()).unwrap();
        assert_eq!(hello.method, "system.hello");
        let response = JsonRpcResponse::success(
            hello.id.unwrap(),
            json!({
                "protocol": {"major": 1, "minor": 0},
                "server": {"name": "fixture", "version": "1"},
                "connection_id": "fixture-connection",
                "features": [],
                "limits": {"max_message_bytes": MAX_FRAME_BYTES, "max_inflight_requests": 2},
            }),
        );
        write_frame(&mut stream, &serde_json::to_string(&response).unwrap())
            .await
            .unwrap();
        let first: JsonRpcRequest =
            serde_json::from_str(&read_frame(&mut stream).await.unwrap().unwrap()).unwrap();
        let second: JsonRpcRequest =
            serde_json::from_str(&read_frame(&mut stream).await.unwrap().unwrap()).unwrap();
        assert_ne!(first.id, second.id);
        for request in [second, first] {
            let response = JsonRpcResponse::success(request.id.unwrap(), json!(request.method));
            write_frame(&mut stream, &serde_json::to_string(&response).unwrap())
                .await
                .unwrap();
        }
        let expired_one: JsonRpcRequest =
            serde_json::from_str(&read_frame(&mut stream).await.unwrap().unwrap()).unwrap();
        let expired_two: JsonRpcRequest =
            serde_json::from_str(&read_frame(&mut stream).await.unwrap().unwrap()).unwrap();
        assert_eq!(expired_one.method, "expire-one");
        assert_eq!(expired_two.method, "expire-two");
        released.await.unwrap();
        let response = JsonRpcResponse::success(expired_one.id.unwrap(), json!(false));
        write_frame(&mut stream, &serde_json::to_string(&response).unwrap())
            .await
            .unwrap();
        let unanswered: JsonRpcRequest =
            serde_json::from_str(&read_frame(&mut stream).await.unwrap().unwrap()).unwrap();
        assert_eq!(unanswered.method, "unanswered");
        received.send(()).unwrap();
        assert_eq!(read_frame(&mut stream).await.unwrap(), None);
    };
    let caller = async {
        let hello = serde_json::from_value(json!({
            "protocol": {"major": 1, "minor": 0},
            "client": {"name": "fixture", "version": "1", "platform": std::env::consts::OS, "ui_runtime": "test"},
            "features": [],
        })).unwrap();
        // Exercise the shared socket state machine even while Windows stays opt-out.
        #[cfg(windows)]
        let (client, _events) = AlephClient::open_local(&endpoint, &hello).await.unwrap();
        #[cfg(unix)]
        let (client, _events) = AlephClient::connect_local(&endpoint, &hello).await.unwrap();
        let (first, second) = tokio::join!(
            client.call::<(), String>("first", None),
            client.call::<(), String>("second", None)
        );
        assert_eq!(first.unwrap(), "first");
        assert_eq!(second.unwrap(), "second");
        assert!(client.reconnect(&CliConfig::default()).await.is_err());
        for method in ["expire-one", "expire-two"] {
            assert!(matches!(
                client
                    .call_with_timeout::<(), Value>(method, None, Duration::ZERO)
                    .await,
                Err(CliError::Timeout(_))
            ));
        }
        assert!(matches!(
            client.call_with_timeout::<(), Value>("over-limit", None, Duration::ZERO).await,
            Err(CliError::Other(message)) if message == "In-flight request limit reached"
        ));
        release.send(()).unwrap();
        let client = std::sync::Arc::new(client);
        let call = async {
            loop {
                match client.call::<(), Value>("unanswered", None).await {
                    Err(CliError::Other(message))
                        if message == "In-flight request limit reached" =>
                    {
                        tokio::task::yield_now().await;
                    }
                    result => break result,
                }
            }
        };
        let (unanswered, closed) = tokio::join!(call, async {
            observed.await.unwrap();
            assert_local_start_is_ordered_with_retirement(client.clone()).await
        });
        closed.unwrap();
        assert!(matches!(unanswered, Err(CliError::Disconnected(_))));
    };
    timeout(HELLO_TIMEOUT, async {
        tokio::join!(peer, caller);
    })
    .await
    .unwrap();
}

// Use the authenticated real socket above, not a synthetic readiness flag.
async fn assert_local_start_is_ordered_with_retirement(
    client: std::sync::Arc<crate::AlephClient>,
) -> crate::CliResult<()> {
    use std::sync::mpsc;
    use tokio::sync::oneshot;

    let socket = client.socket();
    // Separate logical retirement from writer/OS teardown so the closed-first
    // check does not accidentally depend on a peer EOF or timing.
    let writer = socket.write.lock().await;
    let (starting, inside_start) = mpsc::channel();
    let (contended, saw_contention) = mpsc::channel();
    let (retired, after_retirement) = oneshot::channel();
    let runtime = tokio::runtime::Handle::current();
    let closing_client = client.clone();
    let closing_socket = socket.clone();
    let close = tokio::task::spawn_blocking(move || {
        inside_start.recv_timeout(HELLO_TIMEOUT).unwrap();
        contended
            .send(closing_socket.requests.retirement_would_block())
            .unwrap();
        runtime.block_on(async {
            let mut close = std::pin::pin!(closing_client.close());
            // This poll synchronously retires the socket, then waits for the
            // writer. It cannot reach Pending until the start guard releases.
            assert!(futures_util::poll!(&mut close).is_pending());
            retired.send(()).unwrap();
            close.await
        })
    });
    assert_eq!(
        client.with_ready_local_connection(|| {
            starting.send(()).unwrap();
            assert!(saw_contention.recv_timeout(HELLO_TIMEOUT).unwrap());
            "started"
        }),
        Some("started")
    );
    after_retirement.await.unwrap();
    assert!(!client.is_connected());
    assert_eq!(
        client.with_ready_local_connection(|| panic!("retired IPC must not start playback")),
        None::<()>
    );
    drop(writer);
    close.await.unwrap()
}

/// S3: a client that requested `live_events` receives id-less notifications
/// pushed right after the hello response — stream frames on the stream
/// plane, `event` topics on the topic plane. A server granting a feature the
/// client did not request is refused at the handshake.
#[cfg(unix)]
#[tokio::test]
async fn local_client_delivers_live_events_only_when_requested() {
    use crate::AlephClient;
    use aleph_protocol::{JsonRpcRequest, JsonRpcResponse, StreamEvent};
    use serde_json::json;
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;
    use tokio::time::timeout;

    let dir = tempfile::tempdir_in("/tmp").unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let endpoint = dir.path().join("ipc");
    let listener = tokio::net::UnixListener::bind(&endpoint).unwrap();
    let hello_params = |features: serde_json::Value| {
        serde_json::from_value::<aleph_protocol::system_hello::HelloParams>(json!({
            "protocol": {"major": 1, "minor": 0},
            "client": {"name": "fixture", "version": "1", "platform": std::env::consts::OS, "ui_runtime": "test"},
            "features": features,
        }))
        .unwrap()
    };
    // The fixture always grants `live_events` and pushes two notifications.
    let peer = async {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let hello: JsonRpcRequest =
                serde_json::from_str(&read_frame(&mut stream).await.unwrap().unwrap()).unwrap();
            let response = JsonRpcResponse::success(
                hello.id.unwrap(),
                json!({
                    "protocol": {"major": 1, "minor": 0},
                    "server": {"name": "fixture", "version": "1"},
                    "connection_id": "fixture-connection",
                    "features": ["live_events"],
                    "limits": {"max_message_bytes": MAX_FRAME_BYTES, "max_inflight_requests": 2},
                }),
            );
            let frames = [
                serde_json::to_string(&response).unwrap(),
                json!({"jsonrpc":"2.0","method":"stream.run_accepted","params":{
                    "type":"run_accepted","run_id":"r-1","session_key":"agent:main:main","accepted_at":"t"}})
                .to_string(),
                json!({"jsonrpc":"2.0","method":"event","params":{
                    "topic":"connection.warning","data":{"reason":"events_overflow"}}})
                .to_string(),
            ];
            for frame in frames {
                // The refused client may already be gone.
                if write_frame(&mut stream, &frame).await.is_err() {
                    break;
                }
            }
            let _ = read_frame(&mut stream).await;
        }
    };
    let caller = async {
        let refused = AlephClient::connect_local(&endpoint, &hello_params(json!([]))).await;
        assert!(refused.is_err(), "unrequested feature granted");
        let (client, mut events) =
            AlephClient::connect_local(&endpoint, &hello_params(json!(["live_events"])))
                .await
                .unwrap();
        let mut topics = client.take_topic_events().unwrap();
        match events.recv().await.unwrap() {
            StreamEvent::RunAccepted { run_id, .. } => assert_eq!(run_id, "r-1"),
            other => panic!("unexpected stream event {other:?}"),
        }
        let topic = topics.recv().await.unwrap();
        assert_eq!(topic.topic, "connection.warning");
        assert_eq!(client.stream_continuity().dropped_events, 0);
        drop(client);
    };
    timeout(Duration::from_secs(5), async { tokio::join!(peer, caller) })
        .await
        .unwrap();
}

#[tokio::test]
async fn local_panel_voice_abort_uses_exact_wire_lane_and_timeout_retires_socket() {
    use crate::{AlephClient, CliError};
    use aleph_protocol::{
        panel_voice::{
            VOICE_TURN_CANCEL_METHOD, VOICE_TURN_CAPTURE_READY_METHOD, VOICE_TURN_TRANSCRIBE_METHOD,
        },
        JsonRpcRequest, JsonRpcResponse,
    };
    use futures_util::poll;
    use serde_json::{json, Value};
    use std::{cell::Cell, time::Duration};

    async fn read_while_sending<T>(
        peer: &mut (impl tokio::io::AsyncRead + Unpin),
        call: impl std::future::Future<Output = T>,
    ) -> JsonRpcRequest {
        tokio::select! {
            frame = read_frame(peer) => serde_json::from_str(&frame.unwrap().unwrap()).unwrap(),
            _ = call => panic!("RPC completed before the peer observed its request"),
        }
    }

    #[cfg(unix)]
    let (_dir, endpoint, listener) = {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let endpoint = dir.path().join("ipc");
        let listener = tokio::net::UnixListener::bind(&endpoint).unwrap();
        (dir, endpoint, listener)
    };
    #[cfg(windows)]
    let (_dir, endpoint, account, mut stream) = {
        let dir = tempfile::tempdir().unwrap();
        let name = dir.path().file_name().unwrap().to_str().unwrap();
        let endpoint = std::path::PathBuf::from(format!(
            r"\\.\pipe\aleph-client-voice-{}-{name}",
            std::process::id()
        ));
        let account = super::local_ipc::windows::PipeAccount::current().unwrap();
        let stream = account.create(&endpoint, true, 1).unwrap();
        (dir, endpoint, account, stream)
    };
    let peer = async {
        #[cfg(unix)]
        let mut stream = listener.accept().await.unwrap().0;
        #[cfg(windows)]
        {
            stream.connect().await.unwrap();
            account.authenticate_client(&mut stream).await.unwrap();
        }
        let request: JsonRpcRequest =
            serde_json::from_str(&read_frame(&mut stream).await.unwrap().unwrap()).unwrap();
        assert_eq!(request.method, "system.hello");
        let response = JsonRpcResponse::success(
            request.id.unwrap(),
            json!({
                "protocol":{"major":1,"minor":0},"server":{"name":"fixture","version":"1"},
                "connection_id":"panel-voice-fixture","features":[],
                "limits":{"max_message_bytes":MAX_FRAME_BYTES,"max_inflight_requests":3},
            }),
        );
        write_frame(&mut stream, &serde_json::to_string(&response).unwrap())
            .await
            .unwrap();
        stream
    };
    let hello = serde_json::from_value(json!({
        "protocol":{"major":1,"minor":0},
        "client":{"name":"fixture","version":"1","platform":std::env::consts::OS,"ui_runtime":"test"},
        "features":[],
    })).unwrap();
    tokio::time::timeout(HELLO_TIMEOUT, async {
        let (client, mut peer) = tokio::join!(AlephClient::open_local(&endpoint, &hello), peer);
        let (client, _events) = client.unwrap();
        let mut send = std::pin::pin!(client.call_panel_voice("chat.send", None, HELLO_TIMEOUT, || Ok(())));
        let outbound = read_while_sending(&mut peer, send.as_mut()).await;
        assert_eq!(outbound.method, "chat.send");
        let run_id = "00000000-0000-4000-8000-000000000003";
        let response = JsonRpcResponse::success(outbound.id.unwrap(), json!({"run_id":run_id}));
        write_frame(&mut peer, &serde_json::to_string(&response).unwrap()).await.unwrap();
        let receipt = send.await.unwrap();
        let mut transcribe = std::pin::pin!(client.call_panel_voice(VOICE_TURN_TRANSCRIBE_METHOD, None, HELLO_TIMEOUT, || Ok(())));
        let request = read_while_sending(&mut peer, transcribe.as_mut()).await;
        assert_eq!(request.method, VOICE_TURN_TRANSCRIBE_METHOD);
        let exact = json!({"run_id":receipt["run_id"],"run_only":true});
        let abort = || client.call_panel_voice("chat.abort", Some(&exact), HELLO_TIMEOUT, || Ok(()));
        let mut waiting_abort = std::pin::pin!(abort());
        let request = read_while_sending(&mut peer, waiting_abort.as_mut()).await;
        assert_eq!(request.method, "chat.abort");
        assert_eq!(request.params.as_ref(), Some(&exact));
        let mut duplicate = std::pin::pin!(abort());
        assert!(matches!(poll!(&mut duplicate), std::task::Poll::Ready(Err(CliError::Other(message))) if message == "In-flight request limit reached"));
        let mut ordinary = std::pin::pin!(client.call::<(), Value>("health", None));
        let health = read_while_sending(&mut peer, ordinary.as_mut()).await;
        assert_eq!(health.method, "health");
        let mut over_gateway_capacity = std::pin::pin!(client.call::<(), Value>("extra", None));
        assert!(matches!(poll!(&mut over_gateway_capacity), std::task::Poll::Ready(Err(CliError::Other(message))) if message == "In-flight request limit reached"));
        let aborted = json!({"run_id":run_id,"aborted":true,"dropped":0});
        let response = JsonRpcResponse::success(request.id.unwrap(), aborted.clone());
        write_frame(&mut peer, &serde_json::to_string(&response).unwrap()).await.unwrap();
        assert_eq!(waiting_abort.await.unwrap(), aborted);
        assert!(poll!(&mut transcribe).is_pending());
        let mut extra = std::pin::pin!(client.call::<(), Value>("extra", None));
        let request = read_while_sending(&mut peer, extra.as_mut()).await;
        assert_eq!(request.method, "extra");
        let capture = json!({"capture_id":"00000000-0000-4000-8000-000000000001"});
        let ready = || client.call_panel_voice(VOICE_TURN_CAPTURE_READY_METHOD, Some(&capture), HELLO_TIMEOUT, || Ok(()));
        let mut first = std::pin::pin!(ready());
        let mut second = std::pin::pin!(ready());
        let mut cleanup = std::pin::pin!(ready());
        for mut call in [first.as_mut(), second.as_mut(), cleanup.as_mut()] {
            assert!(poll!(&mut call).is_pending());
            let request = read_while_sending(&mut peer, call.as_mut()).await;
            assert_eq!(request.method, VOICE_TURN_CAPTURE_READY_METHOD);
        }
        assert!(matches!(ready().await, Err(CliError::Other(message)) if message == "In-flight request limit reached"));
        let params = json!({"turn_id":"00000000-0000-4000-8000-000000000002"});
        let cancel = || client.call_panel_voice(VOICE_TURN_CANCEL_METHOD, Some(&params), HELLO_TIMEOUT, || Ok(()));
        let mut sent = std::pin::pin!(cancel());
        assert!(poll!(&mut sent).is_pending());
        let request = read_while_sending(&mut peer, sent.as_mut()).await;
        assert_eq!(request.method, VOICE_TURN_CANCEL_METHOD);
        assert!(matches!(cancel().await, Err(CliError::Other(message)) if message == "In-flight request limit reached"));
        let response = JsonRpcResponse::success(request.id.unwrap(), json!({}));
        write_frame(&mut peer, &serde_json::to_string(&response).unwrap()).await.unwrap();
        assert_eq!(sent.await.unwrap(), json!({}));

        // The final guard runs after waiting for the writer. A stale request
        // may have a pending request ID but must never produce a wire frame.
        let socket = client.socket();
        let writer = socket.write.lock().await;
        let visible = Cell::new(true);
        let mut stale = std::pin::pin!(client.call_panel_voice(VOICE_TURN_CANCEL_METHOD, None, HELLO_TIMEOUT, || {
            if visible.get() { Ok(()) } else { Err(CliError::Other("stale writer entry".into())) }
        }));
        assert!(poll!(&mut stale).is_pending());
        visible.set(false);
        drop(writer);
        assert!(matches!(stale.await, Err(CliError::Other(message)) if message == "stale writer entry"));

        // Free one shared wire slot, leaving other pending calls to observe EOF.
        let response = JsonRpcResponse::success(health.id.unwrap(), json!({}));
        write_frame(&mut peer, &serde_json::to_string(&response).unwrap()).await.unwrap();
        assert_eq!(ordinary.await.unwrap(), json!({}));

        let mut unacknowledged = std::pin::pin!(client.call_panel_voice("chat.abort", Some(&exact), Duration::from_millis(100), || Ok(())));
        assert!(poll!(&mut unacknowledged).is_pending());
        let request = read_while_sending(&mut peer, unacknowledged.as_mut()).await;
        assert_eq!(request.method, "chat.abort");
        assert_eq!(request.params.as_ref(), Some(&exact));
        assert!(matches!(unacknowledged.await, Err(CliError::Timeout(_))));
        assert!(!client.is_connected());
        assert_eq!(read_frame(&mut peer).await.unwrap(), None);
        assert!(matches!(transcribe.await, Err(CliError::Disconnected(_))));
        assert!(matches!(extra.await, Err(CliError::Disconnected(_))));
        assert!(matches!(first.await, Err(CliError::Disconnected(_))));
        assert!(matches!(second.await, Err(CliError::Disconnected(_))));
        assert!(matches!(cleanup.await, Err(CliError::Disconnected(_))));
    }).await.unwrap();
}
