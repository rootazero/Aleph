# Cluster Tags + node_invoke_many Fan-out Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add free-text node tags and a `node_invoke_many` LLM tool that fans a command out concurrently to all online nodes matching an AND-set of tags.

**Architecture:** Tags are `Vec<String>` carried on the `connect` frame (CLI-supplied each run via repeatable `--tag`), stored on `NodeSession`, projected into `Environment`, and matched by a new `NodeRegistry::resolve_all_by_tags`. The fan-out tool resolves the match set, dispatches concurrently via `tokio::task::JoinSet` (wall-clock = slowest single node), tolerates partial failure, and returns an aggregate `{invoked, succeeded, failed, results}`. Tags are selection-only — node-side `CommandTable` allowlist remains the execution authority (R7).

**Tech Stack:** Rust, Tokio (`JoinSet`), serde/serde_json, schemars (`JsonSchema`), clap (repeatable arg → `Vec<String>`), `async_trait`.

**Deviations from spec (intentional, flagged during planning):**
- Tags flow via the `connect` frame **only**. NOT persisted in `~/.aleph/node/<name>.json` (avoids flag-vs-disk precedence ambiguity; mirrors how `--name`/`--center` are supplied each run). NOT added to `pairing.start_node` (the center does not consume tags there — sending them would be an advertised-but-unwired dead field, the exact anti-pattern this work avoids). The live `NodeSession` is populated from the `connect` frame, which carries tags, so registry tags are fully wired.

**Project protocol (OVERRIDES the writing-plans "run the test" steps):** Per the task's hard constraint, **do NOT run `cargo check`/`cargo test` this session — author tests for correctness and commit directly.** Each task lists the intended verification command marked `(DEFERRED — do not run)`; it documents what the test asserts, not a step to execute now.

---

### Task 1: Node tags in the registry + tag-based resolution

**Files:**
- Modify: `src/cluster/registry.rs` (add field, `NodeMatch`, `resolve_all_by_tags`, parse tags, fix literals)
- Modify: `src/cluster/mod.rs:20` (export `NodeMatch`)
- Modify: `src/builtin_tools/node_invoke.rs:118,204` (add `tags: vec![]` to test literals)
- Modify: `src/builtin_tools/node_file.rs:229,349` (add `tags: vec![]` to test literals)

- [ ] **Step 1: Add the failing tests** (append to the `tests` module in `src/cluster/registry.rs`, before its closing `}`)

```rust
    #[test]
    fn resolve_all_by_tags_and_semantics() {
        let reg = NodeRegistry::new();
        reg.register(NodeSession {
            node_id: "a".into(),
            conn_id: "ca".into(),
            device_name: "node-a".into(),
            channel: test_channel(),
            declared_commands: vec![],
            tags: vec!["gpu".into(), "us".into()],
            connected_at: 1,
        });
        reg.register(NodeSession {
            node_id: "b".into(),
            conn_id: "cb".into(),
            device_name: "node-b".into(),
            channel: test_channel(),
            declared_commands: vec![],
            tags: vec!["gpu".into()],
            connected_at: 1,
        });
        // AND: both tags required → only "a".
        let both = reg.resolve_all_by_tags(&["gpu".into(), "us".into()]);
        assert_eq!(both.len(), 1);
        assert_eq!(both[0].node_id, "a");
        assert_eq!(both[0].name, "node-a");
        // Single tag both carry → both.
        assert_eq!(reg.resolve_all_by_tags(&["gpu".into()]).len(), 2);
        // Empty tags → every online node.
        assert_eq!(reg.resolve_all_by_tags(&[]).len(), 2);
        // Unmatched tag → none.
        assert!(reg.resolve_all_by_tags(&["fpga".into()]).is_empty());
        // NodeMatch carries the node's tags (used for the zero-match hint).
        let gpu = reg.resolve_all_by_tags(&["gpu".into()]);
        assert!(gpu.iter().any(|m| m.tags.contains(&"us".to_string())));
    }

    #[test]
    fn maybe_register_node_parses_tags_from_params() {
        let reg = NodeRegistry::new();
        let ch = test_channel();
        let params = json!({
            "device_name": "worker",
            "commands": [{"name": "bash", "schema": {}}],
            "tags": ["gpu", "region=us"]
        });
        assert!(maybe_register_node(&reg, Some("node"), "d1", "c1", Some(&params), &ch));
        let m = reg.resolve_all_by_tags(&["region=us".into()]);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].node_id, "d1");
        // Missing "tags" key → empty, not an error.
        let ch2 = test_channel();
        let no_tags = json!({"device_name": "w2", "commands": []});
        assert!(maybe_register_node(&reg, Some("node"), "d2", "c2", Some(&no_tags), &ch2));
        assert_eq!(reg.resolve_all_by_tags(&[]).len(), 2);
    }
```

