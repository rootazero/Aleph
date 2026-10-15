//! Precise public voice lifecycle capability / 精确公共电话生命周期能力.
//!
//! Server code pre-admits a public permit, then mints this opaque pair. Neither
//! JSON, role strings nor raw call metadata grant authority. CallerIdentity
//! authorization and tool registration belong to the service, not this bridge.
//! The handle cancels epoch tokens synchronously; only the driver acknowledges
//! reducer effects and owns bounded carrier futures. No task is spawned here.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, Weak};

use tokio::sync::{watch, Notify};
use tokio_util::sync::CancellationToken;

use super::public_admission::{PublicRunIdentity, PublicRunPermit, PublicRunRegistry};
use crate::gateway::interfaces::voice_call::{AdmittedVoiceCall, VoiceCallLease};

// One valid request per committed epoch (the driver permits at most 32), plus
// one Hangup. Invalid/duplicate requests never grow this receipt ledger.
const MAX_CANCEL_REQUESTS: usize = 32;
const QUEUE_CAP: usize = 8; // one reserved priority slot for Hangup

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlCompletion {
    /// Owned work stopped, pending text removed and reducer fence installed.
    /// This is NOT a remote acceptance or a playback-cleared fact.
    LocalApplied,
    /// The carrier returned Ok for this exact command, not known playback.
    Accepted,
    /// Remote outcome unavailable; never retried or treated as zero effects.
    Unknown,
    Refused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlPhase {
    Admitted,
    Running,
    Closing,
    Closed,
}

/// Structural status only: no transcript, generated text, history or memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CurrentCallStatus {
    pub phase: ControlPhase,
    pub output_epoch: Option<u64>,
    pub output_cancelled: bool,
}

#[derive(Clone, Copy)]
struct Progress {
    local: bool,
    completion: Option<ControlCompletion>,
}

/// Dropping a receipt or its await does not withdraw accepted local intent.
/// None means no reducer acknowledgement yet, NOT command acceptance.
pub struct ControlReceipt {
    progress: watch::Receiver<Progress>,
}
impl ControlReceipt {
    fn refused() -> Self {
        let (_, progress) = watch::channel(Progress {
            local: false,
            completion: Some(ControlCompletion::Refused),
        });
        Self { progress }
    }

    pub fn status(&self) -> Option<ControlCompletion> {
        self.progress.borrow().completion
    }

    pub async fn local_applied(&mut self) -> ControlCompletion {
        loop {
            let current = *self.progress.borrow_and_update();
            if current.local {
                return ControlCompletion::LocalApplied;
            }
            if let Some(done) = current.completion {
                return done;
            }
            if self.progress.changed().await.is_err() {
                return ControlCompletion::Unknown;
            }
        }
    }

    pub async fn completed(&mut self) -> ControlCompletion {
        loop {
            let current = *self.progress.borrow_and_update();
            if let Some(done) = current.completion {
                if done != ControlCompletion::LocalApplied {
                    return done;
                }
            }
            if self.progress.changed().await.is_err() {
                return ControlCompletion::Unknown;
            }
        }
    }
}

#[derive(Clone)]
pub(crate) struct ControlAck {
    progress: watch::Sender<Progress>,
}
impl ControlAck {
    fn new() -> Self {
        let (progress, _) = watch::channel(Progress {
            local: false,
            completion: None,
        });
        Self { progress }
    }
    fn receipt(&self) -> ControlReceipt {
        ControlReceipt {
            progress: self.progress.subscribe(),
        }
    }
    pub(crate) fn local_applied(&self) {
        self.progress.send_if_modified(|p| {
            if p.completion.is_some() {
                return false;
            }
            p.local = true;
            p.completion = Some(ControlCompletion::LocalApplied);
            true
        });
    }
    pub(crate) fn finish(&self, completion: ControlCompletion) {
        self.progress.send_if_modified(|p| {
            if matches!(
                p.completion,
                Some(
                    ControlCompletion::Accepted
                        | ControlCompletion::Unknown
                        | ControlCompletion::Refused
                )
            ) {
                return false;
            }
            p.completion = Some(completion);
            true
        });
    }
}

