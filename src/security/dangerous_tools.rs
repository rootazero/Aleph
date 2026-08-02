//! Dangerous-tool denylist (hard floor for untrusted surfaces).
//!
//! Ported from openclaw's `src/security/dangerous-tools.ts`
//! (`DEFAULT_GATEWAY_HTTP_TOOL_DENY`) and mapped onto Aleph's builtin tool
//! names. The idea is a *transport / identity hard floor*: a remote, guest,
//! or otherwise untrusted caller must never be able to reach
//! Remote-Code-Execution, host-filesystem-mutation, or self-reconfiguration
//! ("control-plane") tools - even when an allowlist, a wildcard guest scope,
//! or a category grant would otherwise permit it.
//!
//! This is *defense in depth*: it sits underneath the per-agent allowlist
//! (`AgentDef::is_tool_allowed`) and the guest `GuestScope` allowlist, and
//! tightens - never loosens - them. Owner / local-trusted callers are never
//! restricted here.
//!
//! # openclaw -> Aleph mapping
//!
//! - `exec` / `spawn` / `shell`                           -> `bash`, `code_exec`
//! - `fs_write` / `fs_delete` / `fs_move` / `apply_patch` -> `file_write`, `file_edit`, `apply_patch`
//! - `gateway` / `cron` / `nodes` (control-plane)         -> `self_config`, `self_manage`,
//!   `agent_create` / `agent_delete` / `agent_switch`, `node_invoke` / `node_invoke_many` / `node_file`

/// Tools that are off-limits to untrusted surfaces by default.
///
/// Static and hardcoded, mirroring openclaw. The only way to re-enable a
/// specific entry is an *explicit, per-tool opt-in* (an exact-name grant in a
/// guest scope, or the `ALEPH_GATEWAY_TOOLS_ALLOW` env var for the
/// `tools.invoke` RPC surface) - never a wildcard or category match.
///
/// Every entry must name a tool that actually exists: a denylist entry for a
/// tool nobody registers denies nothing, and the port from openclaw originally
/// shipped seven such ghosts (`exec`, `process`, `fs_write`, `fs_edit`,
/// `agent_manage`, `provider_config`, `channel_config`) — a denylist that had
/// been inert for its whole life. Pinned by `every_entry_names_a_real_tool`.
pub const DANGEROUS_TOOLS: &[&str] = &[
    // --- Remote code execution ---
    "bash",
    "code_exec",
    // --- Host filesystem mutation ---
    "file_write",
    "file_edit",
    "apply_patch",
    // `file_ops` multiplexes read-only (list/search) and destructive (delete/move)
    // behind one name. The exec tier gates its destructive ops at the ARGUMENT
    // level (`ExecTier::asks_for_arguments`), but `tools.invoke` has no approval
    // transport and cannot honor an argument-level ask — so the destructive path
    // would run un-gated there. Denied outright on this surface (consistent with
    // the blanket deny of file_write/file_edit); the `ALEPH_GATEWAY_TOOLS_ALLOW`
    // escape hatch still permits explicit test opt-in.
    "file_ops",
    // --- Control plane / self-reconfiguration ---
    "self_config",
    "self_manage",
    "agent_create",
    "agent_delete",
    "agent_switch",
    // --- Fleet: one call reaches every machine the center owns ---
    "node_invoke",
    "node_invoke_many",
    "node_file",
];

/// Environment variable that re-permits specific dangerous tools on the
/// gateway `tools.invoke` surface. Comma-separated tool names, mirroring
/// openclaw's `gateway.tools.allow`. Empty / unset means "deny all dangerous".
pub const GATEWAY_TOOLS_ALLOW_ENV: &str = "ALEPH_GATEWAY_TOOLS_ALLOW";

/// Returns `true` if `tool_name` names an RCE / host-mutation /
/// control-plane tool that untrusted surfaces must not reach by default.
///
/// Matching is exact on the full tool name. A leading `category:` segment
/// (Aleph builtins use `_`, but some external tools use `:`) is also checked
/// against the denylist so that e.g. `exec:run` is still caught by `exec`.
#[must_use]
pub fn is_dangerous_tool(tool_name: &str) -> bool {
    let category = tool_name.split(':').next().unwrap_or(tool_name);
    DANGEROUS_TOOLS
        .iter()
        .any(|&d| d == tool_name || d == category)
}

/// Returns `true` if `tool_name` self-declares
/// [`crate::tools::runtime::LoopTool::requires_confirmation`].
///
/// The agent loop answers such a tool with an approval card before it runs.
/// Surfaces that have no approval transport (the `tools.invoke` RPC) cannot
/// raise that card, so they must fail closed rather than silently skip the
/// gate the tool asked for. Reads the adapter's own list so there is exactly
/// one source for "which tools need a card".
#[must_use]
pub fn is_confirmation_gated(tool_name: &str) -> bool {
    crate::tools::adapters::registry_adapter::CONFIRMATION_REQUIRED_TOOLS.contains(&tool_name)
}

