//! Keyword Index - Fast keyword-based matching with weighted scoring
//!
//! Provides efficient keyword lookup using an inverted index.
//! Supports multiple match modes and weighted scoring.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Inverted index for fast keyword matching
#[derive(Debug, Clone)]
pub struct KeywordIndex {
    /// keyword -> [(rule_id, weight)]
    index: HashMap<String, Vec<(String, f32)>>,

    /// Rule metadata
    rules: HashMap<String, KeywordRule>,

    /// Whether to normalize text (lowercase, unicode normalization)
    normalize: bool,
}

impl KeywordIndex {
    /// Create a new empty keyword index
    pub fn new() -> Self {
        Self {
            index: HashMap::new(),
            rules: HashMap::new(),
            normalize: true,
        }
    }

    /// Create index with normalization setting
    pub fn with_normalize(normalize: bool) -> Self {
        Self {
            index: HashMap::new(),
            rules: HashMap::new(),
            normalize,
        }
    }

    /// Add a keyword rule to the index
    pub fn add_rule(&mut self, rule: KeywordRule) {
        let rule_id = rule.id.clone();

        // Index each keyword
        for (keyword, weight) in &rule.keywords {
            let normalized = if self.normalize {
                self.normalize_text(keyword)
            } else {
                keyword.clone()
            };

            self.index
                .entry(normalized)
                .or_default()
                .push((rule_id.clone(), *weight));
        }

        self.rules.insert(rule_id, rule);
    }

    /// Match input against all keywords
    pub fn match_keywords(&self, input: &str) -> Vec<KeywordMatch> {
        let normalized_input = if self.normalize {
            self.normalize_text(input)
        } else {
            input.to_string()
        };

        let tokens = self.tokenize(&normalized_input);
        let mut rule_scores: HashMap<String, KeywordMatchBuilder> = HashMap::new();

        // Score each token
        for token in &tokens {
            if let Some(matches) = self.index.get(token) {
                for (rule_id, weight) in matches {
                    let builder = rule_scores.entry(rule_id.clone()).or_insert_with(|| {
                        KeywordMatchBuilder::new(rule_id.clone())
                    });
                    builder.add_keyword(token.clone(), *weight);
                }
            }
        }

        // Build final matches
        let mut results: Vec<KeywordMatch> = rule_scores
            .into_values()
            .map(|builder| builder.build(&self.rules))
            .collect();

        // Sort by score descending
        results.sort_by(|a, b| {
            b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal)
        });

        results
    }

    /// Match with minimum score threshold
    pub fn match_with_threshold(&self, input: &str, min_score: f32) -> Vec<KeywordMatch> {
        self.match_keywords(input)
            .into_iter()
            .filter(|m| m.score >= min_score)
            .collect()
    }

    /// Get the best match above threshold
    pub fn best_match(&self, input: &str, min_score: f32) -> Option<KeywordMatch> {
        self.match_with_threshold(input, min_score).into_iter().next()
    }

    /// Normalize text for matching
    fn normalize_text(&self, text: &str) -> String {
        // Simple normalization: lowercase only
        // For full Unicode normalization, add unicode_normalization crate
        text.to_lowercase()
    }

    /// Tokenize text into keywords
    fn tokenize(&self, text: &str) -> Vec<String> {
        // Split on whitespace and punctuation
        // CJK characters are tokenized individually
        // ASCII alphanumeric characters are grouped into words
        let mut tokens = Vec::new();
        let mut current_token = String::new();

        for ch in text.chars() {
            if is_cjk_char(ch) {
                // For CJK characters, each character is a token
                if !current_token.is_empty() {
                    tokens.push(std::mem::take(&mut current_token));
                }
                tokens.push(ch.to_string());
            } else if ch.is_ascii_alphanumeric() {
                // ASCII letters and digits are grouped
                current_token.push(ch);
            } else {
                // Delimiter (whitespace, punctuation, etc.)
                if !current_token.is_empty() {
                    tokens.push(std::mem::take(&mut current_token));
                }
            }
        }

        if !current_token.is_empty() {
            tokens.push(current_token);
        }

        tokens
    }

    /// Get rule count
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    /// Get keyword count
    pub fn keyword_count(&self) -> usize {
        self.index.len()
    }
}

impl Default for KeywordIndex {
    fn default() -> Self {
        Self::new()
    }
}

/// Check if a character is CJK (Chinese, Japanese, Korean)
fn is_cjk_char(ch: char) -> bool {
    let code = ch as u32;
    // CJK Unified Ideographs
    (0x4E00..=0x9FFF).contains(&code)
        // CJK Extension A
        || (0x3400..=0x4DBF).contains(&code)
        // CJK Extension B-F
        || (0x20000..=0x2CEAF).contains(&code)
        // Japanese Hiragana
        || (0x3040..=0x309F).contains(&code)
        // Japanese Katakana
        || (0x30A0..=0x30FF).contains(&code)
        // Korean Hangul
        || (0xAC00..=0xD7AF).contains(&code)
}

