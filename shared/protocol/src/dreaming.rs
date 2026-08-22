//! The scheduling-status half of `dreaming.list_insights`: the answer to
//! *"why didn't dreaming run last night?"*
//!
//! # Why this lives in the protocol crate
//!
//! The run history structurally cannot answer that question — a cycle that
//! never started leaves no row — so the daemon publishes its entry gates
//! instead, read without moving any of them. That makes this an operator
//! diagnostic, and operator diagnostics get read from more than one place:
//! the Panel renders it in the memory settings pane, and `aleph memory
//! dreaming` prints it on a headless box where there is no Panel to open.
//!
//! Two faces means two chances to hand-copy a key set, and this repo has paid
//! for that shape repeatedly: `aleph providers list` rendered `type` /
//! `default` for a server that only ever sent `provider_type` / `is_default`,
//! and every row printed a dash. The rule that came out of it is that a wire
//! contract whose halves live in different crates either shares one type — so
//! a rename is a compile error on both sides — or grows a reconciliation test.
//! These are the shared type.
//!
//! Two properties follow, and both are load-bearing:
//!
//! * the server **constructs** its response from [`DreamSchedulingStatus`]
//!   rather than hand-writing the keys, so emitting a field no client knows
//!   about is not something a future edit can do quietly (serde silently drops
//!   unknown keys on the way in — parsing proves a superset, never equality);
//! * every field carries `#[serde(default)]`, because a client may be talking
//!   to an older core that predates it. A missing gate must render as "not
//!   reported", never as a hard parse failure that takes the whole response
//!   down with it.

use serde::{Deserialize, Serialize};

/// The daemon's scheduling preconditions, mirrored one field per entry gate in
/// `DreamDaemon::check_and_run`, so a surface rendering this describes the
/// same decision the daemon actually makes.
///
/// `None` at the call site (no daemon registered in the server process —
/// memory disabled) must render as "not running". Never as an error: an
/// install with dreaming off is healthy, not broken.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DaemonStatus {
    /// `[memory.dreaming] enabled`, as the running daemon holds it — not as
    /// the config file currently reads, which can differ until a restart.
    #[serde(default)]
    pub enabled: bool,
    /// Whether local time is currently inside the dream window.
    #[serde(default)]
    pub within_window: bool,
    /// The same probe `check_and_run` consults: `true` means a cycle starting
    /// now would be deferred (or, mid-pipeline, would yield at the next stage
    /// boundary).
    #[serde(default)]
    pub user_active: bool,
    /// Seconds since the last human message.
    ///
    /// The sensor behind this shipped for months with no producer at all, and
    /// the symptom was not a dormant feature: `idle_seconds` measured process
    /// uptime, so the yield check was dead after 15 minutes and inverted
    /// before it. Rendering this number beside the threshold is what makes
    /// that class of failure visible to a human instead of only to a test.
    #[serde(default)]
    pub idle_seconds: i64,
    /// How long the user must have been quiet before a cycle may start.
    #[serde(default)]
    pub idle_threshold_seconds: u32,
    /// Local-time window bounds, `HH:MM`, as configured.
    #[serde(default)]
    pub window_start_local: String,
    #[serde(default)]
    pub window_end_local: String,
    /// A cycle is executing right now.
    #[serde(default)]
    pub is_running: bool,
    /// Outer timeout on a cycle — also the bound that turns a stranded
    /// `running` row into [`DreamLastRun::status`] `"stale_running"`.
    #[serde(default)]
    pub max_duration_seconds: u32,
}

/// The last persisted run, whatever its outcome — the half of "did anything
/// happen last night" the insight list cannot answer, because a cycle that
/// errored, timed out or yielded files no insight row.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DreamLastRun {
    /// Unix seconds.
    #[serde(default)]
    pub run_at: i64,
    /// `success` | `cancelled` | `error` | `timeout` | `running` |
    /// `stale_running`.
    ///
    /// The last one is derived on read rather than stored: a `running` row
    /// older than the cycle's own hard timeout belongs to a process that died
    /// mid-cycle, and nothing that could correct the row is alive to do so.
    /// A client must not invent that derivation itself — it needs
    /// [`DaemonStatus::max_duration_seconds`] and the server already applied
    /// it.
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub duration_ms: Option<u64>,
}

