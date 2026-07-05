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
    }

    #[test]
    fn test_prompt_mode_parses_lowercase() {
        let parsed: ExecutionConfig = toml::from_str("prompt_mode = \"minimal\"").unwrap();
        assert_eq!(parsed.prompt_mode, PromptMode::Minimal);
        let parsed: ExecutionConfig = toml::from_str("prompt_mode = \"compact\"").unwrap();
        assert_eq!(parsed.prompt_mode, PromptMode::Compact);
    }
}
