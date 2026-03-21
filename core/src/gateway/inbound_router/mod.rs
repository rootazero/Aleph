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
        let ctx = match self.check_permission(ctx).await {
            Ok(ctx) => {
                info!("[Router] Permission granted for {}:{}", channel_id, ctx.sender_normalized);
                ctx
            }
            Err(e) => {
                info!("[Router] Permission denied for {}:{} — {}", channel_id, msg.sender_id.as_str(), e);
                return Ok(());
            }
        };

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
                }
            }

            // Fallback: /groupchat without unified registry
            if slash_text.starts_with("/groupchat") && self.group_chat_orch.is_some() {
                return self.handle_groupchat_command(&msg).await;
            }

            // Fallback: /session_new or /new without unified registry
            let trimmed = slash_text.trim();
            if trimmed == "/session_new" || trimmed == "/new" {
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
}
