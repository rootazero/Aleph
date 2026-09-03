//! The one shape of a resume receipt.
//!
//! `agent.resume` (JSON-RPC), `POST /v1/admin/resume` and
//! `aleph-server resume --json` all report the same pass over the same
//! coordinator. Until this type existed they reported it in **three**
//! hand-written shapes: a `serde_json::json!` literal in the RPC handler, a
//! narrower `ResumeResponse` struct on the admin route that carried four of the
//! nine counters, and a `match` on `response.status.as_str()` in the CLI with an
//! `other =>` arm that printed any word it did not recognise as if it were a
//! sentence. The admin route is the one the CLI actually calls, so the counters
//! the RPC face rendered — `delegated`, `busy`, `refused`,
//! `skipped_unknown_age`, `contradictions` — could not reach an operator at all,
//! and a `--json` caller could not re-derive the status word from the body it
//! got (criterion #10: a wire contract with a shape on each side cancels out).
//!
//! Here the field set is the contract. The server constructs this type and
//! serialises it; a client deserialises the same type. A renamed field is a
//! compile error on both sides rather than a key that silently stops arriving.
//!
//! # Every key is always present
//!
//! No `skip_serializing_if`. A caller sees one object shape whatever the pass
//! did, and [`ResumeReceipt::status`] is the only field that says what happened
//! — the same ruling [`crate::session_thread::AgentRunStatusReport`] records for
//! its `error` key.

use serde::{Deserialize, Serialize};

/// One candidate the pass would not act on.
///
/// A list, not a counter: "something was refused" and "this session's log
/// contradicts itself at seq 41" are different answers, and the operator acts
/// on the second.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefusedEntry {
    /// The session that was refused.
    #[serde(default)]
    pub session_key: String,
    /// The stable word for the refusal kind (`log_inconsistent`,
    /// `agent_missing`, `boundary_repair_failed`, `retrigger_failed`).
    #[serde(default)]
    pub reason: String,
    /// What specifically was wrong, for an operator to act on.
    #[serde(default)]
    pub detail: String,
}

/// What a resume pass did, as every face reports it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumeReceipt {
    /// The one-word summary. **Required** — a server that renames it must fail
    /// loudly here rather than decay to a status this client invents.
    pub status: String,

    /// The session the caller named, echoed back so a batched caller can
    /// correlate. `None` on the boot scan's multi-session report.
    #[serde(default)]
    pub session_key: Option<String>,

    /// Sessions inspected that had at least one run marker.
    #[serde(default)]
    pub scanned: u32,
    /// Interrupted runs successfully re-triggered.
    #[serde(default)]
    pub resumed: u32,
    /// Runs marked `Abandoned` (too old, or the crash-loop cap was reached).
    #[serde(default)]
    pub abandoned: u32,
    /// Sessions skipped because their newest marker is a `RunFinished`.
    #[serde(default)]
    pub skipped: u32,
    /// Sessions left alone because a resume for them was already in flight.
    #[serde(default)]
    pub busy: u32,
    /// Sessions handed back to the scheduler that owns them (team dispatcher /
    /// cron / heartbeat) instead of being resumed here.
    #[serde(default)]
    pub delegated: u32,

    /// Every candidate this pass refused, and why.
    #[serde(default)]
    pub refused: Vec<RefusedEntry>,

    /// REPORT-kind log contradictions seen across every candidate reduced.
    /// A magnitude; the kinds themselves are named per session by the
    /// `core/session-log` doctor check.
    #[serde(default)]
    pub contradictions: u32,
    /// Resumed runs that had to give something up on the way back — a retired
    /// model replaced by its successor, a `project_root` that no longer exists.
    /// The model is told in-band; this is the operator's count of it.
    #[serde(default)]
    pub degraded: u32,
    /// Resumed runs whose `RunStarted` carried no settings envelope, so the
    /// re-triggered run follows today's session and global values rather than
    /// the ones the crashed run was executing under.
    #[serde(default)]
    pub unsnapshotted: u32,
    /// Interrupted candidates left alone because their log's recency could not
    /// be read. Deliberately neither `resumed` nor `abandoned`: both are
    /// decisions taken on an age, and the age is what the log does not support.
    #[serde(default)]
    pub skipped_unknown_age: u32,

    /// Why the pass could not run at all. `None` unless `status` says so.
    #[serde(default)]
    pub error: Option<String>,
    /// The agent whose `allowed_users` list refused the caller, when `status`
    /// is [`ResumeReceipt::AGENT_FORBIDDEN`].
    #[serde(default)]
    pub agent_id: Option<String>,
}

