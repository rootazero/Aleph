//! Real WebSocket dispatch and Core discovery with no forged caller test API.
#[path = "panel_http/common.rs"]
mod common;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use alephcore::gateway::handlers::auth::TransportPolicy;
use alephcore::gateway::handlers::connect::{handle_connect, ConnectContext};
use alephcore::gateway::security::store::UserRole;
use alephcore::gateway::security::{DeviceTokenManager, SecurityStore};
use alephcore::gateway::server::GatewayConfig;
use alephcore::gateway::state_version::StateVersionTracker;
use alephcore::gateway::GatewayServer;
use alephcore::projects::ProjectStore;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

const SELECT: &str = "panel.project.select";
const SNAPSHOT: &str = "panel.capabilities.snapshot";
type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

fn write_named_panel(parent: &Path, name: &str) {
    let directory = parent.join(".aleph");
    common::write_panel(&directory, "1.0.0", b"export const passive = true;");
    let old = directory.join("plugins/community-live");
    let manifest = old.join("aleph.plugin.toml");
    std::fs::write(
        &manifest,
        std::fs::read_to_string(&manifest)
            .unwrap()
            .replace("community-live", name),
    )
    .unwrap();
    std::fs::rename(old, directory.join("plugins").join(name)).unwrap();
}

fn device_token(tokens: &DeviceTokenManager, user: &str, device: &str) -> String {
    tokens
        .exchange_bootstrap_ticket(
            &tokens.create_bootstrap_ticket(None, Some(user)).unwrap(),
            Some(device.into()),
            Some("Panel project test".into()),
            None,
        )
        .unwrap()
        .device_token
}

async fn rpc(socket: &mut Socket, method: &str, params: Value) -> Value {
    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let message = socket.next().await.unwrap().unwrap();
            if let Some(text) = message.to_text().ok() {
                let response: Value = serde_json::from_str(text).unwrap();
                if response.get("id") == Some(&json!(1)) {
                    return response;
                }
            }
        }
    })
    .await
    .expect("production RPC must respond")
}

async fn connect(address: std::net::SocketAddr, token: Option<&str>) -> Socket {
    let mut socket = tokio_tungstenite::connect_async(format!("ws://{address}/ws"))
        .await
        .unwrap()
        .0;
    let response = rpc(&mut socket, "connect", json!({"device_token": token})).await;
    assert_eq!(response["result"]["authorized"], true);
    socket
}

