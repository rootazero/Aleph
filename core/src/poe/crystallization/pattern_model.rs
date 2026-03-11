//! Enhanced sequence pattern model types for cognitive evolution.
//!
//! Provides the foundational type system for representing executable patterns
//! with predicates, conditional branching, loops, and cost estimation.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ============================================================================
// Predicate Types
// ============================================================================

/// A comparison operator for metric-based predicates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CompareOp {
    Gt,
    Gte,
    Lt,
    Lte,
    Eq,
}

/// A cognitive metric that can be evaluated at runtime.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CognitiveMetric {
    Entropy,
    TrustScore,
    RemainingBudgetRatio,
    AttemptCount,
}

/// A predicate that can be evaluated to determine control flow.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value")]
pub enum Predicate {
    /// LLM-evaluated semantic predicate.
    Semantic(String),
    /// Runtime metric threshold check.
    MetricThreshold {
        metric: CognitiveMetric,
        op: CompareOp,
        threshold: f32,
    },
    /// Logical AND of multiple predicates.
    And(Vec<Predicate>),
    /// Logical OR of multiple predicates.
    Or(Vec<Predicate>),
    /// Logical NOT of a predicate.
    Not(Box<Predicate>),
}

// ============================================================================
// Tool Types
// ============================================================================

/// Category of a tool call, used for blast radius and permission analysis.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ToolCategory {
    ReadOnly,
    FileWrite,
    CrossPlugin,
    Shell,
    Network,
    Destructive,
}

/// A template for a tool call within a pattern step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCallTemplate {
    pub tool_name: String,
    pub category: ToolCategory,
}

/// Mapping of parameter variables for tool calls.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ParameterMapping {
    pub variables: HashMap<String, String>,
}

// ============================================================================
// Pattern Steps
// ============================================================================

/// A single step in a pattern sequence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "step_type")]
pub enum PatternStep {
    /// Execute a tool call with parameters.
    Action {
        tool_call: ToolCallTemplate,
        params: ParameterMapping,
    },
    /// Conditional branching based on a predicate.
    Conditional {
        predicate: Predicate,
        then_steps: Vec<PatternStep>,
        else_steps: Vec<PatternStep>,
    },
    /// Loop execution while predicate holds.
    Loop {
        predicate: Predicate,
        body: Vec<PatternStep>,
        max_iterations: u32,
    },
    /// Reference to another pattern by ID.
    SubPattern {
        pattern_id: String,
    },
}

impl PatternStep {
    /// Estimate the cost of executing this step.
    ///
    /// - Action: 1.0
    /// - Conditional: average of branch costs + 0.5 overhead
    /// - Loop: body cost * (max_iterations / 2) + 0.5 overhead
    /// - SubPattern: 3.0 (unknown cost, conservative estimate)
    pub fn estimated_cost(&self) -> f32 {
        match self {
            PatternStep::Action { .. } => 1.0,
            PatternStep::Conditional {
                then_steps,
                else_steps,
                ..
            } => {
                let then_cost: f32 = then_steps.iter().map(|s| s.estimated_cost()).sum();
                let else_cost: f32 = else_steps.iter().map(|s| s.estimated_cost()).sum();
                (then_cost + else_cost) / 2.0 + 0.5
            }
            PatternStep::Loop {
                body,
                max_iterations,
                ..
            } => {
                let body_cost: f32 = body.iter().map(|s| s.estimated_cost()).sum();
                body_cost * (*max_iterations as f32 / 2.0) + 0.5
            }
            PatternStep::SubPattern { .. } => 3.0,
        }
    }

