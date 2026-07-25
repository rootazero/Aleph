//! Client-side typewriter reveal pacing.
//!
//! Wires the `behavior.typing_speed` (chars-per-second) config into the
//! Panel's streaming renderer. The backend streams *real* chunks throttled for
//! network efficiency (`reply_emitter` `debounce_ms`); the rate at which the
//! user *sees* characters appear is a pure presentation concern and lives here
//! (R10: pacing is presentation, not cognition). This mirrors hermes-agent's
//! client-side delta pacing rather than throttling the core's emit path.
//!
//! Before this, `typing_speed` was a configured-but-dead knob — defined,
//! validated (50-400), surfaced in the Panel slider + i18n + example TOML, yet
//! consumed by nothing. The slider did nothing. This module is its consumer.
//!
//! # Why a counter, not a start-timestamp
//!
//! An earlier version paced reveal as `floor((now - reveal_start) * cps)` — a
//! wall-clock offset from a single start stamp. That had two fatal flaws for a
//! real agent stream:
//!
//! 1. **Completion dumped the tail.** Reveal only ran while `is_streaming` was
//!    true; the instant the backend finished (`run_complete`) the bubble
//!    switched to a full render, discarding whatever the sweep had not yet
//!    reached. For any response that generates faster than `cps` (nearly all of
//!    them at 200 cps), the text appeared all at once — the typewriter was
//!    invisible.
//! 2. **Pauses banked budget.** A tool call or model stall mid-turn let elapsed
//!    time accrue while no text arrived; when text resumed, `elapsed * cps` had
//!    run far ahead of the content and dumped it with no pacing.
//!
//! The counter model fixes both. Each message owns a [`Reveal`] cursor advanced
//! by the *real* per-frame delta (`dt * cps`, carrying the sub-integer
//! remainder), clamped to the characters that have actually arrived. It is
//! **decoupled from `is_streaming`**: the sweep keeps advancing after the stream
//! ends until it catches up to the final text, and only then does the bubble
//! switch to full Markdown. When the reveal is caught up to the content that has
//! arrived so far, `last_ms` is re-anchored to *now* so a subsequent content
//! pause never banks budget.

use leptos::prelude::*;
use std::collections::HashMap;

/// Fallback chars-per-second until the live `behavior.typing_speed` loads.
/// Matches `src/config/types/general.rs::default_typing_speed`.
const DEFAULT_CPS: u32 = 200;

/// Reveal cursor for one streaming message's typewriter sweep.
///
/// `Copy` so it lives in a `HashMap` value and round-trips through the untracked
/// bookkeeping in [`TypewriterClock::advance_for`] with zero allocation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reveal {
    /// Characters revealed so far (a prefix length in `char`s, not bytes).
    pub revealed: usize,
    /// Carried sub-integer progress. At 200 cps / 30 fps a frame yields ~6.6
    /// chars, but at low cps a frame can be worth <1 char; carrying the
    /// remainder stops the reveal from stalling by flooring away every frame.
    pub frac: f64,
    /// Wall-clock (ms, page-load relative) of the last advance — the anchor the
    /// next frame's `dt` is measured from.
    pub last_ms: f64,
}

/// Shared, app-root reveal clock read by every streaming bubble.
///
/// `Copy` (all fields are `Copy` signals) so it threads through context with
/// zero clone cost, matching [`ChatState`](crate::views::chat::ChatState) /
/// [`WorkspaceState`](crate::state::layout::WorkspaceState).
#[derive(Clone, Copy)]
pub struct TypewriterClock {
    /// Monotonic animation tick — bumped ~30fps by the app-root interval so
    /// streaming bubbles re-render and advance their reveal *between* token
    /// arrivals (without it, reveal would only step when a delta lands).
    pub tick: RwSignal<u64>,
    /// `behavior.typing_speed` (chars/sec). `0` disables pacing (reveal-all),
    /// so a future "no animation" choice degrades gracefully.
    pub cps: RwSignal<u32>,
    /// `behavior.output_mode == "instant"` — reveal everything immediately
    /// (the backend already coalesces to a single final chunk in instant mode;
    /// this keeps the Panel honest if a stray streamed delta still arrives).
    pub instant: RwSignal<bool>,
    /// `message_id → reveal cursor`. Persists across the per-token remount of a
    /// streaming bubble (the keyed `<For>` recreates the bubble on every delta)
    /// so the reveal is continuous, not reset each token. Also persists *past*
    /// stream completion so the sweep can finish revealing the final text.
    reveals: RwSignal<HashMap<String, Reveal>>,
}

