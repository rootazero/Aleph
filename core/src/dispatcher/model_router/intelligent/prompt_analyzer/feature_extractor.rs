//! Feature extraction logic for prompt analysis
//!
//! Contains regex patterns and extraction utilities for tokens, code, language, etc.

use regex::Regex;
use std::sync::LazyLock;

use super::types::{ContextSize, Language, PromptAnalyzerConfig, ReasoningLevel};

// =============================================================================
// Static Regex Patterns
// =============================================================================

/// Regex for detecting code blocks (markdown fenced code)
pub static CODE_BLOCK_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"```[\s\S]*?```").unwrap());

/// Regex for detecting inline code
pub static INLINE_CODE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"`[^`]+`").unwrap());

/// Regex for detecting code-like patterns (function calls, operators)
pub static CODE_PATTERN_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:\b\w+\s*\([^)]*\)|\w+\.\w+|[+\-*/=<>!&|]{2,}|::\w+|\b(fn|def|func|let|const|var|if|else|for|while|return|import|from|class|struct|enum|trait|impl)\b)").unwrap()
});

/// Regex for detecting Chinese characters
pub static CHINESE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[\u4e00-\u9fff]").unwrap());

/// Regex for detecting Japanese characters (Hiragana and Katakana)
pub static JAPANESE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[\u3040-\u309f\u30a0-\u30ff]").unwrap());

/// Regex for detecting Korean characters
pub static KOREAN_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[\uac00-\ud7af\u1100-\u11ff]").unwrap());

/// Regex for detecting questions
pub static QUESTION_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[?？]").unwrap());

/// Regex for detecting imperative verbs at start of sentences
pub static IMPERATIVE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|\.\s*)(write|create|implement|build|make|generate|explain|describe|list|show|find|fix|debug|analyze|compare|evaluate|请|写|创建|实现|生成|解释|描述|列出|显示|查找|修复|分析|比较)").unwrap()
});

/// Regex for multi-step indicators
pub static MULTI_STEP_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:and then|first.*then|step \d|步骤|然后|首先.*然后|第[一二三四五六七八九十\d]+步)",
    )
    .unwrap()
});

// =============================================================================
// Feature Extraction Functions
// =============================================================================

/// Estimate token count using simple heuristics
///
/// This is a fast approximation. For accurate counts, use tiktoken.
/// Approximation: ~4 chars per token for English, ~1.5 chars per token for CJK
pub fn estimate_tokens(text: &str) -> u32 {
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
pub fn calculate_complexity(text: &str, config: &PromptAnalyzerConfig) -> f64 {
    let weights = &config.complexity_weights;

    // Factor 1: Length-based complexity
    let char_count = text.chars().count() as f64;
    let length_score = (char_count / 2000.0).min(1.0); // Normalize to 0-1

    // Factor 2: Structure complexity (sentence count and average length)
    let sentences: Vec<&str> = text
        .split(['.', '。', '!', '！', '?', '？'])
        .filter(|s| !s.trim().is_empty())
        .collect();
    let sentence_count = sentences.len().max(1) as f64;
    let avg_sentence_length = char_count / sentence_count;
    let structure_score = ((avg_sentence_length / 100.0) + (sentence_count / 10.0)).min(1.0);

    // Factor 3: Technical term density
    let technical_keywords = config
        .programming_keywords
        .iter()
        .chain(config.math_keywords.iter())
        .chain(config.science_keywords.iter());

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
pub fn detect_language(text: &str, config: &PromptAnalyzerConfig) -> (Language, f64) {
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

    if secondary_ratio > config.mixed_language_threshold {
        return (Language::Mixed, 1.0 - secondary_ratio);
    }

    (primary, confidence.min(1.0))
}

/// Detect code ratio and presence of code blocks
pub fn detect_code_ratio(text: &str) -> (f64, bool) {
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
pub fn detect_reasoning_level(text: &str, config: &PromptAnalyzerConfig) -> ReasoningLevel {
    let text_lower = text.to_lowercase();

    // Count English reasoning keywords
    let en_count = config
        .reasoning_keywords_en
        .iter()
        .filter(|kw| text_lower.contains(&kw.to_lowercase()))
        .count();

    // Count Chinese reasoning keywords
    let zh_count = config
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

/// Count the number of questions in the prompt
pub fn count_questions(text: &str) -> u32 {
    QUESTION_REGEX.find_iter(text).count() as u32
}

/// Count the number of imperative commands
pub fn count_imperatives(text: &str) -> u32 {
    IMPERATIVE_REGEX.find_iter(text).count() as u32
}

/// Suggest context size based on tokens and complexity
pub fn suggest_context_size(
    tokens: u32,
    complexity: f64,
    config: &PromptAnalyzerConfig,
) -> ContextSize {
    // Base decision on token count
    let base_size = if tokens < 1_000 {
        ContextSize::Small
    } else if tokens < 8_000 {
        ContextSize::Medium
    } else {
        ContextSize::Large
    };

    // Upgrade if high complexity (may generate longer response)
    if complexity > config.high_complexity_threshold {
        match base_size {
            ContextSize::Small => ContextSize::Medium,
            _ => base_size,
        }
    } else {
        base_size
    }
}