    /// Recursively collect all leaf Action and SubPattern nodes.
    pub fn collect_actions<'a>(&'a self, out: &mut Vec<&'a PatternStep>) {
        match self {
            PatternStep::Action { .. } | PatternStep::SubPattern { .. } => {
                out.push(self);
            }
            PatternStep::Conditional {
                then_steps,
                else_steps,
                ..
            } => {
                for step in then_steps {
                    step.collect_actions(out);
                }
                for step in else_steps {
                    step.collect_actions(out);
                }
            }
            PatternStep::Loop { body, .. } => {
                for step in body {
                    step.collect_actions(out);
                }
            }
        }
    }

    /// Calculate the maximum nesting depth from this step.
    pub fn max_nesting_depth(&self, current: u32) -> u32 {
        match self {
            PatternStep::Action { .. } | PatternStep::SubPattern { .. } => current,
            PatternStep::Conditional {
                then_steps,
                else_steps,
                ..
            } => {
                let next = current + 1;
                let then_max = then_steps
                    .iter()
                    .map(|s| s.max_nesting_depth(next))
                    .max()
                    .unwrap_or(next);
                let else_max = else_steps
                    .iter()
                    .map(|s| s.max_nesting_depth(next))
                    .max()
                    .unwrap_or(next);
                then_max.max(else_max)
            }
            PatternStep::Loop { body, .. } => {
                let next = current + 1;
                body.iter()
                    .map(|s| s.max_nesting_depth(next))
                    .max()
                    .unwrap_or(next)
            }
        }
    }

    /// Validate this step, collecting errors.
    ///
    /// Constraints:
    /// - Loop max_iterations must be 1..=10
    /// - Semantic predicate string length <= 200
    /// - Nesting depth <= 3
    pub fn validate_step(&self, errors: &mut Vec<String>, depth: u32) {
        if depth > 3 {
            errors.push(format!("Nesting depth {} exceeds maximum of 3", depth));
            return;
        }

        match self {
            PatternStep::Action { .. } | PatternStep::SubPattern { .. } => {}
            PatternStep::Conditional {
                predicate,
                then_steps,
                else_steps,
                ..
            } => {
                validate_predicate(predicate, errors);
                for step in then_steps {
                    step.validate_step(errors, depth + 1);
                }
                for step in else_steps {
                    step.validate_step(errors, depth + 1);
                }
            }
            PatternStep::Loop {
                predicate,
                body,
                max_iterations,
            } => {
                if *max_iterations < 1 || *max_iterations > 10 {
                    errors.push(format!(
                        "Loop max_iterations {} must be in range 1..=10",
                        max_iterations
                    ));
                }
                validate_predicate(predicate, errors);
                for step in body {
                    step.validate_step(errors, depth + 1);
                }
            }
        }
    }
}

/// Validate a predicate, checking semantic string length constraints.
fn validate_predicate(predicate: &Predicate, errors: &mut Vec<String>) {
    match predicate {
        Predicate::Semantic(s) => {
            if s.len() > 200 {
                errors.push(format!(
                    "Semantic predicate length {} exceeds maximum of 200",
                    s.len()
                ));
            }
        }
        Predicate::MetricThreshold { .. } => {}
        Predicate::And(preds) => {
            for p in preds {
                validate_predicate(p, errors);
            }
        }
        Predicate::Or(preds) => {
            for p in preds {
                validate_predicate(p, errors);
            }
        }
        Predicate::Not(p) => {
            validate_predicate(p, errors);
        }
    }
}

// ============================================================================
// Pattern Sequence
// ============================================================================

/// A complete pattern sequence with description and expected outputs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PatternSequence {
    pub description: String,
    pub steps: Vec<PatternStep>,
    pub expected_outputs: Vec<String>,
}

impl PatternSequence {
    /// Validate all steps in this sequence.
    pub fn validate(&self) -> Vec<String> {
        let mut errors = Vec::new();
        for step in &self.steps {
            step.validate_step(&mut errors, 0);
        }
        errors
    }

    /// Flatten all leaf steps (Action and SubPattern) from the sequence.
    pub fn iter_all_steps(&self) -> Vec<&PatternStep> {
        let mut out = Vec::new();
        for step in &self.steps {
            step.collect_actions(&mut out);
        }
        out
    }

