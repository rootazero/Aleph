//! SMS channel (thin telecom Relay).
//!
//! Core side of the PSTN/SMS design
//! (`docs/superpowers/specs/2026-10-09-pstn-sms-thin-relay-design.md`). The
//! Relay is a hosted, content-free transport; this channel is the Core
//! adapter: it normalises inbound SMS into [`InboundMessage`]s and turns an
//! outbound [`OutboundMessage`] into a *pending operation* for the Relay.
//!
//! # What this module does not do
//!
//! There is **no network code here**. The Relay connection (authenticated
//! lease, generation fencing, TLS) is Task 3 and plugs in through
//! [`SmsTransport`]. Until a transport is attached:
//!
//! - `start()` fails with `NotConnected` and the channel sits in
//!   [`ChannelStatus::Error`] — configuration alone never makes SMS "up";
//! - `send()` fails with `NotConnected` — it never returns a [`SendResult`]
//!   for a message nothing carried.
//!
//! # Truthfulness of `send`
//!
//! `Ok(SendResult)` means exactly "the Relay took the request"
//! (`accepted`/`submitted`, or a later stage). `SendResult::message_id` is the
//! `operation_id`, which later [`OutcomeReport`]s correlate to; it is **not**
//! a delivery claim. `unknown` is reported as an error that says so and must
//! not be retried implicitly (spec §6).
//!
//! # Authorization
//!
//! This module does not authenticate sends. `approval_reference` in
//! `OutboundMessage::metadata` is an opaque string forwarded to the Relay, not
//! proof of approval. Scoped reply/proactive approval is enforced upstream of
//! the channel (Task 4); until it is wired, outbound SMS is not production
//! ready.
//!
//! # Usage
//!
//! ```toml
//! [channels.sms]
//! type = "sms"
//! region = "CN"                 # explicit; never inferred from a number prefix
//! service_address = "+8613800000000"
//! binding_ref = "core-binding-1"
//! ```
//!
//! Provider credentials are intentionally not part of this config.

pub mod config;
pub mod message_ops;
pub mod transport;

pub use config::{SmsConfig, MAX_COMMAND_TTL_SECS, MAX_SEGMENTS_LIMIT};
pub use message_ops::{
    build_pending_operation, convert_inbound, estimate_segments, max_chars_for_segments,
    normalize_e164, scoped_message_id, PendingSmsOperation, SmsError, SmsInboundMeta,
    SmsOutcomeMeta, METADATA_APPROVAL_REFERENCE,
};
pub use transport::*;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};

use aleph_protocol::telecom_relay::{TelecomEnvelope, TelecomKind, TelecomOutcome};

use crate::gateway::channel::{
    Channel, ChannelCapabilities, ChannelError, ChannelFactory, ChannelId, ChannelInfo,
    ChannelResult, ChannelState, ChannelStatus, MessageId, OutboundDeliveryPolicy, OutboundMessage,
    SendResult,
};
use crate::gateway::telecom::{accept_event, AcceptContext, Acceptance, DedupLedger};
use crate::sync_primitives::{Arc, Mutex, PoisonError};

/// Dedup horizon (spec §9: Relay keeps metadata 7 days; Core mirrors it).
const DEDUP_RETENTION_DAYS: i64 = 7;

/// Result of feeding one Relay envelope to [`SmsChannel::ingest_envelope`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SmsIngest {
    /// Fresh inbound SMS, published to subscribers. Safe to ACK.
    Committed(MessageId),
    /// Already seen; nothing published. Safe to ACK.
    Duplicate,
    /// Older than the dedup horizon; not published, not recorded.
    Stale,
    /// Past its deadline; recorded (redelivery is a duplicate), not published.
    Expired,
    /// A valid frame that carries no inbound text (receipt, command, call
    /// frame). Not an agent turn.
    NotInbound,
}

/// SMS channel.
pub struct SmsChannel {
    info: ChannelInfo,
    config: SmsConfig,
    channel_state: ChannelState,
    transport: Option<Arc<dyn SmsTransport>>,
    ledger: Mutex<DedupLedger>,
}