impl Default for TypewriterClock {
    fn default() -> Self {
        Self::new()
    }
}

impl TypewriterClock {
    #[must_use]
    pub fn new() -> Self {
        Self {
            tick: RwSignal::new(0),
            cps: RwSignal::new(DEFAULT_CPS),
            instant: RwSignal::new(false),
            reveals: RwSignal::new(HashMap::new()),
        }
    }

    /// Whether `id` has an in-flight reveal cursor.
    ///
    /// A message with no cursor was never streamed live in this session (e.g.
    /// loaded from history), so the renderer shows it in full immediately rather
    /// than replaying a typewriter sweep. Untracked: this is a mount-time
    /// routing decision, not a reactive input.
    #[must_use]
    pub fn has_reveal(&self, id: &str) -> bool {
        self.reveals.with_untracked(|m| m.contains_key(id))
    }

    /// Advance `id`'s reveal cursor to `now` and return the new revealed length.
    ///
    /// First sight anchors the cursor at `now` with nothing revealed (so the
    /// first frame's `dt` is ~0 and the sweep starts from the beginning rather
    /// than jumping). Uses untracked read/write: the cursor map is bookkeeping,
    /// not a render input — re-render is driven by [`tick`](Self::tick), so
    /// touching it reactively would be a spurious self-dependency.
    ///
    /// Creating a cursor also sweeps stale ones out (see [`STALE_CURSOR_MS`]).
    #[must_use]
    pub fn advance_for(&self, id: &str, total: usize, now: f64, cps: u32, instant: bool) -> usize {
        let prev = self.reveals.with_untracked(|m| m.get(id).copied());
        let is_new = prev.is_none();
        let next = advance_reveal(
            prev.unwrap_or(Reveal {
                revealed: 0,
                frac: 0.0,
                last_ms: now,
            }),
            total,
            now,
            cps,
            instant,
        );
        self.reveals.update_untracked(|m| {
            if is_new {
                prune_stale(m, now);
            }
            m.insert(id.to_string(), next);
        });
        next.revealed
    }

    /// Drop `id`'s reveal cursor once its sweep has finished.
    ///
    /// Called when a completed message has fully revealed: the bubble switches
    /// to static full Markdown, so the cursor is dead weight. Pruning keeps the
    /// map bounded over a long session and makes any later re-render fall
    /// through the history path (full render, no replay).
    pub fn finish(&self, id: &str) {
        self.reveals.update_untracked(|m| {
            m.remove(id);
        });
    }
}

/// How long a cursor may sit un-advanced before it is treated as abandoned.
///
/// A cursor is normally pruned by [`TypewriterClock::finish`] when its bubble
/// finishes revealing. Two paths never reach that: a step bubble that
/// `ChatState::begin_step` *renames* out from under its cursor, and a
/// conversation switch that replaces the whole transcript. Both leave a cursor
/// nothing will ever advance again, so anything untouched for this long is
/// swept on the next cursor creation. Generous enough that a live bubble
/// stalled behind a slow tool call is never mistaken for an orphan (a live
/// bubble heartbeats `last_ms` every animation frame regardless).
const STALE_CURSOR_MS: f64 = 60_000.0;

