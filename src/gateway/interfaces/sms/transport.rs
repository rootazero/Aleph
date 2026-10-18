//! Offline Core-side lease, admission, and outcome seam for an authenticated
//! SMS Relay.
//!
//! This module deliberately contains no network, URL, TLS, credential, or
//! webhook implementation. A future deployment supplies [`SmsRelayPeer`].
//! The peer receives the authenticated lease and the unchanged operation; it
//! cannot install a generation from an arbitrary envelope.

use std::collections::HashMap;
use std::sync::Arc;

use aleph_protocol::telecom_relay::{EnvelopeError, TelecomEnvelope, TelecomKind, TelecomOutcome};
use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use tokio_util::sync::CancellationToken;

use crate::gateway::telecom::public_context::{CallerDigest, TelecomChannel, TelecomPublicContext};
use crate::gateway::telecom::public_run::ApprovedReplyOutcome;

use crate::gateway::channel::{ChannelError, ChannelResult};
use crate::gateway::telecom::{
    advance_outcome, expire_unknown, fence_generation, OutcomeRecord, OutcomeTransition,
    TelecomError, UnknownExpiry,
};

use super::{
    estimate_segments, normalize_e164, PendingSmsOperation, SmsChannel, SmsConfig, SmsError,
    SmsIngest, MAX_COMMAND_TTL_SECS, MAX_SEGMENTS_LIMIT,
};

/// What a future authenticated Relay adapter must implement. This is a peer
/// seam, not a provider client: production code must supply the connection,
/// authentication, and TLS policy outside this crate.
#[async_trait]
pub trait SmsRelayPeer: Send + Sync {
    /// Establish the peer side of one Core-owned lease generation.
    async fn connect(&self, binding_id: &str, generation: u64) -> Result<(), SmsTransportError>;

    /// Prove that this exact lease is still alive.
    async fn heartbeat(&self, binding_id: &str, generation: u64) -> Result<(), SmsTransportError>;

    /// Submit the operation as-is. `UnknownWriteOutcome` means a frame write
    /// was attempted but the peer could not prove whether it was accepted.
    ///
    /// The peer must revalidate `generation` at the last irreversible write.
    /// Core cannot make disconnect and a remote write atomic; that peer-side
    /// generation fence is part of the authenticated adapter contract.
    async fn submit(
        &self,
        binding_id: &str,
        generation: u64,
        operation: &PendingSmsOperation,
    ) -> Result<TelecomOutcome, SmsTransportError>;
}

/// Naming seam for deployments that prefer to call the peer a driver.
pub trait SmsTransportDriver: SmsRelayPeer {}
impl<T: SmsRelayPeer + ?Sized> SmsTransportDriver for T {}

/// Core-owned authorization/admission capability. No capability means no
/// submission; the optional wire approval reference is not authorization.
#[async_trait]
pub trait SmsSendAuthorizer: Send + Sync {
    async fn authorize(&self, operation: &PendingSmsOperation) -> Result<(), SmsTransportError>;
}

/// What the Relay said when handed a pending operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmsSubmission {
    pub operation_id: String,
    pub outcome: TelecomOutcome,
}

/// Public channel transport contract retained for existing callers. This
/// contract reports only the peer's determined first outcome.
#[async_trait]
pub trait SmsTransport: Send + Sync {
    async fn is_connected(&self) -> bool;
    async fn submit(&self, operation: &PendingSmsOperation) -> ChannelResult<SmsSubmission>;

    /// Legacy/fake transports confer no private authority.
    async fn private_sms_lease(
        &self,
        _ctx: &TelecomPublicContext,
        _recipient: &str,
    ) -> Option<PrivateSmsLease> {
        None
    }

    async fn submit_private_sms(
        &self,
        _operation: &PendingSmsOperation,
        _lease: &PrivateSmsLease,
        _cancel: &CancellationToken,
        _validate: &(dyn for<'a> Fn(&'a str) -> bool + Send + Sync),
    ) -> ChannelResult<ApprovedReplyOutcome> {
        Ok(ApprovedReplyOutcome::Invalid)
    }

    /// Safe default for transports without lease authority.
    async fn disconnect(&self) {}
}

/// Errors produced before a transport can truthfully report a submission.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SmsTransportError {
    #[error("SMS relay lease is not connected")]
    NotConnected,
    #[error("private SMS authority is invalid or revoked")]
    PrivateInvalid,
    #[error("private SMS cancelled before submission")]
    PrivateCancelled,
    #[error("binding mismatch: lease is bound to {expected}, operation/event claims {claimed}")]
    BindingMismatch { expected: String, claimed: String },
    #[error("stale connection generation {got} (active {active})")]
    StaleGeneration { active: u64, got: u64 },
    #[error("future connection generation {got} (active {active})")]
    FutureGeneration { active: u64, got: u64 },
    #[error("SMS relay connection generation is exhausted")]
    GenerationExhausted,
    #[error("SMS operation {operation_id} expired at {expires_at}")]
    Expired {
        operation_id: String,
        expires_at: DateTime<Utc>,
    },
    #[error("SMS operation {operation_id} already has outcome {outcome:?}")]
    AlreadySubmitted {
        operation_id: String,
        outcome: TelecomOutcome,
    },
    #[error("SMS operation {operation_id} is already being submitted")]
    OperationPending { operation_id: String },
    #[error("SMS operation {operation_id} conflicts with the stored operation")]
    OperationConflict { operation_id: String },
    #[error("SMS operation {operation_id} has unknown outcome and cannot be retried")]
    Unknown { operation_id: String },
    #[error(
        "SMS operation {operation_id} is outcome_unknown_expired and requires a new operation"
    )]
    UnknownExpired { operation_id: String },
    #[error("SMS operation {operation_id} was not found")]
    OperationNotFound { operation_id: String },
    #[error("SMS operation {operation_id} has no valid segment admission")]
    InvalidSegmentAdmission { operation_id: String },
    #[error("SMS operation {operation_id} segment admission disagrees with its body")]
    SegmentAdmissionMismatch { operation_id: String },
    #[error("SMS outbound submission is disabled")]
    OutboundDisabled,
    #[error("SMS operation {operation_id} has an invalid peer address")]
    InvalidPeerAddress { operation_id: String },
    #[error("SMS operation {operation_id} has an empty body")]
    EmptyBody { operation_id: String },
    #[error("SMS operation {operation_id} exceeds the configured segment policy")]
    SegmentLimitExceeded { operation_id: String },
    #[error("SMS operation {operation_id} exceeds the configured command TTL")]
    CommandTtlExceeded { operation_id: String },
    #[error("SMS authorization capability is unavailable")]
    AuthorizationUnavailable,
    #[error("SMS authorization denied: {0}")]
    AuthorizationDenied(String),
    #[error("invalid SMS transport configuration: {0}")]
    InvalidConfiguration(String),
    #[error("Relay peer rejected the operation: {0}")]
    Peer(String),
    #[error("Relay write outcome is unknown for operation {operation_id}")]
    UnknownWriteOutcome { operation_id: String },
    #[error("unexpected telecom envelope kind for SMS transport")]
    WrongEnvelopeKind,
    #[error("SMS inbound handling failed: {0}")]
    Inbound(String),
    #[error(transparent)]
    Envelope(#[from] EnvelopeError),
    #[error(transparent)]
    Telecom(#[from] TelecomError),
}

/// Immutable admission policy applied again to caller-provided operations.
pub const DEFAULT_UNKNOWN_RETENTION_SECS: u64 = 3_600;
pub const MAX_UNKNOWN_RETENTION_SECS: u64 = 86_400;
const TERMINAL_OPERATION_RETENTION_SECS: u64 = 7 * 24 * 3_600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SmsTransportPolicy {
    pub outbound_enabled: bool,
    pub max_segments: u32,
    pub command_ttl_secs: u64,
    pub unknown_retention_secs: u64,
}

impl Default for SmsTransportPolicy {
    fn default() -> Self {
        Self {
            outbound_enabled: true,
            max_segments: MAX_SEGMENTS_LIMIT,
            command_ttl_secs: MAX_COMMAND_TTL_SECS,
            unknown_retention_secs: DEFAULT_UNKNOWN_RETENTION_SECS,
        }
    }
}

impl SmsTransportPolicy {
    pub fn with_unknown_retention_secs(mut self, secs: u64) -> Result<Self, SmsTransportError> {
        if !(1..=MAX_UNKNOWN_RETENTION_SECS).contains(&secs) {
            return Err(SmsTransportError::InvalidConfiguration(format!(
                "unknown_retention_secs must be within 1..={MAX_UNKNOWN_RETENTION_SECS}"
            )));
        }
        self.unknown_retention_secs = secs;
        Ok(self)
    }
}

fn valid_policy(policy: &SmsTransportPolicy) -> bool {
    (1..=MAX_UNKNOWN_RETENTION_SECS).contains(&policy.unknown_retention_secs)
        && (1..=MAX_SEGMENTS_LIMIT).contains(&policy.max_segments)
        && (1..=MAX_COMMAND_TTL_SECS).contains(&policy.command_ttl_secs)
}

impl From<&SmsConfig> for SmsTransportPolicy {
    fn from(config: &SmsConfig) -> Self {
        Self {
            outbound_enabled: config.outbound_enabled,
            max_segments: config.max_segments,
            command_ttl_secs: config.command_ttl_secs,
            unknown_retention_secs: DEFAULT_UNKNOWN_RETENTION_SECS,
        }
    }
}

/// The authenticated lease currently accepted by Core.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayLease {
    pub binding_id: String,
    pub generation: u64,
}

/// Opaque authority issued only by a live configured transport. No constructor,
/// serialization or Debug surface exposes its authority. Clones pin the SAME epoch.
#[derive(Clone)]
pub struct PrivateSmsLease {
    relay: RelayLease,
    epoch: Arc<CancellationToken>,
    ctx: TelecomPublicContext,
}

impl PrivateSmsLease {
    pub fn is_active(&self) -> bool {
        !self.epoch.is_cancelled()
    }

    pub async fn revoked(&self) {
        self.epoch.cancelled().await;
    }

    pub(crate) fn matches_context(&self, ctx: &TelecomPublicContext) -> bool {
        self.is_active() && self.ctx == *ctx
    }

    pub(crate) fn matches_channel(&self, config: &SmsConfig, recipient: &str) -> bool {
        self.is_active()
            && config.validate().is_ok()
            && self.ctx.provenance().region() == config.region
            && self.ctx.provenance().binding_ref() == config.binding_ref
            && self.matches_recipient(recipient)
    }

    fn matches_recipient(&self, recipient: &str) -> bool {
        normalize_e164(recipient).is_ok_and(|peer| {
            *self.ctx.provenance().caller_digest() == CallerDigest::from_normalized_e164(&peer)
        })
    }
}

struct ActiveLease {
    relay: RelayLease,
    epoch: Arc<CancellationToken>,
}

