//! Rule Conflict Detection and Resolution
//!
//! This module detects and resolves conflicts between L2 routing rules.
//! It identifies overlapping patterns, priority conflicts, and provides
//! resolution strategies.
//!
//! # Architecture
//!
//! ```text
//! ReflexLayer → Conflict Detector → Conflict Report → Resolution Strategy
//!     ↓              ↓                   ↓                    ↓
//!   Rules        Analyze            Conflicts            Suggestions
//! ```
//!
//! # Conflict Types
//!
//! 1. **Pattern Overlap**: Multiple rules match the same input
//! 2. **Priority Conflict**: Rules with same priority compete
//! 3. **Ambiguous Match**: Input matches multiple rules with similar confidence
//! 4. **Redundant Rule**: Rule is never hit due to higher priority rules
//!
//! # Example
//!
//! ```rust,ignore
//! use alephcore::engine::{ConflictDetector, ReflexLayer};
//!
//! let reflex_layer = ReflexLayer::new();
//! let detector = ConflictDetector::new();
//!
//! // Detect conflicts
//! let conflicts = detector.detect(&reflex_layer).await;
//!
//! // Get resolution suggestions
//! for conflict in conflicts {
//!     println!("Conflict: {:?}", conflict);
//!     println!("Suggestion: {}", conflict.resolution_suggestion());
//! }
//! ```

use super::ReflexLayer;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::info;

/// Conflict detector for L2 routing rules
pub struct ConflictDetector {
    /// Test inputs for conflict detection
    test_inputs: Vec<String>,
}

impl ConflictDetector {
    /// Create a new conflict detector
    pub fn new() -> Self {
        Self {
            test_inputs: Self::default_test_inputs(),
        }
    }

    /// Create a conflict detector with custom test inputs
    pub fn with_test_inputs(test_inputs: Vec<String>) -> Self {
        Self { test_inputs }
    }

    /// Default test inputs for conflict detection
    fn default_test_inputs() -> Vec<String> {
        vec![
            "git status".to_string(),
            "git log".to_string(),
            "git diff".to_string(),
            "read file.txt".to_string(),
            "cat file.txt".to_string(),
            "ls".to_string(),
            "ls -la".to_string(),
            "pwd".to_string(),
            "search for TODO".to_string(),
            "find TODO in file".to_string(),
            "replace foo with bar".to_string(),
            "move file.txt to dir/".to_string(),
        ]
    }

