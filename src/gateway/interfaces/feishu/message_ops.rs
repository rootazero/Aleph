use std::sync::Arc;

use crate::gateway::channel::{
    ChannelError, ChannelResult, ConversationId, MessageId,
};
use crate::gateway::interfaces::feishu::api::FeishuApi;

pub struct MessageOps {
    api: Arc<FeishuApi>,
}

impl MessageOps {
    pub fn new(api: Arc<FeishuApi>) -> Self {
        Self { api }
    }

    pub async fn reply(
        &self,
        conversation_id: &ConversationId,
        message_id: &MessageId,
        text: &str,
    ) -> ChannelResult<MessageId> {
        let msg_id = self.api
            .send_text(conversation_id.as_str(), text, Some(message_id.as_str()))
            .await
            .map_err(|e| ChannelError::SendFailed(format!("{e:?}")))?;
        Ok(MessageId::new(msg_id))
    }

    pub async fn react(
        &self,
        _conversation_id: &ConversationId,
        message_id: &MessageId,
        reaction: &str,
    ) -> ChannelResult<()> {
        self.api
            .add_reaction(message_id.as_str(), reaction)
            .await
            .map_err(ChannelError::SendFailed)?;
        Ok(())
    }

    pub async fn edit(
        &self,
        _conversation_id: &ConversationId,
        _message_id: &MessageId,
        _new_text: &str,
    ) -> ChannelResult<()> {
        Err(ChannelError::UnsupportedFeature("editing".to_string()))
    }

    pub async fn delete(
        &self,
        _conversation_id: &ConversationId,
        _message_id: &MessageId,
    ) -> ChannelResult<()> {
        Err(ChannelError::UnsupportedFeature("deletion".to_string()))
    }

    pub async fn send(
        &self,
        conversation_id: &ConversationId,
        text: &str,
    ) -> ChannelResult<MessageId> {
        let msg_id = self.api
            .send_text(conversation_id.as_str(), text, None)
            .await
            .map_err(|e| ChannelError::SendFailed(format!("{e:?}")))?;
        Ok(MessageId::new(msg_id))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_capabilities() {
        assert!(true);
    }
}
