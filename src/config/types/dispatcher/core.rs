//! Core dispatcher configuration types
//!
//! Contains `DispatcherConfigToml` and `AgentConfigToml` for the Dispatcher Layer (Aleph Cortex).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::warn;

// =============================================================================
// DispatcherConfigToml
// =============================================================================

/// Configuration for the Dispatcher Layer (Aleph Cortex)
///
/// The Dispatcher Layer provides intelligent tool routing through three layers:
/// - L1: Regex-based pattern matching (highest confidence)
/// - L2: Semantic keyword matching (medium confidence)
/// - L3: AI-powered inference (variable confidence)
///
/// When a tool match has low confidence, the system can show a confirmation
/// dialog to the user before execution.
///
/// # Example TOML
///
/// ```toml
/// [dispatcher]
/// enabled = true
/// l3_enabled = true
/// l3_timeout_ms = 5000
/// confirmation_threshold = 0.7
/// confirmation_timeout_ms = 30000
///
/// [dispatcher.agent]
/// enabled = true
/// max_steps = 10
/// step_timeout_ms = 30000
/// enable_rollback = true
/// plan_confirmation_required = true
/// allow_irreversible_without_confirmation = false
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DispatcherConfigToml {
    /// Whether the dispatcher is enabled (default: true)
    #[serde(default = "default_dispatcher_enabled")]
    pub enabled: bool,

    /// Whether L3 AI inference is enabled (default: true)
    #[serde(default = "default_dispatcher_l3_enabled")]
    pub l3_enabled: bool,

    /// L3 routing timeout in milliseconds (default: 5000)
    #[serde(default = "default_dispatcher_l3_timeout")]
    pub l3_timeout_ms: u64,

    /// Confidence threshold below which confirmation is required (0.0-1.0, default: 0.7)
    /// - Values >= 1.0 disable confirmation entirely
    /// - Values <= 0.0 always require confirmation
    #[serde(default = "default_dispatcher_confirmation_threshold")]
    pub confirmation_threshold: f32,

    /// Confirmation dialog timeout in milliseconds (default: 30000)
    #[serde(default = "default_dispatcher_confirmation_timeout")]
    pub confirmation_timeout_ms: u64,

    /// Whether confirmation dialogs are enabled (default: true)
    #[serde(default = "default_dispatcher_confirmation_enabled")]
    pub confirmation_enabled: bool,

    /// L3 Agent configuration for multi-step planning
    #[serde(default)]
    pub agent: AgentConfigToml,
}

pub const fn default_dispatcher_enabled() -> bool {
    true
}

pub const fn default_dispatcher_l3_enabled() -> bool {
    true
}

pub const fn default_dispatcher_l3_timeout() -> u64 {
    5000 // 5 seconds
}

pub const fn default_dispatcher_confirmation_threshold() -> f32 {
    0.7 // Require confirmation if confidence < 70%
}

pub const fn default_dispatcher_confirmation_timeout() -> u64 {
    30000 // 30 seconds
}

pub const fn default_dispatcher_confirmation_enabled() -> bool {
    true
}

impl Default for DispatcherConfigToml {
    fn default() -> Self {
        Self {
            enabled: default_dispatcher_enabled(),
            l3_enabled: default_dispatcher_l3_enabled(),
            l3_timeout_ms: default_dispatcher_l3_timeout(),
            confirmation_threshold: default_dispatcher_confirmation_threshold(),
            confirmation_timeout_ms: default_dispatcher_confirmation_timeout(),
            confirmation_enabled: default_dispatcher_confirmation_enabled(),
            agent: AgentConfigToml::default(),
        }
    }
}

// =============================================================================
// TeamDispatcherConfigToml — `[team_dispatcher]`
// =============================================================================

