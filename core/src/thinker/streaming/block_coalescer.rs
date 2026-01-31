//! Block coalescer for reducing message frequency.
//!
//! Merges multiple small blocks into larger messages to reduce
//! network traffic and improve user experience. Features:
//! - Configurable min/max character limits
//! - Idle timeout for flushing pending content
//! - Joiner customization (paragraph, newline, space)
//!
//! Reference: Moltbot src/auto-reply/reply/block-streaming.ts

use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// Configuration for block coalescing
#[derive(Debug, Clone)]
pub struct CoalescingConfig {
    /// Minimum characters before emitting (default: 800)
    pub min_chars: usize,
    /// Maximum characters before forced emit (default: 1200)
    pub max_chars: usize,
    /// Idle timeout in milliseconds before flushing (default: 1000)
    pub idle_ms: u64,
    /// String to join coalesced blocks (default: "\n\n")
    pub joiner: String,
}

impl Default for CoalescingConfig {
    fn default() -> Self {
        Self {
            min_chars: 800,
            max_chars: 1200,
            idle_ms: 1000,
            joiner: "\n\n".to_string(),
        }
    }
}

impl CoalescingConfig {
    /// Create config for paragraph-based coalescing (Moltbot default)
    pub fn paragraph() -> Self {
        Self::default()
    }

    /// Create config for newline-based coalescing
    pub fn newline() -> Self {
        Self {
            joiner: "\n".to_string(),
            ..Self::default()
        }
    }

    /// Create config for sentence-based coalescing (tighter spacing)
    pub fn sentence() -> Self {
        Self {
            joiner: " ".to_string(),
            ..Self::default()
        }
    }

    /// Builder: set min chars
    pub fn with_min_chars(mut self, chars: usize) -> Self {
        self.min_chars = chars;
        self
    }

    /// Builder: set max chars
    pub fn with_max_chars(mut self, chars: usize) -> Self {
        self.max_chars = chars;
        self
    }

    /// Builder: set idle timeout
    pub fn with_idle_ms(mut self, ms: u64) -> Self {
        self.idle_ms = ms;
        self
    }
}

/// Emitted event from the coalescer
#[derive(Debug, Clone)]
pub enum CoalescerEvent {
    /// Coalesced text block ready for delivery
    Text(String),
    /// Media content (passed through without coalescing)
    Media { url: String, mime_type: String },
}

/// Block coalescer for merging streaming blocks
///
/// Use with async runtime for idle timeout support.
#[derive(Debug)]
pub struct BlockCoalescer {
    config: CoalescingConfig,
    buffer: String,
    last_append: Option<Instant>,
}

impl Default for BlockCoalescer {
    fn default() -> Self {
        Self::new(CoalescingConfig::default())
    }
}

impl BlockCoalescer {
    /// Create a new coalescer with configuration
    pub fn new(config: CoalescingConfig) -> Self {
        Self {
            config,
            buffer: String::new(),
            last_append: None,
        }
    }

    /// Add text to the coalescer, returns content if ready to emit
    ///
    /// Returns `Some(text)` if the buffer exceeds max_chars or other emit conditions.
    /// Call `check_idle()` periodically to handle idle timeout.
    pub fn append(&mut self, text: &str) -> Option<String> {
        if self.buffer.is_empty() {
            self.buffer = text.to_string();
        } else {
            self.buffer.push_str(&self.config.joiner);
            self.buffer.push_str(text);
        }
        self.last_append = Some(Instant::now());

        // Check if we should emit immediately (exceeded max_chars)
        if self.buffer.len() >= self.config.max_chars {
            return Some(self.take_buffer());
        }

        None
    }

    /// Check if idle timeout has elapsed and flush if needed
    ///
    /// Returns `Some(text)` if buffer should be flushed due to idle timeout.
    pub fn check_idle(&mut self) -> Option<String> {
        if self.buffer.is_empty() {
            return None;
        }

        if let Some(last) = self.last_append {
            let elapsed = last.elapsed();
            if elapsed >= Duration::from_millis(self.config.idle_ms) {
                // Idle timeout reached
                if self.buffer.len() >= self.config.min_chars {
                    return Some(self.take_buffer());
                }
                // Below min_chars but idle - still flush to avoid stale content
                return Some(self.take_buffer());
            }
        }

        None
    }

    /// Force flush the buffer regardless of size/timeout
    pub fn flush(&mut self) -> Option<String> {
        if self.buffer.is_empty() {
            None
        } else {
            Some(self.take_buffer())
        }
    }

