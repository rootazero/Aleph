//! Execution engine configuration types

use crate::thinker::prompt_mode::PromptMode;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Execution engine settings (agent timeout, iteration limits)
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExecutionConfig {
    /// Default agent timeout in seconds (default: 172800 = 48 hours)
    #[serde(default = "default_timeout_secs")]
    pub default_timeout_secs: u64,

    /// Maximum iterations per agent run (default: 1000)
    ///
    /// Each "iteration" is one Think→Act loop in the harness. Long-running
    /// scheduled tasks (multi-source research, cross-tool synthesis) can
    /// legitimately need hundreds of iterations, so the default is set
    /// generously. Lower it per-deployment if you want tighter guardrails.
    #[serde(default = "default_max_iterations")]
    pub max_iterations: usize,

    /// System-prompt verbosity tier (`"full"` / `"compact"` / `"minimal"`,
    /// default: `"full"`).
    ///
    /// Drives which prompt layers participate in assembly (see
    /// [`PromptMode`]). `full` keeps every guidance section; `compact` and
    /// `minimal` progressively shed heavy layers (safety constitution, memory
    /// protocol, operational guidelines, …) that frontier models already
    /// internalise — the "thin prompt, trust the model" posture. The default
    /// is byte-identical to prior behaviour.
    #[serde(default)]
    pub prompt_mode: PromptMode,

    /// R5 "AI comes to you" progress push (default: false).
    ///
    /// When enabled, a run bound to a user channel mirrors scratchpad
    /// progress (objective set, plan laid out, steps ticked) and
    /// watchdog-boundary events (verifier-veto / failure-cap) to that
    /// channel, so headless / background long runs aren't a black box.
    /// Pure I/O side-channel — never touches the agent loop. Off by default
    /// to preserve prior behaviour; opt in per-deployment.
    #[serde(default)]
    pub progress_push: bool,

    /// Gateway mid-turn steering (default: true).
    ///
    /// When enabled, a message that arrives while the same session has an
    /// active run is injected into the live event log so the running loop
    /// catches it on its next turn (the `Steer` busy-input mode), instead of
    /// being rejected with `AgentBusy`. Disable to restore the legacy
    /// busy/retry behaviour. Defaults to `true` to preserve current behaviour —
    /// the engine previously hardcoded this on with no operator override.
    #[serde(default = "default_mid_turn_steering")]
    pub mid_turn_steering: bool,

    /// Global cap on concurrently-executing runs across all sessions/agents
    /// (default: 8). Enforced by `ConcurrencyLimiter` (audit 1.4).
    #[serde(default = "default_max_runs_global")]
    pub max_runs_global: usize,

    /// Per-agent sub-cap so one busy agent can't monopolize all global slots
    /// (default: 3, audit C4). Per-session is hard-capped at 1 by
    /// `SessionRunRegistry`.
    #[serde(default = "default_max_runs_per_agent")]
    pub max_runs_per_agent: usize,

    /// Sub-agents one agent run may have executing concurrently (default: 4).
    ///
    /// Every other ceiling of this family is operator-configurable
    /// (`max_runs_global`, `max_runs_per_agent`, `parallel_tool_concurrency`);
    /// this one was a compile-time constant, so a deployment with a
    /// high-throughput provider had no way to widen a fan-out and one behind a
    /// tight rate limit had no way to narrow it.
    ///
    /// Clamped into `[1, 64]` at the enforcement point
    /// (`agents::subagent_tool::clamp_max_concurrent_subagents`): `0` is a
    /// semaphore no child can ever acquire (every fan-out would park until its
    /// batch deadline), and an unbounded value trades a rate-limit wall for a
    /// task-budget one.
    ///
    /// Raising it also widens the synchronous batch's wave arithmetic — a
    /// `rows`-wide batch runs in `ceil(rows / permits)` waves and each child's
    /// wall-clock share is divided by that count — so the per-child timeout cap
    /// moves with this knob by construction.
    #[serde(default = "default_max_concurrent_subagents")]
    pub max_concurrent_subagents: usize,

    /// Messages that may wait in one session's busy FIFO lane (default: 32).
    ///
    /// Past this the newest arrival is refused up front (`REJECT_NEWEST`), so
    /// a flooding channel hears back immediately instead of building a deep
    /// silent backlog. Raise it for chatty group channels; lower it to push
    /// back sooner.
    #[serde(default = "default_busy_queue_max_per_session")]
    pub busy_queue_max_per_session: usize,

    /// How long a queued message may wait before the sender is told delivery
    /// failed (default: 1800 = 30 min).
    ///
    /// Queued channel messages are an async medium (R5), so silently dropping
    /// a "user changed their mind" message minutes into a long run is the
    /// worst outcome — but the wait is bounded so a wedged run can't strand
    /// waiters forever.
    #[serde(default = "default_busy_queue_max_wait_secs")]
    pub busy_queue_max_wait_secs: u64,

    /// Safety net between wake signals for a queued message (default: 30s).
    ///
    /// Waiters are woken by the engine's session-slot release, so this tick
    /// only exists so a missed signal degrades to a slightly later delivery
    /// instead of a wedged lane. It is NOT the delivery latency — that is
    /// governed by the wake signal. Lower it only when debugging.
    #[serde(default = "default_busy_queue_wake_fallback_secs")]
    pub busy_queue_wake_fallback_secs: u64,

    /// Un-consumed mid-loop steering messages one run may accumulate before
    /// the gateway applies backpressure (default: 16).
    ///
    /// Past the cap an injection is refused so the busy FIFO lane redelivers
    /// once the burst drains — backpressure, never a drop. Each pending
    /// message lands in the running loop's next prompt, so this bounds how
    /// much a flooding channel can inflate that prompt.
    #[serde(default = "default_max_pending_steering")]
    pub max_pending_steering: usize,
}

