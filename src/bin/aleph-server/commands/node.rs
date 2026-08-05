//! `aleph-server node` — cluster node (execution arm) outbound runtime.
//!
//! Dials the hub WS, declares capabilities, serves `tool.call` in an inbound
//! loop, and runs bash in the local sandbox. Exponential-backoff reconnect on
//! disconnect. No DB / no harness / no LLM.
//!
//! LAN-trust model: the node holds no token. **Registration happens inside
//! `connect`** — on first boot, the node dials without a `device_id`; the hub
//! mints/merges a `node_id` in connect's `result.node` response and returns it;
//! the node persists it to disk. Every subsequent connect carries it, so the hub
//! stably keys all accounting under the same UUID (touch_device /
//! environments.list / deregister).
//!
//! Why registration must not be a separate RPC: the hub enforces that a
//! connection's FIRST frame must be `connect` (`gateway/server/handler.rs`),
//! and the login wall lets nothing through before `connect`. The old
//! implementation opened a second WS and sent `cluster.enroll` as the first
//! frame — it was always rejected and the connection killed — a fresh node
//! literally never completed registration. `connect` is the sole frame that
//! clears both gates simultaneously, so registration can only live on it.

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

/// `~/.aleph/node/<name>.json`. `None` only if the home dir is unresolvable
/// or if `name` contains path separators or traversal components.
fn identity_path(name: &str) -> Option<PathBuf> {
    if name.is_empty() || name == "." || name == ".." {
        return None;
    }
    let file_name = format!("{name}.json");
    if Path::new(&file_name)
        .components()
        .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return None;
    }
    // `ALEPH_HOME`-aware like every other Aleph state path — the node
    // enrolment record must live beside the config it belongs to.
    let base = alephcore::utils::paths::get_config_dir().ok()?.join("node");
    let candidate = base.join(&file_name);
    if candidate.strip_prefix(&base).is_ok() {
        Some(candidate)
    } else {
        None
    }
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

/// The center's verdict, carried in the `connect` reply under `result.node`.
enum ConnectVerdict {
    /// Registered. `node_id` is authoritative; persist it when `persist` is set
    /// (first boot, or the center adopted an operator's pre-enrolled row).
    Registered { node_id: String, persist: bool },
    /// The center revoked this node's device record (`cluster.deregister`).
    /// Terminal: retrying cannot help until an operator re-enrolls it.
    Deregistered,
}

/// Parse the center's `connect` reply. `None` = the center answered but said
/// nothing about nodes (an older center, or a rejected handshake).
fn parse_connect_verdict(resp: &Value) -> Option<ConnectVerdict> {
    let node = resp.pointer("/result/node")?;
    if node.get("status").and_then(|v| v.as_str()) == Some("deregistered") {
        return Some(ConnectVerdict::Deregistered);
    }
    let node_id = node.get("node_id").and_then(|v| v.as_str())?.to_string();
    let persist = node
        .get("persist")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    Some(ConnectVerdict::Registered { node_id, persist })
}

/// A session that lived at least this long counts as "healthy" — only then do we
/// reset the reconnect backoff. Without this, a center that accepts the socket
/// and immediately closes it (wrong origin, shutting down) would keep the node
/// hot-looping at `BACKOFF_INITIAL_MS` forever.
const HEALTHY_SESSION_MS: u128 = 30_000;

/// Deregistration is terminal, so it needs its own error identity to break the
/// reconnect loop.
#[derive(Debug)]
struct Deregistered;

impl std::fmt::Display for Deregistered {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("node was deregistered by the center")
    }
}

impl std::error::Error for Deregistered {}

