//! Who is acting in **this** tool call.
//!
//! # The bug this exists to fix
//!
//! Every team / collaboration tool used to take its actor identity as a
//! constructor argument, resolved once when `BuiltinToolRegistry` was built.
//! The registry is built at boot from a `BuiltinToolConfig` whose
//! `current_agent_id` field had **no producer anywhere in the tree**, so the
//! `unwrap_or_else(|| "main".to_string())` fallback fired every time and the
//! literal `"main"` was welded into ~20 tools for the process lifetime.
//!
//! The consequences were silent, not loud: `team_member_add` compared
//! `team.leader_id != "main"` no matter which agent was running, so a team led
//! by `researcher` refused its own leader and accepted `main`;
//! `workflow_step_review` filed every approval under `main`; `inbox_read` read
//! `main`'s inbox from inside a worker's turn. Nothing errored — the wrong
//! identity is a perfectly valid identity.
//!
//! # The fix, and why it is shaped this way
//!
//! Identity is resolved **per call**, from the same runtime source
//! `flag_user_correction` uses (see the dispatch arm in
//! `executor::builtin_registry::registry::tool_registry_impl`): the
//! `TURN_CONTEXT` task-local, which
//! [`ScopedToolService::execute`](crate::tools::scoped::ScopedToolService) —
//! the single production tool-dispatch chokepoint — scopes around every tool
//! call. Reading it never crosses a `tokio::spawn` boundary, so it is
//! reliably visible from inside `call`.
//!
//! Tools keep their constructor argument, but it is now only a **fallback**
//! for call sites outside a turn scope (direct construction in tests, RPC
//! faces, background paths). It is no longer the answer.
//!
//! # Known partition-granularity mismatch (deliberate, documented)
//!
//! This returns the **base** agent id (`main`), not the project/user-scoped
//! composite id (`main__u-alice`, `main__proj-…`) that the memory layer writes
//! under via `memory::project_scope::session_write_id`. `flag_user_correction`
//! has the same behaviour, and team membership / task ownership are keyed on
//! the base id — a team roster stores `researcher`, never `researcher__u-bob`.
//! Making this scoped would therefore break every leader comparison in the
//! team layer. If a caller ever needs the scoped form it must compose it
//! explicitly, the same way the `remember` dispatch arm does; do not change
//! the return value here.

/// The agent identity acting in the tool call currently executing.
///
/// Falls back to `boot_fallback` — the id the tool was constructed with —
/// when there is no turn scope, or when the turn scope carries an empty id.
///
/// The empty-string filter matters, and is reachable: most keys are built via
/// `SessionKey::main`, which normalizes `""` to `"main"`, but
/// `routing::resolve::build_session_key` fills `agent_id` straight from its
/// `&str` argument with no normalization. Several tools (`plan_submit`,
/// `plan_resolve`, `lifecycle_resolve_shutdown`) treat an empty actor as
/// "unknown, fall back to the team leader", so an empty turn identity must not
/// be allowed to overwrite a perfectly good constructor value with nothing.
#[must_use]
pub fn acting_agent_id(boot_fallback: &str) -> String {
    crate::tools::turn_context::current_agent_id()
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| boot_fallback.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::session_key::SessionKey;
    use crate::tools::turn_context::{TurnContext, TURN_CONTEXT};

    /// `SessionKey::main` runs the id through `normalize_agent_id`, which turns
    /// `""` into `"main"` — so it cannot build the degenerate key the empty
    /// filter exists for. The struct literal is the shape production actually
    /// reaches it by: `routing::resolve::build_session_key` fills `agent_id`
    /// straight from its `&str` argument, without normalizing.
    fn turn(agent: &str) -> TurnContext {
        TurnContext {
            session_key: SessionKey::Main {
                agent_id: agent.to_string(),
                main_key: crate::routing::session_key::DEFAULT_MAIN_KEY.to_string(),
                epoch: 0,
            },
            run_id: String::new(),
            channel_id: String::new(),
            conversation_id: String::new(),
            caller_role: None,
            channel_tool_permissions: None,
            unattended: false,
            plan_gate: None,
            side_question: false,
        }
    }

    #[tokio::test]
    async fn the_turn_wins_over_the_boot_value() {
        let observed = TURN_CONTEXT
            .scope(turn("researcher"), async { acting_agent_id("main") })
            .await;
        assert_eq!(
            observed, "researcher",
            "the identity of the running turn must beat the one baked in at boot"
        );
    }

    #[test]
    fn outside_a_turn_the_boot_value_stands() {
        assert_eq!(acting_agent_id("main"), "main");
    }

    #[tokio::test]
    async fn an_empty_turn_identity_does_not_erase_the_boot_value() {
        let observed = TURN_CONTEXT
            .scope(turn(""), async { acting_agent_id("leader") })
            .await;
        assert_eq!(observed, "leader");
    }
}