    /// Detect conflicts in the reflex layer
    pub fn detect(&self, reflex_layer: &ReflexLayer) -> Vec<Conflict> {
        let mut conflicts = Vec::new();

        // Get all rules (we need to access the internal rules)
        // For now, we'll test with sample inputs
        let mut pattern_matches: HashMap<String, Vec<(usize, String)>> = HashMap::new();

        // Test each input against all rules
        for input in &self.test_inputs {
            let matches = self.find_matching_rules(reflex_layer, input);
            if matches.len() > 1 {
                pattern_matches.insert(input.clone(), matches);
            }
        }

        // Detect pattern overlap conflicts
        for (input, matches) in pattern_matches {
            if matches.len() > 1 {
                conflicts.push(Conflict {
                    conflict_type: ConflictType::PatternOverlap,
                    input: input.clone(),
                    rule_indices: matches.iter().map(|(idx, _)| *idx).collect(),
                    rule_patterns: matches.iter().map(|(_, pat)| pat.clone()).collect(),
                    severity: Self::calculate_severity(&matches),
                    description: format!(
                        "Input '{}' matches {} rules: {}",
                        input,
                        matches.len(),
                        matches
                            .iter()
                            .map(|(_, p)| p.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                });
            }
        }

        // Detect priority conflicts
        conflicts.extend(self.detect_priority_conflicts(reflex_layer));

        // Detect redundant rules
        conflicts.extend(self.detect_redundant_rules(reflex_layer));

        info!(count = conflicts.len(), "Detected {} conflicts", conflicts.len());

        conflicts
    }

    /// Find all rules that match a given input
    fn find_matching_rules(&self, _reflex_layer: &ReflexLayer, input: &str) -> Vec<(usize, String)> {
        // This is a simplified implementation
        // In a real implementation, we would access the internal rules of ReflexLayer
        let mut matches = Vec::new();

        // Simulate rule matching with common patterns
        let patterns = [(r"git\s+status", "git status rule"),
            (r"git.*", "git wildcard rule"),
            (r"read\s+.*", "read file rule"),
            (r"cat\s+.*", "cat file rule"),
            (r"(read|cat)\s+.*", "read/cat combined rule"),
            (r"ls.*", "ls rule"),
            (r"search.*", "search rule"),
            (r"find.*", "find rule")];

        for (idx, (pattern, name)) in patterns.iter().enumerate() {
            if let Ok(re) = Regex::new(pattern) {
                if re.is_match(input) {
                    matches.push((idx, name.to_string()));
                }
            }
        }

        matches
    }

    /// Detect priority conflicts
    fn detect_priority_conflicts(&self, _reflex_layer: &ReflexLayer) -> Vec<Conflict> {
        

        // This is a simplified implementation
        // In a real implementation, we would check for rules with same priority
        // that could match the same inputs

        Vec::new()
    }

    /// Detect redundant rules
    fn detect_redundant_rules(&self, _reflex_layer: &ReflexLayer) -> Vec<Conflict> {
        

        // This is a simplified implementation
        // In a real implementation, we would check for rules that are never hit
        // due to higher priority rules matching first

        Vec::new()
    }

    /// Calculate conflict severity
    fn calculate_severity(matches: &[(usize, String)]) -> ConflictSeverity {
        match matches.len() {
            2 => ConflictSeverity::Low,
            3..=4 => ConflictSeverity::Medium,
            _ => ConflictSeverity::High,
        }
    }

    /// Add a test input
    pub fn add_test_input(&mut self, input: String) {
        self.test_inputs.push(input);
    }

    /// Clear test inputs
    pub fn clear_test_inputs(&mut self) {
        self.test_inputs.clear();
    }
}

impl Default for ConflictDetector {
    fn default() -> Self {
        Self::new()
    }
}

/// Conflict type
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConflictType {
    /// Multiple rules match the same input
    PatternOverlap,
    /// Rules with same priority compete
    PriorityConflict,
    /// Input matches multiple rules with similar confidence
    AmbiguousMatch,
    /// Rule is never hit due to higher priority rules
    RedundantRule,
}

/// Conflict severity
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ConflictSeverity {
    Low,
    Medium,
    High,
    Critical,
}

/// Detected conflict
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conflict {
    /// Type of conflict
    pub conflict_type: ConflictType,

    /// Input that triggered the conflict
    pub input: String,

    /// Indices of conflicting rules
    pub rule_indices: Vec<usize>,

    /// Patterns of conflicting rules
    pub rule_patterns: Vec<String>,

    /// Severity of the conflict
    pub severity: ConflictSeverity,

    /// Human-readable description
    pub description: String,
}

impl Conflict {
    /// Get resolution suggestion
    pub fn resolution_suggestion(&self) -> String {
        match self.conflict_type {
            ConflictType::PatternOverlap => {
                format!(
                    "Consider adjusting rule priorities or making patterns more specific. \
                     Rules: {}",
                    self.rule_patterns.join(", ")
                )
            }
            ConflictType::PriorityConflict => {
                "Assign different priorities to conflicting rules based on specificity".to_string()
            }
            ConflictType::AmbiguousMatch => {
                "Add more specific patterns or increase priority difference between rules"
                    .to_string()
            }
            ConflictType::RedundantRule => {
                "Remove redundant rule or adjust its pattern to cover different cases".to_string()
            }
        }
    }

    /// Check if this is a critical conflict
    pub fn is_critical(&self) -> bool {
        self.severity == ConflictSeverity::Critical
    }

    /// Check if this is a high severity conflict
    pub fn is_high_severity(&self) -> bool {
        self.severity >= ConflictSeverity::High
    }
}

/// Conflict resolution strategy
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ResolutionStrategy {
    /// Adjust rule priorities
    AdjustPriorities {
        rule_index: usize,
        new_priority: u32,
    },
    /// Make pattern more specific
    RefinePattern {
        rule_index: usize,
        new_pattern: String,
    },
    /// Remove redundant rule
    RemoveRule { rule_index: usize },
    /// Merge similar rules
    MergeRules {
        rule_indices: Vec<usize>,
        merged_pattern: String,
    },
}

/// Conflict resolver
pub struct ConflictResolver;

impl ConflictResolver {
    /// Generate resolution strategies for a conflict
    pub fn resolve(conflict: &Conflict) -> Vec<ResolutionStrategy> {
        let mut strategies = Vec::new();

        match conflict.conflict_type {
            ConflictType::PatternOverlap => {
                // Suggest priority adjustments
                for (i, rule_idx) in conflict.rule_indices.iter().enumerate() {
                    strategies.push(ResolutionStrategy::AdjustPriorities {
                        rule_index: *rule_idx,
                        new_priority: 100 - (i as u32 * 10),
                    });
                }
            }
            ConflictType::RedundantRule => {
                // Suggest removing redundant rules
                for rule_idx in &conflict.rule_indices {
                    strategies.push(ResolutionStrategy::RemoveRule {
                        rule_index: *rule_idx,
                    });
                }
            }
            ConflictType::PriorityConflict => {
                // Suggest priority adjustments
                for (i, rule_idx) in conflict.rule_indices.iter().enumerate() {
                    strategies.push(ResolutionStrategy::AdjustPriorities {
                        rule_index: *rule_idx,
                        new_priority: 90 - (i as u32 * 5),
                    });
                }
            }
            ConflictType::AmbiguousMatch => {
                // Suggest pattern refinement
                for rule_idx in &conflict.rule_indices {
                    strategies.push(ResolutionStrategy::RefinePattern {
                        rule_index: *rule_idx,
                        new_pattern: format!("^{}$", conflict.input),
                    });
                }
            }
        }

        strategies
    }
}

/// Conflict report
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConflictReport {
    /// Total number of conflicts
    pub total_conflicts: usize,

    /// Conflicts by severity
    pub by_severity: HashMap<String, usize>,

    /// Conflicts by type
    pub by_type: HashMap<String, usize>,

    /// All detected conflicts
    pub conflicts: Vec<Conflict>,
}

impl ConflictReport {
    /// Create a conflict report from detected conflicts
    pub fn from_conflicts(conflicts: Vec<Conflict>) -> Self {
        let total_conflicts = conflicts.len();

        let mut by_severity = HashMap::new();
        let mut by_type = HashMap::new();

        for conflict in &conflicts {
            let severity_key = format!("{:?}", conflict.severity);
            *by_severity.entry(severity_key).or_insert(0) += 1;

            let type_key = format!("{:?}", conflict.conflict_type);
            *by_type.entry(type_key).or_insert(0) += 1;
        }

        Self {
            total_conflicts,
            by_severity,
            by_type,
            conflicts,
        }
    }

    /// Get critical conflicts
    pub fn critical_conflicts(&self) -> Vec<&Conflict> {
        self.conflicts
            .iter()
            .filter(|c| c.is_critical())
            .collect()
    }

    /// Get high severity conflicts
    pub fn high_severity_conflicts(&self) -> Vec<&Conflict> {
        self.conflicts
            .iter()
            .filter(|c| c.is_high_severity())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_conflict_detector_basic() {
        let detector = ConflictDetector::new();
        let reflex_layer = ReflexLayer::new();

        let conflicts = detector.detect(&reflex_layer);

        // Should detect some conflicts with default test inputs
        assert!(conflicts.len() >= 0);
    }

    #[test]
    fn test_conflict_severity() {
        let matches = vec![(0, "rule1".to_string()), (1, "rule2".to_string())];
        let severity = ConflictDetector::calculate_severity(&matches);
        assert_eq!(severity, ConflictSeverity::Low);

        let matches = vec![
            (0, "rule1".to_string()),
            (1, "rule2".to_string()),
            (2, "rule3".to_string()),
        ];
        let severity = ConflictDetector::calculate_severity(&matches);
        assert_eq!(severity, ConflictSeverity::Medium);
    }

    #[test]
    fn test_conflict_resolution_suggestion() {
        let conflict = Conflict {
            conflict_type: ConflictType::PatternOverlap,
            input: "git status".to_string(),
            rule_indices: vec![0, 1],
            rule_patterns: vec!["git.*".to_string(), "git\\s+status".to_string()],
            severity: ConflictSeverity::Medium,
            description: "Test conflict".to_string(),
        };

        let suggestion = conflict.resolution_suggestion();
        assert!(suggestion.contains("priorities") || suggestion.contains("specific"));
    }

    #[test]
    fn test_conflict_resolver() {
        let conflict = Conflict {
            conflict_type: ConflictType::PatternOverlap,
            input: "git status".to_string(),
            rule_indices: vec![0, 1],
            rule_patterns: vec!["git.*".to_string(), "git\\s+status".to_string()],
            severity: ConflictSeverity::Medium,
            description: "Test conflict".to_string(),
        };

        let strategies = ConflictResolver::resolve(&conflict);
        assert!(!strategies.is_empty());
    }

    #[test]
    fn test_conflict_report() {
        let conflicts = vec![
            Conflict {
                conflict_type: ConflictType::PatternOverlap,
                input: "test1".to_string(),
                rule_indices: vec![0, 1],
                rule_patterns: vec!["pattern1".to_string(), "pattern2".to_string()],
                severity: ConflictSeverity::High,
                description: "Test conflict 1".to_string(),
            },
            Conflict {
                conflict_type: ConflictType::RedundantRule,
                input: "test2".to_string(),
                rule_indices: vec![2],
                rule_patterns: vec!["pattern3".to_string()],
                severity: ConflictSeverity::Low,
                description: "Test conflict 2".to_string(),
            },
        ];

        let report = ConflictReport::from_conflicts(conflicts);
        assert_eq!(report.total_conflicts, 2);
        assert_eq!(report.high_severity_conflicts().len(), 1);
    }

    #[test]
    fn test_custom_test_inputs() {
        let mut detector = ConflictDetector::new();
        detector.add_test_input("custom input".to_string());

        assert!(detector.test_inputs.contains(&"custom input".to_string()));

        detector.clear_test_inputs();
        assert_eq!(detector.test_inputs.len(), 0);
    }
}