/// Operator tunables for the **team** `TeamDispatcher` loop (multi-agent
/// coordinated-task scheduling), distinct from the L3-Cortex
/// [`DispatcherConfigToml`] above.
///
/// Each field is `Option`: an absent key falls back to the live
/// `teams::dispatcher::DispatcherConfig::default()` at the boot site, so an
/// unconfigured deployment is byte-identical to prior behaviour and the
/// authoritative defaults never drift (they are read from the runtime struct,
/// not duplicated here). Closes the wiring gap where every dispatcher tunable
/// the runtime documented as operator-adjustable (`default_max_retries`,
/// `retry_backoff_*`, `zombie_ttl_secs`, …) was permanently pinned to its
/// default because the dispatcher was always built with `::default()`.
///
/// # Example TOML
///
/// ```toml
/// [team_dispatcher]
/// default_max_retries = 3
/// retry_backoff_base_secs = 10
/// zombie_ttl_secs = 1800   # 30 min — short-task workload
/// max_per_owner = 2
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct TeamDispatcherConfigToml {
    /// Max member tasks executing concurrently across the process.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrent: Option<usize>,
    /// A task lock older than this (seconds) is considered stale.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lock_ttl_secs: Option<u64>,
    /// Per-task execution timeout (seconds).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_timeout_secs: Option<u64>,
    /// Fallback wake interval (seconds) — catches any missed signal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_tick_secs: Option<u64>,
    /// `InProgress` longer than this (seconds) and not running here ⇒ zombie,
    /// force-failed. Clamped at the boot site to never drop below
    /// `task_timeout_secs` (else healthy long-running tasks get clobbered).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zombie_ttl_secs: Option<u64>,
    /// Max concurrent tasks a single owner may hold. `0` disables the hard cap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_per_owner: Option<usize>,
    /// Auto-retry budget for a failed/timed-out task before terminal `Failed`.
    /// `0` = first failure is terminal. Default `2` (3 total attempts).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_max_retries: Option<u32>,
    /// Base delay (seconds) for exponential retry backoff. `0` disables backoff.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_backoff_base_secs: Option<u64>,
    /// Upper bound (seconds) on a single retry's backoff delay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_backoff_cap_secs: Option<u64>,
}

// =============================================================================
// TeamBroadcastConfigToml — `[team_broadcast]`
// =============================================================================

/// Operator tunables for the multi-agent **group-chat broadcast** storm-prevention
/// guards (§4.5) — the broadcast-side parallel to [`TeamDispatcherConfigToml`]
/// (§4.4). Each field is `Option`: an absent key falls back to the live
/// `teams::broadcast::BroadcastConfig::default()` at the boot site, so an
/// unconfigured deployment is byte-identical to prior behaviour and the
/// authoritative defaults never drift (they are read from the runtime struct,
/// not duplicated here). Closes the wiring gap where the three storm-prevention
/// guards (`MAX_CHAIN_DEPTH` / `MAX_FANOUT_WIDTH` / `MAX_TOTAL_ACTIVATIONS`) and
/// the transcript budget were permanently pinned as bare `const`s.
///
/// A `0` for any guard would create a "born-dead" group chat (every chat blocked
/// at depth 0, nobody ever woken, etc.); the boot mapping treats `0` as "use the
/// default" (P7 boundary clamp), mirroring `[team_dispatcher]`'s zombie-ttl clamp.
///
/// # Example TOML
///
/// ```toml
/// [team_broadcast]
/// max_chain_depth = 8          # allow deeper A↔B back-and-forth
/// max_fanout_width = 3         # tighter @all blast radius
/// max_total_activations = 64   # larger team, more total member runs per turn
/// transcript_token_budget = 12000
/// member_run_timeout_secs = 900  # wall-clock cap per member run
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct TeamBroadcastConfigToml {
    /// Max reply-chain depth (guards against A↔B infinite @-pingpong).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_chain_depth: Option<u32>,
    /// Max agents woken in a single round (guards against `@all` blowing open a
    /// large team at once).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_fanout_width: Option<usize>,
    /// Max cumulative member activations across the whole fan-out tree of one
    /// user message (the global storm-prevention cap).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_total_activations: Option<usize>,
    /// Token budget for the group transcript injected into each member's prompt
    /// (over budget ⇒ most-recent kept from the tail).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_token_budget: Option<usize>,
    /// Wall-clock timeout (seconds) for a single group-chat member run.
    /// Absent ⇒ default (600, mirroring the dispatcher's `task_timeout_secs`);
    /// `0` ⇒ use the default (a literal 0 would kill every member run at
    /// birth — same P7 boundary clamp as the storm guards above).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member_run_timeout_secs: Option<u64>,
}

