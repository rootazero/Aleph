//! `TURN_CONTEXT` task-local — carries the originating channel + session of
//! the agent turn currently executing a tool, so human-in-the-loop tools can
//! route a prompt back to the user.
//!
//! Scoped by [`ScopedToolService::execute`](crate::tools::scoped::ScopedToolService)
//! — the single production tool-dispatch chokepoint — for the duration of
//! every tool call. Reading it never crosses a `tokio::spawn` boundary, so
//! unlike `SESSION_ID` (which the gateway path never scopes) it is reliably
//! visible inside tool execution.
//!
//! Read by:
//! - the approval requester adapter (sandbox escalations + `requires_confirmation`)
//! - the `ask_user` clarification tool

use tokio::task_local;

use crate::routing::session_key::SessionKey;

/// Routing context of the agent turn currently executing a tool.
#[derive(Clone, Debug)]
pub struct TurnContext {
    /// Session key of the running turn — the key HITL managers register under.
    pub session_key: SessionKey,
    /// Gateway run id of the running turn. Lets the config-approval gate emit a
    /// run-scoped "waiting for operator approval" notice on the requester's own
    /// output stream. Empty for non-gateway runs (cron, internal, tests) — the
    /// notice is then skipped (best-effort).
    pub run_id: String,
    /// Originating channel id (e.g. `telegram`). Empty for non-channel turns
    /// (cron, webhook, internal).
    pub channel_id: String,
    /// Originating conversation id. Empty for non-channel turns.
    pub conversation_id: String,
    /// Originating gateway connection's authorization role (`"operator"` /
    /// `"guest"`), stamped at run start from `CALLER_ROLE`. `None` for
    /// non-gateway runs (cron, internal) and for the local no-auth daemon —
    /// both treated as trusted by the config-tier gate.
    pub caller_role: Option<String>,
    /// The originating channel's `tool_permissions` override layer, as the JSON
    /// string carried in run metadata under `CHANNEL_TOOL_PERMISSIONS_KEY`.
    /// `None` for non-channel turns. Delegation tools (`session_send`) forward
    /// this so a child run inherits the same restrictive deny layer the channel
    /// turn ran under — without it, a guest channel could bypass its own
    /// `deny` override by delegating (see `sessions/send_tool.rs`).
    pub channel_tool_permissions: Option<String>,
    /// True when this run is UNATTENDED — no human on any surface (a goal /
    /// loop continuation, heartbeat, A2A delegation, cron with no origin
    /// channel). Mirrors the `UNATTENDED_KEY` run-metadata marker the
    /// execution engine already feeds `ScopedToolService::with_unattended`.
    /// Carried here so delegation tools (`session_send`) can propagate it to
    /// wait-mode children too. Losing it in propagation got more expensive on
    /// 2026-08-28, not less: the child then reads as ATTENDED, and an attended
    /// approval has no deadline, so instead of failing closed instantly it now
    /// parks until the run's own wall clock — waiting on a human who is not
    /// there, because the parent already knew there was none.
    pub unattended: bool,
    /// The plan → build handoff cell, when this turn resolved to
    /// [`ExecTier::Plan`](crate::config::types::policies::ExecTier::Plan) and is
    /// not a `/btw` side question; `None` otherwise (and then everything below
    /// is byte-identical to a build with no plan mode at all).
    ///
    /// The side-question exception is deliberate, not an omission: a side
    /// question always resolves to `Plan`, and this cell is the one thing that
    /// can move a turn's tier mid-run. Its ceiling has nothing to hand back to,
    /// so it is withheld — see `resolve_turn_permissions`. Do not reintroduce a
    /// `Plan ⇒ Some` assertion at any consumer.
    ///
    /// It rides HERE rather than as a fourteenth parameter of
    /// `build_request_tool_service` because the run builds the tool service
    /// **twice** — once for itself and once as the parent view handed to
    /// spawned children — and both calls already pass this context. One
    /// `Arc`, therefore one gate: a second construction site would have given
    /// the children a gate that no approval ever opens.
    ///
    /// Read by `ScopedToolService::effective_exec_tier` (the enforcement
    /// chokepoint holds this struct) and flipped by `scratchpad`'s
    /// `request_approval` (which reaches it through the [`TURN_CONTEXT`]
    /// task-local this same struct is scoped into around every dispatch).
    pub plan_gate: Option<std::sync::Arc<crate::tools::plan_gate::PlanGate>>,
    /// This turn is a `/btw` side question.
    ///
    /// Minted beside `plan_gate` in `resolve_turn_permissions` — the one place
    /// that already resolves per-turn permission facts. Deliberately NOT
    /// derived from the session key's shape: matching an `ephemeral_id`
    /// prefix would be a second derivation of a fact the request already
    /// carries, and a string match is exactly the kind of predicate that
    /// keeps working right up until someone renames the prefix.
    pub side_question: bool,
}