fn revoke_active(state: &mut TransportState) {
    if let Some(active) = state.active.take() {
        // Cancellation and removal are one authoritative state-lock transition.
        active.epoch.cancel();
    }
}

struct PinnedGuard<'a> {
    lease: &'a PrivateSmsLease,
    cancel: &'a CancellationToken,
    validate: &'a (dyn for<'s> Fn(&'s str) -> bool + Send + Sync + 'a),
}

impl PinnedGuard<'_> {
    fn check(
        &self,
        state: &TransportState,
        operation: &PendingSmsOperation,
    ) -> Result<(), SmsTransportError> {
        if self.cancel.is_cancelled() {
            return Err(SmsTransportError::PrivateCancelled);
        }
        let active = state
            .active
            .as_ref()
            .ok_or(SmsTransportError::PrivateInvalid)?;
        if !self.lease.is_active()
            || !Arc::ptr_eq(&active.epoch, &self.lease.epoch)
            || active.relay != self.lease.relay
            || !self.lease.matches_recipient(&operation.peer_address)
            || operation.command.recipient_binding_id != self.lease.relay.binding_id
        {
            return Err(SmsTransportError::PrivateInvalid);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OperationPhase {
    Authorizing,
    Submitting,
    Settled,
}

#[derive(Clone)]
struct StoredOperation {
    operation: PendingSmsOperation,
    outcome: Option<OutcomeRecord>,
    phase: OperationPhase,
}

struct TransportState {
    next_generation: u64,
    active: Option<ActiveLease>,
    connecting_generation: Option<u64>,
    operations: HashMap<String, StoredOperation>,
}

/// ACK status is deliberately not a network ACK. The actual Relay adapter is
/// not present in this repository, so callers must not claim persistence on the
/// wire based on a local channel result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayAckState {
    NetworkAckPending,
    NotEligible,
}

impl RelayAckState {
    pub fn for_ingest(ingest: SmsIngest) -> Self {
        match ingest {
            SmsIngest::Committed(_) | SmsIngest::Duplicate | SmsIngest::Expired => {
                Self::NetworkAckPending
            }
            SmsIngest::Stale | SmsIngest::NotInbound => Self::NotEligible,
        }
    }
}

/// Typed result returned after binding/generation fencing and existing channel
/// ingestion. `NetworkAckPending` records the missing Relay adapter honestly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmsInboundRelayResult {
    pub ingest: SmsIngest,
    pub ack: RelayAckState,
}

/// Core-side deterministic lease and operation state machine.
pub struct SmsRelayTransport {
    binding_id: String,
    peer: Arc<dyn SmsRelayPeer>,
    authorizer: Arc<dyn SmsSendAuthorizer>,
    policy: SmsTransportPolicy,
    config_valid: bool,
    private_region: Option<String>,
    connect_lock: tokio::sync::Mutex<()>,
    state: tokio::sync::Mutex<TransportState>,
}

impl SmsRelayTransport {
    /// Construct a dormant transport. It is not a factory activation and does
    /// not create a connection until [`Self::connect`] is called.
    pub fn new(
        binding_id: impl Into<String>,
        peer: Arc<dyn SmsRelayPeer>,
        authorizer: Arc<dyn SmsSendAuthorizer>,
    ) -> Self {
        Self::new_internal(
            binding_id.into(),
            SmsTransportPolicy::default(),
            true,
            peer,
            authorizer,
        )
    }

    /// Construct a transport with an explicit retention policy. Invalid
    /// policy values are retained as a marker and fail closed at submission.
    pub fn new_with_policy(
        binding_id: impl Into<String>,
        policy: SmsTransportPolicy,
        peer: Arc<dyn SmsRelayPeer>,
        authorizer: Arc<dyn SmsSendAuthorizer>,
    ) -> Self {
        let policy_valid = valid_policy(&policy);
        Self::new_internal(binding_id.into(), policy, policy_valid, peer, authorizer)
    }

    /// Construct a transport whose admission policy is derived from the
    /// channel configuration. Invalid configuration is retained as a marker
    /// and fails closed at submission rather than being silently widened.
    pub fn new_with_config(
        binding_id: impl Into<String>,
        config: SmsConfig,
        peer: Arc<dyn SmsRelayPeer>,
        authorizer: Arc<dyn SmsSendAuthorizer>,
    ) -> Self {
        let binding_id = binding_id.into();
        let policy = SmsTransportPolicy::from(&config);
        let config_valid =
            config.validate().is_ok() && valid_policy(&policy) && config.binding_ref == binding_id;
        let mut transport = Self::new_internal(binding_id, policy, config_valid, peer, authorizer);
        transport.private_region = Some(config.region);
        transport
    }

    fn new_internal(
        binding_id: String,
        policy: SmsTransportPolicy,
        config_valid: bool,
        peer: Arc<dyn SmsRelayPeer>,
        authorizer: Arc<dyn SmsSendAuthorizer>,
    ) -> Self {
        Self {
            binding_id,
            peer,
            authorizer,
            policy,
            config_valid,
            private_region: None,
            connect_lock: tokio::sync::Mutex::new(()),
            state: tokio::sync::Mutex::new(TransportState {
                next_generation: 0,
                active: None,
                connecting_generation: None,
                operations: HashMap::new(),
            }),
        }
    }

    pub async fn connect(&self) -> Result<RelayLease, SmsTransportError> {
        let _connect_guard = self.connect_lock.lock().await;
        let generation = {
            let mut state = self.state.lock().await;
            let generation = state
                .next_generation
                .checked_add(1)
                .ok_or(SmsTransportError::GenerationExhausted)?;
            state.next_generation = generation;
            state.connecting_generation = Some(generation);
            // Reserve the generation before awaiting the peer. The old lease
            // remains available only for cleanup/audit, never admission.
            revoke_active(&mut state);
            generation
        };
        let result = self.peer.connect(&self.binding_id, generation).await;
        let lease = RelayLease {
            binding_id: self.binding_id.clone(),
            generation,
        };
        let mut state = self.state.lock().await;
        let still_reserved = state.connecting_generation == Some(generation);
        if still_reserved {
            state.connecting_generation = None;
        }
        result?;
        if !still_reserved || state.next_generation != generation {
            // An explicit disconnect or a newer reservation invalidated this
            // attempt; never resurrect the old lease on a late peer response.
            return Err(SmsTransportError::NotConnected);
        }
        state.active = Some(ActiveLease {
            relay: lease.clone(),
            epoch: Arc::new(CancellationToken::new()),
        });
        Ok(lease)
    }

    pub async fn disconnect(&self) {
        let mut state = self.state.lock().await;
        revoke_active(&mut state);
        state.connecting_generation = None;
    }

    pub async fn lease(&self) -> Option<RelayLease> {
        self.state
            .lock()
            .await
            .active
            .as_ref()
            .map(|active| active.relay.clone())
    }

    pub async fn fence_generation(&self, generation: u64) -> Result<(), SmsTransportError> {
        let state = self.state.lock().await;
        let lease = current_lease(&state)?;
        check_generation(lease.generation, generation)
    }

    pub async fn heartbeat(&self, generation: u64) -> Result<(), SmsTransportError> {
        let lease = {
            let state = self.state.lock().await;
            let lease = current_lease(&state)?;
            check_generation(lease.generation, generation)?;
            lease
        };
        let result = self
            .peer
            .heartbeat(&lease.binding_id, lease.generation)
            .await;
        let mut state = self.state.lock().await;
        match result {
            Err(error) => {
                if state.active.as_ref().map(|active| &active.relay) == Some(&lease) {
                    revoke_active(&mut state);
                }
                Err(error)
            }
            Ok(()) => {
                // A peer success is only provisional: disconnect/reconnect may
                // have replaced this lease while the peer was awaiting I/O.
                let current = current_lease(&state)?;
                check_generation(current.generation, lease.generation)?;
                if current.binding_id != lease.binding_id {
                    return Err(SmsTransportError::BindingMismatch {
                        expected: current.binding_id,
                        claimed: lease.binding_id,
                    });
                }
                Ok(())
            }
        }
    }

    /// Submit at a caller-provided time so expiry and boundary behavior are
    /// deterministic in tests and in a future scheduler.
    pub async fn submit_at(
        &self,
        operation: &PendingSmsOperation,
        now: DateTime<Utc>,
    ) -> Result<TelecomOutcome, SmsTransportError> {
        self.submit_impl(operation, now, None).await
    }

