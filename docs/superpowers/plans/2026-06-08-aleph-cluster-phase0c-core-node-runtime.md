# Phase 0c-core Implementation Plan: Node Runtime + node_invoke + Allowlist

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task.
> Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A thin `aleph-server node` process dials out to the center, serves
reverse-RPC `tool.call`, and runs `bash` in a local sandbox; a center-side
`node_invoke` builtin tool lets the LLM drive it.

**Architecture:** Node = pure execution arm (no DB/harness/LLM). Center-side
`node_invoke` resolves a node via 0b `NodeRegistry`, fail-fast-checks the
declared command, then calls the node over the 0a `ReverseRpcChannel`. The node
runs a generic dispatch table (allowlist = table keys) that delegates `bash` to
`BashExecTool` inside a `SESSION_ID`-scoped local `WorkspaceSandbox`.

**Tech Stack:** Rust, tokio, tokio-tungstenite 0.26, serde_json. Reuses 0a
`reverse_rpc.rs`, 0b `registry.rs`, `BashExecTool`, the sandbox factory.

**Spec:** `docs/superpowers/specs/2026-06-08-aleph-cluster-phase0c-core-node-runtime.md`

**Base / branch:** NEW worktree + branch `feat/cluster-phase0c-core` cut from
**`main`** — a separate session already merged 0a+0b into main (`--no-ff` merge
`0d30b250b`), so main now carries the 0a/0b types 0c depends on (`src/cluster/`,
`src/gateway/handlers/cluster.rs`). Branch from current main HEAD. Do NOT touch
`src/harness/` (R10). Commits append-only; stage explicit paths. The old
`Aleph-wt-cluster-phase0a` worktree is cleaned up by that other session — do not
touch it.

---

## File Structure

| File | Responsibility | New/Mod |
|------|----------------|---------|
| `src/cluster/node_runtime.rs` | Node-side dispatch: `NodeCommand` trait, `CommandTable` (allowlist=keys), `dispatch`, `BashNodeCommand` | NEW |
| `src/cluster/registry.rs` | Add `NodeRegistry::resolve(name_or_id)` (name|id → channel + declared cmds) | Mod |
| `src/cluster/mod.rs` | `mod node_runtime;` + `pub use` | Mod |
| `src/builtin_tools/node_invoke.rs` | Center-side `NodeInvokeTool` (LLM tool) | NEW |
| `src/builtin_tools/mod.rs` | export `NodeInvokeTool` | Mod |
| `src/executor/builtin_registry/registry.rs` | `node_registry` OnceCell field + `set_node_registry` + `node_invoke` dispatch arm + metadata | Mod |
| boot wiring (next to `set_memory_context_provider` call) | `set_node_registry(server.node_registry.clone())` | Mod |
| `src/bin/aleph-server/cli.rs` | `Node { center, token, name }` subcommand | Mod |
| `src/bin/aleph-server/commands/node.rs` | Node dial-out loop + handshake + inbound dispatch + reconnect backoff | NEW |
| `src/bin/aleph-server/commands/mod.rs` | `pub mod node;` | Mod |
| `src/bin/aleph-server/main.rs` | `Command::Node` async match arm | Mod |
| `tests/cluster_node_runtime.rs` | Integration: node dials center, center calls tool.call, node runs bash | NEW |

**NOT needed:** `ConnectParams` is NOT `deny_unknown_fields`, so the node's
`commands` connect-param flows straight to `maybe_register_node` (reads raw
`req.params["commands"]`); no struct change.

---

## Task 1: Node-side dispatch table (`node_runtime.rs`)

**Files:**
- Create: `src/cluster/node_runtime.rs`
- Modify: `src/cluster/mod.rs`

- [ ] **Step 1: Write the failing test**

Create `src/cluster/node_runtime.rs` with the test module first:

