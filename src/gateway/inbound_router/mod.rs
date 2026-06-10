//! Inbound Message Router
//!
//! Consumes the ChannelRegistry's inbound message stream and routes
//! messages to the appropriate Agent/Session.

mod agent_resolver;
pub mod approval_callback;
mod command_handler;
mod dedup;
mod executor;
mod group_chat_handler;
mod permission;

mod types;

pub use command_handler::serialize_parsed_command;
pub use types::{
    ChannelConfig, ChannelPermissionLevel, ChannelPolicyConfig, DmPolicy, GroupPolicy,
    RoutingError, SlashAccessConfig, SLASH_COMMAND_MODE_KEY,
};

use crate::sync_primitives::Arc;
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::{broadcast, Mutex};
use tracing::{error, info, warn};

use super::agent_instance::AgentRegistry;
use super::channel::{InboundMessage, OutboundMessage};
use super::channel_registry::ChannelRegistry;
use super::execution_adapter::ExecutionAdapter;
use super::handlers::group_chat::SharedOrchestrator;

use super::agent_env::AgentEnvStore;
use super::inbound_context::InboundContext;
use super::pair_loop_guard::{PairLoopFacts, PairLoopGuard, PairLoopGuardConfig};
use super::pairing_store::PairingStore;
use super::routing_config::RoutingConfig;
use crate::command::CommandParser;
use crate::group_chat::GroupChatExecutor;
use crate::routing::config::{RouteBinding, SessionConfig};

use command_handler::strip_bot_mention;
use dedup::InboundDedupTracker;
use types::check_link_access;

// Fallback for non-macOS platforms
#[cfg(not(target_os = "macos"))]
fn normalize_phone(phone: &str) -> String {
    let mut result = String::new();
    let mut chars = phone.chars().peekable();

    if chars.peek() == Some(&'+') {
        result.push('+');
        chars.next();
    }

    for c in chars {
        if c.is_ascii_digit() {
            result.push(c);
        }
    }

    // Add country code if missing (assume US)
    if !result.starts_with('+') && result.len() == 10 {
        result = format!("+1{}", result);
    } else if !result.starts_with('+') && result.len() == 11 && result.starts_with('1') {
        result = format!("+{}", result);
    }

    result
}

/// Inbound message router
pub struct InboundMessageRouter {
    pub(super) channel_registry: Arc<ChannelRegistry>,
    pub(super) pairing_store: Arc<dyn PairingStore>,
    pub(super) config: RoutingConfig,
    /// Channel-specific configs (keyed by channel_id)
    pub(super) channel_configs: HashMap<String, ChannelConfig>,
    /// Agent registry for looking up agent instances
    pub(super) agent_registry: Option<Arc<AgentRegistry>>,
    /// Execution adapter for running agents
    pub(super) execution_adapter: Option<Arc<dyn ExecutionAdapter>>,
    /// Workspace manager for channel-level active agent lookup
    pub(super) workspace_manager: Option<Arc<AgentEnvStore>>,
    /// Route bindings for multi-tier agent resolution (peer → guild → channel → default)
    pub(super) route_bindings: Vec<RouteBinding>,
    /// Session configuration (dm_scope, identity_links)
    pub(super) route_session_config: SessionConfig,
    /// Default agent ID when no binding matches
    pub(super) default_agent_id: String,
    /// Inbound message deduplication tracker
    dedup_tracker: Mutex<InboundDedupTracker>,
    /// Bot↔bot pair-loop guard (kicks in when `is_bot_authored()` is true).
    pair_loop_guard: Arc<PairLoopGuard>,
    /// Effective pair-loop-guard config (channel-level overrides may layer on later).
    pair_loop_config: PairLoopGuardConfig,
    /// Group chat orchestrator
    pub(super) group_chat_orch: Option<SharedOrchestrator>,
    /// Group chat executor
    pub(super) group_chat_executor: Option<Arc<GroupChatExecutor>>,
    /// Active group chat sessions: "channel_id:conversation_id" -> session_id
    pub(super) active_group_sessions: Mutex<HashMap<String, String>>,

    /// LLM provider for intent classification and soul generation
    pub(super) llm_provider: Option<Arc<dyn crate::providers::AiProvider>>,
    /// Command parser for unified slash command resolution
    pub(super) command_parser: Option<Arc<CommandParser>>,
    /// Session manager for session lifecycle
    pub(super) session_store: Option<Arc<dyn super::session_store::SessionStore>>,
    /// App config for reading output_mode at runtime
    pub(super) app_config: Option<Arc<tokio::sync::RwLock<crate::Config>>>,
    /// STT config for voice transcription
    pub(super) stt_config: Option<super::voice::inbound::SttConfig>,
    /// Generation provider registry for TTS
    pub(super) generation_registry: Option<Arc<crate::generation::GenerationProviderRegistry>>,
    /// Generation config for TTS
    pub(super) generation_config:
        Option<Arc<tokio::sync::RwLock<crate::config::types::generation::GenerationConfig>>>,
    /// Message coalescer for merging rapid-fire inbound messages
    coalescer: Option<super::coalescer::MessageCoalescer>,
    /// 审批按钮回调汇 —— 注入则拦截 `cb_` 回调，否则按普通消息处理。
    /// Used for in-channel button-callback approval flow.
    pub(super) approval_callback_sink: Option<Arc<dyn approval_callback::ApprovalCallbackSink>>,
    /// Exec-approval manager — resolves `/approve` and `/deny` channel replies
    /// against pending tool/sandbox approvals (HITL P2 text-fallback path,
    /// complementing the button-callback `approval_callback_sink`).
    pub(super) exec_approval_manager: Option<Arc<crate::exec::manager::ExecApprovalManager>>,
    /// Clarification manager — routes a reply back to a pending `ask_user`
    /// question instead of starting a new agent turn (HITL P4).
    pub(super) clarification_manager: Option<Arc<crate::clarification::ClarificationManager>>,
    /// Coord-task store — resolves a reply to a paused workflow `clarify` step.
    /// Unlike `ask_user` (in-memory), a clarify step's awaiting record is the
    /// durable `coord_task` row, so resolution is a stateless lookup that
    /// survives a restart. `None` → workflow clarify resolution disabled.
    pub(super) workflow_clarify_store: Option<Arc<dyn crate::agents::swarm::tasks::CoordTaskStore>>,
    /// Wakes the team dispatcher after a clarify reply completes its task, so the
    /// now-unblocked dependents are picked up without waiting for the next tick.
    pub(super) dispatch_signal: Option<Arc<tokio::sync::Notify>>,
}

