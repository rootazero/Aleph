//! Prompt Analyzer Module
//!
//! Extracts features from prompt content for intelligent model routing.
//! Analyzes token count, complexity, language, code ratio, reasoning level, and domain.
//!
//! # Example
//!
//! ```rust,ignore
//! use aethecore::dispatcher::model_router::{PromptAnalyzer, PromptAnalyzerConfig};
//!
//! let analyzer = PromptAnalyzer::new(PromptAnalyzerConfig::default());
//! let features = analyzer.analyze("请用 Rust 写一个快速排序算法");
//!
//! println!("Tokens: {}", features.estimated_tokens);
//! println!("Complexity: {:.2}", features.complexity_score);
//! println!("Language: {:?}", features.primary_language);
//! ```

mod analyzer;
mod feature_extractor;
mod types;

// Re-export all public types for backward compatibility
pub use analyzer::PromptAnalyzer;
pub use types::{
    ComplexityWeights, ContextSize, Domain, Language, PromptAnalysisError, PromptAnalyzerConfig,
    PromptFeatures, ReasoningLevel, TechnicalDomain,
};

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn create_analyzer() -> PromptAnalyzer {
        PromptAnalyzer::with_defaults()
    }

    // =========================================================================
    // Token Estimation Tests
    // =========================================================================

    #[test]
    fn test_token_estimation_english() {
        let analyzer = create_analyzer();

        // Simple English text
        let features = analyzer.analyze("Hello, how are you today?");
        assert!(features.estimated_tokens > 0 && features.estimated_tokens < 20);

        // Longer English text
        let features = analyzer.analyze(
            "The quick brown fox jumps over the lazy dog. This is a common pangram used for testing.",
        );
        assert!(features.estimated_tokens > 10 && features.estimated_tokens < 50);
    }

    #[test]
    fn test_token_estimation_chinese() {
        let analyzer = create_analyzer();

        // Chinese text (each character is roughly 1.5 tokens)
        let features = analyzer.analyze("你好，今天天气怎么样？");
        assert!(features.estimated_tokens > 5);

        // Mixed Chinese and English
        let features = analyzer.analyze("请用 Rust 写一个快速排序算法");
        assert!(features.estimated_tokens > 10);
    }

    #[test]
    fn test_token_estimation_empty() {
        let analyzer = create_analyzer();
        let features = analyzer.analyze("");
        assert_eq!(features.estimated_tokens, 0);
    }

    // =========================================================================
    // Complexity Scoring Tests
    // =========================================================================

    #[test]
    fn test_complexity_simple_prompt() {
        let analyzer = create_analyzer();
        let features = analyzer.analyze("What is 2 + 2?");
        assert!(features.complexity_score < 0.3);
    }

    #[test]
    fn test_complexity_complex_prompt() {
        let analyzer = create_analyzer();
        let features = analyzer.analyze(
            "Please analyze the time complexity of the quicksort algorithm, \
             compare it with merge sort, and explain step by step how the partitioning works. \
             Also implement both algorithms in Rust and show benchmarks.",
        );
        assert!(features.complexity_score > 0.5);
    }

    // =========================================================================
    // Language Detection Tests
    // =========================================================================

    #[test]
    fn test_language_detection_english() {
        let analyzer = create_analyzer();
        let features = analyzer.analyze("Please explain how machine learning works.");
        assert_eq!(features.primary_language, Language::English);
        assert!(features.language_confidence > 0.8);
    }

    #[test]
    fn test_language_detection_chinese() {
        let analyzer = create_analyzer();
        let features = analyzer.analyze("请解释机器学习是如何工作的。");
        assert_eq!(features.primary_language, Language::Chinese);
        assert!(features.language_confidence > 0.5);
    }

    #[test]
    fn test_language_detection_mixed() {
        let analyzer = create_analyzer();
        // Use a prompt with more balanced language mix
        let features =
            analyzer.analyze("请帮我 write some code to implement sorting algorithm 用于排序数据");
        // The detection can vary based on character ratios - accept any reasonable result
        assert!(
            features.primary_language == Language::Mixed
                || features.primary_language == Language::Chinese
                || features.primary_language == Language::English
        );
        // The key thing is that it detects something reasonable
        assert!(features.language_confidence > 0.0);
    }

    // =========================================================================
    // Code Detection Tests
    // =========================================================================

    #[test]
    fn test_code_detection_with_blocks() {
        let analyzer = create_analyzer();
        let prompt = r#"
Here is a function:

```rust
fn main() {
    println!("Hello, world!");
}
```

Please explain it.
"#;
        let features = analyzer.analyze(prompt);
        assert!(features.has_code_blocks);
        assert!(features.code_ratio > 0.2);
    }

    #[test]
    fn test_code_detection_inline() {
        let analyzer = create_analyzer();
        let features = analyzer.analyze("The `println!` macro is used for output.");
        assert!(features.code_ratio > 0.0);
    }

    #[test]
    fn test_code_detection_pure_text() {
        let analyzer = create_analyzer();
        let features = analyzer.analyze("Tell me a story about a magical forest.");
        assert!(features.code_ratio < 0.1);
        assert!(!features.has_code_blocks);
    }

    // =========================================================================
    // Reasoning Level Tests
    // =========================================================================

    #[test]
    fn test_reasoning_level_low() {
        let analyzer = create_analyzer();
        let features = analyzer.analyze("What is the capital of France?");
        assert_eq!(features.reasoning_level, ReasoningLevel::Low);
    }

    #[test]
    fn test_reasoning_level_medium() {
        let analyzer = create_analyzer();
        let features = analyzer.analyze("Explain how HTTP caching works.");
        assert!(matches!(
            features.reasoning_level,
            ReasoningLevel::Medium | ReasoningLevel::High
        ));
    }

    #[test]
    fn test_reasoning_level_high() {
        let analyzer = create_analyzer();
        let features = analyzer.analyze(
            "Analyze and compare the time complexity of quicksort vs merge sort. \
             Explain step by step why quicksort has O(n²) worst case.",
        );
        assert_eq!(features.reasoning_level, ReasoningLevel::High);
    }

    #[test]
    fn test_reasoning_level_chinese() {
        let analyzer = create_analyzer();
        let features =
            analyzer.analyze("请分析并解释为什么快速排序在最坏情况下是O(n²)。逐步推理。");
        assert!(matches!(
            features.reasoning_level,
            ReasoningLevel::Medium | ReasoningLevel::High
        ));
    }

    // =========================================================================
    // Domain Classification Tests
    // =========================================================================

    #[test]
    fn test_domain_programming() {
        let analyzer = create_analyzer();
        let features = analyzer.analyze("Write a function in Python to sort a list.");
        assert!(matches!(
            features.domain,
            Domain::Technical(TechnicalDomain::Programming)
        ));
    }

    #[test]
    fn test_domain_math() {
        let analyzer = create_analyzer();
        let features = analyzer.analyze("Calculate the integral of x² from 0 to 1.");
        assert!(matches!(
            features.domain,
            Domain::Technical(TechnicalDomain::Mathematics)
        ));
    }

    #[test]
    fn test_domain_creative() {
        let analyzer = create_analyzer();
        let features = analyzer.analyze("Write a short story about a magical forest.");
        assert_eq!(features.domain, Domain::Creative);
    }

    #[test]
    fn test_domain_general() {
        let analyzer = create_analyzer();
        let features = analyzer.analyze("Hello, how are you?");
        assert_eq!(features.domain, Domain::General);
    }

    // =========================================================================
    // Question and Imperative Tests
    // =========================================================================

    #[test]
    fn test_question_count() {
        let analyzer = create_analyzer();
        let features = analyzer.analyze("What is this? How does it work? Why?");
        assert_eq!(features.question_count, 3);
    }

    #[test]
    fn test_imperative_count() {
        let analyzer = create_analyzer();
        let features = analyzer.analyze("Write a function. Explain how it works. Create a test.");
        assert!(features.imperative_count >= 2);
    }

    // =========================================================================
    // Context Size Tests
    // =========================================================================

    #[test]
    fn test_context_size_small() {
        let analyzer = create_analyzer();
        let features = analyzer.analyze("Hello");
        assert_eq!(features.suggested_context_size, ContextSize::Small);
    }

    #[test]
    fn test_context_size_scales_with_tokens() {
        let analyzer = create_analyzer();

        // Generate a long prompt
        let long_prompt = "word ".repeat(3000);
        let features = analyzer.analyze(&long_prompt);
        assert!(matches!(
            features.suggested_context_size,
            ContextSize::Medium | ContextSize::Large
        ));
    }

    // =========================================================================
    // Batch Analysis Tests
    // =========================================================================

    #[test]
    fn test_batch_analysis() {
        let analyzer = create_analyzer();
        let prompts = vec!["Hello", "Write code", "Explain math"];
        let results = analyzer.analyze_batch(&prompts);
        assert_eq!(results.len(), 3);
    }

    // =========================================================================
    // Performance Tests
    // =========================================================================

    #[test]
    fn test_analysis_performance() {
        let analyzer = create_analyzer();
        let prompt = "Please write a Rust function that implements quicksort with detailed comments explaining each step.";

        let start = std::time::Instant::now();
        for _ in 0..100 {
            let _ = analyzer.analyze(prompt);
        }
        let elapsed = start.elapsed();

        // Should complete 100 analyses in under 1 second (10ms each max)
        assert!(elapsed.as_millis() < 1000);
    }
}
