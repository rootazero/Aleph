//! Background task that drains `SecurityAuditLog` entries to SQL.

use std::sync::Arc;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::gateway::security::store::SecurityStore;
use crate::security::audit::AuditEntry;

/// Spawn a single drain task that pulls `AuditEntry` items from `rx` and
/// inserts them into the `security_audit_log` table via `store`. Returns
/// the join handle. The task exits gracefully when `rx`'s sender side
/// drops.
pub fn spawn_audit_drain(
    rx: mpsc::Receiver<AuditEntry>,
    store: Arc<SecurityStore>,
) -> JoinHandle<()> {
    tokio::spawn(async move { drain_loop(rx, store).await })
}

async fn drain_loop(mut rx: mpsc::Receiver<AuditEntry>, store: Arc<SecurityStore>) {
    while let Some(entry) = rx.recv().await {
        if let Err(e) = store.insert_audit_entry(&entry) {
            tracing::error!(error = %e, ?entry.event_type, "audit drain insert failed");
        }
    }
    tracing::debug!("audit drain channel closed; task exiting");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::audit::{AuditEntry, AuditEventType, AuditSeverity};

    fn entry(detail: &str) -> AuditEntry {
        AuditEntry {
            event_type: AuditEventType::SsrfBlocked,
            severity: AuditSeverity::Warn,
            source_ip: None,
            session_id: Some("sess-1".to_string()),
            detail: detail.to_string(),
        }
    }

    #[tokio::test]
    async fn drain_persists_entries_to_store() {
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let (tx, rx) = mpsc::channel(8);
        let handle = spawn_audit_drain(rx, store.clone());

        tx.send(entry("first")).await.unwrap();
        tx.send(entry("second")).await.unwrap();

        // Close the channel so the task exits.
        drop(tx);
        handle.await.unwrap();

        let conn = store.conn.lock().unwrap_or_else(|e| e.into_inner());
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM security_audit_log", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn drain_exits_gracefully_on_sender_drop() {
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let (tx, rx) = mpsc::channel::<AuditEntry>(1);
        let handle = spawn_audit_drain(rx, store);
        drop(tx);
        // Should complete promptly.
        tokio::time::timeout(std::time::Duration::from_secs(1), handle)
            .await
            .expect("drain did not exit within 1s")
            .unwrap();
    }
}
