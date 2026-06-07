//! RPC method authorization tiers (operator vs any-authenticated).
//!
//! Authentication on the gateway is binary at the transport layer: once a
//! connection completes `connect`, the dispatch loop treats it as
//! authenticated and (before this module) let it invoke **any** RPC method.
//! That is fine for operator devices — the panel, desktop shell and CLI all
//! authenticate as [`DeviceRole::Operator`] with a `*` scope — but it means a
//! *guest* connection (invitation-scoped, meant for restricted tool access)
//! could reach privileged control-plane methods such as `config.apply`,
//! `daemon.shutdown`, `secret.*` or `devices.remove`. Tool *invocation* is
//! already scoped by the guest [`PolicyEngine`], but those are direct RPC
//! handler dispatches, not tool calls, so nothing gated them.
//!
//! This module adds the missing **method-level authorization** layer: a pure
//! classifier that marks a conservative set of administrative / destructive /
//! secret-bearing control-plane methods as operator-only. The dispatch loop
//! rejects them for non-operator connections (guests today; node-role devices
//! in the future) while leaving everything a guest legitimately needs — chat,
//! agent runs, memory/session/graph reads — open.
//!
//! Design notes:
//! - **Conservative, allow-by-default.** Only methods on the explicit
//!   operator surface are restricted; every other method stays
//!   [`MethodPrivilege::Authenticated`]. Mis-classifying a benign method as
//!   operator-only would only ever restrict guests/nodes (operators pass
//!   everything), never the panel — but we still keep the set tight.
//! - **Role-based, not scope-based.** All device tokens are issued as
//!   operator with a `*` scope today, so a node-vs-operator role check is the
//!   sound, minimal model; finer per-scope ACLs are deferred (YAGNI) until
//!   restricted-scope tokens actually exist.
//! - **No effect in auth-disabled mode.** The caller only consults this gate
//!   when authentication is required; a local no-auth daemon is unchanged.

/// Privilege a connection must hold to invoke a given RPC method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodPrivilege {
    /// Any authenticated connection may call it (default for all methods).
    Authenticated,
    /// Only an operator-role connection may call it. Guest / node connections
    /// are rejected.
    Operator,
}

/// Whole namespaces (method prefixes) that are operator-only in their
/// entirety — every method under them mutates control-plane state or exposes
/// secrets, with no guest-safe read variant.
const OPERATOR_NAMESPACES: &[&str] = &[
    "secret.", "secrets.",
    // Embedded PTY terminal: spawning/driving an interactive host shell is a
    // privileged control-plane action — never expose it to guest/node roles.
    "pty.",
];

/// Exact operator-only methods. These are administrative, destructive, or
/// lifecycle/config-mutating control-plane RPCs. Read-only siblings in the
/// same namespace (e.g. `config.get`, `cron.list`, `agents.list`) are
/// deliberately absent so they remain available to guests.
const OPERATOR_METHODS: &[&str] = &[
    // Daemon lifecycle
    "daemon.shutdown",
    // Configuration mutation (config.get / config.schema stay open)
    "config.apply",
    "config.patch",
    "config.reload",
    // Device & pairing management
    "devices.remove",
    "devices.revoke",
    "pairing.approve",
    "pairing.reject",
    // Plugin lifecycle
    "plugins.install",
    "plugins.uninstall",
    "plugins.enable",
    "plugins.disable",
    // Skill lifecycle (the registered destructive method is `skills.remove`;
    // `skills.update` / `skills.install_dep` mutate skill state too)
    "skills.install",
    "skills.remove",
    "skills.update",
    "skills.install_dep",
    // MCP server lifecycle
    "mcp.start",
    "mcp.stop",
    // Scheduled jobs (cron.list / cron.get / cron.status / cron.runs stay open)
    "cron.create",
    "cron.delete",
    "cron.update",
    "cron.toggle",
    "cron.run",
    // Heartbeat jobs (heartbeat.list / heartbeat.get / heartbeat.runs stay open)
    "heartbeat.create",
    "heartbeat.delete",
    "heartbeat.update",
    "heartbeat.toggle",
    "heartbeat.wake",
    // Hooks (hooks.list / hooks.events stay open)
    "hooks.add",
    "hooks.remove",
    "hooks.reload",
    // Agent management (agents.list / agents.get / agents.teams / agents.tools_schema stay open)
    "agents.create",
    "agents.delete",
    "agents.update",
    "agents.set_default",
    "agents.bindings",
    "agents.files.set",
    "agents.files.delete",
    "channels.set_agent",
    // Identity management (identity.get / identity.list stay open)
    "identity.set",
    "identity.clear",
    // Background cognition + cross-system observability
    "dreaming.run_now",
    "insights.tools",
    // Agent routing rules + model-route mode (mutate routing bindings and the
    // live failover chain; read variants routing_rules.list/get + route_config.get
    // stay open)
    "routing_rules.create",
    "routing_rules.update",
    "routing_rules.delete",
    "routing_rules.move",
    "route_config.update",
    // Gateway control surface (credentials are sensitive; flow.reload mutates routing)
    "gateway.flow.reload",
    "gateway.credentials",
    // Code/extension installation from remote registries
    "clawhub.install",
    // Exec approval — resolving or reconfiguring the human-in-the-loop
    // command-exec gate is a privileged decision. The request/get/pending
    // reads stay open so a running agent can still raise and surface approvals.
    "exec.approval.resolve",
    "exec.approvals.set",
    "exec.callback.handle",
    // MCP tool-call approval responses — approving/cancelling a pending tool
    // call is privileged, mirroring exec.approval.resolve.
    "mcp.respond_approval",
    "mcp.cancel_approval",
    // Destructive team management (teams.get / teams.list / teams.list_tasks /
    // teams.usage stay open as reads).
    "teams.delete",
    "teams.disband",
    // Destructive session data loss (session.compact / session.usage stay open).
    "session.truncate",
];

