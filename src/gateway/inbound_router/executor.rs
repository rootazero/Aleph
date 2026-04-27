//! Agent execution for inbound messages
//!
//! Collapsed from the original two near-identical methods
//! (execute_for_context / execute_for_context_with_metadata)
//! into a single parameterized implementation.

use crate::sync_primitives::{Arc, Mutex};
use std::collections::HashMap;
use tracing::{error, info};
use uuid::Uuid;

use crate::gateway::execution_engine::RunRequest;
use crate::gateway::inbound_context::InboundContext;
use crate::gateway::reply_emitter::{ReplyEmitter, ReplyEmitterConfig};

use super::types::{RoutingError, SLASH_COMMAND_MODE_KEY};
use super::InboundMessageRouter;

impl InboundMessageRouter {
    /// Execute the agent for the given context
    pub(super) async fn execute_for_context(
        &self,
        ctx: &InboundContext,
    ) -> Result<(), RoutingError> {
        self.execute_for_context_inner(ctx, None).await
    }

    /// Execute the agent with slash command metadata
    pub(super) async fn execute_for_context_with_metadata(
        &self,
        ctx: &InboundContext,
        slash_command_mode: String,
    ) -> Result<(), RoutingError> {
        self.execute_for_context_inner(ctx, Some(slash_command_mode))
            .await
    }

