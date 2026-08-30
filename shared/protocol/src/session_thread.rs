//! Thread-continuity contract — the request a thin client sends to `agent.run`
//! and the settings snapshot `chat.history` hands back when it re-attaches.
//!
//! # Why these types live here
//!
//! Same argument as [`crate::workspace`], and the same defect found a second
//! time. `aleph-tui` deliberately **cannot** depend on `alephcore` (its
//! `Cargo.toml` says so in capitals), so the `agent.run` wire shape was written
//! twice: once as `AgentRunParams` in the handler, once as a
//! `serde_json::json!` literal in `interfaces/tui/src/tui/commands.rs`.
//!
//! They disagreed. The handler's `input` is required — no `#[serde(default)]`,
//! no alias — and the TUI sent `message` (the key `chat.send` takes). Every
//! message ever typed into `aleph-tui` came back `INVALID_PARAMS`, through the
//! one call site whose own doc comment promised "the request shape can never
//! drift across the four paths": the shape was wrong at that single point, so
//! all four paths were wrong identically, and nothing went red because the
//! literal had nobody to disagree with.
//!
//! Sharing one type makes a rename a **compile** error on the client and a
//! **red test** on the server ([`AgentRunRequest`]'s reconciliation test lives
//! in `alephcore`, the crate that depends on both sides — the only place an
//! assertion can compare the two rather than compare a literal to itself).
//!
//! # What a thread carries
//!
//! [`SessionSnapshot`] is the other half. A conversation's durable settings —
//! usage mode, exec tier, thinking depth, model pin, cumulative tokens, memory
//! mode — have always been persisted server-side (`identity_meta.custom` and
//! the `sessions` row). No surface handed them to a client that re-attached by
//! key, so a client that reopened a thread showed the *global* defaults while
//! the server kept enforcing the *session's* values. The snapshot rides on
//! `chat.history` for the reason that response's own doc gives for carrying
//! `active_run` and `plan`: they are one snapshot, and any second call opens a
//! window where a client holds the transcript but not the settings that govern
//! it.

use serde::{Deserialize, Serialize};

/// The two numbers `chat.history` reports about the slice it just served.
///
/// `messages` is the TRAILING `limit` rows, so at exactly `limit` rows a full
/// page and a complete short conversation are byte-for-byte identical: nothing
/// in the page itself can say whether a beginning was cut off. `count` alone
/// therefore cannot answer "is this all of it", and every client that tried
/// answered it by guessing from the length it received — a client inventing an
/// answer the server never gave.
///
/// Shared for the reason the module header gives: `interfaces/cli` and
/// `interfaces/tui` cannot depend on `alephcore`, so each one hand-wrote these
/// keys. `aleph chat history --limit 20` read `count` and printed it under the
/// word **Total**, so a 102-row conversation reported "Total: 20 messages" —
/// not a missing value that reads as missing, but a wrong one that reads as
/// fact. A rename on the server is now a deserialization failure here rather
/// than a column that quietly starts describing something else.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryWindow {
    /// Rows in THIS response. Required: if the server ever renames it, callers
    /// should fail loudly rather than fall back to zero and render an empty
    /// transcript as an empty conversation.
    pub count: usize,

    /// Rows in the whole session, or `None` when the server gave no answer — a
    /// core older than the field, or a count that could not be read.
    ///
    /// `None` must never be rounded to zero or to `count`. Both of those say
    /// "the transcript is complete", which is the single answer that is wrong
    /// in the direction that HIDES content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<usize>,
}

impl HistoryWindow {
    /// Rows above this window, when the server said how many there are.
    ///
    /// `saturating_sub` because `total` and the window are two reads of a store
    /// a live run may be appending to. The handler counts FIRST precisely so
    /// the skew falls on this side: a row that lands in between makes `count`
    /// the larger, and "nothing above" is the honest reading of that. The
    /// other order over-reports, which renders as a "load earlier" control over
    /// a conversation that has nothing earlier.
    #[must_use]
    pub fn above(&self) -> Option<usize> {
        self.total.map(|t| t.saturating_sub(self.count))
    }

