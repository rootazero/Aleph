//! Pure gate functions for the loop continuation hook. No I/O, no `LoopState`
//! mutation — the execution engine calls these to decide whether to re-fire a
//! loop and how long to wait. Mirrors `tasks::goal_pursuit` but clock-gated.

use crate::looping::types::{Cadence, LoopState};

/// Has any safety cap been reached? `tokens_now` is accepted for parity with
/// goal but the hook passes 0 today (token accounting not wired through).
#[must_use]
pub fn exhausted(state: &LoopState, _tokens_now: u64, now_ms: u64) -> bool {
    if let Some(max) = state.max_iterations {
        if state.iterations_used >= max {
            return true;
        }
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
/// reminder of the loop controls (so the model can re-pace or stop). Kept tiny
/// — the intelligence lives in the model, not this scaffold (R9).
#[must_use]
pub fn tick_prompt(state: &LoopState) -> String {
    let n = state.iterations_used.saturating_add(1);
    let pace = match state.cadence {
        Cadence::ModelPaced { .. } => {
            " To change the cadence, call loop(action='update', next_wake='8m')."
        }
        Cadence::Fixed { .. } => "",
    };
    format!(
        "{prompt}\n\n[Timer loop — tick {n}. This runs on a schedule and will \
         not self-stop. To end it, call loop(action='stop').{pace}]",
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
        let p = tick_prompt(&fixed());
        assert!(p.contains("check CI"));
        assert!(p.contains("action='stop'"));
    }

    #[test]
    fn notes_mention_reason() {
        assert!(cap_reached_note(&fixed().with_max_iterations(Some(2))).contains("iteration"));
        assert!(deadline_reached_note(&fixed()).contains("time"));
    }
}
