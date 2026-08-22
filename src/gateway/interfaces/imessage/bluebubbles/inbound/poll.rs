//! Catch-up reconciliation poll. Webhook handles real-time; this recovers
//! messages that arrived while the daemon was offline. Dedup prevents overlap
//! with the webhook path.

use crate::sync_primitives::{Arc, AtomicBool, Mutex as StdMutex, Ordering};
use std::time::Duration;

use crate::gateway::channel::InboundMessageSender;
use crate::gateway::interfaces::imessage::bluebubbles::api::BlueBubblesApi;
use crate::gateway::interfaces::imessage::bluebubbles::inbound::dedup::BbDedup;
use crate::gateway::interfaces::imessage::bluebubbles::inbound::mapper::{
    map_webhook_record, to_inbound,
};
use crate::gateway::interfaces::telegram::offset::OffsetTracker;

/// Highest `dateCreated` among `msgs`, or `prior` if empty. Never below `prior`.
#[must_use]
pub fn newest_cursor(msgs: &[serde_json::Value], prior: i64) -> i64 {
    msgs.iter()
        .filter_map(|m| m.get("dateCreated").and_then(|d| d.as_i64()))
        .max()
        .unwrap_or(prior)
        .max(prior)
}

pub async fn run_catchup_poll(
    api: Arc<BlueBubblesApi>,
    sender: InboundMessageSender,
    dedup: Arc<StdMutex<BbDedup>>,
    tracker: Arc<OffsetTracker>,
    running: Arc<AtomicBool>,
    interval: Duration,
) {
    while running.load(Ordering::SeqCst) {
        let after = tracker.load();
        let msgs = api.query_messages_after(after).await;
        let cursor = newest_cursor(&msgs, after);
        for raw in &msgs {
            if let Some(m) = map_webhook_record(raw) {
                // Drop echoes, remove-tapbacks, and GUID-less records; normal
                // messages and add-tapbacks (surfaced as reactions) route on.
                if !m.is_routable() {
                    continue;
                }
                let dup = {
                    dedup
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .is_duplicate(&m.guid)
                };
                if dup {
                    continue;
                }
                let atts = super::download_attachments(&api, &m.attachment_guids).await;
                let _ = sender.send(to_inbound(&m, atts));
            }
        }
        if cursor > after {
            tracker.advance(cursor, "imessage-bb").await;
        }
        // Piggyback the attachment sweep on the existing poll interval so staged
        // downloads don't accumulate over a long-running daemon — no extra task.
        super::super::staging::sweep_stale(super::super::staging::RETENTION);
        tokio::time::sleep(interval).await;
    }
}

#[cfg(test)]
mod tests {
    use super::newest_cursor;

    #[test]
    fn newest_cursor_takes_max_date() {
        let msgs = vec![
            serde_json::json!({ "guid": "a", "dateCreated": 100 }),
            serde_json::json!({ "guid": "b", "dateCreated": 300 }),
            serde_json::json!({ "guid": "c", "dateCreated": 200 }),
        ];
        assert_eq!(newest_cursor(&msgs, 50), 300);
        assert_eq!(newest_cursor(&[], 50), 50); // empty keeps prior
    }
}
