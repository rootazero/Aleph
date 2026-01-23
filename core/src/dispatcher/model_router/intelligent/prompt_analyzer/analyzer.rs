//! Main analyzer implementation
//!
//! Contains PromptAnalyzer with domain detection and complexity scoring.

use std::collections::HashMap;
use std::time::Instant;

use super::feature_extractor::{
    calculate_complexity, count_imperatives, count_questions, detect_code_ratio, detect_language,
    detect_reasoning_level, estimate_tokens, suggest_context_size, CODE_BLOCK_REGEX,
};
use super::types::{Domain, PromptAnalyzerConfig, PromptFeatures, TechnicalDomain};

// =============================================================================
// Prompt Analyzer
// =============================================================================

/// Prompt analyzer for extracting features from prompt text
pub struct PromptAnalyzer {
    config: PromptAnalyzerConfig,
}

impl PromptAnalyzer {
    /// Create a new prompt analyzer with the given configuration
    pub fn new(config: PromptAnalyzerConfig) -> Self {
        Self { config }
    }

    /// Create a new prompt analyzer with default configuration
    pub fn with_defaults() -> Self {
        Self::new(PromptAnalyzerConfig::default())
    }

    /// Analyze a prompt and extract features
    pub fn analyze(&self, prompt: &str) -> PromptFeatures {
        let start = Instant::now();

        if prompt.is_empty() {
            return PromptFeatures {
                analysis_time_us: start.elapsed().as_micros() as u64,
                ..Default::default()
            };
        }

        // Extract all features
        let estimated_tokens = estimate_tokens(prompt);
        let complexity_score = calculate_complexity(prompt, &self.config);
        let (primary_language, language_confidence) = detect_language(prompt, &self.config);
        let (code_ratio, has_code_blocks) = detect_code_ratio(prompt);
        let reasoning_level = detect_reasoning_level(prompt, &self.config);
        let domain = self.detect_domain(prompt);
        let question_count = count_questions(prompt);
        let imperative_count = count_imperatives(prompt);

        // Determine suggested context size
        let suggested_context_size =
            suggest_context_size(estimated_tokens, complexity_score, &self.config);

        let analysis_time_us = start.elapsed().as_micros() as u64;

        PromptFeatures {
            estimated_tokens,
            complexity_score,
            primary_language,
            language_confidence,
            code_ratio,
            reasoning_level,
            domain,
            suggested_context_size,
            analysis_time_us,
            has_code_blocks,
            question_count,
            imperative_count,
        }
    }

    /// Analyze multiple prompts in batch
    pub fn analyze_batch(&self, prompts: &[&str]) -> Vec<PromptFeatures> {
        prompts.iter().map(|p| self.analyze(p)).collect()
    }

    /// Detect the domain of the prompt
    fn detect_domain(&self, text: &str) -> Domain {
        let text_lower = text.to_lowercase();

        // Count domain-specific keywords
        let mut domain_scores: HashMap<&str, usize> = HashMap::new();

        let programming_count = self
            .config
            .programming_keywords
            .iter()
            .filter(|kw| text_lower.contains(&kw.to_lowercase()))
            .count();
        domain_scores.insert("programming", programming_count);

        let math_count = self
            .config
            .math_keywords
            .iter()
            .filter(|kw| text_lower.contains(&kw.to_lowercase()))
            .count();
        domain_scores.insert("math", math_count);

        let science_count = self
            .config
            .science_keywords
            .iter()
            .filter(|kw| text_lower.contains(&kw.to_lowercase()))
            .count();
        domain_scores.insert("science", science_count);

        let creative_count = self
            .config
            .creative_keywords
            .iter()
            .filter(|kw| text_lower.contains(&kw.to_lowercase()))
            .count();
        domain_scores.insert("creative", creative_count);

        // Check for code blocks (strong indicator of programming)
        if CODE_BLOCK_REGEX.is_match(text) {
            *domain_scores.entry("programming").or_insert(0) += 5;
        }

        // Find the domain with the highest score
        let (top_domain, top_score) = domain_scores
            .iter()
            .max_by_key(|(_, &score)| score)
            .map(|(d, s)| (*d, *s))
            .unwrap_or(("general", 0));

        if top_score == 0 {
            return Domain::General;
        }

        match top_domain {
            "programming" => Domain::Technical(TechnicalDomain::Programming),
            "math" => Domain::Technical(TechnicalDomain::Mathematics),
            "science" => Domain::Technical(TechnicalDomain::Science),
            "creative" => Domain::Creative,
            _ => Domain::General,
        }
    }
}