```rust
//! 节点侧反向 RPC 分发（执行臂）。
//!
//! 收到中心发来的 `tool.call` 请求 → 查命令表（allowlist = 表的 keys，节点侧
//! 权威闸门）→ 命中则跑该命令 → 回 `Result<Value, String>`（节点 loop 据此
//! 构造带 id 的 JsonRpcResponse）。
//!
//! 红线：确定性查表，无 LLM 推理（R7）；不进 `src/harness/`（R10）。

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::cluster::CommandDescriptor;

/// 节点可执行的一个命令。`run` 返回 `Ok(payload)` 或 `Err(message)`。
#[async_trait]
pub trait NodeCommand: Send + Sync {
    async fn run(&self, args: Value) -> Result<Value, String>;
    fn descriptor(&self) -> CommandDescriptor;
}

/// 节点命令表。keys 即 allowlist（节点侧权威）。
#[derive(Default)]
pub struct CommandTable {
    commands: HashMap<String, Arc<dyn NodeCommand>>,
}

impl CommandTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, name: impl Into<String>, cmd: Arc<dyn NodeCommand>) {
        self.commands.insert(name.into(), cmd);
    }

    /// 节点 connect 时声明给中心的命令目录。
    pub fn descriptors(&self) -> Vec<CommandDescriptor> {
        self.commands.values().map(|c| c.descriptor()).collect()
    }

    /// 分发一帧反向 RPC 请求体。`method` 必须是 `"tool.call"`；`params` 形如
    /// `{"tool": "<name>", "args": {...}}`。allowlist 权威：tool 不在表中即拒，
    /// 无论中心发什么。返回 `Ok(payload)` / `Err(message)`，由调用方包成
    /// 带 id 的响应。
    pub async fn dispatch(&self, method: &str, params: &Value) -> Result<Value, String> {
        if method != "tool.call" {
            return Err(format!("unknown method '{method}' (expected tool.call)"));
        }
        let tool = params
            .get("tool")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "tool.call: missing string field `tool`".to_string())?;
        let Some(cmd) = self.commands.get(tool) else {
            return Err(format!("command '{tool}' not permitted on this node"));
        };
        let args = params.get("args").cloned().unwrap_or(Value::Null);
        cmd.run(args).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct EchoCmd;

    #[async_trait]
    impl NodeCommand for EchoCmd {
        async fn run(&self, args: Value) -> Result<Value, String> {
            if args.get("boom").is_some() {
                return Err("echo: boom".to_string());
            }
            Ok(json!({"echoed": args}))
        }
        fn descriptor(&self) -> CommandDescriptor {
            CommandDescriptor { name: "echo".to_string(), schema: json!({"type": "object"}) }
        }
    }

    fn table() -> CommandTable {
        let mut t = CommandTable::new();
        t.register("echo", Arc::new(EchoCmd));
        t
    }

    #[tokio::test]
    async fn dispatch_runs_registered_command() {
        let out = table()
            .dispatch("tool.call", &json!({"tool": "echo", "args": {"x": 1}}))
            .await
            .expect("registered command runs");
        assert_eq!(out["echoed"]["x"], 1);
    }

    #[tokio::test]
    async fn dispatch_rejects_unlisted_command() {
        let err = table()
            .dispatch("tool.call", &json!({"tool": "rm", "args": {}}))
            .await
            .expect_err("allowlist denies");
        assert!(err.contains("not permitted"), "{err}");
    }

    #[tokio::test]
    async fn dispatch_rejects_unknown_method() {
        let err = table()
            .dispatch("evil.method", &json!({"tool": "echo"}))
            .await
            .expect_err("only tool.call");
        assert!(err.contains("unknown method"), "{err}");
    }

    #[tokio::test]
    async fn dispatch_passes_through_command_error() {
        let err = table()
            .dispatch("tool.call", &json!({"tool": "echo", "args": {"boom": true}}))
            .await
            .expect_err("command error surfaces");
        assert!(err.contains("boom"), "{err}");
    }

    #[test]
    fn descriptors_list_registered_commands() {
        let d = table().descriptors();
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].name, "echo");
    }
}
```

In `src/cluster/mod.rs`, add the module + re-exports (place `mod node_runtime;`
after `mod registry;`; extend the existing `pub use`):

```rust
mod node_runtime;
pub use node_runtime::{CommandTable, NodeCommand};
```

- [ ] **Step 2: Run test to verify it fails (then passes — code is included above)**

Run: `cargo test -p alephcore --lib cluster::node_runtime 2>&1 | tail -20`
Expected: the module compiles and all 5 tests PASS (the impl is written alongside
the tests in Step 1; this task is a single cohesive unit).

- [ ] **Step 3: Commit**

```bash
git add src/cluster/node_runtime.rs src/cluster/mod.rs
git commit -m "cluster: node-side dispatch table (allowlist=keys) for reverse-RPC tool.call"
```

---

## Task 2: bash NodeCommand (`BashNodeCommand`)

**Files:**
- Modify: `src/cluster/node_runtime.rs` (add `BashNodeCommand` + `CommandTable::with_bash`)
- Test: same file's test module

**Context:** `BashExecTool::call_json(Value) -> Result<Value>` (default trait method)
runs bash, but reads `crate::sandbox::context::SESSION_ID` (a task-local) to pick
the per-session workspace. So `BashNodeCommand` holds a fixed `SessionKey` and
wraps each call in `SESSION_ID.scope(key, fut).await`. A fixed key per node gives
a stable workspace across calls (files persist between `node_invoke`s — a sensible
"machine" model). `BashExecTool` needs a sandbox attached via `.with_sandbox(...)`.

- [ ] **Step 1: Write the failing test**

Add to `node_runtime.rs` (above the `#[cfg(test)]` module), the bash command:

```rust
use crate::builtin_tools::BashExecTool;
use crate::routing::session_key::SessionKey;
use crate::sandbox::context::SESSION_ID;

/// `bash` 作为节点命令：在固定 session 作用域下委托 `BashExecTool`。
pub struct BashNodeCommand {
    bash: BashExecTool,
    session: SessionKey,
}

impl BashNodeCommand {
    pub fn new(bash: BashExecTool, session: SessionKey) -> Self {
        Self { bash, session }
    }
}

#[async_trait]
impl NodeCommand for BashNodeCommand {
    async fn run(&self, args: Value) -> Result<Value, String> {
        SESSION_ID
            .scope(self.session.clone(), self.bash.call_json(args))
            .await
            .map_err(|e| e.to_string())
    }
    fn descriptor(&self) -> CommandDescriptor {
        CommandDescriptor {
            name: "bash".to_string(),
            schema: serde_json::json!({"type": "object"}),
        }
    }
}

impl CommandTable {
    /// 便捷构造：注册唯一的 `bash` 命令（0c 节点的全部能力）。
    pub fn with_bash(bash: BashExecTool, session: SessionKey) -> Self {
        let mut t = Self::new();
        t.register("bash", Arc::new(BashNodeCommand::new(bash, session)));
        t
    }
}
```

Add this test to the `#[cfg(test)] mod tests`:

```rust
    #[tokio::test]
    async fn bash_command_runs_under_sandbox() {
        use crate::sandbox::test_util::MockSandbox;
        let sandbox = MockSandbox::new();
        let bash = BashExecTool::new().with_sandbox(sandbox);
        let session = SessionKey::ephemeral("node-test");
        let table = CommandTable::with_bash(bash, session);

        let out = table
            .dispatch("tool.call", &json!({"tool": "bash", "args": {"cmd": "echo hi"}}))
            .await
            .expect("bash runs under sandbox");
        // MockSandbox returns a structured CodeExecOutput; assert the envelope shape.
        assert!(out.get("exit_code").is_some(), "bash output envelope: {out}");

        // allowlist still authoritative: bash table denies a non-bash tool.
        let err = table
            .dispatch("tool.call", &json!({"tool": "python", "args": {}}))
            .await
            .expect_err("only bash permitted");
        assert!(err.contains("not permitted"), "{err}");
    }
```

> **Implementer note:** Verify `crate::sandbox::test_util::MockSandbox`'s exact
> constructor (the memory of prior work says `MockSandbox::new() -> Arc<Self>`,
> mirrored from `code_exec.rs` tests). If `MockSandbox` records calls but does not
> actually run `echo`, assert on the envelope (`exit_code`/`success` present) rather
> than the literal `"hi"` stdout — the point is that bash dispatch reaches the
> sandbox, not that the mock executes. Match how `code_exec.rs`/`bash_exec.rs`
> tests use `MockSandbox`.

- [ ] **Step 2: Run test**

Run: `cargo test -p alephcore --lib cluster::node_runtime 2>&1 | tail -20`
Expected: all tests PASS (6 now).

- [ ] **Step 3: Commit**

```bash
git add src/cluster/node_runtime.rs
git commit -m "cluster: bash node command (sandbox-scoped) + CommandTable::with_bash"
```

---

## Task 3: `NodeRegistry::resolve` (name-or-id → channel + declared cmds)

**Files:**
- Modify: `src/cluster/registry.rs`
- Test: same file's test module

**Context:** `node_invoke` addresses a node by name OR id and needs both the
channel (to call) and `declared_commands` (for center-side fail-fast). The existing
`get(node_id)` returns only the channel by id. Add `resolve`.

- [ ] **Step 1: Write the failing test**

Add to `registry.rs`'s `impl NodeRegistry` (after `get`):

```rust
    /// 按 name 或 id 解析一个在线节点，返回其反向 RPC 通道 + 声明的命令目录。
    /// 先按 node_id 精确命中，再按 device_name 命中。`node_invoke` 用它寻址 +
    /// fail-fast 校验。
    pub fn resolve(&self, name_or_id: &str) -> Option<(ReverseRpcChannel, Vec<CommandDescriptor>)> {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        if let Some(s) = inner.nodes_by_id.get(name_or_id) {
            return Some((s.channel.clone(), s.declared_commands.clone()));
        }
        inner
            .nodes_by_id
            .values()
            .find(|s| s.device_name == name_or_id)
            .map(|s| (s.channel.clone(), s.declared_commands.clone()))
    }
```

Add to `registry.rs`'s `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn resolve_by_id_then_by_name() {
        let reg = NodeRegistry::new();
        reg.register(session("node-a", "conn-1")); // device_name = "dev-node-a"
        assert!(reg.resolve("node-a").is_some(), "by id");
        let (_, cmds) = reg.resolve("dev-node-a").expect("by name");
        assert_eq!(cmds[0].name, "bash");
        assert!(reg.resolve("nope").is_none());
    }
```

- [ ] **Step 2: Run test**

Run: `cargo test -p alephcore --lib cluster::registry 2>&1 | tail -15`
Expected: PASS (existing 6 + new 1).

- [ ] **Step 3: Commit**

```bash
git add src/cluster/registry.rs
git commit -m "cluster: NodeRegistry::resolve (name|id -> channel + declared commands)"
```

---

## Task 4: Center-side `node_invoke` tool (`node_invoke.rs`)

**Files:**
- Create: `src/builtin_tools/node_invoke.rs`
- Modify: `src/builtin_tools/mod.rs`

**Context:** `AlephTool` (`src/tools/traits.rs`) requires `NAME`, `DESCRIPTION`,
`Args: Deserialize+JsonSchema`, `Output: Serialize`, `async fn call`. `call_json`
is default-provided. The tool holds `Arc<NodeRegistry>`, resolves the node,
fail-fast-checks the command against `declared_commands` (only when declared is
non-empty — empty = let node authority decide), then calls
`channel.call("tool.call", {tool, args}, timeout)`.

- [ ] **Step 1: Write the failing test**

Create `src/builtin_tools/node_invoke.rs`:

```rust
//! `node_invoke`：中心侧 LLM 工具，向一个已连节点下发命令（经 0a 反向 RPC）。
//!
//! 寻址按 name 或 id；下发前 fail-fast 校验节点声明的命令（节点侧仍权威）。
//! 红线：纯 I/O 翻译（R4），无推理（R7）；命令选择由 LLM 做。

use crate::sync_primitives::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::cluster::NodeRegistry;
use crate::error::{AlephError, Result};
use crate::tools::AlephTool;

