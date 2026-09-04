//! `TerminalTool` — read-only view of the terminal sessions the caller owns
//! on this server (herdr runtime port, phase 1, Task 11).
//!
//! Five actions, no write verb: `list` (which PTY sessions exist), `status`
//! (each session's detected agent state, the same table
//! `runtime.agents.list` serves), `read` (one session's current visible
//! screen, no scrollback), `wait` (block until one session's state enters a
//! requested set) and `explain` (which manifest rule decided a state, over
//! which inputs). A human types into a terminal; the model only looks.
//!
//! `wait` is a read that takes time, not a write: it observes the agent
//! table's change watch (`gateway::runtime::RuntimeAgents::subscribe`) and
//! returns a row. It never polls the screen and never touches the session.
//!
//! # Same disclosure as `pty.*` / `runtime.*`, so it is gated the same way
//!
//! This tool is a THIRD lens over the exact data `pty.*` and `runtime.*`
//! already gate operator-only on both their RPC and event faces (see
//! `gateway::handlers::pty`'s and `gateway::handlers::runtime`'s module
//! docs). A session id, its cwd, and its live screen contents are not less
//! sensitive because an LLM is asking instead of a JSON-RPC client — so
//! `terminal` is listed in [`method_authz::OPERATOR_TOOLS`]
//! (`crate::gateway::method_authz`), which walls it from chat-tier channels
//! and members at the tool-dispatch gate, AND checked again inline in
//! [`TerminalTool::call`].
//!
//! Unlike `select_model`'s `moa:` arm and `loop_manage`'s cross-session arm —
//! which gate their one sensitive action inline INSTEAD OF listing the whole
//! tool in `OPERATOR_TOOLS` — `terminal` gates the WHOLE tool both ways, and
//! the two halves do not agree on every path today. Say that out loud rather
//! than leaving the next reader to derive it:
//!
//! - On the `ScopedToolService` path (`tools/scoped/dispatch.rs`), a
//!   chat-tier or member caller trips `OPERATOR_TOOLS` membership and
//!   `check_operator_gate` raises a live operator-approval card. **Even when
//!   a human approves that card, this tool refuses anyway** — approval only
//!   flips `authorized` for the dispatch pipeline; nothing re-stamps
//!   [`crate::tools::turn_context::TurnContext`], so [`caller_is_operator`]
//!   below reads the same unchanged `caller_role` and answers `false`
//!   regardless.
//! - On the `tools.invoke` path (`handlers/tools_invoke.rs`), no
//!   `TurnContext` is ever set, so [`caller_is_operator`] returns `true`
//!   unconditionally and contributes nothing there; `OPERATOR_TOOLS`
//!   membership (checked directly against `caller_role` by that handler,
//!   not through this tool) is the only thing closing that path.
//!
//! **The `ScopedToolService` outcome above is a KNOWN GAP, not the intended
//! design — do not read it as correct and build on it.** An operator who
//! was interrupted, looked at the call, and said yes is overridden by a
//! check that never learns their answer. The decided fix (R11-14,
//! 2026-09-03) is a SEAM: `check_operator_gate` already computes
//! `approved_by_operator_gate` for exactly this call; carrying that verdict
//! through to this tool — so an operator-approved member call is let
//! through instead of refused a second time — is the direction, but that
//! seam does not exist yet. Building it is out of scope for this task and
//! deliberately unscheduled; this refusal stands until it lands.
//!
//! **Do not "fix" the gap above by deleting the inline check** — that is
//! NOT the decided fix, and it makes things worse: `gate_chain.rs`'s
//! approval card currently reads "…which changes Aleph's own
//! configuration. Approve to allow this change", which is false for a
//! read-only tool. Dropping the inline check would make that mislabeled
//! card actually grant a read of another principal's live terminal screen
//! — right now, before the seam above exists to make that grant correct.
//! (The card's text is shared by every operator tool and is a separate
//! review's problem, not this one — see the `OPERATOR_TOOLS` entry's own
//! comment in `method_authz.rs`; leave `gate_chain.rs` alone here.) This is
//! also exactly how `plugin_manage` once shipped ungated on one face while
//! its RPC twin stayed closed (see `method_authz.rs`'s own module doc) —
//! removing either half without the seam above reopens that failure mode.
//!
//! Absent `caller_role` reads as operator (`role_is_operator`, "no identity
//! was resolved" — internal wiring, cron, a test — not "a stranger"), the
//! same convention every other inline gate in this crate follows.
//!
//! # Ownership filtering
//!
//! Every action is scoped to the caller's own sessions through ONE predicate
//! in this file — [`terminal_admits`], reached directly by `list` (against
//! [`pty::SessionInfo::created_by`], as `handle_list` does) and through
//! [`owner_record_admits`] + [`pty::PtyManager::owner_of`] by every action
//! addressed BY session id (`read` / `wait` / `explain`). One body, so five
//! lenses on the same sessions cannot silently disagree about which rows a
//! caller may see (判据 §9).
//!
//! For a caller that HAS an identity that predicate is [`pty::owner_admits`]
//! exactly — the same answer the other two faces give. For a caller with
//! none it is deliberately NARROWER than `owner_admits`, which answers
//! "unrestricted": here an unresolved actor admits only sessions nobody owns.
//! [`terminal_admits`]'s own doc carries the why; the short version is that
//! this tool, unlike the `pty.*` RPC face, is reachable from a run that
//! genuinely has no caller (cron, A2A, internal wiring).
//!
//! A session the caller does not own is reported as "no such session", byte
//! for byte the same wording `require_owned` uses in `gateway::handlers::pty`
//! and for every addressed action alike — a distinct "not yours" would turn
//! any of them into an oracle for enumerating other operators' session ids.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{notify_tool_result, notify_tool_start};
use crate::error::Result;
use crate::gateway::pty;
use crate::tools::AlephTool;