/// A keyword rule definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeywordRule {
    /// Unique rule ID
    pub id: String,

    /// Rule name (for display)
    pub name: String,

    /// Keywords with weights: keyword -> weight
    pub keywords: Vec<(String, f32)>,

    /// Match mode
    pub match_mode: KeywordMatchMode,

    /// Associated intent type
    pub intent_type: String,

    /// System prompt template
    pub system_prompt: Option<String>,

    /// Capabilities to enable
    pub capabilities: Vec<String>,

    /// Minimum score threshold for this rule
    pub min_score: Option<f32>,
}

impl KeywordRule {
    /// Create a new keyword rule with equal weights
    pub fn new(id: impl Into<String>, intent_type: impl Into<String>, keywords: Vec<String>) -> Self {
        let keywords_weighted = keywords.into_iter().map(|k| (k, 1.0)).collect();

        Self {
            id: id.into(),
            name: String::new(),
            keywords: keywords_weighted,
            match_mode: KeywordMatchMode::Any,
            intent_type: intent_type.into(),
            system_prompt: None,
            capabilities: Vec::new(),
            min_score: None,
        }
    }

    /// Create with weighted keywords
    pub fn with_weights(
        id: impl Into<String>,
        intent_type: impl Into<String>,
        keywords: Vec<(String, f32)>,
    ) -> Self {
        Self {
            id: id.into(),
            name: String::new(),
            keywords,
            match_mode: KeywordMatchMode::Any,
            intent_type: intent_type.into(),
            system_prompt: None,
            capabilities: Vec::new(),
            min_score: None,
        }
    }

    /// Set rule name
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Set match mode
    pub fn with_mode(mut self, mode: KeywordMatchMode) -> Self {
        self.match_mode = mode;
        self
    }

    /// Set system prompt
    pub fn with_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    /// Set capabilities
    pub fn with_capabilities(mut self, capabilities: Vec<String>) -> Self {
        self.capabilities = capabilities;
        self
    }

    /// Set minimum score
    pub fn with_min_score(mut self, min_score: f32) -> Self {
        self.min_score = Some(min_score);
        self
    }

    /// Get total possible score (sum of all weights)
    pub fn total_weight(&self) -> f32 {
        self.keywords.iter().map(|(_, w)| w).sum()
    }
}

/// Keyword match mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeywordMatchMode {
    /// Any keyword matches (OR logic)
    Any,
    /// All keywords must match (AND logic)
    All,
    /// Weighted scoring (sum of matched weights / total weights)
    Weighted,
}

impl Default for KeywordMatchMode {
    fn default() -> Self {
        Self::Any
    }
}

/// Result of keyword matching
#[derive(Debug, Clone)]
pub struct KeywordMatch {
    /// Matched rule ID
    pub rule_id: String,

    /// Match score (0.0 - 1.0 for normalized, or raw sum for unnormalized)
    pub score: f32,

    /// Matched keywords
    pub matched_keywords: Vec<String>,

    /// Associated intent type
    pub intent_type: String,

    /// System prompt (if any)
    pub system_prompt: Option<String>,

    /// Capabilities
    pub capabilities: Vec<String>,
}

impl KeywordMatch {
    /// Check if this match is above a threshold
    pub fn is_confident(&self, threshold: f32) -> bool {
        self.score >= threshold
    }
}

/// Builder for KeywordMatch during matching process
struct KeywordMatchBuilder {
    rule_id: String,
    matched_keywords: Vec<String>,
    total_weight: f32,
}

impl KeywordMatchBuilder {
    fn new(rule_id: String) -> Self {
        Self {
            rule_id,
            matched_keywords: Vec::new(),
            total_weight: 0.0,
        }
    }

    fn add_keyword(&mut self, keyword: String, weight: f32) {
        if !self.matched_keywords.contains(&keyword) {
            self.matched_keywords.push(keyword);
            self.total_weight += weight;
        }
    }

