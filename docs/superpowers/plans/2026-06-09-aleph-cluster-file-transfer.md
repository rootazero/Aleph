# Cluster File Transfer (node ↔ center) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let the center push/pull files to/from a connected cluster node over the 0a reverse-RPC channel, with bytes flowing center-process ↔ node-process and never through the LLM context.

**Architecture:** A center-side `node_file` LLM tool orchestrates push/pull by path; node-side `file.read`/`file.write` `NodeCommand`s do direct host-fs I/O, jailed to the node session workspace. Single-frame, hard 8 MB cap, sha256 integrity both ends, fail-fast.

**Tech Stack:** Rust (alephcore), async-trait, serde_json, base64 0.22, sha2 0.10, hex 0.4, `file_ops::path_utils`, 0a `ReverseRpcChannel`.

**Spec:** `docs/superpowers/specs/2026-06-09-aleph-cluster-file-transfer.md`

**Worktree:** Create a NEW worktree from `main` (main already contains 0a+0b+0c-core+0c-pairing and this spec). Do NOT merge to main — the user manages the cluster merge strategy. Do NOT touch `src/harness/` (R10).

---

## File Structure

| File | Responsibility | Change |
|------|----------------|--------|
| `src/builtin_tools/file_ops/mod.rs` | Re-export `check_and_resolve_path` + `get_denied_paths` to crate | Modify |
| `src/sandbox/workspace.rs` | Add `pub fn session_workspace_dir(root, sid)` helper | Modify |
| `src/cluster/node_file_cmd.rs` | Node-side `FileReadCommand`/`FileWriteCommand` + jail + sha256 + 8 MB cap | **Create** |
| `src/cluster/node_runtime.rs` | `CommandTable::register_file_commands` | Modify |
| `src/cluster/mod.rs` | `pub mod node_file_cmd;` + re-exports | Modify |
| `src/builtin_tools/node_file.rs` | Center-side `node_file` LLM tool | **Create** |
| `src/builtin_tools/mod.rs` | `pub mod node_file;` + re-export | Modify |
| `src/bin/aleph-server/commands/node.rs` | `build_command_table` registers file commands | Modify |
| `src/executor/builtin_registry/definitions.rs` | `node_file` definition (2 sites) | Modify |
| `src/executor/builtin_registry/registry.rs` | `node_file` dispatch arm (reuses node_registry cell) | Modify |
| `src/executor/builtin_registry/groups.rs` | add `node_file` to `cluster` group | Modify |
| `src/executor/builtin_registry/builder/optional_tools.rs` | register `node_file` description + schema | Modify |
| `tests/cluster_node_runtime.rs` | integration: `file.write`→`file.read` byte-identical round-trip | Modify |

> **Note:** `node_file` reuses the **already-injected** `node_registry` `OnceCell` (the same one `node_invoke` uses, injected in `agent_init/mod.rs:762`). No `agent_init` change is needed.

---

## Task 1: Node-side file commands

**Files:**
- Modify: `src/builtin_tools/file_ops/mod.rs` (re-export path helpers)
- Modify: `src/sandbox/workspace.rs` (add `session_workspace_dir`)
- Create: `src/cluster/node_file_cmd.rs`
- Modify: `src/cluster/node_runtime.rs` (`register_file_commands`)
- Modify: `src/cluster/mod.rs` (module + re-exports)
- Test: inline `#[cfg(test)]` in `src/cluster/node_file_cmd.rs`

- [ ] **Step 1: Expose path helpers to the crate**

In `src/builtin_tools/file_ops/mod.rs`, change the private module to expose its two helpers crate-wide. Find:

```rust
mod path_utils;
```

Replace with:

```rust
mod path_utils;
pub(crate) use path_utils::{check_and_resolve_path, get_denied_paths};
```

- [ ] **Step 2: Add `session_workspace_dir` helper**

