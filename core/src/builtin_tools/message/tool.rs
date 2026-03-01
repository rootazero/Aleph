//! Message Tool Implementation
//!
//! The agent-facing tool that dispatches message operations to channel adapters.

use std::collections::HashMap;
use crate::sync_primitives::{Arc, RwLock};

use async_trait::async_trait;
use tracing::{debug, warn};

use crate::error::Result;
use crate::tools::AlephTool;

use super::types::{
    ChannelCapabilities, DeleteParams, EditParams, MessageAction, MessageOperations,
    MessageResult, MessageToolArgs, MessageToolOutput, ReactParams, ReplyParams, SendParams,
};
use super::super::notify_tool_start;

/// Registry of channel message operation adapters
pub struct MessageOperationsRegistry {
    /// Registered adapters by channel ID
    adapters: RwLock<HashMap<String, Arc<dyn MessageOperations>>>,
}

impl Default for MessageOperationsRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl MessageOperationsRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            adapters: RwLock::new(HashMap::new()),
        }
    }

    /// Register a channel adapter
    pub fn register(&self, adapter: Arc<dyn MessageOperations>) {
        let channel_id = adapter.channel_id().to_string();
        debug!(channel_id = %channel_id, "Registering message operations adapter");
        self.adapters.write().unwrap().insert(channel_id, adapter);
    }

    /// Get an adapter by channel ID
    pub fn get(&self, channel_id: &str) -> Option<Arc<dyn MessageOperations>> {
        self.adapters.read().unwrap().get(channel_id).cloned()
    }

    /// List all registered channel IDs
    pub fn list_channels(&self) -> Vec<String> {
        self.adapters.read().unwrap().keys().cloned().collect()
    }

    /// Get capabilities for a channel
    pub fn capabilities(&self, channel_id: &str) -> Option<ChannelCapabilities> {
        self.adapters.read().unwrap().get(channel_id).map(|a| a.capabilities())
    }
}

/// Message tool for cross-channel message operations
///
/// This tool dispatches message actions to the appropriate channel adapter
/// based on the target channel specified in the arguments.
#[derive(Clone)]
pub struct MessageTool {
    /// Registry of channel adapters
    registry: Arc<MessageOperationsRegistry>,
    /// Default channel to use when not specified
    default_channel: Option<String>,
}

impl MessageTool {
    /// Tool identifier
    pub const NAME: &'static str = "message";

    /// Tool description for AI prompt
    pub const DESCRIPTION: &'static str = "Perform message operations on messaging channels. \
        Supports: reply (respond to a message), edit (modify existing message), \
        react (add/remove emoji reaction), delete (remove message), send (new message). \
        Specify the target channel and message details.";

    /// Create a new message tool with a registry
    pub fn new(registry: Arc<MessageOperationsRegistry>) -> Self {
        Self {
            registry,
            default_channel: None,
        }
    }

    /// Create a new message tool with a registry and default channel
    pub fn with_default_channel(
        registry: Arc<MessageOperationsRegistry>,
        default_channel: impl Into<String>,
    ) -> Self {
        Self {
            registry,
            default_channel: Some(default_channel.into()),
        }
    }

    /// Set the default channel
    pub fn set_default_channel(&mut self, channel: impl Into<String>) {
        self.default_channel = Some(channel.into());
    }

    /// Get the registry
    pub fn registry(&self) -> &Arc<MessageOperationsRegistry> {
        &self.registry
    }

    /// Parse target string into (channel, conversation_id)
    ///
    /// Supports formats:
    /// - "channel:conversation_id" (explicit channel)
    /// - "conversation_id" (uses default channel)
    fn parse_target(&self, target: &str, explicit_channel: Option<&str>) -> (String, String) {
        // Explicit channel takes precedence
        if let Some(channel) = explicit_channel {
            return (channel.to_string(), target.to_string());
        }

        // Try to parse "channel:conversation_id" format
        if let Some((channel, conv_id)) = target.split_once(':') {
            // Verify the channel exists in registry
            if self.registry.get(channel).is_some() {
                return (channel.to_string(), conv_id.to_string());
            }
        }

        // Use default channel or first available
        let channel = self
            .default_channel
            .clone()
            .or_else(|| self.registry.list_channels().into_iter().next())
            .unwrap_or_else(|| "unknown".to_string());

        (channel, target.to_string())
    }