    async fn submit_impl(
        &self,
        operation: &PendingSmsOperation,
        now: DateTime<Utc>,
        pinned: Option<PinnedGuard<'_>>,
    ) -> Result<TelecomOutcome, SmsTransportError> {
        let operation_id = operation.operation_id().to_string();
        let lease = {
            let mut state = match &pinned {
                Some(guard) => tokio::select! {
                    biased;
                    _ = guard.cancel.cancelled() => return Err(SmsTransportError::PrivateCancelled),
                    _ = guard.lease.revoked() => return Err(SmsTransportError::PrivateInvalid),
                    state = self.state.lock() => state,
                },
                None => self.state.lock().await,
            };
            if let Some(guard) = &pinned {
                guard.check(&state, operation)?;
            }
            prune_operations(&mut state, now, &self.policy, None);
            let lease = current_lease(&state)?;
            if !self.config_valid {
                return Err(SmsTransportError::InvalidConfiguration(
                    "SmsConfig::validate() failed".into(),
                ));
            }
            if let Some(existing) = state.operations.get(&operation_id) {
                if existing.operation != *operation {
                    return Err(SmsTransportError::OperationConflict { operation_id });
                }
                if let Some(record) = existing.outcome {
                    return match record.outcome {
                        TelecomOutcome::Unknown => Err(SmsTransportError::Unknown { operation_id }),
                        TelecomOutcome::OutcomeUnknownExpired => {
                            Err(SmsTransportError::UnknownExpired { operation_id })
                        }
                        outcome => Err(SmsTransportError::AlreadySubmitted {
                            operation_id,
                            outcome,
                        }),
                    };
                }
                return Err(SmsTransportError::OperationPending { operation_id });
            }
            validate_operation_admission(&self.policy, &lease, operation, now)?;
            state.operations.insert(
                operation_id.clone(),
                StoredOperation {
                    operation: operation.clone(),
                    outcome: None,
                    phase: OperationPhase::Authorizing,
                },
            );
            lease
        };

        let authorization = match &pinned {
            Some(guard) => tokio::select! {
                biased;
                _ = guard.cancel.cancelled() => Err(SmsTransportError::PrivateCancelled),
                _ = guard.lease.revoked() => Err(SmsTransportError::PrivateInvalid),
                result = self.authorizer.authorize(operation) => result,
            },
            None => self.authorizer.authorize(operation).await,
        };
        if let Err(error) = authorization {
            let mut state = self.state.lock().await;
            if state.operations.get(&operation_id).is_some_and(|stored| {
                stored.operation == *operation
                    && stored.outcome.is_none()
                    && stored.phase == OperationPhase::Authorizing
            }) {
                state.operations.remove(&operation_id);
            }
            return Err(error);
        }

        // This is the final Core-side fence before the irreversible peer call.
        // The authenticated peer must perform its own generation check at the
        // actual write, because disconnect and a remote write are not atomic.
        let lease = {
            let mut state = self.state.lock().await;
            prune_operations(&mut state, now, &self.policy, None);
            let current = current_lease(&state);
            let final_lease = current.and_then(|current| {
                check_generation(current.generation, lease.generation)?;
                if current.binding_id != lease.binding_id {
                    return Err(SmsTransportError::BindingMismatch {
                        expected: current.binding_id,
                        claimed: lease.binding_id.clone(),
                    });
                }
                Ok(current)
            });
            let result = (|| {
                let current = final_lease?;
                if let Some(guard) = &pinned {
                    guard.check(&state, operation)?;
                    // The callback is deliberately AFTER the last async state
                    // lock and authorizer wait, at the local commit transition.
                    if !(guard.validate)(&operation.command.body) {
                        return Err(SmsTransportError::PrivateInvalid);
                    }
                    guard.check(&state, operation)?;
                }
                validate_operation_admission(
                    &self.policy,
                    &current,
                    operation,
                    if pinned.is_some() { Utc::now() } else { now },
                )?;
                let stored = state.operations.get_mut(&operation_id).ok_or_else(|| {
                    SmsTransportError::OperationNotFound {
                        operation_id: operation_id.clone(),
                    }
                })?;
                if stored.operation != *operation {
                    return Err(SmsTransportError::OperationConflict {
                        operation_id: operation_id.clone(),
                    });
                }
                if let Some(record) = stored.outcome {
                    return Err(match record.outcome {
                        TelecomOutcome::Unknown => SmsTransportError::Unknown {
                            operation_id: operation_id.clone(),
                        },
                        TelecomOutcome::OutcomeUnknownExpired => {
                            SmsTransportError::UnknownExpired {
                                operation_id: operation_id.clone(),
                            }
                        }
                        outcome => SmsTransportError::AlreadySubmitted {
                            operation_id: operation_id.clone(),
                            outcome,
                        },
                    });
                }
                if stored.phase != OperationPhase::Authorizing {
                    return Err(SmsTransportError::OperationPending {
                        operation_id: operation_id.clone(),
                    });
                }
                stored.phase = OperationPhase::Submitting;
                // Never substitute a replacement generation in the payload.
                Ok(lease.clone())
            })();
            match result {
                Ok(current) => current,
                Err(error) => {
                    if state.operations.get(&operation_id).is_some_and(|stored| {
                        stored.operation == *operation && stored.outcome.is_none()
                    }) {
                        state.operations.remove(&operation_id);
                    }
                    return Err(error);
                }
            }
        };

        let result = self
            .peer
            .submit(&lease.binding_id, lease.generation, operation)
            .await;
        let mut state = self.state.lock().await;
        match result {
            Ok(outcome) => {
                let current = state
                    .operations
                    .get(&operation_id)
                    .and_then(|stored| stored.outcome.map(|record| record.outcome));
                let transition = match advance_outcome(current, outcome) {
                    Ok(transition) => transition,
                    Err(error) => {
                        if state
                            .operations
                            .get(&operation_id)
                            .is_some_and(|stored| stored.outcome.is_none())
                        {
                            state.operations.remove(&operation_id);
                        }
                        return Err(SmsTransportError::Telecom(error));
                    }
                };
                if let Some(stored) = state.operations.get_mut(&operation_id) {
                    stored.phase = OperationPhase::Settled;
                    if let OutcomeTransition::Applied { to, .. } = transition {
                        stored.outcome = Some(OutcomeRecord {
                            outcome: to,
                            entered_at: now,
                        });
                    }
                }
                Ok(outcome)
            }
            Err(SmsTransportError::UnknownWriteOutcome { .. }) => {
                let current = state
                    .operations
                    .get(&operation_id)
                    .and_then(|stored| stored.outcome.map(|record| record.outcome));
                match advance_outcome(current, TelecomOutcome::Unknown) {
                    Ok(OutcomeTransition::Applied { to, .. }) => {
                        if let Some(stored) = state.operations.get_mut(&operation_id) {
                            stored.phase = OperationPhase::Settled;
                            stored.outcome = Some(OutcomeRecord {
                                outcome: to,
                                entered_at: now,
                            });
                        }
                    }
                    Ok(_) => {
                        if let Some(stored) = state.operations.get_mut(&operation_id) {
                            stored.phase = OperationPhase::Settled;
                        }
                    }
                    Err(error) => return Err(SmsTransportError::Telecom(error)),
                }
                Err(SmsTransportError::Unknown { operation_id })
            }
            Err(error) => {
                if state
                    .operations
                    .get(&operation_id)
                    .is_some_and(|stored| stored.outcome.is_none())
                {
                    state.operations.remove(&operation_id);
                }
                Err(error)
            }
        }
    }

    pub async fn expire_unknown(
        &self,
        operation_id: &str,
        now: DateTime<Utc>,
    ) -> Result<OutcomeRecord, SmsTransportError> {
        let mut state = self.state.lock().await;
        prune_operations(&mut state, now, &self.policy, Some(operation_id));
        let stored = state.operations.get_mut(operation_id).ok_or_else(|| {
            SmsTransportError::OperationNotFound {
                operation_id: operation_id.to_string(),
            }
        })?;
        let record = stored
            .outcome
            .ok_or_else(|| SmsTransportError::OperationPending {
                operation_id: operation_id.to_string(),
            })?;
        match expire_unknown(
            &record,
            now,
            Duration::seconds(self.policy.unknown_retention_secs as i64),
        ) {
            UnknownExpiry::Expired(expired) => {
                // The explicit-id exemption only protects the unknown record
                // long enough for this explicit transition. Once it is a
                // tombstone, apply the ordinary terminal horizon immediately.
                stored.outcome = Some(expired);
                stored.phase = OperationPhase::Settled;
                prune_operations(&mut state, now, &self.policy, None);
                Ok(expired)
            }
            UnknownExpiry::Pending { .. } => Err(SmsTransportError::Unknown {
                operation_id: operation_id.to_string(),
            }),
            UnknownExpiry::NotUnknown => Err(SmsTransportError::AlreadySubmitted {
                operation_id: operation_id.to_string(),
                outcome: record.outcome,
            }),
        }
    }

    pub async fn outcome(&self, operation_id: &str) -> Option<OutcomeRecord> {
        self.state
            .lock()
            .await
            .operations
            .get(operation_id)
            .and_then(|stored| stored.outcome)
    }

    /// Return the original immutable operation for audit, including its
    /// operation id, body-free debug representation, encoding, segments, and
    /// original deadline. No retry creates a replacement record.
    pub async fn operation(&self, operation_id: &str) -> Option<PendingSmsOperation> {
        self.state
            .lock()
            .await
            .operations
            .get(operation_id)
            .map(|stored| stored.operation.clone())
    }

    /// Consume a shared `Outcome` envelope only after the current lease fence.
    pub async fn record_outcome(
        &self,
        envelope: &TelecomEnvelope,
        now: DateTime<Utc>,
    ) -> Result<OutcomeTransition, SmsTransportError> {
        let mut state = self.state.lock().await;
        prune_operations(&mut state, now, &self.policy, None);
        let lease = current_lease(&state)?;
        fence_envelope(&lease, envelope)?;
        let TelecomKind::Outcome(report) = &envelope.kind else {
            return Err(SmsTransportError::WrongEnvelopeKind);
        };
        let stored = state
            .operations
            .get_mut(&report.operation_id)
            .ok_or_else(|| SmsTransportError::OperationNotFound {
                operation_id: report.operation_id.clone(),
            })?;
        let current = stored.outcome.map(|record| record.outcome);
        let transition = advance_outcome(current, report.outcome)?;
        if let OutcomeTransition::Applied { to, .. } = transition {
            stored.outcome = Some(OutcomeRecord {
                outcome: to,
                entered_at: now,
            });
            stored.phase = OperationPhase::Settled;
        }
        // provider_account_id and provider_event_id are Relay-authenticated
        // correlation metadata. They are not Core authorization identity;
        // operation_id remains the unique binding to the stored operation until
        // a real authenticated Relay adapter supplies an account policy.
        Ok(transition)
    }

    /// Fence before delegating to the existing channel ingress path. No second
    /// envelope schema or inbound router is introduced here.
    pub async fn ingest_envelope(
        &self,
        channel: &SmsChannel,
        envelope: &TelecomEnvelope,
        now: DateTime<Utc>,
    ) -> Result<SmsInboundRelayResult, SmsTransportError> {
        // Keep the state guard across the synchronous effect: reconnect cannot
        // replace the lease between fencing and channel ingestion.
        let state = self.state.lock().await;
        let lease = current_lease(&state)?;
        fence_envelope(&lease, envelope)?;
        if !matches!(envelope.kind, TelecomKind::SmsInbound(_)) {
            return Err(SmsTransportError::WrongEnvelopeKind);
        }
        let ingest = channel
            .ingest_envelope(envelope, lease.generation, now)
            .map_err(|error: SmsError| SmsTransportError::Inbound(error.to_string()))?;
        Ok(SmsInboundRelayResult {
            ack: RelayAckState::for_ingest(ingest.clone()),
            ingest,
        })
    }
}

#[async_trait]
impl super::SmsTransport for SmsRelayTransport {
    async fn disconnect(&self) {
        SmsRelayTransport::disconnect(self).await;
    }

    async fn private_sms_lease(
        &self,
        ctx: &TelecomPublicContext,
        recipient: &str,
    ) -> Option<PrivateSmsLease> {
        let state = self.state.lock().await;
        let active = state.active.as_ref()?;
        let provenance = ctx.provenance();
        let peer = normalize_e164(recipient).ok()?;
        if !self.config_valid
            || provenance.channel() != TelecomChannel::Sms
            || self.private_region.as_deref() != Some(provenance.region())
            || provenance.binding_ref() != active.relay.binding_id
            || provenance.connection_generation() != active.relay.generation
            || *provenance.caller_digest() != CallerDigest::from_normalized_e164(&peer)
            || active.epoch.is_cancelled()
        {
            return None;
        }
        Some(PrivateSmsLease {
            relay: active.relay.clone(),
            epoch: active.epoch.clone(),
            ctx: ctx.clone(),
        })
    }

