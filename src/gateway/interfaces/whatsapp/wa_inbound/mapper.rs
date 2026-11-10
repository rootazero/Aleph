use crate::gateway::channel::{ChannelId, ConversationId, InboundMessage, MessageId, UserId};
use crate::gateway::interfaces::whatsapp::history_buffer::GroupHistoryBuffer;
use tracing::debug;

pub async fn map_event_to_inbound(
    event: &whatsapp_rust::types::events::Event,
    channel_id: &ChannelId,
    _buffer: &GroupHistoryBuffer,
) -> Vec<InboundMessage> {
    use whatsapp_rust::types::events::Event;

    match event {
        Event::Messages(batch) => {
            let mut messages = Vec::with_capacity(batch.len());
            for message in batch {
                let msg = &message.message;
                let info = &message.info;
                if info.source.is_from_me {
                    continue;
                }

                let text = extract_text(msg);
                let sender_id = info.source.sender.to_string();
                let conversation_id = info.source.chat.to_string();
                let is_group = info.source.is_group;

                let reply_to = msg
                    .extended_text_message
                    .as_option()
                    .and_then(|ext| ext.context_info.as_option())
                    .and_then(|ctx| ctx.stanza_id.as_ref())
                    .map(MessageId::new);

                let inbound = InboundMessage {
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
                };

                messages.push(inbound);
            }
            messages
        }
        _ => {
            debug!("Skipping non-message event");
            Vec::new()
        }
    }
}

fn extract_text(msg: &whatsapp_rust::waproto::whatsapp::Message) -> String {
    if let Some(text) = &msg.conversation {
        return text.clone();
    }
    if let Some(ext) = msg.extended_text_message.as_option() {
        return ext.text.clone().unwrap_or_default();
    }
    let (kind, caption) = if let Some(image) = msg.image_message.as_option() {
        ("image", image.caption.as_deref())
    } else if let Some(document) = msg.document_message.as_option() {
        ("document", document.caption.as_deref())
    } else if let Some(video) = msg.video_message.as_option() {
        ("video", video.caption.as_deref())
    } else if msg.audio_message.as_option().is_some() {
        ("audio/voice note", None)
    } else {
        ("message type", None)
    };
    // Downloads are not implemented: make the missing payload visible to the agent.
    format!(
        "[WhatsApp {kind} unsupported: media is unavailable]{}",
        caption.map(|text| format!("\n{text}")).unwrap_or_default()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::interfaces::whatsapp::wa_runtime::{fake::FakeWaRuntime, WaRuntime};
    use whatsapp_rust::waproto::whatsapp::{
        message::{AudioMessage, DocumentMessage, ImageMessage, VideoMessage},
        Message,
    };

    #[tokio::test]
    async fn unsupported_media_preserves_captions_and_identity_without_empty_turns() {
        let fake = FakeWaRuntime::new();
        let mut events = fake.take_event_receiver().unwrap();
        fake.emit_text_message("123@g.us", "456", "fixture")
            .await
            .unwrap();
        let event = events.recv().await.unwrap();
        let whatsapp_rust::types::events::Event::Messages(batch) = event else {
            panic!("expected message event")
        };
        let info = batch.first().unwrap().info.as_ref().clone();
        let event_for = |body: Message, from_me| {
            use crate::sync_primitives::Arc;
            use whatsapp_rust::types::events::{BatchOrigin, Event, InboundMessage, MessageBatch};
            let mut info = info.clone();
            info.source.is_from_me = from_me;
            Event::Messages(
                MessageBatch::builder()
                    .messages(
                        vec![InboundMessage::builder()
                            .message(Arc::new(body))
                            .info(Arc::new(info))
                            .build()]
                        .into(),
                    )
                    .origin(BatchOrigin::Live)
                    .build(),
            )
        };
        let bodies = [
            Message {
                image_message: ImageMessage {
                    caption: Some("photo caption".into()),
                    ..Default::default()
                }
                .into(),
                ..Default::default()
            },
            Message {
                document_message: DocumentMessage {
                    caption: Some("document caption".into()),
                    ..Default::default()
                }
                .into(),
                ..Default::default()
            },
            Message {
                video_message: VideoMessage {
                    caption: Some("video caption".into()),
                    ..Default::default()
                }
                .into(),
                ..Default::default()
            },
            Message {
                audio_message: AudioMessage::default().into(),
                ..Default::default()
            },
        ];
        let channel = ChannelId::new("wa-test");
        let buffer = GroupHistoryBuffer::new(Default::default());
        for body in bodies {
            let event = event_for(body, false);
            let messages = map_event_to_inbound(&event, &channel, &buffer).await;
            assert_eq!(messages.len(), 1);
            let message = &messages[0];
            assert!(message.text.contains("unsupported"));
            if message.text.contains("image") {
                assert!(message.text.contains("photo caption"));
            }
            if message.text.contains("document") {
                assert!(message.text.contains("document caption"));
            }
            if message.text.contains("video") {
                assert!(message.text.contains("video caption"));
            }
            assert_eq!(message.channel_id, channel);
            assert_eq!(message.conversation_id.as_str(), "123@g.us");
            assert_eq!(message.sender_id.as_str(), "456@s.whatsapp.net");
        }
        let event = event_for(
            Message {
                audio_message: AudioMessage::default().into(),
                ..Default::default()
            },
            true,
        );
        assert!(map_event_to_inbound(&event, &channel, &buffer)
            .await
            .is_empty());
    }

    #[tokio::test]
    async fn mapping_group_messages_does_not_fill_unconsumed_history() {
        let fake = FakeWaRuntime::new();
        let mut events = fake.take_event_receiver().unwrap();
        let buffer = GroupHistoryBuffer::new(Default::default());
        for group in ["111@g.us", "222@g.us"] {
            fake.emit_text_message(group, "456", "hello").await.unwrap();
            let event = events.recv().await.unwrap();
            let messages = map_event_to_inbound(&event, &ChannelId::new("wa-test"), &buffer).await;
            assert_eq!(messages.len(), 1);
            assert!(buffer
                .get_context(&messages[0].conversation_id)
                .await
                .is_none());
        }
    }
}
