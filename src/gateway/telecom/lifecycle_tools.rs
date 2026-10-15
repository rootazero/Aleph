//! Owned, capability-scoped telephone lifecycle / 能力隔离、受控的电话生命周期.
//! No provider/network implementation lives here. The injected peer is a trusted
//! effect boundary; unknown outcomes are retained, never retried or persisted as
//! conversation history. One public permit spans approval, dialing and the driver.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex, Weak,
};
use std::time::Duration;

use serde::Serialize;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::lifecycle_bridge::{voice_control_pair, ControlCompletion, ControlHandle};
use super::public_admission::{
    PublicAdmission, PublicRunIdentity, PublicRunPermit, PublicRunRegistry,
};
use super::PublicHistoryStore;
use crate::approval::operator_requester::OperatorApprovalRequester;
use crate::gateway::interfaces::voice_call::relay::VoiceConnectionWitness;
use crate::gateway::interfaces::voice_call::{
    run_public_voice_call_controlled, AdmittedVoiceCall, PublicVoiceWiring, VoiceCallLease,
    VoiceRelayTransport,
};
use crate::orchestrator::dispatch::HarnessRunner;
use crate::sandbox::exec_approval::{gate::ApprovalOutcome, ApprovalAction};
use crate::tools::turn_context::TurnContext;

const MAX_RECORDS: usize = 256;
const APPROVAL_WINDOW: Duration = Duration::from_secs(60);
const CLEANUP_WINDOW: Duration = Duration::from_secs(30);
const RINGING_WINDOW: Duration = Duration::from_secs(60);

/// Never serde or publicly constructible. A role, JSON body, session owner or
/// ambient actor is not this capability. Its issuer is the very service used by
/// registry dispatch, and its run id was allocated by the authenticated server.
#[derive(Clone)]
pub struct TelecomToolAuthority {
    issuer: Weak<TelecomLifecycleService>,
    run_id: String,
    user: String,
    bindings: Arc<BTreeSet<String>>,
    revoked: CancellationToken,
}
impl std::fmt::Debug for TelecomToolAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TelecomToolAuthority([opaque])")
    }
}
impl TelecomToolAuthority {
    /// Revocation is restrictive and never extends lifetime or grants rights.
    pub fn revoke(&self) {
        self.revoked.cancel();
    }
    pub(crate) fn valid_for_run(&self, run_id: &str) -> bool {
        self.issuer
            .upgrade()
            .is_some_and(|service| service.valid_authority(self, run_id))
    }
}

/// Configuration owned by the server, not by tool input. The connected
/// transport's policy supplies the recipient and provider account.
pub struct LifecycleBinding {
    pub transport: VoiceRelayTransport,
}

pub struct LifecycleWiring {
    pub registry: Arc<PublicRunRegistry>,
    pub runner: Arc<dyn HarnessRunner>,
    pub history: Option<Arc<PublicHistoryStore>>,
    pub operator: Arc<OperatorApprovalRequester>,
    /// Trusted Core target identifier shown on the exact operator action.
    pub core_target: String,
}

/// Known is an authenticated provider call id, not an assertion of playback.
/// Failed means the peer KNOWS no effect occurred. Err/Unknown mean uncertain
/// effects and consume the fallback budget; neither may cause a redial.
pub enum LifecyclePeerOutcome {
    Known(String),
    Failed,
    Unknown,
}
#[derive(Debug, thiserror::Error)]
#[error("lifecycle peer unavailable or outcome uncertain")]
pub struct LifecyclePeerError;

/// Futures MUST be drop-safe, own all their work, and never detach or retry.
/// For initiate/answer, before the irreversible effect claim_effect() must
/// succeed exactly once; check() must also succeed after EVERY internal await
/// and immediately before the effect. A successful return is provider
/// acceptance, never heard audio. The server passes only a typed witness;
/// recipient/account are server routes. Cleanup uses the exact admitted lease
/// instead, because cancellation of the initiating witness must not prevent it.
#[async_trait::async_trait]
pub trait LifecyclePeer: Send + Sync {
    async fn initiate(
        &self,
        witness: &LifecycleEffectWitness,
    ) -> Result<LifecyclePeerOutcome, LifecyclePeerError>;
    async fn answer(
        &self,
        witness: &LifecycleEffectWitness,
    ) -> Result<LifecyclePeerOutcome, LifecyclePeerError>;
    /// One cleanup attempt for this exact authenticated admitted call. The
    /// initiating witness may already be cancelled; it must not gate cleanup.
    /// Ok means provider acceptance, not local lease revocation. Validate
    /// call.check() with an independent non-cancelled cleanup token after EVERY
    /// internal await and immediately before the remote effect. No retry,
    /// arbitrary call selector, or detached work is permitted.
    async fn hangup(&self, call: &VoiceCallLease) -> Result<(), LifecyclePeerError>;
}

/// Unforgeable proof of the exact owner, connection, admission and operation.
/// Cloning is intentionally unavailable: this proof describes one peer future.
pub struct LifecycleEffectWitness {
    issuer: Weak<TelecomLifecycleService>,
    authority: TelecomToolAuthority,
    connection: VoiceConnectionWitness,
    identity: PublicRunIdentity,
    operation_id: String,
    deadline: Instant,
    cancel: CancellationToken,
    call: Option<VoiceCallLease>,
    claimed: AtomicBool,
}
impl LifecycleEffectWitness {
    pub fn check(&self) -> Result<(), LifecyclePeerError> {
        let service = self.issuer.upgrade().ok_or(LifecyclePeerError)?;
        if self.cancel.is_cancelled()
            || Instant::now() >= self.deadline
            || !service.valid_authority(&self.authority, &self.authority.run_id)
            || !self
                .authority
                .bindings
                .contains(self.connection.context().provenance().binding_ref())
            || self.connection.check_current().is_err()
            || !self.identity.is_current()
            || !service.wiring.registry.recheck_spend()
            || self.call.as_ref().is_some_and(|c| !c.is_active())
        {
            return Err(LifecyclePeerError);
        }
        Ok(())
    }
    pub fn claim_effect(&self) -> Result<(), LifecyclePeerError> {
        self.check()?;
        if self.claimed.swap(true, Ordering::AcqRel) {
            return Err(LifecyclePeerError);
        }
        self.check()
    }
    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }
    pub fn recipient(&self) -> &str {
        self.connection.recipient()
    }
    pub fn provider_account(&self) -> &str {
        self.connection.provider_account()
    }
    pub fn call(&self) -> Option<&VoiceCallLease> {
        self.call.as_ref()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecyclePhase {
    AwaitingApproval,
    Dialing,
    Ringing,
    Answering,
    Running,
    Closed,
    Denied,
    Failed,
    Unknown,
}
#[derive(Clone, Debug, Serialize)]
pub struct LifecycleStatus {
    pub operation_id: String,
    pub call_id: String,
    pub phase: LifecyclePhase,
    pub output_epoch: Option<u64>,
    pub output_cancelled: bool,
    /// Remote control outcome; local Closed never proves remote acceptance.
    pub remote_outcome: Option<String>,
}

struct Incoming {
    admitted: AdmittedVoiceCall,
    permit: PublicRunPermit,
}
struct Entry {
    sequence: u64,
    owner: String,
    binding: String,
    status: LifecycleStatus,
    cancel: CancellationToken,
    done: watch::Sender<bool>,
    incoming: Option<Incoming>,
    control: Option<ControlHandle>,
    task: Option<JoinHandle<()>>,
    cleanup_task: Option<JoinHandle<()>>,
    // Owned by existing waiters only; eviction does not create a status cache.
    final_status: Arc<Mutex<Option<LifecycleStatus>>>,
}
impl Entry {
    fn publish_completion(&self) {
        *self.final_status.lock().unwrap_or_else(|e| e.into_inner()) =
            Some(TelecomLifecycleService::snapshot(self));
        self.done.send_replace(true);
    }
}
impl Drop for Entry {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(task) = &self.task {
            task.abort();
        }
        if let Some(task) = &self.cleanup_task {
            task.abort();
        }
    }
}

