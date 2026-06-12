//! Tool-tier authorization (config-mutating tools require operator).
//!
//! LAN-trust note: the method-level RPC authorization gate (operator vs
//! any-authenticated, keyed off the connection role) was removed when the
//! gateway dropped authentication — every connection is now an implicit
//! operator. The remaining classifier below is the *tool-dispatch* tier gate
//! consumed by `ScopedToolService` (`src/tools/scoped/dispatch.rs`): it marks
//! the self-management tools that mutate Aleph's OWN configuration. Under
//! LAN-trust the caller role is always `operator`, so this gate always passes;
//! it is retained as the seam later revert steps fold away with the tool-tier
//! plumbing.

/// Self-management tool names that mutate Aleph's OWN configuration. A
/// chat-tier connection is rejected from these at the tool-dispatch gate
/// (`ScopedToolService::execute_inner`).
///
/// Read-only self-management tools (`config_audit`, `gateway_route`,
/// `*_list`/`*_status`/`*_read`) are deliberately absent — chat tier keeps them.
const OPERATOR_TOOLS: &[&str] = &[
    "self_config",
    "self_manage",
    "vault_store",
    "cron_manage",
    "heartbeat_create",
    "heartbeat_update",
    "heartbeat_delete",
    "heartbeat_toggle",
    "skill_install",
    "skill_manage",
    "agent_create",
    "agent_delete",
    "channel_pairing",
    "clawhub",
];

/// True when `tool` mutates Aleph's own configuration and therefore requires an
/// operator (config-tier) connection. Names not listed stay open to chat tier.
#[must_use]
pub fn tool_requires_operator(tool: &str) -> bool {
    OPERATOR_TOOLS.contains(&tool)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_tools_require_operator() {
        for t in [
            "self_config",
            "self_manage",
            "vault_store",
            "cron_manage",
            "heartbeat_create",
            "heartbeat_update",
            "heartbeat_delete",
            "heartbeat_toggle",
            "skill_install",
            "skill_manage",
            "agent_create",
            "agent_delete",
            "channel_pairing",
            "clawhub",
        ] {
            assert!(tool_requires_operator(t), "{t} must require operator");
        }
    }

    #[test]
    fn chat_safe_tools_stay_open() {
        for t in [
            "search",
            "web_fetch",
            "file_read",
            "config_audit",
            "gateway_route",
            "heartbeat_list",
            "skill_list",
            "agent_list",
            "memory_search",
            "ask_user",
            "bash",
            "code_exec",
        ] {
            assert!(
                !tool_requires_operator(t),
                "{t} must stay open to chat tier"
            );
        }
    }
}
