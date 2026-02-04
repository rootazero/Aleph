# Natural Language Command Detection Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Enable users to invoke Skills, MCP, and Custom commands via natural language without the `/` prefix.

**Architecture:** Two-layer detection (L1: explicit mention via regex, L2: implicit intent via keyword matching) integrated into existing `CommandParser`. A new `UnifiedCommandIndex` aggregates triggers from all command sources.

**Tech Stack:** Rust, regex crate, existing SkillsRegistry/CommandRegistry/RoutingRuleConfig

---

## Task 1: Add `triggers` Field to Config Types

**Files:**
- Modify: `core/src/config/types/routing.rs:55-130`
- Modify: `core/src/config/types/tools.rs:563-590`
- Modify: `core/src/skills/mod.rs:37-49`

**Step 1: Write the failing test for RoutingRuleConfig.triggers**

Add to `core/src/config/types/routing.rs` at the end of the tests module:

```rust
#[test]
fn test_routing_rule_with_triggers() {
    let toml = r#"
        regex = "^/translate"
        hint = "翻译文本"
        triggers = ["翻译", "translate", "转换语言"]
    "#;
    let rule: RoutingRuleConfig = toml::from_str(toml).unwrap();
    assert_eq!(rule.triggers, Some(vec!["翻译".to_string(), "translate".to_string(), "转换语言".to_string()]));
}
```

**Step 2: Run test to verify it fails**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo test test_routing_rule_with_triggers -- --nocapture`
Expected: FAIL with "unknown field `triggers`"

**Step 3: Add triggers field to RoutingRuleConfig**

In `core/src/config/types/routing.rs`, after line 128 (after `hint` field), add:

```rust
    // ===== Natural Language Detection fields =====
    /// Trigger keywords for natural language command detection
    /// When user input contains any of these keywords, this command may be auto-invoked.
    /// Example: triggers = ["翻译", "translate", "转换语言"]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub triggers: Option<Vec<String>>,
```

**Step 4: Run test to verify it passes**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo test test_routing_rule_with_triggers -- --nocapture`
Expected: PASS

**Step 5: Add triggers field to McpServerConfig**

In `core/src/config/types/tools.rs`, after line 589 (after `enabled` field), add:

```rust
    /// Trigger keywords for natural language command detection
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub triggers: Option<Vec<String>>,
```

**Step 6: Add triggers field to SkillFrontmatter**

In `core/src/skills/mod.rs`, after line 48 (after `allowed_tools` field), add:

```rust
    /// Trigger keywords for natural language command detection
    /// When user input contains any of these keywords, this skill may be auto-invoked.
    #[serde(default)]
    pub triggers: Vec<String>,
```

**Step 7: Run full test suite for affected modules**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo test config:: && cargo test skills::`
Expected: All PASS

**Step 8: Commit**

```bash
git add core/src/config/types/routing.rs core/src/config/types/tools.rs core/src/skills/mod.rs
git commit -m "feat(config): add triggers field for natural language command detection"
```

---

## Task 2: Create CommandTriggers Type

**Files:**
- Modify: `core/src/command/types.rs`

**Step 1: Write the failing test**

Add to `core/src/command/types.rs` tests module:

```rust
#[test]
fn test_command_triggers_creation() {
    let triggers = CommandTriggers::new(
        vec!["知识图谱".to_string(), "graph".to_string()],
        vec!["generate".to_string(), "analyze".to_string()],
    );
    assert_eq!(triggers.manual.len(), 2);
    assert_eq!(triggers.auto_extracted.len(), 2);
    assert!(triggers.has_triggers());
}

#[test]
fn test_command_triggers_empty() {
    let triggers = CommandTriggers::empty();
    assert!(triggers.manual.is_empty());
    assert!(triggers.auto_extracted.is_empty());
    assert!(!triggers.has_triggers());
}
```

**Step 2: Run test to verify it fails**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo test test_command_triggers -- --nocapture`
Expected: FAIL with "cannot find type `CommandTriggers`"

**Step 3: Implement CommandTriggers**

Add to `core/src/command/types.rs` before the tests module:

```rust
/// Trigger keywords for natural language command detection
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CommandTriggers {
    /// Manually defined trigger keywords (weight: 1.0)
    pub manual: Vec<String>,
    /// Auto-extracted from description (weight: 0.6)
    pub auto_extracted: Vec<String>,
}

impl CommandTriggers {
    /// Create new triggers with manual and auto-extracted keywords
    pub fn new(manual: Vec<String>, auto_extracted: Vec<String>) -> Self {
        Self { manual, auto_extracted }
    }

    /// Create empty triggers
    pub fn empty() -> Self {
        Self::default()
    }

    /// Create triggers from manual keywords only
    pub fn from_manual(manual: Vec<String>) -> Self {
        Self { manual, auto_extracted: Vec::new() }
    }

    /// Check if there are any triggers
    pub fn has_triggers(&self) -> bool {
        !self.manual.is_empty() || !self.auto_extracted.is_empty()
    }

    /// Get total trigger count
    pub fn len(&self) -> usize {
        self.manual.len() + self.auto_extracted.len()
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.manual.is_empty() && self.auto_extracted.is_empty()
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo test test_command_triggers -- --nocapture`
Expected: PASS

