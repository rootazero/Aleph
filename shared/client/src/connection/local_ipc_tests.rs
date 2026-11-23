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