In `src/sandbox/workspace.rs`, just above the private `fn session_key_to_filename`, add a public wrapper so out-of-band consumers derive the SAME per-session dir the sandbox uses. (`SessionId` and `std::path::{Path, PathBuf}` are already imported in this file.)

```rust
/// Compute the per-session workspace directory the same way `WorkspaceSandbox`
/// does, without instantiating one. Lets out-of-band consumers (cluster node
/// file commands) jail to the exact dir the node's bash sandbox uses.
pub fn session_workspace_dir(workspace_root: &Path, sid: &SessionId) -> PathBuf {
    workspace_root.join(session_key_to_filename(sid))
}
```

- [ ] **Step 3: Write the failing tests for node file commands**

Create `src/cluster/node_file_cmd.rs` with ONLY the test module first (so it fails to compile → RED). Put the production code in later steps.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write as _;

    fn b64(bytes: &[u8]) -> String {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    #[tokio::test]
    async fn file_write_then_read_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().to_path_buf();
        let writer = FileWriteCommand::new(ws.clone());
        let reader = FileReadCommand::new(ws.clone());

        let content = b"hello node bytes\x00\x01\x02";
        let res = writer
            .run(json!({
                "path": "out/data.bin",
                "content_b64": b64(content),
                "sha256": sha256_hex(content),
            }))
            .await
            .expect("write ok");
        assert_eq!(res["written"], content.len());

        let got = reader
            .run(json!({ "path": "out/data.bin" }))
            .await
            .expect("read ok");
        assert_eq!(got["size"], content.len());
        assert_eq!(got["sha256"], sha256_hex(content));
        use base64::Engine as _;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(got["content_b64"].as_str().unwrap())
            .unwrap();
        assert_eq!(decoded, content);
    }

    #[tokio::test]
    async fn file_write_rejects_oversize() {
        let dir = tempfile::tempdir().unwrap();
        let writer = FileWriteCommand::new(dir.path().to_path_buf());
        let big = vec![0u8; MAX_FILE_BYTES + 1];
        let err = writer
            .run(json!({ "path": "big.bin", "content_b64": b64(&big), "sha256": sha256_hex(&big) }))
            .await
            .expect_err("oversize rejected");
        assert!(err.contains("exceeds"), "{err}");
    }

    #[tokio::test]
    async fn file_write_rejects_sha_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let writer = FileWriteCommand::new(dir.path().to_path_buf());
        let err = writer
            .run(json!({ "path": "x.bin", "content_b64": b64(b"abc"), "sha256": "deadbeef" }))
            .await
            .expect_err("sha mismatch rejected");
        assert!(err.contains("sha256 mismatch"), "{err}");
    }

    #[tokio::test]
    async fn file_write_rejects_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let writer = FileWriteCommand::new(dir.path().to_path_buf());
        let err = writer
            .run(json!({
                "path": "../escape.bin",
                "content_b64": b64(b"x"),
                "sha256": sha256_hex(b"x"),
            }))
            .await
            .expect_err("traversal rejected");
        assert!(err.contains("escapes") || err.contains("rejected"), "{err}");
    }

    #[tokio::test]
    async fn file_write_respects_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let writer = FileWriteCommand::new(dir.path().to_path_buf());
        let args = json!({ "path": "f.bin", "content_b64": b64(b"v1"), "sha256": sha256_hex(b"v1") });
        writer.run(args.clone()).await.expect("first write ok");

        let err = writer.run(args).await.expect_err("no overwrite by default");
        assert!(err.contains("exists"), "{err}");

        writer
            .run(json!({ "path": "f.bin", "content_b64": b64(b"v2"), "sha256": sha256_hex(b"v2"), "overwrite": true }))
            .await
            .expect("overwrite ok");
    }

    #[tokio::test]
    async fn file_read_rejects_missing() {
        let dir = tempfile::tempdir().unwrap();
        let reader = FileReadCommand::new(dir.path().to_path_buf());
        let err = reader
            .run(json!({ "path": "nope.bin" }))
            .await
            .expect_err("missing rejected");
        assert!(err.contains("not found"), "{err}");
    }

    #[tokio::test]
    async fn file_read_rejects_oversize() {
        let dir = tempfile::tempdir().unwrap();
        let big_path = dir.path().join("big.bin");
        let mut f = std::fs::File::create(&big_path).unwrap();
        f.write_all(&vec![0u8; MAX_FILE_BYTES + 1]).unwrap();
        drop(f);
        let reader = FileReadCommand::new(dir.path().to_path_buf());
        let err = reader
            .run(json!({ "path": "big.bin" }))
            .await
            .expect_err("oversize read rejected");
        assert!(err.contains("exceeds"), "{err}");
    }
}
```

- [ ] **Step 4: Run the tests to verify they fail to compile**

Run: `cargo test -p alephcore --lib cluster::node_file_cmd 2>&1 | tail -15`
Expected: compile errors — `FileWriteCommand`, `FileReadCommand`, `sha256_hex`, `MAX_FILE_BYTES` not found.

- [ ] **Step 5: Implement the production code**

Prepend the production code ABOVE the test module in `src/cluster/node_file_cmd.rs`:

```rust
//! 节点侧文件命令（执行臂）：`file.read` / `file.write`。
//!
//! 字节直接走 host-fs（节点是执行臂，R1 允许），jail 在节点 session
//! workspace 目录内：相对路径 join workspace，绝对路径必须仍落在 workspace
//! 之下，否则拒。两端硬 8MB 上限 + sha256 完整性。无 LLM 推理（R7）。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine as _;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::builtin_tools::file_ops::{check_and_resolve_path, get_denied_paths};
use crate::cluster::{CommandDescriptor, NodeCommand};