**Step 5: Export CommandTriggers from mod.rs**

In `core/src/command/mod.rs`, update the exports:

```rust
pub use types::{CommandExecutionResult, CommandNode, CommandType, CommandTriggers};
```

**Step 6: Commit**

```bash
git add core/src/command/types.rs core/src/command/mod.rs
git commit -m "feat(command): add CommandTriggers type for NL detection"
```

---

## Task 3: Create Unified Command Index

**Files:**
- Create: `core/src/command/unified_index.rs`
- Modify: `core/src/command/mod.rs`

**Step 1: Write the failing test**

Create `core/src/command/unified_index.rs`:

```rust
//! Unified Command Index
//!
//! Aggregates trigger keywords from all command sources (Skills, MCP, Custom)
//! for natural language command detection.

use crate::command::CommandTriggers;
use crate::dispatcher::ToolSourceType;
use std::collections::HashMap;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_index_entry_creation() {
        let entry = IndexEntry::new(
            ToolSourceType::Skill,
            "knowledge-graph",
            1.0,
        );
        assert_eq!(entry.source_type, ToolSourceType::Skill);
        assert_eq!(entry.command_name, "knowledge-graph");
        assert_eq!(entry.weight, 1.0);
    }

    #[test]
    fn test_unified_index_empty() {
        let index = UnifiedCommandIndex::new();
        assert!(index.is_empty());
        assert_eq!(index.len(), 0);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo test unified_index -- --nocapture`
Expected: FAIL (module not found or types undefined)

**Step 3: Implement basic types**

Add to `core/src/command/unified_index.rs`:

```rust
/// An entry in the unified command index
#[derive(Debug, Clone, PartialEq)]
pub struct IndexEntry {
    /// Command source type
    pub source_type: ToolSourceType,
    /// Command name (ID)
    pub command_name: String,
    /// Weight (1.0 for manual triggers, 0.6 for auto-extracted)
    pub weight: f64,
}

impl IndexEntry {
    /// Create a new index entry
    pub fn new(source_type: ToolSourceType, command_name: impl Into<String>, weight: f64) -> Self {
        Self {
            source_type,
            command_name: command_name.into(),
            weight,
        }
    }
}

/// Unified command index for natural language detection
#[derive(Debug, Default)]
pub struct UnifiedCommandIndex {
    /// Map from trigger keyword (lowercase) to matching entries
    entries: HashMap<String, Vec<IndexEntry>>,
}

impl UnifiedCommandIndex {
    /// Create a new empty index
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if the index is empty
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Get number of unique trigger keywords
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo test unified_index::tests -- --nocapture`
Expected: PASS

**Step 5: Update mod.rs to include new module**

In `core/src/command/mod.rs`, add:

```rust
mod unified_index;

pub use unified_index::{IndexEntry, UnifiedCommandIndex};
```

**Step 6: Commit**

```bash
git add core/src/command/unified_index.rs core/src/command/mod.rs
git commit -m "feat(command): add UnifiedCommandIndex skeleton"
```

---

## Task 4: Implement UnifiedCommandIndex.add_command

**Files:**
- Modify: `core/src/command/unified_index.rs`

**Step 1: Write the failing test**

Add to tests module in `unified_index.rs`:

```rust
#[test]
fn test_add_command_with_triggers() {
    let mut index = UnifiedCommandIndex::new();
    let triggers = CommandTriggers::new(
        vec!["知识图谱".to_string(), "graph".to_string()],
        vec!["dependencies".to_string()],
    );

    index.add_command(ToolSourceType::Skill, "knowledge-graph", &triggers);

    assert!(!index.is_empty());
    assert_eq!(index.len(), 3); // 3 unique triggers
}
```

**Step 2: Run test to verify it fails**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo test test_add_command_with_triggers -- --nocapture`
Expected: FAIL with "no method named `add_command`"

**Step 3: Implement add_command**

Add to `UnifiedCommandIndex` impl:

```rust
    /// Add a command with its triggers to the index
    pub fn add_command(
        &mut self,
        source_type: ToolSourceType,
        command_name: &str,
        triggers: &CommandTriggers,
    ) {
        // Add manual triggers with weight 1.0
        for trigger in &triggers.manual {
            let key = trigger.to_lowercase();
            self.entries
                .entry(key)
                .or_default()
                .push(IndexEntry::new(source_type, command_name, 1.0));
        }

        // Add auto-extracted triggers with weight 0.6
        for trigger in &triggers.auto_extracted {
            let key = trigger.to_lowercase();
            self.entries
                .entry(key)
                .or_default()
                .push(IndexEntry::new(source_type, command_name, 0.6));
        }
    }