pub struct TelecomLifecycleService {
    allowed: BTreeMap<String, Arc<BTreeSet<String>>>,
    bindings: BTreeMap<String, LifecycleBinding>,
    peer: Arc<dyn LifecyclePeer>,
    wiring: LifecycleWiring,
    entries: Mutex<BTreeMap<String, Entry>>,
    next_sequence: AtomicU64,
}
impl TelecomLifecycleService {
    /// Maps are immutable after injection. The ledger retains at most 256
    /// records, evicting only completed terminal entries in insertion order.
    /// Evicted operations become unaddressable, never eligible for retry.
    pub fn new(
        allowed: BTreeMap<String, BTreeSet<String>>,
        bindings: BTreeMap<String, LifecycleBinding>,
        peer: Arc<dyn LifecyclePeer>,
        wiring: LifecycleWiring,
    ) -> Arc<Self> {
        Arc::new(Self {
            allowed: allowed.into_iter().map(|(k, v)| (k, Arc::new(v))).collect(),
            bindings,
            peer,
            wiring,
            entries: Mutex::new(BTreeMap::new()),
            next_sequence: AtomicU64::new(0),
        })
    }

    /// Only the authenticated builder/server may call this. Missing identities
    /// or unconfigured ownership records never mint a default-owner token.
    pub(crate) fn issue_for_authenticated_user(
        self: &Arc<Self>,
        user: &str,
        run_id: &str,
    ) -> Option<TelecomToolAuthority> {
        if user.is_empty() || run_id.is_empty() {
            return None;
        }
        let bindings = self.allowed.get(user)?.clone();
        Some(TelecomToolAuthority {
            issuer: Arc::downgrade(self),
            user: user.into(),
            run_id: run_id.into(),
            bindings,
            revoked: CancellationToken::new(),
        })
    }
    fn valid_authority(&self, authority: &TelecomToolAuthority, run_id: &str) -> bool {
        !authority.revoked.is_cancelled()
            && !run_id.is_empty()
            && authority.run_id == run_id
            && authority
                .issuer
                .upgrade()
                .is_some_and(|s| std::ptr::eq(self, Arc::as_ptr(&s)))
            && self
                .allowed
                .get(&authority.user)
                .is_some_and(|b| Arc::ptr_eq(b, &authority.bindings))
    }
    pub(crate) fn authority_for_turn(
        &self,
        turn: &TurnContext,
    ) -> Result<TelecomToolAuthority, &'static str> {
        turn.telecom_authority
            .as_ref()
            .filter(|a| self.valid_authority(a, &turn.run_id))
            .cloned()
            .ok_or("DENIED: missing or invalid telecom capability")
    }
    fn route(
        &self,
        authority: &TelecomToolAuthority,
        binding: &str,
    ) -> Result<&LifecycleBinding, &'static str> {
        if !authority.bindings.contains(binding) {
            return Err("DENIED: binding is not authorized");
        }
        self.bindings
            .get(binding)
            .ok_or("UNAVAILABLE: binding route is not configured")
    }
    fn insert(
        &self,
        authority: &TelecomToolAuthority,
        binding: &str,
        phase: LifecyclePhase,
        operation_id: String,
        cancel: CancellationToken,
    ) -> Result<watch::Sender<bool>, &'static str> {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if entries.contains_key(&operation_id) {
            return Err("DENIED: operation already exists");
        }
        let sequence = self
            .next_sequence
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
                next.checked_add(1)
            })
            .map_err(|_| "UNAVAILABLE: lifecycle sequence exhausted")?;
        if entries.len() >= MAX_RECORDS {
            let oldest = entries
                .iter()
                .filter(|(_, entry)| {
                    matches!(
                        entry.status.phase,
                        LifecyclePhase::Closed
                            | LifecyclePhase::Denied
                            | LifecyclePhase::Failed
                            | LifecyclePhase::Unknown
                    ) && *entry.done.borrow()
                })
                .min_by_key(|(_, entry)| entry.sequence)
                .map(|(id, _)| id.clone())
                .ok_or("UNAVAILABLE: lifecycle state capacity reached")?;
            entries.remove(&oldest);
        }
        let (done, _) = watch::channel(false);
        entries.insert(
            operation_id.clone(),
            Entry {
                sequence,
                owner: authority.user.clone(),
                binding: binding.into(),
                status: LifecycleStatus {
                    call_id: operation_id.clone(),
                    operation_id,
                    phase,
                    output_epoch: None,
                    output_cancelled: false,
                    remote_outcome: None,
                },
                cancel,
                done: done.clone(),
                incoming: None,
                control: None,
                task: None,
                cleanup_task: None,
                final_status: Arc::new(Mutex::new(None)),
            },
        );
        Ok(done)
    }
    fn set_phase(&self, operation: &str, phase: LifecyclePhase) {
        if let Some(e) = self
            .entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(operation)
        {
            e.status.phase = phase;
        }
    }
    fn status_of(&self, operation: &str) -> Result<LifecycleStatus, &'static str> {
        let entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries
            .get(operation)
            .map(Self::snapshot)
            .ok_or("DENIED: call is not owned by this capability")
    }
    fn status_for_waiter(
        &self,
        operation: &str,
        final_status: &Mutex<Option<LifecycleStatus>>,
    ) -> Result<LifecycleStatus, &'static str> {
        let entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries
            .get(operation)
            .map(Self::snapshot)
            .or_else(|| {
                final_status
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone()
            })
            .ok_or("DENIED: call is not owned by this capability")
    }
    fn complete(&self, operation: &str, phase: LifecyclePhase, remote: Option<ControlCompletion>) {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = entries.get_mut(operation) {
            entry.status.phase = phase;
            entry.status.remote_outcome = remote.map(|outcome| completion_name(outcome).into());
            entry.publish_completion();
        }
    }
    fn snapshot(entry: &Entry) -> LifecycleStatus {
        let mut status = entry.status.clone();
        if let Some(control) = &entry.control {
            let current = control.status();
            status.output_epoch = current.output_epoch;
            status.output_cancelled = current.output_cancelled;
        }
        status
    }
    fn own_entry<'a>(
        entries: &'a mut BTreeMap<String, Entry>,
        authority: &TelecomToolAuthority,
        call_id: &str,
    ) -> Result<&'a mut Entry, &'static str> {
        entries
            .values_mut()
            .find(|e| {
                e.owner == authority.user
                    && authority.bindings.contains(&e.binding)
                    && (e.status.call_id == call_id || e.status.operation_id == call_id)
            })
            .ok_or("DENIED: call is not owned by this capability")
    }

    async fn approval(&self, witness: &LifecycleEffectWitness, binding: &str, kind: &str) -> bool {
        if witness.check().is_err() {
            return false;
        }
        let authority = &witness.authority;
        // The operator sees the exact immutable route. Phone/private values
        // are never placed in builtin logs or returned by call_status.
        let mut action = ApprovalAction::bare(kind, "exact owned binding; single use; no retry");
        action.summary = serde_json::json!({ "tool": kind, "binding_ref": binding,
            "operation_id": witness.operation_id, "generation": witness.connection.context().provenance().connection_generation(),
            "recipient": witness.recipient(), "provider_account": witness.provider_account(),
            "author": authority.user, "run_id": authority.run_id, "core_target": self.wiring.core_target,
            "expires_in_ms": witness.deadline.saturating_duration_since(Instant::now()).as_millis() as u64,
            "public_budget": "current telecom-public registry cap" }).to_string();
        action.allowed_decisions = crate::exec::allowed_decisions::once_only();
        matches!(
            crate::tools::turn_context::with_originator(
                Some(authority.user.clone()),
                self.wiring.operator.request_operator_once(
                    &action,
                    &witness.cancel,
                    witness.deadline
                )
            )
            .await
            .outcome,
            ApprovalOutcome::Approved
        )
    }

    pub(crate) async fn initiate(
        self: &Arc<Self>,
        authority: TelecomToolAuthority,
        binding: String,
    ) -> Result<LifecycleStatus, &'static str> {
        let route = self.route(&authority, &binding)?;
        let operation = uuid::Uuid::new_v4().to_string();
        let connection = route
            .transport
            .connection_witness()
            .map_err(|_| "UNAVAILABLE: no live connection")?;
        if connection.context().provenance().binding_ref() != binding {
            return Err("DENIED: route scope mismatch");
        }
        // No physical call required. The public registry is occupied BEFORE
        // operator notification and peer I/O, including SMS on the same binding.
        let permit = match self.wiring.registry.try_admit(connection.context()) {
            PublicAdmission::Admitted(p) => p,
            PublicAdmission::OverSpend => return Err("DENIED: public budget exhausted"),
            _ => return Err("BUSY: public binding is occupied"),
        };
        let cancel = CancellationToken::new();
        self.insert(
            &authority,
            &binding,
            LifecyclePhase::AwaitingApproval,
            operation.clone(),
            cancel.clone(),
        )?;
        let final_status = self
            .entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&operation)
            .expect("pending operation retained")
            .final_status
            .clone();
        let mut owner = PendingOwner {
            service: Arc::downgrade(self),
            operation: operation.clone(),
            permit: Some(permit),
            uncertain: false,
            transferred: false,
        };
        let deadline = Instant::now() + APPROVAL_WINDOW;
        let identity = owner
            .permit
            .as_ref()
            .unwrap()
            .validated_identity(&self.wiring.registry, connection.context())
            .ok_or("DENIED: stale public reservation")?;
        let witness = LifecycleEffectWitness {
            issuer: Arc::downgrade(self),
            authority,
            connection,
            identity,
            operation_id: operation.clone(),
            deadline,
            cancel,
            call: None,
            claimed: AtomicBool::new(false),
        };
        let approved = tokio::select! {
            biased;
            _ = witness.cancel.cancelled() => false,
            _ = witness.authority.revoked.cancelled() => false,
            _ = witness.identity.cancel_token().cancelled_owned() => false,
            _ = witness.connection.revoked() => false,
            _ = tokio::time::sleep_until(deadline) => false,
            value = self.approval(&witness, &binding, "initiate_call") => value,
        };
        if !approved || witness.check().is_err() {
            self.set_phase(&operation, LifecyclePhase::Denied);
            return self.status_of(&operation);
        }
        self.set_phase(&operation, LifecyclePhase::Dialing);
        // Once the peer future is polled, cancellation/drop/Err is uncertain.
        owner.uncertain = true;
        let outcome = tokio::select! {
            biased;
            _ = witness.cancel.cancelled() => LifecyclePeerOutcome::Unknown,
            _ = witness.authority.revoked.cancelled() => LifecyclePeerOutcome::Unknown,
            _ = witness.identity.cancel_token().cancelled_owned() => LifecyclePeerOutcome::Unknown,
            _ = witness.connection.revoked() => LifecyclePeerOutcome::Unknown,
            _ = tokio::time::sleep_until(deadline) => LifecyclePeerOutcome::Unknown,
            result = self.peer.initiate(&witness) => result.unwrap_or(LifecyclePeerOutcome::Unknown),
        };
        let provider_id = match outcome {
            LifecyclePeerOutcome::Known(id)
                if !id.is_empty()
                    && witness.claimed.load(Ordering::Acquire)
                    && witness.check().is_ok() =>
            {
                id
            }
            LifecyclePeerOutcome::Failed => {
                owner.uncertain = false;
                self.set_phase(&operation, LifecyclePhase::Failed);
                return self.status_of(&operation);
            }
            _ => {
                self.set_phase(&operation, LifecyclePhase::Unknown);
                return self.status_of(&operation);
            }
        };
        let admitted = tokio::select! {
            biased;
            _ = witness.cancel.cancelled() => None,
            _ = witness.authority.revoked.cancelled() => None,
            _ = witness.identity.cancel_token().cancelled_owned() => None,
            _ = witness.connection.revoked() => None,
            _ = tokio::time::sleep_until(deadline) => None,
            result = route.transport.admit_call(&provider_id) => result.ok(),
        };
        let Some(admitted) = admitted else {
            self.set_phase(&operation, LifecyclePhase::Unknown);
            return self.status_of(&operation);
        };
        // Admission is an owned call even if cancellation won immediately
        // afterwards. Never discard it through a post-admission filter.
        let stop = witness.check().is_err() || !witness.connection.matches_call(&admitted);
        if let Some(mut done) = self.start_driver(
            &operation,
            admitted,
            &mut owner,
            route.transport.clone(),
            stop,
        ) {
            while !*done.borrow_and_update() {
                if done.changed().await.is_err() {
                    break;
                }
            }
        }
        self.status_for_waiter(&operation, &final_status)
    }

    /// Trusted incoming registration. The server supplies an already
    /// authenticated admission and owner; no tool input can mint a ringing call.
    pub fn register_incoming(
        self: &Arc<Self>,
        owner_user: &str,
        binding: &str,
        admitted: AdmittedVoiceCall,
    ) -> Result<LifecycleStatus, &'static str> {
        let authority = self
            .issue_for_authenticated_user(owner_user, &uuid::Uuid::new_v4().to_string())
            .ok_or("DENIED: incoming owner is not configured")?;
        let route = self.route(&authority, binding)?;
        let connection = route
            .transport
            .connection_witness()
            .map_err(|_| "UNAVAILABLE: no live connection")?;
        if !connection.matches_call(&admitted)
            || connection.context().provenance().binding_ref() != binding
        {
            return Err("DENIED: incoming scope mismatch");
        }
        let permit = match self.wiring.registry.try_admit(admitted.context()) {
            PublicAdmission::Admitted(p) => p,
            _ => return Err("BUSY: incoming public admission refused"),
        };
        let operation = uuid::Uuid::new_v4().to_string();
        self.insert(
            &authority,
            binding,
            LifecyclePhase::Ringing,
            operation.clone(),
            CancellationToken::new(),
        )?;
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let entry = entries.get_mut(&operation).unwrap();
        entry.status.call_id = admitted.lease().session().phone_call_id.clone();
        let lease = admitted.lease().clone();
        let permit_cancel = permit.cancel_token();
        let entry_cancel = entry.cancel.clone();
        entry.incoming = Some(Incoming { admitted, permit });
        let weak = Arc::downgrade(self);
        let ringing_deadline = Instant::now() + RINGING_WINDOW;
        entry.cleanup_task = Some(tokio::spawn(async move {
            tokio::select! {
                _ = lease.revoked() => {},
                _ = permit_cancel.cancelled() => {},
                _ = entry_cancel.cancelled() => {},
                _ = tokio::time::sleep_until(ringing_deadline) => {},
            }
            if let Some(service) = weak.upgrade() {
                let mut entries = service.entries.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(entry) = entries.get_mut(&operation) {
                    // Answer takes ownership under the same lock. A late
                    // watchdog must never close an answered call.
                    if let Some(incoming) = entry.incoming.take() {
                        drop(incoming);
                        entry.status.phase = LifecyclePhase::Closed;
                        entry.publish_completion();
                    }
                }
            }
        }));
        Ok(Self::snapshot(entry))
    }

    pub(crate) async fn answer(
        self: &Arc<Self>,
        authority: TelecomToolAuthority,
        call_id: String,
    ) -> Result<LifecycleStatus, &'static str> {
        let (operation, binding, cancel, incoming, final_status) = {
            let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
            let e = Self::own_entry(&mut entries, &authority, &call_id)?;
            if e.status.phase != LifecyclePhase::Ringing {
                return Err("DENIED: call is not ringing");
            }
            let incoming = e
                .incoming
                .take()
                .ok_or("DENIED: incoming call already claimed")?;
            if let Some(watchdog) = e.cleanup_task.take() {
                watchdog.abort();
            }
            e.status.phase = LifecyclePhase::AwaitingApproval;
            (
                e.status.operation_id.clone(),
                e.binding.clone(),
                e.cancel.clone(),
                incoming,
                e.final_status.clone(),
            )
        };
        let Incoming { admitted, permit } = incoming;
        let mut owner = PendingOwner {
            service: Arc::downgrade(self),
            operation: operation.clone(),
            permit: Some(permit),
            uncertain: false,
            transferred: false,
        };
        let route = self.route(&authority, &binding)?;
        let connection = route
            .transport
            .connection_witness()
            .map_err(|_| "UNAVAILABLE: no live connection")?;
        if !connection.matches_call(&admitted) {
            return Err("DENIED: stale incoming call");
        }
        let identity = owner
            .permit
            .as_ref()
            .unwrap()
            .validated_identity(&self.wiring.registry, admitted.context())
            .ok_or("DENIED: stale incoming permit")?;
        let deadline = Instant::now() + APPROVAL_WINDOW;
        let witness = LifecycleEffectWitness {
            issuer: Arc::downgrade(self),
            authority,
            connection,
            identity,
            operation_id: operation.clone(),
            deadline,
            cancel,
            call: Some(admitted.lease().clone()),
            claimed: AtomicBool::new(false),
        };
        let approved = tokio::select! {
            biased;
            _ = witness.cancel.cancelled() => false,
            _ = witness.authority.revoked.cancelled() => false,
            _ = witness.identity.cancel_token().cancelled_owned() => false,
            _ = witness.connection.revoked() => false,
            _ = admitted.lease().revoked() => false,
            _ = tokio::time::sleep_until(deadline) => false,
            value = self.approval(&witness, &binding, "answer_call") => value,
        };
        if !approved || witness.check().is_err() {
            self.set_phase(&operation, LifecyclePhase::Denied);
            return self.status_of(&operation);
        }
        self.set_phase(&operation, LifecyclePhase::Answering);
        owner.uncertain = true;
        let result = tokio::select! {
            biased;
            _ = witness.cancel.cancelled() => LifecyclePeerOutcome::Unknown,
            _ = witness.authority.revoked.cancelled() => LifecyclePeerOutcome::Unknown,
            _ = witness.identity.cancel_token().cancelled_owned() => LifecyclePeerOutcome::Unknown,
            _ = witness.connection.revoked() => LifecyclePeerOutcome::Unknown,
            _ = admitted.lease().revoked() => LifecyclePeerOutcome::Unknown,
            _ = tokio::time::sleep_until(deadline) => LifecyclePeerOutcome::Unknown,
            result = self.peer.answer(&witness) => result.unwrap_or(LifecyclePeerOutcome::Unknown),
        };
        match result {
            LifecyclePeerOutcome::Known(id)
                if id == admitted.lease().session().provider_call_id
                    && witness.claimed.load(Ordering::Acquire) =>
            {
                let stop = witness.check().is_err();
                if let Some(mut done) = self.start_driver(
                    &operation,
                    admitted,
                    &mut owner,
                    route.transport.clone(),
                    stop,
                ) {
                    while !*done.borrow_and_update() {
                        if done.changed().await.is_err() {
                            break;
                        }
                    }
                }
            }
            LifecyclePeerOutcome::Failed => {
                owner.uncertain = false;
                self.set_phase(&operation, LifecyclePhase::Failed);
            }
            _ => self.set_phase(&operation, LifecyclePhase::Unknown),
        }
        self.status_for_waiter(&operation, &final_status)
    }

    fn start_driver(
        self: &Arc<Self>,
        operation: &str,
        admitted: AdmittedVoiceCall,
        owner: &mut PendingOwner,
        transport: VoiceRelayTransport,
        stop: bool,
    ) -> Option<watch::Receiver<bool>> {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        // Pending entries are not eligible for retention eviction. Answer
        // claims incoming exactly once, and initiate allocates a fresh id.
        let entry = entries
            .get_mut(operation)
            .expect("pending operation retained");
        let cleanup = stop || entry.cancel.is_cancelled() || entry.task.is_some();
        let pair = if cleanup {
            None
        } else {
            voice_control_pair(
                &admitted,
                &self.wiring.registry,
                owner.permit.as_ref().unwrap(),
            )
            .ok()
        };
        let permit = owner.permit.take().unwrap();
        entry.status.call_id = admitted.lease().session().phone_call_id.clone();
        // Keep nonterminal until the unique task has dropped its permit.
        entry.status.phase = LifecyclePhase::Running;
        let done = entry.done.clone();
        let weak = Arc::downgrade(self);
        let operation = operation.to_string();
        let wait = if let Some((control, receiver)) = pair {
            entry.control = Some(control);
            let wiring = PublicVoiceWiring {
                registry: self.wiring.registry.clone(),
                harness: self.wiring.runner.clone(),
                history: self.wiring.history.clone(),
                transport,
            };
            entry.task = Some(tokio::spawn(async move {
                let report =
                    run_public_voice_call_controlled(admitted, wiring, permit, receiver).await;
                if let Some(service) = weak.upgrade() {
                    service.complete(&operation, LifecyclePhase::Closed, report.remote_hangup);
                }
            }));
            None
        } else {
            let peer = self.peer.clone();
            let cleanup = CleanupOwner {
                admitted: Some(admitted),
                permit: Some(permit),
                registry: self.wiring.registry.clone(),
                accepted: false,
            };
            let wait = done.subscribe();
            entry.cleanup_task = Some(tokio::spawn(async move {
                let mut cleanup = cleanup;
                let lease = cleanup.admitted.as_ref().unwrap().lease();
                let accepted = lease.check(&CancellationToken::new()).is_ok()
                    && matches!(
                        tokio::time::timeout(CLEANUP_WINDOW, peer.hangup(lease)).await,
                        Ok(Ok(()))
                    );
                cleanup.accepted = accepted;
                // The guard also seals unknown spend on abort/drop, while
                // still owning both the exact admission and its reservation.
                drop(cleanup);
                if let Some(service) = weak.upgrade() {
                    service.complete(
                        &operation,
                        if accepted {
                            LifecyclePhase::Closed
                        } else {
                            LifecyclePhase::Unknown
                        },
                        Some(if accepted {
                            ControlCompletion::Accepted
                        } else {
                            ControlCompletion::Unknown
                        }),
                    );
                }
            }));
            Some(wait)
        };
        // Installation and transfer are synchronous under the entry lock;
        // dropping the tool future cannot abandon the cleanup task/permit.
        owner.transferred = true;
        owner.uncertain = false;
        wait
    }

    pub(crate) async fn hangup(
        &self,
        authority: &TelecomToolAuthority,
        call_id: &str,
    ) -> Result<(LifecycleStatus, &'static str), &'static str> {
        let (operation, control, mut done, immediate, final_status) = {
            let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
            let e = Self::own_entry(&mut entries, authority, call_id)?;
            let mut immediate = false;
            if let Some(incoming) = e.incoming.take() {
                // No direct carrier bypass before answer: release/revoke the
                // ringing lease locally. Do not call this remote acceptance.
                drop(incoming);
                e.status.phase = LifecyclePhase::Closed;
                e.publish_completion();
                immediate = true;
            } else if e.control.is_none() {
                e.cancel.cancel();
            }
            (
                e.status.operation_id.clone(),
                e.control.clone(),
                e.done.subscribe(),
                immediate,
                e.final_status.clone(),
            )
        };
        let completion = if let Some(control) = control {
            control.hangup().completed().await
        } else {
            ControlCompletion::LocalApplied
        };
        if !immediate {
            while !*done.borrow_and_update() {
                if done.changed().await.is_err() {
                    break;
                }
            }
        }
        Ok((
            self.status_for_waiter(&operation, &final_status)?,
            completion_name(completion),
        ))
    }
    pub(crate) async fn cancel_output(
        &self,
        authority: &TelecomToolAuthority,
        call_id: &str,
        epoch: u64,
    ) -> Result<(LifecycleStatus, &'static str), &'static str> {
        let (operation, control, final_status) = {
            let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
            let e = Self::own_entry(&mut entries, authority, call_id)?;
            (
                e.status.operation_id.clone(),
                e.control
                    .clone()
                    .ok_or("DENIED: call has no active output")?,
                e.final_status.clone(),
            )
        };
        let completion = control.cancel_output(epoch).completed().await;
        Ok((
            self.status_for_waiter(&operation, &final_status)?,
            completion_name(completion),
        ))
    }
    pub(crate) fn status(
        &self,
        authority: &TelecomToolAuthority,
        call_id: Option<&str>,
    ) -> Result<Vec<LifecycleStatus>, &'static str> {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(id) = call_id {
            return Ok(vec![Self::snapshot(Self::own_entry(
                &mut entries,
                authority,
                id,
            )?)]);
        }
        Ok(entries
            .values()
            .filter(|e| e.owner == authority.user && authority.bindings.contains(&e.binding))
            .take(MAX_RECORDS)
            .map(Self::snapshot)
            .collect())
    }
}

