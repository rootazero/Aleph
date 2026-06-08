//! Goal pursuit — the R7/R10-safe autonomous continuation driver.
//!
//! When a gateway run finishes for a session whose standing goal is `Active`
//! pursuit and still unfinished/under-caps, the gateway spawns ONE more
//! continuation run for the SAME session via `ExecutionAdapter::execute`
//! directly (see `src/gateway/execution_engine/execute.rs`). The cron
//! executor is deliberately NOT used: it runs under a distinct
//! `agent:<id>:cron:<job>` session key, so the goal — stored under the
//! interactive session key — would never be found. Completion is decided
//! solely by the model calling `goal(update, complete)` (read here as plain
//! state — no judgment); iteration/token caps are structural backstops. Lives
//! in `src/tasks/`, never in `src/harness/` (R10 12-file redline).

use crate::goal::{Goal, GoalStatus, PursuitMode};

/// Pure decision: should this goal get one more autonomous continuation?
/// `tokens_now` is the session's current total-token count (pass 0 when a
/// live counter isn't available — then only the iteration cap applies).
pub fn should_continue(goal: &Goal, tokens_now: u64) -> bool {
    let PursuitMode::Active { max_iterations } = goal.pursuit else {
        return false; // Passive goals never self-continue.
    };
    if goal.status != GoalStatus::Active {
        return false; // complete / blocked / paused → stop.
    }
    if goal.continuations_used >= max_iterations {
        return false; // structural backstop (hermes max_turns parity).
    }
    if goal.over_budget(tokens_now) {
        return false; // soft budget becomes a hard stop for autonomous runs.
    }
    true
}

/// Continuation prompt re-stating the goal (hermes parity), used when
/// enqueuing the next autonomous run.
pub fn continuation_prompt(goal: &Goal) -> String {
    format!(
        "[Continuing toward your standing goal]\nGoal: {}\n\nTake the next \
         concrete step. If you have achieved the goal, call \
         goal(action='update', status='complete') and stop. If you are \
         blocked and need the user, call goal(action='update', \
         status='blocked') and stop.",
        goal.objective,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::goal::{Goal, GoalStatus, PursuitMode};

    fn active_goal(max_iter: u32) -> Goal {
        Goal::new("s", "obj", 0, 0).with_pursuit(PursuitMode::Active {
            max_iterations: max_iter,
        })
    }

    #[test]
    fn passive_goal_never_continues() {
        let g = Goal::new("s", "obj", 0, 0); // Passive
        assert!(!should_continue(&g, 0));
    }

    #[test]
    fn active_within_caps_continues() {
        let g = active_goal(5); // continuations_used = 0
        assert!(should_continue(&g, 0));
    }

    #[test]
    fn stops_at_iteration_cap() {
        let mut g = active_goal(3);
        g.continuations_used = 3;
        assert!(!should_continue(&g, 0));
    }

    #[test]
    fn stops_when_complete() {
        let g = active_goal(5).with_status(GoalStatus::Complete, 0);
        assert!(!should_continue(&g, 0));
    }

    #[test]
    fn stops_when_over_budget() {
        let g = active_goal(5).with_budget(Some(100));
        // tokens_at_start=0, so tokens_used(250)=250 > 100 → over budget.
        assert!(!should_continue(&g, 250));
    }

    #[test]
    fn continuation_prompt_restates_objective() {
        let g = active_goal(5);
        assert!(continuation_prompt(&g).contains("obj"));
        assert!(continuation_prompt(&g).contains("status='complete'"));
    }
}