pub async fn handle_node(
    center: String,
    name: String,
    tags: Vec<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    // The node is a standalone process: nothing else installs a tracing
    // subscriber for it (the `start` command sets one up, but this subcommand
    // returns long before that). Without this the node runs SILENT — no enroll
    // confirmation, no refusal, no reconnect diagnostics.
    init_node_tracing();

    let (table, approval_slot) = build_command_table(&name);
    let table = Arc::new(table);
    let declared = table.descriptors();
    let url = format!("{}/ws", center.trim_end_matches('/'));
    let id_path = identity_path(&name)
        .ok_or_else(|| format!("Invalid node name (path traversal not allowed): {name}"))?;

    // Persisted identity, if any. `None` on first boot — `connect` mints one.
    // Unlike the old flow this never blocks startup on the center being up: a
    // node booted before its center just backs off and retries like any other
    // disconnect, instead of exiting with an enroll error.
    let mut node_id = read_identity(&id_path).map(|id| {
        tracing::info!("node '{name}' using persisted identity {}", id.node_id);
        id.node_id
    });

    let mut backoff = BACKOFF_INITIAL_MS;
    loop {
        let started = std::time::Instant::now();

        // Persist the assigned id the MOMENT the center hands it back — not when
        // the session ends. A session lasts as long as the node is up, so a node
        // killed mid-session would otherwise never write its identity file and
        // would re-enroll on its next boot, minting exactly the duplicate device
        // row this whole flow exists to prevent.
        let persisted = std::sync::Mutex::new(node_id.clone());
        let on_identity = |id: &str, persist: bool| {
            let mut cur = persisted.lock().unwrap_or_else(|e| e.into_inner());
            if persist && cur.as_deref() != Some(id) {
                persist_identity(
                    Some(&id_path),
                    &NodeIdentity {
                        node_id: id.to_string(),
                        center: center.clone(),
                    },
                );
                tracing::info!("node '{name}' enrolled as {id}");
            }
            *cur = Some(id.to_string());
        };

        let outcome = run_session(
            &url,
            node_id.as_deref(),
            &name,
            &declared,
            &tags,
            &table,
            &approval_slot,
            &on_identity,
        )
        .await;

        // Carry whatever identity the session settled on into the next attempt.
        node_id = persisted
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        match outcome {
            Ok(()) => tracing::warn!("node session ended cleanly; reconnecting"),
            Err(e) if e.is::<Deregistered>() => {
                tracing::error!(
                    "node '{name}' was deregistered by the center. Its identity file \
                     ({}) is now dead — an operator must re-enroll this node (delete the \
                     file to join as a new node). Exiting.",
                    id_path.display()
                );
                return Err(e);
            }
            Err(e) => tracing::error!("node session error: {e}; retrying in {backoff}ms"),
        }

        // Only a session that actually stayed up earns a backoff reset.
        if started.elapsed().as_millis() >= HEALTHY_SESSION_MS {
            backoff = BACKOFF_INITIAL_MS;
        }
        tokio::time::sleep(Duration::from_millis(with_jitter(backoff))).await;
        backoff = (backoff * 2).min(BACKOFF_MAX_MS);
    }
}

/// Install a stderr tracing subscriber for the node process. `RUST_LOG` wins;
/// otherwise `info`, which is the level the enroll / refusal / reconnect lines
/// are emitted at.
fn init_node_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    // A second init (tests, embedding) is not an error worth aborting a node for.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

