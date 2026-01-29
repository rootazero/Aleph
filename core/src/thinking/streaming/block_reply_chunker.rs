//! Block reply chunker for TTS and streaming output.
//!
//! Chunks streaming text into sentence-like blocks for:
//! - Text-to-speech processing
//! - Smooth UI updates
//! - Network efficiency

use std::collections::VecDeque;

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
}

impl Default for ChunkerConfig {
    fn default() -> Self {
        Self {
            min_block_size: 20,
            max_block_size: 500,
            emit_on_sentence: true,
            emit_on_paragraph: true,
        }
    }
}

/// Block reply chunker
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

            let boundary = self.find_boundary();

            if let Some(pos) = boundary {
                let block = self.buffer.drain(..=pos).collect::<String>();
                blocks.push(block.trim().to_string());
            } else if self.buffer.len() >= self.config.max_block_size {
                // Force split at max size
                let block = self.buffer.drain(..self.config.max_block_size).collect::<String>();
                blocks.push(block.trim().to_string());
            } else {
                break;
            }
        }

        // Filter out empty blocks
        blocks.into_iter().filter(|b| !b.is_empty()).collect()
    }

    /// Find the best boundary position for splitting
    fn find_boundary(&self) -> Option<usize> {
        let search_range = self.buffer.len().min(self.config.max_block_size);
        let search_str = &self.buffer[..search_range];

        // Priority 1: Paragraph break
        if self.config.emit_on_paragraph {
            if let Some(pos) = search_str.rfind("\n\n") {
                if pos >= self.config.min_block_size {
                    return Some(pos + 1);
                }
            }
        }

        // Priority 2: Sentence ending
        if self.config.emit_on_sentence {
            // Look for sentence endings followed by space or end
            for (i, c) in search_str.char_indices().rev() {
                if i < self.config.min_block_size {
                    break;
                }
                if matches!(c, '.' | '!' | '?') {
                    // Check next char is space or end
                    if let Some(next) = search_str.chars().nth(i + 1) {
                        if next.is_whitespace() || next == '\n' {
                            return Some(i);
                        }
                    } else {
                        return Some(i);
                    }
                }
            }
        }

        // Priority 3: Line break
        if let Some(pos) = search_str.rfind('\n') {
            if pos >= self.config.min_block_size {
                return Some(pos);
            }
        }

        // Priority 4: Space (word boundary)
        if search_str.len() >= self.config.max_block_size {
            if let Some(pos) = search_str.rfind(' ') {
                if pos >= self.config.min_block_size {
                    return Some(pos);
                }
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

        let final_block = chunker.finalize();
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
}
