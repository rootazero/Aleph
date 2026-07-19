//! Channel Registry - Central management for all channel instances
//!
//! The `ChannelRegistry` manages the lifecycle of all channels, routes messages,
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

use crate::sync_primitives::{Arc, Mutex};
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::{broadcast, RwLock};
use tracing::{error, info, warn};

use super::channel::{
    Channel, ChannelCapabilities, ChannelConfig, ChannelError, ChannelFactory, ChannelHealth,
    ChannelId, ChannelInfo, ChannelResult, ChannelStatus, ConversationId, HealthStatus,
    InboundMessage, InboundMessageSender, OutboundMessage, SendResult,
};
use super::voice::VoiceState;

/// Type alias for a thread-safe, shareable channel handle
type ChannelHandle = Arc<RwLock<Box<dyn Channel>>>;

/// Policy for retrying transient outbound sends at the registry layer.
///
/// Deliberately scoped to [`ChannelError::RateLimited`]: a channel returns that
/// variant when an upstream `429` rejected the message **before** it was
/// delivered, so honoring the server-provided `retry_after_secs` and retrying
/// is *duplicate-safe*. Every other [`ChannelError`] is treated as terminal and
/// propagates immediately — in particular [`ChannelError::SendFailed`] is
/// ambiguous (the message may already be on the wire) and must never be retried
/// here.
///
/// Before this existed, only Telegram retried rate limits (inside its own
/// delivery loop); msteams / feishu / signal surfaced `RateLimited` to a caller
/// — the reply path — that simply dropped it, losing the reply. Centralizing the
/// wait here gives every channel consistent, bounded back-pressure.
#[derive(Debug, Clone)]
pub struct SendRetryPolicy {
    /// Maximum number of retry-after waits before giving up. `0` preserves the
    /// historical fire-once behavior (the `RateLimited` error propagates).
    pub max_rate_limit_retries: u32,
    /// Upper bound on a single retry-after wait, so a hostile or buggy
    /// `retry_after_secs` cannot wedge the send path for an unbounded time.
    pub max_retry_after: Duration,
}

impl Default for SendRetryPolicy {
    fn default() -> Self {
        Self {
            max_rate_limit_retries: 2,
            max_retry_after: Duration::from_secs(30),
        }
    }
}

/// TOML-facing tuning for the outbound rate-limit retry policy
/// (`[gateway.send_retry]`).
///
/// Mirrors [`SendRetryPolicy`] but uses plain seconds so it round-trips through
/// TOML (the runtime struct stores a [`Duration`]). An empty `[gateway.send_retry]`
/// table — or none at all — is byte-identical to [`SendRetryPolicy::default`].
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct SendRetryTomlConfig {
    /// Maximum retry-after waits before giving up. `0` restores the historical
    /// fire-once behavior (the `RateLimited` error propagates immediately).
    pub max_rate_limit_retries: u32,
    /// Upper bound (seconds) on a single honored `retry_after`, so a hostile or
    /// buggy upstream value cannot wedge the send path indefinitely.
    pub max_retry_after_secs: u64,
}

impl Default for SendRetryTomlConfig {
    fn default() -> Self {
        let p = SendRetryPolicy::default();
        Self {
            max_rate_limit_retries: p.max_rate_limit_retries,
            max_retry_after_secs: p.max_retry_after.as_secs(),
        }
    }
}

impl SendRetryTomlConfig {
    /// Build the runtime [`SendRetryPolicy`]. `max_retry_after_secs` is floored
    /// to 1s when non-zero is intended but `0` is supplied with a non-zero retry
    /// budget, so a configured retry never collapses into a tight no-wait loop.
    #[must_use]
    pub fn to_policy(&self) -> SendRetryPolicy {
        let secs = if self.max_rate_limit_retries > 0 {
            self.max_retry_after_secs.max(1)
        } else {
            self.max_retry_after_secs
        };
        SendRetryPolicy {
            max_rate_limit_retries: self.max_rate_limit_retries,
            max_retry_after: Duration::from_secs(secs),
        }
    }
}