```

**Step 4: Run test to verify it passes**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo test test_add_command_with_triggers -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/command/unified_index.rs
git commit -m "feat(command): implement UnifiedCommandIndex.add_command"
```

---

## Task 5: Implement Keyword Extraction from Description

**Files:**
- Modify: `core/src/command/unified_index.rs`

**Step 1: Write the failing test**

Add to tests module:

```rust
#[test]
fn test_extract_keywords_english() {
    let keywords = extract_keywords_from_description("Generate knowledge graphs and analyze dependencies");
    assert!(keywords.contains(&"generate".to_string()));
    assert!(keywords.contains(&"knowledge".to_string()));
    assert!(keywords.contains(&"graphs".to_string()));
    assert!(keywords.contains(&"analyze".to_string()));
    assert!(keywords.contains(&"dependencies".to_string()));
    // Should not contain stop words
    assert!(!keywords.contains(&"and".to_string()));
}

#[test]
fn test_extract_keywords_chinese() {
    let keywords = extract_keywords_from_description("生成知识图谱，分析代码依赖关系");
    assert!(keywords.contains(&"生成".to_string()));
    assert!(keywords.contains(&"知识图谱".to_string()));
    assert!(keywords.contains(&"分析".to_string()));
    assert!(keywords.contains(&"代码".to_string()));
    assert!(keywords.contains(&"依赖关系".to_string()));
    // Should not contain stop words
    assert!(!keywords.contains(&"的".to_string()));
}
```

**Step 2: Run test to verify it fails**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo test test_extract_keywords -- --nocapture`
Expected: FAIL with "cannot find function `extract_keywords_from_description`"

**Step 3: Implement extract_keywords_from_description**

Add before the impl block:

```rust
/// Stop words to exclude from auto-extraction
const STOP_WORDS: &[&str] = &[
    // English
    "the", "a", "an", "is", "are", "was", "were", "be", "been", "being",
    "to", "for", "and", "or", "but", "in", "on", "at", "by", "with",
    "from", "as", "of", "this", "that", "it", "its", "can", "will",
    // Chinese
    "的", "是", "和", "与", "用", "来", "可以", "进行", "这个", "那个",
    "一个", "在", "了", "有", "不", "也", "就", "都", "而", "及",
];

/// Extract keywords from a description string
pub fn extract_keywords_from_description(description: &str) -> Vec<String> {
    description
        .split(|c: char| {
            c.is_whitespace()
            || c == ','
            || c == '，'
            || c == '。'
            || c == '.'
            || c == ';'
            || c == '；'
            || c == '、'
        })
        .map(|s| s.trim())
        .filter(|w| w.len() >= 2) // At least 2 bytes (1 CJK char = 3 bytes, so this allows single CJK)
        .filter(|w| w.chars().count() >= 2 || w.chars().any(|c| c > '\u{4E00}')) // 2+ chars or has CJK
        .filter(|w| !STOP_WORDS.contains(&w.to_lowercase().as_str()))
        .map(|w| w.to_lowercase())
        .collect()
}
```

**Step 4: Run test to verify it passes**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo test test_extract_keywords -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/command/unified_index.rs
git commit -m "feat(command): implement keyword extraction from description"
```

---

## Task 6: Implement UnifiedCommandIndex.find_matches

**Files:**
- Modify: `core/src/command/unified_index.rs`

**Step 1: Write the failing test**

Add to tests module:

```rust
#[test]
fn test_find_matches_basic() {
    let mut index = UnifiedCommandIndex::new();

    // Add a skill with triggers
    let triggers1 = CommandTriggers::new(
        vec!["知识图谱".to_string(), "graph".to_string()],
        vec!["dependencies".to_string()],
    );
    index.add_command(ToolSourceType::Skill, "knowledge-graph", &triggers1);

    // Add another command
    let triggers2 = CommandTriggers::new(
        vec!["翻译".to_string(), "translate".to_string()],
        Vec::new(),
    );
    index.add_command(ToolSourceType::Custom, "translate", &triggers2);

    // Test finding matches
    let matches = index.find_matches("帮我画个知识图谱");
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].command_name, "knowledge-graph");

    let matches2 = index.find_matches("translate this to English");
    assert_eq!(matches2.len(), 1);
    assert_eq!(matches2[0].command_name, "translate");
}

#[test]
fn test_find_matches_priority_sorting() {
    let mut index = UnifiedCommandIndex::new();

    // Two commands with overlapping triggers
    let triggers1 = CommandTriggers::new(vec!["analyze".to_string()], Vec::new());
    index.add_command(ToolSourceType::Skill, "skill-analyze", &triggers1);

    let triggers2 = CommandTriggers::new(vec!["analyze".to_string()], Vec::new());
    index.add_command(ToolSourceType::Custom, "custom-analyze", &triggers2);

    // Skill should come before Custom due to type priority
    let matches = index.find_matches("analyze this code");
    assert!(matches.len() >= 2);
    assert_eq!(matches[0].source_type, ToolSourceType::Skill);
}
```