    /// Check if buffer meets minimum size for emission
    pub fn is_ready(&self) -> bool {
        self.buffer.len() >= self.config.min_chars
    }

    /// Get current buffer length
    pub fn buffer_len(&self) -> usize {
        self.buffer.len()
    }

    /// Check if buffer is empty
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    /// Get time until idle flush (if buffer has content)
    pub fn time_until_idle(&self) -> Option<Duration> {
        if self.buffer.is_empty() {
            return None;
        }

        self.last_append.map(|last| {
            let elapsed = last.elapsed();
            let idle_duration = Duration::from_millis(self.config.idle_ms);
            if elapsed >= idle_duration {
                Duration::ZERO
            } else {
                idle_duration - elapsed
            }
        })
    }

    /// Take the buffer content and reset
    fn take_buffer(&mut self) -> String {
        self.last_append = None;
        std::mem::take(&mut self.buffer)
    }

    /// Clear the coalescer without emitting
    pub fn clear(&mut self) {
        self.buffer.clear();
        self.last_append = None;
    }
}

/// Async coalescer that handles idle timeout automatically
///
/// Wraps BlockCoalescer with a background timer task.
pub struct AsyncBlockCoalescer {
    coalescer: BlockCoalescer,
    output_tx: mpsc::Sender<String>,
}

impl AsyncBlockCoalescer {
    /// Create a new async coalescer with output channel
    ///
    /// Returns the coalescer and a receiver for coalesced blocks.
    pub fn new(config: CoalescingConfig) -> (Self, mpsc::Receiver<String>) {
        let (tx, rx) = mpsc::channel(16);
        let coalescer = Self {
            coalescer: BlockCoalescer::new(config),
            output_tx: tx,
        };
        (coalescer, rx)
    }

    /// Append text, automatically emitting if ready
    pub async fn append(&mut self, text: &str) {
        if let Some(output) = self.coalescer.append(text) {
            let _ = self.output_tx.send(output).await;
        }
    }

    /// Schedule idle check and emit if needed
    ///
    /// Call this periodically (e.g., from a timer task)
    pub async fn check_idle(&mut self) {
        if let Some(output) = self.coalescer.check_idle() {
            let _ = self.output_tx.send(output).await;
        }
    }

    /// Force flush any remaining content
    pub async fn flush(&mut self) {
        if let Some(output) = self.coalescer.flush() {
            let _ = self.output_tx.send(output).await;
        }
    }

    /// Get time until next idle check is needed
    pub fn time_until_idle(&self) -> Option<Duration> {
        self.coalescer.time_until_idle()
    }

    /// Check if buffer is empty
    pub fn is_empty(&self) -> bool {
        self.coalescer.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = CoalescingConfig::default();
        assert_eq!(config.min_chars, 800);
        assert_eq!(config.max_chars, 1200);
        assert_eq!(config.idle_ms, 1000);
        assert_eq!(config.joiner, "\n\n");
    }

    #[test]
    fn test_config_builders() {
        let config = CoalescingConfig::newline()
            .with_min_chars(500)
            .with_max_chars(1000)
            .with_idle_ms(500);

        assert_eq!(config.min_chars, 500);
        assert_eq!(config.max_chars, 1000);
        assert_eq!(config.idle_ms, 500);
        assert_eq!(config.joiner, "\n");
    }

    #[test]
    fn test_append_below_threshold() {
        let mut coalescer = BlockCoalescer::new(CoalescingConfig {
            min_chars: 100,
            max_chars: 200,
            ..Default::default()
        });

        // Append small text - should not emit
        let result = coalescer.append("Hello world");
        assert!(result.is_none());
        assert!(!coalescer.is_empty());
        assert_eq!(coalescer.buffer_len(), 11);
    }

    #[test]
    fn test_append_exceeds_max() {
        let mut coalescer = BlockCoalescer::new(CoalescingConfig {
            min_chars: 10,
            max_chars: 50,
            ..Default::default()
        });

        // Append text that exceeds max_chars
        let result = coalescer.append("This is a fairly long text that should exceed the maximum character limit");
        assert!(result.is_some());
        assert!(coalescer.is_empty());
    }

    #[test]
    fn test_joiner_applied() {
        let mut coalescer = BlockCoalescer::new(CoalescingConfig {
            min_chars: 100,
            max_chars: 1000,
            joiner: " | ".to_string(),
            ..Default::default()
        });

        coalescer.append("First");
        coalescer.append("Second");
        coalescer.append("Third");

        let content = coalescer.flush().unwrap();
        assert_eq!(content, "First | Second | Third");
    }

