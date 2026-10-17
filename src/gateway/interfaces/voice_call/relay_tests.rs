//! Offline authenticated Relay E2E; no service account or live PSTN required.
mod lifecycle_dispatch;
mod limits;
use super::*;
use std::future::Future;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

use aleph_protocol::telecom_relay::{
    CallTurn, CallTurnRole, PlaybackEvent, PlaybackState, PlayedExtent, TelecomEnvelope,
    TelecomKind, TranscriptState, TELECOM_PROTOCOL_VERSION,
};
use tokio::sync::{broadcast, mpsc, Mutex as AsyncMutex, Notify};
use tokio_util::sync::CancellationToken;

use crate::gateway::telecom::{
    lifecycle_bridge::{voice_control_pair, ControlCompletion, ControlHandle, ControlPhase},
    public_admission::{PublicAdmission, PublicRunPermit, PublicRunRegistry},
    public_context::test_support::sample_public_context_for_peer,
    PublicHistoryConfig, PublicHistorySession, PublicHistoryStore, PublicHistoryTurn,
    TelecomChannel, TelecomPublicContext,
};
use crate::orchestrator::{
    dispatch::{
        FlowOutcome, FlowStreamEvent, HarnessRunner, PublicRunRequest, PUBLIC_PROMPT_VERSION,
    },
    errors::FlowError,
    flow_spec::{FlowInput, FlowSpec},
};
use crate::secrets::crypto::SecretsCrypto;

async fn bounded<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(5), future)
        .await
        .expect("fake E2E stalled")
}

async fn before_call_closes<T>(
    call: &mut (impl Future<Output = VoiceCallReport> + Unpin),
    event: impl Future<Output = T>,
) -> T {
    tokio::select! {
        biased;
        report = call => panic!("coordinator closed before local Clear observation: {:?}", report.termination),
        result = event => result,
    }
}

async fn assert_call_pending(call: &mut (impl Future<Output = VoiceCallReport> + Unpin)) {
    assert!(
        tokio::time::timeout(Duration::from_millis(20), call)
            .await
            .is_err(),
        "coordinator must remain pending until explicit hangup"
    );
}

fn policy() -> VoiceBindingPolicy {
    VoiceBindingPolicy::new(
        "region".into(),
        "bind".into(),
        "acct".into(),
        "+15550000000".into(),
        "+15550000001".into(),
    )
    .unwrap()
}

#[derive(Clone, Copy)]
enum Fault {
    None,
    Account,
    Service,
    Peer,
    ProviderCall,
    CoreCall,
    Binding,
    Generation,
}

struct FakePeer {
    input: AsyncMutex<mpsc::UnboundedReceiver<VoicePeerEvent>>,
    accepted: mpsc::UnboundedSender<CallCommand>,
    entered_submit: mpsc::UnboundedSender<CallCommand>,
    fault: Mutex<Fault>,
    block_auth: AtomicBool,
    auth_entered: Notify,
    auth_gate: Notify,
    block_connect: AtomicBool,
    connect_entered: Notify,
    connect_gate: Notify,
    block_send: AtomicBool,
    send_gate: Notify,
    fail_submit: AtomicBool,
    submit_cancel: Mutex<Option<CancellationToken>>,
    cancel_on_final_poll: Mutex<Option<Arc<PublicRunRegistry>>>,
    cancelled_at_final_check: AtomicBool,
    control_on_final_poll: Mutex<Option<(ControlHandle, u64)>>,
    block_control: AtomicBool,
    control_gate: Notify,
    delivered: mpsc::UnboundedSender<TelecomKind>,
    accepted_log: Mutex<Vec<CallCommand>>,
}
struct Rig {
    peer: Arc<FakePeer>,
    relay: VoiceRelayTransport,
    input: mpsc::UnboundedSender<VoicePeerEvent>,
    accepted: mpsc::UnboundedReceiver<CallCommand>,
    entered_submit: mpsc::UnboundedReceiver<CallCommand>,
    delivered: mpsc::UnboundedReceiver<TelecomKind>,
}
impl Rig {
    async fn new() -> Self {
        let (input, rx) = mpsc::unbounded_channel();
        let (accepted, accepted_rx) = mpsc::unbounded_channel();
        let (entered_submit, entered_rx) = mpsc::unbounded_channel();
        let (delivered, delivered_rx) = mpsc::unbounded_channel();
        let peer = Arc::new(FakePeer {
            input: AsyncMutex::new(rx),
            accepted,
            entered_submit,
            fault: Mutex::new(Fault::None),
            block_auth: AtomicBool::new(false),
            auth_entered: Notify::new(),
            auth_gate: Notify::new(),
            block_connect: AtomicBool::new(false),
            connect_entered: Notify::new(),
            connect_gate: Notify::new(),
            block_send: AtomicBool::new(false),
            send_gate: Notify::new(),
            fail_submit: AtomicBool::new(false),
            submit_cancel: Mutex::new(None),
            cancel_on_final_poll: Mutex::new(None),
            cancelled_at_final_check: AtomicBool::new(false),
            control_on_final_poll: Mutex::new(None),
            block_control: AtomicBool::new(false),
            control_gate: Notify::new(),
            delivered,
            accepted_log: Mutex::new(Vec::new()),
        });
        let relay = VoiceRelayTransport::new(peer.clone());
        assert_eq!(relay.connect(policy()).await.unwrap(), 1);
        Self {
            peer,
            relay,
            input,
            accepted: accepted_rx,
            entered_submit: entered_rx,
            delivered: delivered_rx,
        }
    }
    async fn admit(&self) -> AdmittedVoiceCall {
        self.relay.admit_call("provider-call").await.unwrap()
    }
}
#[async_trait::async_trait]
impl VoiceRelayPeer for FakePeer {
    async fn connect(&self, _: &str, _: u64) -> Result<(), VoiceRelayError> {
        self.connect_entered.notify_one();
        if self.block_connect.load(Ordering::Acquire) {
            self.connect_gate.notified().await;
        }
        Ok(())
    }
    async fn authenticate_call(
        &self,
        binding: &str,
        generation: u64,
        core: &str,
        provider: &str,
    ) -> Result<AuthenticatedVoiceFacts, VoiceRelayError> {
        self.auth_entered.notify_one();
        if self.block_auth.load(Ordering::Acquire) {
            self.auth_gate.notified().await;
        }
        let mut facts = AuthenticatedVoiceFacts {
            binding: binding.to_owned(),
            generation,
            provider_account_id: "acct".into(),
            phone_call_id: core.to_owned(),
            provider_call_id: provider.to_owned(),
            service_address: "+15550000000".into(),
            peer_address: "+15550000001".into(),
        };
        match *self.fault.lock().unwrap() {
            Fault::None => {}
            Fault::Account => facts.provider_account_id = "other".into(),
            Fault::Service => facts.service_address = "+15550000009".into(),
            Fault::Peer => facts.peer_address = "+15550000002".into(),
            Fault::ProviderCall => facts.provider_call_id = "other".into(),
            Fault::CoreCall => facts.phone_call_id = "other".into(),
            Fault::Binding => facts.binding = "other".into(),
            Fault::Generation => facts.generation += 1,
        }
        Ok(facts)
    }
    async fn next_event(&self, _: &VoiceCallLease) -> Result<VoicePeerEvent, VoiceRelayError> {
        let event = self
            .input
            .lock()
            .await
            .recv()
            .await
            .ok_or(VoiceRelayError::Peer("fake closed".into()))?;
        if let VoicePeerEvent::Envelope(envelope) = &event {
            self.delivered.send(envelope.kind.clone()).unwrap();
        }
        Ok(event)
    }
    async fn submit(
        &self,
        lease: &VoiceCallLease,
        command: CallCommand,
        cancel: &CancellationToken,
    ) -> Result<(), VoiceRelayError> {
        self.entered_submit.send(command.clone()).unwrap();
        *self.submit_cancel.lock().unwrap() = Some(cancel.clone());
        if matches!(command, CallCommand::SendText(_)) && self.block_send.load(Ordering::Acquire) {
            self.send_gate.notified().await;
        }
        // Inject stop inside the resumed peer poll, not from the driver while
        // the outer coordinator still has a chance to select cancellation.
        if let Some(registry) = self.cancel_on_final_poll.lock().unwrap().take() {
            assert!(registry.cancel_binding("bind"));
            self.cancelled_at_final_check
                .store(cancel.is_cancelled(), Ordering::Release);
        }
        if let Some((handle, epoch)) = self.control_on_final_poll.lock().unwrap().take() {
            drop(handle.cancel_output(epoch));
            self.cancelled_at_final_check
                .store(cancel.is_cancelled(), Ordering::Release);
        }
        if !matches!(command, CallCommand::SendText(_))
            && self.block_control.load(Ordering::Acquire)
        {
            self.control_gate.notified().await;
        }
        lease.check(cancel)?; // repeat the final check after the peer's waits
        if self.fail_submit.swap(false, Ordering::AcqRel) {
            return Err(VoiceRelayError::Peer("fake submit fail".into()));
        }
        self.accepted_log.lock().unwrap().push(command.clone());
        self.accepted.send(command).unwrap();
        Ok(())
    }
}

struct ProviderDropGuard(Arc<AtomicUsize>);
impl Drop for ProviderDropGuard {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::AcqRel);
    }
}

struct Runner {
    requests: mpsc::UnboundedSender<PublicRunRequest>,
    drops: Arc<AtomicUsize>,
    late_effects: AtomicUsize,
    calls: AtomicUsize,
    block: AtomicBool,
    gate: Notify,
    cancelled_token: Mutex<Option<CancellationToken>>,
    cancel_on_return: Mutex<Option<Arc<PublicRunRegistry>>>,
    cancelled_on_return: AtomicBool,
    control_on_return: Mutex<Option<(ControlHandle, u64)>>,
    fail: AtomicBool,
    reply: String,
}
impl Runner {
    fn new(reply: &str) -> (Arc<Self>, mpsc::UnboundedReceiver<PublicRunRequest>) {
        let (requests, rx) = mpsc::unbounded_channel();
        (
            Arc::new(Self {
                requests,
                drops: Arc::new(AtomicUsize::new(0)),
                late_effects: AtomicUsize::new(0),
                calls: AtomicUsize::new(0),
                block: AtomicBool::new(false),
                gate: Notify::new(),
                cancelled_token: Mutex::new(None),
                cancel_on_return: Mutex::new(None),
                cancelled_on_return: AtomicBool::new(false),
                control_on_return: Mutex::new(None),
                fail: AtomicBool::new(false),
                reply: reply.into(),
            }),
            rx,
        )
    }
}
#[async_trait::async_trait]
impl HarnessRunner for Runner {
    async fn run(
        &self,
        _: String,
        _: Arc<FlowSpec>,
        _: FlowInput,
        _: Arc<dyn crate::sandbox::Sandbox>,
        _: broadcast::Sender<FlowStreamEvent>,
        _: CancellationToken,
        _: Option<Arc<dyn crate::tools::service::ToolService>>,
        _: Option<Arc<dyn crate::harness::TraceSink>>,
        _: Option<crate::thinker::InteractionManifest>,
        _: Option<std::path::PathBuf>,
        _: Option<u32>,
        _: Option<String>,
        _: Option<crate::agents::thinking::ThinkLevel>,
        _: crate::thinker::TurnEnvelope,
        _: Option<crate::providers::session_model_handle::SessionModelPref>,
        _: String,
        _: Option<TelecomPublicContext>,
    ) -> Result<FlowOutcome, FlowError> {
        panic!("owner runner must never be entered")
    }

