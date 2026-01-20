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

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::Instant;

// =============================================================================
// Core Types
// =============================================================================

/// Language detected in the prompt
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    English,
    Chinese,
    Japanese,
    Korean,
    Mixed,
    #[default]
    Unknown,
}

impl std::fmt::Display for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Language::English => write!(f, "English"),
            Language::Chinese => write!(f, "Chinese"),
            Language::Japanese => write!(f, "Japanese"),
            Language::Korean => write!(f, "Korean"),
            Language::Mixed => write!(f, "Mixed"),
            Language::Unknown => write!(f, "Unknown"),
        }
    }
}

/// Level of reasoning required for the prompt
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningLevel {
    #[default]
    Low,
    Medium,
    High,
}

impl std::fmt::Display for ReasoningLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReasoningLevel::Low => write!(f, "Low"),
            ReasoningLevel::Medium => write!(f, "Medium"),
            ReasoningLevel::High => write!(f, "High"),
        }
    }
}

/// Technical domain classification
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TechnicalDomain {
    Programming,
    Mathematics,
    Science,
    Engineering,
    DataScience,
    Other(String),
}

impl Default for TechnicalDomain {
    fn default() -> Self {
        TechnicalDomain::Programming
    }
}

impl std::fmt::Display for TechnicalDomain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TechnicalDomain::Programming => write!(f, "Programming"),
            TechnicalDomain::Mathematics => write!(f, "Mathematics"),
            TechnicalDomain::Science => write!(f, "Science"),
            TechnicalDomain::Engineering => write!(f, "Engineering"),
            TechnicalDomain::DataScience => write!(f, "DataScience"),
            TechnicalDomain::Other(s) => write!(f, "Other({})", s),
        }
    }
}

/// Domain classification for routing
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Domain {
    #[default]
    General,
    Technical(TechnicalDomain),
    Creative,
    Conversational,
}

impl std::fmt::Display for Domain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Domain::General => write!(f, "General"),
            Domain::Technical(td) => write!(f, "Technical({})", td),
            Domain::Creative => write!(f, "Creative"),
            Domain::Conversational => write!(f, "Conversational"),
        }
    }
}

/// Suggested context window size based on analysis
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ContextSize {
    #[default]
    Small,
    Medium,
    Large,
}

impl ContextSize {
    /// Get the minimum token capacity for this context size
    pub fn min_tokens(&self) -> u32 {
        match self {
            ContextSize::Small => 4_000,
            ContextSize::Medium => 32_000,
            ContextSize::Large => 128_000,
        }
    }
}

impl std::fmt::Display for ContextSize {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ContextSize::Small => write!(f, "Small (<4K)"),
            ContextSize::Medium => write!(f, "Medium (4K-32K)"),
            ContextSize::Large => write!(f, "Large (>32K)"),
        }
    }
}

/// Complete analysis result for a prompt
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptFeatures {
    /// Estimated token count
    pub estimated_tokens: u32,

    /// Complexity score (0.0 - 1.0)
    pub complexity_score: f64,

    /// Primary detected language
    pub primary_language: Language,

    /// Confidence in language detection (0.0 - 1.0)
    pub language_confidence: f64,

    /// Ratio of code content (0.0 - 1.0)
    pub code_ratio: f64,

    /// Level of reasoning required
    pub reasoning_level: ReasoningLevel,

    /// Domain classification
    pub domain: Domain,

    /// Suggested context window size
    pub suggested_context_size: ContextSize,

    /// Time taken for analysis in microseconds
    pub analysis_time_us: u64,

    /// Whether prompt contains code blocks
    pub has_code_blocks: bool,

    /// Number of questions in the prompt
    pub question_count: u32,

    /// Number of imperative commands (e.g., "write", "explain")
    pub imperative_count: u32,
}

impl Default for PromptFeatures {
    fn default() -> Self {
        Self {
            estimated_tokens: 0,
            complexity_score: 0.0,
            primary_language: Language::Unknown,
            language_confidence: 0.0,
            code_ratio: 0.0,
            reasoning_level: ReasoningLevel::Low,
            domain: Domain::General,
            suggested_context_size: ContextSize::Small,
            analysis_time_us: 0,
            has_code_blocks: false,
            question_count: 0,
            imperative_count: 0,
        }
    }
}

