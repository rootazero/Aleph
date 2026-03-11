//! Skill risk profiler -- classifies patterns by risk level.
//!
//! Walks all steps in a `PatternSequence` and determines the maximum
//! risk level based on tool categories, loop iterations, and sub-patterns.

use serde::{Deserialize, Serialize};

use crate::poe::crystallization::pattern_model::{PatternSequence, PatternStep, ToolCategory};

// ============================================================================
// Types
// ============================================================================

/// Risk level for a skill pattern, ordered from lowest to highest.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SkillRiskLevel {
    /// Read-only, info processing, format conversion.
    Low,
    /// Local file writes, cross-plugin, complex branching.
    Medium,
    /// Network, shell, delete/overwrite, credentials.
    High,
}

/// Risk profile for a skill pattern with reasoning.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillRiskProfile {
    pub level: SkillRiskLevel,
    pub reasoning: String,
}

// ============================================================================
// SkillRiskProfiler
// ============================================================================

/// Profiles the risk level of a pattern sequence.
pub struct SkillRiskProfiler;

impl SkillRiskProfiler {
    /// Analyze a pattern and return its risk profile.
    pub fn profile(pattern: &PatternSequence) -> SkillRiskProfile {
        let mut max_level = SkillRiskLevel::Low;
        let mut reasons = Vec::new();

        for step in &pattern.steps {
            Self::walk_step(step, &mut max_level, &mut reasons);
        }

        SkillRiskProfile {
            level: max_level,
            reasoning: if reasons.is_empty() {
                "No risk factors detected".to_string()
            } else {
                reasons.join("; ")
            },
        }
    }

    fn walk_step(step: &PatternStep, max_level: &mut SkillRiskLevel, reasons: &mut Vec<String>) {
        match step {
            PatternStep::Action { tool_call, .. } => {
                let level = Self::classify_category(&tool_call.category);
                if level > *max_level {
                    reasons.push(format!(
                        "tool '{}' has category {:?}",
                        tool_call.tool_name, tool_call.category
                    ));
                    *max_level = level;
                }
            }
            PatternStep::Conditional {
                then_steps,
                else_steps,
                ..
            } => {
                for s in then_steps {
                    Self::walk_step(s, max_level, reasons);
                }
                for s in else_steps {
                    Self::walk_step(s, max_level, reasons);
                }
            }
            PatternStep::Loop {
                body,
                max_iterations,
                ..
            } => {
                if *max_iterations > 5 && *max_level < SkillRiskLevel::Medium {
                    *max_level = SkillRiskLevel::Medium;
                    reasons.push(format!(
                        "loop with max_iterations={} > 5",
                        max_iterations
                    ));
                }
                for s in body {
                    Self::walk_step(s, max_level, reasons);
                }
            }
            PatternStep::SubPattern { pattern_id } => {
                if *max_level < SkillRiskLevel::Medium {
                    *max_level = SkillRiskLevel::Medium;
                    reasons.push(format!("sub-pattern reference '{}'", pattern_id));
                }
            }
        }
    }

    fn classify_category(category: &ToolCategory) -> SkillRiskLevel {
        match category {
            ToolCategory::ReadOnly => SkillRiskLevel::Low,
            ToolCategory::FileWrite | ToolCategory::CrossPlugin => SkillRiskLevel::Medium,
            ToolCategory::Shell | ToolCategory::Network | ToolCategory::Destructive => {
                SkillRiskLevel::High
            }
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
        ParameterMapping, PatternStep, Predicate, ToolCallTemplate,
    };

    fn make_action(name: &str, category: ToolCategory) -> PatternStep {
        PatternStep::Action {
            tool_call: ToolCallTemplate {
                tool_name: name.to_string(),
                category,
            },
            params: ParameterMapping::default(),
        }
    }

    fn make_pattern(steps: Vec<PatternStep>) -> PatternSequence {
        PatternSequence {
            description: "test pattern".to_string(),
            steps,
            expected_outputs: vec![],
        }
    }

    #[test]
    fn readonly_is_low_risk() {
        let pattern = make_pattern(vec![make_action("read_file", ToolCategory::ReadOnly)]);
        let profile = SkillRiskProfiler::profile(&pattern);
        assert_eq!(profile.level, SkillRiskLevel::Low);
    }

    #[test]
    fn file_write_is_medium_risk() {
        let pattern = make_pattern(vec![make_action("write_file", ToolCategory::FileWrite)]);
        let profile = SkillRiskProfiler::profile(&pattern);
        assert_eq!(profile.level, SkillRiskLevel::Medium);
    }

    #[test]
    fn shell_is_high_risk() {
        let pattern = make_pattern(vec![make_action("run_shell", ToolCategory::Shell)]);
        let profile = SkillRiskProfiler::profile(&pattern);
        assert_eq!(profile.level, SkillRiskLevel::High);
    }

    #[test]
    fn high_iteration_loop_is_medium_risk() {
        let pattern = make_pattern(vec![PatternStep::Loop {
            predicate: Predicate::Semantic("continue".to_string()),
            body: vec![make_action("read", ToolCategory::ReadOnly)],
            max_iterations: 8,
        }]);
        let profile = SkillRiskProfiler::profile(&pattern);
        assert_eq!(profile.level, SkillRiskLevel::Medium);
    }

    #[test]
    fn risk_level_ordering() {
        assert!(SkillRiskLevel::Low < SkillRiskLevel::Medium);
        assert!(SkillRiskLevel::Medium < SkillRiskLevel::High);
        assert!(SkillRiskLevel::Low < SkillRiskLevel::High);
    }
}