// =============================================================================
// TeamMessagesConfigToml — `[team_messages]`
// =============================================================================

/// Operator tunables for the team **message-router thread escalation** guard
/// (§4.5) — the third and last deterministic storm/escalation guard of the
/// teams subsystem, alongside [`TeamDispatcherConfigToml`] (§4.4) and
/// [`TeamBroadcastConfigToml`] (§4.5 broadcast). Each field is `Option`: an
/// absent key falls back to the live `teams::messages::EscalationRule::default()`
/// at the boot site, so an unconfigured deployment is byte-identical to prior
/// behaviour and the authoritative defaults never drift (they are read from the
/// runtime struct, not duplicated here).
///
/// The escalation guard is advisory-only: when a reply thread exceeds
/// `thread_message_threshold` messages the router sends the team leader ONE
/// `SystemNotification` suggesting a collaborative session — the LLM leader
/// decides what to do (no reasoning in the guard, upholds R7). Before this section
/// the threshold and on/off switch were pinned to `EscalationRule::default()`
/// (threshold 5, enabled) at the sole boot site, so an operator could neither
/// silence a noisy escalation nor tune the threshold without a rebuild — the
/// same config-ification asymmetry the broadcast storm guards already closed.
///
/// A `thread_message_threshold` of `0` would escalate on the very first reply
/// (born-noisy); the boot mapping treats `0` as "use the default" (P7 boundary
/// clamp), mirroring `[team_broadcast]`'s guard clamp. `escalation_enabled` is
/// honoured verbatim (including `false`) so operators can disable escalation.
///
/// # Example TOML
///
/// ```toml
/// [team_messages]
/// thread_message_threshold = 10   # nudge the leader only on longer threads
/// escalation_enabled = false      # or turn thread escalation off entirely
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct TeamMessagesConfigToml {
    /// Messages in a reply thread before the router nudges the leader to start
    /// a collaborative session. `0` ⇒ use the default (5) — a literal 0 would
    /// escalate on the first reply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_message_threshold: Option<u32>,
    /// Master switch for thread escalation. `false` disables the leader nudge
    /// entirely; absent ⇒ default (enabled).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub escalation_enabled: Option<bool>,
}

impl DispatcherConfigToml {
    /// Validate the configuration values
    ///
    /// # Returns
    /// * `Ok(())` - Configuration is valid
    /// * `Err(String)` - Validation error message
    pub fn validate(&self) -> std::result::Result<(), String> {
        // Validate confirmation threshold range
        if self.confirmation_threshold < 0.0 {
            return Err(format!(
                "confirmation_threshold must be >= 0.0, got {}",
                self.confirmation_threshold
            ));
        }
        if self.confirmation_threshold > 1.0 {
            warn!(
                threshold = self.confirmation_threshold,
                "confirmation_threshold > 1.0 will disable confirmation entirely"
            );
        }

        // Validate L3 timeout
        if self.l3_timeout_ms == 0 {
            return Err("l3_timeout_ms must be > 0".to_string());
        }
        if self.l3_timeout_ms > 60000 {
            warn!(
                timeout = self.l3_timeout_ms,
                "l3_timeout_ms > 60000ms may cause poor user experience"
            );
        }

        // Validate confirmation timeout
        if self.confirmation_timeout_ms == 0 {
            return Err("confirmation_timeout_ms must be > 0".to_string());
        }

        // Validate agent configuration
        self.agent.validate()?;

        Ok(())
    }
}

