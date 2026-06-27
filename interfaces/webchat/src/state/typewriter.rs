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

use leptos::prelude::*;
use std::collections::HashMap;

/// Fallback chars-per-second until the live `behavior.typing_speed` loads.
/// Matches `src/config/types/general.rs::default_typing_speed`.
const DEFAULT_CPS: u32 = 50;

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
    /// `message_id → reveal-start timestamp (ms)`. Persists across the
    /// per-token remount of a streaming bubble (the keyed `<For>` recreates the
    /// bubble on every delta) so the reveal is continuous, not reset each token.
    starts: RwSignal<HashMap<String, f64>>,
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
            starts: RwSignal::new(HashMap::new()),
        }
    }

    /// Reveal-start timestamp for `id`, inserting `now` on first sight.
    ///
    /// Uses untracked read/write: the start map is bookkeeping, not a render
    /// input — re-render is driven by [`tick`](Self::tick), so touching the map
    /// reactively would be a spurious dependency. Returns the *existing* start
    /// for a message already streaming, which is what keeps reveal continuous
    /// across the bubble's per-token remount.
    #[must_use]
    pub fn start_for(&self, id: &str, now: f64) -> f64 {
        if let Some(t) = self.starts.with_untracked(|m| m.get(id).copied()) {
            return t;
        }
        self.starts.update_untracked(|m| {
            m.insert(id.to_string(), now);
        });
        now
    }
}

/// Number of leading characters to reveal given elapsed time and pacing.
///
/// Pure (no `web_sys`) so the pacing law is host-testable. Reveal-all when
/// pacing is off (`instant`, `cps == 0`) or the clock is degenerate
/// (`elapsed_ms < 0`, e.g. `performance.now()` unavailable → `0 - start`).
/// Otherwise reveal `floor(elapsed_secs * cps)` chars, clamped to `total`.
#[must_use]
pub fn revealed_len(total: usize, elapsed_ms: f64, cps: u32, instant: bool) -> usize {
    if instant || cps == 0 || elapsed_ms.is_nan() || elapsed_ms < 0.0 {
        return total;
    }
    let allowed = (elapsed_ms / 1000.0 * f64::from(cps)).floor();
    if allowed <= 0.0 {
        0
    } else if allowed >= total as f64 {
        total
    } else {
        allowed as usize
    }
}

#[cfg(test)]
mod tests {
    use super::revealed_len;

    #[test]
    fn instant_reveals_all() {
        assert_eq!(revealed_len(10, 0.0, 50, true), 10);
    }

    #[test]
    fn zero_cps_reveals_all() {
        assert_eq!(revealed_len(10, 0.0, 0, false), 10);
    }

    #[test]
    fn negative_elapsed_reveals_all() {
        // performance.now() unavailable → now_ms()==0, 0 - start < 0.
        assert_eq!(revealed_len(10, -5.0, 50, false), 10);
    }

    #[test]
    fn zero_elapsed_reveals_none() {
        assert_eq!(revealed_len(10, 0.0, 50, false), 0);
    }

    #[test]
    fn paces_proportional_to_elapsed() {
        // 100ms at 50cps = 5 chars.
        assert_eq!(revealed_len(10, 100.0, 50, false), 5);
    }

    #[test]
    fn clamps_to_total_when_caught_up() {
        // 1000ms at 50cps = 50 chars, but only 10 exist.
        assert_eq!(revealed_len(10, 1000.0, 50, false), 10);
    }

    #[test]
    fn floors_partial_char() {
        // 30ms at 50cps = 1.5 → 1 char (never reveal a fractional glyph).
        assert_eq!(revealed_len(10, 30.0, 50, false), 1);
    }
}
