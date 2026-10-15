//! The bounded latest-frame hub: one per live target stream.
//!
//! A viewer never owns the engine's frame pacing. Frames are published into a
//! capacity-1 `watch` slot, so a slow or hidden viewer can only ever lag
//! behind by *skipping* old sequence numbers — it cannot back-pressure the
//! owner's dispatcher, delay an ACK, or starve the recording observer.

use std::sync::Arc;

use tokio::sync::watch;

use super::navigation_gate::ContentGate;

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
    /// The target's content generation when this frame was published. Owner
    /// internal: the r4 wire frame never carries it.
    pub(crate) content_gen: u64,
}

/// Publisher half; lives inside the owner's per-target stream.
pub(crate) struct LatestFrameHub {
    tx: watch::Sender<Option<Arc<LiveFrame>>>,
    next_seq: u64,
    gate: ContentGate,
}

impl LatestFrameHub {
    pub(crate) fn new() -> Self {
        Self::with_gate(ContentGate::default())
    }

    /// A hub whose frames are stamped from `gate` (the owner's per-target
    /// generation, shared across stream re-creation).
    pub(crate) fn with_gate(gate: ContentGate) -> Self {
        let (tx, _rx) = watch::channel(None);
        Self {
            tx,
            next_seq: 1,
            gate,
        }
    }

    /// Drop the stored frame. Called at a document boundary, under the same
    /// owner lock that advances the gate, so a late subscriber can never be
    /// handed the previous document's last frame.
    pub(crate) fn clear(&self) {
        self.tx.send_replace(None);
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
            content_gen: self.gate.current(),
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
            approved: self.gate.current(),
            gate: self.gate.clone(),
            fenced: false,
        }
    }
}

/// Subscriber half. Dropping it detaches only this viewer.
pub struct LatestFrameReceiver {
    rx: watch::Receiver<Option<Arc<LiveFrame>>>,
    last_seq: u64,
    skipped: u64,
    /// The content generation this receiver was approved under. Captured while
    /// the owner holds its state lock, so it is exact.
    approved: u64,
    gate: ContentGate,
    /// Sticky: once the document changed under this receiver it never yields
    /// another frame.
    fenced: bool,
}

impl LatestFrameReceiver {
    /// The newest frame not yet seen, skipping anything older. `None` once the
    /// stream has ended (target closed or destroyed, engine gone).
    pub async fn next(&mut self) -> Option<Arc<LiveFrame>> {
        loop {
            if self.fence_if_stale() {
                return None;
            }
            self.rx.changed().await.ok()?;
            if self.fence_if_stale() {
                return None;
            }
            let latest = self.rx.borrow_and_update().clone();
            if let Some(frame) = latest {
                if frame.content_gen < self.approved {
                    // A frame of an older document than this approval.
                    continue;
                }
                if frame.content_gen != self.approved {
                    self.fenced = true;
                    return None;
                }
                if self.last_seq != 0 {
                    self.skipped += frame.seq.saturating_sub(self.last_seq + 1);
                }
                self.last_seq = frame.seq;
                return Some(frame);
            }
        }
    }

    fn fence_if_stale(&mut self) -> bool {
        if self.gate.current() != self.approved {
            self.fenced = true;
        }
        self.fenced
    }

    /// Whether a document boundary ended this receiver (as opposed to the
    /// stream or the target ending). A fenced receiver yields no more frames.
    #[must_use]
    pub fn is_fenced(&mut self) -> bool {
        self.fence_if_stale()
    }

    /// Last-moment check for the transport: `frame` may only be sent if it is
    /// of the approved document and no boundary has passed since it was taken.
    #[must_use]
    pub fn admits(&mut self, frame: &LiveFrame) -> bool {
        !self.fence_if_stale() && frame.content_gen == self.approved
    }

    /// How many frames this subscriber skipped because it read too slowly.
    #[cfg(test)] // only tests read this; production has no caller
    #[must_use]
    pub fn skipped(&self) -> u64 {
        self.skipped
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::time::timeout;

    const T: Duration = Duration::from_secs(5);

    #[tokio::test]
    async fn late_subscriber_gets_current_frame_immediately() {
        let mut hub = LatestFrameHub::new();
        hub.publish(1, vec![1], None, 10);
        hub.publish(1, vec![2], Some(1.5), 11);
        let mut rx = hub.subscribe();
        let f = timeout(T, rx.next())
            .await
            .expect("no hang")
            .expect("frame");
        assert_eq!(
            (f.epoch, f.seq, &*f.data, f.engine_ack_id),
            (1, 2, &[2u8][..], 11)
        );
        assert_eq!(rx.skipped(), 0);
    }

    #[tokio::test]
    async fn slow_subscriber_sees_only_latest_and_counts_skipped() {
        let mut hub = LatestFrameHub::new();
        let mut rx = hub.subscribe();
        hub.publish(1, vec![1], None, 1);
        let f = timeout(T, rx.next()).await.unwrap().unwrap();
        assert_eq!(f.seq, 1);
        for i in 2..=6u8 {
            hub.publish(1, vec![i], None, i as u64);
        }
        let f = timeout(T, rx.next()).await.unwrap().unwrap();
        assert_eq!(f.seq, 6);
        assert_eq!(rx.skipped(), 4); // seq 2..=5 skipped
    }

    #[tokio::test]
    async fn publish_without_subscribers_never_blocks_and_resubscribe_gets_latest() {
        let mut hub = LatestFrameHub::new();
        let rx = hub.subscribe();
        drop(rx); // hidden viewer
        for i in 0..1000u32 {
            hub.publish(1, i.to_le_bytes().to_vec(), None, i as u64);
        }
        let mut rx = hub.subscribe();
        let f = timeout(T, rx.next()).await.unwrap().unwrap();
        assert_eq!(f.seq, 1000);
        assert_eq!(&*f.data, &999u32.to_le_bytes()[..]);
    }

    #[tokio::test]
    async fn dropping_hub_terminates_receiver() {
        let mut hub = LatestFrameHub::new();
        let mut rx = hub.subscribe();
        hub.publish(1, vec![1], None, 1);
        assert_eq!(timeout(T, rx.next()).await.unwrap().unwrap().seq, 1);
        drop(hub);
        assert!(timeout(T, rx.next()).await.expect("no hang").is_none());
    }
}
