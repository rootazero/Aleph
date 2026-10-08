//! Per-target stream state held by the profile owner.

use std::sync::atomic::AtomicU64;
use std::sync::Arc;

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
}

impl FrameAcker {
    pub(crate) fn new(conn: CdpConnection, session: SessionId) -> Self {
        Self { conn, session }
    }

    pub(crate) async fn ack(&self, engine_ack_id: u64) -> Result<(), CdpError> {
        screencast::screencast_frame_ack(&self.conn, Some(&self.session), engine_ack_id).await
    }
}

/// Owner-side half of a recording observer.
pub(crate) struct TapSlot {
    pub(crate) tx: mpsc::Sender<Arc<LiveFrame>>,
    pub(crate) captured: Arc<AtomicU64>,
    pub(crate) dropped: Arc<AtomicU64>,
    pub(crate) end: watch::Sender<Option<EndReason>>,
}

/// Recorder-side half: what a recording consumes instead of subscribing to
/// the raw event stream itself.
pub(crate) struct RecordingTap {
    pub(crate) frames: mpsc::Receiver<Arc<LiveFrame>>,
    pub(crate) end: watch::Receiver<Option<EndReason>>,
    pub(crate) acker: FrameAcker,
    pub(crate) captured: Arc<AtomicU64>,
    pub(crate) dropped: Arc<AtomicU64>,
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