/// 单文件硬上限（原始字节）。两端一致；超过即 fail-fast。
pub const MAX_FILE_BYTES: usize = 8 * 1024 * 1024;

const B64: base64::engine::general_purpose::GeneralPurpose = base64::engine::general_purpose::STANDARD;

/// 十六进制 sha256。两端用同一算法做完整性校验。
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// 把请求里的 `path` 解析进节点 workspace jail。相对路径 join workspace；
/// 任何最终 canonical 路径必须仍在 workspace 之下（绝对路径越界 → 拒）。
/// 复用 file_ops 的 canonicalize + deny-list，再补一道 containment 闸门
/// （`check_and_resolve_path` 本身不强制 containment，只用 base 解析相对路径）。
fn resolve_in_jail(path: &str, workspace_dir: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(workspace_dir)
        .map_err(|e| format!("workspace dir unavailable: {e}"))?;
    let root = workspace_dir
        .canonicalize()
        .map_err(|e| format!("workspace root unresolved: {e}"))?;
    let resolved = check_and_resolve_path(Path::new(path), &get_denied_paths(), Some(&root))
        .map_err(|e| format!("path rejected: {e}"))?;
    if !resolved.starts_with(&root) {
        return Err("path escapes node workspace".to_string());
    }
    Ok(resolved)
}

/// `file.write`：中心 push 的字节落到节点 workspace。
pub struct FileWriteCommand {
    workspace_dir: PathBuf,
}

impl FileWriteCommand {
    pub fn new(workspace_dir: PathBuf) -> Self {
        Self { workspace_dir }
    }
}

