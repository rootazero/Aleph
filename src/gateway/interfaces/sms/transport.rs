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

use super::{estimate_segments, PendingSmsOperation, SmsChannel, SmsError, SmsIngest};

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
    #[error("SMS authorization capability is unavailable")]
    AuthorizationUnavailable,
    #[error("SMS authorization denied: {0}")]
    AuthorizationDenied(String),
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

/// The authenticated lease currently accepted by Core.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayLease {
    pub binding_id: String,
    pub generation: u64,
}

#[derive(Clone)]
struct StoredOperation {
    operation: PendingSmsOperation,
    outcome: Option<OutcomeRecord>,
}

struct TransportState {
    next_generation: u64,
    active: Option<RelayLease>,
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
        Self {
            binding_id: binding_id.into(),
            peer,
            authorizer,
            state: tokio::sync::Mutex::new(TransportState {
                next_generation: 0,
                active: None,
                operations: HashMap::new(),
            }),
        }
    }

    pub async fn connect(&self) -> Result<RelayLease, SmsTransportError> {
        let mut state = self.state.lock().await;
        let generation = state.next_generation.saturating_add(1);
        state.next_generation = generation;
        self.peer.connect(&self.binding_id, generation).await?;
        let lease = RelayLease {
            binding_id: self.binding_id.clone(),
            generation,
        };
        state.active = Some(lease.clone());
        Ok(lease)
    }

    pub async fn disconnect(&self) {
        self.state.lock().await.active = None;
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
        let mut state = self.state.lock().await;
        let lease = current_lease(&state)?;
        check_generation(lease.generation, generation)?;
        if let Err(error) = self
            .peer
            .heartbeat(&lease.binding_id, lease.generation)
            .await
        {
            state.active = None;
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
        let mut state = self.state.lock().await;
        let lease = current_lease(&state)?;
        if operation.command.recipient_binding_id != lease.binding_id {
            return Err(SmsTransportError::BindingMismatch {
                expected: lease.binding_id.clone(),
                claimed: operation.command.recipient_binding_id.clone(),
            });
        }
        if let Some(existing) = state.operations.get(operation.operation_id()) {
            if let Some(record) = existing.outcome {
                return match record.outcome {
                    TelecomOutcome::Unknown => Err(SmsTransportError::Unknown {
                        operation_id: operation.operation_id().to_string(),
                    }),
                    TelecomOutcome::OutcomeUnknownExpired => {
                        Err(SmsTransportError::UnknownExpired {
                            operation_id: operation.operation_id().to_string(),
                        })
                    }
                    outcome => Err(SmsTransportError::AlreadySubmitted {
                        operation_id: operation.operation_id().to_string(),
                        outcome,
                    }),
                };
            }
            return Err(SmsTransportError::UnknownWriteOutcome {
                operation_id: operation.operation_id().to_string(),
            });
        }
        if now >= operation.command.expires_at {
            return Err(SmsTransportError::Expired {
                operation_id: operation.operation_id().to_string(),
                expires_at: operation.command.expires_at,
            });
        }
        validate_segment_admission(operation)?;
        self.authorizer.authorize(operation).await?;
        state.operations.insert(
            operation.operation_id().to_string(),
            StoredOperation {
                operation: operation.clone(),
                outcome: None,
            },
        );
        let result = self
            .peer
            .submit(&lease.binding_id, lease.generation, operation)
            .await;
        match result {
            Ok(outcome) => {
                if outcome == TelecomOutcome::OutcomeUnknownExpired {
                    return Err(SmsTransportError::Peer(
                        "peer cannot report derived outcome_unknown_expired".into(),
                    ));
                }
                if let Some(stored) = state.operations.get_mut(operation.operation_id()) {
                    stored.outcome = Some(OutcomeRecord {
                        outcome,
                        entered_at: now,
                    });
                }
                Ok(outcome)
            }
            Err(SmsTransportError::UnknownWriteOutcome { .. }) => {
                if let Some(stored) = state.operations.get_mut(operation.operation_id()) {
                    stored.outcome = Some(OutcomeRecord {
                        outcome: TelecomOutcome::Unknown,
                        entered_at: now,
                    });
                }
                Err(SmsTransportError::Unknown {
                    operation_id: operation.operation_id().to_string(),
                })
            }
            Err(error) => {
                state.operations.remove(operation.operation_id());
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
        let stored = state.operations.get_mut(operation_id).ok_or_else(|| {
            SmsTransportError::OperationNotFound {
                operation_id: operation_id.to_string(),
            }
        })?;
        let record = stored.outcome.ok_or_else(|| SmsTransportError::Unknown {
            operation_id: operation_id.to_string(),
        })?;
        match expire_unknown(&record, now, Duration::hours(1)) {
            UnknownExpiry::Expired(expired) => {
                stored.outcome = Some(expired);
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
        }
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
        let lease = self.lease().await.ok_or(SmsTransportError::NotConnected)?;
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

    struct AllowAll;

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
