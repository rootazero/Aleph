//! 反向 RPC：服务端对某条已连 WS 客户端发起带 id 的 JSON-RPC 请求并
//! await 其相关响应。
//!
//! 请求/响应靠**结构**区分（有 `method`=请求；有 `result`/`error`=响应），
//! 不靠 id —— 因此反向 RPC id 与客户端自身 id 空间重叠也不影响路由。

use std::collections::HashMap;
use std::time::Duration;

use crate::sync_primitives::{Arc, Mutex};
use crate::sync_primitives::{AtomicU64, Ordering};

use serde_json::Value;
use tokio::sync::{mpsc, oneshot, Notify};

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse};

/// 关联表：反向 RPC 请求 id → 等待其响应的 oneshot 发送端。
///
/// 线程安全；锁中毒按 P7 处理（`unwrap_or_else(|e| e.into_inner())`）。
#[derive(Default)]
pub struct PendingInvokes {
    counter: AtomicU64,
    waiters: Mutex<HashMap<String, oneshot::Sender<JsonRpcResponse>>>,
}

impl PendingInvokes {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 分配一个新的反向 RPC id 并登记一个等待者。
    /// 返回 `(id, receiver)`：调用方把 `id` 放进出站请求帧，await `receiver`。
    pub fn register(&self) -> (String, oneshot::Receiver<JsonRpcResponse>) {
        // Relaxed: id uniqueness only; the Mutex insert below provides the
        // cross-thread ordering for the HashMap.
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
    /// 返回 `true` 表示该 id 存在等待条目（即使其接收端已被丢弃，例如调用方已
    /// 超时——仍算已处理的反向 RPC 响应）；`false` 表示无此 id（陌生/已解析）。
    ///
    /// Returns `true` iff an entry existed for this id (even if its receiver was
    /// already dropped); `false` means no such id (unknown/already resolved).
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
            Some(tx) => {
                // Best-effort delivery: a dropped receiver (caller timed out)
                // still counts as a known id — return true so callers treat the
                // frame as a handled reverse-RPC response, not an unknown frame.
                let _ = tx.send(response);
                true
            }
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

    /// 丢弃**全部**等待者（连接断开清理用）。返回被取消的条目数。
    ///
    /// 排空 `waiters` 把所有 oneshot `Sender` 一并 drop —— 每个仍在
    /// [`ReverseRpcChannel::call`] 里 await 的调用方随即收到 `RecvError`，被映射成
    /// [`ReverseRpcError::Cancelled`] **立即返回**，而非空等满 `timeout_ms`
    /// （节点掉线时最长 ≤130s）。对应 openclaw `node-registry.unregister()` 在断线时
    /// 立刻 reject 所有 pending node invoke 的语义。
    pub fn cancel_all(&self) -> usize {
        let mut waiters = self.waiters.lock().unwrap_or_else(|e| e.into_inner());
        let n = waiters.len();
        waiters.clear();
        n
    }
}

/// 反向 RPC 调用失败原因。
#[derive(Debug, thiserror::Error)]
pub enum ReverseRpcError {
    /// 出站通道已关闭（对端连接已断）。
    #[error("reverse-rpc transport closed")]
    TransportClosed,
    /// 帧**已投递**给节点，但在预算内没等到响应——节点健康、只是命令慢。
    /// 与 [`OutboundWedged`](Self::OutboundWedged) 类型级区分：调用方据此可知
    /// 是「等结果超时」而非「socket 死」，长命令（大 `timeout_ms`）落此支属正常。
    #[error("reverse-rpc call timed out after {0}ms (no response)")]
    Timeout(u64),
    /// 帧**压不进出站队列**：writer 卡在 `send` 上（对端 TCP 停止收字节 = 慢消费者/
    /// 半开连接），有界 mpsc 灌满、`send().await` 整段预算内都拿不到容量。这是
    /// socket 级背压失败，映射 openclaw `rejectSlowNodeSocket` 的 `bufferedAmount`
    /// 信号——与「节点慢」（[`Timeout`](Self::Timeout)）性质不同：装了 close 信号的
    /// 中心侧通道会据此**主动关掉这条卡死连接**（见 [`ReverseRpcChannel::with_close`]）。
    #[error("reverse-rpc outbound wedged after {0}ms (node socket not draining)")]
    OutboundWedged(u64),
    /// 等待端被丢弃：连接清理时 [`PendingInvokes::cancel_all`] 取消了所有 pending。
    /// 节点掉线后在途 `node_invoke` / `node_file` / 审批调用即时收到此错误（fail-fast），
    /// 不再空等满 `timeout_ms`。
    #[error("reverse-rpc call cancelled (node disconnected)")]
    Cancelled,
    /// 序列化请求帧失败（理论上不可能发生）。
    #[error("failed to serialize JsonRpcRequest: {0}")]
    Serialize(#[from] serde_json::Error),
}

/// 绑定到**单条连接**的反向 RPC 通道：把请求帧写进该连接的出站 mpsc，
/// 并通过共享的 [`PendingInvokes`] 等待相关响应。
#[derive(Clone)]
pub struct ReverseRpcChannel {
    outbound: mpsc::Sender<String>,
    pending: Arc<PendingInvokes>,
    /// 可选的「关连接」信号。装了它的通道在出站卡死（[`ReverseRpcError::OutboundWedged`]）
    /// 时会 `notify_one()`，让拥有这条连接的读循环退出、跑完整清理（deregister +
    /// `node.disconnected` 事件 + `cancel_all` + 关 socket），节点随后退避重连自愈。
    /// `None`（[`new`](Self::new)）= 纯传输语义，卡死只回错误不关连接（节点侧 / 测试）。
    close: Option<Arc<Notify>>,
}

impl ReverseRpcChannel {
    /// 用一条连接的出站发送端构造通道（新建独立的 pending 表）。无 close 信号：
    /// 出站卡死只回 [`ReverseRpcError::OutboundWedged`]、不触发关连接（节点侧拨出
    /// 通道与单测用此——重连由各自的 run loop 自管）。
    #[must_use]
    pub fn new(outbound: mpsc::Sender<String>) -> Self {
        Self {
            outbound,
            pending: Arc::new(PendingInvokes::new()),
            close: None,
        }
    }

