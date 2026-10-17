//! Authenticated, injected Relay seam. No media, provider SDK, retry or task spawn.
//! The embedder authenticates the peer; wire claims only narrow that authority.

use std::sync::{Arc, Mutex, Weak};

use aleph_protocol::telecom_relay::TelecomEnvelope;
use tokio_util::sync::CancellationToken;

use super::{CallCommand, CallSession};
use crate::gateway::interfaces::sms::normalize_e164;
use crate::gateway::telecom::{public_context::CallerDigest, TelecomPublicContext};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VoiceRelayError {
    #[error("invalid voice binding policy")]
    InvalidPolicy,
    #[error("voice Relay authority revoked")]
    Revoked,
    #[error("voice call already active or admission pending")]
    Busy,
    #[error("voice Relay facts do not match authenticated scope")]
    Scope,
    #[error("voice Relay peer failed: {0}")]
    Peer(String),
    #[error("voice connection generation exhausted")]
    GenerationExhausted,
}

/// Trusted configuration, never extracted from a wire envelope.
#[derive(Clone)]
pub struct VoiceBindingPolicy {
    region: String,
    binding: String,
    account: String,
    service: String,
    caller: String,
}

impl VoiceBindingPolicy {
    pub fn new(
        region: String,
        binding: String,
        account: String,
        service: String,
        admitted_caller: String,
    ) -> Result<Self, VoiceRelayError> {
        if region.is_empty() || binding.is_empty() || account.is_empty() {
            return Err(VoiceRelayError::InvalidPolicy);
        }
        let service = normalize_e164(&service).map_err(|_| VoiceRelayError::InvalidPolicy)?;
        let caller =
            normalize_e164(&admitted_caller).map_err(|_| VoiceRelayError::InvalidPolicy)?;
        Ok(Self {
            region,
            binding,
            account,
            service,
            caller,
        })
    }

    pub fn binding(&self) -> &str {
        &self.binding
    }
}

/// Facts obtained by the peer from its authenticated provider/Relay plane,
/// NOT facts deserialized from a caller-supplied event body.
pub struct AuthenticatedVoiceFacts {
    pub binding: String,
    pub generation: u64,
    pub provider_account_id: String,
    pub phone_call_id: String,
    pub provider_call_id: String,
    pub service_address: String,
    pub peer_address: String,
}

/// Hangup is out-of-band authenticated control, scoped as strictly as data.
pub struct AuthenticatedVoiceHangup {
    pub binding: String,
    pub generation: u64,
    pub provider_account_id: String,
    pub phone_call_id: String,
    pub provider_call_id: String,
}

pub enum VoicePeerEvent {
    Envelope(TelecomEnvelope),
    Hangup(AuthenticatedVoiceHangup),
}

/// Owned futures must be cancellation-safe: dropping one stops its work.
/// `next_event` must not consume an event until it returns Ready. `submit`
/// MUST call lease.check(cancel) again after every internal await and directly
/// before external submission. Ok means frame acceptance, never queued/played.
/// An embedder must not detach work or automatically retry commands.
#[async_trait::async_trait]
pub trait VoiceRelayPeer: Send + Sync {
    async fn connect(&self, binding: &str, generation: u64) -> Result<(), VoiceRelayError>;
    async fn authenticate_call(
        &self,
        binding: &str,
        generation: u64,
        phone_call_id: &str,
        provider_call_id: &str,
    ) -> Result<AuthenticatedVoiceFacts, VoiceRelayError>;
    async fn next_event(&self, lease: &VoiceCallLease) -> Result<VoicePeerEvent, VoiceRelayError>;
    async fn submit(
        &self,
        lease: &VoiceCallLease,
        command: CallCommand,
        cancel: &CancellationToken,
    ) -> Result<(), VoiceRelayError>;
}

struct Connection {
    generation: u64,
    policy: VoiceBindingPolicy,
    revoked: CancellationToken,
    ready: bool,
}
struct CallEpoch {
    id: String,
    revoked: CancellationToken,
}
#[derive(Default)]
struct Authority {
    generation: u64,
    connection: Option<Connection>,
    call: Option<CallEpoch>,
}
impl Drop for Authority {
    fn drop(&mut self) {
        revoke_connection(self);
    }
}

/// Pinned opaque capability. No public constructor and no serde surface.
#[derive(Clone)]
pub struct VoiceCallLease {
    authority: Weak<Mutex<Authority>>,
    session: CallSession,
    connection_revoked: CancellationToken,
    call_revoked: CancellationToken,
}
impl VoiceCallLease {
    pub fn session(&self) -> &CallSession {
        &self.session
    }