pub(crate) enum ControlCommand {
    CancelOutput { epoch: u64, ack: ControlAck },
    Hangup { ack: ControlAck },
}

struct Epoch {
    id: u64,
    cancel: CancellationToken,
    request: Option<ControlAck>,
}
struct State {
    closed: bool,
    current: Option<Epoch>,
    hangup: Option<ControlAck>,
    queue: VecDeque<ControlCommand>,
    receipts: Vec<ControlAck>,
}
struct Hub {
    identity: PublicRunIdentity,
    lease: VoiceCallLease,
    output_stop: CancellationToken,
    state: Mutex<State>,
    ready: Notify,
}
impl Hub {
    fn live(&self) -> bool {
        self.identity.is_current() && self.lease.is_active()
    }
}

/// Opaque exact-run capability; cloning it never extends receiver lifetime.
#[derive(Clone)]
pub struct ControlHandle {
    hub: Weak<Hub>,
}
impl ControlHandle {
    pub fn status(&self) -> CurrentCallStatus {
        let closed = CurrentCallStatus {
            phase: ControlPhase::Closed,
            output_epoch: None,
            output_cancelled: true,
        };
        let Some(hub) = self.hub.upgrade() else {
            return closed;
        };
        let state = hub.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.closed || !hub.live() {
            return closed;
        }
        CurrentCallStatus {
            phase: if state.hangup.is_some() {
                ControlPhase::Closing
            } else if state.current.is_some() {
                ControlPhase::Running
            } else {
                ControlPhase::Admitted
            },
            output_epoch: state.current.as_ref().map(|e| e.id),
            output_cancelled: hub.output_stop.is_cancelled()
                || state
                    .current
                    .as_ref()
                    .is_some_and(|e| e.cancel.is_cancelled()),
        }
    }

    /// Epoch is a selector that narrows this capability, never raw authority.
    /// Accepted intent cancels the current epoch BEFORE this method returns,
    /// including inside an awaited provider/peer's final poll.
    pub fn cancel_output(&self, epoch: u64) -> ControlReceipt {
        let Some(hub) = self.hub.upgrade() else {
            return ControlReceipt::refused();
        };
        let mut state = hub.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.closed || !hub.live() || state.hangup.is_some() {
            return ControlReceipt::refused();
        }
        let Some(current) = state.current.as_ref().filter(|e| e.id == epoch) else {
            return ControlReceipt::refused();
        };
        if let Some(ack) = &current.request {
            return ack.receipt();
        }
        // Carrier Cleared or replacement may already have stopped this epoch.
        if current.cancel.is_cancelled()
            || state.queue.len() >= QUEUE_CAP - 1
            || state.receipts.len() >= MAX_CANCEL_REQUESTS
        {
            return ControlReceipt::refused();
        }
        let ack = ControlAck::new();
        let receipt = ack.receipt();
        let current = state.current.as_mut().unwrap();
        current.cancel.cancel();
        current.request = Some(ack.clone());
        state.receipts.push(ack.clone());
        state
            .queue
            .push_back(ControlCommand::CancelOutput { epoch, ack });
        hub.ready.notify_one();
        receipt
    }

    /// Stops all local output, but NOT the permit/lease needed for remote
    /// Hangup. Driver ownership makes tool-await cancellation irrelevant.
    pub fn hangup(&self) -> ControlReceipt {
        let Some(hub) = self.hub.upgrade() else {
            return ControlReceipt::refused();
        };
        let mut state = hub.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.closed || !hub.live() {
            return ControlReceipt::refused();
        }
        if let Some(ack) = &state.hangup {
            return ack.receipt();
        }
        let ack = ControlAck::new();
        let receipt = ack.receipt();
        hub.output_stop.cancel();
        state.hangup = Some(ack.clone());
        state.receipts.push(ack.clone());
        state.queue.push_front(ControlCommand::Hangup { ack });
        hub.ready.notify_one();
        receipt
    }
}