    async fn run_public_text(
        &self,
        request: PublicRunRequest,
        cancel: CancellationToken,
        events: broadcast::Sender<FlowStreamEvent>,
    ) -> Result<FlowOutcome, FlowError> {
        assert_eq!(request.prompt_version, PUBLIC_PROMPT_VERSION);
        assert!(!request.allow_private_lookup);
        assert_eq!(request.ctx.provenance().channel(), TelecomChannel::Voice);
        assert_eq!(events.receiver_count(), 0, "no owner streaming subscribers");
        let _drop_guard = ProviderDropGuard(self.drops.clone());
        self.calls.fetch_add(1, Ordering::AcqRel);
        *self.cancelled_token.lock().unwrap() = Some(cancel.clone());
        self.requests.send(request).unwrap();
        if self.block.load(Ordering::Acquire) {
            self.gate.notified().await;
        }
        self.late_effects.fetch_add(1, Ordering::AcqRel);
        if self.fail.load(Ordering::Acquire) {
            return Err(FlowError::Internal("fake failure".into()));
        }
        if let Some(registry) = self.cancel_on_return.lock().unwrap().take() {
            assert!(registry.cancel_binding("bind"));
            self.cancelled_on_return
                .store(cancel.is_cancelled(), Ordering::Release);
        }
        if let Some((handle, epoch)) = self.control_on_return.lock().unwrap().take() {
            drop(handle.cancel_output(epoch));
            self.cancelled_on_return
                .store(cancel.is_cancelled(), Ordering::Release);
        }
        Ok(FlowOutcome {
            final_text: self.reply.clone(),
            total_tokens: 10,
            ..Default::default()
        })
    }
}
fn store() -> Arc<PublicHistoryStore> {
    Arc::new(
        PublicHistoryStore::open_in_memory(
            SecretsCrypto::new("voice-test-dedicated-key"),
            PublicHistoryConfig::default(),
        )
        .unwrap(),
    )
}
fn wiring(
    rig: &Rig,
    runner: Arc<dyn HarnessRunner>,
    registry: Arc<PublicRunRegistry>,
    history: Option<Arc<PublicHistoryStore>>,
) -> PublicVoiceWiring {
    PublicVoiceWiring {
        registry,
        harness: runner,
        history,
        transport: rig.relay.clone(),
    }
}
fn envelope(s: &CallSession, kind: TelecomKind) -> TelecomEnvelope {
    let now = chrono::Utc::now();
    TelecomEnvelope {
        protocol_version: TELECOM_PROTOCOL_VERSION,
        binding_id: s.binding_id.clone(),
        connection_generation: s.connection_generation,
        event_id: uuid::Uuid::new_v4().to_string(),
        kind,
        created_at: now,
        expires_at: now + chrono::Duration::minutes(1),
    }
}
fn transcript(s: &CallSession, epoch: u64, seq: u64, text: &str, interim: bool) -> VoicePeerEvent {
    VoicePeerEvent::Envelope(envelope(
        s,
        TelecomKind::CallTurn(CallTurn {
            provider_account_id: s.provider_account_id.clone(),
            phone_call_id: s.phone_call_id.clone(),
            provider_call_id: s.provider_call_id.clone(),
            turn_id: format!("turn-{epoch}"),
            role: CallTurnRole::UserTranscript,
            transcript_state: if interim {
                TranscriptState::Interim
            } else {
                TranscriptState::Committed
            },
            output_epoch: epoch,
            output_seq: seq,
            text: text.into(),
        }),
    ))
}
fn playback(s: &CallSession, output: &CallTurn, extent: PlayedExtent) -> VoicePeerEvent {
    VoicePeerEvent::Envelope(envelope(
        s,
        TelecomKind::PlaybackEvent(PlaybackEvent {
            provider_account_id: s.provider_account_id.clone(),
            phone_call_id: s.phone_call_id.clone(),
            turn_id: output.turn_id.clone(),
            output_epoch: output.output_epoch,
            output_seq: output.output_seq,
            state: PlaybackState::Played,
            extent,
        }),
    ))
}
fn cleared(s: &CallSession, output: &CallTurn) -> VoicePeerEvent {
    let VoicePeerEvent::Envelope(mut event) = playback(s, output, PlayedExtent::None) else {
        unreachable!()
    };
    let TelecomKind::PlaybackEvent(fact) = &mut event.kind else {
        unreachable!()
    };
    fact.state = PlaybackState::Cleared;
    VoicePeerEvent::Envelope(event)
}

// These tests drive both futures in the same task. Once the driver receives
// this marker, the coordinator's poll has applied the delivered event and
// returned Pending (there is no await between delivery and reducer apply).
async fn observed_playback(
    delivered: &mut mpsc::UnboundedReceiver<TelecomKind>,
    epoch: u64,
    state: PlaybackState,
) {
    loop {
        if matches!(delivered.recv().await.unwrap(), TelecomKind::PlaybackEvent(e)
            if e.output_epoch == epoch && e.state == state)
        {
            return;
        }
    }
}

fn hangup(s: &CallSession) -> VoicePeerEvent {
    VoicePeerEvent::Hangup(AuthenticatedVoiceHangup {
        binding: s.binding_id.clone(),
        generation: s.connection_generation,
        provider_account_id: s.provider_account_id.clone(),
        phone_call_id: s.phone_call_id.clone(),
        provider_call_id: s.provider_call_id.clone(),
    })
}
fn as_text(command: CallCommand) -> CallTurn {
    let CallCommand::SendText(t) = command else {
        panic!("expected SendText");
    };
    t
}

#[tokio::test]
async fn fake_authenticated_call_only_persists_exact_heard_on_hangup_then_sms_shares_it() {
    let mut rig = Rig::new().await;
    let admitted = rig.admit().await;
    let lease = admitted.lease().clone();
    let s = lease.session().clone();
    let history = store();
    let key = PublicHistorySession::from_provenance(admitted.context().provenance());
    let registry = PublicRunRegistry::new(4, 1000);
    let (runner, mut requests) = Runner::new(" 你好🙂 tail ");
    let cfg = wiring(
        &rig,
        runner.clone(),
        registry.clone(),
        Some(history.clone()),
    );
    let (report, ()) = bounded(async {
        tokio::join!(run_public_voice_call(admitted, cfg), async {
            rig.input
                .send(transcript(&s, 1, 1, "interim", true))
                .unwrap();
            rig.input
                .send(transcript(&s, 1, 1, "question", false))
                .unwrap();
            rig.input
                .send(transcript(&s, 1, 1, "question", false))
                .unwrap();
            let request = requests.recv().await.unwrap();
            assert_eq!(request.text, "question");
            assert!(request.history.is_empty());
            let output = as_text(rig.accepted.recv().await.unwrap());
            assert_eq!(output.output_seq, 1);
            assert!(
                history.load(&key, chrono::Utc::now()).unwrap().is_empty(),
                "accepted != heard"
            );
            let sms = sample_public_context_for_peer("bind", "+15550000001");
            assert!(matches!(registry.try_admit(&sms), PublicAdmission::Busy));
            rig.input
                .send(playback(&s, &output, PlayedExtent::Exact { chars: 4 }))
                .unwrap();
            assert!(
                history.load(&key, chrono::Utc::now()).unwrap().is_empty(),
                "active transcripts stay private"
            );
            rig.input.send(hangup(&s)).unwrap();
        })
    })
    .await;
    assert_eq!(report.termination, VoiceCallTermination::Hangup);
    assert_eq!(report.frames_accepted, 1);
    assert_eq!(report.history_appended, 1);
    assert_eq!(
        runner.calls.load(Ordering::Acquire),
        1,
        "interim/duplicate launch zero extra runs"
    );
    assert!(!lease.is_active());
    assert_eq!(registry.active_count(), 0);
    assert_eq!(registry.tokens_spent(), 10);
    let sms = sample_public_context_for_peer("bind", "+15550000001");
    // Region must match the configured voice region, not the test SMS default.
    let sms_meta = crate::gateway::telecom::SmsInboundMeta {
        region: "region".into(),
        binding_ref: "bind".into(),
        provider_account_id: "acct".into(),
        provider_message_id: "sms-next".into(),
        envelope_event_id: "sms-next".into(),
        connection_generation: 8,
        service_address: "+15550000000".into(),
        peer_address: "+15550000001".into(),
        encoding: aleph_protocol::telecom_relay::SmsEncoding::Gsm7,
        segment_count: 1,
        created_at: chrono::Utc::now(),
        expires_at: chrono::Utc::now(),
    };
    let sms_ctx = TelecomPublicContext::from_admitted_sms(&sms_meta);
    let sms_key = PublicHistorySession::from_provenance(sms_ctx.provenance());
    assert_eq!(key, sms_key);
    let turns = history.load(&sms_key, chrono::Utc::now()).unwrap();
    assert_eq!(
        turns,
        vec![PublicHistoryTurn {
            user: "question".into(),
            assistant: " 你好🙂".into()
        }]
    );
    assert!(matches!(
        registry.try_admit(&sms),
        PublicAdmission::Admitted(_)
    ));
}

#[tokio::test]
async fn uncertain_playback_and_acceptance_without_playback_never_enter_history() {
    for ack in [false, true] {
        let mut rig = Rig::new().await;
        let admitted = rig.admit().await;
        let s = admitted.lease().session().clone();
        let history = store();
        let key = PublicHistorySession::from_provenance(admitted.context().provenance());
        let (runner, _requests) = Runner::new("not proved heard");
        let cfg = wiring(
            &rig,
            runner,
            PublicRunRegistry::new(4, 1000),
            Some(history.clone()),
        );
        let (report, ()) = bounded(async {
            tokio::join!(run_public_voice_call(admitted, cfg), async {
                rig.input.send(transcript(&s, 1, 1, "q", false)).unwrap();
                let output = as_text(rig.accepted.recv().await.unwrap());
                if ack {
                    rig.input
                        .send(playback(&s, &output, PlayedExtent::Uncertain))
                        .unwrap();
                }
                rig.input.send(hangup(&s)).unwrap();
            })
        })
        .await;
        assert_eq!(report.termination, VoiceCallTermination::Hangup);
        assert_eq!(report.history_appended, 0);
        assert!(history.load(&key, chrono::Utc::now()).unwrap().is_empty());
    }
}

#[tokio::test]
async fn authenticated_fact_mismatches_refuse_and_do_not_leak_pending_admission() {
    let rig = Rig::new().await;
    for fault in [
        Fault::Account,
        Fault::Service,
        Fault::Peer,
        Fault::ProviderCall,
        Fault::CoreCall,
        Fault::Binding,
        Fault::Generation,
    ] {
        *rig.peer.fault.lock().unwrap() = fault;
        assert!(matches!(
            rig.relay.admit_call("provider-call").await,
            Err(VoiceRelayError::Scope)
        ));
        *rig.peer.fault.lock().unwrap() = Fault::None;
        let admitted = rig.admit().await;
        assert!(admitted.lease().is_active());
        drop(admitted);
    }
    for raw in [
        "15550000001",
        "+05550000001",
        "+1 5550000001",
        "+15550000001 ",
    ] {
        assert!(VoiceBindingPolicy::new(
            "r".into(),
            "b".into(),
            "a".into(),
            "+15550000000".into(),
            raw.into()
        )
        .is_err());
    }
}

#[tokio::test]
async fn drop_reconnect_and_foreign_transport_revoke_pinned_capability() {
    let rig = Rig::new().await;
    let admitted = rig.admit().await;
    let old = admitted.lease().clone();
    let foreign = Rig::new().await;
    let cmd = CallCommand::Hangup {
        phone_call_id: old.session().phone_call_id.clone(),
    };
    assert!(foreign
        .relay
        .submit(&old, cmd.clone(), &CancellationToken::new())
        .await
        .is_err());
    assert_eq!(rig.relay.connect(policy()).await.unwrap(), 2);
    assert!(!old.is_active());
    assert!(rig
        .relay
        .submit(&old, cmd, &CancellationToken::new())
        .await
        .is_err());
    let current = rig.admit().await;
    drop(admitted); // old owner must not revoke the new call
    assert!(current.lease().is_active());
    let cloned = current.lease().clone();
    drop(current);
    assert!(!cloned.is_active());
    assert!(rig.peer.accepted_log.lock().unwrap().is_empty());
}

#[tokio::test]
async fn reconnect_and_disconnect_interrupt_blocked_authentication_and_connect() {
    let rig = Rig::new().await;
    rig.peer.block_auth.store(true, Ordering::Release);
    let (result, ()) = bounded(async {
        tokio::join!(rig.relay.admit_call("provider-call"), async {
            rig.peer.auth_entered.notified().await;
            assert!(matches!(
                rig.relay.admit_call("another").await,
                Err(VoiceRelayError::Busy)
            ));
            assert_eq!(rig.relay.connect(policy()).await.unwrap(), 2);
            rig.peer.auth_gate.notify_one();
        })
    })
    .await;
    assert!(matches!(result, Err(VoiceRelayError::Revoked)));
    rig.peer.block_auth.store(false, Ordering::Release);
    drop(rig.admit().await);
    // Clear the initial completed-connect notification before blocking anew.
    rig.peer.connect_entered.notified().await;
    rig.peer.block_connect.store(true, Ordering::Release);
    let (result, ()) = bounded(async {
        tokio::join!(rig.relay.connect(policy()), async {
            rig.peer.connect_entered.notified().await;
            rig.relay.disconnect();
            rig.peer.connect_gate.notify_one();
        })
    })
    .await;
    assert!(matches!(result, Err(VoiceRelayError::Revoked)));
    assert!(matches!(
        rig.relay.admit_call("pc").await,
        Err(VoiceRelayError::Revoked)
    ));
}

#[tokio::test]
async fn dropping_pending_admission_or_connect_has_no_authority_leak() {
    let rig = Rig::new().await;
    rig.peer.block_auth.store(true, Ordering::Release);
    let mut future = Box::pin(rig.relay.admit_call("provider-call"));
    tokio::select! { _ = &mut future => panic!("auth should block"), _ = rig.peer.auth_entered.notified() => {} }
    drop(future);
    rig.peer.block_auth.store(false, Ordering::Release);
    drop(rig.admit().await);
    rig.peer.connect_entered.notified().await;
    rig.peer.block_connect.store(true, Ordering::Release);
    let mut future = Box::pin(rig.relay.connect(policy()));
    tokio::select! { _ = &mut future => panic!("connect should block"), _ = rig.peer.connect_entered.notified() => {} }
    drop(future);
    assert!(matches!(
        rig.relay.admit_call("pc").await,
        Err(VoiceRelayError::Revoked)
    ));
}

