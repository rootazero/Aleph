//! `aleph-server node` —— 集群节点（执行臂）拨出运行时。
//!
//! 拨向中心 WS、声明命令、入站循环服务 `tool.call`，在本机 sandbox 跑 bash。
//! 断线指数退避重连。无 DB / 无 harness / 无 LLM。
//!
//! LAN-trust 模型：节点不再持有 token。首启时调 `cluster.enroll` 在中心登记
//! 一条设备记录，拿回 `node_id`（UUID）并落盘；之后每次 `connect` 用该
//! `node_id` 作 `device_id` 声明身份（连同 `commands` + `tags` 形状）。中心
//! 据此把节点稳定地键入同一 UUID（修正 touch_device / environments.list /
//! deregister 的记账漂移）。无 token、无配对码、无 AUTH_FAILED。

use alephcore::sync_primitives::RwLock;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use alephcore::cluster::{
    ApprovalSlot, CenterApprovalRequester, CommandDescriptor, CommandTable, ReverseRpcChannel,
};
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

/// Persisted node identity. LAN-trust: the only credential is the enrolled
/// `node_id` (a center-minted UUID); it is sent verbatim as the `connect`
/// `device_id` so the center keys this node's bookkeeping under one stable id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct NodeIdentity {
    node_id: String,
    center: String,
}

/// `~/.aleph/node/<name>.json`. `None` only if the home dir is unresolvable.
fn identity_path(name: &str) -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".aleph").join("node").join(format!("{name}.json")))
}

/// Migration note: old pre-LAN-trust `NodeCredential` files (which carry an
/// extra `bearer` field) deserialize here successfully too — `NodeIdentity`
/// has no `deny_unknown_fields`, so serde silently drops the dead `bearer`
/// and the upgraded node KEEPS its enrolled `node_id` instead of
/// re-enrolling. That is deliberate: the center's bookkeeping (touch_device,
/// environments.list, deregister) stays keyed to the same UUID across the
/// upgrade. Do not "fix" this into a forced re-enroll.
fn read_identity(path: &Path) -> Option<NodeIdentity> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn write_identity(path: &Path, id: &NodeIdentity) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_vec_pretty(id).map_err(std::io::Error::other)?;
    std::fs::write(path, json)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        if let Err(e) = std::fs::set_permissions(path, perms) {
            tracing::warn!("failed to restrict node identity permissions: {e}");
        }
    }
    Ok(())
}

