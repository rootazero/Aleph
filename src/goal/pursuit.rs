//! Goal pursuit — the R7/R10-safe autonomous continuation decisions.
//!
//! Pure functions only: given a [`Goal`] plus the live token count and clock,
//! decide whether one more autonomous continuation runs, what prompt it
//! carries, and what note explains a structural stop. The *driver* that acts on
//! these decisions is the gateway continuation hook
//! (`src/gateway/execution_engine/goal_continuation.rs`), and the atomic claim
//! that serializes them is [`crate::goal::GoalStore::try_claim_continuation`].
//! The cron executor is deliberately NOT used: it runs under a distinct
//! `agent:<id>:cron:<job>` session key, so the goal — stored under the
//! interactive session key — would never be found. Completion is decided
//! solely by the model calling `goal(update, complete)` (read here as plain
//! state — no judgment); iteration/token/deadline caps are structural
//! backstops. Sibling of `looping::pursuit` (the clock-gated variant); never
//! in `src/harness/` (R10 12-file redline).

use crate::goal::{GateOutcome, Goal, GoalStatus, PursuitMode};
use crate::looping::types::fmt_duration_ms;

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

/// The OTHER two structural budgets the loop silently enforces — the wall-clock
/// deadline and the token budget — rendered as a remaining-quota clause for the
/// continuation prompt. The iteration pace is already in the prompt's header, so
/// this only adds what the model would otherwise be blind to: a goal cut off at
/// its token budget used to just stop mid-work, the binding constraint invisible.
/// Mirrors `looping::pursuit::tick_prompt`'s quota clause (loop hardening ⑧);
/// `tokens_now` is the session's live cumulative total (0 = unavailable → the
/// budget clause is omitted rather than lying about the remainder).
fn render_quota(goal: &Goal, tokens_now: u64, now_ms: u64) -> String {
    let mut quota = String::new();
    if let Some(deadline) = goal.deadline_ms {
        if now_ms != 0 && deadline > now_ms {
            quota.push_str(&format!(
                " ~{} of wall-clock budget left.",
                fmt_duration_ms(deadline - now_ms)
            ));
        }
    }
    if let Some(budget) = goal.token_budget {
        if goal.baseline_captured && tokens_now > 0 {
            quota.push_str(&format!(
                " ~{} of {budget} token budget left.",
                budget.saturating_sub(goal.tokens_used(tokens_now))
            ));
        }
    }
    quota
}

/// Continuation prompt re-stating the goal (hermes parity), used when
/// enqueuing the next autonomous run.
///
/// Iteration-aware: this continuation is the `(continuations_used + 1)`-th
/// autonomous step. The pace (`N/max`) is surfaced so the model can self-budget
/// (R9 — intelligence in the prompt), together with the remaining wall-clock and
/// token budget (see [`render_quota`]). On the FINAL allowed iteration it
/// switches to a wrap-up prompt (hermes "grace call" parity) so the model
/// concludes gracefully instead of being cut mid-thought when the next hook
/// stops it. `tokens_now` / `now_ms` may be 0 when unavailable — the
/// corresponding clause is then simply omitted.
#[must_use]
pub fn continuation_prompt(goal: &Goal, tokens_now: u64, now_ms: u64) -> String {
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
        let quota = render_quota(goal, tokens_now, now_ms);
        format!(
            "[Continuing toward your standing goal — autonomous iteration \
             {this_iter}/{max_iter}]\nGoal: {}{lessons}\n\nTake the next concrete step; \
             pace yourself against the {remaining} continuation(s) remaining after \
             this one.{quota} {AUDIT_CONTRACT} If you have achieved the goal, call \
             goal(action='update', status='complete') and stop. If you are blocked \
             and need the user, call goal(action='update', status='blocked') and stop.",
            goal.objective,
        )
    }
}

/// The two failure modes an unattended pursuit falls into, closed in the prompt
/// rather than in code (R9 — the harness stays dumb): declaring victory on a
/// quietly-shrunk objective, and bailing to `blocked` on the first obstacle.
/// Condensed from codex's continuation contract (`goals/continuation.md`), whose
/// audit sections exist for exactly these two.
const AUDIT_CONTRACT: &str = "Before claiming complete, audit against the \
    objective as the user stated it — not a narrower or easier restatement of it — \
    and treat uncertain or indirect evidence as NOT achieved. Before reporting \
    blocked, make sure the same obstacle has survived a real attempt to work \
    around it; a first-try failure is not a blocker.";

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
             progress, then resume in place by updating status='active' with a \
             higher pursuit_max_iterations, or clear/re-set the goal."
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
     Blocked for your guidance — review progress, then resume by updating \
     status='active' with a fresh timeout_minutes, or clear/re-set the goal."
        .to_string()
}

