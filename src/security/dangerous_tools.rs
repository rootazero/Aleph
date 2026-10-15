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
    // --- Capability-projection diagnostics ---
    // Conditional shape (`ALEPH_CAPABILITY_DIAGNOSTICS=1`): the tool is
    // registered only when startup wires a `DiagnosticControl` into
    // `BuiltinToolConfig.diagnostics_control`. When registered, it is
    // dangerous on the gateway `tools.invoke` surface for the same reason
    // the runtime-facing halves above are: its writes (`Hold` tears down
    // a source plane, `RevokeTool` removes a tool from the live tree,
    // `Close` shuts the host) are state mutations on the same Aleph
    // surface `runtime_manage` rewrites, and this surface has no
    // approval transport to raise the argument-level cards the agent
    // loop would have raised. The handler-local three-part identity
    // check (operator + loopback + conn_id) inside
    // `execute_capability_projection_diagnostics` is the only authority
    // on top of this deny — ALEPH_GATEWAY_TOOLS_ALLOW can lift the
    // surface-level deny, but the ambient task-locals are still
    // re-checked before any host/tree side-effect, so the override
    // cannot reach a non-operator caller. Paired with the
    // `OPERATOR_TOOLS` entry in `method_authz.rs`, so the agent-loop
    // half and the gateway-half travel together.
    "capability_projection_diagnostics",
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
    ///
    /// A name is "real" if it appears in EITHER of the two registration
    /// shapes:
    ///   * `BUILTIN_TOOL_DEFINITIONS` — the unconditional catalog.
    ///   * The source census (`executor::builtin_registry::dispatchable
    ///     ::advertised_tools`) — covers the `reg(…)` shape AND the
    ///     constructor's `if let Some(ref X) = config.X { … }` conditional
    ///     blocks. Some tools live only in the conditional shape
    ///     (`capability_projection_diagnostics`, gated on
    ///     `ALEPH_CAPABILITY_DIAGNOSTICS=1`); checking only the
    ///     unconditional catalog would falsely mark them as ghosts.
    #[test]
    fn every_entry_names_a_real_tool() {
        let catalog: std::collections::BTreeSet<&str> = crate::executor::BUILTIN_TOOL_DEFINITIONS
            .iter()
            .map(|d| d.name)
            .collect();
        let conditional: std::collections::BTreeSet<String> =
            crate::executor::builtin_registry::dispatchable::advertised_tools();
        let conditional_strs: std::collections::BTreeSet<&str> =
            conditional.iter().map(String::as_str).collect();
        for t in DANGEROUS_TOOLS {
            assert!(
                catalog.contains(t) || conditional_strs.contains(t),
                "`{t}` is on the dangerous denylist but no builtin tool is registered \
                 under that name — the entry denies nothing. The name must appear \
                 in either BUILTIN_TOOL_DEFINITIONS (unconditional catalog) or in \
                 the source census (which covers `reg(…)` and the constructor's \
                 conditional `if let Some(ref X) = config.X` blocks)."
            );
        }
    }

    /// `capability_projection_diagnostics` lives on both halves of the
    /// hard floor: a chat-tier run cannot reach it on the agent loop
    /// (operator-required) and a remote caller cannot reach it on the
    /// gateway `tools.invoke` surface (dangerous, with no approval
    /// transport — the only authority on top is the handler-local strict
    /// three-part identity check inside
    /// `execute_capability_projection_diagnostics`). A missing
    /// OPERATOR_TOOLS entry would re-open the runtime surface; a missing
    /// DANGEROUS_TOOLS entry would re-open the gateway surface; this test
    /// pins both.
    #[test]
    fn diagnostics_is_in_dangerous_tools_and_is_a_real_tool() {
        assert!(
            is_dangerous_tool("capability_projection_diagnostics"),
            "capability_projection_diagnostics must be on the dangerous denylist: \
             it mutates live capability state on the host (Hold tears down a \
             source plane, Close shuts the host) and there is no approval \
             transport on the gateway `tools.invoke` surface to raise the \
             argument-level cards the agent loop would have raised"
        );
        let advertised = crate::executor::builtin_registry::dispatchable::advertised_tools();
        assert!(
            advertised.contains("capability_projection_diagnostics"),
            "capability_projection_diagnostics is on the dangerous denylist but \
             the source census cannot find it — the constructor's conditional \
             `Registered schema for capability_projection_diagnostics` line is \
             missing, or the scan was removed"
        );
    }

    /// The two halves of the hard floor must travel together: the agent-loop
    /// operator gate and the gateway-surface denylist. Without the operator
    /// gate, a chat-tier channel run could call the tool via the loop; without
    /// the denylist, a remote `tools.invoke` caller could reach it on a
    /// surface that has no approval transport. The handler-local strict
    /// three-part identity check inside
    /// `execute_capability_projection_diagnostics` is the only authority on
    /// top of both — ALEPH_GATEWAY_TOOLS_ALLOW unblocks the gateway deny
    /// but the ambient task-locals (CALLER_ROLE / CALLER_IS_LOOPBACK /
    /// CALLER_CONN_ID) are still re-checked before any host/tree side-effect.
    /// The combination means there is no surface — gateway OR agent loop —
    /// that can call the tool without the ambient identity.
    #[test]
    fn diagnostics_is_not_a_gateway_surface_bypass() {
        use crate::gateway::method_authz::tool_requires_operator;
        assert!(
            is_dangerous_tool("capability_projection_diagnostics"),
            "operator-gated-but-not-dangerous would let a remote `tools.invoke` \
             caller reach the tool with only ALEPH_GATEWAY_TOOLS_ALLOW"
        );
        assert!(
            tool_requires_operator("capability_projection_diagnostics"),
            "dangerous-but-not-operator-gated would let a chat-tier channel run \
             reach the tool through the agent loop"
        );
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

    /// The gateway `ALEPH_GATEWAY_TOOLS_ALLOW=capability_projection_diagnostics`
    /// override only lifts the surface-level deny. The handler-local strict
    /// three-part ambient-identity check inside
    /// `execute_capability_projection_diagnostics` is the SOLE authority on
    /// top of the override — it reads `CALLER_ROLE` / `CALLER_IS_LOOPBACK` /
    /// `CALLER_CONN_ID` from the ambient task-locals and refuses the call
    /// BEFORE any `DiagnosticControl` method runs. A caller with no ambient
    /// scoping (i.e. outside the WS dispatch loop's `process_request` scope)
    /// must be denied for every mutating operation, with no host/tree
    /// side-effect, even when the override is set.
    ///
    /// Pins the contract that `ALEPH_GATEWAY_TOOLS_ALLOW` cannot be used to
    /// reach the diagnostics surface without the operator identity. Without
    /// this test, a future "simplification" that moved the three-part check
    /// into a config-tier predicate (and therefore behind the override) would
    /// silently re-open the host/tree mutation surface to any `tools.invoke`
    /// caller willing to set the env var.
    ///
    /// The control is built on an empty registry + empty tree (no
    /// `FakeHandler` / `ToolCapabilityDescriptor` scaffolding needed):
    /// `bump_runtime` mutates the tree's Runtime generation,
    /// `dispose_runtime` flips `tree.is_disposed(LifetimeScope::Runtime)`,
    /// `close` shuts the host. None require a registered tool, so the
    /// existing `capability_projection_diagnostics::tests` scaffold stays
    /// put and the brief's "do not modify that module" rule is honored.
    #[test]
    fn gateway_allow_does_not_bypass_handler_local_three_part_gate() {
        use crate::builtin_tools::capability_projection_diagnostics::{
            execute_capability_projection_diagnostics, parse_request, DiagnosticToolError,
        };
        use crate::capability::diagnostic_control::DiagnosticControl;
        use crate::capability::ownership::{LifetimeScope, OwnershipTree};
        use crate::capability::projection_host::ProjectionHost;
        use crate::tools::registry::ToolHandlerRegistry;
        use std::sync::Arc;

        const TEST_NAME: &str =
            "security::dangerous_tools::tests::gateway_allow_does_not_bypass_handler_local_three_part_gate";
        const CHILD_ENV: &str = "ALEPH_DIAGNOSTICS_AUTH_TEST_CHILD";

        // Set only the child's startup environment, never the parallel libtest
        // process environment. An in-process lock cannot protect unrelated
        // tests spawning children while set_var/remove_var runs.
        if std::env::var(CHILD_ENV).ok().as_deref() != Some(TEST_NAME) {
            let output =
                std::process::Command::new(std::env::current_exe().expect("test executable"))
                    .args(["--exact", TEST_NAME, "--nocapture", "--test-threads=1"])
                    .env(CHILD_ENV, TEST_NAME)
                    .env(GATEWAY_TOOLS_ALLOW_ENV, "capability_projection_diagnostics")
                    .output()
                    .expect("spawn isolated diagnostics authorization test");
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(
                output.status.success(),
                "isolated handler test failed: {}\n{stdout}\n{stderr}",
                output.status
            );
            // A stale exact filter must not silently succeed with zero tests.
            assert!(
                stdout.contains("1 passed; 0 failed"),
                "isolated handler test did not execute: {stdout}\n{stderr}"
            );
            println!("isolated diagnostics authorization receipt:\n{stdout}");
            return;
        }

        // Precondition: the override must unblock the surface-level deny —
        // otherwise the test would prove the wrong thing (the Denied reply
        // could come from the floor, not from the handler-local gate).
        assert!(
            !is_denied_on_gateway_surface("capability_projection_diagnostics", &NO_ARGS),
            "precondition: ALEPH_GATEWAY_TOOLS_ALLOW=capability_projection_diagnostics \
             must lift the gateway-surface deny; otherwise a Denied reply says \
             nothing about the handler-local gate"
        );

        // Fresh authority pair: empty registry + empty tree. The host and
        // control both `tokio::spawn` long-lived workers in their constructors,
        // so the entire setup + handler-loop + post-state capture must run
        // inside the runtime — building them outside `block_on` would panic
        // with "there is no reactor running" before any assertion runs. Same
        // pattern as `src/security/ssrf/fetch.rs::bypass_fetch_*` (sync
        // `#[test]` + `Runtime::new` + `block_on`).
        let tree = Arc::new(OwnershipTree::new());
        let mut ownership_changes = tree.subscribe_changes();

        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let (ctrl, status_before) = rt.block_on(async {
            let reg = ToolHandlerRegistry::new();
            let host = ProjectionHost::mount(reg, Arc::clone(&tree));
            let ctrl = Arc::new(DiagnosticControl::new(host, Arc::clone(&tree)).unwrap());

            // Capture pre-mutation state.
            assert!(!tree.is_disposed(LifetimeScope::Runtime));
            let status_before = ctrl.status().expect("status before unauthorized loop");
            (ctrl, status_before)
        });

        // Run the mutating operations under no ambient scoping. Outside
        // any `CALLER_ROLE` / `CALLER_IS_LOOPBACK` / `CALLER_CONN_ID` scope
        // the three task-locals default to `None` / `false` / `None`, which
        // the gate rejects at the very first fact (role != "operator").
        let responses = rt.block_on(async {
            let r1 = execute_capability_projection_diagnostics(
                parse_request(json!({"operation": "bump_runtime"})).expect("parse bump"),
                Arc::clone(&ctrl),
            )
            .await;
            let r2 = execute_capability_projection_diagnostics(
                parse_request(json!({"operation": "dispose_runtime"})).expect("parse dispose"),
                Arc::clone(&ctrl),
            )
            .await;
            let r3 = execute_capability_projection_diagnostics(
                parse_request(json!({"operation": "close"})).expect("parse close"),
                Arc::clone(&ctrl),
            )
            .await;
            (r1, r2, r3)
        });

        // Every mutating call must be DENIED with the structured
        // `DiagnosticToolError::Denied { reason }` variant, and the reason
        // must name a missing fact (operator / loopback / connection). A
        // bare `Ok(_)` here would mean the override reached past the
        // handler-local gate.
        for (label, res) in [
            ("bump_runtime", &responses.0),
            ("dispose_runtime", &responses.1),
            ("close", &responses.2),
        ] {
            match res {
                Err(DiagnosticToolError::Denied { reason }) => {
                    let names_missing_fact = reason.contains("operator")
                        || reason.contains("loopback")
                        || reason.contains("connection");
                    assert!(
                        names_missing_fact,
                        "{label} denied but reason does not name a missing fact: {reason}"
                    );
                }
                Err(other) => {
                    panic!("{label} must deny via DiagnosticToolError::Denied, got {other:?}")
                }
                Ok(ok) => {
                    panic!("{label} was admitted with no ambient identity (override only): {ok:?}")
                }
            }
        }

        // No mutation: the ownership tree's Runtime scope is still alive,
        // and the diagnostic status has not changed. A `dispose_runtime` or
        // `close` that slipped through the gate would have flipped
        // `tree.is_disposed(LifetimeScope::Runtime)` to `true`, or moved
        // the host to `Closing` / `Closed`. Either would mean the override
        // bypassed the gate. Both reads must run inside the runtime because
        // they touch the spawned workers' shared state.
        let (tree_still_alive, status_after) = rt.block_on(async {
            (
                !tree.is_disposed(LifetimeScope::Runtime),
                ctrl.status().expect("status after unauthorized loop"),
            )
        });
        assert!(
            tree_still_alive,
            "ownership tree's Runtime scope must NOT be disposed: the gate denied \
             the call BEFORE any DiagnosticControl method ran"
        );
        assert_eq!(
            status_before.lifecycle, status_after.lifecycle,
            "host lifecycle must be unchanged after the unauthorized loop"
        );

        // Even an empty tree emits a notification if bump/dispose runs.
        // Pin that the denied calls never reached those authority mutations.
        assert!(matches!(
            ownership_changes.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
    }
}
