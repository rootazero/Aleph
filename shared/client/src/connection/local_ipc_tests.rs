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
            client.close().await
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
    let peer = async {
        for features in [
            json!(["live_events"]),
            json!(["live_events"]),
            json!(["live_events", "chat_transcript"]),
        ] {
            let (mut stream, _) = listener.accept().await.unwrap();
            let hello: JsonRpcRequest =
                serde_json::from_str(&read_frame(&mut stream).await.unwrap().unwrap()).unwrap();
            let response = JsonRpcResponse::success(
                hello.id.unwrap(),
                json!({
                    "protocol": {"major": 1, "minor": 0},
                    "server": {"name": "fixture", "version": "1"},
                    "connection_id": "fixture-connection",
                    "features": features,
                    "limits": {"max_message_bytes": MAX_FRAME_BYTES, "max_inflight_requests": 2},
                }),
            );
            let frames = [
                serde_json::to_string(&response).unwrap(),
                json!({"jsonrpc":"2.0","method":"stream.chat_tool","params":{
                    "type":"chat_tool","run_id":"r-1","seq":1,"tool_call_id":"call-1",
                    "change":{"kind":"started","display_name":"Tool","args_text":""}}})
                .to_string(),
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
        for granted in [false, true] {
            let (client, mut events) = AlephClient::connect_local(
                &endpoint,
                &hello_params(json!(["live_events", "chat_transcript"])),
            )
            .await
            .unwrap();
            let mut topics = client.take_topic_events().unwrap();
            if granted {
                assert!(matches!(
                    events.recv().await,
                    Some(StreamEvent::ChatTool { seq: 1, .. })
                ));
            }
            match events.recv().await.unwrap() {
                StreamEvent::RunAccepted { run_id, .. } => assert_eq!(run_id, "r-1"),
                other => panic!("unexpected stream event {other:?}"),
            }
            let topic = topics.recv().await.unwrap();
            assert_eq!(topic.topic, "connection.warning");
            assert_eq!(client.stream_continuity().dropped_events, 0);
            drop(client);
        }
    };
    timeout(Duration::from_secs(5), async { tokio::join!(peer, caller) })
        .await
        .unwrap();
}
