//! Inbound Message Router
//!
//! Consumes the ChannelRegistry's inbound message stream and routes
//! messages to the appropriate Agent/Session.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};
use crate::sync_primitives::Arc;
use tokio::sync::{mpsc, Mutex};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use super::agent_instance::AgentRegistry;
use super::intent_detector::{IntentDetector, DetectedIntent, build_id_resolve_prompt, build_soul_generation_prompt};
use super::channel::{InboundMessage, OutboundMessage};
use super::channel_registry::ChannelRegistry;
use super::execution_adapter::ExecutionAdapter;
use super::execution_engine::RunRequest;
use super::handlers::group_chat::SharedOrchestrator;
use super::inbound_context::{InboundContext, ReplyRoute};
use super::pairing_store::{PairingError, PairingStore};
use super::reply_emitter::ReplyEmitter;
use super::router::{AgentRouter, SessionKey};
use super::routing_config::{DmScope, RoutingConfig};
use super::workspace::WorkspaceManager;
use crate::command::CommandParser;
use crate::group_chat::{
    DefaultGroupChatCommandParser, GroupChatCommandParser, GroupChatExecutor,
    GroupChatRequest, GroupChatStatus,
};
use crate::intent::{DirectToolSource, IntentResult, UnifiedIntentClassifier};

#[cfg(target_os = "macos")]
use super::interfaces::imessage::normalize_phone;

