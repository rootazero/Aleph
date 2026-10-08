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
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use aleph_cdp::methods::screencast::{self, ScreencastFormat};
use aleph_cdp::{CdpConnection, CdpError, CdpEvent, SessionId};
use aleph_protocol::browser_live::BrowserTargetBinding;
use tokio::sync::{mpsc, watch};

use crate::browser::cdp_backend::map_cdp_err;
use crate::browser::engine::Engine;
use crate::browser::error::BrowserError;
use session::{EndReason, FrameAcker, RecordingTap, TapSlot, TargetStream, TAP_CAPACITY};
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
            lifecycle: tokio::sync::Mutex::new(()),
        });
        let weak = Arc::downgrade(&owner);
        tokio::spawn(async move {
            while closed.borrow().is_none() {
                tokio::select! {
                    ev = events.next() => match ev {
                        Some(ev) => match weak.upgrade() {
                            Some(owner) => owner.dispatch(&ev),
                            None => return,
                        },
                        None => break,
                    },
                    _ = closed.changed() => break,
                }
            }
            if let Some(owner) = weak.upgrade() {
                owner.end_all(EndReason::EngineGone);
            }
        });
        owner
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
        &self,
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
        if let Some(stream) = self.state().streams.get_mut(&binding.target_id) {
            stream.attached = true;
        }
        Ok(())
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
        &self,
        target_id: &str,
        session: &SessionId,
        format: ScreencastFormat,
    ) -> Result<RecordingTap, CdpError> {
        let _life = self.lifecycle.lock().await;
        self.ensure_stream(target_id, session, format).await?;
        let (tx, frames) = mpsc::channel(TAP_CAPACITY);
        let (end_tx, end) = watch::channel(None);
        let captured = Arc::new(AtomicU64::new(0));
        let dropped = Arc::new(AtomicU64::new(0));
        if let Some(stream) = self.state().streams.get_mut(target_id) {
            stream.tap = Some(TapSlot {
                tx,
                captured: captured.clone(),
                dropped: dropped.clone(),
                end: end_tx,
            });
        }
        Ok(RecordingTap {
            frames,
            end,
            acker: FrameAcker::new(self.conn.clone(), session.clone()),
            captured,
            dropped,
        })
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
    pub(crate) fn register_fetch(&self, target_id: &str, session: &SessionId) -> FetchFeed {
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
        FetchFeed { rx, dropped }
    }

    pub(crate) fn unregister_fetch(&self, target_id: &str) {
        self.state().fetch.remove(target_id);
    }

    // ---- internals ----

    /// Start the target's engine screencast unless a stream for this exact
    /// session already exists. Caller holds `lifecycle`.
    async fn ensure_stream(
        &self,
        target_id: &str,
        session: &SessionId,
        format: ScreencastFormat,
    ) -> Result<(), CdpError> {
        {
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
        }
        if let Err(e) =
            screencast::start_screencast(&self.conn, Some(session), format, None, None, None).await
        {
            self.state().streams.remove(target_id);
            return Err(e);
        }
        Ok(())
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

    fn dispatch(&self, ev: &Arc<CdpEvent>) {
        match ev.method.as_str() {
            "Page.screencastFrame" => self.on_frame(ev),
            "Target.targetDestroyed" => {
                if let Some(target) = ev.params.get("targetId").and_then(|v| v.as_str()) {
                    let stream = {
                        let mut st = self.state();
                        st.fetch.remove(target);
                        st.streams.remove(target)
                    };
                    if let Some(stream) = stream {
                        stream.end(EndReason::TabDestroyed);
                    }
                }
            }
            "Fetch.requestPaused" => self.on_fetch(ev),
            _ => {}
        }
    }

    fn on_frame(&self, ev: &CdpEvent) {
        let Some(session) = ev.session.as_ref() else {
            return;
        };
        let owner_ack: Option<u64>;
        {
            let mut st = self.state();
            let Some(stream) = st.streams.values_mut().find(|s| s.session == *session) else {
                return;
            };
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
                        match tap.tx.try_send(live) {
                            Ok(()) => claimed = true,
                            Err(mpsc::error::TrySendError::Full(_)) => {
                                tap.dropped.fetch_add(1, Ordering::Relaxed);
                            }
                            Err(mpsc::error::TrySendError::Closed(_)) => tap_gone = true,
                        }
                    }
                    if tap_gone {
                        stream.tap = None;
                    }
                    owner_ack = (!claimed).then_some(ack_id);
                }
            }
        }
        if let Some(ack_id) = owner_ack {
            let acker = FrameAcker::new(self.conn.clone(), session.clone());
            tokio::spawn(async move {
                let _ = acker.ack(ack_id).await;
            });
        }
    }

    fn on_fetch(&self, ev: &Arc<CdpEvent>) {
        let Some(session) = ev.session.as_ref() else {
            return;
        };
        let mut st = self.state();
        let Some((target, slot)) = st.fetch.iter().find(|(_, s)| s.session == *session) else {
            return;
        };
        match slot.tx.try_send(ev.clone()) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                slot.dropped.fetch_add(1, Ordering::Relaxed);
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                let target = target.clone();
                st.fetch.remove(&target);
            }
        }
    }
}

#[cfg(test)]
mod tests;
