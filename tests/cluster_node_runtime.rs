//! 集成测试：节点拨入中心 → 中心经反向 RPC 发 tool.call → 节点 dispatch 跑 bash
//! → 中心拿回结果。AuthMode::None 隔离传输（auth 由 0b 覆盖）。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use alephcore::cluster::{CommandTable, ReverseRpcChannel};
use alephcore::gateway::config::AuthMode;
use alephcore::gateway::server::{GatewayConfig, GatewayServer};
use alephcore::routing::session_key::SessionKey;
use alephcore::sandbox::{Sandbox, SandboxCommand, SandboxError, SandboxOutput};
use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::RwLock;
use tokio_tungstenite::tungstenite::Message;

type ReverseRpcRegistry = Arc<RwLock<HashMap<String, ReverseRpcChannel>>>;

/// 内联 canned sandbox：返回固定输出（test_util::MockSandbox 对集成测试不可见）。
struct CannedSandbox;

#[async_trait]
impl Sandbox for CannedSandbox {
    async fn execute(&self, _cmd: SandboxCommand) -> Result<SandboxOutput, SandboxError> {
        Ok(SandboxOutput {
            stdout: b"hi\n".to_vec(),
            exit_code: Some(0),
            duration_ms: 1,
            ..Default::default()
        })
    }
}

#[tokio::test]
async fn center_runs_bash_on_connected_node() {
    let config = GatewayConfig {
        auth_mode: AuthMode::None,
        ..Default::default()
    };
    let dummy: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let server = GatewayServer::with_config(dummy, config);
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
    let _keepalive = &server;

    let url = format!("ws://{bound}/ws");
    let (mut ws, _r) = tokio_tungstenite::connect_async(url.as_str()).await.unwrap();
    ws.send(Message::Text(
        json!({"jsonrpc":"2.0","id":1,"method":"connect",
               "params":{"device_name":"itest-node","device_id":"node-itest"}})
        .to_string()
        .into(),
    ))
    .await
    .unwrap();
    let _ = ws.next().await.expect("connect resp").unwrap();

    let bash = alephcore::builtin_tools::BashExecTool::new()
        .with_sandbox(Arc::new(CannedSandbox) as Arc<dyn Sandbox>);
    let table = CommandTable::with_bash(bash, SessionKey::ephemeral("itest-node"));
    let node = tokio::spawn(async move {
        while let Some(Ok(Message::Text(text))) = ws.next().await {
            let v: Value = match serde_json::from_str(text.as_str()) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if v["method"] == "tool.call" {
                let id = v["id"].clone();
                let result = table.dispatch("tool.call", &v["params"]).await.unwrap();
                ws.send(Message::Text(
                    json!({"jsonrpc":"2.0","id":id,"result":result})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();
                break;
            }
        }
    });

    let channel = wait_for_one_channel(&reverse_rpc).await;
    let resp = channel
        .call(
            "tool.call",
            json!({"tool":"bash","args":{"cmd":"echo hi"}}),
            5_000,
        )
        .await
        .expect("reverse rpc resolves");
    assert!(resp.is_success());
    assert!(resp.result.unwrap().get("exit_code").is_some());
    node.await.unwrap();
}

async fn wait_for_one_channel(reg: &ReverseRpcRegistry) -> ReverseRpcChannel {
    for _ in 0..100 {
        if let Some((_, ch)) = reg.read().await.iter().next() {
            return ch.clone();
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("no reverse_rpc channel registered within timeout");
}