    /// Whether this response carries the whole conversation. `None` when the
    /// server did not say — which is not `Some(true)`.
    #[must_use]
    pub fn is_complete(&self) -> Option<bool> {
        self.above().map(|above| above == 0)
    }
}

/// Parameters for `agent.run` as a thin client sends them.
///
/// A deliberate **subset** of the server's `AgentRunParams`: exactly the fields
/// a terminal / CLI client can supply. Panel-only fields (attachments,
/// `model_override`, `project_root`, `peer_id`) are absent because no client
/// that shares this type sends them, and a field here with no sender is the
/// same defect in the other direction.
///
/// Every field but `input` is skipped when absent, so the emitted object is
/// byte-identical to the hand-written literal it replaces for the common case.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRunRequest {
    /// The user's message. **Required** — this is the field the TUI got wrong.
    pub input: String,

    /// Canonical session key (`agent:{id}:main:s{n}`, …). Omit to let the
    /// gateway route one.
    ///
    /// Note what happens when this is present but unparseable: `AgentRouter::route`
    /// falls through to its own routing and mints a **new epoch**, silently. A
    /// client that invents its own key format therefore gets a fresh
    /// conversation on every launch and addresses a row no other RPC can find.
    /// Clients must echo back what the server returned (`session_key` in the
    /// `agent.run` result), never a key of their own construction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_key: Option<String>,

    /// Channel identifier (`"cli:term1"`, `"gui:window1"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,

    /// Explicit target agent, bypassing channel-binding resolution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,

    /// Reasoning depth for this turn (`"off"`…`"xhigh"` and the documented
    /// aliases). A value here is also stamped onto the session, so it outlives
    /// the turn that carried it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,

    /// Usage mode for this turn (`"chat"` / `"work"` / `"code"`). Same
    /// stamped-onto-the-session contract as `thinking`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,

    /// Execution tier for this turn (`"ask"` / `"auto"` / `"full"`). Carried on
    /// the message because a brand-new conversation has no session row to write
    /// it to yet — the first turn is the one a tier is most needed for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exec_tier: Option<String>,
}

impl AgentRunRequest {
    /// A run request carrying only text, on an existing session.
    #[must_use]
    pub fn new(session_key: impl Into<String>, input: impl Into<String>) -> Self {
        Self {
            input: input.into(),
            session_key: Some(session_key.into()),
            ..Self::default()
        }
    }
}

/// The result of `agent.run` / `chat.send`, as a thin client reads it.
///
/// `session_key` is the field that matters for thread continuity: it is the
/// **canonical** key the gateway routed to, which is not necessarily the one
/// the client asked for. A client that keeps its own key instead of adopting
/// this one is addressing a session that may not exist.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRunAccepted {
    /// Id of the accepted run.
    #[serde(default)]
    pub run_id: String,
    /// Canonical session key this run was routed to.
    #[serde(default)]
    pub session_key: String,
    /// RFC 3339 acceptance timestamp.
    #[serde(default)]
    pub accepted_at: String,
}

/// Parameters for `agent.status` as a thin client sends them.
///
/// A run id is the only handle some clients have on a run. The `/btw` overlay
/// is the case that forced this type into the shared crate: a side question
/// executes on a *derived* session whose key is hashed server-side (see
/// [`crate::btw`]), so the asking client can never address that conversation —
/// it holds the run id its own `agent.run` handed back, and nothing else.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRunStatusRequest {
    /// The run to ask about.
    pub run_id: String,
}

