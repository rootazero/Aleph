//! Discord Account Pool
//!
//! Manages multiple Discord bot instances with pooled creation and reuse.

use crate::gateway::interfaces::discord::config::{AccountConfig, DiscordChannelConfig};
use crate::sync_primitives::Arc;
use serenity::all::Http;
use std::collections::HashMap;
use tokio::sync::RwLock;

/// Discord bot instance wrapper
#[derive(Clone)]
pub struct DiscordBot {
    pub account_id: String,
    pub client: Arc<Http>,
    pub config: AccountConfig,
}

/// Pool for managing multiple Discord bot instances
pub struct DiscordAccountPool {
    config: DiscordChannelConfig,
    bots: Arc<RwLock<HashMap<String, Arc<DiscordBot>>>>,
}

impl DiscordAccountPool {
    pub fn new(config: DiscordChannelConfig) -> Self {
        Self {
            config,
            bots: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn get_or_create(
        &self,
        account_id: &str,
    ) -> Result<Arc<DiscordBot>, AccountPoolError> {
        {
            let bots = self.bots.read().await;
            if let Some(bot) = bots.get(account_id) {
                return Ok(bot.clone());
            }
        }

        let account_config = self
            .config
            .accounts
            .get(account_id)
            .ok_or_else(|| AccountPoolError::AccountNotFound(account_id.to_string()))?;

        let bot = self.create_bot(account_id, account_config.clone()).await?;

        {
            let mut bots = self.bots.write().await;
            bots.insert(account_id.to_string(), bot.clone());
        }

        Ok(bot)
    }

    async fn create_bot(
        &self,
        account_id: &str,
        config: AccountConfig,
    ) -> Result<Arc<DiscordBot>, AccountPoolError> {
        let http = Http::new(&config.token);
        Ok(Arc::new(DiscordBot {
            account_id: account_id.to_string(),
            client: Arc::new(http),
            config,
        }))
    }

    pub async fn list_accounts(&self) -> Vec<String> {
        let bots = self.bots.read().await;
        bots.keys().cloned().collect()
    }

    pub async fn remove_bot(&self, account_id: &str) -> Result<(), AccountPoolError> {
        let mut bots = self.bots.write().await;
        bots.remove(account_id)
            .ok_or_else(|| AccountPoolError::AccountNotFound(account_id.to_string()))?;
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AccountPoolError {
    #[error("account not found: {0}")]
    AccountNotFound(String),

    #[error("failed to create client: {0}")]
    ClientCreationFailed(String),
}
