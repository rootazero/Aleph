use std::time::{Duration, Instant};
use dashmap::DashMap;

use crate::gateway::channel::{
    ChannelError, ConversationId, MessageId, OutboundMessage, SendResult,
};
use crate::gateway::interfaces::qq::api::QQApiClient;
use crate::gateway::interfaces::qq::types::SendMessagePayload;

const PASSIVE_REPLY_LIMIT: u8 = 4;
const PASSIVE_REPLY_TTL: Duration = Duration::from_secs(60 * 60);
const MAX_MESSAGE_LENGTH: usize = 4000;

#[derive(Debug, Clone)]
struct ReplyRecord {
    count: u8,
    first_reply_at: Instant,
}

pub struct ReplyTracker {
    inner: DashMap<String, ReplyRecord>,
}

impl ReplyTracker {
    pub fn new() -> Self {
        Self {
            inner: DashMap::new(),
        }
    }

    pub fn can_reply_passively(
        &self,
        msg_id: &str,
    ) -> (bool, bool) {
        if let Some(entry) = self.inner.get(msg_id) {
            let record = entry.value();
            let elapsed = Instant::now().duration_since(record.first_reply_at);
            if elapsed > PASSIVE_REPLY_TTL {
                return (false, true);
            }
            if record.count > PASSIVE_REPLY_LIMIT {
                return (false, true);
            }
            return (true, false);
        }
        (false, false)
    }

    pub fn record_reply(
        &self,
        msg_id: &str,
    ) {
        {
            let mut entry = self.inner.entry(msg_id.to_string()).or_insert(ReplyRecord {
                count: 0,
                first_reply_at: Instant::now(),
            });
            entry.count += 1;
        }

        if self.inner.len() > 10000 {
            let now = Instant::now();
            self.inner.retain(|_, v| {
                now.duration_since(v.first_reply_at) <= PASSIVE_REPLY_TTL
            });
        }
    }
}

pub fn parse_target(conversation_id: &ConversationId) -> Result<(&str, &str), ChannelError> {
    let id = conversation_id.as_str();
    if let Some(rest) = id.strip_prefix("c2c:") {
        Ok(("c2c", rest))
    } else if let Some(rest) = id.strip_prefix("group:") {
        Ok(("group", rest))
    } else {
        Err(ChannelError::ConfigError(format!(
            "Invalid QQ conversation id: {}",
            id
        )))
    }
}

pub fn chunk_text(text: &str, max_len: usize) -> Vec<String> {
    if text.len() <= max_len {
        return vec![text.to_string()];
    }
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let end = (start + max_len).min(text.len());
        let split_at = if end < text.len() {
            text[start..end]
                .rfind('\n')
                .or_else(|| text[start..end].rfind(' '))
                .map(|i| start + i + 1)
                .unwrap_or(end)
        } else {
            end
        };
        chunks.push(text[start..split_at].to_string());
        start = split_at;
    }
    chunks
}

pub async fn deliver_message(
    api: &QQApiClient,
    message: &OutboundMessage,
    reply_tracker: &ReplyTracker,
) -> Result<SendResult, ChannelError> {
    let (target_type, target_id) = parse_target(&message.conversation_id)?;

    let passive_msg_id = message
        .metadata
        .get("qq::msg_id")
        .cloned()
        .or_else(|| message.reply_to.as_ref().map(|m| m.as_str().to_string()));

    let mut use_passive = false;
    let mut final_msg_id = None;

    if let Some(ref mid) = passive_msg_id {
        let (can_passive, _) = reply_tracker.can_reply_passively(mid);
        if can_passive {
            use_passive = true;
            final_msg_id = Some(mid.clone());
        }
    }

    let chunks = chunk_text(&message.text, MAX_MESSAGE_LENGTH);

    for (idx, chunk) in chunks.iter().enumerate() {
        let payload = SendMessagePayload {
            msg_id: if use_passive { final_msg_id.clone() } else { None },
            msg_type: 0,
            content: chunk.clone(),
            markdown: None,
            media: None,
            msg_seq: Some(idx as u16),
        };

        let result = match target_type {
            "c2c" => api.send_c2c_message(target_id, payload).await,
            "group" => api.send_group_message(target_id, payload).await,
            _ => unreachable!(),
        };

        match result {
            Ok(_) => {}
            Err(e) => {
                let err_str = e.to_string();
                if use_passive
                    && (err_str.contains("304023")
                        || err_str.contains("304024")
                        || err_str.contains("limit"))
                {
                    use_passive = false;
                    let payload2 = SendMessagePayload {
                        msg_id: None,
                        msg_type: 0,
                        content: chunk.clone(),
                        markdown: None,
                        media: None,
                        msg_seq: Some(idx as u16),
                    };
                    match target_type {
                        "c2c" => api.send_c2c_message(target_id, payload2).await,
                        "group" => api.send_group_message(target_id, payload2).await,
                        _ => unreachable!(),
                    }
                    .map_err(ChannelError::from)?;
                } else {
                    return Err(ChannelError::from(e));
                }
            }
        }
    }

    if use_passive {
        if let Some(mid) = final_msg_id {
            reply_tracker.record_reply(&mid);
        }
    }

    Ok(SendResult {
        message_id: MessageId::new("qq_msg"),
        timestamp: chrono::Utc::now(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunk_text_short() {
        let chunks = chunk_text("hello", 10);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "hello");
    }

    #[test]
    fn test_chunk_text_long() {
        let text = "a".repeat(5000);
        let chunks = chunk_text(&text, 4000);
        assert_eq!(chunks.len(), 2);
    }

    #[test]
    fn test_parse_target_c2c() {
        let conv = ConversationId::new("c2c:openid123");
        assert_eq!(parse_target(&conv).unwrap(), ("c2c", "openid123"));
    }

    #[test]
    fn test_parse_target_group() {
        let conv = ConversationId::new("group:group123");
        assert_eq!(parse_target(&conv).unwrap(), ("group", "group123"));
    }

    #[test]
    fn test_reply_tracker_limits() {
        let tracker = ReplyTracker::new();
        let msg_id = "msg-1";

        assert_eq!(tracker.can_reply_passively(msg_id), (false, false));
        tracker.record_reply(msg_id);
        assert_eq!(tracker.can_reply_passively(msg_id), (true, false));

        tracker.record_reply(msg_id);
        tracker.record_reply(msg_id);
        tracker.record_reply(msg_id);
        assert_eq!(tracker.can_reply_passively(msg_id), (true, false));

        tracker.record_reply(msg_id);
        assert_eq!(tracker.can_reply_passively(msg_id), (false, true));
    }
}
