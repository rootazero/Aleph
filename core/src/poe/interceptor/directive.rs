use serde::{Deserialize, Serialize};

/// POE directive to AgentLoop after evaluating a step.
/// Returned by `LoopCallback::on_step_evaluate()`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub enum StepDirective {
    /// Normal continuation — no intervention.
    #[default]
    Continue,
    /// Continue but inject a hint into the next Think step.
    ContinueWithHint { hint: String },
    /// Suggest strategy switch — terminates loop via GuardTriggered(PoeStrategySwitch).
    SuggestStrategySwitch { reason: String, suggestion: String },
    /// Force loop termination.
    Abort { reason: String },
}

impl StepDirective {
    /// Returns `true` if the directive allows the loop to continue.
    pub fn allows_continue(&self) -> bool {
        matches!(self, Self::Continue | Self::ContinueWithHint { .. })
    }

    /// Extracts the hint string if this is a `ContinueWithHint` directive.
    pub fn hint(&self) -> Option<&str> {
        match self {
            Self::ContinueWithHint { hint } => Some(hint),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_continue() {
        let directive = StepDirective::default();
        assert!(matches!(directive, StepDirective::Continue));
    }

    #[test]
    fn allows_continue_for_continue() {
        assert!(StepDirective::Continue.allows_continue());
    }

    #[test]
    fn allows_continue_for_continue_with_hint() {
        let directive = StepDirective::ContinueWithHint {
            hint: "try a different approach".into(),
        };
        assert!(directive.allows_continue());
    }

    #[test]
    fn disallows_continue_for_suggest_strategy_switch() {
        let directive = StepDirective::SuggestStrategySwitch {
            reason: "stuck in loop".into(),
            suggestion: "use decomposition".into(),
        };
        assert!(!directive.allows_continue());
    }

    #[test]
    fn disallows_continue_for_abort() {
        let directive = StepDirective::Abort {
            reason: "budget exhausted".into(),
        };
        assert!(!directive.allows_continue());
    }

    #[test]
    fn hint_returns_some_for_continue_with_hint() {
        let directive = StepDirective::ContinueWithHint {
            hint: "focus on error handling".into(),
        };
        assert_eq!(directive.hint(), Some("focus on error handling"));
    }

    #[test]
    fn hint_returns_none_for_continue() {
        assert_eq!(StepDirective::Continue.hint(), None);
    }

    #[test]
    fn hint_returns_none_for_abort() {
        let directive = StepDirective::Abort {
            reason: "fatal".into(),
        };
        assert_eq!(directive.hint(), None);
    }

    #[test]
    fn hint_returns_none_for_suggest_strategy_switch() {
        let directive = StepDirective::SuggestStrategySwitch {
            reason: "stuck".into(),
            suggestion: "try again".into(),
        };
        assert_eq!(directive.hint(), None);
    }

    #[test]
    fn serialization_roundtrip_continue() {
        let directive = StepDirective::Continue;
        let json = serde_json::to_string(&directive).unwrap();
        let deserialized: StepDirective = serde_json::from_str(&json).unwrap();
        assert!(matches!(deserialized, StepDirective::Continue));
    }

    #[test]
    fn serialization_roundtrip_continue_with_hint() {
        let directive = StepDirective::ContinueWithHint {
            hint: "check edge cases".into(),
        };
        let json = serde_json::to_string(&directive).unwrap();
        let deserialized: StepDirective = serde_json::from_str(&json).unwrap();
        match deserialized {
            StepDirective::ContinueWithHint { hint } => {
                assert_eq!(hint, "check edge cases");
            }
            other => panic!("expected ContinueWithHint, got {:?}", other),
        }
    }

    #[test]
    fn serialization_roundtrip_suggest_strategy_switch() {
        let directive = StepDirective::SuggestStrategySwitch {
            reason: "no progress".into(),
            suggestion: "decompose task".into(),
        };
        let json = serde_json::to_string(&directive).unwrap();
        let deserialized: StepDirective = serde_json::from_str(&json).unwrap();
        match deserialized {
            StepDirective::SuggestStrategySwitch { reason, suggestion } => {
                assert_eq!(reason, "no progress");
                assert_eq!(suggestion, "decompose task");
            }
            other => panic!("expected SuggestStrategySwitch, got {:?}", other),
        }
    }

    #[test]
    fn serialization_roundtrip_abort() {
        let directive = StepDirective::Abort {
            reason: "critical failure".into(),
        };
        let json = serde_json::to_string(&directive).unwrap();
        let deserialized: StepDirective = serde_json::from_str(&json).unwrap();
        match deserialized {
            StepDirective::Abort { reason } => {
                assert_eq!(reason, "critical failure");
            }
            other => panic!("expected Abort, got {:?}", other),
        }
    }
}
