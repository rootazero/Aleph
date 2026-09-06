//! Background task that drains `SecurityAuditLog` entries to SQL and applies
//! the retention policy.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::gateway::security::store::SecurityStore;
use crate::security::audit::{AuditEntry, DEFAULT_RETENTION_SECS};

/// How often the drain task prunes entries past the retention horizon.
const CLEANUP_INTERVAL: Duration = Duration::from_secs(6 * 3600);

/// How often the drain checks the producer-side drop counter. A full channel
/// means entries are vanishing; sixty seconds is the longest that fact stays
/// out of the table (audit I-4).
const DROP_CHECK_INTERVAL: Duration = Duration::from_secs(60);

/// Spawn a single drain task that pulls `AuditEntry` items from `rx` and
/// inserts them into the `security_audit_log` table via `store`. The same task
/// periodically purges entries older than [`DEFAULT_RETENTION_SECS`] so the
/// table does not grow without bound. Returns the join handle. The task exits
/// gracefully when `rx`'s sender side drops.
///
/// `dropped_counter` is the producer side's drop counter
/// ([`SecurityAuditLog::dropped_counter`]). Whenever it advances, the drain
/// synthesises an [`crate::security::audit::AuditEventType::AuditLogDropped`]
/// row, so a trail that lost entries records the loss in the trail itself —
/// the counter alone is process-memory and dies with it (audit I-4). Pass
/// `None` only where no counter exists (unit tests feeding a raw channel).
///
/// [`SecurityAuditLog::dropped_counter`]: crate::security::audit::SecurityAuditLog::dropped_counter
pub fn spawn_audit_drain(
    rx: mpsc::Receiver<AuditEntry>,
    store: Arc<SecurityStore>,
    dropped_counter: Option<Arc<AtomicU64>>,
) -> JoinHandle<()> {
    tokio::spawn(async move { drain_loop(rx, store, dropped_counter).await })
}

/// Insert one [`AuditEventType::AuditLogDropped`](crate::security::audit::AuditEventType::AuditLogDropped)
/// row if the counter moved since `last_seen`. Best-effort like every insert
/// here: a failed insert is logged, never retried — the counter is monotone,
/// so the next check reports the full delta since the last SUCCESSFUL report.
async fn report_drops(store: &Arc<SecurityStore>, counter: &Arc<AtomicU64>, last_seen: &mut u64) {
    let total = counter.load(Ordering::Acquire);
    let delta = total - *last_seen;
    if delta == 0 {
        return;
    }
    let entry = AuditEntry::audit_log_dropped(delta, total);
    let store = store.clone();
    match tokio::task::spawn_blocking(move || store.insert_audit_entry(&entry)).await {
        Ok(Ok(())) => *last_seen = total,
        Ok(Err(e)) => tracing::error!(error = %e, "audit drop-report insert failed"),
        Err(e) => tracing::error!(error = %e, "audit drop-report task panicked"),
    }
}

async fn drain_loop(
    mut rx: mpsc::Receiver<AuditEntry>,
    store: Arc<SecurityStore>,
    dropped_counter: Option<Arc<AtomicU64>>,
) {
    let mut cleanup = tokio::time::interval(CLEANUP_INTERVAL);
    // The first tick fires immediately; drop it so startup isn't spent purging
    // an empty table.
    cleanup.tick().await;
    let mut drop_check = tokio::time::interval(DROP_CHECK_INTERVAL);
    drop_check.tick().await;
    let mut last_seen_drops: u64 = 0;

    loop {
        tokio::select! {
            received = rx.recv() => match received {
                Some(entry) => {
                    let event_type = entry.event_type;
                    let store_for_insert = store.clone();
                    match tokio::task::spawn_blocking(move || store_for_insert.insert_audit_entry(&entry)).await {
                        Ok(Err(e)) => tracing::error!(error = %e, ?event_type, "audit drain insert failed"),
                        Err(e) => tracing::error!(error = %e, "audit drain insert task panicked"),
                        _ => {}
                    }
                    // Entries flowing again after a full channel is exactly
                    // when a drop-report is due; checking here keeps the
                    // report close to the loss it describes.
                    if let Some(counter) = &dropped_counter {
                        report_drops(&store, counter, &mut last_seen_drops).await;
                    }
                }
                None => break,
            },
            _ = drop_check.tick() => {
                if let Some(counter) = &dropped_counter {
                    report_drops(&store, counter, &mut last_seen_drops).await;
                }
            }
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
    // Final flush: entries dropped after the last tick still belong in the
    // table before the task exits.
    if let Some(counter) = &dropped_counter {
        report_drops(&store, counter, &mut last_seen_drops).await;
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
            actor_user: None,
            detail: detail.to_string(),
        }
    }

    #[tokio::test]
    async fn drain_persists_entries_to_store() {
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let (tx, rx) = mpsc::channel(8);
        let handle = spawn_audit_drain(rx, store.clone(), None);

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

    /// The actor is only worth having if it survives to the table. A field on
    /// the in-memory struct that the `INSERT` never binds is the shape
    /// `memory::explain`'s deleted `memory_audit_log` had — an audit
    /// vocabulary for an audit that never happened, which reads to an operator
    /// as "nothing occurred".
    #[tokio::test]
    async fn the_actor_reaches_the_sql_column() {
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let (tx, rx) = mpsc::channel(4);
        let handle = spawn_audit_drain(rx, store.clone(), None);

        tx.send(AuditEntry::scoped_content_read(
            "u-bob",
            Some("main:conv-alice".to_string()),
            "trace.get: read 3 events of run run-a",
        ))
        .await
        .unwrap();
        drop(tx);
        handle.await.unwrap();

        let conn = store.conn.lock().unwrap_or_else(|e| e.into_inner());
        let (event_type, actor, session): (String, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT event_type, actor_user, session_id FROM security_audit_log",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(event_type, "scoped_content_read");
        assert_eq!(actor.as_deref(), Some("u-bob"));
        assert_eq!(session.as_deref(), Some("main:conv-alice"));
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
        let handle = spawn_audit_drain(rx, store, None);
        drop(tx);
        // Should complete promptly.
        tokio::time::timeout(std::time::Duration::from_secs(1), handle)
            .await
            .expect("drain did not exit within 1s")
            .unwrap();
    }

    /// The fail-open half of I-4 made a full channel invisible: entries
    /// vanished into a process-memory counter and the table read as if
    /// nothing had happened. The drain now mirrors counter deltas into the
    /// table itself, so the trail records its own degradation.
    #[tokio::test]
    async fn dropped_entries_are_reported_in_the_table() {
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let counter = Arc::new(AtomicU64::new(3));
        let (tx, rx) = mpsc::channel(4);
        let handle = spawn_audit_drain(rx, store.clone(), Some(counter));

        // One real entry to pull the drain past its first recv; the drop
        // report rides the same iteration.
        tx.send(entry("survived")).await.unwrap();
        drop(tx);
        handle.await.unwrap();

        let conn = store.conn.lock().unwrap_or_else(|e| e.into_inner());
        let (event_type, severity, detail): (String, String, String) = conn
            .query_row(
                "SELECT event_type, severity, detail FROM security_audit_log \
                 WHERE event_type = 'audit_log_dropped'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(event_type, "audit_log_dropped");
        assert_eq!(severity, "critical");
        assert!(
            detail.contains('3'),
            "detail should name the delta: {detail}"
        );
    }
}