    /// 同 [`new`](Self::new)，但绑定一个「关连接」信号（中心侧每连接一个）。出站
    /// 卡死时 [`call`](Self::call) 除回 [`OutboundWedged`](ReverseRpcError::OutboundWedged)
    /// 外还 `notify_one()` 该信号，令连接读循环退出并跑完整清理——把半开/慢消费者
    /// 连接从舰队里摘除（映射 openclaw `rejectSlowNodeSocket`）。idle-watchdog 只看
    /// 入站活动，对「center→node 写死、node→center 读活」的半开卡死永不触发，故这条
    /// 主动关闭是唯一能收走该僵尸连接的路径。
    #[must_use]
    pub fn with_close(outbound: mpsc::Sender<String>, close: Arc<Notify>) -> Self {
        Self {
            outbound,
            pending: Arc::new(PendingInvokes::new()),
            close: Some(close),
        }
    }

    /// 拿到共享的 pending 表。连接的入站循环用它把响应帧 `resolve` 回来。
    #[must_use]
    pub fn pending(&self) -> Arc<PendingInvokes> {
        self.pending.clone()
    }

    /// 对连接发起一次反向 RPC 请求并等待响应。
    ///
    /// `timeout_ms` 是**整次调用**的预算，覆盖「把帧压进出站队列」与「等响应」两段。
    ///
    /// 出站是**有界** mpsc（由该连接的 writer task 排空）。若对端 TCP 停止收字节
    /// （慢消费者 / 半开连接），writer 会卡在 `send` 上、队列灌满，此时
    /// `outbound.send().await` 会**无限期挂起**——旧实现在这里没有超时，等于把
    /// `timeout_ms` 契约悄悄作废，调用方（`node_invoke` / 审批）永久挂死。现在入队
    /// 也在预算内，满队列到点即 [`OutboundWedged`](ReverseRpcError::OutboundWedged)
    /// （与「等响应超时」= [`Timeout`](ReverseRpcError::Timeout) 类型级区分）；若本通道
    /// 经 [`with_close`](Self::with_close) 装了 close 信号，还会**主动关掉这条卡死连接**，
    /// 把它从舰队摘除、放节点退避重连——否则半开写卡死会无限占着 registry 一个在线位。
    pub async fn call(
        &self,
        method: &str,
        params: Value,
        timeout_ms: u64,
    ) -> Result<JsonRpcResponse, ReverseRpcError> {
        let (id, rx) = self.pending.register();
        let req = JsonRpcRequest::with_id(method, Some(params), Value::String(id.clone()));
        let frame = serde_json::to_string(&req)?;

        let budget = Duration::from_millis(timeout_ms);
        let deadline = tokio::time::Instant::now() + budget;

        match tokio::time::timeout_at(deadline, self.outbound.send(frame)).await {
            // Receiver gone: the connection's writer task is finished.
            Ok(Err(_)) => {
                self.pending.cancel(&id);
                return Err(ReverseRpcError::TransportClosed);
            }
            // Never got the frame queued within the budget — a wedged peer
            // (writer stuck on a socket the peer stopped draining). Distinct from
            // a slow *response*: the frame did not even reach the wire. Ask the
            // owning connection to tear itself down so the zombie is reaped now
            // rather than occupying a registry slot until (or past) the inbound
            // idle-watchdog, which never fires for a half-open write-wedge.
            Err(_) => {
                self.pending.cancel(&id);
                if let Some(close) = &self.close {
                    close.notify_one();
                }
                return Err(ReverseRpcError::OutboundWedged(timeout_ms));
            }
            Ok(Ok(())) => {}
        }

        match tokio::time::timeout_at(deadline, rx).await {
            Ok(Ok(resp)) => Ok(resp),
            Ok(Err(_)) => Err(ReverseRpcError::Cancelled), // sender dropped
            Err(_) => {
                self.pending.cancel(&id);
                Err(ReverseRpcError::Timeout(timeout_ms))
            }
        }
    }
}

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

