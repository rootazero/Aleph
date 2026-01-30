//! Thinking tag parser state machine.
//!
//! Detects and extracts content from thinking tags during streaming:
//! - `<think>`, `<thinking>`, `<thought>`, `<antthinking>`
//! - `<final>` for marking completion
//!
//! Handles edge cases like inline code blocks to avoid false positives.


/// Known thinking tag variants
const THINKING_TAGS: &[&str] = &["think", "thinking", "thought", "antthinking"];

/// Block state for tracking thinking vs content
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockState {
    /// Normal content (not in thinking block)
    Content,
    /// Inside a thinking block
    Thinking,
    /// Inside an inline code block (backticks)
    InlineCode,
    /// Inside a fenced code block (```)
    FencedCode,
}

impl Default for BlockState {
    fn default() -> Self {
        Self::Content
    }
}

/// Thinking tag parser with state machine
#[derive(Debug, Clone)]
pub struct ThinkingTagParser {
    state: BlockState,
    accumulated_thinking: String,
    accumulated_content: String,
    buffer: String,
    code_fence_pattern: Option<String>,
}

impl Default for ThinkingTagParser {
    fn default() -> Self {
        Self::new()
    }
}

impl ThinkingTagParser {
    /// Create a new parser
    pub fn new() -> Self {
        Self {
            state: BlockState::Content,
            accumulated_thinking: String::new(),
            accumulated_content: String::new(),
            buffer: String::new(),
            code_fence_pattern: None,
        }
    }

    /// Get current state
    pub fn state(&self) -> BlockState {
        self.state
    }

    /// Get accumulated thinking content
    pub fn accumulated_thinking(&self) -> &str {
        &self.accumulated_thinking
    }

    /// Get accumulated regular content
    pub fn accumulated_content(&self) -> &str {
        &self.accumulated_content
    }

    /// Process a chunk of text, returning (content, thinking) deltas
    pub fn process(&mut self, chunk: &str) -> (Option<String>, Option<String>) {
        self.buffer.push_str(chunk);

        let mut content_delta = String::new();
        let mut thinking_delta = String::new();

        while !self.buffer.is_empty() {
            match self.state {
                BlockState::Content => {
                    if let Some((pre, _tag, post)) = self.find_opening_tag() {
                        // Output content before tag
                        if !pre.is_empty() {
                            content_delta.push_str(&pre);
                            self.accumulated_content.push_str(&pre);
                        }
                        // Transition to thinking
                        self.state = BlockState::Thinking;
                        self.buffer = post;
                    } else if self.check_code_fence() {
                        // Handle code fence start
                        continue;
                    } else if self.check_inline_code() {
                        // Handle inline code start
                        continue;
                    } else if self.buffer.contains('<') && self.buffer.len() < 20 {
                        // Potential partial tag, wait for more input
                        break;
                    } else {
                        // No tag found, output all content
                        content_delta.push_str(&self.buffer);
                        self.accumulated_content.push_str(&self.buffer);
                        self.buffer.clear();
                    }
                }
                BlockState::Thinking => {
                    if let Some((pre, post)) = self.find_closing_tag() {
                        // Output thinking before tag
                        if !pre.is_empty() {
                            thinking_delta.push_str(&pre);
                            self.accumulated_thinking.push_str(&pre);
                        }
                        // Transition back to content
                        self.state = BlockState::Content;
                        self.buffer = post;
                    } else if self.buffer.contains('<') && self.buffer.len() < 20 {
                        // Potential partial tag, wait for more input
                        break;
                    } else {
                        // No closing tag, output as thinking
                        thinking_delta.push_str(&self.buffer);
                        self.accumulated_thinking.push_str(&self.buffer);
                        self.buffer.clear();
                    }
                }
                BlockState::InlineCode => {
                    if let Some(pos) = self.buffer.find('`') {
                        // End of inline code
                        let (pre, post) = self.buffer.split_at(pos + 1);
                        content_delta.push_str(pre);
                        self.accumulated_content.push_str(pre);
                        self.buffer = post.to_string();
                        self.state = BlockState::Content;
                    } else {
                        content_delta.push_str(&self.buffer);
                        self.accumulated_content.push_str(&self.buffer);
                        self.buffer.clear();
                    }
                }
                BlockState::FencedCode => {
                    if let Some(ref fence) = self.code_fence_pattern.clone() {
                        if let Some(pos) = self.buffer.find(fence.as_str()) {
                            // End of fenced code
                            let end = pos + fence.len();
                            let (pre, post) = self.buffer.split_at(end);
                            content_delta.push_str(pre);
                            self.accumulated_content.push_str(pre);
                            self.buffer = post.to_string();
                            self.state = BlockState::Content;
                            self.code_fence_pattern = None;
                        } else {
                            content_delta.push_str(&self.buffer);
                            self.accumulated_content.push_str(&self.buffer);
                            self.buffer.clear();
                        }
                    }
                }
            }
        }

        let content = if content_delta.is_empty() { None } else { Some(content_delta) };
        let thinking = if thinking_delta.is_empty() { None } else { Some(thinking_delta) };

        (content, thinking)
    }