// =============================================================================
// AgentConfigToml - L3 Agent (Multi-step Planning) Configuration
// =============================================================================

/// Configuration for L3 Agent multi-step planning and execution
///
/// The L3 Agent enables intelligent multi-step task planning where the AI
/// decomposes complex requests into sequential tool invocations.
///
/// # Example TOML
///
/// ```toml
/// [dispatcher.agent]
/// enabled = true
/// max_steps = 10
/// step_timeout_ms = 30000
/// enable_rollback = true
/// plan_confirmation_required = true
/// allow_irreversible_without_confirmation = false
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AgentConfigToml {
    /// Whether agent mode is enabled (default: true)
    #[serde(default = "default_agent_enabled")]
    pub enabled: bool,

    /// Maximum number of steps allowed in a plan (default: 10)
    #[serde(default = "default_agent_max_steps")]
    pub max_steps: u32,

    /// Timeout for each step in milliseconds (default: 30000)
    #[serde(default = "default_agent_step_timeout")]
    pub step_timeout_ms: u64,

    /// Whether to attempt rollback on failure (default: true)
    #[serde(default = "default_agent_enable_rollback")]
    pub enable_rollback: bool,

    /// Whether to require user confirmation before executing plans (default: true)
    #[serde(default = "default_agent_plan_confirmation_required")]
    pub plan_confirmation_required: bool,

    /// Whether irreversible steps can run without additional confirmation (default: false)
    /// When false, plans with irreversible steps will show a warning.
    #[serde(default = "default_agent_allow_irreversible")]
    pub allow_irreversible_without_confirmation: bool,

    /// Heuristics threshold for triggering planning (default: 2)
    /// Number of action verbs/connectors needed to trigger multi-step planning
    #[serde(default = "default_agent_heuristics_threshold")]
    pub heuristics_threshold: u32,
}

pub const fn default_agent_enabled() -> bool {
    true
}

pub const fn default_agent_max_steps() -> u32 {
    10
}

pub const fn default_agent_step_timeout() -> u64 {
    30000 // 30 seconds per step
}

pub const fn default_agent_enable_rollback() -> bool {
    true
}

pub const fn default_agent_plan_confirmation_required() -> bool {
    true
}

pub const fn default_agent_allow_irreversible() -> bool {
    false
}

pub const fn default_agent_heuristics_threshold() -> u32 {
    2 // At least 2 action signals to trigger planning
}

impl Default for AgentConfigToml {
    fn default() -> Self {
        Self {
            enabled: default_agent_enabled(),
            max_steps: default_agent_max_steps(),
            step_timeout_ms: default_agent_step_timeout(),
            enable_rollback: default_agent_enable_rollback(),
            plan_confirmation_required: default_agent_plan_confirmation_required(),
            allow_irreversible_without_confirmation: default_agent_allow_irreversible(),
            heuristics_threshold: default_agent_heuristics_threshold(),
        }
    }
}

impl AgentConfigToml {
    /// Validate the agent configuration
    pub fn validate(&self) -> std::result::Result<(), String> {
        if self.max_steps == 0 {
            return Err("agent.max_steps must be > 0".to_string());
        }
        if self.max_steps > 50 {
            warn!(
                max_steps = self.max_steps,
                "agent.max_steps > 50 may cause excessive processing"
            );
        }

        if self.step_timeout_ms == 0 {
            return Err("agent.step_timeout_ms must be > 0".to_string());
        }
        if self.step_timeout_ms > 120000 {
            warn!(
                timeout = self.step_timeout_ms,
                "agent.step_timeout_ms > 120000ms may cause poor user experience"
            );
        }

        Ok(())
    }
}