    pub fn is_active(&self) -> bool {
        if self.connection_revoked.is_cancelled() || self.call_revoked.is_cancelled() {
            return false;
        }
        let Some(authority) = self.authority.upgrade() else {
            return false;
        };
        let state = authority.lock().unwrap_or_else(|e| e.into_inner());
        state
            .connection
            .as_ref()
            .is_some_and(|c| c.ready && c.generation == self.session.connection_generation)
            && state
                .call
                .as_ref()
                .is_some_and(|c| c.id == self.session.phone_call_id)
    }

    pub fn check(&self, cancel: &CancellationToken) -> Result<(), VoiceRelayError> {
        if cancel.is_cancelled() || !self.is_active() {
            Err(VoiceRelayError::Revoked)
        } else {
            Ok(())
        }
    }

    pub async fn revoked(&self) {
        tokio::select! {
            _ = self.connection_revoked.cancelled() => {},
            _ = self.call_revoked.cancelled() => {},
        }
    }

    pub(crate) fn revoke(&self) {
        self.call_revoked.cancel();
        if let Some(authority) = self.authority.upgrade() {
            let mut state = authority.lock().unwrap_or_else(|e| e.into_inner());
            if state
                .call
                .as_ref()
                .is_some_and(|c| c.id == self.session.phone_call_id)
            {
                state.call = None;
            }
        }
    }
}

/// Pinned live connection proof, minted without requiring a physical call.
/// Private fields prevent raw metadata from reserving or authorizing effects.
#[derive(Clone)]
pub struct VoiceConnectionWitness {
    authority: Weak<Mutex<Authority>>,
    policy: VoiceBindingPolicy,
    generation: u64,
    revoked: CancellationToken,
    context: TelecomPublicContext,
}
impl VoiceConnectionWitness {
    pub fn context(&self) -> &TelecomPublicContext {
        &self.context
    }

    pub fn check_current(&self) -> Result<(), VoiceRelayError> {
        let authority = self.authority.upgrade().ok_or(VoiceRelayError::Revoked)?;
        let state = authority.lock().unwrap_or_else(|e| e.into_inner());
        if self.revoked.is_cancelled()
            || !state.connection.as_ref().is_some_and(|c| {
                c.ready
                    && c.generation == self.generation
                    && !c.revoked.is_cancelled()
                    && c.policy.binding == self.policy.binding
            })
        {
            return Err(VoiceRelayError::Revoked);
        }
        Ok(())
    }

    pub async fn revoked(&self) {
        self.revoked.cancelled().await;
    }

    /// Trusted route access for the injected peer, never a tool argument.
    pub fn recipient(&self) -> &str {
        &self.policy.caller
    }

    pub fn provider_account(&self) -> &str {
        &self.policy.account
    }

    pub(crate) fn matches_call(&self, call: &AdmittedVoiceCall) -> bool {
        Weak::ptr_eq(&self.authority, &call.lease.authority)
            && self.context.provenance() == call.context().provenance()
            && self.check_current().is_ok()
            && call.lease.is_active()
    }
}

/// Only this module can mint the proof. Public context accepts this typed
/// proof, never a CallSession, envelope or raw JSON as authority.
pub(crate) struct VoiceIngressProof {
    policy: VoiceBindingPolicy,
    generation: u64,
}
impl VoiceIngressProof {
    pub(crate) fn region(&self) -> &str {
        &self.policy.region
    }
    pub(crate) fn binding(&self) -> &str {
        &self.policy.binding
    }
    pub(crate) fn caller_digest(&self) -> CallerDigest {
        CallerDigest::from_normalized_e164(&self.policy.caller)
    }
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }
}

/// Unique owner of call authority. Dropping admission or its coordinator
/// synchronously revokes all cloned leases, including a blocked submit.
pub struct AdmittedVoiceCall {
    lease: VoiceCallLease,
    ctx: TelecomPublicContext,
}
impl AdmittedVoiceCall {
    pub fn lease(&self) -> &VoiceCallLease {
        &self.lease
    }
    pub fn context(&self) -> &TelecomPublicContext {
        &self.ctx
    }
}
impl Drop for AdmittedVoiceCall {
    fn drop(&mut self) {
        self.lease.revoke();
    }
}

