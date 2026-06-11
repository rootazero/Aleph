use crate::sync_primitives::Arc;

use crate::gateway::channel::{ChannelError, ChannelResult, ConversationId, MessageId};
use crate::gateway::interfaces::feishu::api::FeishuApi;

pub struct MessageOps {
    api: Arc<FeishuApi>,
}

impl MessageOps {
    pub const fn new(api: Arc<FeishuApi>) -> Self {
        Self { api }
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
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_capabilities() {
        // Placeholder test: presence of this no-op asserts the module compiles.
    }
}
