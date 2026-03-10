//! Decomposition detector for P-stage heuristic analysis.
//!
//! Analyzes a `SuccessManifest` to determine whether it should be
//! split into sub-tasks before execution begins.

use std::collections::HashSet;
use std::path::Path;

use crate::poe::types::{SuccessManifest, ValidationRule};

// ============================================================================
// DecompositionAdvice
// ============================================================================

/// Advice from the decomposition detector.
#[derive(Debug, Clone)]
pub enum DecompositionAdvice {
    /// Task is simple enough to execute directly.
    Proceed,

    /// Task should be decomposed into sub-tasks.
    Decompose {
        /// Suggested sub-objectives derived from analysis.
        sub_objectives: Vec<String>,
        /// Human-readable reason for decomposition.
        reason: String,
    },
}

impl DecompositionAdvice {
    /// Returns true if the advice is to decompose.
    pub fn should_decompose(&self) -> bool {
        matches!(self, DecompositionAdvice::Decompose { .. })
    }
}

// ============================================================================
// DecompositionDetector
// ============================================================================

/// Heuristic detector that analyzes a manifest to decide if decomposition is needed.
///
/// Current heuristics:
/// 1. Many constraints (>5) spanning 3+ distinct directories -> decompose
/// 2. Compound objective with distinct verb phrases -> decompose
pub struct DecompositionDetector;

impl DecompositionDetector {
    /// Analyze a manifest and return decomposition advice.
    pub fn analyze(manifest: &SuccessManifest) -> DecompositionAdvice {
        // Heuristic 1: Many constraints across many directories
        if let Some(advice) = Self::check_constraint_spread(manifest) {
            return advice;
        }

        // Heuristic 2: Compound objective with distinct verb phrases
        if let Some(advice) = Self::check_compound_objective(manifest) {
            return advice;
        }

        DecompositionAdvice::Proceed
    }

    /// Check if constraints span too many directories.
    ///
    /// Triggers when hard_constraints.len() > 5 AND paths touch 3+ distinct directories.
    fn check_constraint_spread(manifest: &SuccessManifest) -> Option<DecompositionAdvice> {
        if manifest.hard_constraints.len() <= 5 {
            return None;
        }

        let dirs = Self::extract_distinct_directories(&manifest.hard_constraints);
        if dirs.len() < 3 {
            return None;
        }

        // Group constraints by directory for sub-objective generation
        let sub_objectives: Vec<String> = dirs
            .iter()
            .map(|dir| format!("Handle constraints for {}", dir))
            .collect();

        Some(DecompositionAdvice::Decompose {
            sub_objectives,
            reason: format!(
                "Task has {} constraints spanning {} directories — too broad for single execution",
                manifest.hard_constraints.len(),
                dirs.len()
            ),
        })
    }

    /// Extract distinct parent directories from validation rules.
    fn extract_distinct_directories(rules: &[ValidationRule]) -> Vec<String> {
        let mut dirs = HashSet::new();

        for rule in rules {
            if let Some(path) = Self::extract_path(rule) {
                if let Some(parent) = Path::new(&path).parent() {
                    let dir_str = parent.to_string_lossy().to_string();
                    // Use "." for files at root level
                    if dir_str.is_empty() {
                        dirs.insert(".".to_string());
                    } else {
                        dirs.insert(dir_str);
                    }
                }
            }
        }

        let mut sorted: Vec<String> = dirs.into_iter().collect();
        sorted.sort();
        sorted
    }

    /// Extract the file path from a validation rule, if it has one.
    fn extract_path(rule: &ValidationRule) -> Option<String> {
        match rule {
            ValidationRule::FileExists { path } => Some(path.to_string_lossy().to_string()),
            ValidationRule::FileNotExists { path } => Some(path.to_string_lossy().to_string()),
            ValidationRule::FileContains { path, .. } => Some(path.to_string_lossy().to_string()),
            ValidationRule::FileNotContains { path, .. } => {
                Some(path.to_string_lossy().to_string())
            }
            ValidationRule::DirStructureMatch { root, .. } => {
                Some(root.to_string_lossy().to_string())
            }
            ValidationRule::JsonSchemaValid { path, .. } => {
                Some(path.to_string_lossy().to_string())
            }
            ValidationRule::CommandPasses { .. }
            | ValidationRule::CommandOutputContains { .. }
            | ValidationRule::SemanticCheck { .. } => None,
        }
    }