struct CallReservation(Option<VoiceCallLease>);
impl Drop for CallReservation {
    fn drop(&mut self) {
        if let Some(lease) = &self.0 {
            lease.revoke();
        }
    }
}

struct ConnectionReservation {
    authority: Arc<Mutex<Authority>>,
    generation: u64,
    committed: bool,
}
impl Drop for ConnectionReservation {
    fn drop(&mut self) {
        if !self.committed {
            let mut state = self.authority.lock().unwrap_or_else(|e| e.into_inner());
            if state
                .connection
                .as_ref()
                .is_some_and(|c| c.generation == self.generation)
            {
                revoke_connection(&mut state);
            }
        }
    }
}

fn revoke_connection(state: &mut Authority) {
    if let Some(c) = state.call.take() {
        c.revoked.cancel();
    }
    if let Some(c) = state.connection.take() {
        c.revoked.cancel();
    }
}

/// One current connection and one current call. Generation is allocated by
/// Core, monotonically, before peer I/O; no mutex spans arbitrary peer waits.
#[derive(Clone)]
pub struct VoiceRelayTransport {
    peer: Arc<dyn VoiceRelayPeer>,
    authority: Arc<Mutex<Authority>>,
}
impl VoiceRelayTransport {
    pub fn new(peer: Arc<dyn VoiceRelayPeer>) -> Self {
        Self {
            peer,
            authority: Arc::new(Mutex::new(Authority::default())),
        }
    }

    pub async fn connect(&self, policy: VoiceBindingPolicy) -> Result<u64, VoiceRelayError> {
        let (generation, revoked) = {
            let mut state = self.authority.lock().unwrap_or_else(|e| e.into_inner());
            revoke_connection(&mut state);
            state.generation = state
                .generation
                .checked_add(1)
                .ok_or(VoiceRelayError::GenerationExhausted)?;
            let generation = state.generation;
            let revoked = CancellationToken::new();
            state.connection = Some(Connection {
                generation,
                policy: policy.clone(),
                revoked: revoked.clone(),
                ready: false,
            });
            (generation, revoked)
        };
        let mut reservation = ConnectionReservation {
            authority: self.authority.clone(),
            generation,
            committed: false,
        };
        tokio::select! {
            biased;
            _ = revoked.cancelled() => return Err(VoiceRelayError::Revoked),
            result = self.peer.connect(&policy.binding, generation) => result?,
        }
        let mut state = self.authority.lock().unwrap_or_else(|e| e.into_inner());
        let connection = state
            .connection
            .as_mut()
            .filter(|c| c.generation == generation && !c.revoked.is_cancelled())
            .ok_or(VoiceRelayError::Revoked)?;
        connection.ready = true;
        reservation.committed = true;
        Ok(generation)
    }

    pub fn disconnect(&self) {
        revoke_connection(&mut self.authority.lock().unwrap_or_else(|e| e.into_inner()));
    }

    /// Reserve the whole binding from the same authenticated live connection
    /// before approval/dial. This does not fabricate an already-existing call.
    pub fn connection_witness(&self) -> Result<VoiceConnectionWitness, VoiceRelayError> {
        let state = self.authority.lock().unwrap_or_else(|e| e.into_inner());
        let c = state
            .connection
            .as_ref()
            .filter(|c| c.ready && !c.revoked.is_cancelled())
            .ok_or(VoiceRelayError::Revoked)?;
        let proof = VoiceIngressProof {
            policy: c.policy.clone(),
            generation: c.generation,
        };
        Ok(VoiceConnectionWitness {
            authority: Arc::downgrade(&self.authority),
            policy: c.policy.clone(),
            generation: c.generation,
            revoked: c.revoked.clone(),
            context: TelecomPublicContext::from_admitted_voice(&proof),
        })
    }