    /// Unified execution implementation
    async fn execute_for_context_inner(
        &self,
        ctx: &InboundContext,
        slash_command_mode: Option<String>,
    ) -> Result<(), RoutingError> {
        // Pipeline path: if debounce buffer is configured, use it
        if let Some(buffer) = &self.debounce_buffer {
            buffer.submit(ctx.clone()).await;
            return Ok(());
        }

        // Check if execution support is configured
        let (agent_registry, execution_adapter) = match (
            self.agent_registry.as_ref(),
            self.execution_adapter.as_ref(),
        ) {
            (Some(ar), Some(ea)) => (ar.clone(), ea.clone()),
            _ => {
                info!(
                    "Would execute agent for session {} with input: {} (execution not configured)",
                    ctx.session_key.to_key_string(),
                    ctx.message.text.chars().take(100).collect::<String>()
                );
                return Ok(());
            }
        };

        // Get the agent ID from the session key
        let agent_id = ctx.session_key.agent_id();

        // Look up the agent in the registry
        let agent = agent_registry
            .get(agent_id)
            .await
            .ok_or_else(|| RoutingError::AgentNotFound(agent_id.to_string()))?;

        // Generate a unique run ID
        let run_id = Uuid::new_v4().to_string();

        // Determine voice state for this channel
        let voice_state = self
            .channel_registry
            .get_voice_state(ctx.reply_route.channel_id.as_str())
            .await;
        let voice_enabled = voice_state.is_active() || ctx.voice_reply_hint;

        // Create a ReplyEmitter config based on output_mode
        let mut reply_config = match &self.app_config {
            Some(cfg) => {
                let cfg = cfg.read().await;
                let mode = cfg
                    .behavior
                    .as_ref()
                    .map(|b| b.output_mode.as_str())
                    .unwrap_or("typewriter");
                ReplyEmitterConfig::from_output_mode(mode)
            }
            None => ReplyEmitterConfig::default(),
        };
        reply_config.voice_enabled = voice_enabled;
        reply_config.voice_reply_hint = ctx.voice_reply_hint;

        // Per-channel streaming override: if the channel declares EditBased,
        // enable streaming and apply channel-specific debounce/threshold.
        if let Some(handle) = self.channel_registry.get(&ctx.reply_route.channel_id).await {
            let ch = handle.read().await;
            let caps = ch.capabilities();
            if caps.stream_protocol == crate::gateway::channel::StreamProtocol::EditBased {
                reply_config.stream_enabled = true;
                reply_config.max_message_length = caps.max_message_length;
            }
        }

        let pending_media: crate::gateway::media::PendingMedia = Arc::new(Mutex::new(Vec::new()));

        // Detect feishu/telegram channels and optionally construct custom emitters
        let (is_feishu, is_telegram) = {
            if let Some(handle) = self.channel_registry.get(&ctx.reply_route.channel_id).await {
                let ch = handle.read().await;
                (
                    ch.channel_type() == "feishu",
                    ch.channel_type() == "telegram",
                )
            } else {
                (false, false)
            }
        };

        // Always attach voice deps so that mid-request voice_mode_set
        // tool calls can take effect immediately (dynamic should_voice check)
        let attach_voice = |emitter: ReplyEmitter| -> ReplyEmitter {
            if let (Some(gen_reg), Some(gen_cfg)) = (
                self.generation_registry.as_ref(),
                self.generation_config.as_ref(),
            ) {
                return emitter.with_voice(voice_state.clone(), gen_reg.clone(), gen_cfg.clone());
            }
            emitter
        };

        let emitter: Arc<dyn crate::gateway::event_emitter::EventEmitter + Send + Sync> =
            if is_feishu {
                // Try to create FeishuEventEmitter with streaming + typing
                match self
                    .try_create_feishu_emitter(
                        ctx,
                        &run_id,
                        reply_config.clone(),
                        pending_media.clone(),
                    )
                    .await
                {
                    Some(fe) => Arc::new(fe),
                    None => {
                        let re = ReplyEmitter::with_config(
                            self.channel_registry.clone(),
                            ctx.reply_route.clone(),
                            run_id.clone(),
                            reply_config,
                            pending_media.clone(),
                        );
                        Arc::new(attach_voice(re))
                    }
                }
            } else if is_telegram {
                // Try to create Telegram orchestrated emitter
                match self
                    .try_create_telegram_emitter(
                        ctx,
                        &run_id,
                        reply_config.clone(),
                        pending_media.clone(),
                    )
                    .await
                {
                    Some(te) => Arc::new(te),
                    None => {
                        let re = ReplyEmitter::with_config(
                            self.channel_registry.clone(),
                            ctx.reply_route.clone(),
                            run_id.clone(),
                            reply_config,
                            pending_media.clone(),
                        );
                        Arc::new(attach_voice(re))
                    }
                }
            } else {
                let re = ReplyEmitter::with_config(
                    self.channel_registry.clone(),
                    ctx.reply_route.clone(),
                    run_id.clone(),
                    reply_config,
                    pending_media.clone(),
                );
                Arc::new(attach_voice(re))
            };

        // Build the run request metadata
        let mut metadata = HashMap::new();
        metadata.insert(
            "channel_id".to_string(),
            ctx.message.channel_id.as_str().to_string(),
        );
        metadata.insert("sender_id".to_string(), ctx.sender_normalized.clone());
        metadata.insert(
            "conversation_id".to_string(),
            ctx.message.conversation_id.as_str().to_string(),
        );
        if let Some(handle) = self.channel_registry.get(&ctx.reply_route.channel_id).await {
            let channel = handle.read().await;
            metadata.insert("platform".to_string(), channel.channel_type().to_string());
        }

        // Inject user locale for downstream i18n (run_loop, error messages)
        if let Some(ref cfg) = self.app_config {
            let cfg = cfg.read().await;
            let lang = cfg.general.language.as_deref().unwrap_or("zh");
            metadata.insert("locale".to_string(), lang.to_string());
        }
        let is_slash = slash_command_mode.is_some();
        if let Some(mode) = slash_command_mode {
            metadata.insert(SLASH_COMMAND_MODE_KEY.to_string(), mode);
        }
        if ctx.message.is_group {
            metadata.insert("is_group".to_string(), "true".to_string());
        }
        if ctx.is_mentioned {
            metadata.insert("is_mentioned".to_string(), "true".to_string());
        }
        if voice_enabled {
            metadata.insert("voice_mode_active".to_string(), "true".to_string());
        }

        let request = RunRequest {
            run_id: run_id.clone(),
            input: ctx.message.text.clone(),
            session_key: ctx.session_key.clone(),
            timeout_secs: None,
            metadata,
            attachments: ctx.message.attachments.clone(),
            pending_media: pending_media.clone(),
        };

        if !request.attachments.is_empty() {
            tracing::info!(
                target: "multimodal",
                probe = "P2_resolve",
                run_id = %request.run_id,
                session_key = %request.session_key.to_key_string(),
                attachment_count = request.attachments.len(),
                "RunRequest created with attachments"
            );
        }

        let label = if is_slash {
            "slash command for agent"
        } else {
            "agent"
        };
        info!(
            "Executing {} '{}' for session {} (run_id: {})",
            label,
            agent_id,
            ctx.session_key.to_key_string(),
            run_id
        );

        // Spawn the execution task (non-blocking)
        // When the agent is busy, retry with backoff instead of surfacing the
        // error to the user. This handles rapid-fire messages (voice bursts,
        // double-tap sends) gracefully.
        let error_channel_registry = self.channel_registry.clone();
        let error_reply_route = ctx.reply_route.clone();
        let error_app_config = self.app_config.clone();
        tokio::spawn(async move {
            const MAX_BUSY_RETRIES: usize = 6;
            const BUSY_BACKOFF_BASE_MS: u64 = 2000;

            let mut attempt = 0usize;
            loop {
                let result = execution_adapter
                    .execute(request.clone(), agent.clone(), emitter.clone())
                    .await;

                match result {
                    Ok(()) => break,
                    Err(e) => {
                        // Check if this is an AgentBusy error (retryable)
                        let is_busy = e.to_string().contains("Agent is busy");
                        if is_busy && attempt < MAX_BUSY_RETRIES {
                            attempt += 1;
                            let delay_ms = BUSY_BACKOFF_BASE_MS * (1 << attempt.min(3));
                            tracing::info!(
                                run_id = %run_id,
                                attempt,
                                delay_ms,
                                "Agent busy, queuing retry"
                            );
                            tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
                            continue;
                        }

                        error!("Agent execution failed (run_id: {}): {}", run_id, e);

                        // Resolve user locale from config
                        let locale = if let Some(ref cfg) = error_app_config {
                            let cfg = cfg.read().await;
                            crate::gateway::i18n::Locale::from_config(
                                cfg.general.language.as_deref(),
                            )
                        } else {
                            crate::gateway::i18n::Locale::Zh
                        };

                        // Send error feedback to user so they know what happened
                        let user_msg =
                            crate::gateway::i18n::format_execution_error(&e.to_string(), locale);
                        let reply = crate::gateway::channel::OutboundMessage::text(
                            error_reply_route.conversation_id.as_str(),
                            &user_msg,
                        );
                        if let Err(send_err) = error_channel_registry
                            .send(&error_reply_route.channel_id, reply)
                            .await
                        {
                            error!("Failed to send error reply: {}", send_err);
                        }
                        break;
                    }
                }
            }
        });

        Ok(())
    }

