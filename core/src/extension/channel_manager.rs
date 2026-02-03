//! Channel manager for plugin-provided messaging channels
//!
//! Manages the lifecycle of messaging channels provided by plugins (e.g., Telegram,
//! Discord, Slack). Each channel can receive incoming messages and send outgoing
//! messages through the plugin that registered it.
//!
//! # Architecture
//!
//! ```text
//! ChannelManager
//! ├── channels: HashMap<channel_key, ChannelHandle>
//! │   └── ChannelHandle
//! │       ├── info: ChannelInfo
//! │       ├── outgoing_tx: Sender<ChannelSendRequest>
//! │       └── incoming_rx: Option<Receiver<ChannelMessage>>
//! └── Methods
//!     ├── connect_channel()    - Connect a channel via plugin
//!     ├── disconnect_channel() - Disconnect a channel
//!     ├── list_channels()      - List all connected channels
//!     └── get_channel()        - Get info for a specific channel
//! ```
//!
//! # Usage
//!
//! ```rust,ignore
//! use aethecore::extension::{ChannelManager, PluginLoader};
//! use aethecore::extension::registry::ChannelRegistration;
//!
//! let mut manager = ChannelManager::new();
//! let mut loader = PluginLoader::new();
//!
//! // Connect a channel
//! let config = serde_json::json!({"bot_token": "..."});
//! let info = manager.connect_channel(&registration, config, &mut loader).await?;
//!
//! // Get channel handle for message passing
//! if let Some(handle) = manager.take_incoming_receiver("plugin-id", "telegram") {
//!     // Use handle.incoming_rx in gateway message loop
//! }
//!
//! // Send a message
//! if let Some(tx) = manager.get_outgoing_sender("plugin-id", "telegram") {
//!     tx.send(ChannelSendRequest { ... }).await?;
//! }
//!
//! // Disconnect
//! manager.disconnect_channel("plugin-id", "telegram", &mut loader).await?;
//! ```

use std::collections::HashMap;
use tokio::sync::mpsc;

use super::plugin_loader::PluginLoader;
use super::registry::ChannelRegistration;
use super::types::{ChannelInfo, ChannelMessage, ChannelSendRequest, ChannelState};
use super::{ExtensionError, ExtensionResult};

/// Buffer size for channel message queues
const CHANNEL_BUFFER_SIZE: usize = 256;

/// Channel handle for message passing
///
/// Contains the channel info and the message passing channels.
/// The outgoing sender is used to send messages to the channel,
/// and the incoming receiver is used to receive messages from the channel.
#[derive(Debug)]
pub struct ChannelHandle {
    /// Channel information
    pub info: ChannelInfo,
    /// Sender for outgoing messages (to plugin)
    pub outgoing_tx: mpsc::Sender<ChannelSendRequest>,
    /// Receiver for outgoing messages (plugin reads from this)
    outgoing_rx: Option<mpsc::Receiver<ChannelSendRequest>>,
    /// Sender for incoming messages (plugin writes to this)
    incoming_tx: Option<mpsc::Sender<ChannelMessage>>,
    /// Receiver for incoming messages (gateway reads from this)
    pub incoming_rx: Option<mpsc::Receiver<ChannelMessage>>,
}

impl ChannelHandle {
    /// Create a new channel handle with message queues
    fn new(info: ChannelInfo) -> Self {
        let (outgoing_tx, outgoing_rx) = mpsc::channel(CHANNEL_BUFFER_SIZE);
        let (incoming_tx, incoming_rx) = mpsc::channel(CHANNEL_BUFFER_SIZE);

        Self {
            info,
            outgoing_tx,
            outgoing_rx: Some(outgoing_rx),
            incoming_tx: Some(incoming_tx),
            incoming_rx: Some(incoming_rx),
        }
    }

    /// Take the outgoing receiver (for plugin to consume)
    ///
    /// This can only be called once - subsequent calls return None.
    pub fn take_outgoing_receiver(&mut self) -> Option<mpsc::Receiver<ChannelSendRequest>> {
        self.outgoing_rx.take()
    }

    /// Take the incoming sender (for plugin to produce)
    ///
    /// This can only be called once - subsequent calls return None.
    pub fn take_incoming_sender(&mut self) -> Option<mpsc::Sender<ChannelMessage>> {
        self.incoming_tx.take()
    }

    /// Take the incoming receiver (for gateway to consume)
    ///
    /// This can only be called once - subsequent calls return None.
    pub fn take_incoming_receiver(&mut self) -> Option<mpsc::Receiver<ChannelMessage>> {
        self.incoming_rx.take()
    }