impl InboundMessageRouter {
    /// Create a new inbound message router (basic, without execution support)
    pub fn new(
        channel_registry: Arc<ChannelRegistry>,
        pairing_store: Arc<dyn PairingStore>,
        config: RoutingConfig,
    ) -> Self {
        Self {
            channel_registry,
            pairing_store,
            config,
            channel_configs: HashMap::new(),
            agent_registry: None,
            execution_adapter: None,
            workspace_manager: None,
            route_bindings: Vec::new(),
            route_session_config: SessionConfig::default(),
            default_agent_id: "main".to_string(),
            dedup_tracker: Mutex::new(InboundDedupTracker::new()),
            pair_loop_guard: Arc::new(PairLoopGuard::new()),
            pair_loop_config: PairLoopGuardConfig::default(),
            group_chat_orch: None,
            group_chat_executor: None,
            active_group_sessions: Mutex::new(HashMap::new()),
            llm_provider: None,
            command_parser: None,
            session_store: None,
            app_config: None,
            stt_config: None,
            generation_registry: None,
            generation_config: None,
            coalescer: None,
            approval_callback_sink: None,
            exec_approval_manager: None,
            clarification_manager: None,
            workflow_clarify_store: None,
            dispatch_signal: None,
        }
    }

    /// Create a new inbound message router with full execution support
    pub fn with_execution(
        channel_registry: Arc<ChannelRegistry>,
        pairing_store: Arc<dyn PairingStore>,
        config: RoutingConfig,
        agent_registry: Arc<AgentRegistry>,
        execution_adapter: Arc<dyn ExecutionAdapter>,
    ) -> Self {
        let mut router = Self::new(channel_registry, pairing_store, config);
        router.agent_registry = Some(agent_registry);
        router.execution_adapter = Some(execution_adapter);
        router
    }

    /// Set the workspace manager for channel-level active agent lookup
    pub fn with_workspace_manager(mut self, manager: Arc<AgentEnvStore>) -> Self {
        self.workspace_manager = Some(manager);
        self
    }

    /// Set route bindings for multi-tier agent resolution
    pub fn with_route_bindings(
        mut self,
        bindings: Vec<RouteBinding>,
        session_config: SessionConfig,
        default_agent: impl Into<String>,
    ) -> Self {
        self.route_bindings = bindings;
        self.route_session_config = session_config;
        self.default_agent_id = default_agent.into();
        self
    }

    /// Enable group chat support
    pub fn with_group_chat(
        mut self,
        orch: SharedOrchestrator,
        executor: Arc<GroupChatExecutor>,
    ) -> Self {
        self.group_chat_orch = Some(orch);
        self.group_chat_executor = Some(executor);
        self
    }

    /// Set the LLM provider for intent classification and soul generation
    pub fn with_llm_provider(mut self, provider: Arc<dyn crate::providers::AiProvider>) -> Self {
        self.llm_provider = Some(provider);
        self
    }

    /// Wire the HITL managers so `/approve` `/deny` replies and replies to a
    /// pending `ask_user` are intercepted before starting a new agent turn.
    pub fn with_hitl(
        mut self,
        exec_approval_manager: Arc<crate::exec::manager::ExecApprovalManager>,
        clarification_manager: Arc<crate::clarification::ClarificationManager>,
    ) -> Self {
        self.exec_approval_manager = Some(exec_approval_manager);
        self.clarification_manager = Some(clarification_manager);
        self
    }

    /// Wire workflow `clarify` resolution: a reply to a paused clarify step is
    /// completed against its durable `coord_task` (not the in-memory
    /// `ask_user` registry), then the dispatcher is signalled to unblock the
    /// step's dependents.
    pub fn with_workflow_clarify(
        mut self,
        coord_store: Arc<dyn crate::agents::swarm::tasks::CoordTaskStore>,
        dispatch_signal: Arc<tokio::sync::Notify>,
    ) -> Self {
        self.workflow_clarify_store = Some(coord_store);
        self.dispatch_signal = Some(dispatch_signal);
        self
    }

    /// Set the command parser for dynamic slash command resolution
    pub fn with_command_parser(mut self, parser: Arc<CommandParser>) -> Self {
        self.command_parser = Some(parser);
        self
    }

    /// Set the session store for session lifecycle operations
    pub fn with_session_store(mut self, sm: Arc<dyn super::session_store::SessionStore>) -> Self {
        self.session_store = Some(sm);
        self
    }

    /// Set the app config for reading output_mode at runtime
    pub fn with_app_config(mut self, config: Arc<tokio::sync::RwLock<crate::Config>>) -> Self {
        self.app_config = Some(config);
        self
    }

    /// Set STT config for voice transcription
    pub fn with_stt_config(mut self, config: super::voice::inbound::SttConfig) -> Self {
        self.stt_config = Some(config);
        self
    }

    /// Set voice output dependencies (generation provider registry + config)
    pub fn with_voice_output(
        mut self,
        registry: Arc<crate::generation::GenerationProviderRegistry>,
        config: Arc<tokio::sync::RwLock<crate::config::types::generation::GenerationConfig>>,
    ) -> Self {
        self.generation_registry = Some(registry);
        self.generation_config = Some(config);
        self
    }

    /// Set the message coalescer for merging rapid-fire inbound messages.
    ///
    /// When enabled, messages from the same conversation (or Telegram media group)
    /// are buffered and merged before being handed off to the agent loop.
    pub fn with_coalescer(mut self, config: super::coalescer::CoalescingConfig) -> Self {
        self.coalescer = Some(super::coalescer::MessageCoalescer::new(config));
        self
    }

    /// Tune the bot↔bot pair-loop guard. Defaults (20 events / 60s window /
    /// 60s cooldown, enabled) are applied when this builder is not called.
    pub fn with_bot_loop_protection(mut self, config: PairLoopGuardConfig) -> Self {
        self.pair_loop_config = config;
        self
    }