impl SmsChannel {
    /// Channel with no Relay transport: `start`/`send` report unavailable.
    pub fn new(id: impl Into<String>, config: SmsConfig) -> Self {
        let info = ChannelInfo {
            id: ChannelId::new(id),
            name: "SMS".to_string(),
            channel_type: "sms".to_string(),
            status: ChannelStatus::Disconnected,
            capabilities: Self::channel_capabilities(&config),
        };
        Self {
            info,
            config,
            channel_state: ChannelState::new(100),
            transport: None,
            ledger: Mutex::new(DedupLedger::new(Duration::days(DEDUP_RETENTION_DAYS))),
        }
    }

    /// Attach the Relay transport (Task 3 wiring; tests use fakes).
    #[must_use]
    pub fn with_transport(mut self, transport: Arc<dyn SmsTransport>) -> Self {
        self.transport = Some(transport);
        self
    }

    pub fn config(&self) -> &SmsConfig {
        &self.config
    }

    /// Number of dedup keys currently held (in-memory; durable ACK is Task 3).
    pub fn dedup_len(&self) -> usize {
        self.ledger
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    /// Plain text only. Everything else a messaging channel can do is `false`
    /// because SMS has no wire for it; `max_message_length` is the UCS-2
    /// multipart limit for the configured segment budget.
    fn channel_capabilities(config: &SmsConfig) -> ChannelCapabilities {
        ChannelCapabilities {
            attachments: false,
            images: false,
            audio: false,
            video: false,
            reactions: false,
            replies: false,
            editing: false,
            deletion: false,
            typing_indicator: false,
            read_receipts: false,
            rich_text: false,
            polls: false,
            group_icons: false,
            max_message_length: max_chars_for_segments(config.max_segments),
            max_attachment_size: 0,
            stream_protocol: Default::default(),
        }
    }

    /// Admit one Relay envelope: dedup, fence, expiry (via
    /// `gateway::telecom`), then publish inbound SMS to subscribers.
    ///
    /// `active_generation` is the connection generation of the lease the
    /// frame arrived on; the authenticated binding is the configured
    /// `binding_ref`. An event is recorded in the dedup ledger **only if** it
    /// was actually committed: with no subscriber the ledger is rolled back
    /// and `NoConsumer` returned, so the Relay redelivers instead of the
    /// message being silently lost.
    pub fn ingest_envelope(
        &self,
        env: &TelecomEnvelope,
        active_generation: u64,
        now: DateTime<Utc>,
    ) -> Result<SmsIngest, SmsError> {
        if !matches!(env.kind, TelecomKind::SmsInbound(_)) {
            return Ok(SmsIngest::NotInbound);
        }
        if !self.config.inbound_enabled {
            return Err(SmsError::InboundDisabled);
        }
        // Convert first: a malformed frame must not be recorded as seen.
        let message = convert_inbound(&self.config, &self.info.id, env)?;

        let ctx = AcceptContext {
            authenticated_binding: &self.config.binding_ref,
            active_generation,
            now,
        };
        let mut ledger = self.ledger.lock().unwrap_or_else(PoisonError::into_inner);
        // Bound memory: drop keys past the retention horizon before admitting.
        ledger.prune(now);
        let acceptance = accept_event(&mut ledger, &ctx, env).map_err(SmsError::from)?;
        match &acceptance {
            Acceptance::Duplicate(_) => Ok(SmsIngest::Duplicate),
            Acceptance::Stale(_) => Ok(SmsIngest::Stale),
            Acceptance::Expired(_) => Ok(SmsIngest::Expired),
            Acceptance::Fresh(key) => {
                debug_assert!(acceptance.admits_agent_turn(env));
                let id = message.id.clone();
                if self.channel_state.send_inbound(message) {
                    Ok(SmsIngest::Committed(id))
                } else {
                    // Undo only this event's record; every other key
                    // (including Duplicate/Stale/Expired ones) stays.
                    ledger.forget(key);
                    Err(SmsError::NoConsumer)
                }
            }
        }
    }

    async fn fail_start(&self, reason: &str) -> ChannelError {
        self.channel_state.set_status(ChannelStatus::Error).await;
        self.channel_state
            .record_failure(Some(reason.to_string()))
            .await;
        ChannelError::NotConnected(reason.to_string())
    }
}

#[async_trait]
impl Channel for SmsChannel {
    fn info(&self) -> &ChannelInfo {
        &self.info
    }

