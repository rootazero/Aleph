# Cluster ③ Node-Approval-Routing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a remote node, when its sandbox hits a capability escalation (today auto-denied because its `ApprovalGate` has `requester=None`), route the approval prompt UP to the center where the operator decides via the existing Panel approval card, with the decision flowing back over a now-bidirectional reverse-RPC channel.

**Architecture:** The node's `ApprovalGate` gets a `CenterApprovalRequester` that calls reverse-RPC `node.approval.request`. The node's `run_session` is restructured for concurrency (read/write split + outbound mpsc + spawned dispatch + node-side `PendingInvokes`) so a blocked bash command does not deadlock the read loop. The center routes `node.approval.request` from registered node connections to `run_node_approval`, which reuses the shared `ExecApprovalManager` + the existing `ApprovalRequested` frame (node context encoded in the `command` field, so the Panel card appears with zero WASM change) + the existing `exec.approval.resolve` RPC. The decision is returned as the JSON-RPC response.

**Tech Stack:** Rust, tokio, async-trait, serde_json, tokio-tungstenite (node WS client), the existing `src/cluster/reverse_rpc.rs`, `src/exec/manager.rs`, `src/approval/operator_requester.rs` (mirror), `src/sandbox/exec_approval/gate.rs`.

**Worktree:** New worktree cut from `main` (main already has the spec `bbb1f0a58`/`37e733901`). Do NOT merge to main — the user manages cluster merge strategy. R10: `src/harness/` is NOT touched.

**Wire contract (byte-level, both directions):**
- Node → center request: `{"jsonrpc":"2.0","id":"rpc-N","method":"node.approval.request","params":{"tool":"<prog>","reason":"<why>"}}`
- Center → node response: `{"jsonrpc":"2.0","id":"rpc-N","result":{"outcome":"approved|approved_session|denied|timeout"}}`
- id collision with the center's own `tool.call` reverse-RPC ids is a non-issue: reverse_rpc distinguishes request vs response by structure (`method` present ⇒ request; `result`/`error` present ⇒ response), per `reverse_rpc.rs` module doc.

---

## File Structure

- **Create** `src/cluster/node_approval.rs` — `CenterApprovalRequester` (node-side `ApprovalRequester` impl) + outcome-string mapping.
- **Modify** `src/cluster/mod.rs` — export `CenterApprovalRequester`.
- **Modify** `src/cluster/registry.rs` — add `NodeRegistry::node_identity_by_conn`.
- **Create** `src/approval/node_requester.rs` — `run_node_approval` (center-side driver mirroring `OperatorApprovalRequester`).
- **Modify** `src/approval/mod.rs` — export `node_requester`.
- **Modify** `src/gateway/server/mod.rs` — add `exec_approval_manager: Option<Arc<ExecApprovalManager>>` to `GatewayServer` + `GatewaySharedState`; clone in `build_router`.
- **Modify** `src/gateway/server/handler.rs` — add field to `ConnectionContext`; clone in ctx build; capture `rpc_out_tx_replies`; insert `node.approval.request` routing block.
- **Modify** `src/gateway/server/probe.rs` — `exec_approval_manager: None` in the test constructor.
- **Modify** `src/bin/aleph-server/commands/start/mod.rs` — set the shared manager on the server after construction.
- **Modify** `src/bin/aleph-server/commands/node.rs` — `build_command_table` returns `(CommandTable, ApprovalSlot)`, wires `CenterApprovalRequester`; `run_session` concurrency restructure.
- **Create** `tests/cluster_node_approval.rs` — in-process full round-trip integration test.

---

## Task 1: Node-side requester + registry conn lookup

**Files:**
- Create: `src/cluster/node_approval.rs`
- Modify: `src/cluster/mod.rs`
- Modify: `src/cluster/registry.rs`

- [ ] **Step 1: Write the failing test for `node_identity_by_conn`**

In `src/cluster/registry.rs`, add to the existing `#[cfg(test)] mod tests` block:

```rust
    #[test]
    fn node_identity_by_conn_returns_id_and_name() {
        let reg = NodeRegistry::new();
        reg.register(session("node-a", "conn-1")); // device_name = "dev-node-a"
        assert_eq!(
            reg.node_identity_by_conn("conn-1"),
            Some(("node-a".to_string(), "dev-node-a".to_string()))
        );
        assert_eq!(reg.node_identity_by_conn("conn-x"), None);
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p alephcore --lib cluster::registry::tests::node_identity_by_conn`
Expected: FAIL — `no method named node_identity_by_conn`.

- [ ] **Step 3: Implement `node_identity_by_conn`**

In `src/cluster/registry.rs`, add this method inside `impl NodeRegistry` (right after `get`, before `resolve`):

