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
//! - `exec` / `spawn` / `shell`                       -> `exec`, `bash`, `process`
//! - `fs_write` / `fs_delete` / `fs_move` / `apply_patch` -> `fs_write`, `fs_edit`
//! - `gateway` / `cron` / `nodes` (control-plane)     -> `agent_manage`, `provider_config`, `channel_config`

/// Tools that are off-limits to untrusted surfaces by default.
///
/// Static and hardcoded, mirroring openclaw. The only way to re-enable a
/// specific entry is an *explicit, per-tool opt-in* (an exact-name grant in a
/// guest scope, or the `ALEPH_GATEWAY_TOOLS_ALLOW` env var for the
/// `tools.invoke` RPC surface) - never a wildcard or category match.
pub const DANGEROUS_TOOLS: &[&str] = &[
    // --- Remote code execution ---
    "exec",
    "bash",
    "process",
    // --- Host filesystem mutation ---
    "fs_write",
    "fs_edit",
    // --- Control plane / self-reconfiguration ---
    "agent_manage",
    "provider_config",
    "channel_config",
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
pub fn is_dangerous_tool(tool_name: &str) -> bool {
    let category = tool_name.split(':').next().unwrap_or(tool_name);
    DANGEROUS_TOOLS
        .iter()
        .any(|&d| d == tool_name || d == category)
}

/// Decide whether `tool_name` must be denied on the gateway `tools.invoke`
/// RPC surface. A tool is denied iff it is [`is_dangerous_tool`] AND not
/// explicitly re-permitted via [`GATEWAY_TOOLS_ALLOW_ENV`].
pub fn is_denied_on_gateway_surface(tool_name: &str) -> bool {
    if !is_dangerous_tool(tool_name) {
        return false;
    }
    let allow = std::env::var(GATEWAY_TOOLS_ALLOW_ENV).unwrap_or_default();
    !allow
        .split(',')
        .map(str::trim)
        .any(|allowed| !allowed.is_empty() && allowed == tool_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_rce_and_mutation_and_control_plane() {
        for t in [
            "exec",
            "bash",
            "process",
            "fs_write",
            "fs_edit",
            "agent_manage",
            "provider_config",
            "channel_config",
        ] {
            assert!(is_dangerous_tool(t), "{t} should be dangerous");
        }
    }

    #[test]
    fn allows_read_only_and_safe_tools() {
        for t in ["fs_read", "memory_search", "note_manage", "web_fetch"] {
            assert!(!is_dangerous_tool(t), "{t} should be safe");
        }
    }

    #[test]
    fn category_prefix_is_caught() {
        assert!(is_dangerous_tool("exec:run"));
        assert!(!is_dangerous_tool("memory:search"));
    }

    #[test]
    fn gateway_surface_denies_dangerous_without_env() {
        std::env::remove_var(GATEWAY_TOOLS_ALLOW_ENV);
        assert!(is_denied_on_gateway_surface("exec"));
        assert!(!is_denied_on_gateway_surface("fs_read"));
    }

    #[test]
    fn gateway_surface_respects_explicit_allow() {
        std::env::set_var(GATEWAY_TOOLS_ALLOW_ENV, "fs_write, exec");
        assert!(!is_denied_on_gateway_surface("fs_write"));
        assert!(!is_denied_on_gateway_surface("exec"));
        assert!(is_denied_on_gateway_surface("bash"));
        std::env::remove_var(GATEWAY_TOOLS_ALLOW_ENV);
    }
}
