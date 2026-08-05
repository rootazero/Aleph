//! Method-level admin authorization for the multi-user role gate (spec §4.6).
//!
//! Sibling of `method_authz` (the per-TOOL channel-tier gate). This one
//! classifies RPC METHODS: server-global configuration, credentials, fleet,
//! and user management are admin-only; everything scoped to the caller's own
//! data stays open to members. Enforced at ONE chokepoint inside
//! `process_request` — both WS dispatch paths (the `do_lane_dispatch`
//! closure and the idempotency `Proceed` arm in `server::handler`) scope
//! `CALLER_ROLE` around `process_request`, so a single check there covers
//! both.
//!
//! ## Enumeration evidence (Task 4, spec §4.6)
//!
//! Every method here was found by a MECHANICAL sweep (fix-round, not the
//! original hand sweep) of all four registration call patterns across the
//! whole `src/` tree (which includes `src/bin/`): `.register("literal")`
//! (covers both `registry.register(...)` in the placeholder registry built
//! at `HandlerRegistry::new()` and `handlers_mut().register(...)` at boot
//! time), `register_handler!(server, "literal", ...)`, and `reg!("literal",
//! ...)` (file-local macro in `builder/handlers/mcp.rs`). Non-RPC
//! `.register(...)` call sites (provider registries, tool registries,
//! background-agent trackers, cluster node-command registries, test-only
//! registries) were identified by source and excluded — they share the
//! method name `.register(` but register into a different table, not the
//! JSON-RPC `HandlerRegistry`. That sweep produced **74 method families**.
//! Every one of them has a ruling in this file: a family is admin iff it
//! prefix-matches [`ADMIN_PREFIXES`] and is not listed in
//! [`MEMBER_CARVE_OUTS`]; every other family is open, and the non-obvious
//! open rulings are written out below. There is no second table anywhere —
//! re-running the sweep above and diffing it against these two constants is
//! the whole audit.
//!
//! The brief's seed prefix list named `hub.` as the extension-install
//! family; no `hub.` method is registered anywhere — the real family is
//! `extensions.*` (`extensions.catalog/installed/toggle/uninstall/
//! disclosure/install`), so `hub.` was replaced with `extensions.` below.
//!
//! Classification rule applied per family: server-global config /
//! credentials / fleet / user management ⇒ admin; surfaces scoped to the
//! caller's own data (or needed for basic chat/tool operation) ⇒ open.
//! `connect` / `chat.*` / `sessions.*` / `memory.*` / `projects.*` /
//! `artifacts.*` are member daily surfaces and deliberately absent — their
//! per-user filtering is P1's visibility chokepoint, enforced in
//! [`crate::gateway::visibility`] and registered in
//! [`crate::gateway::method_visibility`] (`sessions.*`/`chat.*` land there
//! as of Task 6; `memory.*`/`artifacts.*`/`clarification.*`/`subagent.tree`/
//! `graph.query` are Task 7's follow-up — see that module's doc for the
//! current coverage boundary, not this gate's job either way).
//!
//! Two families were read (not guessed) and deliberately left OPEN despite
//! looking admin-shaped at first glance:
//!
//! - `fs.*` (Panel directory-picker browse/read/create) — every
//!   traversal/mutation is bounded by `ProjectsConfig::allowed_roots` and
//!   canonicalisation-checked before touching the filesystem; a path outside
//!   the configured roots never reaches `fs::read_dir`/`fs::create_dir` and
//!   returns `-32600`. See `src/gateway/handlers/fs.rs:14-17` (doc) and
//!   `:93-106` (`validate_in_scope`, called from every handler). This is the
//!   RPC-surface equivalent of member tool execution, which the trust model
//!   already concedes — not a server-global config surface.
//! - `clarification.*` / `subagent.tree` — genuinely member's-own-session
//!   surfaces by *purpose* (answering the agent's own parked question;
//!   viewing your own run's subagent tree for the Panel dashboard). Gating
//!   them here would break the Panel's ONLY path to answer a clarifying
//!   question and to view a running subagent tree (chat.*-equivalent member
//!   necessity), so they stay open at THIS gate — same precedent as
//!   `sessions.*`/`memory.*`. Their per-user visibility is enforced
//!   elsewhere: `crate::gateway::visibility`'s predicates, applied at each
//!   handler site (`gateway/handlers/clarification.rs`,
//!   `gateway/handlers/subagent.rs`), registered in
//!   [`crate::gateway::method_visibility`] (Task 7 entries — see that
//!   module's doc for the exact `Treatment` and coverage boundary).
//!
//! This file is the authoritative source for the classification — both the
//! enforcement and the audit trail. Nothing here defers to a report artifact.