    /// Get a clone of the outgoing sender
    pub fn outgoing_sender(&self) -> mpsc::Sender<ChannelSendRequest> {
        self.outgoing_tx.clone()
    }
}

/// Manages plugin-provided messaging channels
///
/// The ChannelManager handles the lifecycle of channels registered by plugins.
/// Each channel has:
/// - A unique key (plugin_id:channel_id)
/// - Connection state (disconnected, connecting, connected, etc.)
/// - Message queues for bidirectional communication
pub struct ChannelManager {
    /// Map of channel key -> channel handle
    /// Key format: "{plugin_id}:{channel_id}"
    channels: HashMap<String, ChannelHandle>,
}

impl ChannelManager {
    /// Create a new channel manager
    pub fn new() -> Self {
        Self {
            channels: HashMap::new(),
        }
    }

    /// Generate a channel key from plugin_id and channel_id
    fn channel_key(plugin_id: &str, channel_id: &str) -> String {
        format!("{}:{}", plugin_id, channel_id)
    }

    /// Connect a channel
    ///
    /// This method:
    /// 1. Creates message queues for the channel
    /// 2. Calls the plugin's connect handler
    /// 3. Returns the channel info on success
    ///
    /// # Arguments
    ///
    /// * `registration` - The channel registration from the plugin
    /// * `config` - Configuration for the channel (e.g., bot tokens, credentials)
    /// * `loader` - The plugin loader to call the connect handler
    ///
    /// # Returns
    ///
    /// * `Ok(ChannelInfo)` - The connected channel info
    /// * `Err(ExtensionError)` - If connection failed
    pub async fn connect_channel(
        &mut self,
        registration: &ChannelRegistration,
        config: serde_json::Value,
        loader: &mut PluginLoader,
    ) -> ExtensionResult<ChannelInfo> {
        let key = Self::channel_key(&registration.plugin_id, &registration.id);

        // Check if already connected
        if let Some(handle) = self.channels.get(&key) {
            if handle.info.state == ChannelState::Connected {
                return Ok(handle.info.clone());
            }
        }

        // Create initial channel info in connecting state
        let mut info = ChannelInfo {
            id: registration.id.clone(),
            plugin_id: registration.plugin_id.clone(),
            label: registration.label.clone(),
            state: ChannelState::Connecting,
            error: None,
        };

        // Create handle with message queues
        let handle = ChannelHandle::new(info.clone());
        self.channels.insert(key.clone(), handle);

        // Call plugin's connect handler
        // The handler name follows convention: "connect_{channel_id}"
        let handler = format!("connect_{}", registration.id);
        let result = loader.call_tool(&registration.plugin_id, &handler, config);

        match result {
            Ok(_) => {
                // Update state to connected
                if let Some(handle) = self.channels.get_mut(&key) {
                    handle.info.state = ChannelState::Connected;
                    handle.info.error = None;
                    info = handle.info.clone();
                }
                tracing::info!(
                    "Connected channel '{}' from plugin '{}'",
                    registration.id,
                    registration.plugin_id
                );
                Ok(info)
            }
            Err(e) => {
                // Update state to failed
                if let Some(handle) = self.channels.get_mut(&key) {
                    handle.info.state = ChannelState::Failed;
                    handle.info.error = Some(e.to_string());
                }
                tracing::warn!(
                    "Failed to connect channel '{}' from plugin '{}': {}",
                    registration.id,
                    registration.plugin_id,
                    e
                );
                Err(e)
            }
        }
    }

    /// Disconnect a channel
    ///
    /// This method:
    /// 1. Calls the plugin's disconnect handler
    /// 2. Removes the channel from tracking
    ///
    /// # Arguments
    ///
    /// * `plugin_id` - The ID of the plugin that registered the channel
    /// * `channel_id` - The ID of the channel to disconnect
    /// * `loader` - The plugin loader to call the disconnect handler
    ///
    /// # Returns
    ///
    /// * `Ok(())` - If disconnection succeeded
    /// * `Err(ExtensionError)` - If the channel was not found or disconnection failed
    pub async fn disconnect_channel(
        &mut self,
        plugin_id: &str,
        channel_id: &str,
        loader: &mut PluginLoader,
    ) -> ExtensionResult<()> {
        let key = Self::channel_key(plugin_id, channel_id);

        // Check if channel exists
        if !self.channels.contains_key(&key) {
            return Err(ExtensionError::PluginNotFound(format!(
                "Channel not found: {}",
                key
            )));
        }

        // Call plugin's disconnect handler
        let handler = format!("disconnect_{}", channel_id);
        let _ = loader.call_tool(plugin_id, &handler, serde_json::json!({}));

        // Remove from tracking (message queues will be dropped)
        self.channels.remove(&key);

        tracing::info!(
            "Disconnected channel '{}' from plugin '{}'",
            channel_id,
            plugin_id
        );

        Ok(())
    }