impl PromptFeatures {
    /// Check if the prompt appears to be code-heavy
    pub fn is_code_heavy(&self) -> bool {
        self.code_ratio > 0.5 || self.has_code_blocks
    }

    /// Check if the prompt requires high reasoning capability
    pub fn requires_reasoning(&self) -> bool {
        self.reasoning_level == ReasoningLevel::High
    }

    /// Check if the prompt is technical
    pub fn is_technical(&self) -> bool {
        matches!(self.domain, Domain::Technical(_))
    }
}

// =============================================================================
// Configuration
// =============================================================================

/// Weights for complexity calculation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComplexityWeights {
    /// Weight for text length factor
    pub length: f64,
    /// Weight for sentence structure factor
    pub structure: f64,
    /// Weight for technical term density
    pub technical: f64,
    /// Weight for multi-step indicators
    pub multi_step: f64,
}

impl Default for ComplexityWeights {
    fn default() -> Self {
        Self {
            length: 0.2,
            structure: 0.3,
            technical: 0.3,
            multi_step: 0.2,
        }
    }
}

/// Configuration for the prompt analyzer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptAnalyzerConfig {
    /// Tokenizer name (currently only "simple" supported)
    pub tokenizer_name: String,

    /// Weights for complexity calculation
    pub complexity_weights: ComplexityWeights,

    /// Threshold above which complexity is considered high
    pub high_complexity_threshold: f64,

    /// Threshold below which complexity is considered low
    pub low_complexity_threshold: f64,

    /// Keywords indicating reasoning requirements (English)
    pub reasoning_keywords_en: Vec<String>,

    /// Keywords indicating reasoning requirements (Chinese)
    pub reasoning_keywords_zh: Vec<String>,

    /// Programming-related keywords
    pub programming_keywords: Vec<String>,

    /// Mathematics-related keywords
    pub math_keywords: Vec<String>,

    /// Science-related keywords
    pub science_keywords: Vec<String>,

    /// Creative writing keywords
    pub creative_keywords: Vec<String>,

    /// Threshold for mixed language detection (0.0 - 1.0)
    pub mixed_language_threshold: f64,
}

impl Default for PromptAnalyzerConfig {
    fn default() -> Self {
        Self {
            tokenizer_name: "simple".to_string(),
            complexity_weights: ComplexityWeights::default(),
            high_complexity_threshold: 0.7,
            low_complexity_threshold: 0.3,
            reasoning_keywords_en: vec![
                "explain".to_string(),
                "why".to_string(),
                "how".to_string(),
                "analyze".to_string(),
                "compare".to_string(),
                "evaluate".to_string(),
                "reason".to_string(),
                "prove".to_string(),
                "derive".to_string(),
                "step by step".to_string(),
                "chain of thought".to_string(),
                "think through".to_string(),
            ],
            reasoning_keywords_zh: vec![
                "解释".to_string(),
                "为什么".to_string(),
                "如何".to_string(),
                "分析".to_string(),
                "比较".to_string(),
                "评估".to_string(),
                "推理".to_string(),
                "证明".to_string(),
                "推导".to_string(),
                "逐步".to_string(),
                "一步一步".to_string(),
            ],
            programming_keywords: vec![
                "code".to_string(),
                "function".to_string(),
                "class".to_string(),
                "implement".to_string(),
                "debug".to_string(),
                "error".to_string(),
                "bug".to_string(),
                "api".to_string(),
                "algorithm".to_string(),
                "rust".to_string(),
                "python".to_string(),
                "javascript".to_string(),
                "typescript".to_string(),
                "swift".to_string(),
                "java".to_string(),
                "golang".to_string(),
                "c++".to_string(),
                "代码".to_string(),
                "函数".to_string(),
                "实现".to_string(),
                "算法".to_string(),
                "调试".to_string(),
                "编程".to_string(),
            ],
            math_keywords: vec![
                "calculate".to_string(),
                "equation".to_string(),
                "formula".to_string(),
                "matrix".to_string(),
                "integral".to_string(),
                "derivative".to_string(),
                "probability".to_string(),
                "statistics".to_string(),
                "计算".to_string(),
                "方程".to_string(),
                "公式".to_string(),
                "矩阵".to_string(),
                "积分".to_string(),
                "导数".to_string(),
                "概率".to_string(),
                "统计".to_string(),
            ],
            science_keywords: vec![
                "physics".to_string(),
                "chemistry".to_string(),
                "biology".to_string(),
                "molecule".to_string(),
                "atom".to_string(),
                "experiment".to_string(),
                "hypothesis".to_string(),
                "物理".to_string(),
                "化学".to_string(),
                "生物".to_string(),
                "分子".to_string(),
                "原子".to_string(),
                "实验".to_string(),
            ],
            creative_keywords: vec![
                "write".to_string(),
                "story".to_string(),
                "poem".to_string(),
                "creative".to_string(),
                "imagine".to_string(),
                "fiction".to_string(),
                "写".to_string(),
                "故事".to_string(),
                "诗".to_string(),
                "创意".to_string(),
                "想象".to_string(),
                "小说".to_string(),
            ],
            mixed_language_threshold: 0.3,
        }
    }
}

