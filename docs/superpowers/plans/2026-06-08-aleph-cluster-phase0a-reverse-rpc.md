# Aleph 集群 Phase 0a · 反向 RPC 传输原语 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 Gateway 服务端能对某条已连 WS 客户端发起带 id 的 JSON-RPC 请求并 await 其相关响应（带超时），为后续 `node.invoke`（中心→节点）打地基。

**Architecture:** 新增 `src/cluster/` 模块承载一个纯粹的 `PendingInvokes` 关联表（id→oneshot 发送端）与一个 `ReverseRpcChannel`（出站 `mpsc::Sender<String>` + 关联表，提供 `call()`）。在 `src/gateway/server/handler.rs` 的连接循环里加一条**通用出站 select 臂**（把帧逐字写给该连接的 `write`），并在入站分支里**先识别 JSON-RPC 响应帧**（有 `id`、有 `result`/`error`、无 `method`）路由回关联表，否则才按请求分发。每条连接的 `ReverseRpcChannel` 存进 `GatewaySharedState.reverse_rpc`（`conn_id → channel`），供 0b 的 NodeRegistry 与本期集成测试查找。

**Tech Stack:** Rust, tokio (`mpsc` / `oneshot` / `time::timeout`), axum WebSocket, serde_json, 既有 `JsonRpcRequest`/`JsonRpcResponse`（`src/gateway/protocol.rs`）。

**红线对账：** 纯 `src/cluster/` + `src/gateway/`，零 `src/harness/` 改动（R10）；无新重依赖，全用既有 tokio/serde 栈（R3）；反向 RPC 是 infra，不含任何 LLM 推理（R7）。

**关键约束（来自代码勘查）：**
- `JsonRpcRequest`（protocol.rs:57-69）的 `method` 字段**非 Optional 无 default** → 响应帧反序列化为 `JsonRpcRequest` 会失败。因此入站必须**先试 response**。
- 响应/请求靠**结构**区分（有 `method`=请求；有 `result`/`error`=响应），**不靠 id**，故反向 RPC id 与客户端 id 空间是否重叠都不影响路由。
- 锁中毒按 P7 处理：`.lock().unwrap_or_else(|e| e.into_inner())`。
- 出站帧**不走** EventBus/`PerClientBuffer`（那条路有 topic/scope 过滤会丢弃 RPC 帧），必须用新的专用 mpsc 通道 + 新 select 臂。

---

## File Structure

| 文件 | 动作 | 职责 |
|---|---|---|
| `src/cluster/mod.rs` | Create | 模块根，re-export `PendingInvokes` / `ReverseRpcChannel` / `ReverseRpcError` |
| `src/cluster/reverse_rpc.rs` | Create | `PendingInvokes`（关联表）+ `ReverseRpcChannel`（出站+关联+`call()`）+ `ReverseRpcError` |
| `src/lib.rs` | Modify | `pub mod cluster;` |
| `src/gateway/server/mod.rs` | Modify | `GatewaySharedState` + `GatewayServer` 加 `reverse_rpc` 字段；构造点初始化 |
| `src/gateway/server/handler.rs` | Modify | 连接循环：建出站通道、注册 `ReverseRpcChannel`、加出站 select 臂、入站先识别响应帧、清理时注销 |
| `tests/cluster_reverse_rpc.rs` | Create | 集成测试：真实 WS 一对，服务端对连进来的客户端发起 `tool.call` 并拿回响应 |

---

### Task 1: 模块骨架 + `PendingInvokes::register` / `resolve`

**Files:**
- Create: `src/cluster/mod.rs`
- Create: `src/cluster/reverse_rpc.rs`
- Modify: `src/lib.rs`（加 `pub mod cluster;`，紧邻其它 `pub mod` 声明）

- [ ] **Step 1: Write the failing test**

