//! FIFO responses with a fixed backlog. A slow peer is disconnected on overflow.

use tokio::sync::{mpsc, oneshot};

pub(super) struct Writes<T>(mpsc::Sender<Write<T>>);

pub(super) struct Write<T> {
    pub(super) frame: T,
    pub(super) sent: oneshot::Sender<()>,
}

impl<T> Writes<T> {
    pub(super) fn queue(capacity: usize) -> (Self, mpsc::Receiver<Write<T>>) {
        let (tx, rx) = mpsc::channel(capacity);
        (Self(tx), rx)
    }

    pub(super) fn enqueue(&self, frame: T) -> Option<oneshot::Receiver<()>> {
        let (sent, ack) = oneshot::channel();
        self.0.try_send(Write { frame, sent }).ok()?;
        Some(ack)
    }

    pub(super) async fn flush(&self, frame: T) -> Option<()> {
        self.enqueue(frame)?.await.ok()
    }
}
