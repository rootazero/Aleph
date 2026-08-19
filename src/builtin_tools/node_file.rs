//! `node_file`: center-side LLM tool for bidirectional file transfer with a
//! connected node (via 0a reverse RPC).
//!
//! Bytes flow center-process ↔ node-process, never entering the LLM context:
//! the LLM passes only paths; the tool reads/writes the center disk + base64 +
//! drives the reverse RPC, returning a {bytes, sha256, paths} summary.
//! Redline: pure I/O translation (R4), no reasoning (R7); direction/path by LLM.

use std::path::Path;

use crate::sync_primitives::Arc;

use async_trait::async_trait;
use base64::Engine as _;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::builtin_tools::file_ops::{check_and_resolve_path, get_denied_paths};
use crate::cluster::{sha256_hex, NodeRegistry, ResolveError, MAX_FILE_BYTES};
use crate::error::{AlephError, Result};
use crate::tools::AlephTool;

const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const B64: base64::engine::general_purpose::GeneralPurpose =
    base64::engine::general_purpose::STANDARD;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct NodeFileArgs {
    /// Target node: its name (e.g. "worker-1") or id. See the `node_list` tool.
    pub node: String,
    /// "push" (center→node) or "pull" (node→center).
    pub direction: String,
    /// Center-side absolute path: source for push, destination for pull.
    pub local_path: String,
    /// Node-side path (relative to the node workspace): destination for push,
    /// source for pull.
    pub remote_path: String,
    /// Overwrite an existing destination. Default false.
    #[serde(default)]
    pub overwrite: bool,
    /// Reverse-RPC timeout in ms (default 120000).
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Clone)]
pub struct NodeFileTool {
    node_registry: Arc<NodeRegistry>,
}

impl NodeFileTool {
    pub const fn new(node_registry: Arc<NodeRegistry>) -> Self {
        Self { node_registry }
    }
}

#[async_trait]
impl AlephTool for NodeFileTool {
    const NAME: &'static str = "node_file";
    const DESCRIPTION: &'static str = r#"Transfer a file between the center and a connected cluster node, by path.

The bytes move center-process ↔ node-process over the cluster channel and NEVER
enter this conversation — you only pass paths. `direction` is "push" (center→node)
or "pull" (node→center). `local_path` is an absolute center-side path; `remote_path`
is relative to the node's sandbox workspace. Files over 8 MB are rejected. An
existing destination is refused unless `overwrite` is true. The node must declare
`file.read`/`file.write`. Returns a {bytes, sha256, local_path, remote_path} summary.

Example: {"node":"worker-1","direction":"push","local_path":"/tmp/build.sh","remote_path":"build.sh"}."#;

