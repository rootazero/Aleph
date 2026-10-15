//! The single live owner of one browser profile.
//!
//! One profile has ONE CDP connection (the `EngineHandle`'s), and this owner
//! is the only thing that subscribes to that connection's screencast and
//! `Fetch.requestPaused` events on behalf of live consumers:
//!
//! * a viewer (`attach_target` / `subscribe_latest`) reads the newest frame of
//!   a per-target stream through a capacity-1 hub and can never slow the engine;
//! * a recording (`observe_recording`) rides the SAME engine screencast through
//!   a bounded tap and acks after its own write;
//! * the Fetch worker (`register_fetch`) receives its target's
//!   `Fetch.requestPaused` events from the owner's single dispatcher.
//!
//! Every frame is acknowledged to the engine exactly once: by the recorder
//! (after write) when its tap accepted the frame, otherwise by the owner.

pub(crate) mod control;
pub(crate) mod grant;
pub(crate) mod issuer;
#[allow(dead_code)] // consumed by the browser-live producer; boot wiring pending
pub(crate) mod lifecycle;
pub(crate) mod navigation_gate;
#[allow(dead_code)] // Task20 infrastructure only; no production composition yet
pub(crate) mod owner_registry;
#[allow(dead_code)] // consumed by the Task 6 issuer/producer wiring
pub(crate) mod resource;
pub(crate) mod session;
pub mod stream_hub;
pub(crate) mod viewer_backend;

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use aleph_cdp::methods::screencast::{self, ScreencastFormat};
use aleph_cdp::methods::target;
use aleph_cdp::{CdpConnection, CdpError, CdpEvent, SessionId, TargetId};
use aleph_protocol::browser_live::{
    BrowserLiveState, BrowserTargetBinding, ClosedReason, Controller, HumanInputEvent,
    InterventionEnd, ViewerBinding, ViewportCssPx,
};
use control::{AgentPermit, InterventionSink, TargetControl};
use tokio::sync::{mpsc, OwnedMutexGuard};

use crate::browser::cdp_backend::map_cdp_err;
use crate::browser::engine::Engine;
use crate::browser::error::BrowserError;
use navigation_gate::{
    BoundaryCause, BoundaryScope, ContentGate, DocumentBoundary, DocumentBoundarySink,
    OwnerTerminalSink,
};
use session::{lock_pending, EndReason, FrameAcker, RecordingTap, TargetStream};
use stream_hub::{LatestFrameHub, LatestFrameReceiver};

/// Buffered `Fetch.requestPaused` events between the dispatcher and the policy
/// worker. Past it events are dropped and counted, never buffered unboundedly.
const FETCH_FEED_CAPACITY: usize = 256;

/// Wire format of a viewer-started screencast.
const VIEWER_FORMAT: ScreencastFormat = ScreencastFormat::Jpeg { quality: 80 };

pub(crate) type SessionFuture =
    Pin<Box<dyn Future<Output = Result<SessionId, BrowserError>> + Send>>;
/// Maps a target id to its attached CDP session (attaching if needed).
pub(crate) type SessionResolver = Arc<dyn Fn(String) -> SessionFuture + Send + Sync>;

/// One target's `Fetch.requestPaused` events, as the owner's dispatcher routes
/// them. One feed per target by construction: registering replaces.
pub(crate) struct FetchFeed {
    pub(crate) rx: mpsc::Receiver<Arc<CdpEvent>>,
    pub(crate) dropped: Arc<AtomicU64>,
    owner: Weak<LiveProfileOwner>,
    target_id: String,
}

impl Drop for FetchFeed {
    /// The feed going away unhooks ITS slot only: a replacement registered for
    /// the same target in the meantime is a different slot (`dropped` is the
    /// slot's identity) and stays.
    fn drop(&mut self) {
        if let Some(owner) = self.owner.upgrade() {
            let mut st = owner.state();
            if st
                .fetch
                .get(&self.target_id)
                .is_some_and(|slot| Arc::ptr_eq(&slot.dropped, &self.dropped))
            {
                st.fetch.remove(&self.target_id);
            }
        }
    }
}

/// One viewer's reference on a target's shared viewer attachment (see
/// [`LiveProfileOwner::acquire_attachment`]). Holds the owner strongly; the
/// owner never holds a lease, so there is no cycle. Dropping the lease gives
/// the reference back synchronously; the last one out releases the
/// attachment on a background task. [`Self::release`] is the awaited form.
pub(crate) struct AttachmentLease {
    owner: Option<Arc<LiveProfileOwner>>,
    target_id: String,
    session: SessionId,
    epoch: u64,
}

impl AttachmentLease {
    /// Give the reference back and wait for the release it may trigger. The
    /// release itself runs on its own task, so cancelling the awaiter cannot
    /// strand an attachment whose last reference was just returned.
    pub(crate) async fn release(mut self) {
        if let Some(task) = self.release_inner() {
            let _ = task.await;
        }
    }

    /// Return the reference now; `Some` iff it was the last one and a release
    /// task was spawned.
    fn release_inner(&mut self) -> Option<tokio::task::JoinHandle<()>> {
        let owner = self.owner.take()?;
        if !owner.release_lease_ref(&self.target_id, self.epoch) {
            return None;
        }
        // No runtime (shutdown): the reference is already returned; the
        // owner's own teardown ends the stream.
        let runtime = tokio::runtime::Handle::try_current().ok()?;
        let target_id = std::mem::take(&mut self.target_id);
        let session = self.session.clone();
        let epoch = self.epoch;
        Some(runtime.spawn(async move {
            owner
                .finish_lease_release(&target_id, &session, epoch)
                .await;
        }))
    }
}

impl Drop for AttachmentLease {
    fn drop(&mut self) {
        // Dropping the handle detaches the task; it still runs to completion.
        drop(self.release_inner());
    }
}

struct FetchSlot {
    session: SessionId,
    tx: mpsc::Sender<Arc<CdpEvent>>,
    dropped: Arc<AtomicU64>,
}

/// Viewer attachment bookkeeping for one stream epoch. `refs` counts live
/// [`AttachmentLease`]s; `legacy` marks an attachment established by the
/// legacy [`LiveProfileOwner::attach_target`] whose owner releases it with
/// `close_target`, so the last lease must not tear it down. An entry whose
/// `epoch` differs from the stream's is stale (the stream was ended or
/// re-armed) and is never counted against the new stream.
#[derive(Clone, Copy)]
struct AttachmentRefs {
    epoch: u64,
    refs: usize,
    legacy: bool,
}

#[derive(Default)]
struct State {
    attachments: HashMap<String, AttachmentRefs>,
    next_epoch: u64,
    ended: bool,
    routing_retired: bool,
    destroyed: std::collections::HashSet<String>,
    streams: HashMap<String, TargetStream>,
    fetch: HashMap<String, FetchSlot>,
    // Closed controls stay as tombstones (rejecting agent verbs on that target)
    // until `bind_control` replaces one on a re-attach, or the owner dies.
    controls: HashMap<String, Arc<TargetControl>>,
    /// Per-target content generation. Persists across stream re-creation (a
    /// re-armed stream keeps counting) and is advanced ONLY by
    /// [`LiveProfileOwner::declare_boundary`], under this lock.
    gates: HashMap<String, ContentGate>,
    /// The CDP session each target was last seen on, so a page event (which
    /// carries only a session) is attributed to an exact target id. An event
    /// from a session not in here is NOT guessed: it fences every target.
    sessions: HashMap<String, SessionId>,
    /// The main frame id of a target, learnt from its top-level
    /// `Page.frameNavigated`; used only to tell sub-frame same-document
    /// navigations from the main frame's.
    main_frames: HashMap<String, String>,
}

