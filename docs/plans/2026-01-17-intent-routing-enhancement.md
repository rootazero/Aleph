# Intent Routing Enhancement Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Enhance intent detection by integrating KeywordIndex from old design, connecting AiIntentDetector as L3, and ensuring rig-core function calling remains the primary execution path.

**Architecture:**
- Phase 1: Add `KeywordIndex` module (from old design) for weighted keyword matching
- Phase 2: Add `KeywordPolicy` configuration to support config.toml loading
- Phase 3: Integrate `KeywordIndex` into `IntentClassifier` as enhanced L2
- Phase 4: Connect `AiIntentDetector` as L3 fallback in `IntentClassifier`
- Phase 5: Integration testing and cleanup

**Tech Stack:** Rust, serde, tokio, regex

---

## Task 1: Add KeywordIndex Module

**Files:**
- Create: `Aether/core/src/intent/keyword.rs`
- Modify: `Aether/core/src/intent/mod.rs`

**Step 1: Create keyword.rs with KeywordIndex implementation**

```rust
//! Keyword Index - Fast keyword-based matching with weighted scoring
//!
//! Provides efficient keyword lookup using an inverted index.
//! Supports multiple match modes and weighted scoring.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Check if a character is CJK (Chinese, Japanese, Korean)
fn is_cjk_char(ch: char) -> bool {
    let code = ch as u32;
    (0x4E00..=0x9FFF).contains(&code)
        || (0x3400..=0x4DBF).contains(&code)
        || (0x20000..=0x2CEAF).contains(&code)
        || (0x3040..=0x309F).contains(&code)
        || (0x30A0..=0x30FF).contains(&code)
        || (0xAC00..=0xD7AF).contains(&code)
}

/// Keyword match mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum KeywordMatchMode {
    /// Any keyword matches (OR logic)
    #[default]
    Any,
    /// All keywords must match (AND logic)
    All,
    /// Weighted scoring (sum of matched weights / total weights)
    Weighted,
}

/// A keyword rule definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeywordRule {
    /// Unique rule ID
    pub id: String,
    /// Keywords with weights
    pub keywords: Vec<(String, f32)>,
    /// Match mode
    #[serde(default)]
    pub match_mode: KeywordMatchMode,
    /// Associated intent type
    pub intent_type: String,
    /// Minimum score threshold
    pub min_score: Option<f32>,
}

impl KeywordRule {
    /// Create a new keyword rule with equal weights
    pub fn new(id: impl Into<String>, intent_type: impl Into<String>, keywords: Vec<String>) -> Self {
        Self {
            id: id.into(),
            keywords: keywords.into_iter().map(|k| (k, 1.0)).collect(),
            match_mode: KeywordMatchMode::Any,
            intent_type: intent_type.into(),
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
            keywords,
            match_mode: KeywordMatchMode::Weighted,
            intent_type: intent_type.into(),
            min_score: None,
        }
    }

    /// Set match mode
    pub fn with_mode(mut self, mode: KeywordMatchMode) -> Self {
        self.match_mode = mode;
        self
    }

    /// Set minimum score
    pub fn with_min_score(mut self, min_score: f32) -> Self {
        self.min_score = Some(min_score);
        self
    }

    /// Get total possible score
    pub fn total_weight(&self) -> f32 {
        self.keywords.iter().map(|(_, w)| w).sum()
    }
}

/// Result of keyword matching
#[derive(Debug, Clone)]
pub struct KeywordMatch {
    /// Matched rule ID
    pub rule_id: String,
    /// Match score (0.0 - 1.0)
    pub score: f32,
    /// Matched keywords
    pub matched_keywords: Vec<String>,
    /// Associated intent type
    pub intent_type: String,
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
                    if total > 0.0 { (self.total_weight / total).min(1.0) } else { 0.0 }
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
        }
    }
}

/// Inverted index for fast keyword matching
#[derive(Debug, Clone, Default)]
pub struct KeywordIndex {
    /// keyword -> [(rule_id, weight)]
    index: HashMap<String, Vec<(String, f32)>>,
    /// Rule metadata
    rules: HashMap<String, KeywordRule>,
}

impl KeywordIndex {
    /// Create a new empty keyword index
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a keyword rule to the index
    pub fn add_rule(&mut self, rule: KeywordRule) {
        let rule_id = rule.id.clone();
        for (keyword, weight) in &rule.keywords {
            let normalized = keyword.to_lowercase();
            self.index
                .entry(normalized)
                .or_default()
                .push((rule_id.clone(), *weight));
        }
        self.rules.insert(rule_id, rule);
    }

    /// Tokenize text into keywords (handles CJK)
    fn tokenize(&self, text: &str) -> Vec<String> {
        let mut tokens = Vec::new();
        let mut current_token = String::new();

        for ch in text.to_lowercase().chars() {
            if is_cjk_char(ch) {
                if !current_token.is_empty() {
                    tokens.push(std::mem::take(&mut current_token));
                }
                tokens.push(ch.to_string());
            } else if ch.is_ascii_alphanumeric() {
                current_token.push(ch);
            } else if !current_token.is_empty() {
                tokens.push(std::mem::take(&mut current_token));
            }
        }
        if !current_token.is_empty() {
            tokens.push(current_token);
        }
        tokens
    }

    /// Match input against all keywords
    pub fn match_keywords(&self, input: &str) -> Vec<KeywordMatch> {
        let tokens = self.tokenize(input);
        let mut rule_scores: HashMap<String, KeywordMatchBuilder> = HashMap::new();

        for token in &tokens {
            if let Some(matches) = self.index.get(token) {
                for (rule_id, weight) in matches {
                    let builder = rule_scores
                        .entry(rule_id.clone())
                        .or_insert_with(|| KeywordMatchBuilder::new(rule_id.clone()));
                    builder.add_keyword(token.clone(), *weight);
                }
            }
        }

        let mut results: Vec<KeywordMatch> = rule_scores
            .into_values()
            .map(|builder| builder.build(&self.rules))
            .collect();

        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        results
    }

    /// Get the best match above threshold
    pub fn best_match(&self, input: &str, min_score: f32) -> Option<KeywordMatch> {
        self.match_keywords(input)
            .into_iter()
            .find(|m| m.score >= min_score)
    }

    /// Get rule count
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keyword_index_basic() {
        let mut index = KeywordIndex::new();
        index.add_rule(KeywordRule::new(
            "weather", "search",
            vec!["weather".to_string(), "forecast".to_string()],
        ));

        let matches = index.match_keywords("What's the weather today?");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].rule_id, "weather");
    }

    #[test]
    fn test_keyword_index_chinese() {
        let mut index = KeywordIndex::new();
        index.add_rule(KeywordRule::new(
            "file_organize", "FileOrganize",
            vec!["整".to_string(), "理".to_string(), "文".to_string(), "件".to_string()],
        ).with_mode(KeywordMatchMode::Weighted));

        let matches = index.match_keywords("帮我整理文件");
        assert_eq!(matches.len(), 1);
        assert!(matches[0].score > 0.5);
    }

    #[test]
    fn test_keyword_weighted() {
        let mut index = KeywordIndex::new();
        index.add_rule(KeywordRule::with_weights(
            "search", "search",
            vec![("search".to_string(), 2.0), ("find".to_string(), 1.0)],
        ));

        let matches = index.match_keywords("search for info");
        assert_eq!(matches.len(), 1);
        assert!((matches[0].score - 0.667).abs() < 0.01);
    }

    #[test]
    fn test_case_insensitive() {
        let mut index = KeywordIndex::new();
        index.add_rule(KeywordRule::new("test", "test", vec!["hello".to_string()]));

        let matches = index.match_keywords("HELLO world");
        assert_eq!(matches.len(), 1);
    }
}
```

