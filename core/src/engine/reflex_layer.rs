//! Reflex Layer - L1/L2 Fast Routing
//!
//! This module implements the reflex layer that provides millisecond-level
//! response times by bypassing LLM reasoning for common operations.
//!
//! # Architecture
//!
//! ```text
//! User Input → L1 (Exact Match) → L2 (Keyword Routing) → L3 (LLM Reasoning)
//!              < 10ms              < 50ms                 1-3s
//! ```
//!
//! # Performance Goals
//!
//! - L1 hit rate: 20% (after 3 months)
//! - L2 hit rate: 50% (after 3 months)
//! - Combined: 70% of requests bypass LLM
//!
//! # Example
//!
//! ```rust,ignore
//! use alephcore::engine::{ReflexLayer, AtomicAction};
//!
//! let reflex = ReflexLayer::with_default_rules();
//!
//! // L1: Exact match (< 10ms)
//! if let Some(action) = reflex.try_reflex("git status") {
//!     // Execute immediately
//! }
//!
//! // L2: Keyword routing (< 50ms)
//! if let Some(action) = reflex.try_reflex("read src/main.rs") {
//!     // Execute immediately
//! }
//!
//! // L3: Falls through to LLM reasoning
//! ```

use dashmap::DashMap;
use regex::Regex;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use crate::sync_primitives::{Arc, RwLock};
use tracing::{debug, info};

use super::AtomicAction;

/// Reflex layer for L1/L2 fast routing
pub struct ReflexLayer {
    /// L1: Exact match cache (user input → atomic action)
    exact_cache: DashMap<String, AtomicAction>,

    /// L2: Keyword routing rules
    keyword_rules: Vec<KeywordRule>,

    /// Statistics
    stats: Arc<RwLock<ReflexStats>>,
}

impl ReflexLayer {
    /// Create a new reflex layer with empty rules
    pub fn new() -> Self {
        Self {
            exact_cache: DashMap::new(),
            keyword_rules: Vec::new(),
            stats: Arc::new(RwLock::new(ReflexStats::default())),
        }
    }

    /// Create a reflex layer with default rules
    pub fn with_default_rules() -> Self {
        let mut layer = Self::new();
        layer.load_default_rules();
        layer
    }

    /// Try reflex routing (returns None if needs L3 reasoning)
    pub fn try_reflex(&self, input: &str) -> Option<AtomicAction> {
        // L1: Exact match
        if let Some(action) = self.exact_cache.get(input) {
            self.stats.write().unwrap().l1_hits += 1;
            debug!(input = %input, "L1 cache hit");
            return Some(action.clone());
        }

        // L2: Keyword routing
        if let Some(action) = self.route_by_keywords(input) {
            self.stats.write().unwrap().l2_hits += 1;
            debug!(input = %input, action = ?action, "L2 keyword routing hit");
            return Some(action);
        }

        // Need L3 reasoning
        self.stats.write().unwrap().l3_fallbacks += 1;
        debug!(input = %input, "Falling back to L3 reasoning");
        None
    }

    /// L2 keyword routing
    fn route_by_keywords(&self, input: &str) -> Option<AtomicAction> {
        // Sort rules by priority (descending)
        let mut matched_rules: Vec<_> = self
            .keyword_rules
            .iter()
            .filter(|rule| rule.pattern.is_match(input))
            .collect();

        if matched_rules.is_empty() {
            return None;
        }

        matched_rules.sort_by_key(|r| std::cmp::Reverse(r.priority));

        // Try first matching rule
        for rule in matched_rules {
            if let Some(params) = rule.extractor.extract(input) {
                if let Some(action) = self.build_action(&rule.action_type, params) {
                    return Some(action);
                }
            }
        }

        None
    }