impl State {
    fn gate_for(&mut self, target_id: &str) -> ContentGate {
        self.gates.entry(target_id.to_string()).or_default().clone()
    }

    fn target_for_session(&self, session: &SessionId) -> Option<String> {
        if let Some((target, _)) = self.sessions.iter().find(|(_, s)| *s == session) {
            return Some(target.clone());
        }
        self.streams
            .iter()
            .find(|(_, stream)| stream.session == *session)
            .map(|(target, _)| target.clone())
    }

    /// Start a new content generation for `target_id` and drop its stored
    /// frame, so nothing of the old document can be handed out afterwards.
    fn advance_target(&mut self, target_id: &str) {
        self.gate_for(target_id).advance();
        if let Some(stream) = self.streams.get(target_id) {
            stream.hub.clear();
        }
    }
}

pub(crate) struct LiveProfileOwner {
    profile: String,
    engine: Engine,
    conn: CdpConnection,
    resolver: SessionResolver,
    state: Mutex<State>,
    /// Events the dispatcher never saw (broadcast overflow), cumulative.
    lagged: AtomicU64,
    /// Serialises start/stop of engine screencasts so two callers racing on one
    /// target cannot both start (or one stop under the other's feet).
    lifecycle: Arc<tokio::sync::Mutex<()>>,
    // Excludes control registration during an agent operation, including open
    // and global state changes whose affected target does not exist yet.
    mutations: tokio::sync::Mutex<()>,
    browser_session_id: String,
    /// Receivers of document boundaries (the lifecycle). A leaf lock: nothing
    /// else is acquired while it is held, and it is released before any sink
    /// runs.
    boundary_sinks: Mutex<Vec<Arc<dyn DocumentBoundarySink>>>,
    /// Receivers of exact-target destruction and owner end (the lifecycle). A
    /// leaf lock, released before any sink runs.
    terminal_sinks: Mutex<Vec<Arc<dyn OwnerTerminalSink>>>,
    /// Set by the first `notify_owner_ended`: `owner_ended` is delivered at
    /// most once per owner however many fail-closed / end paths run.
    owner_ended_notified: AtomicBool,
}

impl LiveProfileOwner {
    /// Build the owner over the profile's existing connection. Subscribes to
    /// the connection's events synchronously, so nothing sent after this
    /// returns can be missed, then spawns the one dispatcher.
    pub(crate) fn new(
        profile: &str,
        engine: Engine,
        conn: CdpConnection,
        resolver: SessionResolver,
    ) -> Arc<Self> {
        let mut events = conn.events();
        let mut closed = conn.closed();
        let owner = Arc::new(Self {
            profile: profile.to_string(),
            engine,
            conn,
            resolver,
            state: Mutex::new(State::default()),
            lagged: AtomicU64::new(0),
            lifecycle: Arc::new(tokio::sync::Mutex::new(())),
            mutations: tokio::sync::Mutex::new(()),
            browser_session_id: uuid::Uuid::new_v4().to_string(),
            boundary_sinks: Mutex::new(Vec::new()),
            terminal_sinks: Mutex::new(Vec::new()),
            owner_ended_notified: AtomicBool::new(false),
        });
        let weak = Arc::downgrade(&owner);
        tokio::spawn(async move {
            // The dispatcher's only natural wakeups are `events.next()` and
            // `closed.changed()`. A connection that has gone silent (the engine
            // is wedged, the websocket is half-open, the broadcast buffer
            // dropped frames and produced no further event) parks forever in
            // `events.next()`, and the silent loss shows up only the next
            // time somebody pushes a frame. Tick `lagged()` on an interval so
            // a stuck dispatcher fails closed within a known time, without
            // having to wait for the next event.
            let mut lag_tick = tokio::time::interval(Duration::from_secs(1));
            // First tick fires immediately; skip it so we do not double-check
            // the broadcast's own initial counter.
            lag_tick.tick().await;
            while closed.borrow().is_none() {
                tokio::select! {
                    ev = events.next() => match ev {
                        Some(ev) => match weak.upgrade() {
                            Some(owner) => {
                                owner.note_lag(events.lagged());
                                owner.dispatch(&ev);
                            }
                            None => return,
                        },
                        None => break,
                    },
                    _ = closed.changed() => break,
                    _ = lag_tick.tick() => {
                        if let Some(owner) = weak.upgrade() {
                            owner.note_lag(events.lagged());
                        } else {
                            return;
                        }
                    }
                }
            }
            if let Some(owner) = weak.upgrade() {
                owner.end_all(EndReason::EngineGone);
            }
        });
        owner
    }

    /// A subscriber that falls behind loses events (their frames stay unacked
    /// until the engine's own timeout); that must be visible, not silent.
    fn note_lag(self: &Arc<Self>, total: u64) {
        let before = self.lagged.swap(total, Ordering::Relaxed);
        if total > before {
            tracing::warn!(
                profile = %self.profile,
                missed = total - before,
                total,
                "live owner dispatcher lagged — CDP events were dropped"
            );
            self.fail_closed_after_lag();
        }
    }

    /// Lost events mean unknown paused requests and unacked frames. Do not
    /// guess a recovery: end every stream (viewers/recordings see the end),
    /// stop their screencasts, and close every fetch target so its paused
    /// requests are cancelled by the engine rather than left hanging or
    /// continued blindly.
    fn fail_closed_after_lag(self: &Arc<Self>) {
        // Unknown events may have been navigations: no approval survives them.
        self.declare_boundary(BoundaryScope::AllTargets, BoundaryCause::EventGap);
        self.state().ended = true;
        self.end_controls(None, InterventionEnd::TargetClosed);
        let (streams, fetch) = {
            let mut st = self.state();
            st.attachments.clear();
            (
                std::mem::take(&mut st.streams),
                std::mem::take(&mut st.fetch),
            )
        };
        for (target, stream) in streams {
            let session = stream.session.clone();
            stream.end(EndReason::EngineGone);
            // Target-aware: a re-armed stream for the same target in the
            // milliseconds between drain and stop must not be torn down.
            self.spawn_stop_if_unowned(target, session);
        }
        for (target, _slot) in fetch {
            self.spawn_close_target(target);
        }
        // The lag protocol has no `Lagged` reason: fail-closed maps to
        // `EngineExited` (the owner's live view of the engine is gone).
        self.notify_owner_ended(ClosedReason::EngineExited);
    }

    fn spawn_close_target(self: &Arc<Self>, target: String) {
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let owner = self.clone();
        rt.spawn(async move {
            // Unconditional on purpose. This close exists because a
            // `Fetch.requestPaused` event was LOST (dispatcher lag or a full
            // feed): that request is paused in the engine and nobody can ever
            // answer it — a feed re-armed afterwards never saw it either. The
            // only fail-closed exit is to close the target so the engine
            // cancels it. Skipping the close because a fresh slot exists
            // would leave the lost request hanging forever (I4). The
            // lifecycle lock only orders the close after any in-flight
            // register/attach step; a re-armed loop then sees the target die
            // through `targetDestroyed` and exits on its own.
            let _life = owner.lifecycle.lock().await;
            tracing::warn!(
                target = %target,
                "closing target after lost Fetch events (fail-closed)"
            );
            let _ = target::close_target(&owner.conn, &TargetId(target)).await;
        });
    }