/// Parse a `cluster.enroll` reply into the minted `node_id`. Pure.
fn parse_enroll_node_id(resp: &Value) -> Option<String> {
    resp.pointer("/result/node_id")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

pub async fn handle_node(
    center: String,
    name: String,
    tags: Vec<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let (table, approval_slot) = build_command_table(&name);
    let table = Arc::new(table);
    let declared = table.descriptors();
    let url = format!("{}/ws", center.trim_end_matches('/'));
    let id_path = identity_path(&name);

    // Resolve the node identity: persisted enrollment > enroll now.
    let node_id = match id_path.as_deref().and_then(read_identity) {
        Some(id) => {
            tracing::info!("node '{name}' using persisted identity {}", id.node_id);
            id.node_id
        }
        None => {
            let id = enroll_node(&url, &center, &name).await?;
            persist_identity(id_path.as_deref(), &id);
            id.node_id
        }
    };

    let mut backoff = BACKOFF_INITIAL_MS;
    loop {
        match run_session(
            &url,
            &node_id,
            &name,
            &declared,
            &tags,
            &table,
            &approval_slot,
        )
        .await
        {
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

/// Best-effort persist; a write failure warns but does not abort the node
/// (it will re-enroll on next restart).
fn persist_identity(path: Option<&Path>, id: &NodeIdentity) {
    if let Some(p) = path {
        if let Err(e) = write_identity(p, id) {
            tracing::warn!("failed to persist node identity: {e}");
        }
    }
}

/// Build the node sandbox + command table. The `ApprovalGate` is wired to a
/// `CenterApprovalRequester` whose channel slot is filled per-connection by
/// `run_session`; until a connection is live the slot is `None` and escalations
/// fail-closed (same as the old headless `None` requester).
fn build_command_table(name: &str) -> (CommandTable, ApprovalSlot) {
    let cfg = SandboxConfig::default();
    let driver = create_platform_driver_from_config(&cfg);
    let slot: ApprovalSlot = Arc::new(RwLock::new(None));
    let requester = Arc::new(CenterApprovalRequester::new(slot.clone()));
    let gate = Arc::new(ApprovalGate::new(
        ApprovalConfig::default(),
        Some(requester),
    ));
    let sandbox = build_sandbox(
        &cfg,
        driver,
        gate,
        SandboxRateLimitConfig::default(),
        &alephcore::ShellSecurityConfig::default(),
    );
    let bash = alephcore::builtin_tools::BashExecTool::new().with_sandbox(sandbox);
    let session = SessionKey::ephemeral(format!("node-{name}"));
    // file.read/file.write jail to the SAME per-session workspace dir bash uses,
    // so a pushed script can be bash-run and a bash-produced artifact pulled.
    let workspace_dir =
        alephcore::sandbox::workspace::session_workspace_dir(&cfg.workspace_root, &session);
    let mut table = CommandTable::with_bash(bash, session);
    table.register_file_commands(workspace_dir);
    (table, slot)
}

/// Register this node with the center via `cluster.enroll` and return the
/// minted identity. LAN-trust: no token is minted — enroll only records a
/// device entry and hands back the `node_id` (UUID) used for connect keying.
/// No retry/backoff here — the caller's reconnect loop owns that.
async fn enroll_node(
    url: &str,
    center: &str,
    name: &str,
) -> Result<NodeIdentity, Box<dyn std::error::Error>> {
    let (mut ws, _resp) = tokio_tungstenite::connect_async(url).await?;

    let enroll = json!({
        "jsonrpc": "2.0", "id": 1, "method": "cluster.enroll",
        "params": { "node_name": name }
    });
    ws.send(Message::Text(enroll.to_string().into())).await?;
    let reply = ws
        .next()
        .await
        .ok_or("center closed before enroll reply")??;
    let Message::Text(text) = reply else {
        return Err("unexpected non-text enroll reply".into());
    };
    let val: Value = serde_json::from_str(text.as_str())?;
    let node_id = parse_enroll_node_id(&val)
        .ok_or_else(|| format!("center did not return a node_id: {val}"))?;
    tracing::info!("node '{name}' enrolled as {node_id}");
    Ok(NodeIdentity {
        node_id,
        center: center.to_string(),
    })
}

async fn run_session(
    url: &str,
    node_id: &str,
    name: &str,
    declared: &[CommandDescriptor],
    tags: &[String],
    table: &Arc<CommandTable>,
    approval_slot: &ApprovalSlot,
) -> Result<(), Box<dyn std::error::Error>> {
    let (ws, _resp) = tokio_tungstenite::connect_async(url).await?;
    let (mut write, mut read) = ws.split();

    // LAN-trust connect: `device_id` is the enrolled UUID so the center keys
    // this node's records stably; `commands` + `tags` are the node-detection
    // signal (no other client sends them). No token.
    let connect = json!({
        "jsonrpc": "2.0", "id": 1, "method": "connect",
        "params": {
            "device_id": node_id,
            "device_name": name,
            "commands": declared,
            "tags": tags
        }
    });
    write
        .send(Message::Text(connect.to_string().into()))
        .await?;
    // Drain the connect reply (LAN-trust always succeeds; no auth to check).
    let _reply = read
        .next()
        .await
        .ok_or("center closed before connect reply")??;
    tracing::info!("node '{name}' connected to center");

    // Bidirectional channel for this connection: outbound mpsc drained by a
    // writer task; a node-side PendingInvokes resolves center responses. This
    // is what lets a blocked bash command (awaiting approval) NOT deadlock the
    // read loop — dispatch runs on spawned tasks while the read loop keeps
    // pumping frames.
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<String>(64);
    let channel = ReverseRpcChannel::new(out_tx.clone());
    let pending = channel.pending();
    *approval_slot
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(channel);

    let writer = tokio::spawn(async move {
        while let Some(frame) = out_rx.recv().await {
            if write.send(Message::Text(frame.into())).await.is_err() {
                break;
            }
        }
    });

    // Run the read loop in an inner block so the cleanup below (fail-close the
    // approval slot + stop the writer) runs on EVERY exit path — including a WS
    // error early-return via `?`, not just the normal loop fall-through.
    let loop_result: Result<(), Box<dyn std::error::Error>> = async {
        while let Some(msg) = read.next().await {
            let Message::Text(text) = msg? else { continue };
            let text = text.to_string();

            // Center → node RESPONSE: resolve a node-initiated reverse-RPC call.
            if let Ok(resp) = serde_json::from_str::<JsonRpcResponse>(&text) {
                if resp.id.is_some() && (resp.result.is_some() || resp.error.is_some()) {
                    if let Some(id) = resp.id.clone() {
                        pending.resolve(&id, resp);
                    }
                    continue;
                }
            }

            // Center → node REQUEST (tool.call): dispatch on a spawned task so a
            // long-running command (awaiting approval) does not block this loop.
            let table = Arc::clone(table);
            let out = out_tx.clone();
            tokio::spawn(async move {
                if let Some(reply) = handle_frame(&table, &text).await {
                    let _ = out.send(reply).await;
                }
            });
        }
        Ok(())
    }
    .await;

    // Cleanup on every exit path: fail-close the approval slot + stop the writer.
    *approval_slot
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    writer.abort();
    loop_result?;
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
    serde_json::to_string(&resp).ok()
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
    fn node_identity_round_trips_through_disk() {
        let id = NodeIdentity {
            node_id: "n-1".into(),
            center: "ws://c".into(),
        };
        let path = std::env::temp_dir().join("aleph-node-identity-roundtrip-test.json");
        write_identity(&path, &id).unwrap();
        let loaded = read_identity(&path).expect("reads back");
        assert_eq!(loaded, id);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn legacy_node_credential_file_parses_and_keeps_node_id() {
        // Pre-LAN-trust `NodeCredential` files carry an extra `bearer` field.
        // serde must drop it and keep the enrolled node_id so an upgraded
        // node does NOT re-enroll (see the `read_identity` migration note).
        let path = std::env::temp_dir().join("aleph-node-legacy-cred-migration-test.json");
        std::fs::write(
            &path,
            r#"{"node_id":"n-legacy","bearer":"tok:sig","center":"ws://c"}"#,
        )
        .unwrap();
        let loaded = read_identity(&path).expect("legacy credential file still parses");
        assert_eq!(loaded.node_id, "n-legacy");
        assert_eq!(loaded.center, "ws://c");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn parse_enroll_node_id_reads_minted_id() {
        let ok = json!({"result": {"node_id": "uuid-123"}});
        assert_eq!(parse_enroll_node_id(&ok).as_deref(), Some("uuid-123"));
        // Missing / malformed replies yield None (caller errors out).
        assert!(parse_enroll_node_id(&json!({"result": {}})).is_none());
        assert!(parse_enroll_node_id(&json!({"error": {"code": -32000}})).is_none());
    }
}
