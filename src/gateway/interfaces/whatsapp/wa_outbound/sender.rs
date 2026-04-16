use crate::gateway::channel::{ChannelResult, MessageId, OutboundMessage};
use crate::gateway::interfaces::whatsapp::wa_runtime::WaRuntime;
use crate::gateway::interfaces::whatsapp::config::WhatsAppAccountConfig;

pub struct WaOutbound;

impl WaOutbound {
    pub async fn send_message(
        runtime: &WaRuntime,
        msg: OutboundMessage,
        _account: &WhatsAppAccountConfig,
    ) -> ChannelResult<MessageId> {
        runtime.send_message(msg).await
    }

    pub async fn send_reaction(
        runtime: &WaRuntime,
        jid: &str,
        msg_id: &str,
        emoji: &str,
    ) -> ChannelResult<()> {
        runtime.send_reaction(jid, msg_id, emoji).await
    }

    pub async fn mark_read(
        runtime: &WaRuntime,
        jid: &str,
        msg_id: &str,
    ) -> ChannelResult<()> {
        runtime.mark_read(jid, msg_id).await
    }

    pub async fn send_typing(
        runtime: &WaRuntime,
        jid: &str,
    ) -> ChannelResult<()> {
        runtime.send_typing(jid).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::interfaces::whatsapp::wa_auth::WaAuthManager;

    #[tokio::test]
    async fn test_send_message_placeholder() {
        let auth = WaAuthManager::new("test");
        let data = crate::gateway::interfaces::whatsapp::wa_auth::WaAuthData {
            creds_blob: vec![1],
            keys_blob: vec![2],
            app_state_sync: vec![3],
        };
        auth.save(&data).unwrap();
        let (tx, _rx) = tokio::sync::mpsc::channel(4);
        let runtime = WaRuntime::new(auth, tx).await.unwrap();
        runtime.state_handle().set(crate::gateway::interfaces::whatsapp::wa_runtime::ConnectionState::Connected);
        let msg = OutboundMessage::text("jid", "hello");
        let result = WaOutbound::send_message(&runtime, msg, &Default::default()).await;
        assert!(result.is_ok());
    }
}
