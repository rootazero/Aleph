//! Per-target stream state held by the profile owner.

use std::collections::BTreeSet;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use aleph_cdp::methods::screencast;
use aleph_cdp::{CdpConnection, CdpError, SessionId};
use tokio::sync::{mpsc, watch};

use super::stream_hub::{LatestFrameHub, LiveFrame};

/// Capacity of the recording observer's frame queue. Frames beyond it are
/// dropped for the recording (and acked by the owner at once).
pub(crate) const TAP_CAPACITY: usize = 32;

/// Why a stream ended without anyone asking.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EndReason {
    TabDestroyed,
    EngineGone,
}

/// The only way a frame is acknowledged to the engine. Handed to the recording
/// observer so it can ack *after write* (the pacing rule), while the owner
/// acks every frame nobody else claimed.
#[derive(Clone)]
pub(crate) struct FrameAcker {
    conn: CdpConnection,
    session: SessionId,
    /// Set on a recording's acker: the frames the owner handed to the tap and
    /// nobody has acked yet.
    pending: Option<SharedPending>,
}

impl FrameAcker {
    pub(crate) fn new(conn: CdpConnection, session: SessionId) -> Self {
        Self {
            conn,
            session,
            pending: None,
        }
    }

    fn for_tap(conn: CdpConnection, session: SessionId, pending: SharedPending) -> Self {
        Self {
            conn,
            session,
            pending: Some(pending),
        }
    }

    pub(crate) async fn ack(&self, engine_ack_id: u64) -> Result<(), CdpError> {
        if let Some(pending) = &self.pending {
            lock_pending(pending).ids.remove(&engine_ack_id);
        }
        screencast::screencast_frame_ack(&self.conn, Some(&self.session), engine_ack_id).await
    }
}

/// Frames claimed by a recording tap and not yet acked. `closed` is set when
/// the tap goes away: from then on the owner acks everything itself.
#[derive(Default)]
pub(crate) struct PendingAcks {
    pub(crate) ids: BTreeSet<u64>,
    pub(crate) closed: bool,
}

pub(crate) type SharedPending = Arc<Mutex<PendingAcks>>;

pub(crate) fn lock_pending(pending: &SharedPending) -> MutexGuard<'_, PendingAcks> {
    pending.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Dropped with the [`RecordingTap`]: every frame the tap claimed but never
/// acked (queued, or taken and abandoned) is acked here, so a recorder that
/// dies mid-flight cannot leave the engine waiting forever on its ack.
pub(crate) struct TapGuard {
    acker: FrameAcker,
    pending: SharedPending,
}

impl Drop for TapGuard {
    fn drop(&mut self) {
        let ids = {
            let mut pending = lock_pending(&self.pending);
            pending.closed = true;
            std::mem::take(&mut pending.ids)
        };
        if ids.is_empty() {
            return;
        }
        // No runtime means the process is going down; the engine goes with it.
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            let acker = self.acker.clone();
            rt.spawn(async move {
                for id in ids {
                    let _ = acker.ack(id).await;
                }
            });
        }
    }
}

/// Owner-side half of a recording observer.
pub(crate) struct TapSlot {
    pub(crate) tx: mpsc::Sender<Arc<LiveFrame>>,
    pub(crate) captured: Arc<AtomicU64>,
    pub(crate) dropped: Arc<AtomicU64>,
    pub(crate) end: watch::Sender<Option<EndReason>>,
    pub(crate) pending: SharedPending,
}

/// Recorder-side half: what a recording consumes instead of subscribing to
/// the raw event stream itself.
pub(crate) struct RecordingTap {
    pub(crate) frames: mpsc::Receiver<Arc<LiveFrame>>,
    pub(crate) end: watch::Receiver<Option<EndReason>>,
    pub(crate) acker: FrameAcker,
    pub(crate) captured: Arc<AtomicU64>,
    pub(crate) dropped: Arc<AtomicU64>,
    /// Acks whatever the recorder leaves unacked when this tap is dropped.
    _guard: TapGuard,
}

impl RecordingTap {
    /// Both halves of one tap.
    pub(crate) fn pair(conn: &CdpConnection, session: &SessionId) -> (TapSlot, RecordingTap) {
        let (tx, frames) = mpsc::channel(TAP_CAPACITY);
        let (end_tx, end) = watch::channel(None);
        let captured = Arc::new(AtomicU64::new(0));
        let dropped = Arc::new(AtomicU64::new(0));
        let pending = SharedPending::default();
        let slot = TapSlot {
            tx,
            captured: captured.clone(),
            dropped: dropped.clone(),
            end: end_tx,
            pending: pending.clone(),
        };
        let tap = RecordingTap {
            frames,
            end,
            acker: FrameAcker::for_tap(conn.clone(), session.clone(), pending.clone()),
            captured,
            dropped,
            _guard: TapGuard {
                acker: FrameAcker::new(conn.clone(), session.clone()),
                pending,
            },
        };
        (slot, tap)
    }
}

/// One target's live stream: one engine screencast, one epoch, one hub, and
/// at most one recording observer.
pub(crate) struct TargetStream {
    pub(crate) epoch: u64,
    pub(crate) session: SessionId,
    pub(crate) hub: LatestFrameHub,
    pub(crate) tap: Option<TapSlot>,
    /// A viewer binding holds the stream open.
    pub(crate) attached: bool,
}

impl TargetStream {
    pub(crate) fn new(epoch: u64, session: SessionId) -> Self {
        Self {
            epoch,
            session,
            hub: LatestFrameHub::new(),
            tap: None,
            attached: false,
        }
    }

    /// Signal the recording observer (if any) that the stream is over. The
    /// reason is set BEFORE the sender is dropped, so a recorder that sees its
    /// frame channel close can always read why.
    pub(crate) fn end(self, reason: EndReason) {
        if let Some(tap) = self.tap {
            tap.end.send_replace(Some(reason));
        }
    }
}