```rust
    /// Resolve `(node_id, device_name)` for a connection that is a registered
    /// node. Returns `None` for non-node / unregistered connections. The center
    /// uses this to stamp node identity from the AUTHENTICATED connection rather
    /// than trusting request params (anti-spoof).
    pub fn node_identity_by_conn(&self, conn_id: &str) -> Option<(String, String)> {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        let node_id = inner.nodes_by_conn.get(conn_id)?;
        let s = inner.nodes_by_id.get(node_id)?;
        Some((s.node_id.clone(), s.device_name.clone()))
    }
```

- [ ] **Step 4: Run it to verify it passes**

Run: `cargo test -p alephcore --lib cluster::registry::tests::node_identity_by_conn`
Expected: PASS.

- [ ] **Step 5: Write the failing tests for `CenterApprovalRequester`**

Create `src/cluster/node_approval.rs`:

```rust
//! Node-side approval requester (cluster ③).
//!
//! The node runs headless; its `ApprovalGate` would otherwise auto-deny every
//! capability escalation (`requester=None`). This requester instead routes the
//! prompt UP to the center over the now-bidirectional reverse-RPC channel and
//! maps the center's decision back to an `ApprovalOutcome`. Fail-closed: a
//! missing channel (disconnected), a transport error, or a timeout all map to
//! `Denied` — never a silent auto-approve.
//!
//! Redlines: pure routing, no LLM reasoning (R7); not in `src/harness/` (R10).

use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use serde_json::json;

use crate::cluster::ReverseRpcChannel;
use crate::sandbox::exec_approval::gate::{ApprovalOutcome, ApprovalRequester};

/// Shared, per-connection-refreshed channel slot. `run_session` writes
/// `Some(channel)` on connect and `None` on disconnect; the requester reads it
/// per call. `None` ⇒ fail-closed `Denied`.
pub type ApprovalSlot = Arc<RwLock<Option<ReverseRpcChannel>>>;

/// Node-side timeout for the reverse approval call. Deliberately ABOVE the
/// center's `DEFAULT_APPROVAL_TIMEOUT_MS` (120s) so the center decides first and
/// returns an explicit `"timeout"` outcome; this is only a transport-death
/// backstop.
pub const NODE_APPROVAL_TIMEOUT_MS: u64 = 130_000;

/// Map the center's outcome string back to an `ApprovalOutcome`. Any unknown
/// value (including `"denied"`) is fail-closed `Denied`.
pub fn outcome_from_str(s: &str) -> ApprovalOutcome {
    match s {
        "approved" => ApprovalOutcome::Approved,
        "approved_session" => ApprovalOutcome::ApprovedForSession,
        "timeout" => ApprovalOutcome::Timeout,
        _ => ApprovalOutcome::Denied,
    }
}

pub struct CenterApprovalRequester {
    slot: ApprovalSlot,
}

impl CenterApprovalRequester {
    pub fn new(slot: ApprovalSlot) -> Self {
        Self { slot }
    }
}

#[async_trait]
impl ApprovalRequester for CenterApprovalRequester {
    async fn request_approval(&self, tool_name: &str, reason: &str) -> ApprovalOutcome {
        // Clone the channel out of the lock and drop the guard before awaiting —
        // a std RwLock guard is not Send.
        let channel = self
            .slot
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let Some(channel) = channel else {
            tracing::warn!("node approval requested with no live center channel; denying");
            return ApprovalOutcome::Denied;
        };
        let params = json!({ "tool": tool_name, "reason": reason });
        match channel
            .call("node.approval.request", params, NODE_APPROVAL_TIMEOUT_MS)
            .await
        {
            Ok(resp) if resp.is_success() => {
                let outcome = resp
                    .result
                    .as_ref()
                    .and_then(|r| r.get("outcome"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("denied");
                outcome_from_str(outcome)
            }
            Ok(_) => ApprovalOutcome::Denied,
            Err(e) => {
                tracing::warn!(error = %e, "node approval reverse-rpc failed; denying");
                ApprovalOutcome::Denied
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::protocol::JsonRpcResponse;
    use serde_json::Value;
    use tokio::sync::mpsc;

    fn slot_with_channel() -> (ApprovalSlot, mpsc::Receiver<String>, Arc<crate::cluster::PendingInvokes>) {
        let (out_tx, out_rx) = mpsc::channel::<String>(8);
        let channel = ReverseRpcChannel::new(out_tx);
        let pending = channel.pending();
        let slot: ApprovalSlot = Arc::new(RwLock::new(Some(channel)));
        (slot, out_rx, pending)
    }

    #[test]
    fn outcome_mapping_is_fail_closed() {
        assert_eq!(outcome_from_str("approved"), ApprovalOutcome::Approved);
        assert_eq!(
            outcome_from_str("approved_session"),
            ApprovalOutcome::ApprovedForSession
        );
        assert_eq!(outcome_from_str("timeout"), ApprovalOutcome::Timeout);
        assert_eq!(outcome_from_str("denied"), ApprovalOutcome::Denied);
        assert_eq!(outcome_from_str("garbage"), ApprovalOutcome::Denied);
    }

    #[tokio::test]
    async fn none_channel_denies() {
        let slot: ApprovalSlot = Arc::new(RwLock::new(None));
        let requester = CenterApprovalRequester::new(slot);
        assert_eq!(
            requester.request_approval("bash", "needs network").await,
            ApprovalOutcome::Denied
        );
    }

    #[tokio::test]
    async fn round_trip_maps_center_outcome() {
        let (slot, mut out_rx, pending) = slot_with_channel();
        let requester = CenterApprovalRequester::new(slot);

        // Background "center": read the request frame, assert its shape, reply
        // with an approved_session outcome.
        tokio::spawn(async move {
            let frame = out_rx.recv().await.expect("request frame");
            let req: Value = serde_json::from_str(&frame).unwrap();
            assert_eq!(req["method"], "node.approval.request");
            assert_eq!(req["params"]["tool"], "bash");
            assert_eq!(req["params"]["reason"], "needs network");
            let id = req["id"].clone();
            let resp =
                JsonRpcResponse::success(Some(id.clone()), json!({"outcome": "approved_session"}));
            pending.resolve(&id, resp);
        });

        assert_eq!(
            requester.request_approval("bash", "needs network").await,
            ApprovalOutcome::ApprovedForSession
        );
    }

    #[tokio::test]
    async fn transport_closed_denies() {
        let (out_tx, out_rx) = mpsc::channel::<String>(8);
        drop(out_rx); // closed transport → channel.call returns TransportClosed
        let channel = ReverseRpcChannel::new(out_tx);
        let slot: ApprovalSlot = Arc::new(RwLock::new(Some(channel)));
        let requester = CenterApprovalRequester::new(slot);
        assert_eq!(
            requester.request_approval("bash", "x").await,
            ApprovalOutcome::Denied
        );
    }
}
```