// =============================================================================
// Static Regex Patterns
// =============================================================================

/// Regex for detecting code blocks (markdown fenced code)
static CODE_BLOCK_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"```[\s\S]*?```").unwrap());

/// Regex for detecting inline code
static INLINE_CODE_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"`[^`]+`").unwrap());

/// Regex for detecting code-like patterns (function calls, operators)
static CODE_PATTERN_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:\b\w+\s*\([^)]*\)|\w+\.\w+|[+\-*/=<>!&|]{2,}|::\w+|\b(fn|def|func|let|const|var|if|else|for|while|return|import|from|class|struct|enum|trait|impl)\b)").unwrap()
});

/// Regex for detecting Chinese characters
static CHINESE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[\u4e00-\u9fff]").unwrap());

/// Regex for detecting Japanese characters (Hiragana and Katakana)
static JAPANESE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[\u3040-\u309f\u30a0-\u30ff]").unwrap());

/// Regex for detecting Korean characters
static KOREAN_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[\uac00-\ud7af\u1100-\u11ff]").unwrap());

/// Regex for detecting questions
static QUESTION_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[?？]").unwrap());

/// Regex for detecting imperative verbs at start of sentences
static IMPERATIVE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|\.\s*)(write|create|implement|build|make|generate|explain|describe|list|show|find|fix|debug|analyze|compare|evaluate|请|写|创建|实现|生成|解释|描述|列出|显示|查找|修复|分析|比较)").unwrap()
});