/// Unique receiver owner; no Clone, Default, serde or raw-metadata constructor.
pub struct LifecycleControlReceiver {
    hub: Arc<Hub>,
}

/// Service-only minting after server admission. The receiver is consumed by
/// run_public_voice_call_controlled; the service retains only the handle.
pub(crate) fn voice_control_pair(
    admitted: &AdmittedVoiceCall,
    registry: &Arc<PublicRunRegistry>,
    permit: &PublicRunPermit,
) -> Result<(ControlHandle, LifecycleControlReceiver), ControlCompletion> {
    let identity = permit
        .validated_identity(registry, admitted.context())
        .filter(|_| admitted.lease().is_active())
        .ok_or(ControlCompletion::Refused)?;
    let hub = Arc::new(Hub {
        output_stop: identity.cancel_token().child_token(),
        identity,
        lease: admitted.lease().clone(),
        state: Mutex::new(State {
            closed: false,
            current: None,
            hangup: None,
            queue: VecDeque::new(),
            receipts: Vec::new(),
        }),
        ready: Notify::new(),
    });
    Ok((
        ControlHandle {
            hub: Arc::downgrade(&hub),
        },
        LifecycleControlReceiver { hub },
    ))
}

impl LifecycleControlReceiver {
    pub(crate) fn validates(
        &self,
        identity: &PublicRunIdentity,
        admitted: &AdmittedVoiceCall,
    ) -> bool {
        let state = self.hub.state.lock().unwrap_or_else(|e| e.into_inner());
        !state.closed
            && self.hub.live()
            && self.hub.identity.same_run(identity)
            && self.hub.lease.session() == admitted.lease().session()
            && admitted.lease().is_active()
    }

    /// Called only after the coordinator validates and reserves a committed
    /// turn. Installation and handle intent share one lock: no swapped token.
    pub(crate) fn install_epoch(&self, epoch: u64) -> CancellationToken {
        let mut state = self.hub.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(old) = state.current.take() {
            old.cancel.cancel();
        }
        let cancel = self.hub.output_stop.child_token();
        state.current = Some(Epoch {
            id: epoch,
            cancel: cancel.clone(),
            request: None,
        });
        cancel
    }
    pub(crate) fn epoch_token(&self, epoch: u64) -> Option<CancellationToken> {
        let state = self.hub.state.lock().unwrap_or_else(|e| e.into_inner());
        state
            .current
            .as_ref()
            .filter(|e| e.id == epoch)
            .map(|e| e.cancel.clone())
    }
    pub(crate) fn stop_epoch(&self, epoch: u64) {
        if let Some(token) = self.epoch_token(epoch) {
            token.cancel();
        }
    }
    pub(crate) fn has_intent(&self) -> bool {
        !self
            .hub
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .queue
            .is_empty()
    }
    pub(crate) async fn next(&self) -> ControlCommand {
        loop {
            // notify_one retains a permit; registering before inspecting also
            // makes the check/sleep boundary explicit without holding a lock.
            let ready = self.hub.ready.notified();
            if let Some(command) = self
                .hub
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .queue
                .pop_front()
            {
                return command;
            }
            ready.await;
        }
    }
}
impl Drop for LifecycleControlReceiver {
    fn drop(&mut self) {
        let mut state = self.hub.state.lock().unwrap_or_else(|e| e.into_inner());
        state.closed = true;
        self.hub.output_stop.cancel();
        self.hub.lease.revoke();
        for ack in &state.receipts {
            ack.finish(ControlCompletion::Unknown);
        }
        state.queue.clear();
    }
}
