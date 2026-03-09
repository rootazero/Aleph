//! Session queue abstraction for message handling strategies.
//!
//! Defines how incoming messages are handled while the agent is busy
//! processing a previous turn. Three modes are supported:
//!
//! - **Followup**: Messages wait in FIFO order (default)
//! - **Steer**: New message interrupts current tool execution
//! - **Collect**: Messages are batched over a time window then merged

use async_trait::async_trait;
use std::collections::VecDeque;

use super::interrupt::{InterruptSender, InterruptSignal};

/// Queue mode determines how new messages are handled while agent is busy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum QueueMode {
    /// Messages wait in line, processed one by one after current turn.
    Followup,
    /// New message interrupts current tool execution (steering).
    Steer,
    /// Collect messages for N seconds, then merge into one.
    Collect,
}

impl Default for QueueMode {
    fn default() -> Self {
        Self::Followup
    }
}

/// Abstraction for session message queuing strategies.
///
/// Implementations control how messages are buffered and retrieved
/// while the agent loop is busy with a current turn.
#[async_trait]
pub trait SessionQueue: Send + Sync {
    /// Add a message to the queue.
    async fn enqueue(&mut self, content: String);

    /// Retrieve the next message to process, if any.
    async fn next(&mut self) -> Option<String>;

    /// The queue mode this implementation represents.
    fn mode(&self) -> QueueMode;
}

/// Default queue: messages processed sequentially in FIFO order.
pub struct FollowupQueue {
    queue: VecDeque<String>,
}

impl FollowupQueue {
    pub fn new() -> Self {
        Self {
            queue: VecDeque::new(),
        }
    }
}

impl Default for FollowupQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SessionQueue for FollowupQueue {
    async fn enqueue(&mut self, content: String) {
        self.queue.push_back(content);
    }

    async fn next(&mut self) -> Option<String> {
        self.queue.pop_front()
    }

    fn mode(&self) -> QueueMode {
        QueueMode::Followup
    }
}

/// Steering queue: interrupts the agent loop when a new message arrives.
///
/// When `enqueue` is called, SteerQueue sends an [`InterruptSignal::NewMessage`]
/// through the interrupt channel AND stores the message for later retrieval.
/// This allows the agent loop to cancel its current tool execution and re-think
/// with the new user intent.
pub struct SteerQueue {
    interrupt_tx: InterruptSender,
    pending: VecDeque<String>,
}

impl SteerQueue {
    pub fn new(interrupt_tx: InterruptSender) -> Self {
        Self {
            interrupt_tx,
            pending: VecDeque::new(),
        }
    }
}

#[async_trait]
impl SessionQueue for SteerQueue {
    async fn enqueue(&mut self, content: String) {
        self.interrupt_tx.send(InterruptSignal::NewMessage {
            content: content.clone(),
        });
        self.pending.push_back(content);
    }

    async fn next(&mut self) -> Option<String> {
        self.pending.pop_front()
    }

    fn mode(&self) -> QueueMode {
        QueueMode::Steer
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_followup_queue_processes_in_order() {
        let mut queue = FollowupQueue::new();
        queue.enqueue("first message".into()).await;
        queue.enqueue("second message".into()).await;

        assert_eq!(queue.next().await, Some("first message".to_string()));
        assert_eq!(queue.next().await, Some("second message".to_string()));
        assert_eq!(queue.next().await, None);
    }

    #[tokio::test]
    async fn test_followup_queue_does_not_merge() {
        let mut queue = FollowupQueue::new();
        queue.enqueue("hello".into()).await;
        queue.enqueue("world".into()).await;

        assert_eq!(queue.next().await, Some("hello".to_string()));
        assert_eq!(queue.next().await, Some("world".to_string()));
    }

    #[test]
    fn test_queue_mode_default_is_followup() {
        assert_eq!(QueueMode::default(), QueueMode::Followup);
    }

    #[test]
    fn test_queue_mode_serde_roundtrip() {
        let mode = QueueMode::Followup;
        let json = serde_json::to_string(&mode).unwrap();
        assert_eq!(json, "\"followup\"");
        let parsed: QueueMode = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, mode);

        let steer_json = "\"steer\"";
        let parsed: QueueMode = serde_json::from_str(steer_json).unwrap();
        assert_eq!(parsed, QueueMode::Steer);

        let collect_json = "\"collect\"";
        let parsed: QueueMode = serde_json::from_str(collect_json).unwrap();
        assert_eq!(parsed, QueueMode::Collect);
    }

    #[tokio::test]
    async fn test_followup_queue_empty_on_creation() {
        let mut queue = FollowupQueue::new();
        assert_eq!(queue.next().await, None);
    }

    #[test]
    fn test_followup_queue_mode() {
        let queue = FollowupQueue::new();
        assert_eq!(queue.mode(), QueueMode::Followup);
    }

    #[tokio::test]
    async fn test_steer_queue_sends_interrupt() {
        use crate::agent_loop::InterruptChannel;

        let (interrupt_tx, mut interrupt_rx) = InterruptChannel::new();
        let mut queue = SteerQueue::new(interrupt_tx);

        queue.enqueue("change of plan".into()).await;

        let signal: Option<InterruptSignal> = interrupt_rx.try_recv();
        assert!(signal.is_some());
        match signal.unwrap() {
            InterruptSignal::NewMessage { content } => {
                assert_eq!(content, "change of plan");
            }
        }

        assert_eq!(queue.next().await, Some("change of plan".to_string()));
    }

    #[test]
    fn test_steer_queue_mode() {
        let (interrupt_tx, _rx) = crate::agent_loop::InterruptChannel::new();
        let queue = SteerQueue::new(interrupt_tx);
        assert_eq!(queue.mode(), QueueMode::Steer);
    }
}