**Step 2: Run test to verify it fails**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo test test_find_matches -- --nocapture`
Expected: FAIL with "no method named `find_matches`"

**Step 3: Implement ScoredMatch and find_matches**

Add to the file:

```rust
/// A scored match result
#[derive(Debug, Clone, PartialEq)]
pub struct ScoredMatch {
    /// Command source type
    pub source_type: ToolSourceType,
    /// Command name
    pub command_name: String,
    /// Match score (higher = better match)
    pub score: f64,
}

impl ScoredMatch {
    /// Get type priority for sorting (lower = higher priority)
    fn type_priority(&self) -> u8 {
        match self.source_type {
            ToolSourceType::Builtin | ToolSourceType::Native => 0,
            ToolSourceType::Skill => 1,
            ToolSourceType::Mcp => 2,
            ToolSourceType::Custom => 3,
        }
    }
}
```

Add to `UnifiedCommandIndex` impl:

```rust
    /// Find commands matching the input text
    /// Returns matches sorted by: type priority (asc), then score (desc)
    pub fn find_matches(&self, input: &str) -> Vec<ScoredMatch> {
        let input_lower = input.to_lowercase();
        let mut scores: HashMap<String, ScoredMatch> = HashMap::new();

        // Check each trigger keyword
        for (trigger, entries) in &self.entries {
            if input_lower.contains(trigger) {
                for entry in entries {
                    let key = format!("{}:{}", entry.source_type, entry.command_name);
                    scores
                        .entry(key)
                        .and_modify(|m| m.score += entry.weight)
                        .or_insert_with(|| ScoredMatch {
                            source_type: entry.source_type,
                            command_name: entry.command_name.clone(),
                            score: entry.weight,
                        });
                }
            }
        }

        // Sort by type priority (asc), then score (desc)
        let mut matches: Vec<ScoredMatch> = scores.into_values().collect();
        matches.sort_by(|a, b| {
            a.type_priority()
                .cmp(&b.type_priority())
                .then(b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal))
        });

        matches
    }
```

**Step 4: Run test to verify it passes**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo test test_find_matches -- --nocapture`
Expected: PASS

**Step 5: Export ScoredMatch**

Update `core/src/command/mod.rs`:

```rust
pub use unified_index::{IndexEntry, ScoredMatch, UnifiedCommandIndex, extract_keywords_from_description};
```

**Step 6: Commit**

```bash
git add core/src/command/unified_index.rs core/src/command/mod.rs
git commit -m "feat(command): implement UnifiedCommandIndex.find_matches with priority sorting"
```

---

## Task 7: Create Natural Language Detector - Explicit Patterns

**Files:**
- Create: `core/src/command/nl_detector.rs`
- Modify: `core/src/command/mod.rs`

**Step 1: Write the failing test**

Create `core/src/command/nl_detector.rs`:

```rust
//! Natural Language Command Detector
//!
//! Detects command invocations from natural language input:
//! - L1: Explicit mention (e.g., "使用 X", "use X to")
//! - L2: Implicit intent (keyword matching via UnifiedCommandIndex)

use once_cell::sync::Lazy;
use regex::Regex;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_explicit_pattern_chinese_use() {
        let result = extract_explicit_command("使用 knowledge-graph 分析代码");
        assert_eq!(result, Some(("knowledge-graph".to_string(), Some("分析代码".to_string()))));
    }

    #[test]
    fn test_explicit_pattern_chinese_use_short() {
        let result = extract_explicit_command("用 translate 翻译这段话");
        assert_eq!(result, Some(("translate".to_string(), Some("翻译这段话".to_string()))));
    }

    #[test]
    fn test_explicit_pattern_english_use() {
        let result = extract_explicit_command("use knowledge-graph to analyze dependencies");
        assert_eq!(result, Some(("knowledge-graph".to_string(), Some("analyze dependencies".to_string()))));
    }

    #[test]
    fn test_explicit_pattern_english_invoke() {
        let result = extract_explicit_command("invoke translator for this text");
        assert_eq!(result, Some(("translator".to_string(), Some("this text".to_string()))));
    }

    #[test]
    fn test_explicit_pattern_no_match() {
        let result = extract_explicit_command("帮我分析一下这段代码");
        assert_eq!(result, None);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo test nl_detector::tests -- --nocapture`
Expected: FAIL with "cannot find function `extract_explicit_command`"

**Step 3: Implement explicit patterns and extraction**

Add before tests module:

```rust
/// Explicit command mention patterns
/// Group 1: verb, Group 2: command name
static EXPLICIT_PATTERNS: Lazy<Vec<(Regex, usize)>> = Lazy::new(|| {
    vec![
        // Chinese: 使用/用/调用/执行/运行 X ...
        (Regex::new(r"(?i)^(使用|用|调用|执行|运行)\s*[「「\[]?([a-zA-Z0-9_-]+)[」」\]]?\s*(.*)$").unwrap(), 2),

        // Chinese: 让/交给 X 来/处理/做
        (Regex::new(r"(?i)(让|交给)\s*[「「\[]?([a-zA-Z0-9_-]+)[」」\]]?\s*(来|处理|做|帮)(.*)$").unwrap(), 2),

        // English: use/invoke/call/run/execute X to/for ...
        (Regex::new(r"(?i)^(use|invoke|call|run|execute)\s+([a-zA-Z0-9_-]+)\s+(to\s+|for\s+)?(.*)$").unwrap(), 2),

        // English: ask/let X to ...
        (Regex::new(r"(?i)(ask|let)\s+([a-zA-Z0-9_-]+)\s+(to\s+)(.*)$").unwrap(), 2),

        // English: with/using X, ...
        (Regex::new(r"(?i)(with|using)\s+([a-zA-Z0-9_-]+)[,\s]+(.*)$").unwrap(), 2),
    ]
});

/// Extract command name from explicit mention patterns
/// Returns (command_name, remaining_input) if matched
pub fn extract_explicit_command(input: &str) -> Option<(String, Option<String>)> {
    let trimmed = input.trim();

    for (pattern, cmd_group) in EXPLICIT_PATTERNS.iter() {
        if let Some(captures) = pattern.captures(trimmed) {
            let command_name = captures.get(*cmd_group)?.as_str().to_string();

            // Get remaining input (last capture group typically)
            let remaining = captures
                .get(captures.len() - 1)
                .map(|m| m.as_str().trim())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string());

            return Some((command_name, remaining));
        }
    }

    None
}
```

**Step 4: Run test to verify it passes**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo test nl_detector::tests -- --nocapture`
Expected: PASS

**Step 5: Update mod.rs**

Add to `core/src/command/mod.rs`:

```rust
mod nl_detector;

pub use nl_detector::extract_explicit_command;
```

**Step 6: Commit**

```bash
git add core/src/command/nl_detector.rs core/src/command/mod.rs
git commit -m "feat(command): add explicit command pattern detection (L1)"
```

---

## Task 8: Implement NaturalLanguageCommandDetector

**Files:**
- Modify: `core/src/command/nl_detector.rs`

**Step 1: Write the failing test**

Add to tests module:

```rust
use crate::command::{CommandTriggers, UnifiedCommandIndex};
use crate::dispatcher::ToolSourceType;

#[test]
fn test_nl_detector_explicit() {
    let mut index = UnifiedCommandIndex::new();
    let triggers = CommandTriggers::new(vec!["graph".to_string()], Vec::new());
    index.add_command(ToolSourceType::Skill, "knowledge-graph", &triggers);

    let detector = NaturalLanguageCommandDetector::new(index);

    let result = detector.detect("使用 knowledge-graph 分析代码");
    assert!(result.is_some());
    let detection = result.unwrap();
    assert_eq!(detection.command_name, "knowledge-graph");
    assert_eq!(detection.detection_type, DetectionType::Explicit);
    assert_eq!(detection.confidence, 1.0);
}

#[test]
fn test_nl_detector_implicit() {
    let mut index = UnifiedCommandIndex::new();
    let triggers = CommandTriggers::new(vec!["知识图谱".to_string()], Vec::new());
    index.add_command(ToolSourceType::Skill, "knowledge-graph", &triggers);

    let detector = NaturalLanguageCommandDetector::new(index);

    let result = detector.detect("帮我画个知识图谱");
    assert!(result.is_some());
    let detection = result.unwrap();
    assert_eq!(detection.command_name, "knowledge-graph");
    assert_eq!(detection.detection_type, DetectionType::Implicit);
}

#[test]
fn test_nl_detector_no_match() {
    let index = UnifiedCommandIndex::new();
    let detector = NaturalLanguageCommandDetector::new(index);

    let result = detector.detect("今天天气怎么样");
    assert!(result.is_none());
}
```

**Step 2: Run test to verify it fails**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo test test_nl_detector -- --nocapture`
Expected: FAIL with "cannot find type `NaturalLanguageCommandDetector`"

**Step 3: Implement NaturalLanguageCommandDetector**

Add to `nl_detector.rs`:

```rust
use crate::command::unified_index::{ScoredMatch, UnifiedCommandIndex};
use crate::dispatcher::ToolSourceType;

/// Detection type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectionType {
    /// Explicit mention (e.g., "使用 X", "use X")
    Explicit,
    /// Implicit intent (keyword matching)
    Implicit,
}

/// Detection result
#[derive(Debug, Clone, PartialEq)]
pub struct NLDetection {
    /// Command name that was detected
    pub command_name: String,
    /// Source type of the command
    pub source_type: ToolSourceType,
    /// How it was detected
    pub detection_type: DetectionType,
    /// Confidence score (0.0 - 1.0)
    pub confidence: f64,
    /// Remaining input after command extraction (for explicit)
    pub remaining_input: Option<String>,
}

/// Natural language command detector
pub struct NaturalLanguageCommandDetector {
    /// Unified command index for lookups
    index: UnifiedCommandIndex,
    /// Minimum confidence threshold for implicit detection
    min_confidence: f64,
}

impl NaturalLanguageCommandDetector {
    /// Create a new detector with the given index
    pub fn new(index: UnifiedCommandIndex) -> Self {
        Self {
            index,
            min_confidence: 0.5,
        }
    }

    /// Set minimum confidence threshold for implicit detection
    pub fn with_min_confidence(mut self, threshold: f64) -> Self {
        self.min_confidence = threshold;
        self
    }

    /// Detect command from natural language input
    pub fn detect(&self, input: &str) -> Option<NLDetection> {
        // L1: Try explicit detection first
        if let Some(detection) = self.detect_explicit(input) {
            return Some(detection);
        }

        // L2: Try implicit detection
        self.detect_implicit(input)
    }

    /// L1: Explicit command detection
    fn detect_explicit(&self, input: &str) -> Option<NLDetection> {
        let (command_name, remaining) = extract_explicit_command(input)?;

        // Verify command exists in index or is a known command
        let matches = self.index.find_matches(&command_name);

        // If exact match found, use it
        if let Some(m) = matches.iter().find(|m| m.command_name.eq_ignore_ascii_case(&command_name)) {
            return Some(NLDetection {
                command_name: m.command_name.clone(),
                source_type: m.source_type,
                detection_type: DetectionType::Explicit,
                confidence: 1.0,
                remaining_input: remaining,
            });
        }

        // Otherwise, return the command name as-is (let caller verify)
        Some(NLDetection {
            command_name,
            source_type: ToolSourceType::Custom, // Default, caller should verify
            detection_type: DetectionType::Explicit,
            confidence: 1.0,
            remaining_input: remaining,
        })
    }

    /// L2: Implicit intent detection
    fn detect_implicit(&self, input: &str) -> Option<NLDetection> {
        let matches = self.index.find_matches(input);

        // Get best match above threshold
        let best = matches.into_iter().next()?;

        // Normalize score
        let normalized_score = (best.score / 3.0).min(1.0); // Assume max 3 trigger matches

        if normalized_score >= self.min_confidence {
            Some(NLDetection {
                command_name: best.command_name,
                source_type: best.source_type,
                detection_type: DetectionType::Implicit,
                confidence: normalized_score,
                remaining_input: Some(input.to_string()),
            })
        } else {
            None
        }
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo test test_nl_detector -- --nocapture`
Expected: PASS

**Step 5: Export types from mod.rs**

Update exports in `core/src/command/mod.rs`:

```rust
pub use nl_detector::{extract_explicit_command, DetectionType, NLDetection, NaturalLanguageCommandDetector};
```

**Step 6: Commit**

```bash
git add core/src/command/nl_detector.rs core/src/command/mod.rs
git commit -m "feat(command): implement NaturalLanguageCommandDetector"
```

---

## Task 9: Integrate NL Detector into CommandParser

**Files:**
- Modify: `core/src/command/parser.rs`

**Step 1: Write the failing test**

Add to tests in `parser.rs`:

```rust
#[test]
fn test_parser_with_nl_detector_explicit() {
    use crate::command::{CommandTriggers, NaturalLanguageCommandDetector, UnifiedCommandIndex};

    let mut index = UnifiedCommandIndex::new();
    let triggers = CommandTriggers::new(vec!["graph".to_string()], Vec::new());
    index.add_command(ToolSourceType::Skill, "test-skill", &triggers);

    let detector = NaturalLanguageCommandDetector::new(index);
    let parser = CommandParser::new().with_nl_detector(detector);

    // Should detect "使用 test-skill" as a command
    let result = parser.parse("使用 test-skill 做点什么");
    assert!(result.is_some());
    let cmd = result.unwrap();
    assert_eq!(cmd.command_name, "test-skill");
}

#[test]
fn test_parser_slash_command_takes_precedence() {
    use crate::command::{NaturalLanguageCommandDetector, UnifiedCommandIndex};

    let index = UnifiedCommandIndex::new();
    let detector = NaturalLanguageCommandDetector::new(index);
    let parser = CommandParser::new().with_nl_detector(detector);

    // Slash commands should still work
    let result = parser.parse("/search weather");
    assert!(result.is_some());
    let cmd = result.unwrap();
    assert_eq!(cmd.command_name, "search");
    assert_eq!(cmd.source_type, ToolSourceType::Builtin);
}
```

**Step 2: Run test to verify it fails**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo test parser::tests::test_parser_with_nl -- --nocapture`
Expected: FAIL with "no method named `with_nl_detector`"

**Step 3: Add nl_detector field and with_nl_detector method**

In `parser.rs`, update `CommandParser` struct:

```rust
use crate::command::nl_detector::{NaturalLanguageCommandDetector, NLDetection, DetectionType};

pub struct CommandParser {
    // ... existing fields ...

