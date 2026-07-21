//! `WeChatRuntime` Core Polling Loop
//!
//! Implements the long-polling getupdates loop for receiving messages.

use std::time::Duration;
use tokio::sync::{Mutex, RwLock};
use tokio::time::sleep;

use crate::gateway::channel::InboundMessageSender;

use super::api::{ApiResult, ILinkApi, WeChatApiError};
use super::auth::ContextTokenStore;
use super::config::WeChatConfig;
use super::inbound::dedup::MessageDedup;
use super::inbound::mapper::map_message_to_inbound;
use super::inbound::policy::should_accept_message;
use super::sync_buf::{load_sync_buf, save_sync_buf};
use super::types::LONG_POLL_TIMEOUT_MS;

const INITIAL_BACKOFF_MS: u64 = 100;
const MAX_BACKOFF_MS: u64 = 30000;

pub struct WeChatRuntime {
    config: WeChatConfig,
    api: ILinkApi,
    token_store: ContextTokenStore,
    sync_buf: RwLock<String>,
    running: RwLock<bool>,
    dedup: Mutex<MessageDedup>,
}

impl WeChatRuntime {
    pub fn new(config: WeChatConfig, token_store: ContextTokenStore) -> Self {
        Self {
            api: ILinkApi::new(config.base_url.clone()),
            config,
            token_store,
            sync_buf: RwLock::new(String::new()),
            running: RwLock::new(false),
            dedup: Mutex::new(MessageDedup::new()),
        }
    }

    pub async fn start(&self, sender: InboundMessageSender) {
        {
            let mut running = self.running.write().await;
            if *running {
                return;
            }
            *running = true;
        }

        let sync_buf = load_sync_buf(&self.config.data_dir, &self.config.account_id).await;
        self.token_store.restore(&self.config.account_id).await;
        {
            let mut buf = self.sync_buf.write().await;
            *buf = sync_buf;
        }

        let mut backoff_ms = INITIAL_BACKOFF_MS;

        loop {
            {
                let running = self.running.read().await;
                if !*running {
                    break;
                }
            }

            let sync_buf = self.sync_buf.read().await.clone();
            let timeout = LONG_POLL_TIMEOUT_MS;

            match self
                .api
                .get_updates(&self.config.token, &sync_buf, timeout)
                .await
            {
                Ok(resp) => {
                    let processed = if resp.msgs.is_empty() {
                        true
                    } else {
                        self.process_messages(&resp.msgs, &sender).await
                    };
                    if processed {
                        if let Some(new_buf) = resp.get_updates_buf {
                            let mut buf = self.sync_buf.write().await;
                            *buf = new_buf.clone();
                            drop(buf);
                            save_sync_buf(&self.config.data_dir, &self.config.account_id, &new_buf)
                                .await;
                        }
                    }

                    backoff_ms = INITIAL_BACKOFF_MS;
                }
                Err(WeChatApiError::SessionExpired) => {
                    tracing::error!("WeChat session expired — re-authentication required");
                    backoff_ms = (backoff_ms * 2).min(MAX_BACKOFF_MS);
                }
                Err(WeChatApiError::Api {
                    ret,
                    errcode,
                    errmsg,
                }) => {
                    tracing::warn!(
                        ret = ret,
                        errcode = ?errcode,
                        errmsg = ?errmsg,
                        "WeChat get_updates returned non-zero status"
                    );
                    backoff_ms = (backoff_ms * 2).min(MAX_BACKOFF_MS);
                }
                Err(e) => {
                    tracing::warn!("get_updates error: {}", e);
                    backoff_ms = (backoff_ms * 2).min(MAX_BACKOFF_MS);
                }
            }

            sleep(Duration::from_millis(backoff_ms)).await;
        }
    }

    async fn process_messages(
        &self,
        messages: &[super::types::Message],
        sender: &InboundMessageSender,
    ) -> bool {
        for msg in messages {
            if !should_accept_message(msg, &self.config) {
                continue;
            }

            let duplicate = {
                let mut dedup = self.dedup.lock().await;
                dedup.is_duplicate(&msg.msg_id)
            };
            if duplicate {
                tracing::debug!(msg_id = %msg.msg_id, "skipping duplicate message");
                continue;
            }

            if let Some(inbound) = map_message_to_inbound(
                msg,
                &crate::gateway::channel::ChannelId::new("wechat"),
                &self.config.account_id,
            ) {
                if let Some(context_token) = msg.context_token.clone() {
                    self.token_store
                        .set(
                            &self.config.account_id,
                            inbound.conversation_id.as_str(),
                            context_token,
                        )
                        .await;
                }
                if let Err(e) = sender.send(inbound) {
                    tracing::error!("failed to send message to channel: {:?}", e);
                    let mut dedup = self.dedup.lock().await;
                    dedup.remove(&msg.msg_id);
                    return false;
                }
            }
        }
        true
    }

    pub async fn stop(&self) {
        let mut running = self.running.write().await;
        *running = false;
    }

    pub async fn send_message(
        &self,
        token: &str,
        payload: super::types::SendMessagePayload,
    ) -> ApiResult<()> {
        self.api.send_message(token, payload).await
    }

    pub async fn get_context_token(&self, account_id: &str, user_id: &str) -> Option<String> {
        self.token_store.get(account_id, user_id).await
    }

    pub async fn send_typing(&self, token: &str, user_id: &str) -> ApiResult<()> {
        let resp = self.api.get_config(token, user_id, None).await?;

        let ticket = resp.typing_ticket.ok_or_else(|| {
            WeChatApiError::InvalidResponse("No typing ticket available".to_string())
        })?;

        let payload = super::types::SendTypingPayload {
            ilink_user_id: user_id.to_string(),
            typing_ticket: ticket,
            status: super::types::TYPING_START,
        };

        self.api.send_typing(token, payload).await
    }

    pub async fn is_running(&self) -> bool {
        let running = self.running.read().await;
        *running
    }
}
