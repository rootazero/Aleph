//! Agent execution for inbound messages
//!
//! Collapsed from the original two near-identical methods
//! (execute_for_context / execute_for_context_with_metadata)
//! into a single parameterized implementation.

use std::collections::HashMap;
use crate::sync_primitives::Arc;
use tracing::{error, info};
use uuid::Uuid;

use crate::gateway::execution_engine::RunRequest;
use crate::gateway::inbound_context::InboundContext;
use crate::gateway::reply_emitter::{ReplyEmitter, ReplyEmitterConfig};

use super::types::{RoutingError, SLASH_COMMAND_MODE_KEY};
use super::InboundMessageRouter;

impl InboundMessageRouter {
    /// Execute the agent for the given context
    pub(super) async fn execute_for_context(&self, ctx: &InboundContext) -> Result<(), RoutingError> {
        self.execute_for_context_inner(ctx, None).await
    }

    /// Execute the agent with slash command metadata
    pub(super) async fn execute_for_context_with_metadata(
        &self,
        ctx: &InboundContext,
        slash_command_mode: String,
    ) -> Result<(), RoutingError> {
        self.execute_for_context_inner(ctx, Some(slash_command_mode)).await
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
        let agent = agent_registry.get(agent_id).await.ok_or_else(|| {
            RoutingError::AgentNotFound(agent_id.to_string())
        })?;

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
                let mode = cfg.behavior.as_ref()
                    .map(|b| b.output_mode.as_str())
                    .unwrap_or("typewriter");
                ReplyEmitterConfig::from_output_mode(mode)
            }
            None => ReplyEmitterConfig::default(),
        };
        reply_config.voice_enabled = voice_enabled;
        reply_config.voice_reply_hint = ctx.voice_reply_hint;

        // Detect feishu channel and optionally construct FeishuEventEmitter
        let is_feishu = {
            if let Some(handle) = self.channel_registry.get(&ctx.reply_route.channel_id).await {
                let ch = handle.read().await;
                ch.channel_type() == "feishu"
            } else {
                false
            }
        };

        // Helper closure: optionally attach voice deps to a ReplyEmitter
        let attach_voice = |emitter: ReplyEmitter| -> ReplyEmitter {
            if voice_enabled {
                if let (Some(gen_reg), Some(gen_cfg)) =
                    (self.generation_registry.as_ref(), self.generation_config.as_ref())
                {
                    return emitter.with_voice(
                        voice_state.clone(),
                        gen_reg.clone(),
                        gen_cfg.clone(),
                    );
                }
            }
            emitter
        };

        let emitter: Arc<dyn crate::gateway::event_emitter::EventEmitter + Send + Sync> = if is_feishu {
            // Try to create FeishuEventEmitter with streaming + typing
            match self.try_create_feishu_emitter(ctx, &run_id, reply_config.clone()).await {
                Some(fe) => Arc::new(fe),
                None => {
                    let re = ReplyEmitter::with_config(
                        self.channel_registry.clone(),
                        ctx.reply_route.clone(),
                        run_id.clone(),
                        reply_config,
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
            );
            Arc::new(attach_voice(re))
        };

        // Build the run request metadata
        let mut metadata = HashMap::new();
        metadata.insert("channel_id".to_string(), ctx.message.channel_id.as_str().to_string());
        metadata.insert("sender_id".to_string(), ctx.sender_normalized.clone());
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

        let label = if is_slash { "slash command for agent" } else { "agent" };
        info!(
            "Executing {} '{}' for session {} (run_id: {})",
            label,
            agent_id,
            ctx.session_key.to_key_string(),
            run_id
        );

        // Spawn the execution task (non-blocking)
        tokio::spawn(async move {
            if let Err(e) = execution_adapter.execute(request, agent, emitter).await {
                error!("Agent execution failed (run_id: {}): {}", run_id, e);
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
    ) -> Option<crate::gateway::interfaces::feishu::streaming::FeishuEventEmitter> {
        use crate::gateway::interfaces::feishu::FeishuConfig;
        use crate::gateway::interfaces::feishu::streaming::FeishuEventEmitter;
        use crate::gateway::interfaces::feishu::client::FeishuClient;

        // Read feishu config from app config
        let feishu_cfg = {
            let cfg = self.app_config.as_ref()?.read().await;
            let channel_id = ctx.reply_route.channel_id.as_str();
            let raw = cfg.channels.get(channel_id)?;
            serde_json::from_value::<FeishuConfig>(raw.clone()).ok()?
        };

        // Create a dedicated client for the emitter
        let client = Arc::new(FeishuClient::new(&feishu_cfg));
        if let Err(e) = client.refresh_token().await {
            tracing::warn!("Failed to create feishu emitter client: {e}");
            return None;
        }

        let inner = ReplyEmitter::with_config(
            self.channel_registry.clone(),
            ctx.reply_route.clone(),
            run_id.to_string(),
            reply_config,
        );

        let chat_id = ctx.message.conversation_id.as_str().to_string();
        let reply_to = ctx.reply_route.reply_to.as_ref().map(|id| id.as_str().to_string());

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
}