**Step 2: Update mod.rs to export KeywordIndex**

Add to `Aether/core/src/intent/mod.rs`:
```rust
pub mod keyword;
pub use keyword::{KeywordIndex, KeywordMatch, KeywordMatchMode, KeywordRule};
```

**Step 3: Run tests to verify**

Run: `cd Aether/core && cargo test intent::keyword --lib`
Expected: All tests PASS

**Step 4: Commit**

```bash
git add Aether/core/src/intent/keyword.rs Aether/core/src/intent/mod.rs
git commit -m "feat(intent): add KeywordIndex module with weighted scoring and CJK support"
```

---

## Task 2: Add KeywordPolicy Configuration

**Files:**
- Create: `Aether/core/src/config/types/policies/keyword.rs`
- Modify: `Aether/core/src/config/types/policies/mod.rs`

**Step 1: Create keyword.rs policy**

```rust
//! Keyword matching policy configuration
//!
//! Configurable keyword rules for L2 intent detection.

use serde::{Deserialize, Serialize};

/// Single keyword with weight
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeightedKeyword {
    /// Keyword text
    pub word: String,
    /// Weight (default 1.0)
    #[serde(default = "default_weight")]
    pub weight: f32,
}

fn default_weight() -> f32 {
    1.0
}

/// A keyword rule in config
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeywordRuleConfig {
    /// Unique rule ID
    pub id: String,
    /// Target intent type (maps to TaskCategory or TaskIntent)
    pub intent_type: String,
    /// Keywords with optional weights
    pub keywords: Vec<WeightedKeyword>,
    /// Match mode: "any", "all", or "weighted"
    #[serde(default)]
    pub match_mode: String,
    /// Minimum score threshold (0.0-1.0)
    #[serde(default = "default_min_score")]
    pub min_score: f32,
}

fn default_min_score() -> f32 {
    0.5
}

/// Policy for keyword-based intent detection
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct KeywordPolicy {
    /// Enable keyword matching
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Global minimum score threshold
    #[serde(default = "default_global_min_score")]
    pub global_min_score: f32,
    /// Keyword rules
    #[serde(default)]
    pub rules: Vec<KeywordRuleConfig>,
}

fn default_enabled() -> bool {
    true
}

fn default_global_min_score() -> f32 {
    0.6
}

impl KeywordPolicy {
    /// Create a policy with default built-in rules
    pub fn with_builtin_rules() -> Self {
        Self {
            enabled: true,
            global_min_score: 0.6,
            rules: Self::builtin_rules(),
        }
    }

    /// Built-in rules for common intents
    fn builtin_rules() -> Vec<KeywordRuleConfig> {
        vec![
            KeywordRuleConfig {
                id: "file_organize".to_string(),
                intent_type: "FileOrganize".to_string(),
                keywords: vec![
                    WeightedKeyword { word: "整理".to_string(), weight: 2.0 },
                    WeightedKeyword { word: "归类".to_string(), weight: 2.0 },
                    WeightedKeyword { word: "organize".to_string(), weight: 2.0 },
                    WeightedKeyword { word: "sort".to_string(), weight: 1.5 },
                    WeightedKeyword { word: "文件".to_string(), weight: 1.0 },
                    WeightedKeyword { word: "files".to_string(), weight: 1.0 },
                ],
                match_mode: "weighted".to_string(),
                min_score: 0.5,
            },
            KeywordRuleConfig {
                id: "file_cleanup".to_string(),
                intent_type: "FileCleanup".to_string(),
                keywords: vec![
                    WeightedKeyword { word: "删除".to_string(), weight: 2.0 },
                    WeightedKeyword { word: "清理".to_string(), weight: 2.0 },
                    WeightedKeyword { word: "delete".to_string(), weight: 2.0 },
                    WeightedKeyword { word: "clean".to_string(), weight: 1.5 },
                    WeightedKeyword { word: "文件".to_string(), weight: 1.0 },
                    WeightedKeyword { word: "缓存".to_string(), weight: 1.0 },
                ],
                match_mode: "weighted".to_string(),
                min_score: 0.5,
            },
            KeywordRuleConfig {
                id: "code_execution".to_string(),
                intent_type: "CodeExecution".to_string(),
                keywords: vec![
                    WeightedKeyword { word: "运行".to_string(), weight: 2.0 },
                    WeightedKeyword { word: "执行".to_string(), weight: 2.0 },
                    WeightedKeyword { word: "run".to_string(), weight: 2.0 },
                    WeightedKeyword { word: "execute".to_string(), weight: 2.0 },
                    WeightedKeyword { word: "脚本".to_string(), weight: 1.0 },
                    WeightedKeyword { word: "代码".to_string(), weight: 1.0 },
                    WeightedKeyword { word: "script".to_string(), weight: 1.0 },
                ],
                match_mode: "weighted".to_string(),
                min_score: 0.5,
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_policy() {
        let policy = KeywordPolicy::default();
        assert!(policy.enabled);
        assert!(policy.rules.is_empty());
    }

    #[test]
    fn test_builtin_rules() {
        let policy = KeywordPolicy::with_builtin_rules();
        assert!(policy.enabled);
        assert!(!policy.rules.is_empty());
        assert!(policy.rules.iter().any(|r| r.id == "file_organize"));
    }

    #[test]
    fn test_deserialize() {
        let toml = r#"
            enabled = true
            global_min_score = 0.7
            [[rules]]
            id = "test"
            intent_type = "Test"
            keywords = [{ word = "test", weight = 1.0 }]
        "#;
        let policy: KeywordPolicy = toml::from_str(toml).unwrap();
        assert_eq!(policy.global_min_score, 0.7);
        assert_eq!(policy.rules.len(), 1);
    }
}
```