fn completion_name(completion: ControlCompletion) -> &'static str {
    match completion {
        ControlCompletion::LocalApplied => "local_applied",
        ControlCompletion::Accepted => "accepted",
        ControlCompletion::Unknown => "unknown",
        ControlCompletion::Refused => "refused",
    }
}

/// Guard created before spawning cleanup, including abort before first poll.
/// Unknown cost is sealed before releasing the reservation on every exit.
struct CleanupOwner {
    admitted: Option<AdmittedVoiceCall>,
    permit: Option<PublicRunPermit>,
    registry: Arc<PublicRunRegistry>,
    accepted: bool,
}
impl Drop for CleanupOwner {
    fn drop(&mut self) {
        if !self.accepted {
            self.registry.exhaust_unaccounted_spend();
        }
        self.admitted.take();
        self.permit.take();
    }
}

/// Holds the reservation while tool/approval/peer futures are suspended. A
/// dropped future cannot leave a live permit, approval wait or delayed dial.
struct PendingOwner {
    service: Weak<TelecomLifecycleService>,
    operation: String,
    permit: Option<PublicRunPermit>,
    uncertain: bool,
    transferred: bool,
}
impl Drop for PendingOwner {
    fn drop(&mut self) {
        if self.transferred {
            return;
        }
        // Seal unknown cost while the reservation still prevents admission;
        // release it before publishing completion to off-switch waiters.
        if self.uncertain {
            if let Some(permit) = &self.permit {
                permit.exhaust_unaccounted_spend();
            }
        }
        self.permit.take();
        if let Some(service) = self.service.upgrade() {
            let mut entries = service.entries.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(e) = entries.get_mut(&self.operation) {
                e.cancel.cancel();
                if !matches!(
                    e.status.phase,
                    LifecyclePhase::Denied | LifecyclePhase::Failed
                ) {
                    e.status.phase = if self.uncertain {
                        LifecyclePhase::Unknown
                    } else {
                        LifecyclePhase::Denied
                    };
                }
                e.publish_completion();
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use crate::exec::manager::ExecApprovalManager;
    use crate::gateway::event_bus::GatewayEventBus;
    use std::sync::atomic::AtomicUsize;
    use tokio::sync::{mpsc, Notify};

    struct CleanupPeer {
        entered: mpsc::UnboundedSender<String>,
        gate: Notify,
        attempts: AtomicUsize,
        drops: Arc<AtomicUsize>,
        fail: bool,
    }
    struct CleanupDrop(Arc<AtomicUsize>);
    impl Drop for CleanupDrop {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::AcqRel);
        }
    }
    #[async_trait::async_trait]
    impl LifecyclePeer for CleanupPeer {
        async fn initiate(
            &self,
            witness: &LifecycleEffectWitness,
        ) -> Result<LifecyclePeerOutcome, LifecyclePeerError> {
            witness.claim_effect()?;
            Ok(LifecyclePeerOutcome::Known("provider-call".into()))
        }
        async fn answer(
            &self,
            witness: &LifecycleEffectWitness,
        ) -> Result<LifecyclePeerOutcome, LifecyclePeerError> {
            witness.claim_effect()?;
            Ok(LifecyclePeerOutcome::Known(
                witness.call().unwrap().session().provider_call_id.clone(),
            ))
        }
        async fn hangup(&self, call: &VoiceCallLease) -> Result<(), LifecyclePeerError> {
            let _guard = CleanupDrop(self.drops.clone());
            self.attempts.fetch_add(1, Ordering::AcqRel);
            call.check(&CancellationToken::new())
                .map_err(|_| LifecyclePeerError)?;
            self.entered
                .send(call.session().provider_call_id.clone())
                .unwrap();
            self.gate.notified().await;
            call.check(&CancellationToken::new())
                .map_err(|_| LifecyclePeerError)?;
            if self.fail {
                Err(LifecyclePeerError)
            } else {
                Ok(())
            }
        }
    }
    fn service(
        transport: VoiceRelayTransport,
        runner: Arc<dyn HarnessRunner>,
        fail: bool,
    ) -> (
        Arc<TelecomLifecycleService>,
        Arc<CleanupPeer>,
        mpsc::UnboundedReceiver<String>,
    ) {
        let (entered, receiver) = mpsc::unbounded_channel();
        let peer = Arc::new(CleanupPeer {
            entered,
            gate: Notify::new(),
            attempts: AtomicUsize::new(0),
            drops: Arc::new(AtomicUsize::new(0)),
            fail,
        });
        let service = TelecomLifecycleService::new(
            BTreeMap::from([("user".into(), BTreeSet::from(["bind".into()]))]),
            BTreeMap::from([("bind".into(), LifecycleBinding { transport })]),
            peer.clone(),
            LifecycleWiring {
                registry: PublicRunRegistry::new(4, 1000),
                runner,
                history: None,
                operator: Arc::new(OperatorApprovalRequester::for_config_tier(
                    Arc::new(ExecApprovalManager::new()),
                    Arc::new(GatewayEventBus::new()),
                )),
                core_target: "test-core".into(),
            },
        );
        (service, peer, receiver)
    }
    fn reserve(
        service: &Arc<TelecomLifecycleService>,
        transport: &VoiceRelayTransport,
    ) -> PublicRunPermit {
        match service
            .wiring
            .registry
            .try_admit(transport.connection_witness().unwrap().context())
        {
            PublicAdmission::Admitted(permit) => permit,
            _ => panic!("fixture reservation refused"),
        }
    }

    pub(crate) async fn aborted_cleanup_seals_spend(
        transport: VoiceRelayTransport,
        runner: Arc<dyn HarnessRunner>,
        before_first_poll: bool,
    ) {
        let (service, peer, mut entered) = service(transport.clone(), runner, false);
        let authority = service.issue_for_authenticated_user("user", "run").unwrap();
        service
            .insert(
                &authority,
                "bind",
                LifecyclePhase::Dialing,
                "operation".into(),
                CancellationToken::new(),
            )
            .unwrap();
        let registry = service.wiring.registry.clone();
        let mut owner = PendingOwner {
            service: Arc::downgrade(&service),
            operation: "operation".into(),
            permit: Some(reserve(&service, &transport)),
            uncertain: true,
            transferred: false,
        };
        let admitted = transport.admit_call("provider-call").await.unwrap();
        let lease = admitted.lease().clone();
        service.start_driver("operation", admitted, &mut owner, transport, true);
        drop(owner);
        if !before_first_poll {
            assert_eq!(entered.recv().await.as_deref(), Some("provider-call"));
            assert_eq!(registry.active_count(), 1);
        }
        drop(service);
        for _ in 0..20 {
            if registry.active_count() == 0 {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(registry.tokens_spent(), 1000);
        assert_eq!(registry.active_count(), 0);
        assert!(!lease.is_active());
        assert_eq!(
            peer.attempts.load(Ordering::Acquire),
            usize::from(!before_first_poll)
        );
    }

    // Invoked by relay_tests with its authenticated offline relay/runner rig.
    // Explicitly pause at the Known + admitted -> start_driver boundary; no
    // scheduler timing, approval UI, or arbitrary provider command is involved.
    pub(crate) async fn cancelled_admission_cleanup(
        transport: VoiceRelayTransport,
        runner: Arc<dyn HarnessRunner>,
        answering: bool,
        fail: bool,
    ) {
        let (service, peer, mut entered) = service(transport.clone(), runner, fail);
        let authority = service.issue_for_authenticated_user("user", "run").unwrap();
        let cancel = CancellationToken::new();
        let operation = "operation".to_string();
        service
            .insert(
                &authority,
                "bind",
                if answering {
                    LifecyclePhase::Answering
                } else {
                    LifecyclePhase::Dialing
                },
                operation.clone(),
                cancel.clone(),
            )
            .unwrap();
        let mut owner = PendingOwner {
            service: Arc::downgrade(&service),
            operation: operation.clone(),
            permit: Some(reserve(&service, &transport)),
            uncertain: true,
            transferred: false,
        };
        let connection = transport.connection_witness().unwrap();
        let identity = owner
            .permit
            .as_ref()
            .unwrap()
            .validated_identity(&service.wiring.registry, connection.context())
            .unwrap();
        let incoming = if answering {
            Some(transport.admit_call("provider-call").await.unwrap())
        } else {
            None
        };
        let witness = LifecycleEffectWitness {
            issuer: Arc::downgrade(&service),
            authority: authority.clone(),
            connection,
            identity,
            operation_id: operation.clone(),
            deadline: Instant::now() + APPROVAL_WINDOW,
            cancel,
            call: incoming.as_ref().map(|a| a.lease().clone()),
            claimed: AtomicBool::new(false),
        };
        let result = if answering {
            peer.answer(&witness).await
        } else {
            peer.initiate(&witness).await
        };
        let LifecyclePeerOutcome::Known(id) = result.unwrap() else {
            panic!("provider must accept");
        };
        let admitted = match incoming {
            Some(a) => a,
            None => transport.admit_call(&id).await.unwrap(),
        };
        let lease = admitted.lease().clone();
        // Poll hangup once: it cancels the pending entry and waits for its
        // owner. Installation must still retain the admitted call afterwards.
        let mut hangup = Box::pin(service.hangup(&authority, &operation));
        tokio::select! { biased;
            _ = &mut hangup => panic!("pending owner must finish first"),
            _ = std::future::ready(()) => {}
        }
        assert!(witness.cancel.is_cancelled());
        let mut done = service
            .start_driver(&operation, admitted, &mut owner, transport.clone(), false)
            .unwrap();
        drop(owner);
        assert_eq!(entered.recv().await.as_deref(), Some("provider-call"));
        assert_eq!(service.wiring.registry.active_count(), 1);
        assert!(lease.is_active());
        assert_eq!(peer.drops.load(Ordering::Acquire), 0);
        // Dropping BOTH tool waits must not drop the service-owned cleanup.
        drop(hangup);
        peer.gate.notify_one();
        while !*done.borrow_and_update() {
            done.changed().await.unwrap();
        }
        assert_eq!(peer.attempts.load(Ordering::Acquire), 1);
        assert_eq!(peer.drops.load(Ordering::Acquire), 1);
        assert!(!lease.is_active());
        assert_eq!(service.wiring.registry.active_count(), 0);
        let expected = if fail {
            LifecyclePhase::Unknown
        } else {
            LifecyclePhase::Closed
        };
        let status = service
            .status(&authority, Some(&operation))
            .unwrap()
            .remove(0);
        assert_eq!(status.phase, expected);
        assert_eq!(
            status.remote_outcome.as_deref(),
            Some(if fail { "unknown" } else { "accepted" })
        );
        assert!(service
            .answer(authority.clone(), operation.clone())
            .await
            .is_err());
        assert_eq!(
            service.hangup(&authority, &operation).await.unwrap().1,
            "local_applied"
        );
        assert_eq!(
            peer.attempts.load(Ordering::Acquire),
            1,
            "terminal cleanup is never retried"
        );
    }

    pub(crate) async fn stale_cleanup(
        transport: VoiceRelayTransport,
        runner: Arc<dyn HarnessRunner>,
    ) {
        let (service, peer, mut entered) = service(transport.clone(), runner, false);
        let authority = service.issue_for_authenticated_user("user", "run").unwrap();
        let operation = "stale-cleanup".to_string();
        service
            .insert(
                &authority,
                "bind",
                LifecyclePhase::Dialing,
                operation.clone(),
                CancellationToken::new(),
            )
            .unwrap();
        let mut owner = PendingOwner {
            service: Arc::downgrade(&service),
            operation: operation.clone(),
            permit: Some(reserve(&service, &transport)),
            uncertain: true,
            transferred: false,
        };
        let admitted = transport.admit_call("provider-call").await.unwrap();
        admitted.lease().revoke();
        let mut done = service
            .start_driver(&operation, admitted, &mut owner, transport, true)
            .unwrap();
        drop(owner);
        while !*done.borrow_and_update() {
            done.changed().await.unwrap();
        }
        assert_eq!(peer.attempts.load(Ordering::Acquire), 0);
        assert!(entered.try_recv().is_err());
        assert_eq!(service.wiring.registry.active_count(), 0);
        let status = service.status_of(&operation).unwrap();
        assert_eq!(status.phase, LifecyclePhase::Unknown);
        assert_eq!(status.remote_outcome.as_deref(), Some("unknown"));
    }

    pub(crate) async fn running_hangup_status(
        transport: VoiceRelayTransport,
        runner: Arc<dyn HarnessRunner>,
        fail: bool,
    ) {
        let (service, _, _) = service(transport.clone(), runner, false);
        let authority = service.issue_for_authenticated_user("user", "run").unwrap();
        let operation = "owned-waiter".to_string();
        service
            .insert(
                &authority,
                "bind",
                LifecyclePhase::Dialing,
                operation.clone(),
                CancellationToken::new(),
            )
            .unwrap();
        let mut owner = PendingOwner {
            service: Arc::downgrade(&service),
            operation: operation.clone(),
            permit: Some(reserve(&service, &transport)),
            uncertain: true,
            transferred: false,
        };
        let admitted = transport.admit_call("provider-call").await.unwrap();
        assert!(service
            .start_driver(&operation, admitted, &mut owner, transport, false)
            .is_none());
        drop(owner);
        let mut done = service
            .entries
            .lock()
            .unwrap()
            .get(&operation)
            .unwrap()
            .done
            .subscribe();
        let mut waiter = Box::pin(service.hangup(&authority, &operation));
        // Capture the authorized waiter before the driver handles the intent.
        assert!(matches!(
            futures::poll!(&mut waiter),
            std::task::Poll::Pending
        ));
        while !*done.borrow_and_update() {
            done.changed().await.unwrap();
        }
        let status = service.status_of(&operation).unwrap();
        assert_eq!(status.phase, LifecyclePhase::Closed);
        assert_eq!(
            status.remote_outcome.as_deref(),
            Some(if fail { "unknown" } else { "accepted" })
        );
        assert_eq!(service.wiring.registry.active_count(), 0);
        // Evict the terminal entry before polling the lawful waiter again.
        for i in 0..MAX_RECORDS {
            let id = format!("completed-{i}");
            service
                .insert(
                    &authority,
                    "bind",
                    LifecyclePhase::Dialing,
                    id.clone(),
                    CancellationToken::new(),
                )
                .unwrap();
            service.complete(&id, LifecyclePhase::Closed, None);
        }
        assert!(service.status(&authority, Some(&operation)).is_err());
        let (retained, completion) = waiter.await.unwrap();
        assert_eq!(retained.operation_id, operation);
        assert_eq!(retained.remote_outcome, status.remote_outcome);
        assert_eq!(completion, if fail { "unknown" } else { "accepted" });
        assert!(service.hangup(&authority, &operation).await.is_err());
    }

    pub(crate) async fn terminal_retention(
        transport: VoiceRelayTransport,
        runner: Arc<dyn HarnessRunner>,
    ) {
        let (service, _, _) = service(transport.clone(), runner, false);
        let authority = service.issue_for_authenticated_user("user", "run").unwrap();
        for i in 0..MAX_RECORDS {
            // Reverse lexical order proves eviction uses age, not BTree order.
            let operation = format!("terminal-{:03}", MAX_RECORDS - i);
            service
                .insert(
                    &authority,
                    "bind",
                    LifecyclePhase::Dialing,
                    operation.clone(),
                    CancellationToken::new(),
                )
                .unwrap();
            let owner = PendingOwner {
                service: Arc::downgrade(&service),
                operation: operation.clone(),
                permit: Some(reserve(&service, &transport)),
                uncertain: false,
                transferred: false,
            };
            drop(owner);
            service.set_phase(
                &operation,
                [
                    LifecyclePhase::Unknown,
                    LifecyclePhase::Closed,
                    LifecyclePhase::Denied,
                    LifecyclePhase::Failed,
                ][i % 4],
            );
            assert_eq!(service.wiring.registry.active_count(), 0);
        }
        let oldest = format!("terminal-{:03}", MAX_RECORDS);
        assert_eq!(
            service.status(&authority, Some(&oldest)).unwrap()[0].phase,
            LifecyclePhase::Unknown
        );
        assert!(service
            .answer(authority.clone(), oldest.clone())
            .await
            .is_err());
        service
            .insert(
                &authority,
                "bind",
                LifecyclePhase::Dialing,
                "257".into(),
                CancellationToken::new(),
            )
            .unwrap();
        assert_eq!(service.entries.lock().unwrap().len(), MAX_RECORDS);
        assert!(service.status(&authority, Some(&oldest)).is_err());
        assert!(service.status_of(&oldest).is_err());
        assert!(service
            .answer(authority.clone(), oldest.clone())
            .await
            .is_err());
        assert!(service.hangup(&authority, &oldest).await.is_err());
        assert!(service.status(&authority, Some("terminal-001")).is_ok());
        assert_eq!(
            service.status(&authority, Some("257")).unwrap()[0].phase,
            LifecyclePhase::Dialing
        );
        assert_eq!(service.wiring.registry.active_count(), 0);
        drop(reserve(&service, &transport));
    }

    pub(crate) fn active_retention(transport: VoiceRelayTransport, runner: Arc<dyn HarnessRunner>) {
        let (service, _, _) = service(transport, runner, false);
        let authority = service.issue_for_authenticated_user("user", "run").unwrap();
        for i in 0..MAX_RECORDS {
            let done = service
                .insert(
                    &authority,
                    "bind",
                    [
                        LifecyclePhase::Running,
                        LifecyclePhase::AwaitingApproval,
                        LifecyclePhase::Ringing,
                        LifecyclePhase::Dialing,
                        LifecyclePhase::Answering,
                    ][i % 5],
                    format!("active-{i}"),
                    CancellationToken::new(),
                )
                .unwrap();
            // Even a completion signal alone cannot make an active phase evictable.
            done.send_replace(true);
        }
        assert_eq!(
            service
                .insert(
                    &authority,
                    "bind",
                    LifecyclePhase::Dialing,
                    "overflow".into(),
                    CancellationToken::new()
                )
                .err(),
            Some("UNAVAILABLE: lifecycle state capacity reached")
        );
        assert_eq!(service.entries.lock().unwrap().len(), MAX_RECORDS);
        assert!(service.status(&authority, Some("active-0")).is_ok());
        // Terminal-but-unfinished records still own pending cleanup/permits.
        let mut entries = service.entries.lock().unwrap();
        let entry = entries.get_mut("active-0").unwrap();
        entry.status.phase = LifecyclePhase::Unknown;
        entry.done.send_replace(false);
        drop(entries);
        assert!(service
            .insert(
                &authority,
                "bind",
                LifecyclePhase::Dialing,
                "overflow".into(),
                CancellationToken::new()
            )
            .is_err());
        service
            .entries
            .lock()
            .unwrap()
            .get("active-0")
            .unwrap()
            .done
            .send_replace(true);
        service
            .insert(
                &authority,
                "bind",
                LifecyclePhase::Dialing,
                "overflow".into(),
                CancellationToken::new(),
            )
            .unwrap();
        assert!(service.status(&authority, Some("active-0")).is_err());
        assert!(service.status(&authority, Some("active-1")).is_ok());
        assert_eq!(service.entries.lock().unwrap().len(), MAX_RECORDS);
    }
}