    /// Try to create a FeishuEventEmitter for feishu channels.
    async fn try_create_feishu_emitter(
        &self,
        ctx: &InboundContext,
        run_id: &str,
        reply_config: ReplyEmitterConfig,
        pending_media: crate::gateway::media::PendingMedia,
    ) -> Option<crate::gateway::interfaces::feishu::feishu_outbound::streaming::FeishuEventEmitter>
    {
        use crate::gateway::interfaces::feishu::api::FeishuApi;
        use crate::gateway::interfaces::feishu::auth::TokenManager;
        use crate::gateway::interfaces::feishu::feishu_outbound::streaming::FeishuEventEmitter;
        use crate::gateway::interfaces::feishu::FeishuConfig;

        // Read feishu config from app config
        let feishu_cfg = {
            let cfg = self.app_config.as_ref()?.read().await;
            let channel_id = ctx.reply_route.channel_id.as_str();
            let raw = cfg.channels.get(channel_id)?;
            serde_json::from_value::<FeishuConfig>(raw.clone()).ok()?
        };

        // TODO: Share Arc<FeishuApi> from FeishuChannel instead of creating per-emitter.
        // Current approach creates a new TokenManager + FeishuApi per message, causing
        // redundant token refresh requests. Requires exposing the shared API handle from
        // the channel via the registry or trait extension.
        // The lazy get_token() in TokenManager mitigates the worst case.
        let http = reqwest::Client::new();
        let base_url = feishu_cfg.base_url();
        let auth = Arc::new(TokenManager::new(
            &feishu_cfg.app_id,
            &feishu_cfg.app_secret,
            &base_url,
            http.clone(),
        ));
        if let Err(e) = auth.refresh_token().await {
            tracing::warn!("Failed to create feishu emitter client: {e}");
            return None;
        }
        let client = Arc::new(FeishuApi::new(auth, &base_url, http));

        let inner = ReplyEmitter::with_config(
            self.channel_registry.clone(),
            ctx.reply_route.clone(),
            run_id.to_string(),
            reply_config,
            pending_media,
        );

        let chat_id = ctx.message.conversation_id.as_str().to_string();
        let reply_to = ctx
            .reply_route
            .reply_to
            .as_ref()
            .map(|id| id.as_str().to_string());

        Some(FeishuEventEmitter::new(
            inner,
            client,
            ctx.reply_route.clone(),
            chat_id,
            reply_to,
            feishu_cfg.streaming,
            feishu_cfg.typing_indicator,
        ))
    }

    /// Try to create a TelegramEventEmitter for telegram channels.
    async fn try_create_telegram_emitter(
        &self,
        ctx: &InboundContext,
        _run_id: &str,
        _reply_config: ReplyEmitterConfig,
        _pending_media: crate::gateway::media::PendingMedia,
    ) -> Option<crate::gateway::interfaces::telegram::streaming::TelegramEventEmitter> {
        use crate::gateway::interfaces::telegram::parse_telegram_channel_config;
        use crate::gateway::interfaces::telegram::streaming::TelegramEventEmitter;

        let tg_cfg = {
            let cfg = self.app_config.as_ref()?.read().await;
            let channel_id = ctx.reply_route.channel_id.as_str();
            let raw = cfg.channels.get(channel_id)?;
            parse_telegram_channel_config(raw.clone()).ok()?
        };

        let account = tg_cfg.accounts.first()?;
        let streaming = account.streaming.clone().unwrap_or_default();

        // Only use orchestrated emitter when new streaming features are explicitly enabled
        if !streaming.draft_api_enabled
            && !streaming.reasoning_lane_enabled
            && streaming.status_reactions.processing.is_none()
            && streaming.status_reactions.tool_active.is_none()
            && streaming.status_reactions.complete.is_none()
        {
            return None;
        }

        let bot = teloxide::Bot::new(&account.bot_token);
        let conversation_id = ctx.message.conversation_id.as_str().to_string();

        Some(TelegramEventEmitter::new(
            bot,
            streaming,
            conversation_id,
            ctx.reply_route.clone(),
        ))
    }
}
