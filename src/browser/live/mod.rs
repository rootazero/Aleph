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

pub(crate) mod session;
pub mod stream_hub;

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use aleph_cdp::methods::screencast::{self, ScreencastFormat};
use aleph_cdp::methods::target;
use aleph_cdp::{CdpConnection, CdpError, CdpEvent, SessionId, TargetId};
use aleph_protocol::browser_live::BrowserTargetBinding;
use tokio::sync::mpsc;

use crate::browser::cdp_backend::map_cdp_err;
use crate::browser::engine::Engine;
use crate::browser::error::BrowserError;
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

struct FetchSlot {
    session: SessionId,
    tx: mpsc::Sender<Arc<CdpEvent>>,
    dropped: Arc<AtomicU64>,
}

#[derive(Default)]
struct State {
    next_epoch: u64,
    streams: HashMap<String, TargetStream>,
    fetch: HashMap<String, FetchSlot>,
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
    lifecycle: tokio::sync::Mutex<()>,
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
            lifecycle: tokio::sync::Mutex::new(()),
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
        let (streams, fetch) = {
            let mut st = self.state();
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
    }

    fn spawn_close_target(self: &Arc<Self>, target: String) {
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let owner = self.clone();
        rt.spawn(async move {
            // Serialise with `register_fetch`: a close issued by a stale
            // fail-closed path must not race a re-arm that arrived a moment
            // later. Holding the lifecycle lock here lines us up with the
            // same lock `register_fetch` takes to install the new slot, so
            // the check below sees a consistent state and we cannot close a
            // target whose feed is fresh.
            let _life = owner.lifecycle.lock().await;
            if owner.state().fetch.contains_key(&target) {
                tracing::debug!(
                    target = %target,
                    "close_target skipped: a fetch slot is registered for this target"
                );
                return;
            }
            let _ = target::close_target(&owner.conn, &TargetId(target)).await;
        });
    }

    /// Test-only entry point that exposes `spawn_close_target`'s effect
    /// synchronously: waits for the lifecycle lock to be released, then
    /// runs the close path on the current task so a test can hold a gate.
    /// Mirrors the production slot check: a close that runs after a fresh
    /// `register_fetch` for the same target must see the new slot and skip.
    #[cfg(test)]
    pub(crate) async fn spawn_close_target_for_test(self: &Arc<Self>, target: String) {
        let _life = self.lifecycle.lock().await;
        if self.state().fetch.contains_key(&target) {
            return;
        }
        let _ = target::close_target(&self.conn, &TargetId(target)).await;
    }

