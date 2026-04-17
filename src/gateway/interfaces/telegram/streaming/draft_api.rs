//! Draft API handler for Telegram Bot API experimental features.
//!
//! This module provides functionality to display draft text in the user's
//! input box preview area. Requires the `telegram-draft-api` feature flag.

use crate::gateway::channel::ChannelResult;
use std::time::Duration;

/// Handler for Telegram Draft Stream API.
///
/// This is an experimental feature that allows bots to display text
/// in the user's input box as a draft/preview.
#[derive(Clone)]
pub struct DraftStreamHandler {
    bot_token: String,
    chat_id: i64,
    http_client: reqwest::Client,
}

impl DraftStreamHandler {
    /// Create a new draft handler.
    ///
    /// # Arguments
    /// * `bot_token` - Telegram bot token
    /// * `chat_id` - Target chat ID
    pub fn new(bot_token: impl Into<String>, chat_id: i64) -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_default();

        Self {
            bot_token: bot_token.into(),
            chat_id,
            http_client,
        }
    }

    /// Send draft text to the user's input box.
    ///
    /// # Arguments
    /// * `text` - The draft text to display
    pub async fn send_draft(&self, text: &str) -> ChannelResult<()> {
        let url = format!(
            "https://api.telegram.org/bot{}/setChatAction",
            self.bot_token
        );

        let body = serde_json::json!({
            "chat_id": self.chat_id,
            "action": "typing",
            "draft_text": text,
        });

        match self.http_client.post(&url).json(&body).send().await {
            Ok(response) => {
                if response.status().is_success() {
                    tracing::debug!("Draft sent successfully");
                    Ok(())
                } else {
                    let status = response.status();
                    let body = response.text().await.unwrap_or_default();
                    tracing::debug!("Draft API returned error: {} - {}", status, body);
                    Ok(())
                }
            }
            Err(e) => {
                tracing::debug!("Draft API request failed (non-critical): {}", e);
                Ok(())
            }
        }
    }

    /// Clear the draft from the user's input box.
    pub async fn clear_draft(&self) -> ChannelResult<()> {
        let url = format!(
            "https://api.telegram.org/bot{}/setChatAction",
            self.bot_token
        );

        let body = serde_json::json!({
            "chat_id": self.chat_id,
            "action": "cancel",
        });

        match self.http_client.post(&url).json(&body).send().await {
            Ok(response) => {
                if response.status().is_success() {
                    tracing::debug!("Draft cleared successfully");
                } else {
                    tracing::debug!("Draft clear returned error: {}", response.status());
                }
                Ok(())
            }
            Err(e) => {
                tracing::debug!("Draft clear request failed (non-critical): {}", e);
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_draft_handler_creation() {
        let handler = DraftStreamHandler::new("test_token", 123456);
        assert_eq!(handler.chat_id, 123456);
        assert_eq!(handler.bot_token, "test_token");
    }

    #[tokio::test]
    async fn test_send_draft_non_blocking() {
        let handler = DraftStreamHandler::new("invalid_token_for_test", 123456);
        let result = handler.send_draft("test draft").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_clear_draft_non_blocking() {
        let handler = DraftStreamHandler::new("invalid_token_for_test", 123456);
        let result = handler.clear_draft().await;
        assert!(result.is_ok());
    }
}