**Step 2: Update policies/mod.rs**

Add to `Aether/core/src/config/types/policies/mod.rs`:
```rust
pub mod keyword;
pub use keyword::{KeywordPolicy, KeywordRuleConfig, WeightedKeyword};
```

**Step 3: Run tests**

Run: `cd Aether/core && cargo test config::types::policies::keyword --lib`
Expected: All tests PASS

**Step 4: Commit**

```bash
git add Aether/core/src/config/types/policies/keyword.rs Aether/core/src/config/types/policies/mod.rs
git commit -m "feat(config): add KeywordPolicy for configurable keyword matching rules"
```

---

## Task 3: Integrate KeywordIndex into IntentClassifier

**Files:**
- Modify: `Aether/core/src/intent/classifier.rs`

**Step 1: Add KeywordIndex integration**

Add imports and field to IntentClassifier:
```rust
use super::keyword::{KeywordIndex, KeywordMatch, KeywordMatchMode, KeywordRule};
use crate::config::KeywordPolicy;
```

Add to `IntentClassifier` struct:
```rust
/// Keyword index for L2 matching
keyword_index: KeywordIndex,
```

**Step 2: Add method to build KeywordIndex from policy**

```rust
impl IntentClassifier {
    /// Create classifier with keyword policy
    pub fn with_keyword_policy(policy: &KeywordPolicy) -> Self {
        let mut classifier = Self::new();
        if policy.enabled {
            classifier.load_keyword_rules(&policy.rules, policy.global_min_score);
        }
        classifier
    }

    /// Load keyword rules from config
    fn load_keyword_rules(&mut self, rules: &[KeywordRuleConfig], _global_min_score: f32) {
        use crate::config::KeywordRuleConfig;

        for rule_config in rules {
            let keywords: Vec<(String, f32)> = rule_config
                .keywords
                .iter()
                .map(|k| (k.word.clone(), k.weight))
                .collect();

            let mode = match rule_config.match_mode.as_str() {
                "all" => KeywordMatchMode::All,
                "weighted" => KeywordMatchMode::Weighted,
                _ => KeywordMatchMode::Any,
            };

            let rule = KeywordRule::with_weights(&rule_config.id, &rule_config.intent_type, keywords)
                .with_mode(mode)
                .with_min_score(rule_config.min_score);

            self.keyword_index.add_rule(rule);
        }
    }

    /// L2 Enhanced: Keyword index matching
    pub fn match_keywords_enhanced(&self, input: &str) -> Option<ExecutableTask> {
        // Check exclusion patterns first
        if self.contains_exclusion_verb(&input.to_lowercase()) {
            return None;
        }

        // Try keyword index
        if let Some(km) = self.keyword_index.best_match(input, 0.5) {
            // Convert intent_type to TaskCategory
            if let Some(category) = self.intent_type_to_category(&km.intent_type) {
                let target = self.extract_path(input);
                return Some(ExecutableTask {
                    category,
                    action: input.to_string(),
                    target,
                    confidence: km.score,
                });
            }
        }
        None
    }

    /// Convert intent type string to TaskCategory
    fn intent_type_to_category(&self, intent_type: &str) -> Option<TaskCategory> {
        match intent_type {
            "FileOrganize" => Some(TaskCategory::FileOrganize),
            "FileTransfer" => Some(TaskCategory::FileTransfer),
            "FileCleanup" => Some(TaskCategory::FileCleanup),
            "CodeExecution" => Some(TaskCategory::CodeExecution),
            "DocumentGenerate" => Some(TaskCategory::DocumentGenerate),
            _ => None,
        }
    }
}
```