/// `terminal`'s five read-only actions. No write verb — see the tool's own
/// [`TerminalTool::DESCRIPTION`] for why.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TerminalAction {
    /// List the caller's own PTY sessions: `session_id`, `shell`, `cwd`
    /// (where the shell was SPAWNED — empty when it inherited the
    /// server's, and not updated by a later `cd`), `created_at` (epoch
    /// seconds) and `closed`.
    ///
    /// This enumeration is the same key set
    /// `aleph_protocol::pty::PtySessionInfo` defines and
    /// `pty_list_response_round_trips_and_pins_its_key_set` pins — say all
    /// five or none, because a short list here reads to the model as "those
    /// are the fields", and a field it is told does not exist is a field it
    /// will not ask for.
    List,
    /// Read one session's current visible screen (no scrollback). Requires
    /// `session_id`.
    Read,
    /// Report each of the caller's sessions' detected agent state — the
    /// same table `runtime.agents.list` serves.
    Status,
    /// Block until one session's agent state enters `until`, then return it.
    /// Requires `session_id`. Answers `timeout` with the current entry at
    /// `timeout_ms`, and `gone` if the session ends first.
    Wait,
    /// Explain one session's detected state: which manifest rule matched, at
    /// which manifest version, over which screen inputs. Requires
    /// `session_id`.
    Explain,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TerminalArgs {
    /// What to do.
    pub action: TerminalAction,
    /// Required for `read` / `wait` / `explain`: the PTY session id (from
    /// `list`'s output). Ignored for `list` / `status`.
    #[serde(default)]
    pub session_id: Option<String>,
    /// `wait` only: the states that end the wait. Defaults to
    /// `["blocked", "idle"]` — the two that mean "it wants you now".
    #[serde(default)]
    pub until: Option<Vec<aleph_protocol::runtime::RuntimeAgentState>>,
    /// `wait` only: how long to block, in milliseconds. Defaults to 60000 and
    /// is CLAMPED to 150000, never refused — a blocking call has to return
    /// inside this harness's foreground tool budget.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

/// Output envelope shared by all three actions — same shape as
/// `MoaManageOutput`: a flat `success`/`message`/`data` triple rather than a
/// per-action type, since the three actions have nothing in common to
/// factor beyond "did it work" and "here is the payload".
#[derive(Debug, Clone, Serialize)]
pub struct TerminalOutput {
    pub success: bool,
    pub message: String,
    pub data: Option<serde_json::Value>,
}

/// Absent role reads as operator — matches `role_is_operator` and every
/// other inline cross-cutting gate in this crate (`select_model`'s `moa:`
/// arm, `loop_manage`'s cross-session arm). Do not invent a stricter
/// default here: a cron/A2A/internal run has no channel-stamped role to
/// read, and that is not the same thing as a stranger asking.
fn caller_is_operator() -> bool {
    crate::tools::turn_context::current_turn_context().is_none_or(|ctx| ctx.caller_is_operator())
}

#[derive(Clone, Default)]
pub struct TerminalTool;

#[async_trait]
impl AlephTool for TerminalTool {
    const NAME: &'static str = "terminal";
    const DESCRIPTION: &'static str = "Read-only view of the terminal sessions you own on this \
        server; empty when the embedded terminal is disabled in policy. Lists sessions, reads \
        the current visible screen, and reports each agent's detected state (working / blocked \
        / idle / unknown). It cannot type into a terminal or run commands — a human does that. \
        `wait` blocks until one session reaches a state instead of polling `status`, and \
        answers `timeout` with the current entry rather than a guess. `explain` says WHY a \
        state was reported — which manifest rule matched, over which screen text and terminal \
        title — which is the only way to tell a wrong detection from an idle agent.";

    type Args = TerminalArgs;
    type Output = TerminalOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let action_label = match args.action {
            TerminalAction::List => "list",
            TerminalAction::Read => "read",
            TerminalAction::Status => "status",
            TerminalAction::Wait => "wait",
            TerminalAction::Explain => "explain",
        };
        notify_tool_start(Self::NAME, action_label);

        if !caller_is_operator() {
            // Do not shorten this back to "requires operator; refused." — an
            // operator-approval card for THIS call may have just been shown
            // and answered "yes" (see the module doc's `ScopedToolService`
            // paragraph). Worded as a fact about TODAY ("does not currently
            // lift"), not a permanent property of the tool: R11-14 decided
            // the fix is a seam carrying the gate's own verdict here, and
            // once that lands this sentence must change — "this tool can
            // never be approved" would become a lie the day it does.
            let message = "terminal requires operator; refused. An operator approving this \
                call's own escalation card does not currently lift this refusal — nothing \
                re-stamps the caller's role after approval."
                .to_string();
            notify_tool_result(Self::NAME, &message, false);
            return Ok(TerminalOutput {
                success: false,
                message,
                data: None,
            });
        }

        let actor = crate::gateway::visibility::ambient_actor();
        let result = match args.action {
            TerminalAction::List => list_sessions(actor.as_deref()),
            TerminalAction::Status => status(actor.as_deref()),
            TerminalAction::Read => read_session(args.session_id.as_deref(), actor.as_deref()),
            // The only `.await` of the five: `wait` is the one action that
            // blocks, and it blocks on the agent table's change watch, never
            // on a screen poll.
            TerminalAction::Wait => {
                wait_for_session(
                    args.session_id.as_deref(),
                    args.until.as_deref(),
                    args.timeout_ms,
                    actor.as_deref(),
                )
                .await
            }
            TerminalAction::Explain => {
                explain_session(args.session_id.as_deref(), actor.as_deref())
            }
        };

        match result {
            Ok(data) => {
                notify_tool_result(Self::NAME, action_label, true);
                Ok(TerminalOutput {
                    success: true,
                    message: action_label.to_string(),
                    data: Some(data),
                })
            }
            Err(message) => {
                notify_tool_result(Self::NAME, &message, false);
                Ok(TerminalOutput {
                    success: false,
                    message,
                    data: None,
                })
            }
        }
    }
}

/// `terminal`'s ownership predicate: [`pty::owner_admits`] for a caller that
/// HAS an identity, narrowed to `created_by == None` for one that does not.
///
/// The narrowing (D7) is the one place this tool deliberately departs from the
/// predicate the other two `pty.*` faces share, so it is spelled once and used
/// by all four actions rather than four times (判据 §9). `owner_admits`'s
/// unrestricted `actor: None` arm is right for the faces it was written for —
/// an RPC handler reached only through a `connect`-authenticated dispatch,
/// where `None` means "this deployment resolves no users", and where `pty.*`
/// is separately operator-gated on both faces. It is NOT right here: this tool
/// is also reachable from an agent run whose ambient actor is simply absent
/// (cron, A2A, an internal dispatch), and there "I could not tell who is
/// asking" must not be read as "everyone" (判据 §8).
///
/// In production that makes the actor-less arm show NOTHING, and this is the
/// intended shape rather than an oversight: `handlers::pty::handle_spawn` is
/// the only production spawn site and it stamps `visibility::ambient_actor()`,
/// which resolves through `CALLER_USER` for every gateway dispatch, loopback
/// included (it resolves to `OWNER_USER_ID`) — so no production session has
/// `created_by == None` to admit. The alternative is one unauthenticated code
/// path seeing every operator's live terminal.
fn terminal_admits(created_by: Option<&str>, actor: Option<&str>) -> bool {
    match actor {
        // Deliberately NOT `owner_admits`' unrestricted arm — see the doc
        // above. Falsified on 2026-09-04 by restoring `None => true`:
        // `an_actorless_caller_sees_only_unowned_sessions` goes red on its
        // first assertion (see the task-D report for the output).
        None => created_by.is_none(),
        Some(_) => pty::owner_admits(created_by, actor),
    }
}

/// [`terminal_admits`] against a [`pty::SessionOwner`] stamp — the shape the
/// `owner_of` lookup answers with, used by every action that starts from a
/// session id instead of from a listed row.
///
/// `Unknown` (no record at all: never existed, or aged past `OWNER_RETENTION`)
/// is refused for EVERY caller, an actor-less one included.
/// `SessionOwner::admits` answers `actor.is_none()` there for the same reason
/// its `Known` arm is unrestricted, and the same reasoning that narrows one
/// narrows the other: a caller who cannot be identified must not be handed a
/// session whose owner cannot be identified either. Every live session has a
/// record, so this refuses only ids that are gone or were never there — which
/// is what `read` answers `no_such_session` for one line later anyway.
fn owner_record_admits(owner: &pty::SessionOwner, actor: Option<&str>) -> bool {
    match owner {
        pty::SessionOwner::Known(created_by) => terminal_admits(created_by.as_deref(), actor),
        pty::SessionOwner::Unknown => false,
    }
}

/// `list` — the caller's own sessions, filtered as `handle_list` in
/// `gateway::handlers::pty` does — [`terminal_admits`] against each
/// [`pty::SessionInfo::created_by`] directly (no `owner_of` round trip needed
/// — `list()` already carries the field), narrowed for an actor-less caller.
fn list_sessions(actor: Option<&str>) -> std::result::Result<serde_json::Value, String> {
    let body = aleph_protocol::pty::PtyListResponse {
        sessions: pty::manager()
            .list()
            .iter()
            .filter(|s| terminal_admits(s.created_by.as_deref(), actor))
            .map(aleph_protocol::pty::PtySessionInfo::from)
            .collect(),
    };
    serde_json::to_value(&body).map_err(|e| format!("encode failed: {e}"))
}

