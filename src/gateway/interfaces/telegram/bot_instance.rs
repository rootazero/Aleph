use super::config_resolver::ResolvedConfig;
use super::config_v2::TelegramAccountConfig;
use super::offset::OffsetTracker;
use crate::gateway::channel::{CallbackQuery, ChannelState};
use crate::sync_primitives::{Arc, AtomicBool, Ordering};
use teloxide::{prelude::Requester, Bot};
use tokio::sync::{mpsc, oneshot};

fn create_bot_with_proxy(token: &str, proxy_url: Option<&str>) -> Bot {
    match proxy_url {
        Some(url) if !url.is_empty() => match reqwest::Proxy::all(url) {
            Ok(proxy) => {
                match teloxide::net::default_reqwest_settings()
                    .proxy(proxy)
                    .build()
                {
                    Ok(client) => Bot::with_client(token, client),
                    Err(e) => {
                        tracing::warn!(
                            proxy_url = %url,
                            error = %e,
                            "Failed to build HTTP client with proxy; falling back to no proxy"
                        );
                        Bot::new(token)
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                    proxy_url = %url,
                    error = %e,
                    "Invalid proxy URL; falling back to no proxy"
                );
                Bot::new(token)
            }
        },
        _ => Bot::new(token),
    }
}

pub struct BotInstance {
    pub account_id: String,
    pub bot: Bot,
    pub resolved_config: ResolvedConfig,
    pub callback_tx: mpsc::Sender<CallbackQuery>,
    pub channel_state: ChannelState,
    pub offset_tracker: Option<Arc<OffsetTracker>>,
    pub shutdown_tx: Option<oneshot::Sender<()>>,
    /// Connection health flag — set to false if `get_me()` fails
    pub is_healthy: Arc<AtomicBool>,
}

impl BotInstance {
    #[must_use]
    pub fn new(
        account: &TelegramAccountConfig,
        callback_tx: mpsc::Sender<CallbackQuery>,
        resolved_config: ResolvedConfig,
    ) -> Self {
        let bot = create_bot_with_proxy(&account.bot_token, account.proxy_url.as_deref());
        Self {
            account_id: account.id.clone(),
            bot,
            resolved_config,
            callback_tx,
            channel_state: ChannelState::new(100),
            offset_tracker: None,
            shutdown_tx: None,
            is_healthy: Arc::new(AtomicBool::new(true)),
        }
    }

    pub fn set_offset_tracker(&mut self, tracker: Arc<OffsetTracker>) {
        self.offset_tracker = Some(tracker);
    }

    /// Check bot connectivity by calling `get_me()`.
    ///
    /// Updates `is_healthy` flag and returns the result.
    pub async fn health_check(&self) -> bool {
        match tokio::time::timeout(std::time::Duration::from_secs(10), self.bot.get_me()).await {
            Ok(Ok(_me)) => {
                self.is_healthy.store(true, Ordering::Relaxed);
                true
            }
            Ok(Err(e)) => {
                tracing::warn!(
                    account_id = %self.account_id,
                    error = %e,
                    "Bot health check failed: API error"
                );
                self.is_healthy.store(false, Ordering::Relaxed);
                false
            }
            Err(_) => {
                tracing::warn!(
                    account_id = %self.account_id,
                    "Bot health check failed: timeout"
                );
                self.is_healthy.store(false, Ordering::Relaxed);
                false
            }
        }
    }
}
