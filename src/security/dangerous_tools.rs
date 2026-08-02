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

/// Decide whether this call must be denied on a surface with **no approval
/// transport** (the gateway `tools.invoke` RPC, heartbeat probes).
///
/// Denied iff [`is_dangerous_tool`], [`is_confirmation_gated`], or the exec
/// tier would have raised an **argument-level** card for these exact
/// arguments — AND not explicitly re-permitted via [`GATEWAY_TOOLS_ALLOW_ENV`].
///
/// The third disjunct reads `ExecTier::Auto::asks_for_arguments` rather than
/// re-listing anything, so it is the *same* predicate the agent loop enforces
/// and the two cannot drift. It exists because a name-only floor cannot see
/// the class of tools whose danger lives in one argument: `file_ops` was
/// papered over by adding the whole tool to [`DANGEROUS_TOOLS`], but
/// `loop_graph` — the only other tool with an argument-level rule — was never
/// given the same treatment, so a single unauthenticated-shaped `tools.invoke`
/// could rewrite the human `root:` reference that this daemon injects verbatim
/// into every governed session's system prompt. Arguments in, and the whole
/// class is covered at once, including any future rule.
pub fn is_denied_on_gateway_surface(tool_name: &str, args: &serde_json::Value) -> bool {
    let asks_at_argument_level = crate::config::types::policies::exec_tier::ExecTier::Auto
        .asks_for_arguments(tool_name, args);
    if !is_dangerous_tool(tool_name) && !is_confirmation_gated(tool_name) && !asks_at_argument_level
    {
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
    use serde_json::json;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// A call with no arguments — exercises the name-only half of the floor.
    const NO_ARGS: serde_json::Value = serde_json::Value::Null;

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
        assert!(is_denied_on_gateway_surface("bash", &NO_ARGS));
        assert!(!is_denied_on_gateway_surface("file_read", &NO_ARGS));
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
        assert!(is_denied_on_gateway_surface("file_ops", &NO_ARGS));
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
                is_denied_on_gateway_surface(t, &NO_ARGS),
                "{t} needs an approval card and this surface cannot raise one"
            );
        }
        // Not every dangerous tool is confirm-gated, and vice versa.
        assert!(!is_confirmation_gated("bash"));
        assert!(!is_dangerous_tool("team_disband"));
    }

    /// The argument-level half of the floor, on the tool that has no
    /// name-level entry to fall back on.
    ///
    /// `loop_graph` is deliberately absent from `DANGEROUS_TOOLS` (it is not in
    /// `BUILTIN_TOOL_DEFINITIONS`, so `every_entry_names_a_real_tool` would
    /// reject the entry) — the deny has to come from reading the arguments.
    /// A `root:` write is the case that matters: its `body` is re-injected
    /// verbatim into every governed session's system prompt, and this surface
    /// cannot raise the card the exec tier would have raised.
    #[test]
    fn gateway_surface_denies_argument_level_asks_it_cannot_card() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var(GATEWAY_TOOLS_ALLOW_ENV);

        assert!(
            !is_dangerous_tool("loop_graph") && !is_confirmation_gated("loop_graph"),
            "this test is only meaningful while loop_graph has no name-level entry"
        );

        for protected in ["root:aleph", "frozen:budget-ratchet"] {
            assert!(
                is_denied_on_gateway_surface(
                    "loop_graph",
                    &json!({"action": "node", "kind": "root", "id": protected,
                            "label": "x", "origin": "human", "body": "forged"}),
                ),
                "a write to {protected} must not run on a surface with no approval transport"
            );
        }
        // Same tool, ordinary node / read action: still permitted. A blanket
        // name deny would have taken these with it.
        assert!(!is_denied_on_gateway_surface(
            "loop_graph",
            &json!({"action": "status"})
        ));
        assert!(!is_denied_on_gateway_surface(
            "loop_graph",
            &json!({"action": "node", "kind": "daemon", "id": "daemon:dreaming", "label": "x"})
        ));
        // And the pre-existing argument-level rule is covered by the same
        // disjunct, independently of `file_ops`' name-level entry.
        assert!(is_denied_on_gateway_surface(
            "file_ops",
            &json!({"operation": "delete", "path": "/tmp/x"})
        ));
    }

    #[test]
    fn gateway_surface_respects_explicit_allow() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var(GATEWAY_TOOLS_ALLOW_ENV, "file_write, vault_store");
        assert!(!is_denied_on_gateway_surface("file_write", &NO_ARGS));
        // The escape hatch covers the confirm-gated class too (E2E `tools call`).
        assert!(!is_denied_on_gateway_surface("vault_store", &NO_ARGS));
        assert!(is_denied_on_gateway_surface("bash", &NO_ARGS));
        assert!(is_denied_on_gateway_surface("agent_delete", &NO_ARGS));
        std::env::remove_var(GATEWAY_TOOLS_ALLOW_ENV);
    }
}