/// Central registry for all channel instances
pub struct ChannelRegistry {
    /// Registered channel instances
    channels: RwLock<HashMap<ChannelId, ChannelHandle>>,
    /// Channel factories by type
    factories: RwLock<HashMap<String, Arc<dyn ChannelFactory>>>,
    /// Unified inbound message sender
    inbound_tx: InboundMessageSender,
    /// Unified inbound message receiver (for consumers)
    inbound_rx: Arc<Mutex<Option<broadcast::Receiver<InboundMessage>>>>,
    /// Per-channel voice mode state
    voice_states: RwLock<HashMap<String, VoiceState>>,
    /// Bounded retry-after policy for rate-limited outbound sends.
    send_retry: SendRetryPolicy,
    /// Optional durable outbound delivery queue. When attached, an outbound
    /// send that fails with a *definitely-not-delivered* error is persisted and
    /// retried by a background drain task instead of being dropped. `None`
    /// preserves the historic in-memory-only behavior byte-for-byte. See
    /// [`super::delivery_queue`].
    delivery_store: Option<std::sync::Arc<super::delivery_queue::DeliveryStore>>,
}

impl ChannelRegistry {
    /// Create a new channel registry
    #[must_use]
    pub fn new() -> Self {
        let (inbound_tx, inbound_rx) = broadcast::channel(1000);

        Self {
            channels: RwLock::new(HashMap::new()),
            factories: RwLock::new(HashMap::new()),
            inbound_tx: InboundMessageSender::from(inbound_tx),
            inbound_rx: Arc::new(Mutex::new(Some(inbound_rx))),
            voice_states: RwLock::new(HashMap::new()),
            send_retry: SendRetryPolicy::default(),
            delivery_store: None,
        }
    }

    /// Attach a durable outbound delivery queue (builder-style).
    ///
    /// Once attached, [`send`](Self::send) persists transient send failures to
    /// the store for background retry. Pair this with
    /// [`super::delivery_queue::spawn_drain`] to start the drain task. Without
    /// it, `send` behaves exactly as before (`None`).
    pub fn with_delivery_store(
        mut self,
        store: std::sync::Arc<super::delivery_queue::DeliveryStore>,
    ) -> Self {
        self.delivery_store = Some(store);
        self
    }