- [ ] **Step 2: Intended verification** `(DEFERRED — do not run)`

Command: `cargo test -p alephcore --lib cluster::registry`
Expected once implemented: PASS. Right now it would not compile (`tags` field, `resolve_all_by_tags` absent).

- [ ] **Step 3: Add `tags` to `NodeSession`** — in `src/cluster/registry.rs`, inside `pub struct NodeSession`, after the `declared_commands` field (line ~36):

```rust
    /// 节点自声明的 command 目录，0b 只存只显。
    pub declared_commands: Vec<CommandDescriptor>,
    /// Operator-assigned free-text labels (e.g. "gpu", "region=us"). Selection
    /// only — never an authorization gate (R7). Stored verbatim; not kv-parsed.
    pub tags: Vec<String>,
    /// 登记时刻（Unix 秒）。
    pub connected_at: i64,
```

- [ ] **Step 4: Add `tags` to `Environment`** — in `pub struct Environment`, after `commands` (line ~67):

```rust
    pub commands: Vec<CommandDescriptor>,
    pub tags: Vec<String>,
    pub connected_at: i64,
```

And populate it in `list_environments` (the `.map(|s| Environment { ... })`, line ~127):

```rust
            .map(|s| Environment {
                id: s.node_id.clone(),
                name: s.device_name.clone(),
                status: "online",
                commands: s.declared_commands.clone(),
                tags: s.tags.clone(),
                connected_at: s.connected_at,
            })
```

- [ ] **Step 5: Add `NodeMatch` + `resolve_all_by_tags`** — in `src/cluster/registry.rs`, immediately after the `Environment` struct definition (after line ~69):

```rust
/// A matched online node for tag-selected fan-out: enough to dispatch over
/// reverse RPC and run the same per-node fail-fast check `node_invoke` uses.
/// `tags` is carried so the caller can build a "available tags" hint on a
/// zero-match. Cloneable; holds a `ReverseRpcChannel` clone.
#[derive(Clone)]
pub struct NodeMatch {
    pub node_id: String,
    pub name: String,
    pub channel: ReverseRpcChannel,
    pub declared_commands: Vec<CommandDescriptor>,
    pub tags: Vec<String>,
}
```

Then add the method inside `impl NodeRegistry`, after `resolve` (after line ~205):

```rust
    /// All online nodes carrying EVERY tag in `tags` (AND match). An empty
    /// `tags` slice matches every online node (the "broadcast" case). Used by
    /// `node_invoke_many` for tag-selected concurrent fan-out. Returns a clone
    /// snapshot so the caller dispatches without holding the registry lock.
    pub fn resolve_all_by_tags(&self, tags: &[String]) -> Vec<NodeMatch> {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        inner
            .nodes_by_id
            .values()
            .filter(|s| tags.iter().all(|t| s.tags.contains(t)))
            .map(|s| NodeMatch {
                node_id: s.node_id.clone(),
                name: s.device_name.clone(),
                channel: s.channel.clone(),
                declared_commands: s.declared_commands.clone(),
                tags: s.tags.clone(),
            })
            .collect()
    }
```

- [ ] **Step 6: Parse `tags` in `maybe_register_node`** — in `src/cluster/registry.rs`, in `maybe_register_node`, after the `declared_commands` binding (line ~262) and add the field to the `NodeSession` literal (line ~266):