/// `status` — the same table `runtime.agents.list` serves, filtered with the
/// same `owner_of` lookup `handle_list` in `gateway::handlers::runtime` uses,
/// through [`owner_record_admits`] rather than `SessionOwner::admits`.
fn status(actor: Option<&str>) -> std::result::Result<serde_json::Value, String> {
    let agents: Vec<_> = crate::gateway::runtime::agents()
        .snapshot()
        .into_iter()
        .filter(|entry| owner_record_admits(&pty::manager().owner_of(&entry.session_id), actor))
        .collect();
    let body = aleph_protocol::runtime::RuntimeAgentsListResponse { agents };
    serde_json::to_value(&body).map_err(|e| format!("encode failed: {e}"))
}

/// `read` — one session's current visible screen, no scrollback. Ownership
/// is checked BEFORE reading the screen (`PtyManager::visible_text` only
/// checks existence), and a session the caller does not own is refused with
/// exactly `require_owned`'s wording — an unowned session and a nonexistent
/// one must look identical, or `read` becomes an id-enumeration oracle. Both
/// call [`pty::no_such_session`] so a future wording change cannot land in
/// one of the two and re-open that oracle.
fn read_session(
    session_id: Option<&str>,
    actor: Option<&str>,
) -> std::result::Result<serde_json::Value, String> {
    let session_id = owned_session_id(session_id, actor, "read")?;
    let text = pty::manager().visible_text(session_id)?;
    Ok(serde_json::json!({ "session_id": session_id, "text": text }))
}

/// The shared front half of every action that takes a `session_id`: the
/// argument is present and non-blank, and the caller may have that session.
///
/// One body for `read` / `wait` / `explain` so a third addressed action cannot
/// arrive with the ownership check spelled slightly differently — the failure
/// this tool's module doc already describes for the other two faces (判据 §9).
/// The refusal is [`pty::no_such_session`] verbatim: an unowned session and a
/// nonexistent one must be byte-identical on every addressed action, or the
/// one that is not becomes an id-enumeration oracle for all of them.
fn owned_session_id<'a>(
    session_id: Option<&'a str>,
    actor: Option<&str>,
    action: &str,
) -> std::result::Result<&'a str, String> {
    let session_id = session_id
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("{action} requires `session_id`"))?;
    if !owner_record_admits(&pty::manager().owner_of(session_id), actor) {
        return Err(pty::no_such_session(session_id));
    }
    Ok(session_id)
}

/// `wait`'s window when the caller names none.
///
/// Same 60 s `bash_exec`'s `process_action: "wait"` defaults to, and for the
/// same reason: a blocking call spends the foreground tool budget, so the
/// default is modest and an impatient caller can simply ask again.
const WAIT_DEFAULT_TIMEOUT_MS: u64 = 60_000;

/// The hard ceiling on `wait`'s window, in milliseconds.
///
/// Derived from the same constraint as `bash_exec`'s `WAIT_MAX_TIMEOUT_SECS`
/// (170 s): a blocking tool call has to return INSIDE this harness's 180 s
/// foreground budget, or the budget wrapper kills the call and the caller
/// learns nothing — not even the timeout it asked for.
/// `the_wait_ceiling_stays_under_the_foreground_tool_budget` pins this against
/// that constant rather than against a second copy of the number, so
/// shrinking the budget reddens here.
///
/// A request over the ceiling is CLAMPED, never refused: a caller that asks
/// for ten minutes wants to wait as long as it can, and answering "no" to that
/// buys nothing but a retry.
const WAIT_MAX_TIMEOUT_MS: u64 = 150_000;

/// `until`'s default: the two states that mean the agent wants a human now.
///
/// `working` and `unknown` are deliberately absent — waiting for them is
/// legal but is never what "tell me when it needs me" means, and a default
/// that included them would return instantly on almost every call.
const WAIT_DEFAULT_UNTIL: [aleph_protocol::runtime::RuntimeAgentState; 2] = [
    aleph_protocol::runtime::RuntimeAgentState::Blocked,
    aleph_protocol::runtime::RuntimeAgentState::Idle,
];

/// How much of the screen `explain` shows back. Display only — the ENGINE is
/// fed the whole visible text, exactly as the sampler feeds it.
const EXPLAIN_SCREEN_TAIL_LINES: usize = 12;

/// `wait`'s payload. `agent` is the protocol's own entry type, so the shape a
/// waiter gets back is the shape `status` and `runtime.agents.list` hand out
/// (判据 §10) — a caller does not have to learn a second spelling of the same
/// row to read the answer to its own question.
#[derive(Debug, Clone, Serialize)]
struct TerminalWaitOutput {
    session_id: String,
    /// `reached` | `timeout` | `gone`.
    outcome: &'static str,
    /// The entry as of the moment the wait ended. `None` only when the table
    /// has no row: always absent for `gone`, and possible for `timeout` on a
    /// session that has never produced a frame.
    agent: Option<aleph_protocol::runtime::RuntimeAgentEntry>,
}

/// How a wait ended. Three variants and no fourth: "it ended and I am not
/// telling you why" is not an outcome a caller can act on.
#[derive(Debug, Clone, PartialEq, Eq)]
enum WaitOutcome {
    /// The state entered `until`; carries the entry that says so.
    Reached(aleph_protocol::runtime::RuntimeAgentEntry),
    /// The window elapsed. Carries the CURRENT entry — not the last one seen
    /// before the wait started, and never dressed up as a final state
    /// (spec §5's error table).
    Timeout(Option<aleph_protocol::runtime::RuntimeAgentEntry>),
    /// The session's row is gone and the PTY registry has no such session
    /// either: the terminal ended while we were waiting.
    Gone,
}

impl WaitOutcome {
    /// The wire word and the entry, split out so the label and the payload
    /// cannot disagree about which arm produced them.
    fn into_parts(
        self,
    ) -> (
        &'static str,
        Option<aleph_protocol::runtime::RuntimeAgentEntry>,
    ) {
        match self {
            Self::Reached(entry) => ("reached", Some(entry)),
            Self::Timeout(entry) => ("timeout", entry),
            Self::Gone => ("gone", None),
        }
    }
}

/// The window a `timeout_ms` request actually buys.
fn wait_window(requested: Option<u64>) -> std::time::Duration {
    std::time::Duration::from_millis(
        requested
            .unwrap_or(WAIT_DEFAULT_TIMEOUT_MS)
            .min(WAIT_MAX_TIMEOUT_MS),
    )
}

/// Whether the PTY registry still holds this session.
///
/// Consulted only when the agent table has NO row, to tell the two absences
/// apart: a session that has produced no frame yet (keep waiting) from one
/// that ended (`gone`). Folding them would make `wait` answer `gone` for a
/// live shell that simply has not painted — a wrong label, which reads as a
/// fact and costs more than a missing one (判据 §17).
fn session_is_registered(session_id: &str) -> bool {
    pty::manager()
        .list()
        .iter()
        .any(|s| s.session_id == session_id)
}

/// The one classification of "is this wait over yet", applied on every wake-up
/// AND once more when the window closes. `None` = keep waiting.
fn wait_verdict(
    table: &crate::gateway::runtime::RuntimeAgents,
    session_id: &str,
    until: &[aleph_protocol::runtime::RuntimeAgentState],
) -> Option<WaitOutcome> {
    match table.entry(session_id) {
        Some(entry) if until.contains(&entry.state) => Some(WaitOutcome::Reached(entry)),
        Some(_) => None,
        None if session_is_registered(session_id) => None,
        None => Some(WaitOutcome::Gone),
    }
}