    /// Override the outbound send retry policy (builder-style).
    ///
    /// Defaults to [`SendRetryPolicy::default`] (2 retries, 30s cap). Pass a
    /// policy with `max_rate_limit_retries: 0` to restore the historical
    /// drop-on-rate-limit behavior.
    pub const fn with_send_retry_policy(mut self, policy: SendRetryPolicy) -> Self {
        self.send_retry = policy;
        self
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
        let factory = factories.get(&config.channel_type).ok_or_else(|| {
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

        info!(
            "Created channel: {} (type: {})",
            channel_id, config.channel_type
        );
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

    /// Get the capabilities for a channel by ID.
    pub async fn get_capabilities(&self, channel_id: &ChannelId) -> Option<ChannelCapabilities> {
        let channels = self.channels.read().await;
        if let Some(handle) = channels.get(channel_id) {
            let channel = handle.read().await;
            Some(channel.capabilities().clone())
        } else {
            None
        }
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
        let channel_arc = self.get(channel_id).await.ok_or_else(|| {
            ChannelError::NotConnected(format!("Channel not found: {channel_id}"))
        })?;

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
        let channel_arc = self.get(channel_id).await.ok_or_else(|| {
            ChannelError::NotConnected(format!("Channel not found: {channel_id}"))
        })?;

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

    /// Send a message through a specific channel.
    ///
    /// On a *definitely-not-delivered* failure (channel down / not yet
    /// connected, or an exhausted rate-limit) the message is persisted to the
    /// attached [`delivery_store`](Self::with_delivery_store), if any, for
    /// background retry — then the original error is still returned so callers
    /// observe unchanged semantics. When no store is attached this is a
    /// zero-overhead passthrough to [`send_attempt`](Self::send_attempt).
    pub async fn send(
        &self,
        channel_id: &ChannelId,
        message: OutboundMessage,
    ) -> ChannelResult<SendResult> {
        // Fast path: without a durable queue, behave exactly as before (no
        // extra clone, byte-identical to the historic implementation).
        if self.delivery_store.is_none() {
            return self.send_attempt(channel_id, message).await;
        }

        match self.send_attempt(channel_id, message.clone()).await {
            Ok(sent) => Ok(sent),
            Err(e) => {
                self.maybe_enqueue(channel_id, &message, &e);
                Err(e)
            }
        }
    }

    /// Persist a transient outbound failure for durable retry, if a store is
    /// attached and the error is duplicate-safe to retry. Best-effort: a
    /// persistence failure is logged and swallowed (the original send error is
    /// what the caller sees).
    fn maybe_enqueue(&self, channel_id: &ChannelId, message: &OutboundMessage, err: &ChannelError) {
        let Some(store) = &self.delivery_store else {
            return;
        };
        if !super::delivery_queue::should_enqueue(err) {
            return;
        }
        let next =
            super::delivery_queue::now_secs() + store.config().initial_backoff.as_secs() as i64;
        match store.enqueue(channel_id.as_str(), message, &format!("{err:?}"), next) {
            Ok(_) => info!(
                channel = %channel_id,
                "outbound send failed transiently; queued for durable retry"
            ),
            Err(e) => warn!(
                channel = %channel_id,
                error = %e,
                "failed to persist outbound delivery for retry"
            ),
        }
    }

    /// Attempt to deliver a message through a specific channel, with the
    /// bounded in-memory rate-limit retry. This is the enqueue-free send path;
    /// the durable drain task calls it directly so a retry never re-enqueues
    /// the record it is currently draining.
    pub(crate) async fn send_attempt(
        &self,
        channel_id: &ChannelId,
        message: OutboundMessage,
    ) -> ChannelResult<SendResult> {
        let channel_arc = self.get(channel_id).await.ok_or_else(|| {
            ChannelError::NotConnected(format!("Channel not found: {channel_id}"))
        })?;

        {
            let channel = channel_arc.read().await;
            if channel.status() == ChannelStatus::Disabled {
                return Err(ChannelError::NotConnected(format!(
                    "Channel {channel_id} is disabled"
                )));
            }
        }

        // Extension hooks observe outbound channel traffic. Capture the message
        // facts once, before `channel.send` (repeatedly) consumes a clone of the
        // OutboundMessage. The observer fires a single MessageSending regardless
        // of how many rate-limit retries follow.
        let hook_channel = channel_id.as_str().to_string();
        let hook_conversation = message.conversation_id.as_str().to_string();
        let hook_chars = message.text.chars().count();
        crate::extension::hooks::fire_global_observer(
            crate::extension::HookEvent::MessageSending,
            &hook_conversation,
            vec![
                ("CHANNEL_ID", hook_channel.clone()),
                ("CONVERSATION_ID", hook_conversation.clone()),
                ("MESSAGE_CHARS", hook_chars.to_string()),
            ],
        )
        .await;

        // Bounded retry-after loop. Only `RateLimited` (a pre-delivery 429
        // rejection) is retried — every other error is terminal and returned
        // immediately, matching the pre-existing single-attempt semantics. The
        // read lock is re-acquired per attempt so a retry-after sleep never
        // blocks channel restarts.
        let mut retries_left = self.send_retry.max_rate_limit_retries;
        loop {
            let result = {
                let channel = channel_arc.read().await;
                channel.send(message.clone()).await
            };

            match result {
                Ok(sent) => {
                    crate::extension::hooks::fire_global_observer(
                        crate::extension::HookEvent::MessageSent,
                        &hook_conversation,
                        vec![
                            ("CHANNEL_ID", hook_channel),
                            ("CONVERSATION_ID", hook_conversation.clone()),
                            ("MESSAGE_CHARS", hook_chars.to_string()),
                            ("MESSAGE_ID", sent.message_id.as_str().to_string()),
                        ],
                    )
                    .await;
                    return Ok(sent);
                }
                Err(ChannelError::RateLimited { retry_after_secs }) if retries_left > 0 => {
                    let wait =
                        Duration::from_secs(retry_after_secs).min(self.send_retry.max_retry_after);
                    warn!(
                        channel = %channel_id,
                        retry_after_secs,
                        wait_secs = wait.as_secs(),
                        retries_left,
                        "Outbound send rate-limited; honoring retry-after before retry"
                    );
                    retries_left -= 1;
                    tokio::time::sleep(wait).await;
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Edit a previously sent message through a specific channel
    pub async fn edit(
        &self,
        channel_id: &ChannelId,
        conversation_id: &ConversationId,
        message_id: &super::channel::MessageId,
        new_text: &str,
    ) -> ChannelResult<()> {
        let channel_arc = self.get(channel_id).await.ok_or_else(|| {
            ChannelError::NotConnected(format!("Channel not found: {channel_id}"))
        })?;

        let channel = channel_arc.read().await;
        channel.edit(conversation_id, message_id, new_text).await
    }

    /// React to a message through a specific channel
    pub async fn react(
        &self,
        channel_id: &ChannelId,
        conversation_id: &ConversationId,
        message_id: &super::channel::MessageId,
        reaction: &str,
    ) -> ChannelResult<()> {
        let channel_arc = self.get(channel_id).await.ok_or_else(|| {
            ChannelError::NotConnected(format!("Channel not found: {channel_id}"))
        })?;

        let channel = channel_arc.read().await;
        channel.react(conversation_id, message_id, reaction).await
    }

    /// Send typing indicator through a specific channel
    pub async fn send_typing(
        &self,
        channel_id: &ChannelId,
        conversation_id: &ConversationId,
    ) -> ChannelResult<()> {
        let channel_arc = self.get(channel_id).await.ok_or_else(|| {
            ChannelError::NotConnected(format!("Channel not found: {channel_id}"))
        })?;
        let channel = channel_arc.read().await;
        channel.send_typing(conversation_id).await
    }

    /// Take the inbound message receiver
    ///
    /// This can only be called once - subsequent calls return None.
    pub fn take_inbound_receiver(&self) -> Option<broadcast::Receiver<InboundMessage>> {
        let mut rx_guard = self.inbound_rx.lock().unwrap_or_else(|e| e.into_inner());
        rx_guard.take()
    }

    /// Get a clone of the inbound sender (for channel implementations)
    pub fn inbound_sender(&self) -> InboundMessageSender {
        self.inbound_tx.clone()
    }

    /// Start forwarding messages from a channel to the unified stream
    async fn start_message_forwarder(&self, channel_id: ChannelId, channel_arc: ChannelHandle) {
        let inbound_tx = self.inbound_tx.clone();

        tokio::spawn(async move {
            // Capture the health handle once up-front so each forwarded message
            // can stamp `last_event_at` without re-locking the channel. Every
            // inbound message is transport-liveness proof; this is the signal
            // the registry-level `ChannelHealthMonitor` reads via `is_stale`.
            let (receiver, health) = {
                let channel = channel_arc.read().await;
                (channel.inbound_subscribe(), channel.state().health_handle())
            };

            info!(
                "[Forwarder] Channel {} forwarder started — broadcast receiver obtained",
                channel_id
            );
            let mut rx = receiver;
            loop {
                match rx.recv().await {
                    Ok(message) => {
                        // Record liveness before forwarding: a channel that is
                        // delivering messages is healthy regardless of subscribers.
                        health.write().await.record_event();
                        info!(
                            "[Forwarder] Forwarding message from channel {} (text: {:?})",
                            channel_id,
                            message.text.chars().take(50).collect::<String>()
                        );
                        if let Err(e) = inbound_tx.send(message) {
                            error!(error = ?e, "Failed to forward message — no subscribers, continuing");
                            // Don't break: subscribers may join later
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        // Slow consumer: skip ahead instead of killing the
                        // forwarder. Previously the loop bound
                        // `while let Ok(...)` exited on any Err, which
                        // permanently killed this channel's inbound until
                        // the next registry restart.
                        warn!(
                            channel = %channel_id,
                            skipped = skipped,
                            "Channel forwarder lagged; resuming from latest message"
                        );
                        health.write().await.record_lag(skipped);
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        // Channel sender was dropped (channel removed). Exit.
                        info!(channel = %channel_id, "Channel forwarder exiting (sender closed)");
                        break;
                    }
                }
            }

            info!("[Forwarder] Channel {} forwarder stopped", channel_id);
        });
    }

    /// Get the voice state for a channel, returning default if not set.
    pub async fn get_voice_state(&self, channel_id: &str) -> VoiceState {
        let states = self.voice_states.read().await;
        let result = states.get(channel_id).cloned().unwrap_or_default();
        tracing::debug!(
            channel_id = channel_id,
            enabled = result.enabled,
            keys = ?states.keys().collect::<Vec<_>>(),
            registry_ptr = ?std::ptr::addr_of!(*self),
            "get_voice_state"
        );
        result
    }

    /// Overwrite the voice state for a channel.
    pub async fn set_voice_state(&self, channel_id: &str, state: VoiceState) {
        let mut states = self.voice_states.write().await;
        states.insert(channel_id.to_string(), state);
    }

    /// Apply a mutation to the voice state for a channel.
    ///
    /// If no state exists yet for the channel the default state is used as
    /// starting point.
    pub async fn update_voice_state<F>(&self, channel_id: &str, f: F)
    where
        F: FnOnce(&mut VoiceState),
    {
        let mut states = self.voice_states.write().await;
        let state = states
            .entry(channel_id.to_string())
            .or_insert_with(VoiceState::default);
        f(state);
        tracing::debug!(
            channel_id = channel_id,
            enabled = state.enabled,
            registry_ptr = ?std::ptr::addr_of!(*self),
            "update_voice_state AFTER mutation"
        );
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
                ChannelStatus::Pairing => summary.pairing += 1,
                ChannelStatus::Disconnected => summary.disconnected += 1,
                ChannelStatus::Error => summary.error += 1,
                ChannelStatus::Disabled => summary.disabled += 1,
            }
        }

        summary
    }

    pub async fn health_summary(&self) -> ChannelHealthSummary {
        let channels = self.channels.read().await;
        let mut summary = ChannelHealthSummary::default();

        for channel_arc in channels.values() {
            let channel = channel_arc.read().await;
            summary.total += 1;
            let health = channel.health().await;
            match health.status {
                HealthStatus::Healthy => summary.healthy += 1,
                HealthStatus::Stale => summary.stale += 1,
                HealthStatus::Degraded => summary.degraded += 1,
            }
        }

        summary
    }

    /// Per-channel `(id, status, health)` snapshot read by the
    /// [`crate::gateway::channel_health_monitor::ChannelHealthMonitor`] to pick
    /// restart candidates. Pure data access — restart policy lives in the
    /// monitor, not here.
    pub async fn health_states(&self) -> Vec<(ChannelId, ChannelStatus, ChannelHealth)> {
        let channels = self.channels.read().await;
        let mut out = Vec::with_capacity(channels.len());
        for (id, channel_arc) in channels.iter() {
            let channel = channel_arc.read().await;
            out.push((id.clone(), channel.status(), channel.health().await));
        }
        out
    }

    /// Restart a channel **in place**: stop then start the underlying
    /// connection without spawning a second message forwarder.
    ///
    /// `start_channel` always spawns a forwarder, but a channel's inbound
    /// broadcast lives in its persistent `ChannelState` and survives
    /// stop/start. The forwarder created by the original `start_channel`
    /// keeps draining that broadcast, so reusing it here avoids the
    /// duplicate-delivery that a second `start_channel` would cause. This is
    /// the recovery primitive used by the health monitor for wedged channels.
    pub async fn restart_channel(&self, channel_id: &ChannelId) -> ChannelResult<()> {
        let channel_arc = self.get(channel_id).await.ok_or_else(|| {
            ChannelError::NotConnected(format!("Channel not found: {channel_id}"))
        })?;

        let mut channel = channel_arc.write().await;
        channel.stop().await?;
        channel.start().await?;

        info!("Restarted channel in place: {}", channel_id);
        Ok(())
    }

    /// Observability snapshot of the durable outbound delivery queue, or `None`
    /// when no durable store is attached (in-memory-only send path). Pure data
    /// access — the depth/age/per-channel/dead-letter figures are surfaced to
    /// the `channels.list` RPC and the boot log (redline R8: Aleph's own
    /// backlog is inspectable; R5: a stuck proactive push is no longer silent).
    pub fn delivery_queue_stats(&self) -> Option<super::delivery_queue::DeliveryQueueStats> {
        let store = self.delivery_store.as_ref()?;
        match store.stats(super::delivery_queue::now_secs()) {
            Ok(stats) => Some(stats),
            Err(e) => {
                warn!(error = %e, "delivery queue stats query failed");
                None
            }
        }
    }

    /// Most-recently dead-lettered outbound deliveries (newest first), or `None`
    /// when no durable store is attached. Detail-on-demand counterpart to the
    /// `dead_lettered` **count** in [`delivery_queue_stats`](Self::delivery_queue_stats):
    /// surfaces *which* proactive pushes were permanently lost, to whom, and why
    /// — reading back the forensic trail `record_dead_letter` preserves (R8/R5).
    /// A query error degrades to an empty list (still `Some`, so callers can
    /// distinguish "no store" from "store present, nothing to show").
    pub fn recent_dead_letters(
        &self,
        limit: usize,
    ) -> Option<Vec<super::delivery_queue::DeadLetter>> {
        let store = self.delivery_store.as_ref()?;
        match store.recent_dead_letters(limit) {
            Ok(letters) => Some(letters),
            Err(e) => {
                warn!(error = %e, "delivery queue: recent_dead_letters query failed");
                Some(Vec::new())
            }
        }
    }

    /// Move dead-lettered deliveries back into the live outbound queue for
    /// another delivery pass — the recovery half of the dead-letter trail (R5).
    /// `channel` optionally restricts the redrive to one transport. Returns the
    /// number redriven, or `None` when no durable store is attached. Safe by
    /// construction (every dead letter was a duplicate-safe transient failure —
    /// see [`super::delivery_queue::DeliveryStore::redrive_dead_letters`]).
    pub fn redrive_dead_letters(&self, channel: Option<&str>) -> Option<u64> {
        let store = self.delivery_store.as_ref()?;
        match store.redrive_dead_letters(super::delivery_queue::now_secs(), channel) {
            Ok(n) => Some(n),
            Err(e) => {
                warn!(error = %e, "delivery queue: redrive_dead_letters failed");
                Some(0)
            }
        }
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
    pub pairing: usize,
    pub disconnected: usize,
    pub error: usize,
    pub disabled: usize,
}

#[derive(Debug, Clone, Default)]
pub struct ChannelHealthSummary {
    pub total: usize,
    pub healthy: usize,
    pub stale: usize,
    pub degraded: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::channel::{ChannelState, MessageId};
    use chrono::Utc;
    use std::sync::atomic::{AtomicU32, Ordering};

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

    /// Test channel whose `send` returns `RateLimited` for the first
    /// `fail_times` attempts and then succeeds. If `terminal` is set it always
    /// returns a non-retryable `SendFailed` instead. `attempts` is shared so the
    /// test can assert how many sends actually reached the channel.
    struct FlakyChannel {
        info: ChannelInfo,
        state: ChannelState,
        fail_times: AtomicU32,
        attempts: Arc<AtomicU32>,
        terminal: bool,
    }

    impl FlakyChannel {
        fn new(fail_times: u32, terminal: bool, attempts: Arc<AtomicU32>) -> Self {
            Self {
                info: ChannelInfo {
                    id: ChannelId::new("flaky"),
                    name: "flaky".to_string(),
                    channel_type: "test".to_string(),
                    status: ChannelStatus::Connected,
                    capabilities: ChannelCapabilities::default(),
                },
                // `retry_after_secs` is 0 in the error below, so the registry's
                // capped sleep is a no-op — tests stay fast and deterministic.
                state: ChannelState::new(8),
                fail_times: AtomicU32::new(fail_times),
                attempts,
                terminal,
            }
        }
    }

    #[async_trait::async_trait]
    impl Channel for FlakyChannel {
        fn info(&self) -> &ChannelInfo {
            &self.info
        }
        fn state(&self) -> &ChannelState {
            &self.state
        }
        async fn start(&mut self) -> ChannelResult<()> {
            Ok(())
        }
        async fn stop(&mut self) -> ChannelResult<()> {
            Ok(())
        }
        async fn send(&self, _message: OutboundMessage) -> ChannelResult<SendResult> {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            if self.terminal {
                return Err(ChannelError::SendFailed("permanent".to_string()));
            }
            if self.fail_times.load(Ordering::SeqCst) > 0 {
                self.fail_times.fetch_sub(1, Ordering::SeqCst);
                return Err(ChannelError::RateLimited {
                    retry_after_secs: 0,
                });
            }
            Ok(SendResult {
                message_id: MessageId::new("ok"),
                timestamp: Utc::now(),
            })
        }
    }

    async fn registry_with(channel: FlakyChannel, policy: SendRetryPolicy) -> ChannelRegistry {
        let registry = ChannelRegistry::new().with_send_retry_policy(policy);
        registry.register(Box::new(channel)).await;
        registry
    }

    #[tokio::test]
    async fn rate_limited_then_succeeds_within_budget() {
        let attempts = Arc::new(AtomicU32::new(0));
        let registry = registry_with(
            FlakyChannel::new(2, false, attempts.clone()),
            SendRetryPolicy::default(),
        )
        .await;

        let result = registry
            .send(&ChannelId::new("flaky"), OutboundMessage::text("c", "hi"))
            .await;

        assert!(result.is_ok(), "should succeed after honoring retry-after");
        // 2 rate-limited rejections + 1 success.
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn rate_limit_retries_exhausted_propagates() {
        let attempts = Arc::new(AtomicU32::new(0));
        // Fails more times than the budget allows.
        let registry = registry_with(
            FlakyChannel::new(10, false, attempts.clone()),
            SendRetryPolicy {
                max_rate_limit_retries: 2,
                max_retry_after: Duration::from_secs(30),
            },
        )
        .await;

        let result = registry
            .send(&ChannelId::new("flaky"), OutboundMessage::text("c", "hi"))
            .await;

        assert!(matches!(result, Err(ChannelError::RateLimited { .. })));
        // Initial attempt + 2 retries.
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn terminal_error_is_not_retried() {
        let attempts = Arc::new(AtomicU32::new(0));
        let registry = registry_with(
            FlakyChannel::new(0, true, attempts.clone()),
            SendRetryPolicy::default(),
        )
        .await;

        let result = registry
            .send(&ChannelId::new("flaky"), OutboundMessage::text("c", "hi"))
            .await;

        assert!(matches!(result, Err(ChannelError::SendFailed(_))));
        // SendFailed is ambiguous (may already be delivered) → exactly one attempt.
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn send_retry_toml_default_matches_runtime_default() {
        let from_toml = SendRetryTomlConfig::default().to_policy();
        let native = SendRetryPolicy::default();
        assert_eq!(
            from_toml.max_rate_limit_retries,
            native.max_rate_limit_retries
        );
        assert_eq!(from_toml.max_retry_after, native.max_retry_after);
    }

    #[test]
    fn send_retry_toml_floors_wait_when_retries_enabled() {
        // A non-zero retry budget with a 0s cap would collapse into a tight
        // no-wait loop — floored to 1s.
        let cfg = SendRetryTomlConfig {
            max_rate_limit_retries: 3,
            max_retry_after_secs: 0,
        };
        let policy = cfg.to_policy();
        assert_eq!(policy.max_rate_limit_retries, 3);
        assert_eq!(policy.max_retry_after, Duration::from_secs(1));
    }

    #[test]
    fn send_retry_toml_zero_retries_keeps_zero_wait() {
        // With retries disabled the wait is irrelevant and left untouched —
        // no spurious floor.
        let cfg = SendRetryTomlConfig {
            max_rate_limit_retries: 0,
            max_retry_after_secs: 0,
        };
        let policy = cfg.to_policy();
        assert_eq!(policy.max_rate_limit_retries, 0);
        assert_eq!(policy.max_retry_after, Duration::from_secs(0));
    }

    #[tokio::test]
    async fn zero_retry_policy_preserves_legacy_drop() {
        let attempts = Arc::new(AtomicU32::new(0));
        let registry = registry_with(
            FlakyChannel::new(1, false, attempts.clone()),
            SendRetryPolicy {
                max_rate_limit_retries: 0,
                max_retry_after: Duration::from_secs(30),
            },
        )
        .await;

        let result = registry
            .send(&ChannelId::new("flaky"), OutboundMessage::text("c", "hi"))
            .await;

        assert!(matches!(result, Err(ChannelError::RateLimited { .. })));
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }
}