    /// Check if the objective contains compound patterns with distinct verb phrases.
    ///
    /// Looks for patterns like "create X and then test Y", "first ... then ...",
    /// "also", "additionally" that indicate multiple distinct tasks.
    ///
    /// Important: "and" inside a noun phrase (e.g., "read and write functions")
    /// should NOT trigger decomposition.
    fn check_compound_objective(manifest: &SuccessManifest) -> Option<DecompositionAdvice> {
        let objective = &manifest.objective;
        let lower = objective.to_lowercase();

        // Pattern: "and then" — strong signal of sequential tasks
        if let Some(parts) = Self::split_compound(&lower, "and then") {
            if Self::both_have_verbs(&parts.0, &parts.1) {
                return Some(DecompositionAdvice::Decompose {
                    sub_objectives: vec![
                        Self::capitalize_first(&parts.0.trim().to_string()),
                        Self::capitalize_first(&parts.1.trim().to_string()),
                    ],
                    reason: "Objective contains sequential tasks ('and then')".to_string(),
                });
            }
        }

        // Pattern: "first...then" — sequential tasks
        if lower.contains("first") && lower.contains("then") {
            if let Some((before_then, after_then)) = Self::split_first_then(&lower) {
                if Self::both_have_verbs(&before_then, &after_then) {
                    return Some(DecompositionAdvice::Decompose {
                        sub_objectives: vec![
                            Self::capitalize_first(&before_then.trim().to_string()),
                            Self::capitalize_first(&after_then.trim().to_string()),
                        ],
                        reason: "Objective contains sequential tasks ('first...then')".to_string(),
                    });
                }
            }
        }

        // Pattern: "additionally" or "also" with verb phrases
        for separator in &["additionally", "also"] {
            if let Some(parts) = Self::split_compound(&lower, separator) {
                if Self::both_have_verbs(&parts.0, &parts.1) {
                    return Some(DecompositionAdvice::Decompose {
                        sub_objectives: vec![
                            Self::capitalize_first(&parts.0.trim().to_string()),
                            Self::capitalize_first(&parts.1.trim().to_string()),
                        ],
                        reason: format!(
                            "Objective contains compound tasks ('{}')",
                            separator
                        ),
                    });
                }
            }
        }

        None
    }

    /// Split text on a separator, returning (before, after).
    fn split_compound(text: &str, separator: &str) -> Option<(String, String)> {
        if let Some(pos) = text.find(separator) {
            let before = text[..pos].to_string();
            let after = text[pos + separator.len()..].to_string();
            if !before.trim().is_empty() && !after.trim().is_empty() {
                return Some((before, after));
            }
        }
        None
    }

    /// Split "first X then Y" pattern.
    fn split_first_then(text: &str) -> Option<(String, String)> {
        // Find "first" and "then" positions
        let first_pos = text.find("first")?;
        let then_pos = text[first_pos..].find("then")?;
        let then_abs = first_pos + then_pos;

        let before = text[first_pos + 5..then_abs].to_string();
        let after = text[then_abs + 4..].to_string();

        if !before.trim().is_empty() && !after.trim().is_empty() {
            Some((before, after))
        } else {
            None
        }
    }

    /// Check if both parts contain at least one verb-like word.
    ///
    /// Uses a simple heuristic: common programming task verbs.
    fn both_have_verbs(part_a: &str, part_b: &str) -> bool {
        Self::contains_verb(part_a) && Self::contains_verb(part_b)
    }

    /// Check if text contains a verb-like word common in programming tasks.
    fn contains_verb(text: &str) -> bool {
        const VERBS: &[&str] = &[
            "create", "add", "implement", "write", "build", "make", "set up", "setup",
            "configure", "deploy", "test", "run", "check", "validate", "verify", "update",
            "modify", "change", "delete", "remove", "refactor", "migrate", "install",
            "fix", "debug", "optimize", "analyze", "generate", "parse", "convert",
            "integrate", "connect", "send", "fetch", "download", "upload", "read",
            "ensure", "define", "register", "handle",
        ];

        let lower = text.to_lowercase();
        VERBS.iter().any(|v| {
            // Match whole word boundaries (avoid "create" matching "recreate" etc.)
            lower.split(|c: char| !c.is_alphanumeric() && c != ' ')
                .any(|word| {
                    word.split_whitespace()
                        .any(|w| w == *v || word.contains(v))
                })
        })
    }

