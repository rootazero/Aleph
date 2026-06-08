//! `aleph-server node` —— 集群节点（执行臂）拨出运行时。
//!
//! 拨向中心 WS、用 node-token 认证、声明命令、入站循环服务 `tool.call`，
//! 在本机 sandbox 跑 bash。断线指数退避重连。无 DB / 无 harness / 无 LLM。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use alephcore::cluster::{CommandDescriptor, CommandTable};
use alephcore::gateway::protocol::JsonRpcResponse;
use alephcore::routing::session_key::SessionKey;
use alephcore::sandbox::config::SandboxConfig;
use alephcore::sandbox::exec_approval::gate::ApprovalGate;
use alephcore::sandbox::exec_approval::types::ApprovalConfig;
use alephcore::sandbox::factory::build_sandbox;
use alephcore::sandbox::platforms::create_platform_driver_from_config;
use alephcore::sandbox::rate_limit::SandboxRateLimitConfig;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

const BACKOFF_INITIAL_MS: u64 = 2_000;
const BACKOFF_MAX_MS: u64 = 60_000;
const POLL_INTERVAL_MS: u64 = 2_000;
const AUTH_FAILED_CODE: i64 = -32001;

/// Persisted node credential. `bearer` is the combined "{token}:{signature}"
/// string the center's `pairing.poll` hands out — sent verbatim as the
/// `connect` `token` param.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct NodeCredential {
    node_id: String,
    bearer: String,
    center: String,
}

/// `~/.aleph/node/<name>.json`. `None` only if the home dir is unresolvable.
fn credential_path(name: &str) -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".aleph").join("node").join(format!("{name}.json")))
}

fn read_credential(path: &Path) -> Option<NodeCredential> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn write_credential(path: &Path, cred: &NodeCredential) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_vec_pretty(cred).map_err(std::io::Error::other)?;
    std::fs::write(path, json)
}

/// Outcome of parsing a `pairing.poll` reply. Pure.
#[derive(Debug)]
enum PairingOutcome {
    Pending,
    Approved { bearer: String, node_id: String },
    Rejected,
    Expired,
}

fn parse_pairing_outcome(resp: &Value) -> PairingOutcome {
    match resp.pointer("/result/status").and_then(|s| s.as_str()) {
        Some("approved") => PairingOutcome::Approved {
            bearer: resp
                .pointer("/result/token")
                .and_then(|t| t.as_str())
                .unwrap_or_default()
                .to_string(),
            node_id: resp
                .pointer("/result/device_id")
                .and_then(|d| d.as_str())
                .unwrap_or_default()
                .to_string(),
        },
        Some("rejected") => PairingOutcome::Rejected,
        Some("expired") => PairingOutcome::Expired,
        _ => PairingOutcome::Pending,
    }
}

/// Result of one `run_session` attempt.
enum SessionOutcome {
    /// Connected and the inbound loop ended (clean reconnect).
    Ended,
    /// Center rejected `connect` with AUTH_FAILED — credential is stale.
    AuthFailed,
}

/// True when a `connect` reply is an AUTH_FAILED (-32001) error. Pure.
fn connect_rejected_auth(connect_resp: &Value) -> bool {
    connect_resp.pointer("/error/code").and_then(|c| c.as_i64()) == Some(AUTH_FAILED_CODE)
}

pub async fn handle_node(
    center: String,
    token: Option<String>,
    name: String,
) -> Result<(), Box<dyn std::error::Error>> {
    let table = Arc::new(build_command_table(&name));
    let declared = table.descriptors();
    let url = format!("{}/ws", center.trim_end_matches('/'));
    let cred_path = credential_path(&name);

    // Resolve the bearer: persisted credential > --token > interactive pairing.
    let mut bearer = match cred_path.as_deref().and_then(read_credential) {
        Some(cred) => {
            tracing::info!("node '{name}' using persisted credential");
            cred.bearer
        }
        None => match token {
            Some(t) => t,
            None => {
                let cred = run_pairing(&url, &center, &name, &declared).await?;
                persist_credential(cred_path.as_deref(), &cred);
                cred.bearer
            }
        },
    };

    let mut backoff = BACKOFF_INITIAL_MS;
    loop {
        match run_session(&url, &bearer, &name, &declared, &table).await {
            Ok(SessionOutcome::Ended) => {
                tracing::warn!("node session ended cleanly; reconnecting");
                backoff = BACKOFF_INITIAL_MS;
            }
            Ok(SessionOutcome::AuthFailed) => {
                tracing::warn!("node credential rejected by center; clearing and re-pairing");
                if let Some(p) = cred_path.as_deref() {
                    let _ = std::fs::remove_file(p);
                }
                let cred = run_pairing(&url, &center, &name, &declared).await?;
                persist_credential(cred_path.as_deref(), &cred);
                bearer = cred.bearer;
                backoff = BACKOFF_INITIAL_MS;
                continue;
            }
            Err(e) => tracing::error!("node session error: {e}; retrying in {backoff}ms"),
        }
        tokio::time::sleep(Duration::from_millis(backoff)).await;
        backoff = (backoff * 2).min(BACKOFF_MAX_MS);
    }
}

