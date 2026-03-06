//! Channel Registry - Central management for all channel instances
//!
//! The ChannelRegistry manages the lifecycle of all channels, routes messages,
//! and provides a unified interface for channel operations.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────┐
//! │                   ChannelRegistry                        │
//! │  ┌─────────────────────────────────────────────────┐    │
//! │  │              Channel Instances                   │    │
//! │  │  ┌─────────┐  ┌─────────┐  ┌─────────┐         │    │
//! │  │  │ iMessage│  │Telegram │  │  CLI    │         │    │
//! │  │  └────┬────┘  └────┬────┘  └────┬────┘         │    │
//! │  └───────┼────────────┼────────────┼───────────────┘    │
//! │          │            │            │                     │
//! │          └────────────┴────────────┘                     │
//! │                       │                                  │
//! │              Inbound Message Stream                      │
//! │                       │                                  │
//! │                       ▼                                  │
//! │              Gateway Event Bus                           │
//! └─────────────────────────────────────────────────────────┘
//! ```

use std::collections::HashMap;
use crate::sync_primitives::{Arc, Mutex};
use tokio::sync::{mpsc, RwLock};
use tracing::{error, info, warn};

use super::channel::{
    Channel, ChannelConfig, ChannelError, ChannelFactory, ChannelId, ChannelInfo,
    ChannelResult, ChannelStatus, ConversationId, InboundMessage, OutboundMessage, SendResult,
};

/// Type alias for a thread-safe, shareable channel handle
type ChannelHandle = Arc<RwLock<Box<dyn Channel>>>;

/// Central registry for all channel instances
pub struct ChannelRegistry {
    /// Registered channel instances
    channels: RwLock<HashMap<ChannelId, ChannelHandle>>,
    /// Channel factories by type
    factories: RwLock<HashMap<String, Arc<dyn ChannelFactory>>>,
    /// Unified inbound message sender
    inbound_tx: mpsc::Sender<InboundMessage>,
    /// Unified inbound message receiver (for consumers)
    inbound_rx: Arc<Mutex<Option<mpsc::Receiver<InboundMessage>>>>,
}

impl ChannelRegistry {
    /// Create a new channel registry
    pub fn new() -> Self {
        let (inbound_tx, inbound_rx) = mpsc::channel(1000);

        Self {
            channels: RwLock::new(HashMap::new()),
            factories: RwLock::new(HashMap::new()),
            inbound_tx,
            inbound_rx: Arc::new(Mutex::new(Some(inbound_rx))),
        }
    }

    /// Register a channel factory
    pub async fn register_factory(&self, factory: Arc<dyn ChannelFactory>) {
        let channel_type = factory.channel_type().to_string();
        let mut factories = self.factories.write().await;
        factories.insert(channel_type.clone(), factory);
        info!("Registered channel factory: {}", channel_type);
    }

    /// Create and register a channel from configuration
    pub async fn create_channel(&self, config: ChannelConfig) -> ChannelResult<ChannelId> {
        let factories = self.factories.read().await;
        let factory = factories
            .get(&config.channel_type)
            .ok_or_else(|| {
                ChannelError::ConfigError(format!(
                    "No factory registered for channel type: {}",
                    config.channel_type
                ))
            })?;

        let channel = factory.create(config.config.clone()).await?;
        let channel_id = channel.id().clone();

        drop(factories);

        let mut channels = self.channels.write().await;
        channels.insert(channel_id.clone(), Arc::new(RwLock::new(channel)));

        info!("Created channel: {} (type: {})", channel_id, config.channel_type);
        Ok(channel_id)
    }

    /// Register an existing channel instance
    pub async fn register(&self, channel: Box<dyn Channel>) -> ChannelId {
        let channel_id = channel.id().clone();
        let mut channels = self.channels.write().await;
        channels.insert(channel_id.clone(), Arc::new(RwLock::new(channel)));
        info!("Registered channel: {}", channel_id);
        channel_id
    }

