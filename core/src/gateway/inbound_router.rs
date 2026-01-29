//! Inbound Message Router
//!
//! Consumes the ChannelRegistry's inbound message stream and routes
//! messages to the appropriate Agent/Session.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use super::agent_instance::AgentRegistry;
use super::channel::{InboundMessage, OutboundMessage};
use super::channel_registry::ChannelRegistry;
use super::execution_adapter::ExecutionAdapter;
use super::execution_engine::RunRequest;
use super::inbound_context::{InboundContext, ReplyRoute};
use super::pairing_store::{PairingError, PairingStore};
use super::reply_emitter::ReplyEmitter;
use super::router::SessionKey;
use super::routing_config::{DmScope, RoutingConfig};

#[cfg(target_os = "macos")]
use super::channels::imessage::normalize_phone;

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
impl From<&super::channels::imessage::IMessageConfig> for ChannelConfig {
    fn from(cfg: &super::channels::imessage::IMessageConfig) -> Self {
        use super::channels::imessage::{IMessageDmPolicy, IMessageGroupPolicy};

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
        }
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
        let rx = self.channel_registry.take_inbound_receiver().await?;

        let handle = tokio::spawn(async move {
            self.run_loop(rx).await;
        });

        Some(handle)
    }

    /// Main message processing loop
    async fn run_loop(self: Arc<Self>, mut rx: mpsc::Receiver<InboundMessage>) {
        info!("InboundMessageRouter started");

        while let Some(msg) = rx.recv().await {
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
        debug!(
            "Handling message from {}:{} - {}",
            channel_id,
            msg.sender_id.as_str(),
            &msg.text[..msg.text.len().min(50)]
        );

        // Build context
        let ctx = self.build_context(&msg);

        // Check permissions
        let ctx = match self.check_permission(ctx).await {
            Ok(ctx) => ctx,
            Err(e) => {
                debug!("Permission check failed: {}", e);
                return Ok(()); // Not an error, just filtered
            }
        };

        // Execute the agent for this context
        self.execute_for_context(&ctx).await?;

        Ok(())
    }

    /// Build InboundContext from message
    fn build_context(&self, msg: &InboundMessage) -> InboundContext {
        let reply_route = ReplyRoute::new(
            msg.channel_id.clone(),
            msg.conversation_id.clone(),
        );

        let session_key = self.resolve_session_key(msg);

        let sender_normalized = if msg.channel_id.as_str() == "imessage" {
            normalize_phone(msg.sender_id.as_str())
        } else {
            msg.sender_id.as_str().to_string()
        };

        InboundContext::new(msg.clone(), reply_route, session_key)
            .with_sender_normalized(sender_normalized)
    }

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
                    &ctx.message.text[..ctx.message.text.len().min(100)]
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

    /// Resolve SessionKey for a message
    fn resolve_session_key(&self, msg: &InboundMessage) -> SessionKey {
        let agent_id = &self.config.default_agent;
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
        let patterns = ["@aether", "@bot", "aether"];
        patterns.iter().any(|p| text_lower.contains(p))
    }

    /// Send pairing request to unknown sender
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
            // Send pairing message
            let message = format!(
                "Hi! I'm Aether, a personal AI assistant.\n\n\
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
                warn!("Failed to send pairing message: {}", e);
            } else {
                info!("Sent pairing request to {} with code {}", sender_id, code);
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use crate::gateway::channel::{ChannelId, ConversationId, MessageId, UserId};
    use crate::gateway::pairing_store::SqlitePairingStore;

    fn make_test_message(is_group: bool) -> InboundMessage {
        InboundMessage {
            id: MessageId::new("msg-1"),
            channel_id: ChannelId::new("imessage"),
            conversation_id: ConversationId::new(if is_group { "chat_id:42" } else { "+15551234567" }),
            sender_id: UserId::new("+15551234567"),
            sender_name: None,
            text: "Hello".to_string(),
            attachments: vec![],
            timestamp: Utc::now(),
            reply_to: None,
            is_group,
            raw: None,
        }
    }

    #[test]
    fn test_resolve_session_key_dm_per_peer() {
        let registry = Arc::new(ChannelRegistry::new());
        let store = Arc::new(SqlitePairingStore::in_memory().unwrap());
        let config = RoutingConfig::default().with_dm_scope(DmScope::PerPeer);

        let router = InboundMessageRouter::new(registry, store, config);

        let msg = make_test_message(false);
        let key = router.resolve_session_key(&msg);

        assert_eq!(key.to_key_string(), "agent:main:peer:dm:+15551234567");
    }

    #[test]
    fn test_resolve_session_key_dm_main() {
        let registry = Arc::new(ChannelRegistry::new());
        let store = Arc::new(SqlitePairingStore::in_memory().unwrap());
        let config = RoutingConfig::default().with_dm_scope(DmScope::Main);

        let router = InboundMessageRouter::new(registry, store, config);

        let msg = make_test_message(false);
        let key = router.resolve_session_key(&msg);

        assert_eq!(key.to_key_string(), "agent:main:main");
    }

    #[test]
    fn test_resolve_session_key_group() {
        let registry = Arc::new(ChannelRegistry::new());
        let store = Arc::new(SqlitePairingStore::in_memory().unwrap());
        let config = RoutingConfig::default();

        let router = InboundMessageRouter::new(registry, store, config);

        let msg = make_test_message(true);
        let key = router.resolve_session_key(&msg);

        assert_eq!(key.to_key_string(), "agent:main:peer:imessage:group:chat_id:42");
    }

    #[test]
    fn test_is_in_allowlist() {
        let registry = Arc::new(ChannelRegistry::new());
        let store = Arc::new(SqlitePairingStore::in_memory().unwrap());
        let config = RoutingConfig::default();

        let router = InboundMessageRouter::new(registry, store, config);

        let allowlist = vec!["+15551234567".to_string(), "user@example.com".to_string()];

        assert!(router.is_in_allowlist("+15551234567", &allowlist));
        assert!(router.is_in_allowlist("5551234567", &allowlist)); // Normalized
        assert!(router.is_in_allowlist("user@example.com", &allowlist));
        assert!(!router.is_in_allowlist("+19999999999", &allowlist));
    }

    #[test]
    fn test_is_in_allowlist_wildcard() {
        let registry = Arc::new(ChannelRegistry::new());
        let store = Arc::new(SqlitePairingStore::in_memory().unwrap());
        let config = RoutingConfig::default();

        let router = InboundMessageRouter::new(registry, store, config);

        let allowlist = vec!["*".to_string()];
        assert!(router.is_in_allowlist("+19999999999", &allowlist));
    }

    #[test]
    fn test_check_mention() {
        let registry = Arc::new(ChannelRegistry::new());
        let store = Arc::new(SqlitePairingStore::in_memory().unwrap());
        let config = RoutingConfig::default();

        let router = InboundMessageRouter::new(registry, store, config);

        let channel_config = ChannelConfig {
            bot_name: Some("MyBot".to_string()),
            ..Default::default()
        };

        assert!(router.check_mention("Hey @aether, help me", &channel_config));
        assert!(router.check_mention("MyBot can you help?", &channel_config));
        assert!(router.check_mention("Hello AETHER", &channel_config));
        assert!(!router.check_mention("Hello world", &channel_config));
    }
}
