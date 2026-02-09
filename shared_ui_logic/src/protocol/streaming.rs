//! Streaming event handling

use futures::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc;

/// Stream event from the server
///
/// Represents a streaming event sent by the Aleph Gateway.
/// These events are used for real-time updates like Agent thinking,
/// tool execution progress, etc.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamEvent {
    /// Event type (e.g., "agent.thinking", "tool.call_started")
    #[serde(rename = "type")]
    pub event_type: String,

    /// Event payload
    pub payload: Value,

    /// Optional timestamp (milliseconds since epoch)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<u64>,

    /// Optional session/run ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

/// Stream handler for processing streaming events
///
/// This handler provides a convenient way to work with streaming
/// events from the server. It converts the event channel into
/// a Stream that can be easily consumed.
///
/// ## Example
///
/// ```rust,ignore
/// use aleph_ui_logic::protocol::StreamHandler;
///
/// let (handler, tx) = StreamHandler::new();
///
/// // Convert to stream and filter by type
/// let agent_events = handler
///     .filter_by_type("agent.thinking")
///     .into_stream();
///
/// // Process events
/// while let Some(event) = agent_events.next().await {
///     println!("Agent thinking: {:?}", event);
/// }
/// ```
pub struct StreamHandler {
    event_rx: mpsc::UnboundedReceiver<StreamEvent>,
}

impl StreamHandler {
    /// Create a new stream handler
    ///
    /// Returns a tuple of (handler, sender). The sender can be used
    /// to send events to the handler.
    ///
    /// # Example
    ///
    /// ```rust
    /// use aleph_ui_logic::protocol::StreamHandler;
    ///
    /// let (handler, tx) = StreamHandler::new();
    /// ```
    pub fn new() -> (Self, mpsc::UnboundedSender<StreamEvent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self { event_rx: rx }, tx)
    }

    /// Convert the handler into a Stream
    ///
    /// This consumes the handler and returns a Stream that yields
    /// StreamEvents.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let (handler, _tx) = StreamHandler::new();
    /// let mut stream = handler.into_stream();
    ///
    /// while let Some(event) = stream.next().await {
    ///     println!("Event: {:?}", event);
    /// }
    /// ```
    pub fn into_stream(self) -> impl Stream<Item = StreamEvent> {
        futures::stream::unfold(self.event_rx, |mut rx| async move {
            rx.recv().await.map(|event| (event, rx))
        })
    }

    /// Filter events by type
    ///
    /// Returns a new handler that only yields events matching the
    /// specified type.
    ///
    /// # Arguments
    ///
    /// - `event_type`: The event type to filter for
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let (handler, _tx) = StreamHandler::new();
    /// let agent_events = handler.filter_by_type("agent.thinking");
    /// ```
    pub fn filter_by_type(self, event_type: &str) -> FilteredStreamHandler {
        FilteredStreamHandler {
            inner: self,
            event_type: event_type.to_string(),
        }
    }

    /// Filter events by session ID
    ///
    /// Returns a new handler that only yields events matching the
    /// specified session ID.
    pub fn filter_by_session(self, session_id: &str) -> FilteredBySessionHandler {
        FilteredBySessionHandler {
            inner: self,
            session_id: session_id.to_string(),
        }
    }
}

impl Default for StreamHandler {
    fn default() -> Self {
        Self::new().0
    }
}

/// Filtered stream handler (by event type)
pub struct FilteredStreamHandler {
    inner: StreamHandler,
    event_type: String,
}

impl FilteredStreamHandler {
    /// Convert to stream
    pub fn into_stream(self) -> impl Stream<Item = StreamEvent> {
        let event_type = self.event_type;
        self.inner.into_stream().filter(move |event| {
            let matches = event.event_type == event_type;
            async move { matches }
        })
    }
}

/// Filtered stream handler (by session ID)
pub struct FilteredBySessionHandler {
    inner: StreamHandler,
    session_id: String,
}

impl FilteredBySessionHandler {
    /// Convert to stream
    pub fn into_stream(self) -> impl Stream<Item = StreamEvent> {
        let session_id = self.session_id;
        self.inner.into_stream().filter(move |event| {
            let matches = event
                .session_id
                .as_ref()
                .map(|id| id == &session_id)
                .unwrap_or(false);
            async move { matches }
        })
    }
}

/// Stream buffer for accumulating streaming data
///
/// This is useful for handling streaming text output (like LLM responses)
/// where you want to accumulate chunks and process them together.
///
/// ## Example
///
/// ```rust
/// use aleph_ui_logic::protocol::StreamBuffer;
///
/// let mut buffer = StreamBuffer::new();
///
/// buffer.append("Hello ");
/// buffer.append("world!");
///
/// assert_eq!(buffer.content(), "Hello world!");
/// ```
pub struct StreamBuffer {
    content: String,
    max_size: Option<usize>,
}

impl StreamBuffer {
    /// Create a new stream buffer
    pub fn new() -> Self {
        Self {
            content: String::new(),
            max_size: None,
        }
    }

    /// Create a new stream buffer with a maximum size
    ///
    /// When the buffer exceeds the maximum size, old content
    /// will be truncated.
    pub fn with_max_size(max_size: usize) -> Self {
        Self {
            content: String::new(),
            max_size: Some(max_size),
        }
    }

    /// Append content to the buffer
    pub fn append(&mut self, text: &str) {
        self.content.push_str(text);

        // Truncate if exceeds max size
        if let Some(max_size) = self.max_size {
            if self.content.len() > max_size {
                let start = self.content.len() - max_size;
                self.content = self.content[start..].to_string();
            }
        }
    }

    /// Get the current content
    pub fn content(&self) -> &str {
        &self.content
    }

    /// Clear the buffer
    pub fn clear(&mut self) {
        self.content.clear();
    }

    /// Get the current size
    pub fn len(&self) -> usize {
        self.content.len()
    }

    /// Check if the buffer is empty
    pub fn is_empty(&self) -> bool {
        self.content.is_empty()
    }
}

impl Default for StreamBuffer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stream_buffer() {
        let mut buffer = StreamBuffer::new();
        assert!(buffer.is_empty());

        buffer.append("Hello ");
        buffer.append("world!");
        assert_eq!(buffer.content(), "Hello world!");
        assert_eq!(buffer.len(), 12);

        buffer.clear();
        assert!(buffer.is_empty());
    }

    #[test]
    fn test_stream_buffer_max_size() {
        let mut buffer = StreamBuffer::with_max_size(10);

        buffer.append("Hello ");  // 6 chars
        buffer.append("world!");  // 6 chars, total 12
        // After truncation: last 10 chars of "Hello world!" = "llo world!"
        assert_eq!(buffer.content(), "llo world!");
        assert_eq!(buffer.len(), 10);
    }

    #[test]
    fn test_stream_event_serialization() {
        let event = StreamEvent {
            event_type: "test.event".to_string(),
            payload: serde_json::json!({"key": "value"}),
            timestamp: Some(1234567890),
            session_id: Some("session-123".to_string()),
        };

        let json = serde_json::to_string(&event).unwrap();
        let deserialized: StreamEvent = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.event_type, "test.event");
        assert_eq!(deserialized.timestamp, Some(1234567890));
    }
}