    /// Unregister a channel
    pub async fn unregister(&self, channel_id: &ChannelId) -> Option<Box<dyn Channel>> {
        let mut channels = self.channels.write().await;
        if let Some(channel_arc) = channels.remove(channel_id) {
            // Try to extract the inner channel
            match Arc::try_unwrap(channel_arc) {
                Ok(rw_lock) => {
                    let channel = rw_lock.into_inner();
                    info!("Unregistered channel: {}", channel_id);
                    Some(channel)
                }
                Err(_) => {
                    warn!("Could not unregister channel {} - still in use", channel_id);
                    None
                }
            }
        } else {
            None
        }
    }

    /// Get channel by ID
    pub async fn get(&self, channel_id: &ChannelId) -> Option<ChannelHandle> {
        let channels = self.channels.read().await;
        channels.get(channel_id).cloned()
    }

    /// List all channels
    pub async fn list(&self) -> Vec<ChannelInfo> {
        let channels = self.channels.read().await;
        let mut infos = Vec::with_capacity(channels.len());

        for channel_arc in channels.values() {
            let channel = channel_arc.read().await;
            let mut info = channel.info().clone();
            info.status = channel.status(); // override with live status
            infos.push(info);
        }

        infos
    }

    /// List channels by type
    pub async fn list_by_type(&self, channel_type: &str) -> Vec<ChannelInfo> {
        let channels = self.channels.read().await;
        let mut infos = Vec::new();

        for channel_arc in channels.values() {
            let channel = channel_arc.read().await;
            if channel.channel_type() == channel_type {
                let mut info = channel.info().clone();
                info.status = channel.status();
                infos.push(info);
            }
        }

        infos
    }

    /// Start a channel
    pub async fn start_channel(&self, channel_id: &ChannelId) -> ChannelResult<()> {
        let channel_arc = self
            .get(channel_id)
            .await
            .ok_or_else(|| ChannelError::NotConnected(format!("Channel not found: {}", channel_id)))?;

        let mut channel = channel_arc.write().await;
        channel.start().await?;

        // Start forwarding inbound messages
        self.start_message_forwarder(channel_id.clone(), channel_arc.clone())
            .await;

        info!("Started channel: {}", channel_id);
        Ok(())
    }

    /// Stop a channel
    pub async fn stop_channel(&self, channel_id: &ChannelId) -> ChannelResult<()> {
        let channel_arc = self
            .get(channel_id)
            .await
            .ok_or_else(|| ChannelError::NotConnected(format!("Channel not found: {}", channel_id)))?;

        let mut channel = channel_arc.write().await;
        channel.stop().await?;

        info!("Stopped channel: {}", channel_id);
        Ok(())
    }

    /// Start all registered channels
    pub async fn start_all(&self) -> Vec<(ChannelId, ChannelResult<()>)> {
        let channels = self.channels.read().await;
        let channel_ids: Vec<ChannelId> = channels.keys().cloned().collect();
        drop(channels);

        let mut results = Vec::with_capacity(channel_ids.len());
        for channel_id in channel_ids {
            let result = self.start_channel(&channel_id).await;
            results.push((channel_id, result));
        }
        results
    }

    /// Stop all registered channels
    pub async fn stop_all(&self) -> Vec<(ChannelId, ChannelResult<()>)> {
        let channels = self.channels.read().await;
        let channel_ids: Vec<ChannelId> = channels.keys().cloned().collect();
        drop(channels);

        let mut results = Vec::with_capacity(channel_ids.len());
        for channel_id in channel_ids {
            let result = self.stop_channel(&channel_id).await;
            results.push((channel_id, result));
        }
        results
    }

    /// Send a message through a specific channel
    pub async fn send(
        &self,
        channel_id: &ChannelId,
        message: OutboundMessage,
    ) -> ChannelResult<SendResult> {
        let channel_arc = self
            .get(channel_id)
            .await
            .ok_or_else(|| ChannelError::NotConnected(format!("Channel not found: {}", channel_id)))?;

        let channel = channel_arc.read().await;
        if channel.status() == ChannelStatus::Disabled {
            return Err(ChannelError::NotConnected(format!(
                "Channel {} is disabled",
                channel_id
            )));
        }

        channel.send(message).await
    }