/// What `agent.status` reports about one run.
///
/// # Absence is not an answer
///
/// The manager's table is in-process. A run it has no record of is reported as
/// an error, and that error means exactly one thing — *this* gateway process
/// has never heard of this id — which covers both "you asked about someone
/// else's run" (deliberately indistinguishable, see `caller_may_address_run`)
/// and "the process that was running it is gone". Neither is a verdict about
/// the work, and a client must not render either as one.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRunStatusReport {
    /// The run asked about, echoed back.
    #[serde(default)]
    pub run_id: String,
    /// The session the run was routed to. For a `/btw` run this is the **main**
    /// conversation, not the derived side session: the redirect happens inside
    /// `execute()`, long after the run was registered under the routed key.
    #[serde(default)]
    pub session_key: String,
    /// One of the four words in [`AgentRunStatusReport`]'s constants. Read it
    /// through [`Self::phase`] rather than comparing strings at the call site —
    /// a word this client has never heard of is a real possibility (an older
    /// client against a newer gateway) and has exactly one safe reading.
    #[serde(default)]
    pub status: String,
    /// How long the run has been alive, measured by the server at the instant
    /// it answered.
    #[serde(default)]
    pub elapsed_ms: u64,
    /// Why it failed, when it did.
    ///
    /// Not skipped when absent: [`Self::status`] already says whether there is
    /// a failure, so the key is always present and a client sees one shape.
    /// This field exists because the wire used to drop it — `RunStatus::Failed`
    /// carries its reason server-side and the response flattened all four
    /// states to a bare word, so a client told "failed" had nothing to show the
    /// user but the word.
    #[serde(default)]
    pub error: Option<String>,
}

/// What a run's reported [`AgentRunStatusReport::status`] means, with the one
/// arm that matters: whether it is still being worked on.
///
/// [`Self::Unrecognized`] is deliberately **not** grouped with
/// [`Self::Running`]. A client that reads an unknown word as "still going"
/// waits forever on a run that ended; one that reads it as "over" stops waiting
/// on a run that may still be going, and the work itself is unaffected either
/// way. Only the first of those is unrecoverable, so unknown means "not
/// running".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunPhase {
    /// Still being worked on.
    Running,
    /// Finished on its own.
    Completed,
    /// Ended in an error; [`AgentRunStatusReport::error`] carries it.
    Failed,
    /// Stopped by somebody — this client, another client, or a shutdown.
    Cancelled,
    /// A word this client has no reading for. See the type doc for why this is
    /// not `Running`.
    Unrecognized,
}

impl AgentRunStatusReport {
    /// The word for a run still being worked on.
    pub const RUNNING: &'static str = "running";
    /// The word for a run that finished on its own.
    pub const COMPLETED: &'static str = "completed";
    /// The word for a run that ended in an error.
    pub const FAILED: &'static str = "failed";
    /// The word for a run somebody stopped.
    pub const CANCELLED: &'static str = "cancelled";

    /// Classify [`Self::status`].
    ///
    /// **The one reader of the word set.** The server writes these four
    /// constants and this function reads them; a `== "running"` written at a
    /// call site would be a second answer to "is it still going", and it would
    /// be the one that got the unknown-word case wrong.
    #[must_use]
    pub fn phase(&self) -> RunPhase {
        match self.status.as_str() {
            Self::RUNNING => RunPhase::Running,
            Self::COMPLETED => RunPhase::Completed,
            Self::FAILED => RunPhase::Failed,
            Self::CANCELLED => RunPhase::Cancelled,
            _ => RunPhase::Unrecognized,
        }
    }
}

