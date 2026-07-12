//! 集成测试：反向 RPC 端到端（中心 → 已连节点）。
//!
//! 验证传输原语：一个真实 `GatewayServer`（LAN-trust 无鉴权）接受一个**节点形状**
//! 的 WS 连接后，能从 `NodeRegistry` 取出该节点的 `ReverseRpcChannel`，对它发起
//! `tool.call` 请求，并拿回节点构造的响应。
//!
//! 走的是**生产路径**：`node_invoke` / `node_file` / 审批同样经 `NodeRegistry`
//! 拿 channel。（此前这里读的是一张 `GatewayServer::reverse_rpc` 旁路表——它在
//! 生产代码里从来只写不读，唯一的读者就是本测试，已随之删除。）

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use alephcore::cluster::{NodeRegistry, ReverseRpcChannel};
use alephcore::gateway::server::{GatewayConfig, GatewayServer};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio_tungstenite::tungstenite::Message;

#[tokio::test]
async fn center_calls_tool_on_connected_node_and_gets_response() {
    // 1) 起服务端（LAN-trust 无鉴权；不调用 server.run()，自行 bind 随机端口
    //    并 axum::serve，以拿到实际监听地址）。
    let config = GatewayConfig::default();
    let dummy_addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let mut server = GatewayServer::with_config(dummy_addr, config);

    // `connect` must be really registered: node registration hangs off its
    // success reply. (A bare GatewayServer registers no handlers.)
    let connect_ctx = Arc::new(alephcore::gateway::handlers::connect::ConnectContext {
        state_versions: server.state_versions.clone(),
        transport_policy: alephcore::gateway::handlers::auth::TransportPolicy::defaults(),
    });
    server.handlers_mut().register("connect", move |req| {
        let ctx = Arc::clone(&connect_ctx);
        async move { alephcore::gateway::handlers::connect::handle_connect(req, ctx).await }
    });

    let node_registry: Arc<NodeRegistry> = server.node_registry.clone();
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
    // 保持 `server` 存活至测试结束（其拥有的 Arc 经 build_router 共享给 router）。
    let _server_keepalive = &server;

    // 2) 客户端以**节点形状**连接：connect params 带 `commands` + `tags`，这是
    //    LAN-trust 下中心识别节点的唯一信号。随后保持在线扮演应答节点。
    let url = format!("ws://{bound}/ws");
    let (mut ws, _resp) = tokio_tungstenite::connect_async(url.as_str())
        .await
        .expect("client should connect");
    ws.send(Message::Text(
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "connect",
            "params": {
                "device_id": "node-test",
                "device_name": "test-node",
                "commands": [{"name": "bash", "schema": {}}],
                "tags": ["linux"]
            }
        })
        .to_string()
        .into(),
    ))
    .await
    .unwrap();
    // 读 connect 响应（内容不关心，仅消费一帧）。
    let _connect_resp = ws.next().await.expect("connect response").unwrap();

    // 客户端后台：收到 tool.call 请求 → 回成功响应（回显 tool 名）。
    let client_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = ws.next().await {
            if let Message::Text(text) = msg {
                let v: serde_json::Value = match serde_json::from_str(text.as_str()) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if v["method"] == "tool.call" {
                    let id = v["id"].clone();
                    let tool = v["params"]["tool"].clone();
                    ws.send(Message::Text(
                        json!({"jsonrpc": "2.0", "id": id, "result": {"echoed": tool}})
                            .to_string()
                            .into(),
                    ))
                    .await
                    .unwrap();
                    break;
                }
            }
        }
    });

    // 3) 等节点登记进 NodeRegistry（连接建立是异步的）。
    let (channel, _declared) = wait_for_node(&node_registry, "test-node").await;

    // 4) 中心发起反向 RPC，断言拿回节点构造的响应。
    let resp = channel
        .call("tool.call", json!({"tool": "bash"}), 2_000)
        .await
        .expect("reverse rpc should resolve");

    assert!(resp.is_success());
    assert_eq!(resp.result.unwrap()["echoed"], "bash");
    client_task.await.unwrap();
}

/// 轮询 `NodeRegistry` 直到该节点上线，返回 `node_invoke` 走的同一条 channel。
async fn wait_for_node(
    registry: &NodeRegistry,
    name: &str,
) -> (
    ReverseRpcChannel,
    Vec<alephcore::cluster::CommandDescriptor>,
) {
    for _ in 0..100 {
        if let Ok(found) = registry.resolve(name) {
            return found;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("node '{name}' never registered within timeout");
}