    /// Natural language command detector
    nl_detector: Option<NaturalLanguageCommandDetector>,
}
```

Update `new()`:

```rust
pub fn new() -> Self {
    Self {
        command_registry: None,
        skills_registry: None,
        routing_rules: Vec::new(),
        mcp_server_names: Vec::new(),
        builtin_commands: vec!["agent", "search", "youtube", "webfetch"],
        nl_detector: None,
    }
}
```

Add builder method:

```rust
/// Set the natural language detector
pub fn with_nl_detector(mut self, detector: NaturalLanguageCommandDetector) -> Self {
    self.nl_detector = Some(detector);
    self
}
```

**Step 4: Update parse() method**

Modify the `parse()` method:

```rust
pub fn parse(&self, input: &str) -> Option<ParsedCommand> {
    let trimmed = input.trim();

    // 1. Slash commands take precedence
    if trimmed.starts_with('/') {
        return self.parse_slash_command(trimmed);
    }

    // 2. Try natural language detection
    if let Some(ref detector) = self.nl_detector {
        if let Some(detection) = detector.detect(trimmed) {
            return self.create_command_from_detection(detection, trimmed);
        }
    }

    None
}

/// Parse a slash command (original logic, refactored)
fn parse_slash_command(&self, input: &str) -> Option<ParsedCommand> {
    let without_slash = &input[1..];
    let (command_name, arguments) = self.extract_parts(without_slash);

    if command_name.is_empty() {
        return None;
    }

    // ... rest of existing parse logic ...
    // (keep all the existing matching logic here)
}

/// Create ParsedCommand from NL detection result
fn create_command_from_detection(&self, detection: NLDetection, original_input: &str) -> Option<ParsedCommand> {
    let arguments = detection.remaining_input.clone();

    // Try to find the command in registries to get full context
    // First check skills
    if let Some(ref skills_registry) = self.skills_registry {
        if let Some(skill) = skills_registry.get_skill(&detection.command_name) {
            return Some(ParsedCommand {
                source_type: ToolSourceType::Skill,
                command_name: detection.command_name,
                arguments,
                full_input: original_input.to_string(),
                context: CommandContext::Skill {
                    skill_id: skill.id.clone(),
                    instructions: skill.instructions.clone(),
                    display_name: skill.frontmatter.name.clone(),
                },
            });
        }
    }

    // Check MCP servers
    if self.mcp_server_names.contains(&detection.command_name) {
        return Some(ParsedCommand {
            source_type: ToolSourceType::Mcp,
            command_name: detection.command_name.clone(),
            arguments,
            full_input: original_input.to_string(),
            context: CommandContext::Mcp {
                server_name: detection.command_name,
                tool_name: None,
            },
        });
    }

    // Check custom rules
    if let Some(rule) = self.find_matching_rule(&detection.command_name) {
        return Some(ParsedCommand {
            source_type: ToolSourceType::Custom,
            command_name: detection.command_name,
            arguments,
            full_input: original_input.to_string(),
            context: CommandContext::Custom {
                system_prompt: rule.system_prompt.clone(),
                provider: rule.provider.clone(),
                pattern: rule.regex.clone(),
            },
        });
    }

    // Check builtins
    if self.builtin_commands.contains(&detection.command_name.as_str()) {
        return Some(ParsedCommand {
            source_type: ToolSourceType::Builtin,
            command_name: detection.command_name.clone(),
            arguments,
            full_input: original_input.to_string(),
            context: CommandContext::Builtin {
                tool_name: detection.command_name,
            },
        });
    }

    // If explicit detection but command not found, return None
    // For implicit detection, we already verified via index
    if detection.detection_type == DetectionType::Explicit {
        return None;
    }

    // Fallback: use detected source type
    Some(ParsedCommand {
        source_type: detection.source_type,
        command_name: detection.command_name,
        arguments,
        full_input: original_input.to_string(),
        context: CommandContext::None,
    })
}
```

**Step 5: Run test to verify it passes**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo test parser::tests -- --nocapture`
Expected: PASS

**Step 6: Commit**

```bash
git add core/src/command/parser.rs
git commit -m "feat(command): integrate NL detector into CommandParser"
```

---

## Task 10: Build Index from Registries

**Files:**
- Modify: `core/src/command/unified_index.rs`

**Step 1: Write the failing test**

Add to tests:

```rust
#[test]
fn test_build_from_skills() {
    use crate::skills::SkillsRegistry;
    use tempfile::TempDir;
    use std::fs;

    let temp_dir = TempDir::new().unwrap();
    let skills_dir = temp_dir.path().to_path_buf();

    // Create a test skill with triggers
    let skill_dir = skills_dir.join("test-skill");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        r#"---
name: test-skill
description: A test skill for testing
triggers:
  - test
  - 测试
---

# Test Skill
Instructions here.
"#,
    ).unwrap();

    let registry = SkillsRegistry::new(skills_dir);
    registry.load_all().unwrap();

    let index = UnifiedCommandIndex::build_from_skills(&registry);

    let matches = index.find_matches("run a test");
    assert!(!matches.is_empty());
    assert_eq!(matches[0].command_name, "test-skill");
}
```

