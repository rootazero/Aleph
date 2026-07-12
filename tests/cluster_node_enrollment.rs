//! 集成测试：节点**冷启动登记**与**注销粘性**，全程走真实 WS socket。
//!
//! 这两条路径此前一条都没有 socket 级测试——正因如此，「新节点永远登记不上」这个
//! bug 才能一直活着：中心强制「一条连接的首帧必须是 `connect`」，而旧的节点客户端
//! 另开一条 WS、首帧直接发 `cluster.enroll`，必被 `AUTH_REQUIRED` 拒绝并关连接。
//! 单测只直接调 handler 函数，完全绕过了那条规则。
//!
//! 现在登记发生在 `connect` 里（`cluster::admit_node`），本文件把它钉死。

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use alephcore::gateway::security::SecurityStore;
use alephcore::gateway::server::{GatewayConfig, GatewayServer};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

/// 起一个带 security store + 真实 `connect` handler 的中心，返回
/// (`ws_url`, store, 保活的 server)。
///
/// `connect` 必须真注册：节点登记就长在它的成功回包上。
async fn start_center() -> (String, Arc<SecurityStore>, GatewayServer) {
    let store = Arc::new(SecurityStore::in_memory().expect("in-memory store"));
    let dummy: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let mut server = GatewayServer::with_config(dummy, GatewayConfig::default());
    server.set_security_store(store.clone());

    let connect_ctx = Arc::new(alephcore::gateway::handlers::connect::ConnectContext {
        state_versions: server.state_versions.clone(),
        transport_policy: alephcore::gateway::handlers::auth::TransportPolicy::defaults(),
    });
    server.handlers_mut().register("connect", move |req| {
        let ctx = Arc::clone(&connect_ctx);
        async move { alephcore::gateway::handlers::connect::handle_connect(req, ctx).await }
    });

    let router = server.build_router();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let bound = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    (format!("ws://{bound}/ws"), store, server)
}

/// 拨一次 connect，返回中心回包里的 `result.node` 块。
async fn node_connect(url: &str, device_id: Option<&str>, name: &str) -> Value {
    let (mut ws, _r) = tokio_tungstenite::connect_async(url).await.unwrap();
    let mut params = serde_json::Map::new();
    if let Some(id) = device_id {
        params.insert("device_id".into(), json!(id));
    }
    params.insert("device_name".into(), json!(name));
    params.insert("commands".into(), json!([{"name": "bash", "schema": {}}]));
    params.insert("tags".into(), json!(["linux"]));

    ws.send(Message::Text(
        json!({"jsonrpc":"2.0","id":1,"method":"connect","params":params})
            .to_string()
            .into(),
    ))
    .await
    .unwrap();

    let reply = ws.next().await.expect("a connect reply").unwrap();
    let Message::Text(text) = reply else {
        panic!("connect reply must be text");
    };
    let v: Value = serde_json::from_str(text.as_str()).unwrap();
    // 关掉 socket；本测试只关心握手裁决。
    let _ = ws.close(None).await;
    v.pointer("/result/node")
        .cloned()
        .unwrap_or_else(|| panic!("connect reply carried no `node` block: {v}"))
}

#[tokio::test]
async fn first_boot_node_enrolls_through_connect() {
    let (url, store, _server) = start_center().await;

    // 首启：不带 device_id。中心必须当场铸一个 node_id 并让节点落盘。
    let node = node_connect(&url, None, "worker-1").await;
    assert_eq!(node["status"], "registered");
    assert_eq!(
        node["persist"], true,
        "a freshly minted id must be persisted by the node"
    );
    let node_id = node["node_id"].as_str().expect("a minted node_id");
    assert!(!node_id.is_empty());

    // 设备记录确实落进了 store（离线舰队视图的依据）。
    let devices = store.list_devices().unwrap();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].device_id, node_id);
    assert_eq!(devices[0].role, "node");
}

#[tokio::test]
async fn reconnect_reuses_the_persisted_id_without_ghost_rows() {
    let (url, store, _server) = start_center().await;

    let first = node_connect(&url, None, "worker-1").await;
    let node_id = first["node_id"].as_str().unwrap().to_string();

    // 二次启动：带上落盘的 id。同一条记录，无需再落盘。
    let again = node_connect(&url, Some(&node_id), "worker-1").await;
    assert_eq!(again["node_id"], node_id);
    assert_eq!(again["status"], "registered");
    assert_eq!(
        again["persist"], false,
        "an unchanged id must not rewrite the node's identity file"
    );
    assert_eq!(
        store.list_devices().unwrap().len(),
        1,
        "a reconnect must not mint a second, ghost device row"
    );
}

#[tokio::test]
async fn first_boot_adopts_the_operators_pre_enrolled_row() {
    let (url, store, _server) = start_center().await;

    // Operator 在 Panel 预登记（cluster.enroll 走的同一个真源）。
    let pre = alephcore::cluster::mint_node_device(&store, "GPU Box").expect("pre-enroll");

    // 节点用横杠小写变体拨入，且尚无 device_id。
    let node = node_connect(&url, None, "gpu-box").await;
    assert_eq!(
        node["node_id"], pre,
        "the node must adopt the pre-enrolled row, not mint a duplicate"
    );
    assert_eq!(
        store.list_devices().unwrap().len(),
        1,
        "no duplicate ghost row may appear in the offline fleet view"
    );
}

#[tokio::test]
async fn deregistration_survives_the_nodes_reconnect() {
    let (url, store, server) = start_center().await;

    let node = node_connect(&url, None, "worker-1").await;
    let node_id = node["node_id"].as_str().unwrap().to_string();

    // Operator 注销：吊销设备记录（cluster.deregister 的第 ② 步）。
    assert!(store.revoke_device(&node_id).unwrap());

    // 节点仍持有身份文件，退避后重连——中心必须拒绝，否则注销形同虚设。
    let after = node_connect(&url, Some(&node_id), "worker-1").await;
    assert_eq!(
        after["status"], "deregistered",
        "a revoked node must not be able to resurrect itself by reconnecting"
    );

    // 且它没有回到在线舰队里。
    tokio::time::sleep(Duration::from_millis(50)).await;
    let online = server.node_registry.list_environments();
    assert!(
        online.iter().all(|e| e.id != node_id),
        "the deregistered node must stay out of the live registry: {online:?}"
    );
}
