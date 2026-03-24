//! Inbound Message Router
//!
//! Consumes the ChannelRegistry's inbound message stream and routes
//! messages to the appropriate Agent/Session.

mod agent_resolver;
mod command_handler;
mod dedup;
mod executor;
mod group_chat_handler;
mod permission;

mod types;

pub use types::{
    ChannelConfig, DmPolicy, GroupPolicy, RoutingError, SLASH_COMMAND_MODE_KEY,
};

use std::collections::HashMap;
use crate::sync_primitives::Arc;
use tokio::sync::{mpsc, Mutex};
use tracing::{error, info, warn};

use super::agent_instance::AgentRegistry;
use super::channel::{InboundMessage, OutboundMessage};
use super::channel_registry::ChannelRegistry;
use super::execution_adapter::ExecutionAdapter;
use super::handlers::group_chat::SharedOrchestrator;

use super::pairing_store::PairingStore;
use super::routing_config::RoutingConfig;
use super::agent_env::AgentEnvStore;
use crate::command::CommandParser;
use crate::group_chat::GroupChatExecutor;

use command_handler::{serialize_intent_result, strip_bot_mention};
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
    /// Inbound message deduplication tracker
    dedup_tracker: Mutex<InboundDedupTracker>,
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
    /// Debounce buffer for message merging
    pub(super) debounce_buffer: Option<Arc<crate::gateway::pipeline::DebounceBuffer>>,
    /// Session manager for session lifecycle
    pub(super) session_manager: Option<Arc<super::session_manager::SessionManager>>,
    /// App config for reading output_mode at runtime
    pub(super) app_config: Option<Arc<tokio::sync::RwLock<crate::Config>>>,
    /// STT config for voice transcription
    pub(super) stt_config: Option<super::voice::inbound::SttConfig>,
    /// Generation provider registry for TTS
    pub(super) generation_registry: Option<Arc<crate::generation::GenerationProviderRegistry>>,
    /// Generation config for TTS
    pub(super) generation_config: Option<Arc<tokio::sync::RwLock<crate::config::types::generation::GenerationConfig>>>,
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
            dedup_tracker: Mutex::new(InboundDedupTracker::new()),
            group_chat_orch: None,
            group_chat_executor: None,
            active_group_sessions: Mutex::new(HashMap::new()),
            llm_provider: None,
            command_parser: None,
            debounce_buffer: None,
            session_manager: None,
            app_config: None,
            stt_config: None,
            generation_registry: None,
            generation_config: None,
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

    /// Set the command parser for dynamic slash command resolution
    pub fn with_command_parser(mut self, parser: Arc<CommandParser>) -> Self {
        self.command_parser = Some(parser);
        self
    }

    /// Set the session manager for session lifecycle operations
    pub fn with_session_manager(mut self, sm: Arc<super::session_manager::SessionManager>) -> Self {
        self.session_manager = Some(sm);
        self
    }

    /// Set the debounce buffer for message merging
    pub fn with_debounce_buffer(mut self, buffer: Arc<crate::gateway::pipeline::DebounceBuffer>) -> Self {
        self.debounce_buffer = Some(buffer);
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
    async fn run_loop(self: Arc<Self>, mut rx: mpsc::Receiver<InboundMessage>) {
        info!("InboundMessageRouter started");

        while let Some(msg) = rx.recv().await {
            // Deduplication check
            let dedup_key = format!("{}:{}", msg.channel_id.as_str(), msg.id.as_str());
            {
                let mut tracker = self.dedup_tracker.lock().await;
                if !tracker.check_and_record(&dedup_key) {
                    warn!(
                        "Duplicate message detected and dropped: {} from {}:{}",
                        dedup_key, msg.channel_id.as_str(), msg.sender_id.as_str()
                    );
                    continue;
                }
            }

            let router = self.clone();
            tokio::spawn(async move {
                if let Err(e) = router.handle_message(msg).await {
                    error!("Failed to handle inbound message: {}", e);
                }
            });
        }

        info!("InboundMessageRouter stopped");
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

        // Resolve agent ID from 1:1 channel binding (single-tier)
        let agent_id = match self.resolve_agent_id_async(channel_id).await {
            Some(id) => id,
            None => {
                // Unbound channel — send fixed message
                let reply = OutboundMessage::text(
                    msg.conversation_id.as_str(),
                    "此频道未绑定 Agent，请在 Panel 中配置",
                );
                if let Err(e) = self.channel_registry.send(&msg.channel_id, reply).await {
                    error!("[Router] Failed to send unbound-channel message: {}", e);
                }
                return Ok(());
            }
        };

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
        let ctx = self.build_context_with_agent(&msg, &agent_id).await;

        // Check permissions
        let mut ctx = match self.check_permission(ctx).await {
            Ok(ctx) => {
                info!("[Router] Permission granted for {}:{}", channel_id, ctx.sender_normalized);
                ctx
            }
            Err(e) => {
                info!("[Router] Permission denied for {}:{} — {}", channel_id, msg.sender_id.as_str(), e);
                return Ok(());
            }
        };

        // Voice STT: transcribe audio attachments before further processing
        let has_stt = self.stt_config.is_some();
        let has_audio = super::voice::inbound::has_audio_attachment(&ctx.message);
        tracing::debug!(has_stt = has_stt, has_audio = has_audio, attachments = ctx.message.attachments.len(), "Voice STT check");
        if let Some(ref stt_config) = self.stt_config {
            if has_audio {
                let result = super::voice::inbound::process_inbound_voice(
                    ctx.message.clone(),
                    stt_config,
                )
                .await;
                ctx.message = result.message;
                if result.transcribed {
                    ctx.voice_reply_hint = true;
                }
            }
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
                    "on" | "open" | "开" | "开启" => (true, "Voice mode enabled. I'll reply with voice from now on."),
                    "off" | "close" | "关" | "关闭" => (false, "Voice mode disabled. Switching back to text replies."),
                    "" => {
                        // No args: show inline keyboard with voice sub-commands
                        use super::channel::{InlineButton, InlineKeyboard};
                        let state = self.channel_registry.get_voice_state("default").await;
                        let ch_state = self.channel_registry.get_voice_state(ctx.reply_route.channel_id.as_str()).await;
                        let is_on = state.is_active() || ch_state.is_active();
                        let status = if is_on { "🔊 ON" } else { "🔇 OFF" };

                        let mut keyboard = InlineKeyboard::new();
                        keyboard.rows.push(vec![
                            InlineButton { text: "🔊 On".to_string(), callback_data: "/voice on".to_string() },
                            InlineButton { text: "🔇 Off".to_string(), callback_data: "/voice off".to_string() },
                            InlineButton { text: "📊 Status".to_string(), callback_data: "/voice status".to_string() },
                        ]);

                        let reply_msg = super::channel::OutboundMessage {
                            conversation_id: ctx.reply_route.conversation_id.clone(),
                            text: format!("🎙 Voice Mode: {}", status),
                            attachments: vec![],
                            reply_to: ctx.reply_route.reply_to.clone(),
                            inline_keyboard: Some(keyboard),
                            metadata: Default::default(),
                        };
                        let _ = self.channel_registry.send(&ctx.reply_route.channel_id, reply_msg).await;
                        return Ok(());
                    }
                    "status" | "状态" => {
                        // Show current status as text
                        let state = self.channel_registry.get_voice_state("default").await;
                        let ch_state = self.channel_registry.get_voice_state(ctx.reply_route.channel_id.as_str()).await;
                        let is_on = state.is_active() || ch_state.is_active();
                        let provider = state.provider.as_deref()
                            .or(ch_state.provider.as_deref())
                            .unwrap_or("auto");
                        let voice = state.voice.as_deref()
                            .or(ch_state.voice.as_deref())
                            .unwrap_or("default");
                        let status_text = format!(
                            "🎙 Voice Mode: {}\nProvider: {}\nVoice: {}\nFailures: {}",
                            if is_on { "ON" } else { "OFF" },
                            provider, voice, ch_state.consecutive_failures
                        );
                        let reply_msg = super::channel::OutboundMessage {
                            conversation_id: ctx.reply_route.conversation_id.clone(),
                            text: status_text,
                            attachments: vec![],
                            reply_to: ctx.reply_route.reply_to.clone(),
                            inline_keyboard: None,
                            metadata: Default::default(),
                        };
                        let _ = self.channel_registry.send(&ctx.reply_route.channel_id, reply_msg).await;
                        return Ok(());
                    }
                    _ => (true, "Voice mode enabled."),
                };
                self.channel_registry.update_voice_state("default", |s| {
                    s.enabled = enabled;
                    s.consecutive_failures = 0;
                }).await;
                // Also set for the specific channel
                self.channel_registry.update_voice_state(ctx.reply_route.channel_id.as_str(), |s| {
                    s.enabled = enabled;
                    s.consecutive_failures = 0;
                }).await;
                // Send confirmation — as voice if enabling, as text if disabling
                if enabled {
                    // Try voice reply for confirmation
                    if let (Some(ref gen_reg), Some(ref gen_cfg)) = (&self.generation_registry, &self.generation_config) {
                        let voice_state = self.channel_registry.get_voice_state("default").await;
                        let cfg = gen_cfg.read().await.clone();
                        if let Some(attachment) = super::voice::outbound::generate_tts(reply, &voice_state, gen_reg, &cfg).await {
                            let voice_msg = super::channel::OutboundMessage {
                                conversation_id: ctx.reply_route.conversation_id.clone(),
                                text: String::new(),
                                attachments: vec![attachment],
                                reply_to: ctx.reply_route.reply_to.clone(),
                                inline_keyboard: None,
                                metadata: Default::default(),
                            };
                            let _ = self.channel_registry.send(&ctx.reply_route.channel_id, voice_msg).await;
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
                let _ = self.channel_registry.send(&ctx.reply_route.channel_id, reply_msg).await;
                return Ok(());
            }
        }

        // Unified slash command interception
        if ctx.message.text.trim().starts_with('/') {
            let slash_text = strip_bot_mention(ctx.message.text.trim());
            let slash_text = slash_text.as_str();

            // Try unified command resolution first (async, all sources)
            if let Some(ref parser) = self.command_parser {
                if let Some(parsed) = parser.parse_async(slash_text).await {
                    if parsed.command_name == "groupchat" {
                        return self.handle_groupchat_command(&msg).await;
                    }
                    if parsed.command_name == "session_new" || parsed.command_name == "new" {
                        return self.handle_new_session(&msg, &ctx).await;
                    }
                    let result = self.parsed_command_to_intent_result(parsed);
                    if let Some(mode_json) = serialize_intent_result(&result) {
                        info!(
                            "[Router] Slash command resolved: source=unified, name={}",
                            ctx.message.text.split_whitespace().next().unwrap_or("")
                        );
                        self.execute_for_context_with_metadata(&ctx, mode_json).await?;
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
                        .split('@')  // Strip @botname suffix
                        .next()
                        .unwrap_or("");
                    // Skip namespace check for shorthand aliases — these are resolved
                    // by slash_command.rs fast-path (e.g. /image → image_generate).
                    let is_shorthand = matches!(namespace, "image" | "video" | "audio" | "speech" | "rename");
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
                                if let Err(e) = self.channel_registry.send(&msg.channel_id, reply).await {
                                    error!("[Router] Failed to send namespace keyboard: {}", e);
                                }
                                return Ok(());
                            }
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

    /// Build an inline keyboard for namespace sub-commands.
    ///
    /// Each child tool becomes a button. The callback data is the full
    /// slash command (e.g. "/session new") so that callback handling can
    /// route it through the normal message pipeline.
    fn build_namespace_keyboard(
        &self,
        namespace: &str,
        children: &[crate::dispatcher::UnifiedTool],
    ) -> super::channel::InlineKeyboard {
        use super::channel::{InlineButton, InlineKeyboard};

        let prefix = format!("{}_", namespace);
        // Build buttons: 2 per row for compact layout
        let buttons: Vec<InlineButton> = children
            .iter()
            .map(|tool| {
                let sub_name = tool
                    .name
                    .strip_prefix(&prefix)
                    .unwrap_or(&tool.name);
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
