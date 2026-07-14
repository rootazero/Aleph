//! The tool call an approval belongs to.
//!
//! An approval is raised deep inside an `ApprovalRequester` implementation,
//! which only sees `(tool_name, reason)` — no call identity. Clients therefore
//! had to pair a pending approval to a tool row by POSITION, against an
//! unordered `exec.approvals.pending` map: with two concurrent tool calls the
//! card renders under the wrong tool and the user approves something they never
//! read.
//!
//! The tool-dispatch chokepoint (`ScopedToolService::execute`) does know the
//! call identity, so it scopes it here and every approval record built beneath
//! it stamps itself with the id (`ExecApprovalRecord::from_request`). The id is
//! the harness `call.id` — the same string the Panel sees as `ToolStart.tool_id`.

use std::future::Future;

tokio::task_local! {
    /// Harness tool-call id of the tool currently being gated for approval.
    static TOOL_CALL_ID: Option<String>;
}

/// Run `f` with `tool_call_id` as the ambient approval tool call.
pub async fn with_tool_call_id<F: Future>(tool_call_id: Option<String>, f: F) -> F::Output {
    TOOL_CALL_ID.scope(tool_call_id, f).await
}

/// The tool call the current approval belongs to, if any. `None` outside a
/// tool-dispatch approval (cluster node approvals, raw exec commands, tests).
#[must_use]
pub fn current_tool_call_id() -> Option<String> {
    TOOL_CALL_ID.try_with(Clone::clone).ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn id_is_visible_inside_the_scope_and_nowhere_else() {
        assert!(current_tool_call_id().is_none());
        let seen = with_tool_call_id(Some("toolu_01".to_string()), async {
            current_tool_call_id()
        })
        .await;
        assert_eq!(seen.as_deref(), Some("toolu_01"));
        assert!(current_tool_call_id().is_none());
    }

    /// Concurrent approvals must not see each other's id — the whole point of
    /// the pairing key.
    #[tokio::test]
    async fn concurrent_scopes_do_not_leak() {
        let a = tokio::spawn(with_tool_call_id(Some("a".to_string()), async {
            tokio::task::yield_now().await;
            current_tool_call_id()
        }));
        let b = tokio::spawn(with_tool_call_id(Some("b".to_string()), async {
            tokio::task::yield_now().await;
            current_tool_call_id()
        }));
        assert_eq!(a.await.unwrap().as_deref(), Some("a"));
        assert_eq!(b.await.unwrap().as_deref(), Some("b"));
    }
}
