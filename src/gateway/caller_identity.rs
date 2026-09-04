//! `CALLER_ROLE` task-local — carries the originating gateway connection's
//! authorization role across the in-task hop from the WS dispatch loop into
//! [`AgentRunManager::start_run`](crate::gateway::handlers::agent::AgentRunManager),
//! which copies it into the run's metadata BEFORE `tokio::spawn`. From there it
//! rides [`TurnContext`](crate::tools::turn_context::TurnContext) to the
//! tool-dispatch config-tier gate.
//!
//! Scoped only around `process_request` in the dispatch path
//! (`server::handler`). Never crosses the run's spawn boundary (`start_run` reads
//! it while still in-task). Unset for non-gateway callers (cron, internal).
//!
//! Multi-user role model (P0 identity foundation, spec §4): the gateway
//! resolves a `(user, role)` pair per connection at `connect` —
//! [`resolve_connection_identity`](crate::gateway::handlers::connect::resolve_connection_identity),
//! called from `server::handler::resolve_stamped_identity` — and the WS
//! dispatch loop scopes `CALLER_ROLE` / `CALLER_USER` / `CALLER_IS_LOOPBACK`
//! from that resolution around every `process_request` call, at both dispatch
//! stations (`do_lane_dispatch` and the idempotency `Proceed` arm). Loopback
//! and any authorized-but-unbound credential (legacy shared token, a pre-v14
//! device row with no `user_id`) still resolve to the implicit owner as
//! `"operator"` — the single-user zero-change guarantee — but a device bound
//! to a `Member` user role resolves to `"member"`, and a device bound to a
//! deactivated user, or one whose `user_id` dangles (points at a row no
//! longer in `users`), is walled to `"guest"` / no user rather than
//! defaulting to owner (fail-closed on any store error or broken link, never
//! a silent grant of full authority). The task-local is the single source of
//! truth the config-tier tool gate (`TurnContext::caller_is_operator`,
//! `role_is_operator`) and the admin-method gate (`method_admin.rs`, enforced
//! once inside `process_request`) both read.

use tokio::task_local;

task_local! {
    /// Originating connection role — `Some("operator")`, `Some("member")`, or
    /// `Some("guest")`, resolved per connection by
    /// [`resolve_connection_identity`](crate::gateway::handlers::connect::resolve_connection_identity)
    /// (single-user deployments always resolve to `"operator"`, the
    /// zero-change guarantee). `None` for non-gateway callers (cron,
    /// internal).
    ///
    /// Readers of `None` do NOT all agree on what it means, by design —
    /// naming which is which because this is the highest-traffic doc on the
    /// task-local: [`role_is_operator`](crate::tools::turn_context::role_is_operator)
    /// still treats an absent role as trusted (its own doc: "Absent role =
    /// trusted local/internal run" — unchanged, deliberate, documented
    /// there), but [`caller_may_choose_directory`] refuses an absent role
    /// unless the connection is loopback (narrowed in `546984c2b`). Check the
    /// specific predicate's own doc before assuming either behaviour.
    pub static CALLER_ROLE: Option<String>;

    /// Whether the originating gateway connection's peer is a loopback address
    /// (`127.0.0.0/8`, `::1`). Scoped alongside [`CALLER_ROLE`] in the WS
    /// dispatch loop from the connection's peer IP. `false` outside a scope
    /// (non-gateway / internal runs). The desktop App's local Panel is always
    /// loopback, so this is the signal the project-folder gate keys on to allow
    /// a working-directory override without weakening the LAN trust boundary.
    pub static CALLER_IS_LOOPBACK: bool;

    /// The authenticated user behind the originating connection (`users.user_id`),
    /// resolved once at `connect` and scoped alongside [`CALLER_ROLE`] in the WS
    /// dispatch loop. `None` outside a scope (cron, internal) and for walled
    /// connections. Loopback resolves to the implicit owner.
    pub static CALLER_USER: Option<String>;

    /// The originating WebSocket connection's id (`"{peer_addr}"`, minted once
    /// per socket in `server::handler::handle_connection`), scoped alongside
    /// [`CALLER_ROLE`] in `dispatch_with_caller_context`. `None` outside a
    /// scope (cron, internal) and for non-gateway callers.
    ///
    /// Exists for exactly one consumer so far: `pty.resize`, which must key
    /// its per-connection viewport constraint (`PtyManager::note_viewport`)
    /// on *something the caller does not get to pick* — unlike a
    /// caller-supplied `client_id`, this cannot be forged to steal or
    /// impersonate another connection's viewport slot. The same id is used
    /// to release that constraint on disconnect (`PtyManager::release_conn`,
    /// called from the connection-teardown block alongside the cleanup for
    /// subscriptions / reverse-RPC / node registry / presence), which is the
    /// other half of why this needs to be the real transport-level
    /// connection id rather than an application-chosen string: a disconnect
    /// is not a `pty.close`, and nothing else fires when a tab is closed or
    /// a client crashes.
    pub static CALLER_CONN_ID: Option<String>;
}

