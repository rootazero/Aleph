//! The bounded latest-frame hub: one per live target stream.
//!
//! A viewer never owns the engine's frame pacing. Frames are published into a
//! capacity-1 `watch` slot, so a slow or hidden viewer can only ever lag
//! behind by *skipping* old sequence numbers — it cannot back-pressure the
//! owner's dispatcher, delay an ACK, or starve the recording observer.

use std::sync::Arc;

use tokio::sync::watch;

/// One decoded screencast frame as the owner publishes it.
#[derive(Debug)]
pub struct LiveFrame {
    /// Stream epoch of the target this frame belongs to. A re-attach of the
    /// same target gets a fresh epoch, so a consumer can tell "same stream,
    /// next frame" from "new stream, sequence restarted".
    pub epoch: u64,
    /// Per-epoch sequence, starting at 1.
    pub seq: u64,
    pub data: Arc<[u8]>,
    pub timestamp: Option<f64>,
    /// The engine's own frame counter, which the one ACK must quote back.
    /// Viewers must never use it.
    pub(crate) engine_ack_id: u64,
}

/// Publisher half; lives inside the owner's per-target stream.
pub(crate) struct LatestFrameHub {
    tx: watch::Sender<Option<Arc<LiveFrame>>>,
    next_seq: u64,
}

impl LatestFrameHub {
    pub(crate) fn new() -> Self {
        let (tx, _rx) = watch::channel(None);
        Self { tx, next_seq: 1 }
    }

    /// Store `data` as the latest frame. `send_replace` (not `send`) so the
    /// slot is written even while nobody is subscribed.
    pub(crate) fn publish(
        &mut self,
        epoch: u64,
        data: Vec<u8>,
        timestamp: Option<f64>,
        engine_ack_id: u64,
    ) -> Arc<LiveFrame> {
        let frame = Arc::new(LiveFrame {
            epoch,
            seq: self.next_seq,
            data: Arc::from(data),
            timestamp,
            engine_ack_id,
        });
        self.next_seq += 1;
        self.tx.send_replace(Some(frame.clone()));
        frame
    }

    pub(crate) fn subscribe(&self) -> LatestFrameReceiver {
        let mut rx = self.tx.subscribe();
        // A late subscriber should see the current frame immediately.
        if rx.borrow().is_some() {
            rx.mark_changed();
        }
        LatestFrameReceiver {
            rx,
            last_seq: 0,
            skipped: 0,
        }
    }
}

/// Subscriber half. Dropping it detaches only this viewer.
pub struct LatestFrameReceiver {
    rx: watch::Receiver<Option<Arc<LiveFrame>>>,
    last_seq: u64,
    skipped: u64,
}

impl LatestFrameReceiver {
    /// The newest frame not yet seen, skipping anything older. `None` once the
    /// stream has ended (target closed or destroyed, engine gone).
    pub async fn next(&mut self) -> Option<Arc<LiveFrame>> {
        loop {
            self.rx.changed().await.ok()?;
            let latest = self.rx.borrow_and_update().clone();
            if let Some(frame) = latest {
                if self.last_seq != 0 {
                    self.skipped += frame.seq.saturating_sub(self.last_seq + 1);
                }
                self.last_seq = frame.seq;
                return Some(frame);
            }
        }
    }

    /// How many frames this subscriber skipped because it read too slowly.
    #[must_use]
    pub fn skipped(&self) -> u64 {
        self.skipped
    }
}
