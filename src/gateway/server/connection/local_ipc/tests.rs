use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use aleph_client::local_ipc::{parse_payload, read_frame, write_frame};
use serde_json::{json, Value};
use tokio::{
    sync::{oneshot, Notify},
    time::timeout,
};

#[cfg(unix)]
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
#[cfg(windows)]
use tokio::net::windows::named_pipe::NamedPipeClient as TestStream;
#[cfg(unix)]
use tokio::net::UnixStream as TestStream;

use crate::gateway::{
    handlers::session::handle_list_db,
    protocol::{AUTH_REQUIRED, INTERNAL_ERROR, INVALID_PARAMS, METHOD_NOT_FOUND},
    router::SessionKey,
    security::store::{SecurityStore, UserStatus, OWNER_USER_ID},
    server::GatewayServer,
    session_store::{
        file_backend::{FileSessionStore, FileSessionStoreConfig},
        SessionStore,
    },
};
use crate::scope::{with_scope, ScopeAttribution};

fn fixture() -> (tempfile::TempDir, PathBuf) {
    #[cfg(unix)]
    {
        let root = tempfile::tempdir_in("/tmp").unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let endpoint = root.path().join("gateway.sock");
        (root, endpoint)
    }
    #[cfg(windows)]
    {
        let endpoint = PathBuf::from(format!(r"\\.\pipe\aleph-s1b-{}", uuid::Uuid::new_v4()));
        (tempfile::tempdir().unwrap(), endpoint)
    }
}

async fn try_connect(endpoint: &Path) -> std::io::Result<TestStream> {
    #[cfg(unix)]
    return TestStream::connect(endpoint).await;
    #[cfg(windows)]
    aleph_client::local_ipc::windows::connect(endpoint).await
}