```rust
    let declared_commands = params
        .and_then(|p| p.get("commands"))
        .and_then(|v| serde_json::from_value::<Vec<CommandDescriptor>>(v.clone()).ok())
        .unwrap_or_default();
    let tags = params
        .and_then(|p| p.get("tags"))
        .and_then(|v| serde_json::from_value::<Vec<String>>(v.clone()).ok())
        .unwrap_or_default();
    registry.register(NodeSession {
        node_id: device_id.to_string(),
        conn_id: conn_id.to_string(),
        device_name,
        channel: channel.clone(),
        declared_commands,
        tags,
        connected_at: now_unix(),
    });
    true
```

- [ ] **Step 7: Fix `NodeSession` literals so the crate compiles** — add `tags: vec![],` after the `declared_commands` line in each of these existing literals:
  - `src/cluster/registry.rs:296` (the `session()` test helper)
  - `src/cluster/registry.rs:379` and `:387` (the two literals in `resolve_reports_ambiguity_with_sorted_candidates`)
  - `src/builtin_tools/node_invoke.rs:118` and `:204`
  - `src/builtin_tools/node_file.rs:229` and `:349`

Example (the `session()` helper at registry.rs:296):

```rust
            declared_commands: vec![CommandDescriptor {
                name: "bash".to_string(),
                schema: json!({"type": "object"}),
            }],
            tags: vec![],
            connected_at: 1,
```

For the bare literals (registry.rs:379/387, node_invoke.rs:204, node_file.rs) that have `declared_commands: vec![],` add `tags: vec![],` on the next line. For node_invoke.rs:118 (inside `registry_with_node`, which maps `commands` into `declared_commands`) add `tags: vec![],` after the `declared_commands: ... .collect(),` line.

- [ ] **Step 8: Export `NodeMatch`** — in `src/cluster/mod.rs:20`, add `NodeMatch` to the `registry` re-export list:

```rust
    maybe_register_node, CommandDescriptor, Environment, NodeMatch, NodeRegistry, NodeSession,
    ResolveError,
```

- [ ] **Step 9: Commit**

```bash
git add src/cluster/registry.rs src/cluster/mod.rs src/builtin_tools/node_invoke.rs src/builtin_tools/node_file.rs
git commit -m "cluster: node tags on NodeSession/Environment + resolve_all_by_tags"
```

---

### Task 2: `node_invoke_many` fan-out tool

**Files:**
- Create: `src/builtin_tools/node_invoke_many.rs`
- Modify: `src/builtin_tools/mod.rs` (declare module + re-export)

- [ ] **Step 1: Create the tool with its failing tests** — write `src/builtin_tools/node_invoke_many.rs`:

