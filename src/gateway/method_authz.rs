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
//! bot).
//!
//! ⚠️ **"Panel runs are always operator once authorized, so this gate is a no-op
//! for them"** was true when this module was written and stopped being true when
//! P0 introduced `UserRole::Member` (2026-08-04): an authorized Panel connection
//! now resolves to `operator` OR `member`, and a member's run reaches this
//! classifier just like a chat-tier channel does. That is deliberate — the gate
//! covers both surfaces — but it means the gate is load-bearing on the Panel and
//! must be reasoned about as such. In particular the gate does not *deny*: it
//! escalates to an operator via `config_approval_requester`, and that escalation
//! is only a gate if the person who tripped it cannot answer it. See
//! [`crate::approval::operator_requester`] for the half of that which does not
//! live here.

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
    // The two siblings the list forgot. `agent_create`/`agent_delete`/
    // `agent_switch` were gated from the start, which made the omissions read
    // as "already covered" rather than as holes:
    //
    // `agent_update` writes `allowed_users`, the list the run-start gate
    // reads: leaving it open would let the people that gate refuses add
    // themselves to it — the "the gate must cover the verb that removes the
    // gate" rule. Since 2026-08-10 that write is also LIVE
    // (`AgentRegistry::set_allowed_users`), so an ungated version would not
    // even cost the attacker a restart.
    //
    // ⚠️ The reason recorded here until 2026-08-10 was "it rewrites a live
    // agent's `system_prompt`", and that was never true: the tool accepted a
    // `system_prompt` argument that `AgentPatch` had no field for and no
    // surface ever persisted. The argument has been cut; the gate stands on
    // `allowed_users` alone, which is the stronger of the two anyway.
    "agent_update",
    // `agent_unbind` drops a channel→agent binding, i.e. it edits routing. A
    // chat-tier participant severing the binding that decides which agent
    // answers a channel is a config change wearing a conversation's clothes.
    "agent_unbind",
    "channel_pairing",
    "hub_install_run",
    // `runtime_manage{install}` runs `ensure_capability`, i.e. the ledger's
    // bootstrap installers — an npm global install, a `curl … | sh` script, a
    // winget invocation — plus their post-install subcommands. One call
    // installs software on the host. Its three nearest siblings
    // (`skill_install`, `skill_manage`, `hub_install_run`) are already here for
    // the same reason. Deliberately NOT split so `list` stays open: this table
    // matches on the tool NAME, and a chat-tier run that wants to know what is
    // installed has `doctor`, which is open.
    "runtime_manage",
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
    // `hooks_manage` is a control-plane write — adding a shell / HTTP / agent
    // hook fires arbitrary code or POSTs tool I/O to an arbitrary URL on the
    // next lifecycle event. The `HooksManage` ActionType policy already covers
    // the *content* of any added hook, but the chat-tier channel gate exists
    // so a chat-tier run cannot add a hook in the first place.
    "hooks_manage",
    // The governance graph's only write path. Two reasons it sits with
    // `hooks_manage` rather than with the read-only self-management tools:
    // `enable_audit` / `pair` create cron jobs (the capability `cron_manage`
    // is listed for), and a `root:` node's body is re-injected verbatim into
    // every governed session's system prompt on every turn — a persistent
    // prompt-injection surface. The Auto-tier argument card
    // (`ExecTier::asks_for_arguments`) is the only other thing standing in
    // front of it, and on a channel the human it asks IS the chat-tier
    // participant making the request.
    "loop_graph",
    // Workspace records. The `workspace.` RPC family has been admin-gated
    // since 2026-08-08, after real-machine QA watched a member rename and then
    // archive a workspace the operator had just created — both returning `ok`.
    // The tool face is the same verbs over the same seam
    // (`gateway::agent_env::ops`), so leaving it open would reopen exactly that
    // finding one surface over. `"member"` and `"guest"` both fail
    // `turn_context::role_is_operator`, so this one entry covers both.
    "workspace_manage",
    // The tool face of the wholesale-gated `plugin.` / `plugins.` RPC family
    // (`method_admin::ADMIN_PREFIXES`), and exactly the same shape as the
    // `workspace_manage` entry above. `plugin_manage` became dispatchable on
    // 2026-08-19 and its actions are the same verbs one surface over:
    // `trust_enforce` turns owner-trust enforcement off install-wide,
    // `marketplace_add` registers and fetches an arbitrary catalogue,
    // `config_set` REPLACES a plugin's stored configuration, `enable`/`reload`
    // decide what loads next. A default-tier Telegram bot calling
    // `trust_enforce(false)` is issuing the same request that answers
    // AUTH_REQUIRED over JSON-RPC.
    //
    // The gate keys on the tool NAME (`ScopedToolService::check_operator_gate`),
    // so this covers the read-only actions too. That is the honest trade: if
    // `list` / `show` / `trust_status` must stay chat-reachable they need
    // their own read-only tool name — not a carve-out inside a tool whose
    // other arms rewrite install-wide trust.
    "plugin_manage",
    // `terminal` LOOKS like the `*_list`/`*_status`/`*_read` shape this
    // list's own doc comment says is deliberately absent — it is not one of
    // those. Read-only self-management (`config_audit`, `gateway_route`)
    // discloses THIS server's own config; `terminal` discloses *another
    // principal's* live terminal screen. The repo already made this exact
    // call one face over: `"runtime."` sits in `method_admin::ADMIN_PREFIXES`
    // with the reasoning "read-only agent panel over the same PTY sessions
    // `pty.` gates — a session id, its cwd, and what is running in it, seen
    // through a different lens. Same disclosure, same gate." `terminal` is a
    // THIRD lens on that identical disclosure (herdr runtime port, phase 1,
    // Task 11) — leaving it open would make the tool face a lower-privilege
    // bypass of a decision `pty.*`/`runtime.*` already made twice.
    // Before deleting this entry to "clean up" an apparent `*_read`
    // exception: re-read the paragraph above, not just this line.
    //
    // ⚠️ The escalation card that membership here triggers
    // (`gate_chain::GateRule::OperatorRequired::reason`, in
    // `tools/scoped/gate_chain.rs`) reads "… which changes Aleph's own
    // configuration. Approve to allow this change." That sentence is false
    // for `terminal` — a read-only tool that changes nothing. Nothing is
    // disclosed by it today only because `terminal`'s own inline gate
    // refuses the call anyway even after a human approves that card
    // (`caller_is_operator()` reads an unchanged `TurnContext` — see
    // `terminal.rs`'s module doc). The card's TEXT is still wrong for a
    // read-only tool, and the fix belongs in `gate_chain.rs`, not in this
    // list or in `terminal.rs` (task-11 review F1).
    "terminal",
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
        // Control-plane write: adding a hook fires code on future events.
        "hooks_manage",
        // The tool half of an admin-gated RPC family. Pinned because the two
        // gates are different mechanisms: deleting this entry would not make
        // any `workspace.` RPC test go red, it would only widen the tool.
        "workspace_manage",
        // A third lens on the disclosure `pty.`/`runtime.` already gate
        // operator-only on both their faces (session id, cwd, live screen
        // contents). Deleting this entry would not redden any `pty.*` or
        // `runtime.*` test — it would silently expose another principal's
        // terminal content through the one face nothing else gates.
        "terminal",
        // Writes `allowed_users` — the list the run-start gate
        // (`caller_may_act_as_agent`) reads. Ungated, the people that gate
        // refuses could add themselves to it, and nothing in `handlers::agent`
        // would go red: the gate would still be enforced, faithfully, against
        // a list its own subjects had edited.
        "agent_update",
    ];

    #[test]
    fn config_tools_require_operator() {
        for t in MUST_STAY_GATED {
            assert!(tool_requires_operator(t), "{t} must require operator");
        }
    }

    /// A tool that duplicates an admin-gated RPC family must be operator-gated
    /// too, or the same verbs answer AUTH_REQUIRED on one surface and run on
    /// the other.
    ///
    /// The expectation is DERIVED from the other face's own predicate
    /// (`method_admin::method_requires_admin`) rather than restated: adding a
    /// name to the curated `MUST_STAY_GATED` subset above cannot catch a tool
    /// face nobody has thought about yet, which is how `plugin_manage`
    /// shipped ungated while every `plugin.*` RPC was closed. Each pair names
    /// a method the census pins as registered, so a renamed family goes red in
    /// `method_census` rather than silently making a row here vacuous.
    #[test]
    fn every_tool_face_of_an_admin_rpc_family_is_operator_gated() {
        // (builtin tool name, a registered method of the RPC family it duplicates)
        const TOOL_FACES: &[(&str, &str)] = &[
            ("plugin_manage", "plugin.enable"),
            ("workspace_manage", "workspace.create"),
            ("hooks_manage", "hooks.add"),
            ("skill_manage", "skills.remove"),
            ("skill_install", "skills.install"),
            ("node_manage", "cluster.enroll"),
        ];
        for (tool, rpc) in TOOL_FACES {
            assert!(
                super::super::method_admin::method_requires_admin(rpc),
                "precondition: `{rpc}` is supposed to be the admin-gated RPC \
                 half of `{tool}`. If it stopped being admin-gated this row \
                 proves nothing and the pairing needs re-deciding, not deleting"
            );
            assert!(
                tool_requires_operator(tool),
                "`{tool}` is the tool face of the admin-gated `{rpc}` family, \
                 so a chat-tier channel can run over the tool what it is \
                 refused over JSON-RPC"
            );
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
    fn installing_a_runtime_is_operator_only() {
        assert!(
            tool_requires_operator("runtime_manage"),
            "runtime_manage installs software on the host; it sits with \
             skill_install and hub_install_run, not with the read-only tools"
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
            "select_model",
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

    #[test]
    fn config_tools_have_a_model_pick_branch() {
        // `select_model` mutates session model state and persists it across
        // turns (a non-trivial blast radius). Historically the channel gate
        // had a special-case for `moa:` model picks only; any other model
        // name was implicitly trusted. Tightening the policy requires an
        // audit entry; this tripwire keeps the audit honest if the carve-out
        // ever narrows back to "moa: only".
        assert!(
            !tool_requires_operator("select_model"),
            "select_model currently stays open to chat tier; revisit if the \
             blast-radius story changes"
        );
    }
}