    /// Broadcast a message to all channels
    pub async fn broadcast(&self, message: OutboundMessage) -> Vec<(ChannelId, ChannelResult<SendResult>)> {
        let channels = self.channels.read().await;
        let mut results = Vec::with_capacity(channels.len());

        for (channel_id, channel_arc) in channels.iter() {
            let channel = channel_arc.read().await;
            if channel.status() != ChannelStatus::Disabled {
                let result = channel.send(message.clone()).await;
                results.push((channel_id.clone(), result));
            }
        }

        results
    }

    /// Take the inbound message receiver
    ///
    /// This can only be called once - subsequent calls return None.
    pub fn take_inbound_receiver(&self) -> Option<mpsc::Receiver<InboundMessage>> {
        let mut rx_guard = self.inbound_rx.lock().unwrap_or_else(|e| e.into_inner());
        rx_guard.take()
    }

    /// Get a clone of the inbound sender (for channel implementations)
    pub fn inbound_sender(&self) -> mpsc::Sender<InboundMessage> {
        self.inbound_tx.clone()
    }

    /// Start forwarding messages from a channel to the unified stream
    async fn start_message_forwarder(
        &self,
        channel_id: ChannelId,
        channel_arc: ChannelHandle,
    ) {
        let inbound_tx = self.inbound_tx.clone();

        tokio::spawn(async move {
            // Get the channel's inbound receiver
            let channel = channel_arc.write().await;
            let receiver = channel.inbound_receiver();
            drop(channel);

            if let Some(mut rx) = receiver {
                info!("[Forwarder] Channel {} forwarder started — receiver obtained", channel_id);
                while let Some(message) = rx.recv().await {
                    info!(
                        "[Forwarder] Forwarding message from channel {} (text: {:?})",
                        channel_id,
                        message.text.get(..50).unwrap_or(&message.text)
                    );
                    if let Err(e) = inbound_tx.send(message).await {
                        error!("Failed to forward message: {}", e);
                        break;
                    }
                }
            } else {
                warn!("[Forwarder] Channel {} inbound_receiver() returned None! Forwarder NOT started.", channel_id);
            }

            info!("[Forwarder] Channel {} forwarder stopped", channel_id);
        });
    }

    /// Find channels that can handle a conversation
    pub async fn find_channels_for_conversation(
        &self,
        conversation_id: &ConversationId,
    ) -> Vec<ChannelId> {
        // For now, return all connected channels
        // In the future, implement routing based on conversation metadata
        let channels = self.channels.read().await;
        let mut result = Vec::new();

        for (channel_id, channel_arc) in channels.iter() {
            let channel = channel_arc.read().await;
            if channel.status() == ChannelStatus::Connected {
                result.push(channel_id.clone());
            }
        }

        let _ = conversation_id; // Will be used for routing in future
        result
    }

    /// Get channel status summary
    pub async fn status_summary(&self) -> ChannelStatusSummary {
        let channels = self.channels.read().await;
        let mut summary = ChannelStatusSummary::default();

        for channel_arc in channels.values() {
            let channel = channel_arc.read().await;
            summary.total += 1;
            match channel.status() {
                ChannelStatus::Connected => summary.connected += 1,
                ChannelStatus::Connecting => summary.connecting += 1,
                ChannelStatus::Disconnected => summary.disconnected += 1,
                ChannelStatus::Error => summary.error += 1,
                ChannelStatus::Disabled => summary.disabled += 1,
            }
        }

        summary
    }
}

impl Default for ChannelRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Summary of channel statuses
#[derive(Debug, Clone, Default)]
pub struct ChannelStatusSummary {
    pub total: usize,
    pub connected: usize,
    pub connecting: usize,
    pub disconnected: usize,
    pub error: usize,
    pub disabled: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_registry_creation() {
        let registry = ChannelRegistry::new();
        let channels = registry.list().await;
        assert!(channels.is_empty());
    }

    #[tokio::test]
    async fn test_status_summary() {
        let registry = ChannelRegistry::new();
        let summary = registry.status_summary().await;
        assert_eq!(summary.total, 0);
        assert_eq!(summary.connected, 0);
    }
}