/// The originating connection's role for the current task, or `None` outside a
/// `CALLER_ROLE` scope (non-gateway / internal runs) — trusted by the gate.
#[must_use]
pub fn current_caller_role() -> Option<String> {
    CALLER_ROLE.try_with(|r| r.clone()).ok().flatten()
}

/// Whether this call is subject to the member restriction — the ONE predicate
/// `method_admin`'s gate is enforced by, exposed so nothing has to re-derive it.
///
/// `None` (internal / cron / in-process test) and `"operator"` are both
/// unrestricted; `"guest"` never reaches a non-`connect` method (the login wall
/// refuses it first), so the only restricted answer is `"member"`.
///
/// # Why this is not [`visible_owner_filter`](crate::gateway::visibility::visible_owner_filter)
///
/// That predicate answers "whose ROWS may I see" and is keyed on `CALLER_USER`,
/// which a second `UserRole::Admin` principal also carries (their own id rather
/// than `OWNER_USER_ID` — see `resolve_connection_identity`). Using it to decide
/// whether to withhold server-global CONFIG would strip a second operator of the
/// very fields their settings pages exist to edit. Role and ownership are
/// different questions; a surface narrowing on privilege must ask this one.
#[must_use]
pub fn caller_is_member() -> bool {
    current_caller_role().as_deref() == Some("member")
}

/// Whether the current task originated from a loopback (local-machine)
/// connection. `false` outside a `CALLER_IS_LOOPBACK` scope.
#[must_use]
pub fn current_caller_is_loopback() -> bool {
    CALLER_IS_LOOPBACK.try_with(|b| *b).unwrap_or(false)
}

/// The authenticated user id for the current task, or `None` outside a scope.
#[must_use]
pub fn current_caller_user() -> Option<String> {
    CALLER_USER.try_with(|u| u.clone()).ok().flatten()
}

/// The originating WebSocket connection id for the current task, or `None`
/// outside a [`CALLER_CONN_ID`] scope (non-gateway / internal callers, and
/// any run whose work has crossed a `tokio::spawn` boundary — this task-local
/// does not follow a spawned task, same as the other three in this module).
#[must_use]
pub fn current_caller_conn_id() -> Option<String> {
    CALLER_CONN_ID.try_with(|c| c.clone()).ok().flatten()
}

/// May this connection start a run **as** the agent whose `allowed_users` list
/// this is?
///
/// The twin of [`caller_may_choose_directory`] on the other caller-supplied
/// run parameter. That one exists because `project_root` named a directory
/// with nothing checking it; this one exists because `agent_id` named an
/// **identity** with nothing checking it — and an identity is the axis
/// `tool_permissions` is keyed on, so choosing the agent chose the permission
/// set. A caller denied a tool under one agent could name another on the very
/// same `chat.send` and inherit its allowances.
///
/// # Why the session check does not already cover this
///
/// `visibility::existing_session_is_visible` asks "is this SESSION mine", and
/// naming a different agent produces a *brand-new* session key, which that
/// predicate admits **by design** (it is the ordinary first message of a new
/// conversation). Two questions, two predicates: the session one guards
/// reading someone else's transcript, this one guards borrowing someone
/// else's authority.
///
/// Takes the list rather than reading config so the caller passes the list off
/// the object the runtime actually resolved (`AgentInstanceConfig`), not a
/// second lookup that could answer about a different agent.
#[must_use]
pub fn caller_may_act_as_agent(allowed_users: Option<&[String]>) -> bool {
    crate::config::types::agent_admits_user(allowed_users, current_caller_user().as_deref())
}

/// Whether a caller may point the server at an arbitrary folder.
///
/// The ambient form reads the two task-locals; use it from an RPC handler,
/// where the gateway scopes them per request.
///
/// **Do not call it from inside a run.** `CALLER_ROLE` is dead past the
/// `tokio::spawn` every run crosses, so `role` is `None` there, and `None`
/// used to take the admitted arm — the gate was constant-true for exactly the
/// callers it most needed to judge (tool face, cron, A2A). A run's role is not
/// lost, it is stamped into `request.metadata` and enforced by
/// `ScopedToolService`; a tool face must ask that object and pass the answer to
/// [`caller_may_choose_directory_as`] rather than derive a second one.
///
/// Desktop App: the local Panel runs over loopback, so any loopback connection
/// passes regardless of pairing tier — on the desktop the local operator IS the
/// user. Remote LAN connections stay gated, so opening
/// `[gateway] host = "0.0.0.0"` does not hand arbitrary working-directory
/// selection to every device on the network.
#[must_use]
pub fn caller_may_choose_directory() -> bool {
    caller_may_choose_directory_as(
        current_caller_role().as_deref(),
        current_caller_is_loopback(),
    )
}

