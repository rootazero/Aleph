//! Pure gate functions for the loop continuation hook. No I/O, no `LoopState`
//! mutation — the execution engine calls these to decide whether to re-fire a
//! loop and how long to wait. Mirrors `goal::pursuit` but clock-gated.

use crate::looping::types::{fmt_duration_ms, Cadence, LoopState};

/// Has any safety cap been reached? `tokens_now` is the session's current
/// cumulative-token total; the hook captures the per-loop baseline lazily and
/// passes the live total here (0 when no budget is set or no counter is
/// available, in which case the token check is inert and only the
/// iteration/deadline caps apply).
#[must_use]
pub fn exhausted(state: &LoopState, tokens_now: u64, now_ms: u64) -> bool {
    if let Some(max) = state.max_iterations {
        if state.iterations_used >= max {
            return true;
        }
    }
    if state.over_budget(tokens_now) {
        return true; // soft token budget becomes a hard stop for the loop.
    }
    if let Some(deadline) = state.deadline_ms {
        // now_ms == 0 means clock unavailable — never trip on it.
        if now_ms != 0 && now_ms > deadline {
            return true;
        }
    }
    false
}

/// Should this loop fire another tick now? Active and no cap reached.
#[must_use]
pub fn should_fire(state: &LoopState, tokens_now: u64, now_ms: u64) -> bool {
    state.is_active() && !exhausted(state, tokens_now, now_ms)
}

/// Would a tick claimed now — waking at `wake_ms` — still be inside the
/// wall-clock bound when it actually EXECUTES?
///
/// [`exhausted`] answers "may this loop be claimed", which is a different
/// question: a claim happens at the end of a run, the tick runs one cadence
/// later, and nothing in between re-reads the clock (`confirm_fire` matches
/// the marker, not the deadline). Without this projection `timeout_minutes`
/// bounds only when a tick may be *scheduled*, so `interval='2h',
/// timeout_minutes=1` runs a full LLM turn 119 minutes past the limit the user
/// set, and only the claim AFTER that reports the loop stopped.
///
/// The tradeoff is deliberate: the loop now stops at the last claim whose wake
/// lands in-bounds — up to one interval EARLY rather than arbitrarily late.
/// A safety cap is an upper bound, not an approximation.
///
/// `now_ms == 0` (clock unavailable) fails open, matching [`exhausted`].
#[must_use]
pub fn fires_out_of_bounds(state: &LoopState, wake_ms: u64, now_ms: u64) -> bool {
    state
        .deadline_ms
        .is_some_and(|deadline| now_ms != 0 && wake_ms > deadline)
}

/// The delay a claim would use once the model-paced `next_wake` is consumed —
/// i.e. the cadence's own default. `tick_delay_ms` answers "how long until the
/// tick being claimed NOW"; this answers "and the one after that".
const fn cadence_default_ms(state: &LoopState) -> u64 {
    match state.cadence {
        Cadence::Fixed { interval_ms } => interval_ms,
        Cadence::ModelPaced { fallback_ms } => fallback_ms,
    }
}

/// Is the tick being claimed now the last one the wall-clock bound permits?
///
/// The tick fires at `now + tick_delay_ms`; the claim that follows it would
/// schedule one cadence later again, and [`fires_out_of_bounds`] will refuse
/// that. So this tick gets the same wrap-up handoff the iteration cap already
/// gets. It matters more here than there: `start` DROPS the default tick cap
/// when a `timeout_minutes` is given ("a deadline is itself a bound"), so for
/// deadline-bounded loops this is the only bound there is — and until now the
/// only bound with no final turn to report from.
#[must_use]
pub fn is_final_deadline_tick(state: &LoopState, now_ms: u64) -> bool {
    let Some(deadline) = state.deadline_ms else {
        return false;
    };
    if now_ms == 0 {
        return false; // clock unavailable — never claim finality
    }
    let this_wake = now_ms.saturating_add(tick_delay_ms(state, now_ms));
    this_wake.saturating_add(cadence_default_ms(state)) > deadline
}

