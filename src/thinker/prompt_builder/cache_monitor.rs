//! Cache hit/miss monitor for prompt caching.
//!
//! Tracks whether LLM responses include cache read tokens, emitting warnings
//! when consecutive misses suggest the stable prompt prefix has changed
//! unexpectedly.
//!
//! Two grain rules keep the signal honest (both were real false-positive /
//! masking sources when the monitor was a single flat counter):
//!
//! - **Per-prefix tracking.** Each `(agent, session)` pair gets its own
//!   consecutive-miss counter — see [`cache_scope`] — so neither an interleaved
//!   cache-hitting agent nor a second healthy session of the *same* agent can
//!   reset the counter and mask a genuine prefix breakage.
//! - **Armed only after observed cache activity.** Misses are counted only
//!   once an agent has reported at least one non-zero `cache_read` or
//!   `cache_creation` value. Endpoints that never report cache usage (mock
//!   providers, Ollama, `cache_retention = off`, hosts without
//!   `cache_control` support) produce `None`/0 readings that say nothing
//!   about the stable prefix — they no longer pollute the counters.
//!
//! ## Known leniency (acknowledged, not a bug to fix)
//!
//! The health criterion is read-dominance: `reads > 0 && reads >= writes`.
//! Providers that report cache *reads* but never *creation* (OpenAI's
//! automatic prefix caching reports `cached_tokens` only) therefore always
//! read healthy on any hit — the watchdog cannot see an OpenAI prefix bust
//! whose reads keep landing on a shrinking cached span. This is accepted:
//! the monitor's failure direction everywhere else is deliberate
//! under-reporting (it never cries wolf), and OpenAI prefix caching is
//! server-side automatic with no breakpoint layout for us to defend. The
//! streak signal exists for providers with full read/creation accounting
//! (Anthropic), where a churning stable prefix is actionable.

use crate::sync_primitives::Mutex;
use std::collections::HashMap;

// =============================================================================
// Internal state
// =============================================================================

#[derive(Default)]
struct AgentCacheState {
    consecutive_misses: u32,
    total_calls: u64,
    /// Set once this agent has observed any cache activity (read or write).
    /// Miss counting (and the warn) is gated on it — see module docs.
    armed: bool,
    /// Latch so the warn fires on the RISING edge of a streak rather than on
    /// every call past the threshold. Without it a genuinely broken agent
    /// emits one WARN per LLM call, which is how a line gets filtered out —
    /// and this is the only alarm this domain has. Cleared by a healthy call.
    warned: bool,
    /// Logical clock of the last call seen for this scope (monitor-local
    /// tick, not wall time — deterministic under test). Backs the eviction
    /// policy below.
    last_seen: u64,
    /// Hash of the last call's stable-prefix bytes (the `cache: true` system
    /// blocks), handed in by `MeteringProvider`. Exists for ONE purpose:
    /// when a streak fires, the report can say whether the stable prefix
    /// actually changed between calls — the miss attribution the bare
    /// "prefix is churning" alarm used to lack. This is NOT the deleted
    /// per-call hash comparator revived: it never gates anything, it only
    /// annotates the alarm edge, which now has consumers (trace event →
    /// TUI/Panel/doctor).
    last_prefix_hash: Option<u64>,
}

#[derive(Default)]
struct MonitorState {
    map: HashMap<String, AgentCacheState>,
    /// Monotonic logical clock, bumped once per recorded call.
    tick: u64,
}

/// Bound on tracked scopes. Subagent spawns can mint fresh ids over a
/// long daemon lifetime; past the cap the least-recently-active *idle*
/// scope (no in-flight streak) is evicted — never an arbitrary one, so the
/// scope whose streak is about to fire cannot be silently dropped (the
/// monitor is a best-effort watchdog, not an accounting ledger).
const MAX_TRACKED_AGENTS: usize = 64;

/// Separator for the composite tracking key. A unit separator cannot occur in
/// an agent id or a serialized `SessionKey`, so no two distinct pairs collide.
const SCOPE_SEP: char = '\u{1f}';