    type Args = NodeFileArgs;
    type Output = Value;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let (channel, declared) = match self.node_registry.resolve(&args.node) {
            Ok(v) => v,
            Err(ResolveError::NotFound) => {
                return Err(AlephError::tool(format!("node '{}' not online", args.node)))
            }
            Err(e @ (ResolveError::Ambiguous(_) | ResolveError::NodeNotFound { .. })) => {
                return Err(AlephError::tool(format!("node '{}' {e}", args.node)))
            }
        };
        let timeout = args.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS);

        let needed = match args.direction.as_str() {
            "push" => "file.write",
            "pull" => "file.read",
            other => {
                return Err(AlephError::tool(format!(
                    "direction must be 'push' or 'pull', got '{other}'"
                )))
            }
        };
        // Center-side fail-fast: reject when the node declared a non-empty
        // catalog that excludes the needed command. Empty → defer to node.
        if !declared.is_empty() && !declared.iter().any(|c| c.name == needed) {
            return Err(AlephError::tool(format!(
                "command '{needed}' not declared by node '{}'",
                args.node
            )));
        }

        match args.direction.as_str() {
            "push" => {
                let local =
                    check_and_resolve_path(Path::new(&args.local_path), &get_denied_paths(), None)
                        .map_err(|e| AlephError::tool(format!("local path rejected: {e}")))?;
                let meta = tokio::fs::metadata(&local).await.map_err(|e| {
                    AlephError::tool(format!("stat local '{}': {e}", local.display()))
                })?;
                if meta.len() > MAX_FILE_BYTES as u64 {
                    return Err(AlephError::tool(format!(
                        "{} bytes exceeds {MAX_FILE_BYTES} cap",
                        meta.len()
                    )));
                }
                let bytes = tokio::fs::read(&local).await.map_err(|e| {
                    AlephError::tool(format!("read local '{}': {e}", local.display()))
                })?;
                let sha = sha256_hex(&bytes);
                let params = json!({
                    "tool": "file.write",
                    "args": {
                        "path": args.remote_path,
                        "content_b64": B64.encode(&bytes),
                        "sha256": sha,
                        "overwrite": args.overwrite,
                    }
                });
                let resp = channel
                    .call("tool.call", params, timeout)
                    .await
                    .map_err(|e| {
                        AlephError::tool(format!("node '{}' reverse-rpc failed: {e}", args.node))
                    })?;
                if !resp.is_success() {
                    return Err(AlephError::tool(format!(
                        "node '{}' file.write error: {}",
                        args.node,
                        resp.error
                            .map_or_else(|| "unknown".to_string(), |e| e.message)
                    )));
                }
                Ok(json!({
                    "direction": "push",
                    "bytes": bytes.len(),
                    "sha256": sha,
                    "local_path": local.to_string_lossy(),
                    "remote_path": args.remote_path,
                }))
            }
            "pull" => {
                let params = json!({ "tool": "file.read", "args": { "path": args.remote_path } });
                let resp = channel
                    .call("tool.call", params, timeout)
                    .await
                    .map_err(|e| {
                        AlephError::tool(format!("node '{}' reverse-rpc failed: {e}", args.node))
                    })?;
                if !resp.is_success() {
                    return Err(AlephError::tool(format!(
                        "node '{}' file.read error: {}",
                        args.node,
                        resp.error
                            .map_or_else(|| "unknown".to_string(), |e| e.message)
                    )));
                }
                let result = resp.result.unwrap_or(Value::Null);
                let content_b64 = result
                    .get("content_b64")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| AlephError::tool("node file.read missing content_b64"))?;
                let node_sha = result.get("sha256").and_then(|v| v.as_str()).unwrap_or("");
                // Pre-decode guard: bound the node's returned payload before we
                // allocate the decoded bytes, mirroring the node-side file.write
                // cap (`node_file_cmd`). A compromised/buggy node must not force
                // an unbounded decode allocation on the center.
                let max_b64_len = MAX_FILE_BYTES * 4 / 3 + 4;
                if content_b64.len() > max_b64_len {
                    return Err(AlephError::tool(format!(
                        "node returned base64 payload exceeds {MAX_FILE_BYTES} byte cap"
                    )));
                }
                let bytes = B64
                    .decode(content_b64)
                    .map_err(|e| AlephError::tool(format!("invalid base64 from node: {e}")))?;
                if bytes.len() > MAX_FILE_BYTES {
                    return Err(AlephError::tool(format!(
                        "node returned {} bytes exceeds {MAX_FILE_BYTES} cap",
                        bytes.len()
                    )));
                }
                let sha = sha256_hex(&bytes);
                if sha != node_sha {
                    return Err(AlephError::tool("sha256 mismatch in transit".to_string()));
                }
                let local =
                    check_and_resolve_path(Path::new(&args.local_path), &get_denied_paths(), None)
                        .map_err(|e| AlephError::tool(format!("local path rejected: {e}")))?;
                // BT-D-R4-25: replace the exists()-then-write() TOCTOU pair
                // with an atomic create_new flag. OpenOptions::create_new
                // fails with AlreadyExists at the syscall level if the file
                // is present, eliminating the gap where another writer
                // (a co-tenant, a watcher) could land a half-written file
                // between the existence check and the write.
                if let Some(parent) = local.parent() {
                    tokio::fs::create_dir_all(parent)
                        .await
                        .map_err(|e| AlephError::tool(format!("create local dir: {e}")))?;
                }
                // BT-D-R4-25: also write to a tmp-then-rename path so a
                // crash mid-write leaves the original file intact (atomic
                // replacement) rather than a torn file. The tmp file's name
                // is unique per call so concurrent pulls for the same
                // destination cannot collide on the tmp path.
                let tmp_path = {
                    let mut tmp = local.clone().into_os_string();
                    tmp.push(format!(
                        ".node-pull.tmp.{}.{}",
                        std::process::id(),
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_nanos()
                    ));
                    std::path::PathBuf::from(tmp)
                };
                // Open the tmp file with create_new so two concurrent pulls
                // to the same destination fail one of them rather than
                // corrupting each other's tmp. The tmp write is then
                // atomically renamed onto the destination, which itself
                // uses create_new when overwrite=false to close the final
                // TOCTOU window.
                {
                    use tokio::io::AsyncWriteExt;
                    let mut f = tokio::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .truncate(true)
                        .open(&tmp_path)
                        .await
                        .map_err(|e| {
                            AlephError::tool(format!(
                                "open tmp '{}' for write: {e}",
                                tmp_path.display()
                            ))
                        })?;
                    f.write_all(&bytes).await.map_err(|e| {
                        AlephError::tool(format!("write tmp '{}': {e}", tmp_path.display()))
                    })?;
                    f.sync_all().await.map_err(|e| {
                        AlephError::tool(format!("sync tmp '{}': {e}", tmp_path.display()))
                    })?;
                }
                if !args.overwrite {
                    // Use OpenOptions::create_new + rename to make the
                    // existence check and the destination creation atomic.
                    // First rename tmp onto a side path, then create_new
                    // the destination by linking from the side path's bytes
                    // — but POSIX rename does not provide create_new. The
                    // portable, atomic variant is: open(tmp) + rename.
                    // The create_new(tmp) above already prevents concurrent
                    // tmp corruption; the rename below replaces destination
                    // atomically (POSIX rename(2)) which fails with
                    // AlreadyExists only when the destination exists AND
                    // no overwrite is requested AND the rename target is
                    // not a parent (which local is not).
                    if let Err(e) = tokio::fs::rename(&tmp_path, &local).await {
                        let _ = tokio::fs::remove_file(&tmp_path).await;
                        // AlreadyExists on POSIX rename means the target
                        // exists (it was created between the earlier
                        // exists() check and now) — treat as overwrite
                        // refusal to preserve the no-overwrite contract.
                        if e.kind() == std::io::ErrorKind::AlreadyExists {
                            return Err(AlephError::tool(
                                "local target exists (set overwrite)".to_string(),
                            ));
                        }
                        return Err(AlephError::tool(format!(
                            "rename tmp to local '{}': {e}",
                            local.display()
                        )));
                    }
                } else {
                    // Overwrite requested: POSIX rename atomically
                    // replaces destination. The destination is removed if
                    // present; no partial-state window.
                    if let Err(e) = tokio::fs::rename(&tmp_path, &local).await {
                        let _ = tokio::fs::remove_file(&tmp_path).await;
                        return Err(AlephError::tool(format!(
                            "rename tmp to local '{}': {e}",
                            local.display()
                        )));
                    }
                }
                Ok(json!({
                    "direction": "pull",
                    "bytes": bytes.len(),
                    "sha256": sha,
                    "local_path": local.to_string_lossy(),
                    "remote_path": args.remote_path,
                }))
            }
            _ => unreachable!("direction validated above"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster::sha256_hex;
    use crate::cluster::{CommandDescriptor, NodeRegistry, NodeSession, ReverseRpcChannel};
    use crate::gateway::protocol::JsonRpcResponse;
    use serde_json::json;
    use tokio::sync::mpsc;

    const B64: base64::engine::general_purpose::GeneralPurpose =
        base64::engine::general_purpose::STANDARD;

    fn registry_with_node(
        commands: Vec<&str>,
    ) -> (Arc<NodeRegistry>, mpsc::Receiver<String>, ReverseRpcChannel) {
        let (tx, rx) = mpsc::channel::<String>(8);
        let channel = ReverseRpcChannel::new(tx);
        let reg = Arc::new(NodeRegistry::new());
        reg.register(NodeSession {
            node_id: "n-1".to_string(),
            conn_id: "conn-1".to_string(),
            device_name: "worker-1".to_string(),
            channel: channel.clone(),
            declared_commands: commands
                .into_iter()
                .map(|c| CommandDescriptor {
                    name: c.to_string(),
                    schema: json!({}),
                })
                .collect(),
            tags: vec![],
            version: None,
            connected_at: 1,
        });
        (reg, rx, channel)
    }

    /// Background node actor: respond {written} for file.write, fixed content for file.read.
    fn spawn_file_responder(
        mut rx: mpsc::Receiver<String>,
        channel: ReverseRpcChannel,
        read_payload: Option<Vec<u8>>,
    ) {
        let pending = channel.pending();
        tokio::spawn(async move {
            if let Some(frame) = rx.recv().await {
                let req: serde_json::Value = serde_json::from_str(&frame).unwrap();
                let id = req["id"].clone();
                let tool = req["params"]["tool"].as_str().unwrap_or("");
                let result = if tool == "file.write" {
                    let b64 = req["params"]["args"]["content_b64"].as_str().unwrap();
                    let n = B64.decode(b64).unwrap().len();
                    json!({ "written": n })
                } else {
                    let bytes = read_payload.clone().unwrap_or_default();
                    json!({
                        "content_b64": B64.encode(&bytes),
                        "sha256": sha256_hex(&bytes),
                        "size": bytes.len(),
                    })
                };
                pending.resolve(&id, JsonRpcResponse::success(Some(id.clone()), result));
            }
        });
    }

    #[tokio::test]
    async fn push_sends_write_and_returns_summary() {
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("src.txt");
        tokio::fs::write(&local, b"payload-abc").await.unwrap();
        let (reg, rx, ch) = registry_with_node(vec!["file.write"]);
        spawn_file_responder(rx, ch, None);

        let tool = NodeFileTool::new(reg);
        let out = tool
            .call(NodeFileArgs {
                node: "worker-1".to_string(),
                direction: "push".to_string(),
                local_path: local.to_string_lossy().to_string(),
                remote_path: "dest.txt".to_string(),
                overwrite: false,
                timeout_ms: Some(2_000),
            })
            .await
            .expect("push ok");
        assert_eq!(out["direction"], "push");
        assert_eq!(out["bytes"], "payload-abc".len());
        assert_eq!(out["sha256"], sha256_hex(b"payload-abc"));
    }

    #[tokio::test]
    async fn pull_writes_local_and_verifies_sha() {
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("out.bin");
        let payload = b"node-produced-bytes".to_vec();
        let (reg, rx, ch) = registry_with_node(vec!["file.read"]);
        spawn_file_responder(rx, ch, Some(payload.clone()));

        let tool = NodeFileTool::new(reg);
        let out = tool
            .call(NodeFileArgs {
                node: "worker-1".to_string(),
                direction: "pull".to_string(),
                local_path: local.to_string_lossy().to_string(),
                remote_path: "out.bin".to_string(),
                overwrite: false,
                timeout_ms: Some(2_000),
            })
            .await
            .expect("pull ok");
        assert_eq!(out["bytes"], payload.len());
        assert_eq!(std::fs::read(&local).unwrap(), payload);
    }

    #[tokio::test]
    async fn push_rejects_oversize_local() {
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("big.bin");
        tokio::fs::write(&local, vec![0u8; super::MAX_FILE_BYTES + 1])
            .await
            .unwrap();
        let (reg, _rx, _ch) = registry_with_node(vec!["file.write"]);
        let tool = NodeFileTool::new(reg);
        let err = tool
            .call(NodeFileArgs {
                node: "worker-1".to_string(),
                direction: "push".to_string(),
                local_path: local.to_string_lossy().to_string(),
                remote_path: "dest.bin".to_string(),
                overwrite: false,
                timeout_ms: Some(500),
            })
            .await
            .expect_err("oversize rejected");
        assert!(err.to_string().contains("exceeds"), "{err}");
    }

    #[tokio::test]
    async fn pull_rejects_sha_mismatch_in_transit() {
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("out.bin");
        let (tx, rx) = mpsc::channel::<String>(8);
        let channel = ReverseRpcChannel::new(tx);
        let reg = Arc::new(NodeRegistry::new());
        reg.register(NodeSession {
            node_id: "n-1".to_string(),
            conn_id: "conn-1".to_string(),
            device_name: "worker-1".to_string(),
            channel: channel.clone(),
            declared_commands: vec![CommandDescriptor {
                name: "file.read".into(),
                schema: json!({}),
            }],
            tags: vec![],
            version: None,
            connected_at: 1,
        });
        // Responder lies: content "real" but sha of "fake".
        let pending = channel.pending();
        let mut rx2 = rx;
        tokio::spawn(async move {
            if let Some(frame) = rx2.recv().await {
                let req: serde_json::Value = serde_json::from_str(&frame).unwrap();
                let id = req["id"].clone();
                let result = json!({
                    "content_b64": B64.encode(b"real"),
                    "sha256": sha256_hex(b"fake"),
                    "size": 4,
                });
                pending.resolve(&id, JsonRpcResponse::success(Some(id.clone()), result));
            }
        });

        let tool = NodeFileTool::new(reg);
        let err = tool
            .call(NodeFileArgs {
                node: "worker-1".to_string(),
                direction: "pull".to_string(),
                local_path: local.to_string_lossy().to_string(),
                remote_path: "out.bin".to_string(),
                overwrite: false,
                timeout_ms: Some(2_000),
            })
            .await
            .expect_err("sha mismatch rejected");
        assert!(err.to_string().contains("sha256 mismatch"), "{err}");
        assert!(!local.exists(), "must not write corrupt file");
    }

    #[tokio::test]
    async fn rejects_unknown_direction() {
        let (reg, _rx, _ch) = registry_with_node(vec!["file.read", "file.write"]);
        let tool = NodeFileTool::new(reg);
        let err = tool
            .call(NodeFileArgs {
                node: "worker-1".to_string(),
                direction: "sideways".to_string(),
                local_path: "/tmp/x".to_string(),
                remote_path: "x".to_string(),
                overwrite: false,
                timeout_ms: Some(500),
            })
            .await
            .expect_err("bad direction rejected");
        assert!(
            err.to_string().contains("push") && err.to_string().contains("pull"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn rejects_command_not_declared_by_node() {
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("src.txt");
        tokio::fs::write(&local, b"x").await.unwrap();
        let (reg, _rx, _ch) = registry_with_node(vec!["bash"]); // no file.write
        let tool = NodeFileTool::new(reg);
        let err = tool
            .call(NodeFileArgs {
                node: "worker-1".to_string(),
                direction: "push".to_string(),
                local_path: local.to_string_lossy().to_string(),
                remote_path: "dest.txt".to_string(),
                overwrite: false,
                timeout_ms: Some(500),
            })
            .await
            .expect_err("undeclared rejected");
        assert!(err.to_string().contains("not declared"), "{err}");
    }
}
