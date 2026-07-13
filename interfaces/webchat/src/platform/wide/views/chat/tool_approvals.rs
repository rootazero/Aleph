//! Pairing a pending approval with the tool row that is blocked on it.
//!
//! Core's approval record (`exec.approvals.pending`) carries the session and
//! the tool name, but no `tool_id` — the approval gate lives in tool dispatch,
//! which does not see the harness's call id. So the panel pairs them: within
//! the conversation that is waiting, a still-running tool row is matched with a
//! pending approval for a tool of the same name, in arrival order.
//!
//! Positional pairing matters for the parallel case: two `bash` calls in one
//! turn under the Ask tier produce two pending approvals, and each row must own
//! a distinct one — otherwise resolving one card would appear to resolve both.

use super::state::ToolCallEntry;
use crate::state::notifications::PendingApprovalView;
use std::collections::HashMap;

/// Map `tool_id` → the approval that tool call is waiting on.
///
/// Only running rows can be waiting; completed/failed rows are done with the
/// permission question. Approvals from other sessions are ignored — they belong
/// to another conversation (and are still reachable from the bell).
#[must_use]
pub fn match_tool_approvals(
    tools: &[ToolCallEntry],
    session_key: Option<&str>,
    pending: &[PendingApprovalView],
) -> HashMap<String, PendingApprovalView> {
    let Some(session_key) = session_key else {
        return HashMap::new();
    };
    let mut unclaimed: Vec<&PendingApprovalView> = pending
        .iter()
        .filter(|p| p.session_key == session_key)
        .collect();
    let mut out = HashMap::new();
    for tool in tools.iter().filter(|t| t.status == "running") {
        if let Some(pos) = unclaimed.iter().position(|p| p.command == tool.tool_name) {
            let claimed = unclaimed.remove(pos);
            out.insert(tool.tool_id.clone(), claimed.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(id: &str, name: &str, status: &str) -> ToolCallEntry {
        ToolCallEntry {
            tool_id: id.to_string(),
            tool_name: name.to_string(),
            status: status.to_string(),
            duration_ms: None,
            started_at_ms: None,
        }
    }

    fn approval(id: &str, command: &str, session: &str) -> PendingApprovalView {
        PendingApprovalView {
            id: id.to_string(),
            command: command.to_string(),
            agent_id: "main".to_string(),
            session_key: session.to_string(),
            reason: None,
            expires_at_ms: 0,
        }
    }

    #[test]
    fn running_row_claims_the_approval_for_its_tool() {
        let tools = vec![tool("t1", "bash", "running")];
        let pending = vec![approval("a1", "bash", "s1")];
        let m = match_tool_approvals(&tools, Some("s1"), &pending);
        assert_eq!(m.get("t1").map(|a| a.id.as_str()), Some("a1"));
    }

    #[test]
    fn finished_rows_never_claim() {
        let tools = vec![
            tool("t1", "bash", "completed"),
            tool("t2", "bash", "failed"),
        ];
        let pending = vec![approval("a1", "bash", "s1")];
        assert!(match_tool_approvals(&tools, Some("s1"), &pending).is_empty());
    }

    #[test]
    fn other_sessions_approvals_are_not_shown_in_this_conversation() {
        let tools = vec![tool("t1", "bash", "running")];
        let pending = vec![approval("a1", "bash", "other-session")];
        assert!(match_tool_approvals(&tools, Some("s1"), &pending).is_empty());
    }

    #[test]
    fn parallel_same_name_calls_claim_distinct_approvals() {
        // THE reason pairing is positional: one card per blocked call.
        let tools = vec![tool("t1", "bash", "running"), tool("t2", "bash", "running")];
        let pending = vec![approval("a1", "bash", "s1"), approval("a2", "bash", "s1")];
        let m = match_tool_approvals(&tools, Some("s1"), &pending);
        assert_eq!(m.get("t1").map(|a| a.id.as_str()), Some("a1"));
        assert_eq!(m.get("t2").map(|a| a.id.as_str()), Some("a2"));
    }

    #[test]
    fn tool_name_must_match() {
        let tools = vec![tool("t1", "file_write", "running")];
        let pending = vec![approval("a1", "bash", "s1")];
        assert!(match_tool_approvals(&tools, Some("s1"), &pending).is_empty());
    }

    #[test]
    fn no_session_key_yields_nothing() {
        // A conversation with no session key yet cannot own an approval; the
        // bell still surfaces it.
        let tools = vec![tool("t1", "bash", "running")];
        let pending = vec![approval("a1", "bash", "s1")];
        assert!(match_tool_approvals(&tools, None, &pending).is_empty());
    }
}