/// Canonical operator-role predicate for a raw `caller_role` string. `None`
/// (absent = loopback/local) and `"operator"` are operator; everything else
/// (chat-tier channels default to `"guest"`) is not.
#[must_use]
pub fn role_is_operator(role: Option<&str>) -> bool {
    matches!(role, None | Some("operator"))
}

impl TurnContext {
    /// True when the turn has a reachable channel to deliver a prompt to.
    /// HITL tools must deny / fail gracefully when this is false.
    #[must_use]
    pub const fn is_channel_routable(&self) -> bool {
        !self.channel_id.is_empty() && !self.conversation_id.is_empty()
    }

    /// True when the originating connection may mutate Aleph's own config.
    /// Absent role = trusted local/internal run; `"operator"` = config tier;
    /// any other value (e.g. `"guest"`) = chat tier (gated).
    #[must_use]
    pub fn caller_is_operator(&self) -> bool {
        role_is_operator(self.caller_role.as_deref())
    }
}

task_local! {
    /// The routing context of the agent turn currently executing a tool.
    pub static TURN_CONTEXT: TurnContext;
}

tokio::task_local! {
    /// The exec tier the running turn resolved to (`ask` / `auto` / `full`;
    /// `plan` composes through the plan gate instead), scoped by
    /// `ScopedToolService::execute` — the same chokepoint that scopes
    /// [`TURN_CONTEXT`] — from `ScopedToolService::effective_exec_tier`.
    ///
    /// Sole consumer: [`crate::approval::lift_ask_under_full_tier`], which
    /// lets a policy-level `Ask` resolve to `Allow` when the operator already
    /// answered the question by picking the Full tier for this conversation.
    /// Unset (cron, internal, tests, any non-gateway run) reads as `None`,
    /// and `None` lifts nothing — the fail-closed default.
    pub static TURN_EXEC_TIER: crate::config::types::policies::ExecTier;
}

/// The exec tier of the turn currently executing a tool, when the dispatch
/// chokepoint published one. See [`TURN_EXEC_TIER`].
#[must_use]
pub fn current_exec_tier() -> Option<crate::config::types::policies::ExecTier> {
    TURN_EXEC_TIER.try_with(|t| *t).ok()
}

tokio::task_local! {
    /// Raw channel user id of the human whose message triggered the current
    /// run — the approval "originator".
    ///
    /// Unlike [`TURN_CONTEXT`] (scoped per-agent at the tool-dispatch
    /// chokepoint), this is one value for the whole run tree, so it is scoped
    /// once at the run-loop boundary alongside `FsScope` / agent-id /
    /// project-root (`run_agent_loop`). `None` for non-channel runs (cron,
    /// internal, Panel) and for spawned subagents that do not re-publish it —
    /// a missing originator makes the approval-originator gate a no-op, so the
    /// behaviour degrades to the pre-existing "any paired user" rule. Read by
    /// the channel approval bridge so a pending record can carry it to the
    /// button-callback gate (`ManagerCallbackSink::handle_callback`).
    pub static TURN_ORIGINATOR: Option<String>;
}

/// Run `fut` with `originator` visible to [`current_originator`] for the
/// lifetime of the future. Publishing `None` explicitly keeps a nested run from
/// leaking an outer run's originator.
pub async fn with_originator<F, T>(originator: Option<String>, fut: F) -> T
where
    F: std::future::Future<Output = T>,
{
    TURN_ORIGINATOR.scope(originator, fut).await
}

/// The originating channel user id of the current run, or `None` outside a
/// scope / for non-channel runs.
#[must_use]
pub fn current_originator() -> Option<String> {
    TURN_ORIGINATOR.try_with(Clone::clone).ok().flatten()
}