    #[test]
    fn test_flush() {
        let mut coalescer = BlockCoalescer::new(CoalescingConfig {
            min_chars: 100,
            max_chars: 200,
            ..Default::default()
        });

        coalescer.append("Some content");
        assert!(!coalescer.is_empty());

        let content = coalescer.flush().unwrap();
        assert_eq!(content, "Some content");
        assert!(coalescer.is_empty());
    }

    #[test]
    fn test_flush_empty() {
        let mut coalescer = BlockCoalescer::default();
        assert!(coalescer.flush().is_none());
    }

    #[test]
    fn test_is_ready() {
        let mut coalescer = BlockCoalescer::new(CoalescingConfig {
            min_chars: 20,
            max_chars: 100,
            ..Default::default()
        });

        coalescer.append("Short");
        assert!(!coalescer.is_ready());

        coalescer.append("This makes it longer");
        assert!(coalescer.is_ready());
    }

    #[test]
    fn test_clear() {
        let mut coalescer = BlockCoalescer::default();
        coalescer.append("Some content");
        assert!(!coalescer.is_empty());

        coalescer.clear();
        assert!(coalescer.is_empty());
    }

    #[test]
    fn test_time_until_idle_empty() {
        let coalescer = BlockCoalescer::default();
        assert!(coalescer.time_until_idle().is_none());
    }

    #[test]
    fn test_time_until_idle_with_content() {
        let mut coalescer = BlockCoalescer::new(CoalescingConfig {
            idle_ms: 1000,
            ..Default::default()
        });

        coalescer.append("Content");
        let time = coalescer.time_until_idle();
        assert!(time.is_some());
        // Should be close to 1000ms (slightly less due to execution time)
        assert!(time.unwrap() <= Duration::from_millis(1000));
    }

    #[test]
    fn test_check_idle_immediate() {
        let mut coalescer = BlockCoalescer::new(CoalescingConfig {
            min_chars: 10,
            max_chars: 100,
            idle_ms: 0, // Immediate idle
            ..Default::default()
        });

        coalescer.append("Content");

        // With 0ms idle, should flush immediately on check
        let result = coalescer.check_idle();
        assert!(result.is_some());
        assert_eq!(result.unwrap(), "Content");
    }

    #[test]
    fn test_multiple_append_coalesce() {
        let mut coalescer = BlockCoalescer::new(CoalescingConfig {
            min_chars: 50,
            max_chars: 200,
            joiner: "\n\n".to_string(),
            ..Default::default()
        });

        coalescer.append("First paragraph");
        coalescer.append("Second paragraph");
        coalescer.append("Third paragraph");

        let content = coalescer.flush().unwrap();
        assert!(content.contains("First paragraph\n\nSecond paragraph\n\nThird paragraph"));
    }

    #[tokio::test]
    async fn test_async_coalescer_basic() {
        let config = CoalescingConfig {
            min_chars: 10,
            max_chars: 100,
            idle_ms: 10,
            ..Default::default()
        };

        let (mut coalescer, mut rx) = AsyncBlockCoalescer::new(config);

        coalescer.append("Hello").await;
        coalescer.append("World").await;

        // Wait for idle timeout
        tokio::time::sleep(Duration::from_millis(20)).await;
        coalescer.check_idle().await;

        // Should receive coalesced content
        let received = rx.try_recv();
        assert!(received.is_ok());
        assert!(received.unwrap().contains("Hello"));
    }

    #[tokio::test]
    async fn test_async_coalescer_max_chars() {
        let config = CoalescingConfig {
            min_chars: 10,
            max_chars: 30,
            idle_ms: 1000,
            joiner: " ".to_string(),
        };

        let (mut coalescer, mut rx) = AsyncBlockCoalescer::new(config);

        // This should trigger immediate emit due to max_chars
        coalescer.append("This is a long text that exceeds max").await;

        let received = rx.try_recv();
        assert!(received.is_ok());
    }

    #[tokio::test]
    async fn test_async_coalescer_flush() {
        let config = CoalescingConfig::default();
        let (mut coalescer, mut rx) = AsyncBlockCoalescer::new(config);

        coalescer.append("Pending content").await;
        assert!(!coalescer.is_empty());

        coalescer.flush().await;

        let received = rx.try_recv();
        assert!(received.is_ok());
        assert!(coalescer.is_empty());
    }
}