    /// 注入审批回调汇，启用通道按钮 approve/deny 分发。
    pub fn with_approval_callback_sink(
        mut self,
        sink: Arc<dyn approval_callback::ApprovalCallbackSink>,
    ) -> Self {
        self.approval_callback_sink = Some(sink);
        self
    }

    /// Register channel-specific configuration
    pub fn register_channel_config(&mut self, channel_id: &str, config: ChannelConfig) {
        self.channel_configs.insert(channel_id.to_string(), config);
    }

    /// Start consuming inbound messages
    pub async fn start(self: Arc<Self>) -> Option<tokio::task::JoinHandle<()>> {
        let rx = self.channel_registry.take_inbound_receiver()?;

        let handle = tokio::spawn(async move {
            self.run_loop(rx).await;
        });

        Some(handle)
    }

    /// Main message processing loop
    async fn run_loop(self: Arc<Self>, mut rx: broadcast::Receiver<InboundMessage>) {
        info!("InboundMessageRouter started");

        let has_coalescer = self.coalescer.is_some();

        if has_coalescer {
            // Coalescing path: use tokio::select! to interleave recv + tick
            let mut tick_interval = tokio::time::interval(Duration::from_millis(50));
            // The first tick completes immediately; consume it.
            tick_interval.tick().await;

            loop {
                tokio::select! {
                    msg = rx.recv() => {
                        match msg {
                            Ok(msg) => {
                                // Deduplication check
                                if !self.dedup_check(&msg).await {
                                    continue;
                                }
                                // Bot↔bot pair-loop guard (no-op for human senders)
                                if !self.pair_loop_check(&msg) {
                                    continue;
                                }

                                if let Some(ref coalescer) = self.coalescer {
                                    let immediate = coalescer.push(msg);
                                    for flushed in immediate {
                                        let router = self.clone();
                                        tokio::spawn(async move {
                                            if let Err(e) = router.handle_message(flushed).await {
                                                error!("Failed to handle coalesced message: {}", e);
                                            }
                                        });
                                    }
                                }
                            }
                            Err(_) => break, // channel closed
                        }
                    }
                    _ = tick_interval.tick() => {
                        if let Some(ref coalescer) = self.coalescer {
                            let flushed = coalescer.tick();
                            for msg in flushed {
                                let router = self.clone();
                                tokio::spawn(async move {
                                    if let Err(e) = router.handle_message(msg).await {
                                        error!("Failed to handle coalesced message: {}", e);
                                    }
                                });
                            }
                        }
                    }
                }
            }

            // Graceful shutdown: flush remaining coalescer buffers
            if let Some(ref coalescer) = self.coalescer {
                let remaining = coalescer.flush_all();
                if !remaining.is_empty() {
                    info!(
                        pending_buffers = remaining.len(),
                        "Flushing coalescing buffers on shutdown"
                    );
                    for msg in remaining {
                        let router = self.clone();
                        if let Err(e) = router.handle_message(msg).await {
                            error!("Failed to handle flushed message on shutdown: {}", e);
                        }
                    }
                }
            }
        } else {
            // No coalescer — original direct-dispatch path (backward compatible)
            while let Ok(msg) = rx.recv().await {
                if !self.dedup_check(&msg).await {
                    continue;
                }
                if !self.pair_loop_check(&msg) {
                    continue;
                }

                let router = self.clone();
                tokio::spawn(async move {
                    if let Err(e) = router.handle_message(msg).await {
                        error!("Failed to handle inbound message: {}", e);
                    }
                });
            }
        }

        info!("InboundMessageRouter stopped");
    }

    /// Check pair-loop guard for bot-authored inbound messages.
    /// Returns `true` when the message is allowed through, `false` when the
    /// pair has been suppressed (drop silently to avoid feeding the loop).
    /// Non-bot messages bypass the guard entirely.
    fn pair_loop_check(&self, msg: &InboundMessage) -> bool {
        if !msg.is_bot_authored() {
            return true;
        }
        let facts = PairLoopFacts {
            scope_id: msg.channel_id.as_str(),
            conversation_id: msg.conversation_id.as_str(),
            sender_id: msg.sender_id.as_str(),
            receiver_id: msg.channel_id.as_str(),
        };
        let result = self
            .pair_loop_guard
            .record_and_check(&facts, &self.pair_loop_config);
        if result.suppressed {
            warn!(
                channel_id = %msg.channel_id.as_str(),
                conversation_id = %msg.conversation_id.as_str(),
                sender_id = %msg.sender_id.as_str(),
                retry_after_secs = result.retry_after.map(|d| d.as_secs()).unwrap_or(0),
                reason = %result.reason.as_deref().unwrap_or(""),
                "pair-loop guard suppressed bot-authored inbound message"
            );
            return false;
        }
        true
    }

    /// Check dedup tracker, returns `true` if message is new (not a duplicate).
    async fn dedup_check(&self, msg: &InboundMessage) -> bool {
        let dedup_key = format!("{}:{}", msg.channel_id.as_str(), msg.id.as_str());
        let mut tracker = self.dedup_tracker.lock().await;
        if !tracker.check_and_record(&dedup_key) {
            warn!(
                "Duplicate message detected and dropped: {} from {}:{}",
                dedup_key,
                msg.channel_id.as_str(),
                msg.sender_id.as_str()
            );
            return false;
        }
        true
    }

