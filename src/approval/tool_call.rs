//! The tool call an approval belongs to.
//!
//! An approval is raised deep inside an `ApprovalRequester` implementation,
//! which receives an `ApprovalAction` (the redacted action) but no call
//! identity. Clients therefore had to pair a pending approval to a tool row by
//! POSITION, against an
//! unordered `exec.approvals.pending` map: with two concurrent tool calls the
//! card renders under the wrong tool and the user approves something they never
//! read.
//!
//! The harness Act phase — the one layer that *mints* call identities — scopes
//! the full [`CallIdentity`] around each tool's execute future
//! (`AgentHarness::act` / `act_parallel`), so every gate and approval record
//! built anywhere beneath tool dispatch reads the exact identity ambiently:
//! the confirm gates in `ScopedToolService` stamp their approval cards and
//! session-log decisions with it, and `ExecApprovalRecord::from_request`
//! stamps sandbox-elevation cards raised *mid-execution* (e.g. `bash` asking
//! to escalate) with the tool call that spawned them. `call_id` is the harness
//! `call.id` — the same string the Panel sees as `ToolStart.tool_id`.
//!
//! This replaced the session-log scan (`newest_tool_call`) that recovered the
//! id by newest-`ToolCallRequested`-for-this-name — a heuristic that forced
//! every approval-gated call to claim `Exclusive::Global` so it could never
//! share a parallel batch with a same-name sibling. With the ambient identity
//! exact per call, gated calls surface their real bounded claims and multiple
//! approval cards may pend concurrently, each correlated precisely.

use std::future::Future;

use crate::session::events::TurnId;

/// The harness identity of the tool call currently being dispatched:
/// the turn it belongs to and its `call.id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallIdentity {
    pub turn_id: TurnId,
    pub call_id: String,
}

tokio::task_local! {
    /// Identity of the tool call currently being dispatched, scoped by the
    /// harness Act phase around the whole execute future.
    static CALL_IDENTITY: Option<CallIdentity>;
}

/// Run `f` with `identity` as the ambient tool-call identity.
pub async fn with_call_identity<F: Future>(identity: Option<CallIdentity>, f: F) -> F::Output {
    CALL_IDENTITY.scope(identity, f).await
}

/// The full identity of the tool call currently being dispatched, if any.
/// `None` outside harness tool dispatch (direct `tools.invoke` RPC, cluster
/// node approvals, raw exec commands, tests).
#[must_use]
pub fn current_call_identity() -> Option<CallIdentity> {
    CALL_IDENTITY.try_with(Clone::clone).ok().flatten()
}

/// The tool call the current approval belongs to, if any. Shorthand over
/// [`current_call_identity`] for consumers that only stamp the id.
#[must_use]
pub fn current_tool_call_id() -> Option<String> {
    current_call_identity().map(|i| i.call_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(call_id: &str) -> CallIdentity {
        CallIdentity {
            turn_id: TurnId::nil(),
            call_id: call_id.to_string(),
        }
    }

    #[tokio::test]
    async fn identity_is_visible_inside_the_scope_and_nowhere_else() {
        assert!(current_call_identity().is_none());
        assert!(current_tool_call_id().is_none());
        let seen = with_call_identity(Some(identity("toolu_01")), async {
            (current_call_identity(), current_tool_call_id())
        })
        .await;
        assert_eq!(seen.0, Some(identity("toolu_01")));
        assert_eq!(seen.1.as_deref(), Some("toolu_01"));
        assert!(current_call_identity().is_none());
    }

    /// Concurrent approvals must not see each other's identity — the whole
    /// point of the pairing key, and the invariant that lets approval-gated
    /// calls share a parallel batch.
    #[tokio::test]
    async fn concurrent_scopes_do_not_leak() {
        let a = tokio::spawn(with_call_identity(Some(identity("a")), async {
            tokio::task::yield_now().await;
            current_tool_call_id()
        }));
        let b = tokio::spawn(with_call_identity(Some(identity("b")), async {
            tokio::task::yield_now().await;
            current_tool_call_id()
        }));
        assert_eq!(a.await.unwrap().as_deref(), Some("a"));
        assert_eq!(b.await.unwrap().as_deref(), Some("b"));
    }
}