```rust
//! `node_invoke_many`：中心侧 LLM 工具，按标签把一条命令并发扇出到一组节点。
//!
//! 与 `node_invoke`（解析→唯一节点，歧义=报错）语义显式分离：本工具按 tag 的
//! AND 集合匹配一组在线节点，用 `tokio::task::JoinSet` 并发下发，容忍部分失败，
//! 返回聚合结果。零命中报错并附可用标签提示。
//! 红线：纯 I/O 翻译（R4），无推理（R7）；标签选择由 LLM 做，标签不是授权层。

use crate::sync_primitives::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::cluster::{NodeMatch, NodeRegistry};
use crate::error::{AlephError, Result};
use crate::tools::AlephTool;

const DEFAULT_TIMEOUT_MS: u64 = 120_000;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct NodeInvokeManyArgs {
    /// Tags an online node must ALL carry to be selected (AND match). Empty or
    /// omitted = every online node (broadcast). Tags are verbatim labels like
    /// "gpu" or "region=us"; see `environments.list`.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Command to run on each matched node (e.g. "bash"). Each node must
    /// declare it, or that node's result is an error (others still run).
    pub command: String,
    /// JSON arguments for the command, passed through to each node verbatim.
    #[serde(default)]
    pub args: Value,
    /// Per-node reverse-RPC timeout in ms (default 120000). Applied to every
    /// node independently; one slow node does not extend the others.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Clone)]
pub struct NodeInvokeManyTool {
    node_registry: Arc<NodeRegistry>,
}

impl NodeInvokeManyTool {
    pub fn new(node_registry: Arc<NodeRegistry>) -> Self {
        Self { node_registry }
    }
}

/// Invoke `command` on one matched node; never returns Err — every outcome is
/// encoded as a per-node result object so a failure does not abort the fan-out.
async fn invoke_one(m: NodeMatch, command: String, args: Value, timeout_ms: u64) -> Value {
    // Per-node fail-fast, mirroring node_invoke: reject only when the node
    // declared a non-empty catalog that excludes this command.
    if !m.declared_commands.is_empty() && !m.declared_commands.iter().any(|c| c.name == command) {
        return json!({
            "node": m.name, "node_id": m.node_id, "ok": false,
            "error": format!("command '{command}' not declared by node '{}'", m.name)
        });
    }
    let params = json!({ "tool": command, "args": args });
    match m.channel.call("tool.call", params, timeout_ms).await {
        Ok(resp) if resp.is_success() => json!({
            "node": m.name, "node_id": m.node_id, "ok": true,
            "result": resp.result.unwrap_or(Value::Null)
        }),
        Ok(resp) => json!({
            "node": m.name, "node_id": m.node_id, "ok": false,
            "error": resp.error.map(|e| e.message).unwrap_or_else(|| "unknown".to_string())
        }),
        Err(e) => json!({
            "node": m.name, "node_id": m.node_id, "ok": false,
            "error": format!("reverse-rpc failed: {e}")
        }),
    }
}

#[async_trait]
impl AlephTool for NodeInvokeManyTool {
    const NAME: &'static str = "node_invoke_many";
    const DESCRIPTION: &'static str = r#"Run one command CONCURRENTLY on every connected cluster node that carries ALL of the given tags (a scatter-gather fan-out).

Select nodes by `tags` (AND match) — e.g. {"tags": ["gpu"], "command": "bash", "args": {"cmd": "nvidia-smi -L"}}. An empty/omitted `tags` targets every online node. See `environments.list` for online nodes and their tags. `command` must be one each node declares (a node that doesn't declare it returns a per-node error; others still run).

Each node runs in its own sandbox with an independent `timeout_ms` (default 120000). Partial failure is tolerated: you always get back {"invoked", "succeeded", "failed", "results":[{"node","node_id","ok",("result"|"error")}]}. If no online node matches the tags you get a clear error listing the available tags."#;

    type Args = NodeInvokeManyArgs;
    type Output = Value;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let matches = self.node_registry.resolve_all_by_tags(&args.tags);
        if matches.is_empty() {
            // Zero-match fail-fast (mirrors resolve's NotFound style): tell the
            // LLM exactly what tags ARE available so it can correct itself.
            let online = self.node_registry.resolve_all_by_tags(&[]);
            let hint = if online.is_empty() {
                "no nodes are online".to_string()
            } else {
                let mut tags: Vec<String> = online.iter().flat_map(|m| m.tags.clone()).collect();
                tags.sort();
                tags.dedup();
                if tags.is_empty() {
                    format!("{} online node(s) declare no tags", online.len())
                } else {
                    format!("available tags: {}", tags.join(", "))
                }
            };
            return Err(AlephError::tool(format!(
                "no online node matches tags {:?} — {hint}",
                args.tags
            )));
        }
        let timeout_ms = args.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS);
        let mut set = tokio::task::JoinSet::new();
        for m in matches {
            let command = args.command.clone();
            let call_args = args.args.clone();
            set.spawn(async move { invoke_one(m, command, call_args, timeout_ms).await });
        }
        let mut results: Vec<Value> = Vec::new();
        while let Some(joined) = set.join_next().await {
            match joined {
                Ok(v) => results.push(v),
                Err(e) => results.push(json!({"ok": false, "error": format!("task join error: {e}")})),
            }
        }
        let invoked = results.len();
        let succeeded = results.iter().filter(|r| r["ok"] == json!(true)).count();
        Ok(json!({
            "invoked": invoked,
            "succeeded": succeeded,
            "failed": invoked - succeeded,
            "results": results,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster::{CommandDescriptor, NodeRegistry, NodeSession, ReverseRpcChannel};
    use crate::gateway::protocol::JsonRpcResponse;
    use tokio::sync::mpsc;

    /// Register a node with the given tags + declared commands. Returns the
    /// node's outbound receiver so the test can choose to service it (success)
    /// or drop it (timeout).
    fn add_node(
        reg: &Arc<NodeRegistry>,
        node_id: &str,
        name: &str,
        tags: &[&str],
        commands: &[&str],
    ) -> (mpsc::Receiver<String>, ReverseRpcChannel) {
        let (tx, rx) = mpsc::channel::<String>(8);
        let channel = ReverseRpcChannel::new(tx);
        reg.register(NodeSession {
            node_id: node_id.to_string(),
            conn_id: format!("conn-{node_id}"),
            device_name: name.to_string(),
            channel: channel.clone(),
            declared_commands: commands
                .iter()
                .map(|c| CommandDescriptor { name: c.to_string(), schema: json!({}) })
                .collect(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            connected_at: 1,
        });
        (rx, channel)
    }

    /// Background "node": read one tool.call frame → resolve a success response.
    fn spawn_responder(mut rx: mpsc::Receiver<String>, channel: ReverseRpcChannel) {
        let pending = channel.pending();
        tokio::spawn(async move {
            if let Some(frame) = rx.recv().await {
                let req: Value = serde_json::from_str(&frame).unwrap();
                let id = req["id"].clone();
                let resp = JsonRpcResponse::success(Some(id.clone()), json!({"ran": req["params"]["tool"]}));
                pending.resolve(&id, resp);
            }
        });
    }

    #[tokio::test]
    async fn fans_out_to_all_matching_nodes_concurrently() {
        let reg = Arc::new(NodeRegistry::new());
        let (rx1, ch1) = add_node(&reg, "n1", "gpu-1", &["gpu"], &["bash"]);
        let (rx2, ch2) = add_node(&reg, "n2", "gpu-2", &["gpu"], &["bash"]);
        add_node(&reg, "n3", "cpu-1", &["cpu"], &["bash"]); // not matched
        spawn_responder(rx1, ch1);
        spawn_responder(rx2, ch2);
        let tool = NodeInvokeManyTool::new(reg);
        let out = tool
            .call(NodeInvokeManyArgs {
                tags: vec!["gpu".into()],
                command: "bash".into(),
                args: json!({"cmd": "echo hi"}),
                timeout_ms: Some(2_000),
            })
            .await
            .expect("fan-out resolves");
        assert_eq!(out["invoked"], 2);
        assert_eq!(out["succeeded"], 2);
        assert_eq!(out["failed"], 0);
        assert_eq!(out["results"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn tolerates_partial_failure() {
        let reg = Arc::new(NodeRegistry::new());
        let (rx1, ch1) = add_node(&reg, "n1", "gpu-1", &["gpu"], &["bash"]);
        // n2 has no responder → its call times out.
        let (_rx2, _ch2) = add_node(&reg, "n2", "gpu-2", &["gpu"], &["bash"]);
        spawn_responder(rx1, ch1);
        let tool = NodeInvokeManyTool::new(reg);
        let out = tool
            .call(NodeInvokeManyArgs {
                tags: vec!["gpu".into()],
                command: "bash".into(),
                args: json!({}),
                timeout_ms: Some(80),
            })
            .await
            .expect("fan-out resolves even with a failing node");
        assert_eq!(out["invoked"], 2);
        assert_eq!(out["succeeded"], 1);
        assert_eq!(out["failed"], 1);
    }

    #[tokio::test]
    async fn per_node_fail_fast_on_undeclared_command() {
        let reg = Arc::new(NodeRegistry::new());
        add_node(&reg, "n1", "gpu-1", &["gpu"], &["bash"]); // declares only bash
        let tool = NodeInvokeManyTool::new(reg);
        let out = tool
            .call(NodeInvokeManyArgs {
                tags: vec!["gpu".into()],
                command: "python".into(),
                args: json!({}),
                timeout_ms: Some(500),
            })
            .await
            .expect("resolves with a per-node error");
        assert_eq!(out["invoked"], 1);
        assert_eq!(out["succeeded"], 0);
        assert_eq!(out["failed"], 1);
        let err = out["results"][0]["error"].as_str().unwrap();
        assert!(err.contains("not declared"), "{err}");
    }

    #[tokio::test]
    async fn zero_match_errors_with_available_tags_hint() {
        let reg = Arc::new(NodeRegistry::new());
        add_node(&reg, "n1", "gpu-1", &["gpu"], &["bash"]);
        let tool = NodeInvokeManyTool::new(reg);
        let err = tool
            .call(NodeInvokeManyArgs {
                tags: vec!["fpga".into()],
                command: "bash".into(),
                args: json!({}),
                timeout_ms: Some(500),
            })
            .await
            .expect_err("zero match errors");
        let msg = err.to_string();
        assert!(msg.contains("no online node matches"), "{msg}");
        assert!(msg.contains("available tags: gpu"), "{msg}");
    }
}
```

