//! Per-target ownership and the dispatch-start linearization barrier.
//!
//! This is an internal capability API, not a viewer authentication service.
//! Only a server-authenticated adapter may register a lease. No client-supplied
//! binding, permission, generation, or active-tab guess grants authority here.

use std::collections::{HashMap, HashSet};
use std::future::{poll_fn, Future};
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::Poll;
use std::time::Duration;

use aleph_protocol::browser_live::{
    BrowserLiveState, ControlRequest, ControlResult, Controller, HumanInputEvent,
    HumanInputRequest, HumanInputResult, InputRejectReason, InterventionEnd, InterventionFacts,
    InterventionSummary, ViewerBinding, ViewerPermission,
};
use tokio::sync::{watch, OwnedMutexGuard};
use tokio::time::Instant;

use crate::browser::error::BrowserError;

const INPUT_CAPACITY: usize = 64;
const DISPATCH_BUDGET: Duration = Duration::from_millis(250);

/// Must durably accept the frozen summary for its exact destination, or fail.
/// Called synchronously under the dispatch barrier; must not re-enter control.
/// A failed delivery closes the target's control plane, never silently returns
/// Agent authority. No typed text, screenshots, or inferred intent is stored.
pub(crate) type InterventionSink =
    Arc<dyn Fn(InterventionSummary) -> Result<(), BrowserError> + Send + Sync>;

#[derive(Clone)]
pub(crate) struct ViewerLease {
    viewer_session_id: String,
    ws_generation: u64,
    resource_id: String,
    binding: ViewerBinding,
    closed: watch::Receiver<bool>,
}

impl ViewerLease {
    pub(crate) fn closed(&self) -> bool {
        *self.closed.borrow()
    }

    /// The transport adapter selects on this receiver to close a revoked WS
    /// and clear its own rotation/cohort state. Cede does not signal it.
    pub(crate) fn close_signal(&self) -> watch::Receiver<bool> {
        self.closed.clone()
    }
}

struct Viewer {
    ws_generation: u64,
    expires_at_ms: i64,
    permissions: Vec<ViewerPermission>,
    last_action_seq: u64,
    last_input_seq: u64,
    // Dispatch-order watermark: the highest `seq` whose future has started.
    // Reset by epoch bumps, advanced in `dispatch` AFTER validation but
    // BEFORE the future is first polled, so a queued older input that
    // arrives at the dispatch path after a newer one is rejected as
    // `ReplayedSeq` and its future never runs. This is independent from
    // `last_input_seq` (the admission watermark, bumped at `queue_input`)
    // so the queue-side "no replay after cancel" invariant is preserved.
    last_dispatched_seq: u64,
    revoked: bool,
    close: watch::Sender<bool>,
}

struct Intervention {
    principal: aleph_protocol::browser_live::IssuerIdentity,
    started_at_ms: i64,
    epoch: u64,
    facts: InterventionFacts,
}

struct State {
    live: BrowserLiveState,
    alive: bool,
    viewers: HashMap<String, Viewer>,
    pending: HashSet<u64>,
    next_id: u64,
    intervention: Option<Intervention>,
}

pub(crate) struct TargetControl {
    state: Mutex<State>,
    serial: Arc<tokio::sync::Mutex<()>>,
    sink: InterventionSink,
}

/// The serializer remains held for the ENTIRE backend future, not just its
/// pre-check. Dropping a cancelled future also drops this permit.
pub(crate) struct AgentPermit {
    _serial: OwnedMutexGuard<()>,
    pub(crate) control: Arc<TargetControl>,
    pub(crate) epoch: u64,
}

/// A bounded queue reservation. Cancellation frees capacity but never rolls
/// back the admitted sequence (otherwise an aborted input could be replayed).
pub(crate) struct QueuedInput {
    control: Arc<TargetControl>,
    id: u64,
    lease: ViewerLease,
    request: HumanInputRequest,
    deadline: Instant,
}

