//! Agent execution for inbound messages
//!
//! Collapsed from the original two near-identical methods
//! (`execute_for_context` / `execute_for_context_with_metadata`)
//! into a single parameterized implementation.

use crate::sync_primitives::Arc;
use std::collections::HashMap;
use std::path::PathBuf;
use tracing::{error, info};
use uuid::Uuid;

use crate::gateway::execution_engine::RunRequest;
use crate::gateway::inbound_context::InboundContext;
use crate::gateway::reply_emitter::{ReplyEmitter, ReplyEmitterConfig};

use super::types::{ChannelConfig, RoutingError, SLASH_COMMAND_MODE_KEY};
use super::InboundMessageRouter;

/// Resolve the run identity a channel's inbound messages execute under:
/// the `caller_role` fed to the tool-dispatch config-tier gate, and the
/// workspace it is locked into (Layer-1 lock for Chat tier).
///
/// An **unconfigured** channel (`None` in the map) defaults to Chat
/// (`"guest"`) with no locked workspace. This default is the over-permission
/// fix — a missing config must never be treated as operator. The
/// `permission_wiring_tests` below pin exactly this.
/// Returns `(caller_role, locked_workspace, busy_input_mode_wire,
/// tool_permissions)`. The third element is the channel's busy-input policy
/// wire string (`"steer"` default / `"interrupt"` / `"queue"`), the fourth the
/// channel's tool permission override — both stamped into run metadata so the
/// execution engine dispatches without re-reading channel config.
fn channel_run_identity(
    configs: &HashMap<String, ChannelConfig>,
    channel_id: &str,
) -> (
    &'static str,
    Option<PathBuf>,
    &'static str,
    Option<crate::config::types::policies::ToolPermissionsConfig>,
) {
    let cfg = configs.get(channel_id).cloned().unwrap_or_default();
    (
        cfg.caller_role_str(),
        cfg.resolved_default_workspace(),
        cfg.busy_input_mode_wire(),
        cfg.tool_permissions,
    )
}

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
                    .map_or("typewriter", |b| b.output_mode.as_str());
                ReplyEmitterConfig::from_output_mode(mode)
            }
            None => ReplyEmitterConfig::default(),
        };
        reply_config.voice_enabled = voice_enabled;
        reply_config.voice_reply_hint = ctx.voice_reply_hint;
        // Pull the process-wide runtime-footer config installed at boot.
        // Defaults to disabled when never installed (tests, host-only paths).
        reply_config.footer = crate::gateway::runtime_footer::global_config();

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

        let pending_media: crate::gateway::media::PendingMedia =
            Arc::new(tokio::sync::Mutex::new(Vec::new()));

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
                // Capture the global output_mode switch before reply_config is
                // moved into the fallback ReplyEmitter below.
                let stream_enabled = reply_config.stream_enabled;
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
                    Some(te) => {
                        if stream_enabled {
                            Arc::new(te)
                        } else {
                            // output_mode = "instant": the orchestrated emitter
                            // streams independently of ReplyEmitter, so wrap it
                            // to buffer chunks into a single final message —
                            // keeping Telegram in sync with the global switch.
                            Arc::new(crate::gateway::event_emitter::InstantBufferingEmitter::new(
                                te,
                            ))
                        }
                    }
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
        // Raw (un-normalized) originating sender id — the approval "originator".
        // The channel button-approval callback compares the clicker's RAW id
        // against this, so unlike `sender_id` above it must NOT be normalized;
        // it lets a pending approval refuse resolution from anyone but the person
        // who triggered it (closes the group-chat approval bypass). Consumed via
        // `TURN_ORIGINATOR` → the channel approval bridge → the record.
        metadata.insert(
            "originator_user_id".to_string(),
            ctx.message.sender_id.as_str().to_string(),
        );
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
        // Stamp this channel's permission tier as the run's caller_role so the
        // tool-dispatch config gate (tools/scoped/dispatch.rs) applies uniformly
        // to external-channel messages. Unconfigured channels default to Chat
        // ("guest") — closing the prior over-permission where a missing role was
        // treated as operator. An operator opts a channel up to Config tier via
        // `permission_level = "config"` in its config block.
        let (caller_role, channel_workspace, busy_input_mode, channel_tool_permissions) =
            channel_run_identity(&self.channel_configs, ctx.message.channel_id.as_str());
        metadata.insert("caller_role".to_string(), caller_role.to_string());
        // Stamp the channel's busy-input policy so the execution engine's busy
        // branch knows whether a message arriving mid-run should steer (default)
        // or interrupt the in-flight run. Absent on Panel/CLI paths → Steer.
        metadata.insert(
            crate::gateway::execution_engine::BUSY_INPUT_MODE_KEY.to_string(),
            busy_input_mode.to_string(),
        );
        // Stamp the channel's tool permission override (JSON) so `run_loop`
        // merges it as the most specific layer over global + agent
        // permissions. Only stamped when the channel configures one —
        // unconfigured channels stay byte-identical to pre-wiring metadata.
        if let Some(perms) = channel_tool_permissions {
            match serde_json::to_string(&perms) {
                Ok(json) => {
                    metadata.insert(
                        crate::gateway::execution_engine::CHANNEL_TOOL_PERMISSIONS_KEY.to_string(),
                        json,
                    );
                }
                Err(e) => error!(
                    channel_id = %ctx.message.channel_id.as_str(),
                    error = %e,
                    "Failed to serialize channel tool_permissions — channel layer skipped"
                ),
            }
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
        // Record voice mode for this turn in the session-keyed registry the
        // harness bridge reads at prompt-assembly time. Request metadata never
        // reaches `build_system_prompt`, so the old `metadata["voice_mode_active"]`
        // stamp had no reader and `VoiceModeLayer` never fired — this is the
        // wire that makes voice mode actually change agent behavior. Set on both
        // edges so disabling voice clears stale state.
        //
        // A voice turn resolves two config facts here with a single read: the
        // domain-vocabulary hint — recorded in the registry so `VoiceModeLayer`
        // can invite term-accurate transcription repair (the same
        // `[voice] vocabulary` that biases ASR, one dictionary two consumers) —
        // and the low-TTFT model pin (`[voice] llm_provider/llm_model`) so the
        // spoken reply starts faster than the global default. Empty config →
        // `None` → the run uses the global default. Only voice turns are
        // affected; text mode is untouched.
        let (voice_state, model_override) = if voice_enabled {
            let (vocabulary, model) = match &self.app_config {
                Some(cfg) => {
                    let cfg = cfg.read().await;
                    (
                        cfg.voice_local.vocabulary_hint(),
                        crate::gateway::model_override::ModelOverride::from_voice(
                            &cfg.voice_local.llm_provider,
                            &cfg.voice_local.llm_model,
                        ),
                    )
                }
                None => (None, None),
            };
            (
                Some(crate::gateway::voice::voice_mode::VoiceTurnState::new(
                    ctx.transcribed_input,
                    vocabulary,
                )),
                model,
            )
        } else {
            (None, None)
        };
        crate::gateway::voice::voice_mode::set(&ctx.session_key.to_key_string(), voice_state);

        // Channel-routed messages run in the channel's configured workspace
        // (Layer-1 lock): a Chat-tier channel pins its `default_workspace`; a
        // Config-tier channel that sets none falls back to the agent default.
        // Project-mode override (free workdir choice) enters via the desktop
        // Panel's `chat.send` and is gated there on Config tier.
        let request = RunRequest {
            run_id: run_id.clone(),
            input: ctx.message.text.clone(),
            session_key: ctx.session_key.clone(),
            timeout_secs: None,
            metadata,
            attachments: ctx.message.attachments.clone(),
            pending_media: pending_media.clone(),
            sandbox_override: None,
            workspace_override: channel_workspace,
            max_iterations_override: None,
            model_override,
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

        // Busy lane is keyed by SESSION (matches the engine's per-session
        // `SessionRunRegistry` gate). Two sessions of the same agent get
        // independent lanes and run in parallel; only same-session messages
        // serialize FIFO.
        let session_key = request.session_key.to_key_string();
        let agent_id_for_busy = request.session_key.agent_id().to_string();
        let busy_cfg = self.busy_queue_config().await;

        // Take the FIFO ticket HERE — synchronously, on the arrival path —
        // rather than inside the spawned task. Registering after the spawn made
        // lane order depend on task scheduling, so two messages sent a
        // millisecond apart could enqueue inverted, defeating the arrival-order
        // guarantee the lane exists to provide.
        let ticket = crate::gateway::busy_queue::register(
            &session_key,
            busy_cfg.max_per_session,
            &request.run_id,
        );

        // Spawn the delivery task (non-blocking). While the session is busy the
        // waiter parks on the lane's wake signal — fired by the engine when the
        // session's run slot frees — instead of polling, and only the front
        // ticket attempts delivery, so bursts deliver in arrival order.
        let error_channel_registry = self.channel_registry.clone();
        let error_reply_route = ctx.reply_route.clone();
        let error_app_config = self.app_config.clone();
        tokio::spawn(async move {
            use crate::gateway::busy_queue::{deliver_with_ticket, DeliveryOutcome};

            let outcome = match ticket {
                // Lane full — reject newest immediately so the sender hears
                // back now, not after the whole wait window.
                None => DeliveryOutcome::Rejected,
                // The guard is RAII: dropped on every exit — including a panic
                // inside the adapter — so a dead waiter can never leave a
                // corpse ticket wedging the lane (mirrors the engine's
                // `RunSlot` session claim).
                Some(ticket) => {
                    let mut attempt = || {
                        execution_adapter.execute(request.clone(), agent.clone(), emitter.clone())
                    };
                    deliver_with_ticket(ticket, busy_cfg, &mut attempt).await
                }
            };

            // An attempt that actually ran already reported its own failure to
            // this channel through the run's `ReplyEmitter` (`RunError` →
            // `send_error`). Reporting again here sent the user TWO messages
            // for one failure, so only queue-stage outcomes are ours to voice.
            if let DeliveryOutcome::Executed(Err(ref e)) = outcome {
                error!("Agent execution failed (run_id: {}): {}", run_id, e);
            }
            let Some(e) = outcome.user_error(&agent_id_for_busy) else {
                return;
            };
            error!(
                "Message never reached the agent (run_id: {}): {}",
                run_id, e
            );

            // Resolve user locale from config
            let locale = if let Some(ref cfg) = error_app_config {
                let cfg = cfg.read().await;
                crate::gateway::i18n::Locale::from_config(cfg.general.language.as_deref())
            } else {
                crate::gateway::i18n::Locale::Zh
            };

            // Routes through the SAME typed receipt the Panel/engine paths use
            // (`ExecutionError::user_receipt`) — this site used to stringify the
            // typed error and re-classify it with a second, subtly different
            // keyword table, which also echoed up to 200 chars of the raw
            // internal chain back to the channel.
            let (_code, user_msg) = e.user_receipt(locale);
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
        });

        Ok(())
    }

    /// Try to create a `FeishuEventEmitter` for feishu channels.
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

    /// Try to create a `TelegramEventEmitter` for telegram channels.
    async fn try_create_telegram_emitter(
        &self,
        ctx: &InboundContext,
        run_id: &str,
        reply_config: ReplyEmitterConfig,
        pending_media: crate::gateway::media::PendingMedia,
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

        // The orchestrated emitter takes over the wire for text but has no
        // media leg of its own — hand it a `ReplyEmitter` bound to the same
        // `pending_media` buffer the run fills, so `_media` still reaches the
        // chat under this config. Text never routes through it.
        let media = ReplyEmitter::with_config(
            self.channel_registry.clone(),
            ctx.reply_route.clone(),
            run_id.to_string(),
            reply_config,
            pending_media,
        );

        Some(TelegramEventEmitter::new(
            bot,
            streaming,
            conversation_id,
            ctx.reply_route.clone(),
            media,
        ))
    }
}