**Step 3: Update classify() to use enhanced L2**

Modify the `classify()` method:
```rust
pub async fn classify(&self, input: &str) -> ExecutionIntent {
    if input.trim().len() < 3 {
        return ExecutionIntent::Conversational;
    }

    // L1: Regex matching (<5ms)
    if let Some(task) = self.match_regex(input) {
        return ExecutionIntent::Executable(task);
    }

    // L2: Enhanced keyword matching with KeywordIndex
    if let Some(task) = self.match_keywords_enhanced(input) {
        return ExecutionIntent::Executable(task);
    }

    // L2 Fallback: Original keyword matching
    if let Some(task) = self.match_keywords(input) {
        return ExecutionIntent::Executable(task);
    }

    // L3: TODO - integrate AiIntentDetector
    ExecutionIntent::Conversational
}
```

**Step 4: Update new() to initialize keyword_index**

```rust
impl IntentClassifier {
    pub fn new() -> Self {
        Self {
            confidence_threshold: 0.7,
            keyword_index: KeywordIndex::new(),
        }
    }
}
```

**Step 5: Add tests**

```rust
#[test]
fn test_enhanced_keyword_matching() {
    let policy = KeywordPolicy::with_builtin_rules();
    let classifier = IntentClassifier::with_keyword_policy(&policy);

    // Test Chinese file organize
    let result = classifier.match_keywords_enhanced("帮我整理一下文件");
    assert!(result.is_some());
    assert_eq!(result.unwrap().category, TaskCategory::FileOrganize);
}

#[test]
fn test_enhanced_keyword_exclusion() {
    let policy = KeywordPolicy::with_builtin_rules();
    let classifier = IntentClassifier::with_keyword_policy(&policy);

    // Analysis request should NOT trigger agent mode
    let result = classifier.match_keywords_enhanced("分析这个文件");
    assert!(result.is_none());
}
```