fn plugins(snapshot: &Value) -> Vec<String> {
    let mut plugins = snapshot["result"]["declarations"]
        .as_array()
        .expect("canonical discovery result")
        .iter()
        .map(|declaration| {
            declaration["descriptor"]["key"]["plugin_id"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect::<Vec<_>>();
    plugins.sort();
    plugins
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn authenticated_project_selection_controls_real_rpc_discovery() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let _home = common::Home::set(&home, root.path());
    common::write_panel(&home, "1.0.0", b"export const global = true;");
    let projects = ProjectStore::shared();
    projects.create_schema().unwrap();
    let a_root = root.path().join("A");
    let b_root = root.path().join("B");
    write_named_panel(&a_root, "project-a");
    write_named_panel(&b_root, "project-b");
    let a = projects.create("A", Some("bob"), Some(&a_root)).unwrap();
    let b = projects.create("B", Some("bob"), Some(&b_root)).unwrap();
    projects.add_member(&a.id, "alice").unwrap();
    let manager = common::manager(root.path()).await;
    assert_eq!(manager.panel_ui_capability_snapshot().declarations.len(), 1);
    assert!(alephcore::extension::init_extension_manager(manager).is_ok());
    let security = Arc::new(SecurityStore::in_memory().unwrap());
    security
        .create_user("alice", "Alice", UserRole::Member)
        .unwrap();
    security
        .create_user("bob", "Bob", UserRole::Member)
        .unwrap();
    let tokens = Arc::new(DeviceTokenManager::new(security.clone()));
    let alice_token = device_token(&tokens, "alice", "alice-panel");
    let bob_token = device_token(&tokens, "bob", "bob-panel");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mut gateway = GatewayServer::with_config(
        address,
        GatewayConfig {
            require_idempotency_key: true,
            ..GatewayConfig::default()
        },
    );
    gateway.set_device_token_manager(tokens.clone());
    gateway.set_security_store(security);
    let context = Arc::new(ConnectContext {
        state_versions: Arc::new(StateVersionTracker::new()),
        transport_policy: TransportPolicy::defaults(),
    });
    gateway.handlers_mut().register("connect", move |request| {
        handle_connect(request, context.clone())
    });
    let router = gateway.build_router();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let mut alice = connect(address, Some(&alice_token)).await;
    let mut bob = connect(address, Some(&bob_token)).await;
    assert_eq!(
        plugins(&rpc(&mut alice, SNAPSHOT, Value::Null).await),
        ["community-live"]
    );
    for claim in [
        json!({"project_id": b.id}),
        json!({"project_root": b_root}),
        json!({"trusted": true}),
    ] {
        assert!(
            rpc(&mut alice, SNAPSHOT, claim)
                .await
                .get("error")
                .is_some(),
            "snapshot params cannot silently become a Global fallback"
        );
    }
    let selected = rpc(
        &mut alice,
        SELECT,
        json!({"project_id": a.id, "idempotency_key": "A"}),
    )
    .await;
    assert_eq!(
        selected["result"]["project_id"], a.id,
        "a verified project member must select A: {selected}"
    );
    let scoped = rpc(&mut alice, SNAPSHOT, Value::Null).await;
    assert_eq!(plugins(&scoped), ["community-live", "project-a"]);
    assert_eq!(scoped["result"]["project_context"], selected["result"]);
    assert!(
        !scoped
            .to_string()
            .contains(serde_json::to_string(&a_root).unwrap().trim_matches('"')),
        "roots stay internal"
    );
    assert_eq!(
        plugins(&rpc(&mut bob, SNAPSHOT, Value::Null).await),
        ["community-live"]
    );
    let mut same_device = connect(address, Some(&alice_token)).await;
    assert_eq!(
        plugins(&rpc(&mut same_device, SNAPSHOT, Value::Null).await),
        ["community-live", "project-a"]
    );
    let denied = rpc(
        &mut alice,
        SELECT,
        json!({"project_id": b.id, "idempotency_key": "foreign"}),
    )
    .await;
    assert!(denied.get("error").is_some());
    for (index, claim) in [
        json!({}),
        json!({"project_id": true}),
        json!({"project_id": "missing"}),
        json!({"project_id": ""}),
        json!({"project_id": a.id, "project_root": "untrusted"}),
    ]
    .into_iter()
    .enumerate()
    {
        let mut claim = claim;
        claim["idempotency_key"] = json!(format!("bad-{index}"));
        assert!(rpc(&mut alice, SELECT, claim).await.get("error").is_some());
        assert_eq!(
            plugins(&rpc(&mut alice, SNAPSHOT, Value::Null).await),
            ["community-live", "project-a"]
        );
    }
    projects.add_member(&b.id, "alice").unwrap();
    assert_eq!(
        rpc(
            &mut alice,
            SELECT,
            json!({"project_id": b.id, "idempotency_key": "B"})
        )
        .await["result"]["project_id"],
        b.id
    );
    assert_eq!(
        plugins(&rpc(&mut same_device, SNAPSHOT, Value::Null).await),
        ["community-live", "project-b"]
    );
    // Retrying an old key must apply the selection, never replay stale success.
    assert_eq!(
        rpc(
            &mut alice,
            SELECT,
            json!({"project_id": a.id, "idempotency_key": "A"})
        )
        .await["result"]["project_id"],
        a.id
    );
    assert_eq!(
        plugins(&rpc(&mut alice, SNAPSHOT, Value::Null).await),
        ["community-live", "project-a"]
    );
    projects.remove_member(&a.id, "alice").unwrap();
    assert!(
        rpc(&mut alice, SNAPSHOT, Value::Null)
            .await
            .get("error")
            .is_some(),
        "revoked project never falls back to Global"
    );
    assert_eq!(
        rpc(
            &mut alice,
            SELECT,
            json!({"project_id": null, "idempotency_key": "clear"})
        )
        .await["result"]["project_id"],
        Value::Null
    );
    assert_eq!(
        plugins(&rpc(&mut alice, SNAPSHOT, Value::Null).await),
        ["community-live"]
    );
    let mut compatibility = connect(address, None).await;
    assert_eq!(
        plugins(&rpc(&mut compatibility, SNAPSHOT, Value::Null).await),
        ["community-live"]
    );
    assert!(rpc(
        &mut compatibility,
        SELECT,
        json!({"project_id": b.id, "idempotency_key": "compat"})
    )
    .await
    .get("error")
    .is_some());
    tokens.revoke_device("bob-panel").unwrap();
    assert!(
        rpc(&mut bob, SNAPSHOT, Value::Null)
            .await
            .get("error")
            .is_some(),
        "revoked paired identity cannot query even without a selection"
    );
    alice.close(None).await.unwrap();
    bob.close(None).await.unwrap();
    same_device.close(None).await.unwrap();
    compatibility.close(None).await.unwrap();
    server.abort();
}