    async fn submit_private_sms(
        &self,
        operation: &PendingSmsOperation,
        lease: &PrivateSmsLease,
        cancel: &CancellationToken,
        validate: &(dyn for<'a> Fn(&'a str) -> bool + Send + Sync),
    ) -> ChannelResult<ApprovedReplyOutcome> {
        match self
            .submit_impl(
                operation,
                Utc::now(),
                Some(PinnedGuard {
                    lease,
                    cancel,
                    validate,
                }),
            )
            .await
        {
            Ok(
                TelecomOutcome::Accepted | TelecomOutcome::Submitted | TelecomOutcome::Delivered,
            ) => Ok(ApprovedReplyOutcome::Acknowledged),
            Ok(_) => Err(ChannelError::SendFailed(
                "private SMS outcome unacknowledged; not retried".into(),
            )),
            Err(SmsTransportError::PrivateCancelled) => Ok(ApprovedReplyOutcome::Cancelled),
            Err(
                SmsTransportError::PrivateInvalid
                | SmsTransportError::NotConnected
                | SmsTransportError::StaleGeneration { .. }
                | SmsTransportError::FutureGeneration { .. },
            ) => Ok(ApprovedReplyOutcome::Invalid),
            Err(error) => Err(to_channel_error(error)),
        }
    }

    async fn is_connected(&self) -> bool {
        self.lease().await.is_some()
    }

    async fn submit(&self, operation: &PendingSmsOperation) -> ChannelResult<SmsSubmission> {
        self.submit_at(operation, Utc::now())
            .await
            .map(|outcome| SmsSubmission {
                operation_id: operation.operation_id().to_string(),
                outcome,
            })
            .map_err(to_channel_error)
    }
}

fn current_lease(state: &TransportState) -> Result<RelayLease, SmsTransportError> {
    state
        .active
        .as_ref()
        .map(|active| active.relay.clone())
        .ok_or(SmsTransportError::NotConnected)
}

fn prune_operations(
    state: &mut TransportState,
    now: DateTime<Utc>,
    policy: &SmsTransportPolicy,
    explicit_expiry_id: Option<&str>,
) {
    state.operations.retain(|operation_id, stored| {
        if stored.outcome.is_none() {
            return true;
        }
        let record = stored.outcome.expect("checked above");
        if record.outcome == TelecomOutcome::Unknown {
            // Only a still-unknown record needs protection until the caller
            // performs explicit expiry. Existing tombstones never bypass GC.
            if explicit_expiry_id == Some(operation_id.as_str()) {
                return true;
            }
            let unknown_deadline =
                record.entered_at + Duration::seconds(policy.unknown_retention_secs as i64);
            if now < unknown_deadline {
                return true;
            }
            // Unknown is never dropped at its first horizon: it becomes a
            // terminal tombstone so the original operation identity cannot
            // be submitted again during the terminal retention horizon.
            stored.outcome = Some(OutcomeRecord {
                outcome: TelecomOutcome::OutcomeUnknownExpired,
                entered_at: unknown_deadline,
            });
            stored.phase = OperationPhase::Settled;
        }
        let terminal = stored.outcome.expect("unknown was converted above");
        now < terminal.entered_at + Duration::seconds(TERMINAL_OPERATION_RETENTION_SECS as i64)
    });
}

fn check_generation(active: u64, got: u64) -> Result<(), SmsTransportError> {
    match got.cmp(&active) {
        std::cmp::Ordering::Equal => Ok(()),
        std::cmp::Ordering::Less => Err(SmsTransportError::StaleGeneration { active, got }),
        std::cmp::Ordering::Greater => Err(SmsTransportError::FutureGeneration { active, got }),
    }
}

fn fence_envelope(lease: &RelayLease, envelope: &TelecomEnvelope) -> Result<(), SmsTransportError> {
    envelope.check_version()?;
    if envelope.binding_id != lease.binding_id {
        return Err(SmsTransportError::BindingMismatch {
            expected: lease.binding_id.clone(),
            claimed: envelope.binding_id.clone(),
        });
    }
    fence_generation(lease.generation, envelope.connection_generation).map_err(
        |error| match error {
            TelecomError::StaleGeneration { active, got } => {
                SmsTransportError::StaleGeneration { active, got }
            }
            TelecomError::FutureGeneration { active, got } => {
                SmsTransportError::FutureGeneration { active, got }
            }
            other => SmsTransportError::Telecom(other),
        },
    )
}

fn validate_operation_admission(
    policy: &SmsTransportPolicy,
    lease: &RelayLease,
    operation: &PendingSmsOperation,
    now: DateTime<Utc>,
) -> Result<(), SmsTransportError> {
    let operation_id = operation.operation_id().to_string();
    if operation.command.recipient_binding_id != lease.binding_id {
        return Err(SmsTransportError::BindingMismatch {
            expected: lease.binding_id.clone(),
            claimed: operation.command.recipient_binding_id.clone(),
        });
    }
    if !policy.outbound_enabled {
        return Err(SmsTransportError::OutboundDisabled);
    }
    if normalize_e164(&operation.peer_address).is_err() {
        return Err(SmsTransportError::InvalidPeerAddress { operation_id });
    }
    if operation.command.body.is_empty() {
        return Err(SmsTransportError::EmptyBody { operation_id });
    }
    if now >= operation.command.expires_at {
        return Err(SmsTransportError::Expired {
            operation_id,
            expires_at: operation.command.expires_at,
        });
    }
    validate_segment_admission(operation)?;
    if operation.command.segment_count > policy.max_segments {
        return Err(SmsTransportError::SegmentLimitExceeded { operation_id });
    }
    let max_ttl_secs = policy.command_ttl_secs.min(MAX_COMMAND_TTL_SECS);
    if operation.command.expires_at.signed_duration_since(now)
        > Duration::seconds(max_ttl_secs as i64)
    {
        return Err(SmsTransportError::CommandTtlExceeded { operation_id });
    }
    Ok(())
}

fn validate_segment_admission(operation: &PendingSmsOperation) -> Result<(), SmsTransportError> {
    if operation.operation_id().is_empty() || operation.command.segment_count == 0 {
        return Err(SmsTransportError::InvalidSegmentAdmission {
            operation_id: operation.operation_id().to_string(),
        });
    }
    let (encoding, segments) = estimate_segments(&operation.command.body);
    if encoding != operation.command.encoding || segments != operation.command.segment_count {
        return Err(SmsTransportError::SegmentAdmissionMismatch {
            operation_id: operation.operation_id().to_string(),
        });
    }
    Ok(())
}

fn to_channel_error(error: SmsTransportError) -> ChannelError {
    match error {
        SmsTransportError::NotConnected
        | SmsTransportError::StaleGeneration { .. }
        | SmsTransportError::FutureGeneration { .. } => {
            ChannelError::NotConnected(error.to_string())
        }
        other => ChannelError::SendFailed(other.to_string()),
    }
}

/// Test witnesses still come from the real connect/acquire state machine, not
/// from a production constructor or a synthetic approval scope.
#[cfg(test)]
pub(crate) fn test_private_context(binding: &str) -> TelecomPublicContext {
    TelecomPublicContext::from_admitted_sms(&super::SmsInboundMeta {
        region: "US".into(),
        binding_ref: binding.into(),
        provider_account_id: "acct".into(),
        provider_message_id: "pm".into(),
        envelope_event_id: "event".into(),
        connection_generation: 1,
        service_address: "+15550000000".into(),
        peer_address: "+15550000001".into(),
        encoding: aleph_protocol::telecom_relay::SmsEncoding::Gsm7,
        segment_count: 1,
        created_at: Utc::now(),
        expires_at: Utc::now() + Duration::seconds(60),
    })
}

#[cfg(test)]
pub(crate) async fn test_private_lease(binding: &str) -> (Arc<SmsRelayTransport>, PrivateSmsLease) {
    let (transport, _) = test_private_transport(binding).await;
    let ctx = test_private_context(binding);
    let lease = transport
        .private_sms_lease(&ctx, "+15550000001")
        .await
        .unwrap();
    (transport, lease)
}

#[cfg(test)]
pub(crate) async fn test_private_transport(
    binding: &str,
) -> (Arc<SmsRelayTransport>, Arc<InMemorySmsPeer>) {
    let peer = Arc::new(InMemorySmsPeer::default());
    let config = SmsConfig {
        region: "US".into(),
        service_address: "+15550000000".into(),
        binding_ref: binding.into(),
        inbound_enabled: true,
        outbound_enabled: true,
        max_segments: 10,
        command_ttl_secs: 60,
    };
    let transport = Arc::new(SmsRelayTransport::new_with_config(
        binding,
        config,
        peer.clone(),
        tests::allow_all(),
    ));
    transport.connect().await.unwrap();
    (transport, peer)
}

#[cfg(test)]
#[derive(Default)]
pub struct InMemorySmsPeer {
    inner: std::sync::Mutex<InMemorySmsPeerState>,
}

#[cfg(test)]
#[derive(Default)]
struct InMemorySmsPeerState {
    connected_generation: Option<u64>,
    outcomes: std::collections::VecDeque<Result<TelecomOutcome, SmsTransportError>>,
    submissions: Vec<PendingSmsOperation>,
    generations: Vec<u64>,
}

#[cfg(test)]
impl InMemorySmsPeer {
    fn with_outcomes<I>(outcomes: I) -> Self
    where
        I: IntoIterator<Item = Result<TelecomOutcome, SmsTransportError>>,
    {
        Self {
            inner: std::sync::Mutex::new(InMemorySmsPeerState {
                outcomes: outcomes.into_iter().collect(),
                ..Default::default()
            }),
        }
    }

    pub(crate) fn submissions(&self) -> Vec<PendingSmsOperation> {
        self.inner.lock().unwrap().submissions.clone()
    }

    pub(crate) fn fail_heartbeat(&self) {
        self.inner.lock().unwrap().connected_generation = None;
    }

    pub(crate) fn generations(&self) -> Vec<u64> {
        self.inner.lock().unwrap().generations.clone()
    }
}

#[cfg(test)]
#[async_trait]
impl SmsRelayPeer for InMemorySmsPeer {
    async fn connect(&self, _binding_id: &str, generation: u64) -> Result<(), SmsTransportError> {
        self.inner.lock().unwrap().connected_generation = Some(generation);
        Ok(())
    }

    async fn heartbeat(&self, _binding_id: &str, generation: u64) -> Result<(), SmsTransportError> {
        if self.inner.lock().unwrap().connected_generation == Some(generation) {
            Ok(())
        } else {
            Err(SmsTransportError::NotConnected)
        }
    }