- [ ] **Step 6: Export from `src/cluster/mod.rs`**

Add the module + re-export. Match the existing private-`mod` + `pub use` pattern already used in `cluster/mod.rs`. Add near the other `mod`/`pub use` lines:

```rust
mod node_approval;
pub use node_approval::{ApprovalSlot, CenterApprovalRequester, NODE_APPROVAL_TIMEOUT_MS};
```

Also confirm `PendingInvokes` is reachable as `crate::cluster::PendingInvokes` (the test uses it). If `cluster/mod.rs` does not already re-export it, add `pub use reverse_rpc::PendingInvokes;` alongside the existing `pub use reverse_rpc::{...}` (it already re-exports `ReverseRpcChannel`).

- [ ] **Step 7: Run the node_approval + cluster tests**

Run: `cargo test -p alephcore --lib cluster::node_approval`
Expected: PASS (4 tests).
Run: `cargo test -p alephcore --lib cluster::`
Expected: PASS (all cluster tests, including the new `node_identity_by_conn`).

- [ ] **Step 8: Commit**

```bash
git add src/cluster/node_approval.rs src/cluster/mod.rs src/cluster/registry.rs
git commit -m "cluster: node-side CenterApprovalRequester + node_identity_by_conn"
```

---

## Task 2: Center-side `run_node_approval` driver

**Files:**
- Create: `src/approval/node_requester.rs`
- Modify: `src/approval/mod.rs`

- [ ] **Step 1: Write the module with failing tests**

Create `src/approval/node_requester.rs` (mirrors `operator_requester.rs`, but node-flavored: no turn-context, node identity baked into the `command` field, returns an outcome string for the wire):