**Step 2: Run test to verify it fails**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo test test_build_from_skills -- --nocapture`
Expected: FAIL with "no function `build_from_skills`"

**Step 3: Implement build_from_skills**

Add to `UnifiedCommandIndex` impl:

```rust
use crate::command::CommandTriggers;
use crate::skills::SkillsRegistry;

impl UnifiedCommandIndex {
    /// Build index from a SkillsRegistry
    pub fn build_from_skills(registry: &SkillsRegistry) -> Self {
        let mut index = Self::new();

        for skill in registry.list_skills() {
            let triggers = CommandTriggers::new(
                skill.frontmatter.triggers.clone(),
                extract_keywords_from_description(&skill.frontmatter.description),
            );
            index.add_command(ToolSourceType::Skill, &skill.id, &triggers);
        }

        index
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo test test_build_from_skills -- --nocapture`
Expected: PASS

**Step 5: Add build methods for MCP and Custom**

```rust
use crate::config::{McpServerConfig, RoutingRuleConfig};
use std::collections::HashMap;

impl UnifiedCommandIndex {
    /// Build index from MCP server configs
    pub fn build_from_mcp(configs: &HashMap<String, McpServerConfig>) -> Self {
        let mut index = Self::new();

        for (name, config) in configs {
            let manual = config.triggers.clone().unwrap_or_default();
            let triggers = CommandTriggers::new(manual, Vec::new());
            index.add_command(ToolSourceType::Mcp, name, &triggers);
        }

        index
    }

    /// Build index from routing rules (Custom commands)
    pub fn build_from_rules(rules: &[RoutingRuleConfig]) -> Self {
        let mut index = Self::new();

        for rule in rules {
            // Only process command rules (starting with ^/)
            if !rule.regex.starts_with("^/") {
                continue;
            }

            // Extract command name from regex
            let cmd_name = rule.regex
                .trim_start_matches("^/")
                .split(|c: char| !c.is_alphanumeric() && c != '-' && c != '_')
                .next()
                .unwrap_or_default();

            if cmd_name.is_empty() {
                continue;
            }

            let manual = rule.triggers.clone().unwrap_or_default();
            let auto = rule.hint.as_ref()
                .map(|h| extract_keywords_from_description(h))
                .unwrap_or_default();

            let triggers = CommandTriggers::new(manual, auto);
            index.add_command(ToolSourceType::Custom, cmd_name, &triggers);
        }

        index
    }

    /// Merge another index into this one
    pub fn merge(&mut self, other: Self) {
        for (trigger, entries) in other.entries {
            self.entries.entry(trigger).or_default().extend(entries);
        }
    }

    /// Build a complete index from all sources
    pub fn build_all(
        skills_registry: Option<&SkillsRegistry>,
        mcp_configs: Option<&HashMap<String, McpServerConfig>>,
        routing_rules: Option<&[RoutingRuleConfig]>,
    ) -> Self {
        let mut index = Self::new();

        if let Some(registry) = skills_registry {
            index.merge(Self::build_from_skills(registry));
        }

        if let Some(configs) = mcp_configs {
            index.merge(Self::build_from_mcp(configs));
        }

        if let Some(rules) = routing_rules {
            index.merge(Self::build_from_rules(rules));
        }

        index
    }
}
```

**Step 6: Commit**

```bash
git add core/src/command/unified_index.rs
git commit -m "feat(command): add UnifiedCommandIndex build methods for all sources"
```

---

## Task 11: Run Full Test Suite and Fix Any Issues

**Step 1: Run all command module tests**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo test command:: -- --nocapture`
Expected: All PASS

**Step 2: Run all affected module tests**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo test skills:: && cargo test config::`
Expected: All PASS

**Step 3: Run cargo check for any compile errors**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo check`
Expected: No errors

**Step 4: Run cargo clippy for any warnings**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo clippy -- -D warnings`
Expected: No warnings (fix any that appear)

**Step 5: Final commit**

```bash
git add -A
git commit -m "test(command): verify NL command detection implementation"
```

---

## Summary

| Task | Description | Est. Lines |
|------|-------------|-----------|
| 1 | Add triggers field to config types | ~30 |
| 2 | Create CommandTriggers type | ~50 |
| 3 | Create UnifiedCommandIndex skeleton | ~40 |
| 4 | Implement add_command | ~25 |
| 5 | Implement keyword extraction | ~40 |
| 6 | Implement find_matches | ~60 |
| 7 | Create NL detector explicit patterns | ~80 |
| 8 | Implement NaturalLanguageCommandDetector | ~120 |
| 9 | Integrate into CommandParser | ~100 |
| 10 | Build index from registries | ~80 |
| 11 | Full test suite verification | ~0 |

**Total:** ~625 lines of new/modified code