/// Method prefixes that mutate or expose server-global state. A prefix match
/// gates the whole family so newly-registered siblings are gated by default
/// (fail-closed for privilege); carve-outs below re-open member-safe reads.
const ADMIN_PREFIXES: &[&str] = &[
    // --- Gateway trust boundary: tokens, tickets, devices, credentials ---
    "gateway.", // token.{current,rotate}, ticket.create, devices.{list,revoke},
    // identity.get, metrics.*, credentials, flow.reload — no read-only
    // member-safe carve-out exists in this family (verified: no
    // `gateway.status` method is registered anywhere).
    // --- Principal / fleet / process management ---
    "users.", // principal management (carve-outs: me / list) — not yet
    // registered (lands in Task 5); gated pre-emptively so the gate
    // precedes the surface.
    "cluster.",  // enroll / deregister — fleet membership.
    "services.", // background service lifecycle (start/stop/list/status) —
    // server process control, not caller's-own-data.
    // --- Agent persona management: server-global roster, not per-user ---
    "agents.", // create/update/delete/set_default/bindings/files.*/tools_schema/
    // teams — carve-outs: agents.list / agents.get (spec §7: "agent 人格
    // 目录 v1 保持全局、admin 治理"; read-only roster browsing stays open,
    // matching the tool-tier gate's own asymmetry — `agent_create`/
    // `agent_delete`/`agent_switch` are OPERATOR_TOOLS in method_authz.rs
    // while `agent_list` is explicitly chat-safe there).
    // --- Provider & channel configuration (shared, server-global) ---
    "providers.",            // LLM provider CRUD + credentials.
    "embedding_providers.",  // sibling of providers. — embedding backend CRUD.
    "generation_providers.", // sibling of providers. — image/speech gen backend CRUD.
    "channels.",             // channel config (set_agent, list, status).
    "channel.",              // singular: per-channel create/delete/start/stop/send/
    // pairing_data/health, plus the `channel.pairing.*` sub-family
    // (approve/approved/revoke — who may DM the bot at all).
    "discord.", // bot integration credentials/config (validate_token, list_guilds, …).
    // --- Server configuration surfaces (Settings page, one family per section) ---
    "config.",  // schema/get/patch/reload/validate/path/*_tool_permissions.
    "secrets.", // vault CRUD (list/set/delete/verify/providers) — literally credentials.
    "security_config.",
    "generation_config.",
    "memory_config.",
    "execution_config.",
    "fetch_config.",
    "general_config.",
    "behavior_config.",
    "browser_config.",
    "rerank_config.",
    "search_config.",
    "route_config.",
    "routing_rules.",
    "logs.", // getLevel/setLevel/getDirectory — setLevel mutates the process-wide
    // tracing verbosity for every caller; getDirectory discloses a server
    // filesystem path. No read-only carve-out: getLevel alone isn't worth
    // splitting out of a 3-method family for.
    // --- Extension / capability install surfaces ---
    "extensions.", // Aleph Hub install surface (catalog/installed/toggle/
    // uninstall/disclosure/install) — replaces the brief's placeholder
    // `hub.`, which no registered method matches.
    "mcp.",        // MCP server lifecycle (add/update/delete/start/stop/restart/…).
    "mcp_config.", // MCP Settings-page CRUD against the vault.
    "skills.",     // skill install/update/remove (status/update/install_dep/remove).
    "bundled.",    // bundled.sync — re-syncs the official skills/plugins snapshot.
    "plugins.",    // plugin lifecycle, legacy plural namespace.
    "plugin.",     // plugin lifecycle, canonical singular namespace (both registered).
    "hooks.",      // server-wide hook file admin (~/.aleph/hooks.json).
    "runtimes.",   // sandbox/runtime capability install (list/refresh/install).
    // --- Agent-persona / shared config (server-global, not per-user) ---
    "identity.", // agent SOUL.md / persona file admin (get/set/clear/list).
    "moa.",      // shared mixture-of-agents presets (save/delete/setDefault/…) —
    // matches `moa` in method_authz.rs's OPERATOR_TOOLS.
    "acp.", // agent-client-protocol integration presets/config.
    // --- Scheduled automation: cross-checked against method_authz.rs's
    // tool-tier gate, which already treats mutation here as operator-only
    // (`cron_manage`, `heartbeat_{create,update,delete,toggle}` are all
    // OPERATOR_TOOLS) — the RPC surface must not be a lower-privilege
    // bypass of that existing decision. ---
    "cron.", // no read-only LLM-tool counterpart exists (unlike heartbeat_list),
    // so no carve-out: the whole family is gated.
    "heartbeat.", // carve-outs: list/get/runs (heartbeat_list is explicitly
    // chat-safe in method_authz.rs; get/runs are read-only siblings).
    // create/update/delete/toggle/wake stay gated.
    // --- Fleet lifecycle / process control ---
    "daemon.",      // status/logs/shutdown — shutdown affects every connected caller.
    "wizard.",      // first-run setup wizard (walks through server-wide config).
    "diagnostics.", // diagnostics.run — whole-host diagnostic dump (paths, env, …).
    // --- Interactive shell: NOT the same trust story as the `bash` LLM tool ---
    "pty.", // full interactive terminal on the server host. Its own doc
    // comment ("open to all connections" — `src/gateway/handlers/pty.rs:10`)
    // is a pre-multi-user LAN-trust holdover. Unlike the `bash` tool (open
    // to chat tier in method_authz.rs), a PTY session is NOT mediated by
    // `[sandbox.command_policy]` pattern matching or exec-tier approval —
    // it is a raw shell, so the two are not comparable; this is strictly
    // more dangerous, not equally protected by a different layer.
    // --- Direct tool-execution RPC: same cross-check as `cron.`/`heartbeat.`
    // above, and the sharpest case of it. `tools.invoke` dispatches straight
    // off the raw `ToolRegistry` — its own module doc says so, and its own
    // hard floor exists precisely because none of the loop's gates run there.
    // That floor covers RCE / `requires_confirmation` / continuation-driven
    // tools, but NOT the OPERATOR_TOOLS family `method_authz.rs` already
    // rules operator-only (`cron_manage`, `hooks_manage`, `agent_identity`,
    // `agent_create`, `moa`, …), so a member could reach them here and, via
    // `cron_manage`, schedule a run that executes with CALLER_ROLE=None
    // (= trusted internal). Principle: an RPC surface must never be a
    // lower-privilege bypass of the per-tool operator gate. The whole family
    // is gated rather than carved down to `tools.invoke`: this surface is
    // E2E-test-oriented by its own module doc ("production callers should
    // still go through the agent loop"), the siblings are few
    // (catalog/effective/cancel_call/in_flight), and fail-closed-for-privilege
    // is this list's default. A member-safe read carve-out is a P1 decision
    // to make with a member Panel in hand, not a P0 guess. ---
    "tools.",
    // --- Exec-tier approval resolution: a member resolving these is a
    // privilege escalation over the approval gate itself. The delivery-side
    // half is `event_scope.rs`: `approval.*` / `surface.approval` are guarded
    // prefixes there, and a member no longer holds the `"*"` wildcard that
    // short-circuits every rule (`event_scope::scope_for_role` — the wildcard
    // is operator-only). Note the two halves are independent: that guard filters
    // *delivery* of the cards, this list refuses the *resolution* RPCs, and
    // neither implies the other. No carve-outs — ---
    "exec.", // exec.approval.resolve, exec.approvals.pending.
];

