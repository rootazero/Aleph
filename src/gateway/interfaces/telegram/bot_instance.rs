use super::config_resolver::ResolvedConfig;
use super::config_v2::TelegramAccountConfig;
use super::offset::OffsetTracker;
use crate::gateway::channel::{CallbackQuery, ChannelState};
use std::sync::Arc;
use teloxide::Bot;
use tokio::sync::{mpsc, oneshot};

pub struct BotInstance {
    pub account_id: String,
    pub bot: Bot,
    pub resolved_config: ResolvedConfig,
    pub callback_tx: mpsc::Sender<CallbackQuery>,
    pub channel_state: ChannelState,
    pub offset_tracker: Option<Arc<OffsetTracker>>,
    pub shutdown_tx: Option<oneshot::Sender<()>>,
}

impl BotInstance {
    pub fn new(
        account: &TelegramAccountConfig,
        callback_tx: mpsc::Sender<CallbackQuery>,
        resolved_config: ResolvedConfig,
    ) -> Self {
        let bot = Bot::new(&account.bot_token);
        Self {
            account_id: account.id.clone(),
            bot,
            resolved_config,
            callback_tx,
            channel_state: ChannelState::new(100),
            offset_tracker: None,
            shutdown_tx: None,
        }
    }

    pub fn set_offset_tracker(&mut self, tracker: Arc<OffsetTracker>) {
        self.offset_tracker = Some(tracker);
    }
}