impl Drop for QueuedInput {
    fn drop(&mut self) {
        self.control.lock().pending.remove(&self.id);
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn rejected(seq: u64, reason: InputRejectReason) -> HumanInputResult {
    HumanInputResult {
        seq,
        accepted: false,
        reason: Some(reason),
    }
}

impl TargetControl {
    pub(crate) fn new(mut live: BrowserLiveState, sink: InterventionSink) -> Arc<Self> {
        // Generations/control are minted by the owner, never inherited from
        // a purported client controller. The constructor is internal only.
        live.controller = Controller::Agent;
        live.control_epoch = 0;
        Arc::new(Self {
            state: Mutex::new(State {
                live,
                alive: true,
                viewers: HashMap::new(),
                pending: HashSet::new(),
                next_id: 0,
                intervention: None,
            }),
            serial: Arc::new(tokio::sync::Mutex::new(())),
            sink,
        })
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(crate) fn snapshot(&self) -> BrowserLiveState {
        self.lock().live.clone()
    }

    pub(crate) fn is_live(&self) -> bool {
        self.lock().alive
    }

    pub(crate) fn pending_count(&self) -> usize {
        self.lock().pending.len()
    }

    /// Called only AFTER the Task4 AuthOk write commits. Rotation secrets do
    /// not belong here: this control lease has no nonce/ticket/secret or grant.
    pub(crate) fn register_viewer(
        self: &Arc<Self>,
        viewer_session_id: &str,
        ws_generation: u64,
        expires_at_ms: i64,
        permissions: Vec<ViewerPermission>,
    ) -> Result<ViewerLease, InputRejectReason> {
        let runtime =
            tokio::runtime::Handle::try_current().map_err(|_| InputRejectReason::Invalid)?;
        let mut st = self.lock();
        if !st.alive {
            return Err(InputRejectReason::Revoked);
        }
        if expires_at_ms <= now_ms() {
            return Err(InputRejectReason::Expired);
        }
        if viewer_session_id.is_empty()
            || ws_generation == 0
            || permissions
                .iter()
                .enumerate()
                .any(|(i, p)| permissions[..i].contains(p))
            || st.viewers.contains_key(viewer_session_id)
        {
            return Err(InputRejectReason::Invalid);
        }
        let (close, closed) = watch::channel(false);
        let lease = ViewerLease {
            viewer_session_id: viewer_session_id.to_string(),
            ws_generation,
            resource_id: st.live.resource_id.clone(),
            binding: st.live.binding.clone(),
            closed,
        };
        st.viewers.insert(
            viewer_session_id.to_string(),
            Viewer {
                ws_generation,
                expires_at_ms,
                permissions,
                last_action_seq: 0,
                last_input_seq: 0,
                last_dispatched_seq: 0,
                revoked: false,
                close,
            },
        );
        drop(st);
        let weak = Arc::downgrade(self);
        let expiry_lease = lease.clone();
        let mut close = lease.close_signal();
        runtime.spawn(async move {
            loop {
                if *close.borrow() { return; }
                let remaining = expires_at_ms.saturating_sub(now_ms());
                if remaining <= 0 { break; }
                // Bounded sleeps avoid platform Instant overflow for a far
                // future internal lease, and revocation cancels this worker.
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_millis((remaining as u64).min(60_000))) => {}
                    _ = close.changed() => return,
                }
            }
            if let Some(control) = weak.upgrade() {
                if let Err(error) = control.revoke(&expiry_lease, InterventionEnd::Expired) {
                    tracing::error!(%error, "expired browser control remained closed after summary delivery failure");
                }
            }
        });
        Ok(lease)
    }

    fn validate_lease(st: &State, lease: &ViewerLease) -> Result<(), InputRejectReason> {
        if !st.alive || lease.closed() {
            return Err(InputRejectReason::Revoked);
        }
        if lease.binding != st.live.binding || lease.resource_id != st.live.resource_id {
            return Err(InputRejectReason::Invalid);
        }
        let v = st
            .viewers
            .get(&lease.viewer_session_id)
            .ok_or(InputRejectReason::Revoked)?;
        if v.revoked {
            return Err(InputRejectReason::Revoked);
        }
        if v.ws_generation != lease.ws_generation {
            return Err(InputRejectReason::StaleConnection);
        }
        if v.expires_at_ms <= now_ms() {
            return Err(InputRejectReason::Expired);
        }
        Ok(())
    }

    fn validate_input(
        st: &State,
        lease: &ViewerLease,
        req: &HumanInputRequest,
    ) -> Result<(), InputRejectReason> {
        Self::validate_lease(st, lease)?;
        if req.expires_at_ms <= now_ms() {
            return Err(InputRejectReason::Expired);
        }
        if req.connection_epoch != st.live.connection_epoch {
            return Err(InputRejectReason::StaleConnection);
        }
        if req.stream_epoch != st.live.stream_epoch {
            return Err(InputRejectReason::StaleStream);
        }
        if req.control_epoch != st.live.control_epoch {
            return Err(InputRejectReason::StaleControl);
        }
        if !matches!(&st.live.controller, Controller::Human { principal, viewer_session_id }
            if *principal == lease.binding.principal && *viewer_session_id == lease.viewer_session_id)
        {
            return Err(InputRejectReason::NotController);
        }
        let v = &st.viewers[&lease.viewer_session_id];
        if !v.permissions.contains(&ViewerPermission::HumanInput)
            || matches!(req.event, HumanInputEvent::Navigate { .. })
                && !v.permissions.contains(&ViewerPermission::Navigate)
        {
            return Err(InputRejectReason::PermissionDenied);
        }
        // Navigate still needs the existing SSRF policy at the execution
        // adapter. This primitive never claims that permission bypasses it.
        if !valid_event(&req.event, &st.live.viewport_css_px) {
            return Err(InputRejectReason::Invalid);
        }
        Ok(())
    }

    fn bump(st: &mut State) -> Result<(), BrowserError> {
        st.pending.clear();
        for v in st.viewers.values_mut() {
            v.last_input_seq = 0;
            v.last_dispatched_seq = 0;
        }
        match st.live.control_epoch.checked_add(1) {
            Some(epoch) => {
                st.live.control_epoch = epoch;
                Ok(())
            }
            None => {
                st.alive = false;
                Err(BrowserError::ActionFailed(
                    "browser control epoch exhausted".into(),
                ))
            }
        }
    }

    fn finish(&self, st: &mut State, reason: InterventionEnd) -> Result<(), BrowserError> {
        let Some(intervention) = st.intervention.take() else {
            return Ok(());
        };
        let summary = InterventionSummary {
            intervention_id: uuid::Uuid::new_v4().to_string(),
            destination: st.live.binding.task.clone(),
            target: st.live.binding.target.clone(),
            principal: intervention.principal,
            started_at_ms: intervention.started_at_ms,
            ended_at_ms: now_ms(),
            control_epoch_before: intervention.epoch,
            control_epoch_after: st.live.control_epoch,
            ended_by: reason,
            facts: intervention.facts,
        };
        if let Err(error) = (self.sink)(summary) {
            st.alive = false;
            for viewer in st.viewers.values_mut() {
                viewer.revoked = true;
                viewer.close.send_replace(true);
            }
            return Err(error);
        }
        Ok(())
    }

    async fn change(&self, lease: &ViewerLease, req: ControlRequest, take: bool) -> ControlResult {
        let rejection = |reason, epoch| ControlResult {
            action_seq: req.action_seq,
            accepted: false,
            control_epoch: epoch,
            reason: Some(reason),
        };
        let Ok(_serial) =
            tokio::time::timeout(DISPATCH_BUDGET, self.serial.clone().lock_owned()).await
        else {
            return rejection(
                InputRejectReason::DispatchBusy,
                self.snapshot().control_epoch,
            );
        };
        let mut st = self.lock();
        let check = || -> Result<(), InputRejectReason> {
            Self::validate_lease(&st, lease)?;
            if req.expires_at_ms <= now_ms() {
                return Err(InputRejectReason::Expired);
            }
            if req.expected_control_epoch != st.live.control_epoch {
                return Err(InputRejectReason::StaleControl);
            }
            let viewer = &st.viewers[&lease.viewer_session_id];
            if req.action_seq == 0 || req.action_seq <= viewer.last_action_seq {
                return Err(InputRejectReason::ReplayedSeq);
            }
            if !viewer.permissions.contains(&ViewerPermission::HumanInput) {
                return Err(InputRejectReason::PermissionDenied);
            }
            let ours = matches!(&st.live.controller, Controller::Human { viewer_session_id, .. } if *viewer_session_id == lease.viewer_session_id);
            if take && ours {
                return Err(InputRejectReason::Invalid);
            }
            if !take && !ours {
                return Err(InputRejectReason::NotController);
            }
            Ok(())
        };
        if let Err(reason) = check() {
            return rejection(reason, st.live.control_epoch);
        }
        st.viewers
            .get_mut(&lease.viewer_session_id)
            .unwrap()
            .last_action_seq = req.action_seq;
        if Self::bump(&mut st)
            .and_then(|()| {
                self.finish(
                    &mut st,
                    if take {
                        InterventionEnd::Revoked
                    } else {
                        InterventionEnd::Cede
                    },
                )
            })
            .is_err()
        {
            return rejection(InputRejectReason::Revoked, st.live.control_epoch);
        }
        st.live.controller = if take {
            st.intervention = Some(Intervention {
                principal: lease.binding.principal.clone(),
                started_at_ms: now_ms(),
                epoch: st.live.control_epoch,
                facts: InterventionFacts {
                    input_event_count: 0,
                    typed_text_chars: 0,
                    navigation_count: 0,
                    final_origin: None,
                },
            });
            Controller::Human {
                principal: lease.binding.principal.clone(),
                viewer_session_id: lease.viewer_session_id.clone(),
            }
        } else {
            Controller::Agent
        };
        ControlResult {
            action_seq: req.action_seq,
            accepted: true,
            control_epoch: st.live.control_epoch,
            reason: None,
        }
    }

    pub(crate) async fn take(&self, lease: &ViewerLease, req: ControlRequest) -> ControlResult {
        self.change(lease, req, true).await
    }
    pub(crate) async fn cede(&self, lease: &ViewerLease, req: ControlRequest) -> ControlResult {
        self.change(lease, req, false).await
    }

    /// Admission is synchronous and bounded. The returned reservation carries
    /// the COMPLETE tuple, including the server-bound issuer/run/resource/WS.
    pub(crate) fn queue_input(
        self: &Arc<Self>,
        lease: &ViewerLease,
        request: HumanInputRequest,
    ) -> Result<QueuedInput, InputRejectReason> {
        let mut st = self.lock();
        Self::validate_input(&st, lease, &request)?;
        if request.seq == 0 || request.seq <= st.viewers[&lease.viewer_session_id].last_input_seq {
            return Err(InputRejectReason::ReplayedSeq);
        }
        if st.pending.len() >= INPUT_CAPACITY {
            return Err(InputRejectReason::DispatchBusy);
        }
        st.next_id = st
            .next_id
            .checked_add(1)
            .ok_or(InputRejectReason::Revoked)?;
        let id = st.next_id;
        st.viewers
            .get_mut(&lease.viewer_session_id)
            .unwrap()
            .last_input_seq = request.seq;
        st.pending.insert(id);
        Ok(QueuedInput {
            control: self.clone(),
            id,
            lease: lease.clone(),
            request,
            deadline: Instant::now() + DISPATCH_BUDGET,
        })
    }

    pub(crate) async fn dispatch<F, Fut>(
        &self,
        queued: QueuedInput,
        dispatch: F,
    ) -> Result<HumanInputResult, BrowserError>
    where
        F: FnOnce(HumanInputEvent) -> Fut,
        Fut: Future<Output = Result<(), BrowserError>>,
    {
        let seq = queued.request.seq;
        if !std::ptr::eq(self, Arc::as_ptr(&queued.control)) {
            return Ok(rejected(seq, InputRejectReason::Invalid));
        }
        let Ok(_serial) =
            tokio::time::timeout_at(queued.deadline, self.serial.clone().lock_owned()).await
        else {
            return Ok(rejected(seq, InputRejectReason::DispatchBusy));
        };
        let mut start = Some(dispatch);
        let mut future: Option<Pin<Box<Fut>>> = None;
        // The initial poll, not merely creation of a lazy async future, occurs
        // under the SAME state barrier as lifecycle revoke. Thus no sync
        // targetDestroyed/revoke can slip between revalidation and CDP start.
        let result = poll_fn(|cx| {
            if let Some(start) = start.take() {
                let mut st = self.lock();
                if Instant::now() >= queued.deadline {
                    return Poll::Ready(Ok(rejected(seq, InputRejectReason::DispatchBusy)));
                }
                if let Err(reason) = Self::validate_input(&st, &queued.lease, &queued.request) {
                    return Poll::Ready(Ok(rejected(seq, reason)));
                }
                // Reject out-of-order inputs: a queued older `seq` arriving
                // after a newer one has already started its future must not
                // be applied. The dispatch watermark is advanced below, only
                // for the seq that actually wins this race.
                if let Some(v) = st.viewers.get(&queued.lease.viewer_session_id) {
                    if queued.request.seq <= v.last_dispatched_seq {
                        return Poll::Ready(Ok(rejected(seq, InputRejectReason::ReplayedSeq)));
                    }
                }
                if !st.pending.remove(&queued.id) {
                    return Poll::Ready(Ok(rejected(seq, InputRejectReason::StaleControl)));
                }
                st.viewers
                    .get_mut(&queued.lease.viewer_session_id)
                    .unwrap()
                    .last_dispatched_seq = queued.request.seq;
                // These are dispatch facts, not a claim that page effects
                // arrived. A failed CDP call remains an error to the caller.
                if let Some(i) = &mut st.intervention {
                    i.facts.input_event_count += 1;
                    match &queued.request.event {
                        HumanInputEvent::Text { text } => {
                            i.facts.typed_text_chars += text.chars().count() as u64
                        }
                        HumanInputEvent::KeyDown {
                            text: Some(text), ..
                        } => i.facts.typed_text_chars += text.chars().count() as u64,
                        HumanInputEvent::Navigate { .. } => i.facts.navigation_count += 1,
                        _ => {}
                    }
                }
                let mut f = Box::pin(start(queued.request.event.clone()));
                let poll = f.as_mut().poll(cx);
                future = Some(f);
                return poll.map(|result| {
                    result.map(|()| HumanInputResult {
                        seq,
                        accepted: true,
                        reason: None,
                    })
                });
            }
            future
                .as_mut()
                .expect("dispatch future was started")
                .as_mut()
                .poll(cx)
                .map(|result| {
                    result.map(|()| HumanInputResult {
                        seq,
                        accepted: true,
                        reason: None,
                    })
                })
        })
        .await;
        result
    }

    pub(crate) async fn agent_permit(self: &Arc<Self>) -> Result<AgentPermit, BrowserError> {
        let epoch = self.lock().live.control_epoch;
        let serial = tokio::time::timeout(DISPATCH_BUDGET, self.serial.clone().lock_owned())
            .await
            .map_err(|_| {
                BrowserError::ActionFailed("browser mutation permit delayed over 250ms".into())
            })?;
        self.check_agent(epoch)?;
        Ok(AgentPermit {
            _serial: serial,
            control: self.clone(),
            epoch,
        })
    }

    pub(crate) fn check_agent(&self, epoch: u64) -> Result<(), BrowserError> {
        let st = self.lock();
        Self::validate_agent(&st, epoch)
    }

    fn validate_agent(st: &State, epoch: u64) -> Result<(), BrowserError> {
        if !st.alive {
            return Err(BrowserError::ActionFailed(
                "browser target control is closed".into(),
            ));
        }
        if matches!(st.live.controller, Controller::Human { .. }) {
            return Err(BrowserError::HumanHasControl {
                binding: Box::new(st.live.binding.clone()),
            });
        }
        if st.live.control_epoch != epoch {
            return Err(BrowserError::ActionFailed(
                "stale browser mutation epoch; retry after observing ownership".into(),
            ));
        }
        Ok(())
    }

    /// Whole-lease revocation is atomic against dispatch-start. No new
    /// controller is inferred from connection loss; an explicit summary is
    /// delivered before Agent ownership is exposed. Auth rotation belongs to
    /// the transport adapter, which must consume close_signal atomically with
    /// its credential revocation (Task4), not treat this signal as that proof.
    pub(crate) fn revoke(
        &self,
        lease: &ViewerLease,
        reason: InterventionEnd,
    ) -> Result<(), BrowserError> {
        let mut st = self.lock();
        if lease.binding != st.live.binding || lease.resource_id != st.live.resource_id {
            return Err(BrowserError::ActionFailed("revoke binding mismatch".into()));
        }
        let Some(v) = st.viewers.get_mut(&lease.viewer_session_id) else {
            return Ok(());
        };
        if v.revoked || v.ws_generation != lease.ws_generation {
            return Ok(());
        }
        v.revoked = true;
        v.ws_generation = v
            .ws_generation
            .checked_add(1)
            .ok_or_else(|| BrowserError::ActionFailed("viewer WS generation exhausted".into()))?;
        v.permissions.clear();
        v.close.send_replace(true);
        Self::bump(&mut st)?;
        if matches!(&st.live.controller, Controller::Human { viewer_session_id, .. } if *viewer_session_id == lease.viewer_session_id)
        {
            self.finish(&mut st, reason)?;
            st.live.controller = Controller::Agent;
        }
        Ok(())
    }

    pub(crate) fn revoke_human_permission(&self, lease: &ViewerLease) -> Result<(), BrowserError> {
        let mut st = self.lock();
        Self::validate_lease(&st, lease).map_err(|reason| {
            BrowserError::ActionFailed(format!("human permission revoke refused: {reason:?}"))
        })?;
        st.viewers
            .get_mut(&lease.viewer_session_id)
            .unwrap()
            .permissions
            .retain(|p| *p == ViewerPermission::Frames);
        Self::bump(&mut st)?;
        if matches!(&st.live.controller, Controller::Human { viewer_session_id, .. } if *viewer_session_id == lease.viewer_session_id)
        {
            self.finish(&mut st, InterventionEnd::Revoked)?;
            st.live.controller = Controller::Agent;
        }
        Ok(())
    }

    /// Target/engine/run death, event loss, or cancellation of its lifetime.
    /// Idempotent; never removes the tombstone and thereby admits stale work.
    pub(crate) fn invalidate(&self, reason: InterventionEnd) -> Result<(), BrowserError> {
        let mut st = self.lock();
        if !st.alive {
            return Ok(());
        }
        st.alive = false;
        for v in st.viewers.values_mut() {
            v.revoked = true;
            v.ws_generation = v.ws_generation.saturating_add(1);
            v.permissions.clear();
            v.close.send_replace(true);
        }
        Self::bump(&mut st)?;
        self.finish(&mut st, reason)?;
        st.live.controller = Controller::Agent;
        Ok(())
    }
}

/// Linearize the backend's first poll against synchronous lifecycle events.
/// Permits are already acquired in stable target order and remain held through
/// every subsequent poll. Lifecycle callers must release their owner map lock
/// before invalidating controls, avoiding owner-map/control lock inversion.
pub(crate) async fn run_agent<T>(
    permits: &[AgentPermit],
    future: impl Future<Output = Result<T, BrowserError>>,
) -> Result<T, BrowserError> {
    let mut future = Box::pin(future);
    let mut started = false;
    poll_fn(|cx| {
        if !started {
            let guards: Vec<_> = permits.iter().map(|p| p.control.lock()).collect();
            for (guard, permit) in guards.iter().zip(permits) {
                if let Err(error) = TargetControl::validate_agent(guard, permit.epoch) {
                    return Poll::Ready(Err(error));
                }
            }
            started = true;
            future.as_mut().poll(cx)
        } else {
            future.as_mut().poll(cx)
        }
    })
    .await
}

fn valid_event(
    event: &HumanInputEvent,
    viewport: &aleph_protocol::browser_live::ViewportCssPx,
) -> bool {
    let point = |x: f64, y: f64, modifiers: u8| {
        x.is_finite()
            && y.is_finite()
            && x >= 0.0
            && y >= 0.0
            && x < f64::from(viewport.width)
            && y < f64::from(viewport.height)
            && modifiers <= 15
    };
    match event {
        HumanInputEvent::PointerMove {
            x_css_px,
            y_css_px,
            modifiers,
        } => point(*x_css_px, *y_css_px, *modifiers),
        HumanInputEvent::PointerDown {
            x_css_px,
            y_css_px,
            modifiers,
            click_count,
            ..
        }
        | HumanInputEvent::PointerUp {
            x_css_px,
            y_css_px,
            modifiers,
            click_count,
            ..
        } => point(*x_css_px, *y_css_px, *modifiers) && (1..=2).contains(click_count),
        HumanInputEvent::Wheel {
            x_css_px,
            y_css_px,
            modifiers,
            delta_x,
            delta_y,
        } => point(*x_css_px, *y_css_px, *modifiers) && delta_x.is_finite() && delta_y.is_finite(),
        HumanInputEvent::KeyDown {
            key,
            code,
            text,
            modifiers,
        } => {
            !key.is_empty()
                && key.len() <= 128
                && code.len() <= 128
                && *modifiers <= 15
                && text.as_ref().is_none_or(|t| t.len() <= 4096)
        }
        HumanInputEvent::KeyUp {
            key,
            code,
            modifiers,
        } => !key.is_empty() && key.len() <= 128 && code.len() <= 128 && *modifiers <= 15,
        HumanInputEvent::Text { text } => !text.is_empty() && text.len() <= 4096,
        HumanInputEvent::Navigate { url } => !url.is_empty() && url.len() <= 8192,
    }
}

#[cfg(test)]
mod tests;