    /// Execute the tool (internal implementation)
    async fn call_impl(&self, args: MessageToolArgs) -> MessageToolOutput {
        notify_tool_start(
            Self::NAME,
            &format!("{}:{}", args.action, args.target),
        );

        // Parse channel and target
        let (channel, conversation_id) = self.parse_target(&args.target, args.channel.as_deref());

        // Get the adapter
        let adapter = match self.registry.get(&channel) {
            Some(a) => a,
            None => {
                return MessageToolOutput::failed(
                    args.action,
                    &channel,
                    format!("No message adapter registered for channel '{}'", channel),
                );
            }
        };

        // Check capability
        let capabilities = adapter.capabilities();
        if !capabilities.supports(args.action) {
            return MessageToolOutput::unsupported(args.action, &channel);
        }

        // Dispatch to appropriate action
        let result = match args.action {
            MessageAction::Reply => {
                let message_id = match &args.message_id {
                    Some(id) => id.clone(),
                    None => {
                        return MessageToolOutput::failed(
                            args.action,
                            &channel,
                            "message_id is required for reply action",
                        );
                    }
                };
                let text = match &args.text {
                    Some(t) => t.clone(),
                    None => {
                        return MessageToolOutput::failed(
                            args.action,
                            &channel,
                            "text is required for reply action",
                        );
                    }
                };

                adapter
                    .reply(ReplyParams {
                        channel: channel.clone(),
                        conversation_id,
                        message_id,
                        text,
                    })
                    .await
            }

            MessageAction::Edit => {
                let message_id = match &args.message_id {
                    Some(id) => id.clone(),
                    None => {
                        return MessageToolOutput::failed(
                            args.action,
                            &channel,
                            "message_id is required for edit action",
                        );
                    }
                };
                let text = match &args.text {
                    Some(t) => t.clone(),
                    None => {
                        return MessageToolOutput::failed(
                            args.action,
                            &channel,
                            "text is required for edit action",
                        );
                    }
                };

                adapter
                    .edit(EditParams {
                        channel: channel.clone(),
                        conversation_id,
                        message_id,
                        text,
                    })
                    .await
            }

            MessageAction::React => {
                let message_id = match &args.message_id {
                    Some(id) => id.clone(),
                    None => {
                        return MessageToolOutput::failed(
                            args.action,
                            &channel,
                            "message_id is required for react action",
                        );
                    }
                };
                let emoji = match &args.emoji {
                    Some(e) => e.clone(),
                    None => {
                        return MessageToolOutput::failed(
                            args.action,
                            &channel,
                            "emoji is required for react action",
                        );
                    }
                };

                adapter
                    .react(ReactParams {
                        channel: channel.clone(),
                        conversation_id,
                        message_id,
                        emoji,
                        remove: args.remove,
                    })
                    .await
            }

            MessageAction::Delete => {
                let message_id = match &args.message_id {
                    Some(id) => id.clone(),
                    None => {
                        return MessageToolOutput::failed(
                            args.action,
                            &channel,
                            "message_id is required for delete action",
                        );
                    }
                };

                adapter
                    .delete(DeleteParams {
                        channel: channel.clone(),
                        conversation_id,
                        message_id,
                    })
                    .await
            }

            MessageAction::Send => {
                let text = match &args.text {
                    Some(t) => t.clone(),
                    None => {
                        return MessageToolOutput::failed(
                            args.action,
                            &channel,
                            "text is required for send action",
                        );
                    }
                };

                adapter
                    .send(SendParams {
                        channel: channel.clone(),
                        target: conversation_id,
                        text,
                    })
                    .await
            }
        };

        // Convert result to output
        match result {
            Ok(MessageResult { success: true, message_id, .. }) => {
                if let Some(id) = message_id {
                    MessageToolOutput::success_with_id(args.action, &channel, id)
                } else {
                    MessageToolOutput::success(args.action, &channel)
                }
            }
            Ok(MessageResult { error: Some(err), .. }) => {
                MessageToolOutput::failed(args.action, &channel, err)
            }
            Ok(_) => {
                MessageToolOutput::failed(args.action, &channel, "Operation failed")
            }
            Err(e) => {
                warn!(
                    action = %args.action,
                    channel = %channel,
                    error = %e,
                    "Message tool action failed"
                );
                MessageToolOutput::failed(args.action, &channel, e.to_string())
            }
        }
    }
}