在 `src/cluster/reverse_rpc.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::protocol::JsonRpcResponse;
    use serde_json::json;

    #[tokio::test]
    async fn register_then_resolve_delivers_response() {
        let pending = PendingInvokes::new();
        let (id, rx) = pending.register();

        // id 是字符串形态的反向 RPC 关联键
        assert!(id.starts_with("rpc-"));

        let resp = JsonRpcResponse::success(Some(json!(id)), json!({"ok": true}));
        let routed = pending.resolve(&json!(id), resp);
        assert!(routed, "resolve should find the pending entry");

        let got = rx.await.expect("sender should not be dropped");
        assert!(got.is_success());
    }

    #[tokio::test]
    async fn resolve_unknown_id_returns_false() {
        let pending = PendingInvokes::new();
        let resp = JsonRpcResponse::success(Some(json!("rpc-999")), json!(null));
        assert!(!pending.resolve(&json!("rpc-999"), resp));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib cluster::reverse_rpc`
Expected: FAIL — `cannot find type PendingInvokes` / `module cluster not found`.

- [ ] **Step 3: Write minimal implementation**

`src/cluster/mod.rs`:

```rust
//! Aleph 集群（单中心非对称节点联邦）。
//!
//! 本模块承载集群的中心侧基础设施。Phase 0a 只实现反向 RPC 传输原语
//! （服务端→已连客户端的带 id 请求/响应关联），后续 Phase 加 NodeRegistry、
//! node.invoke 路由、environments 聚合等。
//!
//! 红线：本模块不含任何 LLM 推理（R7），不进入 `src/harness/`（R10）。

mod reverse_rpc;

pub use reverse_rpc::{PendingInvokes, ReverseRpcChannel, ReverseRpcError};
```

`src/cluster/reverse_rpc.rs`:

```rust
//! 反向 RPC：服务端对某条已连 WS 客户端发起带 id 的 JSON-RPC 请求并
//! await 其相关响应。
//!
//! 请求/响应靠**结构**区分（有 `method`=请求；有 `result`/`error`=响应），
//! 不靠 id —— 因此反向 RPC id 与客户端自身 id 空间重叠也不影响路由。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde_json::Value;
use tokio::sync::oneshot;

use crate::gateway::protocol::JsonRpcResponse;

/// 关联表：反向 RPC 请求 id → 等待其响应的 oneshot 发送端。
///
/// 线程安全；锁中毒按 P7 处理（`unwrap_or_else(|e| e.into_inner())`）。
#[derive(Default)]
pub struct PendingInvokes {
    counter: AtomicU64,
    waiters: Mutex<HashMap<String, oneshot::Sender<JsonRpcResponse>>>,
}

impl PendingInvokes {
    pub fn new() -> Self {
        Self::default()
    }

    /// 分配一个新的反向 RPC id 并登记一个等待者。
    /// 返回 `(id, receiver)`：调用方把 `id` 放进出站请求帧，await `receiver`。
    pub fn register(&self) -> (String, oneshot::Receiver<JsonRpcResponse>) {
        let n = self.counter.fetch_add(1, Ordering::Relaxed);
        let id = format!("rpc-{n}");
        let (tx, rx) = oneshot::channel();
        self.waiters
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.clone(), tx);
        (id, rx)
    }

    /// 把一条响应路由给等待该 id 的调用方。
    /// 返回 `true` 表示命中了一个等待者；`false` 表示无人等待（陌生/过期 id）。
    pub fn resolve(&self, id: &Value, response: JsonRpcResponse) -> bool {
        let Some(key) = id.as_str() else {
            return false;
        };
        let sender = self
            .waiters
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(key);
        match sender {
            Some(tx) => tx.send(response).is_ok(),
            None => false,
        }
    }

    /// 丢弃一个等待者（超时清理用）。返回是否确实移除了条目。
    pub fn cancel(&self, id: &str) -> bool {
        self.waiters
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id)
            .is_some()
    }
}
```

