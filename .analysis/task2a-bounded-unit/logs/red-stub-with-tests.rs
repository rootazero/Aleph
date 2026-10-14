//! The bounded latest-frame hub (task 2a stub: RED phase).

use std::sync::Arc;

#[derive(Debug)]
pub struct LiveFrame {
    pub epoch: u64,
    pub seq: u64,
    pub data: Arc<[u8]>,
    pub timestamp: Option<f64>,
    pub(crate) engine_ack_id: u64,
}

pub(crate) struct LatestFrameHub;

impl LatestFrameHub {
    pub(crate) fn new() -> Self {
        unimplemented!()
    }
    pub(crate) fn publish(
        &mut self,
        _epoch: u64,
        _data: Vec<u8>,
        _timestamp: Option<f64>,
        _engine_ack_id: u64,
    ) -> Arc<LiveFrame> {
        unimplemented!()
    }
    pub(crate) fn subscribe(&self) -> LatestFrameReceiver {
        unimplemented!()
    }
}

pub struct LatestFrameReceiver;

impl LatestFrameReceiver {
    pub async fn next(&mut self) -> Option<Arc<LiveFrame>> {
        unimplemented!()
    }
    #[must_use]
    pub fn skipped(&self) -> u64 {
        unimplemented!()
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
        let f = timeout(T, rx.next()).await.expect("no hang").expect("frame");
        assert_eq!((f.epoch, f.seq, &*f.data, f.engine_ack_id), (1, 2, &[2u8][..], 11));
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