/// Structured form of the watchdog's rising-edge alarm, returned by
/// [`CacheMonitor::record_cache_usage`] when a streak crosses the warn
/// threshold. The monitor itself stays sink-free (a pure counter); the
/// caller — `MeteringProvider` — turns this into a
/// `LoopTraceEvent::CacheHealthDegraded` on the run's trace stream, which is
/// what lifts the domain's only alarm out of the tracing log and onto every
/// trace consumer (TUI, Panel, `core/cache-health` doctor check).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheHealthReport {
    /// The scope whose streak fired — `cache_scope(agent, session)`.
    pub scope: String,
    /// Consecutive read-dominated-violating calls at firing time.
    pub streak: u32,
    /// The firing call's cache-read / cache-creation token counts.
    pub reads: u64,
    pub writes: u64,
    /// Miss attribution: did the stable-prefix bytes actually change between
    /// the previous call and the firing one? `Some(true)` confirms the
    /// churn hypothesis; `Some(false)` points at provider-side eviction or
    /// a TTL boundary instead; `None` when either call carried no hashable
    /// stable prefix (legacy Basic path).
    pub prefix_changed: Option<bool>,
}

/// Hash of a request's stable-prefix bytes (the `cache: true` system
/// blocks), for miss attribution at the watchdog's alarm edge. `None` on the
/// legacy flat-prompt path — the same bytes that path refuses to
/// content-address (see `openai_common::prompt_cache`). Truncated to `u64`:
/// this is a churn detector, not an identity.
///
/// Computed by `MeteringProvider` per call (one SHA-256 over the stable
/// blocks, microseconds at prompt sizes) and STORED by the monitor; the
/// comparison happens only when a streak fires — see
/// [`CacheHealthReport::prefix_changed`].
#[must_use]
pub fn stable_prefix_hash(payload: &crate::providers::adapter::RequestPayload<'_>) -> Option<u64> {
    use sha2::{Digest, Sha256};
    let parts = payload.system_blocks?;
    let mut hasher = Sha256::new();
    let mut any = false;
    for part in parts.iter().filter(|p| p.cache) {
        hasher.update(part.content.as_bytes());
        hasher.update([0u8]);
        any = true;
    }
    if !any {
        return None;
    }
    let digest = hasher.finalize();
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    Some(u64::from_le_bytes(bytes))
}

/// The watchdog's tracking key: one counter per prompt-cache **prefix**.
///
/// A provider's prefix is scoped to the conversation, not to the agent. Keying
/// on the agent alone meant two concurrent sessions of the same agent shared a
/// counter, so a healthy session zeroed the broken one's streak on every call.
/// That failure is silent and one-directional — the watchdog under-reports, it
/// never cries wolf — which is exactly why it needed finding by hand.
///
/// **Both sides must build the key through here.** The recording side
/// (`MeteringProvider`) and the reset side (compaction) disagreeing about the
/// key shape would be strictly worse than the bug it replaces: resets would
/// stop landing and the watchdog would start warning about compactions the user
/// asked for. `None` (no session in scope) degrades to the historical
/// agent-only key rather than inventing one.
#[must_use]
pub fn cache_scope(agent_id: &str, session_key: Option<&str>) -> String {
    session_key.map_or_else(
        || agent_id.to_string(),
        |session| format!("{agent_id}{SCOPE_SEP}{session}"),
    )
}

// =============================================================================
// CacheMonitor
// =============================================================================

/// Monitor for prompt cache hit/miss tracking.
///
/// Thread-safe via interior mutability. Callers record the
/// `cache_read_tokens` / `cache_creation_tokens` values from each call's
/// `TokenUsage` to detect consecutive cache misses per agent. (A per-call
/// stable-content hash comparator and an aggregate `hit_rate()` accessor
/// used to live here too; neither ever gained a production consumer —
/// removed per YAGNI. Aggregate hit-rate lives in the usage DB rollup and
/// the Panel Usage view, which are per-agent and persisted.)
pub struct CacheMonitor {
    state: Mutex<MonitorState>,
}