async fn assert_retired(endpoint: &Path) {
    #[cfg(unix)]
    {
        assert!(std::fs::symlink_metadata(endpoint)
            .unwrap()
            .file_type()
            .is_socket());
        assert!(try_connect(endpoint).await.is_err());
    }
    #[cfg(windows)]
    {
        let account = aleph_client::local_ipc::windows::PipeAccount::current().unwrap();
        loop {
            match account.create(endpoint, true, super::MAX_CONNECTIONS) {
                Ok(first) => {
                    drop(first);
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                Err(error) => panic!("could not rebind retired pipe: {error}"),
            }
        }
    }
}

async fn connect(endpoint: &Path) -> TestStream {
    timeout(Duration::from_secs(2), async {
        loop {
            if let Ok(stream) = try_connect(endpoint).await {
                return stream;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("Gateway should mount the explicit local endpoint")
}

async fn send(stream: &mut TestStream, value: Value) {
    write_frame(stream, &value.to_string()).await.unwrap();
}

async fn receive(stream: &mut TestStream) -> Value {
    serde_json::from_str(&read_frame(stream).await.unwrap().expect("response frame")).unwrap()
}

async fn assert_closed(stream: &mut TestStream) {
    match read_frame(stream).await {
        Ok(None) => {}
        #[cfg(windows)]
        Err(error)
            if error.raw_os_error()
                == Some(windows_sys::Win32::Foundation::ERROR_PIPE_NOT_CONNECTED as i32) => {}
        result => panic!("expected peer EOF or a disconnected pipe, got {result:?}"),
    }
}

fn hello(id: u32, major: u32) -> Value {
    json!({"jsonrpc":"2.0","method":"system.hello","id":id,"params":{
        "protocol":{"major":major,"minor":9},
        "client":{"name":"fixture","version":"1","platform":std::env::consts::OS,"ui_runtime":"test"},
        "features":["event_replay"],
    }})
}

#[tokio::test]
async fn desktop_ipc_uses_gateway_and_releases_its_connection() {
    let (root, endpoint) = fixture();
    let sessions = Arc::new(
        FileSessionStore::new(FileSessionStoreConfig {
            base_dir: root.path().join("sessions"),
            ..Default::default()
        })
        .unwrap(),
    );
    let own = SessionKey::main("owner-fixture");
    let other = SessionKey::main("other-fixture");
    for (owner, key) in [(OWNER_USER_ID, &own), ("other-user", &other)] {
        let metadata = with_scope(
            Some(ScopeAttribution::personal(owner)),
            sessions.get_or_create(key),
        )
        .await
        .unwrap();
        assert_eq!(metadata.owner_user_id.as_deref(), Some(owner));
    }
    let mut gateway = GatewayServer::new("127.0.0.1:0".parse().unwrap());
    let store = Arc::new(SecurityStore::in_memory().unwrap());
    gateway.set_security_store(store.clone());
    gateway
        .handlers_mut()
        .register("sessions.list", move |req| {
            handle_list_db(req, sessions.clone())
        });
    for method in [
        "graph.query",
        "graph.search",
        "graph.node_detail",
        "graph.update_note",
        "graph.rename_note",
        "graph.delete_note",
    ] {
        gateway
            .handlers_mut()
            .register(method, |request| async move {
                crate::gateway::protocol::JsonRpcResponse::success(
                    request.id,
                    serde_json::json!({"entered": request.method}),
                )
            });
    }
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let calls = Arc::new(AtomicUsize::new(0));
    gateway.handlers_mut().register("health", {
        let (started, release, calls) = (started.clone(), release.clone(), calls.clone());
        move |req| {
            let (started, release, calls) = (started.clone(), release.clone(), calls.clone());
            async move {
                if req.params.as_ref().is_some_and(|p| p["large"] == true) {
                    return crate::gateway::protocol::JsonRpcResponse::success(
                        req.id,
                        json!({"rows": vec![0; 4097]}),
                    );
                }
                calls.fetch_add(1, Ordering::SeqCst);
                started.notify_one();
                release.notified().await;
                crate::gateway::handlers::health::handle(req).await
            }
        }
    });
    let connections = gateway.connections.clone();
    let (stop, shutdown) = oneshot::channel();
    let server_endpoint = endpoint.clone();
    let task =
        tokio::spawn(async move { gateway.run_local_endpoint(shutdown, &server_endpoint).await });
    timeout(Duration::from_secs(5), async {
        let mut first = connect(&endpoint).await;
        send(
            &mut first,
            json!({"jsonrpc":"2.0","method":"sessions.list","id":1}),
        )
        .await;
        assert_eq!(receive(&mut first).await["error"]["code"], AUTH_REQUIRED);
        assert_closed(&mut first).await;
        let mut incompatible = connect(&endpoint).await;
        send(&mut incompatible, hello(1, 2)).await;
        assert_eq!(
            receive(&mut incompatible).await["error"]["code"],
            INVALID_PARAMS
        );
        assert_closed(&mut incompatible).await;

        let mut invalid = connect(&endpoint).await;
        let mut request = hello(42, 1);
        request["params"]["protocol"]["major"] = json!("x".repeat(65_536));
        send(&mut invalid, request).await;
        let frame = read_frame(&mut invalid).await.unwrap().unwrap();
        let response = parse_payload(&frame).expect("hello error must fit the Client shape bounds");
        assert_eq!(response["id"], 42);
        assert_eq!(response["error"]["code"], INVALID_PARAMS);
        assert_closed(&mut invalid).await;
        assert!(connections.read().await.is_empty());
        #[cfg(unix)]
        assert_eq!(
            std::fs::symlink_metadata(root.path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );

        let mut stream = connect(&endpoint).await;
        send(&mut stream, hello(1, 1)).await;
        let response = receive(&mut stream).await;
        assert_eq!(response["id"], 1);
        assert_eq!(response["result"]["protocol"], json!({"major":1,"minor":0}));
        assert_eq!(response["result"]["features"], json!([]));
        let id = response["result"]["connection_id"].as_str().unwrap();
        {
            let table = connections.read().await;
            let row = table.get(id).unwrap();
            assert_eq!(row.caller_user.as_deref(), Some(OWNER_USER_ID));
            assert!(row.verified_user.is_none());
        }
        assert_eq!(response["result"]["limits"]["max_inflight_requests"], 8);
        send(
            &mut stream,
            json!({"jsonrpc":"2.0","method":"health","id":"large","params":{"large":true}}),
        )
        .await;
        let oversized = receive(&mut stream).await;
        assert_eq!(oversized["id"], "large");
        assert_eq!(oversized["error"]["code"], INTERNAL_ERROR);
        send(
            &mut stream,
            json!({"jsonrpc":"2.0","method":"health","id":3}),
        )
        .await;
        started.notified().await;
        send(
            &mut stream,
            json!({"jsonrpc":"2.0","method":"sessions.list","id":2}),
        )
        .await;
        let response = receive(&mut stream).await;
        assert_eq!(response["id"], 2);
        let rows = response["result"]["sessions"].as_array().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["key"], own.to_key_string());
        release.notify_one();
        let health = receive(&mut stream).await;
        assert_eq!(health["id"], 3);
        assert_eq!(health["result"]["status"], "healthy");
        send(&mut stream, hello(4, 1)).await;
        assert_eq!(
            receive(&mut stream).await["error"]["code"],
            METHOD_NOT_FOUND
        );
        // One v1 method set: the capability snapshot is mounted, the
        // project selector (a connection-state write) is not.
        send(
            &mut stream,
            json!({"jsonrpc":"2.0","method":"panel.project.select","id":6,"params":{"project_id":null}}),
        )
        .await;
        assert_eq!(
            receive(&mut stream).await["error"]["code"],
            METHOD_NOT_FOUND
        );
        send(
            &mut stream,
            json!({"jsonrpc":"2.0","method":"panel.capabilities.snapshot","id":7}),
        )
        .await;
        let snapshot = receive(&mut stream).await;
        assert_eq!(snapshot["id"], 7, "{snapshot}");
        assert_ne!(snapshot["error"]["code"], METHOD_NOT_FOUND);
        for (offset, (method, mounted)) in [
            ("graph.query", true),
            ("graph.search", true),
            ("graph.node_detail", true),
            ("graph.update_note", false),
            ("graph.rename_note", false),
            ("graph.delete_note", false),
        ].into_iter().enumerate() {
            let id = 20 + offset;
            send(&mut stream, json!({"jsonrpc": "2.0", "method": method, "id": id})).await;
            let response = receive(&mut stream).await;
            assert_eq!(response["id"], id);
            if mounted {
                assert_eq!(response["result"]["entered"], method, "{response}");
            } else {
                assert_eq!(response["error"]["code"], METHOD_NOT_FOUND, "{response}");
            }
        }

        let held = json!({"jsonrpc":"2.0","method":"health","id":5});
        send(&mut stream, held.clone()).await;
        started.notified().await;
        send(&mut stream, held).await;
        assert_closed(&mut stream).await;
        while connections.read().await.contains_key(id) {
            tokio::task::yield_now().await;
        }
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        let mut revoked = connect(&endpoint).await;
        send(&mut revoked, hello(1, 1)).await;
        let response = receive(&mut revoked).await;
        let revoked_id = response["result"]["connection_id"].as_str().unwrap();
        store
            .update_user(OWNER_USER_ID, None, None, Some(UserStatus::Deactivated))
            .unwrap();
        send(
            &mut revoked,
            json!({"jsonrpc":"2.0","method":"sessions.list","id":2}),
        )
        .await;
        assert_eq!(receive(&mut revoked).await["error"]["code"], AUTH_REQUIRED);
        stop.send(()).unwrap();
        task.await.unwrap().unwrap();
        assert!(!connections.read().await.contains_key(revoked_id));
        assert_closed(&mut revoked).await;
        assert_retired(&endpoint).await;
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn desktop_ipc_cleans_up_when_server_future_is_cancelled() {
    let (_root, endpoint) = fixture();
    let mut gateway = GatewayServer::new("127.0.0.1:0".parse().unwrap());
    gateway.set_security_store(Arc::new(SecurityStore::in_memory().unwrap()));
    let connections = gateway.connections.clone();
    let (_stop, shutdown) = oneshot::channel();
    let path = endpoint.clone();
    let task = tokio::spawn(async move { gateway.run_local_endpoint(shutdown, &path).await });
    timeout(Duration::from_secs(3), async {
        let mut stream = connect(&endpoint).await;
        send(&mut stream, hello(1, 1)).await;
        let response = receive(&mut stream).await;
        let id = response["result"]["connection_id"].as_str().unwrap();
        assert!(connections.read().await.contains_key(id));
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_closed(&mut stream).await;
        while connections.read().await.contains_key(id) {
            tokio::task::yield_now().await;
        }
        assert_retired(&endpoint).await;
    })
    .await
    .expect("cancelled service must retire its server-admitted connection");
}

#[cfg(windows)]
#[tokio::test]
async fn desktop_ipc_caps_unauthenticated_pipes_and_recovers_after_disconnect() {
    use std::fs::OpenOptions;
    use windows_sys::Win32::Foundation::{
        ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_PIPE_BUSY,
    };

    let (_root, endpoint) = fixture();
    let mut gateway = GatewayServer::new("127.0.0.1:0".parse().unwrap());
    gateway.set_security_store(Arc::new(SecurityStore::in_memory().unwrap()));
    let connections = gateway.connections.clone();
    let (stop, shutdown) = oneshot::channel();
    let path = endpoint.clone();
    let task = tokio::spawn(async move { gateway.run_local_endpoint(shutdown, &path).await });
    timeout(Duration::from_secs(3), async {
        // These peers never send bytes. Use plain OS handles so dropping them
        // closes immediately, with no client-side overlapped read to drain.
        let open_silent = || OpenOptions::new().read(true).write(true).open(&endpoint);
        let mut silent = Vec::new();
        while silent.len() < super::MAX_CONNECTIONS {
            match open_silent() {
                Ok(stream) => silent.push(stream),
                Err(error)
                    if [ERROR_FILE_NOT_FOUND as i32, ERROR_PIPE_BUSY as i32]
                        .into_iter()
                        .any(|code| error.raw_os_error() == Some(code)) =>
                {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                Err(error) => panic!("could not open a pre-authentication pipe: {error}"),
            }
        }
        assert_eq!(
            open_silent().unwrap_err().raw_os_error(),
            Some(ERROR_PIPE_BUSY as i32)
        );
        assert!(connections.read().await.is_empty());
        let account = aleph_client::local_ipc::windows::PipeAccount::current().unwrap();
        let error = account
            .create(&endpoint, true, super::MAX_CONNECTIONS)
            .unwrap_err();
        // At the instance cap, the kernel can report saturation before it
        // evaluates first-instance exclusivity.
        assert!(
            matches!(
                error.raw_os_error().map(|code| code as u32),
                Some(ERROR_ACCESS_DENIED | ERROR_PIPE_BUSY)
            ),
            "unexpected first-instance error at capacity: {error:?}"
        );
        drop(silent);
        let mut admitted = connect(&endpoint).await;
        send(&mut admitted, hello(1, 1)).await;
        assert!(receive(&mut admitted).await["result"]["connection_id"].is_string());
        stop.send(()).unwrap();
        task.await.unwrap().unwrap();
        assert_closed(&mut admitted).await;
        assert!(connections.read().await.is_empty());
        assert_retired(&endpoint).await;
    })
    .await
    .expect("the saturated listener must retire disconnected peers and accept again");
}

#[cfg(unix)]
#[tokio::test]
async fn desktop_ipc_leaves_occupied_and_published_entries_untouched() {
    let root = tempfile::tempdir_in("/tmp").unwrap();
    let endpoint = root.path().join("gateway.sock");
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(super::bind(&endpoint).is_err());
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(&endpoint, "operator file").unwrap();
    assert!(super::bind(&endpoint).is_err());
    assert_eq!(std::fs::read_to_string(&endpoint).unwrap(), "operator file");
    std::fs::remove_file(&endpoint).unwrap();
    let stale = super::bind(&endpoint).unwrap();
    drop(stale);
    assert!(std::fs::symlink_metadata(&endpoint)
        .unwrap()
        .file_type()
        .is_socket());
    assert!(super::bind(&endpoint).is_err());
    assert!(try_connect(&endpoint).await.is_err());

    let endpoint = root.path().join("fresh.sock");
    let listener = super::bind(&endpoint).unwrap();
    assert!(super::bind(&endpoint).is_err());
    std::fs::rename(&endpoint, root.path().join("original.sock")).unwrap();
    std::fs::write(&endpoint, "replacement").unwrap();
    drop(listener);
    assert_eq!(std::fs::read_to_string(&endpoint).unwrap(), "replacement");
}

/// S3: the chat trio is mounted on Desktop IPC, and a retried `chat.send`
/// with the same idempotency key is answered from the cache instead of
/// starting a second run — v1 no longer strips the cache key.
#[tokio::test]
async fn desktop_ipc_mounts_chat_and_caches_sends_by_idempotency_key() {
    let (_root, endpoint) = fixture();
    let mut gateway = GatewayServer::new("127.0.0.1:0".parse().unwrap());
    gateway.set_security_store(Arc::new(SecurityStore::in_memory().unwrap()));
    let sends = Arc::new(AtomicUsize::new(0));
    gateway.handlers_mut().register("chat.send", {
        let sends = sends.clone();
        move |req| {
            let run = sends.fetch_add(1, Ordering::SeqCst);
            async move {
                crate::gateway::protocol::JsonRpcResponse::success(
                    req.id,
                    json!({"run_id": format!("run-{run}")}),
                )
            }
        }
    });
    for method in ["chat.history", "chat.abort"] {
        gateway.handlers_mut().register(method, |req| async move {
            crate::gateway::protocol::JsonRpcResponse::success(req.id, json!({"ok": true}))
        });
    }
    let (stop, shutdown) = oneshot::channel();
    let path = endpoint.clone();
    let task = tokio::spawn(async move { gateway.run_local_endpoint(shutdown, &path).await });
    timeout(Duration::from_secs(5), async {
        let mut stream = connect(&endpoint).await;
        send(&mut stream, hello(1, 1)).await;
        assert!(receive(&mut stream).await.get("result").is_some());
        for (id, method) in [(2, "chat.history"), (3, "chat.abort")] {
            send(
                &mut stream,
                json!({"jsonrpc":"2.0","method":method,"id":id,"params":{}}),
            )
            .await;
            let response = receive(&mut stream).await;
            assert_eq!(response["result"]["ok"], true, "{method}: {response}");
        }
        let mut first_run = None;
        for id in [4, 5] {
            send(
                &mut stream,
                json!({"jsonrpc":"2.0","method":"chat.send","id":id,
                       "params":{"message":"hi","idempotency_key":"attempt-1"}}),
            )
            .await;
            let response = receive(&mut stream).await;
            assert_eq!(response["id"], id);
            let run = response["result"]["run_id"].clone();
            assert_eq!(*first_run.get_or_insert(run.clone()), run);
        }
        assert_eq!(sends.load(Ordering::SeqCst), 1);
        stop.send(()).unwrap();
        task.await.unwrap().unwrap();
    })
    .await
    .unwrap();
}

/// S3 live events: a connection that negotiated `live_events` receives the
/// run lifecycle of its own sessions — through the shared admission, then the
/// v1 topic list — and an oversized frame becomes a warning on a live
/// connection. A connection that did not negotiate receives nothing.
#[tokio::test]
async fn desktop_ipc_pushes_admitted_events_only_after_negotiation() {
    let (root, endpoint) = fixture();
    let sessions = Arc::new(
        FileSessionStore::new(FileSessionStoreConfig {
            base_dir: root.path().join("sessions"),
            ..Default::default()
        })
        .unwrap(),
    );
    let own = SessionKey::main("owner-live");
    let other = SessionKey::main("other-live");
    for (owner, key) in [(OWNER_USER_ID, &own), ("other-user", &other)] {
        with_scope(
            Some(ScopeAttribution::personal(owner)),
            sessions.get_or_create(key),
        )
        .await
        .unwrap();
    }
    let mut gateway = GatewayServer::new("127.0.0.1:0".parse().unwrap());
    gateway.set_security_store(Arc::new(SecurityStore::in_memory().unwrap()));
    gateway.set_session_store(sessions);
    gateway
        .handlers_mut()
        .register("health", crate::gateway::handlers::health::handle);
    let bus = gateway.event_bus.clone();
    let (stop, shutdown) = oneshot::channel();
    let path = endpoint.clone();
    let task = tokio::spawn(async move { gateway.run_local_endpoint(shutdown, &path).await });
    let stream_frame = |method: &str, params: Value| -> String {
        json!({"jsonrpc":"2.0","method":method,"params":params}).to_string()
    };
    let accepted = |run: &str, key: &SessionKey| {
        stream_frame(
            "stream.run_accepted",
            json!({"type":"run_accepted","run_id":run,"session_key":key.to_key_string(),"accepted_at":"t"}),
        )
    };
    let chunk = |run: &str, text: &str| {
        stream_frame(
            "stream.response_chunk",
            json!({"type":"response_chunk","run_id":run,"seq":1,"delta":text,"content":text}),
        )
    };
    // Generous: the oversized frame is parsed several times at ~8 MiB in a
    // debug build, which one parallel suite run pushed past 5 s.
    timeout(Duration::from_secs(20), async {
        let mut live = connect(&endpoint).await;
        let mut request = hello(1, 1);
        request["params"]["features"] = json!(["event_replay", "live_events"]);
        send(&mut live, request).await;
        assert_eq!(
            receive(&mut live).await["result"]["features"],
            json!(["live_events"])
        );
        let mut quiet = connect(&endpoint).await;
        send(&mut quiet, hello(1, 1)).await;
        assert_eq!(receive(&mut quiet).await["result"]["features"], json!([]));

        let own_accepted = accepted("r-own", &own);
        let own_chunk = chunk("r-own", "hi");
        let own_complete = stream_frame(
            "stream.run_complete",
            json!({"type":"run_complete","run_id":"r-own","seq":3,"summary":{},"total_duration_ms":1}),
        );
        for frame in [
            accepted("r-other", &other),
            own_accepted.clone(),
            stream_frame(
                "stream.reasoning",
                json!({"type":"reasoning","run_id":"r-own","seq":2,"content":"x"}),
            ),
            own_chunk.clone(),
            chunk("r-other", "not yours"),
            chunk("r-own", &"x".repeat(MAX_FRAME_BYTES_FOR_TEST + 1)),
            own_complete.clone(),
        ] {
            bus.publish(frame);
        }
        for expected in [&own_accepted, &own_chunk] {
            let received = receive(&mut live).await;
            assert_eq!(received, serde_json::from_str::<Value>(expected).unwrap());
        }
        let warning = receive(&mut live).await;
        assert_eq!(warning["params"]["topic"], "connection.warning", "{warning}");
        assert_eq!(warning["params"]["data"]["dropped"], 1);
        assert_eq!(
            receive(&mut live).await,
            serde_json::from_str::<Value>(&own_complete).unwrap()
        );
        // Publish more than the reply queue can hold in one scheduler turn.
        // The client reads continuously once the burst is published.
        let burst: Vec<_> = (0..super::MAX_INFLIGHT + 4)
            .map(|seq| accepted(&format!("r-burst-{seq}"), &own))
            .collect();
        for frame in &burst {
            bus.publish(frame);
        }
        for expected in &burst {
            assert_eq!(
                receive(&mut live).await,
                serde_json::from_str::<Value>(expected).unwrap()
            );
        }
        send(&mut live, json!({"jsonrpc":"2.0","method":"health","id":2})).await;
        assert_eq!(receive(&mut live).await["id"], 2);

        // Anything pushed to the quiet connection would arrive before this.
        send(&mut quiet, json!({"jsonrpc":"2.0","method":"health","id":2})).await;
        assert_eq!(receive(&mut quiet).await["id"], 2);
        stop.send(()).unwrap();
        task.await.unwrap().unwrap();
    })
    .await
    .unwrap();
}

const MAX_FRAME_BYTES_FOR_TEST: usize = aleph_client::local_ipc::MAX_FRAME_BYTES as usize;

#[tokio::test]
async fn voice_turn_v1_ingress_is_server_derived() {
    use crate::gateway::handlers::voice_turn_tests::{
        mount_synthesis_bytes, transcribe_request, transcribe_request_with_samples, SttFixture,
    };
    use aleph_protocol::panel_voice::MAX_UTTERANCE_SAMPLES;
    use std::sync::atomic::Ordering;

    let (_root, endpoint) = fixture();
    let provider = SttFixture::new().await;
    let store = Arc::new(SecurityStore::in_memory().unwrap());
    let mut gateway = GatewayServer::new("127.0.0.1:0".parse().unwrap());
    gateway.set_security_store(store.clone());
    provider.mount(gateway.handlers_mut());
    let tts_calls = mount_synthesis_bytes(gateway.handlers_mut(), 64_000);
    let (stop, shutdown) = oneshot::channel();
    let path = endpoint.clone();
    let task = tokio::spawn(async move { gateway.run_local_endpoint(shutdown, &path).await });
    timeout(Duration::from_secs(8), async {
        let mut unauthenticated = connect(&endpoint).await;
        send(&mut unauthenticated, transcribe_request()).await;
        assert_eq!(
            receive(&mut unauthenticated).await["error"]["code"],
            AUTH_REQUIRED
        );
        assert_closed(&mut unauthenticated).await;
        assert_eq!(provider.calls(), 0);

        let mut stream = connect(&endpoint).await;
        send(&mut stream, hello(1, 1)).await;
        assert!(receive(&mut stream).await.get("result").is_some());
        send(
            &mut stream,
            transcribe_request_with_samples(MAX_UTTERANCE_SAMPLES),
        )
        .await;
        assert_eq!(receive(&mut stream).await["result"]["text"], "fixture");
        assert_eq!(provider.calls(), 1);
        send(
            &mut stream,
            json!({"jsonrpc":"2.0","id":2,"method":"voice.turn.synthesize","params":{
                "turn_id":"1b63aa6b-0688-4be3-991f-e6994e0f795c","text":"Speak."
            }}),
        )
        .await;
        let result = receive(&mut stream).await;
        assert_eq!(
            result["result"]["mime_type"],
            "audio/mpeg",
            "{:?}",
            result.get("error")
        );
        assert_eq!(
            result["result"]["audio_base64"].as_str().unwrap().len(),
            85_336
        );
        assert_eq!(tts_calls.load(Ordering::SeqCst), 1);

        let configured = std::mem::take(
            &mut provider
                .config
                .write()
                .await
                .generation
                .transcription_providers,
        );
        send(&mut stream, transcribe_request()).await;
        assert_eq!(
            receive(&mut stream).await["error"]["data"]["kind"],
            "offline_unavailable"
        );
        assert_eq!(provider.calls(), 1);

        provider
            .config
            .write()
            .await
            .generation
            .transcription_providers = configured;
        store
            .update_user(OWNER_USER_ID, None, None, Some(UserStatus::Deactivated))
            .unwrap();
        send(&mut stream, transcribe_request()).await;
        assert_eq!(receive(&mut stream).await["error"]["code"], AUTH_REQUIRED);
        assert_eq!(provider.calls(), 1);
    })
    .await
    .unwrap();
    let _ = stop.send(());
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn voice_turn_control_bypasses_full_ipc_wire_budget_inflight_and_query_lane() {
    blocked_voice_turn_ipc(BlockedPeerExit::CancelThenDisconnect).await;
}

#[tokio::test]
async fn voice_turn_ipc_disconnect_drops_active_provider_waits() {
    blocked_voice_turn_ipc(BlockedPeerExit::Disconnect).await;
}

#[tokio::test]
async fn voice_turn_ipc_ordinary_wire_budget_overflow_closes_peer() {
    blocked_voice_turn_ipc(BlockedPeerExit::OrdinaryOverflow).await;
}

#[tokio::test]
async fn voice_capture_ipc_control_bypasses_full_wire_budget_and_query_lane() {
    blocked_voice_turn_ipc(BlockedPeerExit::CaptureControls).await;
}

#[derive(Clone, Copy)]
enum BlockedPeerExit {
    CancelThenDisconnect,
    CaptureControls,
    Disconnect,
    OrdinaryOverflow,
}

async fn blocked_voice_turn_ipc(exit: BlockedPeerExit) {
    use crate::gateway::{
        handlers::voice_turn_tests::{
            blocked_stt_cancel, blocked_stt_turn, fill_query_lane, BlockedSttFixture,
        },
        lane::ChannelClass,
    };
    use aleph_client::local_ipc::MAX_INFLIGHT;
    use aleph_protocol::panel_voice::{
        VOICE_TURN_CAPTURE_CANCEL_METHOD, VOICE_TURN_CAPTURE_READY_METHOD,
    };

    let (_root, endpoint) = fixture();
    let mut gateway = GatewayServer::new("127.0.0.1:0".parse().unwrap());
    gateway.set_security_store(Arc::new(SecurityStore::in_memory().unwrap()));
    if matches!(exit, BlockedPeerExit::CaptureControls) {
        Arc::new(crate::gateway::handlers::voice_turn_tests::CaptureMediaFixture::default())
            .mount(gateway.handlers_mut());
    }
    let provider = BlockedSttFixture::new(gateway.handlers_mut()).await;
    let lanes = gateway.lane_manager.clone();
    let (stop, shutdown) = oneshot::channel();
    let path = endpoint.clone();
    let server = tokio::spawn(async move { gateway.run_local_endpoint(shutdown, &path).await });
    timeout(Duration::from_secs(20), async {
        let mut stream = connect(&endpoint).await;
        send(&mut stream, hello(1, 1)).await;
        assert!(receive(&mut stream).await.get("result").is_some());
        let (count, frame_bytes) = match exit {
            BlockedPeerExit::CancelThenDisconnect | BlockedPeerExit::CaptureControls => {
                let count = MAX_INFLIGHT;
                let frame_bytes = super::WIRE_BUDGET / count as usize;
                assert_eq!(frame_bytes * count as usize, super::WIRE_BUDGET);
                (count, Some(frame_bytes))
            }
            BlockedPeerExit::Disconnect => (MAX_INFLIGHT, None),
            BlockedPeerExit::OrdinaryOverflow => (2, Some(super::WIRE_BUDGET / 2)),
        };
        for id in 1..=count {
            let mut text = blocked_stt_turn(id).to_string();
            if let Some(frame_bytes) = frame_bytes {
                assert!(text.len() < frame_bytes && frame_bytes <= super::MAX_FRAME_BYTES as usize);
                text.extend(std::iter::repeat_n(' ', frame_bytes - text.len()));
            }
            write_frame(&mut stream, &text).await.unwrap();
        }
        provider.entered(count).await;
        let _remaining_query_permits = fill_query_lane(&lanes, ChannelClass::Desktop).await;
        let cancelled = match exit {
            BlockedPeerExit::CancelThenDisconnect => {
                send(&mut stream, blocked_stt_cancel(1)).await;
                let mut responses = [receive(&mut stream).await, receive(&mut stream).await];
                responses.sort_by_key(|response| response["id"].as_u64().unwrap());
                assert_eq!(responses[0]["error"]["data"]["kind"], "cancelled");
                assert_eq!(responses[1]["result"]["cancelled"], true);
                provider.exited(1).await;
                1
            }
            BlockedPeerExit::CaptureControls => {
                let capture_id = uuid::Uuid::new_v4();
                for (id, method) in [
                    ("ready-a", VOICE_TURN_CAPTURE_READY_METHOD),
                    ("ready-b", VOICE_TURN_CAPTURE_READY_METHOD),
                    ("cleanup", VOICE_TURN_CAPTURE_CANCEL_METHOD),
                ] {
                    send(
                        &mut stream,
                        json!({"jsonrpc":"2.0","id":id,"method":method,
                        "params":{"capture_id":capture_id}}),
                    )
                    .await;
                }
                let mut replies = [
                    receive(&mut stream).await,
                    receive(&mut stream).await,
                    receive(&mut stream).await,
                ];
                replies.sort_by(|left, right| left["id"].as_str().cmp(&right["id"].as_str()));
                assert_eq!(replies[0]["id"], "cleanup");
                assert_eq!(replies[0]["result"], json!({}));
                for (reply, expected_id) in replies[1..].iter().zip(["ready-a", "ready-b"]) {
                    assert_eq!(reply["id"], expected_id);
                    assert_eq!(reply["error"]["data"]["kind"], "cancelled");
                }
                0
            }
            BlockedPeerExit::OrdinaryOverflow => {
                assert!(count < MAX_INFLIGHT, "exercise bytes, not request count");
                send(&mut stream, blocked_stt_turn(count + 1)).await;
                assert_closed(&mut stream).await;
                0
            }
            BlockedPeerExit::Disconnect => 0,
        };
        drop(stream);
        provider.exited(count - cancelled).await;
    })
    .await
    .expect("blocked IPC provider wait must end without releasing the fixture");
    let _ = stop.send(());
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn voice_snapshot_ipc_requires_negotiated_live_events() {
    use aleph_protocol::panel::{PanelUiCapabilitySnapshot, PANEL_UI_CAPABILITY_SNAPSHOT_METHOD};
    use aleph_protocol::panel_voice::{
        VOICE_TURN_CANCEL_METHOD, VOICE_TURN_SYNTHESIZE_METHOD, VOICE_TURN_TRANSCRIBE_METHOD,
    };

    let (_root, endpoint) = fixture();
    let mut gateway = GatewayServer::new("127.0.0.1:0".parse().unwrap());
    gateway.set_security_store(Arc::new(SecurityStore::in_memory().unwrap()));
    // Only substitute the extension registry's snapshot. Dispatch, negotiation,
    // and its voice projection must still run through the actual connection.
    gateway
        .handlers_mut()
        .register(PANEL_UI_CAPABILITY_SNAPSHOT_METHOD, |request| async move {
            crate::gateway::protocol::JsonRpcResponse::success(
                request.id,
                serde_json::to_value(PanelUiCapabilitySnapshot::new(1, Vec::new())).unwrap(),
            )
        });
    for method in [
        VOICE_TURN_CANCEL_METHOD,
        VOICE_TURN_SYNTHESIZE_METHOD,
        VOICE_TURN_TRANSCRIBE_METHOD,
        "chat.send",
        "chat.abort",
    ] {
        gateway
            .handlers_mut()
            .register(method, |request| async move {
                crate::gateway::protocol::JsonRpcResponse::success(request.id, json!({}))
            });
    }
    let (stop, shutdown) = oneshot::channel();
    let path = endpoint.clone();
    let server = tokio::spawn(async move { gateway.run_local_endpoint(shutdown, &path).await });
    timeout(Duration::from_secs(10), async {
        for live_events in [true, false] {
            let mut stream = connect(&endpoint).await;
            let mut request = hello(1, 1);
            if live_events {
                request["params"]["features"] = json!(["live_events"]);
            }
            send(&mut stream, request).await;
            let greeting = receive(&mut stream).await;
            assert_eq!(
                greeting["result"]["features"],
                if live_events {
                    json!(["live_events"])
                } else {
                    json!([])
                }
            );
            send(
                &mut stream,
                json!({"jsonrpc":"2.0", "id":2, "method":PANEL_UI_CAPABILITY_SNAPSHOT_METHOD}),
            )
            .await;
            let response = receive(&mut stream).await;
            assert!(response.get("error").is_none(), "{response}");
            let snapshot: PanelUiCapabilitySnapshot =
                serde_json::from_value(response["result"].clone()).expect("Panel snapshot");
            assert_eq!(
                snapshot
                    .declarations
                    .iter()
                    .any(|tab| { tab.descriptor.key.qualified() == "aleph:voice" }),
                live_events
            );
        }
    })
    .await
    .expect("v1 snapshot must respond on both sessions");
    let _ = stop.send(());
    server.await.unwrap().unwrap();
}

struct CaptureReplyProbe {
    media: Arc<crate::gateway::handlers::voice_turn_tests::CaptureMediaFixture>,
    releases_at_enqueue: Vec<(u64, usize)>,
}

thread_local! {
    // This fixture and its spawned connection run on one current-thread runtime.
    static CAPTURE_REPLY_PROBE: std::cell::RefCell<Option<CaptureReplyProbe>> =
        const { std::cell::RefCell::new(None) };
}

struct CaptureReplyProbeScope;

impl Drop for CaptureReplyProbeScope {
    fn drop(&mut self) {
        let _ = CAPTURE_REPLY_PROBE.with(|probe| probe.replace(None));
    }
}

pub(super) fn observe_capture_reply_queued(id: &Value) {
    let Some(id @ (2 | 4)) = id.as_u64() else {
        return;
    };
    CAPTURE_REPLY_PROBE.with(|slot| {
        if let Some(probe) = slot.borrow_mut().as_mut() {
            let released = probe
                .media
                .released
                .load(std::sync::atomic::Ordering::SeqCst);
            probe.releases_at_enqueue.push((id, released));
        }
    });
}

#[tokio::test(flavor = "current_thread")]
async fn voice_capture_ipc_seals_before_reply_and_consumes_one_private_clip() {
    use crate::gateway::{
        handlers::voice_turn_tests::{fill_query_lane, CaptureMediaFixture, SttFixture},
        lane::ChannelClass,
    };
    use aleph_protocol::panel_voice::{
        VOICE_TURN_CAPTURE_FINISH_METHOD, VOICE_TURN_CAPTURE_METHOD,
        VOICE_TURN_CAPTURE_READY_METHOD, VOICE_TURN_TRANSCRIBE_METHOD,
    };
    use std::sync::atomic::Ordering;

    let (_root, endpoint) = fixture();
    let mut gateway = GatewayServer::new("127.0.0.1:0".parse().unwrap());
    gateway.set_security_store(Arc::new(SecurityStore::in_memory().unwrap()));
    let media = Arc::new(CaptureMediaFixture::default());
    media.mount(gateway.handlers_mut());
    CAPTURE_REPLY_PROBE.with(|probe| {
        assert!(probe
            .replace(Some(CaptureReplyProbe {
                media: media.clone(),
                releases_at_enqueue: Vec::new(),
            }))
            .is_none());
    });
    let _probe_scope = CaptureReplyProbeScope;
    let stt = SttFixture::new().await;
    stt.mount(gateway.handlers_mut());
    let lanes = gateway.lane_manager.clone();
    let (stop, shutdown) = oneshot::channel();
    let path = endpoint.clone();
    let server = tokio::spawn(async move { gateway.run_local_endpoint(shutdown, &path).await });
    timeout(Duration::from_secs(20), async {
        let mut stream = connect(&endpoint).await;
        send(&mut stream, hello(1, 1)).await;
        assert!(receive(&mut stream).await.get("result").is_some());
        let clip = uuid::Uuid::new_v4();
        let request = |id, method| {
            json!({"jsonrpc":"2.0","id":id,"method":method,
            "params":{"capture_id":clip}})
        };
        send(&mut stream, request(2, VOICE_TURN_CAPTURE_METHOD)).await;
        media.entered.notified().await;
        let query_permits = fill_query_lane(&lanes, ChannelClass::Desktop).await;
        send(&mut stream, request(3, VOICE_TURN_CAPTURE_READY_METHOD)).await;
        media.permit_samples.notify_one();
        let ready = receive(&mut stream).await;
        assert_eq!(ready["id"], 3);
        assert_eq!(ready["result"], json!({"ready":true}));
        send(&mut stream, request(4, VOICE_TURN_CAPTURE_FINISH_METHOD)).await;
        media.sealing.notified().await;
        assert_eq!(media.released.load(Ordering::SeqCst), 0);
        let first_reply = {
            let read_first = receive(&mut stream);
            tokio::pin!(read_first);
            assert!(
                futures::poll!(&mut read_first).is_pending(),
                "no result before release"
            );
            media.permit_release.notify_one();
            read_first.await
        };
        let mut replies = [first_reply, receive(&mut stream).await];
        assert_eq!(media.released.load(Ordering::SeqCst), 1);
        replies.sort_by_key(|reply| reply["id"].as_u64().unwrap());
        assert_eq!(replies[0]["id"], 2);
        assert_eq!(
            replies[0]["result"],
            json!({"capture_id":clip,"limit_reached":false})
        );
        assert_eq!(replies[1]["id"], 4);
        assert_eq!(replies[1]["result"], json!({}));
        CAPTURE_REPLY_PROBE.with(|slot| {
            let mut probe = slot.borrow_mut();
            let releases = &mut probe.as_mut().unwrap().releases_at_enqueue;
            releases.sort_unstable();
            assert_eq!(releases, &[(2, 1), (4, 1)], "both replies require release");
        });
        drop(query_permits);

        let mut other = connect(&endpoint).await;
        send(&mut other, hello(1, 1)).await;
        assert!(receive(&mut other).await.get("result").is_some());
        send(
            &mut other,
            json!({"jsonrpc":"2.0","id":7,"method":VOICE_TURN_TRANSCRIBE_METHOD,
            "params":{"turn_id":uuid::Uuid::new_v4(),"capture_id":clip}}),
        )
        .await;
        assert_eq!(
            receive(&mut other).await["error"]["data"]["kind"],
            "invalid_audio"
        );
        assert_eq!(
            stt.calls(),
            0,
            "a second socket cannot claim the sealed clip"
        );

        for id in [5, 6] {
            send(
                &mut stream,
                json!({"jsonrpc":"2.0","id":id,"method":VOICE_TURN_TRANSCRIBE_METHOD,
                "params":{"turn_id":uuid::Uuid::new_v4(),"capture_id":clip}}),
            )
            .await;
            let reply = receive(&mut stream).await;
            assert_eq!(reply["id"], id);
            if id == 5 {
                assert_eq!(reply["result"]["text"], "fixture");
            } else {
                assert_eq!(reply["error"]["data"]["kind"], "invalid_audio");
            }
        }
        assert_eq!(stt.calls(), 1);
    })
    .await
    .expect("capture controls and seal must still work with a full Query lane");
    let _ = stop.send(());
    server.await.unwrap().unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn voice_capture_ipc_disconnect_keeps_device_until_adapter_release() {
    use crate::gateway::handlers::voice_turn_tests::CaptureMediaFixture;
    use aleph_protocol::panel_voice::{
        VOICE_TURN_CAPTURE_CANCEL_METHOD, VOICE_TURN_CAPTURE_FINISH_METHOD,
        VOICE_TURN_CAPTURE_METHOD, VOICE_TURN_CAPTURE_READY_METHOD,
    };
    use std::sync::atomic::Ordering;

    let (_root, endpoint) = fixture();
    let mut gateway = GatewayServer::new("127.0.0.1:0".parse().unwrap());
    gateway.set_security_store(Arc::new(SecurityStore::in_memory().unwrap()));
    let media = Arc::new(CaptureMediaFixture::default());
    media.mount(gateway.handlers_mut());
    let (stop, shutdown) = oneshot::channel();
    let path = endpoint.clone();
    let server = tokio::spawn(async move { gateway.run_local_endpoint(shutdown, &path).await });
    timeout(Duration::from_secs(20), async {
        let mut owner = connect(&endpoint).await;
        let mut next = connect(&endpoint).await;
        for stream in [&mut owner, &mut next] {
            send(stream, hello(1, 1)).await;
            assert!(receive(stream).await.get("result").is_some());
        }
        let request = |id, method, clip| {
            json!({"jsonrpc":"2.0","id":id,"method":method,
            "params":{"capture_id":clip}})
        };
        let first = uuid::Uuid::new_v4();
        send(&mut owner, request(2, VOICE_TURN_CAPTURE_METHOD, first)).await;
        media.entered.notified().await;
        media.permit_samples.notify_one();
        send(
            &mut owner,
            request(3, VOICE_TURN_CAPTURE_READY_METHOD, first),
        )
        .await;
        assert_eq!(receive(&mut owner).await["result"]["ready"], true);
        send(
            &mut owner,
            request(4, VOICE_TURN_CAPTURE_FINISH_METHOD, first),
        )
        .await;
        media.sealing.notified().await;
        drop(owner);
        media.seal_cancelled.notified().await;
        assert_eq!(media.released.load(Ordering::SeqCst), 0);

        let second = uuid::Uuid::new_v4();
        send(&mut next, request(2, VOICE_TURN_CAPTURE_METHOD, second)).await;
        assert_eq!(
            receive(&mut next).await["error"]["data"]["kind"],
            "capture_unavailable"
        );
        media.permit_release.notify_one();
        // On this current-thread runtime the adapter returns and the supervisor
        // drops its non-async device permit before the test resumes.
        media.release_completed.notified().await;
        assert_eq!(media.released.load(Ordering::SeqCst), 1);

        let third = uuid::Uuid::new_v4();
        send(&mut next, request(3, VOICE_TURN_CAPTURE_METHOD, third)).await;
        media.entered.notified().await;
        send(
            &mut next,
            request(4, VOICE_TURN_CAPTURE_CANCEL_METHOD, third),
        )
        .await;
        let mut replies = [receive(&mut next).await, receive(&mut next).await];
        replies.sort_by_key(|reply| reply["id"].as_u64().unwrap());
        assert_eq!(replies[0]["error"]["data"]["kind"], "cancelled");
        assert_eq!(replies[1]["result"], json!({}));
        assert_eq!(media.released.load(Ordering::SeqCst), 2);
    })
    .await
    .expect("EOF must leave the device reserved until the old adapter returns");
    let _ = stop.send(());
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn voice_turn_ipc_reads_cancel_and_eof_during_partial_reply_write() {
    use crate::gateway::handlers::voice_turn_tests::{
        blocked_stt_cancel, blocked_stt_turn, BlockedSttFixture,
    };
    use tokio::io::{duplex, split, AsyncReadExt};
    use tokio_util::sync::CancellationToken;

    let mut gateway = GatewayServer::new("127.0.0.1:0".parse().unwrap());
    gateway.set_security_store(Arc::new(SecurityStore::in_memory().unwrap()));
    let provider = BlockedSttFixture::new(gateway.handlers_mut()).await;
    let bus = gateway.event_bus.clone();
    let state = gateway.build_shared_state();
    const BUFFER: usize = 64;
    let (mut client, server) = duplex(BUFFER);
    let task = tokio::spawn(async move {
        let (read, write) = split(server);
        let mut cleanup = None;
        let result =
            super::serve_peer(read, write, state, CancellationToken::new(), &mut cleanup).await;
        if let Some(cleanup) = cleanup {
            cleanup.finish().await;
        }
        result
    });

    let outcome = timeout(super::WRITE_TIMEOUT, async {
        let mut request = hello(1, 1);
        request["params"]["features"] = json!(["live_events"]);
        write_frame(&mut client, &request.to_string())
            .await
            .unwrap();
        let hello: Value =
            serde_json::from_str(&read_frame(&mut client).await.unwrap().unwrap()).unwrap();
        assert_eq!(hello["result"]["features"], json!(["live_events"]));
        for id in 1..=2 {
            write_frame(&mut client, &blocked_stt_turn(id).to_string())
                .await
                .unwrap();
        }
        provider.entered(2).await;

        let ordinary = |id| json!({"jsonrpc":"2.0","method":"fixture.unmounted","id":id});
        let large = ordinary(json!("x".repeat(2 * BUFFER))).to_string();
        write_frame(&mut client, &large).await.unwrap();
        // The writer has started this ordinary reply but its body cannot fit.
        let len = client.read_u32().await.unwrap() as usize;
        assert!(len > BUFFER);
        for id in 4..=super::MAX_INFLIGHT {
            write_frame(&mut client, &ordinary(json!(id)).to_string())
                .await
                .unwrap();
        }
        // Two provider waits, the partial reply and the completed ordinary
        // requests together use all ordinary slots. Event admission has
        // priority over reading the control frame.
        bus.publish(super::super::overflow_warning_frame(1, 1));
        write_frame(&mut client, &blocked_stt_cancel(1).to_string())
            .await
            .unwrap();
        provider.exited(1).await;

        let mut body = vec![0; len];
        client.read_exact(&mut body).await.unwrap();
        let mut ids = Vec::new();
        let mut event_count = 0;
        for _ in 0..super::MAX_INFLIGHT {
            let reply: Value =
                serde_json::from_str(&read_frame(&mut client).await.unwrap().unwrap()).unwrap();
            if reply["params"]["topic"] == "connection.warning" {
                event_count += 1;
            } else {
                let id = reply["id"].as_u64().unwrap();
                match id {
                    1 => assert_eq!(reply["error"]["data"]["kind"], "cancelled"),
                    1001 => {
                        assert_eq!(event_count, 1, "event must be ahead of the control reply");
                        assert_eq!(reply["result"]["cancelled"], true);
                    }
                    _ => assert_eq!(reply["error"]["code"], METHOD_NOT_FOUND),
                }
                ids.push(id);
            }
        }
        ids.sort_unstable();
        let expected: Vec<_> = [1]
            .into_iter()
            .chain(4..=u64::from(super::MAX_INFLIGHT))
            .chain([1001])
            .collect();
        assert_eq!(ids, expected);
        assert_eq!(event_count, 1);

        // Stall another reply before observing EOF with the other job active.
        write_frame(&mut client, &large).await.unwrap();
        assert!(client.read_u32().await.unwrap() as usize > BUFFER);
        drop(client);
        provider.exited(1).await;
    })
    .await;
    // The duplex write may observe the close before the reader does.
    let _ = task.await.unwrap();
    outcome.expect("cancel and EOF must not wait for the blocked writer");
}

#[tokio::test]
async fn desktop_ipc_delivers_lag_warning_despite_ready_malformed_requests() {
    use std::task::Poll;
    use tokio::io::duplex;
    use tokio_util::sync::CancellationToken;

    let mut gateway = GatewayServer::new("127.0.0.1:0".parse().unwrap());
    gateway.set_security_store(Arc::new(SecurityStore::in_memory().unwrap()));
    let bus = gateway.event_bus.clone();
    let state = gateway.build_shared_state();
    let (mut requests, read) = duplex(1024);
    let (write, mut replies) = duplex(64);
    let mut cleanup = None;

    timeout(super::WRITE_TIMEOUT, async {
        let mut request = hello(1, 1);
        request["params"]["features"] = json!(["live_events"]);
        write_frame(&mut requests, &request.to_string())
            .await
            .unwrap();
        // Both invalid payloads are already readable before the writer can
        // acknowledge the warning. The request side stays open for the test.
        for text in [
            "{",
            r#"{"jsonrpc":"2.0","method":"health"}"#,
            r#"{"jsonrpc":"2.0","method":"health","id":2}"#,
        ] {
            write_frame(&mut requests, text).await.unwrap();
        }

        let result = {
            let peer =
                super::serve_peer(read, write, state, CancellationToken::new(), &mut cleanup);
            tokio::pin!(peer);
            assert!(matches!(futures::poll!(peer.as_mut()), Poll::Pending));
            // No further peer polls until the real bus has overrun its ring.
            let event = json!({"jsonrpc":"2.0","method":"stream.run_accepted","params":{
                "type":"run_accepted","run_id":"lag-fixture",
                "session_key":SessionKey::main("owner-live").to_key_string(),"accepted_at":"t",
            }})
            .to_string();
            for _ in 0..2048 {
                assert!(bus.publish(&event) > 0, "hello must have subscribed");
            }

            let (result, ()) = tokio::join!(peer, async {
                let hello: Value =
                    serde_json::from_str(&read_frame(&mut replies).await.unwrap().unwrap())
                        .unwrap();
                assert_eq!(hello["result"]["features"], json!(["live_events"]));
                let warning: Value =
                    serde_json::from_str(&read_frame(&mut replies).await.unwrap().unwrap())
                        .unwrap();
                assert_eq!(
                    warning["method"],
                    aleph_protocol::jsonrpc::TOPIC_EVENT_METHOD
                );
                assert!(warning.get("id").is_none());
                assert_eq!(warning["params"]["topic"], "connection.warning");
                let data = &warning["params"]["data"];
                assert_eq!(data["reason"], "events_overflow");
                assert_eq!(data["advice"], "reconnect");
                assert!(data["dropped"].as_u64().unwrap() > 0);
                assert_eq!(data["dropped"], data["total_overflow"]);
                assert!(read_frame(&mut replies).await.unwrap().is_none());
            });
            result
        };
        if let Some(cleanup) = cleanup {
            cleanup.finish().await;
        }
        result.expect("peer must close after acknowledging the warning");
    })
    .await
    .expect("lag warning and EOF must arrive while the request side is still open");
}

#[tokio::test]
async fn voice_turn_ipc_adapter_retains_slot_and_clip_until_release_during_shutdown() {
    use crate::gateway::{
        handlers::voice_turn_tests::{
            blocked_stt_cancel, blocked_stt_turn, mount_gated_offline, SttFixture,
        },
        lane::Lane,
    };

    let (_root, endpoint) = fixture();
    let scratch = tempfile::tempdir().unwrap();
    let provider = SttFixture::new().await;
    let mut gateway = GatewayServer::new("127.0.0.1:0".parse().unwrap());
    gateway.set_security_store(Arc::new(SecurityStore::in_memory().unwrap()));
    let engine = mount_gated_offline(&provider, gateway.handlers_mut(), &scratch).await;
    gateway
        .handlers_mut()
        .register("health", crate::gateway::handlers::health::handle);
    let bus = gateway.event_bus.clone();
    let lanes = gateway.lane_manager.clone();
    let available = || {
        lanes
            .snapshot()
            .into_iter()
            .find(|row| row.lane == Lane::Query.to_string())
            .unwrap()
            .shared_available
    };
    let capacity = available();
    let clip_count = || std::fs::read_dir(scratch.path()).unwrap().count();
    let (stop, shutdown) = oneshot::channel();
    let path = endpoint.clone();
    let mut server = tokio::spawn(async move { gateway.run_local_endpoint(shutdown, &path).await });

    timeout(Duration::from_secs(8), async {
        let mut stream = connect(&endpoint).await;
        let mut request = hello(1, 1);
        request["params"]["features"] = json!(["live_events"]);
        send(&mut stream, request).await;
        assert_eq!(
            receive(&mut stream).await["result"]["features"],
            json!(["live_events"])
        );
        send(&mut stream, blocked_stt_turn(1)).await;
        engine.wait_started().await;
        send(&mut stream, blocked_stt_cancel(1)).await;
        assert_eq!(receive(&mut stream).await["result"]["cancelled"], true);
        engine.wait_cancelled().await;
        assert_eq!(clip_count(), 1);
        assert_eq!(available(), capacity - 1);

        // A second control proves progress without the cancelled work's reply.
        let mut duplicate = blocked_stt_cancel(1);
        duplicate["id"] = json!(1002);
        send(&mut stream, duplicate).await;
        let reply = receive(&mut stream).await;
        assert_eq!(reply["id"], 1002);
        assert_eq!(reply["result"]["cancelled"], false);

        // Force the live feed's clean Close, not the listener stop/error
        // path that unconditionally disconnects the pipe. No outbound frame
        // is needed, so the assertion cannot race unread pipe data.
        bus.publish(json!({"topic": super::super::TOKEN_ROTATED_TOPIC}).to_string());
        assert_closed(&mut stream).await;
        assert_eq!(clip_count(), 1);
        assert_eq!(available(), capacity - 1);

        let mut healthy = connect(&endpoint).await;
        send(&mut healthy, hello(1, 1)).await;
        assert!(receive(&mut healthy).await.get("result").is_some());
        send(
            &mut healthy,
            json!({"jsonrpc":"2.0","method":"health","id":2}),
        )
        .await;
        assert_eq!(receive(&mut healthy).await["result"]["status"], "healthy");
        drop(healthy);
        stop.send(()).unwrap();
        assert!(
            timeout(Duration::from_millis(50), &mut server)
                .await
                .is_err(),
            "shutdown left the adapter unowned"
        );
        assert_eq!(clip_count(), 1);
        assert_eq!(available(), capacity - 1);

        engine.release();
        server.await.unwrap().unwrap();
        assert_eq!(clip_count(), 0);
        assert_eq!(available(), capacity);
    })
    .await
    .unwrap();
}