/// A conversation's durable settings, as `chat.history` reports them on
/// re-attach.
///
/// Every field is read back from something the server already persists — this
/// type creates no storage, it connects storage that had no reader:
///
/// | field | where it lives |
/// |---|---|
/// | `mode` / `exec_tier` / `think_level` / `memory_mode` / `model_pin` | `sessions.metadata` → `identity_meta.custom[…]` |
/// | `model` / `model_provider` | `sessions.model` / `sessions.model_provider`, written per run by `session_projector` |
/// | `input_tokens` / `output_tokens` / `total_tokens` / `estimated_cost_usd` | the `sessions` row's usage columns |
/// | `project_root` | `identity_meta.custom["project_root"]` |
///
/// `None` on a knob means **follow the global default**, not "off" — the same
/// three-valued contract the pickers use. A client must render an unset knob as
/// "following global", never as a concrete value it picked itself.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionSnapshot {
    /// Canonical session key this snapshot describes.
    pub session_key: String,

    /// Agent this conversation is bound to.
    #[serde(default)]
    pub agent_id: String,

    /// Per-session usage mode override (`"chat"` / `"work"` / `"code"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,

    /// Per-session execution tier override (`"ask"` / `"auto"` / `"full"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exec_tier: Option<String>,

    /// Per-session reasoning depth override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub think_level: Option<String>,

    /// Per-session memory mode (`"on"` / `"off"`). `None` follows `[memory] enabled`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_mode: Option<String>,

    /// The model the conversation is **pinned** to (`select_model`), if any.
    ///
    /// Distinct from [`Self::model`]: a pin is a forward-looking instruction,
    /// while `model` records what last served. They disagree for exactly one
    /// turn after a pick — the pick applies from the next run — and a client
    /// that shows only `model` reports the model the user just switched away
    /// from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_pin: Option<String>,

    /// Provider pinned alongside [`Self::model_pin`], if the pick named one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_pin_provider: Option<String>,

    /// The model that last served a run on this session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// The provider that last served a run on this session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_provider: Option<String>,

    /// Cumulative prompt tokens billed to this conversation.
    #[serde(default)]
    pub input_tokens: i64,

    /// Cumulative completion tokens billed to this conversation.
    #[serde(default)]
    pub output_tokens: i64,

    /// `input_tokens + output_tokens`, as the session row records it.
    #[serde(default)]
    pub total_tokens: i64,

    /// Priced cost of everything billed to this conversation, in USD.
    #[serde(default)]
    pub estimated_cost_usd: f64,

    /// Messages recorded on this conversation.
    #[serde(default)]
    pub message_count: i64,

    /// How many times this conversation has been compacted.
    #[serde(default)]
    pub compaction_count: i64,

    /// User-chosen working directory, if the conversation was scoped to one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_root: Option<String>,

    /// User-facing label, if set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl SessionSnapshot {
    /// The model a client should *display* for this conversation: the pin when
    /// one is armed, else whatever last served.
    ///
    /// One derivation, because the two facts disagree for a turn and every
    /// surface that re-derives the rule will eventually pick a different one.
    #[must_use]
    pub fn effective_model(&self) -> Option<&str> {
        self.model_pin
            .as_deref()
            .or(self.model.as_deref())
            .filter(|m| !m.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_run_request_emits_input_not_message() {
        let req = AgentRunRequest::new("agent:main:main", "hello");
        let v = serde_json::to_value(&req).expect("serialize");
        assert_eq!(v["input"], "hello");
        assert_eq!(v["session_key"], "agent:main:main");
        // The bug this type exists to make impossible.
        assert!(
            v.get("message").is_none(),
            "`message` is `chat.send`'s key; `agent.run` takes `input`"
        );
    }

    #[test]
    fn absent_options_are_not_emitted() {
        let req = AgentRunRequest::new("k", "hi");
        let v = serde_json::to_value(&req).expect("serialize");
        let obj = v.as_object().expect("object");
        // Byte-for-byte the shape the hand-written literal produced, minus the
        // wrong key: adding optional fields must not change what an existing
        // client sends.
        assert_eq!(obj.len(), 2, "unexpected keys: {obj:?}");
    }

    fn reported(status: &str) -> AgentRunStatusReport {
        AgentRunStatusReport {
            status: status.to_string(),
            ..AgentRunStatusReport::default()
        }
    }

    #[test]
    fn each_word_the_server_writes_has_a_distinct_reading() {
        assert_eq!(
            reported(AgentRunStatusReport::RUNNING).phase(),
            RunPhase::Running
        );
        assert_eq!(
            reported(AgentRunStatusReport::COMPLETED).phase(),
            RunPhase::Completed
        );
        assert_eq!(
            reported(AgentRunStatusReport::FAILED).phase(),
            RunPhase::Failed
        );
        assert_eq!(
            reported(AgentRunStatusReport::CANCELLED).phase(),
            RunPhase::Cancelled
        );
    }

    /// The one arm a client cannot afford to get wrong. An unknown word read as
    /// `Running` is a spinner that never stops on a run that already ended —
    /// unrecoverable from the client, unlike the other direction.
    #[test]
    fn a_word_this_client_has_never_heard_of_is_not_running() {
        for unknown in ["", "paused", "queued", "RUNNING"] {
            assert_eq!(
                reported(unknown).phase(),
                RunPhase::Unrecognized,
                "{unknown:?} must not read as a state this client claims to know"
            );
        }
    }

    /// A missing `error` is a present key, so a client sees one shape whatever
    /// the run did. The `status` word is what says whether there is a failure.
    #[test]
    fn the_status_report_always_carries_every_key() {
        let v = serde_json::to_value(AgentRunStatusReport::default()).expect("serialize");
        let obj = v.as_object().expect("object");
        for key in ["run_id", "session_key", "status", "elapsed_ms", "error"] {
            assert!(obj.contains_key(key), "{key} missing from {obj:?}");
        }
    }

    #[test]
    fn effective_model_prefers_the_pin() {
        let mut snap = SessionSnapshot {
            model: Some("gpt-5".into()),
            ..SessionSnapshot::default()
        };
        assert_eq!(snap.effective_model(), Some("gpt-5"));
        snap.model_pin = Some("claude-opus-5".into());
        assert_eq!(
            snap.effective_model(),
            Some("claude-opus-5"),
            "a pick the user just made must win over the model it replaced"
        );
    }

    #[test]
    fn effective_model_ignores_an_empty_string() {
        // `sessions.model` is written from a run's report; an empty string
        // there is absence, not a model named "".
        let snap = SessionSnapshot {
            model: Some(String::new()),
            ..SessionSnapshot::default()
        };
        assert_eq!(snap.effective_model(), None);
    }

    #[test]
    fn an_unset_knob_round_trips_as_absent() {
        let snap = SessionSnapshot {
            session_key: "agent:main:main".into(),
            ..SessionSnapshot::default()
        };
        let v = serde_json::to_value(&snap).expect("serialize");
        for knob in ["mode", "exec_tier", "think_level", "memory_mode"] {
            assert!(
                v.get(knob).is_none(),
                "{knob} must be absent, not null: absent means \"follow global\""
            );
        }
        let back: SessionSnapshot = serde_json::from_value(v).expect("round trip");
        assert_eq!(back, snap);
    }
}

#[cfg(test)]
mod history_window_tests {
    use super::HistoryWindow;

    #[test]
    fn a_window_smaller_than_the_session_reports_what_is_above() {
        let w: HistoryWindow = serde_json::from_value(serde_json::json!({
            "count": 20, "total": 102
        }))
        .unwrap();
        assert_eq!(w.above(), Some(82));
        assert_eq!(w.is_complete(), Some(false));
    }

    /// The shape the guess gets wrong: a page that is full AND complete.
    #[test]
    fn a_full_page_that_is_the_whole_session_is_complete() {
        let w = HistoryWindow { count: 200, total: Some(200) };
        assert_eq!(w.above(), Some(0));
        assert_eq!(w.is_complete(), Some(true));
    }

    /// No answer is not "nothing above". Absent, `null`, and a shape change all
    /// have to land on `None`, because the caller's fallback for `None` differs
    /// from its behaviour for `Some(0)`.
    #[test]
    fn a_missing_total_is_unknown_not_zero() {
        for payload in [
            serde_json::json!({ "count": 20 }),
            serde_json::json!({ "count": 20, "total": null }),
        ] {
            let w: HistoryWindow = serde_json::from_value(payload).unwrap();
            assert_eq!(w.total, None);
            assert_eq!(w.above(), None);
            assert_eq!(w.is_complete(), None);
        }
        assert!(
            serde_json::from_value::<HistoryWindow>(serde_json::json!({ "count": 20, "total": "many" }))
                .is_err(),
            "a shape change must fail loudly here rather than decay to a number"
        );
    }

    /// `count` carries no default: a server-side rename has to fail, not read
    /// as an empty transcript.
    #[test]
    fn a_renamed_count_fails_rather_than_reading_as_empty() {
        assert!(serde_json::from_value::<HistoryWindow>(serde_json::json!({ "total": 5 })).is_err());
    }

    /// Two reads of a growing store: the window can come back LONGER than the
    /// count taken just before it. Saturating, not wrapping.
    #[test]
    fn a_window_longer_than_the_count_reads_as_nothing_above() {
        let w = HistoryWindow { count: 5, total: Some(4) };
        assert_eq!(w.above(), Some(0));
        assert_eq!(w.is_complete(), Some(true));
    }
}