- [ ] **Step 2: Intended verification** `(DEFERRED — do not run)`

Command: `cargo test -p alephcore --lib node_invoke_many`
Expected once wired: PASS (4 tests). Asserts concurrent fan-out counts, partial-failure tolerance, per-node fail-fast, zero-match hint.

- [ ] **Step 3: Declare + re-export the module** — in `src/builtin_tools/mod.rs`, add the module declaration next to the other cluster tools (near where `node_invoke` / `node_file` modules are declared) and add the re-export after line 191:

```rust
pub use node_invoke::{NodeInvokeArgs, NodeInvokeTool};
pub use node_invoke_many::{NodeInvokeManyArgs, NodeInvokeManyTool};
pub use node_file::{NodeFileArgs, NodeFileTool};
```

Find the existing `mod node_invoke;` / `mod node_file;` declarations in the same file and add `mod node_invoke_many;` alongside them (match the exact `mod`/`pub mod` visibility the siblings use).

- [ ] **Step 4: Commit**

```bash
git add src/builtin_tools/node_invoke_many.rs src/builtin_tools/mod.rs
git commit -m "builtin_tools: node_invoke_many concurrent tag fan-out tool"
```

---

### Task 3: Register `node_invoke_many` in the executor

**Files:**
- Modify: `src/executor/builtin_registry/builder/optional_tools.rs` (after the `node_file` registration, line ~153)
- Modify: `src/executor/builtin_registry/registry.rs` (after the `node_file` dispatch arm, line ~854)