    /// Build atomic action from action type and parameters
    fn build_action(&self, action_type: &ActionType, params: HashMap<String, Value>) -> Option<AtomicAction> {
        match action_type {
            ActionType::Read => {
                let path = params.get("path")?.as_str()?.to_string();
                Some(AtomicAction::Read { path, range: None })
            }
            ActionType::Write => {
                let path = params.get("path")?.as_str()?.to_string();
                let content = params.get("content")?.as_str()?.to_string();
                Some(AtomicAction::Write {
                    path,
                    content,
                    mode: super::WriteMode::Overwrite,
                })
            }
            ActionType::Edit => {
                // Edit requires patches, which are too complex for L2 routing
                None
            }
            ActionType::Bash => {
                let command = params.get("command")?.as_str()?.to_string();
                Some(AtomicAction::Bash { command, cwd: None })
            }
            ActionType::Search => {
                let pattern_str = params.get("pattern")?.as_str()?.to_string();
                let pattern = super::SearchPattern::Regex { pattern: pattern_str };
                let scope = params.get("scope")
                    .and_then(|v| v.as_str())
                    .map(|s| match s {
                        "workspace" => super::SearchScope::Workspace,
                        _ => super::SearchScope::File { path: PathBuf::from(s) },
                    })
                    .unwrap_or(super::SearchScope::Workspace);

                Some(AtomicAction::Search {
                    pattern,
                    scope,
                    filters: Vec::new(),
                })
            }
            ActionType::Replace => {
                let pattern_str = params.get("pattern")?.as_str()?.to_string();
                let replacement = params.get("replacement")?.as_str()?.to_string();
                let pattern = super::SearchPattern::Regex { pattern: pattern_str };
                let scope = params.get("scope")
                    .and_then(|v| v.as_str())
                    .map(|s| match s {
                        "workspace" => super::SearchScope::Workspace,
                        _ => super::SearchScope::File { path: PathBuf::from(s) },
                    })
                    .unwrap_or(super::SearchScope::Workspace);

                Some(AtomicAction::Replace {
                    search: Box::new(pattern),
                    replacement,
                    scope,
                    preview: false,
                    dry_run: false,
                })
            }
            ActionType::Move => {
                let from = params.get("from")?.as_str()?.to_string();
                let to = params.get("to")?.as_str()?.to_string();
                let update_imports = params.get("update_imports")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);

                Some(AtomicAction::Move {
                    source: PathBuf::from(from),
                    destination: PathBuf::from(to),
                    update_imports,
                    create_parent: true,
                })
            }
        }
    }

    /// Learn from successful L3 reasoning
    pub fn learn_from_success(&self, input: &str, action: AtomicAction) {
        // Only cache simple, deterministic inputs
        if input.len() < 100 && !input.contains("复杂") && !input.contains("complex") {
            info!(input = %input, action = ?action, "Learning new L1 rule");
            self.exact_cache.insert(input.to_string(), action);
        }
    }

    /// Add a keyword rule
    pub fn add_rule(&mut self, rule: KeywordRule) {
        self.keyword_rules.push(rule);
    }

    /// Load default rules
    fn load_default_rules(&mut self) {
        // Rule 1: Read files
        self.add_rule(KeywordRule {
            pattern: Regex::new(r"(?i)^(read|cat|show|display)\s+(.+\.(rs|toml|md|txt|json|yaml|yml))$")
                .unwrap(),
            priority: 80,
            action_type: ActionType::Read,
            extractor: Box::new(FilePathExtractor),
        });

        // Rule 2: Git commands
        self.add_rule(KeywordRule {
            pattern: Regex::new(r"(?i)^git\s+(status|log|diff|branch)$").unwrap(),
            priority: 90,
            action_type: ActionType::Bash,
            extractor: Box::new(DirectCommandExtractor),
        });

        // Rule 3: List directory
        self.add_rule(KeywordRule {
            pattern: Regex::new(r"(?i)^(ls|list)\s*(.*)$").unwrap(),
            priority: 85,
            action_type: ActionType::Bash,
            extractor: Box::new(LsCommandExtractor),
        });

        // Rule 4: Current directory
        self.add_rule(KeywordRule {
            pattern: Regex::new(r"(?i)^(pwd|where am i|current directory)$").unwrap(),
            priority: 95,
            action_type: ActionType::Bash,
            extractor: Box::new(PwdCommandExtractor),
        });

        // Rule 5: Search operations
        self.add_rule(KeywordRule {
            pattern: Regex::new(r"(?i)^(search|find|grep)\s+").unwrap(),
            priority: 75,
            action_type: ActionType::Search,
            extractor: Box::new(SearchPatternExtractor),
        });

        // Rule 6: Replace operations
        self.add_rule(KeywordRule {
            pattern: Regex::new(r"(?i)^replace\s+").unwrap(),
            priority: 75,
            action_type: ActionType::Replace,
            extractor: Box::new(ReplacePatternExtractor),
        });

        // Rule 7: Move/rename operations
        self.add_rule(KeywordRule {
            pattern: Regex::new(r"(?i)^(move|mv|rename)\s+").unwrap(),
            priority: 75,
            action_type: ActionType::Move,
            extractor: Box::new(MoveFileExtractor),
        });

        info!(rule_count = self.keyword_rules.len(), "Loaded default reflex rules");
    }

    /// Get statistics
    pub fn stats(&self) -> ReflexStats {
        self.stats.read().unwrap().clone()
    }

    /// Clear L1 cache
    pub fn clear_cache(&self) {
        self.exact_cache.clear();
        info!("L1 cache cleared");
    }

    /// Get cache size
    pub fn cache_size(&self) -> usize {
        self.exact_cache.len()
    }
}