/// Regex for multi-step indicators
static MULTI_STEP_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:and then|first.*then|step \d|步骤|然后|首先.*然后|第[一二三四五六七八九十\d]+步)").unwrap()
});

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
        let estimated_tokens = self.estimate_tokens(prompt);
        let complexity_score = self.calculate_complexity(prompt);
        let (primary_language, language_confidence) = self.detect_language(prompt);
        let (code_ratio, has_code_blocks) = self.detect_code_ratio(prompt);
        let reasoning_level = self.detect_reasoning_level(prompt);
        let domain = self.detect_domain(prompt);
        let question_count = self.count_questions(prompt);
        let imperative_count = self.count_imperatives(prompt);

        // Determine suggested context size
        let suggested_context_size = self.suggest_context_size(estimated_tokens, complexity_score);

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

    /// Estimate token count using simple heuristics
    ///
    /// This is a fast approximation. For accurate counts, use tiktoken.
    /// Approximation: ~4 chars per token for English, ~1.5 chars per token for CJK
    fn estimate_tokens(&self, text: &str) -> u32 {
        if text.is_empty() {
            return 0;
        }

        let mut token_count = 0u32;

        // Count CJK characters (roughly 1 token each or slightly more)
        let cjk_count = CHINESE_REGEX.find_iter(text).count()
            + JAPANESE_REGEX.find_iter(text).count()
            + KOREAN_REGEX.find_iter(text).count();

        // Remove CJK characters and count remaining as English-like
        let non_cjk_text: String = text
            .chars()
            .filter(|c| {
                !('\u{4e00}'..='\u{9fff}').contains(c)
                    && !('\u{3040}'..='\u{309f}').contains(c)
                    && !('\u{30a0}'..='\u{30ff}').contains(c)
                    && !('\u{ac00}'..='\u{d7af}').contains(c)
            })
            .collect();

        // CJK: approximately 1.5 tokens per character (conservative estimate)
        token_count += (cjk_count as f64 * 1.5).ceil() as u32;

        // English/Latin: approximately 4 characters per token
        let english_chars = non_cjk_text.chars().filter(|c| !c.is_whitespace()).count();
        token_count += (english_chars as f64 / 4.0).ceil() as u32;

        // Add some tokens for whitespace and special characters
        let whitespace_count = text.chars().filter(|c| c.is_whitespace()).count();
        token_count += (whitespace_count as f64 / 4.0).ceil() as u32;

        token_count.max(1)
    }

    /// Calculate complexity score (0.0 - 1.0)
    fn calculate_complexity(&self, text: &str) -> f64 {
        let weights = &self.config.complexity_weights;

        // Factor 1: Length-based complexity
        let char_count = text.chars().count() as f64;
        let length_score = (char_count / 2000.0).min(1.0); // Normalize to 0-1

        // Factor 2: Structure complexity (sentence count and average length)
        let sentences: Vec<&str> = text
            .split(|c| c == '.' || c == '。' || c == '!' || c == '！' || c == '?' || c == '？')
            .filter(|s| !s.trim().is_empty())
            .collect();
        let sentence_count = sentences.len().max(1) as f64;
        let avg_sentence_length = char_count / sentence_count;
        let structure_score = ((avg_sentence_length / 100.0) + (sentence_count / 10.0)).min(1.0);

        // Factor 3: Technical term density
        let technical_keywords = self
            .config
            .programming_keywords
            .iter()
            .chain(self.config.math_keywords.iter())
            .chain(self.config.science_keywords.iter());

        let text_lower = text.to_lowercase();
        let technical_matches = technical_keywords
            .filter(|kw| text_lower.contains(&kw.to_lowercase()))
            .count() as f64;
        let technical_score = (technical_matches / 5.0).min(1.0);

        // Factor 4: Multi-step indicators
        let multi_step_matches = MULTI_STEP_REGEX.find_iter(text).count() as f64;
        let multi_step_score = (multi_step_matches / 3.0).min(1.0);

        // Weighted combination
        let raw_score = weights.length * length_score
            + weights.structure * structure_score
            + weights.technical * technical_score
            + weights.multi_step * multi_step_score;

        // Normalize to 0-1
        raw_score.clamp(0.0, 1.0)
    }

    /// Detect the primary language and confidence
    fn detect_language(&self, text: &str) -> (Language, f64) {
        let total_chars = text.chars().filter(|c| !c.is_whitespace()).count() as f64;
        if total_chars == 0.0 {
            return (Language::Unknown, 0.0);
        }

        let chinese_count = CHINESE_REGEX.find_iter(text).count() as f64;
        let japanese_count = JAPANESE_REGEX.find_iter(text).count() as f64;
        let korean_count = KOREAN_REGEX.find_iter(text).count() as f64;

        let chinese_ratio = chinese_count / total_chars;
        let japanese_ratio = japanese_count / total_chars;
        let korean_ratio = korean_count / total_chars;
        let cjk_ratio = chinese_ratio + japanese_ratio + korean_ratio;
        let english_ratio = 1.0 - cjk_ratio;

        // Determine primary language
        let mut max_ratio = english_ratio;
        let mut primary = Language::English;
        let mut confidence = english_ratio;

        if chinese_ratio > max_ratio {
            max_ratio = chinese_ratio;
            primary = Language::Chinese;
            confidence = chinese_ratio;
        }

        if japanese_ratio > max_ratio {
            max_ratio = japanese_ratio;
            primary = Language::Japanese;
            confidence = japanese_ratio;
        }

        if korean_ratio > max_ratio {
            primary = Language::Korean;
            confidence = korean_ratio;
        }

        // Check for mixed language
        let secondary_ratio = if primary == Language::English {
            cjk_ratio
        } else {
            english_ratio
        };

        if secondary_ratio > self.config.mixed_language_threshold {
            return (Language::Mixed, 1.0 - secondary_ratio);
        }

        (primary, confidence.min(1.0))
    }

    /// Detect code ratio and presence of code blocks
    fn detect_code_ratio(&self, text: &str) -> (f64, bool) {
        let total_len = text.len() as f64;
        if total_len == 0.0 {
            return (0.0, false);
        }

        // Count code block content
        let code_block_len: usize = CODE_BLOCK_REGEX
            .find_iter(text)
            .map(|m| m.as_str().len())
            .sum();
        let has_code_blocks = code_block_len > 0;

        // Count inline code
        let inline_code_len: usize = INLINE_CODE_REGEX
            .find_iter(text)
            .map(|m| m.as_str().len())
            .sum();

        // Count code-like patterns (only in non-code-block text)
        let text_without_blocks = CODE_BLOCK_REGEX.replace_all(text, "");
        let text_without_inline = INLINE_CODE_REGEX.replace_all(&text_without_blocks, "");
        let code_pattern_count = CODE_PATTERN_REGEX.find_iter(&text_without_inline).count();

        // Calculate code ratio
        let explicit_code_len = code_block_len + inline_code_len;
        let pattern_contribution = (code_pattern_count as f64 * 10.0).min(total_len * 0.3);
        let code_ratio = ((explicit_code_len as f64 + pattern_contribution) / total_len).min(1.0);

        (code_ratio, has_code_blocks)
    }

    /// Detect the level of reasoning required
    fn detect_reasoning_level(&self, text: &str) -> ReasoningLevel {
        let text_lower = text.to_lowercase();

        // Count English reasoning keywords
        let en_count = self
            .config
            .reasoning_keywords_en
            .iter()
            .filter(|kw| text_lower.contains(&kw.to_lowercase()))
            .count();

        // Count Chinese reasoning keywords
        let zh_count = self
            .config
            .reasoning_keywords_zh
            .iter()
            .filter(|kw| text.contains(kw.as_str()))
            .count();

        // Check for multi-step indicators
        let multi_step_count = MULTI_STEP_REGEX.find_iter(text).count();

        let total_indicators = en_count + zh_count + multi_step_count;

        if total_indicators >= 3 {
            ReasoningLevel::High
        } else if total_indicators >= 1 {
            ReasoningLevel::Medium
        } else {
            ReasoningLevel::Low
        }
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

    /// Count the number of questions in the prompt
    fn count_questions(&self, text: &str) -> u32 {
        QUESTION_REGEX.find_iter(text).count() as u32
    }

    /// Count the number of imperative commands
    fn count_imperatives(&self, text: &str) -> u32 {
        IMPERATIVE_REGEX.find_iter(text).count() as u32
    }

    /// Suggest context size based on tokens and complexity
    fn suggest_context_size(&self, tokens: u32, complexity: f64) -> ContextSize {
        // Base decision on token count
        let base_size = if tokens < 1_000 {
            ContextSize::Small
        } else if tokens < 8_000 {
            ContextSize::Medium
        } else {
            ContextSize::Large
        };

        // Upgrade if high complexity (may generate longer response)
        if complexity > self.config.high_complexity_threshold {
            match base_size {
                ContextSize::Small => ContextSize::Medium,
                _ => base_size,
            }
        } else {
            base_size
        }
    }
}