    /// Handle a single inbound message
    pub async fn handle_message(&self, msg: InboundMessage) -> Result<(), RoutingError> {
        let channel_id = msg.channel_id.as_str();
        info!(
            "[Router] Handling message from {}:{} - {}",
            channel_id,
            msg.sender_id.as_str(),
            msg.text.chars().take(50).collect::<String>()
        );

        // 审批按钮回调短路：在正常路由之前拦截。
        // callback query 入站消息 id 以 "cb_" 前缀（webhook / 轮询两路一致）。
        if msg.id.as_str().starts_with("cb_") {
            if let Some(ref sink) = self.approval_callback_sink {
                if let Some(result) = sink
                    .handle_callback(&msg.text, msg.sender_id.as_str())
                    .await
                {
                    let reply =
                        OutboundMessage::text(msg.conversation_id.as_str(), result.response_text);
                    let _ = self.channel_registry.send(&msg.channel_id, reply).await;
                    return Ok(());
                }
                // sink 返回 None → 非审批回调 → 落入正常消息流
            }
        }

        // Resolve agent ID via multi-tier route bindings with fallback
        let (agent_id, resolved_route) = self
            .resolve_agent_id_async(&msg)
            .await
            .unwrap_or_else(|| (self.default_agent_id.clone(), None));

        // Check link access control
        if let Some(ref registry) = self.agent_registry {
            if let Some(allowed_links) = registry.get_allowed_links(&agent_id).await {
                if let Err(e) = check_link_access(&allowed_links, channel_id, &agent_id) {
                    warn!("[Router] {}", e);
                    let reply = OutboundMessage::text(
                        msg.conversation_id.as_str(),
                        format!("\u{26d4} {}", e),
                    );
                    let _ = self.channel_registry.send(&msg.channel_id, reply).await;
                    return Ok(());
                }
            }
        }

        // Build context with resolved agent
        let ctx = self
            .build_context_with_agent(&msg, &agent_id, resolved_route.as_ref())
            .await;

        // Check permissions
        let mut ctx = match self.check_permission(ctx).await {
            Ok(ctx) => {
                info!(
                    "[Router] Permission granted for {}:{}",
                    channel_id, ctx.sender_normalized
                );
                ctx
            }
            Err(e) => {
                info!(
                    "[Router] Permission denied for {}:{} — {}",
                    channel_id,
                    msg.sender_id.as_str(),
                    e
                );
                return Ok(());
            }
        };

        // Extension hooks observe inbound channel traffic (post-permission).
        // MESSAGE_PREVIEW is capped at 256 chars to keep message content out of
        // an unbounded env var.
        crate::extension::hooks::fire_global_observer(
            crate::extension::HookEvent::MessageReceived,
            &ctx.session_key.to_key_string(),
            vec![
                ("CHANNEL_ID", channel_id.to_string()),
                ("SENDER_ID", msg.sender_id.as_str().to_string()),
                ("MESSAGE_CHARS", msg.text.chars().count().to_string()),
                (
                    "MESSAGE_PREVIEW",
                    msg.text.chars().take(256).collect::<String>(),
                ),
            ],
        )
        .await;

        // Voice STT: transcribe audio attachments before further processing
        let has_stt = self.stt_config.is_some();
        let has_audio = super::voice::inbound::has_audio_attachment(&ctx.message);
        tracing::debug!(
            has_stt = has_stt,
            has_audio = has_audio,
            attachments = ctx.message.attachments.len(),
            "Voice STT check"
        );
        if let Some(ref stt_config) = self.stt_config {
            if has_audio {
                let result =
                    super::voice::inbound::process_inbound_voice(ctx.message.clone(), stt_config)
                        .await;
                ctx.message = result.message;
                if result.transcribed {
                    ctx.voice_reply_hint = true;
                }
            }
        }

        // HITL interception: a reply to a pending approval or `ask_user`
        // clarification is resolved here instead of starting a new agent turn.
        if self.try_intercept_hitl(&ctx).await {
            return Ok(());
        }

        // Built-in /voice command — direct handler, no LLM needed
        {
            let trimmed = ctx.message.text.trim();
            let lower = trimmed.to_lowercase();
            let stripped = strip_bot_mention(&lower);
            if stripped.starts_with("/voice") {
                let args = stripped.strip_prefix("/voice").unwrap_or("").trim();
                // Strip leading underscore (Telegram may send /voice_on from button callback)
                let args = args.trim_start_matches('_');
                let (enabled, reply) = match args {
                    "on" | "open" | "开" | "开启" => (
                        true,
                        "Voice mode enabled. I'll reply with voice from now on.",
                    ),
                    "off" | "close" | "关" | "关闭" => (
                        false,
                        "Voice mode disabled. Switching back to text replies.",
                    ),
                    "" => {
                        // No args: show inline keyboard with voice sub-commands
                        use super::channel::{InlineButton, InlineKeyboard};
                        let state = self.channel_registry.get_voice_state("default").await;
                        let ch_state = self
                            .channel_registry
                            .get_voice_state(ctx.reply_route.channel_id.as_str())
                            .await;
                        let is_on = state.is_active() || ch_state.is_active();
                        let status = if is_on { "🔊 ON" } else { "🔇 OFF" };

                        let mut keyboard = InlineKeyboard::new();
                        keyboard.rows.push(vec![
                            InlineButton {
                                text: "🔊 On".to_string(),
                                callback_data: "/voice on".to_string(),
                            },
                            InlineButton {
                                text: "🔇 Off".to_string(),
                                callback_data: "/voice off".to_string(),
                            },
                            InlineButton {
                                text: "📊 Status".to_string(),
                                callback_data: "/voice status".to_string(),
                            },
                        ]);

                        let reply_msg = super::channel::OutboundMessage {
                            conversation_id: ctx.reply_route.conversation_id.clone(),
                            text: format!("🎙 Voice Mode: {}", status),
                            attachments: vec![],
                            reply_to: ctx.reply_route.reply_to.clone(),
                            inline_keyboard: Some(keyboard),
                            metadata: Default::default(),
                        };
                        let _ = self
                            .channel_registry
                            .send(&ctx.reply_route.channel_id, reply_msg)
                            .await;
                        return Ok(());
                    }
                    "status" | "状态" => {
                        // Show current status as text
                        let state = self.channel_registry.get_voice_state("default").await;
                        let ch_state = self
                            .channel_registry
                            .get_voice_state(ctx.reply_route.channel_id.as_str())
                            .await;
                        let is_on = state.is_active() || ch_state.is_active();
                        let provider = state
                            .provider
                            .as_deref()
                            .or(ch_state.provider.as_deref())
                            .unwrap_or("auto");
                        let voice = state
                            .voice
                            .as_deref()
                            .or(ch_state.voice.as_deref())
                            .unwrap_or("default");
                        let status_text = format!(
                            "🎙 Voice Mode: {}\nProvider: {}\nVoice: {}\nFailures: {}",
                            if is_on { "ON" } else { "OFF" },
                            provider,
                            voice,
                            ch_state.consecutive_failures
                        );
                        let reply_msg = super::channel::OutboundMessage {
                            conversation_id: ctx.reply_route.conversation_id.clone(),
                            text: status_text,
                            attachments: vec![],
                            reply_to: ctx.reply_route.reply_to.clone(),
                            inline_keyboard: None,
                            metadata: Default::default(),
                        };
                        let _ = self
                            .channel_registry
                            .send(&ctx.reply_route.channel_id, reply_msg)
                            .await;
                        return Ok(());
                    }
                    _ => (true, "Voice mode enabled."),
                };
                self.channel_registry
                    .update_voice_state("default", |s| {
                        s.enabled = enabled;
                        s.consecutive_failures = 0;
                    })
                    .await;
                // Also set for the specific channel
                self.channel_registry
                    .update_voice_state(ctx.reply_route.channel_id.as_str(), |s| {
                        s.enabled = enabled;
                        s.consecutive_failures = 0;
                    })
                    .await;
                // Send confirmation — as voice if enabling, as text if disabling
                if enabled {
                    // Try voice reply for confirmation
                    if let (Some(ref gen_reg), Some(ref gen_cfg)) =
                        (&self.generation_registry, &self.generation_config)
                    {
                        let voice_state = self.channel_registry.get_voice_state("default").await;
                        let cfg = gen_cfg.read().await.clone();
                        if let Some(attachment) =
                            super::voice::outbound::generate_tts(reply, &voice_state, gen_reg, &cfg)
                                .await
                        {
                            let voice_msg = super::channel::OutboundMessage {
                                conversation_id: ctx.reply_route.conversation_id.clone(),
                                text: String::new(),
                                attachments: vec![attachment],
                                reply_to: ctx.reply_route.reply_to.clone(),
                                inline_keyboard: None,
                                metadata: Default::default(),
                            };
                            let _ = self
                                .channel_registry
                                .send(&ctx.reply_route.channel_id, voice_msg)
                                .await;
                            return Ok(());
                        }
                    }
                }
                // Fallback: text reply
                let reply_msg = super::channel::OutboundMessage {
                    conversation_id: ctx.reply_route.conversation_id.clone(),
                    text: reply.to_string(),
                    attachments: vec![],
                    reply_to: ctx.reply_route.reply_to.clone(),
                    inline_keyboard: None,
                    metadata: Default::default(),
                };
                let _ = self
                    .channel_registry
                    .send(&ctx.reply_route.channel_id, reply_msg)
                    .await;
                return Ok(());
            }
        }

        // /btw sidebar: ephemeral question without affecting context
        // Strip @botname first to handle Telegram group format: /btw@BotName text
        let btw_stripped = strip_bot_mention(ctx.message.text.trim());
        if btw_stripped.starts_with("/btw ") || btw_stripped.starts_with("/btw\n") {
            let btw_text = btw_stripped.strip_prefix("/btw").unwrap_or("").trim();
            if !btw_text.is_empty() {
                return self.handle_btw(&msg, &agent_id, btw_text).await;
            }
        }

        // Unified slash command interception
        if ctx.message.text.trim().starts_with('/') {
            let slash_text = strip_bot_mention(ctx.message.text.trim());
            let slash_text = slash_text.as_str();

            // Try unified command resolution first (async, all sources)
            if let Some(ref parser) = self.command_parser {
                if let Some(parsed) = parser.parse_async(slash_text).await {
                    // Slash-command access tiering — backward-compat: empty
                    // `allow_admin_from` keeps gating OFF for that scope.
                    // Must run BEFORE the special-cased dispatches below:
                    // /groupchat and /new used to early-return ahead of this
                    // check, so a configured gate silently never applied to
                    // them.
                    if let Some(cfg) = self.channel_configs.get(ctx.message.channel_id.as_str()) {
                        let decision = cfg.slash_command_gate(
                            &ctx.sender_normalized,
                            &parsed.command_name,
                            ctx.message.is_group,
                        );
                        if decision.gating_enabled && !decision.allowed {
                            warn!(
                                channel = %ctx.message.channel_id.as_str(),
                                sender = %ctx.sender_normalized,
                                command = %parsed.command_name,
                                is_group = ctx.message.is_group,
                                "[Router] Slash command blocked by access policy"
                            );
                            let reply = OutboundMessage {
                                conversation_id: msg.conversation_id.clone(),
                                text: format!(
                                    "/{} is not enabled for your account in this chat.",
                                    parsed.command_name
                                ),
                                attachments: Vec::new(),
                                reply_to: Some(msg.id.clone()),
                                inline_keyboard: None,
                                metadata: Default::default(),
                            };
                            let _ = self.channel_registry.send(&msg.channel_id, reply).await;
                            return Ok(());
                        }
                    }
                    if parsed.command_name == "groupchat" {
                        return self.handle_groupchat_command(&msg).await;
                    }
                    if parsed.command_name == "session_new" || parsed.command_name == "new" {
                        return self.handle_new_session(&msg, &ctx).await;
                    }
                    // Use the rich serializer so Skill / Custom / MCP fields
                    // (instructions, allowed_tools, system_prompt, server_name)
                    // are preserved in the mode JSON instead of being lossily
                    // collapsed to `direct_tool`.
                    if let Some(mode_json) =
                        crate::gateway::inbound_router::command_handler::serialize_parsed_command(
                            &parsed,
                        )
                    {
                        info!(
                            "[Router] Slash command resolved: source=unified, name={}",
                            ctx.message.text.split_whitespace().next().unwrap_or("")
                        );
                        self.execute_for_context_with_metadata(&ctx, mode_json)
                            .await?;
                        return Ok(());
                    }
                } else {
                    // Command not resolved — check if it's a namespace-only command.
                    // e.g. "/session" when tools like "session_new", "session_list" exist.
                    let namespace = slash_text
                        .trim_start_matches('/')
                        .split_whitespace()
                        .next()
                        .unwrap_or("")
                        .split('@') // Strip @botname suffix
                        .next()
                        .unwrap_or("");
                    // Skip namespace check for shorthand aliases — these are resolved
                    // by slash_command.rs fast-path (e.g. /image → image_generate).
                    // Shares slash_command::SHORTHAND_ALIASES instead of keeping
                    // a second hardcoded copy of the alias names here.
                    let is_shorthand =
                        crate::gateway::execution_engine::is_shorthand_alias(namespace);
                    if !namespace.is_empty() && !is_shorthand {
                        let registry = parser.tool_registry();
                        if registry.is_namespace(namespace).await {
                            let children = registry.list_namespace_children(namespace).await;
                            if !children.is_empty() {
                                info!(
                                    "[Router] Namespace command /{} — sending inline keyboard with {} children",
                                    namespace,
                                    children.len()
                                );
                                let keyboard = self.build_namespace_keyboard(namespace, &children);
                                let reply = OutboundMessage {
                                    conversation_id: msg.conversation_id.clone(),
                                    text: format!("/{} — choose a sub-command:", namespace),
                                    attachments: Vec::new(),
                                    reply_to: Some(msg.id.clone()),
                                    inline_keyboard: Some(keyboard),
                                    metadata: Default::default(),
                                };
                                if let Err(e) =
                                    self.channel_registry.send(&msg.channel_id, reply).await
                                {
                                    error!("[Router] Failed to send namespace keyboard: {}", e);
                                }
                                return Ok(());
                            }
                        }

                        // Last-mile suggestion: surface near-matches before
                        // falling through to the agent. Avoids the "silent
                        // typo" footgun where /seasion_new is passed to the
                        // model as plain text. Only handles namespace-like
                        // tokens (not shorthand) — and is_shorthand list
                        // above already short-circuited those.
                        if self.try_send_unknown_command_help(&msg, namespace).await {
                            return Ok(());
                        }
                    }
                }
            }

            // Fallback: /groupchat without unified registry
            if slash_text.starts_with("/groupchat") && self.group_chat_orch.is_some() {
                return self.handle_groupchat_command(&msg).await;
            }

            // Fallback: /new or /session new without unified registry
            let trimmed = slash_text.trim();
            if trimmed == "/new" || trimmed == "/session new" {
                return self.handle_new_session(&msg, &ctx).await;
            }
        }

        // Group chat: check for active sessions
        if self.group_chat_orch.is_some() {
            if let Some(handled) = self.try_handle_group_chat(&msg).await {
                match handled {
                    Ok(()) => return Ok(()),
                    Err(e) => {
                        let error_msg = OutboundMessage::text(
                            msg.conversation_id.as_str(),
                            format!("Group chat error: {}", e),
                        );
                        let _ = self.channel_registry.send(&msg.channel_id, error_msg).await;
                        return Ok(());
                    }
                }
            }
        }

        // Execute the agent for this context
        self.execute_for_context(&ctx).await?;

        Ok(())
    }