/// Block until `session_id`'s state enters `until`, the session ends, or
/// `window` elapses.
///
/// **This does not poll the screen.** It rides
/// [`crate::gateway::runtime::RuntimeAgents::subscribe`], the watch channel
/// the table bumps on every observable change, and re-reads the row on each
/// wake-up. A poll loop here would be a second clock over a table that already
/// publishes when it moves (判据 §12), and it would burn CPU for the entire
/// window on a session that is doing nothing — which is the normal case.
///
/// The subscription is taken BEFORE the first read on purpose: `watch` marks
/// the current value seen at `subscribe()`, so a change landing between the
/// read and the sleep bumps the generation and wakes us immediately instead of
/// being missed until the next one.
///
/// `table` is a parameter rather than the process-global so a test can drive an
/// isolated instance. `session_is_registered` still consults the global PTY
/// registry, which is why tests that exercise the `gone` arm carry the
/// `pty_global_manager` key.
async fn wait_for_state(
    table: &crate::gateway::runtime::RuntimeAgents,
    session_id: &str,
    until: &[aleph_protocol::runtime::RuntimeAgentState],
    window: std::time::Duration,
) -> WaitOutcome {
    let mut changes = table.subscribe();
    let deadline = tokio::time::Instant::now() + window;
    loop {
        if let Some(outcome) = wait_verdict(table, session_id, until) {
            return outcome;
        }
        match tokio::time::timeout_at(deadline, changes.changed()).await {
            Ok(Ok(())) => {}
            // Deadline, or a sender that went away (structurally unreachable
            // while this borrow of the table lives, since the table owns the
            // sender). Re-run the same verdict once: a change that landed in
            // the same instant as the deadline must not be reported as a
            // timeout when the state it was waiting for had already arrived.
            Ok(Err(_)) | Err(_) => {
                return wait_verdict(table, session_id, until)
                    .unwrap_or_else(|| WaitOutcome::Timeout(table.entry(session_id)));
            }
        }
    }
}

/// `wait` — the tool face: ownership gate, defaults, clamp, then
/// [`wait_for_state`] against the process-global table.
async fn wait_for_session(
    session_id: Option<&str>,
    until: Option<&[aleph_protocol::runtime::RuntimeAgentState]>,
    timeout_ms: Option<u64>,
    actor: Option<&str>,
) -> std::result::Result<serde_json::Value, String> {
    let session_id = owned_session_id(session_id, actor, "wait")?;
    let until = match until {
        // An explicit empty list is refused rather than defaulted: it can only
        // produce `timeout`, so honouring it literally spends the caller's
        // whole foreground budget to report nothing. Say which words are
        // accepted instead of guessing which one was meant.
        Some([]) => {
            return Err("wait requires at least one state in `until` \
                 (blocked / idle / working / unknown); omit it for [blocked, idle]"
                .to_string())
        }
        Some(states) => states,
        None => &WAIT_DEFAULT_UNTIL,
    };
    let (outcome, agent) = wait_for_state(
        crate::gateway::runtime::agents(),
        session_id,
        until,
        wait_window(timeout_ms),
    )
    .await
    .into_parts();
    let body = TerminalWaitOutput {
        session_id: session_id.to_owned(),
        outcome,
        agent,
    };
    serde_json::to_value(&body).map_err(|e| format!("encode failed: {e}"))
}

/// `explain`'s payload — which rule decided a state, over which inputs.
#[derive(Debug, Clone, Serialize)]
struct TerminalExplainOutput {
    session_id: String,
    /// The agent the sampler identified, `None` when it identified none.
    agent: Option<String>,
    /// The state a FRESH evaluation of the current screen reports.
    ///
    /// This can differ from the `state` `status` publishes for the same
    /// session, and the difference is information rather than a bug: the
    /// table applies a working -> idle hold and keeps the previous state when
    /// a rule says the screen is mid-repaint, while this is the raw reading of
    /// the screen as it is right now. Same four words in both places
    /// (`runtime::wire_state`), so they are at least comparable.
    state: aleph_protocol::runtime::RuntimeAgentState,
    matched_rule: Option<TerminalExplainRule>,
    /// Where the manifest came from — `bundled` is the only source this phase
    /// ships.
    source: Option<&'static str>,
    /// The manifest revision that produced the answer. `None` = this agent has
    /// no screen manifest, or its manifest declares no version — never "the
    /// manifest is missing".
    manifest_version: Option<String>,
    /// Why there is no rule, when there is none. Absent when a rule matched.
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    inputs: TerminalExplainInputs,
}

#[derive(Debug, Clone, Serialize)]
struct TerminalExplainRule {
    id: String,
    priority: i32,
    region: String,
    state: aleph_protocol::runtime::RuntimeAgentState,
}

/// What the engine was actually shown. Without this an explanation is
/// unfalsifiable: "no rule matched" and "the screen this reached was empty"
/// are the same sentence to a reader who cannot see the input.
#[derive(Debug, Clone, Serialize)]
struct TerminalExplainInputs {
    title: String,
    osc_progress: String,
    /// The LAST [`EXPLAIN_SCREEN_TAIL_LINES`] lines of the visible screen.
    /// Display only — the engine is fed the whole visible text.
    screen_tail: String,
}

/// `explain` — the tool face.
fn explain_session(
    session_id: Option<&str>,
    actor: Option<&str>,
) -> std::result::Result<serde_json::Value, String> {
    let session_id = owned_session_id(session_id, actor, "explain")?;
    // Reads the live screen through the same accessor `flush_session` samples
    // from, under one lock acquisition.
    let screen = pty::manager().detection_inputs(session_id)?;
    let agents = crate::gateway::runtime::agents();
    let body = explain_detection(
        session_id,
        agents.detected_agent(session_id),
        agents.entry(session_id).as_ref(),
        &screen,
    );
    serde_json::to_value(&body).map_err(|e| format!("encode failed: {e}"))
}

