use crate::gateway::channel::{InboundMessage, InboundMessageSender};
use std::time::Duration;

/// Debounce entry for coalescing rapid messages.
pub struct DebounceEntry {
    messages: Vec<InboundMessage>,
    deadline: tokio::time::Instant,
}

/// Debouncer for coalescing rapid messages from the same sender in the same channel.
pub struct SlackDebouncer {
    entries: std::collections::HashMap<String, DebounceEntry>,
    debounce_ms: u64,
    inbound_tx: InboundMessageSender,
}

impl SlackDebouncer {
    pub fn new(debounce_ms: u64, inbound_tx: InboundMessageSender) -> Self {
        Self {
            entries: std::collections::HashMap::new(),
            debounce_ms,
            inbound_tx,
        }
    }

    pub fn build_key(msg: &InboundMessage, thread_ts: Option<&str>) -> String {
        let base = format!(
            "{}:{}",
            msg.conversation_id.as_str(),
            msg.sender_id.as_str()
        );
        match thread_ts {
            Some(ts) => format!("{base}:{ts}"),
            None => base,
        }
    }

    pub async fn enqueue(&mut self, msg: InboundMessage, thread_ts: Option<&str>) -> bool {
        if self.debounce_ms == 0 {
            return self.send_immediate(msg).await;
        }

        let key = Self::build_key(&msg, thread_ts);
        let deadline = tokio::time::Instant::now() + Duration::from_millis(self.debounce_ms);

        if let Some(entry) = self.entries.get_mut(&key) {
            const MAX_MESSAGES: usize = 100;
            const MAX_BYTES: usize = 10 * 1024 * 1024;
            const MAX_RESIDENCE_MS: u64 = 30_000;

            let msg_bytes = msg.text.len()
                + msg
                    .attachments
                    .iter()
                    .map(|a| a.data.as_ref().map(|d| d.len()).unwrap_or(0))
                    .sum::<usize>();
            let current_bytes: usize = entry
                .messages
                .iter()
                .map(|m| {
                    m.text.len()
                        + m.attachments
                            .iter()
                            .map(|a| a.data.as_ref().map(|d| d.len()).unwrap_or(0))
                            .sum::<usize>()
                })
                .sum();

            let first_arrival = entry.deadline - Duration::from_millis(self.debounce_ms);
            let residence = tokio::time::Instant::now().saturating_duration_since(first_arrival);

            if entry.messages.len() >= MAX_MESSAGES
                || current_bytes + msg_bytes > MAX_BYTES
                || residence >= Duration::from_millis(MAX_RESIDENCE_MS)
            {
                let old_entry = self.entries.remove(&key).unwrap();
                if self.flush_entry(old_entry).await {
                    return true;
                }
                self.entries.insert(
                    key,
                    DebounceEntry {
                        messages: vec![msg],
                        deadline,
                    },
                );
            } else {
                entry.messages.push(msg);
                entry.deadline = deadline;
            }
        } else {
            self.entries.insert(
                key,
                DebounceEntry {
                    messages: vec![msg],
                    deadline,
                },
            );
        }
        false
    }

    pub async fn flush_expired(&mut self) -> bool {
        let now = tokio::time::Instant::now();
        let keys_to_flush: Vec<String> = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.deadline <= now)
            .map(|(k, _)| k.clone())
            .collect();

        let mut failed = false;
        for key in keys_to_flush {
            if let Some(entry) = self.entries.remove(&key) {
                failed |= self.flush_entry(entry).await;
            }
        }
        failed
    }

    pub async fn drain(&mut self) -> bool {
        let entries = std::mem::take(&mut self.entries);
        let mut failed = false;
        for entry in entries.into_values() {
            failed |= self.flush_entry(entry).await;
        }
        failed
    }

    pub async fn send_immediate(&self, msg: InboundMessage) -> bool {
        // Returns true only when the inbound channel is closed (send failed),
        // matching the socket loop's "true => channel closed, stop" contract.
        if let Err(msg) = self.inbound_tx.send(msg) {
            tracing::error!(
                conversation = %msg.conversation_id.as_str(),
                message_id = %msg.id.as_str(),
                "Slack: failed to forward accepted message: inbound channel closed"
            );
            return true;
        }
        false
    }

    pub async fn flush_entry(&self, entry: DebounceEntry) -> bool {
        if entry.messages.is_empty() {
            return false;
        }

        let combined = if entry.messages.len() == 1 {
            entry
                .messages
                .into_iter()
                .next()
                .unwrap_or_else(|| unreachable!("length checked above"))
        } else {
            let last = entry
                .messages
                .last()
                .unwrap_or_else(|| unreachable!("non-empty checked above"));
            let combined_text = entry
                .messages
                .iter()
                .map(|m| m.text.as_str())
                .filter(|t| !t.is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            // Preserve attachments from every coalesced message, not just the last.
            let combined_attachments: Vec<_> = entry
                .messages
                .iter()
                .flat_map(|m| m.attachments.iter().cloned())
                .collect();

            InboundMessage {
                id: last.id.clone(),
                channel_id: last.channel_id.clone(),
                conversation_id: last.conversation_id.clone(),
                sender_id: last.sender_id.clone(),
                sender_name: last.sender_name.clone(),
                text: combined_text,
                attachments: combined_attachments,
                timestamp: last.timestamp,
                reply_to: last.reply_to.clone(),
                is_group: last.is_group,
                raw: last.raw.clone(),
                metadata: last.metadata.clone(),
            }
        };

        self.send_immediate(combined).await
    }
}