// Fallback for non-macOS platforms
#[cfg(not(target_os = "macos"))]
fn normalize_phone(phone: &str) -> String {
    // Simple normalization: remove all non-digit characters except leading +
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

/// Error type for routing operations
#[derive(Debug, thiserror::Error)]
pub enum RoutingError {
    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    #[error("Channel error: {0}")]
    Channel(String),

    #[error("Execution error: {0}")]
    Execution(String),

    #[error("Agent not found: {0}")]
    AgentNotFound(String),

    #[error("Pairing error: {0}")]
    Pairing(#[from] PairingError),
}

/// Time window for inbound message deduplication (5 minutes)
const DEDUP_WINDOW: Duration = Duration::from_secs(300);

/// Maximum dedup entries before forced cleanup
const DEDUP_MAX_ENTRIES: usize = 10_000;

/// Tracks recently processed inbound message IDs to prevent duplicate execution
struct InboundDedupTracker {
    /// Set of "channel_id:message_id" keys
    seen: HashSet<String>,
    /// Ordered list of (key, timestamp) for expiry
    entries: Vec<(String, Instant)>,
}

impl InboundDedupTracker {
    fn new() -> Self {
        Self {
            seen: HashSet::new(),
            entries: Vec::new(),
        }
    }

    /// Check if message was already processed. If not, mark it as seen.
    /// Returns true if this is a NEW message (not a duplicate).
    fn check_and_record(&mut self, key: &str) -> bool {
        // Expire old entries first
        self.expire();

        if self.seen.contains(key) {
            return false; // Duplicate
        }

        self.seen.insert(key.to_string());
        self.entries.push((key.to_string(), Instant::now()));
        true
    }

    /// Remove entries older than DEDUP_WINDOW
    fn expire(&mut self) {
        let cutoff = Instant::now() - DEDUP_WINDOW;
        let before = self.entries.len();

        self.entries.retain(|(key, ts)| {
            if *ts < cutoff {
                self.seen.remove(key);
                false
            } else {
                true
            }
        });

        if before > self.entries.len() {
            debug!(
                "Dedup tracker: expired {} entries, {} remaining",
                before - self.entries.len(),
                self.entries.len()
            );
        }

        // Safety cap: if somehow we accumulate too many, drop oldest half
        if self.entries.len() > DEDUP_MAX_ENTRIES {
            let drain_count = self.entries.len() / 2;
            for (key, _) in self.entries.drain(..drain_count) {
                self.seen.remove(&key);
            }
            warn!(
                "Dedup tracker hit max entries, forcibly dropped {} entries",
                drain_count
            );
        }
    }
}

/// Metadata key for slash command execution mode in RunRequest
pub const SLASH_COMMAND_MODE_KEY: &str = "slash_command_mode";

/// Strip @botname suffix from Telegram-style slash commands.
///
/// In Telegram groups, commands are sent as `/command@botname args`.
/// This function normalizes to `/command args` so downstream parsers
/// can resolve the command name correctly.
fn strip_bot_mention(input: &str) -> String {
    if !input.starts_with('/') {
        return input.to_string();
    }
    // Split into command part and arguments
    let (cmd_part, rest) = match input.split_once(char::is_whitespace) {
        Some((cmd, args)) => (cmd, Some(args)),
        None => (input, None),
    };
    // Strip @botname from the command part
    let clean_cmd = match cmd_part.split_once('@') {
        Some((cmd, _)) => cmd,
        None => cmd_part,
    };
    match rest {
        Some(args) => format!("{} {}", clean_cmd, args),
        None => clean_cmd.to_string(),
    }
}

/// Serialize an `IntentResult` to a JSON string for RunRequest metadata.
///
/// Returns `Some(json)` for `DirectTool` (which maps to a specific tool invocation),
/// and `None` for `Execute`, `Converse`, and `Abort` (which are handled by the agent loop).
fn serialize_intent_result(result: &IntentResult) -> Option<String> {
    match result {
        IntentResult::DirectTool {
            tool_id,
            args,
            source,
        } => {
            let source_str = match source {
                DirectToolSource::SlashCommand => "slash_command",
                DirectToolSource::Skill => "skill",
                DirectToolSource::Mcp => "mcp",
                DirectToolSource::Custom => "custom",
            };
            serde_json::to_string(&serde_json::json!({
                "type": "direct_tool",
                "tool_id": tool_id,
                "args": args,
                "source": source_str,
            }))
            .ok()
        }
        // Execute, Converse, and Abort are not direct commands
        IntentResult::Execute { .. }
        | IntentResult::Converse { .. }
        | IntentResult::Abort => None,
    }
}

/// Inbound message router
pub struct InboundMessageRouter {
    channel_registry: Arc<ChannelRegistry>,
    pairing_store: Arc<dyn PairingStore>,
    config: RoutingConfig,
    /// Channel-specific configs (keyed by channel_id)
    channel_configs: HashMap<String, ChannelConfig>,
    /// Agent registry for looking up agent instances
    agent_registry: Option<Arc<AgentRegistry>>,
    /// Execution adapter for running agents
    execution_adapter: Option<Arc<dyn ExecutionAdapter>>,
    /// Agent router for binding-based agent selection (unified with WS routing)
    agent_router: Option<Arc<AgentRouter>>,
    /// Workspace manager for channel-level active agent lookup
    workspace_manager: Option<Arc<WorkspaceManager>>,
    /// Inbound message deduplication tracker
    dedup_tracker: Mutex<InboundDedupTracker>,
    /// Group chat orchestrator (optional — disabled if no API key)
    group_chat_orch: Option<SharedOrchestrator>,
    /// Group chat executor (optional — disabled if no API key)
    group_chat_executor: Option<Arc<GroupChatExecutor>>,
    /// Active group chat sessions: "channel_id:conversation_id" -> session_id
    active_group_sessions: Mutex<HashMap<String, String>>,
    /// Intent detector for natural language agent switching
    intent_detector: Option<IntentDetector>,
    /// LLM provider for intent classification and soul generation
    llm_provider: Option<Arc<dyn crate::providers::AiProvider>>,
    /// Command parser for unified slash command resolution (optional)
    /// When set, resolves all command sources (builtin, skill, MCP, custom)
    command_parser: Option<Arc<CommandParser>>,
    /// New unified intent classifier (v3 pipeline, additive migration)
    unified_classifier: Option<UnifiedIntentClassifier>,
}

/// Unified channel config for permission checking
#[derive(Debug, Clone)]
pub struct ChannelConfig {
    /// DM policy
    pub dm_policy: DmPolicy,
    /// Group policy
    pub group_policy: GroupPolicy,
    /// Allowlist for DMs
    pub allow_from: Vec<String>,
    /// Allowlist for groups
    pub group_allow_from: Vec<String>,
    /// Whether to require mention in groups
    pub require_mention: bool,
    /// Bot name for mention detection
    pub bot_name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DmPolicy {
    Open,
    Allowlist,
    Pairing,
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupPolicy {
    Open,
    Allowlist,
    Disabled,
}

impl Default for ChannelConfig {
    fn default() -> Self {
        Self {
            dm_policy: DmPolicy::Pairing,
            group_policy: GroupPolicy::Open,
            allow_from: Vec::new(),
            group_allow_from: Vec::new(),
            require_mention: true,
            bot_name: None,
        }
    }
}

#[cfg(target_os = "macos")]
impl From<&super::interfaces::imessage::IMessageConfig> for ChannelConfig {
    fn from(cfg: &super::interfaces::imessage::IMessageConfig) -> Self {
        use super::interfaces::imessage::{IMessageDmPolicy, IMessageGroupPolicy};

        Self {
            dm_policy: match cfg.dm_policy {
                IMessageDmPolicy::Open => DmPolicy::Open,
                IMessageDmPolicy::Allowlist => DmPolicy::Allowlist,
                IMessageDmPolicy::Pairing => DmPolicy::Pairing,
                IMessageDmPolicy::Disabled => DmPolicy::Disabled,
            },
            group_policy: match cfg.group_policy {
                IMessageGroupPolicy::Open => GroupPolicy::Open,
                IMessageGroupPolicy::Allowlist => GroupPolicy::Allowlist,
                IMessageGroupPolicy::Disabled => GroupPolicy::Disabled,
            },
            allow_from: cfg.allow_from.clone(),
            group_allow_from: cfg.group_allow_from.clone(),
            require_mention: cfg.require_mention,
            bot_name: cfg.bot_name.clone(),
        }
    }
}

impl InboundMessageRouter {
    /// Create a new inbound message router (basic, without execution support)
    ///
    /// Use `with_execution()` for full execution capabilities.
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
            agent_router: None,
            workspace_manager: None,
            dedup_tracker: Mutex::new(InboundDedupTracker::new()),
            group_chat_orch: None,
            group_chat_executor: None,
            active_group_sessions: Mutex::new(HashMap::new()),
            intent_detector: None,
            llm_provider: None,

            command_parser: None,
            unified_classifier: None,
        }
    }

    /// Create a new inbound message router with full execution support
    ///
    /// This constructor enables the router to actually execute agents when
    /// messages arrive, rather than just logging what would happen.
    pub fn with_execution(
        channel_registry: Arc<ChannelRegistry>,
        pairing_store: Arc<dyn PairingStore>,
        config: RoutingConfig,
        agent_registry: Arc<AgentRegistry>,
        execution_adapter: Arc<dyn ExecutionAdapter>,
    ) -> Self {
        Self {
            channel_registry,
            pairing_store,
            config,
            channel_configs: HashMap::new(),
            agent_registry: Some(agent_registry),
            execution_adapter: Some(execution_adapter),
            agent_router: None,
            workspace_manager: None,
            dedup_tracker: Mutex::new(InboundDedupTracker::new()),
            group_chat_orch: None,
            group_chat_executor: None,
            active_group_sessions: Mutex::new(HashMap::new()),
            intent_detector: None,
            llm_provider: None,

            command_parser: None,
            unified_classifier: None,
        }
    }

    /// Create a new inbound message router with full execution support and unified routing
    ///
    /// This constructor enables:
    /// - Agent execution when messages arrive
    /// - Unified routing using AgentRouter bindings (same as WS agent.run)
    pub fn with_unified_routing(
        channel_registry: Arc<ChannelRegistry>,
        pairing_store: Arc<dyn PairingStore>,
        config: RoutingConfig,
        agent_registry: Arc<AgentRegistry>,
        execution_adapter: Arc<dyn ExecutionAdapter>,
        agent_router: Arc<AgentRouter>,
    ) -> Self {
        Self {
            channel_registry,
            pairing_store,
            config,
            channel_configs: HashMap::new(),
            agent_registry: Some(agent_registry),
            execution_adapter: Some(execution_adapter),
            agent_router: Some(agent_router),
            workspace_manager: None,
            dedup_tracker: Mutex::new(InboundDedupTracker::new()),
            group_chat_orch: None,
            group_chat_executor: None,
            active_group_sessions: Mutex::new(HashMap::new()),
            intent_detector: None,
            llm_provider: None,

            command_parser: None,
            unified_classifier: None,
        }
    }

    /// Set the agent router for unified routing
    ///
    /// When set, the router will use AgentRouter bindings to determine
    /// which agent handles each inbound message (same logic as WS agent.run).
    pub fn with_agent_router(mut self, router: Arc<AgentRouter>) -> Self {
        self.agent_router = Some(router);
        self
    }

    /// Set the workspace manager for channel-level active agent lookup
    ///
    /// When set, the router checks `channel_active_agent` after route bindings
    /// but before the default agent fallback, enabling per-channel agent switching.
    pub fn with_workspace_manager(mut self, manager: Arc<WorkspaceManager>) -> Self {
        self.workspace_manager = Some(manager);
        self
    }

    /// Set the unified intent classifier (v3 pipeline).
    pub fn set_unified_classifier(&mut self, classifier: UnifiedIntentClassifier) {
        self.unified_classifier = Some(classifier);
    }

    /// Enable group chat support.
    ///
    /// When set, the router intercepts `/groupchat` commands and routes
    /// messages from active group chat conversations to the orchestrator
    /// instead of the normal agent loop.
    pub fn with_group_chat(
        mut self,
        orch: SharedOrchestrator,
        executor: Arc<GroupChatExecutor>,
    ) -> Self {
        self.group_chat_orch = Some(orch);
        self.group_chat_executor = Some(executor);
        self
    }

    /// Set the intent detector for natural language agent switching
    pub fn with_intent_detector(mut self, detector: IntentDetector) -> Self {
        self.intent_detector = Some(detector);
        self
    }

    /// Set the LLM provider for intent classification and soul generation
    pub fn with_llm_provider(mut self, provider: Arc<dyn crate::providers::AiProvider>) -> Self {
        self.llm_provider = Some(provider);
        self
    }

    /// Set the command parser for dynamic slash command resolution
    ///
    /// Enables support for skills, MCP tools, and custom commands in addition
    /// to built-in slash commands. Without this, only built-in commands
    /// (/screenshot, /ocr, /search, /webfetch, /gen) are recognized.
    pub fn with_command_parser(mut self, parser: Arc<CommandParser>) -> Self {
        self.command_parser = Some(parser);
        self
    }

    /// Register channel-specific configuration
    pub fn register_channel_config(&mut self, channel_id: &str, config: ChannelConfig) {
        self.channel_configs.insert(channel_id.to_string(), config);
    }

    /// Start consuming inbound messages
    ///
    /// This takes ownership of the inbound receiver from ChannelRegistry.
    /// Returns a handle that can be used to stop the router.
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
            // Deduplication check: skip if we've already processed this message
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

        // Resolve agent ID using unified routing (async)
        let sender_id = msg.sender_id.as_str();
        let agent_id = self.resolve_agent_id_async(channel_id, sender_id).await;

        // Build context with resolved agent
        let ctx = self.build_context_with_agent(&msg, &agent_id);

        // Check permissions
        let ctx = match self.check_permission(ctx).await {
            Ok(ctx) => {
                info!("[Router] Permission granted for {}:{}", channel_id, ctx.sender_normalized);
                ctx
            }
            Err(e) => {
                info!("[Router] Permission denied for {}:{} — {}", channel_id, msg.sender_id.as_str(), e);
                return Ok(()); // Not an error, just filtered
            }
        };

        // Unified slash command interception
        // Resolves /switch, /groupchat, builtin, skill, MCP, and custom commands
        if ctx.message.text.trim().starts_with('/') {
            // Strip @botname suffix from Telegram group commands
            // e.g. "/gen@mybot some args" → "/gen some args"
            let slash_text = strip_bot_mention(ctx.message.text.trim());
            let slash_text = slash_text.as_str();

            // Try unified command resolution first (async, all sources)
            if let Some(ref parser) = self.command_parser {
                if let Some(parsed) = parser.parse_async(slash_text).await {
                    // Handle /switch internally
                    if parsed.command_name == "switch" {
                        if let Some(args) = &parsed.arguments {
                            return self.handle_switch_command(args.trim(), &msg, &ctx).await;
                        }
                    }
                    // Handle /groupchat internally
                    if parsed.command_name == "groupchat" {
                        return self.handle_groupchat_command(&msg).await;
                    }
                    // All other commands → execution engine via metadata
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

            // Fallback: /switch without unified registry
            if let Some(new_agent) = slash_text.strip_prefix("/switch ").map(|s| s.trim().to_string()) {
                if !new_agent.is_empty() {
                    return self.handle_switch_command(&new_agent, &msg, &ctx).await;
                }
            }

            // Fallback: /groupchat without unified registry
            if slash_text.starts_with("/groupchat") && self.group_chat_orch.is_some() {
                return self.handle_groupchat_command(&msg).await;
            }

            // Unrecognized slash command — fall through to normal message handling
        }

        // Natural language switch intent detection
        if let Some(result) = self.try_handle_switch_intent(&msg).await {
            return result;
        }

        // Group chat: check for active sessions (non-slash messages in active group chats)
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

    /// Handle /switch command: change active agent for this channel+peer
    async fn handle_switch_command(
        &self,
        agent_name: &str,
        msg: &InboundMessage,
        ctx: &InboundContext,
    ) -> Result<(), RoutingError> {
        let channel_id = ctx.message.channel_id.as_str();
        let sender_id = msg.sender_id.as_str();

        if let Some(ref manager) = self.workspace_manager {
            let agent_exists = if let Some(ref registry) = self.agent_registry {
                registry.get(agent_name).await.is_some()
            } else {
                false
            };

            let reply_text = if agent_exists {
                match manager.set_active_agent(channel_id, sender_id, agent_name) {
                    Ok(()) => {
                        info!("[Router] Switched agent for {}:{} -> {}", channel_id, sender_id, agent_name);
                        format!("✅ Switched to agent: {}", agent_name)
                    }
                    Err(e) => {
                        error!("[Router] Failed to switch agent: {}", e);
                        format!("❌ Failed to switch agent: {}", e)
                    }
                }
            } else {
                let available = if let Some(ref registry) = self.agent_registry {
                    registry.list().await.join(", ")
                } else {
                    "unknown".to_string()
                };
                format!("❌ Agent '{}' not found. Available: {}", agent_name, available)
            };

            let reply = OutboundMessage::text(msg.conversation_id.as_str(), reply_text);
            if let Err(e) = self.channel_registry.send(&msg.channel_id, reply).await {
                error!("[Router] Failed to send /switch reply: {}", e);
            }
        }
        Ok(())
    }

    /// Handle /groupchat command: dispatch to group chat orchestrator
    async fn handle_groupchat_command(
        &self,
        msg: &InboundMessage,
    ) -> Result<(), RoutingError> {
        if self.group_chat_orch.is_some() {
            if let Some(handled) = self.try_handle_group_chat(msg).await {
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
        Ok(())
    }

    /// Convert a ParsedCommand to IntentResult
    fn parsed_command_to_intent_result(&self, cmd: crate::command::ParsedCommand) -> IntentResult {
        use crate::command::CommandContext;

        let args = cmd.arguments.clone();

        let (tool_id, source) = match cmd.context {
            CommandContext::Builtin { tool_name } => (tool_name, DirectToolSource::SlashCommand),
            CommandContext::Skill { skill_id, .. } => (skill_id, DirectToolSource::Skill),
            CommandContext::Mcp {
                server_name,
                tool_name,
                ..
            } => {
                let id = tool_name.unwrap_or(server_name);
                (id, DirectToolSource::Mcp)
            }
            CommandContext::Custom { .. } => (cmd.command_name.clone(), DirectToolSource::Custom),
            CommandContext::None => (cmd.command_name.clone(), DirectToolSource::SlashCommand),
        };

        IntentResult::DirectTool {
            tool_id,
            args,
            source,
        }
    }

    /// Try to handle a switch intent from the message.
    /// Returns Some(Ok(())) if handled (message consumed), None if not a switch intent.
    async fn try_handle_switch_intent(
        &self,
        msg: &InboundMessage,
    ) -> Option<Result<(), RoutingError>> {
        let detector = self.intent_detector.as_ref()?;
        let manager = self.workspace_manager.as_ref()?;
        let registry = self.agent_registry.as_ref()?;

        let mut intent = detector.detect(&msg.text).await;

        // If LLM returned an id, try to resolve it against registered agents
        if let DetectedIntent::SwitchAgent { ref id, ref name, .. } = intent {
            if id.is_empty() {
                // LLM didn't provide an id — try name match against registered agents
                if let Some(matched_id) = registry.find_by_name(name).await {
                    info!("[Router] Resolved agent by name match: '{}' -> '{}'", name, matched_id);
                    let task = if let DetectedIntent::SwitchAgent { task, .. } = &intent { task.clone() } else { None };
                    intent = DetectedIntent::SwitchAgent {
                        id: matched_id,
                        name: name.clone(),
                        task,
                    };
                }
            }
        }

        match intent {
            DetectedIntent::SwitchAgent { ref id, ref name, ref task } if !id.is_empty() => {
                let channel_id = msg.channel_id.as_str();
                let sender_id = msg.sender_id.as_str();

                // Create agent dynamically if it doesn't exist
                if registry.get(id).await.is_none() {
                    info!("[Router] Agent '{}' not found, creating dynamically", id);

                    let soul_content = if let Some(ref provider) = self.llm_provider {
                        let prompt = build_soul_generation_prompt(id, name);
                        match provider.process(&prompt, None).await {
                            Ok(content) => content,
                            Err(e) => {
                                warn!("[Router] Failed to generate soul: {}, using default", e);
                                format!("You are {}, an AI assistant.", name)
                            }
                        }
                    } else {
                        format!("You are {}, an AI assistant.", name)
                    };

                    if let Err(e) = registry.create_dynamic(id, &soul_content, None).await {
                        let reply = OutboundMessage::text(
                            msg.conversation_id.as_str(),
                            format!("Failed to create agent '{}': {}", id, e),
                        );
                        let _ = self.channel_registry.send(&msg.channel_id, reply).await;
                        return Some(Ok(()));
                    }
                }

                // Switch active agent
                let switch_ok = match manager.set_active_agent(channel_id, sender_id, id) {
                    Ok(()) => {
                        info!("[Router] Switched agent for {}:{} -> {} ({})", channel_id, sender_id, id, name);
                        let reply = OutboundMessage::text(
                            msg.conversation_id.as_str(),
                            format!("✅ Switched to {} ({})", name, id),
                        );
                        if let Err(e) = self.channel_registry.send(&msg.channel_id, reply).await {
                            error!("[Router] Failed to send switch reply: {}", e);
                        }
                        true
                    }
                    Err(e) => {
                        error!("[Router] Failed to switch agent: {}", e);
                        let reply = OutboundMessage::text(
                            msg.conversation_id.as_str(),
                            format!("❌ Failed to switch: {}", e),
                        );
                        let _ = self.channel_registry.send(&msg.channel_id, reply).await;
                        false
                    }
                };

                // If switch succeeded and there's a trailing task, forward it to the new agent
                if switch_ok {
                    if let Some(task_text) = task {
                        if !task_text.is_empty() {
                            info!("[Router] Forwarding task to agent '{}': {}", id, task_text);
                            let mut task_msg = msg.clone();
                            task_msg.text = task_text.clone();
                            let ctx = self.build_context_with_agent(&task_msg, id);
                            if let Err(e) = self.execute_for_context(&ctx).await {
                                error!("[Router] Failed to execute forwarded task: {}", e);
                            }
                        }
                    }
                }

                Some(Ok(()))
            }
            _ => None,
        }
    }

    /// Build InboundContext from message with pre-resolved agent ID
    fn build_context_with_agent(&self, msg: &InboundMessage, agent_id: &str) -> InboundContext {
        let reply_route = ReplyRoute::new(
            msg.channel_id.clone(),
            msg.conversation_id.clone(),
        );

        let session_key = self.resolve_session_key_with_agent(msg, agent_id);

        let sender_normalized = if msg.channel_id.as_str() == "imessage" {
            normalize_phone(msg.sender_id.as_str())
        } else {
            msg.sender_id.as_str().to_string()
        };

        InboundContext::new(msg.clone(), reply_route, session_key)
            .with_sender_normalized(sender_normalized)
    }

    // =========================================================================
    // Group Chat
    // =========================================================================

    /// Try to handle the message as a group chat command or active session message.
    ///
    /// Returns:
    /// - `Some(Ok(()))` if the message was handled by group chat
    /// - `Some(Err(...))` if group chat handling failed
    /// - `None` if the message is not group-chat-related (proceed to normal agent)
    async fn try_handle_group_chat(
        &self,
        msg: &InboundMessage,
    ) -> Option<Result<(), String>> {
        let orch = self.group_chat_orch.as_ref()?;
        let executor = self.group_chat_executor.as_ref()?;

        let conversation_key = format!("{}:{}", msg.channel_id.as_str(), msg.conversation_id.as_str());
        let parser = DefaultGroupChatCommandParser;

        // 1. Check for /groupchat command
        if let Some(request) = parser.parse_group_chat_command(&msg.text) {
            match request {
                GroupChatRequest::Start { personas, topic, initial_message } => {
                    return Some(self.handle_group_chat_start(
                        orch, executor, msg, &conversation_key,
                        personas, topic, initial_message,
                    ).await);
                }
                GroupChatRequest::End { session_id } => {
                    return Some(self.handle_group_chat_end(
                        orch, msg, &conversation_key, &session_id,
                    ).await);
                }
                GroupChatRequest::Continue { session_id, message } => {
                    return Some(self.handle_group_chat_continue(
                        orch, executor, msg, &session_id, &message,
                    ).await);
                }
                GroupChatRequest::Mention { session_id, message, .. } => {
                    return Some(self.handle_group_chat_continue(
                        orch, executor, msg, &session_id, &message,
                    ).await);
                }
            }
        }

        // 2. Check if conversation has an active group chat session
        let session_id = {
            let sessions = self.active_group_sessions.lock().await;
            sessions.get(&conversation_key).cloned()
        };

        if let Some(session_id) = session_id {
            // Verify session is still active
            let session_handle = {
                let orch_guard = orch.lock().await;
                orch_guard.get_session(&session_id)
            };

            if let Some(handle) = session_handle {
                let is_active = {
                    let session = handle.lock().await;
                    session.status == GroupChatStatus::Active
                };

                if is_active {
                    return Some(self.handle_group_chat_continue(
                        orch, executor, msg, &session_id, &msg.text,
                    ).await);
                } else {
                    // Session ended, clean up tracking
                    let mut sessions = self.active_group_sessions.lock().await;
                    sessions.remove(&conversation_key);
                }
            } else {
                // Session gone, clean up tracking
                let mut sessions = self.active_group_sessions.lock().await;
                sessions.remove(&conversation_key);
            }
        }

        // Not a group chat message
        None
    }

    /// Handle `/groupchat start` command
    #[allow(clippy::too_many_arguments)]
    async fn handle_group_chat_start(
        &self,
        orch: &SharedOrchestrator,
        executor: &Arc<GroupChatExecutor>,
        msg: &InboundMessage,
        conversation_key: &str,
        personas: Vec<crate::group_chat::PersonaSource>,
        topic: String,
        initial_message: String,
    ) -> Result<(), String> {
        let channel_id = msg.channel_id.as_str().to_string();
        let conversation_id = msg.conversation_id.as_str().to_string();

        // Create session via orchestrator
        let (session_id, session_handle) = {
            let mut orch_guard = orch.lock().await;
            orch_guard.create_session(
                personas,
                if topic.is_empty() { None } else { Some(topic.clone()) },
                channel_id.clone(),
                conversation_key.to_string(),
            ).map_err(|e| e.to_string())?
        };

        // Track the active session for this conversation
        {
            let mut sessions = self.active_group_sessions.lock().await;
            sessions.insert(conversation_key.to_string(), session_id.clone());
        }

        // Send session start notification
        let participant_names: Vec<String> = {
            let session = session_handle.lock().await;
            session.participants.iter().map(|p| p.name.clone()).collect()
        };
        let topic_line = if topic.is_empty() {
            String::new()
        } else {
            format!("\nTopic: {}", topic)
        };
        let start_msg = format!(
            "🎭 Group chat started!\nParticipants: {}{}\n\nSend messages to continue, /groupchat end to finish.",
            participant_names.join(", "),
            topic_line,
        );
        let outbound = OutboundMessage::text(&conversation_id, start_msg);
        let _ = self.channel_registry.send(&msg.channel_id, outbound).await;

        // Execute first round if initial_message is not empty
        if !initial_message.is_empty() {
            let mut session = session_handle.lock().await;
            match executor.execute_round(&mut session, &initial_message).await {
                Ok(messages) => {
                    self.send_group_chat_messages(msg, &messages).await;
                }
                Err(e) => {
                    let err_msg = OutboundMessage::text(
                        &conversation_id,
                        format!("Round execution failed: {}", e),
                    );
                    let _ = self.channel_registry.send(&msg.channel_id, err_msg).await;
                }
            }
        }

        info!(
            subsystem = "group_chat",
            event = "session_started_via_channel",
            session_id = %session_id,
            channel = %channel_id,
            "group chat session started from channel"
        );

        Ok(())
    }

    /// Handle `/groupchat end` command
    async fn handle_group_chat_end(
        &self,
        orch: &SharedOrchestrator,
        msg: &InboundMessage,
        conversation_key: &str,
        session_id_hint: &str,
    ) -> Result<(), String> {
        // Determine which session to end: explicit session_id or the active one
        let session_id = if session_id_hint.is_empty() {
            let sessions = self.active_group_sessions.lock().await;
            sessions.get(conversation_key).cloned()
                .ok_or_else(|| "No active group chat in this conversation".to_string())?
        } else {
            session_id_hint.to_string()
        };

        // End the session
        let session_handle = {
            let orch_guard = orch.lock().await;
            orch_guard.get_session(&session_id)
        };

        if let Some(handle) = session_handle {
            let mut session = handle.lock().await;
            session.end();
        } else {
            return Err(format!("Session not found: {}", session_id));
        }

        // Remove from active tracking
        {
            let mut sessions = self.active_group_sessions.lock().await;
            sessions.remove(conversation_key);
        }

        // Send end notification
        let outbound = OutboundMessage::text(
            msg.conversation_id.as_str(),
            "🎭 Group chat ended.",
        );
        let _ = self.channel_registry.send(&msg.channel_id, outbound).await;

        info!(
            subsystem = "group_chat",
            event = "session_ended_via_channel",
            session_id = %session_id,
            "group chat session ended from channel"
        );

        Ok(())
    }

    /// Handle a continuation message for an active group chat session
    async fn handle_group_chat_continue(
        &self,
        orch: &SharedOrchestrator,
        executor: &Arc<GroupChatExecutor>,
        msg: &InboundMessage,
        session_id: &str,
        user_message: &str,
    ) -> Result<(), String> {
        let (session_handle, max_rounds) = {
            let orch_guard = orch.lock().await;
            let handle = orch_guard.get_session(session_id)
                .ok_or_else(|| format!("Session not found: {}", session_id))?;
            (handle, orch_guard.max_rounds())
        };

        let mut session = session_handle.lock().await;

        // Check round limit
        if session.current_round >= max_rounds {
            let conversation_key = format!("{}:{}", msg.channel_id.as_str(), msg.conversation_id.as_str());
            session.end();
            drop(session);
            {
                let mut sessions = self.active_group_sessions.lock().await;
                sessions.remove(&conversation_key);
            }
            let outbound = OutboundMessage::text(
                msg.conversation_id.as_str(),
                format!("🎭 Group chat ended (max {} rounds reached).", max_rounds),
            );
            let _ = self.channel_registry.send(&msg.channel_id, outbound).await;
            return Ok(());
        }

        // Execute the round
        match executor.execute_round(&mut session, user_message).await {
            Ok(messages) => {
                drop(session);
                self.send_group_chat_messages(msg, &messages).await;
            }
            Err(e) => {
                let err_msg = OutboundMessage::text(
                    msg.conversation_id.as_str(),
                    format!("Round failed: {}", e),
                );
                let _ = self.channel_registry.send(&msg.channel_id, err_msg).await;
            }
        }

        Ok(())
    }

    /// Send group chat persona messages back to the channel
    async fn send_group_chat_messages(
        &self,
        msg: &InboundMessage,
        messages: &[crate::group_chat::GroupChatMessage],
    ) {
        for gc_msg in messages {
            let text = format!("**[{}]**: {}", gc_msg.speaker.name(), gc_msg.content);
            let outbound = OutboundMessage::text(msg.conversation_id.as_str(), text);
            if let Err(e) = self.channel_registry.send(&msg.channel_id, outbound).await {
                error!("Failed to send group chat message: {}", e);
            }
        }
    }

    // =========================================================================
    // Agent Execution
    // =========================================================================

    /// Execute the agent for the given context
    ///
    /// This method:
    /// 1. Gets the agent from the registry
    /// 2. Generates a unique run ID
    /// 3. Creates a ReplyEmitter to route responses back to the channel
    /// 4. Builds a RunRequest with the message context
    /// 5. Spawns a non-blocking execution task
    ///
    /// If execution support is not configured (agent_registry or execution_adapter
    /// is None), this method logs a warning and returns Ok(()).
    async fn execute_for_context(&self, ctx: &InboundContext) -> Result<(), RoutingError> {
        // Check if execution support is configured
        let (agent_registry, execution_adapter) = match (
            self.agent_registry.as_ref(),
            self.execution_adapter.as_ref(),
        ) {
            (Some(ar), Some(ea)) => (ar.clone(), ea.clone()),
            _ => {
                // No execution support configured, log what would happen
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

        // Create a ReplyEmitter to route responses back to the channel
        let emitter = Arc::new(ReplyEmitter::new(
            self.channel_registry.clone(),
            ctx.reply_route.clone(),
            run_id.clone(),
        ));

        // Build the run request
        let mut metadata = HashMap::new();
        metadata.insert("channel_id".to_string(), ctx.message.channel_id.as_str().to_string());
        metadata.insert("sender_id".to_string(), ctx.sender_normalized.clone());
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

        info!(
            "Executing agent '{}' for session {} (run_id: {})",
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

    /// Execute the agent with additional metadata (e.g., slash command mode)
    ///
    /// Same as `execute_for_context` but injects extra metadata into the RunRequest.
    async fn execute_for_context_with_metadata(
        &self,
        ctx: &InboundContext,
        slash_command_mode: String,
    ) -> Result<(), RoutingError> {
        let (agent_registry, execution_adapter) = match (
            self.agent_registry.as_ref(),
            self.execution_adapter.as_ref(),
        ) {
            (Some(ar), Some(ea)) => (ar.clone(), ea.clone()),
            _ => {
                info!(
                    "Would execute slash command for session {} (execution not configured)",
                    ctx.session_key.to_key_string(),
                );
                return Ok(());
            }
        };

        let agent_id = ctx.session_key.agent_id();
        let agent = agent_registry.get(agent_id).await.ok_or_else(|| {
            RoutingError::AgentNotFound(agent_id.to_string())
        })?;

        let run_id = Uuid::new_v4().to_string();
        let emitter = Arc::new(ReplyEmitter::new(
            self.channel_registry.clone(),
            ctx.reply_route.clone(),
            run_id.clone(),
        ));

        let mut metadata = HashMap::new();
        metadata.insert("channel_id".to_string(), ctx.message.channel_id.as_str().to_string());
        metadata.insert("sender_id".to_string(), ctx.sender_normalized.clone());
        metadata.insert(SLASH_COMMAND_MODE_KEY.to_string(), slash_command_mode);
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

        info!(
            "Executing slash command for agent '{}' session {} (run_id: {})",
            agent_id,
            ctx.session_key.to_key_string(),
            run_id
        );

        tokio::spawn(async move {
            if let Err(e) = execution_adapter.execute(request, agent, emitter).await {
                error!("Slash command execution failed (run_id: {}): {}", run_id, e);
            }
        });

        Ok(())
    }

    /// Resolve SessionKey for a message with pre-resolved agent ID
    fn resolve_session_key_with_agent(&self, msg: &InboundMessage, agent_id: &str) -> SessionKey {
        let channel = msg.channel_id.as_str();

        if msg.is_group {
            // Group message -> isolate by conversation_id
            SessionKey::peer(
                agent_id,
                format!("{}:group:{}", channel, msg.conversation_id.as_str()),
            )
        } else {
            // DM -> based on dm_scope
            match self.config.dm_scope {
                DmScope::Main => SessionKey::main(agent_id),
                DmScope::PerPeer => SessionKey::peer(
                    agent_id,
                    format!("dm:{}", msg.sender_id.as_str()),
                ),
                DmScope::PerChannelPeer => SessionKey::peer(
                    agent_id,
                    format!("{}:dm:{}", channel, msg.sender_id.as_str()),
                ),
            }
        }
    }

    /// Resolve agent ID from channel using AgentRouter bindings and workspace manager (async)
    ///
    /// Resolution priority (highest to lowest):
    /// 1. User's explicit agent switch (WorkspaceManager) — user said "switch to X",
    ///    this MUST take precedence so agent_switch tool works as expected.
    ///    When user switches back to "main", the override is cleared (not set to "main"),
    ///    allowing lower-priority routes to take effect.
    /// 2. Config-layer route bindings (AgentRouter) — channel-to-agent mappings
    ///    from aleph.toml. Only used when no user override is active.
    /// 3. default_agent (RoutingConfig) — lowest priority fallback.
    async fn resolve_agent_id_async(&self, channel: &str, sender_id: &str) -> String {
        // 1. User's explicit agent switch (highest priority)
        if let Some(ref manager) = self.workspace_manager {
            if let Ok(Some(agent_id)) = manager.get_active_agent(channel, sender_id) {
                debug!(
                    "Using user-override agent '{}' for {}:{}",
                    agent_id, channel, sender_id
                );
                return agent_id;
            }
        }

        // 2. Config-layer route bindings
        if let Some(router) = &self.agent_router {
            let resolved = router.route(None, Some(channel), None).await;
            let resolved_id = resolved.agent_id();
            // Only use if it differs from the default (meaning a binding matched)
            if resolved_id != router.default_agent() {
                return resolved_id.to_string();
            }
        }

        // 3. Fall back to default agent
        self.config.default_agent.clone()
    }

    /// Check if message is permitted
    async fn check_permission(&self, mut ctx: InboundContext) -> Result<InboundContext, RoutingError> {
        let channel_id = ctx.message.channel_id.as_str();
        let channel_config = self
            .channel_configs
            .get(channel_id)
            .cloned()
            .unwrap_or_default();

        if ctx.message.is_group {
            // Group message permission check
            match channel_config.group_policy {
                GroupPolicy::Disabled => {
                    return Err(RoutingError::PermissionDenied(
                        "Group messages disabled".to_string(),
                    ));
                }
                GroupPolicy::Allowlist => {
                    let chat_id = ctx.message.conversation_id.as_str();
                    if !channel_config.group_allow_from.iter().any(|a| a == chat_id) {
                        return Err(RoutingError::PermissionDenied(
                            "Group not in allowlist".to_string(),
                        ));
                    }
                }
                GroupPolicy::Open => {
                    // Check mention requirement
                    if channel_config.require_mention {
                        let mentioned = self.check_mention(&ctx.message.text, &channel_config);
                        if !mentioned {
                            return Err(RoutingError::PermissionDenied(
                                "Mention required in group".to_string(),
                            ));
                        }
                        ctx = ctx.with_mention(true);
                    }
                }
            }
        } else {
            // DM permission check
            match channel_config.dm_policy {
                DmPolicy::Disabled => {
                    return Err(RoutingError::PermissionDenied(
                        "DMs disabled".to_string(),
                    ));
                }
                DmPolicy::Open => {
                    // Always allow
                }
                DmPolicy::Allowlist => {
                    if !self.is_in_allowlist(&ctx.sender_normalized, &channel_config.allow_from) {
                        return Err(RoutingError::PermissionDenied(
                            "Sender not in allowlist".to_string(),
                        ));
                    }
                }
                DmPolicy::Pairing => {
                    // Check allowlist first
                    if self.is_in_allowlist(&ctx.sender_normalized, &channel_config.allow_from) {
                        // Already approved via config
                    } else if self.pairing_store.is_approved(channel_id, &ctx.sender_normalized).await? {
                        // Approved via pairing
                    } else {
                        // Need pairing
                        self.send_pairing_request(&ctx).await?;
                        return Err(RoutingError::PermissionDenied(
                            "Pairing required".to_string(),
                        ));
                    }
                }
            }
        }

        ctx = ctx.authorize();
        Ok(ctx)
    }

    /// Check if sender is in allowlist
    fn is_in_allowlist(&self, sender: &str, allowlist: &[String]) -> bool {
        if allowlist.is_empty() {
            return false;
        }
        if allowlist.iter().any(|a| a == "*") {
            return true;
        }

        // Normalize both for comparison
        let sender_normalized = normalize_phone(sender);
        allowlist.iter().any(|a| {
            let allowed_normalized = normalize_phone(a);
            sender == a
                || sender.to_lowercase() == a.to_lowercase()
                || (!sender_normalized.is_empty()
                    && !allowed_normalized.is_empty()
                    && sender_normalized == allowed_normalized)
        })
    }

    /// Check if bot was mentioned in message
    fn check_mention(&self, text: &str, config: &ChannelConfig) -> bool {
        let text_lower = text.to_lowercase();

        // Check bot name
        if let Some(bot_name) = &config.bot_name {
            if text_lower.contains(&bot_name.to_lowercase()) {
                return true;
            }
        }

        // Check common patterns
        let patterns = ["@aleph", "@bot", "aleph"];
        patterns.iter().any(|p| text_lower.contains(p))
    }

    /// Send pairing request to unknown sender
    ///
    /// Always sends the pairing code message, even if the request already exists
    /// (the initial delivery may have failed due to channel not being connected).
    async fn send_pairing_request(&self, ctx: &InboundContext) -> Result<(), RoutingError> {
        let channel_id = ctx.message.channel_id.as_str();
        let sender_id = &ctx.sender_normalized;

        let mut metadata = HashMap::new();
        metadata.insert("sender_display".to_string(), ctx.message.sender_id.as_str().to_string());

        let (code, created) = self
            .pairing_store
            .upsert(channel_id, sender_id, metadata)
            .await?;

        if created {
            info!("Created new pairing request for {}:{} with code {}", channel_id, sender_id, code);
        } else {
            info!("Resending existing pairing code for {}:{}", channel_id, sender_id);
        }

        // Always send the pairing message (not just on first create)
        // because the initial delivery may have failed.
        let message = format!(
            "Hi! I'm Aleph, a personal AI assistant.\n\n\
            To chat with me, please have my owner approve your access.\n\n\
            Your ID: {}\n\
            Pairing code: {}\n\n\
            Once approved, just send me a message!",
            sender_id, code
        );

        let outbound = OutboundMessage::text(
            ctx.reply_route.conversation_id.as_str(),
            message,
        );

        if let Err(e) = self
            .channel_registry
            .send(&ctx.reply_route.channel_id, outbound)
            .await
        {
            warn!("Failed to send pairing message to {}:{}: {}", channel_id, sender_id, e);
        } else {
            info!("Sent pairing code {} to {}:{}", code, channel_id, sender_id);
        }

        Ok(())
    }
}

// BDD tests: core/tests/features/gateway/inbound_router.feature

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dedup_tracker_new_message() {
        let mut tracker = InboundDedupTracker::new();
        assert!(tracker.check_and_record("telegram:123"));
        assert_eq!(tracker.seen.len(), 1);
    }

    #[test]
    fn test_dedup_tracker_duplicate_blocked() {
        let mut tracker = InboundDedupTracker::new();
        assert!(tracker.check_and_record("telegram:123"));
        assert!(!tracker.check_and_record("telegram:123")); // duplicate
    }

    #[test]
    fn test_dedup_tracker_different_messages_allowed() {
        let mut tracker = InboundDedupTracker::new();
        assert!(tracker.check_and_record("telegram:123"));
        assert!(tracker.check_and_record("telegram:124"));
        assert!(tracker.check_and_record("discord:123")); // same msg_id, different channel
        assert_eq!(tracker.seen.len(), 3);
    }

    #[test]
    fn test_dedup_tracker_expire() {
        let mut tracker = InboundDedupTracker::new();
        // Insert an entry with a past timestamp
        let old_key = "telegram:old".to_string();
        tracker.seen.insert(old_key.clone());
        tracker.entries.push((old_key, Instant::now() - Duration::from_secs(600)));

        // Insert a fresh entry
        assert!(tracker.check_and_record("telegram:new"));

        // After expire, old entry should be gone
        assert_eq!(tracker.seen.len(), 1); // only "new" remains
        assert_eq!(tracker.entries.len(), 1);
    }
}