/// Run the detection engine over one session's current screen and say what it
/// decided — pure, so the mapping can be tested without a live PTY.
///
/// `sampled` is the table's row, consulted ONLY to say why there is nothing to
/// explain: no row at all and a row whose foreground program is not an agent
/// are different absences, and answering both with the same sentence is how a
/// "we never looked" is read as "we looked and found nothing" (判据 §8).
fn explain_detection(
    session_id: &str,
    agent: Option<agent_detect::Agent>,
    sampled: Option<&aleph_protocol::runtime::RuntimeAgentEntry>,
    screen: &crate::gateway::pty::manager::DetectionInputs,
) -> TerminalExplainOutput {
    let inputs = TerminalExplainInputs {
        title: screen.title.clone(),
        osc_progress: screen.osc_progress.clone(),
        screen_tail: screen_tail(&screen.text),
    };

    let Some(agent) = agent else {
        return TerminalExplainOutput {
            session_id: session_id.to_owned(),
            agent: None,
            // The engine's own permanent answer for `agent: None`: with no
            // agent there is nothing to match a screen against, so the state
            // is Unknown regardless of what is on it. "I do not know" is not
            // "it is idle".
            state: aleph_protocol::runtime::RuntimeAgentState::Unknown,
            matched_rule: None,
            source: None,
            manifest_version: None,
            reason: Some(match sampled {
                None => "this session has no row in the agent table yet — nothing has been \
                         sampled, which is not the same as nothing running"
                    .to_string(),
                Some(entry) => format!(
                    "the foreground program ({}) is not an agent the bundled manifests know",
                    entry.program.as_deref().unwrap_or("not probed")
                ),
            }),
            inputs,
        };
    };

    // The SAME constructor the sampler's detection call goes through, so the
    // two OSC strings cannot be mapped onto different fields on the two paths.
    let explained = agent_detect::manifest::explain_with_input(
        agent,
        agent_detect::screen_rules::detection_input(
            &screen.text,
            &screen.title,
            &screen.osc_progress,
        ),
    );
    TerminalExplainOutput {
        session_id: session_id.to_owned(),
        agent: explained.agent.clone(),
        state: crate::gateway::runtime::wire_state(explained.state),
        matched_rule: explained
            .matched_rule
            .as_ref()
            .map(|r| TerminalExplainRule {
                id: r.id.clone(),
                priority: r.priority,
                region: r.region.clone(),
                state: crate::gateway::runtime::wire_state(r.state),
            }),
        source: explained
            .source
            .as_ref()
            .map(agent_detect::ManifestSource::kind),
        manifest_version: explained.manifest_version.clone(),
        // A warning outranks a fallback: it says the manifest itself could not
        // be honoured, which is a bigger fact than which fallback ran. Neither
        // is invented here — both are the engine's own words.
        reason: explained
            .warning
            .clone()
            .or_else(|| explained.fallback_reason.clone()),
        inputs,
    }
}

