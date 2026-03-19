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

        // Create a ReplyEmitter to route responses back to the channel,
        // respecting the configured output_mode (typewriter vs instant)
        let reply_config = match &self.app_config {
            Some(cfg) => {
                let cfg = cfg.read().await;
                let mode = cfg.behavior.as_ref()
                    .map(|b| b.output_mode.as_str())
                    .unwrap_or("typewriter");
                ReplyEmitterConfig::from_output_mode(mode)
            }
            None => ReplyEmitterConfig::default(),
        };
        let emitter = Arc::new(ReplyEmitter::with_config(
            self.channel_registry.clone(),
            ctx.reply_route.clone(),
            run_id.clone(),
            reply_config,
        ));

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

        let request = RunRequest {
            run_id: run_id.clone(),
            input: ctx.message.text.clone(),
            session_key: ctx.session_key.clone(),
            timeout_secs: None,
            metadata,
        };

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
}
