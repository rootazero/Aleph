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

/// JSON-RPC methods that mutate Aleph's own configuration / management surface
/// and therefore require an operator (Config-tier) connection. This is the
/// defense-in-depth sibling of [`OPERATOR_TOOLS`]: the Panel hides these behind
/// `ConfigGate`, but a chat-tier device must also be refused at the WS dispatch
/// gate (`server::handler`) so a hand-crafted RPC cannot bypass the hidden UI.
///
/// Scope is **configuration**, not execution — consistent with the tool gate,
/// which leaves `bash` / `code_exec` open to chat tier (the LAN-trust boundary
/// still governs raw execution; the device tier governs *config* mutation).
/// So conversation, sessions, collaboration, reads, and tool/PTY *use* stay
/// open; only config/management writes are listed. Fail-open on anything not
/// listed is acceptable — the tool gate and the hidden UI remain in force.
const OPERATOR_RPC_METHODS: &[&str] = &[
    // Agent management
    "agents.create",
    "agents.delete",
    "agents.update",
    "agents.set_default",
    "agents.bindings",
    "agents.files.set",
    "agents.files.delete",
    // Channel binding + pairing decisions
    "channels.set_agent",
    "channel.pairing.approve",
    "channel.pairing.reject",
    "channel.pairing.revoke",
    // Marketplace / skills / plugins install surface
    "clawhub.install",
    "skills.install",
    "skills.install_dep",
    "skills.remove",
    "skills.update",
    "plugin.install",
    "plugin.uninstall",
    "plugin.enable",
    "plugin.disable",
    "plugin.load",
    "plugin.unload",
    "plugin.reload",
    "plugin.update",
    "plugin.marketplace.add",
    "plugins.install",
    "plugins.uninstall",
    "plugins.enable",
    "plugins.disable",
    "plugins.load",
    "plugins.unload",
    // Scheduling / automation
    "cron.create",
    "cron.delete",
    "cron.toggle",
    "cron.update",
    "cron.run",
    "heartbeat.create",
    "heartbeat.delete",
    "heartbeat.toggle",
    "heartbeat.update",
    "heartbeat.wake",
    "hooks.add",
    "hooks.remove",
    "hooks.reload",
    "dreaming.run_now",
    // Identity / persona
    "identity.set",
    "identity.clear",
    // Runtimes / services / logging
    "runtimes.install",
    "runtimes.refresh",
    "services.start",
    "services.stop",
    "logs.set",
    // Credentials / flow / device management
    "gateway.credentials",
    "gateway.flow.reload",
    "daemon.shutdown",
    "devices.list",
    "devices.set_level",
    "devices.revoke",
    // Approvals an operator alone may resolve
    "exec.approval.resolve",
    "exec.approvals.set",
    "mcp.respond_approval",
    // Memory re-embedding (re-config of the embedding pipeline)
    "memory.reembed",
    "memory.reembed.cancel",
    // Workspace / project creation (Config-tier "free workspace choice")
    "workspace.create",
    "workspace.update",
    "workspace.archive",
    "projects.add",
    "projects.create_blank",
    "projects.remove",
];

/// True when `method` mutates Aleph's configuration / management surface and
/// therefore requires an operator (Config-tier) connection. Methods not listed
/// — conversation, reads, sessions, collaboration, tool/PTY use — stay open to
/// chat tier.
#[must_use]
pub fn rpc_requires_operator(method: &str) -> bool {
    OPERATOR_RPC_METHODS.contains(&method)
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

    #[test]
    fn config_rpc_methods_require_operator() {
        for m in [
            "agents.create",
            "skills.install",
            "plugin.install",
            "cron.create",
            "heartbeat.update",
            "identity.set",
            "gateway.credentials",
            "devices.set_level",
            "exec.approval.resolve",
            "workspace.create",
        ] {
            assert!(rpc_requires_operator(m), "{m} must require operator");
        }
    }

    #[test]
    fn chat_rpc_methods_stay_open() {
        // Conversation, reads, sessions, collaboration, and tool/PTY use are
        // never gated by device tier — only config mutation is.
        for m in [
            "connect",
            "chat.send",
            "chat.history",
            "sessions.delete",
            "session.compact",
            "agents.list",
            "agents.get",
            "memory.compress",
            "tools.invoke",
            "pty.spawn",
            "events.subscribe",
            "teams.chat.send",
            "cron.list",
            "plugin.list",
            "skills.status",
        ] {
            assert!(
                !rpc_requires_operator(m),
                "{m} must stay open to chat tier"
            );
        }
    }
}
