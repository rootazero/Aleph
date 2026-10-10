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
}

/// Errors produced before a transport can truthfully report a submission.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SmsTransportError {
    #[error("SMS relay lease is not connected")]
    NotConnected,
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
    active: Option<RelayLease>,
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
        let policy = SmsTransportPolicy::from(&config);
        let config_valid = config.validate().is_ok() && valid_policy(&policy);
        Self::new_internal(binding_id.into(), policy, config_valid, peer, authorizer)
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
        if let Err(error) = result {
            return Err(error);
        }
        if !still_reserved || state.next_generation != generation {
            // An explicit disconnect or a newer reservation invalidated this
            // attempt; never resurrect the old lease on a late peer response.
            return Err(SmsTransportError::NotConnected);
        }
        state.active = Some(lease.clone());
        Ok(lease)
    }

    pub async fn disconnect(&self) {
        let mut state = self.state.lock().await;
        state.active = None;
        state.connecting_generation = None;
    }

    pub async fn lease(&self) -> Option<RelayLease> {
        self.state.lock().await.active.clone()
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
        if let Err(error) = result {
            let mut state = self.state.lock().await;
            if state.active.as_ref().map(|active| active.generation) == Some(lease.generation) {
                state.active = None;
            }
            return Err(error);
        }
        Ok(())
    }

    /// Submit at a caller-provided time so expiry and boundary behavior are
    /// deterministic in tests and in a future scheduler.
    pub async fn submit_at(
        &self,
        operation: &PendingSmsOperation,
        now: DateTime<Utc>,
    ) -> Result<TelecomOutcome, SmsTransportError> {
        let operation_id = operation.operation_id().to_string();
        let lease = {
            let mut state = self.state.lock().await;
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

        if let Err(error) = self.authorizer.authorize(operation).await {
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
            let result = match final_lease {
                Ok(current) => {
                    let stored = state.operations.get_mut(&operation_id).ok_or_else(|| {
                        SmsTransportError::OperationNotFound {
                            operation_id: operation_id.clone(),
                        }
                    })?;
                    if stored.operation != *operation {
                        return Err(SmsTransportError::OperationConflict { operation_id });
                    }
                    if let Some(record) = stored.outcome {
                        return Err(match record.outcome {
                            TelecomOutcome::Unknown => SmsTransportError::Unknown { operation_id },
                            TelecomOutcome::OutcomeUnknownExpired => {
                                SmsTransportError::UnknownExpired { operation_id }
                            }
                            outcome => SmsTransportError::AlreadySubmitted {
                                operation_id,
                                outcome,
                            },
                        });
                    }
                    if stored.phase != OperationPhase::Authorizing {
                        return Err(SmsTransportError::OperationPending { operation_id });
                    }
                    stored.phase = OperationPhase::Submitting;
                    Ok(current)
                }
                Err(error) => Err(error),
            };
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
                stored.outcome = Some(expired);
                stored.phase = OperationPhase::Settled;
                prune_operations(&mut state, now, &self.policy, Some(operation_id));
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
    state.active.clone().ok_or(SmsTransportError::NotConnected)
}

fn prune_operations(
    state: &mut TransportState,
    now: DateTime<Utc>,
    policy: &SmsTransportPolicy,
    keep_operation_id: Option<&str>,
) {
    state.operations.retain(|operation_id, stored| {
        if keep_operation_id == Some(operation_id.as_str()) || stored.outcome.is_none() {
            return true;
        }
        let record = stored.outcome.expect("checked above");
        let horizon_secs = if record.outcome == TelecomOutcome::Unknown {
            policy.unknown_retention_secs
        } else {
            TERMINAL_OPERATION_RETENTION_SECS
        };
        now < record.entered_at + Duration::seconds(horizon_secs as i64)
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

#[cfg(test)]
#[derive(Default)]
pub struct InMemorySmsPeer {
    inner: std::sync::Mutex<InMemorySmsPeerState>,
}

#[cfg(test)]
struct InMemorySmsPeerState {
    connected_generation: Option<u64>,
    outcomes: std::collections::VecDeque<Result<TelecomOutcome, SmsTransportError>>,
    submissions: Vec<PendingSmsOperation>,
}

#[cfg(test)]
impl Default for InMemorySmsPeerState {
    fn default() -> Self {
        Self {
            connected_generation: None,
            outcomes: std::collections::VecDeque::new(),
            submissions: Vec::new(),
        }
    }
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

    fn submissions(&self) -> Vec<PendingSmsOperation> {
        self.inner.lock().unwrap().submissions.clone()
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

    fn allow_all() -> Arc<dyn SmsSendAuthorizer> {
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
            _generation: u64,
            _operation: &PendingSmsOperation,
        ) -> Result<TelecomOutcome, SmsTransportError> {
            self.started.notify_one();
            self.release.notified().await;
            Ok(TelecomOutcome::Accepted)
        }
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
    async fn generation_overflow_fails_closed() {
        let peer = Arc::new(InMemorySmsPeer::default());
        let transport = SmsRelayTransport::new("binding", peer, allow_all());
        transport.state.lock().await.next_generation = u64::MAX;
        assert_eq!(
            transport.connect().await,
            Err(SmsTransportError::GenerationExhausted)
        );
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
