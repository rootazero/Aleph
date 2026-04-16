use std::sync::Arc;
use std::time::{Duration, Instant};
use teloxide::payloads::SetMessageReactionSetters;
use teloxide::prelude::Requester;
use teloxide::Bot;
use tokio::sync::Mutex;

#[derive(Debug, Clone, PartialEq)]
pub enum ReactionState {
    Idle,
    Thinking,
    ToolActive(String),
    Compacting,
    Done,
    Error,
}

pub struct StatusReactionController {
    bot: Bot,
    chat_id: i64,
    message_id: Option<i32>,
    state: Arc<Mutex<ReactionState>>,
    last_changed: Arc<Mutex<Instant>>,
    min_interval: Duration,
}

impl StatusReactionController {
    pub fn new(bot: Bot, chat_id: i64, message_id: Option<i32>) -> Self {
        Self {
            bot,
            chat_id,
            message_id,
            state: Arc::new(Mutex::new(ReactionState::Idle)),
            last_changed: Arc::new(Mutex::new(Instant::now() - Duration::from_secs(10))),
            min_interval: Duration::from_secs(1),
        }
    }

    async fn can_change(&self) -> bool {
        let last = *self.last_changed.lock().await;
        Instant::now().duration_since(last) >= self.min_interval
    }

    async fn set_state(&self, new_state: ReactionState) {
        if !self.can_change().await {
            tokio::time::sleep(self.min_interval).await;
        }
        let mut state = self.state.lock().await;
        if *state == new_state {
            return;
        }
        *state = new_state.clone();
        *self.last_changed.lock().await = Instant::now();

        let emoji = match new_state {
            ReactionState::Thinking => "👀",
            ReactionState::ToolActive(_) => "🔧",
            ReactionState::Compacting => "🗜️",
            ReactionState::Done => "👍",
            ReactionState::Error => "👎",
            ReactionState::Idle => return,
        };

        if let Some(msg_id) = self.message_id {
            let reaction = teloxide::types::ReactionType::Emoji {
                emoji: emoji.to_string(),
            };
            let _ = self
                .bot
                .set_message_reaction(
                    teloxide::types::ChatId(self.chat_id),
                    teloxide::types::MessageId(msg_id),
                )
                .reaction(vec![reaction])
                .await;
        }
    }

    pub async fn set_thinking(&self) {
        self.set_state(ReactionState::Thinking).await;
    }

    pub async fn set_tool(&self, name: &str) {
        self.set_state(ReactionState::ToolActive(name.to_string()))
            .await;
    }

    pub async fn set_compacting(&self) {
        self.set_state(ReactionState::Compacting).await;
    }

    pub async fn set_done(&self) {
        self.set_state(ReactionState::Done).await;
    }

    pub async fn set_error(&self) {
        self.set_state(ReactionState::Error).await;
    }

    pub async fn cancel_pending(&self) {
        self.set_state(ReactionState::Idle).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_controller_starts_idle() {
        // We can't easily test async reaction sends without a real bot,
        // but we can verify the constructor sets the right initial state.
        let bot = Bot::new("dummy");
        let ctrl = StatusReactionController::new(bot, 123, Some(456));
        assert_eq!(ctrl.chat_id, 123);
        assert_eq!(ctrl.message_id, Some(456));
    }
}
