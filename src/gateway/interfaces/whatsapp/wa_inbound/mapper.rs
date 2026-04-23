use crate::gateway::channel::{ChannelId, ConversationId, InboundMessage, MessageId, UserId};
use tracing::debug;

pub fn map_event_to_inbound(
    event: &whatsapp_rust::types::events::Event,
    channel_id: &ChannelId,
) -> Option<InboundMessage> {
    use whatsapp_rust::types::events::Event;

    match event {
        Event::Message(msg, info) => {
            if info.source.is_from_me {
                return None;
            }

            let text = extract_text(msg);
            let sender_id = info.source.sender.to_string();
            let conversation_id = info.source.chat.to_string();
            let is_group = info.source.is_group;

            let reply_to = msg
                .extended_text_message
                .as_ref()
                .and_then(|ext| ext.context_info.as_ref())
                .and_then(|ctx| ctx.stanza_id.as_ref())
                .map(MessageId::new);

            Some(InboundMessage {
                id: MessageId::new(info.id.to_string()),
                channel_id: channel_id.clone(),
                conversation_id: ConversationId::new(&conversation_id),
                sender_id: UserId::new(&sender_id),
                sender_name: Some(info.push_name.clone()),
                text,
                attachments: vec![],
                timestamp: info.timestamp,
                reply_to,
                is_group,
                raw: None,
                metadata: vec![],
            })
        }
        _ => {
            debug!("Skipping non-message event");
            None
        }
    }
}

fn extract_text(msg: &whatsapp_rust::waproto::whatsapp::Message) -> String {
    if let Some(text) = &msg.conversation {
        return text.clone();
    }
    if let Some(ext) = &msg.extended_text_message {
        return ext.text.clone().unwrap_or_default();
    }
    String::new()
}