    #[cfg(test)]
    pub(crate) fn lagged_events(&self) -> u64 {
        self.lagged.load(Ordering::Relaxed)
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    // ---- viewer API ----

    /// Bind a viewer to a target: ensures the target's one engine screencast.
    /// `browser_session_id` is carried by the binding but not validated here —
    /// that is the gateway's job (Task 3).
    #[allow(dead_code)] // consumed by the Task 3 gateway
    pub(crate) async fn attach_target(
        self: &Arc<Self>,
        binding: BrowserTargetBinding,
    ) -> Result<(), BrowserError> {
        if binding.profile != self.profile {
            return Err(BrowserError::ActionFailed(format!(
                "binding is for profile '{}', this owner serves '{}'",
                binding.profile, self.profile
            )));
        }
        let session = (self.resolver)(binding.target_id.clone()).await?;
        let _life = self.lifecycle.lock().await;
        self.ensure_stream(&binding.target_id, &session, VIEWER_FORMAT)
            .await
            .map_err(|e| map_cdp_err(self.engine, "Page.startScreencast", e))?;
        // The target may have been destroyed while the start was in flight
        // (Target.targetDestroyed removes the stream): that is not an attach.
        match self.state().streams.get_mut(&binding.target_id) {
            Some(stream) if stream.session == session => {
                stream.attached = true;
                Ok(())
            }
            _ => Err(BrowserError::TabNotFound(binding.target_id)),
        }
    }

    /// Drop the viewer binding. Idempotent. A live recording keeps the engine
    /// screencast running; without one the screencast stops.
    #[allow(dead_code)] // consumed by the Task 3 gateway
    pub(crate) async fn close_target(&self, target_id: &str) -> Result<(), BrowserError> {
        let _life = self.lifecycle.lock().await;
        let removed = {
            let mut st = self.state();
            let epoch = st.next_epoch;
            let Some(stream) = st.streams.get_mut(target_id) else {
                return Ok(());
            };
            stream.attached = false;
            if stream.tap.is_some() {
                // Viewers end (old hub dropped); the recording's stream goes on
                // under a fresh epoch and hub.
                stream.hub = LatestFrameHub::new();
                stream.epoch = epoch;
                st.next_epoch += 1;
                None
            } else {
                st.streams.remove(target_id)
            }
        };
        if let Some(stream) = removed {
            // Best effort: the engine may already be gone.
            let _ = screencast::stop_screencast(&self.conn, Some(&stream.session)).await;
        }
        Ok(())
    }

    /// Newest-frame receiver for an attached target. Dropping it detaches only
    /// this viewer.
    #[allow(dead_code)] // consumed by the Task 3 gateway
    pub(crate) fn subscribe_latest(
        &self,
        target_id: &str,
    ) -> Result<LatestFrameReceiver, BrowserError> {
        match self.state().streams.get(target_id) {
            Some(stream) if stream.attached => Ok(stream.hub.subscribe()),
            _ => Err(BrowserError::TabNotFound(target_id.to_string())),
        }
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
        let _life = self.lifecycle.lock().await;
        self.ensure_stream(target_id, session, format).await?;
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
        let _life = self.lifecycle.lock().await;
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
            let _ = screencast::stop_screencast(&self.conn, Some(&stream.session)).await;
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
    ) -> Result<(), CdpError> {
        let epoch = {
            let mut st = self.state();
            match st.streams.get(target_id) {
                Some(stream) if stream.session == *session => return Ok(()),
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
            // Inserted BEFORE the start so the first frame finds its stream.
            st.streams.insert(
                target_id.to_string(),
                TargetStream::new(epoch, session.clone()),
            );
            epoch
        };
        let mut rollback = StartRollback {
            owner: self.clone(),
            target_id: target_id.to_string(),
            session: session.clone(),
            epoch,
            armed: true,
        };
        let started =
            screencast::start_screencast(&self.conn, Some(session), format, None, None, None).await;
        rollback.armed = false;
        if let Err(e) = started {
            // The engine refused: nothing to stop, only the stream to forget.
            self.remove_stream_if_epoch(target_id, epoch);
            return Err(e);
        }
        Ok(())
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

    fn end_all(&self, reason: EndReason) {
        let streams: Vec<TargetStream> = {
            let mut st = self.state();
            st.fetch.clear();
            st.streams.drain().map(|(_, s)| s).collect()
        };
        for stream in streams {
            stream.end(reason);
        }
    }

    fn dispatch(self: &Arc<Self>, ev: &Arc<CdpEvent>) {
        match ev.method.as_str() {
            "Page.screencastFrame" => self.on_frame(ev),
            "Target.targetDestroyed" => {
                if let Some(target) = ev.params.get("targetId").and_then(|v| v.as_str()) {
                    let (stream, session) = {
                        let mut st = self.state();
                        st.fetch.remove(target);
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
                }
            }
            "Fetch.requestPaused" => self.on_fetch(ev),
            _ => {}
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

/// Armed while a screencast start is in flight. If the owning future is
/// dropped before the start completes, the stream it inserted is removed and
/// the engine screencast (which may or may not have started) is stopped.
struct StartRollback {
    owner: Arc<LiveProfileOwner>,
    target_id: String,
    session: SessionId,
    epoch: u64,
    armed: bool,
}

impl Drop for StartRollback {
    fn drop(&mut self) {
        if self.armed
            && self
                .owner
                .remove_stream_if_epoch(&self.target_id, self.epoch)
        {
            self.owner
                .spawn_stop_if_unowned(self.target_id.clone(), self.session.clone());
        }
    }
}

#[cfg(test)]
mod tests;
