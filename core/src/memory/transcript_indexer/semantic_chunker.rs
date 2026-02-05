//! Semantic chunking based on embedding similarity

use std::sync::Arc;

use crate::memory::SmartEmbedder;
use crate::Result;

/// Configuration for semantic chunking
#[derive(Debug, Clone)]
pub struct SemanticChunkConfig {
    /// Similarity threshold for detecting boundaries (default: 0.85)
    pub similarity_threshold: f32,

    /// Minimum chunk size in tokens (default: 50)
    pub min_chunk_size: usize,

    /// Maximum chunk size in tokens (default: 400)
    pub max_chunk_size: usize,
}

impl Default for SemanticChunkConfig {
    fn default() -> Self {
        Self {
            similarity_threshold: 0.85,
            min_chunk_size: 50,
            max_chunk_size: 400,
        }
    }
}

/// Semantic chunker that uses embeddings to detect semantic boundaries
pub struct SemanticChunker {
    embedder: Arc<SmartEmbedder>,
    config: SemanticChunkConfig,
}

impl SemanticChunker {
    /// Create a new semantic chunker
    pub fn new(embedder: Arc<SmartEmbedder>, config: SemanticChunkConfig) -> Self {
        Self { embedder, config }
    }

    /// Chunk text based on semantic boundaries
    ///
    /// Uses embeddings to detect semantic shifts between sentences,
    /// creating chunks that preserve semantic coherence.
    pub async fn chunk(&self, text: &str) -> Result<Vec<String>> {
        // Split into sentences
        let sentences = self.split_sentences(text);

        if sentences.is_empty() {
            return Ok(Vec::new());
        }

        if sentences.len() == 1 {
            return Ok(vec![text.to_string()]);
        }

        // Generate embeddings for each sentence
        let embeddings = self.embed_sentences(&sentences).await?;

        // Detect semantic boundaries
        let boundaries = self.detect_boundaries(&embeddings);

        // Create chunks based on boundaries
        let chunks = self.create_chunks(&sentences, &boundaries);

        Ok(chunks)
    }

    /// Split text into sentences
    fn split_sentences(&self, text: &str) -> Vec<String> {
        // Simple sentence splitting based on punctuation
        let mut sentences = Vec::new();
        let mut current = String::new();

        for c in text.chars() {
            current.push(c);

            // End of sentence markers
            if matches!(c, '.' | '!' | '?' | '。' | '！' | '？') {
                let trimmed = current.trim();
                if !trimmed.is_empty() {
                    sentences.push(trimmed.to_string());
                    current.clear();
                }
            }
        }

        // Add remaining text
        let trimmed = current.trim();
        if !trimmed.is_empty() {
            sentences.push(trimmed.to_string());
        }

        sentences
    }

    /// Generate embeddings for sentences
    async fn embed_sentences(&self, sentences: &[String]) -> Result<Vec<Vec<f32>>> {
        let mut embeddings = Vec::new();

        for sentence in sentences {
            let embedding = self.embedder.embed(sentence).await?;
            embeddings.push(embedding);
        }

        Ok(embeddings)
    }

    /// Detect semantic boundaries based on embedding similarity
    fn detect_boundaries(&self, embeddings: &[Vec<f32>]) -> Vec<usize> {
        let mut boundaries = vec![0];

        for i in 1..embeddings.len() {
            let similarity = cosine_similarity(&embeddings[i - 1], &embeddings[i]);

            // Low similarity indicates a semantic boundary
            if similarity < self.config.similarity_threshold {
                boundaries.push(i);
            }
        }

        boundaries.push(embeddings.len());
        boundaries
    }

    /// Create chunks from sentences based on boundaries
    fn create_chunks(&self, sentences: &[String], boundaries: &[usize]) -> Vec<String> {
        let mut chunks = Vec::new();

        for i in 0..boundaries.len() - 1 {
            let start = boundaries[i];
            let end = boundaries[i + 1];

            // Combine sentences in this segment
            let chunk_sentences = &sentences[start..end];
            let chunk = chunk_sentences.join(" ");

            // Estimate token count (rough approximation)
            let token_count = self.estimate_tokens(&chunk);

            // Split large chunks
            if token_count > self.config.max_chunk_size {
                let sub_chunks = self.split_large_chunk(chunk_sentences);
                chunks.extend(sub_chunks);
            } else if token_count >= self.config.min_chunk_size {
                chunks.push(chunk);
            } else if !chunks.is_empty() {
                // Merge small chunks with previous chunk
                let last_idx = chunks.len() - 1;
                chunks[last_idx].push(' ');
                chunks[last_idx].push_str(&chunk);
            } else {
                // First chunk, keep even if small
                chunks.push(chunk);
            }
        }

        chunks
    }

    /// Split a large chunk into smaller chunks
    fn split_large_chunk(&self, sentences: &[String]) -> Vec<String> {
        let mut chunks = Vec::new();
        let mut current_chunk = String::new();
        let mut current_tokens = 0;

        for sentence in sentences {
            let sentence_tokens = self.estimate_tokens(sentence);

            if current_tokens + sentence_tokens > self.config.max_chunk_size && !current_chunk.is_empty() {
                chunks.push(current_chunk.clone());
                current_chunk.clear();
                current_tokens = 0;
            }

            if !current_chunk.is_empty() {
                current_chunk.push(' ');
            }
            current_chunk.push_str(sentence);
            current_tokens += sentence_tokens;
        }

        if !current_chunk.is_empty() {
            chunks.push(current_chunk);
        }

        chunks
    }

    /// Estimate token count (rough approximation: 1 token ≈ 4 characters)
    fn estimate_tokens(&self, text: &str) -> usize {
        text.len() / 4
    }
}

/// Calculate cosine similarity between two vectors
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }

    let dot_product: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }

    dot_product / (norm_a * norm_b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_sentences() {
        let config = SemanticChunkConfig::default();
        let embedder = Arc::new(SmartEmbedder::new(
            std::path::PathBuf::from("/tmp/models"),
            300,
        ));
        let chunker = SemanticChunker::new(embedder, config);

        let text = "This is sentence one. This is sentence two! Is this sentence three?";
        let sentences = chunker.split_sentences(text);

        assert_eq!(sentences.len(), 3);
        assert_eq!(sentences[0], "This is sentence one.");
        assert_eq!(sentences[1], "This is sentence two!");
        assert_eq!(sentences[2], "Is this sentence three?");
    }

    #[test]
    fn test_split_sentences_chinese() {
        let config = SemanticChunkConfig::default();
        let embedder = Arc::new(SmartEmbedder::new(
            std::path::PathBuf::from("/tmp/models"),
            300,
        ));
        let chunker = SemanticChunker::new(embedder, config);

        let text = "这是第一句。这是第二句！这是第三句？";
        let sentences = chunker.split_sentences(text);

        assert_eq!(sentences.len(), 3);
        assert!(sentences[0].contains("第一句"));
        assert!(sentences[1].contains("第二句"));
        assert!(sentences[2].contains("第三句"));
    }

    #[test]
    fn test_estimate_tokens() {
        let config = SemanticChunkConfig::default();
        let embedder = Arc::new(SmartEmbedder::new(
            std::path::PathBuf::from("/tmp/models"),
            300,
        ));
        let chunker = SemanticChunker::new(embedder, config);

        let text = "This is a test"; // 14 characters
        let tokens = chunker.estimate_tokens(text);
        assert_eq!(tokens, 3); // 14 / 4 = 3
    }

    #[test]
    fn test_cosine_similarity() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 0.001);

        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        assert!((cosine_similarity(&a, &b) - 0.0).abs() < 0.001);
    }
}