    async fn submit(
        &self,
        _binding_id: &str,
        generation: u64,
        operation: &PendingSmsOperation,
    ) -> Result<TelecomOutcome, SmsTransportError> {
        let mut state = self.inner.lock().unwrap();
        if state.connected_generation != Some(generation) {
            return Err(SmsTransportError::StaleGeneration {
                active: state.connected_generation.unwrap_or_default(),
                got: generation,
            });
        }
        state.submissions.push(operation.clone());
        state.generations.push(generation);
        state
            .outcomes
            .pop_front()
            .unwrap_or(Ok(TelecomOutcome::Accepted))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::channel::MessageId;
    use aleph_protocol::telecom_relay::{SmsCommand, SmsEncoding};
    use tokio::sync::Notify;

    struct AllowAll;

    #[derive(Default)]
    struct BlockingAuthorizer {
        started: Notify,
        release: Notify,
    }

    #[async_trait]
    impl SmsSendAuthorizer for BlockingAuthorizer {
        async fn authorize(
            &self,
            _operation: &PendingSmsOperation,
        ) -> Result<(), SmsTransportError> {
            self.started.notify_one();
            self.release.notified().await;
            Ok(())
        }
    }

    #[async_trait]
    impl SmsSendAuthorizer for AllowAll {
        async fn authorize(
            &self,
            _operation: &PendingSmsOperation,
        ) -> Result<(), SmsTransportError> {
            Ok(())
        }
    }

    pub(super) fn allow_all() -> Arc<dyn SmsSendAuthorizer> {
        Arc::new(AllowAll)
    }

    fn outcome_envelope(
        generation: u64,
        operation_id: &str,
        outcome: TelecomOutcome,
    ) -> TelecomEnvelope {
        TelecomEnvelope {
            protocol_version: aleph_protocol::telecom_relay::TELECOM_PROTOCOL_VERSION,
            binding_id: "binding".into(),
            connection_generation: generation,
            event_id: format!("event-{operation_id}"),
            kind: TelecomKind::Outcome(aleph_protocol::telecom_relay::OutcomeReport {
                provider_account_id: "account".into(),
                provider_event_id: format!("receipt-{operation_id}"),
                operation_id: operation_id.into(),
                provider_message_id: None,
                provider_call_id: None,
                outcome,
                error_code: None,
            }),
            created_at: Utc::now(),
            expires_at: Utc::now() + Duration::hours(1),
        }
    }

    fn test_operation(expires_at: DateTime<Utc>) -> PendingSmsOperation {
        PendingSmsOperation {
            peer_address: "+8613900000000".into(),
            command: SmsCommand {
                operation_id: "op-1".into(),
                approval_reference: Some("approved".into()),
                recipient_binding_id: "binding".into(),
                expires_at,
                body: "hello".into(),
                encoding: SmsEncoding::Gsm7,
                segment_count: 1,
            },
        }
    }

    fn test_config(outbound_enabled: bool, max_segments: u32, command_ttl_secs: u64) -> SmsConfig {
        SmsConfig {
            region: "CN".into(),
            service_address: "+8613800000000".into(),
            binding_ref: "binding".into(),
            inbound_enabled: true,
            outbound_enabled,
            max_segments,
            command_ttl_secs,
        }
    }

    #[derive(Default)]
    struct BlockingSmsPeer {
        connected_generation: std::sync::Mutex<Option<u64>>,
        writes: std::sync::Mutex<Vec<(u64, PendingSmsOperation)>>,
        started: Notify,
        release: Notify,
    }

    #[async_trait]
    impl SmsRelayPeer for BlockingSmsPeer {
        async fn connect(
            &self,
            _binding_id: &str,
            generation: u64,
        ) -> Result<(), SmsTransportError> {
            *self.connected_generation.lock().unwrap() = Some(generation);
            Ok(())
        }

        async fn heartbeat(
            &self,
            _binding_id: &str,
            generation: u64,
        ) -> Result<(), SmsTransportError> {
            if *self.connected_generation.lock().unwrap() == Some(generation) {
                Ok(())
            } else {
                Err(SmsTransportError::NotConnected)
            }
        }

        async fn submit(
            &self,
            _binding_id: &str,
            generation: u64,
            operation: &PendingSmsOperation,
        ) -> Result<TelecomOutcome, SmsTransportError> {
            // Fake adapter fences at its write; the later wait is only ACK I/O.
            {
                let active = self.connected_generation.lock().unwrap();
                if *active != Some(generation) {
                    return Err(SmsTransportError::NotConnected);
                }
                self.writes
                    .lock()
                    .unwrap()
                    .push((generation, operation.clone()));
            }
            self.started.notify_one();
            self.release.notified().await;
            Ok(TelecomOutcome::Accepted)
        }
    }

    #[derive(Default)]
    struct HeartbeatBlockingPeer {
        heartbeat_started: Notify,
        heartbeat_release: Notify,
        fail: std::sync::atomic::AtomicBool,
    }

    #[async_trait]
    impl SmsRelayPeer for HeartbeatBlockingPeer {
        async fn connect(
            &self,
            _binding_id: &str,
            _generation: u64,
        ) -> Result<(), SmsTransportError> {
            Ok(())
        }

        async fn heartbeat(
            &self,
            _binding_id: &str,
            _generation: u64,
        ) -> Result<(), SmsTransportError> {
            self.heartbeat_started.notify_one();
            self.heartbeat_release.notified().await;
            if self.fail.load(std::sync::atomic::Ordering::SeqCst) {
                Err(SmsTransportError::NotConnected)
            } else {
                Ok(())
            }
        }

        async fn submit(
            &self,
            _binding_id: &str,
            _generation: u64,
            _operation: &PendingSmsOperation,
        ) -> Result<TelecomOutcome, SmsTransportError> {
            Ok(TelecomOutcome::Accepted)
        }
    }

    #[derive(Default)]
    struct ConnectBlockingPeer {
        connected_generation: std::sync::Mutex<Option<u64>>,
        block_next_connect: std::sync::atomic::AtomicBool,
        connect_error: std::sync::Mutex<Option<SmsTransportError>>,
        connect_started: Notify,
        connect_release: Notify,
    }

    impl ConnectBlockingPeer {
        fn block_next_connect(&self) {
            self.block_next_connect
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }

        fn fail_next_connect(&self, error: SmsTransportError) {
            *self.connect_error.lock().unwrap() = Some(error);
        }
    }

    #[async_trait]
    impl SmsRelayPeer for ConnectBlockingPeer {
        async fn connect(
            &self,
            _binding_id: &str,
            generation: u64,
        ) -> Result<(), SmsTransportError> {
            if generation > 1
                && self
                    .block_next_connect
                    .swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                self.connect_started.notify_one();
                self.connect_release.notified().await;
            }
            if let Some(error) = self.connect_error.lock().unwrap().take() {
                return Err(error);
            }
            *self.connected_generation.lock().unwrap() = Some(generation);
            Ok(())
        }

        async fn heartbeat(
            &self,
            _binding_id: &str,
            generation: u64,
        ) -> Result<(), SmsTransportError> {
            if *self.connected_generation.lock().unwrap() == Some(generation) {
                Ok(())
            } else {
                Err(SmsTransportError::NotConnected)
            }
        }

        async fn submit(
            &self,
            _binding_id: &str,
            generation: u64,
            _operation: &PendingSmsOperation,
        ) -> Result<TelecomOutcome, SmsTransportError> {
            if *self.connected_generation.lock().unwrap() == Some(generation) {
                Ok(TelecomOutcome::Accepted)
            } else {
                Err(SmsTransportError::NotConnected)
            }
        }
    }

    #[tokio::test]
    async fn reconnect_fences_old_lease_during_peer_connect() {
        let peer = Arc::new(ConnectBlockingPeer::default());
        let transport = Arc::new(SmsRelayTransport::new("binding", peer.clone(), allow_all()));
        let first = transport.connect().await.unwrap();
        let operation = test_operation(Utc::now() + Duration::seconds(60));
        peer.block_next_connect();
        let started = peer.connect_started.notified();
        let reconnect = {
            let transport = Arc::clone(&transport);
            tokio::spawn(async move { transport.connect().await })
        };
        started.await;

        assert_eq!(transport.lease().await, None);
        assert_eq!(
            transport.heartbeat(first.generation).await,
            Err(SmsTransportError::NotConnected)
        );
        assert_eq!(
            transport.submit_at(&operation, Utc::now()).await,
            Err(SmsTransportError::NotConnected)
        );
        let receipt = outcome_envelope(
            first.generation,
            operation.operation_id(),
            TelecomOutcome::Delivered,
        );
        assert_eq!(
            transport.record_outcome(&receipt, Utc::now()).await,
            Err(SmsTransportError::NotConnected)
        );

        peer.connect_release.notify_one();
        let second = reconnect.await.unwrap().unwrap();
        assert!(second.generation > first.generation);
        assert_eq!(transport.lease().await, Some(second));
    }

    #[tokio::test]
    async fn failed_reconnect_after_disconnect_cannot_restore_old_lease() {
        let peer = Arc::new(ConnectBlockingPeer::default());
        let transport = Arc::new(SmsRelayTransport::new("binding", peer.clone(), allow_all()));
        let first = transport.connect().await.unwrap();
        peer.block_next_connect();
        peer.fail_next_connect(SmsTransportError::Peer("connect failed".into()));
        let started = peer.connect_started.notified();
        let reconnect = {
            let transport = Arc::clone(&transport);
            tokio::spawn(async move { transport.connect().await })
        };
        started.await;
        transport.disconnect().await;
        peer.connect_release.notify_one();

        assert_eq!(
            reconnect.await.unwrap(),
            Err(SmsTransportError::Peer("connect failed".into()))
        );
        assert_eq!(transport.lease().await, None);
        assert_eq!(
            transport.heartbeat(first.generation).await,
            Err(SmsTransportError::NotConnected)
        );
    }

    #[tokio::test]
    async fn generation_fences_old_peer_events() {
        let peer = Arc::new(InMemorySmsPeer::default());
        let transport = SmsRelayTransport::new("binding", peer, allow_all());
        let first = transport.connect().await.unwrap();
        transport.heartbeat(first.generation).await.unwrap();
        transport.connect().await.unwrap();
        assert_eq!(first.generation, 1);
        assert!(matches!(
            transport.heartbeat(first.generation).await,
            Err(SmsTransportError::StaleGeneration { .. })
        ));
        let op = test_operation(Utc::now() + Duration::seconds(60));
        transport
            .submit_at(&op, Utc::now())
            .await
            .expect("current generation accepts the operation");
        let stale = outcome_envelope(
            first.generation,
            op.operation_id(),
            TelecomOutcome::Delivered,
        );
        assert!(matches!(
            transport.record_outcome(&stale, Utc::now()).await,
            Err(SmsTransportError::StaleGeneration { .. })
        ));
    }

    #[tokio::test]
    async fn disconnected_and_expired_operations_are_not_submitted() {
        let peer = Arc::new(InMemorySmsPeer::default());
        let transport = SmsRelayTransport::new("binding", peer.clone(), allow_all());
        let op = test_operation(Utc::now() - Duration::seconds(1));
        assert!(matches!(
            transport.submit_at(&op, Utc::now()).await,
            Err(SmsTransportError::NotConnected)
        ));
        transport.connect().await.unwrap();
        assert!(matches!(
            transport.submit_at(&op, Utc::now()).await,
            Err(SmsTransportError::Expired { .. })
        ));
        assert!(peer.submissions().is_empty());
    }