    #[tokio::test]
    async fn channel_call_sends_framed_request_and_returns_response() {
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

    #[tokio::test]
    async fn call_times_out_when_no_response() {
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
    async fn cancel_all_drops_every_waiter_so_receivers_resolve_err() {
        // 模拟连接断开清理：cancel_all 排空全部等待者 → 每个 receiver 立即以
        // RecvError 解析（call() 会把它映射成 Cancelled）。
        let pending = PendingInvokes::new();
        let (_id1, rx1) = pending.register();
        let (_id2, rx2) = pending.register();

        assert_eq!(
            pending.cancel_all(),
            2,
            "should report both waiters cancelled"
        );
        assert!(rx1.await.is_err(), "sender dropped → receiver errors");
        assert!(rx2.await.is_err(), "sender dropped → receiver errors");

        // 幂等：再次清空无残留。
        assert_eq!(pending.cancel_all(), 0);
    }

    #[tokio::test]
    async fn inflight_call_returns_cancelled_after_cancel_all() {
        // 出站保活但永不回响应；cancel_all 必须让在途 call 即时返回 Cancelled
        // 而非空等满 timeout。
        let (out_tx, _out_rx_keepalive) = tokio::sync::mpsc::channel::<String>(8);
        let channel = ReverseRpcChannel::new(out_tx);
        let pending = channel.pending();

        let call = tokio::spawn(async move {
            // 大 timeout：若 cancel_all 没接好，本调用会挂到测试超时。
            channel.call("tool.call", json!({}), 60_000).await
        });

        // 自旋等待该 call 注册其 waiter（register 后 waiters 非空），再 cancel_all。
        // 比固定 sleep 更稳，避免 subscribe-before-publish 式时序竞态。
        loop {
            if pending.cancel_all() > 0 {
                break;
            }
            tokio::task::yield_now().await;
        }

        let err = call
            .await
            .expect("task joins")
            .expect_err("must be cancelled");
        assert!(matches!(err, ReverseRpcError::Cancelled));
    }

    #[tokio::test]
    async fn call_times_out_instead_of_hanging_on_a_wedged_outbound_queue() {
        // A peer whose socket stopped draining: the writer task never pulls from
        // the queue, so it fills up. `send().await` would block forever without a
        // budget — the whole point of `timeout_ms` is that it cannot.
        let (out_tx, _out_rx_never_drained) = tokio::sync::mpsc::channel::<String>(1);
        // Fill the single slot so the next send must wait for capacity.
        out_tx.send("stale frame".to_string()).await.unwrap();

        let channel = ReverseRpcChannel::new(out_tx);
        let err = channel
            .call("tool.call", json!({}), 50)
            .await
            .expect_err("a wedged queue must time out, not hang");
        // Enqueue-wedge is typed distinctly from a slow *response* (Timeout): the
        // frame never reached the wire. A plain `new` channel just reports it.
        assert!(matches!(err, ReverseRpcError::OutboundWedged(50)), "{err:?}");
    }

    #[tokio::test]
    async fn wedged_outbound_notifies_close_signal_on_with_close_channel() {
        // A with_close channel over a wedged queue must, in addition to returning
        // OutboundWedged, fire its close signal so the owning connection tears
        // down (maps openclaw rejectSlowNodeSocket). A slow *response* would not.
        let (out_tx, _never_drained) = tokio::sync::mpsc::channel::<String>(1);
        out_tx.send("stale frame".to_string()).await.unwrap();
        let close = Arc::new(Notify::new());
        let channel = ReverseRpcChannel::with_close(out_tx, close.clone());

        let err = channel
            .call("tool.call", json!({}), 50)
            .await
            .expect_err("wedged queue must error");
        assert!(matches!(err, ReverseRpcError::OutboundWedged(50)), "{err:?}");
        // notify_one() before the waiter still leaves a stored permit, so this
        // resolves immediately — no lost wakeup.
        tokio::time::timeout(Duration::from_secs(1), close.notified())
            .await
            .expect("close signal must have been fired by the wedge");
    }

    #[tokio::test]
    async fn slow_response_does_not_fire_close_signal() {
        // Frame enqueues fine (receiver alive, capacity 8) but no response comes:
        // that's a healthy-but-slow node (Timeout), NOT a wedge — the connection
        // must NOT be torn down, so the close signal must stay unfired.
        let (out_tx, _keepalive) = tokio::sync::mpsc::channel::<String>(8);
        let close = Arc::new(Notify::new());
        let channel = ReverseRpcChannel::with_close(out_tx, close.clone());

        let err = channel
            .call("tool.call", json!({}), 50)
            .await
            .expect_err("must time out waiting for response");
        assert!(matches!(err, ReverseRpcError::Timeout(50)), "{err:?}");
        // No permit stored → notified() is still pending → times out here.
        assert!(
            tokio::time::timeout(Duration::from_millis(100), close.notified())
                .await
                .is_err(),
            "a slow response must not tear down the connection"
        );
    }

    #[tokio::test]
    async fn call_fails_when_transport_closed() {
        let (out_tx, out_rx) = tokio::sync::mpsc::channel::<String>(8);
        drop(out_rx); // 立刻关闭出站 → send 失败
        let channel = ReverseRpcChannel::new(out_tx);

        let err = channel
            .call("tool.call", json!({}), 1_000)
            .await
            .expect_err("must fail closed");
        assert!(matches!(err, ReverseRpcError::TransportClosed));
    }
}