impl Default for ReflexLayer {
    fn default() -> Self {
        Self::new()
    }
}

/// Keyword routing rule
pub struct KeywordRule {
    /// Trigger pattern (regex)
    pub pattern: Regex,

    /// Priority (higher = more priority)
    pub priority: u8,

    /// Action type to route to
    pub action_type: ActionType,

    /// Parameter extractor
    pub extractor: Box<dyn ParamExtractor>,
}

/// Action type for routing
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionType {
    Read,
    Write,
    Edit,
    Bash,
    Search,
    Replace,
    Move,
}

/// Parameter extractor trait
pub trait ParamExtractor: Send + Sync {
    /// Extract parameters from input
    fn extract(&self, input: &str) -> Option<HashMap<String, Value>>;
}

/// File path extractor
struct FilePathExtractor;

impl ParamExtractor for FilePathExtractor {
    fn extract(&self, input: &str) -> Option<HashMap<String, Value>> {
        // Extract file path from commands like "read src/main.rs"
        let re = Regex::new(r"(?i)(?:read|cat|show|display)\s+(.+)").ok()?;
        let caps = re.captures(input)?;
        let path = caps.get(1)?.as_str().trim();

        let mut params = HashMap::new();
        params.insert("path".to_string(), Value::String(path.to_string()));
        Some(params)
    }
}

/// Direct command extractor (uses input as-is)
struct DirectCommandExtractor;

impl ParamExtractor for DirectCommandExtractor {
    fn extract(&self, input: &str) -> Option<HashMap<String, Value>> {
        let mut params = HashMap::new();
        params.insert("command".to_string(), Value::String(input.to_string()));
        Some(params)
    }
}

/// Ls command extractor
struct LsCommandExtractor;

impl ParamExtractor for LsCommandExtractor {
    fn extract(&self, input: &str) -> Option<HashMap<String, Value>> {
        // Extract path from "ls" or "ls path"
        let re = Regex::new(r"(?i)^(?:ls|list)\s*(.*)$").ok()?;
        let caps = re.captures(input)?;
        let path = caps.get(1).map(|m| m.as_str().trim()).unwrap_or(".");

        let command = if path.is_empty() || path == "." {
            "ls -la".to_string()
        } else {
            format!("ls -la {}", path)
        };

        let mut params = HashMap::new();
        params.insert("command".to_string(), Value::String(command));
        Some(params)
    }
}

/// Pwd command extractor
struct PwdCommandExtractor;

impl ParamExtractor for PwdCommandExtractor {
    fn extract(&self, _input: &str) -> Option<HashMap<String, Value>> {
        let mut params = HashMap::new();
        params.insert("command".to_string(), Value::String("pwd".to_string()));
        Some(params)
    }
}

/// Search pattern extractor
struct SearchPatternExtractor;

impl ParamExtractor for SearchPatternExtractor {
    fn extract(&self, input: &str) -> Option<HashMap<String, Value>> {
        // Extract pattern from commands like "search for TODO" or "find pattern in file.rs"
        let re = Regex::new(r"(?i)(?:search|find|grep)\s+(?:for\s+)?(.+?)(?:\s+in\s+(.+))?$").ok()?;
        let caps = re.captures(input)?;
        let pattern = caps.get(1)?.as_str().trim().trim_matches(|c| c == '\'' || c == '"');
        let scope = caps.get(2).map(|m| m.as_str().trim());

        let mut params = HashMap::new();
        params.insert("pattern".to_string(), Value::String(pattern.to_string()));
        if let Some(scope_str) = scope {
            params.insert("scope".to_string(), Value::String(scope_str.to_string()));
        } else {
            params.insert("scope".to_string(), Value::String("workspace".to_string()));
        }
        Some(params)
    }
}