impl CacheMonitor {
    /// Create a new monitor with empty state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Mutex::new(MonitorState::default()),
        }
    }

    /// Record cache usage from a completed LLM call attributed to `scope`
    /// (build it with [`cache_scope`] — one counter per prompt-cache prefix).
    ///
    /// A `cache_read_tokens` value of `None` or `Some(0)` counts as a miss —
    /// but only once the agent is *armed* (has ever reported non-zero cache
    /// read or creation tokens). After 3 or more consecutive armed misses
    /// (and at least 4 total calls) a `warn!` is emitted so operators can
    /// investigate a possible stable-prefix change, and a
    /// [`CacheHealthReport`] is returned on that same rising edge (`None`
    /// every other call) so the caller can route the alarm onto the trace
    /// stream — see the struct docs.
    /// `prefix_hash` is the caller-computed hash of this call's stable-prefix
    /// bytes (`None` on the legacy flat-prompt path). It is stored per scope
    /// and compared ONLY at the alarm edge, to fill
    /// [`CacheHealthReport::prefix_changed`] — see the field docs.
    pub fn record_cache_usage(
        &self,
        scope: &str,
        cache_read_tokens: Option<u32>,
        cache_creation_tokens: Option<u32>,
        prefix_hash: Option<u64>,
    ) -> Option<CacheHealthReport> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.tick += 1;
        let tick = state.tick;

        if !state.map.contains_key(scope) && state.map.len() >= MAX_TRACKED_AGENTS {
            // Evict the least-recently-active scope with no in-flight streak.
            // The old `keys().next()` eviction was arbitrary: it could drop
            // the very scope whose streak was one call from firing, muting
            // the domain's only alarm exactly when it was about to speak.
            // Only when EVERY scope is mid-streak (a swarm-wide breakage) do
            // we fall back to evicting the oldest streak.
            let victim = state
                .map
                .iter()
                .filter(|(_, s)| s.consecutive_misses == 0)
                .min_by_key(|(_, s)| s.last_seen)
                .or_else(|| state.map.iter().min_by_key(|(_, s)| s.last_seen))
                .map(|(k, _)| k.clone());
            if let Some(k) = victim {
                state.map.remove(&k);
            }
        }
        let agent = state.map.entry(scope.to_string()).or_default();
        agent.last_seen = tick;

        agent.total_calls += 1;

        let reads = u64::from(cache_read_tokens.unwrap_or(0));
        let writes = u64::from(cache_creation_tokens.unwrap_or(0));
        if reads > 0 || writes > 0 {
            agent.armed = true;
        }

        // A call counts as healthy only when the cache was read *at least as
        // much as* it was written.
        //
        // Treating any non-zero read as healthy made the watchdog blind to the
        // failure mode this codebase actually ships into. The layout is one
        // breakpoint on the small stable system block and three on the
        // conversation; when a prefix ahead of the message breakpoints churns,
        // the system block still hits on every call, so `reads > 0` held
        // forever, the streak reset on every call, and the warn could never
        // accumulate — while 100% of the history was re-created at 1.25x.
        // Read-dominance separates "hitting" from "re-creating and hitting a
        // few hundred bytes".
        let healthy = reads > 0 && reads >= writes;

        // Miss attribution material: compared against the previous call's
        // hash only at the alarm edge below, then stored for the next call.
        let previous_hash = agent.last_prefix_hash;

        if healthy {
            agent.consecutive_misses = 0;
            agent.warned = false;
        } else if agent.armed {
            agent.consecutive_misses += 1;
            if agent.consecutive_misses >= 3 && agent.total_calls > 3 && !agent.warned {
                agent.warned = true;
                let prefix_changed = match (previous_hash, prefix_hash) {
                    (Some(prev), Some(cur)) => Some(prev != cur),
                    _ => None,
                };
                tracing::warn!(
                    scope = %scope,
                    consecutive_misses = agent.consecutive_misses,
                    total_calls = agent.total_calls,
                    cache_read_tokens = reads,
                    cache_creation_tokens = writes,
                    prefix_changed = ?prefix_changed,
                    "prompt cache is being re-created rather than read — a prefix \
                     ahead of the message breakpoints is churning, or the stable \
                     prefix changed"
                );
                agent.last_prefix_hash = prefix_hash;
                return Some(CacheHealthReport {
                    scope: scope.to_string(),
                    streak: agent.consecutive_misses,
                    reads,
                    writes,
                    prefix_changed,
                });
            }
        }
        agent.last_prefix_hash = prefix_hash;
        None
    }

    /// Notify the monitor that a compaction has occurred for `scope`
    /// ([`cache_scope`] — the same key the recording side uses).
    ///
    /// Compaction legitimately breaks the prompt cache (the message list is
    /// rewritten), so consecutive-miss tracking is reset to avoid false
    /// positive warnings immediately after compaction. The reset is scoped
    /// to the compacting conversation when it is known — a global reset would
    /// wipe every OTHER scope's in-progress miss streak on each compaction
    /// and mute the watchdog process-wide in a busy swarm. `None` falls back
    /// to the global reset for call sites without an identity.
    pub fn notify_compaction(&self, scope: Option<&str>) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        match scope {
            Some(id) => {
                if let Some(agent) = state.map.get_mut(id) {
                    agent.consecutive_misses = 0;
                    agent.warned = false;
                }
            }
            None => {
                for agent in state.map.values_mut() {
                    agent.consecutive_misses = 0;
                    agent.warned = false;
                }
            }
        }
    }
}

