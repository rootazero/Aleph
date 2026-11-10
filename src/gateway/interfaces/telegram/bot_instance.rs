use super::access::AccessController;
use super::config_resolver::ResolvedConfig;
use super::config_v2::TelegramAccountConfig;
use super::offset::OffsetTracker;
use crate::sync_primitives::{Arc, AtomicBool};
use reqwest_legacy as reqwest;
use teloxide::Bot;
use tokio::sync::oneshot;

fn create_bot_with_proxy(token: &str, proxy_url: Option<&str>) -> Bot {
    match proxy_url {
        Some(url) if !url.is_empty() => {
            let client = reqwest::Proxy::all(url).and_then(|proxy| {
                teloxide::net::default_reqwest_settings()
                    .proxy(proxy)
                    .build()
            });
            match client {
                Ok(client) => Bot::with_client(token, client),
                Err(error) => {
                    tracing::warn!(%error, "Telegram proxy setup failed; falling back to direct connection");
                    Bot::new(token)
                }
            }
        }
        _ => Bot::new(token),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn invalid_proxy_falls_back_without_panicking() {
        let _bot = super::create_bot_with_proxy("123:test", Some(":// invalid proxy"));
    }
}

pub struct BotInstance {
    pub account_id: String,
    pub bot: Bot,
    pub resolved_config: ResolvedConfig,
    /// Per-account access controller (SW-1): bound to this account's
    /// `account_id`, looks up `chat_id` / `thread_id` overrides per inbound.
    pub access: Arc<AccessController>,
    pub offset_tracker: Option<Arc<OffsetTracker>>,
    pub shutdown_tx: Option<oneshot::Sender<()>>,
    /// Stops the per-account health probe. The probe task owns the receiver,
    /// so the sender has to be held here for `stop()` to reach it.
    pub health_shutdown_tx: Option<oneshot::Sender<()>>,
    /// Connection health flag — updated by the polling watchdog's `get_me()` probe.
    pub is_healthy: Arc<AtomicBool>,
}

impl BotInstance {
    #[must_use]
    pub fn new(
        account: &TelegramAccountConfig,
        resolved_config: ResolvedConfig,
        access: Arc<AccessController>,
    ) -> Self {
        let bot = create_bot_with_proxy(&account.bot_token, account.proxy_url.as_deref());
        Self {
            account_id: account.id.clone(),
            bot,
            resolved_config,
            access,
            offset_tracker: None,
            shutdown_tx: None,
            health_shutdown_tx: None,
            is_healthy: Arc::new(AtomicBool::new(true)),
        }
    }

    pub fn set_offset_tracker(&mut self, tracker: Arc<OffsetTracker>) {
        self.offset_tracker = Some(tracker);
    }
}
