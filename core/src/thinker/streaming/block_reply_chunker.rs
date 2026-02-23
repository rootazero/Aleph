//! Block reply chunker for TTS and streaming output.
//!
//! Chunks streaming text into sentence-like blocks for:
//! - Text-to-speech processing
//! - Smooth UI updates
//! - Network efficiency
//!
//! Features fence-aware chunking to never break inside code blocks.
//! When forced to break inside a fence, closes and reopens it properly.

use std::collections::VecDeque;
use crate::markdown::fences::{parse_fence_spans, is_safe_fence_break, get_fence_split, FenceSpan};

/// Configuration for block chunking
#[derive(Debug, Clone)]
pub struct ChunkerConfig {
    /// Minimum characters before emitting a block
    pub min_block_size: usize,
    /// Maximum characters per block
    pub max_block_size: usize,
    /// Emit on sentence boundaries (. ! ?)
    pub emit_on_sentence: bool,
    /// Emit on paragraph breaks (\n\n)
    pub emit_on_paragraph: bool,
    /// Enable fence-aware chunking (avoids splitting inside code blocks)
    pub fence_aware: bool,
}

impl Default for ChunkerConfig {
    fn default() -> Self {
        Self {
            min_block_size: 20,
            max_block_size: 500,
            emit_on_sentence: true,
            emit_on_paragraph: true,
            fence_aware: true,
        }
    }
}

/// Configuration matching Moltbot defaults for channel streaming
impl ChunkerConfig {
    /// Create config with Moltbot-style defaults (800-1200 chars)
    pub fn moltbot_defaults() -> Self {
        Self {
            min_block_size: 800,
            max_block_size: 1200,
            emit_on_sentence: true,
            emit_on_paragraph: true,
            fence_aware: true,
        }
    }
}

/// Result of a chunk operation, potentially with fence handling
#[derive(Debug, Clone)]
pub struct ChunkResult {
    /// The chunk text (may include fence close/reopen markers)
    pub text: String,
    /// Whether this chunk was split inside a fence
    pub fence_split: bool,
}

/// Block reply chunker with fence awareness
#[derive(Debug, Clone)]
pub struct BlockReplyChunker {
    config: ChunkerConfig,
    buffer: String,
    emitted_blocks: VecDeque<String>,
}

impl Default for BlockReplyChunker {
    fn default() -> Self {
        Self::new(ChunkerConfig::default())
    }
}

impl BlockReplyChunker {
    /// Create a new chunker with configuration
    pub fn new(config: ChunkerConfig) -> Self {
        Self {
            config,
            buffer: String::new(),
            emitted_blocks: VecDeque::new(),
        }
    }

    /// Add text to the chunker, returns any complete blocks
    pub fn push(&mut self, text: &str) -> Vec<String> {
        self.buffer.push_str(text);
        self.try_emit_blocks()
    }

    /// Finalize and return any remaining content as the final block
    pub fn finalize(&mut self) -> Option<String> {
        if self.buffer.is_empty() {
            None
        } else {
            Some(std::mem::take(&mut self.buffer))
        }
    }

    /// Try to emit complete blocks
    fn try_emit_blocks(&mut self) -> Vec<String> {
        let mut blocks = Vec::new();

        loop {
            if self.buffer.len() < self.config.min_block_size {
                break;
            }

            // Parse fence spans for current buffer
            let spans = if self.config.fence_aware {
                parse_fence_spans(&self.buffer)
            } else {
                Vec::new()
            };

            let boundary = self.find_boundary(&spans);

            match boundary {
                Some(BreakPoint::Safe(pos)) => {
                    // Safe break outside fence
                    let block = self.buffer.drain(..=pos).collect::<String>();
                    blocks.push(block.trim().to_string());
                }
                Some(BreakPoint::FenceSplit { pos, close_line, reopen_line }) => {
                    // Breaking inside fence - need to close and reopen
                    let mut block: String = self.buffer.drain(..pos).collect();

                    // Append fence close to this chunk
                    if !block.ends_with('\n') {
                        block.push('\n');
                    }
                    block.push_str(&close_line);
                    blocks.push(block.trim().to_string());

                    // Prepend fence reopen to remaining buffer
                    let remainder = std::mem::take(&mut self.buffer);
                    self.buffer = format!("{}\n{}", reopen_line, remainder);
                }
                None => {
                    if self.buffer.len() >= self.config.max_block_size {
                        // Force split at max size, handling fences
                        let split_pos = self.config.max_block_size;

                        if self.config.fence_aware {
                            if let Some(split) = get_fence_split(&spans, split_pos) {
                                // Inside fence - do fence split
                                let mut block: String = self.buffer.drain(..split_pos).collect();
                                if !block.ends_with('\n') {
                                    block.push('\n');
                                }
                                block.push_str(&split.close_line);
                                blocks.push(block);

                                let remainder = std::mem::take(&mut self.buffer);
                                self.buffer = format!("{}\n{}", split.reopen_line, remainder);
                                continue;
                            }
                        }

                        // Safe to split directly
                        let block: String = self.buffer.drain(..split_pos).collect();
                        blocks.push(block.trim().to_string());
                    } else {
                        break;
                    }
                }
            }
        }

        // Filter out empty blocks
        blocks.into_iter().filter(|b| !b.is_empty()).collect()
    }

