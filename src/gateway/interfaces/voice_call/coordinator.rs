//! Public voice execution driver / 公共电话执行协调器.
//!
//! One permit spans admission, the initial history snapshot, every utterance,
//! closure and final history append. No owner routing, transcript broadcasts,
//! private-memory tools, retries or detached tasks. Voice private A/B is deferred.
//!
//! Limits / 上限: 120s per runner, 30s per frame submission, 10min per call,
//! 32 committed turns and 32 combined completed history turns, 64KiB combined
//! history/current question/generated text. One buffered output chunk per turn.
//! Exact heard prefixes remain call-local until closure; acceptance is NOT
//! playback. Same-turn committed revisions are refused (no repeated effects).

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::future::{pending, Future};
use std::pin::Pin;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

use aleph_protocol::telecom_relay::{
    CallTurn, CallTurnRole, PlaybackState, TelecomEnvelope, TelecomKind, TranscriptState,
};
use tokio_util::sync::CancellationToken;

use super::{
    AdmittedVoiceCall, ApplyOutcome, CallCommand, CallState, VoiceCallLease, VoicePeerEvent,
    VoiceRelayTransport,
};
use crate::gateway::telecom::{
    lifecycle_bridge::{ControlAck, ControlCommand, ControlCompletion, LifecycleControlReceiver},
    public_admission::{PublicAdmission, PublicRunPermit, PublicRunRegistry},
    PublicHistorySession, PublicHistoryStore, PublicHistoryTurn,
};
use crate::orchestrator::dispatch::{
    flow_event_channel, HarnessRunner, PublicRunRequest, PUBLIC_PROMPT_VERSION,
};

pub const VOICE_RUN_TIMEOUT: Duration = Duration::from_secs(120);
pub const VOICE_SUBMIT_TIMEOUT: Duration = Duration::from_secs(30);
pub const VOICE_CALL_TIMEOUT: Duration = Duration::from_secs(10 * 60);
pub const VOICE_MAX_TURNS: usize = 32;
pub const VOICE_MAX_TEXT_BYTES: usize = 64 * 1024;

type OwnedFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