    /// List all channels
    ///
    /// Returns a list of all channel infos, regardless of connection state.
    pub fn list_channels(&self) -> Vec<ChannelInfo> {
        self.channels.values().map(|h| h.info.clone()).collect()
    }

    /// Get channel info
    ///
    /// # Arguments
    ///
    /// * `plugin_id` - The ID of the plugin that registered the channel
    /// * `channel_id` - The ID of the channel
    ///
    /// # Returns
    ///
    /// * `Some(ChannelInfo)` - If the channel exists
    /// * `None` - If the channel was not found
    pub fn get_channel(&self, plugin_id: &str, channel_id: &str) -> Option<ChannelInfo> {
        let key = Self::channel_key(plugin_id, channel_id);
        self.channels.get(&key).map(|h| h.info.clone())
    }

    /// Get channel handle (mutable access)
    ///
    /// Use this to access message queues for a channel.
    ///
    /// # Arguments
    ///
    /// * `plugin_id` - The ID of the plugin that registered the channel
    /// * `channel_id` - The ID of the channel
    ///
    /// # Returns
    ///
    /// * `Some(&mut ChannelHandle)` - If the channel exists
    /// * `None` - If the channel was not found
    pub fn get_channel_handle_mut(
        &mut self,
        plugin_id: &str,
        channel_id: &str,
    ) -> Option<&mut ChannelHandle> {
        let key = Self::channel_key(plugin_id, channel_id);
        self.channels.get_mut(&key)
    }

    /// Get outgoing sender for a channel
    ///
    /// Returns a clone of the sender that can be used to send messages to the channel.
    ///
    /// # Arguments
    ///
    /// * `plugin_id` - The ID of the plugin that registered the channel
    /// * `channel_id` - The ID of the channel
    ///
    /// # Returns
    ///
    /// * `Some(Sender)` - If the channel exists
    /// * `None` - If the channel was not found
    pub fn get_outgoing_sender(
        &self,
        plugin_id: &str,
        channel_id: &str,
    ) -> Option<mpsc::Sender<ChannelSendRequest>> {
        let key = Self::channel_key(plugin_id, channel_id);
        self.channels.get(&key).map(|h| h.outgoing_tx.clone())
    }

    /// Check if a channel is connected
    pub fn is_connected(&self, plugin_id: &str, channel_id: &str) -> bool {
        self.get_channel(plugin_id, channel_id)
            .map(|info| info.state == ChannelState::Connected)
            .unwrap_or(false)
    }

    /// Get the number of channels
    pub fn channel_count(&self) -> usize {
        self.channels.len()
    }

    /// Get the number of connected channels
    pub fn connected_count(&self) -> usize {
        self.channels
            .values()
            .filter(|h| h.info.state == ChannelState::Connected)
            .count()
    }

    /// Update channel state
    ///
    /// Used to update the state of a channel (e.g., when reconnecting).
    ///
    /// # Arguments
    ///
    /// * `plugin_id` - The ID of the plugin that registered the channel
    /// * `channel_id` - The ID of the channel
    /// * `state` - The new state
    /// * `error` - Optional error message (for failed state)
    pub fn update_state(
        &mut self,
        plugin_id: &str,
        channel_id: &str,
        state: ChannelState,
        error: Option<String>,
    ) {
        let key = Self::channel_key(plugin_id, channel_id);
        if let Some(handle) = self.channels.get_mut(&key) {
            handle.info.state = state;
            handle.info.error = error;
        }
    }

    /// Clear all channels
    ///
    /// Removes all channels from tracking. Does not call disconnect handlers.
    pub fn clear(&mut self) {
        self.channels.clear();
    }
}