const DEFAULT_TIMEOUT_MS: u64 = 120_000;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct NodeInvokeArgs {
    /// Target node: its name (e.g. "worker-1") or id. Use `environments.list`
    /// to see online nodes and the commands each declares.
    pub node: String,
    /// Command to run on the node (e.g. "bash"). Must be one the node declares.
    pub command: String,
    /// JSON arguments for the command, passed through to the node verbatim
    /// (for "bash", e.g. {"cmd": "ls -la"}).
    #[serde(default)]
    pub args: Value,
    /// Reverse-RPC timeout in ms (default 120000). Must exceed the node-side
    /// command's own runtime or the channel times out while it still runs.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Clone)]
pub struct NodeInvokeTool {
    node_registry: Arc<NodeRegistry>,
}

impl NodeInvokeTool {
    pub fn new(node_registry: Arc<NodeRegistry>) -> Self {
        Self { node_registry }
    }
}

#[async_trait]
impl AlephTool for NodeInvokeTool {
    const NAME: &'static str = "node_invoke";
    const DESCRIPTION: &'static str = r#"Run a command on a connected cluster node (a remote execution arm).

Address the node by its name or id (see `environments.list` for online nodes and
the commands each declares). `command` must be one the node permits (e.g. "bash");
`args` is that command's JSON payload, passed through verbatim — for bash:
{"node": "worker-1", "command": "bash", "args": {"cmd": "uname -a"}}.

The node runs it in ITS OWN sandboxed workspace and returns the result. Set
`timeout_ms` (default 120000) above the expected runtime for long commands. If the
node is offline or the command isn't permitted, you get a clear error."#;

    type Args = NodeInvokeArgs;
    type Output = Value;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let Some((channel, declared)) = self.node_registry.resolve(&args.node) else {
            return Err(AlephError::tool(format!("node '{}' not online", args.node)));
        };
        // Center-side fail-fast: only reject when the node declared a non-empty
        // catalog that excludes this command. Empty catalog → defer to node authority.
        if !declared.is_empty() && !declared.iter().any(|c| c.name == args.command) {
            return Err(AlephError::tool(format!(
                "command '{}' not declared by node '{}'",
                args.command, args.node
            )));
        }
        let timeout = args.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS);
        let params = json!({ "tool": args.command, "args": args.args });
        match channel.call("tool.call", params, timeout).await {
            Ok(resp) if resp.is_success() => Ok(resp.result.unwrap_or(Value::Null)),
            Ok(resp) => Err(AlephError::tool(format!(
                "node '{}' returned error: {}",
                args.node,
                resp.error
                    .map(|e| e.message)
                    .unwrap_or_else(|| "unknown".to_string())
            ))),
            Err(e) => Err(AlephError::tool(format!(
                "node '{}' reverse-rpc failed: {e}",
                args.node
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster::{CommandDescriptor, NodeRegistry, NodeSession, ReverseRpcChannel};
    use crate::gateway::protocol::JsonRpcResponse;
    use tokio::sync::mpsc;

    /// 建一个登记好的节点会话，并返回中心可读的 channel + 后台"节点应答器"的
    /// 出站接收端（扮演节点：收到 tool.call 帧就 resolve 回一条成功响应）。
    fn registry_with_node(
        node_id: &str,
        name: &str,
        commands: Vec<&str>,
    ) -> (Arc<NodeRegistry>, mpsc::Receiver<String>, ReverseRpcChannel) {
        let (tx, rx) = mpsc::channel::<String>(8);
        let channel = ReverseRpcChannel::new(tx);
        let reg = Arc::new(NodeRegistry::new());
        reg.register(NodeSession {
            node_id: node_id.to_string(),
            conn_id: "conn-1".to_string(),
            device_name: name.to_string(),
            channel: channel.clone(),
            declared_commands: commands
                .into_iter()
                .map(|c| CommandDescriptor { name: c.to_string(), schema: json!({}) })
                .collect(),
            connected_at: 1,
        });
        (reg, rx, channel)
    }

    /// 后台扮节点：读出一帧请求 → 回成功响应（回显 tool）。
    fn spawn_node_responder(mut rx: mpsc::Receiver<String>, channel: ReverseRpcChannel) {
        let pending = channel.pending();
        tokio::spawn(async move {
            if let Some(frame) = rx.recv().await {
                let req: Value = serde_json::from_str(&frame).unwrap();
                let id = req["id"].clone();
                let resp = JsonRpcResponse::success(
                    Some(id.clone()),
                    json!({"ran": req["params"]["tool"]}),
                );
                pending.resolve(&id, resp);
            }
        });
    }

    #[tokio::test]
    async fn invokes_node_by_name_and_returns_result() {
        let (reg, rx, ch) = registry_with_node("n-1", "worker-1", vec!["bash"]);
        spawn_node_responder(rx, ch);
        let tool = NodeInvokeTool::new(reg);
        let out = tool
            .call(NodeInvokeArgs {
                node: "worker-1".to_string(),
                command: "bash".to_string(),
                args: json!({"cmd": "echo hi"}),
                timeout_ms: Some(2_000),
            })
            .await
            .expect("invoke resolves");
        assert_eq!(out["ran"], "bash");
    }

    #[tokio::test]
    async fn invokes_node_by_id() {
        let (reg, rx, ch) = registry_with_node("n-1", "worker-1", vec!["bash"]);
        spawn_node_responder(rx, ch);
        let tool = NodeInvokeTool::new(reg);
        let out = tool
            .call(NodeInvokeArgs {
                node: "n-1".to_string(),
                command: "bash".to_string(),
                args: json!({}),
                timeout_ms: Some(2_000),
            })
            .await
            .expect("invoke by id resolves");
        assert_eq!(out["ran"], "bash");
    }

    #[tokio::test]
    async fn offline_node_is_clear_error() {
        let reg = Arc::new(NodeRegistry::new());
        let tool = NodeInvokeTool::new(reg);
        let err = tool
            .call(NodeInvokeArgs {
                node: "ghost".to_string(),
                command: "bash".to_string(),
                args: json!({}),
                timeout_ms: Some(500),
            })
            .await
            .expect_err("offline node errors");
        assert!(err.to_string().contains("not online"), "{err}");
    }

    #[tokio::test]
    async fn fail_fast_rejects_undeclared_command() {
        // Node declares only "bash"; ask for "python" → reject without dialing.
        let (reg, _rx, _ch) = registry_with_node("n-1", "worker-1", vec!["bash"]);
        let tool = NodeInvokeTool::new(reg);
        let err = tool
            .call(NodeInvokeArgs {
                node: "worker-1".to_string(),
                command: "python".to_string(),
                args: json!({}),
                timeout_ms: Some(500),
            })
            .await
            .expect_err("undeclared command fails fast");
        assert!(err.to_string().contains("not declared"), "{err}");
    }

    #[tokio::test]
    async fn timeout_is_surfaced() {
        // Node responder never replies → channel times out.
        let (reg, _rx, _ch) = registry_with_node("n-1", "worker-1", vec!["bash"]);
        let tool = NodeInvokeTool::new(reg);
        let err = tool
            .call(NodeInvokeArgs {
                node: "worker-1".to_string(),
                command: "bash".to_string(),
                args: json!({}),
                timeout_ms: Some(50),
            })
            .await
            .expect_err("times out");
        assert!(err.to_string().contains("reverse-rpc failed"), "{err}");
    }
}
```

In `src/builtin_tools/mod.rs`, add the module + export (mirror an existing tool
line, e.g. `pub use bash_exec::BashExecTool;`):

```rust
pub mod node_invoke;
pub use node_invoke::{NodeInvokeArgs, NodeInvokeTool};
```

> **Implementer notes:**
> - Verify `JsonRpcResponse` field names: the test reads `resp.result: Option<Value>`,
>   `resp.error: Option<{message}>`, and method `resp.is_success()`. Confirm against
>   `src/gateway/protocol.rs` (the `error` field's inner type — adjust
>   `e.message` if the error struct names it differently). 0a's `reverse_rpc.rs`
>   uses `resp.is_success()` and `resp.result.unwrap()`, so those are correct.
> - Confirm `AlephError::tool(...)` exists (used widely in `registry.rs` dispatch,
>   e.g. the `remember` arm). If the constructor differs, match the codebase.
> - Confirm `crate::cluster::{NodeSession, ReverseRpcChannel}` are re-exported
>   (they are — see `src/cluster/mod.rs`).

- [ ] **Step 2: Run tests**

Run: `cargo test -p alephcore --lib node_invoke 2>&1 | tail -25`
Expected: 5 tests PASS.

- [ ] **Step 3: Commit**

```bash
git add src/builtin_tools/node_invoke.rs src/builtin_tools/mod.rs
git commit -m "builtin_tools: node_invoke (center-side LLM tool, reverse-RPC to node, fail-fast allowlist)"
```

---

## Task 5: Register `node_invoke` in `BuiltinToolRegistry`

**Files:**
- Modify: `src/executor/builtin_registry/registry.rs`
- Modify: boot wiring file that calls `set_memory_context_provider` (find via grep)

**Context:** Tools needing shared state use a `tokio::sync::OnceCell` field +
`set_*` setter, injected after the registry is `Arc`-wrapped (see
`memory_context_provider` at `registry.rs:149` / `set_memory_context_provider` at
`registry.rs:463` / `remember` dispatch arm `registry.rs:805`). Mirror exactly for
`node_registry`.

- [ ] **Step 1: Add the OnceCell field**

In `BuiltinToolRegistry` struct (near `memory_context_provider`, ~`registry.rs:149`):

```rust
    /// 集群节点登记表，启动后经 `set_node_registry` 注入；`node_invoke` 用它寻址。
    pub(crate) node_registry:
        Arc<tokio::sync::OnceCell<Arc<crate::cluster::NodeRegistry>>>,
```

Initialize it in the constructor (`builder/constructor.rs`, wherever
`memory_context_provider` field is initialized — grep for
`memory_context_provider:` in that file and add a sibling):

```rust
        node_registry: Arc::new(tokio::sync::OnceCell::new()),
```

- [ ] **Step 2: Add the setter + dispatch arm + metadata**

Setter, next to `set_memory_context_provider` (~`registry.rs:463`):

```rust
    /// 注入集群节点登记表，启用 `node_invoke` 工具。
    pub fn set_node_registry(&self, registry: Arc<crate::cluster::NodeRegistry>) {
        if self.node_registry.set(registry).is_ok() {
            tracing::info!("NodeRegistry injected — `node_invoke` tool now available");
        }
    }
```

Dispatch arm in `execute_tool`'s match (mirror the `"remember"` arm shape,
~`registry.rs:805`):

```rust
            "node_invoke" => Box::pin(async move {
                let reg = self.node_registry.get().ok_or_else(|| {
                    AlephError::tool("node_invoke not available: NodeRegistry not injected")
                })?;
                let tool = crate::builtin_tools::NodeInvokeTool::new(reg.clone());
                tool.call_json(arguments).await
            }),
```

Register tool metadata so the LLM sees it. Find where builtin tools add their
`UnifiedTool` metadata into `self.tools` (grep for how `"bash"` registers its
schema/description in the registry; e.g. a `register_*` helper or a `tools.insert`).
Add `node_invoke` the same way, using
`<crate::builtin_tools::NodeInvokeTool as AlephTool>::NAME` / `DESCRIPTION` and the
schema from `NodeInvokeArgs` (schemars). Mirror the exact registration call the
neighbouring tools use.

> **Implementer note:** The metadata-registration mechanism varies; do NOT invent
> one. Locate how a simple existing tool (e.g. `bash` or `web_fetch`) gets into the
> `tools: HashMap<String, UnifiedTool>` and replicate it for `node_invoke`. The
> tool's JSON schema comes from `schemars::schema_for!(NodeInvokeArgs)` if the
> codebase derives schemas that way — match the existing pattern.

- [ ] **Step 3: Wire the setter at boot**

Grep for the production call site of `set_memory_context_provider`:

```bash
grep -rn "set_memory_context_provider" src/bin/aleph-server src/executor 2>/dev/null
```

At that same site, both the tool-registry `Arc` and the gateway server (which owns
`node_registry`) are in scope. Add right after it:

```rust
    builtin_registry.set_node_registry(server.node_registry.clone());
```

> **Implementer note:** Match the actual variable names at that site (the registry
> handle may be named `registry` / `builtin_tools` / etc., and the server handle
> may differ). If `server.node_registry` is not directly in scope there, thread the
> `Arc<NodeRegistry>` clone from where `GatewayServer` is built to this call (it is
> the same Arc 0b shares into AuthContext via `initialize_auth`).

- [ ] **Step 4: Build + smoke test**

Run: `cargo build -p alephcore --bin aleph-server 2>&1 | tail -20`
Expected: clean build.
Run: `cargo test -p alephcore --lib builtin_registry 2>&1 | tail -15`
Expected: existing registry tests still PASS.

- [ ] **Step 5: Commit**

```bash
git add src/executor/builtin_registry/registry.rs src/executor/builtin_registry/builder/constructor.rs <boot-wiring-file>
git commit -m "builtin_registry: register node_invoke tool + inject NodeRegistry at boot"
```

---

## Task 6: `aleph-server node` subcommand (dial-out runtime)

**Files:**
- Modify: `src/bin/aleph-server/cli.rs`
- Create: `src/bin/aleph-server/commands/node.rs`
- Modify: `src/bin/aleph-server/commands/mod.rs`
- Modify: `src/bin/aleph-server/main.rs`

**Context:** Mirror two templates: `commands/sandbox_debug.rs` (standalone sandbox
construction) and `tests/cluster_reverse_rpc.rs` (WS client loop with
`tokio_tungstenite::connect_async`, `futures_util::{SinkExt, StreamExt}`,
`Message::Text`). The node: build sandbox → build `CommandTable::with_bash` →
reconnect loop {connect_async → send connect handshake (token + declared commands)
→ read connect resp → inbound loop dispatching `tool.call` → on disconnect, backoff
and redial}.

- [ ] **Step 1: Add the clap subcommand**

In `src/bin/aleph-server/cli.rs`'s `enum Command` (mirror `SandboxDebug`'s
`#[arg(long)]` style, ~`cli.rs:181`):

```rust
    /// Run as a cluster node: dial out to a center, serve reverse-RPC tool.call,
    /// run bash in a LOCAL sandbox. Pure execution arm (no DB/LLM).
    Node {
        /// Center WebSocket base URL, e.g. ws://127.0.0.1:18790
        #[arg(long, value_name = "URL")]
        center: String,
        /// Node auth token (minted via center `cluster.enroll`).
        #[arg(long, value_name = "TOKEN", env = "ALEPH_NODE_TOKEN")]
        token: String,
        /// Human-readable node name shown in `environments.list`.
        #[arg(long, value_name = "NAME", default_value = "aleph-node")]
        name: String,
    },
```

- [ ] **Step 2: Write the node runtime (with a unit-testable frame handler)**

Create `src/bin/aleph-server/commands/node.rs`. Keep the frame-handling logic in a
pure async helper `handle_frame` so it's unit-testable without a live socket:

```rust
//! `aleph-server node` —— 集群节点（执行臂）拨出运行时。
//!
//! 拨向中心 WS、用 node-token 认证、声明命令、入站循环服务 `tool.call`，
//! 在本机 sandbox 跑 bash。断线指数退避重连。无 DB / 无 harness / 无 LLM。

use std::time::Duration;

use alephcore::cluster::CommandTable;
use alephcore::gateway::protocol::JsonRpcResponse;
use alephcore::routing::session_key::SessionKey;
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
    let table = build_command_table(&name);
    let declared = table.descriptors();
    let url = format!("{}/ws", center.trim_end_matches('/'));
    let table = std::sync::Arc::new(table);

    let mut backoff = BACKOFF_INITIAL_MS;
    loop {
        match run_session(&url, &token, &name, &declared, &table).await {
            Ok(()) => {
                tracing::warn!("node session ended cleanly; reconnecting");
                backoff = BACKOFF_INITIAL_MS;
            }
            Err(e) => {
                tracing::error!("node session error: {e}; retrying in {backoff}ms");
            }
        }
        tokio::time::sleep(Duration::from_millis(backoff)).await;
        backoff = (backoff * 2).min(BACKOFF_MAX_MS);
    }
}

/// 建节点 sandbox（镜像 sandbox_debug.rs，生产式 `None` 审批 gate=headless 安全：
/// 转义自动拒，普通 bash 照跑）+ 唯一 bash 命令。
fn build_command_table(name: &str) -> CommandTable {
    use alephcore::sandbox::factory::build_sandbox;
    use alephcore::sandbox::platforms::create_platform_driver_from_config;
    use alephcore::sandbox::rate_limit::SandboxRateLimitConfig;
    use alephcore::config::types::SandboxConfig;
    use alephcore::gateway::security::approval::{ApprovalConfig, ApprovalGate};

    let cfg = SandboxConfig::default();
    let driver = create_platform_driver_from_config(&cfg);
    let gate = std::sync::Arc::new(ApprovalGate::new(ApprovalConfig::default(), None));
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

/// 一次连接会话：连 → 握手 → 入站循环。连接断开即返回。
async fn run_session(
    url: &str,
    token: &str,
    name: &str,
    declared: &[alephcore::cluster::CommandDescriptor],
    table: &CommandTable,
) -> Result<(), Box<dyn std::error::Error>> {
    let (mut ws, _resp) = tokio_tungstenite::connect_async(url).await?;
    // connect 握手：带 token + device_name + 声明的命令。
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
/// 抽成纯函数以便单测。
async fn handle_frame(table: &CommandTable, text: &str) -> Option<String> {
    let v: Value = serde_json::from_str(text).ok()?;
    if v.get("method").and_then(|m| m.as_str()) != Some("tool.call") {
        return None; // 非请求 / 非 tool.call：忽略
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
    use alephcore::sandbox::test_util::MockSandbox;

    fn bash_table() -> CommandTable {
        let bash = alephcore::builtin_tools::BashExecTool::new().with_sandbox(MockSandbox::new());
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
        assert!(v["error"]["message"].as_str().unwrap().contains("not permitted"));
    }

    #[tokio::test]
    async fn handle_frame_ignores_non_tool_call() {
        let table = bash_table();
        let frame = json!({"jsonrpc": "2.0", "id": 1, "result": {"ok": true}}).to_string();
        assert!(handle_frame(&table, &frame).await.is_none());
    }
}
```

> **Implementer notes:**
> - Verify exact import paths by grepping: `SandboxConfig` (it lives where
>   `sandbox_debug.rs` imports it — copy that file's `use` lines verbatim for
>   `SandboxConfig`, `ApprovalConfig`, `ApprovalGate`, `create_platform_driver_from_config`,
>   `build_sandbox`, `SandboxRateLimitConfig`, `ShellSecurityConfig`). The paths in
>   the code above are best-effort; `sandbox_debug.rs:35-37,55` is the ground truth.
> - Confirm `futures_util` is available to the binary crate. `tests/cluster_reverse_rpc.rs`
>   imports it; if it's a dev-only dependency, add `futures-util` to the
>   `[dependencies]` (it's already a transitive dep of the workspace). Verify with
>   `cargo build` in Step 4.
> - `Message::Text(...)` in tokio-tungstenite 0.26 takes a `Utf8Bytes`; the test
>   file uses `.into()` on a `String` — mirror that (`connect.to_string().into()`).

- [ ] **Step 3: Wire the module + dispatch**

In `src/bin/aleph-server/commands/mod.rs`: add `pub mod node;`.

In `src/bin/aleph-server/main.rs`'s `async_main` (the async-dispatch section,
~`main.rs:192-225`, alongside `Pairing`/`Plugins`), add:

```rust
        Some(Command::Node { center, token, name }) => {
            commands::node::handle_node(center, token, name).await
        }
```

> **Implementer note:** Match the surrounding arms' exact return-type adaptation
> (the other async arms return `Result<(), Box<dyn Error>>` or map into the
> function's result — copy the neighbouring arm's shape, e.g. how `SandboxDebug`
> is dispatched, including any `?`/`.map_err` wrapping).

- [ ] **Step 4: Build + unit test**

Run: `cargo build -p alephcore --bin aleph-server 2>&1 | tail -25`
Expected: clean build.
Run: `cargo test -p alephcore --bin aleph-server node:: 2>&1 | tail -15`
(If the bin's tests don't run under that filter, use
`cargo test -p alephcore --bin aleph-server 2>&1 | tail -20` and confirm the
three `handle_frame_*` tests pass.)
Expected: 3 frame tests PASS.

- [ ] **Step 5: Commit**

```bash
git add src/bin/aleph-server/cli.rs src/bin/aleph-server/commands/node.rs \
        src/bin/aleph-server/commands/mod.rs src/bin/aleph-server/main.rs
git commit -m "aleph-server: node subcommand (dial-out runtime, sandboxed bash, reconnect)"
```

---

## Task 7: Integration test — node dials center, runs bash over reverse-RPC

**Files:**
- Create: `tests/cluster_node_runtime.rs`

**Context:** Mirror `tests/cluster_reverse_rpc.rs` exactly (real `GatewayServer`
with `AuthMode::None` to isolate transport; a real `connect_async` client). Here
the client uses the NODE's dispatch logic: on `tool.call` it runs the bash
`CommandTable` and replies. The center drives a `tool.call` through the raw 0a
`reverse_rpc` channel and asserts it gets the node's bash result back. (The
`NodeRegistry`→`node_invoke` path is unit-tested in Task 4; this test exercises the
real socket + the node's frame handling + real bash dispatch.)

- [ ] **Step 1: Write the integration test**

Create `tests/cluster_node_runtime.rs`:

```rust
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
use alephcore::sandbox::test_util::MockSandbox;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::RwLock;
use tokio_tungstenite::tungstenite::Message;

type ReverseRpcRegistry = Arc<RwLock<HashMap<String, ReverseRpcChannel>>>;

#[tokio::test]
async fn center_runs_bash_on_connected_node() {
    let config = GatewayConfig { auth_mode: AuthMode::None, ..Default::default() };
    let dummy: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let server = GatewayServer::with_config(dummy, config);
    let reverse_rpc: ReverseRpcRegistry = server.reverse_rpc.clone();
    let router = server.build_router();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let bound = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>())
            .await
            .unwrap();
    });
    let _keepalive = &server;

    // 节点端：连接 + connect 握手 + 入站循环跑 bash CommandTable。
    let url = format!("ws://{bound}/ws");
    let (mut ws, _r) = tokio_tungstenite::connect_async(url.as_str()).await.unwrap();
    ws.send(Message::Text(
        json!({"jsonrpc":"2.0","id":1,"method":"connect",
               "params":{"device_name":"itest-node","device_id":"node-itest"}})
        .to_string().into(),
    )).await.unwrap();
    let _ = ws.next().await.expect("connect resp").unwrap();

    let bash = alephcore::builtin_tools::BashExecTool::new().with_sandbox(MockSandbox::new());
    let table = CommandTable::with_bash(bash, SessionKey::ephemeral("itest-node"));
    let node = tokio::spawn(async move {
        while let Some(Ok(Message::Text(text))) = ws.next().await {
            let v: Value = match serde_json::from_str(text.as_str()) { Ok(v) => v, Err(_) => continue };
            if v["method"] == "tool.call" {
                let id = v["id"].clone();
                let result = table.dispatch("tool.call", &v["params"]).await.unwrap();
                ws.send(Message::Text(
                    json!({"jsonrpc":"2.0","id":id,"result":result}).to_string().into(),
                )).await.unwrap();
                break;
            }
        }
    });

    let channel = wait_for_one_channel(&reverse_rpc).await;
    let resp = channel
        .call("tool.call", json!({"tool":"bash","args":{"cmd":"echo hi"}}), 5_000)
        .await
        .expect("reverse rpc resolves");
    assert!(resp.is_success());
    // MockSandbox returns a structured bash envelope; assert it round-tripped.
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
```

> **Implementer note:** If `MockSandbox` does not actually run `echo` (it records
> calls), the bash envelope still serializes with `exit_code`/`success` fields — the
> assertion targets the envelope shape, proving the full path (socket → node frame
> handling → CommandTable → bash → sandbox → reply → center) is connected. If
> `MockSandbox` is unavailable to integration tests (it's behind `test_util`,
> normally `#[cfg(test)]` within the lib — confirm it's exported for integration
> tests; if not, gate this test or build a tiny inline sandbox stub). Verify with
> the run below.

- [ ] **Step 2: Run the integration test**

Run: `cargo test -p alephcore --test cluster_node_runtime 2>&1 | tail -25`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add tests/cluster_node_runtime.rs
git commit -m "test: integration — center runs bash on connected node over reverse-RPC"
```

---

## Final verification (after all tasks)

- [ ] Cluster + tool lib tests:

```bash
cargo test -p alephcore --lib cluster:: 2>&1 | tail -25
cargo test -p alephcore --lib node_invoke 2>&1 | tail -10
cargo test -p alephcore --test cluster_node_runtime 2>&1 | tail -15
```
Expected: all PASS.

- [ ] Binary builds (node subcommand + node_invoke wiring compile end-to-end):

```bash
cargo build -p alephcore --bin aleph-server 2>&1 | tail -20
```
Expected: clean.

- [ ] Clippy on touched crate:

```bash
cargo clippy -p alephcore --lib 2>&1 | tail -25
```
Expected: no new warnings on cluster / builtin_tools / node code.

- [ ] **Manual smoke (optional, documents the happy path):** in one shell run the
  center daemon; enroll a node (`cluster.enroll` from 0b) to mint a token; in
  another shell `aleph-server node start --center ws://127.0.0.1:<port> --token <tok>
  --name worker-1`; from the center LLM call
  `node_invoke(node="worker-1", command="bash", args={"cmd":"uname -a"})` and
  confirm the remote uname returns. (Not a CI gate — real two-process run.)

---

## Notes & decisions baked in

- **No `ConnectParams` change:** it's not `deny_unknown_fields`; the node's
  `commands` connect-param reaches `maybe_register_node` via raw `req.params`.
- **Node escalation policy:** `ApprovalGate::new(default, None)` — headless-safe,
  mirrors production daemon (`start/mod.rs:262`); bash capability escalations
  (`allow_network`/`allow_subprocess`) are denied (no human approver on the node).
  Plain bash works. Routing approvals back to the center is a future refinement.
- **Fixed per-node session:** stable workspace across `node_invoke`s (files persist
  between calls — a "machine" model), via one `SessionKey::ephemeral` per process.
- **Center fail-fast is advisory:** node-side `CommandTable` keys are the
  authoritative allowlist; `node_invoke`'s `declared_commands` check only fails fast
  when the node declared a non-empty catalog excluding the command.
- **Pattern-mirroring:** Task 5 mirrors `set_memory_context_provider`; Task 6
  mirrors `sandbox_debug.rs` (sandbox build) + `cluster_reverse_rpc.rs` (WS loop);
  Task 7 mirrors `cluster_reverse_rpc.rs` wholesale. When unsure of an API, grep the
  named template and copy its exact shape.
```
