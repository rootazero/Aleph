//! Core types for prompt analysis
//!
//! Contains PromptAnalysis, PromptFeatures, and configuration types.

use serde::{Deserialize, Serialize};

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
#[derive(Default)]
pub enum TechnicalDomain {
    #[default]
    Programming,
    Mathematics,
    Science,
    Engineering,
    DataScience,
    Other(String),
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
