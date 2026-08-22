//! In-memory loop state: a per-session timer-driven repeat. Mirrors
//! `goal::types` but is NEVER persisted — the registry lives in process
//! memory only, so a daemon restart clears all loops ("dies with the session").
//!
//! Distinct from `goal` (condition-stop) and `cron` (persistent, own session):
//! a loop is "current session, multi-turn, clock-gated, never self-stops".
//!
//! These types deliberately carry NO `Serialize`/`Deserialize`/`JsonSchema`.
//! They used to, along with `#[serde(default)]` on every optional field and
//! comments promising "old payloads read None" — a forward-compatibility
//! contract with an on-disk format that has never existed and that the first
//! line of this file forbids. The only readers were three tests exercising the
//! derives themselves. If a loop ever does need persisting, the wire type
//! belongs beside the store that writes it, not here.

/// How the loop picks the delay before its next tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cadence {
    /// Fixed interval: wait this many ms between ticks.
    Fixed { interval_ms: u64 },
    /// Model-paced: the model sets `next_wake_ms` each tick via
    /// `loop(action='update', next_wake=...)`. If unset, use `fallback_ms`.
    ModelPaced { fallback_ms: u64 },
}

impl Cadence {
    /// Human-readable one-liner for status output (e.g. "every 5m" /
    /// "model-paced (fallback 10m)"). Mirrors the human duration form the
    /// `loop` tool accepts on input, so what the user reads matches what they
    /// type.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Fixed { interval_ms } => format!("every {}", fmt_duration_ms(*interval_ms)),
            Self::ModelPaced { fallback_ms } => {
                format!("model-paced (fallback {})", fmt_duration_ms(*fallback_ms))
            }
        }
    }
}

/// Render a millisecond duration as a compact human string ("5m" / "1h30m" /
/// "45s"). The display inverse of `loop_manage::parse_interval_ms`; kept beside
/// `Cadence` so status output and input parsing share one vocabulary. Sub-second
/// values are never produced (loops reject them), so ms precision is unneeded.
#[must_use]
pub fn fmt_duration_ms(ms: u64) -> String {
    let total_secs = ms / 1_000;
    let (h, m, s) = (
        total_secs / 3_600,
        (total_secs % 3_600) / 60,
        total_secs % 60,
    );
    let mut out = String::new();
    if h > 0 {
        out.push_str(&format!("{h}h"));
    }
    if m > 0 {
        out.push_str(&format!("{m}m"));
    }
    // Only show seconds when they are the sole component or a remainder; an even
    // "5m" stays "5m", not "5m0s".
    if s > 0 || out.is_empty() {
        out.push_str(&format!("{s}s"));
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopStatus {
    /// Ticking: the continuation hook claims a tick after every completed run.
    Active,
    /// Suspended but intact — caps, tick count, prompt and cadence all survive.
    /// The hook skips it (`is_active()` is false), so no tick is ever claimed;
    /// `resume` puts it back to `Active` and the resuming turn's completion
    /// hook claims the next tick. The reversible half of the lifecycle contract
    /// (A4), mirroring `goal`'s long-standing `GoalStatus::Paused`.
    Paused,
    /// Terminal. Only `start` brings a loop back from here.
    Stopped,
}

impl LoopStatus {
    /// Canonical snake_case name — identical to the
    /// `action='…'` verb that produces it. Single source for every renderer
    /// (`human_summary` / `stable_summary` / the tool's `list` line), so a new
    /// variant can never be surfaced in one place and forgotten in another.
    /// Mirrors `goal::GoalStatus::as_str`.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Stopped => "stopped",
        }
    }
}