impl Default for CacheMonitor {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Process-wide singleton
// =============================================================================
//
// The cache monitor is wired into the metering provider — every LLM call
// reports its cache token counts (keyed by agent id) to the singleton.
//
// Same `OnceLock` shape as `pricing` / `tool_result_store`: lazy install on
// first read, `&'static` reads on the hot path.

static GLOBAL_CACHE_MONITOR: std::sync::OnceLock<CacheMonitor> = std::sync::OnceLock::new();

/// Return the process-wide `CacheMonitor`, lazily creating it on first use.
pub fn global_cache_monitor() -> &'static CacheMonitor {
    GLOBAL_CACHE_MONITOR.get_or_init(CacheMonitor::new)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn misses(monitor: &CacheMonitor, agent: &str) -> u32 {
        monitor
            .state
            .lock()
            .expect("lock poisoned")
            .map
            .get(agent)
            .map_or(0, |s| s.consecutive_misses)
    }

    #[test]
    fn unarmed_agent_never_counts_misses() {
        // An endpoint that never reports cache usage (mock, Ollama, caching
        // off) must not accumulate misses — its readings carry no signal.
        let monitor = CacheMonitor::new();
        for _ in 0..10 {
            monitor.record_cache_usage("no-cache-agent", None, None, None);
        }
        assert_eq!(misses(&monitor, "no-cache-agent"), 0);
    }

    #[test]
    fn cache_write_arms_and_counts_as_miss() {
        // A cold cache write (creation > 0, read == 0) is real cache activity:
        // it arms the agent AND counts as the first consecutive miss.
        let monitor = CacheMonitor::new();
        monitor.record_cache_usage("a", None, Some(500), None);
        assert_eq!(misses(&monitor, "a"), 1);
        // A subsequent hit resets the streak.
        monitor.record_cache_usage("a", Some(100), None, None);
        assert_eq!(misses(&monitor, "a"), 0);
    }

    #[test]
    fn agents_are_tracked_independently() {
        // A cache-hitting agent must not reset another agent's miss streak.
        let monitor = CacheMonitor::new();
        monitor.record_cache_usage("broken", Some(10), None, None); // arm via a hit
        monitor.record_cache_usage("broken", None, Some(1), None); // miss 1
        monitor.record_cache_usage("healthy", Some(999), None, None); // unrelated hit
        monitor.record_cache_usage("broken", None, Some(1), None); // miss 2
        assert_eq!(misses(&monitor, "broken"), 2);
        assert_eq!(misses(&monitor, "healthy"), 0);
    }

    #[test]
    fn compaction_resets_consecutive_misses() {
        let monitor = CacheMonitor::new();
        // Drive up consecutive misses past the warning threshold
        // (needs > 3 total calls and >= 3 consecutive misses).
        monitor.record_cache_usage("a", Some(100), None, None); // hit — arms
        monitor.record_cache_usage("a", None, None, None); // miss
        monitor.record_cache_usage("a", None, None, None); // miss
        monitor.record_cache_usage("a", None, None, None); // miss — warn fires

        // Compaction resets the counter
        monitor.notify_compaction(Some("a"));

        // After reset, a single miss should NOT trigger a warning
        // (consecutive_misses is back to 0, so 1 miss = 1 consecutive)
        monitor.record_cache_usage("a", None, None, None);
        assert_eq!(
            misses(&monitor, "a"),
            1,
            "miss count should restart from 1 after compaction"
        );
    }

