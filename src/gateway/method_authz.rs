//! Tool-tier authorization (config-mutating tools require operator).
//!
//! Scope after the Panel collapsed to single-tier Gateway-token auth: the Panel
//! no longer has a Chat/Config sub-tier — a connection is either authorized
//! (operator, full local-equivalent authority) or walled at `connect`. So the
//! classifier below is now purely the **channel** config-tier gate: the
//! inbound router (`inbound_router::executor`) stamps each channel run's
//! `caller_role` from its `ChannelPermissionLevel` (default `Chat` ⇒ `guest`),
//! and `ScopedToolService` (`src/tools/scoped/dispatch.rs`) consults it here to
//! refuse self-config tools to a chat-tier channel (e.g. a default Telegram
//! bot). Panel runs are always operator once authorized, so this gate is a
//! no-op for them — it governs channels only.

/// Self-management tool names that mutate Aleph's OWN configuration. A
/// chat-tier channel run is rejected from these at the tool-dispatch gate
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
    "agent_switch",
    "channel_pairing",
    "clawhub",
    "hub_install_run",
    "moa",
    // Cluster: driving remote execution arms. Local `bash` is deliberately open
    // to chat tier, but the fleet is a different blast radius — one call reaches
    // every machine the center owns, and `node_file` moves bytes across that
    // boundary. Read-only discovery (`node_list`) stays open so a chat-tier run
    // can still *describe* the fleet.
    "node_invoke",
    "node_invoke_many",
    "node_file",
    // Membership is a stronger claim than execution: it decides which machines
    // the center owns at all, and a deregister is only undone by re-enrolling.
    "node_manage",
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
            "agent_switch",
            "channel_pairing",
            "clawhub",
            "hub_install_run",
            "moa",
            // Cluster write tools: remote exec + file transfer across the fleet.
            "node_invoke",
            "node_invoke_many",
            "node_file",
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
            // Read-only fleet discovery stays open — it names nodes, it cannot
            // drive them.
            "node_list",
        ] {
            assert!(
                !tool_requires_operator(t),
                "{t} must stay open to chat tier"
            );
        }
    }
}
