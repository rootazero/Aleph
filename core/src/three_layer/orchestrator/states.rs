//! Orchestrator State Machine

use std::fmt;

/// State of the Orchestrator FSM
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrchestratorState {
    /// Clarify problem definition, constraints, evaluation criteria
    Clarify,
    /// Produce executable plan (which Skills to invoke)
    Plan,
    /// Invoke Skill DAG for execution
    Execute,
    /// Check if goals are met (evidence, test results, etc.)
    Evaluate,
    /// On failure, identify cause, adjust plan or gather more info
    Reflect,
    /// Exit when stop conditions are met
    Stop,
}

impl OrchestratorState {
    /// Check if transition to another state is valid
    pub fn can_transition_to(&self, target: &OrchestratorState) -> bool {
        match self {
            OrchestratorState::Clarify => matches!(
                target,
                OrchestratorState::Plan | OrchestratorState::Stop
            ),
            OrchestratorState::Plan => matches!(
                target,
                OrchestratorState::Execute | OrchestratorState::Clarify | OrchestratorState::Stop
            ),
            OrchestratorState::Execute => matches!(
                target,
                OrchestratorState::Evaluate | OrchestratorState::Stop
            ),
            OrchestratorState::Evaluate => matches!(
                target,
                OrchestratorState::Reflect | OrchestratorState::Stop
            ),
            OrchestratorState::Reflect => matches!(
                target,
                OrchestratorState::Plan | OrchestratorState::Execute | OrchestratorState::Stop
            ),
            OrchestratorState::Stop => false, // Terminal state
        }
    }

    /// Check if this is a terminal state
    pub fn is_terminal(&self) -> bool {
        matches!(self, OrchestratorState::Stop)
    }

    /// Get the initial state
    pub fn initial() -> Self {
        OrchestratorState::Clarify
    }
}

impl fmt::Display for OrchestratorState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OrchestratorState::Clarify => write!(f, "Clarify"),
            OrchestratorState::Plan => write!(f, "Plan"),
            OrchestratorState::Execute => write!(f, "Execute"),
            OrchestratorState::Evaluate => write!(f, "Evaluate"),
            OrchestratorState::Reflect => write!(f, "Reflect"),
            OrchestratorState::Stop => write!(f, "Stop"),
        }
    }
}

impl Default for OrchestratorState {
    fn default() -> Self {
        Self::initial()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_state_transitions_from_clarify() {
        let state = OrchestratorState::Clarify;
        assert!(state.can_transition_to(&OrchestratorState::Plan));
        assert!(state.can_transition_to(&OrchestratorState::Stop));
        assert!(!state.can_transition_to(&OrchestratorState::Execute));
    }

    #[test]
    fn test_state_transitions_from_evaluate() {
        let state = OrchestratorState::Evaluate;
        assert!(state.can_transition_to(&OrchestratorState::Reflect));
        assert!(state.can_transition_to(&OrchestratorState::Stop));
        assert!(!state.can_transition_to(&OrchestratorState::Clarify));
    }

    #[test]
    fn test_state_is_terminal() {
        assert!(!OrchestratorState::Clarify.is_terminal());
        assert!(!OrchestratorState::Execute.is_terminal());
        assert!(OrchestratorState::Stop.is_terminal());
    }

    #[test]
    fn test_state_display() {
        assert_eq!(format!("{}", OrchestratorState::Clarify), "Clarify");
        assert_eq!(format!("{}", OrchestratorState::Execute), "Execute");
    }
}
