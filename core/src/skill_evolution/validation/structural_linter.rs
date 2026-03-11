//! Structural linter (L1) -- validates pattern structure and constraints.
//!
//! Performs static structural checks on a `PatternSequence` to ensure
//! it meets basic validity requirements before semantic evaluation.

use crate::poe::crystallization::pattern_model::PatternSequence;

use super::test_set_generator::ValidationTestSet;

// ============================================================================
// Types
// ============================================================================

/// Result of structural lint validation.
#[derive(Debug, Clone)]
pub struct LintResult {
    pub passed: bool,
    pub errors: Vec<String>,
}

// ============================================================================
// StructuralLinter
// ============================================================================

/// Validates pattern structure (L1 validation tier).
pub struct StructuralLinter;

impl StructuralLinter {
    /// Validate a pattern's structure.
    pub fn validate(&self, pattern: &PatternSequence, _test_set: &ValidationTestSet) -> LintResult {
        let mut errors = Vec::new();

        // 1. Steps non-empty
        if pattern.steps.is_empty() {
            errors.push("Pattern has no steps".to_string());
        }

        // 2. Delegate to PatternSequence constraint validation
        let constraint_errors = pattern.validate();
        errors.extend(constraint_errors);

        // 3. Description non-empty
        if pattern.description.trim().is_empty() {
            errors.push("Pattern description is empty".to_string());
        }

        LintResult {
            passed: errors.is_empty(),
            errors,
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::poe::crystallization::pattern_model::{
        ParameterMapping, PatternStep, Predicate, ToolCallTemplate, ToolCategory,
    };

    fn make_action(name: &str) -> PatternStep {
        PatternStep::Action {
            tool_call: ToolCallTemplate {
                tool_name: name.to_string(),
                category: ToolCategory::ReadOnly,
            },
            params: ParameterMapping::default(),
        }
    }

    fn empty_test_set() -> ValidationTestSet {
        ValidationTestSet { samples: vec![] }
    }

    #[test]
    fn valid_pattern_passes() {
        let pattern = PatternSequence {
            description: "A valid pattern".to_string(),
            steps: vec![make_action("read_file")],
            expected_outputs: vec![],
        };
        let linter = StructuralLinter;
        let result = linter.validate(&pattern, &empty_test_set());
        assert!(result.passed, "Expected pass, got errors: {:?}", result.errors);
    }

    #[test]
    fn empty_steps_fails() {
        let pattern = PatternSequence {
            description: "Empty pattern".to_string(),
            steps: vec![],
            expected_outputs: vec![],
        };
        let linter = StructuralLinter;
        let result = linter.validate(&pattern, &empty_test_set());
        assert!(!result.passed);
        assert!(result.errors.iter().any(|e| e.contains("no steps")));
    }

    #[test]
    fn invalid_constraints_fails() {
        let pattern = PatternSequence {
            description: "Bad loop".to_string(),
            steps: vec![PatternStep::Loop {
                predicate: Predicate::Semantic("go".to_string()),
                body: vec![make_action("work")],
                max_iterations: 15, // > 10 is invalid
            }],
            expected_outputs: vec![],
        };
        let linter = StructuralLinter;
        let result = linter.validate(&pattern, &empty_test_set());
        assert!(!result.passed);
        assert!(result.errors.iter().any(|e| e.contains("max_iterations")));
    }
}