/// One session's loop. Immutable-update style: `with_*` / `spent_*` return
/// new copies, never mutate in place (project immutability rule).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopState {
    pub session_id: String,
    /// The fixed prompt re-injected verbatim each tick.
    pub prompt: String,
    pub cadence: Cadence,
    /// Model-paced override (absolute epoch ms of the next wake). `None` →
    /// fall back to the cadence default. For `Fixed` cadence this is ignored.
    pub next_wake_ms: Option<u64>,
    /// Absolute epoch-ms fire time of the tick already enqueued by the
    /// continuation hook. `None` → no tick in flight. Set when a tick is
    /// enqueued, cleared when it actually fires: an in-flight tick blocks
    /// further enqueues (every completed run in the session re-enters the
    /// hook — without this gate each user turn would spawn one more
    /// self-perpetuating tick chain), and a woken tick only executes if its
    /// wake still matches (a stop/start during the delay supersedes it).
    pub pending_tick_wake_ms: Option<u64>,
    pub iterations_used: u32,
    /// Optional safety cap: stop after this many ticks. `None` → unbounded.
    pub max_iterations: Option<u32>,
    /// Optional safety cap: absolute epoch-ms wall-clock deadline.
    pub deadline_ms: Option<u64>,
    /// Optional soft token budget: stop the loop once it has consumed more than
    /// this many session tokens since the baseline was captured. Enforced by the
    /// continuation hook via [`Self::over_budget`] (codex `tokenStartFresh`
    /// parity, mirroring `goal::Goal`).
    pub token_budget: Option<u64>,
    /// Session cumulative-token baseline at the moment budget enforcement began.
    /// Seeded lazily by the continuation hook on the first tick that sees a
    /// budget set (the `loop` tool has no live token counter at creation). Until
    /// `baseline_captured`, the token budget is not enforced.
    pub tokens_at_start: u64,
    /// Whether `tokens_at_start` holds a real session-token baseline yet. Mirrors
    /// `goal::Goal::baseline_captured`.
    pub baseline_captured: bool,
    pub status: LoopStatus,
    pub created_at_ms: u64,
    /// Why the loop last stopped, in user-facing prose. Set by the continuation
    /// hook when a safety cap (iteration / deadline / token budget) trips, and
    /// by the failure path when a tick errors. `None` while `Active` or for a
    /// loop the user stopped by hand. Surfaced by `loop(action='status')` so a
    /// silently-capped watch loop can explain itself on the next turn — without
    /// it, the stop reason the hook computes was logged and dropped.
    pub stop_reason: Option<String>,
    /// P1 data isolation: the user id that started this loop, stamped once at
    /// `loop(action='start')` time from `scope::current_scope()` inside the
    /// creating run. Owned by the claim pipeline exactly like `goal::Goal::workspace`
    /// — preserved across tool updates by `LoopRegistry::commit_field_update`.
    /// `None` outside any scope (legacy/unattended) — unscoped, zero behavior
    /// change. No `#[serde(default)]` needed: this type is never persisted
    /// (see the module doc).
    pub owner_user_id: Option<String>,
    /// The rendered scope boundary (`scope::ScopeId::render()`) paired with
    /// [`Self::owner_user_id`]. Always set/cleared together — see
    /// [`Self::with_owner_scope`].
    pub scope_id: Option<String>,
}

impl LoopState {
    #[must_use]
    pub fn new(session_id: &str, prompt: &str, cadence: Cadence, now_ms: u64) -> Self {
        Self {
            session_id: session_id.to_string(),
            prompt: prompt.to_string(),
            cadence,
            next_wake_ms: None,
            pending_tick_wake_ms: None,
            iterations_used: 0,
            max_iterations: None,
            deadline_ms: None,
            token_budget: None,
            tokens_at_start: 0,
            baseline_captured: false,
            status: LoopStatus::Active,
            created_at_ms: now_ms,
            stop_reason: None,
            owner_user_id: None,
            scope_id: None,
        }
    }

    /// Count a tick at CLAIM time, so a cap still holds if the tick crashes.
    #[must_use]
    pub const fn spent_iteration(mut self) -> Self {
        self.iterations_used = self.iterations_used.saturating_add(1);
        self
    }

    /// Give back a tick that was claimed but provably never executed.
    ///
    /// Counting at claim time is the right default (a crashed tick must still
    /// consume its budget), but two paths retire a tick that is still *asleep*:
    /// leaving `Active` (`transition`, e.g. `pause`) and a rescheduling
    /// `update`. Both clear the pending marker so the sleeping task's
    /// `confirm_fire` mismatches and it is discarded — yet the counter kept the
    /// claim. Five re-paces of a `max_iterations=5` loop reported `ticks: 5/5`
    /// and stopped "at the cap" having executed nothing.
    ///
    /// Only ever called under the registry lock on an explicit, synchronous
    /// supersede — never for a tick that could still fire — so the crash-safety
    /// rationale above is untouched.
    #[must_use]
    pub const fn refund_iteration(mut self) -> Self {
        self.iterations_used = self.iterations_used.saturating_sub(1);
        self
    }