impl Default for ChannelManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_channel_manager_new() {
        let manager = ChannelManager::new();
        assert_eq!(manager.channel_count(), 0);
        assert_eq!(manager.connected_count(), 0);
    }

    #[test]
    fn test_channel_manager_default() {
        let manager = ChannelManager::default();
        assert_eq!(manager.channel_count(), 0);
    }

    #[test]
    fn test_channel_key_generation() {
        let key = ChannelManager::channel_key("my-plugin", "telegram");
        assert_eq!(key, "my-plugin:telegram");
    }

    #[test]
    fn test_channel_handle_new() {
        let info = ChannelInfo {
            id: "telegram".to_string(),
            plugin_id: "telegram-plugin".to_string(),
            label: "Telegram Bot".to_string(),
            state: ChannelState::Disconnected,
            error: None,
        };

        let mut handle = ChannelHandle::new(info.clone());
        assert_eq!(handle.info.id, "telegram");
        assert!(handle.incoming_rx.is_some());
        assert!(handle.outgoing_rx.is_some());

        // Test taking receivers
        let incoming = handle.take_incoming_receiver();
        assert!(incoming.is_some());
        assert!(handle.incoming_rx.is_none());
        assert!(handle.take_incoming_receiver().is_none()); // Second call returns None
    }

    #[test]
    fn test_channel_handle_take_outgoing() {
        let info = ChannelInfo {
            id: "discord".to_string(),
            plugin_id: "discord-plugin".to_string(),
            label: "Discord Bot".to_string(),
            state: ChannelState::Disconnected,
            error: None,
        };

        let mut handle = ChannelHandle::new(info);
        let outgoing = handle.take_outgoing_receiver();
        assert!(outgoing.is_some());
        assert!(handle.outgoing_rx.is_none());
        assert!(handle.take_outgoing_receiver().is_none());
    }

    #[test]
    fn test_channel_handle_take_incoming_sender() {
        let info = ChannelInfo {
            id: "slack".to_string(),
            plugin_id: "slack-plugin".to_string(),
            label: "Slack Bot".to_string(),
            state: ChannelState::Disconnected,
            error: None,
        };

        let mut handle = ChannelHandle::new(info);
        let sender = handle.take_incoming_sender();
        assert!(sender.is_some());
        assert!(handle.incoming_tx.is_none());
        assert!(handle.take_incoming_sender().is_none());
    }

    #[test]
    fn test_get_channel_not_found() {
        let manager = ChannelManager::new();
        let result = manager.get_channel("nonexistent", "channel");
        assert!(result.is_none());
    }

    #[test]
    fn test_is_connected_false() {
        let manager = ChannelManager::new();
        assert!(!manager.is_connected("plugin", "channel"));
    }

    #[test]
    fn test_list_channels_empty() {
        let manager = ChannelManager::new();
        let channels = manager.list_channels();
        assert!(channels.is_empty());
    }

    #[test]
    fn test_clear() {
        let mut manager = ChannelManager::new();
        // Add a channel manually for testing
        let info = ChannelInfo {
            id: "test".to_string(),
            plugin_id: "test-plugin".to_string(),
            label: "Test Channel".to_string(),
            state: ChannelState::Connected,
            error: None,
        };
        let handle = ChannelHandle::new(info);
        manager
            .channels
            .insert("test-plugin:test".to_string(), handle);

        assert_eq!(manager.channel_count(), 1);
        manager.clear();
        assert_eq!(manager.channel_count(), 0);
    }

    #[test]
    fn test_update_state() {
        let mut manager = ChannelManager::new();
        // Add a channel manually for testing
        let info = ChannelInfo {
            id: "test".to_string(),
            plugin_id: "test-plugin".to_string(),
            label: "Test Channel".to_string(),
            state: ChannelState::Connected,
            error: None,
        };
        let handle = ChannelHandle::new(info);
        manager
            .channels
            .insert("test-plugin:test".to_string(), handle);

        // Update state
        manager.update_state(
            "test-plugin",
            "test",
            ChannelState::Failed,
            Some("Connection lost".to_string()),
        );

        let channel = manager.get_channel("test-plugin", "test").unwrap();
        assert_eq!(channel.state, ChannelState::Failed);
        assert_eq!(channel.error, Some("Connection lost".to_string()));
    }

    #[test]
    fn test_connected_count() {
        let mut manager = ChannelManager::new();

        // Add connected channel
        let info1 = ChannelInfo {
            id: "ch1".to_string(),
            plugin_id: "plugin".to_string(),
            label: "Channel 1".to_string(),
            state: ChannelState::Connected,
            error: None,
        };
        let handle1 = ChannelHandle::new(info1);
        manager.channels.insert("plugin:ch1".to_string(), handle1);

        // Add disconnected channel
        let info2 = ChannelInfo {
            id: "ch2".to_string(),
            plugin_id: "plugin".to_string(),
            label: "Channel 2".to_string(),
            state: ChannelState::Disconnected,
            error: None,
        };
        let handle2 = ChannelHandle::new(info2);
        manager.channels.insert("plugin:ch2".to_string(), handle2);

        assert_eq!(manager.channel_count(), 2);
        assert_eq!(manager.connected_count(), 1);
    }

    #[test]
    fn test_get_outgoing_sender() {
        let mut manager = ChannelManager::new();
        let info = ChannelInfo {
            id: "test".to_string(),
            plugin_id: "plugin".to_string(),
            label: "Test".to_string(),
            state: ChannelState::Connected,
            error: None,
        };
        let handle = ChannelHandle::new(info);
        manager.channels.insert("plugin:test".to_string(), handle);

        let sender = manager.get_outgoing_sender("plugin", "test");
        assert!(sender.is_some());

        // Non-existent channel returns None
        let none_sender = manager.get_outgoing_sender("plugin", "nonexistent");
        assert!(none_sender.is_none());
    }

    #[test]
    fn test_get_channel_handle_mut() {
        let mut manager = ChannelManager::new();
        let info = ChannelInfo {
            id: "test".to_string(),
            plugin_id: "plugin".to_string(),
            label: "Test".to_string(),
            state: ChannelState::Connected,
            error: None,
        };
        let handle = ChannelHandle::new(info);
        manager.channels.insert("plugin:test".to_string(), handle);

        // Get mutable handle and modify it
        let handle = manager.get_channel_handle_mut("plugin", "test");
        assert!(handle.is_some());

        // Take receivers from the handle
        let handle = handle.unwrap();
        let rx = handle.take_incoming_receiver();
        assert!(rx.is_some());

        // Non-existent channel returns None
        let none_handle = manager.get_channel_handle_mut("plugin", "nonexistent");
        assert!(none_handle.is_none());
    }

    #[tokio::test]
    async fn test_channel_message_flow() {
        let mut manager = ChannelManager::new();
        let info = ChannelInfo {
            id: "test".to_string(),
            plugin_id: "plugin".to_string(),
            label: "Test".to_string(),
            state: ChannelState::Connected,
            error: None,
        };
        let handle = ChannelHandle::new(info);
        manager.channels.insert("plugin:test".to_string(), handle);

        // Get handle and extract channels
        let handle = manager.get_channel_handle_mut("plugin", "test").unwrap();
        let incoming_tx = handle.take_incoming_sender().unwrap();
        let mut incoming_rx = handle.take_incoming_receiver().unwrap();

        // Simulate plugin sending a message
        let message = ChannelMessage {
            channel_id: "test".to_string(),
            conversation_id: "conv-123".to_string(),
            sender_id: "user-456".to_string(),
            content: "Hello from plugin".to_string(),
            timestamp: chrono::Utc::now(),
            metadata: None,
        };
        incoming_tx.send(message.clone()).await.unwrap();

        // Receive the message in gateway
        let received = incoming_rx.recv().await.unwrap();
        assert_eq!(received.content, "Hello from plugin");
        assert_eq!(received.conversation_id, "conv-123");
    }

    #[tokio::test]
    async fn test_outgoing_message_flow() {
        let mut manager = ChannelManager::new();
        let info = ChannelInfo {
            id: "test".to_string(),
            plugin_id: "plugin".to_string(),
            label: "Test".to_string(),
            state: ChannelState::Connected,
            error: None,
        };
        let handle = ChannelHandle::new(info);
        manager.channels.insert("plugin:test".to_string(), handle);

        // Get sender and receiver
        let outgoing_tx = manager.get_outgoing_sender("plugin", "test").unwrap();
        let handle = manager.get_channel_handle_mut("plugin", "test").unwrap();
        let mut outgoing_rx = handle.take_outgoing_receiver().unwrap();

        // Gateway sends a message
        let request = ChannelSendRequest {
            conversation_id: "conv-123".to_string(),
            content: "Hello from gateway".to_string(),
            reply_to: None,
            metadata: None,
        };
        outgoing_tx.send(request.clone()).await.unwrap();

        // Plugin receives the message
        let received = outgoing_rx.recv().await.unwrap();
        assert_eq!(received.content, "Hello from gateway");
        assert_eq!(received.conversation_id, "conv-123");
    }
}