    /// Find the best boundary position for splitting
    fn find_boundary(&self, spans: &[FenceSpan]) -> Option<BreakPoint> {
        let search_range = self.buffer.len().min(self.config.max_block_size);
        let search_str = &self.buffer[..search_range];

        // Priority 1: Paragraph break (outside fences)
        if self.config.emit_on_paragraph {
            if let Some(pos) = self.find_paragraph_break(search_str, spans) {
                return Some(BreakPoint::Safe(pos));
            }
        }

        // Priority 2: Sentence ending (outside fences)
        if self.config.emit_on_sentence {
            if let Some(pos) = self.find_sentence_break(search_str, spans) {
                return Some(BreakPoint::Safe(pos));
            }
        }

        // Priority 3: Line break (outside fences)
        if let Some(pos) = self.find_newline_break(search_str, spans) {
            return Some(BreakPoint::Safe(pos));
        }

        // Priority 4: Space at word boundary (outside fences)
        if search_str.len() >= self.config.max_block_size {
            if let Some(pos) = self.find_word_break(search_str, spans) {
                return Some(BreakPoint::Safe(pos));
            }
        }

        // Priority 5: Force break at max_block_size (may require fence handling)
        if self.buffer.len() >= self.config.max_block_size && self.config.fence_aware {
            let pos = self.config.max_block_size;
            if let Some(split) = get_fence_split(spans, pos) {
                return Some(BreakPoint::FenceSplit {
                    pos,
                    close_line: split.close_line,
                    reopen_line: split.reopen_line,
                });
            }
        }

        None
    }

    /// Find paragraph break (\n\n) that's outside fences
    fn find_paragraph_break(&self, search_str: &str, spans: &[FenceSpan]) -> Option<usize> {
        let mut pos = search_str.len();
        while let Some(found) = search_str[..pos].rfind("\n\n") {
            if found >= self.config.min_block_size
                && (!self.config.fence_aware || is_safe_fence_break(spans, found + 1)) {
                    return Some(found + 1);
                }
            pos = found;
            if pos == 0 {
                break;
            }
        }
        None
    }

    /// Find sentence ending that's outside fences
    fn find_sentence_break(&self, search_str: &str, spans: &[FenceSpan]) -> Option<usize> {
        for (i, c) in search_str.char_indices().rev() {
            if i < self.config.min_block_size {
                break;
            }
            if matches!(c, '.' | '!' | '?') {
                // Check next char is space or end
                let is_sentence_end = if let Some(next) = search_str.chars().nth(i + 1) {
                    next.is_whitespace() || next == '\n'
                } else {
                    true
                };

                if is_sentence_end
                    && (!self.config.fence_aware || is_safe_fence_break(spans, i)) {
                        return Some(i);
                    }
            }
        }
        None
    }

    /// Find newline that's outside fences
    fn find_newline_break(&self, search_str: &str, spans: &[FenceSpan]) -> Option<usize> {
        let mut pos = search_str.len();
        while let Some(found) = search_str[..pos].rfind('\n') {
            if found >= self.config.min_block_size
                && (!self.config.fence_aware || is_safe_fence_break(spans, found)) {
                    return Some(found);
                }
            pos = found;
            if pos == 0 {
                break;
            }
        }
        None
    }

    /// Find word boundary (space) that's outside fences
    fn find_word_break(&self, search_str: &str, spans: &[FenceSpan]) -> Option<usize> {
        let mut pos = search_str.len();
        while let Some(found) = search_str[..pos].rfind(' ') {
            if found >= self.config.min_block_size
                && (!self.config.fence_aware || is_safe_fence_break(spans, found)) {
                    return Some(found);
                }
            pos = found;
            if pos == 0 {
                break;
            }
        }
        None
    }

    /// Get current buffer length
    pub fn buffer_len(&self) -> usize {
        self.buffer.len()
    }

    /// Check if buffer is empty
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    /// Clear the chunker
    pub fn clear(&mut self) {
        self.buffer.clear();
        self.emitted_blocks.clear();
    }
}