const fn default_timeout_secs() -> u64 {
    172_800
}

const fn default_max_iterations() -> usize {
    1000
}

const fn default_mid_turn_steering() -> bool {
    true
}

const fn default_max_runs_global() -> usize {
    8
}

const fn default_max_runs_per_agent() -> usize {
    3
}

/// Bound to the enforcement point's own default rather than a second literal
/// `4`: the two must agree or a config-less deployment and a config-with-
/// defaults deployment get different fan-out widths.
const fn default_max_concurrent_subagents() -> usize {
    crate::agents::subagent_tool::DEFAULT_MAX_CONCURRENT_SUBAGENTS
}

const fn default_busy_queue_max_per_session() -> usize {
    32
}

const fn default_busy_queue_max_wait_secs() -> u64 {
    1800
}

const fn default_busy_queue_wake_fallback_secs() -> u64 {
    30
}

const fn default_max_pending_steering() -> usize {
    16
}

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self {
            default_timeout_secs: default_timeout_secs(),
            max_iterations: default_max_iterations(),
            prompt_mode: PromptMode::default(),
            progress_push: false,
            mid_turn_steering: default_mid_turn_steering(),
            max_runs_global: default_max_runs_global(),
            max_runs_per_agent: default_max_runs_per_agent(),
            max_concurrent_subagents: default_max_concurrent_subagents(),
            busy_queue_max_per_session: default_busy_queue_max_per_session(),
            busy_queue_max_wait_secs: default_busy_queue_max_wait_secs(),
            busy_queue_wake_fallback_secs: default_busy_queue_wake_fallback_secs(),
            max_pending_steering: default_max_pending_steering(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_defaults() {
        let config = ExecutionConfig::default();
        assert_eq!(config.default_timeout_secs, 172_800);
        assert_eq!(config.max_iterations, 1000);
    }

    #[test]
    fn test_serde_roundtrip() {
        let config = ExecutionConfig::default();
        let toml = toml::to_string(&config).unwrap();
        let parsed: ExecutionConfig = toml::from_str(&toml).unwrap();
        assert_eq!(parsed.default_timeout_secs, 172_800);
        assert_eq!(parsed.max_iterations, 1000);
    }

    #[test]
    fn test_serde_with_missing_fields() {
        let parsed: ExecutionConfig = toml::from_str("").unwrap();
        assert_eq!(parsed.default_timeout_secs, 172_800);
        assert_eq!(parsed.max_iterations, 1000);
        // Absent `prompt_mode` defaults to Full — preserves prior behaviour.
        assert_eq!(parsed.prompt_mode, PromptMode::Full);
        // Absent `mid_turn_steering` defaults to true — the engine's prior
        // hardcoded behaviour, now operator-overridable.
        assert!(parsed.mid_turn_steering);
        // Absent concurrency caps default to 8 (global) / 3 (per-agent) —
        // backward-compatible with TOML files predating these knobs.
        assert_eq!(parsed.max_runs_global, 8);
        assert_eq!(parsed.max_runs_per_agent, 3);
        // Absent busy-queue / steering knobs keep the values that used to be
        // hardcoded constants — zero drift for TOML files predating them.
        assert_eq!(parsed.busy_queue_max_per_session, 32);
        assert_eq!(parsed.busy_queue_max_wait_secs, 1800);
        assert_eq!(parsed.busy_queue_wake_fallback_secs, 30);
        assert_eq!(parsed.max_pending_steering, 16);
        // W27 — absent sub-agent cap keeps the compile-time constant that used
        // to be the only answer, so nothing moves for existing TOML files.
        assert_eq!(
            parsed.max_concurrent_subagents,
            crate::agents::subagent_tool::DEFAULT_MAX_CONCURRENT_SUBAGENTS
        );
    }

    /// W27 — the point of the key: an operator can widen or narrow the
    /// sub-agent fan-out without recompiling.
    #[test]
    fn the_subagent_concurrency_cap_is_operator_overridable() {
        let parsed: ExecutionConfig = toml::from_str("max_concurrent_subagents = 12").unwrap();
        assert_eq!(parsed.max_concurrent_subagents, 12);
        assert_eq!(parsed.max_runs_global, 8, "unset siblings keep defaults");
    }

    #[test]
    fn busy_queue_knobs_are_operator_overridable() {
        let parsed: ExecutionConfig = toml::from_str(
            "busy_queue_max_per_session = 8\n\
             busy_queue_max_wait_secs = 120\n\
             max_pending_steering = 4\n",
        )
        .unwrap();
        assert_eq!(parsed.busy_queue_max_per_session, 8);
        assert_eq!(parsed.busy_queue_max_wait_secs, 120);
        assert_eq!(parsed.max_pending_steering, 4);
        // Unset sibling still falls back to its default.
        assert_eq!(parsed.busy_queue_wake_fallback_secs, 30);
    }

    #[test]
    fn test_prompt_mode_parses_lowercase() {
        let parsed: ExecutionConfig = toml::from_str("prompt_mode = \"minimal\"").unwrap();
        assert_eq!(parsed.prompt_mode, PromptMode::Minimal);
        let parsed: ExecutionConfig = toml::from_str("prompt_mode = \"compact\"").unwrap();
        assert_eq!(parsed.prompt_mode, PromptMode::Compact);
    }
}
