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

/// Whether the registry *intends* a channel to be carrying traffic right now.
///
/// [`ChannelStatus`] alone cannot answer this. `Disconnected` is written by
/// three unrelated situations — never started, stopped on purpose, and a
/// transport that died under a running channel — and several adapters
/// (`discord`, `irc`, `xmpp`) land in it after their connection task exits for
/// *any* reason. Restart policy needs the operator's intent, which only the
/// registry knows because only the registry serves `start` / `stop`.
///
/// Mapped from openclaw's `isManagedAccount` + `lifecycle` pair in
/// `gateway/channel-health-policy.ts`, collapsed to the one bit Aleph's
/// registry can actually own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesiredChannelState {
    /// `start_channel` / `restart_channel` succeeded and nothing stopped it
    /// since: the channel owes us traffic.
    Running,
    /// Never started, or explicitly stopped. Never a restart candidate.
    Stopped,
}

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
    /// Live webhook mount table. The registry is the single writer: mounting
    /// follows channel lifecycle instead of being a boot-time snapshot, so
    /// `stop` / `delete` / runtime `create` change what HTTP actually serves.
    /// Handed to `GatewayServer::set_webhook_mounts` at boot.
    webhook_mounts: Arc<super::webhook_receiver::WebhookMountTable>,
    /// Per-channel *intent* (see [`DesiredChannelState`]). Written only by the
    /// lifecycle methods on this type, read by the health monitor.
    desired_state: RwLock<HashMap<ChannelId, DesiredChannelState>>,
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
            webhook_mounts: Arc::new(super::webhook_receiver::WebhookMountTable::new()),
            desired_state: RwLock::new(HashMap::new()),
        }
    }

    /// The live webhook mount table (shared handle).
    #[must_use]
    pub fn webhook_mounts(&self) -> Arc<super::webhook_receiver::WebhookMountTable> {
        Arc::clone(&self.webhook_mounts)
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

    /// Register a channel factory.
    ///
    /// Retained on the public API for callers that compose `ChannelRegistry`
    /// outside the boot path (e.g. desktop shells that discover channel types
    /// at runtime). Internal code reaches factories through
    /// `handlers::channel::create_channel_from_config`, so the `factories`
    /// table inside the registry stays empty for the in-tree server. A no-op
    /// write here is harmless because the lookup falls through with a clear
    /// `ConfigError` if the table is empty when `create_channel` runs.
    #[allow(dead_code)]
    pub async fn register_factory(&self, factory: Arc<dyn ChannelFactory>) {
        let channel_type = factory.channel_type().to_string();
        let mut factories = self.factories.write().await;
        factories.insert(channel_type.clone(), factory);
        info!("Registered channel factory: {}", channel_type);
    }

    /// Create and register a channel from configuration.
    ///
    /// Retained for symmetry with `register_factory` — see that method for
    /// why the public surface is wider than the in-tree caller graph. The
    /// boot path does not exercise this entry point because channel
    /// construction is config-driven via
    /// `handlers::channel::create_channel_from_config`.
    #[allow(dead_code)]
    pub async fn create_channel(&self, config: ChannelConfig) -> ChannelResult<ChannelId> {
        let factories = self.factories.read().await;
        let factory = factories.get(&config.channel_type).ok_or_else(|| {
            ChannelError::ConfigError(format!(
                "No factory registered for channel type: {}",
                config.channel_type
            ))
        })?;

        let channel = factory
            .create_with_id(&config.id, config.config.clone())
            .await?;
        let channel_id = channel.id().clone();

        drop(factories);

        // Same reasoning as `register`: the freshly-created instance has not
        // started, so drop whatever the outgoing instance at this id left
        // mounted before the replacement lands.
        self.webhook_mounts.unmount_channel(&channel_id).await;

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

        // A replacement instance has not started, so it owns no handler. Drop
        // whatever the outgoing instance left mounted, or the route keeps
        // serving with the old secret until someone happens to start the new one.
        self.webhook_mounts.unmount_channel(&channel_id).await;

        let mut channels = self.channels.write().await;
        channels.insert(channel_id.clone(), Arc::new(RwLock::new(channel)));
        info!("Registered channel: {}", channel_id);
        channel_id
    }

    /// Unregister a channel
    ///
    /// # Return-value contract (read before relying on `Some(..)`)
    ///
    /// The returned `Option<Box<dyn Channel>>` is `Some` ONLY when the
    /// channel was never started. `start_message_forwarder` clones the
    /// channel `Arc` into the forwarder task at `start_channel`, so after a
    /// start the registry is never the sole strong owner and
    /// `Arc::try_unwrap` below ALWAYS fails — this function then returns
    /// `None` (see `unregister_unmounts_the_webhook` in this file's tests,
    /// which pins that behavior). There is currently NO way to recover the
    /// boxed adapter for a graceful adapter-level `shutdown` once a channel
    /// has started; use `stop_channel` for lifecycle teardown instead of
    /// relying on the return value. Making `Some` reachable post-start would
    /// require the forwarder to hold a `Weak` reference instead — a
    /// lifecycle-semantics change deferred to a dedicated batch (it changes
    /// who keeps the adapter alive while the forwarder runs).
    pub async fn unregister(&self, channel_id: &ChannelId) -> Option<Box<dyn Channel>> {
        // Drop the HTTP surface before the instance leaves the registry —
        // including when `Arc::try_unwrap` below fails and this returns None.
        // `channel.delete` otherwise leaves an authenticated endpoint the
        // operator believes is gone.
        self.webhook_mounts.unmount_channel(channel_id).await;
        // Drop the intent with the instance. Leaving it behind would both leak
        // an entry per deleted channel and hand a later channel that reuses the
        // id a `Running` intent it never asked for.
        self.desired_state.write().await.remove(channel_id);

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
    ///
    /// `None` when the channel is not registered — deliberately not a
    /// `Default`, because "this transport declares no cap" and "this transport
    /// is gone" are different answers and the caller decides.
    ///
    /// Used by the origin fan-out to size its outbound chunks; callers already
    /// holding a `ChannelHandle` from [`ChannelRegistry::get`] read
    /// `capabilities()` through the guard instead.
    pub async fn get_capabilities(&self, channel_id: &ChannelId) -> Option<ChannelCapabilities> {
        let channels = self.channels.read().await;
        if let Some(handle) = channels.get(channel_id) {
            let channel = handle.read().await;
            Some(channel.capabilities().clone())
        } else {
            None
        }
    }

    /// List all channels.
    ///
    /// **Audit fix**: the previous implementation iterated `HashMap::values`
    /// directly, exposing HashMap's non-deterministic iteration order to every
    /// caller. The `channels.list` RPC, every Panel render, and every
    /// `channel.health_states` snapshot used to flicker between calls on the
    /// same channel set. Sorted by `(channel_type, id)` so the order is
    /// stable across runs and reproducible in tests.
    pub async fn list(&self) -> Vec<ChannelInfo> {
        let channels = self.channels.read().await;
        let mut infos = Vec::with_capacity(channels.len());

        for channel_arc in channels.values() {
            let channel = channel_arc.read().await;
            let mut info = channel.info().clone();
            info.status = channel.status(); // override with live status
            infos.push(info);
        }
        infos.sort_by(|a, b| {
            a.channel_type
                .cmp(&b.channel_type)
                .then_with(|| a.id.0.cmp(&b.id.0))
        });
        infos
    }

    /// List channels by type.
    ///
    /// Sorted by `id` so the order is deterministic across runs (see
    /// [`Self::list`] for the full rationale).
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
        infos.sort_by(|a, b| a.id.0.cmp(&b.id.0));
        infos
    }

    /// Build the webhook mount for `channel_id` from `channel`'s just-(re)started
    /// state, if it exposes a webhook handler — `None` for every non-webhook
    /// channel. Shared by `start_channel` and `restart_channel`, the two sites
    /// where `start()` (re)builds the handler.
    fn webhook_mount_for(
        channel: &dyn Channel,
        channel_id: &ChannelId,
    ) -> Option<super::webhook_receiver::WebhookMount> {
        channel
            .webhook_handler()
            .map(|handler| super::webhook_receiver::WebhookMount {
                handler,
                inbound: channel.state().sender(),
                status: channel.state().status_handle(),
                channel_id: channel_id.clone(),
            })
    }

    /// Start a channel
    pub async fn start_channel(&self, channel_id: &ChannelId) -> ChannelResult<()> {
        let channel_arc = self.get(channel_id).await.ok_or_else(|| {
            ChannelError::NotConnected(format!("Channel not found: {channel_id}"))
        })?;

        let mut channel = channel_arc.write().await;
        channel.start().await?;

        // A webhook channel only materialises its handler in `start()`, so this
        // is the earliest point the mount can exist. The sink is the channel's
        // OWN broadcast so `start_message_forwarder` still stamps channel
        // health — going to the registry's sender directly would make a
        // receiving channel look dead to the health monitor.
        let mount = Self::webhook_mount_for(&**channel, channel_id);

        // Start forwarding inbound messages
        self.start_message_forwarder(channel_id.clone(), channel_arc.clone())
            .await;
        drop(channel);

        if let Some(mount) = mount {
            self.webhook_mounts.mount(mount).await;
        }

        // Only after `start()` returned Ok: a channel that refused to start is
        // not something the health monitor should keep resurrecting.
        self.set_desired(channel_id, DesiredChannelState::Running)
            .await;

        info!("Started channel: {}", channel_id);
        Ok(())
    }

    /// Stop a channel
    pub async fn stop_channel(&self, channel_id: &ChannelId) -> ChannelResult<()> {
        let channel_arc = self.get(channel_id).await.ok_or_else(|| {
            ChannelError::NotConnected(format!("Channel not found: {channel_id}"))
        })?;

        // Recorded before `stop()` runs, not after: an adapter whose `stop`
        // returns an error has still been told to shut down, and the health
        // monitor must not race in to restart it.
        self.set_desired(channel_id, DesiredChannelState::Stopped)
            .await;

        let mut channel = channel_arc.write().await;
        channel.stop().await?;
        drop(channel);

        // The route holds its own handler clone, so dropping the channel's
        // copy is not enough — without this the endpoint keeps answering 200
        // and driving agent runs after `channel.stop` reported "stopped".
        self.webhook_mounts.unmount_channel(channel_id).await;

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
    ///
    /// When a store *is* attached, this message's conversation gets its queued
    /// backlog flushed first
    /// ([`flush_conversation`](super::delivery_queue::flush_conversation)) so a
    /// live send cannot overtake replies that failed earlier. That flush is
    /// deliberately invisible from here: it settles nothing this call reports
    /// on, and whether it ran or was skipped, the value returned below is the
    /// same one this method has always returned.
    pub async fn send(
        &self,
        channel_id: &ChannelId,
        message: OutboundMessage,
    ) -> ChannelResult<SendResult> {
        if let Some(store) = &self.delivery_store {
            super::delivery_queue::flush_conversation(
                self,
                store,
                channel_id.as_str(),
                message.conversation_id.as_str(),
            )
            .await;
        }

        // `send_attempt` borrows the message (it clones once per transport
        // attempt internally), so the persist-on-failure path costs no extra
        // deep copy of the payload — which for a message carrying inline
        // attachment bytes is the difference between one and two copies of the
        // whole media blob on every single outbound send.
        match self.send_attempt(channel_id, &message).await {
            Ok(sent) => Ok(sent),
            Err(e) => {
                self.maybe_enqueue(channel_id, &message, &e).await;
                Err(e)
            }
        }
    }

    /// Persist a transient outbound failure for durable retry, if a store is
    /// attached and the error is duplicate-safe to retry. Best-effort: a
    /// persistence failure is logged and swallowed (the original send error is
    /// what the caller sees).
    async fn maybe_enqueue(
        &self,
        channel_id: &ChannelId,
        message: &OutboundMessage,
        err: &ChannelError,
    ) {
        let Some(store) = &self.delivery_store else {
            return;
        };
        if !super::delivery_queue::should_enqueue(err) {
            return;
        }
        // Ordered after the two cheap refusals above so a message that is never
        // going to be queued never pays for a file read.
        let custody =
            super::delivery_queue::take_media_custody(message, store.config().max_payload_bytes)
                .await;
        let message = custody.as_ref().unwrap_or(message);
        let next =
            super::delivery_queue::now_secs() + store.config().initial_backoff.as_secs() as i64;
        match store.enqueue(channel_id.as_str(), message, &format!("{err:?}"), next) {
            Ok(super::delivery_queue::EnqueueOutcome::Queued(_)) => info!(
                channel = %channel_id,
                "outbound send failed transiently; queued for durable retry"
            ),
            // Reported distinctly rather than folded into the success log: the
            // message is *not* coming back, and the operator's next question is
            // "which one?" — answered by the dead-letter trail the store wrote.
            Ok(super::delivery_queue::EnqueueOutcome::TooLarge { bytes }) => warn!(
                channel = %channel_id,
                bytes,
                "outbound payload exceeds the durable-queue byte cap; dead-lettered instead of retried"
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
        message: &OutboundMessage,
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

        // NOTE: the read guard is held across the await on `channel.edit`
        // because `Channel::edit` returns a future that borrows `&self`.
        // Releasing the guard before the await requires the trait to
        // return an owned (Box<dyn Future>) future, which would need a
        // breaking change to every `Channel` impl. Documented risk:
        // a slow adapter edit can block stop_channel / health; mitigated
        // by the per-adapter `editing` capability gate.
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

        // NOTE: the read guard is held across the await on `channel.react`
        // (see `edit` for the trait-borrow rationale). The same `Channel`
        // trait is involved.
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
        // NOTE: guard held across await (see `edit`).
        let channel = channel_arc.read().await;
        channel.send_typing(conversation_id).await
    }

    /// List addressable conversations on a specific channel.
    ///
    /// Read half of the outbound API: `send`/`edit`/`react` all need a
    /// [`ConversationId`] the caller already has, which is why a proactive
    /// "post this to #eng-releases" was impossible before this.
    pub async fn list_conversations(
        &self,
        channel_id: &ChannelId,
        query: &str,
        limit: usize,
    ) -> ChannelResult<super::channel::ConversationPage> {
        let channel_arc = self.get(channel_id).await.ok_or_else(|| {
            ChannelError::NotConnected(format!("Channel not found: {channel_id}"))
        })?;

        // Known trade-off: the `Channel` trait uses `async fn` returning a
        // future that borrows `&self`, so the read guard must be held for
        // the full duration of the adapter call. We mitigate the writer-
        // starvation risk by using `try_read` and returning a transient
        // `Busy` error if a writer (`stop_channel`/`restart_channel`) is
        // already queued — that way the writer can proceed instead of
        // waiting behind us for the adapter's full HTTP sweep.
        //
        // Adapters are still expected to serve `list_conversations` from a
        // TTL cache (see the existing per-adapter `editing` capability
        // gate) so the read-lock window stays short in practice.
        // tokio 1.52's `sync::TryLockError` is a unit struct (no variant
        // distinction between "would block" and "poisoned"); the review
        // commit pattern-matched variants that do not exist in this
        // version. Treat any try_read failure as "channel busy, retry":
        // the channel will surface real failures when the next read
        // succeeds and the guard goes out of scope.
        let channel = channel_arc.try_read().map_err(|_| {
            ChannelError::Busy(format!(
                "channel {channel_id} is restarting; retry list_conversations"
            ))
        })?;
        channel.list_conversations(query, limit).await
    }

    /// Take the inbound message receiver
    ///
    /// This can only be called once - subsequent calls return None.
    ///
    /// Unlike `ChannelState::take_receiver()` — which was named after
    /// `mpsc::Receiver::take()` semantics that `broadcast` does not have —
    /// THIS method genuinely consumes the receiver via `Option::take()`,
    /// so the single-consumer contract is real here.
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
                        // **Audit fix**: do NOT call `record_event()` here.
                        // Lagged means OUR forwarder fell behind the
                        // broadcast ring — it is not proof of transport
                        // liveness on the channel. Stamping `last_event_at`
                        // here reset the staleness window from the lag
                        // instant, so a wedged channel whose inbound queue
                        // backed up exactly at its last real message would
                        // evade the health monitor for another full
                        // DEFAULT_STALE_SECS. The monitor only trusts real
                        // messages now.
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

    /// Per-channel lifecycle snapshot read by the
    /// [`crate::gateway::channel_health_monitor::ChannelHealthMonitor`] to pick
    /// restart candidates. Pure data access — restart policy lives in the
    /// monitor, not here.
    pub async fn health_states(&self) -> Vec<ChannelLifecycleSnapshot> {
        let channels = self.channels.read().await;
        let desired = self.desired_state.read().await;
        let mut out = Vec::with_capacity(channels.len());
        for (id, channel_arc) in channels.iter() {
            let channel = channel_arc.read().await;
            out.push(ChannelLifecycleSnapshot {
                // A channel registered but never started has no entry, which is
                // `Stopped` — the correct answer, and the reason this defaults
                // rather than erroring.
                desired: desired
                    .get(id)
                    .copied()
                    .unwrap_or(DesiredChannelState::Stopped),
                id: id.clone(),
                status: channel.status(),
                health: channel.health().await,
            });
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

        // This path does NOT go through stop_channel/start_channel, so it needs
        // its own refresh: `start()` builds a NEW handler and the table would
        // otherwise keep serving the pre-restart clone. If `start()` yielded no
        // handler this time, unmount instead of leaving the stale one behind.
        let mount = Self::webhook_mount_for(&**channel, channel_id);
        drop(channel);

        match mount {
            Some(mount) => {
                self.webhook_mounts.mount(mount).await;
            }
            None => {
                self.webhook_mounts.unmount_channel(channel_id).await;
            }
        }

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
    /// [`RedriveOutcome`](super::delivery_queue::RedriveOutcome), or `None` when
    /// no durable store is attached. Only records whose failure is
    /// *definitely-not-delivered* are moved; see
    /// [`super::delivery_queue::DeliveryStore::redrive_dead_letters`].
    pub fn redrive_dead_letters(
        &self,
        channel: Option<&str>,
    ) -> Option<super::delivery_queue::RedriveOutcome> {
        let store = self.delivery_store.as_ref()?;
        match store.redrive_dead_letters(super::delivery_queue::now_secs(), channel) {
            Ok(outcome) => Some(outcome),
            Err(e) => {
                warn!(error = %e, "delivery queue: redrive_dead_letters failed");
                Some(super::delivery_queue::RedriveOutcome::default())
            }
        }
    }

    /// Record that `channel_id` is *supposed to be running*, so the health
    /// monitor can tell a transport that died from one an operator stopped.
    async fn set_desired(&self, channel_id: &ChannelId, desired: DesiredChannelState) {
        self.desired_state
            .write()
            .await
            .insert(channel_id.clone(), desired);
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

/// One channel's lifecycle facts, as the health monitor sees them.
///
/// Carries `desired` alongside the live `status` because the two answer
/// different questions and only their *conjunction* identifies a wedged
/// channel: `status` says what the transport reports, `desired` says whether
/// anyone still expects it to work.
#[derive(Debug, Clone)]
pub struct ChannelLifecycleSnapshot {
    pub id: ChannelId,
    pub desired: DesiredChannelState,
    pub status: ChannelStatus,
    pub health: ChannelHealth,
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

    // --- Webhook mount follows channel lifecycle ---

    /// Channel that materialises a webhook handler in `start()` and drops it in
    /// `stop()` — the shape of `WebhookChannel`.
    struct WebhookyChannel {
        info: ChannelInfo,
        state: ChannelState,
        path: String,
        handler: Option<Arc<TestHandler>>,
    }

    struct TestHandler {
        path: String,
    }

    #[async_trait::async_trait]
    impl crate::gateway::webhook_receiver::WebhookHandler for TestHandler {
        fn verify(&self, _headers: &axum::http::HeaderMap, _body: &[u8]) -> bool {
            true
        }
        async fn handle(
            &self,
            _headers: &axum::http::HeaderMap,
            _body: axum::body::Bytes,
        ) -> ChannelResult<Vec<crate::gateway::channel::InboundMessage>> {
            Ok(vec![])
        }
        fn path(&self) -> &str {
            &self.path
        }
    }

    impl WebhookyChannel {
        fn new(id: &str, path: &str) -> Self {
            Self {
                info: ChannelInfo {
                    id: ChannelId::new(id),
                    name: id.to_string(),
                    channel_type: "test-webhook".to_string(),
                    status: ChannelStatus::Disconnected,
                    capabilities: ChannelCapabilities::default(),
                },
                state: ChannelState::new(8),
                path: path.to_string(),
                handler: None,
            }
        }
    }

    #[async_trait::async_trait]
    impl Channel for WebhookyChannel {
        fn info(&self) -> &ChannelInfo {
            &self.info
        }
        fn state(&self) -> &ChannelState {
            &self.state
        }
        async fn start(&mut self) -> ChannelResult<()> {
            self.handler = Some(Arc::new(TestHandler {
                path: self.path.clone(),
            }));
            self.state.set_status(ChannelStatus::Connected).await;
            Ok(())
        }
        async fn stop(&mut self) -> ChannelResult<()> {
            self.handler = None;
            self.state.set_status(ChannelStatus::Disconnected).await;
            Ok(())
        }
        fn webhook_handler(
            &self,
        ) -> Option<Arc<dyn crate::gateway::webhook_receiver::WebhookHandler>> {
            self.handler
                .clone()
                .map(|h| h as Arc<dyn crate::gateway::webhook_receiver::WebhookHandler>)
        }
        async fn send(&self, _message: OutboundMessage) -> ChannelResult<SendResult> {
            Ok(SendResult {
                message_id: MessageId::new("ok"),
                timestamp: Utc::now(),
            })
        }
    }

    #[tokio::test]
    async fn start_channel_mounts_the_webhook() {
        let registry = ChannelRegistry::new();
        let id = registry
            .register(Box::new(WebhookyChannel::new("wh", "/webhook/wh")))
            .await;

        assert_eq!(registry.webhook_mounts().mounted_count().await, 0);
        registry.start_channel(&id).await.unwrap();
        assert_eq!(registry.webhook_mounts().mounted_count().await, 1);
    }

    #[tokio::test]
    async fn stop_channel_unmounts_the_webhook() {
        let registry = ChannelRegistry::new();
        let id = registry
            .register(Box::new(WebhookyChannel::new("wh", "/webhook/wh")))
            .await;
        registry.start_channel(&id).await.unwrap();

        registry.stop_channel(&id).await.unwrap();
        assert_eq!(
            registry.webhook_mounts().mounted_count().await,
            0,
            "a stopped channel must not keep an authenticated HTTP endpoint"
        );
    }

    #[tokio::test]
    async fn unregister_unmounts_the_webhook() {
        let registry = ChannelRegistry::new();
        let id = registry
            .register(Box::new(WebhookyChannel::new("wh", "/webhook/wh")))
            .await;
        registry.start_channel(&id).await.unwrap();

        // `None` is the deterministic outcome here, not a scheduling accident:
        // `channel_arc.clone()` (channel_registry.rs:360) runs before the
        // forwarder is spawned, so the strong count is ≥2 immediately and the
        // spawned task never releases it. `Arc::try_unwrap` therefore always
        // fails on this path, and `unregister` always returns `None`.
        assert!(registry.unregister(&id).await.is_none());
        assert_eq!(
            registry.webhook_mounts().mounted_count().await,
            0,
            "channel.delete must remove the endpoint even when the Arc is still held"
        );
    }

    #[tokio::test]
    async fn restart_channel_refreshes_the_mount() {
        let registry = ChannelRegistry::new();
        let id = registry
            .register(Box::new(WebhookyChannel::new("wh", "/webhook/wh")))
            .await;
        registry.start_channel(&id).await.unwrap();
        let first = registry
            .webhook_mounts()
            .lookup("/webhook/wh")
            .await
            .expect("mounted");

        // restart_channel does NOT go through stop_channel/start_channel, so it
        // needs its own hook — otherwise the table keeps the pre-restart
        // handler clone forever.
        registry.restart_channel(&id).await.unwrap();
        let second = registry
            .webhook_mounts()
            .lookup("/webhook/wh")
            .await
            .expect("still mounted");

        assert!(
            !Arc::ptr_eq(&first.handler, &second.handler),
            "the table must hold the handler built by the restart"
        );
    }

    #[tokio::test]
    async fn re_registering_drops_the_outgoing_instance_mount() {
        let registry = ChannelRegistry::new();
        let id = registry
            .register(Box::new(WebhookyChannel::new("wh", "/webhook/wh")))
            .await;
        registry.start_channel(&id).await.unwrap();

        // `channel.start` re-creates the instance from fresh config and
        // re-registers it. The replacement has not started, so it owns no
        // handler — the old mount must not keep serving with the old secret.
        registry
            .register(Box::new(WebhookyChannel::new("wh", "/webhook/wh")))
            .await;
        assert_eq!(registry.webhook_mounts().mounted_count().await, 0);
    }
}
