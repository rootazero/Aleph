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
    /// `message_id → (already-rendered HTML for the safe prefix, chars that
    /// prefix represents)`. Separate from `reveals` (not folded into
    /// [`Reveal`]) because `Reveal` is deliberately `Copy`/allocation-free;
    /// this cache holds an owned `String` and is invalidated independently
    /// (see [`TypewriterClock::clear_stable_prefix`]).
    stable_prefixes: RwSignal<std::collections::HashMap<String, (String, usize)>>,
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
            stable_prefixes: RwSignal::new(HashMap::new()),
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
        let mut pruned: Vec<String> = Vec::new();
        self.reveals.update_untracked(|m| {
            if is_new {
                pruned = prune_stale(m, now);
            }
            m.insert(id.to_string(), next);
        });
        if !pruned.is_empty() {
            self.stable_prefixes.update_untracked(|m| {
                for k in &pruned {
                    m.remove(k);
                }
            });
        }
        next.revealed
    }

    /// Whether any bubble is still revealing.
    ///
    /// The transcript's auto-scroll needs this because a reveal grows the
    /// rendered height with no accompanying `messages` write — the sweep
    /// deliberately outlives the stream — so "follow the bottom" cannot be
    /// driven off transcript changes alone. Cursors are dropped by
    /// [`Self::finish`] the frame a bubble catches up and swept by
    /// [`prune_stale`] when one is abandoned, so an empty map really is
    /// "nothing is animating" rather than "nothing has animated recently".
    ///
    /// Untracked on purpose: the map is written through `update_untracked`, so
    /// a tracked read would subscribe to a signal that never notifies. Callers
    /// drive re-evaluation off [`tick`](Self::tick) instead.
    #[must_use]
    pub fn is_sweeping(&self) -> bool {
        self.reveals.with_untracked(|m| !m.is_empty())
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
        self.clear_stable_prefix(id);
    }

    /// Cached `(html, safe_offset)` for `id`'s already-rendered prefix, if
    /// any. `safe_offset` is a byte offset into the message's content, per
    /// [`shared_ui_logic::markdown_stream::safe_freeze_offset`].
    #[must_use]
    pub fn stable_prefix_for(&self, id: &str) -> Option<(String, usize)> {
        self.stable_prefixes.with_untracked(|m| m.get(id).cloned())
    }

    /// Replace `id`'s cached stable prefix.
    pub fn set_stable_prefix(&self, id: &str, html: String, safe_offset: usize) {
        self.stable_prefixes.update_untracked(|m| {
            m.insert(id.to_string(), (html, safe_offset));
        });
    }

    /// Drop `id`'s cached stable prefix. Called from [`Self::finish`] and
    /// whenever a caller observes `is_streaming == false` for a still-
    /// sweeping message: `finalize_answer`/`set_step_text` can swap a
    /// message's `content` wholesale rather than append to it, and a cached
    /// HTML prefix computed against the old content would then describe text
    /// that no longer exists at that offset.
    pub fn clear_stable_prefix(&self, id: &str) {
        self.stable_prefixes.update_untracked(|m| {
            m.remove(id);
        });
    }

    /// Reveal the whole message NOW (the user clicked a sweeping bubble).
    ///
    /// Sets the cursor straight to `total` instead of dropping it: a bubble
    /// whose stream is still live would re-anchor a dropped cursor at zero
    /// and replay the sweep from the beginning, which is the opposite of
    /// "skip". No-op without a live cursor (history bubbles never had one).
    pub fn skip(&self, id: &str, total: usize) {
        self.reveals.update_untracked(|m| {
            if let Some(cursor) = m.get_mut(id) {
                cursor.revealed = total;
            }
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

/// Drop cursors nothing has advanced for [`STALE_CURSOR_MS`], returning the
/// ids removed so callers can prune dependent bookkeeping (see
/// [`TypewriterClock::advance_for`], which uses this to also drop the
/// pruned ids' cached stable prefixes). Pure so the bound is host-testable.
/// `now` going backwards (clock adjustment) yields a negative age and prunes
/// nothing, which is the safe direction.
fn prune_stale(map: &mut HashMap<String, Reveal>, now: f64) -> Vec<String> {
    let stale: Vec<String> = map
        .iter()
        .filter(|(_, r)| {
            // Spelled through `partial_cmp` because the age can be NaN on a
            // degenerate clock: only a definite `Greater` prunes, so NaN keeps.
            (now - r.last_ms).partial_cmp(&STALE_CURSOR_MS) == Some(std::cmp::Ordering::Greater)
        })
        .map(|(k, _)| k.clone())
        .collect();
    for k in &stale {
        map.remove(k);
    }
    stale
}

/// Worst-case seconds the reveal is allowed to trail the text that has already
/// arrived.
///
/// `cps` is a *taste* setting — how fast characters should appear when the
/// model is producing at a human-ish rate. It is not a promise about how long
/// the user waits, and read as one it is a bad promise: a model that emits
/// 2,500 characters in six seconds outruns the default 200 cps by a factor of
/// two, so the sweep is still crawling six seconds after `run_complete` fired,
/// the composer unlocked, and the spinner went away. At 10 KB (one code block)
/// it is nearly a minute. Nothing in the fixed-rate law bounds that: the lag is
/// `backlog / cps` and `backlog` is whatever the model felt like producing.
///
/// So the rate is a preference and this is the promise — see
/// [`lag_floor`].
const MAX_REVEAL_LAG_SECS: f64 = 2.0;

/// The smallest `revealed` the cursor is allowed to hold given that `total`
/// characters have arrived: `total - cps * MAX_REVEAL_LAG_SECS`.
///
/// Read it as a sliding window. The reveal may trail the arrived text by at
/// most [`MAX_REVEAL_LAG_SECS`] *of configured playback* — 400 characters at
/// the default 200 cps — and the deficit beyond that is forfeited rather than
/// queued. Two properties fall out, and both are things a flat `dt * cps` law
/// cannot state:
///
/// - **While the model is producing**, the window pins the cursor to the
///   arrival rate, so what is on screen is a fixed two seconds behind the
///   stream no matter how fast the stream runs.
/// - **Once the last chunk lands**, `total` stops moving, so the residue is at
///   most `cps * MAX_REVEAL_LAG_SECS` characters and drains at exactly `cps`:
///   the sweep is guaranteed to finish within two seconds of the run ending,
///   for any answer of any length.
///
/// # Why a window rather than codex's two gears
///
/// codex solves the same problem in `tui/src/streaming/chunking.rs` with an
/// `AdaptiveChunkingPolicy`: a `Smooth`/`CatchUp` mode pair, four enter/exit
/// thresholds, two hold windows and a severe-backlog bypass — eight tuning
/// constants and a mode field carried across ticks. It needs all of that
/// because its queue unit is a *rendered line* whose display cost it cannot
/// price: queue depth and oldest-line age are two independent proxies for a lag
/// it can never compute, discrete gears are the only lever those proxies
/// support, and discrete gears flap at the threshold — which is what the
/// hysteresis is for.
///
/// Aleph's unit is a character and the reveal is a counter, so the lag is
/// directly expressible and the policy collapses to one `max` on the cursor.
/// It carries no state (so [`Reveal`] stays `Copy` and the law stays pure) and
/// it is monotone in both arguments, so there is no boundary to oscillate
/// across and nothing to hold.
///
/// # What it deliberately is not
///
/// Not a *rate* floor. Pacing at `backlog / MAX_REVEAL_LAG_SECS` — the first
/// shape this took — reads as the same idea and is not: the rate shrinks with
/// the backlog it is draining, so the approach is exponential and the sweep
/// never actually terminates. Measured, a 10 000-character answer still took
/// 8.4 s to finish under that law. The bound has to be on the cursor, not on
/// its derivative.
///
/// Below the window this is `0`, i.e. a no-op: an answer that already keeps up
/// paces byte-for-byte as it did before the window existed.
fn lag_floor(total: usize, cps: u32) -> usize {
    // `as` truncation is intended and harmless: the window is a UX budget, and
    // a fractional character is not a unit anything downstream can render.
    let window = (f64::from(cps) * MAX_REVEAL_LAG_SECS) as usize;
    total.saturating_sub(window)
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
///
/// The step is taken at `cps` flat and then held above [`lag_floor`], so the
/// sweep trails the arrived text by at most [`MAX_REVEAL_LAG_SECS`] however
/// fast the model produces.
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
    let paced = prev.revealed.saturating_add(whole as usize).min(total);
    // Forfeit any deficit past the lag window rather than queueing it: the
    // cursor is dragged forward to the window's trailing edge, never backward
    // (`max`), so this can only ever move the reveal along.
    let new_revealed = paced.max(lag_floor(total, cps)).min(total);
    // A dragged cursor discards the banked remainder too — it belongs to the
    // characters that were just skipped, and carrying it would pay for them
    // twice.
    let frac = if new_revealed >= total || new_revealed > paced {
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
    use super::{advance_reveal, prune_stale, Reveal, TypewriterClock, STALE_CURSOR_MS};
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
    fn stable_prefix_round_trips() {
        let clock = TypewriterClock::new();
        assert_eq!(clock.stable_prefix_for("m1"), None);
        clock.set_stable_prefix("m1", "<p>hi</p>".to_string(), 5);
        assert_eq!(
            clock.stable_prefix_for("m1"),
            Some(("<p>hi</p>".to_string(), 5))
        );
    }

    #[test]
    fn finish_clears_the_stable_prefix_too() {
        let clock = TypewriterClock::new();
        clock.set_stable_prefix("m1", "<p>hi</p>".to_string(), 5);
        clock.finish("m1");
        assert_eq!(clock.stable_prefix_for("m1"), None);
    }

    #[test]
    fn clear_stable_prefix_is_a_no_op_on_a_missing_id() {
        let clock = TypewriterClock::new();
        clock.clear_stable_prefix("does-not-exist"); // must not panic
    }

    #[test]
    fn stale_cursor_pruning_also_drops_its_stable_prefix() {
        let clock = TypewriterClock::new();
        // First sight of "orphan" — advance_for creates a cursor.
        let _ = clock.advance_for("orphan", 10, 0.0, 200, false);
        clock.set_stable_prefix("orphan", "<p>partial</p>".to_string(), 4);
        // Advance a different id far enough in the future that "orphan" is
        // stale (> STALE_CURSOR_MS old) — this triggers prune_stale.
        let _ = clock.advance_for("fresh", 10, 100_000.0, 200, false);
        assert_eq!(clock.stable_prefix_for("orphan"), None);
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

    // ---- backlog convergence (`converged_cps`) ----------------------------

    /// Sweep a cursor at 30 fps until it catches `total`, or until `max_ms`
    /// of simulated wall-clock has passed. Returns the elapsed simulated time.
    ///
    /// Deliberately steps the same way the render loop does (fixed ~33 ms
    /// frames driven by `TypewriterClock::tick`) so the assertions below are
    /// about the pacing law under the real frame budget, not about an
    /// idealised continuous integral.
    /// One animation frame at the ~30 fps the app root ticks at.
    const FRAME_MS: f64 = 1000.0 / 30.0;

    /// Discretisation slack on the lag ceiling: one frame is spent dragging the
    /// cursor onto the window's edge, and the `floor()` + carried remainder in
    /// `advance_reveal` lands the final character one frame after the ideal
    /// continuous integral would. Named rather than rounded away — the law is
    /// exact, the 30 fps sampling of it is not.
    const SWEEP_SLACK_MS: f64 = 2.0 * FRAME_MS;

    fn sweep_ms(total: usize, cps: u32, max_ms: f64) -> f64 {
        let mut cur = fresh();
        let mut t = 0.0;
        while cur.revealed < total && t < max_ms {
            t += FRAME_MS;
            cur = advance_reveal(cur, total, t, cps, false);
        }
        t
    }

    #[test]
    fn an_answer_inside_the_window_paces_at_the_configured_speed() {
        // 200 chars at 200 cps is one second of playback — inside the window,
        // so the floor is 0 and must not touch the sweep. Anything else would
        // make the taste setting a lie on the answers it governs.
        assert_eq!(super::lag_floor(200, 200), 0);
        assert_eq!(super::lag_floor(400, 200), 0);
        let elapsed = sweep_ms(200, 200, 5_000.0);
        assert!(
            (900.0..1_200.0).contains(&elapsed),
            "an answer inside the window must keep the configured pace, took {elapsed}ms"
        );
    }

    #[test]
    fn a_long_answer_finishes_within_the_lag_ceiling() {
        // The defect this bounds: 10 000 chars at 200 cps used to be a
        // 50-second typewriter still crawling long after `run_complete`
        // unlocked the composer. One frame of slack for the 30 fps step.
        let elapsed = sweep_ms(10_000, 200, 60_000.0);
        let ceiling_ms = super::MAX_REVEAL_LAG_SECS * 1000.0 + SWEEP_SLACK_MS;
        assert!(
            elapsed <= ceiling_ms,
            "a 10k-char answer must finish within the lag ceiling, took {elapsed}ms"
        );
    }

    #[test]
    fn the_ceiling_scales_with_the_configured_speed() {
        // The window is measured in *playback seconds*, not characters, so a
        // slower setting keeps its slower feel and still terminates on time.
        for cps in [50_u32, 200, 400] {
            let elapsed = sweep_ms(40_000, cps, 120_000.0);
            let ceiling_ms = super::MAX_REVEAL_LAG_SECS * 1000.0 + SWEEP_SLACK_MS;
            assert!(
                elapsed <= ceiling_ms,
                "cps={cps}: took {elapsed}ms, ceiling {ceiling_ms}ms"
            );
        }
    }

    #[test]
    fn the_window_never_moves_the_cursor_backwards() {
        // The clamp is a `max`, so a cursor already ahead of the window (the
        // ordinary case — everything revealed) is left exactly where it is.
        let ahead = Reveal {
            revealed: 9_000,
            frac: 0.0,
            last_ms: 0.0,
        };
        let r = advance_reveal(ahead, 10_000, 33.0, 200, false);
        assert!(
            r.revealed >= 9_000,
            "clamp must never rewind: {}",
            r.revealed
        );
    }

    #[test]
    fn the_window_is_monotone_in_both_arguments() {
        // Monotone and continuous is what removes the need for hysteresis:
        // there is no threshold for the reveal to oscillate across.
        let mut prev = 0;
        for total in [0_usize, 100, 400, 1_000, 10_000, 100_000] {
            let now = super::lag_floor(total, 200);
            assert!(now >= prev, "total={total} regressed the floor");
            prev = now;
        }
        // A faster setting is allowed to trail by more characters (same
        // seconds), so the floor is non-increasing in cps.
        let mut prev = usize::MAX;
        for cps in [50_u32, 100, 200, 400] {
            let now = super::lag_floor(10_000, cps);
            assert!(now <= prev, "cps={cps} raised the floor");
            prev = now;
        }
    }

    #[test]
    fn a_still_streaming_answer_trails_by_the_window_not_more() {
        // Simulate a model producing 1 000 chars/s — five times the 200 cps
        // setting — for three seconds. The on-screen text must stay within one
        // window of what has arrived instead of falling further behind every
        // frame, which is what the flat law did.
        let mut cur = fresh();
        let mut t;
        let mut worst_deficit = 0usize;
        for frame in 1..=90 {
            t = f64::from(frame) * FRAME_MS;
            let arrived = (t / 1000.0 * 1_000.0) as usize;
            cur = advance_reveal(cur, arrived, t, 200, false);
            worst_deficit = worst_deficit.max(arrived.saturating_sub(cur.revealed));
        }
        assert!(
            worst_deficit <= 400 + 7,
            "must trail by at most one window (400 chars) plus a frame, trailed {worst_deficit}"
        );
    }

    #[test]
    fn pacing_off_still_short_circuits_under_pressure() {
        // `instant` / `cps == 0` are answered before the window is consulted,
        // so a huge answer cannot resurrect pacing for a user who turned it
        // off.
        assert_eq!(
            advance_reveal(fresh(), 50_000, 33.0, 200, true).revealed,
            50_000
        );
        assert_eq!(
            advance_reveal(fresh(), 50_000, 33.0, 0, false).revealed,
            50_000
        );
    }
}