/// Self-management tool names that mutate Aleph's OWN configuration. Sibling to
/// [`OPERATOR_METHODS`] (the RPC table) — kept in this one file so the two stay
/// in sync (the spec's "同源" requirement). A chat-tier connection is rejected
/// from these at the tool-dispatch gate (`ScopedToolService::execute_inner`),
/// mirroring how `required_privilege` rejects config RPCs.
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
pub fn tool_requires_operator(tool: &str) -> bool {
    OPERATOR_TOOLS.contains(&tool)
}

/// Classify an RPC method into the privilege required to invoke it.
///
/// Returns [`MethodPrivilege::Operator`] for the conservative administrative
/// surface and [`MethodPrivilege::Authenticated`] for everything else.
pub fn required_privilege(method: &str) -> MethodPrivilege {
    if OPERATOR_NAMESPACES.iter().any(|ns| method.starts_with(ns)) {
        return MethodPrivilege::Operator;
    }
    if OPERATOR_METHODS.contains(&method) {
        return MethodPrivilege::Operator;
    }
    MethodPrivilege::Authenticated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admin_methods_require_operator() {
        for m in [
            "daemon.shutdown",
            "config.apply",
            "config.reload",
            "devices.remove",
            "pairing.approve",
            "plugins.install",
            "skills.remove",
            "routing_rules.create",
            "route_config.update",
            "mcp.start",
            "cron.create",
            "heartbeat.delete",
            "hooks.add",
            "agents.delete",
            "identity.set",
            "dreaming.run_now",
            "insights.tools",
            "gateway.credentials",
            "clawhub.install",
            "exec.approval.resolve",
            "exec.approvals.set",
            "mcp.respond_approval",
            "teams.delete",
            "session.truncate",
        ] {
            assert_eq!(
                required_privilege(m),
                MethodPrivilege::Operator,
                "{m} should be operator-only"
            );
        }
    }

    #[test]
    fn secret_namespace_is_fully_operator() {
        assert_eq!(required_privilege("secret.set"), MethodPrivilege::Operator);
        assert_eq!(required_privilege("secret.get"), MethodPrivilege::Operator);
        assert_eq!(
            required_privilege("secrets.list"),
            MethodPrivilege::Operator
        );
    }

    #[test]
    fn pty_namespace_is_fully_operator() {
        for m in [
            "pty.spawn",
            "pty.input",
            "pty.resize",
            "pty.close",
            "pty.list",
        ] {
            assert_eq!(
                required_privilege(m),
                MethodPrivilege::Operator,
                "{m} should be operator-only"
            );
        }
    }

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
    fn guest_safe_methods_stay_authenticated() {
        for m in [
            "connect",
            "chat.send",
            "agent.run",
            "config.get",
            "config.schema",
            "cron.list",
            "cron.get",
            "agents.list",
            "agents.get",
            "graph.query",
            "graph.search",
            "session.history",
            "memory.search",
            "events.subscribe",
            "identity.get",
            "unknown.method",
        ] {
            assert_eq!(
                required_privilege(m),
                MethodPrivilege::Authenticated,
                "{m} should stay open to any authenticated connection"
            );
        }
    }
}