    /// Intercept a reply to a pending HITL interaction (tool/sandbox approval
    /// or an `ask_user` clarification) before it would start a new agent turn.
    ///
    /// Returns `true` if the message was consumed as an interaction reply.
    async fn try_intercept_hitl(&self, ctx: &InboundContext) -> bool {
        let session_key = ctx.session_key.to_string();
        let raw = strip_bot_mention(ctx.message.text.trim());
        let lower = raw.trim().to_lowercase();

        // Approval reply: `/approve [once|session|always]` or `/deny`.
        // A bare `/approve` (or any unrecognized suffix) stays the
        // least-privilege `AllowOnce`; `session` / `always` widen the grant
        // scope (hermes' once/session/always tiers). The manager clamps an
        // `always` on a Danger-classified command down to a session grant.
        let is_approve = lower == "/approve" || lower.starts_with("/approve ");
        let is_deny = lower == "/deny" || lower.starts_with("/deny ");
        if is_approve || is_deny {
            if let Some(ref mgr) = self.exec_approval_manager {
                use crate::exec::socket::ApprovalDecisionType;
                let decision = if is_approve {
                    match lower.strip_prefix("/approve").map(str::trim) {
                        Some("once") => ApprovalDecisionType::AllowOnce,
                        Some("session") => ApprovalDecisionType::AllowSession,
                        Some("always") => ApprovalDecisionType::AllowAlways,
                        _ => ApprovalDecisionType::AllowOnce,
                    }
                } else {
                    ApprovalDecisionType::Deny
                };
                // Reply with the EFFECTIVE decision: the manager may clamp an
                // `always` on a Danger-classified command to a session grant.
                let reply = match mgr.resolve_for_session(
                    &session_key,
                    decision,
                    ctx.message.sender_name.clone(),
                ) {
                    Some(ApprovalDecisionType::AllowOnce) => "✅ Approved (once).",
                    Some(ApprovalDecisionType::AllowSession) => "✅ Approved for this session.",
                    Some(ApprovalDecisionType::AllowAlways) => "✅ Approved (always).",
                    Some(ApprovalDecisionType::Deny) => "❌ Denied.",
                    None => "Nothing is awaiting your approval right now.",
                };
                let _ = self
                    .channel_registry
                    .send(
                        &ctx.reply_route.channel_id,
                        OutboundMessage::text(ctx.reply_route.conversation_id.as_str(), reply),
                    )
                    .await;
            }
            // `/approve` and `/deny` are always consumed here — never forwarded
            // to the agent loop.
            return true;
        }

        // Clarification reply: any message while an `ask_user` is pending for
        // this session is taken as the answer.
        if let Some(ref mgr) = self.clarification_manager {
            if mgr.has_pending(&session_key).await
                && mgr.resolve(&session_key, &ctx.message.text).await
            {
                info!("[Router] Routed reply to pending clarification for {session_key}");
                return true;
            }
        }

        // Workflow clarify reply: a paused `clarify` step for this session is
        // completed with the answer, unblocking its dependents on the next tick.
        if let Some(ref store) = self.workflow_clarify_store {
            if self
                .try_resolve_workflow_clarify(store.as_ref(), &session_key, &ctx.message.text)
                .await
            {
                info!("[Router] Routed reply to workflow clarify step for {session_key}");
                return true;
            }
        }

        false
    }

