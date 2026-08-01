//! Team dispatcher / broadcast / message-router configuration types.
//!
//! Each `Option` field falls back to the live runtime struct's `Default`
//! at the boot site, so an unconfigured deployment is byte-identical to
//! prior behaviour and the authoritative defaults never drift (they are
//! read from the runtime struct, not duplicated here).
//!
//! Live surface:
//! - `TeamDispatcherConfigToml` — `[team_dispatcher]`
//! - `TeamBroadcastConfigToml` — `[team_broadcast]`
//! - `TeamMessagesConfigToml` — `[team_messages]`

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// =============================================================================
// TeamDispatcherConfigToml — `[team_dispatcher]`
// =============================================================================

/// Operator tunables for the **team** `TeamDispatcher` loop (multi-agent
/// coordinated-task scheduling).
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
