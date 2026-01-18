//! Weighted keyword matching module with CJK support.
//!
//! This module provides a flexible keyword index for intent classification
//! that supports:
//! - Weighted keyword scoring
//! - CJK character tokenization (each CJK char is a token)
//! - Multiple match modes (Any, All, Weighted)
//! - Case-insensitive matching

use std::collections::HashMap;

/// Match mode for keyword rules
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeywordMatchMode {
    /// Match if any keyword is found (OR logic)
    Any,
    /// Match only if all keywords are found (AND logic)
    All,
    /// Use weighted scoring - sum weights of matched keywords
    Weighted,
}

/// A keyword matching rule with weights and intent mapping
#[derive(Debug, Clone)]
pub struct KeywordRule {
    /// Unique identifier for this rule
    pub id: String,
    /// Keywords with their weights (keyword -> weight)
    pub keywords: HashMap<String, f32>,
    /// How to combine keyword matches
    pub match_mode: KeywordMatchMode,
    /// The intent type this rule maps to
    pub intent_type: String,
    /// Minimum score required for this rule to match
    pub min_score: f32,
}

impl KeywordRule {
    /// Create a new keyword rule
    pub fn new(id: impl Into<String>, intent_type: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            keywords: HashMap::new(),
            match_mode: KeywordMatchMode::Weighted,
            intent_type: intent_type.into(),
            min_score: 0.0,
        }
    }

    /// Add a keyword with weight
    pub fn with_keyword(mut self, keyword: impl Into<String>, weight: f32) -> Self {
        self.keywords.insert(keyword.into().to_lowercase(), weight);
        self
    }

    /// Add multiple keywords with the same weight
    pub fn with_keywords(mut self, keywords: &[&str], weight: f32) -> Self {
        for keyword in keywords {
            self.keywords.insert(keyword.to_lowercase(), weight);
        }
        self
    }

    /// Set the match mode
    pub fn with_match_mode(mut self, mode: KeywordMatchMode) -> Self {
        self.match_mode = mode;
        self
    }

    /// Set the minimum score threshold
    pub fn with_min_score(mut self, min_score: f32) -> Self {
        self.min_score = min_score;
        self
    }
}

/// Result of a keyword match
#[derive(Debug, Clone)]
pub struct KeywordMatch {
    /// ID of the matched rule
    pub rule_id: String,
    /// Calculated match score
    pub score: f32,
    /// Keywords that were matched
    pub matched_keywords: Vec<String>,
    /// The intent type from the matched rule
    pub intent_type: String,
}

/// Inverted index for efficient keyword matching
#[derive(Debug, Default)]
pub struct KeywordIndex {
    /// Maps keyword -> list of (rule_id, weight)
    inverted_index: HashMap<String, Vec<(String, f32)>>,
    /// Stores all rules by ID
    rules: HashMap<String, KeywordRule>,
}

impl KeywordIndex {
    /// Create a new empty keyword index
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a rule to the index
    pub fn add_rule(&mut self, rule: KeywordRule) {
        // Add to inverted index
        for (keyword, weight) in &rule.keywords {
            self.inverted_index
                .entry(keyword.clone())
                .or_default()
                .push((rule.id.clone(), *weight));
        }
        // Store the rule
        self.rules.insert(rule.id.clone(), rule);
    }

    /// Check if a character is a CJK character
    pub fn is_cjk_char(c: char) -> bool {
        // CJK Unified Ideographs: U+4E00 - U+9FFF
        // CJK Unified Ideographs Extension A: U+3400 - U+4DBF
        // CJK Unified Ideographs Extension B-F: U+20000 - U+2FA1F
        // CJK Compatibility Ideographs: U+F900 - U+FAFF
        // CJK Radicals Supplement: U+2E80 - U+2EFF
        // Kangxi Radicals: U+2F00 - U+2FDF
        matches!(c,
            '\u{4E00}'..='\u{9FFF}' |
            '\u{3400}'..='\u{4DBF}' |
            '\u{F900}'..='\u{FAFF}' |
            '\u{2E80}'..='\u{2EFF}' |
            '\u{2F00}'..='\u{2FDF}' |
            '\u{20000}'..='\u{2FA1F}'
        )
    }