#[async_trait]
impl AlephTool for MessageTool {
    const NAME: &'static str = "message";
    const DESCRIPTION: &'static str = "Perform message operations on messaging channels. \
        Supports: reply (respond to a message), edit (modify existing message), \
        react (add/remove emoji reaction), delete (remove message), send (new message). \
        Specify the target channel and message details.";

    type Args = MessageToolArgs;
    type Output = MessageToolOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        Ok(self.call_impl(args).await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Mock adapter for testing
    struct MockAdapter {
        channel_id: String,
        capabilities: ChannelCapabilities,
    }

    impl MockAdapter {
        fn new(channel_id: &str, capabilities: ChannelCapabilities) -> Self {
            Self {
                channel_id: channel_id.to_string(),
                capabilities,
            }
        }
    }

    #[async_trait]
    impl MessageOperations for MockAdapter {
        fn channel_id(&self) -> &str {
            &self.channel_id
        }

        fn capabilities(&self) -> ChannelCapabilities {
            self.capabilities.clone()
        }

        async fn reply(&self, _params: ReplyParams) -> Result<MessageResult> {
            Ok(MessageResult::success_with_id("reply-123"))
        }

        async fn edit(&self, _params: EditParams) -> Result<MessageResult> {
            Ok(MessageResult::success_with_id("edit-456"))
        }

        async fn react(&self, _params: ReactParams) -> Result<MessageResult> {
            Ok(MessageResult::success())
        }

        async fn delete(&self, _params: DeleteParams) -> Result<MessageResult> {
            Ok(MessageResult::success())
        }

        async fn send(&self, _params: SendParams) -> Result<MessageResult> {
            Ok(MessageResult::success_with_id("send-789"))
        }
    }

    #[test]
    fn test_registry_operations() {
        let registry = MessageOperationsRegistry::new();

        // Initially empty
        assert!(registry.list_channels().is_empty());
        assert!(registry.get("telegram").is_none());

        // Register adapter
        let adapter = Arc::new(MockAdapter::new("telegram", ChannelCapabilities::all()));
        registry.register(adapter);

        // Now available
        assert!(registry.get("telegram").is_some());
        assert_eq!(registry.list_channels(), vec!["telegram"]);

        // Capabilities
        let caps = registry.capabilities("telegram").unwrap();
        assert!(caps.supports(MessageAction::Reply));
    }

    #[test]
    fn test_parse_target_explicit_channel() {
        let registry = Arc::new(MessageOperationsRegistry::new());
        let tool = MessageTool::new(registry);

        let (channel, conv_id) = tool.parse_target("chat123", Some("telegram"));
        assert_eq!(channel, "telegram");
        assert_eq!(conv_id, "chat123");
    }

    #[test]
    fn test_parse_target_channel_prefix() {
        let registry = Arc::new(MessageOperationsRegistry::new());
        let adapter = Arc::new(MockAdapter::new("telegram", ChannelCapabilities::all()));
        registry.register(adapter);

        let tool = MessageTool::new(registry);

        let (channel, conv_id) = tool.parse_target("telegram:chat123", None);
        assert_eq!(channel, "telegram");
        assert_eq!(conv_id, "chat123");
    }

    #[test]
    fn test_parse_target_default_channel() {
        let registry = Arc::new(MessageOperationsRegistry::new());
        let tool = MessageTool::with_default_channel(registry, "discord");

        let (channel, conv_id) = tool.parse_target("chat123", None);
        assert_eq!(channel, "discord");
        assert_eq!(conv_id, "chat123");
    }

    #[tokio::test]
    async fn test_reply_action() {
        let registry = Arc::new(MessageOperationsRegistry::new());
        let adapter = Arc::new(MockAdapter::new("telegram", ChannelCapabilities::all()));
        registry.register(adapter);

        let tool = MessageTool::new(registry);
        let args = MessageToolArgs {
            action: MessageAction::Reply,
            channel: Some("telegram".to_string()),
            target: "chat123".to_string(),
            message_id: Some("msg456".to_string()),
            text: Some("Hello!".to_string()),
            emoji: None,
            remove: false,
        };

        let output = tool.call(args).await.unwrap();
        assert!(output.success);
        assert_eq!(output.action, MessageAction::Reply);
        assert_eq!(output.channel, "telegram");
        assert_eq!(output.message_id, Some("reply-123".to_string()));
    }

    #[tokio::test]
    async fn test_reply_missing_message_id() {
        let registry = Arc::new(MessageOperationsRegistry::new());
        let adapter = Arc::new(MockAdapter::new("telegram", ChannelCapabilities::all()));
        registry.register(adapter);

        let tool = MessageTool::new(registry);
        let args = MessageToolArgs {
            action: MessageAction::Reply,
            channel: Some("telegram".to_string()),
            target: "chat123".to_string(),
            message_id: None, // Missing
            text: Some("Hello!".to_string()),
            emoji: None,
            remove: false,
        };

        let output = tool.call(args).await.unwrap();
        assert!(!output.success);
        assert!(output.error.unwrap().contains("message_id is required"));
    }

    #[tokio::test]
    async fn test_unsupported_action() {
        let registry = Arc::new(MessageOperationsRegistry::new());
        // iMessage-like capabilities (no edit)
        let adapter = Arc::new(MockAdapter::new(
            "imessage",
            ChannelCapabilities {
                reply: true,
                edit: false,
                react: true,
                delete: false,
                send: true,
            },
        ));
        registry.register(adapter);

        let tool = MessageTool::new(registry);
        let args = MessageToolArgs {
            action: MessageAction::Edit,
            channel: Some("imessage".to_string()),
            target: "chat123".to_string(),
            message_id: Some("msg456".to_string()),
            text: Some("New text".to_string()),
            emoji: None,
            remove: false,
        };

        let output = tool.call(args).await.unwrap();
        assert!(!output.success);
        assert!(output.error.unwrap().contains("not supported"));
    }

    #[tokio::test]
    async fn test_unknown_channel() {
        let registry = Arc::new(MessageOperationsRegistry::new());
        let tool = MessageTool::new(registry);

        let args = MessageToolArgs {
            action: MessageAction::Send,
            channel: Some("unknown".to_string()),
            target: "chat123".to_string(),
            message_id: None,
            text: Some("Hello!".to_string()),
            emoji: None,
            remove: false,
        };

        let output = tool.call(args).await.unwrap();
        assert!(!output.success);
        assert!(output.error.unwrap().contains("No message adapter registered"));
    }

    #[tokio::test]
    async fn test_send_action() {
        let registry = Arc::new(MessageOperationsRegistry::new());
        let adapter = Arc::new(MockAdapter::new("discord", ChannelCapabilities::all()));
        registry.register(adapter);

        let tool = MessageTool::new(registry);
        let args = MessageToolArgs {
            action: MessageAction::Send,
            channel: Some("discord".to_string()),
            target: "channel123".to_string(),
            message_id: None,
            text: Some("New message".to_string()),
            emoji: None,
            remove: false,
        };

        let output = tool.call(args).await.unwrap();
        assert!(output.success);
        assert_eq!(output.action, MessageAction::Send);
        assert_eq!(output.message_id, Some("send-789".to_string()));
    }

    #[tokio::test]
    async fn test_react_action() {
        let registry = Arc::new(MessageOperationsRegistry::new());
        let adapter = Arc::new(MockAdapter::new("telegram", ChannelCapabilities::all()));
        registry.register(adapter);

        let tool = MessageTool::new(registry);
        let args = MessageToolArgs {
            action: MessageAction::React,
            channel: Some("telegram".to_string()),
            target: "chat123".to_string(),
            message_id: Some("msg456".to_string()),
            text: None,
            emoji: Some("👍".to_string()),
            remove: false,
        };

        let output = tool.call(args).await.unwrap();
        assert!(output.success);
        assert_eq!(output.action, MessageAction::React);
    }
}
