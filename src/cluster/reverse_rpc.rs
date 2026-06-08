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
    // used by ReverseRpcChannel in Task 2
    #[allow(dead_code)]
    pub fn cancel(&self, id: &str) -> bool {
        self.waiters
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id)
            .is_some()
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
}