    /// Tokenize text, handling CJK characters specially
    /// Each CJK character becomes its own token, while non-CJK words
    /// are split by whitespace/punctuation
    pub fn tokenize(text: &str) -> Vec<String> {
        let text = text.to_lowercase();
        let mut tokens = Vec::new();
        let mut current_word = String::new();

        for c in text.chars() {
            if Self::is_cjk_char(c) {
                // Flush current word if any
                if !current_word.is_empty() {
                    tokens.push(std::mem::take(&mut current_word));
                }
                // CJK character is its own token
                tokens.push(c.to_string());
            } else if c.is_alphanumeric() {
                current_word.push(c);
            } else {
                // Whitespace or punctuation: flush current word
                if !current_word.is_empty() {
                    tokens.push(std::mem::take(&mut current_word));
                }
            }
        }

        // Flush remaining word
        if !current_word.is_empty() {
            tokens.push(current_word);
        }

        tokens
    }

    /// Match keywords against input text, returning all matching rules
    #[must_use]
    pub fn match_keywords(&self, text: &str) -> Vec<KeywordMatch> {
        let tokens: Vec<String> = Self::tokenize(text);
        let token_set: std::collections::HashSet<&str> =
            tokens.iter().map(|s| s.as_str()).collect();

        // Track scores per rule
        let mut rule_scores: HashMap<String, (f32, Vec<String>)> = HashMap::new();

        // Check each token against inverted index
        for token in &token_set {
            if let Some(rules) = self.inverted_index.get(*token) {
                for (rule_id, weight) in rules {
                    let entry = rule_scores.entry(rule_id.clone()).or_default();
                    entry.0 += weight;
                    entry.1.push(token.to_string());
                }
            }
        }

        // Build matches based on match mode
        let mut matches = Vec::new();
        for (rule_id, (score, matched_keywords)) in rule_scores {
            if let Some(rule) = self.rules.get(&rule_id) {
                let should_match = match rule.match_mode {
                    KeywordMatchMode::Any => !matched_keywords.is_empty(),
                    KeywordMatchMode::All => matched_keywords.len() == rule.keywords.len(),
                    KeywordMatchMode::Weighted => score >= rule.min_score,
                };

                if should_match {
                    matches.push(KeywordMatch {
                        rule_id: rule_id.clone(),
                        score,
                        matched_keywords,
                        intent_type: rule.intent_type.clone(),
                    });
                }
            }
        }

        // Sort by score descending
        matches.sort_by(|a, b| b.score.total_cmp(&a.score));
        matches
    }

