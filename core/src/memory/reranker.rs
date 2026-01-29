//! Reranker trait for extensible reranking support
//!
//! This module provides a trait for reranking retrieved memories. The default
//! implementation is a no-op, but users can implement custom rerankers (e.g.,
//! Cohere, cross-encoder models) for improved retrieval quality.

use crate::error::AetherError;
use async_trait::async_trait;

/// Result of reranking operation
#[derive(Debug, Clone)]
pub struct RerankResult<T> {
    /// Reranked items in order of relevance
    pub items: Vec<T>,
    /// Relevance scores for each item (optional)
    pub scores: Option<Vec<f32>>,
}

impl<T> RerankResult<T> {
    /// Create a new rerank result with items and optional scores
    pub fn new(items: Vec<T>, scores: Option<Vec<f32>>) -> Self {
        Self { items, scores }
    }

    /// Create a result without scores
    pub fn without_scores(items: Vec<T>) -> Self {
        Self {
            items,
            scores: None,
        }
    }
}

/// Trait for reranking retrieved items
///
/// Rerankers take a query and a list of retrieved items, and reorder them
/// based on semantic relevance. This is an extension point for integrating
/// external reranking services (Cohere, cross-encoder models, etc.).
#[async_trait]
pub trait Reranker: Send + Sync {
    /// The type of items being reranked
    type Item: Send + Sync;

    /// Rerank items based on query relevance
    ///
    /// # Arguments
    ///
    /// * `query` - The user's query string
    /// * `items` - Items to rerank
    /// * `top_k` - Maximum number of items to return
    ///
    /// # Returns
    ///
    /// Reranked items with optional relevance scores
    async fn rerank(
        &self,
        query: &str,
        items: Vec<Self::Item>,
        top_k: usize,
    ) -> Result<RerankResult<Self::Item>, AetherError>;

    /// Get the name of this reranker
    fn name(&self) -> &str;
}

/// No-op reranker that preserves original order
///
/// This is the default reranker used when no external reranking service
/// is configured. It simply returns items in their original order.
pub struct NoOpReranker<T> {
    _phantom: std::marker::PhantomData<T>,
}

impl<T> NoOpReranker<T> {
    /// Create a new no-op reranker
    pub fn new() -> Self {
        Self {
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<T> Default for NoOpReranker<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl<T: Send + Sync + 'static> Reranker for NoOpReranker<T> {
    type Item = T;

    async fn rerank(
        &self,
        _query: &str,
        items: Vec<Self::Item>,
        top_k: usize,
    ) -> Result<RerankResult<Self::Item>, AetherError> {
        let truncated = items.into_iter().take(top_k).collect();
        Ok(RerankResult::without_scores(truncated))
    }

    fn name(&self) -> &str {
        "no-op"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_noop_reranker() {
        let reranker: NoOpReranker<String> = NoOpReranker::new();

        let items = vec![
            "first".to_string(),
            "second".to_string(),
            "third".to_string(),
        ];

        let result = reranker.rerank("query", items.clone(), 10).await.unwrap();

        assert_eq!(result.items.len(), 3);
        assert_eq!(result.items[0], "first");
        assert_eq!(result.items[1], "second");
        assert_eq!(result.items[2], "third");
        assert!(result.scores.is_none());
    }

    #[tokio::test]
    async fn test_noop_reranker_truncation() {
        let reranker: NoOpReranker<i32> = NoOpReranker::new();

        let items = vec![1, 2, 3, 4, 5];

        let result = reranker.rerank("query", items, 3).await.unwrap();

        assert_eq!(result.items.len(), 3);
        assert_eq!(result.items, vec![1, 2, 3]);
    }

    #[test]
    fn test_reranker_name() {
        let reranker: NoOpReranker<String> = NoOpReranker::new();
        assert_eq!(reranker.name(), "no-op");
    }
}