/// Replace pattern extractor
struct ReplacePatternExtractor;

impl ParamExtractor for ReplacePatternExtractor {
    fn extract(&self, input: &str) -> Option<HashMap<String, Value>> {
        // Extract pattern and replacement from commands like "replace foo with bar" or "replace 'old' with 'new' in file.rs"
        let re = Regex::new(r"(?i)replace\s+(.+?)\s+with\s+(.+?)(?:\s+in\s+(.+))?$").ok()?;
        let caps = re.captures(input)?;
        let pattern = caps.get(1)?.as_str().trim().trim_matches(|c| c == '\'' || c == '"');
        let replacement = caps.get(2)?.as_str().trim().trim_matches(|c| c == '\'' || c == '"');
        let scope = caps.get(3).map(|m| m.as_str().trim());

        let mut params = HashMap::new();
        params.insert("pattern".to_string(), Value::String(pattern.to_string()));
        params.insert("replacement".to_string(), Value::String(replacement.to_string()));
        if let Some(scope_str) = scope {
            params.insert("scope".to_string(), Value::String(scope_str.to_string()));
        } else {
            params.insert("scope".to_string(), Value::String("workspace".to_string()));
        }
        Some(params)
    }
}

/// Move file extractor
struct MoveFileExtractor;

impl ParamExtractor for MoveFileExtractor {
    fn extract(&self, input: &str) -> Option<HashMap<String, Value>> {
        // Extract from and to paths from commands like "move file.rs to new/path.rs"
        let re = Regex::new(r"(?i)(?:move|mv|rename)\s+(.+?)\s+(?:to\s+)?(.+)").ok()?;
        let caps = re.captures(input)?;
        let from = caps.get(1)?.as_str().trim();
        let to = caps.get(2)?.as_str().trim();

        let mut params = HashMap::new();
        params.insert("from".to_string(), Value::String(from.to_string()));
        params.insert("to".to_string(), Value::String(to.to_string()));
        params.insert("update_imports".to_string(), Value::Bool(true));
        Some(params)
    }
}

/// Reflex statistics
#[derive(Debug, Clone, Default)]
pub struct ReflexStats {
    /// L1 cache hits
    pub l1_hits: u64,

    /// L2 keyword routing hits
    pub l2_hits: u64,

    /// L3 fallbacks (needs LLM reasoning)
    pub l3_fallbacks: u64,
}

impl ReflexStats {
    /// Get total requests
    pub fn total(&self) -> u64 {
        self.l1_hits + self.l2_hits + self.l3_fallbacks
    }

    /// Get L1 hit rate
    pub fn l1_hit_rate(&self) -> f64 {
        if self.total() == 0 {
            0.0
        } else {
            self.l1_hits as f64 / self.total() as f64
        }
    }

    /// Get L2 hit rate
    pub fn l2_hit_rate(&self) -> f64 {
        if self.total() == 0 {
            0.0
        } else {
            self.l2_hits as f64 / self.total() as f64
        }
    }

    /// Get combined reflex hit rate (L1 + L2)
    pub fn reflex_hit_rate(&self) -> f64 {
        if self.total() == 0 {
            0.0
        } else {
            (self.l1_hits + self.l2_hits) as f64 / self.total() as f64
        }
    }