```rust
//! `run_node_approval` — the center-side driver for a node-initiated approval
//! (cluster ③). Mirrors `OperatorApprovalRequester` but:
//!   - there is no turn-context (the node has no chat conversation),
//!   - node identity + tool + reason are encoded into the `ApprovalRequest`
//!     `command` field, so the existing Panel card (which renders
//!     `ExecApprovalRecord.command` after refetching `exec.approvals.pending`)
//!     shows the node context with ZERO frontend change,
//!   - it returns a wire outcome string instead of an `ApprovalOutcome`.
//!
//! Reuses the shared `ExecApprovalManager` so the operator's existing
//! `exec.approval.resolve` RPC wakes this driver's oneshot.

use crate::exec::analysis::CommandAnalysis;
use crate::exec::decision::ApprovalRequest;
use crate::exec::manager::{ExecApprovalManager, DEFAULT_APPROVAL_TIMEOUT_MS};
use crate::exec::socket::ApprovalDecisionType;
use crate::gateway::event_bus::GatewayEventBus;
use crate::gateway::events::GatewayEventFrame;

/// Map an `ExecApprovalManager` decision into the wire outcome string consumed
/// by the node's `outcome_from_str`. `AllowAlways` collapses to a session grant
/// (permanent device elevation is out of scope, same as Phase 2b).
fn decision_to_wire(decision: Option<ApprovalDecisionType>) -> &'static str {
    match decision {
        Some(ApprovalDecisionType::AllowOnce) => "approved",
        Some(ApprovalDecisionType::AllowSession) => "approved_session",
        Some(ApprovalDecisionType::AllowAlways) => "approved_session",
        Some(ApprovalDecisionType::Deny) => "denied",
        None => "timeout",
    }
}

/// Drive one node-initiated approval to a decision. Blocks on the operator
/// decision (up to `DEFAULT_APPROVAL_TIMEOUT_MS`); callers MUST run this on a
/// spawned task so the connection's select loop is not blocked.
pub async fn run_node_approval(
    manager: &ExecApprovalManager,
    event_bus: &GatewayEventBus,
    node_id: &str,
    node_name: &str,
    tool: &str,
    reason: &str,
) -> &'static str {
    let command = format!("node '{node_name}': {tool} — {reason}");
    let request = ApprovalRequest {
        id: uuid::Uuid::new_v4().to_string(),
        command,
        cwd: None,
        analysis: CommandAnalysis {
            ok: true,
            reason: None,
            segments: vec![],
            chains: None,
        },
        agent_id: format!("node:{node_id}"),
        session_key: String::new(),
    };
    let record = manager.create(&request, DEFAULT_APPROVAL_TIMEOUT_MS);
    // Register BEFORE publishing so an instantly-resolving operator cannot race
    // ahead of registration (mirrors operator_requester).
    let (approval_id, rx, timeout) = manager.register_pending(record);

    if let Err(e) = event_bus.publish_frame(&GatewayEventFrame::ApprovalRequested {
        approval_id: approval_id.clone(),
        session_key: String::new(),
        channel_id: String::new(),
        conversation_id: String::new(),
    }) {
        tracing::warn!(error = %e, "failed to publish ApprovalRequested for node approval");
    }

    let decision = manager
        .await_registered(approval_id.clone(), rx, timeout)
        .await;

    let frame = match decision {
        Some(d) => GatewayEventFrame::ApprovalResolved {
            approval_id,
            session_key: String::new(),
            decision: d,
            resolved_by: None,
        },
        None => GatewayEventFrame::ApprovalExpired {
            approval_id,
            session_key: String::new(),
        },
    };
    let _ = event_bus.publish_frame(&frame);

    decision_to_wire(decision)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync_primitives::Arc;
    use std::time::Duration;

    #[test]
    fn decision_mapping() {
        assert_eq!(decision_to_wire(Some(ApprovalDecisionType::AllowOnce)), "approved");
        assert_eq!(
            decision_to_wire(Some(ApprovalDecisionType::AllowSession)),
            "approved_session"
        );
        assert_eq!(
            decision_to_wire(Some(ApprovalDecisionType::AllowAlways)),
            "approved_session"
        );
        assert_eq!(decision_to_wire(Some(ApprovalDecisionType::Deny)), "denied");
        assert_eq!(decision_to_wire(None), "timeout");
    }

    #[tokio::test]
    async fn publishes_node_context_and_resolves() {
        let event_bus = Arc::new(GatewayEventBus::new());
        let manager = Arc::new(ExecApprovalManager::new());
        let mut rx = event_bus.subscribe_typed();

        let mgr = manager.clone();
        let bus = event_bus.clone();
        let handle = tokio::spawn(async move {
            run_node_approval(&mgr, &bus, "node-1", "worker", "bash", "needs network").await
        });

        // Observe the ApprovalRequested frame, then resolve via the manager (as
        // the operator's exec.approval.resolve would).
        let mut approval_id: Option<String> = None;
        for _ in 0..6 {
            let Ok(Ok(frame)) = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await
            else {
                break;
            };
            if let GatewayEventFrame::ApprovalRequested { approval_id: id, .. } = frame {
                approval_id = Some(id);
                break;
            }
        }
        let id = approval_id.expect("ApprovalRequested published");
        // The pending record carries the node-context command for the Panel card.
        // `list_pending() -> Vec<PendingApproval>` (manager.rs:381); each
        // `PendingApproval` exposes `.record.command` (same path the Panel's
        // exec.approvals.pending handler reads).
        let pending = manager.list_pending();
        assert!(
            pending
                .iter()
                .any(|p| p.record.command == "node 'worker': bash — needs network"),
            "pending record must carry node-context command, got {:?}",
            pending.iter().map(|p| &p.record.command).collect::<Vec<_>>()
        );
        assert!(manager.resolve(&id, ApprovalDecisionType::AllowSession, None));

        assert_eq!(handle.await.unwrap(), "approved_session");
    }
}
```

> If `PendingApproval`'s field is not literally `record` (verify in `src/exec/manager.rs`), adjust the field path; the Panel API reads `p.record.command`, so that path is expected. Do NOT invent an accessor.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p alephcore --lib approval::node_requester`
Expected: FAIL — module not declared / type errors until wired.

- [ ] **Step 3: Export from `src/approval/mod.rs`**

Add alongside the existing `pub use operator_requester::...`:

```rust
pub mod node_requester;
pub use node_requester::run_node_approval;
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p alephcore --lib approval::node_requester`
Expected: PASS (2 tests). If `list_pending_records` does not exist, apply the NOTE above first.

- [ ] **Step 5: Commit**

```bash
git add src/approval/node_requester.rs src/approval/mod.rs
git commit -m "approval: run_node_approval center driver (reuses ApprovalRequested + manager)"
```

---

## Task 3: Thread `ExecApprovalManager` into the connection + route `node.approval.request`

**Files:**
- Modify: `src/gateway/server/mod.rs` — `GatewaySharedState` (struct ending ~:216, `node_registry` at :215) + `GatewayServer` (struct at :296, `node_registry` at :381) + `build_router` (:618) + the two `GatewayServer` constructors (:432, :484)
- Modify: `src/gateway/server/probe.rs:106`
- Modify: `src/gateway/server/handler.rs` (`ConnectionContext` `:108` area, ctx build `:385`, outbound clone `:461`, routing block after `:527`)
- Modify: `src/bin/aleph-server/commands/start/mod.rs` (set the manager on the server)

- [ ] **Step 1: Add the field to `GatewayServer` and `GatewaySharedState`**

In `src/gateway/server/mod.rs`, in the `GatewayServer` struct (near its `pub node_registry` field at `:381`) add the OWNED field:

```rust
    /// Shared exec-approval manager (cluster ③). `Some` once boot wires the
    /// canonical instance; `None` in test/probe constructors ⇒ node-approval
    /// routing is inert (the handler refuses `node.approval.request`).
    pub exec_approval_manager: Option<Arc<crate::exec::manager::ExecApprovalManager>>,
```

In the `GatewaySharedState` struct (near its `pub node_registry` at `:215`) add the same field (this is the per-router projection `build_router` fills from the server):

```rust
    pub exec_approval_manager: Option<Arc<crate::exec::manager::ExecApprovalManager>>,
```

In BOTH `GatewayServer` constructors (`mod.rs:432` and `:484`, the ones that do `node_registry: Arc::new(crate::cluster::NodeRegistry::new())`) add:

```rust
            exec_approval_manager: None,
```

In `build_router` (`mod.rs:618`, where it does `node_registry: self.node_registry.clone()`) add:

```rust
            exec_approval_manager: self.exec_approval_manager.clone(),
```

In `src/gateway/server/probe.rs:106` (the test constructor that sets `node_registry: Arc::new(...)`) add:

```rust
            exec_approval_manager: None,
```

- [ ] **Step 2: Add the field to `ConnectionContext` and clone it in ctx build**

In `src/gateway/server/handler.rs`, in `struct ConnectionContext` (after `node_registry` at `:108`) add:

```rust
    /// Shared exec-approval manager for node-initiated approvals (cluster ③).
    /// `None` ⇒ `node.approval.request` is refused.
    exec_approval_manager: Option<Arc<crate::exec::manager::ExecApprovalManager>>,
```

In the ctx construction (`handler.rs:385`, after `node_registry: state.node_registry.clone(),`) add:

```rust
            exec_approval_manager: state.exec_approval_manager.clone(),
```

- [ ] **Step 3: Capture an outbound sender clone for node-initiated replies**

In `src/gateway/server/handler.rs` at lines 461–462, change:

```rust
    let (rpc_out_tx, mut rpc_out_rx) = tokio::sync::mpsc::channel::<String>(64);
    let rpc_channel = crate::cluster::ReverseRpcChannel::new(rpc_out_tx);
```

to:

```rust
    let (rpc_out_tx, mut rpc_out_rx) = tokio::sync::mpsc::channel::<String>(64);
    // Clone kept for node-initiated request replies (cluster ③): a spawned
    // approval task sends its JSON-RPC response here; the select arm below
    // writes it to the socket.
    let rpc_out_tx_replies = rpc_out_tx.clone();
    let rpc_channel = crate::cluster::ReverseRpcChannel::new(rpc_out_tx);
```

- [ ] **Step 4: Insert the routing block**

In `src/gateway/server/handler.rs`, immediately AFTER the reverse-RPC response-interception block (after its `continue;` and closing braces at ~line 527) and BEFORE `// Parse request to check method for auth gating` (~line 529), insert:

```rust
                        // Node-initiated reverse request (cluster ③): a
                        // `node.approval.request` from a REGISTERED node
                        // connection is driven asynchronously and answered with a
                        // JSON-RPC response on this connection's outbound. Spawned
                        // so the select loop is not blocked for the (up to 120s)
                        // operator decision. Node identity is taken from the
                        // authenticated connection (anti-spoof), never params.
                        if let Ok(node_req) = serde_json::from_str::<JsonRpcRequest>(&text) {
                            if node_req.method == "node.approval.request" {
                                match (
                                    ctx.node_registry.node_identity_by_conn(&conn_id),
                                    ctx.exec_approval_manager.clone(),
                                ) {
                                    (Some((node_id, node_name)), Some(manager)) => {
                                        let event_bus = ctx.event_bus.clone();
                                        let out = rpc_out_tx_replies.clone();
                                        let req_id = node_req.id.clone();
                                        let params =
                                            node_req.params.clone().unwrap_or(Value::Null);
                                        let tool = params
                                            .get("tool")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or_default()
                                            .to_string();
                                        let reason = params
                                            .get("reason")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or_default()
                                            .to_string();
                                        tokio::spawn(async move {
                                            let outcome =
                                                crate::approval::run_node_approval(
                                                    &manager, &event_bus, &node_id,
                                                    &node_name, &tool, &reason,
                                                )
                                                .await;
                                            let resp = JsonRpcResponse::success(
                                                req_id,
                                                serde_json::json!({ "outcome": outcome }),
                                            );
                                            if let Ok(s) = serde_json::to_string(&resp) {
                                                let _ = out.send(s).await;
                                            }
                                        });
                                    }
                                    _ => {
                                        // Not a registered node conn, or no manager
                                        // wired: refuse.
                                        let resp = JsonRpcResponse::error(
                                            node_req.id.clone(),
                                            -32000,
                                            "node.approval.request not permitted".to_string(),
                                        );
                                        if let Ok(s) = serde_json::to_string(&resp) {
                                            let _ = rpc_out_tx_replies.send(s).await;
                                        }
                                    }
                                }
                                continue;
                            }
                        }
```

> Confirm `Value` and `JsonRpcRequest`/`JsonRpcResponse` are in scope in handler.rs (they are — used at lines 517/530). If `Value` is not imported, use `serde_json::Value` inline.

- [ ] **Step 5: Set the shared manager on the server at boot**

In `src/bin/aleph-server/commands/start/mod.rs`: `server` is already `let mut server = GatewayServer::with_config(...)` at `:167`, and the shared manager is built at `:250` (`let exec_approval_manager = Arc::new(...)`). Immediately after line 250, add the direct field assignment (mirrors the existing `server.execution_adapter = ...` style at `:1032`):

```rust
    server.exec_approval_manager = Some(exec_approval_manager.clone());
```

This runs long before `build_router`/serve, so the clone propagates into the per-router `GatewaySharedState`. `exec_approval_manager` stays in scope (it is reused for `OperatorApprovalRequester` wiring at `:2087`+).

- [ ] **Step 6: Build and run the gateway + cluster tests**

Run: `cargo build -p alephcore`
Expected: clean compile.
Run: `cargo test -p alephcore --lib gateway::server::`
Expected: PASS (existing server tests still green — the new field is `None` there).
Run: `cargo build -p alephcore --bin aleph-server`
Expected: clean compile.

- [ ] **Step 7: Commit**

```bash
git add src/gateway/server/mod.rs src/gateway/server/handler.rs src/gateway/server/probe.rs src/bin/aleph-server/commands/start/mod.rs
git commit -m "gateway: route node.approval.request to run_node_approval (thread ExecApprovalManager into ctx)"
```

---

## Task 4: Node binary — wire requester + `run_session` concurrency restructure

**Files:**
- Modify: `src/bin/aleph-server/commands/node.rs` (`build_command_table` `:171`, `handle_node` `:113`, `run_session` `:259`)

- [ ] **Step 1: Update imports**

In `src/bin/aleph-server/commands/node.rs`, add to the imports (top of file, ~:6–22):

```rust
use std::sync::RwLock;
```

and extend the cluster import:

```rust
use alephcore::cluster::{
    CenterApprovalRequester, CommandDescriptor, CommandTable, ApprovalSlot, ReverseRpcChannel,
};
```

(Keep `JsonRpcResponse` import; it is already present.)

- [ ] **Step 2: `build_command_table` returns the table + the approval slot, wiring the requester**

Replace the body of `build_command_table` (`:171`) — change the signature to return the slot and wire a `CenterApprovalRequester` instead of `None`:

```rust
/// Build the node sandbox + command table. The `ApprovalGate` is wired to a
/// `CenterApprovalRequester` whose channel slot is filled per-connection by
/// `run_session`; until a connection is live the slot is `None` and escalations
/// fail-closed (same as the old headless `None` requester).
fn build_command_table(name: &str) -> (CommandTable, ApprovalSlot) {
    let cfg = SandboxConfig::default();
    let driver = create_platform_driver_from_config(&cfg);
    let slot: ApprovalSlot = Arc::new(RwLock::new(None));
    let requester = Arc::new(CenterApprovalRequester::new(slot.clone()));
    let gate = Arc::new(ApprovalGate::new(ApprovalConfig::default(), Some(requester)));
    let sandbox = build_sandbox(
        &cfg,
        driver,
        gate,
        SandboxRateLimitConfig::default(),
        &alephcore::ShellSecurityConfig::default(),
    );
    let bash = alephcore::builtin_tools::BashExecTool::new().with_sandbox(sandbox);
    let session = SessionKey::ephemeral(format!("node-{name}"));
    let workspace_dir =
        alephcore::sandbox::workspace::session_workspace_dir(&cfg.workspace_root, &session);
    let mut table = CommandTable::with_bash(bash, session);
    table.register_file_commands(workspace_dir);
    (table, slot)
}
```

