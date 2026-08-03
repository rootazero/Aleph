//! Background task that drains `SecurityAuditLog` entries to SQL and applies
//! the retention policy.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::gateway::security::store::SecurityStore;
use crate::security::audit::{AuditEntry, DEFAULT_RETENTION_SECS};

/// How often the drain task prunes entries past the retention horizon.
const CLEANUP_INTERVAL: Duration = Duration::from_secs(6 * 3600);

/// Spawn a single drain task that pulls `AuditEntry` items from `rx` and
/// inserts them into the `security_audit_log` table via `store`. The same task
/// periodically purges entries older than [`DEFAULT_RETENTION_SECS`] so the
/// table does not grow without bound. Returns the join handle. The task exits
/// gracefully when `rx`'s sender side drops.
pub fn spawn_audit_drain(
    rx: mpsc::Receiver<AuditEntry>,
    store: Arc<SecurityStore>,
) -> JoinHandle<()> {
    tokio::spawn(async move { drain_loop(rx, store).await })
}

async fn drain_loop(mut rx: mpsc::Receiver<AuditEntry>, store: Arc<SecurityStore>) {
    let mut cleanup = tokio::time::interval(CLEANUP_INTERVAL);
    // The first tick fires immediately; drop it so startup isn't spent purging
    // an empty table.
    cleanup.tick().await;

    loop {
        tokio::select! {
            received = rx.recv() => match received {
                Some(entry) => {
                    let event_type = entry.event_type;
                    let store = store.clone();
                    match tokio::task::spawn_blocking(move || store.insert_audit_entry(&entry)).await {
                        Ok(Err(e)) => tracing::error!(error = %e, ?event_type, "audit drain insert failed"),
                        Err(e) => tracing::error!(error = %e, "audit drain insert task panicked"),
                        _ => {}
                    }
                }
                None => break,
            },
            _ = cleanup.tick() => {
                let store = store.clone();
                match tokio::task::spawn_blocking(move || store.purge_audit_entries(DEFAULT_RETENTION_SECS)).await {
                    Ok(Ok(n)) if n > 0 => {
                        tracing::debug!(removed = n, "audit retention purge complete");
                    }
                    Ok(Ok(_)) => {}
                    Ok(Err(e)) => tracing::error!(error = %e, "audit retention purge failed"),
                    Err(e) => tracing::error!(error = %e, "audit retention purge task panicked"),
                }
            }
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
            event_type: AuditEventType::ExecBlocked,
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
    async fn purge_removes_entries_past_retention() {
        let store = SecurityStore::in_memory().unwrap();
        {
            let conn = store.conn.lock().unwrap_or_else(|e| e.into_inner());
            // One ancient row (well past any retention window) and one fresh row.
            conn.execute(
                "INSERT INTO security_audit_log (timestamp, event_type, severity, detail) \
                 VALUES (strftime('%s','now') - 1000000, 'exec_blocked', 'warn', 'old')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO security_audit_log (event_type, severity, detail) \
                 VALUES ('exec_blocked', 'warn', 'fresh')",
                [],
            )
            .unwrap();
        }

        // Retention horizon of 100s removes only the ancient row.
        let removed = store.purge_audit_entries(100).unwrap();
        assert_eq!(removed, 1);

        let conn = store.conn.lock().unwrap_or_else(|e| e.into_inner());
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM security_audit_log", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
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
