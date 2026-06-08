//! `aleph-server node` —— 集群节点（执行臂）拨出运行时。
//!
//! 拨向中心 WS、用 node-token 认证、声明命令、入站循环服务 `tool.call`，
//! 在本机 sandbox 跑 bash。断线指数退避重连。无 DB / 无 harness / 无 LLM。

use std::sync::Arc;
use std::time::Duration;

use alephcore::cluster::{CommandDescriptor, CommandTable};
use alephcore::gateway::protocol::JsonRpcResponse;
use alephcore::routing::session_key::SessionKey;
use alephcore::sandbox::exec_approval::gate::ApprovalGate;
use alephcore::sandbox::exec_approval::types::ApprovalConfig;
use alephcore::sandbox::factory::build_sandbox;
use alephcore::sandbox::platforms::create_platform_driver_from_config;
use alephcore::sandbox::rate_limit::SandboxRateLimitConfig;
use alephcore::sandbox::config::SandboxConfig;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

const BACKOFF_INITIAL_MS: u64 = 2_000;
const BACKOFF_MAX_MS: u64 = 60_000;

pub async fn handle_node(
    center: String,
    token: String,
    name: String,
) -> Result<(), Box<dyn std::error::Error>> {
    let table = Arc::new(build_command_table(&name));
    let declared = table.descriptors();
    let url = format!("{}/ws", center.trim_end_matches('/'));

    let mut backoff = BACKOFF_INITIAL_MS;
    loop {
        match run_session(&url, &token, &name, &declared, &table).await {
            Ok(()) => {
                tracing::warn!("node session ended cleanly; reconnecting");
                backoff = BACKOFF_INITIAL_MS;
            }
            Err(e) => tracing::error!("node session error: {e}; retrying in {backoff}ms"),
        }
        tokio::time::sleep(Duration::from_millis(backoff)).await;
        backoff = (backoff * 2).min(BACKOFF_MAX_MS);
    }
}

/// 建节点 sandbox（镜像 sandbox_debug.rs，生产式 `None` 审批 gate=headless 安全，
/// 升权一律拒）+ 唯一 bash 命令。
fn build_command_table(name: &str) -> CommandTable {
    let cfg = SandboxConfig::default();
    let driver = create_platform_driver_from_config(&cfg);
    // Production-style headless gate: `None` requester means any capability
    // escalation is denied (no operator to prompt). Mirror the daemon, NOT
    // the debug CLI's auto-approver.
    let gate = Arc::new(ApprovalGate::new(ApprovalConfig::default(), None));
    let sandbox = build_sandbox(
        &cfg,
        driver,
        gate,
        SandboxRateLimitConfig::default(),
        &alephcore::ShellSecurityConfig::default(),
    );
    let bash = alephcore::builtin_tools::BashExecTool::new().with_sandbox(sandbox);
    let session = SessionKey::ephemeral(format!("node-{name}"));
    CommandTable::with_bash(bash, session)
}

async fn run_session(
    url: &str,
    token: &str,
    name: &str,
    declared: &[CommandDescriptor],
    table: &CommandTable,
) -> Result<(), Box<dyn std::error::Error>> {
    let (mut ws, _resp) = tokio_tungstenite::connect_async(url).await?;
    let connect = json!({
        "jsonrpc": "2.0", "id": 1, "method": "connect",
        "params": { "token": token, "device_name": name, "commands": declared }
    });
    ws.send(Message::Text(connect.to_string().into())).await?;
    let _connect_resp = ws.next().await.ok_or("center closed before connect reply")??;
    tracing::info!("node '{name}' connected to center");

    while let Some(msg) = ws.next().await {
        let Message::Text(text) = msg? else { continue };
        if let Some(reply) = handle_frame(table, text.as_str()).await {
            ws.send(Message::Text(reply.into())).await?;
        }
    }
    Ok(())
}

/// 解析一帧；若是 `tool.call` 请求则 dispatch 并返回应答帧 JSON；否则 None。
async fn handle_frame(table: &CommandTable, text: &str) -> Option<String> {
    let v: Value = serde_json::from_str(text).ok()?;
    if v.get("method").and_then(|m| m.as_str()) != Some("tool.call") {
        return None;
    }
    let id = v.get("id").cloned().unwrap_or(Value::Null);
    let params = v.get("params").cloned().unwrap_or(Value::Null);
    let resp = match table.dispatch("tool.call", &params).await {
        Ok(result) => JsonRpcResponse::success(Some(id), result),
        Err(message) => JsonRpcResponse::error(Some(id), -32000, message),
    };
    Some(serde_json::to_string(&resp).ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alephcore::sandbox::{Sandbox, SandboxCommand, SandboxError, SandboxOutput};

    /// Inline canned-output sandbox. The library's `MockSandbox` lives under
    /// `#[cfg(test)]` in the alephcore crate, so it is invisible to this
    /// binary's separate test compilation — we stand up a tiny equivalent
    /// here to keep the frame tests meaningful without the real OS driver.
    struct CannedSandbox(SandboxOutput);

    #[async_trait::async_trait]
    impl Sandbox for CannedSandbox {
        async fn execute(&self, _cmd: SandboxCommand) -> Result<SandboxOutput, SandboxError> {
            Ok(self.0.clone())
        }
    }

    fn bash_table() -> CommandTable {
        let sandbox = Arc::new(CannedSandbox(SandboxOutput {
            stdout: b"hi\n".to_vec(),
            exit_code: Some(0),
            duration_ms: 1,
            ..Default::default()
        }));
        let bash = alephcore::builtin_tools::BashExecTool::new().with_sandbox(sandbox);
        CommandTable::with_bash(bash, SessionKey::ephemeral("node-frame-test"))
    }

    #[tokio::test]
    async fn handle_frame_dispatches_tool_call() {
        let table = bash_table();
        let frame = json!({
            "jsonrpc": "2.0", "id": "rpc-1", "method": "tool.call",
            "params": {"tool": "bash", "args": {"cmd": "echo hi"}}
        })
        .to_string();
        let reply = handle_frame(&table, &frame).await.expect("a reply");
        let v: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(v["id"], "rpc-1");
        assert!(v.get("result").is_some(), "success envelope: {v}");
    }

    #[tokio::test]
    async fn handle_frame_rejects_unlisted_tool_with_error() {
        let table = bash_table();
        let frame = json!({
            "jsonrpc": "2.0", "id": 7, "method": "tool.call",
            "params": {"tool": "rm", "args": {}}
        })
        .to_string();
        let reply = handle_frame(&table, &frame).await.expect("a reply");
        let v: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(v["id"], 7);
        assert!(v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("not permitted"));
    }

    #[tokio::test]
    async fn handle_frame_ignores_non_tool_call() {
        let table = bash_table();
        let frame = json!({"jsonrpc": "2.0", "id": 1, "result": {"ok": true}}).to_string();
        assert!(handle_frame(&table, &frame).await.is_none());
    }
}
