use crate::gateway::channel::{ChannelResult, MessageId, OutboundMessage};
use crate::gateway::interfaces::whatsapp::config::WhatsAppAccountConfig;
use crate::gateway::interfaces::whatsapp::wa_runtime::WaRuntime;

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

    pub async fn mark_read(runtime: &WaRuntime, msg_id: &str) -> ChannelResult<()> {
        runtime.mark_read(msg_id).await
    }

    pub async fn send_typing(runtime: &WaRuntime, jid: &str) -> ChannelResult<()> {
        runtime.send_typing(jid).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::interfaces::whatsapp::wa_auth::WaAuthManager;
    use crate::secrets::vault::SecretVault;
    use tempfile::TempDir;

    /// An absent client is `NotConnected` even when the state says otherwise.
    ///
    /// The auth manager is scaffolding and is built against a throwaway vault
    /// on purpose. `WaAuthManager::new` resolves `SecretVault::default_path()`,
    /// so the previous `WaAuthManager::new("test")` reached into whatever vault
    /// the developer running the suite actually owns — and the `save` beside it
    /// then wrote a `whatsapp/auth/test` entry into it. `WaRuntime::new` never
    /// reads the stored blob (it moves the manager into the struct and nothing
    /// else touches it before `start`), so that write bought this test nothing;
    /// what it did buy, once `vault_store::save` became fail-closed on the
    /// shared-token manager, was a panic on the scaffolding instead of an
    /// assertion about the subject.
    #[tokio::test]
    async fn test_send_message_without_client_returns_error() {
        let dir = TempDir::new().unwrap();
        let vault = SecretVault::open(dir.path().join("test.vault")).unwrap();
        let auth = WaAuthManager::with_vault(vault, "test");
        let (tx, _rx) = tokio::sync::mpsc::channel(4);
        let runtime = WaRuntime::new(auth, tx).await.unwrap();
        runtime
            .state_handle()
            .set(crate::gateway::interfaces::whatsapp::wa_runtime::ConnectionState::Connected);
        let msg = OutboundMessage::text("jid", "hello");
        let result = WaOutbound::send_message(&runtime, msg, &Default::default()).await;
        assert!(matches!(
            result,
            Err(crate::gateway::channel::ChannelError::NotConnected(_))
        ));
    }
}