    #[cfg(test)]
    pub(crate) fn lagged_events(&self) -> u64 {
        self.lagged.load(Ordering::Relaxed)
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The principal-scoped profile key this owner was created for.
    pub(crate) fn profile(&self) -> &str {
        &self.profile
    }

    pub(crate) fn browser_session_id(&self) -> &str {
        &self.browser_session_id
    }

    /// Routing liveness is derived from this owner's real event/connection
    /// state; retirement adds no CDP call and does not guess another owner.
    pub(crate) fn route_available(&self, target_id: &str) -> bool {
        let st = self.state();
        !target_id.is_empty()
            && !st.ended
            && !st.routing_retired
            && !st.destroyed.contains(target_id)
            && self.conn.closed().borrow().is_none()
    }

    /// Why a route to `target_id` on this owner is unavailable, for closing a
    /// resource published against it. Owner ended / connection closed is
    /// `EngineExited` (same as the terminal sinks' owner-end notification);
    /// an exact destroyed target is `TargetClosed`. Undeterminable (e.g. routing
    /// retired without either) conservatively reports `TargetClosed`.
    pub(crate) fn route_close_reason(&self, target_id: &str) -> ClosedReason {
        let st = self.state();
        if st.ended || self.conn.closed().borrow().is_some() {
            ClosedReason::EngineExited
        } else {
            // Exact target destroyed, or undeterminable: conservative default.
            let _destroyed = st.destroyed.contains(target_id);
            ClosedReason::TargetClosed
        }
    }

    pub(crate) fn retire_routing(&self) {
        self.state().routing_retired = true;
    }

    /// Install an internal control capability for an ALREADY attached exact
    /// target. No auth/grant is issued here. Epochs and browser-session identity
    /// come from this one owner, not a client or profile active-tab pointer.
    pub(crate) async fn bind_control(
        &self,
        resource_id: &str,
        binding: ViewerBinding,
        viewport: ViewportCssPx,
        sink: InterventionSink,
    ) -> Result<Arc<TargetControl>, BrowserError> {
        let _mutation = self.mutations.lock().await;
        if self.state().ended
            || resource_id.is_empty()
            || binding.target.profile != self.profile
            || binding.target.browser_session_id != self.browser_session_id
            || viewport.width == 0
            || viewport.height == 0
            || self.conn.closed().borrow().is_some()
        {
            return Err(BrowserError::ActionFailed(
                "live control binding does not name this browser owner".into(),
            ));
        }
        // Controls are inserted only here, under `mutations`, and only ever go
        // live -> dead. Probe liveness BEFORE taking the owner map lock (never
        // lock a control while holding it); a dead control is a tombstone that
        // a re-attach after `close_target` may replace, a live one may not.
        let existing = self
            .state()
            .controls
            .get(&binding.target.target_id)
            .cloned();
        if existing.is_some_and(|c| c.is_live()) {
            return Err(BrowserError::ActionFailed(
                "target already has a control binding; explicit revoke/rebind required".into(),
            ));
        }
        let mut st = self.state();
        if st.ended || st.destroyed.contains(&binding.target.target_id) {
            return Err(BrowserError::TabNotFound(binding.target.target_id));
        }
        let stream = st
            .streams
            .get(&binding.target.target_id)
            .filter(|s| s.attached)
            .ok_or_else(|| BrowserError::TabNotFound(binding.target.target_id.clone()))?;
        let control = TargetControl::new(
            BrowserLiveState {
                resource_id: resource_id.to_string(),
                binding: binding.clone(),
                connection_epoch: 1,
                stream_epoch: stream.epoch,
                viewport_css_px: viewport,
                controller: Controller::Agent,
                control_epoch: 0,
            },
            sink,
        );
        st.controls
            .insert(binding.target.target_id, control.clone());
        Ok(control)
    }

    /// The sole CDP backend mutation gate. A target verb acquires only that
    /// target's control permit; open/global mutations acquire all live target
    /// permits in stable order. Registration cannot race the empty-map case.
    /// No backend future is polled until all permits are acquired/revalidated.
    pub(crate) async fn mutate<T>(
        &self,
        target_id: Option<&str>,
        future: impl Future<Output = Result<T, BrowserError>>,
    ) -> Result<T, BrowserError> {
        let deadline = tokio::time::Instant::now() + Duration::from_millis(250);
        let _mutation = tokio::time::timeout_at(deadline, self.mutations.lock())
            .await
            .map_err(|_| {
                BrowserError::ActionFailed("browser mutation permit delayed over 250ms".into())
            })?;
        let controls = {
            let st = self.state();
            if st.ended || self.conn.closed().borrow().is_some() {
                return Err(BrowserError::ActionFailed(
                    "browser live owner is closed".into(),
                ));
            }
            let mut controls: Vec<_> = st
                .controls
                .iter()
                .filter(|(target, _)| target_id.is_none_or(|id| id == target.as_str()))
                .map(|(id, control)| (id.clone(), control.clone()))
                .collect();
            controls.sort_by(|a, b| a.0.cmp(&b.0));
            controls
        };
        let mut permits: Vec<AgentPermit> = Vec::with_capacity(controls.len());
        for (_, control) in controls {
            // A target tombstone rejects operations on that target, not open
            // or profile-wide operations on its surviving sibling targets.
            // Never take control locks while holding the owner map lock.
            if target_id.is_none() && !control.is_live() {
                continue;
            }
            permits.push(
                tokio::time::timeout_at(deadline, control.agent_permit())
                    .await
                    .map_err(|_| {
                        BrowserError::ActionFailed(
                            "browser mutation permit delayed over 250ms".into(),
                        )
                    })??,
            );
        }
        control::run_agent(&permits, future).await
    }

    fn end_controls(&self, target: Option<&str>, reason: InterventionEnd) {
        let controls: Vec<_> = {
            let st = self.state();
            st.controls
                .iter()
                .filter(|(id, _)| target.is_none_or(|t| t == id.as_str()))
                .map(|(_, c)| c.clone())
                .collect()
        };
        for control in controls {
            if let Err(error) = control.invalidate(reason) {
                tracing::error!(%error, "browser intervention destination delivery failed; control remains closed");
            }
        }
    }

    // ---- viewer API ----

    /// Bind a viewer to a target: ensures the target's one engine screencast.
    /// `browser_session_id` is carried by the binding but not validated here —
    /// that is the gateway's job (Task 3).
    ///
    /// Returns `true` iff THIS call established the viewer attachment (the
    /// stream was not `attached` when read under the lifecycle lock), `false`
    /// when a viewer was already attached. The lifecycle lock makes this the
    /// single atomic ownership decision: only the `true` caller may unwind the
    /// attachment with `close_target`. Errors (retired/ended owner, destroyed
    /// target) never return `true`.
    #[cfg(test)] // only tests read this; production has no caller
    pub(crate) async fn attach_target(
        self: &Arc<Self>,
        binding: BrowserTargetBinding,
    ) -> Result<bool, BrowserError> {
        if binding.profile != self.profile {
            return Err(BrowserError::ActionFailed(format!(
                "binding is for profile '{}', this owner serves '{}'",
                binding.profile, self.profile
            )));
        }
        let session = self.resolve_session(&binding.target_id).await?;
        let life = self.lifecycle.clone().lock_owned().await;
        let was_attached = {
            let st = self.state();
            // Same liveness predicate as route lease validation
            // (`route_available`): a retired owner accepts no new attachment.
            if st.ended || st.routing_retired || st.destroyed.contains(&binding.target_id) {
                return Err(BrowserError::TabNotFound(binding.target_id));
            }
            st.streams
                .get(&binding.target_id)
                .is_some_and(|stream| stream.attached)
        };
        let _life = self
            .ensure_stream(&binding.target_id, &session, VIEWER_FORMAT, life)
            .await
            .map_err(|e| map_cdp_err(self.engine, "Page.startScreencast", e))?;
        // The target may have been destroyed while the start was in flight
        // (Target.targetDestroyed removes the stream): that is not an attach.
        let mut guard = self.state();
        let st = &mut *guard;
        match st.streams.get_mut(&binding.target_id) {
            Some(stream) if stream.session == session => {
                stream.attached = true;
                let epoch = stream.epoch;
                // A zero-reference lease-owned attachment is only waiting for
                // its release task: this caller takes it over (legacy
                // ownership), so that release finds `legacy` and stands down.
                let adopt = st
                    .attachments
                    .get(&binding.target_id)
                    .is_some_and(|e| e.epoch == epoch && e.refs == 0 && !e.legacy);
                if !was_attached || adopt {
                    // Legacy ownership: only `close_target` releases it.
                    st.attachments.insert(
                        binding.target_id.clone(),
                        AttachmentRefs {
                            epoch,
                            refs: 0,
                            legacy: true,
                        },
                    );
                }
                Ok(!was_attached || adopt)
            }
            _ => Err(BrowserError::TabNotFound(binding.target_id)),
        }
    }

    /// Reference-counted viewer attachment. Many viewers share ONE engine
    /// screencast; the returned lease keeps it attached and the LAST lease out
    /// releases it (ends the viewer controls, drops the viewer hub, and stops
    /// the screencast unless a recording still holds the stream). Unlike
    /// [`Self::attach_target`] the caller never has to know whether it "owns"
    /// the attachment, so one viewer's failure or cancellation cannot tear
    /// down another's.
    ///
    /// Strict identity: the binding must name this owner's profile AND browser
    /// session. Cancel-safe: dropping the future mid-start rolls the stream back
    /// (`StartRollback`) and no reference is taken before the final
    /// synchronous step.
    pub(crate) async fn acquire_attachment(
        self: &Arc<Self>,
        binding: BrowserTargetBinding,
    ) -> Result<AttachmentLease, BrowserError> {
        if binding.profile != self.profile {
            return Err(BrowserError::ActionFailed(format!(
                "binding is for profile '{}', this owner serves '{}'",
                binding.profile, self.profile
            )));
        }
        if binding.browser_session_id != self.browser_session_id {
            return Err(BrowserError::ActionFailed(
                "binding is for a different browser session".into(),
            ));
        }
        let session = self.resolve_session(&binding.target_id).await?;
        let life = self.lifecycle.clone().lock_owned().await;
        {
            let st = self.state();
            if st.ended || st.routing_retired || st.destroyed.contains(&binding.target_id) {
                return Err(BrowserError::TabNotFound(binding.target_id));
            }
        }
        let _life = self
            .ensure_stream(&binding.target_id, &session, VIEWER_FORMAT, life)
            .await
            .map_err(|e| map_cdp_err(self.engine, "Page.startScreencast", e))?;
        // Synchronous from here to the lease: nothing can cancel between the
        // increment and the lease that owns it.
        let mut guard = self.state();
        let st = &mut *guard;
        match st.streams.get_mut(&binding.target_id) {
            Some(stream) if stream.session == session => {
                stream.attached = true;
                let epoch = stream.epoch;
                let entry =
                    st.attachments
                        .entry(binding.target_id.clone())
                        .or_insert(AttachmentRefs {
                            epoch,
                            refs: 0,
                            legacy: false,
                        });
                if entry.epoch != epoch {
                    *entry = AttachmentRefs {
                        epoch,
                        refs: 0,
                        legacy: false,
                    };
                }
                entry.refs += 1;
                Ok(AttachmentLease {
                    owner: Some(self.clone()),
                    target_id: binding.target_id,
                    session,
                    epoch,
                })
            }
            _ => Err(BrowserError::TabNotFound(binding.target_id)),
        }
    }

    /// Synchronously give back one reference. `true` iff it was the last one
    /// of a lease-owned attachment of the same epoch, i.e. a release must
    /// follow under the lifecycle lock.
    fn release_lease_ref(&self, target_id: &str, epoch: u64) -> bool {
        let mut st = self.state();
        match st.attachments.get_mut(target_id) {
            Some(entry) if entry.epoch == epoch && entry.refs > 0 => {
                entry.refs -= 1;
                entry.refs == 0 && !entry.legacy
            }
            _ => false,
        }
    }

    /// Release a lease-owned attachment that dropped to zero references,
    /// unless it was re-acquired / re-armed / closed since. Takes the
    /// lifecycle lock, so it serialises with `acquire_attachment` and
    /// `close_target`; the checks below are the generation recheck.
    async fn finish_lease_release(&self, target_id: &str, session: &SessionId, epoch: u64) {
        let life = self.lifecycle.clone().lock_owned().await;
        let still_ours = {
            let mut st = self.state();
            let entry = st.attachments.get(target_id).copied();
            match entry {
                Some(e) if e.epoch == epoch && e.refs == 0 && !e.legacy => {
                    let live = st
                        .streams
                        .get(target_id)
                        .is_some_and(|s| s.epoch == epoch && s.session == *session && s.attached);
                    if !live {
                        // Stream already gone / re-armed: just forget the entry.
                        st.attachments.remove(target_id);
                    }
                    live
                }
                _ => false,
            }
        };
        if still_ours {
            self.detach_locked(target_id, life).await;
        }
    }

    /// Drop the viewer binding. Idempotent. A live recording keeps the engine
    /// screencast running; without one the screencast stops.
    #[cfg(test)] // only tests read this; production has no caller
    pub(crate) async fn close_target(&self, target_id: &str) -> Result<(), BrowserError> {
        let life = self.lifecycle.clone().lock_owned().await;
        self.detach_locked(target_id, life).await;
        Ok(())
    }

    /// Force-release the viewer attachment of a target. Caller holds
    /// `lifecycle`. Outstanding leases of the ended epoch become stale and
    /// release nothing.
    async fn detach_locked(&self, target_id: &str, life: OwnedMutexGuard<()>) {
        self.end_controls(Some(target_id), InterventionEnd::TargetClosed);
        let removed = {
            let mut st = self.state();
            st.attachments.remove(target_id);
            let epoch = st.next_epoch;
            let gate = st.gate_for(target_id);
            let Some(stream) = st.streams.get_mut(target_id) else {
                return;
            };
            stream.attached = false;
            if stream.tap.is_some() {
                // Viewers end (old hub dropped); the recording's stream goes on
                // under a fresh epoch and hub.
                stream.hub = LatestFrameHub::with_gate(gate);
                stream.epoch = epoch;
                st.next_epoch += 1;
                None
            } else {
                st.streams.remove(target_id)
            }
        };
        if let Some(stream) = removed {
            // Cleanup retains the lifecycle lock even if this caller is cancelled.
            stop_screencast_detached(&self.conn, stream.session, life).await;
        }
    }

    /// Newest-frame receiver for an attached target. Dropping it detaches only
    /// this viewer.
    pub(crate) fn subscribe_latest(
        &self,
        target_id: &str,
    ) -> Result<LatestFrameReceiver, BrowserError> {
        // The receiver captures the target's content generation under this
        // lock, the same lock a document boundary advances it under, so
        // "approved before the boundary" is exact, not a timing guess.
        match self.state().streams.get(target_id) {
            Some(stream) if stream.attached => Ok(stream.hub.subscribe()),
            _ => Err(BrowserError::TabNotFound(target_id.to_string())),
        }
    }

    /// Dispatch one admitted human input event to a target. Lease, epoch, and
    /// permission validation happen in `TargetControl::queue_input`/`dispatch`
    /// BEFORE the transport reaches this; navigation URLs are SSRF-checked in
    /// `viewer_backend` before that. This is the mechanical CDP mapping only —
    /// the viewer transport itself never touches the connection (R1/R4).
    ///
    /// A human is not an agent, so this does NOT take an `AgentPermit` (the
    /// control plane would rightly refuse one while a Human holds control).
    /// It does take the owner's global `mutations` gate, bounded to 250ms like
    /// `mutate`, so the resolver and CDP call never run concurrently with an
    /// agent/open/global mutation on the same connection.
    // Test-only: Panel/TUI viewers are read-only; no production input path.
    #[cfg(test)]
    pub(crate) async fn dispatch_human_input(
        &self,
        target_id: &str,
        event: HumanInputEvent,
    ) -> Result<(), BrowserError> {
        self.dispatch_human_input_with_route(target_id, event, None)
            .await
    }

    pub(crate) async fn dispatch_routed_human_input(
        &self,
        route: &owner_registry::OwnerRouteLease,
        event: HumanInputEvent,
    ) -> Result<(), BrowserError> {
        if !std::ptr::eq(self, route.owner().as_ref()) {
            return Err(BrowserError::ActionFailed(
                "foreign exact owner lease".into(),
            ));
        }
        self.dispatch_human_input_with_route(&route.binding().target_id, event, Some(route))
            .await
    }

    async fn dispatch_human_input_with_route(
        &self,
        target_id: &str,
        event: HumanInputEvent,
        route: Option<&owner_registry::OwnerRouteLease>,
    ) -> Result<(), BrowserError> {
        let _mutation = tokio::time::timeout(Duration::from_millis(250), self.mutations.lock())
            .await
            .map_err(|_| {
                BrowserError::ActionFailed("browser mutation permit delayed over 250ms".into())
            })?;
        if self.state().ended || self.conn.closed().borrow().is_some() {
            return Err(BrowserError::ActionFailed(
                "browser live owner is closed".into(),
            ));
        }
        if let Some(route) = route {
            route.validate()?;
        }
        self.dispatch_human_input_gated(target_id, event, route)
            .await
    }

    /// CDP mapping; the caller holds the `mutations` gate.
    async fn dispatch_human_input_gated(
        &self,
        target_id: &str,
        event: HumanInputEvent,
        route: Option<&owner_registry::OwnerRouteLease>,
    ) -> Result<(), BrowserError> {
        use aleph_cdp::methods::input::{
            self, KeyEvent, KeyType, MouseButton, MouseEvent, MouseType,
        };
        use aleph_cdp::methods::page;
        use aleph_protocol::browser_live::PointerButton;

        let session = self.resolve_session(target_id).await?;
        // Recheck after the async resolver, immediately before the CDP send.
        if let Some(route) = route {
            route.validate()?;
        }
        let session = Some(&session);
        let result = match event {
            HumanInputEvent::PointerMove {
                x_css_px,
                y_css_px,
                modifiers,
            } => {
                input::dispatch_mouse_event(
                    &self.conn,
                    session,
                    &MouseEvent {
                        r#type: MouseType::Moved,
                        x: x_css_px,
                        y: y_css_px,
                        button: MouseButton::None,
                        click_count: 0,
                        modifiers: u32::from(modifiers),
                        delta_x: 0.0,
                        delta_y: 0.0,
                    },
                )
                .await
            }
            HumanInputEvent::PointerDown {
                x_css_px,
                y_css_px,
                button,
                click_count,
                modifiers,
            } => {
                let button = match button {
                    PointerButton::Left => MouseButton::Left,
                    PointerButton::Right => MouseButton::Right,
                    PointerButton::Middle => MouseButton::Middle,
                };
                input::dispatch_mouse_event(
                    &self.conn,
                    session,
                    &MouseEvent {
                        r#type: MouseType::Pressed,
                        x: x_css_px,
                        y: y_css_px,
                        button,
                        click_count: u32::from(click_count),
                        modifiers: u32::from(modifiers),
                        delta_x: 0.0,
                        delta_y: 0.0,
                    },
                )
                .await
            }
            HumanInputEvent::PointerUp {
                x_css_px,
                y_css_px,
                button,
                click_count,
                modifiers,
            } => {
                let button = match button {
                    PointerButton::Left => MouseButton::Left,
                    PointerButton::Right => MouseButton::Right,
                    PointerButton::Middle => MouseButton::Middle,
                };
                input::dispatch_mouse_event(
                    &self.conn,
                    session,
                    &MouseEvent {
                        r#type: MouseType::Released,
                        x: x_css_px,
                        y: y_css_px,
                        button,
                        click_count: u32::from(click_count),
                        modifiers: u32::from(modifiers),
                        delta_x: 0.0,
                        delta_y: 0.0,
                    },
                )
                .await
            }
            HumanInputEvent::Wheel {
                x_css_px,
                y_css_px,
                delta_x,
                delta_y,
                modifiers,
            } => {
                input::dispatch_mouse_event(
                    &self.conn,
                    session,
                    &MouseEvent {
                        r#type: MouseType::Wheel,
                        x: x_css_px,
                        y: y_css_px,
                        button: MouseButton::None,
                        click_count: 0,
                        modifiers: u32::from(modifiers),
                        delta_x,
                        delta_y,
                    },
                )
                .await
            }
            HumanInputEvent::KeyDown {
                key,
                code,
                text,
                modifiers,
            } => {
                input::dispatch_key_event(
                    &self.conn,
                    session,
                    &KeyEvent {
                        r#type: KeyType::KeyDown,
                        key,
                        code,
                        text,
                        windows_virtual_key_code: None,
                        modifiers: u32::from(modifiers),
                    },
                )
                .await
            }
            HumanInputEvent::KeyUp {
                key,
                code,
                modifiers,
            } => {
                input::dispatch_key_event(
                    &self.conn,
                    session,
                    &KeyEvent {
                        r#type: KeyType::KeyUp,
                        key,
                        code,
                        text: None,
                        windows_virtual_key_code: None,
                        modifiers: u32::from(modifiers),
                    },
                )
                .await
            }
            HumanInputEvent::Text { text } => input::insert_text(&self.conn, session, &text).await,
            HumanInputEvent::Navigate { url } => {
                // Before the engine is told: the old page's approval must be
                // gone by the time the first frame of the new one can exist.
                self.declare_boundary_off_control_lock(
                    BoundaryScope::Target(target_id.to_string()),
                    BoundaryCause::HumanNavigation,
                )
                .await;
                page::navigate(&self.conn, session, &url).await.map(|_| ())
            }
        };
        result.map_err(|e| map_cdp_err(self.engine, "viewer input", e))
    }

    // ---- recording observer ----

    /// Observe a target's frames for a recording. Starts the engine screencast
    /// (with `format`) only if no viewer already did.
    pub(crate) async fn observe_recording(
        self: &Arc<Self>,
        target_id: &str,
        session: &SessionId,
        format: ScreencastFormat,
    ) -> Result<RecordingTap, CdpError> {
        let life = self.lifecycle.clone().lock_owned().await;
        let _life = self.ensure_stream(target_id, session, format, life).await?;
        let (slot, tap) = RecordingTap::pair(&self.conn, session);
        match self.state().streams.get_mut(target_id) {
            Some(stream) => stream.tap = Some(slot),
            // The stream ended (tab destroyed) between the start and now: a
            // tap on nothing would be a recording that never gets a frame.
            None => {
                return Err(CdpError::Transport(
                    "target ended while the recording was starting".to_string(),
                ))
            }
        }
        Ok(tap)
    }

    /// The recording is over. Without a viewer the engine screencast stops too.
    /// `tap_id` is the `captured` counter handed out by `observe_recording`: it
    /// names ONE tap, so a late release from a finished recording can never
    /// close the tap of a newer one on the same target.
    pub(crate) async fn release_recording(&self, target_id: &str, tap_id: &Arc<AtomicU64>) {
        let life = self.lifecycle.clone().lock_owned().await;
        let removed = {
            let mut st = self.state();
            let Some(stream) = st.streams.get_mut(target_id) else {
                return;
            };
            if !stream
                .tap
                .as_ref()
                .is_some_and(|t| Arc::ptr_eq(&t.captured, tap_id))
            {
                return;
            }
            stream.tap = None;
            if stream.attached {
                None
            } else {
                st.streams.remove(target_id)
            }
        };
        if let Some(stream) = removed {
            stop_screencast_detached(&self.conn, stream.session, life).await;
        }
    }

    // ---- Fetch worker feed ----

    /// Route this target's `Fetch.requestPaused` events to a new feed. Replaces
    /// any earlier feed, so a target never has two consumers.
    ///
    /// Async on purpose: the slot is installed under `lifecycle` so the
    /// matching `spawn_close_target` (which also takes that lock) cannot
    /// race a re-arm. The lock is short — a single map insert — so the cost
    /// is not on the hot path.
    pub(crate) async fn register_fetch(
        self: &Arc<Self>,
        target_id: &str,
        session: &SessionId,
    ) -> FetchFeed {
        let _life = self.lifecycle.lock().await;
        let (tx, rx) = mpsc::channel(FETCH_FEED_CAPACITY);
        let dropped = Arc::new(AtomicU64::new(0));
        self.state().fetch.insert(
            target_id.to_string(),
            FetchSlot {
                session: session.clone(),
                tx,
                dropped: dropped.clone(),
            },
        );
        FetchFeed {
            rx,
            dropped,
            owner: Arc::downgrade(self),
            target_id: target_id.to_string(),
        }
    }

    // ---- internals ----

    /// Start the target's engine screencast unless a stream for this exact
    /// session already exists. Caller holds `lifecycle`. Cancel-safe: if the
    /// future is dropped mid-start, the half-built stream is rolled back and
    /// the engine screencast (which may have started) is stopped.
    async fn ensure_stream(
        self: &Arc<Self>,
        target_id: &str,
        session: &SessionId,
        format: ScreencastFormat,
        life: OwnedMutexGuard<()>,
    ) -> Result<OwnedMutexGuard<()>, CdpError> {
        let epoch = {
            let mut st = self.state();
            match st.streams.get(target_id) {
                Some(stream) if stream.session == *session => return Ok(life),
                Some(_) => {
                    // The target now lives in a different session: the old
                    // stream is dead.
                    if let Some(old) = st.streams.remove(target_id) {
                        old.end(EndReason::TabDestroyed);
                    }
                }
                None => {}
            }
            let epoch = st.next_epoch;
            st.next_epoch += 1;
            let gate = st.gate_for(target_id);
            st.sessions.insert(target_id.to_string(), session.clone());
            // Inserted BEFORE the start so the first frame finds its stream.
            st.streams.insert(
                target_id.to_string(),
                TargetStream::new(epoch, session.clone()).with_gate(gate),
            );
            epoch
        };
        let mut rollback = StartRollback {
            owner: self.clone(),
            target_id: target_id.to_string(),
            session: session.clone(),
            epoch,
            life: Some(life),
            armed: true,
        };
        let started =
            screencast::start_screencast(&self.conn, Some(session), format, None, None, None).await;
        if let Err(e) = started {
            if matches!(&e, CdpError::Protocol { .. }) {
                // Only an explicit refusal proves no screencast was started.
                rollback.armed = false;
                self.remove_stream_if_epoch(target_id, epoch);
            }
            // Timeout/transport/decode errors leave the engine outcome unknown.
            // Keep rollback armed and preserve the original error category.
            return Err(e);
        }
        rollback.armed = false;
        Ok(rollback.life.take().expect("start owns the lifecycle lock"))
    }

    fn remove_stream_if_epoch(&self, target_id: &str, epoch: u64) -> bool {
        let mut st = self.state();
        if st.streams.get(target_id).is_some_and(|s| s.epoch == epoch) {
            st.streams.remove(target_id);
            true
        } else {
            false
        }
    }

    /// Stop the engine screencast of a stream that was just forgotten, unless
    /// somebody re-created a stream for the target in the meantime. Runs on its
    /// own task because the callers (a `Drop`, the sync dispatcher) cannot await.
    fn spawn_stop_if_unowned(self: &Arc<Self>, target_id: String, session: SessionId) {
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let owner = self.clone();
        rt.spawn(async move {
            let _life = owner.lifecycle.lock().await;
            if !owner.state().streams.contains_key(&target_id) {
                let _ = screencast::stop_screencast(&owner.conn, Some(&session)).await;
            }
        });
    }

    pub(crate) fn retire(self: &Arc<Self>) {
        self.fail_closed_after_lag();
    }

    fn end_all(&self, reason: EndReason) {
        self.declare_boundary(BoundaryScope::AllTargets, BoundaryCause::EventGap);
        self.state().ended = true;
        self.end_controls(None, InterventionEnd::TargetClosed);
        let streams: Vec<TargetStream> = {
            let mut st = self.state();
            st.fetch.clear();
            st.attachments.clear();
            st.streams.drain().map(|(_, s)| s).collect()
        };
        for stream in streams {
            stream.end(reason);
        }
        // Idempotent: a duplicate `end_all` (or a preceding fail-closed) does
        // not notify, hence not close, twice.
        self.notify_owner_ended(ClosedReason::EngineExited);
    }

    fn dispatch(self: &Arc<Self>, ev: &Arc<CdpEvent>) {
        match ev.method.as_str() {
            "Page.screencastFrame" => self.on_frame(ev),
            "Page.frameNavigated" => self.on_frame_navigated(ev),
            "Page.navigatedWithinDocument" => self.on_navigated_within_document(ev),
            "Target.attachedToTarget" => self.on_attached_to_target(ev),
            "Target.targetDestroyed" => {
                if let Some(target) = ev.params.get("targetId").and_then(|v| v.as_str()) {
                    {
                        let mut st = self.state();
                        st.destroyed.insert(target.to_string());
                        st.gates.remove(target);
                        st.sessions.remove(target);
                        st.main_frames.remove(target);
                    }
                    self.end_controls(Some(target), InterventionEnd::TargetClosed);
                    let (stream, session) = {
                        let mut st = self.state();
                        st.fetch.remove(target);
                        st.attachments.remove(target);
                        match st.streams.remove(target) {
                            Some(s) => {
                                let sess = s.session.clone();
                                (Some(s), Some(sess))
                            }
                            None => (None, None),
                        }
                    };
                    if let (Some(stream), Some(session)) = (stream, session) {
                        stream.end(EndReason::TabDestroyed);
                        // The engine may have already torn the screencast down
                        // alongside the target, but the contract is OUR stop:
                        // send it. Target-aware cleanup avoids stopping a
                        // screencast that a fresh stream (same target, new
                        // session) has already taken over.
                        self.spawn_stop_if_unowned(target.to_string(), session);
                    }
                    // State and stream cleanup are done and no lock is held:
                    // the exact target's resources may now be closed.
                    self.notify_target_destroyed(target);
                }
            }
            "Fetch.requestPaused" => self.on_fetch(ev),
            _ => {}
        }
    }

    // ---- document boundaries ----

    /// Report this owner's document boundaries to `sink` (idempotent per sink).
    pub(crate) fn add_boundary_sink(&self, sink: Arc<dyn DocumentBoundarySink>) {
        let mut sinks = self
            .boundary_sinks
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if !sinks.iter().any(|known| Arc::ptr_eq(known, &sink)) {
            sinks.push(sink);
        }
    }

    /// Report exact-target destruction and owner end to `sink` (idempotent per
    /// sink). Registration races with owner end are closed by deciding under
    /// the `terminal_sinks` lock, the same lock `notify_owner_ended` flips
    /// `owner_ended_notified` and snapshots under: a sink is either in that
    /// snapshot or sees the flag here and gets `owner_ended` replayed. The
    /// replay runs after the lock is released. The sink is always retained
    /// (deduped under the lock), so re-registering a late sink replays once.
    pub(crate) fn add_terminal_sink(&self, sink: Arc<dyn OwnerTerminalSink>) {
        let owner_ended = {
            let mut sinks = self
                .terminal_sinks
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if sinks.iter().any(|known| Arc::ptr_eq(known, &sink)) {
                return;
            }
            sinks.push(sink.clone());
            self.owner_ended_notified.load(Ordering::SeqCst)
        };
        if !owner_ended {
            return;
        }
        // The owner already ended (and notified its then-current sinks): a late
        // sink would otherwise never learn it and could keep resources live.
        sink.owner_ended(
            &self.profile,
            &self.browser_session_id,
            ClosedReason::EngineExited,
        );
    }

    #[cfg(test)]
    pub(crate) fn terminal_sink_count(&self) -> usize {
        self.terminal_sinks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    fn terminal_sinks_snapshot(&self) -> Vec<Arc<dyn OwnerTerminalSink>> {
        self.terminal_sinks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Tell every terminal sink the exact `target_id` is gone. Sinks are
    /// copied out first: no owner lock is held while they run.
    fn notify_target_destroyed(&self, target_id: &str) {
        for sink in self.terminal_sinks_snapshot() {
            sink.target_destroyed(&self.profile, &self.browser_session_id, target_id);
        }
    }

    /// Tell every terminal sink the whole owner ended, at most once per owner.
    /// The flag flip and the snapshot happen under the `terminal_sinks` lock
    /// (see [`Self::add_terminal_sink`]); sinks run with that lock released.
    fn notify_owner_ended(&self, reason: ClosedReason) {
        let snapshot = {
            let sinks = self
                .terminal_sinks
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if self.owner_ended_notified.swap(true, Ordering::SeqCst) {
                return;
            }
            sinks.clone()
        };
        for sink in snapshot {
            sink.owner_ended(&self.profile, &self.browser_session_id, reason);
        }
    }

    #[cfg(test)]
    pub(crate) fn boundary_sink_count(&self) -> usize {
        self.boundary_sinks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    /// Declare a document boundary, synchronously and in this order:
    ///
    /// 1. every sink runs (the lifecycle demotes the matching approvals to
    ///    Pending at a new revision and revokes their grants) with NO owner
    ///    lock held;
    /// 2. under the state lock, the content gate of every affected target
    ///    advances and its hub slot is cleared.
    ///
    /// Sinks first, gate second: a viewer prepare reads "Safe at revision R",
    /// subscribes (capturing the generation), then re-checks R. If that
    /// re-check still passes, the sink has not demoted yet, so the capture
    /// happened before the advance and the new receiver is fenced by it. If the
    /// sink already ran, the re-check fails and no receiver is handed out.
    /// Either way no frame stamped with a later generation reaches an approval
    /// of an earlier one.
    ///
    /// Identity is the owner's own `(profile, browser_session_id)` plus the
    /// exact target id, never a URL, hash, current tab or loader id.
    pub(crate) fn declare_boundary(&self, scope: BoundaryScope, cause: BoundaryCause) {
        let boundary = DocumentBoundary {
            profile: self.profile.clone(),
            browser_session_id: self.browser_session_id.clone(),
            scope,
            cause,
        };
        let sinks: Vec<Arc<dyn DocumentBoundarySink>> = self
            .boundary_sinks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        tracing::debug!(
            profile = %boundary.profile,
            cause = ?boundary.cause,
            scope = ?boundary.scope,
            "browser document boundary"
        );
        for sink in &sinks {
            sink.document_boundary(&boundary);
        }
        let mut st = self.state();
        match &boundary.scope {
            BoundaryScope::Target(target_id) => st.advance_target(target_id),
            BoundaryScope::AllTargets => {
                let mut known: Vec<String> = st.gates.keys().cloned().collect();
                known.extend(st.streams.keys().cloned());
                known.sort();
                known.dedup();
                for target_id in known {
                    st.advance_target(&target_id);
                }
            }
        }
    }

    /// An agent verb is about to navigate / reload / traverse history on
    /// `target_id` (reached on `session`). Called BEFORE the engine is told.
    pub(crate) async fn agent_navigation(&self, target_id: &str, session: &SessionId) {
        self.state()
            .sessions
            .insert(target_id.to_string(), session.clone());
        self.declare_boundary_off_control_lock(
            BoundaryScope::Target(target_id.to_string()),
            BoundaryCause::AgentNavigation,
        )
        .await;
    }

    /// [`Self::declare_boundary`] for callers that run inside a control's
    /// dispatch / agent future. `TargetControl::dispatch` and `run_agent` poll
    /// such a future ONCE while holding the control's (non-reentrant) state
    /// lock, and the sink revokes grants, which locks the same control. One
    /// cooperative yield guarantees the boundary runs on a later poll, after
    /// that lock is released. It is still synchronous with respect to the
    /// engine: the caller has not sent the navigation yet.
    async fn declare_boundary_off_control_lock(&self, scope: BoundaryScope, cause: BoundaryCause) {
        tokio::task::yield_now().await;
        self.declare_boundary(scope, cause);
    }

    async fn resolve_session(&self, target_id: &str) -> Result<SessionId, BrowserError> {
        let session = (self.resolver)(target_id.to_string()).await?;
        self.state()
            .sessions
            .insert(target_id.to_string(), session.clone());
        Ok(session)
    }

    /// `Page.frameNavigated`. A main-frame commit is a boundary REGARDLESS of
    /// its loader id: obscura reports a click-driven navigation with the old
    /// loader id, and the loader-based reset in the agent pump cannot see it.
    /// Anything that cannot be attributed to one target fences all of them.
    fn on_frame_navigated(&self, ev: &CdpEvent) {
        let Some(frame) = ev.params.get("frame").and_then(|f| f.as_object()) else {
            self.declare_boundary(BoundaryScope::AllTargets, BoundaryCause::Unattributed);
            return;
        };
        if frame.get("parentId").is_some_and(|p| !p.is_null()) {
            return; // a sub-frame commit is not a document boundary of the page
        }
        let frame_id = frame.get("id").and_then(|v| v.as_str());
        let target = ev
            .session
            .as_ref()
            .and_then(|session| self.state().target_for_session(session));
        match (target, frame_id) {
            (Some(target), Some(frame_id)) => {
                self.state()
                    .main_frames
                    .insert(target.clone(), frame_id.to_string());
                self.declare_boundary(
                    BoundaryScope::Target(target),
                    BoundaryCause::TopLevelNavigation,
                );
            }
            _ => self.declare_boundary(BoundaryScope::AllTargets, BoundaryCause::Unattributed),
        }
    }

    /// `Page.navigatedWithinDocument` (hash, `pushState`). Ignored only when
    /// the owner KNOWS the frame is a different frame than the target's main
    /// one; unknown means boundary.
    fn on_navigated_within_document(&self, ev: &CdpEvent) {
        let frame_id = ev.params.get("frameId").and_then(|v| v.as_str());
        let target = ev
            .session
            .as_ref()
            .and_then(|session| self.state().target_for_session(session));
        match (target, frame_id) {
            (Some(target), Some(frame_id)) => {
                let other_frame = self
                    .state()
                    .main_frames
                    .get(&target)
                    .is_some_and(|main| main != frame_id);
                if !other_frame {
                    self.declare_boundary(
                        BoundaryScope::Target(target),
                        BoundaryCause::SameDocumentNavigation,
                    );
                }
            }
            _ => self.declare_boundary(BoundaryScope::AllTargets, BoundaryCause::Unattributed),
        }
    }

    /// Learn `target -> session` for page targets as the engine attaches them.
    fn on_attached_to_target(&self, ev: &CdpEvent) {
        let info = ev.params.get("targetInfo");
        let session = ev.params.get("sessionId").and_then(|v| v.as_str());
        let target = info
            .and_then(|i| i.get("targetId"))
            .and_then(|v| v.as_str());
        let is_page = info
            .and_then(|i| i.get("type"))
            .and_then(|v| v.as_str())
            .is_some_and(|t| t == "page");
        if let (true, Some(session), Some(target)) = (is_page, session, target) {
            self.state()
                .sessions
                .insert(target.to_string(), SessionId(session.to_string()));
        }
    }

    fn on_frame(self: &Arc<Self>, ev: &CdpEvent) {
        let Some(session) = ev.session.as_ref() else {
            return;
        };
        let owner_ack: Option<u64>;
        let mut stop_target: Option<String> = None;
        {
            let mut st = self.state();
            let Some((target, stream)) = st.streams.iter_mut().find(|(_, s)| s.session == *session)
            else {
                return;
            };
            let target = target.clone();
            let mut forget = false;
            match screencast::screencast_frame(&ev.params) {
                // Undecodable: nobody can use it, but the engine still wants its ack.
                Err(_) => owner_ack = ev.params.get("sessionId").and_then(|v| v.as_u64()),
                Ok(frame) => {
                    let ack_id = frame.session_id;
                    let live =
                        stream
                            .hub
                            .publish(stream.epoch, frame.data, frame.timestamp, ack_id);
                    let mut claimed = false;
                    let mut tap_gone = false;
                    if let Some(tap) = &stream.tap {
                        tap.captured.fetch_add(1, Ordering::Relaxed);
                        // The id is recorded BEFORE the send: whatever happens
                        // to the tap afterwards, its guard knows the frame is
                        // owed an ack.
                        let mut pending = lock_pending(&tap.pending);
                        if pending.closed {
                            tap_gone = true;
                        } else {
                            pending.ids.insert(ack_id);
                            match tap.tx.try_send(live) {
                                Ok(()) => claimed = true,
                                Err(mpsc::error::TrySendError::Full(_)) => {
                                    pending.ids.remove(&ack_id);
                                    tap.dropped.fetch_add(1, Ordering::Relaxed);
                                }
                                Err(mpsc::error::TrySendError::Closed(_)) => {
                                    pending.ids.remove(&ack_id);
                                    tap_gone = true;
                                }
                            }
                        }
                    }
                    if tap_gone {
                        stream.tap = None;
                        // Nobody left holding the stream open: the screencast
                        // would otherwise run for no one.
                        forget = !stream.attached;
                    }
                    owner_ack = (!claimed).then_some(ack_id);
                }
            }
            if forget {
                st.streams.remove(&target);
                stop_target = Some(target);
            }
        }
        if let Some(target) = stop_target {
            self.spawn_stop_if_unowned(target, session.clone());
        }
        if let Some(ack_id) = owner_ack {
            let acker = FrameAcker::new(self.conn.clone(), session.clone());
            tokio::spawn(async move {
                let _ = acker.ack(ack_id).await;
            });
        }
    }

    fn on_fetch(self: &Arc<Self>, ev: &Arc<CdpEvent>) {
        let Some(session) = ev.session.as_ref() else {
            return;
        };
        let mut st = self.state();
        let Some((target, slot)) = st.fetch.iter().find(|(_, s)| s.session == *session) else {
            return;
        };
        let target = target.clone();
        match slot.tx.try_send(ev.clone()) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                // A dropped paused request would hang forever, and continuing
                // it unseen would bypass policy: drop the feed (receiver sees
                // the end) and close the target so the engine cancels them.
                slot.dropped.fetch_add(1, Ordering::Relaxed);
                st.fetch.remove(&target);
                drop(st);
                self.spawn_close_target(target);
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                st.fetch.remove(&target);
            }
        }
    }
}

/// Stop a stream's engine screencast after its entry was already removed. The
/// entry is gone, so nothing else will ever stop this screencast: the send is
/// detached onto the runtime so cancelling the caller (e.g. a dropped
/// `detach` / `release_recording` future) cannot lose it. The task owns the
/// lifecycle lock, so even a cancelled caller cannot let a replacement start
/// overtake this stop. The caller still awaits cleanup when not cancelled.
async fn stop_screencast_detached(
    conn: &CdpConnection,
    session: SessionId,
    life: OwnedMutexGuard<()>,
) {
    let conn = conn.clone();
    let send = tokio::spawn(async move {
        let _life = life;
        // Best effort: the engine may already be gone.
        let _ = screencast::stop_screencast(&conn, Some(&session)).await;
    });
    let _ = send.await;
}

/// Armed while a screencast start is in flight. If the owning future is
/// dropped before the start completes, the stream it inserted is removed and
/// the engine screencast (which may or may not have started) is stopped.
struct StartRollback {
    owner: Arc<LiveProfileOwner>,
    target_id: String,
    session: SessionId,
    epoch: u64,
    life: Option<OwnedMutexGuard<()>>,
    armed: bool,
}

impl Drop for StartRollback {
    fn drop(&mut self) {
        if self.armed
            && self
                .owner
                .remove_stream_if_epoch(&self.target_id, self.epoch)
        {
            let Some(life) = self.life.take() else {
                return;
            };
            let Ok(rt) = tokio::runtime::Handle::try_current() else {
                return;
            };
            let conn = self.owner.conn.clone();
            let session = self.session.clone();
            rt.spawn(async move {
                let _life = life;
                let _ = screencast::stop_screencast(&conn, Some(&session)).await;
            });
        }
    }
}

#[cfg(test)]
mod attachment_tests;
#[cfg(test)]
mod navigation_gate_tests;
#[cfg(test)]
pub(crate) mod tests;