#[tokio::test]
async fn inbound_wire_wrong_identity_generation_expiry_version_direction_fails_closed() {
    for fault in 0..9 {
        let rig = Rig::new().await;
        let admitted = rig.admit().await;
        let lease = admitted.lease().clone();
        let VoicePeerEvent::Envelope(mut event) = transcript(lease.session(), 1, 1, "q", false)
        else {
            unreachable!()
        };
        match fault {
            0 => event.binding_id = "other".into(),
            1 => event.connection_generation += 1,
            2 => event.expires_at = chrono::Utc::now() - chrono::Duration::seconds(1),
            3 => event.protocol_version += 1,
            4..=8 => {
                if let TelecomKind::CallTurn(t) = &mut event.kind {
                    match fault {
                        4 => t.provider_account_id = "other".into(),
                        5 => t.provider_call_id = "other".into(),
                        6 => t.phone_call_id = "other".into(),
                        7 => t.role = CallTurnRole::AssistantText,
                        _ => t.output_epoch = 0,
                    }
                }
            }
            _ => unreachable!(),
        }
        rig.input.send(VoicePeerEvent::Envelope(event)).unwrap();
        let (runner, _requests) = Runner::new("never");
        let registry = PublicRunRegistry::new(4, 100);
        let report = bounded(run_public_voice_call(
            admitted,
            wiring(&rig, runner.clone(), registry.clone(), None),
        ))
        .await;
        assert_eq!(report.termination, VoiceCallTermination::InvalidEvent);
        assert_eq!(runner.calls.load(Ordering::Acquire), 0);
        assert!(!lease.is_active());
        assert_eq!(registry.active_count(), 0);
    }
}

#[tokio::test]
async fn same_turn_revision_does_not_reassociate_or_repeat_side_effects() {
    let rig = Rig::new().await;
    let admitted = rig.admit().await;
    let s = admitted.lease().session().clone();
    let registry = PublicRunRegistry::new(4, 100);
    let (runner, mut requests) = Runner::new("never");
    runner.block.store(true, Ordering::Release);
    let cfg = wiring(&rig, runner.clone(), registry.clone(), None);
    let (report, ()) = bounded(async {
        tokio::join!(run_public_voice_call(admitted, cfg), async {
            rig.input
                .send(transcript(&s, 1, 1, "original", false))
                .unwrap();
            assert_eq!(requests.recv().await.unwrap().text, "original");
            rig.input
                .send(transcript(&s, 1, 2, "revised", false))
                .unwrap();
        })
    })
    .await;
    assert_eq!(report.termination, VoiceCallTermination::SameTurnRevision);
    assert_eq!(runner.calls.load(Ordering::Acquire), 1);
    assert_eq!(registry.tokens_spent(), 100);
    assert!(runner
        .cancelled_token
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .is_cancelled());
}

#[tokio::test]
async fn barge_in_drops_blocked_started_runner_and_unknown_spend_stops_next_invocation() {
    let rig = Rig::new().await;
    let admitted = rig.admit().await;
    let s = admitted.lease().session().clone();
    let registry = PublicRunRegistry::new(4, 100);
    let (runner, mut requests) = Runner::new("late output");
    runner.block.store(true, Ordering::Release);
    let cfg = wiring(&rig, runner.clone(), registry.clone(), None);
    let (report, ()) = bounded(async {
        tokio::join!(run_public_voice_call(admitted, cfg), async {
            rig.input.send(transcript(&s, 1, 1, "old", false)).unwrap();
            requests.recv().await.unwrap();
            rig.input.send(transcript(&s, 2, 1, "new", false)).unwrap();
        })
    })
    .await;
    assert_eq!(report.termination, VoiceCallTermination::OverSpend);
    assert_eq!(runner.calls.load(Ordering::Acquire), 1);
    assert_eq!(registry.tokens_spent(), 100);
    runner.gate.notify_one();
    assert!(
        rig.peer.accepted_log.lock().unwrap().is_empty(),
        "dropped old runner has no continuation"
    );
}

#[tokio::test]
async fn barge_in_blocked_send_cancels_epoch_before_new_send_and_late_playback_is_ignored() {
    let mut rig = Rig::new().await;
    rig.peer.block_send.store(true, Ordering::Release);
    let admitted = rig.admit().await;
    let s = admitted.lease().session().clone();
    let history = store();
    let key = PublicHistorySession::from_provenance(admitted.context().provenance());
    let registry = PublicRunRegistry::new(4, 1000);
    let (runner, _requests) = Runner::new("answer");
    let cfg = wiring(
        &rig,
        runner.clone(),
        registry.clone(),
        Some(history.clone()),
    );
    let (report, ()) = bounded(async {
        tokio::join!(run_public_voice_call(admitted, cfg), async {
            rig.input.send(transcript(&s, 1, 1, "old", false)).unwrap();
            let old = as_text(rig.entered_submit.recv().await.unwrap());
            rig.peer.block_send.store(false, Ordering::Release);
            rig.input.send(transcript(&s, 2, 1, "new", false)).unwrap();
            rig.input
                .send(playback(&s, &old, PlayedExtent::Exact { chars: 6 }))
                .unwrap();
            assert!(matches!(
                rig.accepted.recv().await.unwrap(),
                CallCommand::CancelOutput {
                    output_epoch: 1,
                    ..
                }
            ));
            let new = as_text(rig.accepted.recv().await.unwrap());
            assert_eq!(new.output_epoch, 2);
            rig.input
                .send(playback(&s, &new, PlayedExtent::Exact { chars: 3 }))
                .unwrap();
            rig.input.send(hangup(&s)).unwrap();
            rig.peer.send_gate.notify_one();
        })
    })
    .await;
    assert_eq!(report.termination, VoiceCallTermination::Hangup);
    assert_eq!(runner.calls.load(Ordering::Acquire), 2);
    assert_eq!(registry.tokens_spent(), 20);
    assert_eq!(
        history.load(&key, chrono::Utc::now()).unwrap(),
        vec![PublicHistoryTurn {
            user: "new".into(),
            assistant: "ans".into()
        }]
    );
    assert!(rig
        .peer
        .accepted_log
        .lock()
        .unwrap()
        .iter()
        .all(|c| !matches!(c, CallCommand::SendText(t) if t.output_epoch == 1)));
}

