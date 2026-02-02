//! Stream buffer for block-level text flushing before tool execution

/// Manages accumulated text with flush-before-tool semantics
#[derive(Debug, Default)]
pub struct StreamBuffer {
    /// Accumulated text content
    text: String,
    /// Position up to which text has been flushed
    flushed_at: usize,
    /// Whether currently executing a tool
    in_tool_execution: bool,
}

impl StreamBuffer {
    /// Create a new empty buffer
    pub fn new() -> Self {
        Self::default()
    }

    /// Append text to the buffer
    pub fn append(&mut self, content: &str) {
        self.text.push_str(content);
    }

    /// Flush unflushed text before tool execution
    ///
    /// Returns Some(text) if there's non-empty unflushed content,
    /// None otherwise. Marks buffer as in tool execution state.
    pub fn flush_before_tool(&mut self) -> Option<String> {
        self.in_tool_execution = true;

        if self.flushed_at >= self.text.len() {
            return None;
        }

        let unflushed = self.text[self.flushed_at..].to_string();
        self.flushed_at = self.text.len();

        if unflushed.trim().is_empty() {
            None
        } else {
            Some(unflushed)
        }
    }

    /// Mark tool execution as ended
    pub fn tool_ended(&mut self) {
        self.in_tool_execution = false;
    }

    /// Check if currently in tool execution
    pub fn is_in_tool_execution(&self) -> bool {
        self.in_tool_execution
    }

    /// Get all accumulated text
    pub fn full_text(&self) -> &str {
        &self.text
    }

    /// Get unflushed text (without flushing)
    pub fn unflushed_text(&self) -> &str {
        &self.text[self.flushed_at..]
    }

    /// Get length of accumulated text
    pub fn len(&self) -> usize {
        self.text.len()
    }

    /// Check if buffer is empty
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Reset buffer to initial state
    pub fn reset(&mut self) {
        self.text.clear();
        self.flushed_at = 0;
        self.in_tool_execution = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_buffer() {
        let buffer = StreamBuffer::new();
        assert!(buffer.is_empty());
        assert!(!buffer.is_in_tool_execution());
    }

    #[test]
    fn test_append() {
        let mut buffer = StreamBuffer::new();
        buffer.append("Hello ");
        buffer.append("World");
        assert_eq!(buffer.full_text(), "Hello World");
        assert_eq!(buffer.len(), 11);
    }

    #[test]
    fn test_flush_before_tool() {
        let mut buffer = StreamBuffer::new();
        buffer.append("First chunk. ");

        let flushed = buffer.flush_before_tool();
        assert_eq!(flushed, Some("First chunk. ".to_string()));
        assert!(buffer.is_in_tool_execution());

        // Second flush should return None
        let flushed2 = buffer.flush_before_tool();
        assert!(flushed2.is_none());
    }

    #[test]
    fn test_flush_empty_returns_none() {
        let mut buffer = StreamBuffer::new();
        buffer.append("   ");  // Only whitespace

        let flushed = buffer.flush_before_tool();
        assert!(flushed.is_none());
    }

    #[test]
    fn test_tool_ended() {
        let mut buffer = StreamBuffer::new();
        buffer.flush_before_tool();
        assert!(buffer.is_in_tool_execution());

        buffer.tool_ended();
        assert!(!buffer.is_in_tool_execution());
    }

    #[test]
    fn test_append_after_flush() {
        let mut buffer = StreamBuffer::new();
        buffer.append("Before tool. ");
        buffer.flush_before_tool();
        buffer.tool_ended();

        buffer.append("After tool.");
        let flushed = buffer.flush_before_tool();
        assert_eq!(flushed, Some("After tool.".to_string()));
    }

    #[test]
    fn test_reset() {
        let mut buffer = StreamBuffer::new();
        buffer.append("Some text");
        buffer.flush_before_tool();

        buffer.reset();
        assert!(buffer.is_empty());
        assert!(!buffer.is_in_tool_execution());
        assert_eq!(buffer.unflushed_text(), "");
    }

    #[test]
    fn test_unflushed_text() {
        let mut buffer = StreamBuffer::new();
        buffer.append("Part 1. ");
        buffer.flush_before_tool();
        buffer.append("Part 2.");

        assert_eq!(buffer.unflushed_text(), "Part 2.");
        assert_eq!(buffer.full_text(), "Part 1. Part 2.");
    }
}