- [ ] **Step 3: `handle_node` threads the slot into `run_session`**

In `handle_node` (`:113`) change:

```rust
    let table = Arc::new(build_command_table(&name));
    let declared = table.descriptors();
```

to:

```rust
    let (table, approval_slot) = build_command_table(&name);
    let table = Arc::new(table);
    let declared = table.descriptors();
```

and update the `run_session` call (`:136`) to pass the slot:

```rust
        match run_session(&url, &bearer, &name, &declared, &table, &approval_slot).await {
```

- [ ] **Step 4: Restructure `run_session` for concurrency**

Replace `run_session` (`:259`–`:293`) entirely:

```rust
async fn run_session(
    url: &str,
    token: &str,
    name: &str,
    declared: &[CommandDescriptor],
    table: &Arc<CommandTable>,
    approval_slot: &ApprovalSlot,
) -> Result<SessionOutcome, Box<dyn std::error::Error>> {
    let (ws, _resp) = tokio_tungstenite::connect_async(url).await?;
    let (mut write, mut read) = ws.split();

    let connect = json!({
        "jsonrpc": "2.0", "id": 1, "method": "connect",
        "params": { "token": token, "device_name": name, "commands": declared }
    });
    write.send(Message::Text(connect.to_string().into())).await?;
    let reply = read
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

    // Bidirectional channel for this connection: outbound mpsc drained by a
    // writer task; a node-side PendingInvokes resolves center responses. This
    // is what lets a blocked bash command (awaiting approval) NOT deadlock the
    // read loop — dispatch runs on spawned tasks while the read loop keeps
    // pumping frames.
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<String>(64);
    let channel = ReverseRpcChannel::new(out_tx.clone());
    let pending = channel.pending();
    *approval_slot.write().unwrap_or_else(|e| e.into_inner()) = Some(channel);

    let writer = tokio::spawn(async move {
        while let Some(frame) = out_rx.recv().await {
            if write.send(Message::Text(frame.into())).await.is_err() {
                break;
            }
        }
    });

    while let Some(msg) = read.next().await {
        let Message::Text(text) = msg? else { continue };
        let text = text.to_string();

        // Center → node RESPONSE (id + result/error, no method): resolve a
        // node-initiated reverse-RPC call (e.g. node.approval.request).
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

    // Connection ended: fail-close the approval slot and stop the writer.
    *approval_slot.write().unwrap_or_else(|e| e.into_inner()) = None;
    writer.abort();
    Ok(SessionOutcome::Ended)
}
```

> `JsonRpcResponse` already imported. `futures_util::StreamExt` (for `read.next()`) and `SinkExt` (for `write.send`) are already imported (`:19`). `ws.split()` comes from `StreamExt`. `handle_frame` is unchanged.

- [ ] **Step 5: Build the binary**

Run: `cargo build -p alephcore --bin aleph-server`
Expected: clean compile.

- [ ] **Step 6: Run the node binary's inline tests**

Run: `cargo test -p alephcore --bin aleph-server`
Expected: PASS (existing frame tests unaffected — `handle_frame` is unchanged).

- [ ] **Step 7: Commit**

```bash
git add src/bin/aleph-server/commands/node.rs
git commit -m "node: wire CenterApprovalRequester + concurrency-restructure run_session"
```

---

## Task 5: Integration test — full in-process round trip

**Files:**
- Create: `tests/cluster_node_approval.rs`

- [ ] **Step 1: Write the integration test**

This wires the node's `CenterApprovalRequester` to a center loop (`run_node_approval` + a mock operator) over an in-process `ReverseRpcChannel`, proving the full byte round trip and outcome mapping without a real socket. Create `tests/cluster_node_approval.rs`:

```rust
//! Cluster ③ integration: node-initiated approval round trip, in-process.
//!
//! node CenterApprovalRequester --(node.approval.request frame)--> center loop
//!   -> run_node_approval -> ExecApprovalManager -> mock operator resolve
//!   -> JSON-RPC response -> node PendingInvokes -> ApprovalOutcome
//!
//! No socket: the node's outbound mpsc is the center's inbound, and the node's
//! PendingInvokes is resolved with the center's response — exactly the frames
//! that would cross the WS.

use std::sync::{Arc, RwLock};
use std::time::Duration;

use alephcore::cluster::{ApprovalSlot, CenterApprovalRequester, ReverseRpcChannel};
use alephcore::exec::manager::ExecApprovalManager;
use alephcore::exec::socket::ApprovalDecisionType;
use alephcore::gateway::event_bus::GatewayEventBus;
use alephcore::gateway::events::GatewayEventFrame;
use alephcore::gateway::protocol::JsonRpcResponse;
use alephcore::sandbox::exec_approval::gate::{ApprovalOutcome, ApprovalRequester};
use serde_json::{json, Value};

/// Spawn a center that services exactly one node.approval.request frame,
/// resolving it with `decision` (or letting it expire if `None`).
fn spawn_center(
    mut out_rx: tokio::sync::mpsc::Receiver<String>,
    pending: Arc<alephcore::cluster::PendingInvokes>,
    manager: Arc<ExecApprovalManager>,
    event_bus: Arc<GatewayEventBus>,
    decision: Option<ApprovalDecisionType>,
) {
    tokio::spawn(async move {
        let frame = out_rx.recv().await.expect("node sent a request frame");
        let req: Value = serde_json::from_str(&frame).unwrap();
        assert_eq!(req["method"], "node.approval.request");
        let id = req["id"].clone();
        let tool = req["params"]["tool"].as_str().unwrap().to_string();
        let reason = req["params"]["reason"].as_str().unwrap().to_string();

        // Mock operator: resolve the pending record as soon as it is published.
        let mgr = manager.clone();
        let bus = event_bus.clone();
        tokio::spawn(async move {
            let mut rx = bus.subscribe_typed();
            for _ in 0..10 {
                let Ok(Ok(f)) = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await
                else {
                    break;
                };
                if let GatewayEventFrame::ApprovalRequested { approval_id, .. } = f {
                    if let Some(d) = decision {
                        mgr.resolve(&approval_id, d, Some("operator".to_string()));
                    }
                    break;
                }
            }
        });

        let outcome = alephcore::approval::run_node_approval(
            &manager, &event_bus, "node-1", "worker", &tool, &reason,
        )
        .await;
        let resp = JsonRpcResponse::success(Some(id.clone()), json!({ "outcome": outcome }));
        pending.resolve(&id, resp);
    });
}

async fn run_case(decision: Option<ApprovalDecisionType>, expect: ApprovalOutcome) {
    let (out_tx, out_rx) = tokio::sync::mpsc::channel::<String>(8);
    let channel = ReverseRpcChannel::new(out_tx);
    let pending = channel.pending();
    let slot: ApprovalSlot = Arc::new(RwLock::new(Some(channel)));
    let requester = CenterApprovalRequester::new(slot);

    let manager = Arc::new(ExecApprovalManager::new());
    let event_bus = Arc::new(GatewayEventBus::new());
    spawn_center(out_rx, pending, manager, event_bus, decision);

    let outcome = requester.request_approval("bash", "needs network").await;
    assert_eq!(outcome, expect);
}

#[tokio::test]
async fn approve_once_round_trip() {
    run_case(Some(ApprovalDecisionType::AllowOnce), ApprovalOutcome::Approved).await;
}

#[tokio::test]
async fn approve_session_round_trip() {
    run_case(
        Some(ApprovalDecisionType::AllowSession),
        ApprovalOutcome::ApprovedForSession,
    )
    .await;
}

#[tokio::test]
async fn deny_round_trip() {
    run_case(Some(ApprovalDecisionType::Deny), ApprovalOutcome::Denied).await;
}
```

> The expiry/timeout case (`None` decision) is intentionally omitted from the integration test because it would block ~120s on the real `DEFAULT_APPROVAL_TIMEOUT_MS`. The timeout→`"timeout"`→`Timeout` mapping is already covered by the unit tests in Tasks 1 & 2.

- [ ] **Step 2: Run the integration test**

Run: `cargo test -p alephcore --test cluster_node_approval`
Expected: PASS (3 tests).

> If any import path is wrong (e.g. `alephcore::approval::run_node_approval` vs a different re-export), fix the path to match the actual `pub use` added in Task 2 — do not change the test's logic.

- [ ] **Step 3: Commit**

```bash
git add tests/cluster_node_approval.rs
git commit -m "test: cluster node-approval full round-trip integration (approve/session/deny)"
```

---

## Final Verification

- [ ] `cargo test -p alephcore --lib cluster::` — node_approval (4) + registry (incl. node_identity_by_conn) green
- [ ] `cargo test -p alephcore --lib approval::node_requester` — driver (2) green
- [ ] `cargo test -p alephcore --lib gateway::server::` — existing server tests green (new field `None`)
- [ ] `cargo test -p alephcore --test cluster_node_approval` — round trip (3) green
- [ ] `cargo test -p alephcore --bin aleph-server` — node frame tests green
- [ ] `cargo build -p alephcore --bin aleph-server` — clean
- [ ] `cargo clippy -p alephcore` — no NEW warnings on touched files
- [ ] R10 check: `git diff --stat main` shows ZERO changes under `src/harness/`

Do NOT merge to main. Leave the worktree + branch for the user to merge per cluster strategy.
```