    /// Complete a paused workflow `clarify` step for `session_key` from the
    /// user's `reply`, if one is awaiting. Returns `true` when a step was
    /// resolved.
    ///
    /// The `coord_task` row is the awaiting record (durable across restart), so
    /// this is a stateless lookup: find the paused clarify task whose stored
    /// `session_key` matches, interpret the reply against its question/choices
    /// exactly as `ask_user` does, write the answer into the task's `result`,
    /// and mark it `Completed`. The dispatcher's handoff context then feeds that
    /// answer into the downstream steps.
    async fn try_resolve_workflow_clarify(
        &self,
        store: &dyn crate::agents::swarm::tasks::CoordTaskStore,
        session_key: &str,
        reply: &str,
    ) -> bool {
        use crate::agents::swarm::tasks::{CoordTaskFilter, CoordTaskStatus, CoordTaskUpdate};
        use crate::workflow::clarify::ClarifyTaskMeta;

        if session_key.is_empty() {
            return false;
        }
        let paused = match store
            .list_tasks(CoordTaskFilter {
                status: Some(CoordTaskStatus::Paused),
                ..Default::default()
            })
            .await
        {
            Ok(tasks) => tasks,
            Err(e) => {
                warn!(error = %e, "[Router] workflow clarify: list_tasks failed");
                return false;
            }
        };

        for task in paused {
            let Some(meta) = ClarifyTaskMeta::from_metadata(&task.metadata) else {
                continue;
            };
            if meta.session_key != session_key {
                continue;
            }
            // Interpret number / label / free-text exactly like `ask_user`.
            let request = meta.build_request(&task.id);
            let result = crate::clarification::session::interpret_reply(&request, reply);
            let answer = result.value.unwrap_or_default();
            if let Err(e) = store
                .update_task(
                    &task.id,
                    CoordTaskUpdate {
                        status: Some(CoordTaskStatus::Completed),
                        result: Some(answer),
                        ..Default::default()
                    },
                )
                .await
            {
                warn!(task_id = %task.id, error = %e, "[Router] workflow clarify: complete failed");
                return false;
            }
            if let Some(ref signal) = self.dispatch_signal {
                signal.notify_one();
            }
            return true;
        }
        false
    }