    /// Get the best match above a threshold
    #[must_use]
    pub fn best_match(&self, text: &str, threshold: f32) -> Option<KeywordMatch> {
        self.match_keywords(text)
            .into_iter()
            .find(|m| m.score >= threshold)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_cjk_char() {
        // Common Chinese characters
        assert!(KeywordIndex::is_cjk_char('你'));
        assert!(KeywordIndex::is_cjk_char('好'));
        assert!(KeywordIndex::is_cjk_char('中'));
        assert!(KeywordIndex::is_cjk_char('文'));

        // Japanese Kanji (same range)
        assert!(KeywordIndex::is_cjk_char('日'));
        assert!(KeywordIndex::is_cjk_char('本'));

        // Non-CJK
        assert!(!KeywordIndex::is_cjk_char('a'));
        assert!(!KeywordIndex::is_cjk_char('Z'));
        assert!(!KeywordIndex::is_cjk_char('1'));
        assert!(!KeywordIndex::is_cjk_char(' '));
        assert!(!KeywordIndex::is_cjk_char('あ')); // Hiragana is not CJK ideograph
        assert!(!KeywordIndex::is_cjk_char('ア')); // Katakana is not CJK ideograph
    }

    #[test]
    fn test_tokenize_english() {
        let tokens = KeywordIndex::tokenize("Hello World");
        assert_eq!(tokens, vec!["hello", "world"]);
    }

    #[test]
    fn test_tokenize_chinese() {
        let tokens = KeywordIndex::tokenize("你好世界");
        assert_eq!(tokens, vec!["你", "好", "世", "界"]);
    }

    #[test]
    fn test_tokenize_mixed() {
        let tokens = KeywordIndex::tokenize("Hello 你好 World");
        assert_eq!(tokens, vec!["hello", "你", "好", "world"]);
    }

    #[test]
    fn test_tokenize_with_punctuation() {
        let tokens = KeywordIndex::tokenize("Hello, World! 你好。");
        assert_eq!(tokens, vec!["hello", "world", "你", "好"]);
    }

    #[test]
    fn test_tokenize_case_insensitive() {
        let tokens = KeywordIndex::tokenize("HELLO World");
        assert_eq!(tokens, vec!["hello", "world"]);
    }

    #[test]
    fn test_basic_keyword_matching() {
        let mut index = KeywordIndex::new();

        let rule = KeywordRule::new("search", "web_search")
            .with_keywords(&["search", "find", "look"], 1.0)
            .with_match_mode(KeywordMatchMode::Any);

        index.add_rule(rule);

        let matches = index.match_keywords("I want to search for something");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].rule_id, "search");
        assert_eq!(matches[0].intent_type, "web_search");
        assert!(matches[0].matched_keywords.contains(&"search".to_string()));
    }

    #[test]
    fn test_weighted_scoring() {
        let mut index = KeywordIndex::new();

        // Note: CJK characters are tokenized individually, so use single chars as keywords
        let rule = KeywordRule::new("file_organize", "organize")
            .with_keyword("整", 1.5) // High weight
            .with_keyword("理", 0.5)
            .with_keyword("文", 0.5)
            .with_keyword("件", 0.3)
            .with_match_mode(KeywordMatchMode::Weighted)
            .with_min_score(1.5);

        index.add_rule(rule);

        // Should match - 整(1.5) + 理(0.5) + 文(0.5) + 件(0.3) = 2.8
        let matches = index.match_keywords("整理文件夹");
        assert!(!matches.is_empty());

        // Score should be at least 2.0
        assert!(matches[0].score >= 2.0);
    }

    #[test]
    fn test_all_match_mode() {
        let mut index = KeywordIndex::new();

        let rule = KeywordRule::new("transfer", "file_transfer")
            .with_keyword("move", 1.0)
            .with_keyword("files", 1.0)
            .with_match_mode(KeywordMatchMode::All);

        index.add_rule(rule);

        // Should not match - only one keyword
        let matches = index.match_keywords("move something");
        assert!(matches.is_empty());

        // Should match - both keywords present
        let matches = index.match_keywords("move files to folder");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].rule_id, "transfer");
    }

    #[test]
    fn test_multiple_rules() {
        let mut index = KeywordIndex::new();

        let search_rule = KeywordRule::new("search", "web_search")
            .with_keyword("search", 2.0)
            .with_keyword("find", 1.5)
            .with_match_mode(KeywordMatchMode::Any);

        let translate_rule = KeywordRule::new("translate", "translation")
            .with_keyword("translate", 2.0)
            .with_keyword("翻", 2.0)
            .with_keyword("译", 1.0)
            .with_match_mode(KeywordMatchMode::Any);

        index.add_rule(search_rule);
        index.add_rule(translate_rule);

        let matches = index.match_keywords("translate this");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].intent_type, "translation");

        let matches = index.match_keywords("search something");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].intent_type, "web_search");
    }

    #[test]
    fn test_chinese_keyword_matching() {
        let mut index = KeywordIndex::new();

        let rule = KeywordRule::new("search_cn", "web_search")
            .with_keyword("搜", 1.0)
            .with_keyword("索", 0.5)
            .with_keyword("查", 1.0)
            .with_keyword("找", 1.0)
            .with_match_mode(KeywordMatchMode::Weighted)
            .with_min_score(1.0);

        index.add_rule(rule);

        // Should match - 搜(1.0) + 索(0.5) = 1.5
        let matches = index.match_keywords("帮我搜索一下");
        assert_eq!(matches.len(), 1);
        assert!(matches[0].score >= 1.0);
    }

    #[test]
    fn test_best_match_with_threshold() {
        let mut index = KeywordIndex::new();

        let high_rule = KeywordRule::new("high", "high_intent")
            .with_keyword("important", 3.0)
            .with_match_mode(KeywordMatchMode::Any);

        let low_rule = KeywordRule::new("low", "low_intent")
            .with_keyword("maybe", 0.5)
            .with_match_mode(KeywordMatchMode::Any);

        index.add_rule(high_rule);
        index.add_rule(low_rule);

        // Should return high intent (above threshold)
        let result = index.best_match("this is important", 2.0);
        assert!(result.is_some());
        assert_eq!(result.unwrap().intent_type, "high_intent");

        // Should return low intent (above 0.4 threshold)
        let result = index.best_match("maybe something", 0.4);
        assert!(result.is_some());
        assert_eq!(result.unwrap().intent_type, "low_intent");

        // Should return None (below threshold)
        let result = index.best_match("maybe something", 1.0);
        assert!(result.is_none());
    }

    #[test]
    fn test_score_sorting() {
        let mut index = KeywordIndex::new();

        let rule_a = KeywordRule::new("a", "intent_a")
            .with_keyword("word", 1.0)
            .with_match_mode(KeywordMatchMode::Any);

        let rule_b = KeywordRule::new("b", "intent_b")
            .with_keyword("word", 3.0) // Higher weight for same keyword
            .with_match_mode(KeywordMatchMode::Any);

        index.add_rule(rule_a);
        index.add_rule(rule_b);

        let matches = index.match_keywords("the word");
        assert_eq!(matches.len(), 2);
        // Higher score should come first
        assert_eq!(matches[0].intent_type, "intent_b");
        assert_eq!(matches[1].intent_type, "intent_a");
    }

    #[test]
    fn test_case_insensitivity() {
        let mut index = KeywordIndex::new();

        let rule = KeywordRule::new("test", "test_intent")
            .with_keyword("hello", 1.0)
            .with_match_mode(KeywordMatchMode::Any);

        index.add_rule(rule);

        let matches = index.match_keywords("HELLO");
        assert_eq!(matches.len(), 1);

        let matches = index.match_keywords("Hello");
        assert_eq!(matches.len(), 1);

        let matches = index.match_keywords("hello");
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn test_empty_input() {
        let mut index = KeywordIndex::new();

        let rule = KeywordRule::new("test", "test_intent")
            .with_keyword("hello", 1.0)
            .with_match_mode(KeywordMatchMode::Any);

        index.add_rule(rule);

        let matches = index.match_keywords("");
        assert!(matches.is_empty());
    }

    #[test]
    fn test_no_matching_keywords() {
        let mut index = KeywordIndex::new();

        let rule = KeywordRule::new("test", "test_intent")
            .with_keyword("hello", 1.0)
            .with_match_mode(KeywordMatchMode::Any);

        index.add_rule(rule);

        let matches = index.match_keywords("goodbye world");
        assert!(matches.is_empty());
    }

    #[test]
    fn test_keyword_rule_builder() {
        let rule = KeywordRule::new("test", "test_intent")
            .with_keyword("single", 1.0)
            .with_keywords(&["multi1", "multi2"], 0.5)
            .with_match_mode(KeywordMatchMode::All)
            .with_min_score(2.0);

        assert_eq!(rule.id, "test");
        assert_eq!(rule.intent_type, "test_intent");
        assert_eq!(rule.keywords.len(), 3);
        assert_eq!(rule.keywords.get("single"), Some(&1.0));
        assert_eq!(rule.keywords.get("multi1"), Some(&0.5));
        assert_eq!(rule.keywords.get("multi2"), Some(&0.5));
        assert_eq!(rule.match_mode, KeywordMatchMode::All);
        assert_eq!(rule.min_score, 2.0);
    }
}
