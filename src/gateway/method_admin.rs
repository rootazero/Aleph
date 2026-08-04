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
//! Every method here was found via `registry.register("...")` /
//! `handlers_mut().register("...")` / `register_handler!(...)` /
//! `reg!(...)` call sites across `src/gateway/handlers/mod.rs` (the
//! placeholder registry built at `HandlerRegistry::new()`) AND the real
//! boot-time wiring in `src/bin/aleph-server/commands/start/**` (which
//! overrides most placeholders with live handlers — several families,
//! e.g. `gateway.token.rotate`, `providers.*`, `mcp.*`, `secrets.*`, are
//! ONLY registered there, never in `handlers/mod.rs`). The brief's seed
//! prefix list named `hub.` as the extension-install family; no `hub.`
//! method is registered anywhere — the real family is `extensions.*`
//! (`extensions.catalog/installed/toggle/uninstall/disclosure/install`),
//! so `hub.` was replaced with `extensions.` below.
//!
//! Classification rule applied per family: server-global config /
//! credentials / fleet / user management ⇒ admin; surfaces scoped to the
//! caller's own data (or needed for basic chat/tool operation) ⇒ open.
//! `connect` / `chat.*` / `sessions.*` / `memory.*` / `projects.*` /
//! `artifacts.*` are member daily surfaces and deliberately absent — their
//! per-user filtering is P1's visibility chokepoint, not this gate's job.
//!
//! Full per-family table (including families deliberately left OPEN, with
//! rationale) is in the Task 4 report, not duplicated here — this file is
//! the enforcement source of truth, not the audit trail.

/// Method prefixes that mutate or expose server-global state. A prefix match
/// gates the whole family so newly-registered siblings are gated by default
/// (fail-closed for privilege); carve-outs below re-open member-safe reads.
const ADMIN_PREFIXES: &[&str] = &[
    // --- Gateway trust boundary: tokens, tickets, devices, credentials ---
    "gateway.", // token.{current,rotate}, ticket.create, devices.{list,revoke},
    // identity.get, metrics.*, credentials, flow.reload — no read-only
    // member-safe carve-out exists in this family (verified: no
    // `gateway.status` method is registered anywhere).
    // --- Principal / fleet management ---
    "users.", // principal management (carve-outs: me / list) — not yet
    // registered (lands in Task 5); gated pre-emptively so the gate
    // precedes the surface.
    "cluster.", // enroll / deregister — fleet membership.
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
    // --- Agent / persona configuration (server-global, not per-user) ---
    "identity.", // agent SOUL.md / persona file admin (get/set/clear/list).
    "moa.",      // shared mixture-of-agents presets (save/delete/setDefault/…).
    "acp.",      // agent-client-protocol integration presets/config.
    // --- Fleet lifecycle / process control ---
    "daemon.",      // status/logs/shutdown — shutdown affects every connected caller.
    "wizard.",      // first-run setup wizard (walks through server-wide config).
    "diagnostics.", // diagnostics.run — whole-host diagnostic dump (paths, env, …).
];

/// Member-safe reads inside otherwise-admin families.
const MEMBER_CARVE_OUTS: &[&str] = &[
    "users.me",   // a member reading their own principal record.
    "users.list", // project roster picking needs the member list.
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
    /// a curated pin, not a second source of truth. One representative
    /// method per family in `ADMIN_PREFIXES`, each confirmed registered by
    /// the Task 4 enumeration (or, for `users.*`, pre-emptively pinned ahead
    /// of Task 5's surface).
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
            // cluster.
            "cluster.enroll",
            // providers. family
            "providers.create",
            "embedding_providers.add",
            "generation_providers.create",
            // channels. / channel. / discord.
            "channels.set_agent",
            "channel.create",
            "channel.pairing.revoke",
            "discord.validate_token",
            // config. + settings-page *_config. families + secrets.
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
            // fleet lifecycle
            "daemon.shutdown",
            "wizard.start",
            "diagnostics.run",
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
            "tools.invoke",
            "agents.list",
            "teams.list",
            "workspace.list",
            "voice.transcribe",
            "fs.read_file",
            "group_chat.start",
            "graph.query",
            "dreaming.list_insights",
        ] {
            assert!(!method_requires_admin(m), "{m} must stay open to members");
        }
    }
}