- [ ] **Step 1: Register name + description + schema** — in `optional_tools.rs`, after the `node_file` `reg(...)` block (after line 153):

```rust
        // node_invoke_many — cluster tag fan-out. Same deferred NodeRegistry.
        reg(
            tools,
            "node_invoke_many",
            crate::builtin_tools::NodeInvokeManyTool::DESCRIPTION,
            schema::<crate::builtin_tools::node_invoke_many::NodeInvokeManyArgs>("node_invoke_many"),
        );
        info!("Registered node_invoke_many tool in BuiltinToolRegistry");
```

- [ ] **Step 2: Add the dispatch arm** — in `registry.rs`, after the `"node_file" => Box::pin(...)` arm (after line 854):

```rust
            // Cluster tag fan-out — same injected NodeRegistry as node_invoke.
            "node_invoke_many" => Box::pin(async move {
                let reg = self.node_registry.get().ok_or_else(|| {
                    AlephError::tool("node_invoke_many not available: NodeRegistry not injected")
                })?;
                let tool = crate::builtin_tools::NodeInvokeManyTool::new(reg.clone());
                tool.call_json(arguments).await
            }),
```

- [ ] **Step 3: Intended verification** `(DEFERRED — do not run)`

Command: `cargo build -p alephcore`
Expected once implemented: builds; `node_invoke_many` is registered and dispatchable. (No new unit test here — registration is glue mirroring `node_invoke`; its behavior is covered by Task 2.)

- [ ] **Step 4: Commit**

```bash
git add src/executor/builtin_registry/builder/optional_tools.rs src/executor/builtin_registry/registry.rs
git commit -m "executor: register + dispatch node_invoke_many"
```

---

### Task 4: `--tag` CLI flag → connect frame

**Files:**
- Modify: `src/bin/aleph-server/cli.rs:206-218` (add `tags` to the `Node` variant + a parse test near line 771)
- Modify: `src/bin/aleph-server/main.rs:250-256` (thread `tags` into `handle_node`)
- Modify: `src/bin/aleph-server/commands/node.rs` (`handle_node` signature + connect frame)

- [ ] **Step 1: Add the failing CLI parse test** — in `src/bin/aleph-server/cli.rs`, in the test module that already has the `Node` parsing tests (around line 771), add:

```rust
    #[test]
    fn node_command_collects_repeated_tags() {
        let cli = Cli::try_parse_from([
            "aleph-server", "node",
            "--center", "ws://127.0.0.1:18790",
            "--name", "gpu-1",
            "--tag", "gpu",
            "--tag", "region=us",
        ])
        .expect("parses");
        match cli.command {
            Some(Command::Node { tags, .. }) => {
                assert_eq!(tags, vec!["gpu".to_string(), "region=us".to_string()]);
            }
            _ => panic!("Expected Node command"),
        }
    }

    #[test]
    fn node_command_defaults_to_no_tags() {
        let cli = Cli::try_parse_from([
            "aleph-server", "node", "--center", "ws://c",
        ])
        .expect("parses");
        match cli.command {
            Some(Command::Node { tags, .. }) => assert!(tags.is_empty()),
            _ => panic!("Expected Node command"),
        }
    }
```

(If the existing test uses a constructor other than `Cli::try_parse_from`, mirror that exact pattern — the two existing `Node` tests at lines 771 and 796 show the canonical form to copy.)

- [ ] **Step 2: Intended verification** `(DEFERRED — do not run)`

Command: `cargo test -p alephcore --bin aleph-server cli::tests::node_command`
Expected once implemented: PASS. Asserts repeated `--tag` collects into a `Vec<String>` and defaults empty.

- [ ] **Step 3: Add `tags` to the `Node` clap variant** — in `src/bin/aleph-server/cli.rs`, inside the `Node { ... }` variant, after the `name` field (line ~217):

```rust
        /// Human-readable node name shown in `environments.list`.
        #[arg(long, value_name = "NAME", default_value = "aleph-node")]
        name: String,
        /// Operator label for tag-based fan-out (repeatable), e.g.
        /// `--tag gpu --tag region=us`. Stored verbatim; shown in
        /// `environments.list` and used by `node_invoke_many`.
        #[arg(long = "tag", value_name = "TAG")]
        tags: Vec<String>,
    },
```

- [ ] **Step 4: Thread `tags` through dispatch** — in `src/bin/aleph-server/main.rs:250-256`:

```rust
        Some(Command::Node {
            center,
            token,
            name,
            tags,
        }) => {
            return commands::node::handle_node(center, token, name, tags).await;
        }
```

- [ ] **Step 5: Accept `tags` in `handle_node` and put them on the connect frame** — in `src/bin/aleph-server/commands/node.rs`:

Change the signature (line 111):

```rust
pub async fn handle_node(
    center: String,
    token: Option<String>,
    name: String,
    tags: Vec<String>,
) -> Result<(), Box<dyn std::error::Error>> {
```

