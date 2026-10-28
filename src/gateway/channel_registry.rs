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

pub(crate) mod binding;
#[cfg(test)]
pub(crate) mod test_support;

use crate::capability::descriptor::{
    CapabilityDescriptor, CapabilityFacet, CapabilityFacets, CapabilityId, CapabilityKind,
    CapabilityRevision,
};
use crate::capability::ownership::{OwnershipTree, VisibilityScope};
use crate::capability::provider::{
    ProviderHandle, ProviderId, ProviderLifecycle, ProviderState, ProviderTransport,
};
use crate::sync_primitives::{Arc, Mutex};
use binding::{
    canonical_descriptor_fingerprint, BoundChannelCapture, ChannelBindingError,
    ChannelInitialState, ChannelLifecycleContract,
};
use std::collections::HashMap;
use std::sync::atomic::AtomicU64;
use std::time::Duration;
use tokio::sync::{broadcast, Mutex as AsyncMutex, RwLock};
use tracing::{error, info, warn};

use super::channel::CoreSmsChannelExt;
use super::channel::{
    Channel, ChannelCapabilities, ChannelConfig, ChannelError, ChannelFactory, ChannelHealth,
    ChannelId, ChannelInfo, ChannelResult, ChannelStatus, ConversationId, HealthStatus,
    InboundMessage, InboundMessageSender, OutboundDeliveryPolicy, OutboundMessage, SendResult,
};
use super::voice::VoiceState;

/// Type alias for a thread-safe, shareable channel handle
type ChannelHandle = Arc<RwLock<Box<dyn Channel>>>;

struct ChannelRegistrationEntry {
    channel: ChannelHandle,
    registration: ChannelRegistration,
    runtime: Mutex<binding::EntryRuntime>,
    drained: tokio::sync::Notify,
}

enum ChannelRegistration {
    /// A legacy registration published at `publication_revision`, a revision
    /// unique to this publication for as long as the registry lives.
    ///
    /// The revision is the identity a webhook mount carries: it is stamped once
    /// at publication and never rewritten, so `mount == current entry` is
    /// decidable at dispatch without an owner map or a second instance
    /// authority. Bound registrations keep their own `entry_revision`.
    Legacy {
        publication_revision: u64,
    },
    Bound(Box<BoundChannelRegistration>),
}

#[allow(dead_code)] // Immutable source publication, consumed by later lifecycle tasks.
struct BoundChannelRegistration {
    source: Arc<()>,
    entry_revision: u64,
    contract: ChannelLifecycleContract,
    descriptor: CapabilityDescriptor,
    descriptor_fingerprint: [u8; 32],
    provider: ProviderHandle,
    ownership: Arc<OwnershipTree>,
}

/// A source-owned capture of one registered channel instance.
///
/// The handle identity is the admission fence. `source_revision` records the
/// registry publication cut for diagnostics and future descriptor projection;
/// an unrelated channel publication must not invalidate this capture.
///
/// This contract is intentionally crate-private and not mounted into a
/// capability route until the provider binding gate closes.
#[allow(dead_code)]
#[derive(Clone)]
pub(crate) struct ChannelCapture {
    channel_id: ChannelId,
    source: Arc<()>,
    entry: Arc<ChannelRegistrationEntry>,
    source_revision: u64,
    handle: ChannelHandle,
}

#[allow(dead_code)]
impl ChannelCapture {
    #[must_use]
    pub(crate) fn source_revision(&self) -> u64 {
        self.source_revision
    }
}

/// Private telecom precommit fence result. An error or timeout is deliberately
/// not an `Invalid` result: transport acceptance may already have occurred.
pub(crate) use super::telecom::public_run::ApprovedReplyOutcome as ApprovedTelecomSend;

/// Actual custody of a fresh message after the transport attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueDisposition {
    Delivered,
    Queued,
    NoStore,
    NotQueueable,
    PolicyRefused,
    DeadLettered,
    PersistFailed,
}

/// Transport result and its actual policy/custody witnesses. Only `Queued`
/// permits a caller to rely on the queue for continuation or media retention.
#[derive(Debug)]
pub struct SendReport {
    pub result: ChannelResult<SendResult>,
    pub policy: Option<OutboundDeliveryPolicy>,
    pub queue: QueueDisposition,
}

#[derive(Clone, Copy)]
pub(crate) enum AttemptKind<'a> {
    Fresh,
    Replay {
        store: &'a super::delivery_queue::DeliveryStore,
        record_id: i64,
    },
}

pub(crate) struct Attempt {
    pub result: ChannelResult<SendResult>,
    pub policy: Option<OutboundDeliveryPolicy>,
    pub replay_refused: bool,
}

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
    /// Configured enabled at boot, but the first start has not yet committed.
    /// This is an internal boot intent and is not a restart candidate.
    Enabled,
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

/// Dispatch-time authority over a channel's Legacy publication identity.
///
/// Owned by the registry and handed to its webhook mount table. The HTTP
/// endpoint holds the mount table, not the registry: a registry is a plain
/// value until its owner wraps it, so it cannot hand out a back-reference to
/// itself, and a second map of "which channels are live" would be a second
/// instance authority.
///
/// Validation is a positive match against the *current* entry, so a mount whose
/// channel id was replaced, retired, or converted to Bound is refused even
/// though its handler object and signature are still live.
struct ChannelSourceGuard {
    channels: Arc<RwLock<HashMap<ChannelId, Arc<ChannelRegistrationEntry>>>>,
}

impl ChannelSourceGuard {
    /// Registry-owned mount validation: does `channel_id` still denote the
    /// Legacy registration published with `source_revision`?
    ///
    /// Holds the channel map for one read only — no lifecycle gate, no I/O — so
    /// dispatch can call this before status, handler, or inbound work without
    /// any registry lock outliving it.
    async fn validate_legacy_webhook(
        &self,
        channel_id: &ChannelId,
        source_revision: u64,
    ) -> ChannelResult<()> {
        let channels = self.channels.read().await;
        let published = channels.get(channel_id).is_some_and(|entry| {
            matches!(
                entry.registration,
                ChannelRegistration::Legacy {
                    publication_revision
                } if publication_revision == source_revision
            )
        });
        if published {
            return Ok(());
        }
        Err(ChannelError::NotConnected(format!(
            "webhook mount for {channel_id} is not its published Legacy registration"
        )))
    }
}

#[async_trait::async_trait]
impl super::webhook_receiver::LegacyWebhookGuard for ChannelSourceGuard {
    async fn validate_legacy_webhook(
        &self,
        channel_id: &ChannelId,
        source_revision: u64,
    ) -> ChannelResult<()> {
        ChannelSourceGuard::validate_legacy_webhook(self, channel_id, source_revision).await
    }
}

/// Central registry for all channel instances
pub struct ChannelRegistry {
    /// Registered channel instances
    channels: Arc<RwLock<HashMap<ChannelId, Arc<ChannelRegistrationEntry>>>>,
    /// Private identity shared by this source's immutable publications.
    source: Arc<()>,
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
    /// Serializes registration/replacement with lifecycle snapshots and starts.
    /// This is a synchronization boundary only; channel maps remain the sole
    /// instance authority and desired_state remains the sole intent authority.
    lifecycle_sync: AsyncMutex<()>,
    /// Publication revision for the channel source. It advances only after a
    /// registration or unregistration is committed to `channels`.
    source_revision: AtomicU64,
    /// Per-channel *intent* (see [`DesiredChannelState`]). Written only by the
    /// lifecycle methods on this type, read by the health monitor.
    desired_state: RwLock<HashMap<ChannelId, DesiredChannelState>>,
}

