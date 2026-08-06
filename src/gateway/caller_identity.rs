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
    /// internal) — the gate treats absent role as trusted.
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
}

/// The originating connection's role for the current task, or `None` outside a
/// `CALLER_ROLE` scope (non-gateway / internal runs) — trusted by the gate.
#[must_use]
pub fn current_caller_role() -> Option<String> {
    CALLER_ROLE.try_with(|r| r.clone()).ok().flatten()
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
}
