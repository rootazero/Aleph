use crate::gateway::channel::{
    InboundMessage, InboundMessageSender,
};
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
            Some(ts) => format!("{}:{}", base, ts),
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
            entry.messages.push(msg);
            entry.deadline = deadline;
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

    pub async fn flush_expired(&mut self) {
        let now = tokio::time::Instant::now();
        let keys_to_flush: Vec<String> = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.deadline <= now)
            .map(|(k, _)| k.clone())
            .collect();

        for key in keys_to_flush {
            if let Some(entry) = self.entries.remove(&key) {
                self.flush_entry(entry).await;
            }
        }
    }

    pub async fn send_immediate(&self, msg: InboundMessage) -> bool {
        self.inbound_tx.send(msg).is_ok()
    }

    pub async fn flush_entry(&self, entry: DebounceEntry) {
        if entry.messages.is_empty() {
            return;
        }

        let combined = if entry.messages.len() == 1 {
            entry.messages.into_iter().next().unwrap()
        } else {
            let last = entry.messages.last().unwrap();
            let combined_text = entry
                .messages
                .iter()
                .map(|m| m.text.as_str())
                .filter(|t| !t.is_empty())
                .collect::<Vec<_>>()
                .join("\n");

            InboundMessage {
                id: last.id.clone(),
                channel_id: last.channel_id.clone(),
                conversation_id: last.conversation_id.clone(),
                sender_id: last.sender_id.clone(),
                sender_name: last.sender_name.clone(),
                text: combined_text,
                attachments: last.attachments.clone(),
                timestamp: last.timestamp,
                reply_to: last.reply_to.clone(),
                is_group: last.is_group,
                raw: last.raw.clone(),
                metadata: last.metadata.clone(),
            }
        };

        let _ = self.inbound_tx.send(combined);
    }
}