/// Decide whether `tool_name` must be denied on the gateway `tools.invoke`
/// RPC surface. A tool is denied iff it is [`is_dangerous_tool`] or
/// [`is_confirmation_gated`], AND not explicitly re-permitted via
/// [`GATEWAY_TOOLS_ALLOW_ENV`].
pub fn is_denied_on_gateway_surface(tool_name: &str) -> bool {
    if !is_dangerous_tool(tool_name) && !is_confirmation_gated(tool_name) {
        return false;
    }
    !gateway_surface_override(tool_name)
}

/// Has the operator explicitly re-permitted `tool_name` on the gateway
/// `tools.invoke` surface via [`GATEWAY_TOOLS_ALLOW_ENV`]?
///
/// The single parser for that env var, shared by every hard floor this
/// surface applies — the dangerous/confirmation floor above and the
/// continuation-driven floor in `handlers::tools_invoke` — so a test harness
/// only ever has to learn one escape hatch.
#[must_use]
pub fn gateway_surface_override(tool_name: &str) -> bool {
    let allow = std::env::var(GATEWAY_TOOLS_ALLOW_ENV).unwrap_or_default();
    allow
        .split(',')
        .map(str::trim)
        .any(|allowed| !allowed.is_empty() && allowed == tool_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync_primitives::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn flags_rce_and_mutation_and_control_plane() {
        for t in [
            "bash",
            "code_exec",
            "file_write",
            "file_edit",
            "apply_patch",
            "file_ops",
            "self_config",
            "self_manage",
            "agent_create",
            "agent_delete",
            "agent_switch",
            "node_invoke",
            "node_invoke_many",
            "node_file",
        ] {
            assert!(is_dangerous_tool(t), "{t} should be dangerous");
        }
    }

    /// A denylist entry that names no real tool denies nothing. The port from
    /// openclaw shipped seven such ghosts and nobody noticed for the list's
    /// whole life, because no test ever asked whether the names were real.
    #[test]
    fn every_entry_names_a_real_tool() {
        for t in DANGEROUS_TOOLS {
            assert!(
                crate::executor::BUILTIN_TOOL_DEFINITIONS
                    .iter()
                    .any(|d| d.name == *t),
                "`{t}` is on the dangerous denylist but no builtin tool is registered \
                 under that name — the entry denies nothing"
            );
        }
    }

    #[test]
    fn allows_read_only_and_safe_tools() {
        for t in ["file_read", "memory_search", "note_manage", "web_fetch"] {
            assert!(!is_dangerous_tool(t), "{t} should be safe");
        }
    }

    #[test]
    fn category_prefix_is_caught() {
        assert!(is_dangerous_tool("bash:run"));
        assert!(!is_dangerous_tool("memory:search"));
    }

    #[test]
    fn gateway_surface_denies_dangerous_without_env() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var(GATEWAY_TOOLS_ALLOW_ENV);
        assert!(is_denied_on_gateway_surface("bash"));
        assert!(!is_denied_on_gateway_surface("file_read"));
    }

    /// `file_ops` gates its destructive ops (delete/move) at the ARGUMENT level
    /// via the exec tier, which `tools.invoke` cannot honor (no approval
    /// transport). It must therefore be denied outright on this surface — the
    /// argument-level parity gap that would otherwise let a `delete` slip through.
    #[test]
    fn gateway_surface_denies_file_ops() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var(GATEWAY_TOOLS_ALLOW_ENV);
        assert!(is_dangerous_tool("file_ops"));
        assert!(is_denied_on_gateway_surface("file_ops"));
    }

    /// `tools.invoke` dispatches straight off the raw registry: it has no
    /// approval transport, so a tool that DECLARES `requires_confirmation`
    /// would otherwise run there with no card, at any tier.
    #[test]
    fn gateway_surface_denies_confirmation_gated_tools() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var(GATEWAY_TOOLS_ALLOW_ENV);
        for t in ["vault_store", "agent_delete", "team_disband"] {
            assert!(
                is_confirmation_gated(t),
                "{t} declares requires_confirmation"
            );
            assert!(
                is_denied_on_gateway_surface(t),
                "{t} needs an approval card and this surface cannot raise one"
            );
        }
        // Not every dangerous tool is confirm-gated, and vice versa.
        assert!(!is_confirmation_gated("bash"));
        assert!(!is_dangerous_tool("team_disband"));
    }

    #[test]
    fn gateway_surface_respects_explicit_allow() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var(GATEWAY_TOOLS_ALLOW_ENV, "file_write, vault_store");
        assert!(!is_denied_on_gateway_surface("file_write"));
        // The escape hatch covers the confirm-gated class too (E2E `tools call`).
        assert!(!is_denied_on_gateway_surface("vault_store"));
        assert!(is_denied_on_gateway_surface("bash"));
        assert!(is_denied_on_gateway_surface("agent_delete"));
        std::env::remove_var(GATEWAY_TOOLS_ALLOW_ENV);
    }
}
