//! WeChatRuntime Core Polling Loop
//!
//! Implements the long-polling getupdates loop for receiving messages.

use reqwest::Client;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::time::sleep;

use super::api::ILinkApi;
use super::auth::ContextTokenStore;
use super::config::WeChatConfig;
use super::inbound::mapper::map_message_to_inbound;
use super::inbound::policy::should_accept_message;
use super::sync_buf::{load_sync_buf, save_sync_buf};
use super::types::{GetUpdatesResponse, LONG_POLL_TIMEOUT_MS};

pub struct WeChatRuntime {
    config: WeChatConfig,
    api: ILinkApi,
    http: Client,
    token_store: ContextTokenStore,
    sync_buf: RwLock<String>,
    running: RwLock<bool>,
}

impl WeChatRuntime {
    pub fn new(config: WeChatConfig, token_store: ContextTokenStore) -> Self {
        Self {
            api: ILinkApi::new(config.base_url.clone()),
            http: Client::new(),
            config,
            token_store,
            sync_buf: RwLock::new(String::new()),
            running: RwLock::new(false),
        }
    }

    pub async fn start(&self, sender: tokio::sync::mpsc::Sender<crate::gateway::channel::InboundMessage>) {
        {
            let mut running = self.running.write().await;
            if *running {
                return;
            }
            *running = true;
        }

        let sync_buf = load_sync_buf(&self.config.account_id, &self.config.account_id).await;
        {
            let mut buf = self.sync_buf.write().await;
            *buf = sync_buf;
        }

        loop {
            {
                let running = self.running.read().await;
                if !*running {
                    break;
                }
            }

            let sync_buf = self.sync_buf.read().await.clone();
            let timeout = LONG_POLL_TIMEOUT_MS;

            match self.api.get_updates(&self.config.token, &sync_buf, timeout).await {
                Ok(resp) => {
                    if resp.ret == 0 {
                        if let Some(new_buf) = resp.get_updates_buf {
                            let mut buf = self.sync_buf.write().await;
                            *buf = new_buf.clone();
                            drop(buf);
                            save_sync_buf(&self.config.account_id, &self.config.account_id, &new_buf).await;
                        }

                        self.process_messages(&resp.msgs, &sender).await;
                    }
                }
                Err(e) => {
                    tracing::warn!("get_updates error: {}", e);
                }
            }

            sleep(Duration::from_millis(100)).await;
        }
    }

    async fn process_messages(
        &self,
        messages: &[super::types::Message],
        sender: &tokio::sync::mpsc::Sender<crate::gateway::channel::InboundMessage>,
    ) {
        for msg in messages {
            if !should_accept_message(msg, &self.config) {
                continue;
            }

            if let Some(inbound) = map_message_to_inbound(msg, &crate::gateway::channel::ChannelId::new("wechat"), &self.config.account_id) {
                if let Err(e) = sender.send(inbound).await {
                    tracing::error!("failed to send message to channel: {}", e);
                }
            }
        }
    }

    pub async fn stop(&self) {
        let mut running = self.running.write().await;
        *running = false;
    }

    pub async fn send_message(&self, token: &str, payload: super::types::SendMessagePayload) -> Result<(), String> {
        self.api.send_message(token, payload).await
    }

    pub fn is_running(&self) -> impl std::future::Future<Output = bool> + Send + '_ {
        async move {
            let running = self.running.read().await;
            *running
        }
    }
}