impl ResumeReceipt {
    /// The interrupted run was re-triggered.
    pub const RESUMED: &'static str = "resumed";
    /// The interrupted run was marked `Abandoned` and will not be retried.
    pub const ABANDONED: &'static str = "abandoned";
    /// The newest run marker is a `RunFinished` — nothing to resume.
    pub const ALREADY_FINISHED: &'static str = "already_finished";
    /// Scanned and interrupted, but the repair or the re-trigger failed.
    pub const NOT_RESUMED: &'static str = "not_resumed";
    /// The session has no run markers at all.
    pub const NO_RUNS: &'static str = "no_runs";
    /// Handed back to the scheduler that owns the session.
    pub const DELEGATED: &'static str = "delegated";
    /// A resume for this session was already in flight.
    pub const ALREADY_RESUMING: &'static str = "already_resuming";
    /// The reducer refused the session's log; the operator's next move is
    /// `aleph doctor`, not another resume.
    pub const LOG_INCONSISTENT: &'static str = "log_inconsistent";
    /// This server has no run executor wired.
    pub const UNAVAILABLE: &'static str = "unavailable";
    /// The coordinator was reached and its marker read failed.
    pub const FAILED: &'static str = "failed";
    /// No such session, or it is not visible to the caller.
    pub const NOT_FOUND: &'static str = "not_found";
    /// The `session_key` string is not a session key.
    pub const INVALID_SESSION_KEY: &'static str = "invalid_session_key";
    /// The caller is not on the agent's `allowed_users` list.
    pub const AGENT_FORBIDDEN: &'static str = "agent_forbidden";

    /// Every key this type puts on the wire, in declaration order.
    ///
    /// The list a server-side test compares the serialised object against, so
    /// "the field exists" and "the field reaches the caller" stay one fact. A
    /// new field that is not added here fails that test; a name here with no
    /// member fails it too.
    pub const WIRE_FIELDS: [&'static str; 15] = [
        "status",
        "session_key",
        "scanned",
        "resumed",
        "abandoned",
        "skipped",
        "busy",
        "delegated",
        "refused",
        "contradictions",
        "degraded",
        "unsnapshotted",
        "skipped_unknown_age",
        "error",
        "agent_id",
    ];

    /// Classify [`Self::status`].
    ///
    /// **The one reader of the word set.** A `== "resumed"` at a call site
    /// would be a second answer to "what happened", and it would be the one
    /// that got the unknown-word case wrong: a status this client has never
    /// heard of is [`ResumeStatus::Unrecognized`], never a success.
    #[must_use]
    pub fn outcome(&self) -> ResumeStatus {
        match self.status.as_str() {
            Self::RESUMED => ResumeStatus::Resumed,
            Self::ABANDONED => ResumeStatus::Abandoned,
            Self::ALREADY_FINISHED => ResumeStatus::AlreadyFinished,
            Self::NOT_RESUMED => ResumeStatus::NotResumed,
            Self::NO_RUNS => ResumeStatus::NoRuns,
            Self::DELEGATED => ResumeStatus::Delegated,
            Self::ALREADY_RESUMING => ResumeStatus::AlreadyResuming,
            Self::LOG_INCONSISTENT => ResumeStatus::LogInconsistent,
            Self::UNAVAILABLE => ResumeStatus::Unavailable,
            Self::FAILED => ResumeStatus::Failed,
            Self::NOT_FOUND => ResumeStatus::NotFound,
            Self::INVALID_SESSION_KEY => ResumeStatus::InvalidSessionKey,
            Self::AGENT_FORBIDDEN => ResumeStatus::AgentForbidden,
            _ => ResumeStatus::Unrecognized,
        }
    }
}

