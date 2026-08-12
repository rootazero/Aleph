//! Audit-identity resolution for approval [`ActionRequest`](super::ActionRequest)s.
//!
//! Every policy-gated tool (`desktop`, `browser`, `system`, `pim`,
//! `automation`) stamps the issuing agent + a human-readable context onto the
//! [`ActionRequest`](super::ActionRequest) it submits, so
//! [`ApprovalPolicy::record`](super::ApprovalPolicy::record) produces an
//! accountable audit trail rather than blank entries. This module is the single
//! source of that resolution — previously each tool re-implemented it (two near
//! identical copies in `desktop`/`browser`, three tools left it blank).

/// Resolve `(agent_id, context)` for an approval request from the agent-loop
/// [`TURN_CONTEXT`](crate::tools::turn_context).
///
/// - `domain` — the tool family that emitted the action (`"desktop"`,
///   `"browser"`, `"system"`, `"pim"`, `"automation"`).
/// - `action` — the specific operation name (`"click"`, `"run_script"`, …).
/// - `target` — the action target (URL, app id, command, …).
///
/// Returns:
/// - `agent_id` — the issuing agent; falls back to `"main"` outside a scoped
///   turn (direct calls, tests), consistent with `parse_caller_agent_id`.
/// - `context` — `"{domain}.{action} ({target})"`, suffixed with
///   ` via {channel}/{conversation}` for channel-originated turns.
///
/// `check_approval` callers run this before any `spawn_blocking`, so the
/// task-local is still in scope at the read.
#[must_use]
pub fn audit_identity(domain: &str, action: &str, target: &str) -> (String, String) {
    match crate::tools::turn_context::current_turn_context() {
        Some(turn) => {
            let agent_id = turn.session_key.agent_id().to_string();
            let context = if turn.is_channel_routable() {
                format!(
                    "{domain}.{action} ({target}) via {}/{}",
                    turn.channel_id, turn.conversation_id
                )
            } else {
                format!("{domain}.{action} ({target})")
            };
            (agent_id, context)
        }
        None => ("main".to_string(), format!("{domain}.{action} ({target})")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::session_key::SessionKey;
    use crate::tools::turn_context::{TurnContext, TURN_CONTEXT};

    #[test]
    fn falls_back_to_main_outside_turn() {
        let (agent_id, context) = audit_identity("desktop", "click", "click(1,2)");
        assert_eq!(agent_id, "main");
        assert_eq!(context, "desktop.click (click(1,2))");
    }

    #[test]
    fn reads_agent_and_channel_from_turn_context() {
        let turn = TurnContext {
            session_key: SessionKey::task("research-agent", "cron", "daily"),
            run_id: String::new(),
            channel_id: "slack".to_string(),
            conversation_id: "C123".to_string(),
            caller_role: None,
            channel_tool_permissions: None,
            unattended: false,
            plan_gate: None,
        };
        let (agent_id, context) =
            TURN_CONTEXT.sync_scope(turn, || audit_identity("browser", "navigate", "https://x"));
        assert_eq!(agent_id, "research-agent");
        assert_eq!(context, "browser.navigate (https://x) via slack/C123");
    }

    #[test]
    fn omits_origin_for_non_channel_turn() {
        let turn = TurnContext {
            session_key: SessionKey::task("cron-agent", "cron", "daily"),
            run_id: String::new(),
            channel_id: String::new(),
            conversation_id: String::new(),
            caller_role: None,
            channel_tool_permissions: None,
            unattended: false,
            plan_gate: None,
        };
        let (agent_id, context) =
            TURN_CONTEXT.sync_scope(turn, || audit_identity("automation", "run_script", "sh"));
        assert_eq!(agent_id, "cron-agent");
        assert_eq!(context, "automation.run_script (sh)");
    }
}