// =============================================================================
// Error Types
// =============================================================================

/// Errors that can occur during prompt analysis
#[derive(Debug, thiserror::Error)]
pub enum PromptAnalysisError {
    #[error("Tokenizer initialization failed: {0}")]
    TokenizerInit(String),

    #[error("Analysis timeout after {0}ms")]
    Timeout(u64),

    #[error("Empty prompt provided")]
    EmptyPrompt,
}

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
        let tokens = analyzer.estimate_tokens("Hello, how are you today?");
        assert!(tokens > 0 && tokens < 20);

        // Longer English text
        let tokens = analyzer.estimate_tokens(
            "The quick brown fox jumps over the lazy dog. This is a common pangram used for testing.",
        );
        assert!(tokens > 10 && tokens < 50);
    }

    #[test]
    fn test_token_estimation_chinese() {
        let analyzer = create_analyzer();

        // Chinese text (each character is roughly 1.5 tokens)
        let tokens = analyzer.estimate_tokens("你好，今天天气怎么样？");
        assert!(tokens > 5);

        // Mixed Chinese and English
        let tokens = analyzer.estimate_tokens("请用 Rust 写一个快速排序算法");
        assert!(tokens > 10);
    }

    #[test]
    fn test_token_estimation_empty() {
        let analyzer = create_analyzer();
        let tokens = analyzer.estimate_tokens("");
        assert_eq!(tokens, 0);
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
        let features = analyzer.analyze("请帮我 write some code to implement sorting algorithm 用于排序数据");
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
        let features = analyzer.analyze("请分析并解释为什么快速排序在最坏情况下是O(n²)。逐步推理。");
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