    /// Sum the estimated cost of all steps.
    pub fn estimated_total_cost(&self) -> f32 {
        self.steps.iter().map(|s| s.estimated_cost()).sum()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_action(name: &str) -> PatternStep {
        PatternStep::Action {
            tool_call: ToolCallTemplate {
                tool_name: name.to_string(),
                category: ToolCategory::ReadOnly,
            },
            params: ParameterMapping::default(),
        }
    }

    #[test]
    fn test_action_step_roundtrip() {
        let step = make_action("read_file");
        let json = serde_json::to_string(&step).unwrap();
        let deserialized: PatternStep = serde_json::from_str(&json).unwrap();
        assert_eq!(step, deserialized);
    }

    #[test]
    fn test_conditional_step_roundtrip() {
        let step = PatternStep::Conditional {
            predicate: Predicate::MetricThreshold {
                metric: CognitiveMetric::Entropy,
                op: CompareOp::Gt,
                threshold: 0.5,
            },
            then_steps: vec![make_action("analyze")],
            else_steps: vec![make_action("skip")],
        };
        let json = serde_json::to_string(&step).unwrap();
        let deserialized: PatternStep = serde_json::from_str(&json).unwrap();
        assert_eq!(step, deserialized);
    }

    #[test]
    fn test_loop_step_max_iterations_constraint() {
        let step = PatternStep::Loop {
            predicate: Predicate::MetricThreshold {
                metric: CognitiveMetric::AttemptCount,
                op: CompareOp::Lt,
                threshold: 10.0,
            },
            body: vec![make_action("retry")],
            max_iterations: 15,
        };
        let mut errors = Vec::new();
        step.validate_step(&mut errors, 0);
        assert!(!errors.is_empty());
        assert!(errors[0].contains("max_iterations"));
    }

    #[test]
    fn test_subpattern_nesting_depth() {
        // Build 4 levels of nesting: Cond > Cond > Cond > Cond > Action
        let deep = PatternStep::Conditional {
            predicate: Predicate::Semantic("ok".to_string()),
            then_steps: vec![PatternStep::Conditional {
                predicate: Predicate::Semantic("ok".to_string()),
                then_steps: vec![PatternStep::Conditional {
                    predicate: Predicate::Semantic("ok".to_string()),
                    then_steps: vec![PatternStep::Conditional {
                        predicate: Predicate::Semantic("ok".to_string()),
                        then_steps: vec![make_action("leaf")],
                        else_steps: vec![],
                    }],
                    else_steps: vec![],
                }],
                else_steps: vec![],
            }],
            else_steps: vec![],
        };
        let mut errors = Vec::new();
        deep.validate_step(&mut errors, 0);
        assert!(!errors.is_empty(), "Should fail: nesting depth > 3");
        assert!(errors[0].contains("Nesting depth"));
    }

    #[test]
    fn test_semantic_predicate_length_limit() {
        let long_pred = Predicate::Semantic("x".repeat(201));
        let step = PatternStep::Conditional {
            predicate: long_pred,
            then_steps: vec![make_action("a")],
            else_steps: vec![],
        };
        let mut errors = Vec::new();
        step.validate_step(&mut errors, 0);
        assert!(!errors.is_empty());
        assert!(errors[0].contains("Semantic predicate length"));
    }

    #[test]
    fn test_pattern_step_cost_estimation() {
        // Action = 1.0
        assert!((make_action("x").estimated_cost() - 1.0).abs() < f32::EPSILON);

        // Loop with 1 action, max_iterations=6: 1.0 * (6/2) + 0.5 = 3.5
        let loop_step = PatternStep::Loop {
            predicate: Predicate::Semantic("go".to_string()),
            body: vec![make_action("work")],
            max_iterations: 6,
        };
        assert!((loop_step.estimated_cost() - 3.5).abs() < f32::EPSILON);

        // SubPattern = 3.0
        let sub = PatternStep::SubPattern {
            pattern_id: "other".to_string(),
        };
        assert!((sub.estimated_cost() - 3.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_iter_all_steps_flattens() {
        let seq = PatternSequence {
            description: "test".to_string(),
            steps: vec![
                make_action("first"),
                PatternStep::Conditional {
                    predicate: Predicate::Semantic("check".to_string()),
                    then_steps: vec![make_action("then_action")],
                    else_steps: vec![make_action("else_action")],
                },
            ],
            expected_outputs: vec![],
        };
        let leaves = seq.iter_all_steps();
        // first + then_action + else_action = 3
        assert_eq!(leaves.len(), 3);
    }
}