#[tokio::test]
async fn hangup_disconnect_permit_cancel_and_drop_interrupt_blocked_runner_without_stale_work() {
    for mode in 0..4 {
        let rig = Rig::new().await;
        let admitted = rig.admit().await;
        let lease = admitted.lease().clone();
        let s = lease.session().clone();
        let registry = PublicRunRegistry::new(4, 100);
        let (runner, mut requests) = Runner::new("late");
        runner.block.store(true, Ordering::Release);
        let mut work = Box::pin(run_public_voice_call(
            admitted,
            wiring(&rig, runner.clone(), registry.clone(), None),
        ));
        rig.input.send(transcript(&s, 1, 1, "q", false)).unwrap();
        tokio::select! { _ = &mut work => panic!("runner should block"), _ = bounded(requests.recv()) => {} }
        match mode {
            0 => rig.input.send(hangup(&s)).unwrap(),
            1 => rig.relay.disconnect(),
            2 => {
                assert!(registry.cancel_binding("bind"));
            }
            _ => {}
        }
        if mode < 3 {
            let report = bounded(&mut work).await;
            assert!(matches!(
                report.termination,
                VoiceCallTermination::Hangup
                    | VoiceCallTermination::Revoked
                    | VoiceCallTermination::Cancelled
            ));
        }
        drop(work);
        assert!(!lease.is_active());
        assert_eq!(registry.active_count(), 0);
        assert_eq!(registry.tokens_spent(), 100);
        runner.gate.notify_one();
        assert!(rig.peer.accepted_log.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn hangup_and_reconnect_interrupt_blocked_send_before_external_acceptance() {
    for reconnect in [false, true] {
        let mut rig = Rig::new().await;
        rig.peer.block_send.store(true, Ordering::Release);
        let admitted = rig.admit().await;
        let lease = admitted.lease().clone();
        let s = lease.session().clone();
        let registry = PublicRunRegistry::new(4, 100);
        let (runner, _requests) = Runner::new("late");
        let cfg = wiring(&rig, runner, registry.clone(), None);
        let (report, ()) = bounded(async {
            tokio::join!(run_public_voice_call(admitted, cfg), async {
                rig.input.send(transcript(&s, 1, 1, "q", false)).unwrap();
                rig.entered_submit.recv().await.unwrap();
                if reconnect {
                    rig.relay.connect(policy()).await.unwrap();
                } else {
                    rig.input.send(hangup(&s)).unwrap();
                }
            })
        })
        .await;
        assert!(matches!(
            report.termination,
            VoiceCallTermination::Hangup | VoiceCallTermination::Revoked
        ));
        assert!(!lease.is_active());
        assert_eq!(registry.active_count(), 0);
        rig.peer.send_gate.notify_one();
        assert!(rig.peer.accepted_log.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn spend_is_rechecked_for_each_utterance_and_failed_started_invocation_exhausts_fallback() {
    for fail in [false, true] {
        let mut rig = Rig::new().await;
        let admitted = rig.admit().await;
        let s = admitted.lease().session().clone();
        let registry = PublicRunRegistry::new(4, 10);
        let (runner, _requests) = Runner::new("answer");
        runner.fail.store(fail, Ordering::Release);
        let cfg = wiring(&rig, runner.clone(), registry.clone(), None);
        let report = if fail {
            rig.input.send(transcript(&s, 1, 1, "q", false)).unwrap();
            bounded(run_public_voice_call(admitted, cfg)).await
        } else {
            let (report, ()) = bounded(async {
                tokio::join!(run_public_voice_call(admitted, cfg), async {
                    rig.input.send(transcript(&s, 1, 1, "q", false)).unwrap();
                    rig.accepted.recv().await.unwrap();
                    rig.input.send(transcript(&s, 2, 1, "next", false)).unwrap();
                })
            })
            .await;
            report
        };
        assert_eq!(
            report.termination,
            if fail {
                VoiceCallTermination::RunnerFailed
            } else {
                VoiceCallTermination::OverSpend
            }
        );
        assert_eq!(runner.calls.load(Ordering::Acquire), 1);
        assert_eq!(registry.tokens_spent(), 10);
        assert_eq!(registry.active_count(), 0);
    }
}

#[tokio::test]
async fn unreadable_completed_history_refuses_before_public_invocation_without_reset() {
    let rig = Rig::new().await;
    let admitted = rig.admit().await;
    let lease = admitted.lease().clone();
    let key = PublicHistorySession::from_provenance(admitted.context().provenance());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.db");
    let correct = PublicHistoryStore::open(
        &path,
        SecretsCrypto::new("original-key"),
        PublicHistoryConfig::default(),
    )
    .unwrap();
    correct
        .append(
            &key,
            &PublicHistoryTurn {
                user: "prior".into(),
                assistant: "heard".into(),
            },
            chrono::Utc::now(),
            false,
        )
        .unwrap();
    let wrong = Arc::new(
        PublicHistoryStore::open(
            &path,
            SecretsCrypto::new("wrong-key"),
            PublicHistoryConfig::default(),
        )
        .unwrap(),
    );
    let registry = PublicRunRegistry::new(4, 100);
    let (runner, _requests) = Runner::new("never");
    let report = run_public_voice_call(
        admitted,
        wiring(&rig, runner.clone(), registry.clone(), Some(wrong)),
    )
    .await;
    assert_eq!(report.termination, VoiceCallTermination::HistoryUnavailable);
    assert_eq!(runner.calls.load(Ordering::Acquire), 0);
    assert_eq!(registry.active_count(), 0);
    assert!(!lease.is_active());
    assert_eq!(correct.load(&key, chrono::Utc::now()).unwrap().len(), 1);
}

#[tokio::test]
async fn dropped_coordinator_finalizes_exact_prefix_before_releasing_busy_slot() {
    let mut rig = Rig::new().await;
    let admitted = rig.admit().await;
    let lease = admitted.lease().clone();
    let s = lease.session().clone();
    let history = store();
    let key = PublicHistorySession::from_provenance(admitted.context().provenance());
    let registry = PublicRunRegistry::new(4, 1000);
    let (runner, _requests) = Runner::new("🙂 text");
    let mut work = Box::pin(run_public_voice_call(
        admitted,
        wiring(&rig, runner, registry.clone(), Some(history.clone())),
    ));
    rig.input
        .send(transcript(&s, 1, 1, "question", false))
        .unwrap();
    let output = tokio::select! { _ = &mut work => panic!("not closed"), c = bounded(rig.accepted.recv()) => as_text(c.unwrap()) };
    rig.input
        .send(playback(&s, &output, PlayedExtent::Exact { chars: 2 }))
        .unwrap();
    // A new blocked send proves the previous ACK and the new turn both went
    // through the reducer, including the old exact-prefix snapshot.
    rig.peer.block_send.store(true, Ordering::Release);
    rig.input.send(transcript(&s, 2, 1, "next", false)).unwrap();
    loop {
        tokio::select! { _ = &mut work => panic!("not closed"), c = bounded(rig.entered_submit.recv()) => {
            if matches!(c, Some(CallCommand::SendText(t)) if t.output_epoch == 2) { break; }
        } }
    }
    assert!(history.load(&key, chrono::Utc::now()).unwrap().is_empty());
    drop(work);
    assert!(!lease.is_active());
    assert_eq!(registry.active_count(), 0);
    assert_eq!(
        history.load(&key, chrono::Utc::now()).unwrap(),
        vec![PublicHistoryTurn {
            user: "question".into(),
            assistant: "🙂 ".into()
        }]
    );
}

#[tokio::test]
async fn early_exact_playback_during_blocked_send_then_hangup_is_not_accepted_history() {
    let mut rig = Rig::new().await;
    rig.peer.block_send.store(true, Ordering::Release);
    let admitted = rig.admit().await;
    let s = admitted.lease().session().clone();
    let history = store();
    let key = PublicHistorySession::from_provenance(admitted.context().provenance());
    let registry = PublicRunRegistry::new(4, 1000);
    let (runner, _requests) = Runner::new(" 你好🙂 tail ");
    let cfg = wiring(&rig, runner, registry.clone(), Some(history.clone()));
    let (report, ()) = bounded(async {
        tokio::join!(run_public_voice_call(admitted, cfg), async {
            rig.input.send(transcript(&s, 1, 1, "q", false)).unwrap();
            let output = as_text(rig.entered_submit.recv().await.unwrap());
            rig.input
                .send(playback(&s, &output, PlayedExtent::Exact { chars: 4 }))
                .unwrap();
            observed_playback(&mut rig.delivered, 1, PlaybackState::Played).await;
            assert!(history.load(&key, chrono::Utc::now()).unwrap().is_empty());
            rig.input.send(hangup(&s)).unwrap();
        })
    })
    .await;
    assert_eq!(report.termination, VoiceCallTermination::Hangup);
    assert_eq!(report.frames_accepted, 0);
    assert_eq!(report.history_appended, 0);
    assert!(history.load(&key, chrono::Utc::now()).unwrap().is_empty());
    assert!(rig
        .peer
        .submit_cancel
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .is_cancelled());
    rig.peer.send_gate.notify_one();
    assert!(rig.peer.accepted_log.lock().unwrap().is_empty());
    assert_eq!(registry.active_count(), 0);
}

#[tokio::test]
async fn early_exact_playback_during_blocked_send_then_drop_is_not_history() {
    let mut rig = Rig::new().await;
    rig.peer.block_send.store(true, Ordering::Release);
    let admitted = rig.admit().await;
    let lease = admitted.lease().clone();
    let s = lease.session().clone();
    let history = store();
    let key = PublicHistorySession::from_provenance(admitted.context().provenance());
    let registry = PublicRunRegistry::new(4, 1000);
    let (runner, _requests) = Runner::new("🙂 text");
    let mut work = Box::pin(run_public_voice_call(
        admitted,
        wiring(&rig, runner, registry.clone(), Some(history.clone())),
    ));
    rig.input.send(transcript(&s, 1, 1, "q", false)).unwrap();
    let output = bounded(async {
        tokio::select! { biased;
            _ = &mut work => panic!("send should block"),
            c = rig.entered_submit.recv() => as_text(c.unwrap()),
        }
    })
    .await;
    rig.input
        .send(playback(&s, &output, PlayedExtent::Exact { chars: 2 }))
        .unwrap();
    bounded(async {
        tokio::select! { biased;
            _ = &mut work => panic!("send should still block"),
            _ = observed_playback(&mut rig.delivered, 1, PlaybackState::Played) => {},
        }
    })
    .await;
    assert!(history.load(&key, chrono::Utc::now()).unwrap().is_empty());
    drop(work);
    assert!(history.load(&key, chrono::Utc::now()).unwrap().is_empty());
    assert!(!lease.is_active());
    assert_eq!(registry.active_count(), 0);
    assert!(rig
        .peer
        .submit_cancel
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .is_cancelled());
    rig.peer.send_gate.notify_one();
    assert!(rig.peer.accepted_log.lock().unwrap().is_empty());
}

#[tokio::test]
async fn early_exact_playback_only_enters_history_after_successful_submit_ack() {
    for fail in [true, false] {
        let mut rig = Rig::new().await;
        rig.peer.block_send.store(true, Ordering::Release);
        let admitted = rig.admit().await;
        let s = admitted.lease().session().clone();
        let history = store();
        let key = PublicHistorySession::from_provenance(admitted.context().provenance());
        let registry = PublicRunRegistry::new(4, 1000);
        let (runner, _requests) = Runner::new(" 你好🙂 tail ");
        let cfg = wiring(&rig, runner, registry.clone(), Some(history.clone()));
        let (report, ()) = bounded(async {
            tokio::join!(run_public_voice_call(admitted, cfg), async {
                rig.input
                    .send(transcript(&s, 1, 1, "question", false))
                    .unwrap();
                let output = as_text(rig.entered_submit.recv().await.unwrap());
                rig.input
                    .send(playback(&s, &output, PlayedExtent::Exact { chars: 4 }))
                    .unwrap();
                observed_playback(&mut rig.delivered, 1, PlaybackState::Played).await;
                assert!(history.load(&key, chrono::Utc::now()).unwrap().is_empty());
                rig.peer.fail_submit.store(fail, Ordering::Release);
                rig.peer.send_gate.notify_one();
                if !fail {
                    assert_eq!(as_text(rig.accepted.recv().await.unwrap()), output);
                    assert!(history.load(&key, chrono::Utc::now()).unwrap().is_empty());
                    rig.input.send(hangup(&s)).unwrap();
                }
            })
        })
        .await;
        assert_eq!(
            report.termination,
            if fail {
                VoiceCallTermination::SubmissionFailed
            } else {
                VoiceCallTermination::Hangup
            }
        );
        assert_eq!(report.frames_accepted, usize::from(!fail));
        assert_eq!(report.history_appended, usize::from(!fail));
        let expected = if fail {
            Vec::new()
        } else {
            vec![PublicHistoryTurn {
                user: "question".into(),
                assistant: " 你好🙂".into(),
            }]
        };
        assert_eq!(history.load(&key, chrono::Utc::now()).unwrap(), expected);
        assert_eq!(
            rig.peer.accepted_log.lock().unwrap().len(),
            usize::from(!fail)
        );
        assert_eq!(registry.active_count(), 0);
    }
}

#[tokio::test]
async fn current_cleared_cancels_blocked_send_even_after_early_terminal_playback() {
    for early_played in [false, true] {
        let mut rig = Rig::new().await;
        rig.peer.block_send.store(true, Ordering::Release);
        let admitted = rig.admit().await;
        let s = admitted.lease().session().clone();
        let history = store();
        let key = PublicHistorySession::from_provenance(admitted.context().provenance());
        let registry = PublicRunRegistry::new(4, 1000);
        let (runner, _requests) = Runner::new("answer");
        let cfg = wiring(&rig, runner, registry.clone(), Some(history.clone()));
        let (report, ()) = bounded(async {
            tokio::join!(run_public_voice_call(admitted, cfg), async {
                rig.input.send(transcript(&s, 1, 1, "q", false)).unwrap();
                let output = as_text(rig.entered_submit.recv().await.unwrap());
                if early_played {
                    rig.input
                        .send(playback(&s, &output, PlayedExtent::Exact { chars: 3 }))
                        .unwrap();
                    observed_playback(&mut rig.delivered, 1, PlaybackState::Played).await;
                }
                rig.input.send(cleared(&s, &output)).unwrap();
                observed_playback(&mut rig.delivered, 1, PlaybackState::Cleared).await;
                // Assert before hangup: dropping the coordinator must not hide a
                // missed epoch cancellation, including DuplicateSequence Cleared.
                assert!(rig
                    .peer
                    .submit_cancel
                    .lock()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .is_cancelled());
                rig.peer.send_gate.notify_one();
                rig.input.send(cleared(&s, &output)).unwrap();
                observed_playback(&mut rig.delivered, 1, PlaybackState::Cleared).await;
                assert!(rig.peer.accepted_log.lock().unwrap().is_empty());
                rig.input.send(hangup(&s)).unwrap();
            })
        })
        .await;
        assert_eq!(report.termination, VoiceCallTermination::Hangup);
        assert_eq!(report.frames_accepted, 0);
        assert_eq!(report.history_appended, 0);
        assert!(history.load(&key, chrono::Utc::now()).unwrap().is_empty());
        assert!(rig.peer.accepted_log.lock().unwrap().is_empty());
        assert_eq!(registry.active_count(), 0);
    }
}

#[tokio::test]
async fn controlled_unknown_cleared_cancels_pending_send_without_history_or_acceptance() {
    let mut rig = Rig::new().await;
    rig.peer.block_send.store(true, Ordering::Release);
    let admitted = rig.admit().await;
    let lease = admitted.lease().clone();
    let ctx = admitted.context().clone();
    let s = lease.session().clone();
    let history = store();
    let key = PublicHistorySession::from_provenance(ctx.provenance());
    let registry = PublicRunRegistry::new(4, 1000);
    let permit = preadmit(&registry, &admitted);
    let (handle, controls) = voice_control_pair(&admitted, &registry, &permit).unwrap();
    let (runner, _requests) = Runner::new("answer");
    let cfg = wiring(
        &rig,
        runner.clone(),
        registry.clone(),
        Some(history.clone()),
    );
    let report = bounded(async {
        let call = run_public_voice_call_controlled(admitted, cfg, permit, controls);
        tokio::pin!(call);
        rig.input.send(transcript(&s, 1, 1, "q", false)).unwrap();
        let output = as_text(
            before_call_closes(&mut call, rig.entered_submit.recv())
                .await
                .unwrap(),
        );
        let mut unknown = output.clone();
        unknown.output_seq = 99;
        rig.input.send(cleared(&s, &unknown)).unwrap();
        before_call_closes(
            &mut call,
            observed_playback(&mut rig.delivered, 1, PlaybackState::Cleared),
        )
        .await;
        assert!(rig
            .peer
            .submit_cancel
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .is_cancelled());
        // Poll the owned coordinator through a short deadline: cleanup cannot
        // masquerade as a local epoch cancellation or silently release its slot.
        assert_call_pending(&mut call).await;
        assert!(lease.check(&CancellationToken::new()).is_ok());
        assert_eq!(handle.status().phase, ControlPhase::Running);
        assert_eq!(registry.active_count(), 1);
        assert!(matches!(registry.try_admit(&ctx), PublicAdmission::Busy));
        rig.peer.send_gate.notify_one();
        rig.input
            .send(playback(&s, &output, PlayedExtent::Exact { chars: 6 }))
            .unwrap();
        before_call_closes(
            &mut call,
            observed_playback(&mut rig.delivered, 1, PlaybackState::Played),
        )
        .await;
        assert_call_pending(&mut call).await;
        assert!(lease.check(&CancellationToken::new()).is_ok());
        assert_eq!(handle.status().phase, ControlPhase::Running);
        assert!(matches!(registry.try_admit(&ctx), PublicAdmission::Busy));
        assert!(history.load(&key, chrono::Utc::now()).unwrap().is_empty());
        assert!(rig.peer.accepted_log.lock().unwrap().is_empty());
        assert!(rig.accepted.try_recv().is_err());
        rig.input.send(hangup(&s)).unwrap();
        call.await
    })
    .await;
    assert_eq!(report.termination, VoiceCallTermination::Hangup);
    assert_eq!(report.turns_started, 1);
    assert_eq!(runner.calls.load(Ordering::Acquire), 1);
    assert_eq!(report.frames_accepted, 0);
    assert_eq!(report.history_appended, 0);
    assert!(history.load(&key, chrono::Utc::now()).unwrap().is_empty());
    assert!(rig.peer.accepted_log.lock().unwrap().is_empty());
    assert!(rig.accepted.try_recv().is_err());
    assert_eq!(registry.active_count(), 0);
    assert_eq!(handle.status().phase, ControlPhase::Closed);
}

#[tokio::test]
async fn noncontrolled_unknown_cleared_cancels_pending_send_without_history_or_acceptance() {
    let mut rig = Rig::new().await;
    rig.peer.block_send.store(true, Ordering::Release);
    let admitted = rig.admit().await;
    let lease = admitted.lease().clone();
    let ctx = admitted.context().clone();
    let s = lease.session().clone();
    let history = store();
    let key = PublicHistorySession::from_provenance(ctx.provenance());
    let registry = PublicRunRegistry::new(4, 1000);
    let (runner, _requests) = Runner::new("answer");
    let cfg = wiring(
        &rig,
        runner.clone(),
        registry.clone(),
        Some(history.clone()),
    );
    let report = bounded(async {
        let call = run_public_voice_call(admitted, cfg);
        tokio::pin!(call);
        rig.input.send(transcript(&s, 1, 1, "q", false)).unwrap();
        let output = as_text(
            before_call_closes(&mut call, rig.entered_submit.recv())
                .await
                .unwrap(),
        );
        let mut unknown = output.clone();
        unknown.output_seq = 99;
        rig.input.send(cleared(&s, &unknown)).unwrap();
        before_call_closes(
            &mut call,
            observed_playback(&mut rig.delivered, 1, PlaybackState::Cleared),
        )
        .await;
        assert!(rig
            .peer
            .submit_cancel
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .is_cancelled());
        assert_call_pending(&mut call).await;
        assert!(lease.check(&CancellationToken::new()).is_ok());
        assert_eq!(registry.active_count(), 1);
        assert!(matches!(registry.try_admit(&ctx), PublicAdmission::Busy));
        rig.peer.send_gate.notify_one();
        rig.input
            .send(playback(&s, &output, PlayedExtent::Exact { chars: 6 }))
            .unwrap();
        before_call_closes(
            &mut call,
            observed_playback(&mut rig.delivered, 1, PlaybackState::Played),
        )
        .await;
        assert_call_pending(&mut call).await;
        assert!(lease.check(&CancellationToken::new()).is_ok());
        assert!(matches!(registry.try_admit(&ctx), PublicAdmission::Busy));
        assert!(history.load(&key, chrono::Utc::now()).unwrap().is_empty());
        assert!(rig.peer.accepted_log.lock().unwrap().is_empty());
        assert!(rig.accepted.try_recv().is_err());
        rig.input.send(hangup(&s)).unwrap();
        call.await
    })
    .await;
    assert_eq!(report.termination, VoiceCallTermination::Hangup);
    assert_eq!(report.turns_started, 1);
    assert_eq!(runner.calls.load(Ordering::Acquire), 1);
    assert_eq!(report.frames_accepted, 0);
    assert_eq!(report.history_appended, 0);
    assert!(history.load(&key, chrono::Utc::now()).unwrap().is_empty());
    assert!(rig.peer.accepted_log.lock().unwrap().is_empty());
    assert!(rig.accepted.try_recv().is_err());
    assert_eq!(registry.active_count(), 0);
}

#[tokio::test]
async fn unknown_cleared_drops_pending_provider_before_late_effect() {
    let mut rig = Rig::new().await;
    let admitted = rig.admit().await;
    let lease = admitted.lease().clone();
    let ctx = admitted.context().clone();
    let s = lease.session().clone();
    let history = store();
    let key = PublicHistorySession::from_provenance(ctx.provenance());
    let registry = PublicRunRegistry::new(4, 1000);
    let permit = preadmit(&registry, &admitted);
    let identity = permit.validated_identity(&registry, &ctx).unwrap();
    let (handle, controls) = voice_control_pair(&admitted, &registry, &permit).unwrap();
    let (runner, mut requests) = Runner::new("answer");
    runner.block.store(true, Ordering::Release);
    let cfg = wiring(
        &rig,
        runner.clone(),
        registry.clone(),
        Some(history.clone()),
    );
    let report = bounded(async {
        let call = run_public_voice_call_controlled(admitted, cfg, permit, controls);
        tokio::pin!(call);
        rig.input.send(transcript(&s, 1, 1, "q", false)).unwrap();
        before_call_closes(&mut call, requests.recv())
            .await
            .unwrap();
        assert_eq!(runner.drops.load(Ordering::Acquire), 0);
        let VoicePeerEvent::Envelope(turn_event) = transcript(&s, 1, 99, "", false) else {
            unreachable!()
        };
        let TelecomKind::CallTurn(unknown) = turn_event.kind else {
            unreachable!()
        };
        rig.input.send(cleared(&s, &unknown)).unwrap();
        before_call_closes(
            &mut call,
            observed_playback(&mut rig.delivered, 1, PlaybackState::Cleared),
        )
        .await;
        assert_call_pending(&mut call).await;
        assert_eq!(
            runner.drops.load(Ordering::Acquire),
            1,
            "owned provider future must actually drop"
        );
        assert!(runner
            .cancelled_token
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .is_cancelled());
        assert!(lease.check(&CancellationToken::new()).is_ok());
        assert_eq!(handle.status().phase, ControlPhase::Running);
        assert_eq!(registry.active_count(), 1);
        // Unaccounted provider spend still exhausts the fallback cap. The exact
        // live identity proves the binding is retained even when admission is
        // denied first by OverSpend, rather than by Busy.
        assert!(identity.is_current());
        assert!(matches!(
            registry.try_admit(&ctx),
            PublicAdmission::OverSpend
        ));
        runner.gate.notify_one();
        assert_call_pending(&mut call).await;
        assert_eq!(runner.late_effects.load(Ordering::Acquire), 0);
        assert_eq!(runner.drops.load(Ordering::Acquire), 1);
        assert!(lease.check(&CancellationToken::new()).is_ok());
        assert!(identity.is_current());
        assert!(rig.entered_submit.try_recv().is_err());
        assert!(rig.accepted.try_recv().is_err());
        assert!(rig.peer.accepted_log.lock().unwrap().is_empty());
        assert!(history.load(&key, chrono::Utc::now()).unwrap().is_empty());
        rig.input.send(hangup(&s)).unwrap();
        call.await
    })
    .await;
    assert_eq!(report.termination, VoiceCallTermination::Hangup);
    assert_eq!(runner.calls.load(Ordering::Acquire), 1);
    assert_eq!(runner.drops.load(Ordering::Acquire), 1);
    assert_eq!(runner.late_effects.load(Ordering::Acquire), 0);
    assert_eq!(report.frames_accepted, 0);
    assert_eq!(report.history_appended, 0);
    assert_eq!(registry.active_count(), 0);
    assert!(!identity.is_current());
    assert_eq!(handle.status().phase, ControlPhase::Closed);
    assert!(rig.entered_submit.try_recv().is_err());
    assert!(history.load(&key, chrono::Utc::now()).unwrap().is_empty());
}

#[tokio::test]
async fn invalid_playback_sequence_is_refused_by_coordinator() {
    for (state, sequence) in [(PlaybackState::Cleared, 0), (PlaybackState::Played, 99)] {
        let mut rig = Rig::new().await;
        rig.peer.block_send.store(true, Ordering::Release);
        let admitted = rig.admit().await;
        let s = admitted.lease().session().clone();
        let (runner, _requests) = Runner::new("answer");
        let registry = PublicRunRegistry::new(4, 1000);
        let cfg = wiring(&rig, runner, registry.clone(), None);
        let report = bounded(async {
            let call = run_public_voice_call(admitted, cfg);
            tokio::pin!(call);
            rig.input.send(transcript(&s, 1, 1, "q", false)).unwrap();
            let output = as_text(
                before_call_closes(&mut call, rig.entered_submit.recv())
                    .await
                    .unwrap(),
            );
            let VoicePeerEvent::Envelope(mut event) = cleared(&s, &output) else {
                unreachable!()
            };
            let TelecomKind::PlaybackEvent(fact) = &mut event.kind else {
                unreachable!()
            };
            fact.state = state;
            fact.output_seq = sequence;
            rig.input.send(VoicePeerEvent::Envelope(event)).unwrap();
            call.await
        })
        .await;
        assert_eq!(report.termination, VoiceCallTermination::InvalidEvent);
        assert_eq!(report.frames_accepted, 0);
        assert_eq!(report.history_appended, 0);
        assert!(rig.peer.accepted_log.lock().unwrap().is_empty());
        assert_eq!(registry.active_count(), 0);
    }
}

#[tokio::test]
async fn stale_cleared_does_not_cancel_new_epoch_submission() {
    let mut rig = Rig::new().await;
    let admitted = rig.admit().await;
    let s = admitted.lease().session().clone();
    let history = store();
    let key = PublicHistorySession::from_provenance(admitted.context().provenance());
    let (runner, _requests) = Runner::new("answer");
    let cfg = wiring(
        &rig,
        runner,
        PublicRunRegistry::new(4, 1000),
        Some(history.clone()),
    );
    let (report, ()) = bounded(async {
        tokio::join!(run_public_voice_call(admitted, cfg), async {
            rig.input.send(transcript(&s, 1, 1, "old", false)).unwrap();
            let old = as_text(rig.accepted.recv().await.unwrap());
            rig.peer.block_send.store(true, Ordering::Release);
            rig.input.send(transcript(&s, 2, 1, "new", false)).unwrap();
            let new = loop {
                if let CallCommand::SendText(output) = rig.entered_submit.recv().await.unwrap() {
                    if output.output_epoch == 2 {
                        break output;
                    }
                }
            };
            rig.input.send(cleared(&s, &old)).unwrap();
            observed_playback(&mut rig.delivered, 1, PlaybackState::Cleared).await;
            assert!(!rig
                .peer
                .submit_cancel
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .is_cancelled());
            rig.peer.send_gate.notify_one();
            assert!(matches!(
                rig.accepted.recv().await.unwrap(),
                CallCommand::CancelOutput {
                    output_epoch: 1,
                    ..
                }
            ));
            assert_eq!(as_text(rig.accepted.recv().await.unwrap()), new);
            rig.input
                .send(playback(&s, &new, PlayedExtent::Exact { chars: 3 }))
                .unwrap();
            rig.input.send(hangup(&s)).unwrap();
        })
    })
    .await;
    assert_eq!(report.termination, VoiceCallTermination::Hangup);
    assert_eq!(report.frames_accepted, 2);
    assert_eq!(report.history_appended, 1);
    assert_eq!(
        history.load(&key, chrono::Utc::now()).unwrap(),
        vec![PublicHistoryTurn {
            user: "new".into(),
            assistant: "ans".into()
        }]
    );
}

#[tokio::test]
async fn peer_final_poll_binding_stop_synchronously_cancels_send_child_before_acceptance() {
    let mut rig = Rig::new().await;
    rig.peer.block_send.store(true, Ordering::Release);
    let admitted = rig.admit().await;
    let s = admitted.lease().session().clone();
    let history = store();
    let key = PublicHistorySession::from_provenance(admitted.context().provenance());
    let registry = PublicRunRegistry::new(4, 1000);
    let (runner, _requests) = Runner::new("answer");
    let cfg = wiring(&rig, runner, registry.clone(), Some(history.clone()));
    let (report, ()) = bounded(async {
        tokio::join!(run_public_voice_call(admitted, cfg), async {
            rig.input.send(transcript(&s, 1, 1, "q", false)).unwrap();
            let output = as_text(rig.entered_submit.recv().await.unwrap());
            rig.input
                .send(playback(&s, &output, PlayedExtent::Exact { chars: 3 }))
                .unwrap();
            observed_playback(&mut rig.delivered, 1, PlaybackState::Played).await;
            *rig.peer.cancel_on_final_poll.lock().unwrap() = Some(registry.clone());
            // Stop happens inside the resumed peer future, after the outer
            // select already polled its whole-call cancellation branch.
            rig.peer.send_gate.notify_one();
        })
    })
    .await;
    assert!(rig.peer.cancelled_at_final_check.load(Ordering::Acquire));
    assert_eq!(report.termination, VoiceCallTermination::SubmissionFailed);
    assert_eq!(report.frames_accepted, 0);
    assert_eq!(report.history_appended, 0);
    assert!(rig.peer.accepted_log.lock().unwrap().is_empty());
    assert!(history.load(&key, chrono::Utc::now()).unwrap().is_empty());
    assert_eq!(registry.active_count(), 0);
}

#[tokio::test]
async fn provider_final_poll_binding_stop_synchronously_cancels_run_child() {
    let rig = Rig::new().await;
    let admitted = rig.admit().await;
    let s = admitted.lease().session().clone();
    let registry = PublicRunRegistry::new(4, 1000);
    let history = store();
    let key = PublicHistorySession::from_provenance(admitted.context().provenance());
    let (runner, mut requests) = Runner::new("must not send");
    runner.block.store(true, Ordering::Release);
    let cfg = wiring(
        &rig,
        runner.clone(),
        registry.clone(),
        Some(history.clone()),
    );
    let (report, ()) = bounded(async {
        tokio::join!(run_public_voice_call(admitted, cfg), async {
            rig.input.send(transcript(&s, 1, 1, "q", false)).unwrap();
            let request = requests.recv().await.unwrap();
            assert!(!request.allow_private_lookup);
            *runner.cancel_on_return.lock().unwrap() = Some(registry.clone());
            runner.gate.notify_one();
        })
    })
    .await;
    assert!(runner.cancelled_on_return.load(Ordering::Acquire));
    assert_eq!(report.termination, VoiceCallTermination::Revoked);
    assert_eq!(report.frames_accepted, 0);
    assert_eq!(report.history_appended, 0);
    assert_eq!(registry.tokens_spent(), 10);
    assert_eq!(registry.active_count(), 0);
    assert!(rig.peer.accepted_log.lock().unwrap().is_empty());
    assert!(history.load(&key, chrono::Utc::now()).unwrap().is_empty());
}

#[tokio::test]
async fn prior_31_reserves_last_combined_turn_and_refuses_next_without_losing_heard_history() {
    let mut rig = Rig::new().await;
    let admitted = rig.admit().await;
    let s = admitted.lease().session().clone();
    let history = store();
    let key = PublicHistorySession::from_provenance(admitted.context().provenance());
    let prior: Vec<_> = (0..31)
        .map(|n| PublicHistoryTurn {
            user: format!("prior-{n}"),
            assistant: "heard".into(),
        })
        .collect();
    for turn in &prior {
        history
            .append(&key, turn, chrono::Utc::now(), false)
            .unwrap();
    }
    assert_eq!(history.load(&key, chrono::Utc::now()).unwrap(), prior);
    let registry = PublicRunRegistry::new(4, 1000);
    let (runner, mut requests) = Runner::new("answer");
    let cfg = wiring(
        &rig,
        runner.clone(),
        registry.clone(),
        Some(history.clone()),
    );
    let (report, ()) = bounded(async {
        tokio::join!(run_public_voice_call(admitted, cfg), async {
            rig.input
                .send(transcript(&s, 1, 1, "last slot", false))
                .unwrap();
            let request = requests.recv().await.unwrap();
            assert_eq!(request.history, prior);
            assert!(!request.allow_private_lookup);
            let output = as_text(rig.accepted.recv().await.unwrap());
            rig.input
                .send(playback(&s, &output, PlayedExtent::Exact { chars: 3 }))
                .unwrap();
            rig.input
                .send(transcript(&s, 2, 1, "no slot", false))
                .unwrap();
        })
    })
    .await;
    assert_eq!(report.termination, VoiceCallTermination::LimitExceeded);
    assert_eq!(report.frames_accepted, 1);
    assert_eq!(report.history_appended, 1);
    assert!(report.history_error.is_none());
    assert_eq!(runner.calls.load(Ordering::Acquire), 1);
    assert_eq!(registry.tokens_spent(), 10);
    assert_eq!(registry.active_count(), 0);
    let mut expected = prior;
    expected.push(PublicHistoryTurn {
        user: "last slot".into(),
        assistant: "ans".into(),
    });
    assert_eq!(history.load(&key, chrono::Utc::now()).unwrap(), expected);
}

#[tokio::test]
async fn prior_32_refuses_before_provider_invocation_or_acceptance_and_preserves_history() {
    let rig = Rig::new().await;
    let admitted = rig.admit().await;
    let s = admitted.lease().session().clone();
    let history = store();
    let key = PublicHistorySession::from_provenance(admitted.context().provenance());
    let prior: Vec<_> = (0..32)
        .map(|n| PublicHistoryTurn {
            user: format!("prior-{n}"),
            assistant: "heard".into(),
        })
        .collect();
    for turn in &prior {
        history
            .append(&key, turn, chrono::Utc::now(), false)
            .unwrap();
    }
    assert_eq!(history.load(&key, chrono::Utc::now()).unwrap(), prior);
    let registry = PublicRunRegistry::new(4, 1000);
    let (runner, _requests) = Runner::new("must not invoke");
    rig.input
        .send(transcript(&s, 1, 1, "no slot", false))
        .unwrap();
    let report = bounded(run_public_voice_call(
        admitted,
        wiring(
            &rig,
            runner.clone(),
            registry.clone(),
            Some(history.clone()),
        ),
    ))
    .await;
    assert_eq!(report.termination, VoiceCallTermination::LimitExceeded);
    assert_eq!(report.frames_accepted, 0);
    assert_eq!(report.history_appended, 0);
    assert!(report.history_error.is_none());
    assert_eq!(runner.calls.load(Ordering::Acquire), 0);
    assert_eq!(registry.tokens_spent(), 0);
    assert_eq!(registry.active_count(), 0);
    assert!(rig.peer.accepted_log.lock().unwrap().is_empty());
    assert_eq!(history.load(&key, chrono::Utc::now()).unwrap(), prior);
}

#[tokio::test]
async fn replacement_installs_cancelled_ledger_on_final_next_state() {
    let rig = Rig::new().await;
    let admitted = rig.admit().await;
    let s = admitted.lease().session().clone();
    let mut state = CallState::new(s.clone()).unwrap();
    let VoicePeerEvent::Envelope(first) = transcript(&s, 1, 1, "old", false) else {
        unreachable!()
    };
    state.apply_wire(&first).unwrap();
    for epoch in [2, 3] {
        let VoicePeerEvent::Envelope(event) = transcript(&s, epoch, 1, "new", false) else {
            unreachable!()
        };
        let mut next = state.clone();
        assert!(matches!(
            next.apply_wire(&event).unwrap(),
            ApplyOutcome::TurnStarted { .. }
        ));
        coordinator::install_replacement(&mut state, next, epoch - 1);
        assert_eq!(state.active_turn.as_ref().unwrap().output_epoch, epoch);
        for old_epoch in 1..epoch {
            assert!(state.is_output_epoch_cancelled(old_epoch));
        }
        assert!(!state.is_output_epoch_cancelled(epoch));
    }
    let VoicePeerEvent::Envelope(event) = transcript(&s, 2, 1, "stale", false) else {
        unreachable!()
    };
    assert_eq!(
        state.apply_wire(&event).unwrap(),
        ApplyOutcome::Ignored(IgnoredEvent::StaleOutputEpoch)
    );
    assert!(state.is_output_epoch_cancelled(1));
    assert!(state.is_output_epoch_cancelled(2));
}

fn preadmit(registry: &Arc<PublicRunRegistry>, admitted: &AdmittedVoiceCall) -> PublicRunPermit {
    let PublicAdmission::Admitted(permit) = registry.try_admit(admitted.context()) else {
        panic!("server pre-admission");
    };
    permit
}

// Break caught: controlled entry accidentally re-admits the occupied binding.
#[tokio::test]
async fn controlled_preoccupied_permit_uses_same_driver_without_busy() {
    let mut rig = Rig::new().await;
    let admitted = rig.admit().await;
    let s = admitted.lease().session().clone();
    let registry = PublicRunRegistry::new(4, 1000);
    let permit = preadmit(&registry, &admitted);
    let (handle, controls) = voice_control_pair(&admitted, &registry, &permit).unwrap();
    assert!(matches!(
        registry.try_admit(admitted.context()),
        PublicAdmission::Busy
    ));
    let (runner, _requests) = Runner::new("answer");
    let cfg = wiring(&rig, runner, registry.clone(), None);
    let (report, ()) = bounded(async {
        tokio::join!(
            run_public_voice_call_controlled(admitted, cfg, permit, controls),
            async {
                rig.input.send(transcript(&s, 1, 1, "q", false)).unwrap();
                as_text(rig.accepted.recv().await.unwrap());
                assert_eq!(handle.status().phase, ControlPhase::Running);
                assert_eq!(handle.status().output_epoch, Some(1));
                rig.input.send(hangup(&s)).unwrap();
            }
        )
    })
    .await;
    assert_eq!(report.termination, VoiceCallTermination::Hangup);
    assert_eq!(report.frames_accepted, 1);
    assert_eq!(registry.active_count(), 0);
    assert_eq!(handle.status().phase, ControlPhase::Closed);
    assert_eq!(handle.hangup().status(), Some(ControlCompletion::Refused));
}

// Break caught: a supplied permit skips registry/digest/binding/current checks.
#[tokio::test]
async fn controlled_foreign_wrong_caller_binding_and_cancelled_permits_are_refused() {
    for fault in 0..4 {
        let mut rig = Rig::new().await;
        let admitted = rig.admit().await;
        rig.input
            .send(transcript(
                admitted.lease().session(),
                1,
                1,
                "must not be read",
                false,
            ))
            .unwrap();
        let registry = PublicRunRegistry::new(4, 1000);
        let foreign = PublicRunRegistry::new(4, 1000);
        let source = if fault == 0 { &foreign } else { &registry };
        let ctx = match fault {
            1 => sample_public_context_for_peer("other-binding", "+15550000001"),
            2 => sample_public_context_for_peer("bind", "+15550000002"),
            _ => admitted.context().clone(),
        };
        let PublicAdmission::Admitted(permit) = source.try_admit(&ctx) else {
            panic!("admit")
        };
        if fault == 3 {
            assert!(registry.cancel_binding("bind"));
        }
        assert!(voice_control_pair(&admitted, &registry, &permit).is_err());
        let history_dir = tempfile::tempdir().unwrap();
        let path = history_dir.path().join("denied-history.db");
        let key = PublicHistorySession::from_provenance(admitted.context().provenance());
        let correct = PublicHistoryStore::open(
            &path,
            SecretsCrypto::new("correct-key"),
            PublicHistoryConfig::default(),
        )
        .unwrap();
        correct
            .append(
                &key,
                &PublicHistoryTurn {
                    user: "prior".into(),
                    assistant: "heard".into(),
                },
                chrono::Utc::now(),
                false,
            )
            .unwrap();
        let unreadable = Arc::new(
            PublicHistoryStore::open(
                &path,
                SecretsCrypto::new("wrong-key"),
                PublicHistoryConfig::default(),
            )
            .unwrap(),
        );
        assert!(unreadable.load(&key, chrono::Utc::now()).is_err());
        let (runner, _requests) = Runner::new("never invoke");
        // A valid control receiver cannot rescue an invalid supplied permit.
        // An early history read would fail with HistoryUnavailable instead.
        let other = Rig::new().await;
        let other_call = other.admit().await;
        let other_registry = PublicRunRegistry::new(4, 1000);
        let other_permit = preadmit(&other_registry, &other_call);
        let (_, controls) =
            voice_control_pair(&other_call, &other_registry, &other_permit).unwrap();
        let report = bounded(run_public_voice_call_controlled(
            admitted,
            wiring(&rig, runner.clone(), registry.clone(), Some(unreadable)),
            permit,
            controls,
        ))
        .await;
        assert_eq!(report.termination, VoiceCallTermination::ControlRefused);
        assert_eq!(runner.calls.load(Ordering::Acquire), 0);
        assert!(rig.peer.accepted_log.lock().unwrap().is_empty());
        assert_eq!(report.history_appended, 0);
        assert!(rig.entered_submit.try_recv().is_err());
        assert!(rig.delivered.try_recv().is_err());
        assert_eq!(source.active_count(), 0);
    }
}

// Break caught: equal binding/caller lets another provenance mint voice control.
#[tokio::test]
async fn controlled_same_binding_caller_but_other_provenance_is_refused_before_work() {
    for fault in 0..3 {
        let mut rig = Rig::new().await;
        let admitted = rig.admit().await;
        rig.input
            .send(transcript(
                admitted.lease().session(),
                1,
                1,
                "must not be read",
                false,
            ))
            .unwrap();
        let other_relay = VoiceRelayTransport::new(rig.peer.clone());
        other_relay
            .connect(
                VoiceBindingPolicy::new(
                    if fault == 1 { "other-region" } else { "region" }.into(),
                    "bind".into(),
                    "acct".into(),
                    "+15550000000".into(),
                    "+15550000001".into(),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        if fault == 2 {
            other_relay.connect(policy()).await.unwrap();
        }
        let other_call = other_relay.admit_call("provider-call").await.unwrap();
        let ctx = if fault == 0 {
            TelecomPublicContext::from_admitted_sms(&crate::gateway::telecom::SmsInboundMeta {
                region: "region".into(),
                binding_ref: "bind".into(),
                provider_account_id: "acct".into(),
                provider_message_id: "pm".into(),
                envelope_event_id: "ev".into(),
                connection_generation: 1,
                service_address: "+15550000000".into(),
                peer_address: "+15550000001".into(),
                encoding: aleph_protocol::telecom_relay::SmsEncoding::Gsm7,
                segment_count: 1,
                created_at: chrono::Utc::now(),
                expires_at: chrono::Utc::now(),
            })
        } else {
            other_call.context().clone()
        };
        let expected = admitted.context().provenance();
        let other = ctx.provenance();
        assert_eq!(other.binding_ref(), expected.binding_ref());
        assert_eq!(other.caller_digest(), expected.caller_digest());
        assert_eq!(other.channel() == expected.channel(), fault != 0);
        assert_eq!(other.region() == expected.region(), fault != 1);
        assert_eq!(
            other.connection_generation() == expected.connection_generation(),
            fault != 2
        );
        let registry = PublicRunRegistry::new(4, 1000);
        let PublicAdmission::Admitted(permit) = registry.try_admit(&ctx) else {
            panic!("admit")
        };
        assert!(permit.validated_identity(&registry, &ctx).is_some());
        assert!(permit
            .validated_identity(&registry, admitted.context())
            .is_none());
        assert!(voice_control_pair(&admitted, &registry, &permit).is_err());
        assert!(!permit.cancel_token().is_cancelled());

        let history_dir = tempfile::tempdir().unwrap();
        let path = history_dir.path().join("denied-provenance-history.db");
        let key = PublicHistorySession::from_provenance(expected);
        let correct = PublicHistoryStore::open(
            &path,
            SecretsCrypto::new("correct-key"),
            PublicHistoryConfig::default(),
        )
        .unwrap();
        correct
            .append(
                &key,
                &PublicHistoryTurn {
                    user: "prior".into(),
                    assistant: "heard".into(),
                },
                chrono::Utc::now(),
                false,
            )
            .unwrap();
        let unreadable = Arc::new(
            PublicHistoryStore::open(
                &path,
                SecretsCrypto::new("wrong-key"),
                PublicHistoryConfig::default(),
            )
            .unwrap(),
        );
        assert!(unreadable.load(&key, chrono::Utc::now()).is_err());
        let other_registry = PublicRunRegistry::new(4, 1000);
        let other_permit = preadmit(&other_registry, &other_call);
        let (_, controls) =
            voice_control_pair(&other_call, &other_registry, &other_permit).unwrap();
        let (runner, _requests) = Runner::new("never invoke");
        let report = bounded(run_public_voice_call_controlled(
            admitted,
            wiring(&rig, runner.clone(), registry.clone(), Some(unreadable)),
            permit,
            controls,
        ))
        .await;
        assert_eq!(report.termination, VoiceCallTermination::ControlRefused);
        assert_eq!(runner.calls.load(Ordering::Acquire), 0);
        assert_eq!(report.history_appended, 0);
        assert_eq!(report.frames_accepted, 0);
        assert!(rig.peer.accepted_log.lock().unwrap().is_empty());
        assert!(rig.entered_submit.try_recv().is_err());
        assert!(rig.delivered.try_recv().is_err());
        assert_eq!(registry.active_count(), 0);
    }
}

// Break caught: same metadata grants a receiver authority over another run/call.
#[tokio::test]
async fn control_pair_is_exact_run_and_generation_and_drop_revokes_it() {
    let rig = Rig::new().await;
    let admitted = rig.admit().await;
    let registry = PublicRunRegistry::new(4, 1000);
    let permit = preadmit(&registry, &admitted);
    let (old, controls) = voice_control_pair(&admitted, &registry, &permit).unwrap();
    drop(permit);
    let permit = preadmit(&registry, &admitted);
    let (runner, _requests) = Runner::new("never invoke");
    let report = bounded(run_public_voice_call_controlled(
        admitted,
        wiring(&rig, runner.clone(), registry.clone(), None),
        permit,
        controls,
    ))
    .await;
    assert_eq!(report.termination, VoiceCallTermination::ControlRefused);
    assert_eq!(runner.calls.load(Ordering::Acquire), 0);
    assert_eq!(old.hangup().status(), Some(ControlCompletion::Refused));
    assert_eq!(registry.active_count(), 0);

    rig.relay.connect(policy()).await.unwrap();
    let admitted = rig.admit().await;
    let permit = preadmit(&registry, &admitted);
    let (handle, receiver) = voice_control_pair(&admitted, &registry, &permit).unwrap();
    drop(receiver);
    assert_eq!(handle.status().phase, ControlPhase::Closed);
    assert_eq!(handle.hangup().status(), Some(ControlCompletion::Refused));
    drop(permit);
    drop(admitted);
    rig.relay.connect(policy()).await.unwrap();
    assert_eq!(handle.hangup().status(), Some(ControlCompletion::Refused));
    assert!(rig.peer.accepted_log.lock().unwrap().is_empty());
}

// Break caught: control cancellation reaches only the outer select, not a final
// inner peer/provider poll. Dropping the receipt must not retract local intent.
#[tokio::test]
async fn control_final_poll_cancel_blocks_send_and_provider_without_cancelling_call() {
    for provider in [false, true] {
        let mut rig = Rig::new().await;
        rig.peer.block_send.store(!provider, Ordering::Release);
        let admitted = rig.admit().await;
        let s = admitted.lease().session().clone();
        let registry = PublicRunRegistry::new(4, 1000);
        let permit = preadmit(&registry, &admitted);
        let parent = permit.cancel_token();
        let (handle, controls) = voice_control_pair(&admitted, &registry, &permit).unwrap();
        let (runner, mut requests) = Runner::new("must not send");
        runner.block.store(provider, Ordering::Release);
        let history = store();
        let key = PublicHistorySession::from_provenance(admitted.context().provenance());
        let cfg = wiring(
            &rig,
            runner.clone(),
            registry.clone(),
            Some(history.clone()),
        );
        let (report, ()) = bounded(async {
            tokio::join!(
                run_public_voice_call_controlled(admitted, cfg, permit, controls),
                async {
                    rig.input.send(transcript(&s, 1, 1, "q", false)).unwrap();
                    requests.recv().await.unwrap();
                    if provider {
                        *runner.control_on_return.lock().unwrap() = Some((handle.clone(), 1));
                        runner.gate.notify_one();
                    } else {
                        let output = as_text(rig.entered_submit.recv().await.unwrap());
                        rig.input
                            .send(playback(&s, &output, PlayedExtent::Exact { chars: 2 }))
                            .unwrap();
                        observed_playback(&mut rig.delivered, 1, PlaybackState::Played).await;
                        *rig.peer.control_on_final_poll.lock().unwrap() = Some((handle.clone(), 1));
                        rig.peer.send_gate.notify_one();
                    }
                    assert!(matches!(
                        rig.accepted.recv().await.unwrap(),
                        CallCommand::CancelOutput {
                            output_epoch: 1,
                            ..
                        }
                    ));
                    assert!(
                        !parent.is_cancelled(),
                        "output stop is not run cancellation"
                    );
                    assert_eq!(handle.status().phase, ControlPhase::Running);
                    rig.input.send(hangup(&s)).unwrap();
                }
            )
        })
        .await;
        assert!(if provider {
            runner.cancelled_on_return.load(Ordering::Acquire)
        } else {
            rig.peer.cancelled_at_final_check.load(Ordering::Acquire)
        });
        assert_eq!(report.termination, VoiceCallTermination::Hangup);
        assert_eq!(report.frames_accepted, 0);
        assert_eq!(report.history_appended, 0);
        assert!(history.load(&key, chrono::Utc::now()).unwrap().is_empty());
        assert_eq!(registry.active_count(), 0);
    }
}

// Break caught: a genuinely blocked provider must be dropped locally without
// waiting for it to return, even though unknown-spend accounting stays intact.
#[tokio::test]
async fn controlled_cancel_drops_blocked_provider_without_waiting_or_cancelling_permit() {
    let mut rig = Rig::new().await;
    let admitted = rig.admit().await;
    let s = admitted.lease().session().clone();
    let registry = PublicRunRegistry::new(4, 1000);
    let permit = preadmit(&registry, &admitted);
    let parent = permit.cancel_token();
    let (handle, controls) = voice_control_pair(&admitted, &registry, &permit).unwrap();
    let (runner, mut requests) = Runner::new("blocked answer");
    runner.block.store(true, Ordering::Release);
    let cfg = wiring(&rig, runner.clone(), registry.clone(), None);
    let (report, ()) = bounded(async {
        tokio::join!(
            run_public_voice_call_controlled(admitted, cfg, permit, controls),
            async {
                rig.input.send(transcript(&s, 1, 1, "q", false)).unwrap();
                requests.recv().await.unwrap();
                let mut receipt = handle.cancel_output(1);
                assert!(runner
                    .cancelled_token
                    .lock()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .is_cancelled());
                assert_eq!(
                    receipt.local_applied().await,
                    ControlCompletion::LocalApplied
                );
                assert_eq!(receipt.completed().await, ControlCompletion::Accepted);
                assert!(matches!(
                    rig.accepted.recv().await.unwrap(),
                    CallCommand::CancelOutput {
                        output_epoch: 1,
                        ..
                    }
                ));
                assert!(!parent.is_cancelled());
                assert_eq!(
                    registry.tokens_spent(),
                    1000,
                    "preserve unknown-cost fail-closed accounting"
                );
                runner.gate.notify_one(); // no future remains to resume
                rig.input.send(hangup(&s)).unwrap();
            }
        )
    })
    .await;
    assert_eq!(report.termination, VoiceCallTermination::Hangup);
    assert_eq!(report.frames_accepted, 0);
    assert_eq!(report.history_appended, 0);
    assert_eq!(runner.calls.load(Ordering::Acquire), 1);
    assert_eq!(registry.active_count(), 0);
}

// Break caught: merely enqueueing cancellation acknowledges an effect, or a
// newer generated buffer escapes before the carrier cancellation ACK.
#[tokio::test]
async fn controlled_cancel_clears_buffer_dedups_and_gates_next_output_on_remote_ack() {
    let mut rig = Rig::new().await;
    rig.peer.block_send.store(true, Ordering::Release);
    rig.peer.block_control.store(true, Ordering::Release);
    let admitted = rig.admit().await;
    let s = admitted.lease().session().clone();
    let registry = PublicRunRegistry::new(4, 1000);
    let permit = preadmit(&registry, &admitted);
    let parent = permit.cancel_token();
    let (handle, controls) = voice_control_pair(&admitted, &registry, &permit).unwrap();
    let (runner, mut requests) = Runner::new("answer");
    let cfg = wiring(&rig, runner, registry.clone(), None);
    let (report, ()) = bounded(async {
        tokio::join!(
            run_public_voice_call_controlled(admitted, cfg, permit, controls),
            async {
                rig.input.send(transcript(&s, 1, 1, "q", false)).unwrap();
                requests.recv().await.unwrap();
                as_text(rig.entered_submit.recv().await.unwrap());
                let mut receipt = handle.cancel_output(1);
                assert_eq!(receipt.status(), None, "queueing is not a reducer ACK");
                assert!(rig
                    .peer
                    .submit_cancel
                    .lock()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .is_cancelled());
                assert!(!parent.is_cancelled());
                for _ in 0..200 {
                    assert_eq!(
                        handle.cancel_output(999).status(),
                        Some(ControlCompletion::Refused)
                    );
                    assert_eq!(handle.cancel_output(1).status(), None);
                }
                assert_eq!(
                    receipt.local_applied().await,
                    ControlCompletion::LocalApplied
                );
                assert!(matches!(
                    rig.entered_submit.recv().await.unwrap(),
                    CallCommand::CancelOutput {
                        output_epoch: 1,
                        ..
                    }
                ));
                assert_eq!(receipt.status(), Some(ControlCompletion::LocalApplied));
                rig.peer.block_send.store(false, Ordering::Release);
                rig.input.send(transcript(&s, 2, 1, "new", false)).unwrap();
                requests.recv().await.unwrap();
                // The completed new run is still buffered while old cancel blocks.
                assert!(rig.peer.accepted_log.lock().unwrap().is_empty());
                assert_eq!(
                    handle.cancel_output(1).status(),
                    Some(ControlCompletion::Refused)
                );
                let mut buffered = handle.cancel_output(2);
                assert_eq!(
                    buffered.local_applied().await,
                    ControlCompletion::LocalApplied
                );
                rig.peer.control_gate.notify_one();
                assert_eq!(receipt.completed().await, ControlCompletion::Accepted);
                assert!(matches!(
                    rig.accepted.recv().await.unwrap(),
                    CallCommand::CancelOutput {
                        output_epoch: 1,
                        ..
                    }
                ));
                assert!(matches!(
                    rig.entered_submit.recv().await.unwrap(),
                    CallCommand::CancelOutput {
                        output_epoch: 2,
                        ..
                    }
                ));
                rig.input
                    .send(transcript(&s, 3, 1, "allowed after ACK", false))
                    .unwrap();
                requests.recv().await.unwrap();
                assert_eq!(rig.peer.accepted_log.lock().unwrap().len(), 1);
                rig.peer.control_gate.notify_one();
                assert_eq!(buffered.completed().await, ControlCompletion::Accepted);
                assert!(matches!(
                    rig.accepted.recv().await.unwrap(),
                    CallCommand::CancelOutput {
                        output_epoch: 2,
                        ..
                    }
                ));
                let new = as_text(rig.accepted.recv().await.unwrap());
                assert_eq!(new.output_epoch, 3);
                rig.input.send(hangup(&s)).unwrap();
            }
        )
    })
    .await;
    assert_eq!(report.termination, VoiceCallTermination::Hangup);
    assert_eq!(report.frames_accepted, 1);
    assert_eq!(
        rig.peer.accepted_log.lock().unwrap().len(),
        3,
        "two cancels, one new output; never replay blocked send or cancelled buffer"
    );
    assert_eq!(registry.active_count(), 0);
}

// Break caught: tool await owns Hangup, or local hangup cancels the independent
// carrier future, releases the permit early, or implies a remote ACK.
#[tokio::test]
async fn controlled_hangup_outlives_tool_await_and_preserves_only_accepted_heard() {
    for fail in [false, true] {
        let mut rig = Rig::new().await;
        let admitted = rig.admit().await;
        let lease = admitted.lease().clone();
        let s = lease.session().clone();
        let registry = PublicRunRegistry::new(4, 1000);
        let permit = preadmit(&registry, &admitted);
        let parent = permit.cancel_token();
        let (handle, controls) = voice_control_pair(&admitted, &registry, &permit).unwrap();
        let (runner, _requests) = Runner::new("🙂 tail");
        let history = store();
        let key = PublicHistorySession::from_provenance(admitted.context().provenance());
        let cfg = wiring(&rig, runner, registry.clone(), Some(history.clone()));
        let (report, ()) = bounded(async {
            tokio::join!(run_public_voice_call_controlled(admitted, cfg, permit, controls), async {
                rig.input.send(transcript(&s, 1, 1, "q", false)).unwrap();
                let output = as_text(rig.accepted.recv().await.unwrap());
                rig.input.send(playback(&s, &output, PlayedExtent::Exact { chars: 2 })).unwrap();
                observed_playback(&mut rig.delivered, 1, PlaybackState::Played).await;
                rig.peer.block_send.store(true, Ordering::Release);
                rig.input.send(transcript(&s, 2, 1, "unheard", false)).unwrap();
                loop { if matches!(rig.entered_submit.recv().await.unwrap(), CallCommand::SendText(t) if t.output_epoch == 2) { break; } }
                rig.peer.block_control.store(true, Ordering::Release);
                let mut receipt = handle.hangup();
                assert!(rig.peer.submit_cancel.lock().unwrap().as_ref().unwrap().is_cancelled());
                let mut abandoned = handle.hangup();
                let mut tool_await = Box::pin(abandoned.completed());
                assert!(matches!(futures::poll!(&mut tool_await), std::task::Poll::Pending));
                drop(tool_await);
                drop(abandoned);
                assert_eq!(receipt.local_applied().await, ControlCompletion::LocalApplied);
                loop { if matches!(rig.entered_submit.recv().await.unwrap(), CallCommand::Hangup { .. }) { break; } }
                assert_eq!(handle.status().phase, ControlPhase::Closing);
                assert_eq!(receipt.status(), Some(ControlCompletion::LocalApplied));
                assert_eq!(registry.active_count(), 1);
                assert!(lease.is_active(), "remote future needs the exact live lease until finalization");
                assert!(!parent.is_cancelled());
                assert!(history.load(&key, chrono::Utc::now()).unwrap().is_empty());
                rig.peer.fail_submit.store(fail, Ordering::Release);
                rig.peer.control_gate.notify_one();
                assert_eq!(receipt.completed().await, if fail { ControlCompletion::Unknown } else { ControlCompletion::Accepted });
            })
        }).await;
        assert_eq!(report.termination, VoiceCallTermination::Hangup);
        assert_eq!(
            report.remote_hangup,
            Some(if fail {
                ControlCompletion::Unknown
            } else {
                ControlCompletion::Accepted
            })
        );
        assert_eq!(report.frames_accepted, 1);
        assert_eq!(report.history_appended, 1);
        assert_eq!(
            history.load(&key, chrono::Utc::now()).unwrap(),
            vec![PublicHistoryTurn {
                user: "q".into(),
                assistant: "🙂 ".into()
            }]
        );
        assert_eq!(registry.active_count(), 0);
        assert!(!lease.is_active());
        assert_eq!(handle.status().phase, ControlPhase::Closed);
        assert_eq!(
            rig.peer
                .accepted_log
                .lock()
                .unwrap()
                .iter()
                .filter(|c| matches!(c, CallCommand::Hangup { .. }))
                .count(),
            usize::from(!fail)
        );
        // A failed attempt is Unknown, not retryable. Count attempts too, not
        // merely the accepted log (which cannot see failed remote effects).
        while let Ok(command) = rig.entered_submit.try_recv() {
            assert!(
                !matches!(command, CallCommand::Hangup { .. }),
                "no second remote attempt"
            );
        }
    }
}

// Break caught: a live permit alone grants a stale/foreign call lease access.
#[tokio::test]
async fn controlled_refuses_revoked_generation_or_different_call_before_work() {
    for fault in 0..3 {
        let mut rig = Rig::new().await;
        let admitted = rig.admit().await;
        let registry = PublicRunRegistry::new(4, 1000);
        let permit = preadmit(&registry, &admitted);
        let (handle, controls) = voice_control_pair(&admitted, &registry, &permit).unwrap();
        let other = Rig::new().await;
        let call = if fault == 2 {
            other.admit().await
        } else {
            if fault == 0 {
                admitted.lease().revoke();
            } else {
                rig.relay.connect(policy()).await.unwrap();
            }
            assert!(voice_control_pair(&admitted, &registry, &permit).is_err());
            admitted
        };
        let (runner, _requests) = Runner::new("never invoke");
        let report = bounded(run_public_voice_call_controlled(
            call,
            wiring(&rig, runner.clone(), registry.clone(), None),
            permit,
            controls,
        ))
        .await;
        assert_eq!(report.termination, VoiceCallTermination::ControlRefused);
        assert_eq!(runner.calls.load(Ordering::Acquire), 0);
        assert!(rig.entered_submit.try_recv().is_err());
        assert_eq!(handle.hangup().status(), Some(ControlCompletion::Refused));
        assert_eq!(registry.active_count(), 0);
    }
}

// Break caught: Unknown cancellation is consumed as a gate-opening value or
// retried. A new generated turn must not escape after this uncertain attempt.
#[tokio::test]
async fn controlled_unknown_cancel_never_unlocks_output_or_retries() {
    let mut rig = Rig::new().await;
    rig.peer.block_send.store(true, Ordering::Release);
    rig.peer.block_control.store(true, Ordering::Release);
    let admitted = rig.admit().await;
    let s = admitted.lease().session().clone();
    let registry = PublicRunRegistry::new(4, 1000);
    let permit = preadmit(&registry, &admitted);
    let (handle, controls) = voice_control_pair(&admitted, &registry, &permit).unwrap();
    let (runner, mut requests) = Runner::new("buffer must never leave");
    let cfg = wiring(&rig, runner, registry.clone(), None);
    let (report, ()) = bounded(async {
        tokio::join!(
            run_public_voice_call_controlled(admitted, cfg, permit, controls),
            async {
                rig.input.send(transcript(&s, 1, 1, "old", false)).unwrap();
                requests.recv().await.unwrap();
                as_text(rig.entered_submit.recv().await.unwrap());
                let mut receipt = handle.cancel_output(1);
                assert_eq!(
                    receipt.local_applied().await,
                    ControlCompletion::LocalApplied
                );
                assert!(matches!(
                    rig.entered_submit.recv().await.unwrap(),
                    CallCommand::CancelOutput {
                        output_epoch: 1,
                        ..
                    }
                ));
                rig.peer.block_send.store(false, Ordering::Release);
                rig.input.send(transcript(&s, 2, 1, "new", false)).unwrap();
                requests.recv().await.unwrap();
                rig.peer.fail_submit.store(true, Ordering::Release);
                rig.peer.control_gate.notify_one();
                assert_eq!(receipt.completed().await, ControlCompletion::Unknown);
            }
        )
    })
    .await;
    assert_eq!(report.termination, VoiceCallTermination::SubmissionFailed);
    assert_eq!(report.frames_accepted, 0);
    assert!(
        rig.entered_submit.try_recv().is_err(),
        "no retry or later SendText"
    );
    assert!(rig.peer.accepted_log.lock().unwrap().is_empty());
    assert_eq!(registry.active_count(), 0);
}

// Break caught: structural status starts exposing text/history, or pre-start
// Hangup is falsely Accepted without driver effects/independent carrier work.
#[tokio::test]
async fn controlled_prestart_hangup_and_status_have_no_transcript_or_provider_work() {
    let mut rig = Rig::new().await;
    rig.peer.block_control.store(true, Ordering::Release);
    let admitted = rig.admit().await;
    let registry = PublicRunRegistry::new(4, 1000);
    let permit = preadmit(&registry, &admitted);
    let (handle, controls) = voice_control_pair(&admitted, &registry, &permit).unwrap();
    // Exhaustive structural shape: adding a transcript field breaks this guard.
    let crate::gateway::telecom::CurrentCallStatus {
        phase,
        output_epoch,
        output_cancelled,
    } = handle.status();
    assert_eq!(phase, ControlPhase::Admitted);
    assert_eq!(output_epoch, None);
    assert!(!output_cancelled);
    assert_eq!(
        handle.cancel_output(0).status(),
        Some(ControlCompletion::Refused)
    );
    let mut receipt = handle.hangup();
    assert_eq!(receipt.status(), None);
    assert!(handle.status().output_cancelled);
    let (runner, _requests) = Runner::new("private-looking text must not run");
    let cfg = wiring(&rig, runner.clone(), registry.clone(), None);
    let (report, ()) = bounded(async {
        tokio::join!(
            run_public_voice_call_controlled(admitted, cfg, permit, controls),
            async {
                assert_eq!(
                    receipt.local_applied().await,
                    ControlCompletion::LocalApplied
                );
                assert!(matches!(
                    rig.entered_submit.recv().await.unwrap(),
                    CallCommand::Hangup { .. }
                ));
                assert_eq!(registry.active_count(), 1);
                assert_eq!(runner.calls.load(Ordering::Acquire), 0);
                assert_eq!(receipt.status(), Some(ControlCompletion::LocalApplied));
                let status = format!("{:?}", handle.status());
                for forbidden in ["transcript", "history", "memory", "private-looking"] {
                    assert!(!status.contains(forbidden));
                }
                rig.peer.control_gate.notify_one();
                assert_eq!(receipt.completed().await, ControlCompletion::Accepted);
            }
        )
    })
    .await;
    assert_eq!(report.termination, VoiceCallTermination::Hangup);
    assert_eq!(report.turns_started, 0);
    assert_eq!(report.frames_accepted, 0);
    assert_eq!(registry.active_count(), 0);
}

// Break caught: dropping driver/receiver leaves grants or pending receipts alive
// across reconnect, or replays an uncertain remote command.
#[tokio::test]
async fn controlled_disconnect_and_outer_drop_revoke_without_replay() {
    for disconnect in [false, true] {
        let mut rig = Rig::new().await;
        let admitted = rig.admit().await;
        let s = admitted.lease().session().clone();
        let registry = PublicRunRegistry::new(4, 1000);
        let permit = preadmit(&registry, &admitted);
        let (handle, controls) = voice_control_pair(&admitted, &registry, &permit).unwrap();
        let (runner, _requests) = Runner::new("answer");
        rig.peer.block_send.store(true, Ordering::Release);
        let mut driver = Box::pin(run_public_voice_call_controlled(
            admitted,
            wiring(&rig, runner, registry.clone(), None),
            permit,
            controls,
        ));
        rig.input.send(transcript(&s, 1, 1, "q", false)).unwrap();
        bounded(async { tokio::select! { biased; _ = &mut driver => panic!("still live"), c = rig.entered_submit.recv() => { as_text(c.unwrap()); } } }).await;
        rig.peer.block_control.store(true, Ordering::Release);
        let mut receipt = handle.hangup();
        bounded(async { tokio::select! { biased; _ = &mut driver => panic!("hangup blocked"), result = receipt.local_applied() => assert_eq!(result, ControlCompletion::LocalApplied) } }).await;
        if disconnect {
            rig.relay.disconnect();
            assert_eq!(
                bounded(driver.as_mut()).await.termination,
                VoiceCallTermination::Revoked
            );
        }
        drop(driver);
        assert_eq!(receipt.completed().await, ControlCompletion::Unknown);
        assert_eq!(registry.active_count(), 0);
        rig.peer.control_gate.notify_one();
        rig.peer.send_gate.notify_one();
        rig.relay.connect(policy()).await.unwrap();
        assert_eq!(handle.hangup().status(), Some(ControlCompletion::Refused));
        assert_eq!(
            handle.cancel_output(1).status(),
            Some(ControlCompletion::Refused)
        );
        assert!(rig.peer.accepted_log.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn lifecycle_admitted_then_concurrent_hangup_keeps_owned_cleanup() {
    for answering in [false, true] {
        let rig = Rig::new().await;
        let (runner, _requests) = Runner::new("unused");
        bounded(
            crate::gateway::telecom::lifecycle_tools::test_support::cancelled_admission_cleanup(
                rig.relay.clone(),
                runner,
                answering,
                false,
            ),
        )
        .await;
        assert!(
            rig.peer.accepted_log.lock().unwrap().is_empty(),
            "no arbitrary carrier bypass"
        );
    }
}

#[tokio::test]
async fn lifecycle_uncertain_cleanup_is_unknown_and_never_retried() {
    for answering in [false, true] {
        let rig = Rig::new().await;
        let (runner, _requests) = Runner::new("unused");
        bounded(
            crate::gateway::telecom::lifecycle_tools::test_support::cancelled_admission_cleanup(
                rig.relay.clone(),
                runner,
                answering,
                true,
            ),
        )
        .await;
        assert!(rig.peer.accepted_log.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn lifecycle_257th_record_evicts_oldest_completed_terminal() {
    let rig = Rig::new().await;
    let (runner, _requests) = Runner::new("unused");
    bounded(
        crate::gateway::telecom::lifecycle_tools::test_support::terminal_retention(
            rig.relay.clone(),
            runner,
        ),
    )
    .await;
}

#[tokio::test]
async fn lifecycle_stale_cleanup_never_polls_peer() {
    let rig = Rig::new().await;
    let (runner, _requests) = Runner::new("unused");
    bounded(
        crate::gateway::telecom::lifecycle_tools::test_support::stale_cleanup(
            rig.relay.clone(),
            runner,
        ),
    )
    .await;
}

#[tokio::test]
async fn lifecycle_running_hangup_records_remote_outcome_and_survives_eviction() {
    for fail in [false, true] {
        let rig = Rig::new().await;
        rig.peer.fail_submit.store(fail, Ordering::Release);
        let (runner, _requests) = Runner::new("unused");
        bounded(
            crate::gateway::telecom::lifecycle_tools::test_support::running_hangup_status(
                rig.relay.clone(),
                runner,
                fail,
            ),
        )
        .await;
    }
}

#[tokio::test]
async fn lifecycle_active_and_unfinished_records_block_eviction() {
    let rig = Rig::new().await;
    let (runner, _requests) = Runner::new("unused");
    crate::gateway::telecom::lifecycle_tools::test_support::active_retention(
        rig.relay.clone(),
        runner,
    );
}
