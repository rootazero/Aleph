//! Text Chunking for Channel Messages
//!
//! Provides message text splitting strategies for channels with length limits.

use serde::{Deserialize, Serialize};

/// Text chunking strategy
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChunkMode {
    Length { limit: usize },
    Newline { limit: usize },
}

impl Default for ChunkMode {
    fn default() -> Self {
        Self::Length { limit: 4000 }
    }
}

/// Text chunker trait
pub trait TextChunker: Send + Sync {
    fn chunk(&self, text: &str) -> Vec<String>;
    fn chunk_mode(&self) -> ChunkMode;
    fn max_chunk_size(&self) -> usize;
}

/// WhatsApp-specific text chunker
#[derive(Debug, Clone)]
pub struct WhatsAppChunker {
    mode: ChunkMode,
    max_size: usize,
}

impl Default for WhatsAppChunker {
    fn default() -> Self {
        Self {
            mode: ChunkMode::default(),
            max_size: 65536,
        }
    }
}

impl WhatsAppChunker {
    pub fn new(mode: ChunkMode, max_size: usize) -> Self {
        Self { mode, max_size }
    }
}

impl TextChunker for WhatsAppChunker {
    fn chunk(&self, text: &str) -> Vec<String> {
        if text.len() <= self.max_size {
            return vec![text.to_string()];
        }

        match &self.mode {
            ChunkMode::Length { limit } => Self::chunk_by_length(text, *limit),
            ChunkMode::Newline { limit } => Self::chunk_by_newline(text, *limit),
        }
    }

    fn chunk_mode(&self) -> ChunkMode {
        self.mode
    }

    fn max_chunk_size(&self) -> usize {
        self.max_size
    }
}

impl WhatsAppChunker {
    pub fn chunk_by_length(text: &str, limit: usize) -> Vec<String> {
        if text.len() <= limit {
            return vec![text.to_string()];
        }

        let mut chunks = Vec::new();
        let mut current = String::with_capacity(limit);

        for line in text.lines() {
            if current.len() + line.len() + 1 > limit && !current.is_empty() {
                chunks.push(current.clone());
                current.clear();
            }
            if !current.is_empty() {
                current.push('\n');
            }
            current.push_str(line);
        }

        if !current.is_empty() {
            chunks.push(current);
        }

        chunks
    }

    pub fn chunk_by_newline(text: &str, limit: usize) -> Vec<String> {
        let paragraphs: Vec<&str> = text.split("\n\n").collect();

        let mut chunks = Vec::new();
        let mut current = String::new();

        for para in paragraphs {
            if current.len() + para.len() + 2 <= limit {
                if !current.is_empty() {
                    current.push_str("\n\n");
                }
                current.push_str(para);
            } else {
                if !current.is_empty() {
                    chunks.push(current.clone());
                    current.clear();
                }
                if para.len() > limit {
                    chunks.extend(Self::chunk_by_length(para, limit));
                } else {
                    current.push_str(para);
                }
            }
        }

        if !current.is_empty() {
            chunks.push(current);
        }

        chunks
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunk_by_length_short() {
        let text = "Hello";
        let chunks = WhatsAppChunker::chunk_by_length(text, 4000);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "Hello");
    }

    #[test]
    fn test_chunk_by_length_multiple() {
        let text = "Line1\nLine2\nLine3\nLine4\nLine5";
        let chunks = WhatsAppChunker::chunk_by_length(text, 10);
        assert!(chunks.len() > 1);
    }

    #[test]
    fn test_chunk_by_newline_respects_limit() {
        let text = "Short paragraph.\n\nAnother paragraph that might need chunking.";
        let chunks = WhatsAppChunker::chunk_by_newline(text, 20);
        for chunk in &chunks {
            assert!(chunk.len() <= 20);
        }
    }

    #[test]
    fn test_single_chunk_under_limit() {
        let chunker = WhatsAppChunker::default();
        let text = "Short message";
        let chunks = chunker.chunk(text);
        assert_eq!(chunks.len(), 1);
    }
}