    #[must_use]
    pub fn with_status(mut self, status: LoopStatus) -> Self {
        self.status = status;
        self
    }

    /// Record the user-facing reason the loop stopped (or clear it with `None`).
    /// Pairs with `with_status(Stopped)` at every stop site so the reason and
    /// the status flip stay together.
    #[must_use]
    pub fn with_stop_reason(mut self, reason: Option<String>) -> Self {
        self.stop_reason = reason;
        self
    }

    /// Stamp (or clear) the owning scope attribution. Set once at creation
    /// from `scope::current_scope()`; claim-pipeline-owned thereafter, mirrors
    /// `goal::Goal::with_owner_scope`.
    #[must_use]
    pub fn with_owner_scope(mut self, attr: Option<&crate::scope::ScopeAttribution>) -> Self {
        self.owner_user_id = attr.map(|a| a.owner_user_id.clone());
        self.scope_id = attr.map(|a| a.scope.render());
        self
    }

    #[must_use]
    pub const fn with_max_iterations(mut self, max: Option<u32>) -> Self {
        self.max_iterations = max;
        self
    }

    #[must_use]
    pub const fn with_deadline_ms(mut self, deadline_ms: Option<u64>) -> Self {
        self.deadline_ms = deadline_ms;
        self
    }

    #[must_use]
    pub const fn with_token_budget(mut self, budget: Option<u64>) -> Self {
        self.token_budget = budget;
        self
    }

    /// Seed the real session-token baseline and mark it captured. Called once by
    /// the continuation hook on the first tick that sees a budget set. Mirrors
    /// `goal::Goal::with_baseline` (loop has no `updated_at` to bump).
    #[must_use]
    pub const fn with_baseline(mut self, tokens_at_start: u64) -> Self {
        self.tokens_at_start = tokens_at_start;
        self.baseline_captured = true;
        self
    }

    /// Tokens consumed since the baseline. Saturates on a counter reset so a
    /// backwards-moving total never reports phantom spend (mirrors `goal`).
    #[must_use]
    pub const fn tokens_used(&self, now_total_tokens: u64) -> u64 {
        now_total_tokens.saturating_sub(self.tokens_at_start)
    }

    /// True only when a budget is set, the baseline was captured, and spend
    /// since baseline exceeds it.
    ///
    /// Without `baseline_captured`, `tokens_at_start` is the placeholder 0, so
    /// `tokens_used` would return the session's ENTIRE lifetime token count and
    /// a fresh loop in a long-running session would read as instantly over
    /// budget. Every current caller holds this invariant by hand (the claim
    /// path seeds before enforcing); making it a property of the type means no
    /// future caller can get it wrong — mirrors `goal::Goal::over_budget`.
    #[must_use]
    pub fn over_budget(&self, now_total_tokens: u64) -> bool {
        match self.token_budget {
            Some(b) if self.baseline_captured => self.tokens_used(now_total_tokens) > b,
            _ => false,
        }
    }

    #[must_use]
    pub const fn with_next_wake_ms(mut self, next_wake_ms: Option<u64>) -> Self {
        self.next_wake_ms = next_wake_ms;
        self
    }

    /// Record (or clear) the in-flight tick's scheduled fire time. Paired
    /// with the registry's claim/confirm cycle; see `pending_tick_wake_ms`.
    #[must_use]
    pub const fn with_pending_tick(mut self, wake_ms: Option<u64>) -> Self {
        self.pending_tick_wake_ms = wake_ms;
        self
    }

    /// Re-target the watch: swap the prompt re-injected each tick without a
    /// stop/start cycle. Empty input is ignored by the caller.
    #[must_use]
    pub fn with_prompt(mut self, prompt: &str) -> Self {
        self.prompt = prompt.to_string();
        self
    }

    /// Re-pace the loop in place: change a Fixed interval, or convert a
    /// model-paced loop to a fixed cadence (and vice-versa). Clears any stale
    /// `next_wake_ms` when switching to Fixed, where it is meaningless.
    #[must_use]
    pub fn with_cadence(mut self, cadence: Cadence) -> Self {
        if matches!(cadence, Cadence::Fixed { .. }) {
            self.next_wake_ms = None;
        }
        self.cadence = cadence;
        self
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        self.status == LoopStatus::Active
    }

