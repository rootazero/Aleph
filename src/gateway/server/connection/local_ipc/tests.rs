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