    #[test]
    fn scoped_compaction_reset_preserves_other_agents_streaks() {
        // Agent A compacting must not wipe agent B's in-progress miss streak
        // — a global reset would mute the watchdog process-wide whenever any
        // agent in a busy swarm compacts.
        let monitor = CacheMonitor::new();
        monitor.record_cache_usage("b", Some(10), None, None); // arm B
        monitor.record_cache_usage("b", None, None, None); // B miss 1
        monitor.record_cache_usage("b", None, None, None); // B miss 2
        monitor.record_cache_usage("a", Some(10), None, None); // arm A
        monitor.record_cache_usage("a", None, None, None); // A miss 1

        monitor.notify_compaction(Some("a")); // A compacts

        assert_eq!(misses(&monitor, "a"), 0, "compacting agent reset");
        assert_eq!(misses(&monitor, "b"), 2, "other agent's streak survives");

        // Global fallback (no identity) still resets everyone.
        monitor.notify_compaction(None);
        assert_eq!(misses(&monitor, "b"), 0);
    }

    #[test]
    fn one_agents_healthy_session_cannot_mask_its_broken_one() {
        // The provider's prefix is per conversation. Keyed on the agent alone,
        // the interleaved healthy session below zeroed the broken session's
        // streak on every call, so the streak never reached 3 and the only
        // alarm in this domain never fired. Silent and one-directional: the
        // watchdog under-reports, it never cries wolf.
        let monitor = CacheMonitor::new();
        let broken = cache_scope("writer", Some("agent:writer:main"));
        let healthy = cache_scope("writer", Some("agent:writer:review"));

        monitor.record_cache_usage(&broken, Some(10), None, None); // arm
        for _ in 0..3 {
            monitor.record_cache_usage(&broken, None, Some(50_000), None); // re-creating
            monitor.record_cache_usage(&healthy, Some(50_000), None, None); // interleaved, fine
        }

        assert_eq!(
            misses(&monitor, &broken),
            3,
            "the broken prefix kept its streak"
        );
        assert_eq!(misses(&monitor, &healthy), 0);

        // And a compaction in the healthy session does not silence the broken one.
        monitor.notify_compaction(Some(&healthy));
        assert_eq!(misses(&monitor, &broken), 3);
    }

    #[test]
    fn a_scope_without_a_session_is_the_bare_agent_id() {
        // Call sites with no conversation in hand degrade to the historical
        // key rather than inventing one — a scope that does not match what the
        // recording side uses would stop resets landing, which is the strictly
        // worse direction (warning about compactions the user asked for).
        assert_eq!(cache_scope("solo", None), "solo");
        assert_ne!(
            cache_scope("solo", Some("s1")),
            cache_scope("solo", Some("s2"))
        );
    }

    #[test]
    fn report_returned_on_rising_edge_only() {
        // The trace-stream hand-off contract: `Some(report)` exactly once per
        // streak (the same edge the warn latch uses), `None` on every other
        // call, so the sink sees one alarm per degradation, not one per call.
        let monitor = CacheMonitor::new();
        assert_eq!(
            monitor.record_cache_usage("edge", Some(10), None, None),
            None
        ); // arm
        assert_eq!(
            monitor.record_cache_usage("edge", None, Some(50), None),
            None
        ); // miss 1
        assert_eq!(
            monitor.record_cache_usage("edge", None, Some(50), None),
            None
        ); // miss 2
        let report = monitor
            .record_cache_usage("edge", None, Some(50), None)
            .expect("streak 3 with 4 total calls fires");
        assert_eq!(report.scope, "edge");
        assert_eq!(report.streak, 3);
        assert_eq!(report.writes, 50);
        // Latched: further misses do not re-report.
        assert_eq!(
            monitor.record_cache_usage("edge", None, Some(50), None),
            None
        );
        // A healthy call rearms — a fresh streak reports again.
        assert_eq!(
            monitor.record_cache_usage("edge", Some(10), None, None),
            None
        );
        for expected in [None, None] {
            assert_eq!(
                monitor.record_cache_usage("edge", None, Some(50), None),
                expected
            );
        }
        assert!(
            monitor
                .record_cache_usage("edge", None, Some(50), None)
                .is_some(),
            "rearmed streak reports on its own rising edge"
        );
    }