/// Internal break point representation
#[derive(Debug)]
enum BreakPoint {
    /// Safe break outside any fence
    Safe(usize),
    /// Break inside fence requiring close/reopen
    FenceSplit {
        pos: usize,
        close_line: String,
        reopen_line: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sentence_chunking() {
        let mut chunker = BlockReplyChunker::default();

        let blocks = chunker.push("Hello world. This is a test sentence. ");
        assert!(!blocks.is_empty());
    }

    #[test]
    fn test_streaming_chunks() {
        let mut chunker = BlockReplyChunker::new(ChunkerConfig {
            min_block_size: 10,
            max_block_size: 50,
            ..Default::default()
        });

        // Simulate streaming
        let b1 = chunker.push("Hello ");
        assert!(b1.is_empty()); // Too short

        let b2 = chunker.push("world. How are you doing today? ");
        assert!(!b2.is_empty()); // Should emit

        let _final_block = chunker.finalize();
        // Remaining content
    }

    #[test]
    fn test_paragraph_chunking() {
        let mut chunker = BlockReplyChunker::default();

        let blocks = chunker.push("First paragraph with enough text here.\n\nSecond paragraph.");
        // Should split on paragraph boundary
        assert!(!blocks.is_empty());
    }

    #[test]
    fn test_max_size_split() {
        let config = ChunkerConfig {
            min_block_size: 5,
            max_block_size: 20,
            emit_on_sentence: false,
            emit_on_paragraph: false,
            fence_aware: false, // Disable for this test
        };
        let mut chunker = BlockReplyChunker::new(config);

        let blocks = chunker.push("This is a very long string that exceeds the max size");
        assert!(!blocks.is_empty());
        for block in &blocks {
            assert!(block.len() <= 25); // Some slack for word boundaries
        }
    }

    #[test]
    fn test_empty_finalize() {
        let mut chunker = BlockReplyChunker::default();
        assert!(chunker.finalize().is_none());
    }

    // Fence-aware tests

    #[test]
    fn test_fence_aware_avoids_code_block() {
        let config = ChunkerConfig {
            min_block_size: 10,
            max_block_size: 50,
            fence_aware: true,
            ..Default::default()
        };
        let mut chunker = BlockReplyChunker::new(config);

        // Text with code block - should not split inside
        let text = "Here is code:\n```rust\nfn main() {}\n```\nAfter code.";
        let blocks = chunker.push(text);

        // Should not have broken inside the fence
        for block in &blocks {
            // If block contains opening fence, it should also contain closing
            if block.contains("```rust") && !block.ends_with("```") {
                // The block should contain the full fence or be properly closed
                assert!(
                    block.contains("\n```\n") || block.ends_with("```") || block.contains("```\n"),
                    "Block should not leave fence open: {}",
                    block
                );
            }
        }
    }

    #[test]
    fn test_fence_split_long_code() {
        let config = ChunkerConfig {
            min_block_size: 20,
            max_block_size: 60,
            fence_aware: true,
            emit_on_sentence: false,
            emit_on_paragraph: false,
        };
        let mut chunker = BlockReplyChunker::new(config);

        // Very long code block that must be split
        let text = "```rust\nfn very_long_function_name() {\n    let x = 1;\n    let y = 2;\n    let z = x + y;\n    println!(\"{}\", z);\n}\n```";
        let blocks = chunker.push(text);
        let final_block = chunker.finalize();

        // Collect all blocks
        let mut all_blocks = blocks;
        if let Some(fb) = final_block {
            all_blocks.push(fb);
        }

        // Each split chunk should have proper fence handling
        // First chunk should end with ```
        // Middle/last chunks should start with ```rust (or similar)
        if all_blocks.len() > 1 {
            // First block should close the fence
            assert!(
                all_blocks[0].trim_end().ends_with("```"),
                "First block should close fence: {}",
                all_blocks[0]
            );
            // Second block should reopen
            assert!(
                all_blocks[1].starts_with("```"),
                "Second block should reopen fence: {}",
                all_blocks[1]
            );
        }
    }

    #[test]
    fn test_no_fence_split_when_safe() {
        let config = ChunkerConfig {
            min_block_size: 10,
            max_block_size: 100,
            fence_aware: true,
            ..Default::default()
        };
        let mut chunker = BlockReplyChunker::new(config);

        // Text with code block that fits within max_block_size
        let text = "Start.\n```rust\nshort\n```\nEnd of text here with more content.";
        let blocks = chunker.push(text);

        // Should split on sentence/paragraph outside fence, not inside
        for block in &blocks {
            if block.contains("```rust") {
                assert!(
                    block.contains("\n```\n") || block.ends_with("```"),
                    "Fence should remain intact: {}",
                    block
                );
            }
        }
    }

    #[test]
    fn test_moltbot_defaults() {
        let config = ChunkerConfig::moltbot_defaults();
        assert_eq!(config.min_block_size, 800);
        assert_eq!(config.max_block_size, 1200);
        assert!(config.fence_aware);
    }

    #[test]
    fn test_multiple_fences() {
        let config = ChunkerConfig {
            min_block_size: 20,
            max_block_size: 150,
            fence_aware: true,
            ..Default::default()
        };
        let mut chunker = BlockReplyChunker::new(config);

        let text = "First block.\n```js\nconsole.log('a');\n```\nMiddle text with enough length.\n```python\nprint('b')\n```\nEnd.";
        let blocks = chunker.push(text);
        let _ = chunker.finalize();

        // Should handle multiple fences correctly
        assert!(!blocks.is_empty());
    }
}
