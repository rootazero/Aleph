//! `gateway.metrics.run_concurrency`, as one shape.
//!
//! The server built this response as a `serde_json::json!` envelope and the
//! Panel parsed it into a hand-written mirror in
//! `interfaces/webchat/src/api/system.rs`. Two shapes for one wire contract:
//! the mirror can only ever prove it is a *superset* reader of what the
//! literal happens to emit, so a renamed key on the server degrades to a
//! `#[serde(default)]` zero on the client — a saturated engine rendering as an
//! idle one (criterion #10).
//!
//! The envelope is now a type. The server constructs it, the Panel
//! deserialises it, and a rename is a compile error on both sides.

use serde::{Deserialize, Serialize};

/// The whole `gateway.metrics.run_concurrency` result.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunConcurrencyMetrics {
    /// Run-slot occupancy across the server.
    #[serde(default)]
    pub run_concurrency: RunConcurrency,
    /// Session keys with a run in flight, filtered to what the caller may see.
    ///
    /// The cold-load seed for the very dots `stream.running_set_changed` keeps
    /// live — the two move together or a member's sidebar leaks or lies.
    #[serde(default)]
    pub running_sessions: Vec<String>,
    /// Messages parked in the per-session busy wait lanes — the backlog
    /// *behind* the run slots.
    #[serde(default)]
    pub busy_queue: BusyQueueMetrics,
}

/// Run-slot occupancy, as the concurrency limiter reports it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunConcurrency {
    /// Slots currently held, server-wide.
    #[serde(default)]
    pub global_in_use: usize,
    /// Total slots configured, server-wide.
    #[serde(default)]
    pub global_total: usize,
    /// The per-agent sub-cap: the most concurrent runs one agent may hold.
    #[serde(default)]
    pub per_agent_cap: usize,
    /// Runs blocked waiting for a slot — the depth behind the semaphores.
    #[serde(default)]
    pub waiting: usize,
    /// Per-agent in-use counts, for agents holding at least one run.
    ///
    /// **Withheld from a member** — it names agent personas, which are
    /// server-global configuration, so the question is privilege rather than
    /// ownership. Absent means "not yours to see", never "no agent is busy":
    /// the empty list and the withheld field are the same bytes, which is why
    /// the numbers a member DOES get (`global_in_use` / `global_total`) are
    /// the ones that answer "how busy is the server".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub per_agent: Vec<AgentSlotUsage>,
}

/// One agent's live run-slot usage.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSlotUsage {
    /// The agent id — the memory/storage isolation boundary.
    #[serde(default)]
    pub agent_id: String,
    /// Slots this agent currently holds.
    #[serde(default)]
    pub in_use: usize,
}

/// The queue behind the run slots.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BusyQueueMetrics {
    /// Messages waiting across every lane. Process-wide for every caller: it
    /// is load, not whose work it is.
    #[serde(default)]
    pub total_waiting: usize,
    /// Per-session backlog, deepest lane first, filtered to what the caller
    /// may see — the same narrowing
    /// [`RunConcurrencyMetrics::running_sessions`] gets, because these keys
    /// name sessions too.
    #[serde(default)]
    pub per_session: Vec<SessionQueueDepth>,
}

/// One session's queued-message backlog.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionQueueDepth {
    /// The session whose lane this is.
    #[serde(default)]
    pub session_key: String,
    /// Messages parked in it.
    #[serde(default)]
    pub depth: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A response from a server predating `busy_queue` still parses, and the
    /// missing backlog reads as zero rather than failing the whole gauge.
    #[test]
    fn an_older_response_parses_without_the_queue() {
        let m: RunConcurrencyMetrics = serde_json::from_value(serde_json::json!({
            "run_concurrency": { "global_in_use": 2, "global_total": 8 },
            "running_sessions": ["agent:main:main"],
        }))
        .expect("parse");
        assert_eq!(m.run_concurrency.global_in_use, 2);
        assert_eq!(m.busy_queue.total_waiting, 0);
        assert!(m.busy_queue.per_session.is_empty());
    }

    /// The privilege drop is an absent key, and a client must read it as
    /// "withheld", which is what an empty list already means here.
    #[test]
    fn a_withheld_per_agent_is_an_absent_key() {
        let v = serde_json::to_value(RunConcurrencyMetrics::default()).expect("serialize");
        assert!(v["run_concurrency"].get("per_agent").is_none());
        let with = RunConcurrencyMetrics {
            run_concurrency: RunConcurrency {
                per_agent: vec![AgentSlotUsage {
                    agent_id: "main".into(),
                    in_use: 1,
                }],
                ..RunConcurrency::default()
            },
            ..RunConcurrencyMetrics::default()
        };
        let v = serde_json::to_value(&with).expect("serialize");
        assert_eq!(v["run_concurrency"]["per_agent"][0]["agent_id"], "main");
    }

    #[test]
    fn the_envelope_round_trips() {
        let m = RunConcurrencyMetrics {
            run_concurrency: RunConcurrency {
                global_in_use: 3,
                global_total: 8,
                per_agent_cap: 2,
                waiting: 1,
                per_agent: vec![AgentSlotUsage {
                    agent_id: "ops".into(),
                    in_use: 2,
                }],
            },
            running_sessions: vec!["agent:ops:main".into()],
            busy_queue: BusyQueueMetrics {
                total_waiting: 4,
                per_session: vec![SessionQueueDepth {
                    session_key: "agent:ops:main".into(),
                    depth: 4,
                }],
            },
        };
        let back: RunConcurrencyMetrics =
            serde_json::from_value(serde_json::to_value(&m).unwrap()).unwrap();
        assert_eq!(back, m);
    }
}