**Step 6: Run tests**

Run: `cd Aether/core && cargo test intent::classifier --lib`
Expected: All tests PASS

**Step 7: Commit**

```bash
git add Aether/core/src/intent/classifier.rs
git commit -m "feat(intent): integrate KeywordIndex for enhanced L2 matching"
```

---

## Task 4: Connect AiIntentDetector as L3

**Files:**
- Modify: `Aether/core/src/intent/classifier.rs`

**Step 1: Add AiIntentDetector field**

```rust
use super::ai_detector::{AiIntentDetector, AiIntentResult};
use crate::providers::AiProvider;
use std::sync::Arc;

pub struct IntentClassifier {
    confidence_threshold: f32,
    keyword_index: KeywordIndex,
    /// Optional AI detector for L3 fallback
    ai_detector: Option<Arc<AiIntentDetector>>,
}
```

**Step 2: Add with_ai_detector method**

```rust
impl IntentClassifier {
    /// Set AI detector for L3 classification
    pub fn with_ai_detector(mut self, detector: Arc<AiIntentDetector>) -> Self {
        self.ai_detector = Some(detector);
        self
    }

    /// Create with AI provider
    pub fn with_ai_provider(mut self, provider: Arc<dyn AiProvider>) -> Self {
        self.ai_detector = Some(Arc::new(AiIntentDetector::new(provider)));
        self
    }
}
```

**Step 3: Update classify() for L3**

```rust
pub async fn classify(&self, input: &str) -> ExecutionIntent {
    if input.trim().len() < 3 {
        return ExecutionIntent::Conversational;
    }

    // L1: Regex matching (<5ms)
    if let Some(task) = self.match_regex(input) {
        return ExecutionIntent::Executable(task);
    }

    // L2: Enhanced keyword matching
    if let Some(task) = self.match_keywords_enhanced(input) {
        return ExecutionIntent::Executable(task);
    }

    // L2 Fallback: Original keyword matching
    if let Some(task) = self.match_keywords(input) {
        return ExecutionIntent::Executable(task);
    }

    // L3: AI-based classification (optional)
    if let Some(ref detector) = self.ai_detector {
        if let Ok(Some(ai_result)) = detector.detect(input).await {
            if let Some(task) = self.convert_ai_result(&ai_result, input) {
                return ExecutionIntent::Executable(task);
            }
        }
    }

    ExecutionIntent::Conversational
}

/// Convert AiIntentResult to ExecutableTask
fn convert_ai_result(&self, result: &AiIntentResult, input: &str) -> Option<ExecutableTask> {
    // Map AI intent to TaskCategory
    let category = match result.intent.as_str() {
        "file_organize" => Some(TaskCategory::FileOrganize),
        "file_cleanup" => Some(TaskCategory::FileCleanup),
        "code_execution" => Some(TaskCategory::CodeExecution),
        _ => None,
    }?;

    Some(ExecutableTask {
        category,
        action: input.to_string(),
        target: result.params.get("path").cloned(),
        confidence: result.confidence as f32,
    })
}
```

