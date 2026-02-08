//! Feature Extractor for ML-based Rule Learning
//!
//! This module implements feature extraction from user inputs for machine learning-based
//! rule generation. It extracts keywords, intents, entities, and context from natural
//! language commands.
//!
//! # Architecture
//!
//! ```text
//! User Input → Tokenization → Feature Extraction → Feature Vector
//!                ↓              ↓                    ↓
//!           Keywords      Intent Detection      Entity Recognition
//! ```
//!
//! # Features
//!
//! - **Keywords**: Important words extracted from input (verbs, nouns)
//! - **Intent**: Detected intent (read, write, execute, search, move)
//! - **Entities**: Extracted entities (file paths, commands, patterns)
//! - **Context**: Session context (working directory, recent actions)

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;

/// Feature vector extracted from user input
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureVector {
    /// Extracted keywords (verbs, nouns)
    pub keywords: Vec<String>,

    /// Detected intent
    pub intent: Intent,

    /// Extracted entities
    pub entities: Vec<Entity>,

    /// Confidence score (0.0-1.0)
    pub confidence: f64,
}

/// Intent types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Intent {
    /// Read operation (read, show, display, cat)
    Read,
    /// Write operation (write, create, save)
    Write,
    /// Execute operation (run, execute, bash)
    Execute,
    /// Search operation (search, find, grep)
    Search,
    /// Replace operation (replace, substitute)
    Replace,
    /// Move operation (move, rename, mv)
    Move,
    /// Unknown intent
    Unknown,
}

/// Entity types
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Entity {
    /// File path
    FilePath(String),
    /// Command name
    Command(String),
    /// Search pattern
    Pattern(String),
    /// Directory path
    Directory(String),
}

/// Feature extractor
pub struct FeatureExtractor {
    /// Intent keywords mapping
    intent_keywords: IntentKeywords,

    /// Common stop words to filter out
    stop_words: HashSet<String>,

    /// File extension patterns
    file_extension_regex: Regex,

    /// Command patterns
    command_regex: Regex,
}

impl FeatureExtractor {
    /// Create a new feature extractor
    pub fn new() -> Self {
        Self {
            intent_keywords: IntentKeywords::default(),
            stop_words: Self::default_stop_words(),
            file_extension_regex: Regex::new(r"\b\w+\.(rs|toml|md|txt|json|yaml|yml|js|ts|py|go|java|c|cpp|h|hpp)\b").unwrap(),
            command_regex: Regex::new(r"\b(git|cargo|npm|python|node|bash|sh|ls|cd|pwd|cat|grep|find|mv|cp|rm|mkdir)\b").unwrap(),
        }
    }

    /// Extract features from user input
    pub fn extract(&self, input: &str) -> FeatureVector {
        let normalized = input.trim().to_lowercase();

        // Extract keywords
        let keywords = self.extract_keywords(&normalized);

        // Detect intent
        let intent = self.detect_intent(&normalized, &keywords);

        // Extract entities
        let entities = self.extract_entities(&normalized);

        // Calculate confidence based on feature quality
        let confidence = self.calculate_confidence(&keywords, &intent, &entities);

        FeatureVector {
            keywords,
            intent,
            entities,
            confidence,
        }
    }

    /// Extract keywords from input
    fn extract_keywords(&self, input: &str) -> Vec<String> {
        input
            .split_whitespace()
            .filter(|word| !self.stop_words.contains(*word))
            .filter(|word| word.len() > 2) // Filter out very short words
            .map(|word| word.to_string())
            .collect()
    }

    /// Detect intent from input and keywords
    fn detect_intent(&self, input: &str, keywords: &[String]) -> Intent {
        // Check for read intent
        if keywords.iter().any(|k| self.intent_keywords.read.contains(&k.as_str())) {
            return Intent::Read;
        }

        // Check for write intent
        if keywords.iter().any(|k| self.intent_keywords.write.contains(&k.as_str())) {
            return Intent::Write;
        }

        // Check for search intent
        if keywords.iter().any(|k| self.intent_keywords.search.contains(&k.as_str())) {
            return Intent::Search;
        }

        // Check for replace intent
        if keywords.iter().any(|k| self.intent_keywords.replace.contains(&k.as_str())) {
            return Intent::Replace;
        }

        // Check for move intent
        if keywords.iter().any(|k| self.intent_keywords.move_.contains(&k.as_str())) {
            return Intent::Move;
        }

        // Check for execute intent
        if keywords.iter().any(|k| self.intent_keywords.execute.contains(&k.as_str())) {
            return Intent::Execute;
        }

        // Check for command patterns
        if self.command_regex.is_match(input) {
            return Intent::Execute;
        }

        Intent::Unknown
    }