#[async_trait]
impl NodeCommand for FileWriteCommand {
    async fn run(&self, args: Value) -> Result<Value, String> {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or("file.write: missing string field `path`")?;
        let content_b64 = args
            .get("content_b64")
            .and_then(|v| v.as_str())
            .ok_or("file.write: missing string field `content_b64`")?;
        let expected_sha = args
            .get("sha256")
            .and_then(|v| v.as_str())
            .ok_or("file.write: missing string field `sha256`")?;
        let overwrite = args.get("overwrite").and_then(|v| v.as_bool()).unwrap_or(false);

        let bytes = B64
            .decode(content_b64)
            .map_err(|e| format!("file.write: invalid base64: {e}"))?;
        if bytes.len() > MAX_FILE_BYTES {
            return Err(format!(
                "file.write: {} bytes exceeds {MAX_FILE_BYTES} cap",
                bytes.len()
            ));
        }
        if sha256_hex(&bytes) != expected_sha {
            return Err("file.write: sha256 mismatch".to_string());
        }

        let dest = resolve_in_jail(path, &self.workspace_dir)?;
        if dest.exists() && !overwrite {
            return Err("file.write: target exists (set overwrite)".to_string());
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("file.write: {e}"))?;
        }
        std::fs::write(&dest, &bytes).map_err(|e| format!("file.write: {e}"))?;
        Ok(json!({ "written": bytes.len() }))
    }

    fn descriptor(&self) -> CommandDescriptor {
        CommandDescriptor {
            name: "file.write".to_string(),
            schema: json!({"type": "object"}),
        }
    }
}

/// `file.read`：中心 pull 节点 workspace 里的字节。
pub struct FileReadCommand {
    workspace_dir: PathBuf,
}

impl FileReadCommand {
    pub fn new(workspace_dir: PathBuf) -> Self {
        Self { workspace_dir }
    }
}

#[async_trait]
impl NodeCommand for FileReadCommand {
    async fn run(&self, args: Value) -> Result<Value, String> {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or("file.read: missing string field `path`")?;
        let src = resolve_in_jail(path, &self.workspace_dir)?;
        if !src.exists() {
            return Err("file.read: not found".to_string());
        }
        let size = std::fs::metadata(&src)
            .map_err(|e| format!("file.read: {e}"))?
            .len() as usize;
        if size > MAX_FILE_BYTES {
            return Err(format!("file.read: {size} bytes exceeds {MAX_FILE_BYTES} cap"));
        }
        let bytes = std::fs::read(&src).map_err(|e| format!("file.read: {e}"))?;
        Ok(json!({
            "content_b64": B64.encode(&bytes),
            "sha256": sha256_hex(&bytes),
            "size": bytes.len(),
        }))
    }

    fn descriptor(&self) -> CommandDescriptor {
        CommandDescriptor {
            name: "file.read".to_string(),
            schema: json!({"type": "object"}),
        }
    }
}
```

> **Note on `resolve_in_jail` for `file.read` missing files:** `check_and_resolve_path` handles non-existent paths via `safe_normalize`, so a missing-but-in-jail path resolves fine and the explicit `!src.exists()` check then returns the "not found" error. A missing path that also escapes the jail is caught by the `starts_with` check first.

- [ ] **Step 6: Add `Arc` import note + run the tests**

`tempfile` is a dev-dependency in this crate (used across tests). If `cargo test` reports `tempfile` unresolved, add it under `[dev-dependencies]` in `Cargo.toml`: `tempfile = "3"`. (Check first: `grep -n '^tempfile' Cargo.toml`.)

Run: `cargo test -p alephcore --lib cluster::node_file_cmd 2>&1 | tail -20`
Expected: PASS — 7 tests green.

- [ ] **Step 7: Wire the module + `register_file_commands`**

In `src/cluster/mod.rs`, add the module and re-exports. **Match the existing pattern exactly** — this file uses private `mod` + `pub use` (e.g. `mod node_runtime;` then `pub use node_runtime::{...}`), NOT `pub mod`. Add after the existing `mod` lines:

```rust
mod node_file_cmd;
```

and after the existing `pub use` lines:

```rust
pub use node_file_cmd::{FileReadCommand, FileWriteCommand, MAX_FILE_BYTES};
pub(crate) use node_file_cmd::sha256_hex;
```

> The `pub(crate) use` for `sha256_hex` lets `node_file.rs` and the integration test reach it as `crate::cluster::sha256_hex` (the module itself is private, so the full `crate::cluster::node_file_cmd::sha256_hex` path would NOT compile).

In `src/cluster/node_runtime.rs`, add a convenience method on `CommandTable` (place it next to `with_bash`). Add `use std::path::PathBuf;` if not already present (the file already imports `std::sync::Arc`):

```rust
impl CommandTable {
    /// 在已有命令之外注册 `file.read` / `file.write`，两者共享同一 jail 根
    /// （应传入节点 bash 的同一 session workspace 目录）。
    pub fn register_file_commands(&mut self, workspace_dir: std::path::PathBuf) {
        use crate::cluster::{FileReadCommand, FileWriteCommand};
        self.register("file.read", Arc::new(FileReadCommand::new(workspace_dir.clone())));
        self.register("file.write", Arc::new(FileWriteCommand::new(workspace_dir)));
    }
}
```

- [ ] **Step 8: Verify compile + tests still green**

Run: `cargo test -p alephcore --lib cluster:: 2>&1 | tail -15`
Expected: PASS — node_file_cmd tests + existing node_runtime tests all green.

- [ ] **Step 9: Commit**

```bash
git add src/builtin_tools/file_ops/mod.rs src/sandbox/workspace.rs \
        src/cluster/node_file_cmd.rs src/cluster/node_runtime.rs src/cluster/mod.rs
