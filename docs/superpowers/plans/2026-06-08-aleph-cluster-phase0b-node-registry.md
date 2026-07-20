# Cluster Phase 0b — NodeRegistry + role:node enroll + environments.list — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the center-side node-registry layer — mint node tokens (`cluster.enroll`), register `role:node` connections into a `NodeRegistry`, and expose online nodes read-only via `environments.list`.

**Architecture:** Net-new `NodeRegistry` in `src/cluster/registry.rs` (consumes the Phase 0a `ReverseRpcChannel`). A `role:node` connection is recognized by a surgical change in `connect.rs` (token's `DeviceRole::Node` → response `role:"node"`), then registered by a pure testable glue helper called from `handler.rs`. `cluster.enroll` / `environments.list` are operator/read gateway RPCs (matching the existing `devices.*`/`pairing.*` pattern), reaching `token_manager`/`node_registry` via `AuthContext`. `src/harness/` is untouched (R10).

**Tech Stack:** Rust, tokio, axum WebSocket, JSON-RPC 2.0, `std::sync::RwLock`, serde/serde_json, existing `TokenManager`/`SecurityStore`/`AuthContext`/handler-registry.

**Spec:** `docs/superpowers/specs/2026-06-08-aleph-cluster-phase0b-node-registry.md`

**Worktree/branch:** Grow on the existing `feat/cluster-phase0a-reverse-rpc` worktree (`/Volumes/TBU4/Workspace/Aleph-wt-cluster-phase0a`). Do NOT `git worktree remove` in-session (shell-corruption hazard). Commit on this branch; do not merge to main this phase.

**Spec deviation already recorded:** enroll/list are gateway RPCs (not builtin tools) because no builtin tool holds a `TokenManager`; the LLM-facing tool surface ships with `node_invoke` in 0c. The full-stack WS smoke test is deferred to 0c (a bare `GatewayServer::with_config` has `token_manager: None`).

---

## File Structure

| File | Responsibility |
|---|---|
| `src/cluster/registry.rs` (new) | `CommandDescriptor`, `NodeSession`, `Environment`, `NodeRegistry` + `maybe_register_node` glue. Pure; no gateway deps beyond `crate::cluster::ReverseRpcChannel`. |
| `src/cluster/mod.rs` (modify) | Export the new registry types + glue. |
| `src/gateway/server/mod.rs` (modify) | Add `node_registry: Arc<NodeRegistry>` to `GatewaySharedState` + `GatewayServer`; share via `build_router`. |
| `src/gateway/server/probe.rs` (modify) | Add the new field to its `GatewaySharedState` construction. |
| `src/gateway/server/handler.rs` (modify) | `ConnectionContext.node_registry` field + populate; keep a `ReverseRpcChannel` clone; call `maybe_register_node` on connect-success; `deregister` in cleanup. |
| `src/gateway/handlers/auth/connect.rs` (modify) | Case 1: `validation.role == DeviceRole::Node` → response `role:"node"`. |
| `src/gateway/handlers/auth/mod.rs` (modify) | `AuthContext.node_registry` field. |
| `src/bin/aleph-server/commands/start/builder/subsystems.rs` (modify) | Production `AuthContext` literal gets `node_registry` (the server's Arc). |
| ~10 test `AuthContext` literals (modify) | Compiler-enumerated; each gets `node_registry: Arc::new(NodeRegistry::new())`. |
| `src/gateway/handlers/cluster.rs` (new) | `handle_cluster_enroll` + `handle_environments_list`. |
| `src/gateway/handlers/mod.rs` (modify) | `pub mod cluster;`. |
| `src/bin/aleph-server/commands/start/builder/handlers/auth.rs` (modify) | Register `cluster.enroll` + `environments.list`. |
| `src/gateway/method_authz.rs` (modify) | Add `cluster.enroll` to `OPERATOR_METHODS`. |

---

## Task 1: NodeRegistry core + glue

**Files:**
- Create: `src/cluster/registry.rs`
- Modify: `src/cluster/mod.rs`

- [ ] **Step 1: Write the failing tests**

Create `src/cluster/registry.rs` with ONLY this test module at the bottom (types come next step). Put the imports/types above as you implement Step 3.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::sync::mpsc;

    fn test_channel() -> ReverseRpcChannel {
        let (tx, _rx) = mpsc::channel::<String>(8);
        ReverseRpcChannel::new(tx)
    }

    fn session(node_id: &str, conn_id: &str) -> NodeSession {
        NodeSession {
            node_id: node_id.to_string(),
            conn_id: conn_id.to_string(),
            device_name: format!("dev-{node_id}"),
            channel: test_channel(),
            declared_commands: vec![CommandDescriptor {
                name: "bash".to_string(),
                schema: json!({"type": "object"}),
            }],
            connected_at: 1,
        }
    }

    #[test]
    fn register_then_list_projects_environment() {
        let reg = NodeRegistry::new();
        reg.register(session("node-a", "conn-1"));
        let envs = reg.list_environments();
        assert_eq!(envs.len(), 1);
        assert_eq!(envs[0].id, "node-a");
        assert_eq!(envs[0].name, "dev-node-a");
        assert_eq!(envs[0].status, "online");
        assert_eq!(envs[0].commands.len(), 1);
        assert_eq!(envs[0].commands[0].name, "bash");
    }

    #[test]
    fn deregister_removes_from_both_maps() {
        let reg = NodeRegistry::new();
        reg.register(session("node-a", "conn-1"));
        assert!(reg.deregister("conn-1"));
        assert!(reg.list_environments().is_empty());
        assert!(reg.get("node-a").is_none());
        // Deregistering an unknown conn is a no-op returning false.
        assert!(!reg.deregister("conn-x"));
    }

    #[test]
    fn reconnect_same_node_overwrites_and_old_cleanup_does_not_evict_new() {
        let reg = NodeRegistry::new();
        reg.register(session("node-a", "conn-1"));
        // Same node reconnects on a new connection.
        reg.register(session("node-a", "conn-2"));
        assert_eq!(reg.list_environments().len(), 1);
        // The OLD connection's cleanup must NOT evict the live (conn-2) session.
        assert!(!reg.deregister("conn-1"));
        assert_eq!(reg.list_environments().len(), 1);
        // The live connection's cleanup does evict.
        assert!(reg.deregister("conn-2"));
        assert!(reg.list_environments().is_empty());
    }

    #[test]
    fn get_returns_channel_for_known_node() {
        let reg = NodeRegistry::new();
        reg.register(session("node-a", "conn-1"));
        assert!(reg.get("node-a").is_some());
        assert!(reg.get("missing").is_none());
    }

    #[test]
    fn maybe_register_node_registers_only_for_node_role() {
        let reg = NodeRegistry::new();
        let ch = test_channel();
        let params = json!({"device_name": "worker", "commands": [{"name": "bash", "schema": {}}]});
        // Non-node role: not registered.
        assert!(!maybe_register_node(&reg, Some("operator"), "d1", "c1", Some(&params), &ch));
        assert!(reg.list_environments().is_empty());
        // Node role: registered with declared commands.
        assert!(maybe_register_node(&reg, Some("node"), "d2", "c2", Some(&params), &ch));
        let envs = reg.list_environments();
        assert_eq!(envs.len(), 1);
        assert_eq!(envs[0].id, "d2");
        assert_eq!(envs[0].commands[0].name, "bash");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib cluster::registry 2>&1 | tail -20`
Expected: FAIL — `cannot find type NodeRegistry` / `maybe_register_node` etc.

- [ ] **Step 3: Implement the types + registry + glue**

Put this ABOVE the test module in `src/cluster/registry.rs`:

```rust
//! 集群节点登记表（中心侧）。
//!
//! 追踪「哪些已连 WS 连接是已登记节点」，并把它们投影成只读「环境」视图供
//! `environments.list` 渲染。消费 Phase 0a 的 [`ReverseRpcChannel`]——每个
//! `NodeSession` 持一份 channel clone，0c 的 `node_invoke` 经它向节点下发。
//!
//! 红线：纯数据结构，无 LLM 推理（R7），不进 `src/harness/`（R10）。

use std::collections::HashMap;
use std::sync::RwLock;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cluster::ReverseRpcChannel;

/// 节点声明的一个 command（名字 + 自描述 schema）。0b 不解析 schema，原样透传。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CommandDescriptor {
    pub name: String,
    pub schema: Value,
}

/// 一个已连入的节点会话（中心侧视图）。
pub struct NodeSession {
    /// = device_id，直接当环境 id。
    pub node_id: String,
    /// 对应 0a reverse_rpc 表的键，断线清理对账用。
    pub conn_id: String,
    /// 人类可读名（来自 connect 帧）。
    pub device_name: String,
    /// 0a 通道的 clone —— 0c 的 node_invoke 经它下发。
    pub channel: ReverseRpcChannel,
    /// 节点自声明的 command 目录，0b 只存只显。
    pub declared_commands: Vec<CommandDescriptor>,
    /// 登记时刻（Unix 秒）。
    pub connected_at: i64,
}

/// `environments.list` 的对外序列化视图（薄渲染契约，R4）。绝不含凭证。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Environment {
    pub id: String,
    pub name: String,
    pub status: &'static str,
    pub commands: Vec<CommandDescriptor>,
    pub connected_at: i64,
}

#[derive(Default)]
struct RegistryInner {
    /// node_id → session（权威）。
    nodes_by_id: HashMap<String, NodeSession>,
    /// conn_id → node_id（断线反查）。
    nodes_by_conn: HashMap<String, String>,
}

/// 节点注册表。线程安全；锁中毒按 P7（`unwrap_or_else(|e| e.into_inner())`）。
#[derive(Default)]
pub struct NodeRegistry {
    inner: RwLock<RegistryInner>,
}

impl NodeRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 登记一个节点会话。同 node_id 重连 → 覆盖旧会话，并清掉旧 conn 映射。
    pub fn register(&self, session: NodeSession) {
        let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
        let node_id = session.node_id.clone();
        let conn_id = session.conn_id.clone();
        // Drop any stale conn→node mapping the previous session for this node_id held,
        // so an old connection's later cleanup can't evict the new session.
        if let Some(prev) = inner.nodes_by_id.get(&node_id) {
            let prev_conn = prev.conn_id.clone();
            inner.nodes_by_conn.remove(&prev_conn);
        }
        inner.nodes_by_conn.insert(conn_id, node_id.clone());
        inner.nodes_by_id.insert(node_id, session);
    }

    /// 注销一个连接的节点会话。仅当该 node_id 当前会话确属此 conn_id 时才移除
    /// （重连安全：旧连接 cleanup 不会误删新会话）。返回是否移除了会话。
    pub fn deregister(&self, conn_id: &str) -> bool {
        let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
        let Some(node_id) = inner.nodes_by_conn.remove(conn_id) else {
            return false;
        };
        match inner.nodes_by_id.get(&node_id) {
            Some(s) if s.conn_id == conn_id => {
                inner.nodes_by_id.remove(&node_id);
                true
            }
            _ => false,
        }
    }

    /// 在线节点的只读投影快照。
    pub fn list_environments(&self) -> Vec<Environment> {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        inner
            .nodes_by_id
            .values()
            .map(|s| Environment {
                id: s.node_id.clone(),
                name: s.device_name.clone(),
                status: "online",
                commands: s.declared_commands.clone(),
                connected_at: s.connected_at,
            })
            .collect()
    }

    /// 取某节点的反向 RPC 通道 clone（0c 的 node_invoke 用；0b 建好接口不调）。
    pub fn get(&self, node_id: &str) -> Option<ReverseRpcChannel> {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        inner.nodes_by_id.get(node_id).map(|s| s.channel.clone())
    }
}