/// How long to wait before the next tick, in ms.
#[must_use]
pub fn tick_delay_ms(state: &LoopState, now_ms: u64) -> u64 {
    match &state.cadence {
        Cadence::Fixed { interval_ms } => *interval_ms,
        Cadence::ModelPaced { fallback_ms } => match state.next_wake_ms {
            Some(wake) => wake.saturating_sub(now_ms),
            None => *fallback_ms,
        },
    }
}

/// The prompt re-injected each tick: the user's fixed prompt plus a one-line
/// reminder of the loop controls (so the model can re-pace or stop) and the
/// *remaining quota* (ticks left before the cap, time left until the deadline,
/// token budget left) so the model can self-pace and wrap up — the
/// intelligence lives in the prompt, not in deterministic no-progress
/// detection (R7/R9). Mirrors `goal_pursuit::continuation_prompt`'s pace
/// surfacing. `now_ms` (0 = clock unavailable) turns an absolute deadline into
/// a relative "time left"; `tokens_now` (0 = counter unavailable) turns the
/// token budget into a "left of budget" clause.
#[must_use]
pub fn tick_prompt(state: &LoopState, tokens_now: u64, now_ms: u64) -> String {
    let n = state.iterations_used.saturating_add(1);

    // Last tick permitted by whichever bound binds first. Iteration cap: `n >=
    // max`, the hook marks the loop exhausted before tick `n + 1`. Deadline:
    // the claim after this one would schedule a wake past the limit and be
    // refused. Either way this is the loop's only chance to hand off, so it
    // replaces the remaining-quota clause rather than adding to it.
    let capped = state.max_iterations.is_some_and(|max| n >= max);
    if capped || is_final_deadline_tick(state, now_ms) {
        let bound = if capped {
            "iteration cap"
        } else {
            "time limit"
        };
        return format!(
            "{prompt}\n\n[Timer loop — tick {n}. This is the LAST tick before the \
             {bound}; the loop will not fire again. Wrap up and report what \
             you found so the user can take over.]",
            prompt = state.prompt,
        );
    }

    // Remaining-quota clause so the model can pace itself against the bound it
    // is running under, exactly as goal pursuit surfaces "N/max".
    let mut quota = String::new();
    if let Some(max) = state.max_iterations {
        quota.push_str(&format!(" {} tick(s) left before the cap.", max - n));
    }
    if let Some(deadline) = state.deadline_ms {
        if now_ms != 0 && deadline > now_ms {
            quota.push_str(&format!(
                " ~{} until the time limit.",
                fmt_duration_ms(deadline - now_ms)
            ));
        }
    }
    // Surface the token budget the hook silently enforces — without this the
    // loop's binding constraint is invisible and it just stops mid-watch.
    if let Some(budget) = state.token_budget {
        if state.baseline_captured && tokens_now > 0 {
            quota.push_str(&format!(
                " ~{} of {budget} token budget left.",
                budget.saturating_sub(state.tokens_used(tokens_now))
            ));
        }
    }

    let pace = match state.cadence {
        Cadence::ModelPaced { .. } => {
            " To change the cadence, call loop(action='update', next_wake='8m')."
        }
        Cadence::Fixed { .. } => "",
    };
    format!(
        "{prompt}\n\n[Timer loop — tick {n}. This runs on a schedule and will \
         not self-stop.{quota} To end it, call loop(action='stop').{pace}]",
        prompt = state.prompt,
    )
}

/// User-facing reason when the iteration cap stops the loop.
#[must_use]
pub fn cap_reached_note(state: &LoopState) -> String {
    format!(
        "Loop stopped: reached the iteration cap ({} ticks).",
        state.max_iterations.unwrap_or(0)
    )
}