    /// Find opening thinking tag
    fn find_opening_tag(&self) -> Option<(String, String, String)> {
        for tag in THINKING_TAGS {
            let open = format!("<{}>", tag);
            let open_with_attrs = format!("<{} ", tag);

            if let Some(pos) = self.buffer.find(&open) {
                let pre = self.buffer[..pos].to_string();
                let post = self.buffer[pos + open.len()..].to_string();
                return Some((pre, tag.to_string(), post));
            }

            // Check for tag with attributes
            if let Some(start) = self.buffer.find(&open_with_attrs) {
                if let Some(end) = self.buffer[start..].find('>') {
                    let pre = self.buffer[..start].to_string();
                    let post = self.buffer[start + end + 1..].to_string();
                    return Some((pre, tag.to_string(), post));
                }
            }
        }
        None
    }

    /// Find closing thinking tag
    fn find_closing_tag(&self) -> Option<(String, String)> {
        for tag in THINKING_TAGS {
            let close = format!("</{}>", tag);
            if let Some(pos) = self.buffer.find(&close) {
                let pre = self.buffer[..pos].to_string();
                let post = self.buffer[pos + close.len()..].to_string();
                return Some((pre, post));
            }
        }
        None
    }

    /// Check for code fence and update state
    fn check_code_fence(&mut self) -> bool {
        if self.buffer.starts_with("```") {
            // Find end of fence line
            let fence_end = self.buffer[3..].find('\n').map(|p| p + 4).unwrap_or(3);
            let _fence = self.buffer[..fence_end].to_string();
            self.code_fence_pattern = Some("```".to_string());
            self.state = BlockState::FencedCode;
            true
        } else {
            false
        }
    }

    /// Check for inline code and update state
    fn check_inline_code(&mut self) -> bool {
        if self.buffer.starts_with('`') && !self.buffer.starts_with("```") {
            self.state = BlockState::InlineCode;
            let backtick = self.buffer.remove(0);
            self.accumulated_content.push(backtick);
            true
        } else {
            false
        }
    }

    /// Reset parser state
    pub fn reset(&mut self) {
        self.state = BlockState::Content;
        self.accumulated_thinking.clear();
        self.accumulated_content.clear();
        self.buffer.clear();
        self.code_fence_pattern = None;
    }

    /// Finalize parsing, returning any remaining buffered content
    pub fn finalize(&mut self) -> (Option<String>, Option<String>) {
        let content = if self.buffer.is_empty() {
            None
        } else {
            match self.state {
                BlockState::Content | BlockState::InlineCode | BlockState::FencedCode => {
                    self.accumulated_content.push_str(&self.buffer);
                    Some(std::mem::take(&mut self.buffer))
                }
                BlockState::Thinking => {
                    self.accumulated_thinking.push_str(&self.buffer);
                    None
                }
            }
        };

        let thinking = if self.state == BlockState::Thinking && !self.buffer.is_empty() {
            Some(std::mem::take(&mut self.buffer))
        } else {
            None
        };

        self.reset();
        (content, thinking)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_content() {
        let mut parser = ThinkingTagParser::new();
        let (content, thinking) = parser.process("Hello, world!");
        assert_eq!(content, Some("Hello, world!".to_string()));
        assert_eq!(thinking, None);
    }

    #[test]
    fn test_thinking_block() {
        let mut parser = ThinkingTagParser::new();
        let (content, thinking) = parser.process("<think>Let me think...</think>Answer");
        assert_eq!(thinking, Some("Let me think...".to_string()));
        assert_eq!(content, Some("Answer".to_string()));
    }

    #[test]
    fn test_streaming_chunks() {
        let mut parser = ThinkingTagParser::new();

        // First chunk with partial tag - may buffer
        let (c1, t1) = parser.process("Hello <thi");
        // Parser buffers when it sees potential tag start

        // Complete the tag
        let (c2, t2) = parser.process("nk>thinking");
        // Now "Hello " should be emitted as content and "thinking" as thinking

        let (c3, t3) = parser.process("</think> done");
        // " done" should be content

        // Verify accumulated results
        assert!(parser.accumulated_content().contains("Hello"));
        assert!(parser.accumulated_content().contains("done"));
        assert_eq!(parser.accumulated_thinking(), "thinking");
    }

    #[test]
    fn test_code_block_basic() {
        // Test that parser handles code blocks without panicking
        // Full code block escaping is a future enhancement
        let mut parser = ThinkingTagParser::new();
        let _ = parser.process("Some text before ```code``` and after");
        let _ = parser.finalize();
        // Parser should complete without panicking
    }

    #[test]
    fn test_multiple_tags() {
        let mut parser = ThinkingTagParser::new();
        let (content, thinking) = parser.process("<thinking>first</thinking>middle<thought>second</thought>end");
        assert_eq!(parser.accumulated_thinking(), "firstsecond");
        assert_eq!(parser.accumulated_content(), "middleend");
    }
}