impl ChannelRegistry {
    /// Create a new channel registry
    #[must_use]
    pub fn new() -> Self {
        let (inbound_tx, inbound_rx) = broadcast::channel(1000);
        let channels: Arc<RwLock<HashMap<ChannelId, Arc<ChannelRegistrationEntry>>>> =
            Arc::new(RwLock::new(HashMap::new()));

        Self {
            channels: Arc::clone(&channels),
            source: Arc::new(()),
            factories: RwLock::new(HashMap::new()),
            inbound_tx: InboundMessageSender::from(inbound_tx),
            inbound_rx: Arc::new(Mutex::new(Some(inbound_rx))),
            voice_states: RwLock::new(HashMap::new()),
            send_retry: SendRetryPolicy::default(),
            delivery_store: None,
            webhook_mounts: Arc::new(super::webhook_receiver::WebhookMountTable::guarded_by(
                Arc::new(ChannelSourceGuard { channels }),
            )),
            lifecycle_sync: AsyncMutex::new(()),
            source_revision: AtomicU64::new(0),
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
        let config_id = ChannelId::new(&config.id);
        let operation = match self.channels.read().await.get(&config_id).cloned() {
            Some(entry) => Some((
                entry.clone(),
                self.begin_legacy_operation(&config_id, &entry).await?,
            )),
            None => None,
        };
        let factories = self.factories.read().await;
        let factory = match factories.get(&config.channel_type).cloned() {
            Some(factory) => factory,
            None => {
                if let Some((entry, marker)) = &operation {
                    self.finish_legacy_operation(entry, marker).await;
                }
                return Err(ChannelError::ConfigError(format!(
                    "No factory registered for channel type: {}",
                    config.channel_type
                )));
            }
        };
        drop(factories);

        let channel = match factory
            .create_with_id(&config.id, config.config.clone())
            .await
        {
            Ok(channel) => channel,
            Err(error) => {
                if let Some((entry, marker)) = &operation {
                    self.finish_legacy_operation(entry, marker).await;
                }
                return Err(error);
            }
        };
        let channel_id = channel.id().clone();

        if let Err(error) = self.reject_bound_legacy_access(&channel_id).await {
            if let Some((entry, marker)) = &operation {
                self.finish_legacy_operation(entry, marker).await;
            }
            return Err(error);
        }

        // Observe the replacement policy before touching the outgoing channel
        // runtime, webhook mount, or publication. DeliveryStore deliberately
        // latches observation failures fail closed; returning here therefore
        // retains the incumbent's live state while preserving that durable
        // safety fence.
        if let Err(error) =
            self.observe_delivery_policy(&channel_id, channel.outbound_delivery_policy())
        {
            warn!(
                channel = %channel_id,
                phase = "replacement_policy_observation",
                incumbent = "retained",
                runtime = "untouched",
                publication = "untouched",
                admission = "fail_closed_by_delivery_store",
                %error,
                "channel replacement refused before channel teardown"
            );
            if let Some((entry, marker)) = &operation {
                self.finish_legacy_operation(entry, marker).await;
            }
            return Err(error);
        }

        let stop_result = if let Some((entry, _marker)) = &operation {
            self.stop_channel_inner(&channel_id, entry).await
        } else {
            self.stop_outgoing_channel(&channel_id).await
        };
        if let Err(error) = stop_result {
            if let Some((entry, marker)) = &operation {
                self.finish_legacy_operation(entry, marker).await;
            }
            return Err(error);
        }

        // Same reasoning as `register`: the freshly-created instance has not
        // started, so drop whatever the outgoing instance at this id left
        // mounted before the replacement lands.
        self.webhook_mounts.unmount_channel(&channel_id).await;

        let mut channels = self.channels.write().await;
        // Stamp the publication while the map is locked: the mount built from
        // this entry later carries the stamp, and dispatch compares it against
        // this very entry.
        let publication_revision = self.advance_source_revision();
        channels.insert(
            channel_id.clone(),
            Arc::new(ChannelRegistrationEntry {
                channel: Arc::new(RwLock::new(channel)),
                registration: ChannelRegistration::Legacy {
                    publication_revision,
                },
                runtime: Mutex::new(binding::EntryRuntime::default()),
                drained: tokio::sync::Notify::new(),
            }),
        );
        drop(channels);

        // Preserve the configured boot intent: a successful replacement is
        // selected by `start_all` only when its config explicitly enables it.
        let boot_intent = if config.enabled {
            DesiredChannelState::Enabled
        } else {
            DesiredChannelState::Stopped
        };
        self.set_desired(&channel_id, boot_intent).await;

        if let Some((entry, marker)) = &operation {
            self.finish_legacy_operation(entry, marker).await;
        }

        info!(
            "Created channel: {} (type: {})",
            channel_id, config.channel_type
        );
        Ok(channel_id)
    }

    /// Register an existing channel instance with the normal boot-enabled intent.
    ///
    /// Public callers historically relied on `start_all` starting every channel
    /// they registered. Always writing `Enabled` preserves that contract and
    /// deliberately does not inherit a replaced instance's prior intent.
    pub async fn register(&self, channel: Box<dyn Channel>) -> ChannelId {
        self.register_with_boot_intent(channel, DesiredChannelState::Enabled)
            .await
    }

    /// Register a channel with an explicit boot-time enabled decision owned by
    /// this registry. The requested intent is written in the same registration
    /// operation, so a disabled boot registration is never briefly enabled.
    pub async fn register_for_boot(&self, channel: Box<dyn Channel>, enabled: bool) -> ChannelId {
        self.register_with_boot_intent(
            channel,
            if enabled {
                DesiredChannelState::Enabled
            } else {
                DesiredChannelState::Stopped
            },
        )
        .await
    }

    async fn register_with_boot_intent(
        &self,
        channel: Box<dyn Channel>,
        boot_intent: DesiredChannelState,
    ) -> ChannelId {
        let channel_id = channel.id().clone();
        if let Err(error) = self.reject_bound_legacy_access(&channel_id).await {
            warn!(channel = %channel_id, %error, "legacy registration refused; bound incumbent retained");
            return channel_id;
        }

        // Observe the replacement policy before touching the outgoing channel
        // runtime, webhook mount, or publication. DeliveryStore deliberately
        // latches observation failures fail closed; returning here therefore
        // retains the incumbent's live state while preserving that durable
        // safety fence.
        if let Err(error) =
            self.observe_delivery_policy(&channel_id, channel.outbound_delivery_policy())
        {
            warn!(
                channel = %channel_id,
                phase = "replacement_policy_observation",
                incumbent = "retained",
                runtime = "untouched",
                publication = "untouched",
                admission = "fail_closed_by_delivery_store",
                %error,
                "channel replacement refused before channel teardown"
            );
            return channel_id;
        }

        if let Err(error) = self.stop_outgoing_channel(&channel_id).await {
            // This legacy API returns only an id. Preserve the failed owner and
            // its structured residual instead of dropping failure evidence.
            warn!(
                channel = %channel_id,
                %error,
                "channel replacement refused; incumbent retained"
            );
            return channel_id;
        }

        // A replacement instance has not started, so it owns no handler. Drop
        // whatever the outgoing instance left mounted, or the route keeps
        // serving with the old secret until someone happens to start the new one.
        self.webhook_mounts.unmount_channel(&channel_id).await;

        let mut channels = self.channels.write().await;
        // Stamped under the map lock, exactly like `create_channel`: a retained
        // mount is refused once this id denotes a different publication.
        let publication_revision = self.advance_source_revision();
        channels.insert(
            channel_id.clone(),
            Arc::new(ChannelRegistrationEntry {
                channel: Arc::new(RwLock::new(channel)),
                registration: ChannelRegistration::Legacy {
                    publication_revision,
                },
                runtime: Mutex::new(binding::EntryRuntime::default()),
                drained: tokio::sync::Notify::new(),
            }),
        );
        drop(channels);

        // This unconditional write is intentional: replacement gets the new
        // registration's boot intent rather than inheriting the old instance's.
        self.desired_state
            .write()
            .await
            .insert(channel_id.clone(), boot_intent);
        info!("Registered channel: {}", channel_id);
        channel_id
    }

    /// Tear down the current channel before replacing or removing it.
    /// The entry operation marker serializes the same channel while the shared
    /// lifecycle gate remains available to unrelated channels.
    async fn stop_outgoing_channel(&self, channel_id: &ChannelId) -> ChannelResult<()> {
        let entry = self.channels.read().await.get(channel_id).cloned();
        let Some(entry) = entry else {
            return Ok(());
        };
        let marker = self.begin_legacy_operation(channel_id, &entry).await?;
        let result = self.stop_channel_inner(channel_id, &entry).await;
        self.finish_legacy_operation(&entry, &marker).await;
        result
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
        if let Err(error) = self.stop_outgoing_channel(channel_id).await {
            warn!(
                channel = %channel_id,
                %error,
                "channel removal refused; incumbent retained"
            );
            return None;
        }
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
        if let Some(entry) = channels.remove(channel_id) {
            self.advance_source_revision();
            // Legacy captures retain the entry as well as its channel handle.
            let channel_arc = Arc::try_unwrap(entry).ok()?.channel;
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

    /// Capture the current registered instance without starting or authenticating it.
    ///
    /// The capture and its source revision come from the same registry read
    /// boundary. The returned handle is opaque to callers; use admission must
    /// revalidate it through [`Self::validate_channel_capture`].
    #[allow(dead_code)]
    pub(crate) async fn capture_channel(&self, channel_id: &ChannelId) -> Option<ChannelCapture> {
        let _lifecycle = self.lifecycle_sync.lock().await;
        let channels = self.channels.read().await;
        let entry = channels.get(channel_id)?;
        if !matches!(entry.registration, ChannelRegistration::Legacy { .. }) {
            return None;
        }
        Some(ChannelCapture {
            channel_id: channel_id.clone(),
            source: self.source.clone(),
            entry: entry.clone(),
            source_revision: self
                .source_revision
                .load(std::sync::atomic::Ordering::Relaxed),
            handle: entry.channel.clone(),
        })
    }

    /// Revalidate a source-owned capture at the use boundary.
    ///
    /// An unrelated publication may advance the source revision without
    /// invalidating this entry. Replacement or unregister always changes the
    /// registered `Arc` identity, so stale work fails closed here.
    #[allow(dead_code)]
    pub(crate) async fn validate_channel_capture(
        &self,
        capture: &ChannelCapture,
    ) -> Option<ChannelHandle> {
        let _lifecycle = self.lifecycle_sync.lock().await;
        let channels = self.channels.read().await;
        let current = channels.get(&capture.channel_id)?;
        (Arc::ptr_eq(&self.source, &capture.source)
            && Arc::ptr_eq(current, &capture.entry)
            && matches!(current.registration, ChannelRegistration::Legacy { .. })
            && Arc::ptr_eq(&current.channel, &capture.handle))
        .then(|| current.channel.clone())
    }

    /// Advance the source publication revision and return the new value.
    ///
    /// Callers that publish an entry stamp it with the returned revision while
    /// holding the `channels` write lock, so the atomic cut and the stamped
    /// entry are never observed out of step and every stamp is unique.
    fn try_advance_source_revision(&self) -> Result<u64, binding::ChannelBindingError> {
        self.source_revision
            .try_update(
                std::sync::atomic::Ordering::Relaxed,
                std::sync::atomic::Ordering::Relaxed,
                |revision| revision.checked_add(1),
            )
            .map(|previous| previous + 1)
            .map_err(|_| binding::ChannelBindingError::PublicationExhausted)
    }

    fn advance_source_revision(&self) -> u64 {
        self.try_advance_source_revision()
            .expect("channel source revision exhausted")
    }

    fn prepare_bound_entry(
        &self,
        channel: Box<dyn Channel>,
        contract: ChannelLifecycleContract,
        ownership: Arc<OwnershipTree>,
        initial: ChannelInitialState,
        revision: u64,
    ) -> Result<Arc<ChannelRegistrationEntry>, ChannelBindingError> {
        let channel_id = channel.id().clone();
        if channel_id.as_str().is_empty() {
            return Err(ChannelBindingError::InvalidContract);
        }
        let provider = ProviderHandle::new(ProviderLifecycle::new(
            ProviderId::new(format!("aleph/gateway/channels/{channel_id}"))
                .map_err(|_| ChannelBindingError::InvalidContract)?,
            ProviderTransport::InProcess,
        ));
        match initial {
            ChannelInitialState::Discovered => {}
            ChannelInitialState::Cold => {
                provider.transition(ProviderState::Cold)?;
            }
            ChannelInitialState::Disabled => {
                provider.transition(ProviderState::Disabled)?;
            }
        }
        let descriptor = CapabilityDescriptor {
            id: CapabilityId {
                namespace: "aleph/gateway/channels".into(),
                name: channel_id.as_str().into(),
            },
            kind: CapabilityKind::Resource,
            schema: contract.schema,
            revision: CapabilityRevision(revision),
            owner: contract.owner.clone(),
            lifetime: contract.lifetime,
            visibility: contract.visibility.clone(),
            metadata: contract.metadata.clone(),
            services: Vec::new(),
            facets: CapabilityFacets::new().with(CapabilityFacet::ProviderLifecycle),
        };
        Ok(Arc::new(ChannelRegistrationEntry {
            channel: Arc::new(RwLock::new(channel)),
            runtime: Mutex::new(binding::EntryRuntime::default()),
            drained: tokio::sync::Notify::new(),
            registration: ChannelRegistration::Bound(Box::new(BoundChannelRegistration {
                source: self.source.clone(),
                entry_revision: revision,
                descriptor_fingerprint: canonical_descriptor_fingerprint(&descriptor),
                descriptor,
                contract,
                provider,
                ownership,
            })),
        }))
    }
    /// Publish one complete, cold lifecycle declaration. No runtime activation.
    #[allow(dead_code)]
    pub(crate) async fn register_bound_channel(
        &self,
        channel: Box<dyn Channel>,
        contract: ChannelLifecycleContract,
        ownership: Arc<OwnershipTree>,
        initial: ChannelInitialState,
    ) -> Result<ChannelId, ChannelBindingError> {
        let _lifecycle = self.lifecycle_sync.lock().await;
        let channel_id = channel.id().clone();
        let mut channels = self.channels.write().await;
        if channels.contains_key(&channel_id) {
            return Err(ChannelBindingError::Busy);
        }
        let revision = self
            .source_revision
            .load(std::sync::atomic::Ordering::Relaxed)
            .checked_add(1)
            .ok_or(ChannelBindingError::PublicationExhausted)?;
        let entry = self.prepare_bound_entry(channel, contract, ownership, initial, revision)?;
        // Acquire every publication lock before the first write: cancellation
        // cannot leave a half-published tuple or mismatched desired intent.
        let mut desired = self.desired_state.write().await;
        self.try_advance_source_revision()?;
        channels.insert(channel_id.clone(), entry);
        desired.insert(channel_id.clone(), DesiredChannelState::Stopped);
        Ok(channel_id)
    }

    #[allow(dead_code)]
    pub(crate) async fn capture_bound_channel(
        &self,
        id: &ChannelId,
    ) -> Result<BoundChannelCapture, ChannelBindingError> {
        let _lifecycle = self.lifecycle_sync.lock().await;
        let channels = self.channels.read().await;
        let entry = channels.get(id).ok_or(ChannelBindingError::NotFound)?;
        let ChannelRegistration::Bound(bound) = &entry.registration else {
            return Err(ChannelBindingError::UnboundLegacy);
        };
        Ok(BoundChannelCapture {
            source: bound.source.clone(),
            entry: entry.clone(),
            channel: entry.channel.clone(),
            source_revision: self
                .source_revision
                .load(std::sync::atomic::Ordering::Relaxed),
            entry_revision: bound.entry_revision,
            descriptor: bound.descriptor.clone(),
            descriptor_fingerprint: bound.descriptor_fingerprint,
            provider: bound.provider.clone(),
            provider_generation: bound.provider.snapshot().1,
        })
    }

    /// Exact-scope, source-local projection only; never creates ownership.
    #[allow(dead_code)]
    pub(crate) async fn snapshot_bound_channels(
        &self,
        visibility: &VisibilityScope,
    ) -> (Vec<CapabilityDescriptor>, u64) {
        let _lifecycle = self.lifecycle_sync.lock().await;
        let channels = self.channels.read().await;
        let mut descriptors: Vec<_> = channels
            .values()
            .filter_map(|entry| {
                let ChannelRegistration::Bound(bound) = &entry.registration else {
                    return None;
                };
                (&bound.descriptor.visibility == visibility).then(|| bound.descriptor.clone())
            })
            .collect();
        descriptors.sort_by(|a, b| a.id.name.cmp(&b.id.name));
        (
            descriptors,
            self.source_revision
                .load(std::sync::atomic::Ordering::Relaxed),
        )
    }

    /// Validate the current registration without exposing its Legacy handle.
    pub(crate) async fn require_legacy_channel(&self, id: &ChannelId) -> ChannelResult<()> {
        let channels = self.channels.read().await;
        match channels.get(id) {
            Some(entry) if matches!(entry.registration, ChannelRegistration::Legacy { .. }) => {
                Ok(())
            }
            Some(_) => Err(ChannelError::UnsupportedFeature(
                "bound channel runtime is not implemented".into(),
            )),
            None => Err(ChannelError::NotConnected(format!(
                "Channel not found: {id}"
            ))),
        }
    }

    async fn reject_bound_legacy_access(&self, id: &ChannelId) -> ChannelResult<()> {
        if self
            .channels
            .read()
            .await
            .get(id)
            .is_some_and(|entry| matches!(entry.registration, ChannelRegistration::Bound(_)))
        {
            return Err(ChannelError::UnsupportedFeature(
                "bound channel runtime is not implemented".into(),
            ));
        }
        Ok(())
    }

    /// Legacy execution adapters must never receive a Bound raw handle.
    async fn legacy_channel(&self, id: &ChannelId) -> ChannelResult<ChannelHandle> {
        let channels = self.channels.read().await;
        let entry = channels
            .get(id)
            .ok_or_else(|| ChannelError::NotConnected(format!("Channel not found: {id}")))?;
        match entry.registration {
            ChannelRegistration::Legacy { .. } => Ok(entry.channel.clone()),
            ChannelRegistration::Bound(_) => Err(ChannelError::UnsupportedFeature(
                "bound channel runtime is not implemented".into(),
            )),
        }
    }

    async fn legacy_entry(&self, id: &ChannelId) -> ChannelResult<Arc<ChannelRegistrationEntry>> {
        let channels = self.channels.read().await;
        let entry = channels
            .get(id)
            .cloned()
            .ok_or_else(|| ChannelError::NotConnected(format!("Channel not found: {id}")))?;
        if matches!(entry.registration, ChannelRegistration::Bound(_)) {
            return Err(ChannelError::UnsupportedFeature(
                "bound channel runtime is not implemented".into(),
            ));
        }
        Ok(entry)
    }

    async fn begin_legacy_operation(
        &self,
        id: &ChannelId,
        entry: &Arc<ChannelRegistrationEntry>,
    ) -> ChannelResult<Arc<()>> {
        let _source = self.lifecycle_sync.lock().await;
        let current = self
            .channels
            .read()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| ChannelError::NotConnected(format!("Channel not found: {id}")))?;
        if !Arc::ptr_eq(&current, entry) {
            return Err(ChannelError::Busy(format!(
                "Channel changed during lifecycle admission: {id}"
            )));
        }
        if !matches!(current.registration, ChannelRegistration::Legacy { .. }) {
            return Err(ChannelError::UnsupportedFeature(
                "bound channel runtime is not implemented".into(),
            ));
        }
        let mut runtime = entry
            .runtime
            .lock()
            .expect("channel runtime mutex poisoned");
        if runtime.operation.is_some() {
            return Err(ChannelError::Busy(format!(
                "Channel lifecycle operation already active: {id}"
            )));
        }
        let marker = Arc::new(());
        runtime.operation = Some(marker.clone());
        Ok(marker)
    }

    async fn finish_legacy_operation(
        &self,
        entry: &Arc<ChannelRegistrationEntry>,
        marker: &Arc<()>,
    ) {
        let _source = self.lifecycle_sync.lock().await;
        let mut runtime = entry
            .runtime
            .lock()
            .expect("channel runtime mutex poisoned");
        if runtime
            .operation
            .as_ref()
            .is_some_and(|active| Arc::ptr_eq(active, marker))
        {
            runtime.operation = None;
        }
    }

    /// Get a legacy channel by ID. Bound handles remain source-private.
    pub async fn get(&self, channel_id: &ChannelId) -> Option<ChannelHandle> {
        self.legacy_channel(channel_id).await.ok()
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
            let channel = handle.channel.read().await;
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

        for entry in channels.values() {
            let channel = entry.channel.read().await;
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

        for entry in channels.values() {
            let channel = entry.channel.read().await;
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
    /// channel and for a Bound registration, which owns no Legacy publication.
    /// The mount carries the publication stamp of `entry`, so dispatch can tell
    /// whether this id still denotes that registration.
    /// Shared by `start_channel` and `restart_channel`, the two sites where
    /// `start()` (re)builds the handler.
    fn webhook_mount_for(
        channel: &dyn Channel,
        channel_id: &ChannelId,
        entry: &ChannelRegistrationEntry,
    ) -> Option<super::webhook_receiver::WebhookMount> {
        let publication_revision = match entry.registration {
            ChannelRegistration::Legacy {
                publication_revision,
            } => publication_revision,
            ChannelRegistration::Bound(_) => return None,
        };
        channel
            .webhook_handler()
            .map(|handler| super::webhook_receiver::WebhookMount {
                handler,
                inbound: channel.state().sender(),
                status: channel.state().status_handle(),
                channel_id: channel_id.clone(),
                source_revision: publication_revision,
            })
    }

    /// Start a channel
    pub async fn start_channel(&self, channel_id: &ChannelId) -> ChannelResult<()> {
        if self
            .channels
            .read()
            .await
            .get(channel_id)
            .is_some_and(|entry| matches!(entry.registration, ChannelRegistration::Bound(_)))
        {
            return self
                .start_bound_channel_compat(channel_id)
                .await
                .map_err(|error| ChannelError::Internal(error.to_string()));
        }
        let entry = self.legacy_entry(channel_id).await?;
        let marker = self.begin_legacy_operation(channel_id, &entry).await?;
        let result = self.start_channel_inner(channel_id, &entry).await;
        self.finish_legacy_operation(&entry, &marker).await;
        result
    }

    async fn start_channel_inner(
        &self,
        channel_id: &ChannelId,
        channel_arc: &Arc<ChannelRegistrationEntry>,
    ) -> ChannelResult<()> {
        let entry = channel_arc;
        let channel_arc = channel_arc.channel.clone();

        let mut channel = channel_arc.write().await;
        channel.start().await?;

        // A webhook channel only materialises its handler in `start()`, so this
        // is the earliest point the mount can exist. The sink is the channel's
        // OWN broadcast so `start_message_forwarder` still stamps channel
        // health — going to the registry's sender directly would make a
        // receiving channel look dead to the health monitor.
        let mount = Self::webhook_mount_for(&**channel, channel_id, entry);

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
        if self
            .channels
            .read()
            .await
            .get(channel_id)
            .is_some_and(|entry| matches!(entry.registration, ChannelRegistration::Bound(_)))
        {
            return self
                .stop_bound_channel_compat(channel_id)
                .await
                .map_err(|error| ChannelError::Internal(error.to_string()));
        }
        let entry = self.legacy_entry(channel_id).await?;
        let marker = self.begin_legacy_operation(channel_id, &entry).await?;
        let result = self.stop_channel_inner(channel_id, &entry).await;
        self.finish_legacy_operation(&entry, &marker).await;
        result
    }

    async fn stop_channel_inner(
        &self,
        channel_id: &ChannelId,
        entry: &Arc<ChannelRegistrationEntry>,
    ) -> ChannelResult<()> {
        let channel_arc = entry.channel.clone();

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

    /// Start all configured-enabled channels.
    pub async fn start_all(&self) -> Vec<(ChannelId, ChannelResult<()>)> {
        let desired = self.desired_state.read().await;
        let channel_ids: Vec<ChannelId> = desired
            .iter()
            .filter_map(|(id, state)| {
                (*state == DesiredChannelState::Enabled).then_some(id.clone())
            })
            .collect();
        drop(desired);

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

    /// Submit one public telecom reply directly to its transport.
    ///
    /// The result acknowledges only `Channel::send`, not receipt by the caller.
    /// The public driver bounds the entire sink call (including lock acquisition).
    /// Deliberately bypasses owner observers, durable queue preparation/flush,
    /// persistence, and registry retries, even for an Automatic channel.
    pub(crate) async fn send_public_telecom(
        &self,
        channel_id: &ChannelId,
        message: OutboundMessage,
    ) -> ChannelResult<SendResult> {
        let channel_arc = self.legacy_channel(channel_id).await?;
        let channel = channel_arc.read().await;
        channel.send(message).await
    }

    /// Cancel-safe pre-submit variant of [`Self::send_public_telecom`].
    /// `Ok(None)` proves cancellation was observed before `Channel::send` was
    /// polled; no transport call occurred. Both registry lookup and channel lock
    /// waits are cancellable. After the final check, polling `Channel::send` is
    /// the commit boundary: cancellation cannot retract it and its eventual ACK
    /// is returned normally. The caller still bounds the entire delivery time.
    /// Like the unfenced public path, this bypasses hooks, queues and retries.
    pub(crate) async fn send_public_telecom_if_not_cancelled(
        &self,
        channel_id: &ChannelId,
        message: OutboundMessage,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> ChannelResult<Option<SendResult>> {
        if cancel.is_cancelled() {
            return Ok(None);
        }
        let channel_arc = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Ok(None),
            channel = self.legacy_channel(channel_id) => channel?,
        };
        let channel = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Ok(None),
            channel = channel_arc.read() => channel,
        };
        if cancel.is_cancelled() {
            return Ok(None);
        }
        // No await or cancellation select between this fence and the first
        // poll of send. Once committed, only the caller's delivery timeout can
        // abandon an ACK (with an unknown external outcome, never retracted).
        channel.send(message).await.map(Some)
    }

    /// Ask the actual registered channel, never its advertised type string.
    pub(crate) async fn private_sms_lease(
        &self,
        channel_id: &ChannelId,
        ctx: &super::telecom::TelecomPublicContext,
        recipient: &str,
    ) -> Option<super::interfaces::sms::PrivateSmsLease> {
        let channel = self.get(channel_id).await?;
        let channel = channel.read().await;
        channel.private_sms_lease(ctx, recipient).await
    }

    /// Pinned private send. Registry locks are precommit waits, not the commit
    /// boundary. Only the concrete transport may commit after its final fence.
    pub(crate) async fn send_approved_public_telecom(
        &self,
        channel_id: &ChannelId,
        message: OutboundMessage,
        lease: &super::interfaces::sms::PrivateSmsLease,
        cancel: &tokio_util::sync::CancellationToken,
        validate: &(dyn for<'a> Fn(&'a str) -> bool + Send + Sync),
    ) -> ChannelResult<ApprovedTelecomSend> {
        if cancel.is_cancelled() {
            return Ok(ApprovedTelecomSend::Cancelled);
        }
        if !lease.is_active() {
            return Ok(ApprovedTelecomSend::Invalid);
        }
        let channel_arc = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Ok(ApprovedTelecomSend::Cancelled),
            _ = lease.revoked() => return Ok(ApprovedTelecomSend::Invalid),
            channel = self.legacy_channel(channel_id) => channel?,
        };
        let channel = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Ok(ApprovedTelecomSend::Cancelled),
            _ = lease.revoked() => return Ok(ApprovedTelecomSend::Invalid),
            channel = channel_arc.read() => channel,
        };
        if cancel.is_cancelled() {
            return Ok(ApprovedTelecomSend::Cancelled);
        }
        if !validate(&message.text) {
            return Ok(ApprovedTelecomSend::Invalid);
        }
        if cancel.is_cancelled() {
            return Ok(ApprovedTelecomSend::Cancelled);
        }
        if !lease.is_active() {
            return Ok(ApprovedTelecomSend::Invalid);
        }
        // No ordinary-send fallback, observers, persistence, queue or retry.
        // The pinned transport checks again under its own last async lock.
        channel
            .send_private_sms(message, lease, cancel, validate)
            .await
    }

    /// Send a message through a specific channel.
    ///
    /// On a *definitely-not-delivered* failure (channel down / not yet
    /// connected, or an exhausted rate-limit) the message is persisted to the
    /// attached [`delivery_store`](Self::with_delivery_store), if any, for
    /// background retry — then the original error is still returned so callers
    /// observe unchanged semantics. When no store is attached this is a
    /// zero-overhead passthrough to `send_attempt`.
    ///
    /// When a store *is* attached, this message's conversation gets its queued
    /// backlog flushed first
    /// (`flush_conversation`) so a
    /// live send cannot overtake replies that failed earlier. That flush is
    /// deliberately invisible from here: it settles nothing this call reports
    /// on, and whether it ran or was skipped, the value returned below is the
    /// same one this method has always returned.
    pub async fn send(
        &self,
        channel_id: &ChannelId,
        message: OutboundMessage,
    ) -> ChannelResult<SendResult> {
        self.send_reported(channel_id, message).await.result
    }

    /// [`send`](Self::send) plus what actually happened to a failed message:
    /// the policy of the channel instance that carried the transport call and
    /// the queue's real disposition. Callers that decide "is the rest of this
    /// reply still worth offering?" must ask this, not re-derive it from the
    /// error class — an error the queue *accepts* in principle was not
    /// necessarily queued (explicit-only channel, no store, persist failure).
    pub async fn send_reported(
        &self,
        channel_id: &ChannelId,
        message: OutboundMessage,
    ) -> SendReport {
        if let Err(error) = self.reject_bound_legacy_access(channel_id).await {
            return SendReport {
                result: Err(error),
                policy: None,
                queue: QueueDisposition::NotQueueable,
            };
        }
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
        let attempt = self
            .send_attempt(channel_id, &message, AttemptKind::Fresh)
            .await;
        match attempt.result {
            Ok(sent) => SendReport {
                result: Ok(sent),
                policy: attempt.policy,
                queue: QueueDisposition::Delivered,
            },
            Err(e) => {
                let queue = self
                    .maybe_enqueue(channel_id, &message, &e, attempt.policy)
                    .await;
                SendReport {
                    result: Err(e),
                    policy: attempt.policy,
                    queue,
                }
            }
        }
    }

    /// Persist a transient outbound failure for durable retry, if a store is
    /// attached and the error is duplicate-safe to retry. Best-effort: a
    /// persistence failure is logged and swallowed (the original send error is
    /// what the caller sees).
    ///
    /// `policy` is the one captured from the channel instance that actually
    /// carried the transport call. It is never re-resolved by id here: the
    /// channel can have been unregistered or replaced while the send was in
    /// flight, and an absent registration is not permission to persist the body
    /// of a message an explicit-only channel just failed to send.
    async fn maybe_enqueue(
        &self,
        channel_id: &ChannelId,
        message: &OutboundMessage,
        err: &ChannelError,
        policy: Option<OutboundDeliveryPolicy>,
    ) -> QueueDisposition {
        let Some(store) = &self.delivery_store else {
            return QueueDisposition::NoStore;
        };
        if !super::delivery_queue::should_enqueue(err) {
            return QueueDisposition::NotQueueable;
        }
        // An explicit-only channel's failure is the caller's to see and to
        // re-request: it must not be stored (the body would be persisted in
        // plaintext) nor replayed later without a fresh instruction. A channel
        // that never produced a policy (not registered) is still refused when
        // the store remembers it declared one.
        let explicit = match policy {
            Some(OutboundDeliveryPolicy::ExplicitOnly) => true,
            _ => match store.is_explicit_only_channel(channel_id.as_str()) {
                Ok(explicit) => explicit,
                Err(_) => return QueueDisposition::PolicyRefused,
            },
        };
        if explicit {
            return QueueDisposition::PolicyRefused;
        }
        // Ordered after the cheap refusals above so a message that is never
        // going to be queued never pays for a file read.
        let custody =
            super::delivery_queue::take_media_custody(message, store.config().max_payload_bytes)
                .await;
        let message = custody.as_ref().unwrap_or(message);
        let next =
            super::delivery_queue::now_secs() + store.config().initial_backoff.as_secs() as i64;
        match store.enqueue_fresh(channel_id.as_str(), message, &format!("{err:?}"), next) {
            Ok(super::delivery_queue::EnqueueOutcome::Queued(_)) => {
                info!(
                    channel = %channel_id,
                    "outbound send failed transiently; queued for durable retry"
                );
                QueueDisposition::Queued
            }
            // Reported distinctly rather than folded into the success log: the
            // message is *not* coming back, and the operator's next question is
            // "which one?" — answered by the dead-letter trail the store wrote.
            Ok(super::delivery_queue::EnqueueOutcome::TooLarge { bytes }) => {
                warn!(
                    channel = %channel_id,
                    bytes,
                    "outbound payload exceeds the durable-queue byte cap; dead-lettered instead of retried"
                );
                QueueDisposition::DeadLettered
            }
            Ok(super::delivery_queue::EnqueueOutcome::PolicyRefused) => {
                QueueDisposition::PolicyRefused
            }
            Err(_) => {
                warn!(
                    channel = %channel_id,
                    "failed to persist outbound delivery for retry"
                );
                QueueDisposition::PersistFailed
            }
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
        kind: AttemptKind<'_>,
    ) -> Attempt {
        let channel_arc = match self.legacy_channel(channel_id).await {
            Ok(channel) => channel,
            Err(error) => {
                return Attempt {
                    result: Err(error),
                    policy: None,
                    replay_refused: false,
                }
            }
        };

        self.send_on(&channel_arc, channel_id, message, kind).await
    }

    /// The transport leg of [`send_attempt`](Self::send_attempt), on the
    /// instance and policy it already resolved.
    async fn send_on(
        &self,
        channel_arc: &Arc<RwLock<Box<dyn Channel>>>,
        channel_id: &ChannelId,
        message: &OutboundMessage,
        kind: AttemptKind<'_>,
    ) -> Attempt {
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
        //
        // An explicit-only channel never gets the automatic retry: each extra
        // transport call is a second submission the caller did not ask for.
        let mut retries_left = self.send_retry.max_rate_limit_retries;
        let mut is_retry = false;
        loop {
            // Revalidate on every acquisition: `get` exposes this Arc and a
            // writer can replace its Box during an observer or retry wait.
            // This exact guard carries both the policy and the transport call.
            let channel = channel_arc.read().await;
            let policy = channel.outbound_delivery_policy();
            if (matches!(kind, AttemptKind::Replay { .. }) || is_retry)
                && policy == OutboundDeliveryPolicy::ExplicitOnly
            {
                // Refuse only. This Arc may be stale (replaced by a newer
                // registration of the same id), so it has no authority to
                // classify the id or scrub rows: register/prepare own that.
                return Attempt {
                    result: Err(ChannelError::ConfigError(
                        "explicit-only channel: replay refused".into(),
                    )),
                    policy: Some(policy),
                    replay_refused: true,
                };
            }
            if let AttemptKind::Replay { store, record_id } = kind {
                // A claimed body can outlive its row (scrub followed by an
                // Automatic replacement). Validate row and marker while
                // stamping the actual attempt under one store transaction.
                if !matches!(store.begin_replay(record_id, channel_id.as_str()), Ok(true)) {
                    return Attempt {
                        result: Err(ChannelError::ConfigError(
                            "delivery row/policy unavailable: replay refused".into(),
                        )),
                        policy: Some(policy),
                        replay_refused: true,
                    };
                }
            }
            let result = if channel.status() == ChannelStatus::Disabled {
                Err(ChannelError::NotConnected(format!(
                    "Channel {channel_id} is disabled"
                )))
            } else {
                channel.send(message.clone()).await
            };
            drop(channel);

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
                    return Attempt {
                        result: Ok(sent),
                        policy: Some(policy),
                        replay_refused: false,
                    };
                }
                Err(ChannelError::RateLimited { retry_after_secs })
                    if retries_left > 0 && policy == OutboundDeliveryPolicy::Automatic =>
                {
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
                    is_retry = true;
                    tokio::time::sleep(wait).await;
                }
                Err(e) => {
                    return Attempt {
                        result: Err(e),
                        policy: Some(policy),
                        replay_refused: false,
                    }
                }
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
        let channel_arc = self.legacy_channel(channel_id).await?;

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
        let channel_arc = self.legacy_channel(channel_id).await?;

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
        let channel_arc = self.legacy_channel(channel_id).await?;
        // NOTE: guard held across await (see `edit`).
        let channel = channel_arc.read().await;
        channel.send_typing(conversation_id).await
    }

    /// Create a poll in a conversation through a specific channel.
    ///
    /// R8 (tools = everything): iMessage's BlueBubbles adapter exposes
    /// `createPoll`; this is the single channel-registry entry point the
    /// `channel_message` tool dispatches through. Channels without poll
    /// support inherit the trait default `ChannelError::UnsupportedFeature`.
    pub async fn create_poll(
        &self,
        channel_id: &ChannelId,
        conversation_id: &ConversationId,
        question: &str,
        options: &[String],
        allow_multiple: bool,
    ) -> ChannelResult<super::channel::MessageId> {
        let channel_arc = self.legacy_channel(channel_id).await?;
        let channel = channel_arc.read().await;
        channel
            .create_poll(conversation_id, question, options, allow_multiple)
            .await
    }

    /// Set the icon of a group conversation through a specific channel.
    ///
    /// iMessage-native (BlueBubbles `setChatIcon`). Same R8 rationale as
    /// `create_poll`. `icon_data_url` is a `data:` URL carrying the image
    /// payload; adapters are responsible for parsing it.
    pub async fn set_group_icon(
        &self,
        channel_id: &ChannelId,
        conversation_id: &ConversationId,
        icon_data_url: &str,
    ) -> ChannelResult<()> {
        let channel_arc = self.legacy_channel(channel_id).await?;
        let channel = channel_arc.read().await;
        channel.set_group_icon(conversation_id, icon_data_url).await
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
        let channel_arc = self.legacy_channel(channel_id).await?;

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

        for entry in channels.values() {
            let channel = entry.channel.read().await;
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

        for entry in channels.values() {
            let channel = entry.channel.read().await;
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
        for (id, entry) in channels.iter() {
            let channel = entry.channel.read().await;
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
        if self
            .channels
            .read()
            .await
            .get(channel_id)
            .is_some_and(|entry| matches!(entry.registration, ChannelRegistration::Bound(_)))
        {
            self.stop_bound_channel_compat(channel_id)
                .await
                .map_err(|error| ChannelError::Internal(error.to_string()))?;
            return self
                .start_bound_channel_compat(channel_id)
                .await
                .map_err(|error| ChannelError::Internal(error.to_string()));
        }
        let entry = self.legacy_entry(channel_id).await?;
        let marker = self.begin_legacy_operation(channel_id, &entry).await?;
        let result = self.restart_channel_inner(channel_id, &entry).await;
        self.finish_legacy_operation(&entry, &marker).await;
        result
    }

    async fn restart_channel_inner(
        &self,
        channel_id: &ChannelId,
        entry: &Arc<ChannelRegistrationEntry>,
    ) -> ChannelResult<()> {
        let channel_arc = entry.channel.clone();

        let mut channel = channel_arc.write().await;
        channel.stop().await?;
        if let Err(error) = channel.start().await {
            drop(channel);
            // A successful stop leaves the live transport disconnected, so a
            // failed replacement start must clear Running intent. Only a stop
            // error above preserves the prior intent and returns unchanged.
            self.set_desired(channel_id, DesiredChannelState::Stopped)
                .await;
            return Err(error);
        }

        // This path does NOT go through stop_channel/start_channel, so it needs
        // its own refresh: `start()` builds a NEW handler and the table would
        // otherwise keep serving the pre-restart clone. If `start()` yielded no
        // handler this time, unmount instead of leaving the stale one behind.
        let mount = Self::webhook_mount_for(&**channel, channel_id, entry);
        drop(channel);

        match mount {
            Some(mount) => {
                self.webhook_mounts.mount(mount).await;
            }
            None => {
                self.webhook_mounts.unmount_channel(channel_id).await;
            }
        }

        // Only a complete stop+start is a successful restart. Keeping this
        // write after both fallible lifecycle calls preserves the prior intent
        // on failure and prevents a failed restart from fabricating Running.
        self.set_desired(channel_id, DesiredChannelState::Running)
            .await;

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

    fn observe_delivery_policy(
        &self,
        channel_id: &ChannelId,
        policy: OutboundDeliveryPolicy,
    ) -> ChannelResult<()> {
        if let Some(store) = &self.delivery_store {
            store
                .observe_channel_policy(channel_id.as_str(), policy)
                .map_err(|_| ChannelError::Internal("delivery policy persistence failed".into()))?;
        }
        Ok(())
    }

    /// Lifecycle/drain/tool-entry privacy hook. No map lock crosses channel I/O.
    /// Tool consumption and richer outbox reporting belong to the next stage.
    pub async fn prepare_delivery_store(&self) -> ChannelResult<()> {
        if let Some(store) = &self.delivery_store {
            self.prepare_delivery_store_on(store).await?;
        }
        Ok(())
    }

    pub(crate) async fn prepare_delivery_store_on(
        &self,
        store: &super::delivery_queue::DeliveryStore,
    ) -> ChannelResult<()> {
        let handles: Vec<_> = self
            .channels
            .read()
            .await
            .iter()
            .map(|(id, handle)| (id.clone(), handle.clone()))
            .collect();
        for (id, entry) in handles {
            if matches!(entry.registration, ChannelRegistration::Bound(_)) {
                continue;
            }
            let channel = entry.channel.read().await;
            // Recheck map identity after acquiring the channel guard. No map
            // guard crosses a channel await; it spans only synchronous store
            // observation so a stale Arc cannot overwrite a new registration.
            // Lock order is map -> instance everywhere else, so blocking on
            // the map while holding the instance guard can close a fair-RwLock
            // cycle. Never wait here: contention is a fail-closed, retryable
            // refusal (the instance guard drops on return).
            let Ok(channels) = self.channels.try_read() else {
                return Err(ChannelError::Internal(
                    "delivery registry busy; retry".into(),
                ));
            };
            if channels
                .get(&id)
                .is_some_and(|current| Arc::ptr_eq(current, &entry))
                && channel.outbound_delivery_policy() == OutboundDeliveryPolicy::ExplicitOnly
            {
                store
                    .observe_channel_policy(id.as_str(), OutboundDeliveryPolicy::ExplicitOnly)
                    .map_err(|_| {
                        ChannelError::Internal("delivery policy persistence failed".into())
                    })?;
            }
        }
        store
            .scrub_marked_channels()
            .map_err(|_| ChannelError::Internal("delivery payload retirement failed".into()))?;
        Ok(())
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
    async fn real_bound(
        url: String,
    ) -> (
        Arc<ChannelRegistry>,
        ChannelId,
        super::test_support::ResourceProbe,
        Arc<OwnershipTree>,
    ) {
        use super::binding::ChannelLifecycleDeclarationV1;
        use crate::capability::descriptor::ProjectionMetadata;
        use crate::capability::ownership::{LifetimeScope, OwnerRef, VisibilityScope};
        use crate::gateway::interfaces::webhook::{WebhookChannel, WebhookChannelConfig};
        let probe = super::test_support::ResourceProbe::default();
        let tree = Arc::new(OwnershipTree::new());
        let registry = Arc::new(ChannelRegistry::new());
        let id = registry
            .register_bound_channel(
                Box::new(WebhookChannel::observed(
                    "real-bound",
                    WebhookChannelConfig {
                        secret: "test-secret".into(),
                        callback_url: url,
                        ..Default::default()
                    },
                    probe.clone(),
                )),
                ChannelLifecycleContract::new(
                    OwnerRef::Runtime,
                    LifetimeScope::Runtime,
                    VisibilityScope::default(),
                    ChannelLifecycleDeclarationV1::schema(),
                    ProjectionMetadata::default(),
                )
                .unwrap(),
                tree.clone(),
                ChannelInitialState::Cold,
            )
            .await
            .unwrap();
        let capture = registry.capture_bound_channel(&id).await.unwrap();
        tree.register(
            capture.descriptor.id.clone(),
            capture.descriptor.owner.clone(),
            capture.descriptor.lifetime,
            capture.descriptor.visibility.clone(),
        )
        .unwrap();
        (registry, id, probe, tree)
    }

    #[tokio::test]
    async fn bound_webhook_stop_drains_without_deadlock() {
        use super::binding::BoundCompletion;
        use crate::capability::ownership::VisibilityScope;
        let peer = super::test_support::LoopbackPeer::new(200).await;
        let (registry, id, probe, _) = real_bound(peer.url).await;
        registry
            .start_bound_channel(&id)
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let capture = registry.capture_bound_channel(&id).await.unwrap();
        assert_eq!(capture.provider.snapshot(), (ProviderState::Ready, 1));
        let lease = registry
            .resolve_bound_channel(capture, &VisibilityScope::default())
            .await
            .unwrap();
        let result = registry
            .send_bound_once(lease, OutboundMessage::text("test", "real effect"))
            .await
            .unwrap();
        let bytes = peer.arrived.await.unwrap();
        assert!(String::from_utf8_lossy(&bytes).contains("real effect"));
        let stop = registry.stop_bound_channel(&id).await.unwrap();
        assert_eq!(
            registry
                .capture_bound_channel(&id)
                .await
                .unwrap()
                .provider
                .state(),
            ProviderState::Stopping
        );
        let stale = registry
            .resolve_bound_channel(
                registry.capture_bound_channel(&id).await.unwrap(),
                &VisibilityScope::default(),
            )
            .await;
        assert!(stale.is_err());
        assert_eq!(probe.count("client_drop"), 0);
        peer.release.send(()).unwrap();
        assert!(matches!(
            result.await.unwrap(),
            BoundCompletion::Accepted(Ok(_))
        ));
        stop.await.unwrap().unwrap();
        peer.task.await.unwrap();
        assert_eq!(capture_state(&registry, &id).await, ProviderState::Stopped);
        assert_eq!(probe.count("client_built"), 1);
        assert_eq!(probe.count("client_drop"), 1);
        let entry = registry.channels.read().await.get(&id).unwrap().clone();
        assert_eq!(entry.runtime.lock().unwrap().active_work, 0);
        assert!(entry.runtime.lock().unwrap().forwarder.is_none());
    }

    async fn capture_state(registry: &ChannelRegistry, id: &ChannelId) -> ProviderState {
        registry
            .capture_bound_channel(id)
            .await
            .unwrap()
            .provider
            .state()
    }

    #[tokio::test]
    async fn bound_webhook_owner_change_rejects_completion() {
        use super::binding::BoundCompletion;
        use crate::capability::ownership::{LifetimeScope, OwnerRef, TaskId, VisibilityScope};
        for change in 0..4 {
            let peer = super::test_support::LoopbackPeer::new(200).await;
            let (registry, id, _, tree) = real_bound(peer.url).await;
            registry
                .start_bound_channel(&id)
                .await
                .unwrap()
                .await
                .unwrap()
                .unwrap();
            let capture = registry.capture_bound_channel(&id).await.unwrap();
            let capability = capture.descriptor.id.clone();
            let lease = registry
                .resolve_bound_channel(capture, &VisibilityScope::default())
                .await
                .unwrap();
            let result = registry
                .send_bound_once(lease, OutboundMessage::text("test", "held"))
                .await
                .unwrap();
            peer.arrived.await.unwrap();
            match change {
                0 => {
                    tree.bump(LifetimeScope::Runtime).unwrap();
                }
                1 => {
                    tree.revoke(&capability);
                }
                2 => {
                    tree.dispose(LifetimeScope::Runtime);
                }
                _ => {
                    tree.register(
                        capability,
                        OwnerRef::Task(TaskId("new-owner".into())),
                        LifetimeScope::Task,
                        VisibilityScope::default(),
                    )
                    .unwrap();
                }
            }
            peer.release.send(()).unwrap();
            let BoundCompletion::DiscardedStale(error) = result.await.unwrap() else {
                panic!("stale owner must reject completion");
            };
            assert!(matches!(error, ChannelBindingError::Owner(_)));
            assert_eq!(capture_state(&registry, &id).await, ProviderState::Ready);
            registry
                .stop_bound_channel(&id)
                .await
                .unwrap()
                .await
                .unwrap()
                .unwrap();
            peer.task.await.unwrap();
        }
    }

    #[tokio::test]
    async fn bound_webhook_cancelled_observer_keeps_work_owned() {
        use crate::capability::ownership::VisibilityScope;
        let peer = super::test_support::LoopbackPeer::new(200).await;
        let (registry, id, probe, _) = real_bound(peer.url).await;
        registry
            .start_bound_channel(&id)
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let lease = registry
            .resolve_bound_channel(
                registry.capture_bound_channel(&id).await.unwrap(),
                &VisibilityScope::default(),
            )
            .await
            .unwrap();
        let observer = registry
            .send_bound_once(lease, OutboundMessage::text("test", "cancelled observer"))
            .await
            .unwrap();
        peer.arrived.await.unwrap();
        drop(observer);
        let stop = registry.stop_bound_channel(&id).await.unwrap();
        assert_eq!(probe.count("client_drop"), 0);
        let entry = registry.channels.read().await.get(&id).unwrap().clone();
        assert_eq!(entry.runtime.lock().unwrap().active_work, 1);
        peer.release.send(()).unwrap();
        stop.await.unwrap().unwrap();
        peer.task.await.unwrap();
        assert_eq!(entry.runtime.lock().unwrap().active_work, 0);
        assert_eq!(probe.count("client_drop"), 1);
    }

    fn stage_b_contract(surface: Option<&str>) -> ChannelLifecycleContract {
        use super::binding::ChannelLifecycleDeclarationV1;
        use crate::capability::descriptor::ProjectionMetadata;
        use crate::capability::ownership::{LifetimeScope, OwnerRef, VisibilityScope};

        ChannelLifecycleContract::new(
            OwnerRef::Runtime,
            LifetimeScope::Runtime,
            VisibilityScope::default(),
            ChannelLifecycleDeclarationV1::schema(),
            ProjectionMetadata {
                surface: surface.map(str::to_owned),
            },
        )
        .unwrap()
    }

    #[tokio::test]
    async fn bound_retirement_discards_original_completion() {
        use super::binding::BoundCompletion;
        use crate::capability::ownership::VisibilityScope;
        use std::time::Duration;
        let peer = super::test_support::LoopbackPeer::new(200).await;
        let (registry, id, _, tree) = real_bound(peer.url.clone()).await;
        registry
            .start_bound_channel(&id)
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let capture = registry.capture_bound_channel(&id).await.unwrap();
        let lease = registry
            .resolve_bound_channel(capture.clone(), &VisibilityScope::default())
            .await
            .unwrap();
        let completion = registry
            .send_bound_once(lease, OutboundMessage::text("test", "retire me"))
            .await
            .unwrap();
        peer.arrived.await.unwrap();

        let successor = WebhookChannel::observed(
            "real-bound",
            WebhookChannelConfig {
                secret: "successor-secret".into(),
                callback_url: peer.url.clone(),
                ..Default::default()
            },
            super::test_support::ResourceProbe::default(),
        );
        let replacement = tokio::spawn({
            let registry = registry.clone();
            let replacement_id = id.clone();
            async move {
                registry
                    .replace_bound_channel(
                        &replacement_id,
                        Box::new(successor),
                        stage_b_contract(Some("successor")),
                        tree,
                        ChannelInitialState::Cold,
                    )
                    .await
            }
        });
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if capture.entry.runtime.lock().unwrap().retiring {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        peer.release.send(()).unwrap();
        let stale = completion.await.unwrap();
        assert!(matches!(
            stale,
            BoundCompletion::DiscardedStale(ChannelBindingError::StaleEntry)
                | BoundCompletion::DiscardedStale(ChannelBindingError::Retiring)
        ));
        assert!(replacement.await.unwrap().is_ok());
        assert_eq!(capture.entry.runtime.lock().unwrap().active_work, 0);
        assert!(registry.capture_bound_channel(&id).await.is_ok());
        peer.task.await.unwrap();
    }

    #[tokio::test]
    async fn bound_replace_publishes_whole_tuple() {
        use super::binding::BoundCompletion;
        use crate::capability::descriptor::ProjectionMetadata;
        use crate::capability::ownership::VisibilityScope;
        use std::time::Duration;
        let peer = super::test_support::LoopbackPeer::new(200).await;
        let (registry, id, _, tree) = real_bound(peer.url.clone()).await;
        registry
            .start_bound_channel(&id)
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let before = registry.capture_bound_channel(&id).await.unwrap();
        let before_revision = before.source_revision;
        let lease = registry
            .resolve_bound_channel(before.clone(), &VisibilityScope::default())
            .await
            .unwrap();
        let completion = registry
            .send_bound_once(lease, OutboundMessage::text("test", "publication boundary"))
            .await
            .unwrap();
        peer.arrived.await.unwrap();
        let successor_probe = super::test_support::ResourceProbe::default();
        let successor = WebhookChannel::observed(
            "real-bound",
            WebhookChannelConfig {
                secret: "successor-secret".into(),
                callback_url: peer.url.clone(),
                ..Default::default()
            },
            successor_probe,
        );
        let replacement = tokio::spawn({
            let registry = registry.clone();
            let id = id.clone();
            async move {
                registry
                    .replace_bound_channel(
                        &id,
                        Box::new(successor),
                        stage_b_contract(Some("replacement")),
                        tree,
                        ChannelInitialState::Cold,
                    )
                    .await
            }
        });
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if before.entry.runtime.lock().unwrap().retiring {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let during = registry.capture_bound_channel(&id).await.unwrap();
        assert!(Arc::ptr_eq(&before.entry, &during.entry));
        assert_eq!(during.source_revision, before_revision);
        assert_eq!(during.provider.state(), ProviderState::Stopping);
        peer.release.send(()).unwrap();
        assert!(matches!(
            completion.await.unwrap(),
            BoundCompletion::DiscardedStale(_)
        ));
        assert_eq!(replacement.await.unwrap().unwrap(), id);
        let after = registry.capture_bound_channel(&id).await.unwrap();
        assert!(!Arc::ptr_eq(&before.entry, &after.entry));
        assert!(!before.provider.same_instance(&after.provider));
        assert_ne!(after.source_revision, before_revision);
        assert_ne!(before.descriptor_fingerprint, after.descriptor_fingerprint);
        assert_eq!(
            after.descriptor.metadata,
            ProjectionMetadata {
                surface: Some("replacement".into()),
            }
        );
        assert_eq!(after.provider.snapshot().0, ProviderState::Cold);
        peer.task.await.unwrap();
    }

    #[tokio::test]
    async fn bound_unregister_waits_for_cleanup() {
        use std::time::Duration;
        let peer = super::test_support::LoopbackPeer::new(200).await;
        let (registry, id, _, _) = real_bound(peer.url.clone()).await;
        registry
            .start_bound_channel(&id)
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let lease = registry
            .resolve_bound_channel(
                registry.capture_bound_channel(&id).await.unwrap(),
                &VisibilityScope::default(),
            )
            .await
            .unwrap();
        let completion = registry
            .send_bound_once(lease, OutboundMessage::text("test", "wait"))
            .await
            .unwrap();
        peer.arrived.await.unwrap();
        let mut unregister = tokio::spawn({
            let registry = registry.clone();
            let id = id.clone();
            async move { registry.unregister_bound_channel(&id).await }
        });
        let result = tokio::time::timeout(Duration::from_millis(20), &mut unregister).await;
        assert!(result.is_err(), "unregister must wait for real cleanup");
        assert!(registry.capture_bound_channel(&id).await.is_ok());
        peer.release.send(()).unwrap();
        assert!(completion.await.is_ok());
        assert!(unregister.await.unwrap().is_ok());
        peer.task.await.unwrap();
    }

    #[tokio::test]
    async fn bound_contract_update_retires_old_publication() {
        use crate::capability::ownership::VisibilityScope;
        let (registry, id, _, tree) = real_bound("http://127.0.0.1:1/callback".into()).await;
        registry
            .start_bound_channel(&id)
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let old = registry.capture_bound_channel(&id).await.unwrap();
        let old_lease = registry
            .resolve_bound_channel(old.clone(), &VisibilityScope::default())
            .await
            .unwrap();
        registry
            .replace_bound_channel(
                &id,
                Box::new(WebhookChannel::new(
                    "real-bound",
                    WebhookChannelConfig {
                        secret: "contract-update".into(),
                        callback_url: "http://127.0.0.1:1/callback".into(),
                        ..Default::default()
                    },
                )),
                stage_b_contract(Some("contract-update")),
                tree,
                ChannelInitialState::Cold,
            )
            .await
            .unwrap();
        assert!(registry
            .resolve_bound_channel(old, &VisibilityScope::default())
            .await
            .is_err());
        drop(old_lease);
    }

    #[tokio::test]
    async fn bound_failed_cleanup_retains_incumbent() {
        let probe = super::test_support::ResourceProbe::default();
        let tree = Arc::new(OwnershipTree::new());
        let registry = Arc::new(ChannelRegistry::new());
        let id = registry
            .register_bound_channel(
                Box::new(super::test_support::FailingStopWebhook::new(
                    "failed-cleanup",
                    "http://127.0.0.1:1/callback".into(),
                    probe,
                )),
                stage_b_contract(None),
                tree.clone(),
                ChannelInitialState::Cold,
            )
            .await
            .unwrap();
        let capture = registry.capture_bound_channel(&id).await.unwrap();
        tree.register(
            capture.descriptor.id.clone(),
            capture.descriptor.owner.clone(),
            capture.descriptor.lifetime,
            capture.descriptor.visibility.clone(),
        )
        .unwrap();
        registry
            .start_bound_channel(&id)
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let error = registry.unregister_bound_channel(&id).await.unwrap_err();
        assert!(matches!(error, ChannelBindingError::Runtime { .. }));
        assert!(registry.capture_bound_channel(&id).await.is_ok());
        let entry = registry.channels.read().await.get(&id).unwrap().clone();
        assert!(entry.runtime.lock().unwrap().operation.is_some());
    }

    #[tokio::test]
    async fn bound_revision_exhaustion_never_half_publishes() {
        use std::sync::atomic::Ordering;
        use std::time::Duration;
        let (registry, id, _, tree) = real_bound("http://127.0.0.1:1/callback".into()).await;
        let before = registry.capture_bound_channel(&id).await.unwrap();
        registry.source_revision.store(u64::MAX, Ordering::Relaxed);
        let result = registry
            .replace_bound_channel(
                &id,
                Box::new(WebhookChannel::new(
                    "real-bound",
                    WebhookChannelConfig::default(),
                )),
                stage_b_contract(Some("exhausted")),
                tree,
                ChannelInitialState::Cold,
            )
            .await;
        assert!(matches!(
            result,
            Err(ChannelBindingError::PublicationExhausted)
        ));
        let after = registry.capture_bound_channel(&id).await.unwrap();
        assert!(Arc::ptr_eq(&before.entry, &after.entry));
        assert_eq!(after.source_revision, u64::MAX);
        assert_eq!(before.provider.snapshot(), after.provider.snapshot());

        let peer = super::test_support::LoopbackPeer::new(200).await;
        let (registry, id, _, tree) = real_bound(peer.url.clone()).await;
        registry
            .start_bound_channel(&id)
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let before = registry.capture_bound_channel(&id).await.unwrap();
        let lease = registry
            .resolve_bound_channel(before.clone(), &VisibilityScope::default())
            .await
            .unwrap();
        let completion = registry
            .send_bound_once(lease, OutboundMessage::text("test", "revision race"))
            .await
            .unwrap();
        peer.arrived.await.unwrap();
        registry
            .source_revision
            .store(u64::MAX - 1, Ordering::Relaxed);
        let replacement = tokio::spawn({
            let registry = registry.clone();
            let id = id.clone();
            let peer_url = peer.url.clone();
            async move {
                registry
                    .replace_bound_channel(
                        &id,
                        Box::new(WebhookChannel::new(
                            "real-bound",
                            WebhookChannelConfig {
                                callback_url: peer_url,
                                ..Default::default()
                            },
                        )),
                        stage_b_contract(Some("late-successor")),
                        tree,
                        ChannelInitialState::Cold,
                    )
                    .await
            }
        });
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if before.entry.runtime.lock().unwrap().retiring {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        registry
            .register_bound_channel(
                Box::new(WebhookChannel::new(
                    "revision-consumer",
                    WebhookChannelConfig {
                        callback_url: "http://127.0.0.1:1/callback".into(),
                        ..Default::default()
                    },
                )),
                stage_b_contract(None),
                Arc::new(OwnershipTree::new()),
                ChannelInitialState::Cold,
            )
            .await
            .unwrap();
        assert_eq!(
            registry
                .source_revision
                .load(std::sync::atomic::Ordering::Relaxed),
            u64::MAX
        );
        peer.release.send(()).unwrap();
        assert!(matches!(
            completion.await.unwrap(),
            super::binding::BoundCompletion::DiscardedStale(_)
        ));
        assert!(matches!(
            replacement.await.unwrap().unwrap_err(),
            ChannelBindingError::PublicationExhausted
        ));
        let after = registry.capture_bound_channel(&id).await.unwrap();
        assert!(Arc::ptr_eq(&before.entry, &after.entry));
        assert_eq!(after.provider.state(), ProviderState::Stopped);
        assert_eq!(after.source_revision, u64::MAX);
        peer.task.await.unwrap();
        use super::binding::BoundCompletion;
        use super::test_support::{HeldFactory, HeldWebhook, LifecycleBlock, LoopbackPeer};
        use crate::capability::ownership::VisibilityScope;
        use crate::gateway::interfaces::webhook::{WebhookChannel, WebhookChannelConfig};
        for mode in [
            "start",
            "stop",
            "restart-start",
            "restart-stop",
            "start-all",
            "factory",
            "replace",
            "create-replace",
            "unregister",
        ] {
            let peer = LoopbackPeer::new(200).await;
            let (registry, bound_id, probe, _) = real_bound(peer.url).await;
            registry
                .start_bound_channel(&bound_id)
                .await
                .unwrap()
                .await
                .unwrap()
                .unwrap();
            let entry = registry
                .channels
                .read()
                .await
                .get(&bound_id)
                .unwrap()
                .clone();
            let observation = registry
                .resolve_bound_channel(
                    registry.capture_bound_channel(&bound_id).await.unwrap(),
                    &VisibilityScope::default(),
                )
                .await
                .unwrap();
            let completion = registry
                .send_bound_once(observation, OutboundMessage::text("test", mode))
                .await
                .unwrap();
            peer.arrived.await.unwrap();
            let (entered, waiting) = tokio::sync::oneshot::channel();
            let (release, blocked) = tokio::sync::oneshot::channel();
            let block = LifecycleBlock {
                entered,
                release: blocked,
            };
            let config = WebhookChannelConfig {
                secret: "test-secret".into(),
                callback_url: "http://127.0.0.1:1/".into(),
                ..Default::default()
            };
            let mut held = HeldWebhook {
                channel: WebhookChannel::new("held-legacy", config.clone()),
                start: None,
                stop: None,
            };
            if mode == "factory" {
                registry
                    .register_factory(Arc::new(HeldFactory(Mutex::new(Some(block)))))
                    .await;
            } else if matches!(mode, "start" | "restart-start" | "start-all") {
                held.start = Some(block);
            } else {
                held.stop = Some(block);
            }
            let legacy_id = registry.register(Box::new(held)).await;
            if !matches!(mode, "start" | "start-all" | "restart-start") {
                registry.start_channel(&legacy_id).await.unwrap();
            }
            if mode == "create-replace" {
                registry
                    .register_factory(Arc::new(
                        crate::gateway::interfaces::webhook::WebhookChannelFactory,
                    ))
                    .await;
            }
            let driver = {
                let registry = registry.clone();
                let legacy_id = legacy_id.clone();
                tokio::spawn(async move {
                    match mode {
                        "start" => registry.start_channel(&legacy_id).await,
                        "stop" => registry.stop_channel(&legacy_id).await,
                        "restart-start" | "restart-stop" => {
                            registry.restart_channel(&legacy_id).await
                        }
                        "start-all" => {
                            let results = registry.start_all().await;
                            assert_eq!(results.len(), 1);
                            results.into_iter().next().unwrap().1
                        }
                        "factory" | "create-replace" => registry
                            .create_channel(ChannelConfig {
                                id: legacy_id.to_string(),
                                channel_type: if mode == "factory" {
                                    "held-webhook"
                                } else {
                                    "webhook"
                                }
                                .into(),
                                enabled: true,
                                config: serde_json::to_value(config).unwrap(),
                            })
                            .await
                            .map(|_| ()),
                        "replace" => {
                            registry
                                .register(Box::new(WebhookChannel::new("held-legacy", config)))
                                .await;
                            Ok(())
                        }
                        "unregister" => {
                            registry.unregister(&legacy_id).await;
                            Ok(())
                        }
                        _ => unreachable!(),
                    }
                })
            };
            waiting.await.unwrap();
            let progress = tokio::time::timeout(Duration::from_secs(2), async {
                // Both completion handoff and drain require the shared source gate.
                let stop = registry.stop_bound_channel(&bound_id).await.unwrap();
                peer.release.send(()).unwrap();
                assert!(matches!(
                    completion.await.unwrap(),
                    BoundCompletion::Accepted(Ok(_))
                ));
                stop.await.unwrap().unwrap();
                assert_eq!(entry.runtime.lock().unwrap().active_work, 0);
                assert!(entry.runtime.lock().unwrap().forwarder.is_none());
                assert_eq!(probe.count("client_drop"), 1);
                registry
                    .start_bound_channel(&bound_id)
                    .await
                    .unwrap()
                    .await
                    .unwrap()
                    .unwrap();
                registry
                    .stop_bound_channel(&bound_id)
                    .await
                    .unwrap()
                    .await
                    .unwrap()
                    .unwrap();
                // A competing operation must not reach the same Legacy adapter.
                assert!(
                    registry.stop_channel(&legacy_id).await.is_err(),
                    "{mode}: same-entry operation must fail closed"
                );
            })
            .await;
            release.send(()).unwrap();
            driver.await.unwrap().unwrap();
            // Timeout is a deadlock watchdog, never evidence of scheduling order.
            assert!(
                progress.is_ok(),
                "{mode}: Legacy I/O blocked Bound completion/drain/lifecycle"
            );
            peer.task.await.unwrap();
            registry.stop_channel(&legacy_id).await.ok();
        }
    }

    #[tokio::test]
    async fn bound_webhook_construct_and_capture_are_cold() {
        use super::binding::ChannelLifecycleDeclarationV1;
        use super::test_support::ResourceProbe;
        use crate::capability::descriptor::ProjectionMetadata;
        use crate::capability::ownership::{
            LifetimeScope, OwnerRef, OwnershipTree, VisibilityScope,
        };
        use crate::gateway::interfaces::webhook::{WebhookChannel, WebhookChannelConfig};
        let probe = ResourceProbe::default();
        let channel = WebhookChannel::observed(
            "cold-real-webhook",
            WebhookChannelConfig {
                secret: "test-secret".into(),
                callback_url: "http://127.0.0.1:1/callback".into(),
                ..Default::default()
            },
            probe.clone(),
        );
        let registry = ChannelRegistry::new();
        let id = registry
            .register_bound_channel(
                Box::new(channel),
                ChannelLifecycleContract::new(
                    OwnerRef::Runtime,
                    LifetimeScope::Runtime,
                    VisibilityScope::default(),
                    ChannelLifecycleDeclarationV1::schema(),
                    ProjectionMetadata::default(),
                )
                .unwrap(),
                Arc::new(OwnershipTree::new()),
                ChannelInitialState::Cold,
            )
            .await
            .unwrap();
        for _ in 0..3 {
            registry.capture_bound_channel(&id).await.unwrap();
            registry
                .snapshot_bound_channels(&VisibilityScope::default())
                .await;
        }
        assert_eq!(
            probe.count("client_build_attempt"),
            0,
            "cold declaration must not construct an HTTP runtime client"
        );
        assert_eq!(probe.count("client_built"), 0);
    }

    use super::*;
    use crate::gateway::channel::{ChannelState, MessageId};
    use crate::gateway::interfaces::webhook::{WebhookChannel, WebhookChannelConfig};
    use chrono::Utc;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

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

    #[tokio::test]
    async fn create_channel_is_started_by_start_all() {
        let registry = ChannelRegistry::new();
        registry
            .register_factory(Arc::new(
                crate::gateway::interfaces::webhook::WebhookChannelFactory,
            ))
            .await;
        let id = registry
            .create_channel(ChannelConfig {
                id: "created-webhook".to_string(),
                channel_type: "webhook".to_string(),
                enabled: true,
                config: serde_json::json!({
                    "secret": "test-secret",
                    "callback_url": "https://example.com/callback",
                    "path": "/webhook/created"
                }),
            })
            .await
            .unwrap();

        let results = registry.start_all().await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, id);
        assert!(results[0].1.is_ok());
        let snapshot = registry
            .health_states()
            .await
            .into_iter()
            .find(|snapshot| snapshot.id == id)
            .expect("created channel remains observable");
        assert_eq!(snapshot.desired, DesiredChannelState::Running);
        assert_eq!(snapshot.status, ChannelStatus::Connected);
    }

    #[tokio::test]
    async fn disabled_create_config_stays_stopped_across_start_all() {
        let registry = ChannelRegistry::new();
        registry
            .register_factory(Arc::new(
                crate::gateway::interfaces::webhook::WebhookChannelFactory,
            ))
            .await;
        let id = registry
            .create_channel(ChannelConfig {
                id: "created-disabled-webhook".to_string(),
                channel_type: "webhook".to_string(),
                enabled: false,
                config: serde_json::json!({
                    "secret": "test-secret",
                    "callback_url": "https://example.com/callback",
                    "path": "/webhook/created-disabled"
                }),
            })
            .await
            .unwrap();

        assert!(registry.start_all().await.is_empty());
        let snapshot = registry
            .health_states()
            .await
            .into_iter()
            .find(|snapshot| snapshot.id == id)
            .expect("disabled created channel remains observable");
        assert_eq!(snapshot.desired, DesiredChannelState::Stopped);
        assert_eq!(snapshot.status, ChannelStatus::Disconnected);
    }

    #[tokio::test]
    async fn create_policy_failure_retains_active_feishu_incumbent() {
        use super::super::delivery_queue::{DeliveryQueueConfig, DeliveryStore};

        let temp = tempfile::tempdir().unwrap();
        let store_path = temp.path().join("delivery.sqlite");
        let store =
            Arc::new(DeliveryStore::open(&store_path, DeliveryQueueConfig::default()).unwrap());
        let registry = ChannelRegistry::new().with_delivery_store(store.clone());
        let id = ChannelId::new("create-policy-failure-feishu");
        let feishu_config = serde_json::from_value::<
            crate::gateway::interfaces::feishu::FeishuConfig,
        >(serde_json::json!({
            "app_id": "create-policy-failure-feishu",
            "app_secret": "test-secret"
        }))
        .unwrap();

        registry
            .register(Box::new(
                crate::gateway::interfaces::feishu::FeishuChannel::for_test(
                    id.as_str(),
                    feishu_config,
                )
                .unwrap(),
            ))
            .await;
        registry.start_channel(&id).await.unwrap();
        let incumbent = registry.get(&id).await.unwrap();
        assert_eq!(incumbent.read().await.status(), ChannelStatus::Connected);
        assert_eq!(registry.webhook_mounts().mounted_count().await, 0);
        assert!(
            crate::gateway::interfaces::feishu::api_handle::get(id.as_str()).is_none(),
            "for_test Feishu has no publication before replacement"
        );

        // The store is the existing delivery-policy test double. The trigger
        // fails the replacement's ExplicitOnly observation after the store has
        // latched its fail-closed policy-failure witness, but before teardown.
        rusqlite::Connection::open(&store_path)
            .unwrap()
            .execute_batch(
                "CREATE TRIGGER reject_policy_observation
                 BEFORE INSERT ON explicit_only_channels
                 BEGIN SELECT RAISE(ABORT, 'fixture policy observation rejected'); END;",
            )
            .unwrap();
        registry
            .register_factory(Arc::new(PolicyTestChannelFactory {
                policy: OutboundDeliveryPolicy::ExplicitOnly,
            }))
            .await;

        let result = registry
            .create_channel(ChannelConfig {
                id: id.to_string(),
                channel_type: "policy-test-webhook".to_string(),
                enabled: false,
                config: serde_json::json!({
                    "path": "/webhook/create-policy-failure"
                }),
            })
            .await;
        assert!(result.is_err());

        assert!(
            Arc::ptr_eq(&incumbent, &registry.get(&id).await.unwrap()),
            "policy preflight failure must retain the incumbent registration"
        );
        let snapshot = registry
            .health_states()
            .await
            .into_iter()
            .find(|snapshot| snapshot.id == id)
            .expect("incumbent remains observable after refused replacement");
        assert_eq!(snapshot.desired, DesiredChannelState::Running);
        assert_eq!(snapshot.status, ChannelStatus::Connected);
        assert_eq!(incumbent.read().await.status(), ChannelStatus::Connected);
        assert_eq!(registry.webhook_mounts().mounted_count().await, 0);
        assert!(
            crate::gateway::interfaces::feishu::api_handle::get(id.as_str()).is_none(),
            "the pre-existing test-mode publication state remains unchanged"
        );
        assert!(
            store.is_explicit_only_channel(id.as_str()).is_err(),
            "the delivery store's failed observation remains a structured fail-closed admission residual"
        );
    }

    #[tokio::test]
    async fn register_policy_failure_retains_active_feishu_incumbent() {
        use super::super::delivery_queue::{DeliveryQueueConfig, DeliveryStore};

        let temp = tempfile::tempdir().unwrap();
        let store_path = temp.path().join("delivery.sqlite");
        let store =
            Arc::new(DeliveryStore::open(&store_path, DeliveryQueueConfig::default()).unwrap());
        let registry = ChannelRegistry::new().with_delivery_store(store.clone());
        let id = ChannelId::new("register-policy-failure-feishu");
        let feishu_config = serde_json::from_value::<
            crate::gateway::interfaces::feishu::FeishuConfig,
        >(serde_json::json!({
            "app_id": "register-policy-failure-feishu",
            "app_secret": "test-secret"
        }))
        .unwrap();

        registry
            .register_for_boot(
                Box::new(
                    crate::gateway::interfaces::feishu::FeishuChannel::for_test(
                        id.as_str(),
                        feishu_config,
                    )
                    .unwrap(),
                ),
                true,
            )
            .await;
        registry.start_all().await;
        let incumbent = registry.get(&id).await.unwrap();
        assert_eq!(incumbent.read().await.status(), ChannelStatus::Connected);
        assert_eq!(registry.webhook_mounts().mounted_count().await, 0);
        assert!(
            crate::gateway::interfaces::feishu::api_handle::get(id.as_str()).is_none(),
            "for_test Feishu has no publication before replacement"
        );

        // Reuse the delivery-policy failure fixture: the store latches its
        // fail-closed witness before SQLite rejects the incoming observation.
        rusqlite::Connection::open(&store_path)
            .unwrap()
            .execute_batch(
                "CREATE TRIGGER reject_policy_observation
                 BEFORE INSERT ON explicit_only_channels
                 BEGIN SELECT RAISE(ABORT, 'fixture policy observation rejected'); END;",
            )
            .unwrap();
        let replacement = WebhookyChannel::new(id.as_str(), "/webhook/register-policy-failure")
            .with_delivery_policy(OutboundDeliveryPolicy::ExplicitOnly);

        let returned_id = registry
            .register_for_boot(Box::new(replacement), false)
            .await;
        assert_eq!(returned_id, id);
        assert!(
            Arc::ptr_eq(&incumbent, &registry.get(&id).await.unwrap()),
            "policy preflight failure must retain the incumbent registration"
        );
        let snapshot = registry
            .health_states()
            .await
            .into_iter()
            .find(|snapshot| snapshot.id == id)
            .expect("incumbent remains observable after refused replacement");
        assert_eq!(snapshot.desired, DesiredChannelState::Running);
        assert_eq!(snapshot.status, ChannelStatus::Connected);
        assert_eq!(incumbent.read().await.status(), ChannelStatus::Connected);
        assert_eq!(registry.webhook_mounts().mounted_count().await, 0);
        assert!(
            crate::gateway::interfaces::feishu::api_handle::get(id.as_str()).is_none(),
            "the pre-existing test-mode publication state remains unchanged"
        );
        assert!(
            store.is_explicit_only_channel(id.as_str()).is_err(),
            "the delivery store's failed observation remains a structured fail-closed admission residual"
        );
    }

    #[tokio::test]
    async fn registration_does_not_wait_on_shared_lifecycle_gate() {
        let registry = ChannelRegistry::new();
        let id = ChannelId::new("register-lifecycle-boundary");
        let lifecycle = registry.lifecycle_sync.lock().await;
        let result = tokio::time::timeout(
            Duration::from_secs(1),
            registry.register(Box::new(WebhookyChannel::new(
                id.as_str(),
                "/webhook/register-lifecycle-boundary",
            ))),
        )
        .await;

        assert!(
            result.is_ok(),
            "registration must not wait on an unrelated lifecycle gate"
        );
        assert_eq!(result.unwrap(), id);
        assert!(registry.get(&id).await.is_some());
        drop(lifecycle);
    }

    #[tokio::test]
    async fn public_register_remains_start_all_compatible() {
        let registry = ChannelRegistry::new();
        let id = registry
            .register(Box::new(FlakyChannel::new(
                0,
                false,
                Arc::new(AtomicU32::new(0)),
            )))
            .await;

        let results = registry.start_all().await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, id);
        assert!(results[0].1.is_ok());
    }

    #[tokio::test]
    async fn disabled_feishu_boot_has_no_runtime_effects_and_explicit_start_works() {
        let registry = ChannelRegistry::new();
        let config = serde_json::from_value::<crate::gateway::interfaces::feishu::FeishuConfig>(
            serde_json::json!({
                "app_id": "registry-disabled-feishu",
                "app_secret": "test-secret"
            }),
        )
        .unwrap();
        let id = ChannelId::new("registry-disabled-feishu");
        registry
            .register_for_boot(
                Box::new(
                    crate::gateway::interfaces::feishu::FeishuChannel::for_test(
                        id.as_str(),
                        config,
                    )
                    .unwrap(),
                ),
                false,
            )
            .await;

        let boot_results = registry.start_all().await;
        assert!(
            boot_results.is_empty(),
            "disabled boot must not start Feishu"
        );
        assert_eq!(registry.webhook_mounts().mounted_count().await, 0);
        assert!(
            crate::gateway::interfaces::feishu::api_handle::get(id.as_str()).is_none(),
            "disabled boot must not publish a Feishu API handle"
        );
        let snapshot = registry
            .health_states()
            .await
            .into_iter()
            .find(|snapshot| snapshot.id == id)
            .expect("registered disabled channel remains observable");
        assert_eq!(snapshot.desired, DesiredChannelState::Stopped);
        assert_eq!(snapshot.status, ChannelStatus::Disconnected);

        // Explicit enable uses the registry-owned start path and is allowed to
        // start a channel whose boot request was disabled.
        registry.start_channel(&id).await.unwrap();
        assert_eq!(
            registry.get(&id).await.unwrap().read().await.status(),
            ChannelStatus::Connected
        );
        registry.stop_channel(&id).await.unwrap();
    }

    #[tokio::test]
    async fn enabled_feishu_boot_uses_the_same_registry_lifecycle_path() {
        let registry = ChannelRegistry::new();
        let config = serde_json::from_value::<crate::gateway::interfaces::feishu::FeishuConfig>(
            serde_json::json!({
                "app_id": "registry-enabled-feishu",
                "app_secret": "test-secret"
            }),
        )
        .unwrap();
        let id = ChannelId::new("registry-enabled-feishu");
        registry
            .register_for_boot(
                Box::new(
                    crate::gateway::interfaces::feishu::FeishuChannel::for_test(
                        id.as_str(),
                        config,
                    )
                    .unwrap(),
                ),
                true,
            )
            .await;

        let boot_results = registry.start_all().await;
        assert_eq!(boot_results.len(), 1);
        assert!(boot_results[0].1.is_ok());
        let snapshot = registry
            .health_states()
            .await
            .into_iter()
            .find(|snapshot| snapshot.id == id)
            .expect("registered enabled channel remains observable");
        assert_eq!(snapshot.desired, DesiredChannelState::Running);
        assert_eq!(snapshot.status, ChannelStatus::Connected);

        registry.stop_channel(&id).await.unwrap();
        assert_eq!(registry.webhook_mounts().mounted_count().await, 0);
        assert!(
            crate::gateway::interfaces::feishu::api_handle::get(id.as_str()).is_none(),
            "test mode must not fabricate a publication"
        );
    }

    #[tokio::test]
    async fn disabled_replacement_stays_stopped_across_repeated_start_all() {
        let registry = ChannelRegistry::new();
        let config = || {
            serde_json::from_value::<crate::gateway::interfaces::feishu::FeishuConfig>(
                serde_json::json!({
                    "app_id": "registry-replacement-feishu",
                    "app_secret": "test-secret"
                }),
            )
            .unwrap()
        };
        let id = ChannelId::new("registry-replacement-feishu");

        registry
            .register_for_boot(
                Box::new(
                    crate::gateway::interfaces::feishu::FeishuChannel::for_test(
                        id.as_str(),
                        config(),
                    )
                    .unwrap(),
                ),
                true,
            )
            .await;
        assert_eq!(registry.start_all().await.len(), 1);

        for _ in 0..3 {
            registry
                .register_for_boot(
                    Box::new(
                        crate::gateway::interfaces::feishu::FeishuChannel::for_test(
                            id.as_str(),
                            config(),
                        )
                        .unwrap(),
                    ),
                    false,
                )
                .await;

            assert!(
                registry.start_all().await.is_empty(),
                "a disabled replacement must not be selected by start_all"
            );
            let snapshot = registry
                .health_states()
                .await
                .into_iter()
                .find(|snapshot| snapshot.id == id)
                .expect("replacement remains registered");
            assert_eq!(snapshot.desired, DesiredChannelState::Stopped);
            assert_eq!(snapshot.status, ChannelStatus::Disconnected);
            assert_eq!(registry.webhook_mounts().mounted_count().await, 0);
        }
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
        delivery_policy: OutboundDeliveryPolicy,
        retry_after_secs: u64,
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
                delivery_policy: OutboundDeliveryPolicy::Automatic,
                retry_after_secs: 0,
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
        fn outbound_delivery_policy(&self) -> OutboundDeliveryPolicy {
            self.delivery_policy
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
                    retry_after_secs: self.retry_after_secs,
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
    async fn public_telecom_cancellation_fences_registry_and_channel_lock_waits() {
        use std::future::{poll_fn, Future};
        use std::task::Poll;
        use tokio_util::sync::CancellationToken;

        let attempts = Arc::new(AtomicU32::new(0));
        let registry = registry_with(
            FlakyChannel::new(0, false, attempts.clone()),
            SendRetryPolicy::default(),
        )
        .await;
        let id = ChannelId::new("flaky");
        let handle = registry.get(&id).await.unwrap();
        for block_lookup in [true, false] {
            let registry_guard = if block_lookup {
                Some(registry.channels.write().await)
            } else {
                None
            };
            let channel_guard = if block_lookup {
                None
            } else {
                Some(handle.write().await)
            };
            let cancel = CancellationToken::new();
            let send = registry.send_public_telecom_if_not_cancelled(
                &id,
                OutboundMessage::text("c", "must-not-submit"),
                &cancel,
            );
            tokio::pin!(send);
            // A real first poll parks on the held lock; no sleeps/scheduler race.
            poll_fn(|cx| {
                assert!(send.as_mut().poll(cx).is_pending());
                Poll::Ready(())
            })
            .await;
            cancel.cancel();
            assert!(tokio::time::timeout(Duration::from_secs(1), send)
                .await
                .expect("cancellation must finish while the lock is still held")
                .unwrap()
                .is_none());
            assert_eq!(attempts.load(Ordering::SeqCst), 0);
            drop(channel_guard);
            drop(registry_guard);
        }
        // A cancelled token also fences resolution of a nonexistent channel.
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert!(registry
            .send_public_telecom_if_not_cancelled(
                &ChannelId::new("missing"),
                OutboundMessage::text("c", "must-not-submit"),
                &cancel,
            )
            .await
            .unwrap()
            .is_none());
        assert_eq!(attempts.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn approved_telecom_rejects_modified_outbound_bytes_before_send() {
        use tokio_util::sync::CancellationToken;

        let attempts = Arc::new(AtomicU32::new(0));
        let registry = registry_with(
            FlakyChannel::new(0, false, attempts.clone()),
            SendRetryPolicy::default(),
        )
        .await;
        let result = registry
            .send_approved_public_telecom(
                &ChannelId::new("flaky"),
                OutboundMessage::text("c", "altered"),
                &crate::gateway::interfaces::sms::transport::test_private_lease("binding")
                    .await
                    .1,
                &CancellationToken::new(),
                &|body| body == "approved",
            )
            .await
            .unwrap();
        assert!(matches!(result, ApprovedTelecomSend::Invalid));
        assert_eq!(attempts.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn approved_telecom_lock_wait_revalidates_retirement_and_expiry_before_submit() {
        use crate::gateway::telecom::public_approval::{
            ApprovalSource, PublicApprovalLedger, PublicApprovalScope, PublicApprovalStage,
            SMS_APPROVAL_TTL_SECS,
        };
        use crate::gateway::telecom::public_context::test_support::sample_public_context;
        use std::future::{poll_fn, Future};
        use std::task::Poll;
        use tokio_util::sync::CancellationToken;

        let attempts = Arc::new(AtomicU32::new(0));
        let registry = registry_with(
            FlakyChannel::new(0, false, attempts.clone()),
            SendRetryPolicy::default(),
        )
        .await;
        let id = ChannelId::new("flaky");
        let channel = registry.get(&id).await.unwrap();
        for (block_lookup, expire) in [(true, false), (false, true)] {
            let ledger = Arc::new(PublicApprovalLedger::new());
            let scope = PublicApprovalScope::new(
                &sample_public_context("precommit-binding"),
                "core",
                "agent",
                "turn",
                1,
            );
            let t0 = Utc::now();
            let op = ApprovalSource::operator_control_plane();
            let read = ledger.request_private_read(scope.clone(), "q", t0).unwrap();
            ledger.decide(&read, op, true, t0).unwrap();
            ledger
                .consume(&read, PublicApprovalStage::PrivateRead, op, &scope, "q", t0)
                .unwrap();
            let disclosure = ledger
                .request_disclosure(&read, scope.clone(), "secret", t0)
                .unwrap();
            ledger.decide(&disclosure, op, true, t0).unwrap();
            ledger
                .consume(
                    &disclosure,
                    PublicApprovalStage::Disclosure,
                    op,
                    &scope,
                    "secret",
                    t0,
                )
                .unwrap();
            let now = Arc::new(Mutex::new(t0));
            let check = |body: &str| {
                ledger
                    .validate_consumed_disclosure(
                        &disclosure,
                        &read,
                        &scope,
                        body,
                        *now.lock().unwrap(),
                    )
                    .is_ok()
            };
            let registry_guard = if block_lookup {
                Some(registry.channels.write().await)
            } else {
                None
            };
            let channel_guard = if block_lookup {
                None
            } else {
                Some(channel.write().await)
            };
            let cancel = CancellationToken::new();
            let (_transport, lease) =
                crate::gateway::interfaces::sms::transport::test_private_lease("binding").await;
            let send = registry.send_approved_public_telecom(
                &id,
                OutboundMessage::text("c", "secret"),
                &lease,
                &cancel,
                &check,
            );
            tokio::pin!(send);
            poll_fn(|cx| {
                assert!(send.as_mut().poll(cx).is_pending());
                Poll::Ready(())
            })
            .await;
            if expire {
                *now.lock().unwrap() = t0 + chrono::Duration::seconds(SMS_APPROVAL_TTL_SECS);
            } else {
                ledger.cancel_binding(scope.binding_ref());
            }
            drop(channel_guard);
            drop(registry_guard);
            assert!(matches!(send.await.unwrap(), ApprovedTelecomSend::Invalid));
            assert_eq!(
                attempts.load(Ordering::SeqCst),
                0,
                "invalidated body never submits"
            );
        }
    }

    #[tokio::test]
    async fn real_private_channel_registry_wait_fences_reconnect_expiry_and_body_changes() {
        use crate::gateway::interfaces::sms::{
            test_private_channel, transport::test_private_context,
        };
        use std::future::{poll_fn, Future};
        use std::task::Poll;
        use tokio_util::sync::CancellationToken;
        for block_registry in [true, false] {
            for invalidate in ["reconnect", "expiry", "body"] {
                let registry = ChannelRegistry::new();
                let (sms, transport, _lease, peer) = test_private_channel("binding").await;
                let id = registry.register(Box::new(sms)).await;
                let lease = registry
                    .private_sms_lease(&id, &test_private_context("binding"), "+15550000001")
                    .await
                    .unwrap();
                let channel = registry.get(&id).await.unwrap();
                let current_body = Mutex::new("secret");
                let deadline = Utc::now() + chrono::Duration::seconds(60);
                let now = Mutex::new(Utc::now());
                let check = |body: &str| {
                    body == *current_body.lock().unwrap() && *now.lock().unwrap() < deadline
                };
                let registry_guard = if block_registry {
                    Some(registry.channels.write().await)
                } else {
                    None
                };
                let channel_guard = if block_registry {
                    None
                } else {
                    Some(channel.write().await)
                };
                let cancel = CancellationToken::new();
                let send = registry.send_approved_public_telecom(
                    &id,
                    OutboundMessage::text("+15550000001", "secret"),
                    &lease,
                    &cancel,
                    &check,
                );
                tokio::pin!(send);
                poll_fn(|cx| {
                    assert!(send.as_mut().poll(cx).is_pending());
                    Poll::Ready(())
                })
                .await;
                match invalidate {
                    "reconnect" => {
                        transport.connect().await.unwrap();
                    }
                    "expiry" => {
                        *now.lock().unwrap() = deadline;
                    }
                    "body" => {
                        *current_body.lock().unwrap() = "mutated";
                    }
                    _ => unreachable!(),
                }
                if invalidate == "reconnect" {
                    assert!(
                        matches!(send.await.unwrap(), ApprovedTelecomSend::Invalid),
                        "revocation interrupts even held registry locks"
                    );
                    drop(channel_guard);
                    drop(registry_guard);
                } else {
                    drop(channel_guard);
                    drop(registry_guard);
                    assert!(matches!(send.await.unwrap(), ApprovedTelecomSend::Invalid));
                }
                assert!(
                    peer.submissions().is_empty(),
                    "{invalidate}/{block_registry}: no private peer writes"
                );
            }
        }
    }

    #[tokio::test]
    async fn registry_requires_concrete_witness_and_uses_private_path_without_ordinary_fallback() {
        use crate::gateway::interfaces::sms::{
            test_private_channel, transport::test_private_context,
        };
        use tokio_util::sync::CancellationToken;
        let attempts = Arc::new(AtomicU32::new(0));
        let mut fake = FlakyChannel::new(0, false, attempts.clone());
        fake.info.channel_type = "sms".into(); // A type string is still not authority.
        let registry = registry_with(fake, SendRetryPolicy::default()).await;
        let ctx = test_private_context("binding");
        assert!(registry
            .private_sms_lease(&ChannelId::new("flaky"), &ctx, "+15550000001")
            .await
            .is_none());
        let (sms, transport, lease, peer) = test_private_channel("binding").await;
        assert!(matches!(
            registry
                .send_approved_public_telecom(
                    &ChannelId::new("flaky"),
                    OutboundMessage::text("+15550000001", "secret"),
                    &lease,
                    &CancellationToken::new(),
                    &|_| true
                )
                .await
                .unwrap(),
            ApprovedTelecomSend::Invalid
        ));
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            0,
            "no ordinary send fallback"
        );
        let id = registry.register(Box::new(sms)).await;
        let live = registry
            .private_sms_lease(&id, &ctx, "+15550000001")
            .await
            .unwrap();
        assert!(matches!(
            registry
                .send_approved_public_telecom(
                    &id,
                    OutboundMessage::text("+15550000001", "secret"),
                    &live,
                    &CancellationToken::new(),
                    &|b| b == "secret"
                )
                .await
                .unwrap(),
            ApprovedTelecomSend::Acknowledged
        ));
        registry
            .send_public_telecom(&id, OutboundMessage::text("+15550000001", "ordinary"))
            .await
            .unwrap();
        assert_eq!(
            peer.submissions()
                .iter()
                .map(|op| op.command.body.as_str())
                .collect::<Vec<_>>(),
            vec!["secret", "ordinary"]
        );
        registry.stop_channel(&id).await.unwrap();
        assert!(
            !live.is_active(),
            "SmsChannel::stop disconnects the actual attached transport"
        );
        assert!(transport.lease().await.is_none());
        assert!(registry
            .private_sms_lease(&id, &ctx, "+15550000001")
            .await
            .is_none());
    }

    #[tokio::test]
    async fn public_telecom_transport_result_never_enters_durable_custody_or_retries() {
        use super::super::delivery_queue::{DeliveryQueueConfig, DeliveryStore};

        // Automatic policy is intentional: isolation must not depend on SMS's
        // ExplicitOnly declaration. A tiny cap also catches dead-letter writes.
        for (fail_times, max_payload_bytes) in [(10, usize::MAX), (0, usize::MAX), (10, 1)] {
            let temp = tempfile::tempdir().unwrap();
            let store = Arc::new(
                DeliveryStore::open(
                    &temp.path().join("delivery.db"),
                    DeliveryQueueConfig {
                        max_payload_bytes,
                        ..Default::default()
                    },
                )
                .unwrap(),
            );
            let attempts = Arc::new(AtomicU32::new(0));
            let registry = registry_with(
                FlakyChannel::new(fail_times, false, attempts.clone()),
                SendRetryPolicy::default(),
            )
            .await
            .with_delivery_store(store.clone());

            let result = registry
                .send_public_telecom(
                    &ChannelId::new("flaky"),
                    OutboundMessage::text("c", "public-body"),
                )
                .await;
            if fail_times == 0 {
                assert_eq!(result.unwrap().message_id, MessageId::new("ok"));
            } else {
                assert!(matches!(
                    result,
                    Err(ChannelError::RateLimited {
                        retry_after_secs: 0
                    })
                ));
            }
            assert_eq!(attempts.load(Ordering::SeqCst), 1, "no automatic retry");
            assert_eq!(store.len().unwrap(), 0, "no plaintext queued");
            assert!(store.recent_dead_letters(10).unwrap().is_empty());
        }
    }

    #[tokio::test]
    async fn public_telecom_does_not_flush_or_attempt_same_conversation_backlog() {
        use super::super::delivery_queue::{DeliveryQueueConfig, DeliveryStore, EnqueueOutcome};

        for fail_times in [0, 10] {
            let temp = tempfile::tempdir().unwrap();
            let store = Arc::new(
                DeliveryStore::open(
                    &temp.path().join("delivery.db"),
                    DeliveryQueueConfig::default(),
                )
                .unwrap(),
            );
            let attempts = Arc::new(AtomicU32::new(0));
            let registry = registry_with(
                FlakyChannel::new(fail_times, false, attempts.clone()),
                SendRetryPolicy::default(),
            )
            .await
            .with_delivery_store(store.clone());
            let id = ChannelId::new("flaky");
            let EnqueueOutcome::Queued(row) =
                super::super::delivery_queue::enqueue_validated_legacy(
                    &registry,
                    &store,
                    id.as_str(),
                    &OutboundMessage::text("c", "owner-backlog"),
                    "down",
                    0,
                )
                .await
                .unwrap()
            else {
                panic!("fixture must queue");
            };

            let result = registry
                .send_public_telecom(&id, OutboundMessage::text("c", "public-body"))
                .await;
            assert_eq!(result.is_ok(), fail_times == 0);
            assert_eq!(
                attempts.load(Ordering::SeqCst),
                1,
                "only the public transport call"
            );
            assert_eq!(store.len().unwrap(), 1);
            let backlog = store.claim_conversation(id.as_str(), "c", 10).unwrap();
            assert_eq!(backlog.len(), 1);
            assert_eq!(backlog[0].id, row);
            assert_eq!(backlog[0].message.text, "owner-backlog");
            assert_eq!(backlog[0].attempts, 0, "no backlog attempt stamped");
            assert!(store.recent_dead_letters(10).unwrap().is_empty());
        }
    }

    #[tokio::test]
    async fn ordinary_send_still_retries_and_enqueues_transient_failure() {
        use super::super::delivery_queue::{DeliveryQueueConfig, DeliveryStore};
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(
            DeliveryStore::open(
                &temp.path().join("delivery.db"),
                DeliveryQueueConfig::default(),
            )
            .unwrap(),
        );
        let attempts = Arc::new(AtomicU32::new(0));
        let registry = registry_with(
            FlakyChannel::new(10, false, attempts.clone()),
            SendRetryPolicy::default(),
        )
        .await
        .with_delivery_store(store.clone());
        let id = ChannelId::new("flaky");

        let result = registry
            .send(&id, OutboundMessage::text("c", "ordinary-body"))
            .await;
        assert!(matches!(result, Err(ChannelError::RateLimited { .. })));
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
        assert_eq!(store.len().unwrap(), 1);
        let queued = store.claim_conversation(id.as_str(), "c", 10).unwrap();
        assert_eq!(queued[0].message.text, "ordinary-body");
        assert_eq!(queued[0].attempts, 0);
        assert!(store.recent_dead_letters(10).unwrap().is_empty());
    }

    #[tokio::test]
    async fn replay_revalidates_policy_when_the_box_changes_inside_the_same_arc() {
        use super::super::delivery_queue::{DeadLetterReason, DeliveryQueueConfig, DeliveryStore};
        let attempts = Arc::new(AtomicU32::new(0));
        let replacement_attempts = Arc::new(AtomicU32::new(0));
        let mut automatic = FlakyChannel::new(10, false, attempts.clone());
        automatic.retry_after_secs = 1;
        let registry = Arc::new(registry_with(automatic, SendRetryPolicy::default()).await);
        let id = ChannelId::new("flaky");
        let handle = registry.get(&id).await.unwrap();
        let store =
            Arc::new(DeliveryStore::open_in_memory(DeliveryQueueConfig::default()).unwrap());
        let message = OutboundMessage::text("c", "private-replay-body");
        let row = match super::super::delivery_queue::enqueue_validated_legacy(
            &registry,
            &store,
            id.as_str(),
            &message,
            "down",
            0,
        )
        .await
        .unwrap()
        {
            super::super::delivery_queue::EnqueueOutcome::Queued(row) => row,
            outcome => panic!("unexpected fixture outcome: {outcome:?}"),
        };
        let run = {
            let registry = registry.clone();
            let store = store.clone();
            let id = id.clone();
            tokio::spawn(async move {
                registry
                    .send_attempt(
                        &id,
                        &message,
                        AttemptKind::Replay {
                            store: &store,
                            record_id: row,
                        },
                    )
                    .await
            })
        };
        tokio::time::timeout(Duration::from_secs(3), async {
            while attempts.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
            let mut explicit = FlakyChannel::new(0, false, replacement_attempts.clone());
            explicit.delivery_policy = OutboundDeliveryPolicy::ExplicitOnly;
            *handle.write().await = Box::new(explicit);
        })
        .await
        .unwrap();
        let attempt = tokio::time::timeout(Duration::from_secs(4), run)
            .await
            .unwrap()
            .unwrap();
        assert!(attempt.replay_refused);
        assert_eq!(attempt.policy, Some(OutboundDeliveryPolicy::ExplicitOnly));
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert_eq!(
            replacement_attempts.load(Ordering::SeqCst),
            0,
            "new Box must receive no replay"
        );
        // Refusal is not classification: the refused replay leaves the row for
        // the guarded prepare path, which sees the live ExplicitOnly policy.
        registry.prepare_delivery_store_on(&store).await.unwrap();
        assert!(store.is_empty());
        let letter = store.recent_dead_letters(1).unwrap().remove(0);
        assert_eq!(
            letter.reason,
            DeadLetterReason::UnknownOutcome,
            "first attempt was on wire"
        );
        assert!(letter.retired_by_policy);
        assert!(!format!("{letter:?}").contains("private-replay-body"));
    }

    /// F1 guard: `get_capabilities`/`list` hold the map read guard while they
    /// await the instance guard. `prepare` must therefore never *await* the map
    /// while holding the instance read guard. The future is polled by hand (no
    /// sleeps, no timeouts): after the instance writer leaves, prepare owns the
    /// instance read guard while a map writer is parked. It must fail closed
    /// (release the instance guard) instead of parking behind that writer.
    #[tokio::test]
    async fn prepare_never_awaits_the_map_while_holding_an_instance_guard() {
        use super::super::delivery_queue::{DeliveryQueueConfig, DeliveryStore};
        use std::future::{poll_fn, Future};
        use std::task::Poll;
        let attempts = Arc::new(AtomicU32::new(0));
        let store =
            Arc::new(DeliveryStore::open_in_memory(DeliveryQueueConfig::default()).unwrap());
        let registry = registry_with(
            FlakyChannel::new(0, false, attempts),
            SendRetryPolicy::default(),
        )
        .await
        .with_delivery_store(store.clone());
        let handle = registry.get(&ChannelId::new("flaky")).await.unwrap();

        let instance_writer = handle.write().await;
        let mut prepare = Box::pin(registry.prepare_delivery_store_on(&store));
        let first = poll_fn(|cx| Poll::Ready(prepare.as_mut().poll(cx))).await;
        assert!(
            first.is_pending(),
            "snapshot done, parked on the instance guard"
        );

        let map_writer = registry.channels.write().await;
        drop(instance_writer);
        let mut outcome = None;
        for _ in 0..8 {
            if let Poll::Ready(done) = poll_fn(|cx| Poll::Ready(prepare.as_mut().poll(cx))).await {
                outcome = Some(done);
                break;
            }
            tokio::task::yield_now().await;
        }
        let outcome =
            outcome.expect("prepare must not park on the map while holding the instance guard");
        assert!(
            outcome.is_err(),
            "map contention is a fail-closed refusal, not a silent success"
        );
        assert!(
            handle.try_write().is_ok(),
            "the instance guard was released by the refusal"
        );
        drop(map_writer);
        registry
            .prepare_delivery_store_on(&store)
            .await
            .expect("uncontended prepare succeeds");
    }

    /// F2 guard: an already-captured ExplicitOnly handle must be refused on
    /// replay without re-classifying the channel id that a newer Automatic
    /// instance now owns.
    #[tokio::test]
    async fn stale_explicit_only_handle_cannot_reclassify_a_newer_automatic_registration() {
        use super::super::delivery_queue::{DeliveryQueueConfig, DeliveryStore, EnqueueOutcome};
        let store =
            Arc::new(DeliveryStore::open_in_memory(DeliveryQueueConfig::default()).unwrap());
        let registry = ChannelRegistry::new().with_delivery_store(store.clone());
        let id = ChannelId::new("flaky");
        registry
            .register(Box::new(FlakyChannel::new(
                0,
                false,
                Arc::new(AtomicU32::new(0)),
            )))
            .await;
        let old = OutboundMessage::text("c", "old-body");
        let EnqueueOutcome::Queued(r) = super::super::delivery_queue::enqueue_validated_legacy(
            &registry,
            &store,
            id.as_str(),
            &old,
            "down",
            0,
        )
        .await
        .unwrap() else {
            panic!("fixture row must queue");
        };
        let explicit_attempts = Arc::new(AtomicU32::new(0));
        let mut explicit = FlakyChannel::new(0, false, explicit_attempts.clone());
        explicit.delivery_policy = OutboundDeliveryPolicy::ExplicitOnly;
        registry.register(Box::new(explicit)).await; // retires r
        let stale = registry.get(&id).await.unwrap();
        let new_attempts = Arc::new(AtomicU32::new(0));
        registry
            .register(Box::new(FlakyChannel::new(0, false, new_attempts.clone())))
            .await;
        assert!(!store.is_explicit_only_channel(id.as_str()).unwrap());
        let fresh = OutboundMessage::text("c", "fresh-body");
        let EnqueueOutcome::Queued(n) =
            store.enqueue_fresh(id.as_str(), &fresh, "down", 0).unwrap()
        else {
            panic!("fresh row must queue under the Automatic instance");
        };

        let attempt = registry
            .send_on(
                &stale,
                &id,
                &old,
                AttemptKind::Replay {
                    store: &store,
                    record_id: r,
                },
            )
            .await;
        assert!(attempt.replay_refused);
        assert_eq!(explicit_attempts.load(Ordering::SeqCst), 0);
        assert!(
            !store.is_explicit_only_channel(id.as_str()).unwrap(),
            "a stale handle has no authority to reclassify the channel id"
        );
        assert_eq!(
            store.len().unwrap(),
            1,
            "the newer registration's message is untouched"
        );
        assert!(store
            .recent_dead_letters(10)
            .unwrap()
            .iter()
            .all(|l| !l.message.text.is_empty() || l.retired_by_policy));

        let later = registry
            .send_attempt(
                &id,
                &fresh,
                AttemptKind::Replay {
                    store: &store,
                    record_id: n,
                },
            )
            .await;
        assert!(
            !later.replay_refused,
            "replay through the current Automatic instance stays allowed"
        );
        assert!(later.result.is_ok());
        assert_eq!(new_attempts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn report_distinguishes_real_admission_from_no_store_and_dead_letter() {
        use super::super::delivery_queue::{DeliveryQueueConfig, DeliveryStore};
        let policy = SendRetryPolicy {
            max_rate_limit_retries: 0,
            ..Default::default()
        };
        let registry = registry_with(
            FlakyChannel::new(10, false, Arc::new(AtomicU32::new(0))),
            policy,
        )
        .await;
        let report = registry
            .send_reported(
                &ChannelId::new("flaky"),
                OutboundMessage::text("c", "hello"),
            )
            .await;
        assert_eq!(report.queue, QueueDisposition::NoStore);
        assert_eq!(report.policy, Some(OutboundDeliveryPolicy::Automatic));
        let store = Arc::new(
            DeliveryStore::open_in_memory(DeliveryQueueConfig {
                max_payload_bytes: 1,
                ..Default::default()
            })
            .unwrap(),
        );
        let registry = registry.with_delivery_store(store.clone());
        let report = registry
            .send_reported(
                &ChannelId::new("flaky"),
                OutboundMessage::text("c", "too large"),
            )
            .await;
        assert_eq!(
            report.queue,
            QueueDisposition::DeadLettered,
            "not Queued when persistence retires over-cap payload"
        );
        assert_eq!(store.len().unwrap(), 0);
        let store =
            Arc::new(DeliveryStore::open_in_memory(DeliveryQueueConfig::default()).unwrap());
        let registry = registry.with_delivery_store(store.clone());
        let report = registry
            .send_reported(
                &ChannelId::new("flaky"),
                OutboundMessage::text("c", "queued"),
            )
            .await;
        assert_eq!(report.queue, QueueDisposition::Queued);
        assert_eq!(store.len().unwrap(), 1);
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
        fail_start: Arc<AtomicBool>,
        fail_stop: Arc<AtomicBool>,
        delivery_policy: OutboundDeliveryPolicy,
    }

    struct PolicyTestChannelFactory {
        policy: OutboundDeliveryPolicy,
    }

    #[async_trait::async_trait]
    impl ChannelFactory for PolicyTestChannelFactory {
        fn channel_type(&self) -> &str {
            "policy-test-webhook"
        }

        async fn create(&self, _config: serde_json::Value) -> ChannelResult<Box<dyn Channel>> {
            Err(ChannelError::ConfigError(
                "test factory requires an instance id".to_string(),
            ))
        }

        async fn create_with_id(
            &self,
            id: &str,
            config: serde_json::Value,
        ) -> ChannelResult<Box<dyn Channel>> {
            let path = config
                .get("path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("/webhook/policy-test");
            Ok(Box::new(
                WebhookyChannel::new(id, path).with_delivery_policy(self.policy),
            ))
        }
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
                fail_start: Arc::new(AtomicBool::new(false)),
                fail_stop: Arc::new(AtomicBool::new(false)),
                delivery_policy: OutboundDeliveryPolicy::Automatic,
            }
        }

        fn with_delivery_policy(mut self, policy: OutboundDeliveryPolicy) -> Self {
            self.delivery_policy = policy;
            self
        }

        fn with_start_failure(self) -> Self {
            self.fail_start.store(true, Ordering::SeqCst);
            self
        }

        fn start_failure_control(&self) -> Arc<AtomicBool> {
            self.fail_start.clone()
        }

        fn stop_failure_control(&self) -> Arc<AtomicBool> {
            self.fail_stop.clone()
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
            if self.fail_start.load(Ordering::SeqCst) {
                return Err(ChannelError::Internal("test start failure".to_string()));
            }
            self.handler = Some(Arc::new(TestHandler {
                path: self.path.clone(),
            }));
            self.state.set_status(ChannelStatus::Connected).await;
            Ok(())
        }
        async fn stop(&mut self) -> ChannelResult<()> {
            if self.fail_stop.load(Ordering::SeqCst) {
                return Err(ChannelError::Internal("test stop failure".to_string()));
            }
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
        fn outbound_delivery_policy(&self) -> OutboundDeliveryPolicy {
            self.delivery_policy
        }
        async fn send(&self, _message: OutboundMessage) -> ChannelResult<SendResult> {
            Ok(SendResult {
                message_id: MessageId::new("ok"),
                timestamp: Utc::now(),
            })
        }
    }

    #[tokio::test]
    async fn capture_is_read_only_and_unrelated_publication_keeps_it_valid() {
        let registry = ChannelRegistry::new();
        let first_id = registry
            .register(Box::new(WebhookyChannel::new(
                "capture-first",
                "/webhook/capture-first",
            )))
            .await;
        let capture = registry.capture_channel(&first_id).await.unwrap();

        assert_eq!(
            registry.get(&first_id).await.unwrap().read().await.status(),
            ChannelStatus::Disconnected,
            "capture must not start the channel"
        );

        let second_id = registry
            .register(Box::new(WebhookyChannel::new(
                "capture-second",
                "/webhook/capture-second",
            )))
            .await;
        let second_capture = registry.capture_channel(&second_id).await.unwrap();

        assert_ne!(capture.source_revision(), second_capture.source_revision());
        assert!(registry.validate_channel_capture(&capture).await.is_some());
    }

    #[tokio::test]
    async fn capture_rejects_replaced_channel_instance() {
        let registry = ChannelRegistry::new();
        let id = registry
            .register(Box::new(WebhookyChannel::new(
                "capture-replaced",
                "/webhook/capture-replaced",
            )))
            .await;
        let capture = registry.capture_channel(&id).await.unwrap();

        registry
            .register(Box::new(WebhookyChannel::new(
                "capture-replaced",
                "/webhook/capture-replaced",
            )))
            .await;

        assert!(registry.validate_channel_capture(&capture).await.is_none());
    }

    #[tokio::test]
    async fn capture_rejects_unregistered_channel() {
        let registry = ChannelRegistry::new();
        let id = registry
            .register(Box::new(WebhookyChannel::new(
                "capture-removed",
                "/webhook/capture-removed",
            )))
            .await;
        let capture = registry.capture_channel(&id).await.unwrap();

        registry.unregister(&id).await;
        assert!(registry.get(&id).await.is_none());
        assert!(registry.validate_channel_capture(&capture).await.is_none());
    }

    fn lifecycle_contract() -> binding::ChannelLifecycleContract {
        binding::ChannelLifecycleContract::new(
            crate::capability::ownership::OwnerRef::Runtime,
            crate::capability::ownership::LifetimeScope::Runtime,
            crate::capability::ownership::VisibilityScope::default(),
            binding::ChannelLifecycleDeclarationV1::schema(),
            crate::capability::descriptor::ProjectionMetadata::default(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn bound_registration_mints_canonical_provider_while_cold() {
        use crate::capability::descriptor::{CapabilityFacet, CapabilityKind};
        use crate::capability::ownership::{OwnershipTree, VisibilityScope};
        use crate::capability::provider::{ProviderState, ProviderTransport};

        let registry = ChannelRegistry::new();
        let ownership = Arc::new(OwnershipTree::new());
        let id = registry
            .register_bound_channel(
                Box::new(WebhookyChannel::new("team/a/b", "/webhook/cold")),
                lifecycle_contract(),
                ownership.clone(),
                binding::ChannelInitialState::Cold,
            )
            .await
            .unwrap();
        let first = registry.capture_bound_channel(&id).await.unwrap();
        let second = registry.capture_bound_channel(&id).await.unwrap();
        assert!(first.provider.same_instance(&second.provider));
        assert_eq!(first.provider.snapshot(), (ProviderState::Cold, 0));
        assert_eq!(first.provider_generation, 0);
        first.provider.with_snapshot(|provider| {
            assert_eq!(
                provider.provider_id().as_str(),
                "aleph/gateway/channels/team/a/b"
            );
            assert_eq!(provider.transport(), ProviderTransport::InProcess);
        });
        assert_eq!(first.descriptor.id.namespace, "aleph/gateway/channels");
        assert_eq!(first.descriptor.id.name, "team/a/b");
        assert_eq!(first.descriptor.revision.0, first.entry_revision);
        assert_eq!(first.descriptor.kind, CapabilityKind::Resource);
        assert!(first
            .descriptor
            .supports(CapabilityFacet::ProviderLifecycle));
        assert!(!first.descriptor.supports(CapabilityFacet::Callable));
        assert!(first.descriptor.services.is_empty());
        assert_eq!(first.descriptor.metadata.surface, None);
        assert!(Arc::ptr_eq(
            &first.entry,
            &registry.channels.read().await[&id]
        ));
        assert!(Arc::ptr_eq(&first.channel, &first.entry.channel));
        let ChannelRegistration::Bound(bound) = &first.entry.registration else {
            panic!("not Bound")
        };
        assert!(Arc::ptr_eq(&bound.ownership, &ownership));
        assert_eq!(
            first.descriptor_fingerprint,
            binding::canonical_descriptor_fingerprint(&first.descriptor)
        );
        assert_eq!(
            registry.desired_state.read().await[&id],
            DesiredChannelState::Stopped
        );
        assert!(registry.start_all().await.is_empty());
        let (snapshot, revision) = registry
            .snapshot_bound_channels(&VisibilityScope::default())
            .await;
        assert_eq!(snapshot, vec![first.descriptor.clone()]);
        assert_eq!(revision, first.source_revision);
        assert!(registry
            .snapshot_bound_channels(&VisibilityScope {
                principal: Some("unknown".into()),
                ..Default::default()
            })
            .await
            .0
            .is_empty());
    }

    #[tokio::test]
    async fn legacy_capture_never_synthesizes_provider() {
        let registry = ChannelRegistry::new();
        let direct = registry
            .register(Box::new(WebhookyChannel::new("legacy", "/webhook/legacy")))
            .await;
        registry
            .register_factory(Arc::new(PolicyTestChannelFactory {
                policy: OutboundDeliveryPolicy::Automatic,
            }))
            .await;
        let created = registry
            .create_channel(ChannelConfig {
                id: "factory-legacy".into(),
                channel_type: "policy-test-webhook".into(),
                enabled: false,
                config: serde_json::json!({}),
            })
            .await
            .unwrap();
        for id in [direct, created] {
            let capture = registry.capture_channel(&id).await.unwrap();
            for _ in 0..3 {
                assert!(registry.validate_channel_capture(&capture).await.is_some());
                assert!(matches!(
                    registry.capture_bound_channel(&id).await,
                    Err(binding::ChannelBindingError::UnboundLegacy)
                ));
            }
        }
        assert!(registry
            .snapshot_bound_channels(&Default::default())
            .await
            .0
            .is_empty());
        assert_eq!(registry.channels.read().await.len(), 2);
    }

    #[tokio::test]
    async fn bound_capture_preserves_full_id_and_source_identity() {
        use crate::capability::ownership::OwnershipTree;
        let first = ChannelRegistry::new();
        let second = ChannelRegistry::new();
        for registry in [&first, &second] {
            registry
                .register_bound_channel(
                    Box::new(WebhookyChannel::new("team/a/b", "/webhook/id")),
                    lifecycle_contract(),
                    Arc::new(OwnershipTree::new()),
                    binding::ChannelInitialState::Cold,
                )
                .await
                .unwrap();
        }
        let a = first
            .capture_bound_channel(&ChannelId::new("team/a/b"))
            .await
            .unwrap();
        let b = second
            .capture_bound_channel(&ChannelId::new("team/a/b"))
            .await
            .unwrap();
        assert_eq!(a.descriptor.id.name, "team/a/b");
        assert_eq!(a.source_revision, b.source_revision);
        assert!(!Arc::ptr_eq(&a.source, &b.source));
        assert!(!Arc::ptr_eq(&a.entry, &b.entry));
        assert!(!a.provider.same_instance(&b.provider));
        let legacy_id = first
            .register(Box::new(WebhookyChannel::new(
                "legacy-source",
                "/webhook/source",
            )))
            .await;
        second
            .register(Box::new(WebhookyChannel::new(
                "legacy-source",
                "/webhook/source",
            )))
            .await;
        assert!(second
            .validate_channel_capture(&first.capture_channel(&legacy_id).await.unwrap())
            .await
            .is_none());
    }

    #[tokio::test]
    async fn bound_cold_compatibility_paths_and_collisions_fail_closed() {
        use crate::capability::ownership::OwnershipTree;
        use crate::capability::provider::ProviderState;
        for (initial, state) in [
            (
                binding::ChannelInitialState::Discovered,
                ProviderState::Discovered,
            ),
            (binding::ChannelInitialState::Cold, ProviderState::Cold),
            (
                binding::ChannelInitialState::Disabled,
                ProviderState::Disabled,
            ),
        ] {
            let registry = ChannelRegistry::new();
            let id = registry
                .register_bound_channel(
                    Box::new(WebhookyChannel::new("bound", "/webhook/bound")),
                    lifecycle_contract(),
                    Arc::new(OwnershipTree::new()),
                    initial,
                )
                .await
                .unwrap();
            let before = registry.capture_bound_channel(&id).await.unwrap();
            assert_eq!(before.provider.snapshot(), (state, 0));
            assert!(registry.get(&id).await.is_none());
            assert!(registry.capture_channel(&id).await.is_none());
            assert!(registry.get_capabilities(&id).await.is_some());
            assert_eq!(registry.list().await.len(), 1);
            assert_eq!(registry.list_by_type("test-webhook").await.len(), 1);
            assert_eq!(registry.status_summary().await.total, 1);
            assert_eq!(registry.health_summary().await.total, 1);
            assert_eq!(
                registry.health_states().await[0].desired,
                DesiredChannelState::Stopped
            );
            // Lifecycle compatibility is covered by the source-routing test;
            // this cold-read contract deliberately performs no start/stop.
            assert!(matches!(
                registry.send(&id, OutboundMessage::text("c", "no")).await,
                Err(ChannelError::UnsupportedFeature(_))
            ));
            assert!(registry.unregister(&id).await.is_none());
            registry
                .register(Box::new(WebhookyChannel::new(
                    "bound",
                    "/webhook/replacement",
                )))
                .await;
            assert!(matches!(
                registry
                    .register_bound_channel(
                        Box::new(WebhookyChannel::new("bound", "/webhook/replacement")),
                        lifecycle_contract(),
                        Arc::new(OwnershipTree::new()),
                        initial,
                    )
                    .await,
                Err(binding::ChannelBindingError::Busy)
            ));
            let after = registry.capture_bound_channel(&id).await.unwrap();
            assert!(Arc::ptr_eq(&before.entry, &after.entry));
            assert_eq!(before.source_revision, after.source_revision);
            assert_eq!(after.provider.snapshot(), (state, 0));
            assert_eq!(registry.webhook_mounts().mounted_count().await, 0);
        }
    }

    #[tokio::test]
    async fn bound_publication_exhaustion_and_legacy_collision_preserve_source() {
        use crate::capability::ownership::OwnershipTree;
        let registry = ChannelRegistry::new();
        let id = registry
            .register(Box::new(WebhookyChannel::new("legacy", "/webhook/legacy")))
            .await;
        let capture = registry.capture_channel(&id).await.unwrap();
        assert!(matches!(
            registry
                .register_bound_channel(
                    Box::new(WebhookyChannel::new("legacy", "/webhook/legacy")),
                    lifecycle_contract(),
                    Arc::new(OwnershipTree::new()),
                    binding::ChannelInitialState::Cold,
                )
                .await,
            Err(binding::ChannelBindingError::Busy)
        ));
        assert!(registry.validate_channel_capture(&capture).await.is_some());
        registry.source_revision.store(u64::MAX, Ordering::Relaxed);
        assert!(matches!(
            registry
                .register_bound_channel(
                    Box::new(WebhookyChannel::new("exhausted", "/webhook/exhausted")),
                    lifecycle_contract(),
                    Arc::new(OwnershipTree::new()),
                    binding::ChannelInitialState::Cold,
                )
                .await,
            Err(binding::ChannelBindingError::PublicationExhausted)
        ));
        assert_eq!(registry.channels.read().await.len(), 1);
        assert_eq!(registry.source_revision.load(Ordering::Relaxed), u64::MAX);
    }

    #[tokio::test]
    async fn replacement_stops_any_running_channel_before_publication() {
        let registry = ChannelRegistry::new();
        let incumbent = WebhookyChannel::new("wh", "/webhook/wh");
        let incumbent_state = incumbent.state().status_handle();
        let id = registry.register(Box::new(incumbent)).await;
        registry.start_channel(&id).await.unwrap();

        registry
            .register(Box::new(WebhookyChannel::new("wh", "/webhook/wh")))
            .await;

        assert_eq!(
            *incumbent_state.read().await,
            ChannelStatus::Disconnected,
            "replacement must stop the incumbent regardless of channel type"
        );
        assert_eq!(registry.webhook_mounts().mounted_count().await, 0);
        assert_eq!(
            registry.get(&id).await.unwrap().read().await.status(),
            ChannelStatus::Disconnected
        );
    }

    #[tokio::test]
    async fn replacement_stop_failure_retains_the_incumbent() {
        let registry = ChannelRegistry::new();
        let incumbent = WebhookyChannel::new("stop-fails", "/webhook/stop-fails");
        let stop_failure = incumbent.stop_failure_control();
        let id = registry.register(Box::new(incumbent)).await;
        registry.start_channel(&id).await.unwrap();
        stop_failure.store(true, Ordering::SeqCst);
        let incumbent_handle = registry.get(&id).await.unwrap();

        registry
            .register(Box::new(WebhookyChannel::new(
                "stop-fails",
                "/webhook/stop-fails",
            )))
            .await;

        assert!(
            Arc::ptr_eq(&incumbent_handle, &registry.get(&id).await.unwrap()),
            "a failed teardown must retain the incumbent registration"
        );
        assert_eq!(
            registry.get(&id).await.unwrap().read().await.status(),
            ChannelStatus::Connected,
            "a failed teardown must leave the incumbent usable"
        );
    }

    #[tokio::test]
    async fn unregister_stop_failure_retains_the_incumbent() {
        let registry = ChannelRegistry::new();
        let incumbent = WebhookyChannel::new("remove-fails", "/webhook/remove-fails");
        let stop_failure = incumbent.stop_failure_control();
        let id = registry.register(Box::new(incumbent)).await;
        registry.start_channel(&id).await.unwrap();
        stop_failure.store(true, Ordering::SeqCst);
        let incumbent_handle = registry.get(&id).await.unwrap();

        assert!(registry.unregister(&id).await.is_none());
        assert!(
            Arc::ptr_eq(&incumbent_handle, &registry.get(&id).await.unwrap()),
            "a failed teardown must retain the incumbent registration"
        );
        assert_eq!(
            registry.webhook_mounts().mounted_count().await,
            1,
            "a failed teardown must retain the incumbent HTTP surface"
        );
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
    async fn restart_updates_health_state_to_running_after_success() {
        let registry = ChannelRegistry::new();
        let id = registry
            .register(Box::new(WebhookyChannel::new(
                "restart-health",
                "/webhook/restart-health",
            )))
            .await;
        registry.start_channel(&id).await.unwrap();

        registry.restart_channel(&id).await.unwrap();
        let snapshot = registry
            .health_states()
            .await
            .into_iter()
            .find(|snapshot| snapshot.id == id)
            .expect("restarted channel remains observable");
        assert_eq!(snapshot.desired, DesiredChannelState::Running);
        assert_eq!(snapshot.status, ChannelStatus::Connected);
    }

    #[tokio::test]
    async fn failed_restart_preserves_prior_desired_state() {
        let registry = ChannelRegistry::new();
        let id = registry
            .register(Box::new(
                WebhookyChannel::new("restart-fails", "/webhook/restart-fails")
                    .with_start_failure(),
            ))
            .await;

        let result = registry.restart_channel(&id).await;
        assert!(result.is_err());
        let snapshot = registry
            .health_states()
            .await
            .into_iter()
            .find(|snapshot| snapshot.id == id)
            .expect("failed restart leaves channel observable");
        assert_eq!(snapshot.desired, DesiredChannelState::Stopped);
        assert_eq!(snapshot.status, ChannelStatus::Disconnected);
    }

    #[tokio::test]
    async fn failed_restart_after_running_clears_running_intent() {
        let registry = ChannelRegistry::new();
        let channel = WebhookyChannel::new(
            "restart-fails-after-running",
            "/webhook/restart-fails-after-running",
        );
        let fail_start = channel.start_failure_control();
        let id = registry.register(Box::new(channel)).await;

        registry.start_channel(&id).await.unwrap();
        fail_start.store(true, Ordering::SeqCst);

        let result = registry.restart_channel(&id).await;
        assert!(result.is_err());
        let snapshot = registry
            .health_states()
            .await
            .into_iter()
            .find(|snapshot| snapshot.id == id)
            .expect("failed restart leaves channel observable");
        assert_eq!(snapshot.desired, DesiredChannelState::Stopped);
        assert_eq!(snapshot.status, ChannelStatus::Disconnected);
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

    // --- Stage C: Legacy compatibility surfaces around a Bound channel ---

    /// Register a Bound channel on an existing registry/ownership tree and
    /// mirror the claim the source-owned lifecycle needs. Test setup only.
    async fn register_bound_on(
        registry: &Arc<ChannelRegistry>,
        tree: &Arc<OwnershipTree>,
        channel: Box<dyn Channel>,
        initial: ChannelInitialState,
    ) -> ChannelId {
        let id = registry
            .register_bound_channel(channel, lifecycle_contract(), tree.clone(), initial)
            .await
            .unwrap();
        let capture = registry.capture_bound_channel(&id).await.unwrap();
        tree.register(
            capture.descriptor.id.clone(),
            capture.descriptor.owner.clone(),
            capture.descriptor.lifetime,
            capture.descriptor.visibility.clone(),
        )
        .unwrap();
        id
    }

    fn observed_webhook(
        id: &str,
        callback_url: String,
        path: &str,
        secret: &str,
        probe: &super::test_support::ResourceProbe,
    ) -> crate::gateway::interfaces::webhook::WebhookChannel {
        use crate::gateway::interfaces::webhook::{WebhookChannel, WebhookChannelConfig};
        WebhookChannel::observed(
            id,
            WebhookChannelConfig {
                secret: secret.into(),
                callback_url,
                path: path.into(),
                ..Default::default()
            },
            probe.clone(),
        )
    }

    fn desired_of(snapshots: &[ChannelLifecycleSnapshot], id: &ChannelId) -> DesiredChannelState {
        snapshots
            .iter()
            .find(|snapshot| &snapshot.id == id)
            .unwrap_or_else(|| panic!("no lifecycle snapshot for {id}"))
            .desired
    }

    /// Every Legacy compatibility surface must refuse a Bound registration
    /// *explicitly* and produce no side effect.
    ///
    /// A denial may not degrade into `Ok(None)`, `false`, an empty result, or a
    /// fallback permission, and it may not reach the transport, the durable
    /// queue, the inbound stream, or provider activation. The queue half is
    /// asserted on persisted rows, not on a returned receipt: a `ChannelId`
    /// alone must never authorize a dynamic send admission.
    #[tokio::test]
    async fn bound_compatibility_effect_paths_are_denied() {
        use super::test_support::{LoopbackPeer, ResourceProbe};
        use crate::gateway::delivery_queue::{DeliveryQueueConfig, DeliveryStore};

        let peer = LoopbackPeer::new(200).await;
        let mut arrival = peer.arrived;
        let probe = ResourceProbe::default();
        let store = Arc::new(
            DeliveryStore::open_in_memory(DeliveryQueueConfig::default())
                .expect("in-memory delivery store"),
        );
        let tree = Arc::new(OwnershipTree::new());
        let registry = Arc::new(ChannelRegistry::new().with_delivery_store(Arc::clone(&store)));
        let mut inbound = registry
            .take_inbound_receiver()
            .expect("the registry owns the only inbound receiver");
        let id = register_bound_on(
            &registry,
            &tree,
            Box::new(observed_webhook(
                "effect-bound",
                peer.url.clone(),
                "/webhook/effect-bound",
                "effect-secret",
                &probe,
            )),
            ChannelInitialState::Cold,
        )
        .await;
        let conversation = ConversationId::new("conv-1");
        let message_id = super::super::channel::MessageId::new("msg-1");
        let options = ["a".to_string(), "b".to_string()];

        assert!(
            registry.get(&id).await.is_none(),
            "a Bound id must never yield a Legacy channel handle"
        );

        let refusals: [(&str, ChannelResult<()>); 5] = [
            (
                "edit",
                registry
                    .edit(&id, &conversation, &message_id, "denied")
                    .await,
            ),
            (
                "react",
                registry
                    .react(&id, &conversation, &message_id, "denied")
                    .await,
            ),
            (
                "send_typing",
                registry.send_typing(&id, &conversation).await,
            ),
            (
                "set_group_icon",
                registry
                    .set_group_icon(&id, &conversation, "data:image/png;base64,AA==")
                    .await,
            ),
            (
                "list_conversations",
                registry.list_conversations(&id, "", 10).await.map(|_| ()),
            ),
        ];
        for (surface, result) in refusals {
            assert!(
                matches!(result, Err(ChannelError::UnsupportedFeature(_))),
                "{surface} must refuse a Bound registration explicitly, got {result:?}"
            );
        }
        assert!(
            matches!(
                registry
                    .create_poll(&id, &conversation, "q", &options, false)
                    .await,
                Err(ChannelError::UnsupportedFeature(_))
            ),
            "create_poll must refuse a Bound registration explicitly"
        );

        let report = registry
            .send_reported(&id, OutboundMessage::text("conv-1", "denied send"))
            .await;
        assert!(
            matches!(report.result, Err(ChannelError::UnsupportedFeature(_))),
            "delivery admission must refuse a Bound registration, got {:?}",
            report.result
        );
        assert_eq!(report.queue, QueueDisposition::NotQueueable);

        // HANDOFF (Task 4, not this task): `DeliveryStore::enqueue` is the raw
        // `pub` legacy/import seam today, and this call site exists only to pin
        // its RED. Task 4 makes the store write private and publishes
        // `pub(crate) async fn enqueue_validated_legacy(registry, store, ...)`,
        // which validates the current Legacy registration before writing
        // (design §6 "Effects and queue"; plan Task 4 Steps 1-2, Step 2 already
        // names Registry/queue tests as a call site to migrate). Task 4 must
        // move this call onto that helper and delete the raw call: final API
        // must not expose an unvalidated `ChannelId`-only queue write.
        // When migrating, preserve these behavioral assertions verbatim rather
        // than turning them into "the helper was called":
        //   (a) the outcome is not `EnqueueOutcome::Queued(_)`,
        //   (b) `store.len()` is unchanged,
        //   (c) `delivery_queue_stats().pending == 0`.
        // Rows before/after are the assertion; the returned outcome is only
        // corroboration.
        let before = store.len().expect("queue length");
        let outcome = super::super::delivery_queue::enqueue_validated_legacy(
            &registry,
            &store,
            id.as_str(),
            &OutboundMessage::text("conv-1", "raw admission"),
            "",
            0,
        )
        .await;
        assert!(
            !matches!(
                outcome,
                Ok(crate::gateway::delivery_queue::EnqueueOutcome::Queued(_))
            ),
            "a channel id alone must not authorize queue admission, got {outcome:?}"
        );
        assert_eq!(
            store.len().expect("queue length"),
            before,
            "a refused admission must persist no row"
        );
        assert_eq!(
            registry
                .delivery_queue_stats()
                .expect("attached store")
                .pending,
            0
        );

        assert_eq!(probe.count("client_build_attempt"), 0);
        assert_eq!(probe.count("client_built"), 0);
        assert_eq!(probe.count("request_attempt"), 0);
        assert_eq!(probe.count("request_ended"), 0);
        assert_eq!(probe.count("client_drop"), 0);
        assert!(
            arrival.try_recv().is_err(),
            "no effect helper may reach the loopback transport"
        );
        assert!(
            inbound.try_recv().is_err(),
            "no refused effect may emit an inbound message"
        );
        let capture = registry.capture_bound_channel(&id).await.unwrap();
        assert_eq!(
            capture.provider.state(),
            ProviderState::Cold,
            "a refused effect must not activate the provider"
        );
    }

    /// Metadata reads may see a Bound row; none of them may cost a resource.
    ///
    /// Cold, Discovered, and Disabled declarations are enumerated because the
    /// read paths must not care which one it is: none of them may build a
    /// client, start the channel, create a forwarder, admit a queue row,
    /// activate the provider, or speak to the directory/transport.
    #[tokio::test]
    async fn bound_cold_lists_never_call_transport() {
        use super::test_support::{LoopbackPeer, ResourceProbe};
        use crate::gateway::delivery_queue::{DeliveryQueueConfig, DeliveryStore};

        for initial in [
            ChannelInitialState::Discovered,
            ChannelInitialState::Cold,
            ChannelInitialState::Disabled,
        ] {
            let label = match initial {
                ChannelInitialState::Discovered => "discovered",
                ChannelInitialState::Cold => "cold",
                ChannelInitialState::Disabled => "disabled",
            };
            let expected = match initial {
                ChannelInitialState::Discovered => ProviderState::Discovered,
                ChannelInitialState::Cold => ProviderState::Cold,
                ChannelInitialState::Disabled => ProviderState::Disabled,
            };
            let peer = LoopbackPeer::new(200).await;
            let mut arrival = peer.arrived;
            let probe = ResourceProbe::default();
            let store = Arc::new(
                DeliveryStore::open_in_memory(DeliveryQueueConfig::default())
                    .expect("in-memory delivery store"),
            );
            let tree = Arc::new(OwnershipTree::new());
            let registry = Arc::new(ChannelRegistry::new().with_delivery_store(Arc::clone(&store)));
            let mut inbound = registry
                .take_inbound_receiver()
                .expect("the registry owns the only inbound receiver");
            let id = register_bound_on(
                &registry,
                &tree,
                Box::new(observed_webhook(
                    "cold-list-bound",
                    peer.url.clone(),
                    "/webhook/cold-list",
                    "cold-secret",
                    &probe,
                )),
                initial,
            )
            .await;
            let channel_type = registry.list().await[0].channel_type.clone();

            for _ in 0..3 {
                let list = registry.list().await;
                assert_eq!(list.len(), 1, "{label}: metadata rows stay visible");
                assert_eq!(list[0].id, id);
                assert_eq!(registry.list_by_type(&channel_type).await.len(), 1);
                assert_eq!(registry.status_summary().await.total, 1);
                assert_eq!(registry.health_summary().await.total, 1);
                let snapshots = registry.health_states().await;
                assert_eq!(snapshots.len(), 1, "{label}: health snapshot row");
                assert_eq!(
                    desired_of(&snapshots, &id),
                    DesiredChannelState::Stopped,
                    "{label}: a read never writes desire"
                );
                assert!(
                    registry.get_capabilities(&id).await.is_some(),
                    "{label}: capability metadata is observable"
                );
                assert!(
                    registry.get(&id).await.is_none(),
                    "{label}: a handle-returning read stays Legacy-only"
                );
                assert!(matches!(
                    registry.list_conversations(&id, "", 10).await,
                    Err(ChannelError::UnsupportedFeature(_))
                ));
                assert!(registry.capture_bound_channel(&id).await.is_ok());
                let (descriptors, _) = registry
                    .snapshot_bound_channels(&VisibilityScope::default())
                    .await;
                assert_eq!(descriptors.len(), 1);
            }

            assert_eq!(probe.count("client_build_attempt"), 0, "{label}");
            assert_eq!(probe.count("client_built"), 0, "{label}");
            assert_eq!(probe.count("request_attempt"), 0, "{label}");
            assert_eq!(probe.count("request_ended"), 0, "{label}");
            assert_eq!(probe.count("client_drop"), 0, "{label}");
            assert_eq!(
                registry.webhook_mounts().mounted_count().await,
                0,
                "{label}: a read must not mount ingress"
            );
            assert_eq!(store.len().expect("queue length"), 0, "{label}");
            assert!(inbound.try_recv().is_err(), "{label}");
            assert!(arrival.try_recv().is_err(), "{label}");
            let capture = registry.capture_bound_channel(&id).await.unwrap();
            assert_eq!(capture.provider.state(), expected, "{label}");
            let entry = registry.channels.read().await.get(&id).unwrap().clone();
            let runtime = entry.runtime.lock().unwrap();
            assert!(runtime.driver.is_none(), "{label}");
            assert!(runtime.forwarder.is_none(), "{label}");
            assert_eq!(runtime.active_work, 0, "{label}");
        }
    }

    /// The compatibility lifecycle entry points must dispatch by registration
    /// variant: Bound work belongs to the source-owned driver, never to the
    /// Legacy adapter path.
    #[tokio::test]
    async fn bound_compat_lifecycle_uses_source() {
        use super::test_support::{
            FailingStopWebhook, HeldWebhook, LifecycleBlock, LoopbackPeer, ResourceProbe,
        };
        use crate::gateway::interfaces::webhook::{WebhookChannel, WebhookChannelConfig};
        use std::time::Duration;

        let peer = LoopbackPeer::new(200).await;
        let (registry, id, probe, tree) = real_bound(peer.url.clone()).await;

        // start: the driver, the provider, and the entry runtime all move
        // through the existing binding path.
        registry
            .start_channel(&id)
            .await
            .expect("compat start must route to the source-owned lifecycle");
        let capture = registry.capture_bound_channel(&id).await.unwrap();
        assert_eq!(capture.provider.state(), ProviderState::Ready);
        assert_eq!(
            capture.provider.snapshot().1,
            1,
            "the provider generation advances through the binding path"
        );
        assert_eq!(probe.count("client_built"), 1);
        assert_eq!(probe.count("request_attempt"), 0);
        assert_eq!(
            desired_of(&registry.health_states().await, &id),
            DesiredChannelState::Running
        );
        assert_eq!(
            registry.webhook_mounts().mounted_count().await,
            0,
            "no Bound ingress is mounted"
        );
        let entry = registry.channels.read().await.get(&id).unwrap().clone();
        assert!(entry.runtime.lock().unwrap().forwarder.is_some());

        // stop: same source-owned drain, ending Stopped.
        registry
            .stop_channel(&id)
            .await
            .expect("compat stop must route to the source-owned lifecycle");
        assert_eq!(capture_state(&registry, &id).await, ProviderState::Stopped);
        assert_eq!(probe.count("client_drop"), 1);
        assert!(entry.runtime.lock().unwrap().forwarder.is_none());
        assert_eq!(
            desired_of(&registry.health_states().await, &id),
            DesiredChannelState::Stopped
        );

        // restart: a source-owned stop followed by a source-owned start.
        registry
            .restart_channel(&id)
            .await
            .expect("compat restart must be a source-owned stop then start");
        assert_eq!(capture_state(&registry, &id).await, ProviderState::Ready);
        assert_eq!(probe.count("client_built"), 2);
        // Restart begins from Stopped: the source drain is intentionally a
        // no-op for an already drained transport, so no second drop occurs.
        assert_eq!(probe.count("client_drop"), 1);
        assert_eq!(
            desired_of(&registry.health_states().await, &id),
            DesiredChannelState::Running
        );

        // The shared source gate must not be held while channel I/O blocks.
        let (entered, waiting) = tokio::sync::oneshot::channel();
        let (release, blocked) = tokio::sync::oneshot::channel();
        let held_probe = ResourceProbe::default();
        let held_id = register_bound_on(
            &registry,
            &tree,
            Box::new(HeldWebhook {
                channel: WebhookChannel::observed(
                    "held-bound",
                    WebhookChannelConfig {
                        secret: "held-secret".into(),
                        callback_url: "http://127.0.0.1:1/callback".into(),
                        path: "/webhook/held-bound".into(),
                        ..Default::default()
                    },
                    held_probe.clone(),
                ),
                start: Some(LifecycleBlock {
                    entered,
                    release: blocked,
                }),
                stop: None,
            }),
            ChannelInitialState::Cold,
        )
        .await;
        let driver = {
            let registry = Arc::clone(&registry);
            let held_id = held_id.clone();
            tokio::spawn(async move { registry.start_channel(&held_id).await })
        };
        waiting.await.expect("the driver must reach channel.start");
        assert_eq!(
            capture_state(&registry, &held_id).await,
            ProviderState::Starting,
            "the source-owned driver is active while the adapter blocks"
        );
        let progressed = tokio::time::timeout(Duration::from_secs(2), async {
            registry.capture_bound_channel(&held_id).await.unwrap();
            registry
                .register(Box::new(observed_webhook(
                    "gate-probe",
                    "http://127.0.0.1:1/callback".into(),
                    "/webhook/gate-probe",
                    "gate-secret",
                    &ResourceProbe::default(),
                )))
                .await;
        })
        .await;
        assert!(
            progressed.is_ok(),
            "the source gate must stay free while channel I/O blocks"
        );
        release.send(()).expect("release the held start");
        driver
            .await
            .expect("join the compat start")
            .expect("compat start must succeed through the source");
        assert_eq!(
            capture_state(&registry, &held_id).await,
            ProviderState::Ready
        );
        assert_eq!(held_probe.count("client_built"), 1);

        // A failed cleanup must not fabricate Ready or revive the entry.
        let failing_probe = ResourceProbe::default();
        let failing_id = register_bound_on(
            &registry,
            &tree,
            Box::new(FailingStopWebhook::new(
                "failing-bound",
                "http://127.0.0.1:1/callback".into(),
                failing_probe.clone(),
            )),
            ChannelInitialState::Cold,
        )
        .await;
        registry
            .start_channel(&failing_id)
            .await
            .expect("start must succeed before the injected stop failure");
        assert_eq!(
            capture_state(&registry, &failing_id).await,
            ProviderState::Ready
        );
        assert!(
            registry.stop_channel(&failing_id).await.is_err(),
            "the injected stop failure must be reported, not swallowed"
        );
        assert_ne!(
            capture_state(&registry, &failing_id).await,
            ProviderState::Ready,
            "a failed stop must never fabricate Ready"
        );
        assert!(
            registry.restart_channel(&failing_id).await.is_err(),
            "restart must not revive an entry whose cleanup failed"
        );
        assert_ne!(
            capture_state(&registry, &failing_id).await,
            ProviderState::Ready
        );
        assert_eq!(failing_probe.count("client_drop"), 0);
    }

    /// Legacy traffic must behave exactly as before: same admission, same
    /// transport, same mount, same cleanup.
    #[tokio::test]
    async fn legacy_positive_controls_unchanged() {
        use super::test_support::{LoopbackPeer, ResourceProbe};
        use std::time::Duration;

        let peer = LoopbackPeer::new(200).await;
        let probe = ResourceProbe::default();
        let registry = Arc::new(ChannelRegistry::new());
        let mut inbound = registry
            .take_inbound_receiver()
            .expect("the registry owns the only inbound receiver");
        let id = registry
            .register(Box::new(observed_webhook(
                "legacy-control",
                peer.url.clone(),
                "/webhook/legacy-control",
                "legacy-secret",
                &probe,
            )))
            .await;

        registry
            .start_channel(&id)
            .await
            .expect("Legacy start stays available");
        assert_eq!(probe.count("client_built"), 1);
        assert!(registry.get(&id).await.is_some());
        assert_eq!(registry.webhook_mounts().mounted_count().await, 1);

        let send = {
            let registry = Arc::clone(&registry);
            let id = id.clone();
            tokio::spawn(async move {
                registry
                    .send_reported(&id, OutboundMessage::text("conv-1", "legacy control"))
                    .await
            })
        };
        let bytes = peer.arrived.await.expect("the loopback peer is reached");
        assert!(String::from_utf8_lossy(&bytes).contains("legacy control"));
        peer.release.send(()).expect("release the held response");
        let report = send.await.expect("join the send");
        assert!(
            report.result.is_ok(),
            "Legacy send must still deliver: {:?}",
            report.result.err()
        );
        assert_eq!(report.queue, QueueDisposition::Delivered);
        assert_eq!(probe.count("request_attempt"), 1);

        let body = br#"{"message":"inbound control"}"#;
        let signature = crate::gateway::webhook_receiver::WebhookReceiver::compute_signature(
            "legacy-secret",
            body,
        );
        let router =
            crate::gateway::webhook_receiver::WebhookReceiver::router(registry.webhook_mounts());
        let response = {
            use tower::ServiceExt;
            router
                .oneshot(
                    axum::http::Request::builder()
                        .method("POST")
                        .uri("/webhook/legacy-control")
                        .header("x-webhook-signature", signature)
                        .body(axum::body::Body::from(body.to_vec()))
                        .expect("build the signed request"),
                )
                .await
                .expect("dispatch the inbound webhook")
        };
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let forwarded = tokio::time::timeout(Duration::from_secs(2), inbound.recv())
            .await
            .expect("the forwarder must publish the inbound message")
            .expect("inbound broadcast is open");
        assert_eq!(forwarded.text, "inbound control");
        assert_eq!(forwarded.channel_id, id);

        registry
            .stop_channel(&id)
            .await
            .expect("Legacy stop stays available");
        assert_eq!(probe.count("client_drop"), 1);
        assert_eq!(
            registry.webhook_mounts().mounted_count().await,
            0,
            "stop must unmount"
        );
        peer.task.await.expect("the loopback peer task ends");
    }
}