/// The two keys `dreaming.list_insights` carries for scheduling status.
///
/// Flattened onto the response body rather than nested under a key of its own,
/// which is why it exists as a struct at all: it gives the *envelope* keys an
/// owner too. "The last hand-copied place" in this repo has more than once
/// turned out to be the wrapper around the rows rather than the rows.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DreamSchedulingStatus {
    /// `None` when no daemon runs in the server process.
    #[serde(default)]
    pub daemon: Option<DaemonStatus>,
    /// `None` when this install has never completed a cycle.
    #[serde(default)]
    pub last_run: Option<DreamLastRun>,
}

impl DaemonStatus {
    /// Why a cycle would not start right now, in the order `check_and_run`
    /// asks — or `None` when every gate is open.
    ///
    /// Shared so the Panel pane and the CLI cannot disagree about what the
    /// gates mean. Returning the *first* closed gate rather than a list is
    /// deliberate: the gates are sequential, so naming a later one alongside
    /// an earlier one would tell the operator to go change something that
    /// would not have mattered.
    #[must_use]
    pub fn blocking_gate(&self) -> Option<DreamGateBlock> {
        if !self.enabled {
            Some(DreamGateBlock::Disabled)
        } else if self.is_running {
            None
        } else if !self.within_window {
            Some(DreamGateBlock::OutsideWindow)
        } else if self.user_active {
            Some(DreamGateBlock::UserActive)
        } else {
            None
        }
    }
}

/// The first gate standing between the daemon and a cycle.
///
/// A named enum rather than a rendered string because the two client faces
/// word it differently — the Panel has a translation catalogue, the CLI does
/// not — and a shared *string* would force one of them to re-derive the
/// decision from the raw fields. Re-deriving is how the two surfaces of one
/// predicate drift apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DreamGateBlock {
    /// `[memory.dreaming] enabled = false` in the running daemon.
    Disabled,
    /// Local time is outside the configured window.
    OutsideWindow,
    /// Someone was typing more recently than the idle threshold allows.
    UserActive,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The gate ladder mirrors `check_and_run`'s order, and a running cycle is
    /// not "blocked" — it is the thing the gates exist to start.
    #[test]
    fn blocking_gate_follows_the_daemons_own_order() {
        let open = DaemonStatus {
            enabled: true,
            within_window: true,
            ..DaemonStatus::default()
        };
        assert_eq!(open.blocking_gate(), None);

        assert_eq!(
            DaemonStatus {
                enabled: false,
                within_window: true,
                ..open.clone()
            }
            .blocking_gate(),
            Some(DreamGateBlock::Disabled),
        );

        // Disabled outranks everything: telling an operator "outside the
        // window" when the daemon is off sends them to edit a time range that
        // changes nothing.
        assert_eq!(
            DaemonStatus {
                enabled: false,
                within_window: false,
                user_active: true,
                ..open.clone()
            }
            .blocking_gate(),
            Some(DreamGateBlock::Disabled),
        );

        assert_eq!(
            DaemonStatus {
                within_window: false,
                user_active: true,
                ..open.clone()
            }
            .blocking_gate(),
            Some(DreamGateBlock::OutsideWindow),
        );

        assert_eq!(
            DaemonStatus {
                user_active: true,
                ..open.clone()
            }
            .blocking_gate(),
            Some(DreamGateBlock::UserActive),
        );

        // Running: every gate already answered yes at start time, and
        // `user_active` now means "would yield at the next stage boundary",
        // not "was refused entry".
        assert_eq!(
            DaemonStatus {
                is_running: true,
                user_active: true,
                within_window: false,
                ..open
            }
            .blocking_gate(),
            None,
        );
    }

    /// An older core that predates a field must not take the whole response
    /// down — the operator asking why dreaming is quiet is the last person who
    /// should get a parse error instead of an answer.
    #[test]
    fn a_partial_payload_still_deserialises() {
        let status: DreamSchedulingStatus =
            serde_json::from_str(r#"{"daemon":{"enabled":true},"last_run":{"status":"error"}}"#)
                .expect("partial payload");
        let daemon = status.daemon.expect("daemon");
        assert!(daemon.enabled);
        assert_eq!(daemon.idle_threshold_seconds, 0);
        assert_eq!(status.last_run.expect("last_run").status, "error");

        let empty: DreamSchedulingStatus = serde_json::from_str("{}").expect("empty payload");
        assert!(empty.daemon.is_none());
        assert!(empty.last_run.is_none());
    }
}
