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

/// Output envelope shared by every action — same shape as `MoaManageOutput`:
/// a flat `success`/`message`/`data` triple rather than a per-action type,
/// since the actions have nothing in common to factor beyond "did it work"
/// and "here is the payload".
///
/// No count in that sentence on purpose: this doc said "all three" while the
/// enum had five, which is a list that rots (判据 §1/§16). The set is
/// [`TerminalAction`]; read it there.
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
/// by EVERY action rather than once per action (判据 §9 — and deliberately
/// without a number: the previous wording said "all four" while five actions
/// reached it, and a reader who counts four consumers of the ownership
/// predicate does not go looking for the fifth). `owner_admits`'s
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
        // Falsified on 2026-09-04 by replacing this arm with
        // `&WAIT_DEFAULT_UNTIL`: `wait_refuses_an_empty_until_instead_of_stalling`
        // goes red (see the task-D report). Its first version could not —
        // the ownership gate refused the id before this arm ran.
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
mod tests;
