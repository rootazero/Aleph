//! Value Estimator Module
//!
//! Provides importance scoring for memory entries to filter low-value content.

pub mod estimator;
pub mod signals;

pub use estimator::ValueEstimator;
pub use signals::{Signal, SignalDetector};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::{ContextAnchor, MemoryEntry};

    #[tokio::test]
    async fn test_value_estimation_high_value() {
        let estimator = ValueEstimator::new();

        // High-value: user preference
        let entry = MemoryEntry::new(
            uuid::Uuid::new_v4().to_string(),
            ContextAnchor::now("test".to_string(), "test".to_string()),
            "I prefer using Rust for systems programming".to_string(),
            "That's a great choice!".to_string(),
        );

        let score = estimator.estimate(&entry).await.unwrap();
        assert!(score > 0.7, "Expected high score, got {}", score);
    }

    #[tokio::test]
    async fn test_value_estimation_low_value() {
        let estimator = ValueEstimator::new();

        // Low-value: greeting
        let entry = MemoryEntry::new(
            uuid::Uuid::new_v4().to_string(),
            ContextAnchor::now("test".to_string(), "test".to_string()),
            "Hello".to_string(),
            "Hi there!".to_string(),
        );

        let score = estimator.estimate(&entry).await.unwrap();
        assert!(score < 0.3, "Expected low score, got {}", score);
    }

    #[tokio::test]
    async fn test_value_estimation_medium_value() {
        let estimator = ValueEstimator::new();

        // Medium-value: question and answer
        let entry = MemoryEntry::new(
            uuid::Uuid::new_v4().to_string(),
            ContextAnchor::now("test".to_string(), "test".to_string()),
            "What is the capital of France?".to_string(),
            "The capital of France is Paris.".to_string(),
        );

        let score = estimator.estimate(&entry).await.unwrap();
        assert!(score >= 0.3 && score <= 0.7, "Expected medium score, got {}", score);
    }

    #[test]
    fn test_signal_detection_preference() {
        let detector = SignalDetector::new();
        let text = "I prefer using Rust";
        let signals = detector.detect(text);

        assert!(signals.contains(&Signal::UserPreference));
    }

    #[test]
    fn test_signal_detection_greeting() {
        let detector = SignalDetector::new();
        let text = "Hello there!";
        let signals = detector.detect(text);

        assert!(signals.contains(&Signal::Greeting));
    }

    #[test]
    fn test_signal_detection_question() {
        let detector = SignalDetector::new();
        let text = "What is Rust?";
        let signals = detector.detect(text);

        assert!(signals.contains(&Signal::Question));
    }

    #[test]
    fn test_signal_detection_multiple() {
        let detector = SignalDetector::new();
        let text = "I prefer Rust. What do you think?";
        let signals = detector.detect(text);

        assert!(signals.contains(&Signal::UserPreference));
        assert!(signals.contains(&Signal::Question));
    }
}
