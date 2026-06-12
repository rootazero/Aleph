//! 集成测试：反向 RPC 端到端（服务端 → 已连 WS 客户端）。
//!
//! 验证 Phase 0a 的传输原语：一个真实 `GatewayServer`（LAN-trust 无鉴权）
//! 接受一个 WS 客户端连接后，能从 `server.reverse_rpc` 取出该连接的
//! `ReverseRpcChannel`，对客户端发起 `tool.call` 请求，并拿回客户端构造的响应。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use alephcore::cluster::ReverseRpcChannel;
use alephcore::gateway::server::{GatewayConfig, GatewayServer};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::sync::RwLock;
use tokio_tungstenite::tungstenite::Message;

type ReverseRpcRegistry = Arc<RwLock<HashMap<String, ReverseRpcChannel>>>;

#[tokio::test]
async fn server_calls_method_on_connected_client_and_gets_response() {
    // 1) 起服务端（LAN-trust 无鉴权；不调用 server.run()，自行 bind
    //    随机端口并 axum::serve，以拿到实际监听地址）。
    let config = GatewayConfig::default();
    let dummy_addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let server = GatewayServer::with_config(dummy_addr, config);

    let reverse_rpc: ReverseRpcRegistry = server.reverse_rpc.clone();
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

    // 2) 客户端连接 + connect 握手（无鉴权下也照常发，顺带验证 connect 这个
    //    *请求* 帧不会被服务端的"响应拦截"误吞），随后保持在线扮演应答节点。
    let url = format!("ws://{bound}/ws");
    let (mut ws, _resp) = tokio_tungstenite::connect_async(url.as_str())
        .await
        .expect("client should connect");
    ws.send(Message::Text(
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "connect",
            "params": {"device_name": "test-node", "device_id": "node-test"}
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

    // 3) 等连接登记进 reverse_rpc（连接建立时入站循环已 insert）。
    let channel = wait_for_one_channel(&reverse_rpc).await;

    // 4) 服务端发起反向 RPC，断言拿回客户端构造的响应。
    let resp = channel
        .call("tool.call", json!({"tool": "bash"}), 2_000)
        .await
        .expect("reverse rpc should resolve");

    assert!(resp.is_success());
    assert_eq!(resp.result.unwrap()["echoed"], "bash");
    client_task.await.unwrap();
}

/// 轮询 reverse_rpc 注册表直到出现一条连接通道（连接建立是异步的）。
async fn wait_for_one_channel(reg: &ReverseRpcRegistry) -> ReverseRpcChannel {
    for _ in 0..100 {
        if let Some((_, ch)) = reg.read().await.iter().next() {
            return ch.clone();
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("no reverse_rpc channel registered within timeout");
}