    /// Suspended-but-intact. The tick pipeline treats `Paused` exactly like
    /// `Stopped` (both fail `is_active`, so no tick is ever claimed or
    /// confirmed); this predicate exists for the *lifecycle* callers that must
    /// tell a reversible pause from a terminal stop — `update`, which re-paces
    /// a paused loop, and `resume`, which will not resurrect a stopped one.
    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.status == LoopStatus::Paused
    }

    /// May the `loop` tool mutate this loop's fields in place? A paused loop is
    /// the whole point of pausing — adjust caps/cadence/prompt while it is
    /// quiet, then `resume`. A stopped one cannot be re-paced: the hook only
    /// ever fires `Active` loops, so an "update" there would report success
    /// over a loop that can never run again.
    #[must_use]
    pub fn is_adjustable(&self) -> bool {
        self.is_active() || self.is_paused()
    }

    /// User-facing one-paragraph status. Replaces a raw `{:?}` Debug dump with
    /// prose the model can relay verbatim: cadence in human units, ticks done
    /// (vs cap), any caps, the next wake for model-paced loops, and the stop
    /// reason when stopped. `now_ms` (0 = clock unavailable) turns the absolute
    /// `next_wake_ms` into a relative "in 8m".
    ///
    /// This is the **tool-reply** renderer (`loop_manage`), where a live
    /// countdown is exactly what the user asked for. The *prompt* path must not
    /// use it — see [`Self::stable_summary`].
    #[must_use]
    pub fn human_summary(&self, now_ms: u64) -> String {
        let mut parts = vec![format!("Loop {}", self.status.as_str())];
        parts.push(format!("cadence: {}", self.cadence.describe()));
        match self.max_iterations {
            Some(max) => parts.push(format!("ticks: {}/{max}", self.iterations_used)),
            None => parts.push(format!("ticks: {}", self.iterations_used)),
        }
        if let Some(deadline) = self.deadline_ms {
            if now_ms != 0 && deadline > now_ms {
                parts.push(format!("time left: {}", fmt_duration_ms(deadline - now_ms)));
            } else {
                parts.push("deadline: set".to_string());
            }
        }
        if let Some(budget) = self.token_budget {
            if self.baseline_captured {
                parts.push(format!("token budget: {budget}"));
            } else {
                // A budget was declared but the session token counter never
                // produced a number — `try_claim_tick` therefore never enforced
                // the budget. Surface that explicitly so the operator does not
                // see "token budget: 50000" and conclude enforcement is on.
                parts.push(format!(
                    "token budget: {budget} (declared, unenforced: counter unavailable)"
                ));
            }
        }
        if matches!(self.cadence, Cadence::ModelPaced { .. }) {
            match self.next_wake_ms {
                Some(wake) if now_ms != 0 && wake > now_ms => {
                    parts.push(format!("next wake: in {}", fmt_duration_ms(wake - now_ms)));
                }
                Some(_) => parts.push("next wake: due now".to_string()),
                None => parts.push("next wake: unset (uses fallback)".to_string()),
            }
        }
        // A fixed-cadence loop knows its next fire too: `try_claim_tick` clears
        // `next_wake_ms` (meaningless for Fixed) but stamps the absolute wake into
        // `pending_tick_wake_ms` when it enqueues the tick. Surface it so a fixed
        // watch-loop reports when it next runs, mirroring the model-paced next-wake
        // line above. Cleared during a firing tick (`confirm_fire`) → omitted then.
        if matches!(self.cadence, Cadence::Fixed { .. }) {
            if let Some(wake) = self.pending_tick_wake_ms {
                if now_ms != 0 && wake > now_ms {
                    parts.push(format!("next tick: in {}", fmt_duration_ms(wake - now_ms)));
                }
            }
        }
        if let Some(reason) = &self.stop_reason {
            parts.push(format!("reason: {reason}"));
        }
        parts.join(", ")
    }

    /// The half of the status that does NOT move with the wall clock: status,
    /// cadence, ticks, token budget, and whether a deadline exists at all.
    ///
    /// This is what may enter the SYSTEM PROMPT. Anthropic's prompt cache is
    /// prefix-keyed and the system prompt sits ahead of every message-level
    /// breakpoint, so a single byte that changes each run re-keys the entire
    /// conversation prefix — the growing history, i.e. the expensive part —
    /// forcing a full cache WRITE (1.25×) instead of a READ (0.1×) on every
    /// turn. A `Utc::now()`-derived countdown does exactly that, unconditionally
    /// and forever, for any session with a live loop.
    ///
    /// The counters here (`ticks`) do change — but only when the loop actually
    /// advances, which is a genuine state change that *should* re-key the
    /// prefix. That is caching working correctly, not cache thrash.
    #[must_use]
    pub fn stable_summary(&self) -> String {
        let mut parts = vec![format!("Loop {}", self.status.as_str())];
        parts.push(format!("cadence: {}", self.cadence.describe()));
        match self.max_iterations {
            Some(max) => parts.push(format!("ticks: {}/{max}", self.iterations_used)),
            None => parts.push(format!("ticks: {}", self.iterations_used)),
        }
        if self.deadline_ms.is_some() {
            parts.push("deadline: set".to_string());
        }
        if let Some(budget) = self.token_budget {
            if self.baseline_captured {
                parts.push(format!("token budget: {budget}"));
            } else {
                parts.push(format!(
                    "token budget: {budget} (declared, unenforced: counter unavailable)"
                ));
            }
        }
        parts.join(", ")
    }

    /// The half that DOES move with the wall clock — the live countdowns.
    ///
    /// Delivered to the model on the transient trailing user message (the same
    /// channel `HarnessDeps::recall_context` uses), which sits on the far side
    /// of the cache breakpoint: changing bytes there cost only themselves. The
    /// model still sees a live countdown every turn; it just no longer bills the
    /// whole conversation for it.
    ///
    /// Empty when the loop has no deadline and is not model-paced.
    #[must_use]
    pub fn live_status(&self, now_ms: u64) -> Vec<String> {
        let mut parts = Vec::new();
        if let Some(deadline) = self.deadline_ms {
            if now_ms != 0 && deadline > now_ms {
                parts.push(format!("time left: {}", fmt_duration_ms(deadline - now_ms)));
            }
        }
        if matches!(self.cadence, Cadence::ModelPaced { .. }) {
            match self.next_wake_ms {
                Some(wake) if now_ms != 0 && wake > now_ms => {
                    parts.push(format!("next wake: in {}", fmt_duration_ms(wake - now_ms)));
                }
                Some(_) => parts.push("next wake: due now".to_string()),
                None => parts.push("next wake: unset (uses fallback)".to_string()),
            }
        }
        // Fixed-cadence next fire rides the transient tail too — see
        // `human_summary` for why `pending_tick_wake_ms` (not `next_wake_ms`)
        // holds the known wake for a fixed loop.
        if matches!(self.cadence, Cadence::Fixed { .. }) {
            if let Some(wake) = self.pending_tick_wake_ms {
                if now_ms != 0 && wake > now_ms {
                    parts.push(format!("next tick: in {}", fmt_duration_ms(wake - now_ms)));
                }
            }
        }
        parts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> LoopState {
        LoopState::new(
            "sess-1",
            "check deploy status",
            Cadence::Fixed {
                interval_ms: 300_000,
            },
            1_000,
        )
    }

    #[test]
    fn new_loop_is_active_with_zero_iterations() {
        let l = sample();
        assert_eq!(l.status, LoopStatus::Active);
        assert_eq!(l.iterations_used, 0);
        assert_eq!(l.session_id, "sess-1");
        assert_eq!(l.created_at_ms, 1_000);
        assert!(l.max_iterations.is_none());
        assert!(l.deadline_ms.is_none());
        assert!(l.next_wake_ms.is_none());
    }

    #[test]
    fn spent_iteration_increments_and_returns_new_copy() {
        let l = sample();
        let after = l.clone().spent_iteration();
        assert_eq!(after.iterations_used, 1);
        assert_eq!(l.iterations_used, 0, "original unchanged");
    }

    #[test]
    fn with_status_returns_new_copy() {
        let l = sample();
        let stopped = l.clone().with_status(LoopStatus::Stopped);
        assert_eq!(stopped.status, LoopStatus::Stopped);
        assert_eq!(l.status, LoopStatus::Active, "original unchanged");
    }

    #[test]
    fn with_caps_sets_optional_bounds() {
        let l = sample()
            .with_max_iterations(Some(20))
            .with_deadline_ms(Some(99_999));
        assert_eq!(l.max_iterations, Some(20));
        assert_eq!(l.deadline_ms, Some(99_999));
    }

    #[test]
    fn with_next_wake_sets_model_paced_delay() {
        let l = LoopState::new(
            "s",
            "p",
            Cadence::ModelPaced {
                fallback_ms: 600_000,
            },
            0,
        )
        .with_next_wake_ms(Some(480_000));
        assert_eq!(l.next_wake_ms, Some(480_000));
    }

    #[test]
    fn with_prompt_retargets_without_touching_other_fields() {
        let l = sample()
            .with_max_iterations(Some(9))
            .with_prompt("watch staging");
        assert_eq!(l.prompt, "watch staging");
        assert_eq!(l.max_iterations, Some(9), "other fields preserved");
    }

    #[test]
    fn with_cadence_to_fixed_clears_stale_next_wake() {
        // A model-paced loop carrying a next_wake, re-paced to Fixed: the
        // next_wake is meaningless for Fixed and must be cleared.
        let l = LoopState::new(
            "s",
            "p",
            Cadence::ModelPaced {
                fallback_ms: 600_000,
            },
            0,
        )
        .with_next_wake_ms(Some(123_456))
        .with_cadence(Cadence::Fixed {
            interval_ms: 120_000,
        });
        assert!(matches!(
            l.cadence,
            Cadence::Fixed {
                interval_ms: 120_000
            }
        ));
        assert!(l.next_wake_ms.is_none(), "stale next_wake cleared on Fixed");
    }

    #[test]
    fn new_loop_has_no_owner_scope() {
        let l = sample();
        assert_eq!(l.owner_user_id, None);
        assert_eq!(l.scope_id, None);
    }

    #[test]
    fn with_owner_scope_stamps_both_fields() {
        let attr = crate::scope::ScopeAttribution::personal("u-alice");
        let l = sample().with_owner_scope(Some(&attr));
        assert_eq!(l.owner_user_id.as_deref(), Some("u-alice"));
        assert_eq!(l.scope_id.as_deref(), Some("personal:u-alice"));
        // None clears both, mirroring goal::Goal::with_owner_scope(None).
        let cleared = l.with_owner_scope(None);
        assert_eq!(cleared.owner_user_id, None);
        assert_eq!(cleared.scope_id, None);
        // original unaffected (immutability)
        assert_eq!(sample().owner_user_id, None);
    }

    #[test]
    fn new_loop_has_no_baseline() {
        let l = sample();
        assert_eq!(l.tokens_at_start, 0);
        assert!(!l.baseline_captured);
        assert!(l.token_budget.is_none());
    }

    #[test]
    fn with_baseline_seeds_tokens_and_marks_captured() {
        let l = sample().with_baseline(12_345);
        assert_eq!(l.tokens_at_start, 12_345);
        assert!(l.baseline_captured);
        // original unchanged (immutability)
        assert_eq!(sample().tokens_at_start, 0);
    }

    #[test]
    fn tokens_used_saturates_on_counter_reset() {
        let l = sample().with_baseline(1_000);
        assert_eq!(l.tokens_used(1_750), 750);
        assert_eq!(l.tokens_used(500), 0, "counter going backwards saturates");
    }

    #[test]
    fn over_budget_only_when_budget_set_and_exceeded() {
        let l = sample().with_token_budget(Some(500)).with_baseline(1_000);
        assert!(!l.over_budget(1_200), "200 spent under a 500 budget");
        assert!(l.over_budget(1_600), "600 spent over a 500 budget");
        // No budget → never over budget, even at u64::MAX.
        assert!(!sample().with_baseline(1_000).over_budget(u64::MAX));
    }

    #[test]
    fn over_budget_is_inert_without_captured_baseline() {
        // tokens_at_start is the placeholder 0 until the claim path seeds the
        // baseline — the session's lifetime total must never read as loop
        // spend (type-level guard, goal hardening ⑨ parity).
        let l = sample().with_token_budget(Some(500));
        assert!(!l.over_budget(u64::MAX));
    }

    /// `stable_summary` is the only renderer allowed into the system prompt, so
    /// it must be a pure function of the loop state — feeding it two different
    /// clocks must not change a byte. A countdown there re-keys the whole
    /// conversation's cache prefix on every turn (write at 1.25x instead of read
    /// at 0.1x), for as long as the loop lives.
    #[test]
    fn stable_summary_is_clock_free() {
        let l = sample()
            .with_deadline_ms(Some(10_000))
            .with_token_budget(Some(500));

        let out = l.stable_summary();
        assert!(
            out.contains("deadline: set"),
            "existence, not remaining time"
        );
        assert!(
            !out.contains("time left") && !out.contains("next wake") && !out.contains("next tick"),
            "no countdown may reach the cached prefix; got: {out}"
        );
        // Byte-identical regardless of when it is rendered — the whole point.
        assert_eq!(out, l.stable_summary());
    }

    /// The countdown is not lost, only relocated: it rides the transient tail
    /// message, on the far side of the cache breakpoint, where changing bytes
    /// cost only themselves. The model still sees a live countdown every turn.
    #[test]
    fn live_status_carries_the_countdown_the_prompt_gave_up() {
        let l = sample().with_deadline_ms(Some(10_000));
        let live = l.live_status(4_000);
        assert!(
            live.iter().any(|s| s.contains("time left: 6s")),
            "got: {live:?}"
        );
        // No clock available → no misleading countdown, and nothing to relocate.
        assert!(l.live_status(0).is_empty());
    }

    /// The user-facing tool reply keeps its live countdown — `human_summary` is
    /// unchanged by the prompt-side split.
    #[test]
    fn human_summary_still_renders_the_live_countdown_for_the_tool_reply() {
        let l = sample().with_deadline_ms(Some(10_000));
        assert!(l.human_summary(4_000).contains("time left: 6s"));
    }

    /// A budget declared but never enforced (the session token counter never
    /// produced a number) must read in BOTH renderers as "declared,
    /// unenforced". The previous wording printed "token budget: 50000" and
    /// let the operator assume enforcement was on — exactly the lie the type
    /// invariant `baseline_captured ⇒ enforced` exists to forbid.
    #[test]
    fn token_budget_unenforced_is_visible_in_both_renderers() {
        let l = sample().with_token_budget(Some(50_000));
        // baseline_captured stays false (the constructor never captures it;
        // only the first claim that sees a budget + a total does).
        let human = l.human_summary(1_000);
        assert!(
            human.contains("declared, unenforced"),
            "human_summary must surface unenforced: {human}"
        );
        let stable = l.stable_summary();
        assert!(
            stable.contains("declared, unenforced"),
            "stable_summary must surface unenforced (it is the only renderer \
             allowed into the system prompt): {stable}"
        );

        // Once the baseline is captured, the unenforced hint disappears.
        let captured = l.with_baseline(1_000);
        assert!(captured
            .human_summary(1_000)
            .contains("token budget: 50000"));
        assert!(captured
            .human_summary(1_000)
            .contains("token budget: 50000"));
        assert!(!captured.stable_summary().contains("unenforced"));
    }

    #[test]
    fn fmt_duration_renders_compact_human_units() {
        assert_eq!(fmt_duration_ms(45_000), "45s");
        assert_eq!(fmt_duration_ms(300_000), "5m");
        assert_eq!(fmt_duration_ms(3_600_000), "1h");
        assert_eq!(fmt_duration_ms(5_400_000), "1h30m");
        assert_eq!(fmt_duration_ms(90_000), "1m30s");
        assert_eq!(fmt_duration_ms(0), "0s");
    }

    #[test]
    fn cadence_describe_matches_input_vocabulary() {
        assert_eq!(
            Cadence::Fixed {
                interval_ms: 300_000
            }
            .describe(),
            "every 5m"
        );
        assert!(Cadence::ModelPaced {
            fallback_ms: 600_000
        }
        .describe()
        .contains("model-paced"));
    }

    #[test]
    fn paused_is_inert_to_the_tick_pipeline_but_still_adjustable() {
        // The whole safety argument for Paused: it fails `is_active`, so
        // `try_claim_tick` / `confirm_fire` / `get_active` treat it exactly like
        // Stopped and no tick can ever fire. What it keeps is adjustability —
        // that is what separates a pause from a stop.
        let paused = sample().with_status(LoopStatus::Paused);
        assert!(!paused.is_active(), "a paused loop must never fire");
        assert!(paused.is_paused());
        assert!(paused.is_adjustable(), "pause exists to be re-paced");

        let stopped = sample().with_status(LoopStatus::Stopped);
        assert!(!stopped.is_paused());
        assert!(
            !stopped.is_adjustable(),
            "a stopped loop can never run again — update must refuse it"
        );
        assert!(sample().is_adjustable(), "active loops are adjustable too");
    }

    #[test]
    fn paused_loop_keeps_its_accumulated_progress() {
        // Pause is reversible: unlike stop→start, the tick count, caps, prompt
        // and cadence all survive so `resume` continues where it left off.
        let paused = sample()
            .with_max_iterations(Some(20))
            .spent_iteration()
            .spent_iteration()
            .with_status(LoopStatus::Paused);
        assert_eq!(paused.iterations_used, 2);
        assert_eq!(paused.max_iterations, Some(20));
        let resumed = paused.with_status(LoopStatus::Active);
        assert_eq!(
            resumed.iterations_used, 2,
            "resume does not rewind the count"
        );
        assert!(resumed.is_active());
    }

    #[test]
    fn status_as_str_is_the_single_render_source() {
        // Both summaries render the status through `as_str`, so a new variant
        // can never surface in one and be forgotten in the other.
        assert_eq!(LoopStatus::Active.as_str(), "active");
        assert_eq!(LoopStatus::Paused.as_str(), "paused");
        assert_eq!(LoopStatus::Stopped.as_str(), "stopped");
        let paused = sample().with_status(LoopStatus::Paused);
        assert!(paused.human_summary(2_000).starts_with("Loop paused"));
        assert!(paused.stable_summary().starts_with("Loop paused"));
    }

    #[test]
    fn with_stop_reason_sets_and_clears() {
        let l = sample().with_stop_reason(Some("hit cap".to_string()));
        assert_eq!(l.stop_reason.as_deref(), Some("hit cap"));
        assert!(l.with_stop_reason(None).stop_reason.is_none());
        assert!(sample().stop_reason.is_none(), "new loop has no reason");
    }

    #[test]
    fn human_summary_surfaces_stop_reason_and_caps() {
        let active = sample().with_max_iterations(Some(20));
        let s = active.human_summary(2_000);
        assert!(s.contains("Loop active"));
        assert!(s.contains("every 5m"));
        assert!(s.contains("ticks: 0/20"));

        let stopped = sample()
            .with_status(LoopStatus::Stopped)
            .with_stop_reason(Some("reached the iteration cap (20 ticks).".to_string()));
        let s = stopped.human_summary(2_000);
        assert!(s.contains("Loop stopped"));
        assert!(s.contains("reason: reached the iteration cap"));
    }

    #[test]
    fn human_summary_shows_next_tick_for_fixed_with_pending() {
        // A fixed loop with a tick enqueued (pending_tick_wake_ms set) reports the
        // known next fire — the gap this arm closes: before it, only model-paced
        // loops rendered a next-fire countdown. sample() is a Fixed 5m loop.
        let l = sample().with_pending_tick(Some(10_000));
        assert!(
            l.human_summary(4_000).contains("next tick: in 6s"),
            "{}",
            l.human_summary(4_000)
        );
        // No pending (a firing tick cleared it) → no next-tick line.
        assert!(!sample().human_summary(4_000).contains("next tick"));
        // Clock unavailable (0) → no misleading countdown.
        assert!(!l.human_summary(0).contains("next tick"));
    }

    #[test]
    fn live_status_shows_next_tick_for_fixed_with_pending() {
        let l = sample().with_pending_tick(Some(10_000));
        assert!(l
            .live_status(4_000)
            .iter()
            .any(|s| s.contains("next tick: in 6s")));
        // No pending and no deadline → nothing to relocate onto the tail.
        assert!(sample().live_status(4_000).is_empty());
    }

    #[test]
    fn human_summary_shows_relative_next_wake_for_model_paced() {
        let l = LoopState::new(
            "s",
            "p",
            Cadence::ModelPaced {
                fallback_ms: 600_000,
            },
            0,
        )
        .with_next_wake_ms(Some(10_000));
        // now=4_000, wake=10_000 → "in 6s"
        assert!(l.human_summary(4_000).contains("next wake: in 6s"));
    }
}