/// [`caller_may_choose_directory`] with the actor supplied explicitly.
///
/// `role` is the caller's connection tier: `Some("operator")` is config tier,
/// any other `Some` is chat tier, and `None` means "this surface could not
/// resolve a role at all" — refused unless the connection is loopback. That
/// refusal is the whole point of splitting this out: the ambient form's `None`
/// arm was admitting every spawned run.
#[must_use]
pub fn caller_may_choose_directory_as(role: Option<&str>, is_loopback: bool) -> bool {
    let is_config_tier = matches!(role, Some("operator"));
    is_config_tier || is_loopback
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn scope_round_trips() {
        let seen = CALLER_ROLE
            .scope(Some("guest".to_string()), async { current_caller_role() })
            .await;
        assert_eq!(seen.as_deref(), Some("guest"));
    }

    #[tokio::test]
    async fn unset_is_none() {
        assert_eq!(current_caller_role(), None);
    }

    #[tokio::test]
    async fn caller_user_scope_round_trips() {
        let seen = CALLER_USER
            .scope(Some("u-alice".to_string()), async { current_caller_user() })
            .await;
        assert_eq!(seen.as_deref(), Some("u-alice"));
        assert_eq!(current_caller_user(), None); // unset outside a scope
    }

    #[tokio::test]
    async fn caller_conn_id_scope_round_trips() {
        let seen = CALLER_CONN_ID
            .scope(Some("127.0.0.1:5555".to_string()), async {
                current_caller_conn_id()
            })
            .await;
        assert_eq!(seen.as_deref(), Some("127.0.0.1:5555"));
        assert_eq!(current_caller_conn_id(), None); // unset outside a scope
    }

    /// The ambient wrapper outside any scope reads `None`/`false` from both
    /// task-locals — exactly the shape `caller_may_choose_directory_as` now
    /// refuses. This used to assert the opposite (cron/A2A/in-process callers
    /// kept their pre-multi-user freedom); that was the gate being
    /// constant-true for exactly the callers it most needed to judge.
    #[tokio::test]
    async fn an_unscoped_ambient_caller_may_not_choose_a_directory() {
        assert!(!caller_may_choose_directory());
    }

    #[tokio::test]
    async fn a_remote_chat_tier_caller_may_not() {
        let allowed = CALLER_ROLE
            .scope(Some("member".to_string()), async {
                CALLER_IS_LOOPBACK
                    .scope(false, async { caller_may_choose_directory() })
                    .await
            })
            .await;
        assert!(!allowed);
    }

    /// The desktop App's own Panel: chat-tier by pairing, but local. The
    /// loopback arm is what keeps zero-config single-machine use working.
    #[tokio::test]
    async fn the_same_caller_over_loopback_may() {
        let allowed = CALLER_ROLE
            .scope(Some("member".to_string()), async {
                CALLER_IS_LOOPBACK
                    .scope(true, async { caller_may_choose_directory() })
                    .await
            })
            .await;
        assert!(allowed);
    }

    #[tokio::test]
    async fn a_remote_operator_may() {
        let allowed = CALLER_ROLE
            .scope(Some("operator".to_string()), async {
                CALLER_IS_LOOPBACK
                    .scope(false, async { caller_may_choose_directory() })
                    .await
            })
            .await;
        assert!(allowed);
    }

    /// The predicate's whole reason for existing is to separate a caller who
    /// may point the server at an arbitrary folder from one who may not. An
    /// arm that is constant-true is not a gate, and `None` — the value every
    /// spawned run sees — used to take exactly that arm.
    #[test]
    fn an_unknown_role_may_not_choose_a_directory_without_loopback() {
        assert!(
            !caller_may_choose_directory_as(None, false),
            "a caller with no connection role and no loopback must be refused: \
             None is what every tool call inside a spawned run sees, so admitting \
             it makes the gate constant-true exactly where it matters"
        );
    }

    #[test]
    fn the_three_admitted_shapes_are_unchanged() {
        assert!(
            caller_may_choose_directory_as(Some("operator"), false),
            "an operator-tier connection still chooses"
        );
        assert!(
            caller_may_choose_directory_as(None, true),
            "a loopback caller still chooses — this is the zero-config desktop \
             install, and narrowing it would break single-user deployments"
        );
        assert!(
            !caller_may_choose_directory_as(Some("guest"), false),
            "a chat-tier connection is still refused"
        );
    }
}