    pub async fn admit_call(
        &self,
        provider_call_id: &str,
    ) -> Result<AdmittedVoiceCall, VoiceRelayError> {
        if provider_call_id.is_empty() {
            return Err(VoiceRelayError::Scope);
        }
        let (policy, lease) = {
            let mut state = self.authority.lock().unwrap_or_else(|e| e.into_inner());
            if state.call.is_some() {
                return Err(VoiceRelayError::Busy);
            }
            let c = state
                .connection
                .as_ref()
                .filter(|c| c.ready && !c.revoked.is_cancelled())
                .ok_or(VoiceRelayError::Revoked)?;
            let policy = c.policy.clone();
            let session = CallSession {
                provider_account_id: policy.account.clone(),
                phone_call_id: uuid::Uuid::new_v4().to_string(),
                provider_call_id: provider_call_id.to_owned(),
                binding_id: policy.binding.clone(),
                connection_generation: c.generation,
            };
            let lease = VoiceCallLease {
                authority: Arc::downgrade(&self.authority),
                session,
                connection_revoked: c.revoked.clone(),
                call_revoked: CancellationToken::new(),
            };
            state.call = Some(CallEpoch {
                id: lease.session.phone_call_id.clone(),
                revoked: lease.call_revoked.clone(),
            });
            (policy, lease)
        };
        // The owner exists before authentication awaits, so dropping this
        // future or any error path cannot leave an admission reservation live.
        let mut reservation = CallReservation(Some(lease.clone()));
        let s = lease.session();
        let facts = tokio::select! {
            biased;
            _ = lease.revoked() => return Err(VoiceRelayError::Revoked),
            result = self.peer.authenticate_call(&s.binding_id, s.connection_generation, &s.phone_call_id, &s.provider_call_id) => result?,
        };
        if facts.binding != policy.binding
            || facts.generation != s.connection_generation
            || facts.provider_account_id != policy.account
            || facts.phone_call_id != s.phone_call_id
            || facts.provider_call_id != s.provider_call_id
            || normalize_e164(&facts.service_address).ok().as_ref() != Some(&policy.service)
            || normalize_e164(&facts.peer_address).ok().as_ref() != Some(&policy.caller)
        {
            return Err(VoiceRelayError::Scope);
        }
        lease.check(&CancellationToken::new())?;
        let proof = VoiceIngressProof {
            policy,
            generation: s.connection_generation,
        };
        let admitted = AdmittedVoiceCall {
            ctx: TelecomPublicContext::from_admitted_voice(&proof),
            lease,
        };
        // Transfer unique ownership only after authenticated facts and the
        // post-await epoch check succeeded. No authority comes from wire JSON.
        reservation.0 = None;
        Ok(admitted)
    }

    fn owns(&self, lease: &VoiceCallLease) -> bool {
        Weak::ptr_eq(&lease.authority, &Arc::downgrade(&self.authority))
    }

    pub async fn next_event(
        &self,
        lease: &VoiceCallLease,
    ) -> Result<VoicePeerEvent, VoiceRelayError> {
        if !self.owns(lease) || !lease.is_active() {
            return Err(VoiceRelayError::Revoked);
        }
        let event = tokio::select! {
            biased;
            _ = lease.revoked() => return Err(VoiceRelayError::Revoked),
            event = self.peer.next_event(lease) => event?,
        };
        lease.check(&CancellationToken::new())?;
        if let VoicePeerEvent::Hangup(h) = &event {
            let s = lease.session();
            if h.binding != s.binding_id
                || h.generation != s.connection_generation
                || h.provider_account_id != s.provider_account_id
                || h.phone_call_id != s.phone_call_id
                || h.provider_call_id != s.provider_call_id
            {
                return Err(VoiceRelayError::Scope);
            }
        }
        Ok(event)
    }

    pub async fn submit(
        &self,
        lease: &VoiceCallLease,
        command: CallCommand,
        cancel: &CancellationToken,
    ) -> Result<(), VoiceRelayError> {
        if !self.owns(lease) {
            return Err(VoiceRelayError::Scope);
        }
        let s = lease.session();
        let scoped = match &command {
            CallCommand::SendText(t) => {
                t.provider_account_id == s.provider_account_id
                    && t.provider_call_id == s.provider_call_id
                    && t.phone_call_id == s.phone_call_id
                    && t.role == aleph_protocol::telecom_relay::CallTurnRole::AssistantText
            }
            CallCommand::CancelOutput { phone_call_id, .. }
            | CallCommand::Hangup { phone_call_id } => phone_call_id == &s.phone_call_id,
        };
        if !scoped {
            return Err(VoiceRelayError::Scope);
        }
        tokio::select! {
            biased;
            _ = lease.revoked() => Err(VoiceRelayError::Revoked),
            _ = cancel.cancelled() => Err(VoiceRelayError::Revoked),
            result = async {
                // This check and the first peer poll share one poll: there is
                // no await or detached scheduler hop between them.
                lease.check(cancel)?;
                self.peer.submit(lease, command, cancel).await
            } => {
                result?;
                lease.check(cancel)
            },
        }
    }
}