git commit -m "cluster: node-side file.read/file.write commands with workspace jail"
```

---

## Task 2: Center-side `node_file` tool

**Files:**
- Create: `src/builtin_tools/node_file.rs`
- Modify: `src/builtin_tools/mod.rs` (module + re-export)
- Test: inline `#[cfg(test)]` in `src/builtin_tools/node_file.rs`

- [ ] **Step 1: Write the failing tests**

Create `src/builtin_tools/node_file.rs` with ONLY the test module first (RED). It reuses the `NodeRegistry` + `ReverseRpcChannel` harness pattern from `node_invoke.rs`, with a node responder that simulates `file.write`/`file.read`.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster::{CommandDescriptor, NodeRegistry, NodeSession, ReverseRpcChannel};
    use crate::cluster::sha256_hex;
    use crate::gateway::protocol::JsonRpcResponse;
    use base64::Engine as _;
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
                .map(|c| CommandDescriptor { name: c.to_string(), schema: json!({}) })
                .collect(),
            connected_at: 1,
        });
        (reg, rx, channel)
    }

    /// 后台扮节点：对 file.write 回 {written}，对 file.read 回固定内容。
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
        std::fs::write(&local, b"payload-abc").unwrap();
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
        std::fs::write(&local, vec![0u8; super::MAX_FILE_BYTES + 1]).unwrap();
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
            declared_commands: vec![CommandDescriptor { name: "file.read".into(), schema: json!({}) }],
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
        assert!(err.to_string().contains("push") && err.to_string().contains("pull"), "{err}");
    }

    #[tokio::test]
    async fn rejects_command_not_declared_by_node() {
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("src.txt");
        std::fs::write(&local, b"x").unwrap();
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
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p alephcore --lib builtin_tools::node_file 2>&1 | tail -15`
Expected: compile errors — `NodeFileTool`, `NodeFileArgs`, `MAX_FILE_BYTES` not found.

- [ ] **Step 3: Implement the production tool**

Prepend ABOVE the test module in `src/builtin_tools/node_file.rs`:

```rust
//! `node_file`：中心侧 LLM 工具，与某个已连节点双向传输文件（经 0a 反向 RPC）。
//!
//! 字节在中心进程 ↔ 节点进程间流动，永不进入 LLM 上下文：LLM 只传路径，
//! 工具读写中心盘 + base64 + 驱动反向 RPC，返回 {bytes, sha256, paths} 摘要。
//! 红线：纯 I/O 翻译（R4），无推理（R7）；方向/路径由 LLM 决定。

use std::path::Path;

use crate::sync_primitives::Arc;

use async_trait::async_trait;
use base64::Engine as _;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::builtin_tools::file_ops::{check_and_resolve_path, get_denied_paths};
use crate::cluster::{sha256_hex, NodeRegistry, MAX_FILE_BYTES};
use crate::error::{AlephError, Result};
use crate::tools::AlephTool;

const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const B64: base64::engine::general_purpose::GeneralPurpose = base64::engine::general_purpose::STANDARD;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct NodeFileArgs {
    /// Target node: its name (e.g. "worker-1") or id. See `environments.list`.
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
    pub fn new(node_registry: Arc<NodeRegistry>) -> Self {
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
        let Some((channel, declared)) = self.node_registry.resolve(&args.node) else {
            return Err(AlephError::tool(format!("node '{}' not online", args.node)));
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
                let local = check_and_resolve_path(
                    Path::new(&args.local_path),
                    &get_denied_paths(),
                    None,
                )
                .map_err(|e| AlephError::tool(format!("local path rejected: {e}")))?;
                let bytes = std::fs::read(&local)
                    .map_err(|e| AlephError::tool(format!("read local '{}': {e}", local.display())))?;
                if bytes.len() > MAX_FILE_BYTES {
                    return Err(AlephError::tool(format!(
                        "{} bytes exceeds {MAX_FILE_BYTES} cap",
                        bytes.len()
                    )));
                }
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
                    .map_err(|e| AlephError::tool(format!("node '{}' reverse-rpc failed: {e}", args.node)))?;
                if !resp.is_success() {
                    return Err(AlephError::tool(format!(
                        "node '{}' file.write error: {}",
                        args.node,
                        resp.error.map(|e| e.message).unwrap_or_else(|| "unknown".to_string())
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
                    .map_err(|e| AlephError::tool(format!("node '{}' reverse-rpc failed: {e}", args.node)))?;
                if !resp.is_success() {
                    return Err(AlephError::tool(format!(
                        "node '{}' file.read error: {}",
                        args.node,
                        resp.error.map(|e| e.message).unwrap_or_else(|| "unknown".to_string())
                    )));
                }
                let result = resp.result.unwrap_or(Value::Null);
                let content_b64 = result
                    .get("content_b64")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| AlephError::tool("node file.read missing content_b64"))?;
                let node_sha = result.get("sha256").and_then(|v| v.as_str()).unwrap_or("");
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
                let local = check_and_resolve_path(
                    Path::new(&args.local_path),
                    &get_denied_paths(),
                    None,
                )
                .map_err(|e| AlephError::tool(format!("local path rejected: {e}")))?;
                if local.exists() && !args.overwrite {
                    return Err(AlephError::tool(
                        "local target exists (set overwrite)".to_string(),
                    ));
                }
                if let Some(parent) = local.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| AlephError::tool(format!("create local dir: {e}")))?;
                }
                std::fs::write(&local, &bytes)
                    .map_err(|e| AlephError::tool(format!("write local '{}': {e}", local.display())))?;
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
```

> **Note:** `sha256_hex` reaches `node_file.rs` via the `pub(crate) use node_file_cmd::sha256_hex;` re-export added in Task 1 Step 7 (imported here as `crate::cluster::sha256_hex`). The test module references `super::MAX_FILE_BYTES`; the production `use crate::cluster::{sha256_hex, NodeRegistry, MAX_FILE_BYTES};` brings `MAX_FILE_BYTES` into module scope so `super::MAX_FILE_BYTES` resolves.

- [ ] **Step 4: Register the module**

In `src/builtin_tools/mod.rs`, mirror the `node_invoke` declarations. Find `pub mod node_invoke;` and add after it:

```rust
pub mod node_file;
```

Find `pub use node_invoke::{NodeInvokeArgs, NodeInvokeTool};` and add after it:

```rust
pub use node_file::{NodeFileArgs, NodeFileTool};
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p alephcore --lib builtin_tools::node_file 2>&1 | tail -20`
Expected: PASS — 6 tests green.

- [ ] **Step 6: Commit**

```bash
git add src/builtin_tools/node_file.rs src/builtin_tools/mod.rs
git commit -m "cluster: center-side node_file push/pull tool with sha256 integrity"
```

---

## Task 3: Wiring + integration test

**Files:**
- Modify: `src/bin/aleph-server/commands/node.rs` (`build_command_table`)
- Modify: `src/executor/builtin_registry/definitions.rs`
- Modify: `src/executor/builtin_registry/registry.rs`
- Modify: `src/executor/builtin_registry/groups.rs`
- Modify: `src/executor/builtin_registry/builder/optional_tools.rs`
- Test: `tests/cluster_node_runtime.rs`

- [ ] **Step 1: Register file commands on the node**

In `src/bin/aleph-server/commands/node.rs`, update `build_command_table` to derive the session workspace dir and register the file commands alongside bash. Replace the last two lines of the function:

```rust
    let bash = alephcore::builtin_tools::BashExecTool::new().with_sandbox(sandbox);
    let session = SessionKey::ephemeral(format!("node-{name}"));
    CommandTable::with_bash(bash, session)
}
```

with:

```rust
    let bash = alephcore::builtin_tools::BashExecTool::new().with_sandbox(sandbox);
    let session = SessionKey::ephemeral(format!("node-{name}"));
    // file.read/file.write jail to the SAME per-session workspace dir bash uses,
    // so a pushed script can be bash-run and a bash-produced artifact pulled.
    let workspace_dir =
        alephcore::sandbox::workspace::session_workspace_dir(&cfg.workspace_root, &session);
    let mut table = CommandTable::with_bash(bash, session);
    table.register_file_commands(workspace_dir);
    table
}
```

- [ ] **Step 2: Add the `node_file` tool definition (2 sites)**

In `src/executor/builtin_registry/definitions.rs`, after the `node_invoke` `BuiltinToolDefinition { ... }` block (the one with `name: "node_invoke"`), add:

```rust
    BuiltinToolDefinition {
        name: "node_file",
        description: "Transfer a file between the center and a connected cluster node by path (push/pull). Bytes move host-to-host over the cluster channel and never enter the conversation; 8 MB cap; the node must declare file.read/file.write.",
        requires_config: true, // Requires NodeRegistry (deferred via OnceCell)
    },
```

Then in the schema-override `match` (near the `"node_invoke" => None,` arm), add after it:

```rust
        // node_file requires the gateway NodeRegistry, injected at boot via
        // set_node_registry; built fresh per call — same pattern as node_invoke.
        "node_file" => None,
```

- [ ] **Step 3: Add the dispatch arm**

In `src/executor/builtin_registry/registry.rs`, after the `"node_invoke" => Box::pin(async move { ... }),` arm, add the sibling arm (reuses the same `node_registry` cell):

```rust
            // Cluster file-transfer tool — same injected NodeRegistry as node_invoke.
            "node_file" => Box::pin(async move {
                let reg = self.node_registry.get().ok_or_else(|| {
                    AlephError::tool("node_file not available: NodeRegistry not injected")
                })?;
                let tool = crate::builtin_tools::NodeFileTool::new(reg.clone());
                tool.call_json(arguments).await
            }),
```

- [ ] **Step 4: Add to the `cluster` group + optional_tools registration**

In `src/executor/builtin_registry/groups.rs`, change the cluster category's tools:

```rust
        tools: &["node_invoke"],
```

to:

```rust
        tools: &["node_invoke", "node_file"],
```

In `src/executor/builtin_registry/builder/optional_tools.rs`, after the `node_invoke` `reg(...)` block and its `info!`, add:

```rust
        // node_file — cluster file transfer. Same deferred NodeRegistry as node_invoke.
        reg(
            tools,
            "node_file",
            crate::builtin_tools::NodeFileTool::DESCRIPTION,
            schema::<crate::builtin_tools::node_file::NodeFileArgs>("node_file"),
        );
        info!("Registered node_file tool in BuiltinToolRegistry");
```

- [ ] **Step 5: Verify the wiring compiles**

Run: `cargo check -p alephcore 2>&1 | tail -15`
Expected: clean compile (no errors).

- [ ] **Step 6: Write the failing integration test**

In `tests/cluster_node_runtime.rs`, add a new test that exercises a `CommandTable` with file commands directly (no WS server needed — dispatch round-trip). Append at the end of the file:

```rust
#[tokio::test]
async fn command_table_file_roundtrip() {
    use alephcore::cluster::CommandTable;

    let dir = tempfile::tempdir().unwrap();
    let mut table = CommandTable::new();
    table.register_file_commands(dir.path().to_path_buf());

    let content = b"integration-bytes\x00\xff";
    let sha = {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(content);
        hex::encode(h.finalize())
    };
    let b64 = {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(content)
    };

    let write = table
        .dispatch(
            "tool.call",
            &json!({ "tool": "file.write", "args": { "path": "round/trip.bin", "content_b64": b64, "sha256": sha } }),
        )
        .await
        .expect("file.write dispatch ok");
    assert_eq!(write["written"], content.len());

    let read = table
        .dispatch("tool.call", &json!({ "tool": "file.read", "args": { "path": "round/trip.bin" } }))
        .await
        .expect("file.read dispatch ok");
    use base64::Engine as _;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(read["content_b64"].as_str().unwrap())
        .unwrap();
    assert_eq!(decoded, content);
}
```

If `tests/cluster_node_runtime.rs` lacks the needed imports, ensure the test file (or this test) can reach `sha2`, `hex`, `base64` — they are workspace deps; the test already imports `serde_json::json`. Add `CommandTable` via the `use` inside the test (shown above).

- [ ] **Step 7: Run the integration test**

Run: `cargo test -p alephcore --test cluster_node_runtime 2>&1 | tail -15`
Expected: PASS — `center_runs_bash_on_connected_node` + `command_table_file_roundtrip` both green.

- [ ] **Step 8: Full verification sweep**

Run:
```bash
cargo test -p alephcore --lib cluster:: 2>&1 | tail -5
cargo test -p alephcore --lib builtin_tools::node_file 2>&1 | tail -5
cargo build -p alephcore --bin aleph-server 2>&1 | tail -3
cargo clippy -p alephcore 2>&1 | grep -E "warning|error" | grep -iE "node_file|node_runtime|cluster" | head
```
Expected: all tests green, bin builds, no new clippy warnings in touched files.

- [ ] **Step 9: Commit**

```bash
git add src/bin/aleph-server/commands/node.rs \
        src/executor/builtin_registry/definitions.rs \
        src/executor/builtin_registry/registry.rs \
        src/executor/builtin_registry/groups.rs \
        src/executor/builtin_registry/builder/optional_tools.rs \
        tests/cluster_node_runtime.rs
git commit -m "cluster: wire node_file tool + register node file commands + integration test"
```

---

## Final Verification (after all tasks)

- [ ] `cargo test -p alephcore --lib cluster:: --` green
- [ ] `cargo test -p alephcore --lib builtin_tools::node_file` green
- [ ] `cargo test -p alephcore --test cluster_node_runtime` green
- [ ] `cargo build -p alephcore --bin aleph-server` clean
- [ ] `git diff --stat src/harness` is EMPTY (R10 untouched)
- [ ] Do NOT merge to main; leave the worktree + branch for the user's cluster merge.

---

## Redline Compliance Checklist

- **R1** (brain-limb): node file commands are direct host-fs on the execution arm; no platform-specific API in `src`. ✓
- **R4** (I/O-only interface): `node_file` is pure path→bytes translation, no business logic. ✓
- **R7** (LLM sovereignty): LLM chooses node/direction/paths; system moves bytes deterministically. ✓
- **R10** (thin harness): `src/harness/` untouched. ✓
- **P7** (defensive): two-end 8 MB cap, sha256 integrity, node workspace jail (`starts_with` containment + deny-list), overwrite protection. ✓