/// User-facing reason when the wall-clock deadline stops the loop.
#[must_use]
pub fn deadline_reached_note(_state: &LoopState) -> String {
    "Loop stopped: reached its time limit.".to_string()
}

/// User-facing reason when the token budget stops the loop. The hook picks this
/// over the iteration/deadline notes when `over_budget` is the binding stop, so
/// the user sees the true cause. Mirrors `goal_pursuit::budget_reached_note`.
#[must_use]
pub fn budget_reached_note(state: &LoopState) -> String {
    match state.token_budget {
        Some(budget) => format!("Loop stopped: reached its token budget ({budget} tokens)."),
        // No budget set → token budget cannot be the binding stop; fall back.
        None => cap_reached_note(state),
    }
}

/// Pick the user-facing reason for the *binding* exhaustion cause, in priority
/// order token-budget → deadline → iteration cap. Single source for the choice
/// the continuation hook previously inlined as a 3-way `if/else`, so the stored
/// `stop_reason` and any future surfacing stay consistent. Call only when
/// [`exhausted`] is true; otherwise it falls through to the iteration-cap note.
#[must_use]
pub fn stop_reason_note(state: &LoopState, tokens_now: u64, now_ms: u64) -> String {
    if state.over_budget(tokens_now) {
        budget_reached_note(state)
    } else if state.deadline_ms.is_some_and(|d| now_ms != 0 && now_ms > d) {
        deadline_reached_note(state)
    } else {
        cap_reached_note(state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::looping::types::{Cadence, LoopState, LoopStatus};

    fn fixed() -> LoopState {
        LoopState::new(
            "s",
            "check CI",
            Cadence::Fixed {
                interval_ms: 300_000,
            },
            1_000,
        )
    }

    #[test]
    fn active_uncapped_loop_fires() {
        assert!(should_fire(&fixed(), 0, 2_000));
    }

    #[test]
    fn stopped_loop_never_fires() {
        let l = fixed().with_status(LoopStatus::Stopped);
        assert!(!should_fire(&l, 0, 2_000));
    }

    #[test]
    fn loop_at_max_iterations_does_not_fire() {
        let mut l = fixed().with_max_iterations(Some(2));
        l = l.spent_iteration().spent_iteration(); // used == 2
        assert!(!should_fire(&l, 0, 2_000));
        assert!(exhausted(&l, 0, 2_000));
    }

    #[test]
    fn loop_below_max_iterations_fires() {
        let l = fixed().with_max_iterations(Some(2)).spent_iteration(); // used == 1
        assert!(should_fire(&l, 0, 2_000));
        assert!(!exhausted(&l, 0, 2_000));
    }

    #[test]
    fn past_deadline_does_not_fire() {
        let l = fixed().with_deadline_ms(Some(5_000));
        assert!(!should_fire(&l, 0, 6_000));
        assert!(exhausted(&l, 0, 6_000));
    }

    #[test]
    fn before_deadline_fires() {
        let l = fixed().with_deadline_ms(Some(10_000));
        assert!(should_fire(&l, 0, 6_000));
    }

    #[test]
    fn now_ms_zero_never_trips_deadline() {
        // now_ms == 0 means "clock unavailable" — must not falsely exhaust.
        let l = fixed().with_deadline_ms(Some(5_000));
        assert!(should_fire(&l, 0, 0));
    }

    #[test]
    fn fixed_cadence_delay_is_interval() {
        assert_eq!(tick_delay_ms(&fixed(), 2_000), 300_000);
    }

    #[test]
    fn model_paced_uses_next_wake_relative_to_now() {
        let l = LoopState::new(
            "s",
            "p",
            Cadence::ModelPaced {
                fallback_ms: 600_000,
            },
            0,
        )
        .with_next_wake_ms(Some(10_000));
        // next wake at epoch 10_000, now 4_000 → 6_000 ms from now.
        assert_eq!(tick_delay_ms(&l, 4_000), 6_000);
    }

    #[test]
    fn model_paced_past_next_wake_clamps_to_zero() {
        let l = LoopState::new(
            "s",
            "p",
            Cadence::ModelPaced {
                fallback_ms: 600_000,
            },
            0,
        )
        .with_next_wake_ms(Some(3_000));
        assert_eq!(tick_delay_ms(&l, 9_000), 0);
    }

    #[test]
    fn model_paced_without_next_wake_uses_fallback() {
        let l = LoopState::new(
            "s",
            "p",
            Cadence::ModelPaced {
                fallback_ms: 600_000,
            },
            0,
        );
        assert_eq!(tick_delay_ms(&l, 9_000), 600_000);
    }

    #[test]
    fn tick_prompt_restates_prompt_and_controls() {
        let p = tick_prompt(&fixed(), 0, 2_000);
        assert!(p.contains("check CI"));
        assert!(p.contains("action='stop'"));
    }

    #[test]
    fn tick_prompt_surfaces_remaining_ticks_before_cap() {
        // max 5, used 1 → this is tick 2, four left after it: "3 tick(s) left".
        let l = fixed().with_max_iterations(Some(5)).spent_iteration();
        let p = tick_prompt(&l, 0, 2_000);
        assert!(p.contains("3 tick(s) left before the cap"), "{p}");
        assert!(!p.contains("LAST tick"), "{p}");
    }

    #[test]
    fn tick_prompt_warns_on_last_tick() {
        // max 2, used 1 → this is tick 2 == cap: the loop will not fire again.
        let l = fixed().with_max_iterations(Some(2)).spent_iteration();
        let p = tick_prompt(&l, 0, 2_000);
        assert!(p.contains("LAST tick"), "{p}");
        assert!(p.contains("Wrap up"), "{p}");
        // No misleading "N tick(s) left" on the final tick.
        assert!(!p.contains("tick(s) left"), "{p}");
    }

    #[test]
    fn tick_prompt_surfaces_deadline_time_left() {
        // 5m cadence, 25m of window left at now=10_000: this tick fires at 5m
        // and another still fits, so the quota clause (not the wrap-up) applies.
        let l = fixed().with_deadline_ms(Some(1_510_000));
        let p = tick_prompt(&l, 0, 10_000);
        assert!(p.contains("~25m until the time limit"), "{p}");
        assert!(!p.contains("LAST tick"), "{p}");
    }

    #[test]
    fn tick_prompt_uncapped_loop_has_no_quota_clause() {
        let p = tick_prompt(&fixed(), 0, 2_000);
        assert!(!p.contains("tick(s) left"), "{p}");
        assert!(!p.contains("time limit"), "{p}");
        assert!(!p.contains("token budget"), "{p}");
    }

    #[test]
    fn tick_prompt_surfaces_token_budget_left() {
        // budget 5000, baseline 1000, live total 2500 → 1500 spent, 3500 left.
        let l = fixed().with_token_budget(Some(5_000)).with_baseline(1_000);
        let p = tick_prompt(&l, 2_500, 2_000);
        assert!(p.contains("~3500 of 5000 token budget left"), "{p}");
        // Counter unavailable (0) or baseline not captured → clause omitted.
        assert!(!tick_prompt(&l, 0, 2_000).contains("token budget"), "inert");
        let uncaptured = fixed().with_token_budget(Some(5_000));
        assert!(!tick_prompt(&uncaptured, 2_500, 2_000).contains("token budget"));
    }

    #[test]
    fn deadline_bounded_loop_gets_the_same_last_tick_handoff() {
        // 5m cadence, deadline 8m out: this tick fires at 5m, the next would be
        // at 10m and be refused — so this one must say so. Before the fix only
        // iteration-capped loops got a wrap-up turn, and `start` drops the tick
        // cap whenever a deadline is set, so this was the common case.
        let l = fixed().with_deadline_ms(Some(480_000));
        let p = tick_prompt(&l, 0, 1);
        assert!(p.contains("LAST tick before the time limit"), "{p}");
        assert!(p.contains("Wrap up"), "{p}");
        // The quota clause is replaced, not duplicated.
        assert!(!p.contains("until the time limit"), "{p}");
    }

    #[test]
    fn a_mid_window_tick_still_gets_the_plain_quota_clause() {
        // 5m cadence, 30m window: several ticks left, so no finality claim.
        let l = fixed().with_deadline_ms(Some(1_800_000));
        let p = tick_prompt(&l, 0, 1);
        assert!(!p.contains("LAST tick"), "{p}");
        assert!(p.contains("until the time limit"), "{p}");
    }

    #[test]
    fn iteration_cap_still_wins_the_wording_when_both_bind() {
        let l = fixed()
            .with_max_iterations(Some(1))
            .with_deadline_ms(Some(480_000));
        let p = tick_prompt(&l, 0, 1);
        assert!(p.contains("LAST tick before the iteration cap"), "{p}");
    }

    #[test]
    fn clock_unavailable_never_claims_a_final_tick() {
        let l = fixed().with_deadline_ms(Some(480_000));
        assert!(!is_final_deadline_tick(&l, 0));
        assert!(!tick_prompt(&l, 0, 0).contains("LAST tick"));
    }

    #[test]
    fn notes_mention_reason() {
        assert!(cap_reached_note(&fixed().with_max_iterations(Some(2))).contains("iteration"));
        assert!(deadline_reached_note(&fixed()).contains("time"));
    }

    #[test]
    fn over_budget_loop_does_not_fire_and_is_exhausted() {
        let l = fixed().with_token_budget(Some(500)).with_baseline(1_000);
        // 600 spent over a 500 budget.
        assert!(exhausted(&l, 1_600, 2_000));
        assert!(!should_fire(&l, 1_600, 2_000));
        // 200 spent under the budget → still fires.
        assert!(should_fire(&l, 1_200, 2_000));
        assert!(!exhausted(&l, 1_200, 2_000));
    }

    #[test]
    fn uncaptured_baseline_budget_is_inert() {
        // token_budget set but baseline not captured (tokens_at_start = 0):
        // a large live total would falsely exhaust if the hook passed it before
        // capturing — but `exhausted` itself just measures from baseline 0. The
        // hook is responsible for passing 0 until capture; here we assert the
        // pure function measures spend from the seeded baseline correctly.
        let l = fixed().with_token_budget(Some(500)).with_baseline(2_000);
        assert!(!exhausted(&l, 2_000, 2_000), "no spend at baseline");
        assert!(
            exhausted(&l, 2_600, 2_000),
            "600 over budget after baseline"
        );
    }

    #[test]
    fn stop_reason_note_picks_binding_cause_in_priority_order() {
        // Token budget binds first.
        let budgeted = fixed()
            .with_token_budget(Some(500))
            .with_baseline(1_000)
            .with_deadline_ms(Some(5_000))
            .with_max_iterations(Some(2));
        assert!(stop_reason_note(&budgeted, 1_600, 6_000).contains("token budget"));
        // No budget overrun → deadline binds next.
        let timed = fixed()
            .with_deadline_ms(Some(5_000))
            .with_max_iterations(Some(2));
        assert!(stop_reason_note(&timed, 0, 6_000).contains("time"));
        // Neither → iteration cap.
        let capped = fixed().with_max_iterations(Some(2));
        assert!(stop_reason_note(&capped, 0, 2_000).contains("iteration"));
    }

    #[test]
    fn budget_note_distinguishes_token_stop() {
        let l = fixed().with_token_budget(Some(500));
        assert!(budget_reached_note(&l).contains("token budget"));
        // No budget → falls back to the iteration-cap note.
        let no_budget = fixed().with_max_iterations(Some(3));
        assert!(budget_reached_note(&no_budget).contains("iteration"));
    }
}