    #[test]
    fn eviction_prefers_idle_scopes_and_never_drops_a_live_streak() {
        // Regression for the old `keys().next()` arbitrary eviction: with the
        // map full, the next new scope must evict the least-recently-active
        // IDLE scope — never the one whose streak is a call away from the
        // domain's only alarm.
        let monitor = CacheMonitor::new();
        // Scope 0: armed and mid-streak (miss 1) — the one to protect.
        monitor.record_cache_usage("scope-00", Some(10), None, None); // arm
        monitor.record_cache_usage("scope-00", None, Some(50), None); // miss 1
                                                                      // Scopes 1..64: healthy, oldest first.
        for i in 1..MAX_TRACKED_AGENTS {
            monitor.record_cache_usage(&format!("scope-{i:02}"), Some(10), None, None);
        }
        assert_eq!(misses(&monitor, "scope-00"), 1);

        // One more scope forces an eviction. The victim must be scope-01
        // (oldest idle), NOT scope-00 (mid-streak).
        monitor.record_cache_usage("scope-new", Some(10), None, None);
        assert_eq!(
            misses(&monitor, "scope-00"),
            1,
            "mid-streak scope survived eviction"
        );
        let state = monitor.state.lock().expect("lock poisoned");
        assert!(state.map.contains_key("scope-new"));
        assert!(
            !state.map.contains_key("scope-01"),
            "oldest idle scope was the victim"
        );
        assert!(state.map.len() <= MAX_TRACKED_AGENTS);
    }

    #[test]
    fn alarm_edge_reports_whether_stable_prefix_changed() {
        // Miss attribution (Step 9): the report must say whether the stable
        // prefix bytes actually moved, so an operator can tell "our prompt
        // churned" from "the provider evicted us".
        let monitor = CacheMonitor::new();
        monitor.record_cache_usage("attr", Some(10), None, Some(7)); // arm
        monitor.record_cache_usage("attr", None, Some(50), Some(7));
        monitor.record_cache_usage("attr", None, Some(50), Some(7));
        let report = monitor
            .record_cache_usage("attr", None, Some(50), Some(7))
            .expect("fires");
        assert_eq!(
            report.prefix_changed,
            Some(false),
            "same prefix bytes → provider-side eviction, not churn"
        );

        let monitor = CacheMonitor::new();
        monitor.record_cache_usage("attr2", Some(10), None, Some(7)); // arm
        monitor.record_cache_usage("attr2", None, Some(50), Some(8));
        monitor.record_cache_usage("attr2", None, Some(50), Some(9));
        let report = monitor
            .record_cache_usage("attr2", None, Some(50), Some(10))
            .expect("fires");
        assert_eq!(
            report.prefix_changed,
            Some(true),
            "moving prefix bytes confirm the churn hypothesis"
        );

        // Legacy flat-prompt path carries no hashable stable prefix.
        let monitor = CacheMonitor::new();
        monitor.record_cache_usage("attr3", Some(10), None, None); // arm
        monitor.record_cache_usage("attr3", None, Some(50), None);
        monitor.record_cache_usage("attr3", None, Some(50), None);
        let report = monitor
            .record_cache_usage("attr3", None, Some(50), None)
            .expect("fires");
        assert_eq!(report.prefix_changed, None);
    }

    #[test]
    fn tracked_agents_stay_bounded() {
        let monitor = CacheMonitor::new();
        for i in 0..(MAX_TRACKED_AGENTS + 10) {
            monitor.record_cache_usage(&format!("agent-{i}"), Some(1), None, None);
        }
        let len = monitor.state.lock().expect("lock poisoned").map.len();
        assert!(len <= MAX_TRACKED_AGENTS, "map stayed bounded, got {len}");
    }
}