/// connect→register 接缝：仅当 `role == Some("node")` 时把这条连接登记进
/// NodeRegistry。`params` 是 connect 帧的 params（取 device_name + commands）。
/// 返回是否登记。抽成纯函数以便单测，且让 `handler.rs` 保持薄。
pub fn maybe_register_node(
    registry: &NodeRegistry,
    role: Option<&str>,
    device_id: &str,
    conn_id: &str,
    params: Option<&Value>,
    channel: &ReverseRpcChannel,
) -> bool {
    if role != Some("node") {
        return false;
    }
    let device_name = params
        .and_then(|p| p.get("device_name"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let declared_commands = params
        .and_then(|p| p.get("commands"))
        .and_then(|v| serde_json::from_value::<Vec<CommandDescriptor>>(v.clone()).ok())
        .unwrap_or_default();
    registry.register(NodeSession {
        node_id: device_id.to_string(),
        conn_id: conn_id.to_string(),
        device_name,
        channel: channel.clone(),
        declared_commands,
        connected_at: now_unix(),
    });
    true
}

fn now_unix() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
```

- [ ] **Step 4: Export from `src/cluster/mod.rs`**

Change the module file. Current content:

```rust
mod reverse_rpc;

pub use reverse_rpc::{PendingInvokes, ReverseRpcChannel, ReverseRpcError};
```

to:

```rust
mod registry;
mod reverse_rpc;

pub use registry::{
    maybe_register_node, CommandDescriptor, Environment, NodeRegistry, NodeSession,
};
pub use reverse_rpc::{PendingInvokes, ReverseRpcChannel, ReverseRpcError};
```

(Also update the module doc comment in `mod.rs` to mention NodeRegistry is now present — change "后续 Phase 加 NodeRegistry" to past tense if you wish; optional.)

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib cluster::registry 2>&1 | tail -20`
Expected: PASS — 6 tests ok.

- [ ] **Step 6: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph-wt-cluster-phase0a
git add src/cluster/registry.rs src/cluster/mod.rs
git commit -m "cluster: NodeRegistry + node-session registry + maybe_register_node glue"
```

---

## Task 2: Wire node_registry into GatewayServer shared state

**Files:**
- Modify: `src/gateway/server/mod.rs` (struct `GatewaySharedState` ~line 129/205; struct `GatewayServer` ~line 286/368; `new`/`with_config` inits ~418/469; `build_router` ~602)
- Modify: `src/gateway/server/probe.rs` (its `GatewaySharedState` construction)

This MIRRORS the existing `reverse_rpc` field exactly. Use `reverse_rpc` as the template at every site.

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` in `src/gateway/server/mod.rs` (next to `reverse_rpc_registry_is_empty_on_fresh_server` ~line 844):

```rust
#[tokio::test]
async fn node_registry_is_empty_on_fresh_server() {
    let server = GatewayServer::new("127.0.0.1:0".parse().unwrap());
    assert!(server.node_registry.list_environments().is_empty());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib gateway::server::mod 2>&1 | tail -15` (or `cargo build -p alephcore 2>&1 | tail`)
Expected: FAIL — `no field node_registry on type GatewayServer`.

- [ ] **Step 3: Add the field at all five sites**

In `GatewaySharedState` (after the `reverse_rpc` field ~line 205):

```rust
    /// Cluster node registry (shared Arc with GatewayServer). Center-side view
    /// of connected `role:node` peers; populated by the connect handler.
    pub node_registry: Arc<crate::cluster::NodeRegistry>,
```

In `GatewayServer` (after the `reverse_rpc` field ~line 368):

```rust
    /// See [`GatewaySharedState::node_registry`]. `build_router` clones this Arc
    /// into the shared state so both point at the same registry.
    pub node_registry: Arc<crate::cluster::NodeRegistry>,
```

In BOTH `new` (~418) and `with_config` (~469) initializers, right after `reverse_rpc: Arc::new(RwLock::new(HashMap::new())),`:

```rust
            node_registry: Arc::new(crate::cluster::NodeRegistry::new()),
```

In `build_router`'s `GatewaySharedState { ... }` literal (~602), right after `reverse_rpc: self.reverse_rpc.clone(),`:

```rust
            node_registry: self.node_registry.clone(),
```

- [ ] **Step 4: Fix `probe.rs`**

`src/gateway/server/probe.rs` constructs a `GatewaySharedState` for tests. Add to its literal, next to its `reverse_rpc` field:

```rust
        node_registry: std::sync::Arc::new(crate::cluster::NodeRegistry::new()),
```

(Let the compiler point you at the exact line if unsure: `cargo build -p alephcore 2>&1 | grep -A3 probe.rs`.)

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p alephcore --lib node_registry_is_empty_on_fresh_server 2>&1 | tail -15`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/gateway/server/mod.rs src/gateway/server/probe.rs
git commit -m "gateway: add node_registry to shared state (mirrors reverse_rpc)"
```

---

## Task 3: connect.rs emits role="node" for node-role tokens

**Files:**
- Modify: `src/gateway/handlers/auth/connect.rs` (Case 1, ~line 382-383)
- Test: same file's `#[cfg(test)] mod tests`

- [ ] **Step 1: Write the failing test**

Add to `connect.rs`'s test module. Use the existing test `AuthContext` builder pattern already used in that module (the other connect tests construct `Arc::new(AuthContext { ... })` around line 604+; copy that exact construction, or reuse a local helper if one exists). The test mints a Node token via `ctx.token_manager`, then connects with it.

```rust
#[tokio::test]
async fn connect_with_node_role_token_yields_role_node() {
    // Build the same AuthContext the other connect tests use. If the module has
    // a local helper, use it; otherwise copy an existing `Arc::new(AuthContext{..})`
    // literal from a sibling test in this file. auth_mode MUST require auth
    // (AuthMode::Token) so the token path (Case 1) runs.
    let ctx = build_test_auth_context_token_mode(); // see note below

    // Mint a node-role device + token (token table FKs to a device row).
    let device_id = "node-test-1";
    ctx.security_store
        .upsert_device(&DeviceUpsertData {
            device_id,
            device_name: "worker-1",
            device_type: None,
            public_key: &[0u8; 32],
            fingerprint: &device_id.chars().take(16).collect::<String>(),
            role: DeviceRole::Node.as_str(),
            scopes: &["node".to_string()],
        })
        .unwrap();
    let tok = ctx
        .token_manager
        .issue_token(device_id, DeviceRole::Node, vec!["node".to_string()])
        .unwrap();

    let req = JsonRpcRequest::with_id(
        "connect",
        Some(serde_json::json!({
            "device_id": device_id,
            "token": format!("{}:{}", tok.token, tok.signature),
            "device_name": "worker-1",
        })),
        serde_json::json!(1),
    );
    // Call the connect handler exactly as the sibling tests in this module do
    // (same entry fn + return type — copy their call line). `resp` is the
    // JsonRpcResponse the handler returns.
    let resp = call_connect_handler(req, ctx).await;
    let role = resp
        .result
        .as_ref()
        .and_then(|r| r.get("role"))
        .and_then(|v| v.as_str());
    assert_eq!(role, Some("node"), "node-role token must yield role=node");
}
```

> **Implementer note:** Match the actual connect entry point + return type used by sibling tests in this module (they already call the connect handler and inspect the `JsonRpcResponse`). Reuse their exact `AuthContext` construction (auth-required mode) for `build_test_auth_context_token_mode`. The point of the test is only the `role == "node"` assertion. Do not invent new infra — clone the nearest existing connect test and swap the token role.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib connect_with_node_role_token_yields_role_node 2>&1 | tail -20`
Expected: FAIL — `role` is `Some("guest")` (current `role_for_permissions(["node"])` → "guest").

- [ ] **Step 3: Implement the role override**

In `connect.rs` Case 1, the current lines are:

```rust
                    let permissions = validation.scopes;
                    let role = super::tier::role_for_permissions(&permissions).to_string();
```

Change the second line to honor a node-role token explicitly (the tier helper only knows operator/guest):

```rust
                    let permissions = validation.scopes;
                    // A node-role token authenticates a cluster execution arm, not a
                    // human tier. role_for_permissions() only maps operator/guest from
                    // scopes, so surface "node" directly from the token's DeviceRole.
                    let role = if validation.role == DeviceRole::Node {
                        "node".to_string()
                    } else {
                        super::tier::role_for_permissions(&permissions).to_string()
                    };
```

(`DeviceRole` is already imported in `connect.rs` — it uses `DeviceRole::Operator` elsewhere. If not, add `use crate::gateway::security::device::DeviceRole;` matching the existing import.)

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib connect_with_node_role_token_yields_role_node 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/gateway/handlers/auth/connect.rs
git commit -m "gateway: connect emits role=node for DeviceRole::Node tokens"
```

---

## Task 4: handler.rs wires connect→register glue + cleanup

**Files:**
- Modify: `src/gateway/server/handler.rs` (`ConnectionContext` ~95-106; populate ~381; rpc channel block ~458-463; connect-success ~708-753; cleanup ~1399-1400)

No new test here (the glue is unit-tested in Task 1; this task is mechanical plumbing mirroring `reverse_rpc`). Verify by full build + existing gateway tests staying green.

- [ ] **Step 1: Add `node_registry` to `ConnectionContext`**

In the `ConnectionContext` struct (after the `reverse_rpc` field ~line 105):

```rust
    /// Cluster node registry (shared Arc). The connect handler registers a
    /// `role:node` connection here and cleanup deregisters it.
    node_registry: Arc<crate::cluster::NodeRegistry>,
```

- [ ] **Step 2: Populate it where `ConnectionContext` is built**

At the `ConnectionContext { ... }` literal (~line 381, right after `reverse_rpc: state.reverse_rpc.clone(),`):

```rust
            node_registry: state.node_registry.clone(),
```

- [ ] **Step 3: Keep a channel clone for node registration**

In the reverse-RPC setup block (~458-463), the `rpc_channel` is moved into the reverse_rpc registry. Add a clone BEFORE that move so the connect-success branch can hand it to the node registry. Change:

```rust
    let (rpc_out_tx, mut rpc_out_rx) = tokio::sync::mpsc::channel::<String>(64);
    let rpc_channel = crate::cluster::ReverseRpcChannel::new(rpc_out_tx);
    let rpc_pending = rpc_channel.pending();
    {
        let mut reg = ctx.reverse_rpc.write().await;
        reg.insert(conn_id.clone(), rpc_channel);
    }
```

to:

```rust
    let (rpc_out_tx, mut rpc_out_rx) = tokio::sync::mpsc::channel::<String>(64);
    let rpc_channel = crate::cluster::ReverseRpcChannel::new(rpc_out_tx);
    let rpc_pending = rpc_channel.pending();
    // Clone kept in scope so a successful `role:node` connect can register this
    // same channel into the NodeRegistry (cluster Phase 0b).
    let rpc_channel_for_node = rpc_channel.clone();
    {
        let mut reg = ctx.reverse_rpc.write().await;
        reg.insert(conn_id.clone(), rpc_channel);
    }
```

- [ ] **Step 4: Register the node on connect-success**

The connect-success block computes `role` (~695-706) then takes the connections write lock and calls `state.authenticate(device_id.clone(), permissions, role)` (~708-710), which MOVES `role`. Capture node-ness before the move and register after releasing the conns lock.

Just BEFORE `let mut conns = ctx.connections.write().await;` (~708), add:

```rust
                                                let is_node = role.as_deref() == Some("node");
```

Then, immediately AFTER the closing brace of the `if let Some(state) = conns.get_mut(&conn_id) { ... }` block AND after `conns` goes out of scope — concretely, place this right before the closing `}` of the `if resp.is_success() && req.method == "connect"` block (~753), having first dropped the conns guard. The simplest safe placement: wrap the existing `let mut conns = ...; if let Some(state) { ... }` in an explicit block is NOT required — instead add an explicit `drop(conns);` then the registration:

```rust
                                                drop(conns);
                                                // Cluster Phase 0b: register a node-role connection so it
                                                // surfaces in environments.list (and becomes node_invoke-reachable
                                                // in 0c). Pure glue; no-op for non-node roles.
                                                if is_node {
                                                    crate::cluster::maybe_register_node(
                                                        &ctx.node_registry,
                                                        Some("node"),
                                                        &device_id,
                                                        &conn_id,
                                                        req.params.as_ref(),
                                                        &rpc_channel_for_node,
                                                    );
                                                }
```

> **Implementer note:** `conns` is the `RwLockWriteGuard`; `drop(conns)` releases it before taking the NodeRegistry lock (avoids holding two locks). `device_id`, `conn_id`, `req`, `is_node`, `rpc_channel_for_node` are all in scope here. If borrow-checker complains that `conns` was already moved/dropped on some path, place `drop(conns);` at the exact end of the `if let Some(state)` usage. Confirm with `cargo build`.

- [ ] **Step 5: Deregister on cleanup**

In the cleanup block, right after the reverse_rpc deregister (~1399-1400):

```rust
    {
        let mut reg = ctx.reverse_rpc.write().await;
        reg.remove(&conn_id);
    }
    // Cluster Phase 0b: drop this connection's node session if it was a node.
    ctx.node_registry.deregister(&conn_id);
```

- [ ] **Step 6: Build + run gateway tests**

Run: `cargo build -p alephcore 2>&1 | tail -15`
Expected: clean build.
Run: `cargo test -p alephcore --lib gateway::server 2>&1 | tail -20`
Expected: existing tests still PASS (no regression).

- [ ] **Step 7: Commit**

```bash
git add src/gateway/server/handler.rs
git commit -m "gateway: register/deregister role:node connections into NodeRegistry"
```

---

## Task 5: AuthContext carries node_registry

**Files:**
- Modify: `src/gateway/handlers/auth/mod.rs` (`AuthContext` struct ~170-245)
- Modify: every `AuthContext { ... }` literal (compiler-enumerated). Known sites: `src/bin/aleph-server/commands/start/builder/subsystems.rs:186` (production), `src/gateway/auth_probe_tests.rs` (×2), `src/gateway/handlers/auth/connect.rs` (×6), `src/gateway/handlers/auth/mod.rs` (×2), `src/gateway/handlers/auth_tools.rs` (×1).

Mirrors how Phase 3a added the `connections` field.

- [ ] **Step 1: Add the field**

In `AuthContext` (after the `connections` field ~line 245):

```rust
    /// Cluster node registry, shared with `GatewayServer`. Lets
    /// `environments.list` enumerate connected nodes without a separate
    /// dispatch path.
    pub node_registry: Arc<crate::cluster::NodeRegistry>,
```

- [ ] **Step 2: Let the compiler enumerate the broken literals**

Run: `cargo build -p alephcore 2>&1 | grep -E "missing field .node_registry|AuthContext" | head -40`
Expected: one error per `AuthContext { ... }` literal missing the field.

- [ ] **Step 3: Fix the production site**

`src/bin/aleph-server/commands/start/builder/subsystems.rs:186` — the `auth_ctx` is built where the server's Arcs are in scope. Add (using the server's shared registry so handlers and the connect path share ONE registry):

```rust
        node_registry: server.node_registry.clone(),
```

> **Implementer note:** Confirm the in-scope variable holding the `GatewayServer` (likely `server`) at that point. The field must be the SAME Arc the server uses, or `environments.list` would read a different (empty) registry. If the `GatewayServer` isn't in scope at the literal, construct the Arc earlier and pass it into both `GatewayServer` and `AuthContext`.

- [ ] **Step 4: Fix every test site**

For each test `AuthContext { ... }` the compiler flagged, add:

```rust
            node_registry: std::sync::Arc::new(crate::cluster::NodeRegistry::new()),
```

(Test contexts don't share with a server, so a fresh registry is correct.)

- [ ] **Step 5: Build to confirm all sites fixed**

Run: `cargo build -p alephcore --all-targets 2>&1 | grep -E "node_registry|error" | head`
Expected: no `missing field` errors. (`--all-targets` so integration tests' `AuthContext` literals, if any, are caught too.)

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "gateway: thread node_registry through AuthContext (all construction sites)"
```

> Use `git add -A` here ONLY for the AuthContext field churn — verify `git status` first shows only the intended `auth/mod.rs` + the enumerated literal sites and no unrelated WIP from concurrent sessions. If other files appear, stage explicit paths instead.

---

## Task 6: cluster.rs handlers — enroll + environments.list

**Files:**
- Create: `src/gateway/handlers/cluster.rs`
- Modify: `src/gateway/handlers/mod.rs` (add `pub mod cluster;`)

- [ ] **Step 1: Write the failing tests**

Create `src/gateway/handlers/cluster.rs` with the test module (types/impl next). Reuse the crate's existing test `AuthContext` helper — `crate::gateway::handlers::auth::create_test_context()` returns `Arc<AuthContext>` (provisions a real token_manager + security_store). If it is not reachable cross-module, copy the nearest existing `AuthContext` test literal instead.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::handlers::auth::create_test_context;
    use crate::gateway::protocol::JsonRpcRequest;

    #[tokio::test]
    async fn enroll_mints_node_token_that_validates_as_node() {
        let ctx = create_test_context();
        let req = JsonRpcRequest::with_id(
            "cluster.enroll",
            Some(serde_json::json!({"node_name": "worker-1"})),
            serde_json::json!(1),
        );
        let resp = handle_cluster_enroll(req, ctx.clone()).await;
        assert!(resp.is_success(), "enroll should succeed: {:?}", resp.error);
        let result = resp.result.unwrap();
        let node_id = result["node_id"].as_str().unwrap().to_string();
        let token = result["token"].as_str().unwrap().to_string();
        let signature = result["signature"].as_str().unwrap().to_string();
        assert!(!token.is_empty());

        // The minted token must validate and carry DeviceRole::Node.
        let v = ctx.token_manager.validate_token(&token, &signature).unwrap();
        assert_eq!(v.device_id, node_id);
        assert_eq!(v.role, crate::gateway::security::device::DeviceRole::Node);
    }

    #[tokio::test]
    async fn environments_list_projects_registered_nodes() {
        let ctx = create_test_context();
        // Empty registry → empty list.
        let req = JsonRpcRequest::with_id("environments.list", None, serde_json::json!(1));
        let resp = handle_environments_list(req, ctx.clone()).await;
        assert!(resp.is_success());
        assert_eq!(resp.result.unwrap()["environments"].as_array().unwrap().len(), 0);

        // Register a node directly into the shared registry, then list.
        let (tx, _rx) = tokio::sync::mpsc::channel::<String>(8);
        let ch = crate::cluster::ReverseRpcChannel::new(tx);
        crate::cluster::maybe_register_node(
            &ctx.node_registry,
            Some("node"),
            "node-a",
            "conn-1",
            Some(&serde_json::json!({"device_name": "worker-1", "commands": [{"name":"bash","schema":{}}]})),
            &ch,
        );
        let req = JsonRpcRequest::with_id("environments.list", None, serde_json::json!(2));
        let resp = handle_environments_list(req, ctx.clone()).await;
        let envs = resp.result.unwrap();
        let arr = envs["environments"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["id"], "node-a");
        assert_eq!(arr[0]["status"], "online");
        assert_eq!(arr[0]["commands"][0]["name"], "bash");
        // Must NOT leak any credential field.
        assert!(arr[0].get("token").is_none());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib handlers::cluster 2>&1 | tail -20`
Expected: FAIL — `handle_cluster_enroll` / `handle_environments_list` not found.

- [ ] **Step 3: Implement the handlers**

Put ABOVE the test module in `src/gateway/handlers/cluster.rs`:

```rust
//! 集群中心侧 RPC：`cluster.enroll`（operator 铸 node token）+
//! `environments.list`（read，枚举在线节点）。形态为 gateway RPC 而非 builtin
//! 工具——凭证操作的既有模式（同 devices.*/pairing.*）。LLM-callable 工具面随
//! 0c 的 node_invoke 一起落地。

use std::sync::Arc;

use serde::Deserialize;

use crate::gateway::handlers::auth::AuthContext;
use crate::gateway::handlers::{parse_params, INTERNAL_ERROR};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse};
use crate::gateway::security::device::DeviceRole;
use crate::gateway::security::store::DeviceUpsertData;

#[derive(Deserialize)]
struct EnrollParams {
    node_name: String,
}

/// operator-gated：铸一个 DeviceRole::Node 设备 + token，返回给操作员转交节点机。
pub async fn handle_cluster_enroll(
    request: JsonRpcRequest,
    ctx: Arc<AuthContext>,
) -> JsonRpcResponse {
    let params: EnrollParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    let device_id = uuid::Uuid::new_v4().to_string();
    let fingerprint: String = device_id.chars().take(16).collect();

    // Token table FKs to a device row — create it first (mirrors connect.rs).
    if let Err(e) = ctx.security_store.upsert_device(&DeviceUpsertData {
        device_id: &device_id,
        device_name: &params.node_name,
        device_type: None,
        public_key: &[0u8; 32],
        fingerprint: &fingerprint,
        role: DeviceRole::Node.as_str(),
        scopes: &["node".to_string()],
    }) {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("failed to register node device: {e}"),
        );
    }

    let signed = match ctx.token_manager.issue_token(
        &device_id,
        DeviceRole::Node,
        vec!["node".to_string()],
    ) {
        Ok(t) => t,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("failed to issue node token: {e}"),
            )
        }
    };

    JsonRpcResponse::success(
        request.id,
        serde_json::json!({
            "node_id": device_id,
            "token": signed.token,
            "signature": signed.signature,
        }),
    )
}

/// read：枚举当前在线节点（薄渲染契约，不含凭证）。
pub async fn handle_environments_list(
    request: JsonRpcRequest,
    ctx: Arc<AuthContext>,
) -> JsonRpcResponse {
    let envs = ctx.node_registry.list_environments();
    JsonRpcResponse::success(
        request.id,
        serde_json::json!({ "environments": envs }),
    )
}
```

> **Implementer note:** Confirm exact import paths against the codebase: `DeviceUpsertData` (the connect.rs `use` line), `INTERNAL_ERROR` (defined in `handlers/mod.rs`; `parse_params` is `pub(crate)` there), `JsonRpcResponse::success/error` signatures (id is `Option<Value>`; `request.id` is already that). `uuid` is already a dependency (connect.rs uses `uuid::Uuid::new_v4()`).

- [ ] **Step 4: Register the module**

In `src/gateway/handlers/mod.rs`, add to the module declarations (alphabetical-ish, near other handler mods):

```rust
pub mod cluster;
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib handlers::cluster 2>&1 | tail -20`
Expected: PASS — 2 tests ok.

- [ ] **Step 6: Commit**

```bash
git add src/gateway/handlers/cluster.rs src/gateway/handlers/mod.rs
git commit -m "gateway: cluster.enroll + environments.list RPC handlers"
```

---

## Task 7: Register handlers + operator-gate cluster.enroll

**Files:**
- Modify: `src/bin/aleph-server/commands/start/builder/handlers/auth.rs` (register the two methods)
- Modify: `src/gateway/method_authz.rs` (add `cluster.enroll` to `OPERATOR_METHODS`; ~line 58)
- Test: `src/gateway/method_authz.rs` test module

- [ ] **Step 1: Write the failing test**

Add to `method_authz.rs`'s `#[cfg(test)] mod tests`:

```rust
#[test]
fn cluster_enroll_requires_operator_but_environments_list_is_open() {
    assert!(method_requires_operator("cluster.enroll"));
    assert!(!method_requires_operator("environments.list"));
}
```

> **Implementer note:** Use the actual predicate name in this file for method gating (the file has `OPERATOR_METHODS` + a predicate around line 186 `if OPERATOR_METHODS.contains(&method)`). If the public predicate is named differently (e.g. `method_requires_operator` / `requires_operator`), call that. Match the existing sibling tests' calling convention.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib method_authz 2>&1 | tail -15`
Expected: FAIL — `cluster.enroll` not yet operator-gated.

- [ ] **Step 3: Add to OPERATOR_METHODS**

In `method_authz.rs`, inside the `OPERATOR_METHODS` array, add (near the device/pairing group):

```rust
    // Cluster node enrollment (mints a node token — credential issuance)
    "cluster.enroll",
```

(`environments.list` is intentionally NOT added — it stays open to chat/guest read.)

- [ ] **Step 4: Register the RPC handlers**

In `src/bin/aleph-server/commands/start/builder/handlers/auth.rs`, next to the `devices.*` registrations, add (the `cluster` handlers live in `crate::gateway::handlers::cluster`; import or fully-qualify to match how `auth_handlers` is referenced in this file):

```rust
    register_handler!(
        server,
        "cluster.enroll",
        crate::gateway::handlers::cluster::handle_cluster_enroll,
        auth_ctx
    );
    register_handler!(
        server,
        "environments.list",
        crate::gateway::handlers::cluster::handle_environments_list,
        auth_ctx
    );
```

> **Implementer note:** Confirm `register_handler!`'s expected handler signature matches `async fn(JsonRpcRequest, Arc<AuthContext>) -> JsonRpcResponse` (the `devices.*` handlers it already registers have this shape). Match the import style this file uses for the other handlers (it aliases `auth_handlers`; you may add a `use crate::gateway::handlers::cluster as cluster_handlers;` and use `cluster_handlers::handle_cluster_enroll`).

- [ ] **Step 5: Run test + build**

Run: `cargo test -p alephcore --lib method_authz 2>&1 | tail -15`
Expected: PASS.
Run: `cargo build -p alephcore --bin aleph-server 2>&1 | tail -15`
Expected: clean build (handler registration compiles).

- [ ] **Step 6: Commit**

```bash
git add src/gateway/method_authz.rs src/bin/aleph-server/commands/start/builder/handlers/auth.rs
git commit -m "gateway: register cluster.enroll (operator) + environments.list (read) RPCs"
```

---

## Final verification (after all tasks)

- [ ] Run the cluster + gateway lib tests:

```bash
cargo test -p alephcore --lib cluster:: 2>&1 | tail -20
cargo test -p alephcore --lib handlers::cluster 2>&1 | tail -10
cargo test -p alephcore --lib method_authz 2>&1 | tail -10
cargo test -p alephcore --lib gateway::server 2>&1 | tail -10
```
Expected: all PASS.

- [ ] Build the binary (handler wiring compiles end-to-end):

```bash
cargo build -p alephcore --bin aleph-server 2>&1 | tail -15
```
Expected: clean.

- [ ] Clippy on the touched crate:

```bash
cargo clippy -p alephcore --lib 2>&1 | tail -20
```
Expected: no new warnings on cluster/gateway code.

> Do NOT run `cargo check --all-targets` across the whole workspace unless needed — it triggers a massive cold compile of unrelated integration tests/benches (per the 0a compile-fatigue lesson). The targeted commands above cover this phase's surface.

---

## Notes for the executor

- **Branch discipline:** all commits land on `feat/cluster-phase0a-reverse-rpc`. Do NOT merge to main; do NOT `git worktree remove` (shell-corruption hazard). Concurrent sessions may commit to main — never `reset`/`rebase`/`amend`; append only. Stage explicit paths (Task 5's `git add -A` is the one exception — gate it on a clean `git status`).
- **Pattern-mirroring:** Tasks 2 and 4 mirror the proven `reverse_rpc` wiring; Task 5 mirrors Phase 3a's `connections` field addition; Task 7 mirrors `devices.*` registration. When unsure of an exact line, grep for `reverse_rpc` / `connections` / `devices.set_level` and copy the adjacent shape.
- **R10:** `src/harness/` must remain untouched. If any task tempts you to edit it, stop — the design routes everything through cluster/ + gateway/.
- **No LLM-facing tool surface this phase** — `environments` as a tool / prompt-injection and `node_invoke` are 0c.