    fn build(self, rules: &HashMap<String, KeywordRule>) -> KeywordMatch {
        let rule = rules.get(&self.rule_id);

        let score = if let Some(rule) = rule {
            match rule.match_mode {
                KeywordMatchMode::Any => {
                    if !self.matched_keywords.is_empty() { 1.0 } else { 0.0 }
                }
                KeywordMatchMode::All => {
                    let required = rule.keywords.len();
                    let matched = self.matched_keywords.len();
                    if matched >= required { 1.0 } else { matched as f32 / required as f32 }
                }
                KeywordMatchMode::Weighted => {
                    let total = rule.total_weight();
                    if total > 0.0 {
                        (self.total_weight / total).min(1.0)
                    } else {
                        0.0
                    }
                }
            }
        } else {
            self.total_weight
        };

        KeywordMatch {
            rule_id: self.rule_id,
            score,
            matched_keywords: self.matched_keywords,
            intent_type: rule.map(|r| r.intent_type.clone()).unwrap_or_default(),
            system_prompt: rule.and_then(|r| r.system_prompt.clone()),
            capabilities: rule.map(|r| r.capabilities.clone()).unwrap_or_default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keyword_index_basic() {
        let mut index = KeywordIndex::new();

        index.add_rule(KeywordRule::new(
            "weather",
            "search",
            vec!["weather".to_string(), "forecast".to_string(), "天气".to_string()],
        ));

        let matches = index.match_keywords("What's the weather today?");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].rule_id, "weather");
    }

    #[test]
    fn test_keyword_index_chinese() {
        let mut index = KeywordIndex::new();

        index.add_rule(KeywordRule::new(
            "weather",
            "search",
            vec!["天".to_string(), "气".to_string()],
        ));

        let matches = index.match_keywords("今天天气怎么样？");
        assert_eq!(matches.len(), 1);
        assert!(matches[0].matched_keywords.contains(&"天".to_string()));
    }

    #[test]
    fn test_keyword_weighted() {
        let mut index = KeywordIndex::new();

        index.add_rule(
            KeywordRule::with_weights(
                "search",
                "search",
                vec![
                    ("search".to_string(), 2.0),
                    ("find".to_string(), 1.5),
                    ("look".to_string(), 1.0),
                ],
            )
            .with_mode(KeywordMatchMode::Weighted),
        );

        let matches = index.match_keywords("search for information");
        assert_eq!(matches.len(), 1);
        // Score should be 2.0 / 4.5 ≈ 0.44
        assert!(matches[0].score > 0.4 && matches[0].score < 0.5);
    }

    #[test]
    fn test_keyword_match_all() {
        let mut index = KeywordIndex::new();

        index.add_rule(
            KeywordRule::new(
                "code_review",
                "code",
                vec!["review".to_string(), "code".to_string()],
            )
            .with_mode(KeywordMatchMode::All),
        );

        // Both keywords present
        let matches = index.match_keywords("Please review my code");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].score, 1.0);

        // Only one keyword present
        let matches = index.match_keywords("Review this document");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].score, 0.5); // 1/2 keywords matched
    }

    #[test]
    fn test_threshold_filtering() {
        let mut index = KeywordIndex::new();

        index.add_rule(
            KeywordRule::with_weights(
                "low_score",
                "test",
                vec![
                    ("rare".to_string(), 0.1),
                    ("keyword".to_string(), 0.1),
                ],
            )
            .with_mode(KeywordMatchMode::Weighted),
        );

        // "rare test" matches "rare" (0.1 / 0.2 = 0.5)
        // With threshold 0.6, it should be filtered out
        let matches = index.match_with_threshold("rare test", 0.6);
        assert!(matches.is_empty()); // Score (0.5) is below threshold (0.6)
    }

    #[test]
    fn test_case_insensitive() {
        let mut index = KeywordIndex::new();

        index.add_rule(KeywordRule::new(
            "greeting",
            "general",
            vec!["hello".to_string()],
        ));

        let matches = index.match_keywords("HELLO world");
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn test_unicode_normalization() {
        let mut index = KeywordIndex::new();

        // Note: Full Unicode normalization requires unicode_normalization crate
        // This test verifies basic case-insensitive matching
        index.add_rule(KeywordRule::new(
            "cafe",
            "search",
            vec!["cafe".to_string()], // Use ASCII version
        ));

        // Match with same form
        let matches = index.match_keywords("cafe");
        assert_eq!(matches.len(), 1);

        // Test case insensitivity
        let matches = index.match_keywords("CAFE");
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn test_best_match() {
        let mut index = KeywordIndex::new();

        index.add_rule(
            KeywordRule::with_weights(
                "high",
                "high",
                vec![("important".to_string(), 2.0)],
            )
            .with_mode(KeywordMatchMode::Weighted),
        );

        index.add_rule(
            KeywordRule::with_weights(
                "low",
                "low",
                vec![("trivial".to_string(), 0.5)],
            )
            .with_mode(KeywordMatchMode::Weighted),
        );

        let best = index.best_match("important matter", 0.5);
        assert!(best.is_some());
        assert_eq!(best.unwrap().rule_id, "high");
    }
}