#[cfg(test)]
mod permission_wiring_tests {
    use super::channel_run_identity;
    use crate::gateway::inbound_router::types::{ChannelConfig, ChannelPermissionLevel};
    use std::collections::HashMap;
    use std::path::PathBuf;

    /// THE security line: a channel with no registered config runs as the gated
    /// "guest" tier with no locked workspace. A regression here silently reopens
    /// the external-channel over-permission hole.
    #[test]
    fn unconfigured_channel_defaults_to_guest_with_no_workspace() {
        let empty: HashMap<String, ChannelConfig> = HashMap::new();
        // Unconfigured → guest, no workspace, the safe `steer` busy default,
        // and no channel tool-permission layer.
        assert_eq!(
            channel_run_identity(&empty, "telegram"),
            ("guest", None, "steer", None)
        );

        // Unknown id in a populated map → still the safe default.
        let mut configs = HashMap::new();
        configs.insert("slack".to_string(), ChannelConfig::default());
        assert_eq!(
            channel_run_identity(&configs, "telegram"),
            ("guest", None, "steer", None)
        );
    }

    /// A Chat-tier channel with an absolute, existing default_workspace is
    /// stamped "guest" and pinned to that workspace (Layer-1 lock).
    #[test]
    fn chat_tier_channel_locks_to_default_workspace() {
        let ws = std::env::temp_dir(); // absolute + existing
        let mut configs = HashMap::new();
        configs.insert(
            "telegram".to_string(),
            ChannelConfig {
                permission_level: ChannelPermissionLevel::Chat,
                default_workspace: Some(ws.clone()),
                ..Default::default()
            },
        );
        assert_eq!(
            channel_run_identity(&configs, "telegram"),
            ("guest", Some(ws), "steer", None)
        );
    }