**Step 4: Update new() and Default**

```rust
impl IntentClassifier {
    pub fn new() -> Self {
        Self {
            confidence_threshold: 0.7,
            keyword_index: KeywordIndex::new(),
            ai_detector: None,
        }
    }
}

impl Default for IntentClassifier {
    fn default() -> Self {
        Self::new()
    }
}
```

**Step 5: Run tests**

Run: `cd Aether/core && cargo test intent::classifier --lib`
Expected: All tests PASS

**Step 6: Commit**

```bash
git add Aether/core/src/intent/classifier.rs
git commit -m "feat(intent): connect AiIntentDetector as L3 fallback"
```

---

## Task 5: Integration and Export

**Files:**
- Modify: `Aether/core/src/config/mod.rs`
- Modify: `Aether/core/src/lib.rs`

**Step 1: Export KeywordPolicy from config**

Add to `Aether/core/src/config/mod.rs` or appropriate location:
```rust
pub use types::policies::keyword::{KeywordPolicy, KeywordRuleConfig, WeightedKeyword};
```

**Step 2: Run full test suite**

Run: `cd Aether/core && cargo test --lib`
Expected: All tests PASS

**Step 3: Build check**

Run: `cd Aether/core && cargo build`
Expected: Build succeeds without errors

**Step 4: Commit**

```bash
git add -A
git commit -m "feat(intent): complete intent routing enhancement with KeywordIndex and L3 AI detection"
```

---

## Task 6: Documentation and Cleanup

**Files:**
- Modify: `Aether/core/src/intent/mod.rs` (update docs)

**Step 1: Update module documentation**

```rust
//! Intent detection module for AI-powered conversation flow.
//!
//! # Three-Layer Architecture
//!
//! ```text
//! User Input
//!     ↓
//! [L1: Regex Matching] (<5ms)
//!     - Fast pattern matching for explicit commands
//!     - Confidence: 1.0
//!     ↓
//! [L2: Keyword Matching] (<20ms)
//!     - KeywordIndex with weighted scoring
//!     - Supports CJK characters
//!     - Configurable via KeywordPolicy
//!     - Confidence: 0.5-0.95
//!     ↓
//! [L3: AI Classification] (optional, 1-3s)
//!     - AiIntentDetector for complex cases
//!     - Language-agnostic
//!     - Confidence: varies
//!     ↓
//! ExecutionIntent { Executable | Ambiguous | Conversational }
//! ```
//!
//! # Usage
//!
//! ```ignore
//! // Basic usage
//! let classifier = IntentClassifier::new();
//! let intent = classifier.classify("帮我整理文件").await;
//!
//! // With keyword policy
//! let policy = KeywordPolicy::with_builtin_rules();
//! let classifier = IntentClassifier::with_keyword_policy(&policy);
//!
//! // With AI L3
//! let classifier = classifier.with_ai_provider(provider);
//! ```
```

**Step 2: Commit**

```bash
git add Aether/core/src/intent/mod.rs
git commit -m "docs(intent): update module documentation with three-layer architecture"
```

---

## Summary

This plan enhances the intent detection system by:

1. **Task 1**: Adding `KeywordIndex` module with weighted scoring and CJK support
2. **Task 2**: Adding `KeywordPolicy` for config.toml-based rule configuration
3. **Task 3**: Integrating `KeywordIndex` into `IntentClassifier` as enhanced L2
4. **Task 4**: Connecting `AiIntentDetector` as L3 fallback
5. **Task 5**: Integration and export
6. **Task 6**: Documentation update

The rig-core function calling remains the primary execution path. Intent detection serves as:
- **Pre-filtering**: Fast classification before AI call
- **Agent Mode trigger**: Identifying executable tasks
- **Model selection**: Choosing optimal model based on task type