    fn state(&self) -> &ChannelState {
        &self.channel_state
    }

    /// One requested SMS is one attempt: it is a separately authorised, paid
    /// external effect, so the generic registry must not retry it or persist
    /// its body for replay.
    fn outbound_delivery_policy(&self) -> OutboundDeliveryPolicy {
        OutboundDeliveryPolicy::ExplicitOnly
    }

    async fn start(&mut self) -> ChannelResult<()> {
        if let Err(e) = self.config.validate() {
            self.channel_state.set_status(ChannelStatus::Error).await;
            return Err(ChannelError::ConfigError(e));
        }
        self.channel_state
            .set_status(ChannelStatus::Connecting)
            .await;
        let Some(transport) = self.transport.clone() else {
            return Err(self
                .fail_start("SMS relay transport is not connected (configured but unavailable)")
                .await);
        };
        if !transport.is_connected().await {
            return Err(self
                .fail_start("SMS relay connection is not established")
                .await);
        }
        self.channel_state
            .set_status(ChannelStatus::Connected)
            .await;
        Ok(())
    }

    async fn stop(&mut self) -> ChannelResult<()> {
        self.channel_state
            .set_status(ChannelStatus::Disconnected)
            .await;
        Ok(())
    }

    async fn send(&self, message: OutboundMessage) -> ChannelResult<SendResult> {
        let Some(transport) = self.transport.as_ref() else {
            return Err(ChannelError::NotConnected(
                "SMS relay transport is not connected; nothing was sent".into(),
            ));
        };
        if self.channel_state.status() != ChannelStatus::Connected {
            return Err(ChannelError::NotConnected(
                "SMS channel is not connected; nothing was sent".into(),
            ));
        }
        // The cached status can be stale (relay dropped after start): ask the
        // transport now, before building or submitting anything.
        if !transport.is_connected().await {
            self.channel_state
                .set_status(ChannelStatus::Disconnected)
                .await;
            self.channel_state
                .record_failure(Some("SMS relay connection lost".to_string()))
                .await;
            return Err(ChannelError::NotConnected(
                "SMS relay connection is not established; nothing was sent".into(),
            ));
        }
        let operation = build_pending_operation(&self.config, &message, Utc::now())?;
        let submission = transport.submit(&operation).await?;
        if submission.operation_id != operation.operation_id() {
            return Err(ChannelError::SendFailed(
                "relay acknowledged a different operation id; outcome unverified".into(),
            ));
        }
        match submission.outcome {
            // The Relay took (or has progressed) the request. Not a delivery claim.
            TelecomOutcome::Accepted | TelecomOutcome::Submitted | TelecomOutcome::Delivered => {
                Ok(SendResult {
                    message_id: MessageId::new(submission.operation_id),
                    timestamp: Utc::now(),
                })
            }
            TelecomOutcome::Unknown => Err(ChannelError::SendFailed(format!(
                "SMS outcome unknown for operation {}: it may have been accepted; not retried",
                submission.operation_id
            ))),
            other => Err(ChannelError::SendFailed(format!(
                "SMS operation {} ended as {}",
                submission.operation_id,
                other.as_wire()
            ))),
        }
    }
}

/// Factory for `[channels.sms]`.
pub struct SmsChannelFactory;

#[async_trait]
impl ChannelFactory for SmsChannelFactory {
    fn channel_type(&self) -> &str {
        "sms"
    }

    async fn create(&self, config: serde_json::Value) -> ChannelResult<Box<dyn Channel>> {
        self.create_with_id("sms", config).await
    }

    async fn create_with_id(
        &self,
        id: &str,
        config: serde_json::Value,
    ) -> ChannelResult<Box<dyn Channel>> {
        let config: SmsConfig = serde_json::from_value(config)
            .map_err(|e| ChannelError::ConfigError(format!("Invalid SMS config: {e}")))?;
        config.validate().map_err(ChannelError::ConfigError)?;
        Ok(Box::new(SmsChannel::new(id, config)))
    }
}