    #[tokio::test]
    async fn unknown_is_terminal_after_expiry_and_never_resubmits() {
        let peer = Arc::new(InMemorySmsPeer::with_outcomes([Ok(
            TelecomOutcome::Unknown,
        )]));
        let transport = SmsRelayTransport::new("binding", peer.clone(), allow_all());
        transport.connect().await.unwrap();
        let op = test_operation(Utc::now() + Duration::seconds(60));
        assert_eq!(
            transport.submit_at(&op, Utc::now()).await.unwrap(),
            TelecomOutcome::Unknown
        );
        let now = Utc::now() + Duration::hours(1);
        assert_eq!(
            transport
                .expire_unknown(op.operation_id(), now)
                .await
                .unwrap()
                .outcome,
            TelecomOutcome::OutcomeUnknownExpired
        );
        assert!(matches!(
            transport.submit_at(&op, now).await,
            Err(SmsTransportError::UnknownExpired { .. })
        ));
        assert_eq!(peer.submissions().len(), 1);
    }

    #[tokio::test]
    async fn outcomes_remain_distinct_and_duplicate_text_is_not_deduped() {
        let peer = Arc::new(InMemorySmsPeer::with_outcomes([
            Ok(TelecomOutcome::Accepted),
            Ok(TelecomOutcome::Submitted),
            Ok(TelecomOutcome::Unknown),
            Ok(TelecomOutcome::Delivered),
        ]));
        let transport = SmsRelayTransport::new("binding", peer.clone(), allow_all());
        transport.connect().await.unwrap();
        let first = test_operation(Utc::now() + Duration::seconds(60));
        let mut second = first.clone();
        second.command.operation_id = "op-2".into();
        let mut third = first.clone();
        third.command.operation_id = "op-3".into();
        let mut fourth = first.clone();
        fourth.command.operation_id = "op-4".into();
        assert_eq!(
            transport.submit_at(&first, Utc::now()).await.unwrap(),
            TelecomOutcome::Accepted
        );
        assert_eq!(
            transport.submit_at(&second, Utc::now()).await.unwrap(),
            TelecomOutcome::Submitted
        );
        assert_eq!(
            transport.submit_at(&third, Utc::now()).await.unwrap(),
            TelecomOutcome::Unknown
        );
        assert_eq!(
            transport.submit_at(&fourth, Utc::now()).await.unwrap(),
            TelecomOutcome::Delivered
        );
        assert_eq!(peer.submissions().len(), 4);
        assert_eq!(first.command.body, second.command.body);
        assert_eq!(third.command.body, fourth.command.body);
    }

    #[tokio::test]
    async fn admission_preserves_operation_encoding_and_segments() {
        let peer = Arc::new(InMemorySmsPeer::with_outcomes([Ok(
            TelecomOutcome::Accepted,
        )]));
        let transport = SmsRelayTransport::new("binding", peer.clone(), allow_all());
        transport.connect().await.unwrap();
        let op = test_operation(Utc::now() + Duration::seconds(60));
        transport.submit_at(&op, Utc::now()).await.unwrap();
        let sent = peer.submissions().pop().unwrap();
        assert_eq!(sent.command.encoding, op.command.encoding);
        assert_eq!(sent.command.segment_count, op.command.segment_count);
        assert_eq!(sent.command.body, op.command.body);
    }

    #[tokio::test]
    async fn manual_operations_are_rechecked_by_transport_policy() {
        let peer = Arc::new(InMemorySmsPeer::default());
        let transport = SmsRelayTransport::new_with_config(
            "binding",
            test_config(true, 1, 30),
            peer.clone(),
            allow_all(),
        );
        transport.connect().await.unwrap();
        let now = Utc::now();

        let mut too_many_segments = test_operation(now + Duration::seconds(10));
        too_many_segments.command.operation_id = "too-many-segments".into();
        too_many_segments.command.body = "a".repeat(161);
        too_many_segments.command.segment_count = 2;
        assert!(matches!(
            transport.submit_at(&too_many_segments, now).await,
            Err(SmsTransportError::SegmentLimitExceeded { .. })
        ));

        let mut invalid_peer = test_operation(now + Duration::seconds(10));
        invalid_peer.command.operation_id = "invalid-peer".into();
        invalid_peer.peer_address = "not-e164".into();
        assert!(matches!(
            transport.submit_at(&invalid_peer, now).await,
            Err(SmsTransportError::InvalidPeerAddress { .. })
        ));

        let disabled = SmsRelayTransport::new_with_config(
            "binding",
            test_config(false, 1, 30),
            peer.clone(),
            allow_all(),
        );
        disabled.connect().await.unwrap();
        let mut outbound_disabled = test_operation(now + Duration::seconds(10));
        outbound_disabled.command.operation_id = "outbound-disabled".into();
        assert!(matches!(
            disabled.submit_at(&outbound_disabled, now).await,
            Err(SmsTransportError::OutboundDisabled)
        ));

        let mut over_ttl = test_operation(now + Duration::seconds(31));
        over_ttl.command.operation_id = "over-ttl".into();
        assert!(matches!(
            transport.submit_at(&over_ttl, now).await,
            Err(SmsTransportError::CommandTtlExceeded { .. })
        ));
        assert!(peer.submissions().is_empty());
    }

    #[tokio::test]
    async fn disconnect_reconnect_during_authorization_never_submits_old_lease() {
        let peer = Arc::new(InMemorySmsPeer::default());
        let authorizer = Arc::new(BlockingAuthorizer::default());
        let transport = Arc::new(SmsRelayTransport::new(
            "binding",
            peer.clone(),
            authorizer.clone(),
        ));
        let first = transport.connect().await.unwrap();
        let operation = test_operation(Utc::now() + Duration::seconds(60));
        let submit_transport = Arc::clone(&transport);
        let submit_operation = operation.clone();
        let submit = tokio::spawn(async move {
            submit_transport
                .submit_at(&submit_operation, Utc::now())
                .await
        });
        authorizer.started.notified().await;
        transport.disconnect().await;
        let second = transport.connect().await.unwrap();
        assert!(second.generation > first.generation);
        authorizer.release.notify_one();
        assert!(matches!(
            submit.await.unwrap(),
            Err(SmsTransportError::StaleGeneration { .. })
        ));
        assert!(peer.submissions().is_empty());
        assert!(transport
            .operation(operation.operation_id())
            .await
            .is_none());
    }

    #[tokio::test]
    async fn same_operation_id_with_changed_request_is_a_conflict() {
        let peer = Arc::new(InMemorySmsPeer::default());
        let transport = SmsRelayTransport::new("binding", peer, allow_all());
        transport.connect().await.unwrap();
        let now = Utc::now();
        let first = test_operation(now + Duration::seconds(60));
        transport.submit_at(&first, now).await.unwrap();
        let mut changed = first.clone();
        changed.command.body = "changed".into();
        changed.command.segment_count = 1;
        assert_eq!(
            transport.submit_at(&changed, now).await,
            Err(SmsTransportError::OperationConflict {
                operation_id: first.operation_id().into()
            })
        );
    }

    #[tokio::test]
    async fn delivered_receipt_before_peer_result_cannot_be_regressed() {
        let peer = Arc::new(BlockingSmsPeer::default());
        let transport = Arc::new(SmsRelayTransport::new("binding", peer.clone(), allow_all()));
        let lease = transport.connect().await.unwrap();
        let operation = test_operation(Utc::now() + Duration::seconds(60));
        let submit_transport = Arc::clone(&transport);
        let submit_operation = operation.clone();
        let submit = tokio::spawn(async move {
            submit_transport
                .submit_at(&submit_operation, Utc::now())
                .await
        });
        peer.started.notified().await;
        let receipt = outcome_envelope(
            lease.generation,
            operation.operation_id(),
            TelecomOutcome::Delivered,
        );
        assert!(matches!(
            transport.record_outcome(&receipt, Utc::now()).await,
            Ok(OutcomeTransition::Applied {
                to: TelecomOutcome::Delivered,
                ..
            })
        ));
        peer.release.notify_one();
        assert_eq!(submit.await.unwrap().unwrap(), TelecomOutcome::Accepted);
        assert_eq!(
            transport
                .outcome(operation.operation_id())
                .await
                .unwrap()
                .outcome,
            TelecomOutcome::Delivered
        );
    }

    #[tokio::test]
    async fn invalid_config_fails_closed_without_widening_segment_policy() {
        let peer = Arc::new(InMemorySmsPeer::default());
        for max_segments in [0, MAX_SEGMENTS_LIMIT + 1] {
            let transport = SmsRelayTransport::new_with_config(
                "binding",
                test_config(true, max_segments, 30),
                peer.clone(),
                allow_all(),
            );
            transport.connect().await.unwrap();
            let operation = test_operation(Utc::now() + Duration::seconds(10));
            assert!(matches!(
                transport.submit_at(&operation, Utc::now()).await,
                Err(SmsTransportError::InvalidConfiguration(_))
            ));
        }

        let mismatched_binding = SmsRelayTransport::new_with_config(
            "different-binding",
            test_config(true, 1, 30),
            peer,
            allow_all(),
        );
        mismatched_binding.connect().await.unwrap();
        assert!(matches!(
            mismatched_binding
                .submit_at(
                    &test_operation(Utc::now() + Duration::seconds(10)),
                    Utc::now()
                )
                .await,
            Err(SmsTransportError::InvalidConfiguration(_))
        ));
    }

    #[tokio::test]
    async fn prune_unknown_at_retention_boundary_creates_tombstone() {
        let peer = Arc::new(InMemorySmsPeer::with_outcomes([
            Ok(TelecomOutcome::Unknown),
            Ok(TelecomOutcome::Accepted),
        ]));
        let policy = SmsTransportPolicy::default()
            .with_unknown_retention_secs(5)
            .unwrap();
        let transport =
            SmsRelayTransport::new_with_policy("binding", policy, peer.clone(), allow_all());
        transport.connect().await.unwrap();
        let base = Utc::now();
        let operation = test_operation(base + Duration::seconds(60));
        assert_eq!(
            transport.submit_at(&operation, base).await.unwrap(),
            TelecomOutcome::Unknown
        );

        let mut trigger = operation.clone();
        trigger.command.operation_id = "trigger-prune".into();
        assert_eq!(
            transport
                .submit_at(&trigger, base + Duration::seconds(5))
                .await
                .unwrap(),
            TelecomOutcome::Accepted
        );
        assert_eq!(
            transport
                .outcome(operation.operation_id())
                .await
                .unwrap()
                .outcome,
            TelecomOutcome::OutcomeUnknownExpired
        );
        assert_eq!(
            transport
                .submit_at(&operation, base + Duration::seconds(5))
                .await,
            Err(SmsTransportError::UnknownExpired {
                operation_id: operation.operation_id().into(),
            })
        );
        assert_eq!(peer.submissions().len(), 2);
    }

