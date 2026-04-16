use crate::gateway::channel::{ChannelError, ChannelResult};
use teloxide::{prelude::*, types::ChatId};

pub async fn send_poll(
    bot: &Bot,
    chat_id: ChatId,
    question: &str,
    options: Vec<String>,
    is_anonymous: bool,
    allows_multiple_answers: bool,
    open_period: Option<u16>,
) -> ChannelResult<String> {
    let mut req = bot.send_poll(chat_id, question, options);
    req.is_anonymous = Some(is_anonymous);
    req.allows_multiple_answers = Some(allows_multiple_answers);
    if let Some(period) = open_period {
        req.open_period = Some(period);
    }
    match req.await {
        Ok(msg) => Ok(msg.poll().map(|p| p.id.clone()).unwrap_or_default()),
        Err(e) => Err(ChannelError::SendFailed(format!("Poll failed: {}", e))),
    }
}
