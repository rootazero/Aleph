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
///
/// Iteration-aware: this continuation is the `(continuations_used + 1)`-th
/// autonomous step. The pace (`N/max`) is surfaced so the model can self-budget
/// (R9 — intelligence in the prompt). On the FINAL allowed iteration it switches
/// to a wrap-up prompt (hermes "grace call" parity) so the model concludes
/// gracefully instead of being cut mid-thought when the next hook stops it.
pub fn continuation_prompt(goal: &Goal) -> String {
    let (this_iter, max_iter) = match goal.pursuit {
        PursuitMode::Active { max_iterations } => {
            (goal.continuations_used.saturating_add(1), max_iterations)
        }
        // Passive goals never reach the continuation path; stay graceful.
        PursuitMode::Passive => (goal.continuations_used.saturating_add(1), 0),
    };
    let is_final = max_iter != 0 && this_iter >= max_iter;
    if is_final {
        format!(
            "[Final autonomous iteration {this_iter}/{max_iter} toward your \
             standing goal]\nGoal: {}\n\nThis is your LAST autonomous step — no \
             further continuations will run after it. Wrap up now: if the goal is \
             achieved, call goal(action='update', status='complete'); if work \
             remains, call goal(action='update', status='blocked') with a note on \
             what's left so the user can take over. Do not begin anything you \
             cannot finish in this step.",
            goal.objective,
        )
    } else {
        let remaining = max_iter.saturating_sub(this_iter);
        format!(
            "[Continuing toward your standing goal — autonomous iteration \
             {this_iter}/{max_iter}]\nGoal: {}\n\nTake the next concrete step; \
             pace yourself against the {remaining} continuation(s) remaining after \
             this one. If you have achieved the goal, call goal(action='update', \
             status='complete') and stop. If you are blocked and need the user, \
             call goal(action='update', status='blocked') and stop.",
            goal.objective,
        )
    }
}

/// True when an `Active`-pursuit goal whose status is still `Active` can no
/// longer continue — i.e. the autonomous loop has hit its iteration (or budget)
/// cap without the model self-reporting `complete`/`blocked`. The continuation
/// hook uses this to transition the goal to `Blocked` (codex `BudgetLimited`
/// parity) so an exhausted goal stops surfacing as an active pursuit every turn.
///
/// This is a structural backstop, not a judgment about the work (R7): it fires
/// purely on the same caps `should_continue` enforces.
pub fn exhausted_while_active(goal: &Goal, tokens_now: u64) -> bool {
    matches!(goal.pursuit, PursuitMode::Active { .. })
        && goal.status == GoalStatus::Active
        && !should_continue(goal, tokens_now)
}

/// Human-readable note stamped on a goal when autonomous pursuit is cut off by
/// the caps, so the user sees why the loop stopped on their next turn.
pub fn cap_reached_note(goal: &Goal) -> String {
    match goal.pursuit {
        PursuitMode::Active { max_iterations } => format!(
            "Autonomous pursuit reached its iteration cap ({max_iterations} \
             iterations) without completing. Blocked for your guidance — review \
             progress, then clear or re-set the goal to continue."
        ),
        PursuitMode::Passive => "Autonomous pursuit ended.".to_string(),
    }
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

    #[test]
    fn continuation_prompt_surfaces_pace_when_not_final() {
        let g = active_goal(5); // continuations_used = 0 → this is iteration 1/5
        let p = continuation_prompt(&g);
        assert!(p.contains("iteration 1/5"), "got: {p}");
        assert!(p.contains("4 continuation"), "remaining shown: {p}");
        assert!(!p.contains("Final autonomous"));
    }

    #[test]
    fn continuation_prompt_is_grace_call_on_final_iteration() {
        let mut g = active_goal(3);
        g.continuations_used = 2; // about to run iteration 3/3 — the last one
        let p = continuation_prompt(&g);
        assert!(p.contains("Final autonomous iteration 3/3"), "got: {p}");
        assert!(p.contains("LAST autonomous step"));
        assert!(p.contains("status='complete'"));
        assert!(p.contains("status='blocked'"));
    }

    #[test]
    fn exhausted_while_active_true_only_at_cap() {
        let mut g = active_goal(3);
        assert!(!exhausted_while_active(&g, 0), "fresh goal can still continue");
        g.continuations_used = 3;
        assert!(exhausted_while_active(&g, 0), "at cap while still active");
    }

    #[test]
    fn exhausted_while_active_false_for_passive_or_terminal() {
        let passive = Goal::new("s", "obj", 0, 0); // Passive
        assert!(!exhausted_while_active(&passive, 0));
        let done = active_goal(3).with_status(GoalStatus::Complete, 0);
        assert!(!exhausted_while_active(&done, 0), "completed is not 'exhausted'");
    }

    #[test]
    fn cap_reached_note_mentions_the_cap() {
        let g = active_goal(7);
        assert!(cap_reached_note(&g).contains('7'));
        assert!(cap_reached_note(&g).contains("Blocked"));
    }
}
