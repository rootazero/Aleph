//! In-flight RPC table — maps a u64 request id to a oneshot reply channel.
//!
//! Registered when the client sends a request; completed when the response
//! arrives; `fail_all` drains everything when the helper subprocess crashes.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{oneshot, Mutex};

use crate::error::{DesktopError, Result};

pub type Slot = oneshot::Sender<Result<serde_json::Value>>;

#[derive(Clone, Default)]
pub struct InflightTable {
    inner: Arc<Mutex<HashMap<u64, Slot>>>,
}

impl InflightTable {
    pub async fn register(&self, id: u64, tx: Slot) {
        self.inner.lock().await.insert(id, tx);
    }

    pub async fn complete(&self, id: u64, value: serde_json::Value) -> Result<()> {
        let tx = self
            .inner
            .lock()
            .await
            .remove(&id)
            .ok_or_else(|| DesktopError::BridgeFailed(format!("inflight id={id} not found")))?;
        let _ = tx.send(Ok(value));
        Ok(())
    }

    pub async fn fail(&self, id: u64, reason: impl Into<String>) {
        if let Some(tx) = self.inner.lock().await.remove(&id) {
            let _ = tx.send(Err(DesktopError::BridgeFailed(reason.into())));
        }
    }

    /// Fail a specific in-flight request with a pre-built `DesktopError`.
    ///
    /// Used by the bridge client's error mapper so it can pass a structured
    /// `DesktopError::PermissionDenied` without going through the string path.
    pub async fn fail_err(&self, id: u64, err: DesktopError) {
        if let Some(tx) = self.inner.lock().await.remove(&id) {
            let _ = tx.send(Err(err));
        }
    }

    /// Drop a pending request without completing or failing it.
    ///
    /// Used by the bridge client when an RPC times out: the caller has
    /// already given up, so the slot is removed to avoid leaking it. If the
    /// helper later sends a response for this id, `complete` simply reports
    /// "id not found" — which the reader loop already tolerates.
    pub async fn cancel(&self, id: u64) {
        self.inner.lock().await.remove(&id);
    }

    pub async fn fail_all(&self, reason: impl Into<String>) {
        let reason: String = reason.into();
        let mut guard = self.inner.lock().await;
        for (_, tx) in guard.drain() {
            let _ = tx.send(Err(DesktopError::BridgeFailed(reason.clone())));
        }
    }

    pub async fn len(&self) -> usize {
        self.inner.lock().await.len()
    }

    pub async fn is_empty(&self) -> bool {
        self.inner.lock().await.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::oneshot;

    #[tokio::test]
    async fn register_and_complete() {
        let table = InflightTable::default();
        let (tx, rx) = oneshot::channel();
        table.register(1, tx).await;
        table
            .complete(1, serde_json::json!({"ok": true}))
            .await
            .unwrap();
        let val = rx.await.unwrap().unwrap();
        assert_eq!(val["ok"], true);
    }

    #[tokio::test]
    async fn complete_missing_id_errors() {
        let table = InflightTable::default();
        let err = table.complete(999, serde_json::json!(null)).await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn fail_all_empties_table() {
        let table = InflightTable::default();
        let (tx1, _rx1) = oneshot::channel();
        let (tx2, _rx2) = oneshot::channel();
        table.register(1, tx1).await;
        table.register(2, tx2).await;
        table.fail_all("crashed").await;
        assert_eq!(table.len().await, 0);
    }

    #[tokio::test]
    async fn fail_single_id() {
        let table = InflightTable::default();
        let (tx, rx) = oneshot::channel();
        table.register(42, tx).await;
        table.fail(42, "timeout").await;
        let res = rx.await.unwrap();
        assert!(res.is_err());
        assert_eq!(table.len().await, 0);
    }

    #[tokio::test]
    async fn cancel_removes_slot_without_leaking() {
        let table = InflightTable::default();
        let (tx, rx) = oneshot::channel();
        table.register(7, tx).await;
        assert_eq!(table.len().await, 1);
        table.cancel(7).await;
        assert_eq!(table.len().await, 0);
        // The receiver observes the dropped sender as a recv error.
        assert!(rx.await.is_err());
        // A late `complete` for a cancelled id is a tolerated no-op error.
        assert!(table.complete(7, serde_json::json!(null)).await.is_err());
    }
}