    /// A channel that opts into interrupt mode is stamped `"interrupt"` (the
    /// metadata the execution engine's busy branch dispatches on), independent of
    /// its permission tier.
    #[test]
    fn interrupt_busy_mode_is_stamped() {
        use crate::gateway::execution_engine::BusyInputMode;
        let mut configs = HashMap::new();
        configs.insert(
            "ops-bot".to_string(),
            ChannelConfig {
                busy_input_mode: BusyInputMode::Interrupt,
                ..Default::default()
            },
        );
        let (_role, _ws, busy, _perms) = channel_run_identity(&configs, "ops-bot");
        assert_eq!(busy, "interrupt");
    }

    /// A channel carrying a tool-permission override surfaces it for the
    /// metadata stamp; unconfigured channels surface `None` (no stamp, no
    /// behavior change).
    #[test]
    fn channel_tool_permissions_surface_through_identity() {
        use crate::config::types::policies::ToolPermissionsConfig;
        use crate::extension::PermissionAction;

        let perms = ToolPermissionsConfig {
            default: PermissionAction::Allow,
            overrides: [("bash".to_string(), PermissionAction::Deny)]
                .into_iter()
                .collect(),
        };
        let mut configs = HashMap::new();
        configs.insert(
            "group-bot".to_string(),
            ChannelConfig {
                tool_permissions: Some(perms),
                ..Default::default()
            },
        );
        let (_role, _ws, _busy, channel_perms) = channel_run_identity(&configs, "group-bot");
        let channel_perms = channel_perms.expect("configured override must surface");
        assert_eq!(channel_perms.resolve("bash"), PermissionAction::Deny);
        assert_eq!(channel_perms.resolve("read_file"), PermissionAction::Allow);
    }

    /// A Config-tier channel is stamped "operator" (Layer-2). With no
    /// default_workspace set it carries no lock (the agent default applies).
    #[test]
    fn config_tier_channel_maps_to_operator() {
        let mut configs = HashMap::new();
        configs.insert(
            "ops-bot".to_string(),
            ChannelConfig {
                permission_level: ChannelPermissionLevel::Config,
                ..Default::default()
            },
        );
        let (role, workspace, busy, _perms) = channel_run_identity(&configs, "ops-bot");
        assert_eq!(role, "operator");
        assert_eq!(workspace, None::<PathBuf>);
        // Config tier inherits the safe steer default unless explicitly opted in.
        assert_eq!(busy, "steer");
    }
}