/// Drop cursors nothing has advanced for [`STALE_CURSOR_MS`]. Pure so the
/// bound is host-testable. `now` going backwards (clock adjustment) yields a
/// negative age and prunes nothing, which is the safe direction.
fn prune_stale(map: &mut HashMap<String, Reveal>, now: f64) {
    map.retain(|_, r| {
        // Spelled through `partial_cmp` because the age can be NaN on a
        // degenerate clock: only a definite `Greater` prunes, so NaN keeps.
        (now - r.last_ms).partial_cmp(&STALE_CURSOR_MS) != Some(std::cmp::Ordering::Greater)
    });
}

/// Advance a [`Reveal`] by the real elapsed wall-clock at `cps`, clamped to
/// `total`.
///
/// Pure (no `web_sys`) so the pacing law is host-testable. Reveals everything at
/// once when pacing is off (`instant`, `cps == 0`). A non-positive or non-finite
/// `dt` (first frame, or `performance.now()` unavailable → `0`) advances
/// nothing — it only re-anchors `last_ms`, so the sweep never jumps on a
/// degenerate clock. When already caught up to `total`, `last_ms` is re-anchored
/// to `now` without banking budget, so a following content pause (tool call /
/// model stall) resumes smoothly instead of dumping.
#[must_use]
pub fn advance_reveal(prev: Reveal, total: usize, now: f64, cps: u32, instant: bool) -> Reveal {
    if instant || cps == 0 {
        return Reveal {
            revealed: total,
            frac: 0.0,
            last_ms: now,
        };
    }
    let dt = now - prev.last_ms;
    if !dt.is_finite() || dt <= 0.0 {
        // Degenerate clock (first frame / unavailable perf): anchor, don't move.
        return Reveal {
            revealed: prev.revealed.min(total),
            frac: prev.frac,
            last_ms: now,
        };
    }
    if prev.revealed >= total {
        // Caught up to the content that has arrived — heartbeat, never bank.
        return Reveal {
            revealed: total,
            frac: 0.0,
            last_ms: now,
        };
    }
    let budget = dt / 1000.0 * f64::from(cps) + prev.frac;
    let whole = budget.floor();
    let new_revealed = prev.revealed.saturating_add(whole as usize).min(total);
    let frac = if new_revealed >= total {
        0.0
    } else {
        budget - whole
    };
    Reveal {
        revealed: new_revealed,
        frac,
        last_ms: now,
    }
}

#[cfg(test)]
mod tests {
    use super::{advance_reveal, prune_stale, Reveal, STALE_CURSOR_MS};
    use std::collections::HashMap;

    /// Cursor anchored at `t=0` with nothing revealed — the first-sight state.
    fn fresh() -> Reveal {
        Reveal {
            revealed: 0,
            frac: 0.0,
            last_ms: 0.0,
        }
    }

    #[test]
    fn instant_reveals_all() {
        let r = advance_reveal(fresh(), 10, 0.0, 200, true);
        assert_eq!(r.revealed, 10);
    }

    #[test]
    fn zero_cps_reveals_all() {
        let r = advance_reveal(fresh(), 10, 0.0, 0, false);
        assert_eq!(r.revealed, 10);
    }

    #[test]
    fn first_frame_does_not_advance() {
        // now == last_ms → dt 0 → anchor only, reveal stays 0 (no jump even
        // when the cursor is first created many ms into the page's life).
        let r = advance_reveal(fresh(), 10, 0.0, 200, false);
        assert_eq!(r.revealed, 0);
        assert_eq!(r.last_ms, 0.0);
    }

    #[test]
    fn advances_by_elapsed() {
        // 100ms at 200cps = 20 chars, but only 10 exist → clamp.
        let r = advance_reveal(fresh(), 10, 100.0, 200, false);
        assert_eq!(r.revealed, 10);
        // 100ms at 50cps = 5 chars.
        let r = advance_reveal(fresh(), 10, 100.0, 50, false);
        assert_eq!(r.revealed, 5);
    }