/// Spread reconnects over ±25% so a fleet does not stampede the center in
/// lockstep after it restarts. Uses the process-wide RNG already in the tree.
fn with_jitter(backoff_ms: u64) -> u64 {
    let spread = backoff_ms / 4;
    if spread == 0 {
        return backoff_ms;
    }
    let offset = rand::random_range(0..=spread * 2);
    backoff_ms.saturating_sub(spread).saturating_add(offset)
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
    let gate = Arc::new(ApprovalGate::new(Some(requester)));
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

/// Run one connected session.
///
/// `on_identity(node_id, persist)` fires as soon as the center's `connect` reply
/// lands — i.e. before the (arbitrarily long) read loop — so the node's identity
/// reaches disk immediately rather than only if the session terminates cleanly.
///
/// Errors carry [`Deregistered`] when the center refused this node outright;
/// the caller treats that as terminal rather than backing off forever.
async fn run_session(
    url: &str,
    node_id: Option<&str>,
    name: &str,
    declared: &[CommandDescriptor],
    tags: &[String],
    table: &Arc<CommandTable>,
    approval_slot: &ApprovalSlot,
    on_identity: &(dyn Fn(&str, bool) + Send + Sync),
) -> Result<(), Box<dyn std::error::Error>> {
    let (ws, _resp) = tokio_tungstenite::connect_async(url).await?;
    let (mut write, mut read) = ws.split();

    // LAN-trust connect. `commands` + `tags` are the node-detection signal (no
    // other client sends them). `device_id` is our persisted id — omitted on the
    // very first boot, in which case the center mints one and returns it below.
    // No token.
    let mut params = serde_json::Map::new();
    if let Some(id) = node_id {
        params.insert("device_id".into(), json!(id));
    }
    params.insert("device_name".into(), json!(name));
    params.insert("commands".into(), json!(declared));
    params.insert("tags".into(), json!(tags));
    // Version handshake (observation only — the center never refuses a skewed
    // node, see `cluster::maybe_register_node`). Without it a fleet member three
    // releases behind is indistinguishable from a current one, and "that node
    // behaves oddly" has no correlatable signal anywhere.
    params.insert("version".into(), json!(env!("ALEPH_VERSION")));
    let connect = json!({
        "jsonrpc": "2.0", "id": 1, "method": "connect",
        "params": Value::Object(params)
    });
    write
        .send(Message::Text(connect.to_string().into()))
        .await?;

    // Read the connect reply — this is where enrollment happens, so it must NOT
    // be discarded (the old code dropped it and never noticed a refusal).
    let reply = read
        .next()
        .await
        .ok_or("center closed before connect reply")??;
    let Message::Text(text) = reply else {
        return Err("unexpected non-text connect reply".into());
    };
    let reply: Value = serde_json::from_str(text.as_str())?;
    if let Some(err) = reply.get("error") {
        return Err(format!("center refused connect: {err}").into());
    }
    match parse_connect_verdict(&reply) {
        Some(ConnectVerdict::Deregistered) => return Err(Box::new(Deregistered)),
        Some(ConnectVerdict::Registered { node_id, persist }) => on_identity(&node_id, persist),
        // A center that says nothing about nodes accepted the socket but did not
        // register us — surface it instead of silently idling as a zombie.
        None => {
            return Err(format!("center did not register this node: {reply}").into());
        }
    }
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

/// Parse a frame; if it is a `tool.call` request, dispatch and return the
/// response frame JSON; otherwise `None`.
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
    fn connect_verdict_reads_minted_id_and_persist_flag() {
        // First boot: the center mints an id and tells us to persist it.
        let minted = json!({"result": {"node": {"node_id": "uuid-123", "status": "registered", "persist": true}}});
        match parse_connect_verdict(&minted).expect("a verdict") {
            ConnectVerdict::Registered { node_id, persist } => {
                assert_eq!(node_id, "uuid-123");
                assert!(persist);
            }
            ConnectVerdict::Deregistered => panic!("must be registered"),
        }

        // Reconnect: same id echoed back, nothing new to write to disk.
        let reconnect = json!({"result": {"node": {"node_id": "uuid-123", "status": "registered", "persist": false}}});
        match parse_connect_verdict(&reconnect).expect("a verdict") {
            ConnectVerdict::Registered { node_id, persist } => {
                assert_eq!(node_id, "uuid-123");
                assert!(
                    !persist,
                    "an unchanged id must not rewrite the identity file"
                );
            }
            ConnectVerdict::Deregistered => panic!("must be registered"),
        }
    }

    #[test]
    fn connect_verdict_detects_deregistration() {
        let kicked = json!({"result": {"node": {"node_id": "uuid-123", "status": "deregistered"}}});
        assert!(matches!(
            parse_connect_verdict(&kicked),
            Some(ConnectVerdict::Deregistered)
        ));
    }

    #[test]
    fn connect_verdict_absent_when_center_says_nothing_about_nodes() {
        // A plain Panel-style connect reply, or an error envelope: no node block
        // ⇒ the session errors out instead of idling as an unregistered zombie.
        assert!(parse_connect_verdict(&json!({"result": {"role": "operator"}})).is_none());
        assert!(parse_connect_verdict(&json!({"error": {"code": -32000}})).is_none());
    }

    #[test]
    fn jitter_stays_within_a_quarter_of_the_backoff() {
        for backoff in [BACKOFF_INITIAL_MS, 8_000, BACKOFF_MAX_MS] {
            let spread = backoff / 4;
            for _ in 0..64 {
                let j = with_jitter(backoff);
                assert!(
                    j >= backoff - spread && j <= backoff + spread,
                    "jitter {j} out of ±25% band around {backoff}"
                );
            }
        }
        // Degenerate backoffs must not panic on an empty range.
        assert_eq!(with_jitter(0), 0);
        assert_eq!(with_jitter(3), 3);
    }
}