/// Member-safe reads inside otherwise-admin families.
const MEMBER_CARVE_OUTS: &[&str] = &[
    "users.me",       // a member reading their own principal record.
    "users.list",     // project roster picking needs the member list.
    "agents.list",    // browsing the shared agent-persona roster (read-only).
    "agents.get",     // reading a single persona's detail (read-only).
    "heartbeat.list", // matches chat-safe `heartbeat_list` in method_authz.rs.
    "heartbeat.get",  // read-only sibling of heartbeat.list.
    "heartbeat.runs", // read-only run history, sibling of heartbeat.list.
];

#[must_use]
pub fn method_requires_admin(method: &str) -> bool {
    if MEMBER_CARVE_OUTS.contains(&method) {
        return false;
    }
    ADMIN_PREFIXES.iter().any(|p| method.starts_with(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tripwire subset — mirrors method_authz::MUST_STAY_GATED's philosophy:
    /// a curated pin, not a second source of truth. This table intentionally
    /// near-mirrors `ADMIN_PREFIXES` one-for-one (one representative real
    /// method per family) rather than sampling a subset — for a security
    /// gate, a deletion or typo in `ADMIN_PREFIXES` should fail a test by
    /// name, not rely on a smaller sample happening to still cover it. Every
    /// method below is confirmed registered by the Task 4 enumeration (or,
    /// for `users.*`, pre-emptively pinned ahead of Task 5's surface).
    #[test]
    fn credential_and_config_methods_require_admin() {
        for m in [
            // gateway. (given)
            "gateway.token.rotate",
            "gateway.ticket.create",
            "gateway.devices.revoke",
            "gateway.devices.list",
            // users. (given — pre-emptive, Task 5 surface)
            "users.create",
            "users.update",
            // cluster. / services.
            "cluster.enroll",
            "services.start",
            // agents. (fix round — Finding 1)
            "agents.create",
            "agents.update",
            "agents.delete",
            "agents.set_default",
            // providers. family
            "providers.create",
            "embedding_providers.add",
            "generation_providers.create",
            // channels. / channel. / discord.
            "channels.set_agent",
            "channel.create",
            "channel.pairing.revoke",
            "discord.validate_token",
            // config. + settings-page *_config. families + secrets. + logs.
            "config.patch",
            "secrets.set",
            "security_config.update",
            "generation_config.update",
            "memory_config.update",
            "execution_config.update",
            "fetch_config.update",
            "general_config.update",
            "behavior_config.update",
            "browser_config.update",
            "rerank_config.update",
            "search_config.update",
            "route_config.update",
            "routing_rules.create",
            "logs.setLevel",
            // extension / capability install
            "extensions.install",
            "mcp.add",
            "mcp_config.create",
            "skills.remove",
            "bundled.sync",
            "plugins.install",
            "plugin.install",
            "hooks.add",
            "runtimes.install",
            // agent / persona config
            "identity.set",
            "moa.savePreset",
            "acp.create",
            // scheduled automation (fix round — method_authz.rs cross-check)
            "cron.create",
            "heartbeat.create",
            // fleet lifecycle
            "daemon.shutdown",
            "wizard.start",
            "diagnostics.run",
            "pty.spawn",
            // exec approval (fix round — Finding 3)
            "exec.approval.resolve",
            "exec.approvals.pending",
            // direct tool execution (final-review round — C2). `tools.invoke`
            // is the escalation vector (raw-registry dispatch, no operator
            // tool gate); the siblings are pinned too so the family stays
            // whole and a future carve-out has to be deliberate.
            "tools.invoke",
            "tools.catalog",
            "tools.effective",
            "tools.cancel_call",
            "tools.in_flight",
        ] {
            assert!(method_requires_admin(m), "{m} must require admin");
        }
    }

    #[test]
    fn member_daily_methods_stay_open() {
        for m in [
            "connect",
            "chat.send",
            "sessions.list",
            "users.me",
            "users.list",
            "projects.list",
            "memory.search",
            "artifacts.list",
            // NOTE: `tools.invoke` used to be pinned open here. It is now
            // admin-gated (see ADMIN_PREFIXES) — a raw-registry dispatch
            // surface must not be a lower-privilege bypass of the per-tool
            // operator gate. The pin moved to the admin table above.
            "agents.list",
            "agents.get",
            "heartbeat.list",
            "heartbeat.get",
            "heartbeat.runs",
            "teams.list",
            "workspace.list",
            "voice.transcribe",
            "fs.read_file",
            "fs.allowed_roots",
            "group_chat.start",
            "graph.query",
            "dreaming.list_insights",
            "clarification.resolve",
            "clarification.pending",
            "subagent.tree",
            "agent.run",
            "session.compact",
            "command.execute",
            "commands.list",
        ] {
            assert!(!method_requires_admin(m), "{m} must stay open to members");
        }
    }

    /// Prefix false-positive safety: a method whose name merely SHARES a
    /// prefix's leading characters, but does not have the trailing `.`, must
    /// stay open. `starts_with("gateway.")` on `"gatewayx.foo"` is false
    /// because the literal dot is part of the match — this pins that the
    /// trailing dot is load-bearing and not accidentally droppable.
    #[test]
    fn prefix_match_requires_the_trailing_dot() {
        for m in ["gatewayx.foo", "configx.get", "usersx.list", "mcpx.add"] {
            assert!(
                !method_requires_admin(m),
                "{m} must NOT match on a bare prefix without the trailing dot"
            );
        }
    }
}