/// Best-effort persist; a write failure warns but does not abort the node
/// (it will re-pair on next restart).
fn persist_credential(path: Option<&Path>, cred: &NodeCredential) {
    if let Some(p) = path {
        if let Err(e) = write_credential(p, cred) {
            tracing::warn!("failed to persist node credential: {e}");
        }
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

/// Interactive pairing: anonymous WS → `pairing.start_node` → print the code →
/// poll `pairing.poll` every 2s until the operator approves from the Panel.
/// Returns the minted node credential. No retry/backoff here — the caller's
/// reconnect loop owns that; pairing is linear and readable.
async fn run_pairing(
    url: &str,
    center: &str,
    name: &str,
    declared: &[CommandDescriptor],
) -> Result<NodeCredential, Box<dyn std::error::Error>> {
    let (mut ws, _resp) = tokio_tungstenite::connect_async(url).await?;

    let start = json!({
        "jsonrpc": "2.0", "id": 1, "method": "pairing.start_node",
        "params": { "node_name": name, "commands": declared }
    });
    ws.send(Message::Text(start.to_string().into())).await?;
    let start_reply = ws
        .next()
        .await
        .ok_or("center closed before start_node reply")??;
    let Message::Text(start_text) = start_reply else {
        return Err("unexpected non-text start_node reply".into());
    };
    let start_val: Value = serde_json::from_str(start_text.as_str())?;
    let code = start_val
        .pointer("/result/code")
        .and_then(|c| c.as_str())
        .ok_or("center did not return a pairing code")?
        .to_string();

    println!("\n  Aleph 节点配对码: {code}");
    println!("  请在中心 Panel 通知卡批准此节点\n");

    let mut poll_id = 2;
    loop {
        let poll = json!({
            "jsonrpc": "2.0", "id": poll_id, "method": "pairing.poll",
            "params": { "code": code }
        });
        ws.send(Message::Text(poll.to_string().into())).await?;
        let poll_reply = ws.next().await.ok_or("center closed during poll")??;
        if let Message::Text(poll_text) = poll_reply {
            let poll_val: Value = serde_json::from_str(poll_text.as_str())?;
            match parse_pairing_outcome(&poll_val) {
                PairingOutcome::Approved { bearer, node_id } => {
                    tracing::info!("node '{name}' pairing approved");
                    return Ok(NodeCredential {
                        node_id,
                        bearer,
                        center: center.to_string(),
                    });
                }
                PairingOutcome::Rejected => return Err("pairing rejected by operator".into()),
                PairingOutcome::Expired => return Err("pairing code expired".into()),
                PairingOutcome::Pending => {}
            }
        }
        poll_id += 1;
        tokio::time::sleep(Duration::from_millis(POLL_INTERVAL_MS)).await;
    }
}

async fn run_session(
    url: &str,
    token: &str,
    name: &str,
    declared: &[CommandDescriptor],
    table: &CommandTable,
) -> Result<SessionOutcome, Box<dyn std::error::Error>> {
    let (mut ws, _resp) = tokio_tungstenite::connect_async(url).await?;
    let connect = json!({
        "jsonrpc": "2.0", "id": 1, "method": "connect",
        "params": { "token": token, "device_name": name, "commands": declared }
    });
    ws.send(Message::Text(connect.to_string().into())).await?;
    let reply = ws
        .next()
        .await
        .ok_or("center closed before connect reply")??;
    if let Message::Text(text) = &reply {
        if let Ok(v) = serde_json::from_str::<Value>(text.as_str()) {
            if connect_rejected_auth(&v) {
                tracing::warn!("node '{name}' rejected by center (auth failed)");
                return Ok(SessionOutcome::AuthFailed);
            }
        }
    }
    tracing::info!("node '{name}' connected to center");

    while let Some(msg) = ws.next().await {
        let Message::Text(text) = msg? else { continue };
        if let Some(reply) = handle_frame(table, text.as_str()).await {
            ws.send(Message::Text(reply.into())).await?;
        }
    }
    Ok(SessionOutcome::Ended)
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

    #[test]
    fn node_credential_round_trips_through_disk() {
        let cred = NodeCredential {
            node_id: "n-1".into(),
            bearer: "tok:sig".into(),
            center: "ws://c".into(),
        };
        let path = std::env::temp_dir().join("aleph-node-cred-roundtrip-test.json");
        write_credential(&path, &cred).unwrap();
        let loaded = read_credential(&path).expect("reads back");
        assert_eq!(loaded, cred);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn parse_pairing_outcome_reads_each_status() {
        let approved = json!({"result":{"status":"approved","token":"t:s","device_id":"d"}});
        match parse_pairing_outcome(&approved) {
            PairingOutcome::Approved { bearer, node_id } => {
                assert_eq!(bearer, "t:s");
                assert_eq!(node_id, "d");
            }
            other => panic!("expected Approved, got {other:?}"),
        }
        assert!(matches!(
            parse_pairing_outcome(&json!({"result":{"status":"rejected"}})),
            PairingOutcome::Rejected
        ));
        assert!(matches!(
            parse_pairing_outcome(&json!({"result":{"status":"expired"}})),
            PairingOutcome::Expired
        ));
        assert!(matches!(
            parse_pairing_outcome(&json!({"result":{"status":"pending"}})),
            PairingOutcome::Pending
        ));
    }

    #[test]
    fn connect_rejected_auth_detects_auth_failed() {
        assert!(connect_rejected_auth(
            &json!({"error":{"code":-32001,"message":"x"}})
        ));
        assert!(!connect_rejected_auth(&json!({"result":{"ok":true}})));
        assert!(!connect_rejected_auth(
            &json!({"error":{"code":-32000,"message":"transient"}})
        ));
    }
}