/// Returns the current turn's routing context, or `None` outside a turn scope.
#[must_use]
pub fn current_turn_context() -> Option<TurnContext> {
    TURN_CONTEXT.try_with(|t| t.clone()).ok()
}

/// The plan → build handoff cell of the turn currently executing a tool.
///
/// `None` outside a scoped turn and on every turn that is not planning — the
/// two are indistinguishable here on purpose, because the one caller
/// (`scratchpad`'s `request_approval`) treats both the same way: there is no
/// read-only gate to lift, so approval stays the advisory checkpoint it has
/// always been.
#[must_use]
pub fn current_plan_gate() -> Option<std::sync::Arc<crate::tools::plan_gate::PlanGate>> {
    TURN_CONTEXT
        .try_with(|t| t.plan_gate.clone())
        .ok()
        .flatten()
}

/// Agent id of the turn currently executing a tool, or `None` outside a
/// scoped turn (direct calls, non-gateway paths, tests).
///
/// Agent-scoped tools (`memory_search` workspace + smart-recall resolution)
/// MUST prefer this over the shared `Arc<RwLock<…>>` registry handles: those
/// handles are process-global and rewritten at every run start, so a
/// concurrent run of another agent can overwrite them mid-turn and the tool
/// would bind to the wrong agent's workspace/profile.
#[must_use]
pub fn current_agent_id() -> Option<String> {
    TURN_CONTEXT
        .try_with(|t| t.session_key.agent_id().to_string())
        .ok()
}

/// Session key (wire form) of the turn currently executing a tool, or `None`
/// outside a scoped turn (direct calls, non-gateway paths, tests).
///
/// Session-binding tools (`loop`, `goal`, `scratchpad`, `strategy`,
/// `memory_search` scope=current_session) MUST prefer this over the shared
/// `Arc<RwLock<String>>` registry handle: the handle is process-global and
/// rewritten at every run start, so a concurrent run of another agent can
/// overwrite it mid-turn and the tool would bind to the wrong session.
#[must_use]
pub fn current_session_key() -> Option<String> {
    TURN_CONTEXT
        .try_with(|t| t.session_key.to_key_string())
        .ok()
}

#[cfg(test)]
mod caller_tier_tests {
    use super::*;
    use crate::routing::session_key::SessionKey;

    fn ctx(role: Option<&str>) -> TurnContext {
        TurnContext {
            session_key: SessionKey::main("t"),
            run_id: String::new(),
            channel_id: String::new(),
            conversation_id: String::new(),
            caller_role: role.map(String::from),
            channel_tool_permissions: None,
            unattended: false,
            plan_gate: None,
            side_question: false,
        }
    }

    #[test]
    fn operator_and_local_are_operator() {
        assert!(ctx(Some("operator")).caller_is_operator());
        assert!(
            ctx(None).caller_is_operator(),
            "no role = trusted local/internal run"
        );
    }

    #[test]
    fn chat_tier_is_not_operator() {
        assert!(!ctx(Some("guest")).caller_is_operator());
    }

    #[test]
    fn role_is_operator_table() {
        assert!(role_is_operator(None), "absent role = local/operator");
        assert!(role_is_operator(Some("operator")));
        assert!(!role_is_operator(Some("guest")));
        assert!(!role_is_operator(Some("chat")));
    }

    /// Pin (spec §4.6, P0 identity foundation, Task 7): a `"member"` caller
    /// role — the role `resolve_connection_identity` stamps for a device
    /// bound to a `UserRole::Member` user — must NOT satisfy the operator
    /// predicate. This is what bounds member tool execution by the EXISTING
    /// per-tool operator gate (`ScopedToolService::check_operator_gate` in
    /// `src/tools/scoped/dispatch.rs`, which calls
    /// `TurnContext::caller_is_operator()`) without that gate needing any
    /// member-specific code of its own. If this assertion ever fails, the
    /// gate has silently loosened for every OPERATOR_TOOLS-gated tool.
    #[test]
    fn member_role_is_not_operator() {
        assert!(!ctx(Some("member")).caller_is_operator());
        assert!(!role_is_operator(Some("member")));
    }
}