    /// Capitalize the first character of a string.
    fn capitalize_first(s: &str) -> String {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return String::new();
        }
        let mut chars = trimmed.chars();
        match chars.next() {
            None => String::new(),
            Some(c) => c.to_uppercase().to_string() + chars.as_str(),
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Helper to create a manifest with file constraints in given directories.
    fn manifest_with_file_constraints(dirs: &[&str], files_per_dir: usize) -> SuccessManifest {
        let mut manifest = SuccessManifest::new("test-task", "Test objective");
        for dir in dirs {
            for i in 0..files_per_dir {
                manifest.hard_constraints.push(ValidationRule::FileExists {
                    path: PathBuf::from(format!("{}/file{}.rs", dir, i)),
                });
            }
        }
        manifest
    }

    #[test]
    fn simple_single_constraint_proceeds() {
        let manifest = SuccessManifest::new("t1", "Create a file")
            .with_hard_constraint(ValidationRule::FileExists {
                path: PathBuf::from("src/main.rs"),
            });

        let advice = DecompositionDetector::analyze(&manifest);
        assert!(!advice.should_decompose());
    }

    #[test]
    fn many_constraints_same_directory_proceeds() {
        // 6 constraints but all in same directory -> Proceed
        let manifest = manifest_with_file_constraints(&["src"], 6);

        let advice = DecompositionDetector::analyze(&manifest);
        assert!(
            !advice.should_decompose(),
            "Should proceed when all constraints are in the same directory"
        );
    }

    #[test]
    fn many_constraints_across_directories_decomposes() {
        // 9 constraints across 3 directories -> Decompose
        let manifest = manifest_with_file_constraints(&["src/api", "tests/unit", "config"], 3);

        let advice = DecompositionDetector::analyze(&manifest);
        assert!(
            advice.should_decompose(),
            "Should decompose when constraints span 3+ directories"
        );

        if let DecompositionAdvice::Decompose {
            sub_objectives,
            reason,
        } = advice
        {
            assert_eq!(sub_objectives.len(), 3);
            assert!(reason.contains("9 constraints"));
            assert!(reason.contains("3 directories"));
        }
    }

    #[test]
    fn compound_objective_and_then_decomposes() {
        let manifest = SuccessManifest::new(
            "t1",
            "Create the authentication module and then test the login flow",
        );

        let advice = DecompositionDetector::analyze(&manifest);
        assert!(
            advice.should_decompose(),
            "Should decompose 'and then' compound objective"
        );

        if let DecompositionAdvice::Decompose {
            sub_objectives,
            reason,
        } = advice
        {
            assert_eq!(sub_objectives.len(), 2);
            assert!(reason.contains("and then"));
        }
    }

    #[test]
    fn single_objective_with_and_in_noun_phrase_proceeds() {
        // "read and write" is a noun phrase, not two separate tasks
        let manifest = SuccessManifest::new("t1", "Create read and write functions");

        let advice = DecompositionDetector::analyze(&manifest);
        assert!(
            !advice.should_decompose(),
            "'and' in noun phrase should not trigger decomposition"
        );
    }

    #[test]
    fn boundary_five_constraints_two_dirs_proceeds() {
        // Exactly 5 constraints, 2 directories -> Proceed (needs >5 AND >=3 dirs)
        let mut manifest = SuccessManifest::new("t1", "Test task");
        for i in 0..3 {
            manifest.hard_constraints.push(ValidationRule::FileExists {
                path: PathBuf::from(format!("src/file{}.rs", i)),
            });
        }
        for i in 0..2 {
            manifest.hard_constraints.push(ValidationRule::FileExists {
                path: PathBuf::from(format!("tests/file{}.rs", i)),
            });
        }
        assert_eq!(manifest.hard_constraints.len(), 5);

        let advice = DecompositionDetector::analyze(&manifest);
        assert!(
            !advice.should_decompose(),
            "Exactly 5 constraints with 2 dirs should proceed"
        );
    }

    #[test]
    fn first_then_pattern_decomposes() {
        let manifest = SuccessManifest::new(
            "t1",
            "First implement the parser, then validate the output format",
        );

        let advice = DecompositionDetector::analyze(&manifest);
        assert!(
            advice.should_decompose(),
            "Should decompose 'first...then' pattern"
        );
    }

    #[test]
    fn additionally_pattern_decomposes() {
        let manifest = SuccessManifest::new(
            "t1",
            "Create the config module, additionally implement error handling for all endpoints",
        );

        let advice = DecompositionDetector::analyze(&manifest);
        assert!(
            advice.should_decompose(),
            "Should decompose 'additionally' pattern"
        );
    }

    #[test]
    fn empty_constraints_proceeds() {
        let manifest = SuccessManifest::new("t1", "Simple task");
        let advice = DecompositionDetector::analyze(&manifest);
        assert!(!advice.should_decompose());
    }

    #[test]
    fn command_constraints_not_counted_for_dirs() {
        // 6 command constraints have no paths -> should not trigger dir spread
        let mut manifest = SuccessManifest::new("t1", "Run tests");
        for i in 0..6 {
            manifest
                .hard_constraints
                .push(ValidationRule::CommandPasses {
                    cmd: "cargo".to_string(),
                    args: vec![format!("test-{}", i)],
                    timeout_ms: 30_000,
                });
        }

        let advice = DecompositionDetector::analyze(&manifest);
        assert!(
            !advice.should_decompose(),
            "Command-only constraints have no directory spread"
        );
    }
}