    #[test]
    fn carries_fractional_progress() {
        // 1ms at 200cps = 0.2 char — floors to 0 revealed but banks the 0.2 so
        // repeated tiny frames still make progress instead of stalling forever.
        let a = advance_reveal(fresh(), 10, 1.0, 200, false);
        assert_eq!(a.revealed, 0);
        assert!((a.frac - 0.2).abs() < 1e-9);
        // Next 1ms frame: 0.2 banked + 0.2 = 0.4, still 0 whole chars.
        let b = advance_reveal(a, 10, 2.0, 200, false);
        assert_eq!(b.revealed, 0);
        assert!((b.frac - 0.4).abs() < 1e-9);
        // Reach t=5ms: 5 * 0.2 = 1.0 → exactly 1 char revealed.
        let c = advance_reveal(b, 10, 5.0, 200, false);
        assert_eq!(c.revealed, 1);
    }

    #[test]
    fn clamps_to_total_and_drops_frac() {
        let prev = Reveal {
            revealed: 8,
            frac: 0.5,
            last_ms: 0.0,
        };
        // 1000ms at 200cps = 200 chars, far past the remaining 2.
        let r = advance_reveal(prev, 10, 1000.0, 200, false);
        assert_eq!(r.revealed, 10);
        assert_eq!(r.frac, 0.0);
    }

    #[test]
    fn caught_up_heartbeats_without_banking() {
        // Already showing all 10 chars. A long idle gap (tool call) must NOT
        // bank budget: last_ms re-anchors to now, revealed stays clamped.
        let prev = Reveal {
            revealed: 10,
            frac: 0.0,
            last_ms: 0.0,
        };
        let r = advance_reveal(prev, 10, 5000.0, 200, false);
        assert_eq!(r.revealed, 10);
        assert_eq!(r.last_ms, 5000.0);
        // Content grows to 30 chars after the pause. The next 33ms frame paces
        // ~6.6 chars from where we were — NOT a 5000ms dump.
        let grown = advance_reveal(r, 30, 5033.0, 200, false);
        assert!(
            grown.revealed <= 17,
            "paced, not dumped: {}",
            grown.revealed
        );
        assert!(grown.revealed >= 10);
    }

    #[test]
    fn prune_stale_drops_only_abandoned_cursors() {
        let cursor = |last_ms: f64| Reveal {
            revealed: 3,
            frac: 0.0,
            last_ms,
        };
        let now = 1_000_000.0;
        let mut map = HashMap::from([
            // Advanced this frame — a live bubble.
            ("live".to_string(), cursor(now)),
            // Stalled behind a slow tool call, but still heartbeating.
            ("slow".to_string(), cursor(now - STALE_CURSOR_MS + 1.0)),
            // Renamed out from under its cursor, or a switched-away session.
            ("orphan".to_string(), cursor(now - STALE_CURSOR_MS - 1.0)),
        ]);
        prune_stale(&mut map, now);
        assert!(map.contains_key("live"));
        assert!(map.contains_key("slow"));
        assert!(!map.contains_key("orphan"));
    }

    #[test]
    fn prune_stale_is_a_no_op_when_the_clock_goes_backwards() {
        let mut map = HashMap::from([(
            "a".to_string(),
            Reveal {
                revealed: 1,
                frac: 0.0,
                last_ms: 5_000.0,
            },
        )]);
        prune_stale(&mut map, 0.0);
        assert_eq!(map.len(), 1, "a negative age must never prune");
    }

    #[test]
    fn negative_or_nan_dt_anchors_without_moving() {
        let prev = Reveal {
            revealed: 3,
            frac: 0.0,
            last_ms: 100.0,
        };
        // Clock went backwards (now < last_ms).
        let back = advance_reveal(prev, 10, 50.0, 200, false);
        assert_eq!(back.revealed, 3);
        assert_eq!(back.last_ms, 50.0);
        // NaN now (performance unavailable path is 0, but guard NaN too).
        let nan = advance_reveal(prev, 10, f64::NAN, 200, false);
        assert_eq!(nan.revealed, 3);
    }
}