    /// Extract entities from input
    fn extract_entities(&self, input: &str) -> Vec<Entity> {
        let mut entities = Vec::new();

        // Extract file paths
        for cap in self.file_extension_regex.captures_iter(input) {
            if let Some(path) = cap.get(0) {
                entities.push(Entity::FilePath(path.as_str().to_string()));
            }
        }

        // Extract commands
        for cap in self.command_regex.captures_iter(input) {
            if let Some(cmd) = cap.get(0) {
                entities.push(Entity::Command(cmd.as_str().to_string()));
            }
        }

        // Extract patterns (quoted strings)
        let pattern_regex = Regex::new(r#"['"]([^'"]+)['"]"#).unwrap();
        for cap in pattern_regex.captures_iter(input) {
            if let Some(pattern) = cap.get(1) {
                entities.push(Entity::Pattern(pattern.as_str().to_string()));
            }
        }

        entities
    }

    /// Calculate confidence score based on feature quality
    fn calculate_confidence(&self, keywords: &[String], intent: &Intent, entities: &[Entity]) -> f64 {
        let mut confidence: f64 = 0.0;

        // Base confidence from keywords
        if !keywords.is_empty() {
            confidence += 0.3;
        }

        // Confidence from intent detection
        if *intent != Intent::Unknown {
            confidence += 0.4;
        }

        // Confidence from entity extraction
        if !entities.is_empty() {
            confidence += 0.3;
        }

        confidence.min(1.0)
    }

    /// Default stop words
    fn default_stop_words() -> HashSet<String> {
        vec![
            "the", "a", "an", "and", "or", "but", "in", "on", "at", "to", "for",
            "of", "with", "by", "from", "up", "about", "into", "through", "during",
            "is", "are", "was", "were", "be", "been", "being", "have", "has", "had",
            "do", "does", "did", "will", "would", "should", "could", "may", "might",
            "can", "this", "that", "these", "those", "i", "you", "he", "she", "it",
            "we", "they", "me", "him", "her", "us", "them", "my", "your", "his",
            "its", "our", "their",
        ]
        .into_iter()
        .map(String::from)
        .collect()
    }
}

impl Default for FeatureExtractor {
    fn default() -> Self {
        Self::new()
    }
}

/// Intent keywords mapping
struct IntentKeywords {
    read: Vec<&'static str>,
    write: Vec<&'static str>,
    execute: Vec<&'static str>,
    search: Vec<&'static str>,
    replace: Vec<&'static str>,
    move_: Vec<&'static str>,
}

impl Default for IntentKeywords {
    fn default() -> Self {
        Self {
            read: vec!["read", "show", "display", "cat", "view", "see", "get", "fetch"],
            write: vec!["write", "create", "save", "make", "generate", "produce"],
            execute: vec!["run", "execute", "exec", "launch", "start", "invoke"],
            search: vec!["search", "find", "grep", "look", "locate", "query"],
            replace: vec!["replace", "substitute", "change", "swap", "update"],
            move_: vec!["move", "rename", "mv", "relocate", "transfer"],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_read_intent() {
        let extractor = FeatureExtractor::new();
        let features = extractor.extract("show me the config.toml file");

        assert_eq!(features.intent, Intent::Read);
        assert!(features.keywords.contains(&"show".to_string()));
        assert!(features.entities.iter().any(|e| matches!(e, Entity::FilePath(_))));
        assert!(features.confidence > 0.5);
    }

    #[test]
    fn test_extract_search_intent() {
        let extractor = FeatureExtractor::new();
        let features = extractor.extract("search for TODO in src/main.rs");

        assert_eq!(features.intent, Intent::Search);
        assert!(features.keywords.contains(&"search".to_string()));
        assert!(features.entities.iter().any(|e| matches!(e, Entity::FilePath(_))));
    }

    #[test]
    fn test_extract_execute_intent() {
        let extractor = FeatureExtractor::new();
        let features = extractor.extract("run git status");

        assert_eq!(features.intent, Intent::Execute);
        assert!(features.entities.iter().any(|e| matches!(e, Entity::Command(cmd) if cmd == "git")));
    }

    #[test]
    fn test_extract_replace_intent() {
        let extractor = FeatureExtractor::new();
        let features = extractor.extract("replace 'old' with 'new' in file.txt");

        assert_eq!(features.intent, Intent::Replace);
        assert!(features.entities.iter().any(|e| matches!(e, Entity::Pattern(_))));
    }

    #[test]
    fn test_extract_move_intent() {
        let extractor = FeatureExtractor::new();
        let features = extractor.extract("move old.rs to new.rs");

        assert_eq!(features.intent, Intent::Move);
        assert!(features.keywords.contains(&"move".to_string()));
    }

    #[test]
    fn test_extract_entities() {
        let extractor = FeatureExtractor::new();
        let features = extractor.extract("git status in src/main.rs");

        // Should extract both command and file path
        assert!(features.entities.iter().any(|e| matches!(e, Entity::Command(cmd) if cmd == "git")));
        assert!(features.entities.iter().any(|e| matches!(e, Entity::FilePath(_))));
    }

    #[test]
    fn test_stop_words_filtering() {
        let extractor = FeatureExtractor::new();
        let features = extractor.extract("show me the file");

        // "me" and "the" should be filtered out as stop words
        assert!(!features.keywords.contains(&"me".to_string()));
        assert!(!features.keywords.contains(&"the".to_string()));
        assert!(features.keywords.contains(&"show".to_string()));
        assert!(features.keywords.contains(&"file".to_string()));
    }

    #[test]
    fn test_confidence_calculation() {
        let extractor = FeatureExtractor::new();

        // High confidence: has keywords, intent, and entities
        let features1 = extractor.extract("search for TODO in main.rs");
        assert!(features1.confidence > 0.8);

        // Medium confidence: has keywords and intent, but no entities
        let features2 = extractor.extract("search for something");
        assert!(features2.confidence > 0.5);
        assert!(features2.confidence < 0.8);

        // Low confidence: only keywords
        let features3 = extractor.extract("something random");
        assert!(features3.confidence < 0.5);
    }
}
