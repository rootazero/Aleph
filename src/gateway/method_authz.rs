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
    // Agent signing keys and the operation ledger. Rotating or revoking an
    // identity changes who the accountability record can attribute actions to;
    // reading it exposes every agent's activity. Neither is chat tier.
    "agent_identity",
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

    /// Gates that must never be removed, whatever else changes.
    ///
    /// Deliberately a **subset**, not a mirror of `OPERATOR_TOOLS`. The full
    /// duplicate this replaced had already drifted — it never learned about
    /// `node_manage`, so the newest operator tool was the one tool the test
    /// meant to pin did not pin. A curated tripwire catches a deletion without
    /// pretending to be a second source of truth.
    const MUST_STAY_GATED: &[&str] = &[
        "self_config",
        "self_manage",
        "vault_store",
        "skill_install",
        "agent_create",
        "agent_delete",
        // Fleet: remote execution, file movement across the trust boundary,
        // and membership itself.
        "node_invoke",
        "node_file",
        "node_manage",
        // Accountability: rotating a signing key changes who the record can
        // name; reading the ledger exposes every agent's activity.
        "agent_identity",
    ];

    #[test]
    fn config_tools_require_operator() {
        for t in MUST_STAY_GATED {
            assert!(tool_requires_operator(t), "{t} must require operator");
        }
    }

    #[test]
    fn operator_tools_has_no_duplicates() {
        let unique: std::collections::HashSet<_> = OPERATOR_TOOLS.iter().collect();
        assert_eq!(
            unique.len(),
            OPERATOR_TOOLS.len(),
            "OPERATOR_TOOLS must not list a tool twice"
        );
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