并在 `src/lib.rs` 加 `pub mod cluster;`（与现有 `pub mod gateway;` 等并列）。

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib cluster::reverse_rpc`
Expected: PASS（2 tests）。

- [ ] **Step 5: Commit**

```bash
git add src/cluster/mod.rs src/cluster/reverse_rpc.rs src/lib.rs
git commit -m "cluster: add PendingInvokes reverse-RPC correlation table"
```

---

### Task 2: `ReverseRpcError` + `ReverseRpcChannel` 骨架（出站 + 关联）

**Files:**
- Modify: `src/cluster/reverse_rpc.rs`
- Modify: `src/cluster/mod.rs`（已在 Task 1 re-export 这些名字，无需再改）

- [ ] **Step 1: Write the failing test**

在 `src/cluster/reverse_rpc.rs` 的 `mod tests` 里追加：

```rust
    #[tokio::test]
    async fn channel_call_sends_framed_request_and_returns_response() {
        use serde_json::json;

        // 出站接收端模拟"连接的 write 半边"：读到一帧就把它当请求，
        // 解析出 id，构造响应回灌 resolve。
        let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<String>(8);
        let channel = ReverseRpcChannel::new(out_tx);
        let pending = channel.pending();

        // 在后台扮演"客户端"：收到出站帧→回一条成功响应。
        let bg_pending = pending.clone();
        tokio::spawn(async move {
            let frame = out_rx.recv().await.expect("a frame should be sent");
            let req: serde_json::Value = serde_json::from_str(&frame).unwrap();
            assert_eq!(req["method"], "tool.call");
            let id = req["id"].clone();
            let resp = crate::gateway::protocol::JsonRpcResponse::success(
                Some(id.clone()),
                json!({"echoed": req["params"]["tool"]}),
            );
            bg_pending.resolve(&id, resp);
        });

        let resp = channel
            .call("tool.call", json!({"tool": "bash"}), 1_000)
            .await
            .expect("call should resolve");
        assert!(resp.is_success());
        assert_eq!(resp.result.unwrap()["echoed"], "bash");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib cluster::reverse_rpc`
Expected: FAIL — `cannot find type ReverseRpcChannel` / `ReverseRpcError`.

- [ ] **Step 3: Write minimal implementation**

在 `src/cluster/reverse_rpc.rs` 顶部 imports 追加：

```rust
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value as JsonValue;
use tokio::sync::mpsc;

use crate::gateway::protocol::JsonRpcRequest;
```

在文件中（`PendingInvokes` 之后）加：

```rust
/// 反向 RPC 调用失败原因。
#[derive(Debug, thiserror::Error)]
pub enum ReverseRpcError {
    /// 出站通道已关闭（对端连接已断）。
    #[error("reverse-rpc transport closed")]
    TransportClosed,
    /// 在超时内没等到响应。
    #[error("reverse-rpc call timed out after {0}ms")]
    Timeout(u64),
    /// 等待端被丢弃（连接清理时取消了 pending）。
    #[error("reverse-rpc call cancelled")]
    Cancelled,
}

/// 绑定到**单条连接**的反向 RPC 通道：把请求帧写进该连接的出站 mpsc，
/// 并通过共享的 [`PendingInvokes`] 等待相关响应。
#[derive(Clone)]
pub struct ReverseRpcChannel {
    outbound: mpsc::Sender<String>,
    pending: Arc<PendingInvokes>,
}

impl ReverseRpcChannel {
    /// 用一条连接的出站发送端构造通道（新建独立的 pending 表）。
    pub fn new(outbound: mpsc::Sender<String>) -> Self {
        Self {
            outbound,
            pending: Arc::new(PendingInvokes::new()),
        }
    }

    /// 拿到共享的 pending 表。连接的入站循环用它把响应帧 `resolve` 回来。
    pub fn pending(&self) -> Arc<PendingInvokes> {
        self.pending.clone()
    }

    /// 对连接发起一次反向 RPC 请求并等待响应。
    ///
    /// `timeout_ms` 到点仍无响应 → `Timeout`（并清理 pending 条目）。
    pub async fn call(
        &self,
        method: &str,
        params: JsonValue,
        timeout_ms: u64,
    ) -> Result<JsonRpcResponse, ReverseRpcError> {
        let (id, rx) = self.pending.register();
        let req = JsonRpcRequest::with_id(method, Some(params), JsonValue::String(id.clone()));
        let frame = serde_json::to_string(&req).map_err(|_| ReverseRpcError::TransportClosed)?;

        if self.outbound.send(frame).await.is_err() {
            self.pending.cancel(&id);
            return Err(ReverseRpcError::TransportClosed);
        }

        match tokio::time::timeout(Duration::from_millis(timeout_ms), rx).await {
            Ok(Ok(resp)) => Ok(resp),
            Ok(Err(_)) => Err(ReverseRpcError::Cancelled), // sender dropped
            Err(_) => {
                self.pending.cancel(&id);
                Err(ReverseRpcError::Timeout(timeout_ms))
            }
        }
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib cluster::reverse_rpc`
Expected: PASS（3 tests）。

- [ ] **Step 5: Commit**

```bash
git add src/cluster/reverse_rpc.rs
git commit -m "cluster: add ReverseRpcChannel with timeout-bounded call()"
```

---

### Task 3: 超时与传输关闭的错误路径测试

**Files:**
- Modify: `src/cluster/reverse_rpc.rs`（仅追加测试，无生产代码改动 —— 验证 Task 2 行为）

- [ ] **Step 1: Write the failing test**

追加到 `mod tests`：

```rust
    #[tokio::test]
    async fn call_times_out_when_no_response() {
        use serde_json::json;
        // 出站接收端保活但永不回响应 → 必须超时（而非永久挂起）。
        let (out_tx, _out_rx_keepalive) = tokio::sync::mpsc::channel::<String>(8);
        let channel = ReverseRpcChannel::new(out_tx);

        let err = channel
            .call("tool.call", json!({}), 50)
            .await
            .expect_err("must time out");
        assert!(matches!(err, ReverseRpcError::Timeout(50)));
    }

    #[tokio::test]
    async fn call_fails_when_transport_closed() {
        use serde_json::json;
        let (out_tx, out_rx) = tokio::sync::mpsc::channel::<String>(8);
        drop(out_rx); // 立刻关闭出站 → send 失败
        let channel = ReverseRpcChannel::new(out_tx);

        let err = channel
            .call("tool.call", json!({}), 1_000)
            .await
            .expect_err("must fail closed");
        assert!(matches!(err, ReverseRpcError::TransportClosed));
    }
```

- [ ] **Step 2: Run test to verify it fails (or compiles-then-passes)**

Run: `cargo test -p alephcore --lib cluster::reverse_rpc`
Expected: 这两个测试应直接 PASS（Task 2 已实现超时/关闭路径）。若任一 FAIL，说明 Task 2 的 `call()` 路径有 bug，回到 Task 2 修复后再继续。

- [ ] **Step 3: (no new impl — behavior already implemented in Task 2)**

无需新增生产代码。本任务是对错误路径的回归锁定。

- [ ] **Step 4: Run full module tests**

Run: `cargo test -p alephcore --lib cluster::reverse_rpc`
Expected: PASS（5 tests）。

- [ ] **Step 5: Commit**

```bash
git add src/cluster/reverse_rpc.rs
git commit -m "cluster: lock reverse-rpc timeout and transport-closed error paths"
```

---

### Task 4: `GatewaySharedState` / `GatewayServer` 加 `reverse_rpc` 注册表

**Files:**
- Modify: `src/gateway/server/mod.rs`（`GatewaySharedState` 结构体 + `GatewayServer` 结构体 + `new`/`with_config`/`build_router` 三处构造）

**说明：** 新增一个 `conn_id → ReverseRpcChannel` 的共享表，供入站循环登记/查找/注销，并供 0b 的 NodeRegistry 与本期集成测试按 conn_id 取用。

- [ ] **Step 1: Write the failing test**

在 `src/gateway/server/mod.rs` 的 `#[cfg(test)] mod tests` 追加：

```rust
    #[tokio::test]
    async fn reverse_rpc_registry_is_empty_on_fresh_server() {
        let addr = "127.0.0.1:0".parse().unwrap();
        let server = GatewayServer::new(addr);
        assert_eq!(server.reverse_rpc.read().await.len(), 0);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib gateway::server::tests::reverse_rpc_registry_is_empty_on_fresh_server`
Expected: FAIL — `no field reverse_rpc on type GatewayServer`.

- [ ] **Step 3: Write minimal implementation**

3a. 在 `GatewaySharedState`（mod.rs:128-202）末尾字段后加：

```rust
    /// 每条连接的反向 RPC 通道（`conn_id → channel`）。连接建立时由入站
    /// 循环登记，断开时注销。0b 的 NodeRegistry 经此对 node 连接发起
    /// `node.invoke`；本表本身不区分 node/非 node —— 仅是传输层注册表。
    pub reverse_rpc:
        Arc<RwLock<HashMap<String, crate::cluster::ReverseRpcChannel>>>,
```

3b. 在 `GatewayServer`（mod.rs:282-362）末尾字段后加同名字段：

```rust
    /// 见 [`GatewaySharedState::reverse_rpc`]。`build_router` 把它 clone 进
    /// 共享状态，因此两侧指向同一张表。
    pub reverse_rpc:
        Arc<RwLock<HashMap<String, crate::cluster::ReverseRpcChannel>>>,
```

3c. 在 `GatewayServer::new`（mod.rs:377-411 的结构体字面量）和 `with_config`（mod.rs:427-461 的结构体字面量）各加一行初始化：

```rust
            reverse_rpc: Arc::new(RwLock::new(HashMap::new())),
```

3d. 在 `build_router` 的 `GatewaySharedState { ... }` 字面量（mod.rs:563-593）加：

```rust
            reverse_rpc: self.reverse_rpc.clone(),
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib gateway::server::tests::reverse_rpc_registry_is_empty_on_fresh_server`
Expected: PASS。

- [ ] **Step 5: Commit**

```bash
git add src/gateway/server/mod.rs
git commit -m "gateway: add per-connection reverse_rpc channel registry to shared state"
```

---

### Task 5: 连接循环接出站通道 + 出站 select 臂 + 入站响应识别 + 清理

**Files:**
- Modify: `src/gateway/server/handler.rs`

**说明：** 这是 0a 唯一触碰热路径的改动，必须最小化。四处插入：(a) 建出站 mpsc 通道并登记 `ReverseRpcChannel`；(b) 入站 Text 分支最前面识别响应帧→`resolve`→`continue`；(c) 新增一条出站 select 臂把帧逐字写给 `write`；(d) 清理段注销注册表。

- [ ] **Step 1: Write the failing test (integration — drives the full wiring)**

该任务的验收测试在 Task 6（集成测试）。本步先做**最小可编译断言**：在 `handler.rs` 现有逻辑外不便单测，故本任务以"编译通过 + Task 6 集成测试通过"为验收。先创建 Task 6 的测试文件骨架使其 FAIL（见 Task 6 Step 1），再回到本任务实现。

- [ ] **Step 2: Run to verify current state fails**

Run: `cargo test -p alephcore --test cluster_reverse_rpc`
Expected: FAIL（Task 6 测试存在但 wiring 未完成，调用 `reverse_rpc` 取不到 channel 或超时）。

- [ ] **Step 3: Write minimal implementation**

3a. **建出站通道并登记**（handler.rs:442 附近，`PerClientBuffer::new()` 之后、`forward_bus_to_client` spawn 之后、`Initialize connection state` 块之前）插入：

```rust
    // Reverse-RPC outbound channel for this connection. Frames pushed here are
    // written verbatim to the socket by the dedicated select arm below (they
    // bypass the EventBus topic/scope filtering, which would drop RPC frames).
    // Registered under conn_id so the NodeRegistry (Phase 0b) and reverse-RPC
    // callers can reach this specific connection; deregistered on cleanup.
    let (rpc_out_tx, mut rpc_out_rx) = tokio::sync::mpsc::channel::<String>(64);
    let rpc_channel = crate::cluster::ReverseRpcChannel::new(rpc_out_tx);
    let rpc_pending = rpc_channel.pending();
    {
        let mut reg = ctx.reverse_rpc.write().await;
        reg.insert(conn_id.clone(), rpc_channel);
    }
```

> 这要求 `ConnectionContext`（handler.rs:46-102）新增字段
> `reverse_rpc: Arc<RwLock<HashMap<String, crate::cluster::ReverseRpcChannel>>>`，
> 并在 `ws_upgrade_handler` 构造 `ConnectionContext` 处（handler.rs:355-377）加
> `reverse_rpc: state.reverse_rpc.clone(),`。一并加上。

3b. **入站响应识别**（handler.rs:478 `Some(Ok(WsMessage::Text(text))) =>` 分支体最前面，`debug!("WS recv ...")` 之后、`let request: Result<JsonRpcRequest, _>` 之前）插入：

```rust
                        // Reverse-RPC response interception: a frame that is a
                        // JSON-RPC *response* (has `id` + `result`/`error`, no
                        // `method`) is the reply to a server-initiated request.
                        // Route it to the pending table and stop — do NOT treat
                        // it as a client request (it would fail JsonRpcRequest
                        // parsing, which requires `method`).
                        if let Ok(maybe_resp) =
                            serde_json::from_str::<crate::gateway::protocol::JsonRpcResponse>(&text)
                        {
                            let looks_like_response = maybe_resp.id.is_some()
                                && (maybe_resp.result.is_some() || maybe_resp.error.is_some());
                            if looks_like_response {
                                if let Some(id) = maybe_resp.id.clone() {
                                    rpc_pending.resolve(&id, maybe_resp);
                                }
                                continue;
                            }
                        }
```

> 注意：`continue` 在 `tokio::select!` 的分支体内合法（它 continue 外层 `loop`）。`JsonRpcResponse` 已 `Deserialize`（protocol.rs:113）。一条普通客户端请求 `{method,...}` 因 `result`/`error` 皆 `None` 不会被误判。

3c. **出站 select 臂**（handler.rs 的 `tokio::select! { ... }` 内，与 `event = client_event_rx.recv()` 臂并列，建议放在它之后、`_ = ping_timer.tick()` 之前）加：

```rust
            // Reverse-RPC outbound: write server-initiated frames verbatim.
            // These are full JSON-RPC request strings produced by
            // ReverseRpcChannel::call(); no filtering, no wrapping.
            frame = rpc_out_rx.recv() => {
                match frame {
                    Some(text) => {
                        if let Err(e) = write.send(WsMessage::Text(text.into())).await {
                            error!("Failed to send reverse-rpc frame to {}: {}", conn_id, e);
                            break;
                        }
                    }
                    None => {
                        // All ReverseRpcChannel senders dropped (registry entry
                        // removed). Nothing more to push; keep serving inbound.
                    }
                }
            }
```

3d. **清理注销**（handler.rs 末尾 cleanup 段，与现有 `ctx.connections.write()` 清理并列，约 mod.rs handler.rs:1294 起的 Cleanup 块内）加：

```rust
    {
        let mut reg = ctx.reverse_rpc.write().await;
        reg.remove(&conn_id);
    }
```

- [ ] **Step 4: Run to verify it compiles + Task 6 passes**

Run: `cargo test -p alephcore --test cluster_reverse_rpc`
Expected: 需 Task 6 测试已就绪；本步与 Task 6 Step 4 合并验证。先确保 `cargo check -p alephcore --all-targets` 通过。

- [ ] **Step 5: Commit**

```bash
git add src/gateway/server/handler.rs
git commit -m "gateway: wire per-connection reverse-rpc outbound arm + response interception"
```

---

### Task 6: 集成测试 —— 真实 WS 上服务端调用客户端方法

**Files:**
- Create: `tests/cluster_reverse_rpc.rs`

**说明：** 起一个真实 `GatewayServer`（auth 关闭以隔离传输逻辑），用 `tokio-tungstenite` 连一个客户端，客户端 `connect` 后保持在线并扮演"会应答 `tool.call` 的节点"；测试主体从 `server.reverse_rpc` 取出该连接的 channel，`call("tool.call", …)`，断言拿回客户端构造的响应。

- [ ] **Step 1: Write the failing test**

`tests/cluster_reverse_rpc.rs`：

```rust
//! 集成测试：反向 RPC 端到端（服务端 → 已连 WS 客户端）。

use std::net::SocketAddr;
use std::time::Duration;

use alephcore::gateway::server::GatewayServer;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio_tungstenite::tungstenite::Message;

#[tokio::test]
async fn server_calls_method_on_connected_client_and_gets_response() {
    // 1) 起服务端（auth 默认；本测试走 no-auth 路径需确认 AuthMode）。
    //    GatewayServer::new 默认 AuthMode::Token；为隔离传输，用 with_config
    //    构造 AuthMode::None。
    use alephcore::gateway::config::AuthMode;
    use alephcore::gateway::server::GatewayConfig;
    let mut config = GatewayConfig::default();
    config.auth_mode = AuthMode::None;
    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();

    // 绑定到随机端口并取回实际地址：用 run_until_shutdown 前先 bind。
    // GatewayServer::run 内部 bind，这里改用显式 axum::serve 以拿端口。
    let server = std::sync::Arc::new(GatewayServer::with_config(addr, config));
    let router = server.build_router();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let bound = listener.local_addr().unwrap();
    let reverse_rpc = server.reverse_rpc.clone();
    tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });

    // 2) 客户端连接 + connect 握手 + 保持在线扮演应答节点。
    let url = format!("ws://{bound}/ws");
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    ws.send(Message::Text(
        json!({"jsonrpc":"2.0","id":1,"method":"connect",
               "params":{"device_name":"test-node","device_id":"node-test"}})
        .to_string()
        .into(),
    ))
    .await
    .unwrap();
    // 读 connect 响应
    let _connect_resp = ws.next().await.unwrap().unwrap();

    // 客户端后台：收到 tool.call 请求 → 回成功响应（回显 tool 名）。
    let client_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = ws.next().await {
            if let Message::Text(text) = msg {
                let v: serde_json::Value = serde_json::from_str(&text).unwrap();
                if v["method"] == "tool.call" {
                    let id = v["id"].clone();
                    let tool = v["params"]["tool"].clone();
                    ws.send(Message::Text(
                        json!({"jsonrpc":"2.0","id":id,
                               "result":{"echoed":tool}})
                        .to_string()
                        .into(),
                    ))
                    .await
                    .unwrap();
                    break;
                }
            }
        }
    });

    // 3) 等连接登记进 reverse_rpc（connect 完成后入站循环已 insert）。
    let channel = wait_for_one_channel(&reverse_rpc).await;

    // 4) 服务端发起反向 RPC。
    let resp = channel
        .call("tool.call", json!({"tool": "bash"}), 2_000)
        .await
        .expect("reverse rpc should resolve");

    assert!(resp.is_success());
    assert_eq!(resp.result.unwrap()["echoed"], "bash");
    client_task.await.unwrap();
}

async fn wait_for_one_channel(
    reg: &std::sync::Arc<
        tokio::sync::RwLock<
            std::collections::HashMap<String, alephcore::cluster::ReverseRpcChannel>,
        >,
    >,
) -> alephcore::cluster::ReverseRpcChannel {
    for _ in 0..50 {
        if let Some((_, ch)) = reg.read().await.iter().next() {
            return ch.clone();
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("no reverse_rpc channel registered within timeout");
}
```

> 验证点：`GatewayServer::reverse_rpc`、`GatewayServer::build_router`、
> `alephcore::cluster::ReverseRpcChannel` 必须是 `pub` 且可达。若 `gateway::server`
> 未 re-export `GatewayServer`/`GatewayConfig`，按现有 `pub use` 风格补。
> `AuthMode::None` 名称以 `src/gateway/config.rs` 实际枚举为准（若是 `AuthMode::Disabled`
> 则相应调整）。

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --test cluster_reverse_rpc`
Expected: FAIL（编译期：字段/类型未 pub，或运行期：未实现 wiring 时 channel 取不到 / call 超时）。

- [ ] **Step 3: Make it pass**

完成 Task 5 的全部 wiring 后本测试应通过。若 `AuthMode`/re-export 名称不符，按编译器提示修正测试与必要的 `pub use`（不改变行为）。

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --test cluster_reverse_rpc -- --nocapture`
Expected: PASS。

- [ ] **Step 5: Commit**

```bash
git add tests/cluster_reverse_rpc.rs
git commit -m "test: end-to-end reverse-rpc over real websocket (center calls connected client)"
```

---

### Task 7: 全量校验 + clippy + fmt

**Files:** 无（仅校验）

- [ ] **Step 1: 全量编译（含集成测试）**

Run: `cargo check -p alephcore --all-targets`
Expected: 无错误。（注意 `--all-targets` 才会编译 `tests/`，`cargo check -p alephcore` 不会。）

- [ ] **Step 2: 跑本期相关测试**

Run: `cargo test -p alephcore --lib cluster::reverse_rpc && cargo test -p alephcore --test cluster_reverse_rpc`
Expected: 全绿。

- [ ] **Step 3: clippy（本期文件零警告）**

Run: `cargo clippy -p alephcore --all-targets -- -D warnings`
Expected: 无 `src/cluster/` / 改动处的新增警告。

- [ ] **Step 4: fmt**

Run: `cargo fmt`
Expected: 无改动或仅本期文件格式化。

- [ ] **Step 5: Commit（若 fmt 有改动）**

```bash
git add -u
git commit -m "cluster: rustfmt phase-0a reverse-rpc"
```

---

## Self-Review

**1. Spec coverage（对照 spec 第 8 节"传输/反向 RPC"四项需新增）：**
- pending 关联表 → Task 1（`PendingInvokes`）。✓
- 服务端下发 → Task 5 (3a/3c) 出站通道 + select 臂；`ReverseRpcChannel::call` 组帧 → Task 2。✓
- 客户端路由回 → Task 5 (3b) 入站响应识别 + `resolve`。✓
- 超时/取消 → Task 2（`timeout` + `cancel`）+ Task 3（错误路径回归）。✓
- 共享可达（供 0b NodeRegistry）→ Task 4（`GatewaySharedState.reverse_rpc`）。✓

**2. Placeholder scan：** 无 TBD/TODO；每个代码步给出完整代码。两处显式标注"以实际枚举/re-export 名为准"（`AuthMode::None`、`gateway::server` re-export）属真实不确定项，已给出判定方法与回退，非占位符。

**3. Type consistency：** `PendingInvokes`（`register`/`resolve`/`cancel`）、`ReverseRpcChannel`（`new`/`pending`/`call`）、`ReverseRpcError`（`TransportClosed`/`Timeout(u64)`/`Cancelled`）在 Task 1-6 全程一致。`reverse_rpc: Arc<RwLock<HashMap<String, ReverseRpcChannel>>>` 在 mod.rs（Task 4）、handler.rs（Task 5）、集成测试（Task 6）三处签名一致。响应识别用 `JsonRpcResponse`（`id`+`result|error`），与 protocol.rs 字段一致。

**已知后续依赖（非本期）：** 本期不引入 node 概念/不改 connect/不注册任何工具；`reverse_rpc` 表对所有连接都建 channel（对非 node 连接无害，永不被 `call`）。0b 将在此表之上建 NodeRegistry（node→conn_id 映射）。
