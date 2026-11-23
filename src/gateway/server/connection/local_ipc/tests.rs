use std::{
    os::unix::fs::{FileTypeExt, PermissionsExt},
    path::Path,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use aleph_client::local_ipc::{parse_payload, read_frame, write_frame};
use serde_json::{json, Value};
use tokio::{
    net::UnixStream,
    sync::{oneshot, Notify},
    time::timeout,
};

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

async fn connect(endpoint: &Path) -> UnixStream {
    timeout(Duration::from_secs(2), async {
        loop {
            if let Ok(stream) = UnixStream::connect(endpoint).await {
                return stream;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("Gateway should mount the explicit UNIX socket")
}

async fn send(stream: &mut UnixStream, value: Value) {
    write_frame(stream, &value.to_string()).await.unwrap();
}

async fn receive(stream: &mut UnixStream) -> Value {
    serde_json::from_str(&read_frame(stream).await.unwrap().expect("response frame")).unwrap()
}

fn hello(id: u32, major: u32) -> Value {
    json!({"jsonrpc":"2.0","method":"system.hello","id":id,"params":{
        "protocol":{"major":major,"minor":9},
        "client":{"name":"fixture","version":"1","platform":"unix","ui_runtime":"test"},
        "features":["event_replay"],
    }})
}

#[tokio::test]
async fn desktop_ipc_uses_gateway_and_releases_its_connection() {
    let root = tempfile::tempdir_in("/tmp").unwrap();
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let endpoint = root.path().join("gateway.sock");
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
    let task = tokio::spawn(async move {
        gateway
            .run_with_desktop_ipc(shutdown, Some(&server_endpoint))
            .await
    });
    timeout(Duration::from_secs(5), async {
        let mut first = connect(&endpoint).await;
        send(
            &mut first,
            json!({"jsonrpc":"2.0","method":"sessions.list","id":1}),
        )
        .await;
        assert_eq!(receive(&mut first).await["error"]["code"], AUTH_REQUIRED);
        assert!(read_frame(&mut first).await.unwrap().is_none());
        let mut incompatible = connect(&endpoint).await;
        send(&mut incompatible, hello(1, 2)).await;
        assert_eq!(
            receive(&mut incompatible).await["error"]["code"],
            INVALID_PARAMS
        );
        assert!(read_frame(&mut incompatible).await.unwrap().is_none());

        let mut invalid = connect(&endpoint).await;
        let mut request = hello(42, 1);
        request["params"]["protocol"]["major"] = json!("x".repeat(65_536));
        send(&mut invalid, request).await;
        let frame = read_frame(&mut invalid).await.unwrap().unwrap();
        let response = parse_payload(&frame).expect("hello error must fit the Client shape bounds");
        assert_eq!(response["id"], 42);
        assert_eq!(response["error"]["code"], INVALID_PARAMS);
        assert!(read_frame(&mut invalid).await.unwrap().is_none());
        assert!(connections.read().await.is_empty());
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
        assert!(read_frame(&mut stream).await.unwrap().is_none());
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
        assert!(std::fs::symlink_metadata(&endpoint)
            .unwrap()
            .file_type()
            .is_socket());
        assert!(UnixStream::connect(&endpoint).await.is_err());
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn desktop_ipc_cleans_up_when_server_future_is_cancelled() {
    let root = tempfile::tempdir_in("/tmp").unwrap();
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let endpoint = root.path().join("gateway.sock");
    let mut gateway = GatewayServer::new("127.0.0.1:0".parse().unwrap());
    gateway.set_security_store(Arc::new(SecurityStore::in_memory().unwrap()));
    let connections = gateway.connections.clone();
    let (_stop, shutdown) = oneshot::channel();
    let path = endpoint.clone();
    let task =
        tokio::spawn(async move { gateway.run_with_desktop_ipc(shutdown, Some(&path)).await });
    timeout(Duration::from_secs(3), async {
        let mut stream = connect(&endpoint).await;
        send(&mut stream, hello(1, 1)).await;
        let response = receive(&mut stream).await;
        let id = response["result"]["connection_id"].as_str().unwrap();
        assert!(connections.read().await.contains_key(id));
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(read_frame(&mut stream).await.unwrap().is_none());
        while connections.read().await.contains_key(id) {
            tokio::task::yield_now().await;
        }
        assert!(UnixStream::connect(&endpoint).await.is_err());
        assert!(std::fs::symlink_metadata(&endpoint)
            .unwrap()
            .file_type()
            .is_socket());
    })
    .await
    .expect("cancelled service must retire its server-admitted connection");
}

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
    assert!(UnixStream::connect(&endpoint).await.is_err());

    let endpoint = root.path().join("fresh.sock");
    let listener = super::bind(&endpoint).unwrap();
    assert!(super::bind(&endpoint).is_err());
    std::fs::rename(&endpoint, root.path().join("original.sock")).unwrap();
    std::fs::write(&endpoint, "replacement").unwrap();
    drop(listener);
    assert_eq!(std::fs::read_to_string(&endpoint).unwrap(), "replacement");
}
