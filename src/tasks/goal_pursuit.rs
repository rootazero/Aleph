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

use crate::goal::{GateOutcome, Goal, GoalStatus, PursuitMode};

/// Render accumulated lessons (the state file) for injection into a
/// continuation prompt. Empty → empty string (regression-safe: no prompt change
/// when there are no lessons). Newest last, matching their append order.
fn render_lessons(goal: &Goal) -> String {
    if goal.lessons.is_empty() {
        return String::new();
    }
    let mut s = String::from("\n\nLessons from earlier iterations (avoid repeating these):\n");
    for lesson in &goal.lessons {
        let trimmed: String = lesson.chars().take(300).collect();
        s.push_str(&format!("- {trimmed}\n"));
    }
    s
}

/// Pure decision: should this goal get one more autonomous continuation?
/// `tokens_now` is the session's current total-token count (pass 0 when a
/// live counter isn't available — then only the iteration cap applies).
/// `now_ms` is the current wall-clock (Unix epoch ms); pass 0 when no clock is
/// available — a set deadline is then NOT enforced (the iteration cap remains
/// the backstop), keeping clock-less callers behavior-identical.
#[must_use]
pub fn should_continue(goal: &Goal, tokens_now: u64, now_ms: u64) -> bool {
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
    if let Some(deadline) = goal.deadline_ms {
        if now_ms != 0 && now_ms > deadline {
            return false; // wall-clock budget exhausted.
        }
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
#[must_use]
pub fn continuation_prompt(goal: &Goal) -> String {
    let (this_iter, max_iter) = match goal.pursuit {
        PursuitMode::Active { max_iterations } => {
            (goal.continuations_used.saturating_add(1), max_iterations)
        }
        // Passive goals never reach the continuation path; stay graceful.
        PursuitMode::Passive => (goal.continuations_used.saturating_add(1), 0),
    };
    let is_final = max_iter != 0 && this_iter >= max_iter;
    let lessons = render_lessons(goal);
    if is_final {
        format!(
            "[Final autonomous iteration {this_iter}/{max_iter} toward your \
             standing goal]\nGoal: {}{lessons}\n\nThis is your LAST autonomous step — no \
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
             {this_iter}/{max_iter}]\nGoal: {}{lessons}\n\nTake the next concrete step; \
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
#[must_use]
pub fn exhausted_while_active(goal: &Goal, tokens_now: u64, now_ms: u64) -> bool {
    matches!(goal.pursuit, PursuitMode::Active { .. })
        && goal.status == GoalStatus::Active
        && !should_continue(goal, tokens_now, now_ms)
}

/// Human-readable note stamped on a goal when autonomous pursuit is cut off by
/// the caps, so the user sees why the loop stopped on their next turn.
#[must_use]
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

/// Note stamped when autonomous pursuit is cut off specifically by the
/// wall-clock deadline (distinct from the iteration cap). The continuation hook
/// picks this over `cap_reached_note` when the deadline was the binding stop.
#[must_use]
pub fn deadline_reached_note(_goal: &Goal) -> String {
    "Autonomous pursuit reached its wall-clock budget without completing. \
     Blocked for your guidance — review progress, then clear or re-set the \
     goal to continue."
        .to_string()
}

/// 模型在 `Active` 续跑下自报 `Complete`，但客观闸门尚未确认。
/// 调用方据此在续跑钩子里跑闸门。被动/交互 goal（非 Active 续跑）永远
/// 返回 false——它们的 complete 立即终止，不经闸门。
#[must_use]
pub fn awaiting_gate(goal: &Goal, gate_configured: bool) -> bool {
    gate_configured
        && matches!(goal.pursuit, PursuitMode::Active { .. })
        && goal.status == GoalStatus::Complete
        && goal.gate_outcome == GateOutcome::Unchecked
}

/// 闸门通过：完成被确认（`gate_outcome = Passed`），循环终止。
#[must_use]
pub fn confirm_complete(goal: &Goal, now_ms: u64) -> Goal {
    goal.clone().with_gate_outcome(GateOutcome::Passed, now_ms)
}

/// 闸门否决（Ralph Wiggum 营救）：把误报完成的 goal 退回 `Active` 并把
/// 闸门失败原因写入 `note`，让下一次续跑能据此行动。若迭代上限已耗尽，
/// 退回 Active 会立刻再次 exhaust——直接转 `Blocked`（复用 `cap_reached_note`，
/// 不复制 Blocked 逻辑）。无论哪条路径都把 `gate_outcome` 复位 `Unchecked`，
/// 保证下一次 complete 主张会被重新 gate。
#[must_use]
pub fn reopen_after_gate_failure(goal: &Goal, reason: &str, now_ms: u64) -> Goal {
    let cap_spent = match goal.pursuit {
        PursuitMode::Active { max_iterations } => goal.continuations_used >= max_iterations,
        PursuitMode::Passive => true,
    };
    let trimmed_lesson: String = format!("Objective gate vetoed: {reason}")
        .chars()
        .take(300)
        .collect();
    if cap_spent {
        let note = cap_reached_note(goal);
        goal.clone()
            .with_status(GoalStatus::Blocked, now_ms)
            .with_note(Some(note), now_ms)
            .with_gate_outcome(GateOutcome::Unchecked, now_ms)
            .with_lesson_appended(trimmed_lesson, now_ms)
    } else {
        let trimmed: String = reason.chars().take(300).collect();
        let note = format!("Objective gate vetoed completion: {trimmed}");
        goal.clone()
            .with_status(GoalStatus::Active, now_ms)
            .with_note(Some(note), now_ms)
            .with_gate_outcome(GateOutcome::Unchecked, now_ms)
            .with_lesson_appended(trimmed_lesson, now_ms)
    }
}

/// 闸门失败后的续跑 prompt——把客观失败信号注入下一轮（R9 智慧在 prompt）。
#[must_use]
pub fn gate_failure_prompt(goal: &Goal, reason: &str) -> String {
    let trimmed: String = reason.chars().take(600).collect();
    let lessons = render_lessons(goal);
    format!(
        "[Your standing goal is NOT done — the objective gate rejected your \
         completion claim]\nGoal: {}{lessons}\n\nThe automated gate (tests / build / \
         lint) failed with:\n{trimmed}\n\nThis is an objective signal, not an \
         opinion. Fix what the gate flagged, then call goal(action='update', \
         status='complete') again only when the work truly passes. If you \
         cannot resolve it, call goal(action='update', status='blocked') with \
         a note describing what remains.",
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
        assert!(!should_continue(&g, 0, 0));
    }

    #[test]
    fn active_within_caps_continues() {
        let g = active_goal(5); // continuations_used = 0
        assert!(should_continue(&g, 0, 0));
    }

    #[test]
    fn stops_at_iteration_cap() {
        let mut g = active_goal(3);
        g.continuations_used = 3;
        assert!(!should_continue(&g, 0, 0));
    }

    #[test]
    fn stops_when_complete() {
        let g = active_goal(5).with_status(GoalStatus::Complete, 0);
        assert!(!should_continue(&g, 0, 0));
    }

    #[test]
    fn stops_when_over_budget() {
        let g = active_goal(5).with_budget(Some(100));
        // tokens_at_start=0, so tokens_used(250)=250 > 100 → over budget.
        assert!(!should_continue(&g, 250, 0));
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
        assert!(
            !exhausted_while_active(&g, 0, 0),
            "fresh goal can still continue"
        );
        g.continuations_used = 3;
        assert!(exhausted_while_active(&g, 0, 0), "at cap while still active");
    }

    #[test]
    fn exhausted_while_active_false_for_passive_or_terminal() {
        let passive = Goal::new("s", "obj", 0, 0); // Passive
        assert!(!exhausted_while_active(&passive, 0, 0));
        let done = active_goal(3).with_status(GoalStatus::Complete, 0);
        assert!(
            !exhausted_while_active(&done, 0, 0),
            "completed is not 'exhausted'"
        );
    }

    #[test]
    fn cap_reached_note_mentions_the_cap() {
        let g = active_goal(7);
        assert!(cap_reached_note(&g).contains('7'));
        assert!(cap_reached_note(&g).contains("Blocked"));
    }

    #[test]
    fn awaiting_gate_true_only_for_active_pursuit_complete_unchecked() {
        let mut g = active_goal(5);
        g = g.with_status(GoalStatus::Complete, 1);
        // Active pursuit + Complete + Unchecked + gate → true
        assert!(awaiting_gate(&g, true));
        // 无闸门 → false（回归保护：无 stop_hooks 用户行为不变）
        assert!(!awaiting_gate(&g, false));
        // 已 Passed → false（不重复 gate）
        let passed = g.clone().with_gate_outcome(GateOutcome::Passed, 2);
        assert!(!awaiting_gate(&passed, true));
        // 仍 Active 状态（未自报 complete）→ false
        assert!(!awaiting_gate(&active_goal(5), true));
        // 被动 goal → false（交互 complete 不经闸门）
        let mut passive = Goal::new("s", "o", 0, 0);
        passive = passive.with_status(GoalStatus::Complete, 1);
        assert!(!awaiting_gate(&passive, true));
    }

    #[test]
    fn confirm_complete_sets_passed() {
        let g = active_goal(5).with_status(GoalStatus::Complete, 1);
        let c = confirm_complete(&g, 9);
        assert_eq!(c.gate_outcome, GateOutcome::Passed);
        assert_eq!(c.updated_at_ms, 9);
    }

    #[test]
    fn reopen_after_gate_failure_reopens_active_when_cap_remaining() {
        let g = active_goal(5).with_status(GoalStatus::Complete, 1);
        let r = reopen_after_gate_failure(&g, "tests failed: 3 errors", 9);
        assert_eq!(r.status, GoalStatus::Active);
        assert_eq!(r.gate_outcome, GateOutcome::Unchecked);
        assert!(r.note.unwrap().contains("tests failed"));
    }

    #[test]
    fn reopen_after_gate_failure_blocks_when_cap_spent() {
        let mut g = active_goal(3).with_status(GoalStatus::Complete, 1);
        g.continuations_used = 3; // cap 已满
        let r = reopen_after_gate_failure(&g, "still red", 9);
        assert_eq!(r.status, GoalStatus::Blocked);
        assert!(r.note.unwrap().contains("Blocked"));
    }

    #[test]
    fn gate_failure_prompt_restates_goal_and_reason() {
        let g = active_goal(5);
        let p = gate_failure_prompt(&g, "lint: 2 warnings");
        assert!(p.contains(&g.objective));
        assert!(p.contains("lint: 2 warnings"));
        assert!(p.contains("objective gate"));
    }

    #[test]
    fn reopen_after_gate_failure_appends_lesson() {
        let g = active_goal(5).with_status(GoalStatus::Complete, 1);
        let r = reopen_after_gate_failure(&g, "tests failed: 3 errors", 9);
        assert_eq!(r.lessons.len(), 1);
        assert!(r.lessons[0].contains("tests failed: 3 errors"));
        assert!(r.lessons[0].contains("Objective gate vetoed"));
    }

    #[test]
    fn continuation_prompt_includes_prior_lessons() {
        let g = active_goal(5)
            .with_lesson_appended("forgot to run migrations".into(), 2);
        let p = continuation_prompt(&g);
        assert!(p.contains("Lessons from earlier iterations"), "got: {p}");
        assert!(p.contains("forgot to run migrations"));
    }

    #[test]
    fn continuation_prompt_unchanged_when_no_lessons() {
        let g = active_goal(5);
        assert!(!continuation_prompt(&g).contains("Lessons from earlier"));
    }

    #[test]
    fn gate_failure_prompt_includes_prior_lessons() {
        let g = active_goal(5).with_lesson_appended("missing index".into(), 2);
        let p = gate_failure_prompt(&g, "still red");
        assert!(p.contains("missing index"));
        assert!(p.contains("still red"));
    }

    #[test]
    fn stops_when_past_deadline() {
        let g = active_goal(5).with_deadline_ms(Some(1_000));
        assert!(should_continue(&g, 0, 999), "before deadline → continue");
        assert!(!should_continue(&g, 0, 1_001), "past deadline → stop");
    }

    #[test]
    fn deadline_ignored_without_clock() {
        // now_ms == 0 means "no clock" — a set deadline must NOT fire, so
        // clock-less callers keep iteration-cap-only behavior.
        let g = active_goal(5).with_deadline_ms(Some(1_000));
        assert!(should_continue(&g, 0, 0));
    }

    #[test]
    fn exhausted_when_past_deadline_even_with_iterations_left() {
        let g = active_goal(5).with_deadline_ms(Some(1_000));
        // iterations remain (0/5) but the wall-clock budget is spent.
        assert!(exhausted_while_active(&g, 0, 2_000));
    }

    #[test]
    fn deadline_reached_note_mentions_wall_clock() {
        let g = active_goal(5);
        assert!(deadline_reached_note(&g).to_lowercase().contains("wall-clock"));
    }
}