pub struct PublicVoiceWiring {
    pub registry: Arc<PublicRunRegistry>,
    pub harness: Arc<dyn HarnessRunner>,
    /// None disables cross-call persistence, not the public isolation ceiling.
    pub history: Option<Arc<PublicHistoryStore>>,
    pub transport: VoiceRelayTransport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceCallTermination {
    Hangup,
    Revoked,
    Cancelled,
    Busy,
    ControlRefused,
    OverSpend,
    HistoryUnavailable,
    InvalidEvent,
    SameTurnRevision,
    LimitExceeded,
    CallTimeout,
    RunnerFailed,
    SubmissionFailed,
}

/// A partial append is reported once; rows already appended are never reset
/// or retried. No transcripts or caller addresses appear in this report.
#[derive(Debug)]
pub struct VoiceCallReport {
    pub termination: VoiceCallTermination,
    pub turns_started: usize,
    pub frames_accepted: usize,
    pub history_appended: usize,
    pub history_error: Option<String>,
    /// Remote control acceptance is independent from local termination.
    pub remote_hangup: Option<ControlCompletion>,
}
impl VoiceCallReport {
    fn new(termination: VoiceCallTermination) -> Self {
        Self {
            termination,
            turns_started: 0,
            frames_accepted: 0,
            history_appended: 0,
            history_error: None,
            remote_hangup: None,
        }
    }
}

// A runner is owned, not spawned. Dropping it cancels its token and accounts
// unknown spend only if the public invocation actually began polling.
struct ActiveRun {
    future: OwnedFuture<Result<CallTurn, VoiceCallTermination>>,
    cancel: CancellationToken,
    started: Arc<AtomicBool>,
    accounted: Arc<AtomicBool>,
    registry: Arc<PublicRunRegistry>,
}
impl Drop for ActiveRun {
    fn drop(&mut self) {
        self.cancel.cancel();
        if self.started.load(Ordering::Acquire) && !self.accounted.load(Ordering::Acquire) {
            self.registry.exhaust_unaccounted_spend();
        }
    }
}

struct Submission {
    future: OwnedFuture<Result<(), VoiceCallTermination>>,
    cancel: CancellationToken,
    /// Only a successful SendText acknowledgement can make playback eligible.
    accepted_key: Option<(u64, u64)>,
}
impl Drop for Submission {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

async fn poll_run(run: &mut Option<ActiveRun>) -> Result<CallTurn, VoiceCallTermination> {
    match run {
        Some(run) => run.future.as_mut().await,
        None => pending().await,
    }
}
async fn poll_submission(work: &mut Option<Submission>) -> Result<(), VoiceCallTermination> {
    match work {
        Some(work) => work.future.as_mut().await,
        None => pending().await,
    }
}

async fn next_control(controls: &Option<LifecycleControlReceiver>) -> ControlCommand {
    match controls {
        Some(controls) => controls.next().await,
        None => pending().await,
    }
}

async fn next_peer_event(
    transport: &VoiceRelayTransport,
    lease: &VoiceCallLease,
    deferred: &mut Option<Result<VoicePeerEvent, super::VoiceRelayError>>,
    closing: bool,
) -> Result<VoicePeerEvent, super::VoiceRelayError> {
    if closing {
        return pending().await;
    }
    match deferred.take() {
        Some(event) => event,
        None => transport.next_event(lease).await,
    }
}

struct CancelWork {
    epoch: u64,
    ack: Option<ControlAck>,
}

fn start_submission(
    transport: VoiceRelayTransport,
    lease: VoiceCallLease,
    command: CallCommand,
    parent_cancel: &CancellationToken,
) -> Submission {
    // Permit stop reaches the peer's final check without another outer poll.
    let cancel = parent_cancel.child_token();
    let token = cancel.clone();
    let accepted_key = match &command {
        CallCommand::SendText(output) => Some((output.output_epoch, output.output_seq)),
        CallCommand::CancelOutput { .. } | CallCommand::Hangup { .. } => None,
    };
    Submission {
        cancel,
        accepted_key,
        future: Box::pin(async move {
            match tokio::time::timeout(
                VOICE_SUBMIT_TIMEOUT,
                transport.submit(&lease, command, &token),
            )
            .await
            {
                Ok(Ok(())) => Ok(()),
                _ => Err(VoiceCallTermination::SubmissionFailed),
            }
        }),
    }
}

fn start_run(
    wiring: &PublicVoiceWiring,
    admitted: &AdmittedVoiceCall,
    turn: &super::TurnState,
    history: Vec<PublicHistoryTurn>,
    parent_cancel: &CancellationToken,
) -> ActiveRun {
    // The provider and both lease checks share this permit-derived child.
    let cancel = parent_cancel.child_token();
    let token = cancel.clone();
    let started = Arc::new(AtomicBool::new(false));
    let accounted = Arc::new(AtomicBool::new(false));
    let began = started.clone();
    let paid = accounted.clone();
    let registry = wiring.registry.clone();
    let harness = wiring.harness.clone();
    let lease = admitted.lease().clone();
    let session = lease.session().clone();
    let epoch = turn.output_epoch;
    let turn_id = turn.turn_id.clone();
    let request = PublicRunRequest {
        ctx: admitted.context().clone(),
        text: turn.committed_transcript.clone(),
        history,
        run_id: uuid::Uuid::new_v4().to_string(),
        prompt_version: PUBLIC_PROMPT_VERSION,
        allow_private_lookup: false,
    };
    ActiveRun {
        cancel,
        started,
        accounted,
        registry: wiring.registry.clone(),
        future: Box::pin(async move {
            // Spend is checked at every invocation, including after a
            // previous cancelled utterance exhausted the fallback cap.
            if !registry.recheck_spend() {
                return Err(VoiceCallTermination::OverSpend);
            }
            lease
                .check(&token)
                .map_err(|_| VoiceCallTermination::Revoked)?;
            let (events, rx) = flow_event_channel();
            drop(rx); // local sender only: no owner event stream is wired
            let result = tokio::time::timeout(VOICE_RUN_TIMEOUT, async {
                began.store(true, Ordering::Release);
                harness
                    .run_public_text(request, token.clone(), events)
                    .await
            })
            .await;
            let outcome = match result {
                Ok(Ok(outcome)) => {
                    registry.record_spend(u64::from(outcome.total_tokens));
                    paid.store(true, Ordering::Release);
                    outcome
                }
                _ => {
                    registry.exhaust_unaccounted_spend();
                    paid.store(true, Ordering::Release);
                    token.cancel();
                    return Err(VoiceCallTermination::RunnerFailed);
                }
            };
            lease
                .check(&token)
                .map_err(|_| VoiceCallTermination::Revoked)?;
            if outcome.private_lookup.is_some() {
                return Err(VoiceCallTermination::RunnerFailed);
            }
            Ok(CallTurn {
                provider_account_id: session.provider_account_id,
                phone_call_id: session.phone_call_id,
                provider_call_id: session.provider_call_id,
                turn_id,
                role: CallTurnRole::AssistantText,
                transcript_state: TranscriptState::Committed,
                output_epoch: epoch,
                output_seq: 1,
                text: outcome.final_text,
            })
        }),
    }
}

fn bytes(history: &[PublicHistoryTurn]) -> usize {
    history.iter().fold(0usize, |n, t| {
        n.saturating_add(t.user.len())
            .saturating_add(t.assistant.len())
    })
}

/// Snapshot before replacement/cancellation/hangup, since reducer hangup
/// erases active state. Unicode scalar slicing and whitespace are unchanged.
/// The active turn's output is only carried into `heard` if the matching
/// (output_epoch, output_seq=1) SendText was actually accepted by the carrier;
/// an early Played exact prefix without an Ok submit does not become history.
fn snapshot_heard(
    state: &CallState,
    heard: &mut Vec<PublicHistoryTurn>,
    accepted: &BTreeSet<(u64, u64)>,
) {
    if let Some(turn) = &state.active_turn {
        if !accepted.contains(&(turn.output_epoch, 1)) {
            return;
        }
        if let Some(text) = turn.heard_text_for_output(1).filter(|s| !s.is_empty()) {
            heard.push(PublicHistoryTurn {
                user: turn.committed_transcript.clone(),
                assistant: text,
            });
        }
    }
}

// Synchronous RAII closure also covers dropping the coordinator while it is
// blocked on peer/provider I/O. Permit release follows revoke + finalization.
struct CallClosure {
    admitted: AdmittedVoiceCall,
    permit: Option<PublicRunPermit>,
    state: CallState,
    heard: Vec<PublicHistoryTurn>,
    /// Successful submission keys, distinct from reducer playback facts.
    /// Early playback stays provisional until its matching key is present.
    submission_accepted: BTreeSet<(u64, u64)>,
    history: Option<Arc<PublicHistoryStore>>,
    key: PublicHistorySession,
    prior_turns: usize,
    prior_bytes: usize,
    finalized: bool,
}
impl CallClosure {
    fn finalize(&mut self, report: &mut VoiceCallReport) {
        if self.finalized {
            return;
        }
        self.finalized = true;
        snapshot_heard(&self.state, &mut self.heard, &self.submission_accepted);
        self.admitted.lease().revoke();
        let _ = self.state.hangup();
        if self.prior_turns + self.heard.len() > VOICE_MAX_TURNS
            || self.prior_bytes.saturating_add(bytes(&self.heard)) > VOICE_MAX_TEXT_BYTES
        {
            report.termination = VoiceCallTermination::LimitExceeded;
        } else if let Some(store) = &self.history {
            for turn in &self.heard {
                match store.append(&self.key, turn, chrono::Utc::now(), false) {
                    Ok(crate::gateway::telecom::PublicHistoryAppend::Stored { .. }) => {
                        report.history_appended += 1
                    }
                    Ok(crate::gateway::telecom::PublicHistoryAppend::Disabled) => {}
                    Err(error) => {
                        report.history_error = Some(error.to_string());
                        break;
                    }
                }
            }
        }
        drop(self.permit.take());
    }
}
impl Drop for CallClosure {
    fn drop(&mut self) {
        let mut report = VoiceCallReport::new(VoiceCallTermination::Cancelled);
        self.finalize(&mut report);
        if report.history_error.is_some() {
            tracing::warn!("[public voice] closure history append failed; no retry/reset");
        }
    }
}

/// Preserve the old epoch's cancellation marker on the state actually installed.
pub(super) fn install_replacement(state: &mut CallState, mut next: CallState, old_epoch: u64) {
    next.cancel_output_epoch(old_epoch);
    *state = next;
}

/// Clear sendable buffers, retaining only reducer chunk facts needed to pair
/// exact Played extents with the successful-submission ledger at finalization.
fn clear_pending_text(state: &mut CallState) {
    if let Some(turn) = &mut state.active_turn {
        turn.generated_text.clear();
        turn.queued_text.clear();
    }
}

fn validate_envelope(envelope: &TelecomEnvelope) -> bool {
    let now = chrono::Utc::now();
    envelope.check_version().is_ok()
        && !envelope.event_id.is_empty()
        && envelope.created_at <= now
        && envelope.expires_at > now
        && envelope.expires_at > envelope.created_at
        && match &envelope.kind {
            TelecomKind::CallTurn(t) => {
                t.role == CallTurnRole::UserTranscript && t.output_epoch > 0 && t.output_seq > 0
            }
            TelecomKind::PlaybackEvent(e) => {
                // Clear acknowledges an epoch fence, not necessarily our one
                // buffered output chunk. Other playback still names that chunk.
                e.output_epoch > 0
                    && if e.state == PlaybackState::Cleared {
                        e.output_seq > 0
                    } else {
                        e.output_seq == 1
                    }
            }
            _ => false,
        }
}

/// Trusted embedder entry point. Dropping this future revokes call authority,
/// cancels owned work, and releases its public permit; it cannot spawn a
/// second agent or leave a caller's active transcript in shared history.
pub async fn run_public_voice_call(
    admitted: AdmittedVoiceCall,
    wiring: PublicVoiceWiring,
) -> VoiceCallReport {
    let permit = match wiring.registry.try_admit(admitted.context()) {
        PublicAdmission::Admitted(p) => p,
        PublicAdmission::OverSpend => return VoiceCallReport::new(VoiceCallTermination::OverSpend),
        _ => return VoiceCallReport::new(VoiceCallTermination::Busy),
    };
    drive_public_voice_call(admitted, wiring, permit, None).await
}

/// Service pre-admits once and supplies an opaque precise-run control receiver.
/// Denials happen before history, provider or carrier work. No new admission
/// or raw-metadata authority is inferred here; the permit is consumed once.
pub async fn run_public_voice_call_controlled(
    admitted: AdmittedVoiceCall,
    wiring: PublicVoiceWiring,
    permit: PublicRunPermit,
    controls: LifecycleControlReceiver,
) -> VoiceCallReport {
    let valid = permit
        .validated_identity(&wiring.registry, admitted.context())
        .is_some_and(|identity| controls.validates(&identity, &admitted));
    if !valid {
        return VoiceCallReport::new(VoiceCallTermination::ControlRefused);
    }
    drive_public_voice_call(admitted, wiring, permit, Some(controls)).await
}

async fn drive_public_voice_call(
    admitted: AdmittedVoiceCall,
    wiring: PublicVoiceWiring,
    permit: PublicRunPermit,
    controls: Option<LifecycleControlReceiver>,
) -> VoiceCallReport {
    let lease = admitted.lease().clone();
    let cancel = permit.cancel_token();
    let history_key = PublicHistorySession::from_provenance(admitted.context().provenance());
    let mut call = CallClosure {
        state: CallState::new(lease.session().clone()).expect("authenticated session validated"),
        admitted,
        permit: Some(permit),
        heard: Vec::new(),
        submission_accepted: BTreeSet::new(),
        history: wiring.history.clone(),
        key: history_key,
        prior_turns: 0,
        prior_bytes: 0,
        finalized: false,
    };
    let prior = match &wiring.history {
        Some(store) => match store.load(&call.key, chrono::Utc::now()) {
            Ok(history) => history,
            Err(_) => {
                let mut report = VoiceCallReport::new(VoiceCallTermination::HistoryUnavailable);
                call.finalize(&mut report);
                return report;
            }
        },
        None => Vec::new(),
    };
    if prior.len() > VOICE_MAX_TURNS || bytes(&prior) > VOICE_MAX_TEXT_BYTES {
        let mut report = VoiceCallReport::new(VoiceCallTermination::LimitExceeded);
        call.finalize(&mut report);
        return report;
    }
    call.prior_turns = prior.len();
    call.prior_bytes = bytes(&prior);
    let deadline = tokio::time::sleep(VOICE_CALL_TIMEOUT);
    tokio::pin!(deadline);
    let mut report = VoiceCallReport::new(VoiceCallTermination::Hangup);
    let mut run = None;
    let mut send = None;
    let mut cancel_send = None;
    let mut cancel_queue: VecDeque<CancelWork> = VecDeque::new();
    let mut active_cancel_ack: Option<ControlAck> = None;
    let mut active_cancel_epoch = None;
    let mut cancel_requested = BTreeSet::new();
    let mut cancel_results = BTreeMap::new();
    let mut terminal_failure = None;
    let mut hangup_send = None;
    let mut hangup_ack: Option<ControlAck> = None;
    let mut closing = false;
    let mut deferred_peer = None;
    let mut epoch_cancel = cancel.child_token();
    let mut generated: Option<CallTurn> = None;

    let termination = loop {
        // Carrier cancellation must be accepted before any newer SendText is
        // first polled. A cancellation request is never a playback-clear fact.
        let has_intent = controls.as_ref().is_some_and(|c| c.has_intent());
        if !closing && !has_intent {
            if let Some(reason) = terminal_failure {
                break reason;
            }
        }
        if !closing && !has_intent && cancel_send.is_none() {
            if let Some(work) = cancel_queue.pop_front() {
                active_cancel_ack = work.ack;
                active_cancel_epoch = Some(work.epoch);
                cancel_send = Some(start_submission(
                    wiring.transport.clone(),
                    lease.clone(),
                    CallCommand::CancelOutput {
                        phone_call_id: call.state.session.phone_call_id.clone(),
                        output_epoch: work.epoch,
                    },
                    &cancel,
                ));
            } else if send.is_none() {
                if let Some(output) = generated.take() {
                    if call.state.validate_output_for_send(&output).is_err() {
                        break VoiceCallTermination::InvalidEvent;
                    }
                    send = Some(start_submission(
                        wiring.transport.clone(),
                        lease.clone(),
                        CallCommand::SendText(output),
                        &epoch_cancel,
                    ));
                }
            }
        }
        tokio::select! {
            biased;
            _ = lease.revoked() => break VoiceCallTermination::Revoked,
            _ = cancel.cancelled() => break VoiceCallTermination::Cancelled,
            _ = &mut deadline => break VoiceCallTermination::CallTimeout,
            command = next_control(&controls) => {
                match command {
                    ControlCommand::CancelOutput { epoch, ack } => {
                        // The handle already stopped the exact epoch token. This
                        // branch is the reducer-effect ACK, not a queue receipt.
                        // A replacement can race accepted intent between event
                        // validation and hub installation. This trusted command
                        // still fences ONLY its old epoch, never the new token.
                        if call.state.active_turn.as_ref().is_some_and(|t| t.output_epoch == epoch) {
                            epoch_cancel.cancel();
                            drop(run.take());
                            drop(send.take());
                            generated = None;
                            clear_pending_text(&mut call.state);
                        }
                        call.state.cancel_output_epoch(epoch);
                        ack.local_applied();
                        if closing {
                            ack.finish(ControlCompletion::Unknown);
                        } else if let Some(accepted) = cancel_results.get(&epoch) {
                            ack.finish(if *accepted { ControlCompletion::Accepted } else { ControlCompletion::Unknown });
                        } else if active_cancel_epoch == Some(epoch) {
                            active_cancel_ack = Some(ack);
                        } else if let Some(work) = cancel_queue.iter_mut().find(|w| w.epoch == epoch) {
                            work.ack = Some(ack);
                        } else if cancel_requested.insert(epoch) {
                            cancel_queue.push_back(CancelWork { epoch, ack: Some(ack) });
                        } else {
                            ack.finish(ControlCompletion::Unknown);
                        }
                    }
                    ControlCommand::Hangup { ack } => {
                        epoch_cancel.cancel();
                        drop(run.take());
                        drop(send.take());
                        generated = None;
                        snapshot_heard(&call.state, &mut call.heard, &call.submission_accepted);
                        clear_pending_text(&mut call.state);
                        if let Some(epoch) = call.state.active_turn.as_ref().map(|t| t.output_epoch) {
                            call.state.cancel_output_epoch(epoch);
                        }
                        let _ = call.state.hangup();
                        drop(cancel_send.take());
                        active_cancel_epoch = None;
                        if let Some(old) = active_cancel_ack.take() { old.finish(ControlCompletion::Unknown); }
                        for work in cancel_queue.drain(..) {
                            if let Some(old) = work.ack { old.finish(ControlCompletion::Unknown); }
                        }
                        closing = true;
                        terminal_failure = None;
                        // Independent of the output-stop token. No lease or
                        // permit revoke until this bounded attempt/finalization.
                        hangup_send = Some(start_submission(
                            wiring.transport.clone(), lease.clone(),
                            CallCommand::Hangup { phone_call_id: call.state.session.phone_call_id.clone() },
                            &cancel,
                        ));
                        ack.local_applied();
                        hangup_ack = Some(ack);
                    }
                }
            },
            event = next_peer_event(&wiring.transport, &lease, &mut deferred_peer, closing) => {
                // Intent may be accepted inside the peer's Ready poll. Keep one
                // owned event, not a lossy dequeue, while priority control applies.
                if controls.as_ref().is_some_and(|c| c.has_intent()) {
                    deferred_peer = Some(event);
                    continue;
                }
                let envelope = match event {
                    Ok(VoicePeerEvent::Hangup(_)) => break VoiceCallTermination::Hangup,
                    Ok(VoicePeerEvent::Envelope(e)) => e,
                    Err(_) => break VoiceCallTermination::Revoked,
                };
                if !validate_envelope(&envelope) { break VoiceCallTermination::InvalidEvent; }
                let mut replacing = false;
                if let TelecomKind::CallTurn(t) = &envelope.kind {
                    if t.text.len() > VOICE_MAX_TEXT_BYTES { break VoiceCallTermination::LimitExceeded; }
                    if let Some(old) = &call.state.active_turn {
                        // Once a turn is scheduled, revisions cannot change
                        // the question associated with its side effects/reply.
                        if t.starts_agent_turn() && t.output_epoch == old.output_epoch
                            && t.turn_id == old.turn_id && t.output_seq > old.committed_transcript_seq {
                            break VoiceCallTermination::SameTurnRevision;
                        }
                        replacing = t.starts_agent_turn() && t.output_epoch > old.output_epoch;
                    }
                }
                // Validate using a cloned reducer before snapshotting or
                // cancelling anything: a hostile identity must not barge in.
                let mut next = call.state.clone();
                let outcome = match next.apply_wire(&envelope) {
                    Ok(outcome) => outcome,
                    Err(_) => break VoiceCallTermination::InvalidEvent,
                };
                // A current Cleared fences pending work even when an earlier
                // terminal playback fact makes its reducer outcome a duplicate.
                // A retained old marker must never cancel the newer submission.
                if let TelecomKind::PlaybackEvent(e) = &envelope.kind {
                    if e.state == PlaybackState::Cleared
                        && next.active_turn.as_ref().is_some_and(|t| t.output_epoch == e.output_epoch)
                        && next.is_output_epoch_cancelled(e.output_epoch)
                    {
                        epoch_cancel.cancel();
                        drop(run.take());
                        if let Some(controls) = &controls {
                            controls.stop_epoch(e.output_epoch);
                        }
                        if send.as_ref().is_some_and(|s| s.accepted_key.is_some_and(|(epoch, _)| epoch == e.output_epoch)) {
                            drop(send.take());
                        }
                        if generated.as_ref().is_some_and(|t| t.output_epoch == e.output_epoch) {
                            generated = None;
                        }
                    }
                }
                if matches!(outcome, ApplyOutcome::TurnStarted { .. }) {
                    if replacing {
                        snapshot_heard(&call.state, &mut call.heard, &call.submission_accepted);
                        let old_epoch = call.state.active_turn.as_ref().unwrap().output_epoch;
                        install_replacement(&mut call.state, next, old_epoch);
                        epoch_cancel.cancel();
                        drop(run.take());
                        drop(send.take());
                        generated = None;
                        if cancel_requested.insert(old_epoch) {
                            cancel_queue.push_back(CancelWork { epoch: old_epoch, ack: None });
                        }
                    } else {
                        call.state = next;
                    }
                    report.turns_started += 1;
                    let turn = call.state.active_turn.as_ref().unwrap();
                    // Reserve one combined-history slot before invoking this turn.
                    if report.turns_started > VOICE_MAX_TURNS
                        || prior.len() + call.heard.len() >= VOICE_MAX_TURNS
                        || bytes(&prior).saturating_add(bytes(&call.heard)).saturating_add(turn.committed_transcript.len()) > VOICE_MAX_TEXT_BYTES
                    {
                        break VoiceCallTermination::LimitExceeded;
                    }
                    // Cancelled started work has just exhausted unaccounted
                    // fallback spend, if applicable. Do not launch around it.
                    if !wiring.registry.recheck_spend() { break VoiceCallTermination::OverSpend; }
                    let history = prior.iter().chain(call.heard.iter()).cloned().collect();
                    epoch_cancel = match &controls {
                        Some(controls) => controls.install_epoch(turn.output_epoch),
                        None => cancel.child_token(),
                    };
                    run = Some(start_run(
                        &wiring,
                        &call.admitted,
                        turn,
                        history,
                        &epoch_cancel,
                    ));
                } else { call.state = next; }
            },
            result = poll_run(&mut run) => {
                drop(run.take());
                // A control accepted inside the provider's final poll has already
                // cancelled this token. Ignore its late result, not the intent.
                if controls.as_ref().is_some_and(|c| c.has_intent()) { continue; }
                let output = match result { Ok(output) => output, Err(reason) => break reason };
                let user_bytes = call.state.active_turn.as_ref().map_or(0, |t| t.committed_transcript.len());
                if bytes(&prior).saturating_add(bytes(&call.heard)).saturating_add(user_bytes).saturating_add(output.text.len()) > VOICE_MAX_TEXT_BYTES {
                    break VoiceCallTermination::LimitExceeded;
                }
                if call.state.record_generated_output(&output).is_err() { break VoiceCallTermination::InvalidEvent; }
                generated = Some(output);
            },
            result = poll_submission(&mut cancel_send) => {
                drop(cancel_send.take());
                if let Some(epoch) = active_cancel_epoch.take() { cancel_results.insert(epoch, result.is_ok()); }
                if let Some(ack) = active_cancel_ack.take() {
                    ack.finish(if result.is_ok() { ControlCompletion::Accepted } else { ControlCompletion::Unknown });
                }
                if let Err(reason) = result {
                    if controls.as_ref().is_some_and(|c| c.has_intent()) {
                        // Apply pending local intent, but never open the gate
                        // after an uncertain cancellation or retry the command.
                        terminal_failure = Some(reason);
                        continue;
                    }
                    break reason;
                }
            },
            result = poll_submission(&mut hangup_send) => {
                drop(hangup_send.take());
                let completion = if result.is_ok() { ControlCompletion::Accepted } else { ControlCompletion::Unknown };
                report.remote_hangup = Some(completion);
                if let Some(ack) = hangup_ack.take() {
                    ack.finish(completion);
                }
                break VoiceCallTermination::Hangup;
            },
            result = poll_submission(&mut send) => {
                // Playback may already be buffered; only this Ok unlocks it.
                let key = send.as_ref().and_then(|s| s.accepted_key);
                drop(send.take());
                if let Err(reason) = result {
                    if controls.as_ref().is_some_and(|c| c.has_intent()) { continue; }
                    break reason;
                }
                // Acceptance does not mutate reducer queue/playback facts.
                if let Some(k) = key {
                    call.submission_accepted.insert(k);
                }
                report.frames_accepted += 1; // no queue/playback mutation
            },
        }
    };

    // No awaited peer work survives closure; local revoke is the reliable
    // off-switch, not a claim that a remote Hangup/CancelOutput was executed.
    drop(run.take());
    drop(send.take());
    drop(cancel_send.take());
    drop(hangup_send.take());
    if let Some(ack) = active_cancel_ack.take() {
        ack.finish(ControlCompletion::Unknown);
    }
    if let Some(ack) = hangup_ack.take() {
        report.remote_hangup = Some(ControlCompletion::Unknown);
        ack.finish(ControlCompletion::Unknown);
    }
    report.termination = termination;
    call.finalize(&mut report);
    drop(controls);
    report
}