Thread `tags` into both `run_session` call sites (lines 140 and the loop's reconnect at the same call) by adding a `&tags` argument. Change `run_session`'s signature (line 264) and its connect-frame params (line 277):

```rust
async fn run_session(
    url: &str,
    token: &str,
    name: &str,
    declared: &[CommandDescriptor],
    tags: &[String],
    table: &Arc<CommandTable>,
    approval_slot: &ApprovalSlot,
) -> Result<SessionOutcome, Box<dyn std::error::Error>> {
```

```rust
    let connect = json!({
        "jsonrpc": "2.0", "id": 1, "method": "connect",
        "params": { "token": token, "device_name": name, "commands": declared, "tags": tags }
    });
```

And update the single `run_session(...)` call in `handle_node` (line 140) to pass `&tags`:

```rust
        match run_session(&url, &bearer, &name, &declared, &tags, &table, &approval_slot).await {
```

(Do NOT touch `run_pairing` or `NodeCredential` — tags flow via the connect frame only, by design; see the plan header.)

- [ ] **Step 6: Commit**

```bash
git add src/bin/aleph-server/cli.rs src/bin/aleph-server/main.rs src/bin/aleph-server/commands/node.rs
git commit -m "node: --tag flag carried on the connect frame for fan-out selection"
```

---

### Task 5: Document tags + node_invoke_many in CLUSTER.md

**Files:**
- Modify: `docs/reference/CLUSTER.md`

- [ ] **Step 1: Update the module map row + tools section** — in `docs/reference/CLUSTER.md`:

In the `## 模块版图` table, update the `node_invoke.rs` row and add a `node_invoke_many.rs` row:

```markdown
| `src/builtin_tools/node_invoke.rs` | 中心侧 **LLM 工具**:在单个节点上跑命令 | `NodeInvokeTool` |
| `src/builtin_tools/node_invoke_many.rs` | 中心侧 **LLM 工具**:按标签把命令并发扇出到一组节点 | `NodeInvokeManyTool` / `invoke_one` |
```

Under `## 中心侧 LLM 工具`, after the `node_invoke` subsection, add:

```markdown
### `node_invoke_many` — 按标签并发扇出

```jsonc
{ "tags": ["gpu"],            // AND 匹配:节点须含全部 tag;[] = 所有在线节点
  "command": "bash",          // 每个命中节点都要声明该命令(否则该节点单独报错)
  "args": { "cmd": "nvidia-smi -L" },
  "timeout_ms": 120000 }      // 每节点独立超时
```

经 `NodeRegistry::resolve_all_by_tags`(AND 语义)取命中集合,用 `tokio::task::JoinSet`
**并发**下发 `tool.call`——墙钟 = 最慢单节点。**容忍部分失败**:逐节点 fail-fast
(节点声明非空命令目录却不含该命令 → 该节点错,其余照跑),返回聚合
`{ invoked, succeeded, failed, results:[{node,node_id,ok,(result|error)}] }`。
**零命中报错**并附"available tags: …"提示(镜像 `resolve` 的 fail-fast 风格)。
标签纯用于选择,不构成授权层(R7);命令执行权威仍是节点侧 `CommandTable` allowlist。
```

In the `## 节点接入` → `### 2. 拨出` block, add the `--tag` flag to the example:

```markdown
aleph-server node \
  --center ws://<center-host>:18790 \
  --token  <token-from-enroll> \
  --name   <node-name> \
  --tag    gpu --tag region=us      # 可重复;经 connect 帧上报,供 node_invoke_many 选择
```

Add a sentence noting tags are CLI-supplied each run (carried on the `connect` frame,
surfaced in `environments.list`), not persisted in the credential.

In the `## 线协议速查` table, update the `connect` row:

```markdown
| node → center | `connect { token, device_name, commands, tags }` |
```

- [ ] **Step 2: Commit**

```bash
git add docs/reference/CLUSTER.md
git commit -m "docs: cluster tags + node_invoke_many in CLUSTER.md"
```

---

## Self-Review

**Spec coverage:**
- A (node tags): Task 1 (NodeSession/Environment field + parse), Task 4 (CLI → connect frame), Task 5 (docs). ✓
- B (resolve_all_by_tags + fan-out): Task 1 (`resolve_all_by_tags`/`NodeMatch`), Task 2 (`node_invoke_many`), Task 3 (registration). ✓
- AND semantics + zero-match error: Task 1 test `resolve_all_by_tags_and_semantics`, Task 2 test `zero_match_errors_with_available_tags_hint`. ✓
- Concurrency (JoinSet): Task 2. ✓
- Per-node fail-fast + partial-failure tolerance: Task 2 tests. ✓
- environments.list surfaces tags: Task 1 Step 4. ✓
- Spec deviations (no credential persist, no pairing.start_node): flagged in header; Task 4 Step 5 explicitly says do not touch run_pairing/NodeCredential. ✓

**Type consistency:** `NodeMatch{node_id,name,channel,declared_commands,tags}` defined in Task 1, consumed in Task 2 `invoke_one`/`call`. `NodeInvokeManyArgs{tags,command,args,timeout_ms}` consistent across Task 2 and Task 3 schema. `resolve_all_by_tags(&[String]) -> Vec<NodeMatch>` signature identical at definition (Task 1) and all call sites (Task 2). `handle_node(center, token, name, tags)` consistent between main.rs (Task 4 Step 4) and node.rs (Task 4 Step 5).

**Placeholder scan:** No TBD/TODO; every code step shows complete code. The only "find the sibling pattern" notes (mod declaration visibility in Task 2 Step 3; test constructor form in Task 4 Step 1) point at exact existing line numbers to copy, not vague instructions.

**Protocol note:** All `cargo` commands are marked `(DEFERRED — do not run)` per the project's no-check constraint; tests are authored for correctness and committed, to be run later by the user/CI.
