//! Who a record is attributed to when the turn's session key is not the actor.
//!
//! The ledger normally reads the acting agent off `TURN_CONTEXT`, which the
//! tool chokepoint scopes from the running turn. That is correct for every
//! agent that owns its own turn — a main session, a cron task, a team member
//! (`SessionKey::task(agent_id, "team", …)`).
//!
//! It is **wrong for a subagent**. A subagent executes through its *parent's*
//! `ScopedToolService`, so it inherits the parent's `TURN_CONTEXT`; worse,
//! [`SessionKey::Subagent`](crate::routing::session_key::SessionKey) resolves
//! `agent_id()` by delegating to `parent_key`, so even the child's own session
//! key names the parent. Everything a delegated role did was therefore filed
//! on the chain of whoever spawned it — which makes "this agent's record"
//! false for exactly the agents a delegation model exists to isolate.
//!
//! The acting role *is* known one layer up: the spawner already wraps the
//! parent service in an
//! [`AllowlistToolService`](crate::agents::allowlist_tool_service) built from
//! the child's `AgentDef`, and already labels the child's provider usage and
//! tool signals with that same `agent_def.id`. This task-local carries that id
//! down to the chokepoint.
//!
//! ## Why a task-local and not a field
//!
//! Same reason `TURN_CONTEXT` is one, and with the same constraint: it must be
//! scoped by the **immediate caller** of the inner service, never across a
//! `tokio::spawn`. The harness Act phase spawns a task per concurrent tool
//! call, so a scope opened around the child harness's *run* would be invisible
//! inside those tasks. `AllowlistToolService::execute*` runs inside each of
//! them and delegates by direct await, so a scope opened there is visible for
//! the whole dispatch — including through `McpScopedToolService`, which layers
//! between the two.
//!
//! Nesting behaves as the innermost scope winning, which is what a delegation
//! chain wants: the deepest role that actually issued the call owns the record.

use std::future::Future;

tokio::task_local! {
    /// The agent id a ledger record produced under this scope belongs to.
    static LEDGER_ACTOR: String;
}

/// The overriding actor for the current dispatch, if one is scoped.
///
/// `None` — the overwhelmingly common case — means the turn's own session key
/// is the actor and the chokepoint should use it.
#[must_use]
pub fn current_actor() -> Option<String> {
    LEDGER_ACTOR.try_with(Clone::clone).ok()
}

/// Attribute every ledger record produced by `fut` to `agent_id`.
///
/// A blank id is ignored rather than minting a key for the empty string: an
/// unnamed role is not an identity, and the turn's own attribution is the
/// honest fallback.
pub async fn as_actor<F: Future>(agent_id: &str, fut: F) -> F::Output {
    if agent_id.is_empty() {
        return fut.await;
    }
    LEDGER_ACTOR.scope(agent_id.to_string(), fut).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unscoped_reads_none() {
        assert_eq!(current_actor(), None);
    }

    #[tokio::test]
    async fn scope_is_visible_to_the_awaited_future() {
        let seen = as_actor("reviewer", async { current_actor() }).await;
        assert_eq!(seen.as_deref(), Some("reviewer"));
        assert_eq!(
            current_actor(),
            None,
            "the scope must not leak past the await"
        );
    }

    #[tokio::test]
    async fn the_innermost_role_owns_the_record() {
        // A delegation chain attributes to the role that actually issued the
        // call, not to the one that started the chain.
        let seen = as_actor("planner", async {
            as_actor("reviewer", async { current_actor() }).await
        })
        .await;
        assert_eq!(seen.as_deref(), Some("reviewer"));
    }

    #[tokio::test]
    async fn a_blank_role_falls_back_to_the_turn() {
        // Minting a signing key for the empty string would create an identity
        // nobody can be held to.
        let seen = as_actor("", async { current_actor() }).await;
        assert_eq!(seen, None);
    }
}