/// The last [`EXPLAIN_SCREEN_TAIL_LINES`] lines of `text`.
///
/// Line-based rather than a byte count so no slice can land inside a
/// multi-byte character, and because the bottom of the screen is where every
/// agent paints its prompt and its spinner — a head-anchored excerpt would
/// show scrollback padding on exactly the sessions worth explaining.
fn screen_tail(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(EXPLAIN_SCREEN_TAIL_LINES)..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pull the accepted action strings out of a tool schema, whichever of
    /// the two shapes schemars emitted.
    ///
    /// schemars 1.2 renders a fieldless enum as a flat `enum` array ONLY when
    /// no variant carries a doc comment; the moment one does — and all three
    /// of `TerminalAction`'s do, because the model reads them — it emits
    /// `oneOf` of `{const, description}` instead, to have somewhere to put
    /// the per-variant text. Both shapes mean the same thing to a provider,
    /// and which one ships is decided by something as innocent as deleting a
    /// `///` line, so the guard reads both rather than pinning the accident.
    ///
    /// Panics rather than returning an empty list when it recognises neither:
    /// "I cannot find the actions" must not be answerable as "there are no
    /// write verbs" (判据 §8).
    fn declared_actions(schema: &serde_json::Value) -> Vec<String> {
        let action = &schema["$defs"]["TerminalAction"];
        if let Some(flat) = action["enum"].as_array() {
            return flat
                .iter()
                .map(|v| v.as_str().expect("enum member is a string").to_string())
                .collect();
        }
        if let Some(variants) = action["oneOf"].as_array() {
            return variants
                .iter()
                .map(|v| {
                    v["const"]
                        .as_str()
                        .expect("oneOf member carries a const")
                        .to_string()
                })
                .collect();
        }
        panic!(
            "neither $defs.TerminalAction.enum nor .oneOf found; schema was {}",
            serde_json::to_string_pretty(schema).unwrap_or_default()
        );
    }

    /// 本期没有写入动词。多一个就是多一个授权面。
    ///
    /// 2026-09-04 (task D): `wait` and `explain` join the list. Both are still
    /// reads — `wait` blocks on the agent table's change watch and returns a
    /// row, `explain` re-runs the detection engine over the screen — so the
    /// claim this test pins ("no write verb") is unchanged, and
    /// `the_description_says_it_is_read_only` stays true beside it. The
    /// EXPECTED list is spelled out rather than counted so adding a verb is a
    /// deliberate edit here and not a number that quietly grows.
    ///
    /// Read out of `$defs`, not `properties.action`: schemars 1.2 emits a
    /// NAMED type as a `$ref`, so `properties.action` carries no action
    /// vocabulary at all and a guard reading it asserts against `Null`.
    /// That is the shape every sibling tool with an enum-typed argument
    /// already ships, and `schema_strictify` rewrites those refs explicitly.
    ///
    /// Not to be "fixed" by forcing `#[schemars(inline)]` to match
    /// `moa_manage`'s flat schema: that tool hand-writes `impl JsonSchema`
    /// because `#[serde(tag = "action")]` puts a `oneOf` at the ROOT, which
    /// grammar-constrained endpoints cannot compile — they answer with EMPTY
    /// arguments. `TerminalArgs` is a plain struct; its root is already a
    /// flat object, so that hazard is not this tool's to carry, and inlining
    /// would make `terminal` the one tool shipping a shape its nine siblings
    /// do not.
    #[test]
    fn the_tool_exposes_no_write_verb() {
        let def = TerminalTool.definition();
        let actions = declared_actions(&def.parameters);
        assert_eq!(actions, ["list", "read", "status", "wait", "explain"]);
    }

    /// DESCRIPTION 必须自己说清只读——这句话归这个工具所有，
    /// 不进 system prompt（R9 第二把尺）。不写，模型会反复试着发命令。
    #[test]
    fn the_description_says_it_is_read_only() {
        assert!(TerminalTool::DESCRIPTION
            .to_lowercase()
            .contains("read-only"));
    }

    /// No `TurnContext` at all reads as operator (cron/A2A/internal
    /// convention) — a caller with a scoped, non-operator role is refused.
    ///
    /// Reaches the process-global `PtyManager` via `list_sessions`, so it
    /// carries the same `pty_global_manager` parallel key every other test
    /// in the crate that touches the singleton does — see the module doc on
    /// `gateway::handlers::pty::every_test_that_reaches_the_global_pty_manager_is_tagged`,
    /// which cannot see this reacher itself (it lives behind a function
    /// call from the production half of this file, not inside a
    /// `#[cfg(test)]` block the census scans — task-11 review F7).
    #[tokio::test]
    #[serial_test::parallel(pty_global_manager)]
    async fn no_turn_context_is_treated_as_operator() {
        let out = TerminalTool
            .call(TerminalArgs {
                action: TerminalAction::List,
                session_id: None,
                until: None,
                timeout_ms: None,
            })
            .await
            .unwrap();
        assert!(out.success, "{}", out.message);
    }

    #[tokio::test]
    async fn non_operator_caller_is_refused() {
        use crate::routing::session_key::SessionKey;
        use crate::tools::turn_context::{TurnContext, TURN_CONTEXT};

        let ctx = TurnContext {
            session_key: SessionKey::Ephemeral {
                agent_id: "main".to_string(),
                ephemeral_id: "terminal-guest-test".to_string(),
            },
            run_id: String::new(),
            channel_id: String::new(),
            conversation_id: String::new(),
            caller_role: Some("guest".to_string()),
            channel_tool_permissions: None,
            unattended: false,
            plan_gate: None,
            side_question: false,
        };
        let out = TURN_CONTEXT
            .scope(ctx, async {
                TerminalTool
                    .call(TerminalArgs {
                        action: TerminalAction::List,
                        session_id: None,
                        until: None,
                        timeout_ms: None,
                    })
                    .await
            })
            .await
            .unwrap();
        assert!(!out.success);
        assert!(out.message.contains("operator"), "{}", out.message);
        // A refusal that still carried session data would be a gate that
        // reports "no" and means "yes" (task-11 review F10) — discarding
        // the `data: None` in the two arms of `TerminalTool::call` and
        // keeping only the label check would leave this test green.
        assert!(out.data.is_none(), "a refusal must not carry session data");
    }

    #[tokio::test]
    async fn read_without_session_id_is_refused_not_panicking() {
        let out = TerminalTool
            .call(TerminalArgs {
                action: TerminalAction::Read,
                session_id: None,
                until: None,
                timeout_ms: None,
            })
            .await
            .unwrap();
        assert!(!out.success);
        assert!(out.message.contains("session_id"), "{}", out.message);
    }

    /// Reaches the global `PtyManager` via `read_session`'s
    /// `owner_of`/`visible_text` calls — same F7 rationale as
    /// `no_turn_context_is_treated_as_operator` above.
    #[tokio::test]
    #[serial_test::parallel(pty_global_manager)]
    async fn read_of_unknown_session_is_no_such_session() {
        let out = TerminalTool
            .call(TerminalArgs {
                action: TerminalAction::Read,
                session_id: Some("does-not-exist".to_string()),
                until: None,
                timeout_ms: None,
            })
            .await
            .unwrap();
        assert!(!out.success);
        assert!(out.message.contains("no such session"), "{}", out.message);
        // Same reasoning as `non_operator_caller_is_refused` (F10): the
        // refusal's payload, not just its label, must be asserted.
        assert!(out.data.is_none(), "a refusal must not carry session data");
    }

    /// A session that EXISTS but belongs to someone else must look
    /// identical to one that does not exist at all — the assertion whose
    /// absence let `read_session`'s ownership check (`terminal.rs:241`) be
    /// deleted without reddening anything, since every existing test used an
    /// id that never existed either way (task-11 review F8).
    #[test]
    #[serial_test::parallel(pty_global_manager)]
    fn read_of_someone_elses_session_is_refused_like_unknown() {
        use crate::gateway::pty::SpawnOptions;

        let id = pty::manager()
            .spawn(&SpawnOptions {
                created_by: Some("u-owner".to_string()),
                ..Default::default()
            })
            .expect("spawn")
            .session_id;

        let result = read_session(Some(&id), Some("u-someone-else"));

        // Close BEFORE asserting: this spawns on the process-global manager,
        // so a failing assert would leak a live PTY for the rest of the test
        // binary and every later test sharing that singleton would inherit it.
        let _ = pty::manager().close(&id);

        assert_eq!(
            result,
            Err(pty::no_such_session(&id)),
            "an unowned session and a nonexistent one must produce byte-identical \
             refusals, or `read` becomes an id-enumeration oracle"
        );
    }

    /// D7: a caller with NO resolved identity sees only the sessions nobody
    /// owns — and still sees those.
    ///
    /// Both halves are asserted because the rule they separate is the whole
    /// change: "actor-less admits everything" (what `pty::owner_admits` says,
    /// and what this tool used to inherit) and "actor-less admits nothing"
    /// both pass a test that only checks the owned session is hidden. The
    /// unowned session is what says which of the two shipped.
    ///
    /// The identified caller is asserted too: spec §10 ruled the narrowing
    /// must not blind an operator to their own sessions, and that claim needs
    /// a witness rather than a comment.
    ///
    /// `status` reads the runtime table rather than the PTY registry, so the
    /// owned session is sampled into it — otherwise the `status` half is
    /// vacuous (an empty table hides everything, whatever the predicate says).
    ///
    /// All FOUR verbs that can name or hand out a session id, not the three
    /// spec §4.4 lists: `wait` takes a `session_id` too, and a gate applied to
    /// three of four addressed actions is not a gate — it is the shape this
    /// tool's own module doc describes for `plugin_manage` (one face closed,
    /// one open). `wait`'s window is zero here so the refusal (or the
    /// immediate timeout) is what is measured, not a sleep.
    #[tokio::test]
    #[serial_test::parallel(pty_global_manager)]
    async fn an_actorless_caller_sees_only_unowned_sessions() {
        use crate::gateway::pty::screen::Screen;
        use crate::gateway::pty::SpawnOptions;
        use crate::gateway::runtime::{agents, SampleInput};

        let owned = pty::manager()
            .spawn(&SpawnOptions {
                created_by: Some("u-owner".to_string()),
                ..Default::default()
            })
            .expect("spawn owned")
            .session_id;
        let unowned = pty::manager()
            .spawn(&SpawnOptions {
                created_by: None,
                ..Default::default()
            })
            .expect("spawn unowned")
            .session_id;

        let screen = Screen::new(4, 40);
        for id in [&owned, &unowned] {
            agents().sample(SampleInput {
                session_id: id,
                shell: "zsh",
                program: None,
                argv0: None,
                cmdline: None,
                cwd: "",
                screen: &screen,
                process_exited: false,
                frame_produced: true,
                now: 0,
            });
        }

        let anon_list = list_sessions(None).expect("list");
        let anon_status = status(None).expect("status");
        let anon_read_owned = read_session(Some(&owned), None);
        let anon_read_unowned = read_session(Some(&unowned), None);
        let anon_wait_owned = wait_for_session(Some(&owned), None, Some(0), None).await;
        let anon_wait_unowned = wait_for_session(Some(&unowned), None, Some(0), None).await;
        let owner_list = list_sessions(Some("u-owner")).expect("list as owner");

        // Close BEFORE asserting — same reason as
        // `read_of_someone_elses_session_is_refused_like_unknown`: a failing
        // assert would leak two live PTYs into every later test in this
        // binary.
        for id in [&owned, &unowned] {
            agents().remove(id);
            let _ = pty::manager().close(id);
        }

        let ids = |v: &serde_json::Value, key: &str| -> Vec<String> {
            v[key]
                .as_array()
                .expect("array")
                .iter()
                .map(|e| e["session_id"].as_str().expect("session_id").to_string())
                .collect()
        };

        assert!(
            !ids(&anon_list, "sessions").contains(&owned),
            "an actor-less caller must not see a session someone else owns"
        );
        assert!(
            !ids(&anon_status, "agents").contains(&owned),
            "`status` must filter with the same predicate `list` does"
        );
        assert_eq!(
            anon_read_owned,
            Err(pty::no_such_session(&owned)),
            "an owned session must read as nonexistent to an actor-less caller"
        );
        assert_eq!(
            anon_wait_owned,
            Err(pty::no_such_session(&owned)),
            "`wait` is addressed by session id too — the same refusal, byte for byte, or it \
             becomes the oracle `read` refuses to be"
        );

        assert!(
            ids(&anon_list, "sessions").contains(&unowned),
            "a session nobody owns is what the actor-less arm still admits"
        );
        assert!(
            ids(&anon_status, "agents").contains(&unowned),
            "…on the status face too"
        );
        assert!(
            anon_read_unowned.is_ok(),
            "…and it must still be readable: {anon_read_unowned:?}"
        );
        assert_eq!(
            anon_wait_unowned
                .as_ref()
                .map(|v| v["outcome"].as_str().unwrap_or_default().to_string()),
            Ok("timeout".to_string()),
            "…and waitable: an unowned session in `unknown` with a zero window times out, \
             which is the shape that proves the gate let the call through at all"
        );

        assert!(
            ids(&owner_list, "sessions").contains(&owned),
            "the narrowing must not blind an identified caller to its own session (spec §10)"
        );
    }

    // ── Step 1 (task D): what a loopback operator actually is ─────────────

    /// The premise D7 turns on, asserted rather than assumed: a loopback
    /// operator is NOT the actor-less caller.
    ///
    /// Spec §10 left the arm's shape conditional on this — if a Panel-spawned
    /// session carried `created_by: None`, narrowing the actor-less arm to
    /// unowned rows would have been a no-op. It does not: the loopback
    /// handshake resolves a user, that user is scoped as `CALLER_USER` around
    /// every dispatched request, and `ambient_actor` reads it.
    ///
    /// The identity is taken FROM the production resolver rather than written
    /// here as a literal — a test that scopes its own constant and then reads
    /// it back would be asserting `task_local`, not this chain (判据 §10).
    /// The last link (that `handle_spawn` stamps `ambient_actor()` onto
    /// `SessionInfo::created_by`) is already pinned, for an arbitrary user, by
    /// `handlers::pty::tests::a_spawn_through_the_handler_carries_both_the_actor_and_the_scrollback`.
    #[tokio::test]
    async fn a_loopback_operator_is_not_an_actor_less_caller() {
        use crate::gateway::caller_identity::CALLER_USER;
        use crate::gateway::security::store::SecurityStore;

        let store = SecurityStore::in_memory().expect("in-memory security store");
        let (user, role) =
            crate::gateway::handlers::connect::resolve_connection_identity(true, None, &store);
        assert_eq!(role, "operator", "loopback resolves to the implicit owner");

        let actor = CALLER_USER
            .scope(user.clone(), async {
                crate::gateway::visibility::ambient_actor()
            })
            .await;

        assert_eq!(
            actor, user,
            "the connection's resolved user must be the ambient actor a tool call sees"
        );
        assert!(
            actor.is_some(),
            "a loopback operator has an identity, so the actor-less arm is NOT its arm — \
             this is spec §10's second case, and the arm narrows to `created_by == None`"
        );
    }

    // ── wait ──────────────────────────────────────────────────────────────

    /// An isolated table plus a screen, so a wait test never races the
    /// process-global sampler.
    fn sample_state(
        table: &crate::gateway::runtime::RuntimeAgents,
        session_id: &str,
        shell: &str,
        bytes: &[u8],
    ) {
        use crate::gateway::pty::screen::Screen;
        use crate::gateway::runtime::SampleInput;

        let mut screen = Screen::new(4, 40);
        screen.feed(bytes);
        table.sample(SampleInput {
            session_id,
            shell,
            program: None,
            argv0: None,
            cmdline: None,
            cwd: "",
            screen: &screen,
            process_exited: false,
            frame_produced: true,
            now: 0,
        });
    }

    /// `grok`'s OSC 9;4 progress payload for "working" — the same wire
    /// `gateway::runtime::tests::the_osc_progress_wire_is_actually_connected`
    /// uses, so this test is not inventing a signal the engine may stop
    /// honouring without anything going red.
    const OSC_PROGRESS_WORKING: &[u8] = b"\x1b]9;4;1;-1\x07";

    /// The wake-up edge: a state that arrives AFTER the wait started must end
    /// it. Starting in `unknown` and waiting for `working` means an
    /// implementation that answered from the first read alone cannot pass.
    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::parallel(pty_global_manager)]
    async fn wait_returns_when_the_state_enters_the_until_set() {
        use aleph_protocol::runtime::RuntimeAgentState;
        use std::sync::Arc;

        let table = Arc::new(crate::gateway::runtime::RuntimeAgents::default());
        // A shell is not an agent, so this row starts at `unknown`.
        sample_state(&table, "s-wait", "zsh", b"");

        let writer = Arc::clone(&table);
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
            sample_state(&writer, "s-wait", "grok", OSC_PROGRESS_WORKING);
        });

        let outcome = wait_for_state(
            &table,
            "s-wait",
            &[RuntimeAgentState::Working],
            std::time::Duration::from_secs(5),
        )
        .await;

        match outcome {
            WaitOutcome::Reached(entry) => assert_eq!(entry.state, RuntimeAgentState::Working),
            other => panic!("the wait must end when the state arrives, got {other:?}"),
        }
    }

    /// A timeout carries the CURRENT entry, not a manufactured final state
    /// (spec §5: `timeout` + the current entry, never "the last entry as if
    /// it were the answer"). Asserting only the label would leave an
    /// implementation that reports `timeout` with `agent: null` green, and a
    /// caller cannot tell "still working" from "I lost sight of it".
    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::parallel(pty_global_manager)]
    async fn wait_times_out_with_the_current_entry() {
        use aleph_protocol::runtime::RuntimeAgentState;

        let table = crate::gateway::runtime::RuntimeAgents::default();
        sample_state(&table, "s-timeout", "grok", OSC_PROGRESS_WORKING);

        let outcome = wait_for_state(
            &table,
            "s-timeout",
            &[RuntimeAgentState::Blocked],
            std::time::Duration::from_millis(60),
        )
        .await;

        match outcome {
            WaitOutcome::Timeout(Some(entry)) => {
                assert_eq!(entry.state, RuntimeAgentState::Working);
                assert_eq!(entry.session_id, "s-timeout");
            }
            other => {
                panic!("a window that closes with nothing reached is a timeout, got {other:?}")
            }
        }
    }

    /// The session ending is its own outcome. `gone` and `timeout` must not be
    /// the same answer: a caller that gets `timeout` will wait again, and a
    /// caller that gets `gone` knows there is nothing left to wait for.
    ///
    /// Reaches the global PTY registry through `session_is_registered` — the
    /// id below is in no registry, which is exactly the "the terminal ended"
    /// shape.
    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::parallel(pty_global_manager)]
    async fn wait_reports_gone_when_the_session_is_removed() {
        use aleph_protocol::runtime::RuntimeAgentState;
        use std::sync::Arc;

        let table = Arc::new(crate::gateway::runtime::RuntimeAgents::default());
        sample_state(&table, "s-gone", "grok", OSC_PROGRESS_WORKING);

        let remover = Arc::clone(&table);
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
            remover.remove("s-gone");
        });

        let outcome = wait_for_state(
            &table,
            "s-gone",
            &[RuntimeAgentState::Blocked],
            std::time::Duration::from_secs(5),
        )
        .await;

        assert_eq!(
            outcome,
            WaitOutcome::Gone,
            "a session whose row is gone and which the registry does not know is `gone`"
        );
    }

    /// A row that is absent only because nothing has painted yet is NOT
    /// `gone`. Without this, `wait` on a freshly spawned shell answers "the
    /// terminal ended" — a wrong label, which reads as a fact.
    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::parallel(pty_global_manager)]
    async fn wait_on_a_live_session_with_no_row_yet_keeps_waiting() {
        use crate::gateway::pty::SpawnOptions;
        use aleph_protocol::runtime::RuntimeAgentState;

        let live = pty::manager()
            .spawn(&SpawnOptions::default())
            .expect("spawn")
            .session_id;

        // Empty table: the session is registered, but nothing was ever
        // sampled for it.
        let table = crate::gateway::runtime::RuntimeAgents::default();
        let outcome = wait_for_state(
            &table,
            &live,
            &[RuntimeAgentState::Blocked],
            std::time::Duration::from_millis(60),
        )
        .await;

        let _ = pty::manager().close(&live);

        assert_eq!(
            outcome,
            WaitOutcome::Timeout(None),
            "a live session that has not painted yet must time out, not read as gone"
        );
    }

    /// The clamp, and the reason there is one. `600_000` is the brief's own
    /// over-ask; the assertion below it is the one that matters — the ceiling
    /// is checked against `bash_exec`'s budget constant, not against a second
    /// copy of "150 seconds", so a shrunk foreground budget reddens here
    /// instead of silently letting a blocking call outlive it.
    #[test]
    fn wait_timeout_is_capped_at_the_tool_budget() {
        assert_eq!(
            wait_window(Some(600_000)),
            std::time::Duration::from_millis(WAIT_MAX_TIMEOUT_MS),
            "an over-ask is clamped, not refused"
        );
        assert_eq!(
            wait_window(None),
            std::time::Duration::from_millis(WAIT_DEFAULT_TIMEOUT_MS)
        );
        assert_eq!(
            wait_window(Some(1_500)),
            std::time::Duration::from_millis(1_500),
            "a request under the ceiling is honoured exactly"
        );
    }

    /// See `WAIT_MAX_TIMEOUT_MS`'s doc: this is the constraint the number
    /// exists to satisfy, and it is checked rather than restated.
    #[test]
    fn the_wait_ceiling_stays_under_the_foreground_tool_budget() {
        let budget_ms = crate::builtin_tools::bash_exec::WAIT_MAX_TIMEOUT_SECS * 1_000;
        assert!(
            WAIT_MAX_TIMEOUT_MS < budget_ms,
            "terminal{{wait}} may block for {WAIT_MAX_TIMEOUT_MS} ms, which is not under the \
             {budget_ms} ms a blocking builtin is allowed — the budget wrapper would kill the \
             call before it could report even its own timeout"
        );
    }

    /// An empty `until` can only produce a timeout, so it is refused with the
    /// vocabulary instead of honoured literally for a full window.
    #[tokio::test]
    #[serial_test::parallel(pty_global_manager)]
    async fn wait_refuses_an_empty_until_instead_of_stalling() {
        let out = wait_for_session(Some("s-empty-until"), Some(&[]), None, None).await;
        let message = out.expect_err("an empty `until` is refused");
        assert!(message.contains("until"), "{message}");
    }

    // ── explain ───────────────────────────────────────────────────────────

    /// `explain` names the rule that decided the state and the manifest
    /// revision it came from — G3's mitigation (a stale manifest is invisible
    /// until someone can see which one answered).
    ///
    /// Driven through the OSC progress payload rather than screen text so the
    /// assertion does not depend on chrome that upstream may repaint: the rule
    /// id, its region and the state it carries all come from `grok.toml`.
    #[test]
    fn explain_names_the_matched_rule_and_manifest_version() {
        let screen = crate::gateway::pty::manager::DetectionInputs {
            text: String::new(),
            title: String::new(),
            osc_progress: "4;1;-1".to_string(),
        };
        let out = explain_detection(
            "s-explain",
            agent_detect::identify_agent("grok"),
            None,
            &screen,
        );

        let rule = out
            .matched_rule
            .expect("the osc-progress payload matches a grok rule");
        assert_eq!(rule.id, "osc_progress_working");
        assert_eq!(rule.region, "osc_progress");
        assert_eq!(
            out.state,
            aleph_protocol::runtime::RuntimeAgentState::Working
        );
        assert_eq!(out.agent.as_deref(), Some("grok"));
        assert_eq!(out.source, Some("bundled"));
        assert_eq!(
            out.manifest_version,
            agent_detect::manifest_version(
                agent_detect::identify_agent("grok").expect("grok is an agent")
            ),
            "the version reported must be the one the loaded manifest declares"
        );
        assert_eq!(
            out.inputs.osc_progress, "4;1;-1",
            "the explanation has to show what the engine was fed, or `no rule matched` and \
             `the input never arrived` are the same sentence"
        );
    }

    /// The two absences are different sentences. A session with no row has
    /// never been looked at; a row whose program is not an agent has been.
    #[test]
    fn explain_tells_an_unsampled_session_from_an_unrecognised_program() {
        let screen = crate::gateway::pty::manager::DetectionInputs {
            text: String::new(),
            title: String::new(),
            osc_progress: String::new(),
        };

        let never_sampled = explain_detection("s-none", None, None, &screen);
        assert!(
            never_sampled
                .reason
                .as_deref()
                .expect("an unexplainable state carries a reason")
                .contains("no row"),
            "{:?}",
            never_sampled.reason
        );

        let row = aleph_protocol::runtime::RuntimeAgentEntry {
            session_id: "s-vim".to_string(),
            label: "zsh".to_string(),
            cwd: String::new(),
            agent: None,
            program: Some("vim".to_string()),
            state: aleph_protocol::runtime::RuntimeAgentState::Unknown,
            updated_at: 0,
            quiet_since: None,
        };
        let unrecognised = explain_detection("s-vim", None, Some(&row), &screen);
        assert!(
            unrecognised
                .reason
                .as_deref()
                .expect("reason")
                .contains("vim"),
            "the program that WAS found belongs in the sentence: {:?}",
            unrecognised.reason
        );
        assert_eq!(
            unrecognised.state,
            aleph_protocol::runtime::RuntimeAgentState::Unknown,
            "no agent means unknown, never idle"
        );
    }

    /// The wire between the tool and the live screen, which the pure test
    /// above cannot see: cut `PtyManager::detection_inputs` down to empty
    /// strings and this is what goes red.
    ///
    /// The child paints an OSC 0 title and then sleeps, so the assertion is
    /// about a value only the real screen can produce.
    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::parallel(pty_global_manager)]
    async fn explain_reads_the_live_session_screen() {
        use crate::gateway::pty::SpawnOptions;

        let (command, args) = if cfg!(windows) {
            (
                "cmd.exe",
                vec![
                    "/C".to_string(),
                    "echo \x1b]0;ALEPH-EXPLAIN-TITLE\x07 & ping -n 20 127.0.0.1 > NUL".to_string(),
                ],
            )
        } else {
            (
                "sh",
                vec![
                    "-c".to_string(),
                    "printf '\\033]0;ALEPH-EXPLAIN-TITLE\\007'; sleep 20".to_string(),
                ],
            )
        };
        let id = pty::manager()
            .spawn(&SpawnOptions {
                command: Some(command.to_string()),
                args,
                created_by: Some("u-explain".to_string()),
                rows: 10,
                cols: 40,
                ..Default::default()
            })
            .expect("spawn")
            .session_id;

        // The reader thread feeds the screen; poll rather than sleep a fixed
        // amount, the shape every other PTY test in this crate uses.
        let mut seen = String::new();
        let mut found = false;
        for _ in 0..100 {
            let out = explain_session(Some(&id), Some("u-explain")).expect("explain");
            seen = out["inputs"]["title"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            if seen == "ALEPH-EXPLAIN-TITLE" {
                found = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        let _ = pty::manager().close(&id);
        assert!(
            found,
            "explain must read the LIVE screen, not an empty placeholder; title held: {seen:?}"
        );
    }

    /// `explain` is addressed by session id, so it is an id-enumeration
    /// oracle unless it refuses exactly as `read` does.
    #[test]
    #[serial_test::parallel(pty_global_manager)]
    fn explain_of_someone_elses_session_is_refused_like_unknown() {
        use crate::gateway::pty::SpawnOptions;

        let id = pty::manager()
            .spawn(&SpawnOptions {
                created_by: Some("u-owner".to_string()),
                ..Default::default()
            })
            .expect("spawn")
            .session_id;

        let stranger = explain_session(Some(&id), Some("u-someone-else"));
        let unknown = explain_session(Some("does-not-exist"), Some("u-someone-else"));

        let _ = pty::manager().close(&id);

        assert_eq!(stranger, Err(pty::no_such_session(&id)));
        assert_eq!(unknown, Err(pty::no_such_session("does-not-exist")));
    }
}
