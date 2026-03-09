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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum QueueMode {
    /// Messages wait in line, processed one by one after current turn.
    #[default]
    Followup,
    /// New message interrupts current tool execution (steering).
    Steer,
    /// Collect messages for N seconds, then merge into one.
    Collect,
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

/// Collecting queue: buffers messages within a time window, then merges them.
///
/// When the first message arrives, a timer starts. Subsequent messages within
/// the window are buffered. Once the window elapses, `next()` returns all
/// buffered messages merged with double-newline separators.
///
/// Useful for group chat or rapid-fire input scenarios where you want to
/// batch multiple messages into one before triggering the agent.
pub struct CollectQueue {
    buffer: Vec<String>,
    window: std::time::Duration,
    first_received: Option<std::time::Instant>,
}

impl CollectQueue {
    pub fn new(window: std::time::Duration) -> Self {
        Self {
            buffer: Vec::new(),
            window,
            first_received: None,
        }
    }
}

#[async_trait]
impl SessionQueue for CollectQueue {
    async fn enqueue(&mut self, content: String) {
        if self.first_received.is_none() {
            self.first_received = Some(std::time::Instant::now());
        }
        self.buffer.push(content);
    }

    async fn next(&mut self) -> Option<String> {
        if self.buffer.is_empty() {
            return None;
        }

        // Check if window has elapsed since first message
        if let Some(first) = self.first_received {
            if first.elapsed() < self.window {
                return None; // Still collecting
            }
        }

        // Window elapsed — merge and return
        let merged = self.buffer.join("\n\n");
        self.buffer.clear();
        self.first_received = None;
        Some(merged)
    }

    fn mode(&self) -> QueueMode {
        QueueMode::Collect
    }
}

/// Create a session queue based on the configured mode.
///
/// This factory function instantiates the appropriate queue implementation
/// based on the given `QueueMode`. For `Steer` mode, an `InterruptSender`
/// is required. For `Collect` mode, an optional window duration can be
/// specified (defaults to 3000ms).
pub fn create_session_queue(
    mode: QueueMode,
    collect_window_ms: Option<u64>,
    interrupt_tx: Option<InterruptSender>,
) -> Box<dyn SessionQueue> {
    match mode {
        QueueMode::Followup => Box::new(FollowupQueue::new()),
        QueueMode::Steer => {
            let tx = interrupt_tx.expect("SteerQueue requires interrupt channel");
            Box::new(SteerQueue::new(tx))
        }
        QueueMode::Collect => {
            let window = std::time::Duration::from_millis(
                collect_window_ms.unwrap_or(3000),
            );
            Box::new(CollectQueue::new(window))
        }
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

    #[tokio::test]
    async fn test_collect_queue_merges_within_window() {
        let mut queue = CollectQueue::new(std::time::Duration::from_millis(50));

        queue.enqueue("hello".into()).await;
        queue.enqueue("world".into()).await;

        // Within window — should not yield yet
        assert_eq!(queue.next().await, None);

        // Wait for window to elapse
        tokio::time::sleep(std::time::Duration::from_millis(60)).await;

        let merged = queue.next().await;
        assert!(merged.is_some());
        let text = merged.unwrap();
        assert!(text.contains("hello"));
        assert!(text.contains("world"));
    }

    #[tokio::test]
    async fn test_collect_queue_empty_returns_none() {
        let mut queue = CollectQueue::new(std::time::Duration::from_millis(50));
        assert_eq!(queue.next().await, None);
    }

    #[test]
    fn test_collect_queue_mode() {
        let queue = CollectQueue::new(std::time::Duration::from_millis(100));
        assert_eq!(queue.mode(), QueueMode::Collect);
    }

    #[test]
    fn test_queue_mode_deserializes_from_json() {
        let json = r#""steer""#;
        let mode: QueueMode = serde_json::from_str(json).unwrap();
        assert_eq!(mode, QueueMode::Steer);

        let json = r#""followup""#;
        let mode: QueueMode = serde_json::from_str(json).unwrap();
        assert_eq!(mode, QueueMode::Followup);

        let json = r#""collect""#;
        let mode: QueueMode = serde_json::from_str(json).unwrap();
        assert_eq!(mode, QueueMode::Collect);
    }

    #[test]
    fn test_create_session_queue_followup() {
        let queue = create_session_queue(QueueMode::Followup, None, None);
        assert_eq!(queue.mode(), QueueMode::Followup);
    }

    #[test]
    fn test_create_session_queue_steer() {
        let (tx, _rx) = crate::agent_loop::InterruptChannel::new();
        let queue = create_session_queue(QueueMode::Steer, None, Some(tx));
        assert_eq!(queue.mode(), QueueMode::Steer);
    }

    #[test]
    fn test_create_session_queue_collect() {
        let queue = create_session_queue(QueueMode::Collect, Some(5000), None);
        assert_eq!(queue.mode(), QueueMode::Collect);
    }

    #[test]
    fn test_create_session_queue_collect_default_window() {
        let queue = create_session_queue(QueueMode::Collect, None, None);
        assert_eq!(queue.mode(), QueueMode::Collect);
    }

    #[test]
    #[should_panic(expected = "SteerQueue requires interrupt channel")]
    fn test_create_session_queue_steer_panics_without_interrupt() {
        create_session_queue(QueueMode::Steer, None, None);
    }

    // ── Integration tests ──────────────────────────────────────────

    #[tokio::test]
    async fn test_steering_full_chain() {
        use crate::agent_loop::{InterruptChannel, InterruptSignal};

        // Setup: create interrupt channel and steer queue
        let (interrupt_tx, mut interrupt_rx) = InterruptChannel::new();
        let mut queue = SteerQueue::new(interrupt_tx);

        // Simulate: user sends message while agent is "busy"
        queue.enqueue("actually, do something else".into()).await;

        // Verify: interrupt signal was sent
        let signal = interrupt_rx.try_recv();
        assert!(signal.is_some(), "Expected interrupt signal from SteerQueue");
        match signal.unwrap() {
            InterruptSignal::NewMessage { content } => {
                assert_eq!(content, "actually, do something else");
            }
        }

        // Verify: message is still available for processing
        let next_msg = queue.next().await;
        assert_eq!(next_msg, Some("actually, do something else".to_string()));

        // Verify: queue is drained
        assert_eq!(queue.next().await, None);
        assert!(interrupt_rx.try_recv().is_none());
    }

    #[tokio::test]
    async fn test_steer_queue_multiple_steers_in_sequence() {
        use crate::agent_loop::{InterruptChannel, InterruptSignal};

        let (tx, mut rx) = InterruptChannel::new();
        let mut queue = SteerQueue::new(tx);

        // First steer
        queue.enqueue("stop, do X".into()).await;
        let sig = rx.try_recv().unwrap();
        match sig {
            InterruptSignal::NewMessage { content } => assert_eq!(content, "stop, do X"),
        }
        assert_eq!(queue.next().await, Some("stop, do X".into()));

        // Second steer — channel should still work
        queue.enqueue("no wait, do Y".into()).await;
        let sig = rx.try_recv().unwrap();
        match sig {
            InterruptSignal::NewMessage { content } => assert_eq!(content, "no wait, do Y"),
        }
        assert_eq!(queue.next().await, Some("no wait, do Y".into()));

        // Both drained
        assert_eq!(queue.next().await, None);
        assert!(rx.try_recv().is_none());
    }

    #[tokio::test]
    async fn test_all_queue_modes_lifecycle() {
        use crate::agent_loop::InterruptChannel;

        // Followup: FIFO, no merging
        let mut followup = FollowupQueue::new();
        assert_eq!(followup.mode(), QueueMode::Followup);
        followup.enqueue("a".into()).await;
        followup.enqueue("b".into()).await;
        assert_eq!(followup.next().await, Some("a".into()));
        assert_eq!(followup.next().await, Some("b".into()));
        assert_eq!(followup.next().await, None);

        // Steer: sends interrupt + retains message
        let (tx, mut rx) = InterruptChannel::new();
        let mut steer = SteerQueue::new(tx);
        assert_eq!(steer.mode(), QueueMode::Steer);
        steer.enqueue("x".into()).await;
        assert!(rx.try_recv().is_some());
        assert_eq!(steer.next().await, Some("x".into()));
        assert_eq!(steer.next().await, None);

        // Collect: buffers within window, merges after
        let mut collect = CollectQueue::new(std::time::Duration::from_millis(30));
        assert_eq!(collect.mode(), QueueMode::Collect);
        collect.enqueue("1".into()).await;
        collect.enqueue("2".into()).await;
        assert_eq!(collect.next().await, None); // Still in window
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
        let merged = collect.next().await.unwrap();
        assert!(merged.contains("1") && merged.contains("2"));
        assert_eq!(collect.next().await, None);
    }

    #[test]
    fn test_create_session_queue_factory_all_modes() {
        use crate::agent_loop::InterruptChannel;

        // Followup mode
        let queue = create_session_queue(QueueMode::Followup, None, None);
        assert_eq!(queue.mode(), QueueMode::Followup);

        // Steer mode
        let (tx, _rx) = InterruptChannel::new();
        let queue = create_session_queue(QueueMode::Steer, None, Some(tx));
        assert_eq!(queue.mode(), QueueMode::Steer);

        // Collect mode with custom window
        let queue = create_session_queue(QueueMode::Collect, Some(5000), None);
        assert_eq!(queue.mode(), QueueMode::Collect);

        // Collect mode with default window
        let queue = create_session_queue(QueueMode::Collect, None, None);
        assert_eq!(queue.mode(), QueueMode::Collect);
    }
}