/// Note stamped when autonomous pursuit is cut off specifically by the token
/// budget (distinct from the iteration cap and the wall-clock deadline). The
/// continuation hook picks this when `over_budget` is the binding stop reason,
/// so the user sees the true cause instead of a misleading "iteration cap"
/// message.
#[must_use]
pub fn budget_reached_note(goal: &Goal) -> String {
    match goal.token_budget {
        Some(budget) => format!(
            "Autonomous pursuit reached its token budget ({budget} tokens) \
             without completing. Blocked for your guidance — review progress, \
             then resume by updating status='active' with a higher token_budget, \
             or clear/re-set the goal."
        ),
        // No budget set → token budget cannot be the binding stop; fall back.
        None => cap_reached_note(goal),
    }
}

/// Pick the note that explains WHY autonomous pursuit stopped, from the three
/// structural stops `should_continue` enforces (token budget / wall-clock
/// deadline / iteration cap). Single source for the continuation hook, which
/// previously hand-rolled this three-way choice inline — the same collapse
/// `looping::pursuit::stop_reason_note` did for the loop (loop hardening ④ 熵减).
#[must_use]
pub fn stop_reason_note(goal: &Goal, tokens_now: u64, now_ms: u64) -> String {
    if goal.over_budget(tokens_now) {
        budget_reached_note(goal)
    } else if goal.deadline_ms.is_some_and(|d| now_ms != 0 && now_ms > d) {
        deadline_reached_note(goal)
    } else {
        cap_reached_note(goal)
    }
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

/// Active 续跑的 `Complete` 且**没有任何闸门可仲裁**：权威终态——续跑钩子
/// 据此清理焊入的 strategy weld。工具侧刻意不清这个 case
/// （`builtin_tools/goal.rs`：gate veto 要能带着计划复活 goal），而
/// `try_claim_continuation` 对它返回 `Idle`（`awaiting_gate` 要求
/// `gate_configured`），所以钩子的 Idle 臂是唯一能看见它的位置。
/// 2026-07-12 重构曾把这条清理分支丢失（weld 泄漏回归，round-4 修复）；
/// 单独成谓词让不变量可测试。gate-Passed 的 `Complete` 只在
/// `gate_configured` 下产生并经 `commit_gate_pass` 清理，不会命中此谓词。
#[must_use]
pub fn gateless_terminal_complete(goal: &Goal, gate_configured: bool) -> bool {
    !gate_configured
        && matches!(goal.pursuit, PursuitMode::Active { .. })
        && goal.status == GoalStatus::Complete
}

/// 闸门通过：完成被确认（`gate_outcome = Passed`），循环终止。
#[must_use]
pub fn confirm_complete(goal: &Goal, now_ms: u64) -> Goal {
    goal.clone().with_gate_outcome(GateOutcome::Passed, now_ms)
}

/// 闸门否决（Ralph Wiggum 营救）：把误报完成的 goal 退回 `Active` 并把
/// 闸门失败原因写入 `note`，让下一次续跑能据此行动。若**任一**结构上限已耗尽
/// （迭代 / 墙钟 deadline / token 预算），退回 Active 会立刻再次 exhaust——
/// 直接转 `Blocked`，note 说明真正的绑定上限（`stop_reason_note`）。
/// 无论哪条路径都把 `gate_outcome` 复位 `Unchecked`，保证下一次 complete
/// 主张会被重新 gate。
///
/// `tokens_now` / `now_ms` 与 `should_continue` 同义（0 = 该维度不可用，不参与
/// 判定），所以"还有跑道吗"这个问题在整个子系统里只有一个答案来源。
#[must_use]
pub fn reopen_after_gate_failure(goal: &Goal, reason: &str, tokens_now: u64, now_ms: u64) -> Goal {
    let trimmed_lesson: String = format!("Objective gate vetoed: {reason}")
        .chars()
        .take(300)
        .collect();
    // Reopen only if the goal could actually run again — the iteration cap was
    // the only limit consulted here before, so a veto happily reopened a goal
    // whose deadline had passed or whose token budget was spent, and the very
    // next hook re-blocked it (one wasted autonomous run + a contradictory note).
    let reopened = goal.clone().with_status(GoalStatus::Active, now_ms);
    if should_continue(&reopened, tokens_now, now_ms) {
        let trimmed: String = reason.chars().take(300).collect();
        let note = format!("Objective gate vetoed completion: {trimmed}");
        reopened
            .with_note(Some(note), now_ms)
            .with_gate_outcome(GateOutcome::Unchecked, now_ms)
            .with_lesson_appended(trimmed_lesson, now_ms)
    } else {
        let note = stop_reason_note(goal, tokens_now, now_ms);
        goal.clone()
            .with_status(GoalStatus::Blocked, now_ms)
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
    fn higher_cap_re_enables_continuation() {
        // The resume mechanism behind goal(update, pursuit_max_iterations=…):
        // a goal exhausted at its cap regains runway the instant the cap is
        // raised above the used count — no clear+set, no lost lessons.
        let mut g = active_goal(3);
        g.continuations_used = 3;
        assert!(!should_continue(&g, 0, 0), "exhausted at cap");
        let resumed = g.with_pursuit(PursuitMode::Active { max_iterations: 8 });
        assert!(
            should_continue(&resumed, 0, 0),
            "raising the cap above the used count resumes pursuit"
        );
    }

    #[test]
    fn stops_when_complete() {
        let g = active_goal(5).with_status(GoalStatus::Complete, 0);
        assert!(!should_continue(&g, 0, 0));
    }

    #[test]
    fn stops_when_over_budget() {
        let g = active_goal(5).with_budget(Some(100)).with_baseline(0, 0);
        // baseline at 0, so tokens_used(250)=250 > 100 → over budget.
        assert!(!should_continue(&g, 250, 0));
        // …but an uncaptured baseline never enforces the budget (the tool's
        // placeholder 0 would otherwise read as "the whole session was spent").
        let no_baseline = active_goal(5).with_budget(Some(100));
        assert!(should_continue(&no_baseline, 250, 0));
    }

    #[test]
    fn continuation_prompt_restates_objective() {
        let g = active_goal(5);
        assert!(continuation_prompt(&g, 0, 0).contains("obj"));
        assert!(continuation_prompt(&g, 0, 0).contains("status='complete'"));
    }

    #[test]
    fn continuation_prompt_surfaces_pace_when_not_final() {
        let g = active_goal(5); // continuations_used = 0 → this is iteration 1/5
        let p = continuation_prompt(&g, 0, 0);
        assert!(p.contains("iteration 1/5"), "got: {p}");
        assert!(p.contains("4 continuation"), "remaining shown: {p}");
        assert!(!p.contains("Final autonomous"));
    }

    #[test]
    fn continuation_prompt_is_grace_call_on_final_iteration() {
        let mut g = active_goal(3);
        g.continuations_used = 2; // about to run iteration 3/3 — the last one
        let p = continuation_prompt(&g, 0, 0);
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
        assert!(
            exhausted_while_active(&g, 0, 0),
            "at cap while still active"
        );
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
    fn gateless_terminal_complete_is_the_hook_owned_weld_clear_case() {
        let done = active_goal(5).with_status(GoalStatus::Complete, 1);
        // Active pursuit + Complete + NO gate → the hook must clear the weld
        // (the 2026-07-12 refactor regression this predicate guards against).
        assert!(gateless_terminal_complete(&done, false));
        // A configured gate owns arbitration — never both paths at once.
        assert!(!gateless_terminal_complete(&done, true));
        assert!(
            !(awaiting_gate(&done, true) && gateless_terminal_complete(&done, true)),
            "gate arbitration and gate-less clear are mutually exclusive"
        );
        // Still-active / passive / blocked goals are not terminal completes.
        assert!(!gateless_terminal_complete(&active_goal(5), false));
        let passive = Goal::new("s", "o", 0, 0).with_status(GoalStatus::Complete, 1);
        assert!(!gateless_terminal_complete(&passive, false));
        let blocked = active_goal(5).with_status(GoalStatus::Blocked, 1);
        assert!(!gateless_terminal_complete(&blocked, false));
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
        let r = reopen_after_gate_failure(&g, "tests failed: 3 errors", 0, 9);
        assert_eq!(r.status, GoalStatus::Active);
        assert_eq!(r.gate_outcome, GateOutcome::Unchecked);
        assert!(r.note.unwrap().contains("tests failed"));
    }

    #[test]
    fn reopen_after_gate_failure_blocks_when_cap_spent() {
        let mut g = active_goal(3).with_status(GoalStatus::Complete, 1);
        g.continuations_used = 3; // cap 已满
        let r = reopen_after_gate_failure(&g, "still red", 0, 9);
        assert_eq!(r.status, GoalStatus::Blocked);
        assert!(r.note.unwrap().contains("Blocked"));
    }

    #[test]
    fn reopen_after_gate_failure_blocks_when_the_deadline_is_spent() {
        // Iterations remain (0/5) but the wall clock is gone: reopening would hand
        // the goal runway it does not have, and the next hook would re-block it
        // with a contradictory note. Only the iteration cap was checked before.
        let g = active_goal(5)
            .with_deadline_ms(Some(1_000))
            .with_status(GoalStatus::Complete, 1);
        let r = reopen_after_gate_failure(&g, "still red", 0, 2_000);
        assert_eq!(r.status, GoalStatus::Blocked);
        assert!(r.note.unwrap().to_lowercase().contains("wall-clock"));
        assert_eq!(r.gate_outcome, GateOutcome::Unchecked);
    }

    #[test]
    fn reopen_after_gate_failure_blocks_when_the_token_budget_is_spent() {
        let g = active_goal(5)
            .with_budget(Some(100))
            .with_baseline(0, 0)
            .with_status(GoalStatus::Complete, 1);
        let r = reopen_after_gate_failure(&g, "still red", 250, 9);
        assert_eq!(r.status, GoalStatus::Blocked);
        assert!(r.note.unwrap().contains("token budget"));
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
        let r = reopen_after_gate_failure(&g, "tests failed: 3 errors", 0, 9);
        assert_eq!(r.lessons.len(), 1);
        assert!(r.lessons[0].contains("tests failed: 3 errors"));
        assert!(r.lessons[0].contains("Objective gate vetoed"));
    }

    #[test]
    fn continuation_prompt_includes_prior_lessons() {
        let g = active_goal(5).with_lesson_appended("forgot to run migrations".into(), 2);
        let p = continuation_prompt(&g, 0, 0);
        assert!(p.contains("Lessons from earlier iterations"), "got: {p}");
        assert!(p.contains("forgot to run migrations"));
    }

    #[test]
    fn continuation_prompt_unchanged_when_no_lessons() {
        let g = active_goal(5);
        assert!(!continuation_prompt(&g, 0, 0).contains("Lessons from earlier"));
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
        assert!(deadline_reached_note(&g)
            .to_lowercase()
            .contains("wall-clock"));
    }

    #[test]
    fn budget_reached_note_mentions_token_budget() {
        let g = active_goal(5).with_budget(Some(50_000));
        let note = budget_reached_note(&g);
        assert!(note.contains("token budget"), "got: {note}");
        assert!(note.contains("50000"), "states the budget: {note}");
    }

    #[test]
    fn budget_reached_note_falls_back_without_budget() {
        // No budget set → cannot be the binding stop; reuse the cap note.
        let g = active_goal(7);
        assert_eq!(budget_reached_note(&g), cap_reached_note(&g));
    }

    #[test]
    fn continuation_prompt_surfaces_remaining_token_budget() {
        // The binding constraint the hook silently enforces must be visible to
        // the model (loop's tick_prompt parity): 1000-budget goal, baseline at
        // 10_000, live total 10_400 → ~600 left.
        let g = active_goal(5)
            .with_budget(Some(1_000))
            .with_baseline(10_000, 1);
        let p = continuation_prompt(&g, 10_400, 0);
        assert!(p.contains("~600 of 1000 token budget left"), "got: {p}");
    }

    #[test]
    fn continuation_prompt_omits_budget_clause_without_a_live_count() {
        // No baseline captured / no live counter → say nothing rather than
        // report a bogus remainder.
        let g = active_goal(5).with_budget(Some(1_000));
        assert!(!continuation_prompt(&g, 0, 0).contains("token budget left"));
    }

    #[test]
    fn continuation_prompt_surfaces_remaining_wall_clock() {
        let g = active_goal(5).with_deadline_ms(Some(600_000));
        let p = continuation_prompt(&g, 0, 300_000); // 5 minutes left
        assert!(p.contains("of wall-clock budget left"), "got: {p}");
        assert!(p.contains("5m"), "got: {p}");
        // Clock-less callers get no clause (no lying countdown).
        assert!(!continuation_prompt(&g, 0, 0).contains("wall-clock budget left"));
    }

    #[test]
    fn stop_reason_note_picks_the_binding_limit() {
        // Token budget binds first.
        let over = active_goal(5).with_budget(Some(100)).with_baseline(0, 0);
        assert!(stop_reason_note(&over, 250, 0).contains("token budget"));
        // Then the wall clock.
        let late = active_goal(5).with_deadline_ms(Some(1_000));
        assert!(stop_reason_note(&late, 0, 2_000)
            .to_lowercase()
            .contains("wall-clock"));
        // Otherwise the iteration cap.
        let mut capped = active_goal(3);
        capped.continuations_used = 3;
        assert!(stop_reason_note(&capped, 0, 0).contains("iteration cap"));
    }

    #[test]
    fn over_budget_with_captured_baseline_stops_continuation() {
        // End-to-end of the wiring fix: a real baseline + live token count makes
        // `should_continue` enforce the budget (previously dead — tokens_now was
        // hardcoded to 0 at the call site).
        let g = active_goal(5)
            .with_budget(Some(1_000))
            .with_baseline(10_000, 1);
        assert!(should_continue(&g, 10_500, 0), "500 spent < 1000 budget");
        assert!(!should_continue(&g, 11_200, 0), "1200 spent > 1000 budget");
        assert!(
            exhausted_while_active(&g, 11_200, 0),
            "budget is binding stop"
        );
    }
}