    /// Build an inline keyboard for namespace sub-commands.
    ///
    /// Each child tool becomes a button. The callback data is the full
    /// slash command (e.g. "/session new") so that callback handling can
    /// route it through the normal message pipeline.
    fn build_namespace_keyboard(
        &self,
        namespace: &str,
        children: &[crate::tool_metadata::UnifiedTool],
    ) -> super::channel::InlineKeyboard {
        use super::channel::{InlineButton, InlineKeyboard};

        let prefix = format!("{}_", namespace);
        // Build buttons: 2 per row for compact layout
        let buttons: Vec<InlineButton> = children
            .iter()
            .map(|tool| {
                let sub_name = tool.name.strip_prefix(&prefix).unwrap_or(&tool.name);
                let display = sub_name.to_string();
                // Callback data: "/namespace sub" format for re-injection
                let callback = format!("/{} {}", namespace, sub_name);
                InlineButton {
                    text: display,
                    callback_data: callback,
                }
            })
            .collect();

        let mut keyboard = InlineKeyboard::new();
        for chunk in buttons.chunks(2) {
            keyboard.rows.push(chunk.to_vec());
        }
        keyboard
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::channel::{ChannelId, ConversationId, MessageId, UserId};
    use crate::gateway::inbound_router::approval_callback::{
        ApprovalCallbackResult, ApprovalCallbackSink,
    };
    use crate::gateway::pairing_store::SqlitePairingStore;
    use crate::gateway::routing_config::DmScope;

    /// Sink stub —— 任何回调都拦截。
    struct AlwaysIntercept;
    #[async_trait::async_trait]
    impl ApprovalCallbackSink for AlwaysIntercept {
        async fn handle_callback(&self, _d: &str, _u: &str) -> Option<ApprovalCallbackResult> {
            Some(ApprovalCallbackResult {
                resolved: true,
                response_text: "ok".to_string(),
            })
        }
    }

    fn cb_message() -> InboundMessage {
        InboundMessage {
            id: MessageId::new("cb_1"),
            channel_id: ChannelId::new("telegram"),
            conversation_id: ConversationId::new("123"),
            sender_id: UserId::new("u1"),
            sender_name: None,
            text: "approve:rec-1:once".to_string(),
            attachments: vec![],
            timestamp: chrono::Utc::now(),
            reply_to: None,
            is_group: false,
            raw: None,
            metadata: vec![],
        }
    }

    /// `cb_` 前缀 + 注入 sink → handle_message 拦截并早返回 Ok，
    /// 不进入 agent 解析（空 registry，send 静默失败）。
    #[tokio::test]
    async fn cb_message_with_approval_sink_is_intercepted() {
        let router = InboundMessageRouter::new(
            Arc::new(ChannelRegistry::new()),
            Arc::new(SqlitePairingStore::in_memory().unwrap()),
            RoutingConfig::default(),
        )
        .with_approval_callback_sink(Arc::new(AlwaysIntercept));

        assert!(router.handle_message(cb_message()).await.is_ok());
    }

    /// H1 — a zero-config (no route bindings) group message must resolve to a
    /// `Group` session key, not a `DirectMessage`. Before the fix the fallback
    /// built `SessionKey::peer(...)` → a DM variant with an empty channel,
    /// mistyping the group chat and splitting its history from the
    /// configured-binding path's `SessionKey::group(...)`.
    #[test]
    fn group_message_resolves_to_group_session_key() {
        let router = InboundMessageRouter::new(
            Arc::new(ChannelRegistry::new()),
            Arc::new(SqlitePairingStore::in_memory().unwrap()),
            RoutingConfig::default(),
        );
        let msg = InboundMessage {
            id: MessageId::new("m1"),
            channel_id: ChannelId::new("telegram"),
            conversation_id: ConversationId::new("group-123"),
            sender_id: UserId::new("u1"),
            sender_name: None,
            text: "hello group".to_string(),
            attachments: vec![],
            timestamp: chrono::Utc::now(),
            reply_to: None,
            is_group: true,
            raw: None,
            metadata: vec![],
        };
        let key = router.resolve_session_key_with_agent(&msg, "main");
        assert!(
            matches!(
                &key,
                crate::routing::session_key::SessionKey::Group { channel, peer_id, .. }
                    if channel == "telegram" && peer_id == "group-123"
            ),
            "zero-config group message must resolve to a Group key; got: {key:?}",
        );
    }

    /// 单用户 owner：dm_scope=Main 时，零配置 fallback 路径的 DM 必须坍缩到
    /// `agent:<id>:main`，使同一 agent 在不同 channel 的 DM 共享同一 Main 会话。
    #[test]
    fn dm_main_scope_collapses_to_main_session_key() {
        let router = InboundMessageRouter::new(
            Arc::new(ChannelRegistry::new()),
            Arc::new(SqlitePairingStore::in_memory().unwrap()),
            RoutingConfig::default().with_dm_scope(DmScope::Main),
        );
        let make = |channel: &str| InboundMessage {
            id: MessageId::new("m1"),
            channel_id: ChannelId::new(channel),
            conversation_id: ConversationId::new("dm-conv"),
            sender_id: UserId::new("owner"),
            sender_name: None,
            text: "hi".to_string(),
            attachments: vec![],
            timestamp: chrono::Utc::now(),
            reply_to: None,
            is_group: false,
            raw: None,
            metadata: vec![],
        };
        // 同一 agent、不同 channel → 同一 Main key（跨 channel 共享）。
        let k_tg = router.resolve_session_key_with_agent(&make("telegram"), "main");
        let k_sl = router.resolve_session_key_with_agent(&make("slack"), "main");
        assert_eq!(k_tg.to_key_string(), "agent:main:main");
        assert_eq!(k_sl.to_key_string(), "agent:main:main");
        assert_eq!(k_tg.to_key_string(), k_sl.to_key_string());
    }

    /// 反向保护：默认 PerPeer 下 DM 仍按 peer 隔离（零回归）。
    #[test]
    fn dm_per_peer_scope_stays_isolated() {
        let router = InboundMessageRouter::new(
            Arc::new(ChannelRegistry::new()),
            Arc::new(SqlitePairingStore::in_memory().unwrap()),
            RoutingConfig::default(), // PerPeer
        );
        let msg = InboundMessage {
            id: MessageId::new("m2"),
            channel_id: ChannelId::new("telegram"),
            conversation_id: ConversationId::new("dm-conv"),
            sender_id: UserId::new("owner"),
            sender_name: None,
            text: "hi".to_string(),
            attachments: vec![],
            timestamp: chrono::Utc::now(),
            reply_to: None,
            is_group: false,
            raw: None,
            metadata: vec![],
        };
        let key = router.resolve_session_key_with_agent(&msg, "main");
        // Fallback uses SessionKey::peer(agent, "dm:{sender}") with empty channel →
        // sanitize_component replaces ':' with '-' → "agent:main:peer:dm-owner".
        assert_eq!(key.to_key_string(), "agent:main:peer:dm-owner");
    }

    /// 不同 agent 各自 Main，互不串味。
    #[test]
    fn dm_main_scope_different_agents_isolated() {
        let router = InboundMessageRouter::new(
            Arc::new(ChannelRegistry::new()),
            Arc::new(SqlitePairingStore::in_memory().unwrap()),
            RoutingConfig::default().with_dm_scope(DmScope::Main),
        );
        let msg = InboundMessage {
            id: MessageId::new("m3"),
            channel_id: ChannelId::new("telegram"),
            conversation_id: ConversationId::new("dm-conv"),
            sender_id: UserId::new("owner"),
            sender_name: None,
            text: "hi".to_string(),
            attachments: vec![],
            timestamp: chrono::Utc::now(),
            reply_to: None,
            is_group: false,
            raw: None,
            metadata: vec![],
        };
        let work = router.resolve_session_key_with_agent(&msg, "work");
        let personal = router.resolve_session_key_with_agent(&msg, "personal");
        assert_eq!(work.to_key_string(), "agent:work:main");
        assert_eq!(personal.to_key_string(), "agent:personal:main");
        assert_ne!(work.to_key_string(), personal.to_key_string());
    }

    fn bot_authored_msg(id: &str) -> InboundMessage {
        use crate::gateway::channel::MessageMeta;
        InboundMessage {
            id: MessageId::new(id),
            channel_id: ChannelId::new("telegram"),
            conversation_id: ConversationId::new("conv-bot"),
            sender_id: UserId::new("other-bot"),
            sender_name: None,
            text: "ping".to_string(),
            attachments: vec![],
            timestamp: chrono::Utc::now(),
            reply_to: None,
            is_group: false,
            raw: None,
            metadata: vec![MessageMeta::BotAuthored],
        }
    }

    /// pair_loop_check: human-authored messages bypass the guard entirely
    /// (no metadata → guard skipped, returns true).
    #[test]
    fn pair_loop_check_skips_human_messages() {
        let router = InboundMessageRouter::new(
            Arc::new(ChannelRegistry::new()),
            Arc::new(SqlitePairingStore::in_memory().unwrap()),
            RoutingConfig::default(),
        );
        let human = InboundMessage {
            id: MessageId::new("m-h"),
            channel_id: ChannelId::new("telegram"),
            conversation_id: ConversationId::new("conv-h"),
            sender_id: UserId::new("human"),
            sender_name: None,
            text: "hi".to_string(),
            attachments: vec![],
            timestamp: chrono::Utc::now(),
            reply_to: None,
            is_group: false,
            raw: None,
            metadata: vec![],
        };
        for _ in 0..1_000 {
            assert!(router.pair_loop_check(&human));
        }
    }

    /// pair_loop_check: a bot-authored sender exceeding the configured budget
    /// is suppressed; subsequent attempts during cooldown stay suppressed.
    #[test]
    fn pair_loop_check_suppresses_bot_storm() {
        let cfg = PairLoopGuardConfig {
            enabled: true,
            max_events_per_window: 3,
            window_seconds: 60,
            cooldown_seconds: 30,
        };
        let router = InboundMessageRouter::new(
            Arc::new(ChannelRegistry::new()),
            Arc::new(SqlitePairingStore::in_memory().unwrap()),
            RoutingConfig::default(),
        )
        .with_bot_loop_protection(cfg);

        // First 3 events fit in the budget.
        for i in 0..3 {
            assert!(router.pair_loop_check(&bot_authored_msg(&format!("m{i}"))));
        }
        // 4th tips the pair into cooldown.
        assert!(!router.pair_loop_check(&bot_authored_msg("m3")));
        // Subsequent attempts during cooldown remain blocked.
        assert!(!router.pair_loop_check(&bot_authored_msg("m4")));
    }
}