    #[tokio::test]
    async fn custom_unknown_retention_controls_expiry() {
        let peer = Arc::new(InMemorySmsPeer::with_outcomes([Ok(
            TelecomOutcome::Unknown,
        )]));
        let policy = SmsTransportPolicy::default()
            .with_unknown_retention_secs(5)
            .unwrap();
        let transport = SmsRelayTransport::new_with_policy("binding", policy, peer, allow_all());
        transport.connect().await.unwrap();
        let now = Utc::now();
        let operation = test_operation(now + Duration::seconds(60));
        transport.submit_at(&operation, now).await.unwrap();
        assert!(matches!(
            transport
                .expire_unknown(operation.operation_id(), now + Duration::seconds(4))
                .await,
            Err(SmsTransportError::Unknown { .. })
        ));
        assert_eq!(
            transport
                .expire_unknown(operation.operation_id(), now + Duration::seconds(5))
                .await
                .unwrap()
                .outcome,
            TelecomOutcome::OutcomeUnknownExpired
        );
    }

    #[tokio::test]
    async fn expiry_paths_share_terminal_horizon_and_allow_new_operation_only_after_gc() {
        // Cover automatic GC, first explicit expiry, and repeated explicit
        // expiry just before, at, and after the same terminal deadline.
        for path in ["automatic", "explicit", "repeated"] {
            for offset in [-1, 0, 1] {
                let peer = Arc::new(InMemorySmsPeer::with_outcomes([Ok(
                    TelecomOutcome::Unknown,
                )]));
                let policy = SmsTransportPolicy::default()
                    .with_unknown_retention_secs(5)
                    .unwrap();
                let transport = SmsRelayTransport::new_with_policy(
                    "binding",
                    policy,
                    peer.clone(),
                    allow_all(),
                );
                transport.connect().await.unwrap();
                let base = Utc::now();
                let operation = test_operation(base + Duration::seconds(60));
                transport.submit_at(&operation, base).await.unwrap();
                let tombstone = OutcomeRecord {
                    outcome: TelecomOutcome::OutcomeUnknownExpired,
                    entered_at: base + Duration::seconds(5),
                };
                let terminal_deadline = tombstone.entered_at
                    + Duration::seconds(TERMINAL_OPERATION_RETENTION_SECS as i64);
                let now = terminal_deadline + Duration::nanoseconds(offset);

                if path == "repeated" {
                    assert_eq!(
                        transport
                            .expire_unknown(operation.operation_id(), tombstone.entered_at)
                            .await,
                        Ok(tombstone)
                    );
                }
                if path == "automatic" {
                    // Trigger ordinary pruning without touching the old id.
                    let mut trigger = test_operation(now + Duration::seconds(60));
                    trigger.command.operation_id = "trigger-prune".into();
                    transport.submit_at(&trigger, now).await.unwrap();
                } else if path == "explicit" {
                    let result = transport
                        .expire_unknown(operation.operation_id(), now)
                        .await;
                    assert_eq!(result, Ok(tombstone));
                } else {
                    let result = transport
                        .expire_unknown(operation.operation_id(), now)
                        .await;
                    let expected = if offset < 0 {
                        Err(SmsTransportError::AlreadySubmitted {
                            operation_id: operation.operation_id().into(),
                            outcome: tombstone.outcome,
                        })
                    } else {
                        Err(SmsTransportError::OperationNotFound {
                            operation_id: operation.operation_id().into(),
                        })
                    };
                    assert_eq!(result, expected, "path={path}, offset={offset}");
                }

                let submissions = peer.submissions().len();
                if offset < 0 {
                    assert_eq!(
                        transport.outcome(operation.operation_id()).await,
                        Some(tombstone)
                    );
                    assert_eq!(
                        transport.submit_at(&operation, now).await,
                        Err(SmsTransportError::UnknownExpired {
                            operation_id: operation.operation_id().into(),
                        })
                    );
                    assert_eq!(peer.submissions().len(), submissions);
                } else {
                    assert!(transport
                        .operation(operation.operation_id())
                        .await
                        .is_none());
                    assert!(transport.outcome(operation.operation_id()).await.is_none());
                    assert_eq!(
                        transport
                            .expire_unknown(operation.operation_id(), now)
                            .await,
                        Err(SmsTransportError::OperationNotFound {
                            operation_id: operation.operation_id().into(),
                        })
                    );
                    // GC does not revive the original expired command. A new
                    // request with a fresh deadline may reuse the cleared id.
                    let mut replacement = operation.clone();
                    replacement.command.expires_at = now + Duration::seconds(60);
                    assert_eq!(
                        transport.submit_at(&replacement, now).await.unwrap(),
                        TelecomOutcome::Accepted
                    );
                    assert_eq!(peer.submissions().len(), submissions + 1);
                }
            }
        }
    }

    #[tokio::test]
    async fn terminal_operation_pruning_is_bounded_and_deterministic() {
        let peer = Arc::new(InMemorySmsPeer::default());
        let transport = SmsRelayTransport::new("binding", peer, allow_all());
        transport.connect().await.unwrap();
        let base = Utc::now();
        let old = test_operation(base + Duration::seconds(60));
        transport.submit_at(&old, base).await.unwrap();
        assert!(transport.operation(old.operation_id()).await.is_some());
        let mut fresh = test_operation(base + Duration::days(8) + Duration::seconds(60));
        fresh.command.operation_id = "fresh-after-prune".into();
        transport
            .submit_at(&fresh, base + Duration::days(8))
            .await
            .unwrap();
        assert!(transport.operation(old.operation_id()).await.is_none());
        assert!(transport.operation(fresh.operation_id()).await.is_some());
    }

    #[tokio::test]
    async fn pending_peer_does_not_block_heartbeat_or_reconnect() {
        let peer = Arc::new(BlockingSmsPeer::default());
        let transport = Arc::new(SmsRelayTransport::new("binding", peer.clone(), allow_all()));
        let first = transport.connect().await.unwrap();
        let operation = test_operation(Utc::now() + Duration::seconds(60));
        let started = peer.started.notified();
        let submit_transport = Arc::clone(&transport);
        let submit =
            tokio::spawn(async move { submit_transport.submit_at(&operation, Utc::now()).await });
        started.await;
        tokio::time::timeout(
            std::time::Duration::from_millis(100),
            transport.heartbeat(first.generation),
        )
        .await
        .expect("heartbeat is not held behind peer submit")
        .unwrap();
        tokio::time::timeout(std::time::Duration::from_millis(100), transport.connect())
            .await
            .expect("reconnect is not held behind peer submit")
            .unwrap();
        peer.release.notify_one();
        assert_eq!(submit.await.unwrap().unwrap(), TelecomOutcome::Accepted);
    }

    #[tokio::test]
    async fn late_successful_heartbeat_is_fenced_after_disconnect_and_reconnect() {
        let peer = Arc::new(HeartbeatBlockingPeer::default());
        let transport = Arc::new(SmsRelayTransport::new("binding", peer.clone(), allow_all()));
        let first = transport.connect().await.unwrap();
        let started = peer.heartbeat_started.notified();
        let heartbeat = {
            let transport = Arc::clone(&transport);
            tokio::spawn(async move { transport.heartbeat(first.generation).await })
        };
        started.await;
        transport.disconnect().await;
        peer.heartbeat_release.notify_one();
        assert_eq!(
            heartbeat.await.unwrap(),
            Err(SmsTransportError::NotConnected)
        );

        let peer = Arc::new(HeartbeatBlockingPeer::default());
        let transport = Arc::new(SmsRelayTransport::new("binding", peer.clone(), allow_all()));
        let first = transport.connect().await.unwrap();
        let started = peer.heartbeat_started.notified();
        let heartbeat = {
            let transport = Arc::clone(&transport);
            tokio::spawn(async move { transport.heartbeat(first.generation).await })
        };
        started.await;
        transport.disconnect().await;
        let second = transport.connect().await.unwrap();
        assert!(second.generation > first.generation);
        peer.heartbeat_release.notify_one();
        assert_eq!(
            heartbeat.await.unwrap(),
            Err(SmsTransportError::StaleGeneration {
                active: second.generation,
                got: first.generation,
            })
        );
        assert_eq!(transport.lease().await, Some(second));
    }

    #[tokio::test]
    async fn late_successful_heartbeat_requires_the_same_binding() {
        let peer = Arc::new(HeartbeatBlockingPeer::default());
        let transport = Arc::new(SmsRelayTransport::new("binding", peer.clone(), allow_all()));
        let first = transport.connect().await.unwrap();
        let started = peer.heartbeat_started.notified();
        let heartbeat = {
            let transport = Arc::clone(&transport);
            tokio::spawn(async move { transport.heartbeat(first.generation).await })
        };
        started.await;
        // Binding is immutable through the public API; inject a same-generation
        // replacement here to exercise both components of the final fence.
        let replacement = RelayLease {
            binding_id: "other-binding".into(),
            generation: first.generation,
        };
        transport.state.lock().await.active = Some(ActiveLease {
            relay: replacement.clone(),
            epoch: Arc::new(CancellationToken::new()),
        });
        peer.heartbeat_release.notify_one();
        assert_eq!(
            heartbeat.await.unwrap(),
            Err(SmsTransportError::BindingMismatch {
                expected: replacement.binding_id.clone(),
                claimed: "binding".into(),
            })
        );
        assert_eq!(transport.lease().await, Some(replacement));
    }

    #[tokio::test]
    async fn generation_overflow_fails_closed() {
        let peer = Arc::new(InMemorySmsPeer::default());
        let transport = SmsRelayTransport::new("binding", peer, allow_all());
        transport.state.lock().await.next_generation = u64::MAX;
        assert_eq!(
            transport.connect().await,
            Err(SmsTransportError::GenerationExhausted)
        );
    }

    fn private_config() -> SmsConfig {
        let mut config = test_config(true, 10, 60);
        config.region = "US".into();
        config.service_address = "+15550000000".into();
        config
    }

    fn private_operation() -> PendingSmsOperation {
        let mut operation = test_operation(Utc::now() + Duration::seconds(60));
        operation.peer_address = "+15550000001".into();
        operation
    }

