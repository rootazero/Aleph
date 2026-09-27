use crate::gateway::channel::{ChannelResult, MessageId, OutboundMessage};
use crate::gateway::interfaces::whatsapp::config::WhatsAppConfig;
use crate::gateway::interfaces::whatsapp::wa_runtime::WaRuntime;
use crate::sync_primitives::Arc;

pub struct WaOutbound;

impl WaOutbound {
    pub async fn send_message(
        runtime: &Arc<dyn WaRuntime>,
        msg: OutboundMessage,
        _config: &WhatsAppConfig,
    ) -> ChannelResult<MessageId> {
        runtime.send_message(msg).await.map_err(|e| {
            crate::gateway::channel::ChannelError::Internal(format!("send_message: {e}"))
        })
    }

    pub async fn send_reaction(
        runtime: &Arc<dyn WaRuntime>,
        jid: &str,
        msg_id: &str,
        emoji: &str,
    ) -> ChannelResult<()> {
        runtime.send_reaction(jid, msg_id, emoji).await.map_err(|e| {
            crate::gateway::channel::ChannelError::Internal(format!("send_reaction: {e}"))
        })
    }

    pub async fn mark_read(
        runtime: &Arc<dyn WaRuntime>,
        msg_id: &str,
    ) -> ChannelResult<()> {
        runtime.mark_read(msg_id).await.map_err(|e| {
            crate::gateway::channel::ChannelError::Internal(format!("mark_read: {e}"))
        })
    }

    pub async fn send_typing(
        runtime: &Arc<dyn WaRuntime>,
        jid: &str,
    ) -> ChannelResult<()> {
        runtime.send_typing(jid).await.map_err(|e| {
            crate::gateway::channel::ChannelError::Internal(format!("send_typing: {e}"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::interfaces::whatsapp::wa_auth::WaAuthManager;
    use crate::gateway::interfaces::whatsapp::wa_runtime::RealWaRuntime;
    use crate::secrets::vault::SecretVault;
    use tempfile::TempDir;

    /// An absent client is `NotConnected` even when the state says otherwise.
    ///
    /// The auth manager is scaffolding and is built against a throwaway vault
    /// on purpose. `WaAuthManager::new` resolves `SecretVault::default_path()`,
    /// so the previous `WaAuthManager::new("test")` reached into whatever vault
    /// the developer running the suite actually owns — and the `save` beside it
    /// then wrote a `whatsapp/auth/test` entry into it. `RealWaRuntime::new`
    /// never reads the stored blob (it moves the manager into the struct and
    /// nothing else touches it before `start`), so that write bought this test
    /// nothing; what it did buy, once `vault_store::save` became fail-closed
    /// on the shared-token manager, was a panic on the scaffolding instead of
    /// an assertion about the subject.
    #[tokio::test]
    async fn test_send_message_without_client_returns_error() {
        let dir = TempDir::new().unwrap();
        let vault = SecretVault::open(dir.path().join("test.vault")).unwrap();
        let auth = WaAuthManager::with_vault(vault, "test");
        let (tx, _rx) = tokio::sync::mpsc::channel(4);
        let runtime = RealWaRuntime::new(auth, tx).await.unwrap();
        runtime
            .state_handle()
            .set(crate::gateway::interfaces::whatsapp::wa_runtime::ConnectionState::Connected);
        // Stash the client would have lived in; instead, deliberately leave
        // it `None` so `get_client()` returns `NotConnected`.
        let runtime: Arc<dyn WaRuntime> = Arc::new(runtime);
        let msg = OutboundMessage::text("jid", "hello");
        let result = WaOutbound::send_message(&runtime, msg, &Default::default()).await;
        assert!(matches!(
            result,
            Err(crate::gateway::channel::ChannelError::Internal(_))
        ));
    }
}