/// The closed set of resume outcomes, plus the arm for a word this build does
/// not know.
///
/// Clients match this **exhaustively** (the CLI does, deliberately without a
/// catch-all): a new outcome then has to be given a sentence at every face
/// before it compiles, which is the ratchet the `other =>` arm removed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResumeStatus {
    /// [`ResumeReceipt::RESUMED`].
    Resumed,
    /// [`ResumeReceipt::ABANDONED`].
    Abandoned,
    /// [`ResumeReceipt::ALREADY_FINISHED`].
    AlreadyFinished,
    /// [`ResumeReceipt::NOT_RESUMED`].
    NotResumed,
    /// [`ResumeReceipt::NO_RUNS`].
    NoRuns,
    /// [`ResumeReceipt::DELEGATED`].
    Delegated,
    /// [`ResumeReceipt::ALREADY_RESUMING`].
    AlreadyResuming,
    /// [`ResumeReceipt::LOG_INCONSISTENT`].
    LogInconsistent,
    /// [`ResumeReceipt::UNAVAILABLE`].
    Unavailable,
    /// [`ResumeReceipt::FAILED`].
    Failed,
    /// [`ResumeReceipt::NOT_FOUND`].
    NotFound,
    /// [`ResumeReceipt::INVALID_SESSION_KEY`].
    InvalidSessionKey,
    /// [`ResumeReceipt::AGENT_FORBIDDEN`].
    AgentForbidden,
    /// A word this build has never heard of. Never read as a success.
    Unrecognized,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_status(status: &str) -> ResumeReceipt {
        ResumeReceipt {
            status: status.to_string(),
            ..ResumeReceipt::default()
        }
    }

    #[test]
    fn every_constant_the_server_writes_has_a_distinct_reading() {
        for (word, expected) in [
            (ResumeReceipt::RESUMED, ResumeStatus::Resumed),
            (ResumeReceipt::ABANDONED, ResumeStatus::Abandoned),
            (
                ResumeReceipt::ALREADY_FINISHED,
                ResumeStatus::AlreadyFinished,
            ),
            (ResumeReceipt::NOT_RESUMED, ResumeStatus::NotResumed),
            (ResumeReceipt::NO_RUNS, ResumeStatus::NoRuns),
            (ResumeReceipt::DELEGATED, ResumeStatus::Delegated),
            (
                ResumeReceipt::ALREADY_RESUMING,
                ResumeStatus::AlreadyResuming,
            ),
            (
                ResumeReceipt::LOG_INCONSISTENT,
                ResumeStatus::LogInconsistent,
            ),
            (ResumeReceipt::UNAVAILABLE, ResumeStatus::Unavailable),
            (ResumeReceipt::FAILED, ResumeStatus::Failed),
            (ResumeReceipt::NOT_FOUND, ResumeStatus::NotFound),
            (
                ResumeReceipt::INVALID_SESSION_KEY,
                ResumeStatus::InvalidSessionKey,
            ),
            (ResumeReceipt::AGENT_FORBIDDEN, ResumeStatus::AgentForbidden),
        ] {
            assert_eq!(with_status(word).outcome(), expected, "{word}");
        }
    }

    /// The arm the `other =>` fallback got wrong: it printed the raw word as if
    /// it were a sentence the operator could act on.
    #[test]
    fn a_word_this_build_has_never_heard_of_is_not_a_success() {
        for unknown in ["", "RESUMED", "queued", "partially_resumed"] {
            assert_eq!(
                with_status(unknown).outcome(),
                ResumeStatus::Unrecognized,
                "{unknown:?} must not read as an outcome this build claims to know"
            );
        }
    }

    /// Every declared field reaches the wire, and nothing else does. The
    /// destructure is exhaustive (no `..`), so a new member is a compile error
    /// here rather than a key that silently never arrives.
    #[test]
    fn the_wire_field_list_is_the_struct() {
        let ResumeReceipt {
            status: _,
            session_key: _,
            scanned: _,
            resumed: _,
            abandoned: _,
            skipped: _,
            busy: _,
            delegated: _,
            refused: _,
            contradictions: _,
            degraded: _,
            unsnapshotted: _,
            skipped_unknown_age: _,
            error: _,
            agent_id: _,
        } = ResumeReceipt::default();

        let v = serde_json::to_value(ResumeReceipt::default()).expect("serialize");
        let obj = v.as_object().expect("object");
        for field in ResumeReceipt::WIRE_FIELDS {
            assert!(obj.contains_key(field), "{field} missing from {obj:?}");
        }
        assert_eq!(
            obj.len(),
            ResumeReceipt::WIRE_FIELDS.len(),
            "a key on the wire is not in WIRE_FIELDS: {obj:?}"
        );
    }

    /// A `None` option is a present `null`, not an absent key: a caller reading
    /// `error` must not have to distinguish "no error" from "old server".
    #[test]
    fn an_absent_option_is_still_a_key() {
        let v = serde_json::to_value(ResumeReceipt::default()).expect("serialize");
        assert!(v["error"].is_null());
        assert!(v["agent_id"].is_null());
        assert!(v["session_key"].is_null());
    }

    /// A body from a server that predates half these counters still parses,
    /// and the counters it did not send read as zero rather than failing the
    /// whole receipt.
    #[test]
    fn an_older_body_parses_with_zero_counters() {
        let r: ResumeReceipt = serde_json::from_value(serde_json::json!({
            "status": "resumed",
            "session_key": "agent:main:main",
            "scanned": 1,
            "resumed": 1,
        }))
        .expect("parse");
        assert_eq!(r.outcome(), ResumeStatus::Resumed);
        assert_eq!(r.resumed, 1);
        assert_eq!(r.degraded, 0);
        assert!(r.refused.is_empty());
    }

    /// `status` carries no default: a rename on the server has to fail here,
    /// not read as an outcome this client then renders as a sentence.
    #[test]
    fn a_renamed_status_fails_rather_than_reading_as_unknown() {
        assert!(
            serde_json::from_value::<ResumeReceipt>(serde_json::json!({ "outcome": "resumed" }))
                .is_err()
        );
    }

    #[test]
    fn a_refusal_round_trips_with_its_reason_and_detail() {
        let receipt = ResumeReceipt {
            status: ResumeReceipt::LOG_INCONSISTENT.to_string(),
            refused: vec![RefusedEntry {
                session_key: "agent:main:main".into(),
                reason: "log_inconsistent".into(),
                detail: "events are out of order at seq 41".into(),
            }],
            ..ResumeReceipt::default()
        };
        let back: ResumeReceipt =
            serde_json::from_value(serde_json::to_value(&receipt).unwrap()).unwrap();
        assert_eq!(back, receipt);
        assert_eq!(back.outcome(), ResumeStatus::LogInconsistent);
    }
}