    #[tokio::test]
    async fn private_witness_rejects_binding_region_caller_generation_and_other_transport() {
        let (transport, peer) = test_private_transport("binding").await;
        let ctx = test_private_context("binding");
        let witness = transport
            .private_sms_lease(&ctx, "+15550000001")
            .await
            .unwrap();
        assert!(transport
            .private_sms_lease(&ctx, "+15550000002")
            .await
            .is_none());
        assert!(transport
            .private_sms_lease(&test_private_context("other"), "+15550000001")
            .await
            .is_none());
        let mut meta = super::super::SmsInboundMeta {
            region: "CN".into(),
            binding_ref: "binding".into(),
            provider_account_id: "acct".into(),
            provider_message_id: "pm".into(),
            envelope_event_id: "ev".into(),
            connection_generation: 1,
            service_address: "+15550000000".into(),
            peer_address: "+15550000001".into(),
            encoding: aleph_protocol::telecom_relay::SmsEncoding::Gsm7,
            segment_count: 1,
            created_at: Utc::now(),
            expires_at: Utc::now(),
        };
        assert!(transport
            .private_sms_lease(
                &TelecomPublicContext::from_admitted_sms(&meta),
                "+15550000001"
            )
            .await
            .is_none());
        meta.region = "US".into();
        meta.connection_generation = 2;
        assert!(transport
            .private_sms_lease(
                &TelecomPublicContext::from_admitted_sms(&meta),
                "+15550000001"
            )
            .await
            .is_none());
        meta.connection_generation = 1;
        meta.peer_address = "+15550000002".into();
        assert!(transport
            .private_sms_lease(
                &TelecomPublicContext::from_admitted_sms(&meta),
                "+15550000001"
            )
            .await
            .is_none());
        let (_other, foreign) = test_private_lease("binding").await;
        assert_eq!(
            transport
                .submit_private_sms(
                    &private_operation(),
                    &foreign,
                    &CancellationToken::new(),
                    &|_| true
                )
                .await
                .unwrap(),
            ApprovedReplyOutcome::Invalid
        );
        assert!(peer.submissions().is_empty());
        transport.connect().await.unwrap();
        assert!(!witness.is_active());
        assert!(
            transport
                .private_sms_lease(&ctx, "+15550000001")
                .await
                .is_none(),
            "never refresh an old ctx into a new generation"
        );
        assert_eq!(
            transport
                .submit_private_sms(
                    &private_operation(),
                    &witness,
                    &CancellationToken::new(),
                    &|_| true
                )
                .await
                .unwrap(),
            ApprovedReplyOutcome::Invalid
        );
        assert!(peer.submissions().is_empty());
    }

    #[tokio::test]
    async fn private_authorizer_wait_rechecks_reconnect_cancel_body_and_expiry_without_writes() {
        use std::sync::atomic::{AtomicBool, Ordering};
        for invalidate in ["reconnect", "disconnect", "stop", "body", "expiry"] {
            let peer = Arc::new(InMemorySmsPeer::default());
            let authorizer = Arc::new(BlockingAuthorizer::default());
            let transport = Arc::new(SmsRelayTransport::new_with_config(
                "binding",
                private_config(),
                peer.clone(),
                authorizer.clone(),
            ));
            transport.connect().await.unwrap();
            let lease = transport
                .private_sms_lease(&test_private_context("binding"), "+15550000001")
                .await
                .unwrap();
            let mut operation = private_operation();
            if invalidate == "expiry" {
                operation.command.expires_at = Utc::now() + Duration::milliseconds(100);
            }
            let cancel = CancellationToken::new();
            let valid = AtomicBool::new(true);
            let validate = |body: &str| body == "hello" && valid.load(Ordering::SeqCst);
            let submit = transport.submit_private_sms(&operation, &lease, &cancel, &validate);
            let (result, ()) = tokio::join!(submit, async {
                authorizer.started.notified().await;
                match invalidate {
                    "reconnect" => {
                        transport.connect().await.unwrap();
                    }
                    "disconnect" => transport.disconnect().await,
                    "stop" => cancel.cancel(),
                    "body" => valid.store(false, Ordering::SeqCst),
                    "expiry" => tokio::time::sleep(std::time::Duration::from_millis(150)).await,
                    _ => unreachable!(),
                }
                authorizer.release.notify_one();
            });
            if invalidate == "expiry" {
                assert!(result.is_err());
            } else {
                assert!(matches!(
                    result.unwrap(),
                    ApprovedReplyOutcome::Invalid | ApprovedReplyOutcome::Cancelled
                ));
            }
            assert!(
                peer.submissions().is_empty(),
                "{invalidate}: no irreversible write"
            );
            assert!(transport
                .operation(operation.operation_id())
                .await
                .is_none());
        }
    }

    #[tokio::test]
    async fn private_callback_runs_only_after_last_state_lock_and_second_cancel_fence() {
        use std::future::{poll_fn, Future};
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use std::task::Poll;
        for invalidate in ["body", "cancel_in_callback"] {
            let peer = Arc::new(InMemorySmsPeer::default());
            let authorizer = Arc::new(BlockingAuthorizer::default());
            let transport = SmsRelayTransport::new_with_config(
                "binding",
                private_config(),
                peer.clone(),
                authorizer.clone(),
            );
            transport.connect().await.unwrap();
            let lease = transport
                .private_sms_lease(&test_private_context("binding"), "+15550000001")
                .await
                .unwrap();
            let operation = private_operation();
            let valid = AtomicBool::new(true);
            let callbacks = AtomicUsize::new(0);
            let cancel = CancellationToken::new();
            let validate = |body: &str| {
                callbacks.fetch_add(1, Ordering::SeqCst);
                if invalidate == "cancel_in_callback" {
                    cancel.cancel();
                }
                body == "hello" && valid.load(Ordering::SeqCst)
            };
            let submit = transport.submit_private_sms(&operation, &lease, &cancel, &validate);
            tokio::pin!(submit);
            poll_fn(|cx| {
                assert!(submit.as_mut().poll(cx).is_pending());
                Poll::Ready(())
            })
            .await;
            authorizer.started.notified().await;
            let state = transport.state.lock().await;
            authorizer.release.notify_one();
            poll_fn(|cx| {
                assert!(submit.as_mut().poll(cx).is_pending());
                Poll::Ready(())
            })
            .await;
            assert_eq!(
                callbacks.load(Ordering::SeqCst),
                0,
                "last async lock not acquired yet"
            );
            if invalidate == "body" {
                valid.store(false, Ordering::SeqCst);
            }
            drop(state);
            assert!(matches!(
                submit.await.unwrap(),
                ApprovedReplyOutcome::Invalid | ApprovedReplyOutcome::Cancelled
            ));
            assert_eq!(callbacks.load(Ordering::SeqCst), 1);
            assert!(peer.submissions().is_empty());
        }
    }

    #[tokio::test]
    async fn private_cancel_and_reconnect_after_commit_preserve_ack_and_original_generation() {
        let peer = Arc::new(BlockingSmsPeer::default());
        let transport = SmsRelayTransport::new_with_config(
            "binding",
            private_config(),
            peer.clone(),
            allow_all(),
        );
        transport.connect().await.unwrap();
        let lease = transport
            .private_sms_lease(&test_private_context("binding"), "+15550000001")
            .await
            .unwrap();
        let operation = private_operation();
        let cancel = CancellationToken::new();
        let (result, ()) = tokio::join!(
            transport.submit_private_sms(&operation, &lease, &cancel, &|_| true),
            async {
                peer.started.notified().await;
                cancel.cancel();
                transport.disconnect().await;
                assert_eq!(transport.connect().await.unwrap().generation, 2);
                assert!(!lease.is_active());
                peer.release.notify_one();
            }
        );
        assert_eq!(result.unwrap(), ApprovedReplyOutcome::Acknowledged);
        let writes = peer.writes.lock().unwrap();
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0], (1, operation));
    }

    #[tokio::test]
    async fn private_unknown_is_not_retried_or_replaced_with_new_lease() {
        let peer = Arc::new(InMemorySmsPeer::with_outcomes([Err(
            SmsTransportError::UnknownWriteOutcome {
                operation_id: "op-1".into(),
            },
        )]));
        let transport = SmsRelayTransport::new_with_config(
            "binding",
            private_config(),
            peer.clone(),
            allow_all(),
        );
        transport.connect().await.unwrap();
        let lease = transport
            .private_sms_lease(&test_private_context("binding"), "+15550000001")
            .await
            .unwrap();
        let operation = private_operation();
        let cancel = CancellationToken::new();
        assert!(transport
            .submit_private_sms(&operation, &lease, &cancel, &|_| true)
            .await
            .is_err());
        assert!(transport
            .submit_private_sms(&operation, &lease, &cancel, &|_| true)
            .await
            .is_err());
        assert_eq!(peer.submissions(), vec![operation.clone()]);
        assert_eq!(peer.generations(), vec![1]);
        assert_eq!(
            transport
                .outcome(operation.operation_id())
                .await
                .unwrap()
                .outcome,
            TelecomOutcome::Unknown
        );
    }

    #[tokio::test]
    async fn late_old_heartbeat_failure_never_revokes_replacement_epoch() {
        let peer = Arc::new(HeartbeatBlockingPeer::default());
        peer.fail.store(true, std::sync::atomic::Ordering::SeqCst);
        let transport = SmsRelayTransport::new_with_config(
            "binding",
            private_config(),
            peer.clone(),
            allow_all(),
        );
        transport.connect().await.unwrap();
        let first = transport
            .private_sms_lease(&test_private_context("binding"), "+15550000001")
            .await
            .unwrap();
        let (heartbeat, replacement) = tokio::join!(transport.heartbeat(1), async {
            peer.heartbeat_started.notified().await;
            let relay = transport.connect().await.unwrap();
            let epoch = transport
                .state
                .lock()
                .await
                .active
                .as_ref()
                .unwrap()
                .epoch
                .clone();
            assert!(!first.is_active());
            peer.heartbeat_release.notify_one();
            (relay, epoch)
        });
        assert!(heartbeat.is_err());
        assert_eq!(transport.lease().await, Some(replacement.0));
        assert!(
            !replacement.1.is_cancelled(),
            "old generation failure must not kill replacement"
        );
    }

    #[tokio::test]
    async fn private_epoch_revokes_on_reservation_disconnect_and_current_heartbeat_failure() {
        let (transport, peer) = test_private_transport("binding").await;
        let first = transport
            .private_sms_lease(&test_private_context("binding"), "+15550000001")
            .await
            .unwrap();
        peer.fail_heartbeat();
        assert!(transport.heartbeat(1).await.is_err());
        assert!(!first.is_active());
        assert!(transport.lease().await.is_none());
        // A connect reservation revokes before the peer connect resolves.
        let peer = Arc::new(ConnectBlockingPeer::default());
        let transport = SmsRelayTransport::new_with_config(
            "binding",
            private_config(),
            peer.clone(),
            allow_all(),
        );
        transport.connect().await.unwrap();
        let first = transport
            .private_sms_lease(&test_private_context("binding"), "+15550000001")
            .await
            .unwrap();
        peer.block_next_connect();
        let (second, ()) = tokio::join!(transport.connect(), async {
            peer.connect_started.notified().await;
            assert!(!first.is_active());
            assert!(transport.lease().await.is_none());
            peer.connect_release.notify_one();
        });
        assert_eq!(second.unwrap().generation, 2);
        transport.disconnect().await;
        assert!(transport.lease().await.is_none());
    }

    #[test]
    fn inbound_ack_is_explicitly_pending_relay_adapter() {
        assert_eq!(
            RelayAckState::for_ingest(SmsIngest::Committed(MessageId::new("m"))),
            RelayAckState::NetworkAckPending
        );
        assert_eq!(
            RelayAckState::for_ingest(SmsIngest::Expired),
            RelayAckState::NetworkAckPending
        );
        assert_eq!(
            RelayAckState::for_ingest(SmsIngest::Stale),
            RelayAckState::NotEligible
        );
    }
}
