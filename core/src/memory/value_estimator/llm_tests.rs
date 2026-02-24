//! Integration tests for LLM-based scoring

use std::sync::Arc;

use crate::memory::{ContextAnchor, MemoryEntry, ValueEstimator};
use crate::providers::AiProvider;
use crate::Result;

/// Mock provider that returns predictable scores
struct MockScoringProvider {
    score: f32,
}

impl MockScoringProvider {
    fn new(score: f32) -> Self {
        Self { score }
    }
}

impl AiProvider for MockScoringProvider {
    fn process(
        &self,
        _input: &str,
        _system_prompt: Option<&str>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String>> + Send + '_>> {
        let score = self.score;
        Box::pin(async move { Ok(format!("{}", score)) })
    }

    fn process_with_image(
        &self,
        _input: &str,
        _image: Option<&crate::clipboard::ImageData>,
        _system_prompt: Option<&str>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String>> + Send + '_>> {
        let score = self.score;
        Box::pin(async move { Ok(format!("{}", score)) })
    }

    fn name(&self) -> &str {
        "mock_scoring"
    }

    fn color(&self) -> &str {
        "#000000"
    }
}

#[tokio::test]
async fn test_llm_scoring_high_value() {
    let provider = Arc::new(MockScoringProvider::new(0.9));
    let estimator = ValueEstimator::with_llm(provider);

    let entry = MemoryEntry::new(
        uuid::Uuid::new_v4().to_string(),
        ContextAnchor::now("test".to_string(), "test".to_string()),
        "I prefer using Rust for systems programming".to_string(),
        "That's a great choice!".to_string(),
    );

    let score = estimator.estimate(&entry).await.unwrap();

    // Should be high (LLM returned 0.9, weighted with keyword score)
    assert!(score > 0.7, "Expected high score, got {}", score);
}

#[tokio::test]
async fn test_llm_scoring_low_value() {
    let provider = Arc::new(MockScoringProvider::new(0.1));
    let estimator = ValueEstimator::with_llm(provider);

    let entry = MemoryEntry::new(
        uuid::Uuid::new_v4().to_string(),
        ContextAnchor::now("test".to_string(), "test".to_string()),
        "Hello".to_string(),
        "Hi there!".to_string(),
    );

    let score = estimator.estimate(&entry).await.unwrap();

    // Should be low (LLM returned 0.1, weighted with keyword score)
    assert!(score < 0.4, "Expected low score, got {}", score);
}

#[tokio::test]
async fn test_llm_scoring_hybrid() {
    // LLM gives medium score, but keywords detect high value
    let provider = Arc::new(MockScoringProvider::new(0.5));
    let estimator = ValueEstimator::with_llm(provider);

    let entry = MemoryEntry::new(
        uuid::Uuid::new_v4().to_string(),
        ContextAnchor::now("test".to_string(), "test".to_string()),
        "I prefer using Rust".to_string(), // Keyword: preference
        "Good choice!".to_string(),
    );

    let score = estimator.estimate(&entry).await.unwrap();

    // Should be weighted average of LLM (0.5) and keyword (high)
    // 0.5 * 0.7 + keyword * 0.3
    assert!(score > 0.4 && score < 0.8, "Expected hybrid score, got {}", score);
}

#[tokio::test]
async fn test_keyword_only_scoring() {
    // No LLM provider, should use keyword-based scoring only
    let estimator = ValueEstimator::new();

    let entry = MemoryEntry::new(
        uuid::Uuid::new_v4().to_string(),
        ContextAnchor::now("test".to_string(), "test".to_string()),
        "I prefer using Rust".to_string(),
        "Good choice!".to_string(),
    );

    let score = estimator.estimate(&entry).await.unwrap();

    // Should use keyword-based scoring
    assert!(score > 0.6, "Expected high keyword score, got {}", score);
}

#[tokio::test]
async fn test_batch_scoring_with_llm() {
    let provider = Arc::new(MockScoringProvider::new(0.8));
    let estimator = ValueEstimator::with_llm(provider);

    let entries = vec![
        MemoryEntry::new(
            uuid::Uuid::new_v4().to_string(),
            ContextAnchor::now("test".to_string(), "test".to_string()),
            "Entry 1".to_string(),
            "Response 1".to_string(),
        ),
        MemoryEntry::new(
            uuid::Uuid::new_v4().to_string(),
            ContextAnchor::now("test".to_string(), "test".to_string()),
            "Entry 2".to_string(),
            "Response 2".to_string(),
        ),
    ];

    let scores = estimator.estimate_batch(&entries).await.unwrap();

    assert_eq!(scores.len(), 2);
    for score in scores {
        assert!((0.0..=1.0).contains(&score));
    }
}
