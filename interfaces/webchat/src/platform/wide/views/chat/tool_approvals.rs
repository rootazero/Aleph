//! Pairing a pending approval with the tool row that is blocked on it.
//!
//! Core stamps the harness call id on the approval record (`tool_call_id`), so
//! the pairing is an exact key match against the row's `tool_id` — never a
//! guess. Positional pairing (match the Nth approval for a tool name to the Nth
//! running row) was a safety defect: `exec.approvals.pending` is not ordered by
//! the panel's row order, so with two concurrent calls the card could render
//! under a tool the operator did not read.
//!
//! An approval with no `tool_call_id` has no owning tool row (cluster-node and
//! raw exec-command approvals are raised below the dispatch gate). Those are
//! left unattached and stay reachable from the notification bell.

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
    let mut out = HashMap::new();
    for tool in tools.iter().filter(|t| t.status == "running") {
        if let Some(approval) = pending.iter().find(|p| {
            p.session_key == session_key && p.tool_call_id.as_deref() == Some(tool.tool_id.as_str())
        }) {
            out.insert(tool.tool_id.clone(), approval.clone());
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

    fn approval(
        id: &str,
        command: &str,
        session: &str,
        tool_call_id: Option<&str>,
    ) -> PendingApprovalView {
        PendingApprovalView {
            id: id.to_string(),
            command: command.to_string(),
            agent_id: "main".to_string(),
            session_key: session.to_string(),
            tool_call_id: tool_call_id.map(str::to_string),
            reason: None,
            expires_at_ms: 0,
            allowed_decisions: Vec::new(),
        }
    }

    #[test]
    fn running_row_claims_the_approval_stamped_with_its_call_id() {
        let tools = vec![tool("t1", "bash", "running")];
        let pending = vec![approval("a1", "bash", "s1", Some("t1"))];
        let m = match_tool_approvals(&tools, Some("s1"), &pending);
        assert_eq!(m.get("t1").map(|a| a.id.as_str()), Some("a1"));
    }

    #[test]
    fn finished_rows_never_claim() {
        let tools = vec![
            tool("t1", "bash", "completed"),
            tool("t2", "bash", "failed"),
        ];
        let pending = vec![
            approval("a1", "bash", "s1", Some("t1")),
            approval("a2", "bash", "s1", Some("t2")),
        ];
        assert!(match_tool_approvals(&tools, Some("s1"), &pending).is_empty());
    }

    #[test]
    fn other_sessions_approvals_are_not_shown_in_this_conversation() {
        let tools = vec![tool("t1", "bash", "running")];
        let pending = vec![approval("a1", "bash", "other-session", Some("t1"))];
        assert!(match_tool_approvals(&tools, Some("s1"), &pending).is_empty());
    }

    #[test]
    fn parallel_same_name_calls_pair_by_id_not_by_arrival_order() {
        // THE defect this replaces: the pending list is not in row order, so a
        // positional match could put a2's card under t1. Each row must show the
        // approval stamped with its own call id, whatever order they arrive in.
        let tools = vec![tool("t1", "bash", "running"), tool("t2", "bash", "running")];
        let pending = vec![
            approval("a2", "bash", "s1", Some("t2")),
            approval("a1", "bash", "s1", Some("t1")),
        ];
        let m = match_tool_approvals(&tools, Some("s1"), &pending);
        assert_eq!(m.get("t1").map(|a| a.id.as_str()), Some("a1"));
        assert_eq!(m.get("t2").map(|a| a.id.as_str()), Some("a2"));
    }

    #[test]
    fn approval_without_a_call_id_is_never_attached_to_a_row() {
        // Cluster-node / raw exec-command approvals: no owning tool row. They
        // must not be fabricated onto one — the bell is their surface.
        let tools = vec![tool("t1", "bash", "running")];
        let pending = vec![approval("a1", "bash", "s1", None)];
        assert!(match_tool_approvals(&tools, Some("s1"), &pending).is_empty());
    }

    #[test]
    fn no_session_key_yields_nothing() {
        // A conversation with no session key yet cannot own an approval; the
        // bell still surfaces it.
        let tools = vec![tool("t1", "bash", "running")];
        let pending = vec![approval("a1", "bash", "s1", Some("t1"))];
        assert!(match_tool_approvals(&tools, None, &pending).is_empty());
    }
}