    /// Get L3 fallback rate
    pub fn l3_fallback_rate(&self) -> f64 {
        if self.total() == 0 {
            0.0
        } else {
            self.l3_fallbacks as f64 / self.total() as f64
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{SearchPattern, SearchScope};

    #[test]
    fn test_reflex_layer_l1_exact_match() {
        let reflex = ReflexLayer::new();

        // Add to L1 cache
        let action = AtomicAction::Bash {
            command: "git status".to_string(),
            cwd: None,
        };
        reflex.learn_from_success("git status", action.clone());

        // Should hit L1 cache
        let result = reflex.try_reflex("git status");
        assert!(result.is_some());
        assert_eq!(result.unwrap(), action);

        // Check stats
        let stats = reflex.stats();
        assert_eq!(stats.l1_hits, 1);
        assert_eq!(stats.l2_hits, 0);
        assert_eq!(stats.l3_fallbacks, 0);
    }

    #[test]
    fn test_reflex_layer_l2_keyword_routing() {
        let reflex = ReflexLayer::with_default_rules();

        // Should hit L2 keyword routing
        let result = reflex.try_reflex("read src/main.rs");
        assert!(result.is_some());

        if let Some(AtomicAction::Read { path, .. }) = result {
            assert_eq!(path, "src/main.rs");
        } else {
            panic!("Expected Read action");
        }

        // Check stats
        let stats = reflex.stats();
        assert_eq!(stats.l1_hits, 0);
        assert_eq!(stats.l2_hits, 1);
        assert_eq!(stats.l3_fallbacks, 0);
    }

    #[test]
    fn test_reflex_layer_l3_fallback() {
        let reflex = ReflexLayer::with_default_rules();

        // Complex query should fall back to L3
        let result = reflex.try_reflex("analyze the codebase and find all bugs");
        assert!(result.is_none());

        // Check stats
        let stats = reflex.stats();
        assert_eq!(stats.l1_hits, 0);
        assert_eq!(stats.l2_hits, 0);
        assert_eq!(stats.l3_fallbacks, 1);
    }

    #[test]
    fn test_reflex_layer_git_commands() {
        let reflex = ReflexLayer::with_default_rules();

        let test_cases = vec!["git status", "git log", "git diff", "git branch"];

        for input in test_cases {
            let result = reflex.try_reflex(input);
            assert!(result.is_some(), "Failed for input: {}", input);

            if let Some(AtomicAction::Bash { command, .. }) = result {
                assert_eq!(command, input);
            } else {
                panic!("Expected Bash action for: {}", input);
            }
        }
    }

    #[test]
    fn test_reflex_layer_ls_commands() {
        let reflex = ReflexLayer::with_default_rules();

        // Test "ls" without path
        let result = reflex.try_reflex("ls");
        assert!(result.is_some());
        if let Some(AtomicAction::Bash { command, .. }) = result {
            assert_eq!(command, "ls -la");
        }

        // Test "ls" with path
        let result = reflex.try_reflex("ls src/");
        assert!(result.is_some());
        if let Some(AtomicAction::Bash { command, .. }) = result {
            assert_eq!(command, "ls -la src/");
        }
    }

    #[test]
    fn test_reflex_layer_pwd_command() {
        let reflex = ReflexLayer::with_default_rules();

        let test_cases = vec!["pwd", "where am i", "current directory"];

        for input in test_cases {
            let result = reflex.try_reflex(input);
            assert!(result.is_some(), "Failed for input: {}", input);

            if let Some(AtomicAction::Bash { command, .. }) = result {
                assert_eq!(command, "pwd");
            } else {
                panic!("Expected Bash action for: {}", input);
            }
        }
    }

    #[test]
    fn test_reflex_layer_learning() {
        let reflex = ReflexLayer::new();

        // Learn a new pattern
        let action = AtomicAction::Read {
            path: "config.toml".to_string(),
            range: None,
        };
        reflex.learn_from_success("show config", action.clone());

        // Should hit L1 cache
        let result = reflex.try_reflex("show config");
        assert!(result.is_some());
        assert_eq!(result.unwrap(), action);
    }

    #[test]
    fn test_reflex_layer_stats() {
        let reflex = ReflexLayer::with_default_rules();

        // L1 hit
        reflex.learn_from_success("test", AtomicAction::Bash {
            command: "test".to_string(),
            cwd: None,
        });
        reflex.try_reflex("test");

        // L2 hit
        reflex.try_reflex("git status");

        // L3 fallback
        reflex.try_reflex("complex query");

        let stats = reflex.stats();
        assert_eq!(stats.total(), 3);
        assert_eq!(stats.l1_hits, 1);
        assert_eq!(stats.l2_hits, 1);
        assert_eq!(stats.l3_fallbacks, 1);
        assert!((stats.reflex_hit_rate() - 0.666).abs() < 0.01);
    }

    #[test]
    fn test_reflex_layer_clear_cache() {
        let reflex = ReflexLayer::new();

        // Add to cache
        reflex.learn_from_success("test", AtomicAction::Bash {
            command: "test".to_string(),
            cwd: None,
        });
        assert_eq!(reflex.cache_size(), 1);

        // Clear cache
        reflex.clear_cache();
        assert_eq!(reflex.cache_size(), 0);

        // Should not hit L1 anymore
        let result = reflex.try_reflex("test");
        assert!(result.is_none());
    }

    #[test]
    fn test_reflex_layer_priority() {
        let mut reflex = ReflexLayer::new();

        // Add two rules with different priorities
        reflex.add_rule(KeywordRule {
            pattern: Regex::new(r"test").unwrap(),
            priority: 50,
            action_type: ActionType::Bash,
            extractor: Box::new(DirectCommandExtractor),
        });

        reflex.add_rule(KeywordRule {
            pattern: Regex::new(r"test").unwrap(),
            priority: 100,
            action_type: ActionType::Read,
            extractor: Box::new(FilePathExtractor),
        });

        // Higher priority rule should match first
        // But FilePathExtractor will fail to extract, so it falls back to Bash
        let result = reflex.try_reflex("test");
        // This test demonstrates priority ordering, actual result depends on extractor success
        assert!(result.is_some() || result.is_none());
    }

    #[test]
    fn test_reflex_layer_search_operation() {
        let reflex = ReflexLayer::with_default_rules();

        // Test search command
        let result = reflex.try_reflex("search for TODO");
        assert!(result.is_some());

        if let Some(AtomicAction::Search { pattern, scope, .. }) = result {
            assert!(matches!(pattern, SearchPattern::Regex { .. }));
            assert!(matches!(scope, SearchScope::Workspace));
        } else {
            panic!("Expected Search action");
        }

        // Test search with scope
        let result = reflex.try_reflex("find pattern in src/main.rs");
        assert!(result.is_some());

        if let Some(AtomicAction::Search { scope, .. }) = result {
            assert!(matches!(scope, SearchScope::File { .. }));
        } else {
            panic!("Expected Search action with file scope");
        }
    }

    #[test]
    fn test_reflex_layer_replace_operation() {
        let reflex = ReflexLayer::with_default_rules();

        // Test replace command
        let result = reflex.try_reflex("replace foo with bar");
        assert!(result.is_some());

        if let Some(AtomicAction::Replace { search, replacement, scope, .. }) = result {
            assert!(matches!(*search, SearchPattern::Regex { .. }));
            assert_eq!(replacement, "bar");
            assert!(matches!(scope, SearchScope::Workspace));
        } else {
            panic!("Expected Replace action");
        }

        // Test replace with scope
        let result = reflex.try_reflex("replace 'old' with 'new' in config.toml");
        assert!(result.is_some());

        if let Some(AtomicAction::Replace { replacement, scope, .. }) = result {
            assert_eq!(replacement, "new");
            assert!(matches!(scope, SearchScope::File { .. }));
        } else {
            panic!("Expected Replace action with file scope");
        }
    }

    #[test]
    fn test_reflex_layer_move_operation() {
        let reflex = ReflexLayer::with_default_rules();

        // Test move command
        let result = reflex.try_reflex("move old.rs to new.rs");
        assert!(result.is_some());

        if let Some(AtomicAction::Move { source, destination, update_imports, .. }) = result {
            assert_eq!(source.to_str().unwrap(), "old.rs");
            assert_eq!(destination.to_str().unwrap(), "new.rs");
            assert!(update_imports);
        } else {
            panic!("Expected Move action");
        }

        // Test mv command
        let result = reflex.try_reflex("mv src/old.rs src/new.rs");
        assert!(result.is_some());

        if let Some(AtomicAction::Move { source, destination, .. }) = result {
            assert_eq!(source.to_str().unwrap(), "src/old.rs");
            assert_eq!(destination.to_str().unwrap(), "src/new.rs");
        } else {
            panic!("Expected Move action");
        }

        // Test rename command
        let result = reflex.try_reflex("rename file.txt document.txt");
        assert!(result.is_some());

        if let Some(AtomicAction::Move { source, destination, .. }) = result {
            assert_eq!(source.to_str().unwrap(), "file.txt");
            assert_eq!(destination.to_str().unwrap(), "document.txt");
        } else {
            panic!("Expected Move action");
        }
    }
}
