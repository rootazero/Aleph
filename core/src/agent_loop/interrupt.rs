//! Interrupt channel for agent loop steering.
//!
//! Provides a signaling mechanism for soft interrupts — when a user sends
//! a new message while the agent is executing, this channel carries the
//! interrupt signal so the loop can redirect rather than hard-abort.

use tokio::sync::mpsc;

/// Signal sent through the interrupt channel.
#[derive(Debug, Clone)]
pub enum InterruptSignal {
    /// A new user message arrived while the agent is busy.
    NewMessage { content: String },
}

/// Sending half of the interrupt channel.
pub struct InterruptSender {
    tx: mpsc::Sender<InterruptSignal>,
}

/// Receiving half of the interrupt channel.
pub struct InterruptReceiver {
    rx: mpsc::Receiver<InterruptSignal>,
}

/// Factory for creating interrupt channel pairs.
pub struct InterruptChannel;

impl InterruptChannel {
    /// Create a new interrupt channel with a bounded buffer of 16.
    pub fn new() -> (InterruptSender, InterruptReceiver) {
        let (tx, rx) = mpsc::channel(16);
        (InterruptSender { tx }, InterruptReceiver { rx })
    }
}

impl InterruptSender {
    /// Send an interrupt signal (non-blocking, drops if channel is full).
    pub fn send(&self, signal: InterruptSignal) {
        let _ = self.tx.try_send(signal);
    }
}

impl InterruptReceiver {
    /// Drain channel, return the latest signal (newest message wins).
    pub fn try_recv(&mut self) -> Option<InterruptSignal> {
        let mut latest = None;
        while let Ok(signal) = self.rx.try_recv() {
            latest = Some(signal);
        }
        latest
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_interrupt_signal_send_receive() {
        let (tx, mut rx) = InterruptChannel::new();
        assert!(rx.try_recv().is_none());

        tx.send(InterruptSignal::NewMessage {
            content: "stop that".into(),
        });

        let signal = rx.try_recv();
        assert!(signal.is_some());
        match signal.unwrap() {
            InterruptSignal::NewMessage { content } => assert_eq!(content, "stop that"),
        }
    }

    #[tokio::test]
    async fn test_interrupt_channel_latest_wins() {
        let (tx, mut rx) = InterruptChannel::new();
        tx.send(InterruptSignal::NewMessage {
            content: "first".into(),
        });
        tx.send(InterruptSignal::NewMessage {
            content: "second".into(),
        });

        let signal = rx.try_recv().unwrap();
        match signal {
            InterruptSignal::NewMessage { content } => assert_eq!(content, "second"),
        }
    }
}
