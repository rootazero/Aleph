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
//! current coverage boundary, not this gate's job either way). The RPC-side
//! chokepoint above has an event-bus-side sibling for the same reason:
//! `EventScopeGuard` (`event_scope.rs`) is role-based (admin-topic delivery
//! gating, independent of who owns the session), while owner-scoped WS event
//! delivery — the same per-user filtering this gate leaves to
//! `crate::gateway::visibility` for RPCs — is Task 8's
//! [`crate::gateway::event_visibility`], the 4th term in
//! `server::handler`'s `should_forward` filter chain.
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
    // identity.get, metrics.*, credentials, flow.reload. One carve-out since
    // 2026-08-07: `gateway.metrics.run_concurrency`, whose response is now
    // narrowed to the caller instead of refused (see MEMBER_CARVE_OUTS).
    // --- Principal / fleet / process management ---
    "users.", // principal management (carve-outs: me / list). Registered since
    // P0 Task 5 — `users.me/list/create/update` in `start/mod.rs`; the
    // "not yet registered, gated pre-emptively" note this comment carried
    // until 2026-08-09 described the day it was written, not the code.
    "cluster.", // enroll / deregister — fleet membership.
    // The READ face of that same fleet (`environments.list`): node ids, names,
    // tags, command inventories and last-seen times for every machine in the
    // cluster. Gating `cluster.` alone gated the two WRITES and left the
    // enumeration open; the Panel compensated with a client-side
    // `is_operator()` check, which could never have been the enforcement point
    // — that role is captured once at connect and `restamp_live_connections`
    // can invalidate it without telling the client. Prefix-gated (not
    // method-gated) so a future `environments.get`/`.describe` is gated by
    // default. The delivery-side half is `event_scope.rs`'s `node.` rule:
    // `node.connected`/`node.disconnected` carry the same ids and names, so
    // gating only the RPC would leave a member who hand-subscribes `node.**`
    // reading the fleet live. Neither half implies the other.
    "environments.",
    "services.", // background service lifecycle (start/stop/list/status) —
    // server process control, not caller's-own-data.
    // --- Agent persona management: server-global roster, not per-user ---
    "agents.", // create/update/delete/set_default/bindings/files.*/tools_schema/
    // teams — carve-outs: agents.list / agents.get (spec §7: "agent 人格
    // 目录 v1 保持全局、admin 治理"; read-only roster browsing stays open,
    // matching the tool-tier gate's own asymmetry — `agent_create`/
    // `agent_delete`/`agent_switch` are OPERATOR_TOOLS in method_authz.rs
    // while `agent_list` is explicitly chat-safe there).
    // The SECOND face of the same `AgentEnvStore` (2026-08-08). `agents.*` was
    // gated on the reasoning above while `workspace.*` — create/list/get/
    // update/archive over the very same `agent_envs` table — stayed open to
    // members, which is the "one verb, two faces, only one of them gated"
    // shape. The 2026-08-08 real-machine QA exercised the write half: a member
    // renamed and then archived a workspace the operator had just created,
    // both returning `ok`. `visibility::partition_visible` cannot close that —
    // a workspace id is a user-chosen name (`"crypto"`) encoding no owner and
    // `agent_envs` has no owner column, so an ordinary id passes for every
    // caller (see `handlers/workspace.rs`, which still calls it as defense in
    // depth against partition-COMPOSED ids and says so at each site).
    //
    // Gated whole, with no carve-out, because every client of this family is
    // already an operator surface. There are two: `interfaces/cli/src/commands/
    // workspace_cmd.rs` (`aleph workspace list|get|create|update|archive|
    // unarchive`), reaching the server over loopback/IPC, and — since
    // 2026-08-09 — the Panel's `/settings/workspaces` page. A
    // `MEMBER_CARVE_OUTS` entry for the reads would therefore be a
    // zero-consumer opening (R10 YAGNI), not a preserved capability.
    //
    // This used to read "The Panel has NONE", citing
    // `interfaces/webchat/src/api/workspace.rs`'s own doc that `workspace.list`
    // was dead and removed. True when written, false now. The ruling stands
    // because what carries it was never the client COUNT but whether any of
    // them is a member surface — and that page is not one. It renders for
    // anybody, calls, and reports this gate's refusal through
    // `components::admin_refusal` rather than showing a member an empty list;
    // the Panel deliberately holds no client-side role predicate to hide itself
    // with (see that module's doc for why pre-emptive hiding is the same gate
    // under a new name).
    //
    // `get`/`update` had no client at all until 2026-08-08 and were listed here
    // as such; they were CONNECTed rather than CUT because `update` is the only
    // metadata edit a workspace has. `archive` is a soft delete that keeps the
    // id taken — and that half did NOT change when `unarchive` landed
    // (2026-08-09). A workspace id is also the directory name under
    // `~/.aleph/agents/<id>/`, which holds that workspace's notes vault and
    // memory partition, so a `create` that reclaimed an archived id would hand
    // a brand-new workspace the previous one's memory. Both arrived already
    // inside this gate, which is why the ruling above did not have to be
    // reopened.
    //
    // NOT an owner column: that would build a permission model for per-user
    // workspaces, a capability no surface currently lets a member use. If that
    // product ever lands, the column layers on top of this gate rather than
    // replacing it.
    "workspace.",
    // --- Provider & channel configuration (shared, server-global) ---
    "providers.",            // LLM provider CRUD + credentials.
    "embedding_providers.",  // sibling of providers. — embedding backend CRUD.
    "generation_providers.", // sibling of providers. — image/speech gen backend CRUD.
    "channels.",             // channel config (set_agent, list, status).
    "channel.",              // singular: per-channel create/delete/start/stop/send/
    // pairing_data/health, plus the `channel.pairing.*` sub-family
    // (list/approve/reject/approved/revoke — who may DM the bot at all).
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
    // tools. It now ALSO covers the OPERATOR_TOOLS family `method_authz.rs`
    // already rules operator-only (`cron_manage`, `hooks_manage`,
    // `agent_identity`, `agent_create`, `moa`, …) — P1 member hardening
    // (Task 9) added a third hard floor to `handle_invoke` that reuses the
    // identical predicate (`method_authz::tool_requires_operator` +
    // `turn_context::role_is_operator`) the agent loop's own gate uses, so a
    // member can no longer reach `cron_manage` here to schedule a run that
    // executes with CALLER_ROLE=None (= trusted internal). `tools.invoke` is
    // carved open below (`MEMBER_CARVE_OUTS`); the family stays gated for the
    // siblings (`tools.catalog`/`tools.effective`/`tools.cancel_call`/
    // `tools.in_flight`) — narrowing those is a separate, not-yet-made
    // decision, and fail-closed-for-privilege is still this list's default
    // for anything not individually carved out. ---
    "tools.",
    // --- Exec configuration. The two APPROVAL methods are carved out below
    // (2026-08-08); everything else in the family stays operator-only, so a
    // future `exec.*` sibling is gated by default. ---
    //
    // The old comment here read: "a member resolving these is a privilege
    // escalation over the approval gate itself", with "No carve-outs". Both
    // halves of that were closed — this list refused the resolution RPCs and
    // `event_scope.rs` refused delivery of the cards — and the note observed
    // approvingly that "neither implies the other". Closing both is what made
    // the gate structurally unusable for the person it gates: a member's run
    // blocked on a confirmation they could neither see nor give, waited out the
    // 120-second timeout, and the documented way to get work done was to send
    // `exec_tier: "full"`. The safest tier was unusable and the least safe one
    // was not — a permission system that pushes its users toward the wide
    // setting has inverted its own purpose.
    //
    // What made a member resolving an approval an escalation was the ABSENCE of
    // a predicate, not the verb. The handlers now filter by
    // `visibility::session_visible_to`: a caller sees and answers exactly the
    // approvals raised for sessions they can already see, which is strictly
    // less than the run they are already permitted to start. Fleet approvals
    // (empty `session_key`, raised by a cluster node over reverse RPC) remain
    // operator-only on both faces.
    "exec.", // exec.approval.resolve, exec.approvals.pending, exec config.
    // --- Persisted agent-trace replay (final-review round — C2). A trace is
    // a full transcript (prompts, tool inputs, tool outputs) keyed only by
    // `run_id`, with no owner recorded anywhere in the trace store, so this
    // family was an enumeration oracle (`trace.list` returns EVERY task_id in
    // the process) feeding arbitrary-id readers. Gated whole so a future
    // sibling is gated by default; `trace.by_runs` — the Panel's
    // session-hydration path, the only member-reachable one — is carved out
    // below and owner-scoped in its own handler instead. `trace.list` /
    // `trace.get` have no member surface: their only callers are the operator
    // debugging commands `aleph trace list|get` (CLI) and the TUI's `/trace`.
    // See `handlers/trace_replay.rs`'s module doc for the full split. ---
    "trace.",
    // --- Offline memory maintenance. `dreaming.run_now` force-drives a full
    // cycle of the globally-registered daemon, and that cycle's corpus list is
    // `maintenance_corpora` — every `main__u-*` and `main__p-*` partition on
    // disk. It is an LLM-driven lint / consolidate / synthesize / ARCHIVE pass
    // over every other principal's notes, and there is no visibility predicate
    // anywhere in `src/memory/dreaming/`. Nothing gated it: this prefix did not
    // exist, so the family fell through to the default-open tail, while three
    // separate places in the tree (`handlers/dreaming.rs`'s module doc,
    // `dreaming/mod.rs:470` and `:800`) called it "the admin RPC" — a gate
    // three documents asserted and no code performed.
    //
    // Gated as a family so a future `dreaming.*` sibling is gated by default;
    // `dreaming.list_insights` is the read face and stays member-open (it is
    // pinned as such below), narrowed by its own partition predicate. ---
    "dreaming.",
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
    // P1 member hardening (Task 9): safe now that `handle_invoke` enforces
    // the OPERATOR_TOOLS gate itself (see the `tools.` comment above) —
    // identity propagation, not a hole. `Panel team_from_template` needs
    // only `invoke`; the siblings stay admin-gated (widen later on demand).
    "tools.invoke",
    // Final-review C2: the Panel calls this on EVERY session open to replay
    // tool calls into the transcript, so admin-gating it would break member
    // transcripts. It is owner-scoped in the handler instead (KeyChecked on
    // the addressed session, then the requested runs intersected with that
    // session's own) — see `handlers/trace_replay.rs::handle_by_runs`. Its
    // siblings stay gated; this carve-out is as narrow as `tools.invoke`'s.
    "trace.by_runs",
    // The approval gate's two faces, carved out 2026-08-08 so that the gate
    // works for the person it gates. Both are filtered in
    // `handlers/exec_approvals.rs` by the same visibility predicate the rest of
    // the perimeter uses, and both are registered in `method_visibility`
    // (ListFiltered / KeyChecked). This is the round's only carve-out that
    // hands a member a WRITE verb, which is why the refusal shape is
    // load-bearing: a foreign approval id gets the message a stale id already
    // produces, so a refusal cannot be used to enumerate.
    //
    // The reason this matters at all: `exec.` gated the whole family, which
    // made the default `Auto` tier a dead end for every member — any
    // non-idempotent tool call parked for the full approval window and then
    // died as `Timeout`, because the only principal allowed to resolve it was
    // someone else. Putting a member at a door they can neither see nor answer
    // does not constrain them; the documented workaround was `exec_tier:
    // "full"`, i.e. the least safe setting became the only usable one.
    //
    // ⚠️ These two literals appeared TWICE in this table until 2026-08-09,
    // with two justifications written by two rounds. A duplicate in a curated
    // security table is not merely untidy: `contains` answers the same either
    // way, so removing one copy looks like it changed nothing, and the next
    // reader cannot tell which comment is the live ruling. One entry, one
    // reason.
    "exec.approvals.pending",
    "exec.approval.resolve",
    // The same gate's peacetime face (2026-08-11): a member who answered
    // "allow for this session" must be able to SEE that grant and take it
    // back. Refusing them the list is the shape that made the resolve RPCs a
    // dead end for the population they gate — with the difference that a
    // revoke can only ever narrow authority, so the worst outcome of an
    // over-permissive revoke is one extra approval card.
    //
    // Scoped per row in `handlers/exec_grants.rs` by the SAME predicate as its
    // sibling above: a member sees only session grants for sessions already
    // visible to them, and no install-wide ("always") grant at all — they
    // cannot create one, and enumerating them would list the operator's
    // exceptions to somebody who cannot change them.
    "exec.grants.list",
    "exec.grant.revoke",
    // The Panel's cold-load seed for the sidebar running dot, and its usage
    // gauge. Admin-gating it (inherited from the `gateway.` family) meant both
    // were silently dead for every member — the RPC fallback that the
    // `stream.running_set_changed` event's own "members still need their own
    // red dot" rationale leaned on did not work for the population that
    // rationale is about. Safe now that the handler drops every identity from
    // the response for a scoped caller (2026-08-07, `ListFiltered` in
    // `method_visibility`): the two session-key arrays are narrowed to the
    // caller and `run_concurrency.per_agent` — which names the agent personas
    // holding live slots right now — is removed outright. What remains is
    // slot/queue COUNTERS. Siblings (`gateway.metrics.lanes`,
    // `gateway.metrics.subagent_concurrency`) stay gated — narrowing those is
    // a separate, not-yet-made decision.
    "gateway.metrics.run_concurrency",
    // The enumeration behind the composer's two session pills — the exec-tier
    // dial and the session-mode dial. Both pills READ this method and WRITE
    // through `sessions.patch` / `chat.send`'s per-request `exec_tier`+`mode`,
    // which are member daily surfaces and always have been. Gating only the
    // read left the door open and the menu locked: a member could set a session
    // tier over the wire but had no way to learn which tiers exist, so the tier
    // popover degraded to a lone "follow global" with a blank label and the
    // mode pill — whose own code hides itself on an empty `modes` — vanished
    // outright. Session mode is defined as a tool-PRESENTATION partition that
    // "grants and denies no permission" (CLAUDE.md), so refusing its id list
    // bought nothing at all.
    //
    // Safe because the handler narrows the response for a member
    // (`config::exec_permissions_value` vs `member_visible_permissions_value`):
    // the two server-global policy axes Settings → Policies edits — the
    // per-tool `overrides` map and its `default` — are dropped, leaving four
    // id enumerations (`exec_tier`/`tiers`/`mode`/`modes`). The write sibling
    // `config.update_tool_permissions` stays gated with the rest of `config.`;
    // this carve-out is a read of the dial positions, not a hand on the dial.
    "config.get_tool_permissions",
    // The read face of `dreaming.` — daily digests, synthesis notes and run
    // history for a corpus the caller can already see. Member-open since it
    // was written and pinned as such in the tests below; the family prefix
    // added in this round would otherwise have closed it, which would have
    // gated a read while leaving the write to be discovered later.
    // Its own narrowing is `visibility::partition_visible` in the handler.
    "dreaming.list_insights",
];

/// Server-global verbs that live inside a **deliberately open** family.
///
/// [`ADMIN_PREFIXES`] gates by family, which works when the family is
/// uniformly one thing. `memory.` is not: its module doc above calls it a
/// member daily surface whose per-caller narrowing is P1's job, and that is
/// right for `memory.search` / `memory.stats` / `memory.list_corrections`. It
/// is not right for the three maintenance verbs below, which do not read or
/// write the *caller's* data — they sweep **every corpus on disk**, including
/// every `main__u-*` and `main__p-*` partition:
///
/// - `memory.compress` iterates `SELECT DISTINCT agent_id FROM raw_memories`
///   and rewrites each one's raw memories.
/// - `memory.reembed` enumerates every corpus and re-embeds it; a member could
///   start an install-wide migration.
/// - `memory.reembed.cancel` kills whichever migration is running, including
///   one an operator started.
///
/// A per-caller *filter* cannot express this: there is no caller-owned subset
/// of "recompress the whole install". The question is admission, not
/// visibility — which is also why `method_visibility.rs` records `memory.reembed`
/// as having no filter to apply. That ruling is correct and says nothing about
/// who may call it; this table is the half it was silent on.
///
/// Checked **before** [`MEMBER_CARVE_OUTS`] so this is a floor rather than a
/// default: a name here cannot be re-opened by adding it to the carve-out list
/// on the other side of the file. The two tables naming the same method is a
/// contradiction, and `no_admin_method_is_also_a_carve_out` fails on it.
const ADMIN_METHODS: &[&str] = &["memory.compress", "memory.reembed", "memory.reembed.cancel"];

#[must_use]
pub fn method_requires_admin(method: &str) -> bool {
    if ADMIN_METHODS.contains(&method) {
        return true;
    }
    if MEMBER_CARVE_OUTS.contains(&method) {
        return false;
    }
    ADMIN_PREFIXES.iter().any(|p| method.starts_with(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two tables answer the same question with opposite answers, so a
    /// name in both is not a redundancy — it is a contradiction whose resolved
    /// value depends only on which `if` was written first.
    #[test]
    fn no_admin_method_is_also_a_carve_out() {
        for m in ADMIN_METHODS {
            assert!(
                !MEMBER_CARVE_OUTS.contains(m),
                "{m} is listed as both admin-only and a member carve-out"
            );
        }
    }

    /// An entry here is only worth its maintenance if the family around it is
    /// open — otherwise the prefix already said it, and the duplicate is a
    /// second place to drift when the prefix is later removed.
    #[test]
    fn every_admin_method_names_a_verb_inside_an_open_family() {
        for m in ADMIN_METHODS {
            assert!(
                !ADMIN_PREFIXES.iter().any(|p| m.starts_with(p)),
                "{m} is already covered by an ADMIN_PREFIX — drop the redundant entry"
            );
        }
    }

    /// The four server-global maintenance verbs, pinned by name.
    ///
    /// Each one sweeps every corpus on disk — every `main__u-*` and
    /// `main__p-*` partition — so "who may call it" is an admission question
    /// that no per-caller filter can answer. They were reachable by any member
    /// until 2026-08-13: `dreaming.` had no prefix at all, and the three
    /// `memory.*` verbs sat inside a family that is deliberately open because
    /// its *other* members really are per-caller reads.
    #[test]
    fn the_server_global_maintenance_verbs_are_admin_only() {
        for m in [
            "dreaming.run_now",
            "memory.compress",
            "memory.reembed",
            "memory.reembed.cancel",
        ] {
            assert!(method_requires_admin(m), "{m} must be admin-only");
        }
    }

    /// …and the per-caller reads beside them are untouched. Gating a family to
    /// close a write must not close the read a member's own page depends on —
    /// that is how a permission fix becomes a visible feature regression.
    #[test]
    fn the_member_daily_memory_surface_stays_open() {
        for m in [
            "memory.search",
            "memory.stats",
            "memory.list_corrections",
            "dreaming.list_insights",
        ] {
            assert!(!method_requires_admin(m), "{m} must stay member-open");
        }
    }

    /// A curated security table must not say the same thing twice.
    ///
    /// `MEMBER_CARVE_OUTS` carried `exec.approvals.pending` and
    /// `exec.approval.resolve` twice, each with its own justification comment
    /// written by a different round. `contains` answers identically either way,
    /// so nothing failed and nothing could — but a reader auditing the table
    /// cannot tell which comment is the live ruling, and deleting one copy
    /// looks like a no-op change to the reviewer too. Same for
    /// `ADMIN_PREFIXES`, where a duplicated prefix would hide a second,
    /// possibly contradictory, rationale.
    #[test]
    fn the_curated_tables_list_nothing_twice() {
        for (name, table) in [
            ("MEMBER_CARVE_OUTS", MEMBER_CARVE_OUTS),
            ("ADMIN_PREFIXES", ADMIN_PREFIXES),
        ] {
            let mut seen = std::collections::HashSet::new();
            let dupes: Vec<&str> = table
                .iter()
                .filter(|entry| !seen.insert(**entry))
                .copied()
                .collect();
            assert!(
                dupes.is_empty(),
                "{name} lists {dupes:?} more than once — each entry carries a justification \
                 comment, and two of them for one entry means one is unreviewed"
            );
        }
    }

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
            // cluster. / environments. / services.
            "cluster.enroll",
            // The fleet's read face. It lived outside `cluster.` all along and
            // was therefore open to every member until 2026-08-07; the Panel's
            // client-side `is_operator()` block was the only thing hiding it.
            "environments.list",
            "services.start",
            // agents. (fix round — Finding 1)
            "agents.create",
            "agents.update",
            "agents.delete",
            "agents.set_default",
            // workspace. — the second face of the same `AgentEnvStore`
            // (2026-08-08). All six are listed, not one representative:
            // `update`/`archive` are the two the real-machine QA actually
            // exercised as a member, and `list` moved out of
            // `member_daily_methods_stay_open` in the same change, so a
            // regression that reopens any one of them should fail by name.
            // The non-enumerating twin of this list — every `workspace.` method
            // the handler registry knows about — is
            // `handlers::tests::every_registered_workspace_method_is_admin_gated`,
            // which is what actually catches a method added after today.
            "workspace.create",
            "workspace.list",
            "workspace.get",
            "workspace.update",
            "workspace.archive",
            "workspace.unarchive",
            // providers. family
            "providers.create",
            "embedding_providers.add",
            "generation_providers.create",
            // channels. / channel. / discord.
            "channels.set_agent",
            "channel.create",
            "channel.pairing.revoke",
            // Registered 2026-08-09, having been advertised by the startup
            // banner since they were written without ever reaching the
            // dispatcher. `list` is named here rather than left to the
            // `channel.` prefix because it is an enumeration face: it returns
            // every pending sender id across every channel, so reopening it to
            // members should fail by name.
            "channel.pairing.list",
            "channel.pairing.reject",
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
            // exec approval — BOTH registered methods are carved open (a
            // member resolving their own parked call; see MEMBER_CARVE_OUTS),
            // so what is pinned here is the PREFIX, through a name that is
            // deliberately not registered. The family stays fail-closed, and a
            // future `exec.*` sibling has to be carved out on purpose rather
            // than inheriting the two narrow rulings made for these two.
            "exec.some_future_sibling",
            // direct tool execution (final-review round — C2). `tools.invoke`
            // is carved open below (Task 9, P1 member hardening — the handler
            // now enforces the operator gate itself); the siblings are
            // pinned here so the family stays whole and a future carve-out
            // has to be deliberate.
            "tools.catalog",
            "tools.effective",
            "tools.cancel_call",
            "tools.in_flight",
            // persisted trace replay (final-review round — C2)
            "trace.list",
            "trace.get",
        ] {
            assert!(method_requires_admin(m), "{m} must require admin");
        }
    }

    /// Final-review C2's exact shape: `trace.list` is the enumeration oracle
    /// and must be gated; `trace.by_runs` must stay open because the Panel's
    /// session hydration depends on it — its isolation is the handler's job,
    /// not this gate's.
    #[test]
    fn trace_by_runs_is_carved_open_but_its_siblings_stay_gated() {
        assert!(
            !method_requires_admin("trace.by_runs"),
            "trace.by_runs must stay open — the Panel replays tool calls with it \
             on every session open; it is owner-scoped in its own handler"
        );
        for sibling in ["trace.list", "trace.get"] {
            assert!(
                method_requires_admin(sibling),
                "{sibling} must be admin-gated — only trace.by_runs is carved out"
            );
        }
    }

    /// The `gateway.` family's one carve-out (2026-08-07). It exists because
    /// the response is NARROWED for a member rather than refused — the RPC half
    /// of the `stream.running_set_changed` projection — so the carve-out and
    /// `handle_gateway_metrics_run_concurrency`'s filtering are one decision:
    /// if that filtering is ever removed, this line has to go with it.
    #[test]
    fn run_concurrency_is_carved_open_but_its_gateway_siblings_stay_gated() {
        assert!(
            !method_requires_admin("gateway.metrics.run_concurrency"),
            "gateway.metrics.run_concurrency must stay open — it is the Panel's \
             cold-load seed for a member's OWN running dot, and it filters its \
             session-key arrays to the caller in the handler"
        );
        for sibling in [
            "gateway.metrics.lanes",
            "gateway.metrics.subagent_concurrency",
            "gateway.token.rotate",
            "gateway.devices.list",
        ] {
            assert!(
                method_requires_admin(sibling),
                "{sibling} must stay admin-gated — only \
                 gateway.metrics.run_concurrency is carved out"
            );
        }
    }

    /// The read of the two composer dials is carved open; every other way of
    /// touching `config.` — above all the WRITE sibling that sets the same two
    /// dials server-wide — stays gated. Reading a dial position is not a hand
    /// on the dial.
    #[test]
    fn the_tool_permissions_read_is_carved_open_but_its_write_sibling_is_not() {
        assert!(
            !method_requires_admin("config.get_tool_permissions"),
            "config.get_tool_permissions must stay open — it is the id \
             enumeration behind the composer's tier and mode pills, whose \
             write face (sessions.patch / chat.send) a member already has"
        );
        for sibling in [
            "config.update_tool_permissions",
            "config.set_tool_permissions",
            "config.patch",
            "config.get",
            "config.reload",
        ] {
            assert!(
                method_requires_admin(sibling),
                "{sibling} must stay admin-gated — only the tool-permissions \
                 READ is carved out"
            );
        }
    }

    /// Both `exec.*` methods are open to a member for the same reason: without
    /// them the default `Auto` tier is a dead end, because a member's own
    /// parked tool call had no principal allowed to resolve it and died at the
    /// approval timeout. Their owner-scoping lives in the handler, so this
    /// carve-out and that scoping are one decision.
    #[test]
    fn a_member_may_resolve_their_own_parked_tool_call() {
        for m in ["exec.approvals.pending", "exec.approval.resolve"] {
            assert!(
                !method_requires_admin(m),
                "{m} must stay open — it is how a member unblocks their OWN \
                 tool call; the handler scopes it to their own sessions"
            );
        }
        assert!(
            method_requires_admin("exec.some_future_sibling"),
            "the `exec.` prefix must stay fail-closed for methods nobody has \
             ruled on yet"
        );
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
            // `tools.invoke` (Task 9, P1 member hardening): carved open now
            // that `handle_invoke` enforces the OPERATOR_TOOLS gate itself —
            // see `member_role_is_denied_an_operator_tier_tool` in
            // `tools_invoke.rs` for the C2 regression this depends on. The
            // siblings (`tools.catalog` etc., pinned in the admin table
            // above) are unaffected.
            "tools.invoke",
            // The approval gate's two faces (2026-08-08). A member's own run
            // blocks on a confirmation; if they cannot see it and cannot answer
            // it, the run dies at the 120s timeout and the only way through is
            // the widest tier — so leaving these closed did not restrict the
            // member, it pushed them past the gate entirely. Filtered per
            // caller in the handler, fleet approvals still operator-only.
            "exec.approvals.pending",
            "exec.approval.resolve",
            "agents.list",
            "agents.get",
            "heartbeat.list",
            "heartbeat.get",
            "heartbeat.runs",
            "teams.list",
            // `workspace.list` was here until 2026-08-08. It moved into the
            // admin table above with the rest of its family — it had no member
            // consumer to keep open (the Panel's `workspace.list` call was
            // removed long ago), so leaving the read half open would have been
            // a zero-consumer hole next to a closed write half.
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
            // Its twin: resuming a session's interrupted run is the same
            // authorization question as starting one, and the run re-enters
            // under the SESSION's persisted owner/scope, never the caller's.
            // `handlers::resume` gates on `session_visible`, so a member can
            // only name a session it can already see — AND, since 2026-08-10,
            // on `caller_may_act_as_agent`, so seeing a session is no longer
            // enough to put its agent back to work. Until then this entry was
            // the one member-open way past the agent axis: the sentence above
            // claimed the two verbs ask the same question while only one of
            // them was asking it.
            "agent.resume",
            "trace.by_runs",
            "gateway.metrics.run_concurrency",
            "session.compact",
            "command.execute",
            "commands.list",
        ] {
            assert!(!method_requires_admin(m), "{m} must stay open to members");
        }
    }

    /// Pins the exact carve-out shape Task 9 relies on: `tools.invoke` is
    /// open, but its admin-gated siblings inside the same `tools.` family
    /// are not — the carve-out is narrow, not a family-wide reopening.
    #[test]
    fn tools_invoke_is_carved_open_but_its_siblings_stay_gated() {
        assert!(
            !method_requires_admin("tools.invoke"),
            "tools.invoke must be open — the handler enforces the operator gate itself"
        );
        for sibling in [
            "tools.catalog",
            "tools.effective",
            "tools.cancel_call",
            "tools.in_flight",
        ] {
            assert!(
                method_requires_admin(sibling),
                "{sibling} must stay admin-gated — only tools.invoke is carved out"
            );
        }
    }

    /// The 2026-08-08 real-machine QA, written down as a test.
    ///
    /// A member connected over the LAN renamed a workspace the operator had
    /// just created (`workspace.update`) and then archived it
    /// (`workspace.archive`); both returned `ok`. Neither call is refusable by
    /// `visibility::partition_visible` — the id was an ordinary name with no
    /// owner encoded in it — so the refusal has to come from this gate.
    ///
    /// Named after the ACTIONS rather than the family so a future reopening
    /// fails with the QA finding in the test name.
    #[test]
    fn a_member_cannot_rename_or_archive_the_operators_workspace() {
        for m in [
            "workspace.update",
            "workspace.archive",
            // Not exercised by that QA because it did not exist yet, but it is
            // the same verb class — a member who could reach it would be
            // resurrecting a workspace the operator retired.
            "workspace.unarchive",
        ] {
            assert!(
                method_requires_admin(m),
                "{m} must be admin-gated — a member reached it in the 2026-08-08 QA"
            );
        }
    }

    /// The whole `workspace.` family is gated, with NO carve-out — unlike
    /// `agents.`, whose read half (`list`/`get`) is member-visible.
    ///
    /// The asymmetry is deliberate and worth pinning: `agents.list`/`.get`
    /// have live member consumers (the Panel's persona roster), while every
    /// `workspace.*` read has none. That used to be the same sentence as "the
    /// CLI is the only client"; since 2026-08-09 there are two clients — the
    /// CLI and the Panel's `/settings/workspaces` page — and the sentence
    /// survives because BOTH are operator surfaces. A member opening that page
    /// gets this refusal and the page says so out loud
    /// (`components::admin_refusal`); it does not render an empty list, which
    /// is the failure that module exists to prevent. Carving the reads open
    /// still buys nothing.
    ///
    /// The original wording added that carving them open would "reopen
    /// `env_vars` / `system_prompt_override` / `allowed_tools` to every
    /// caller". That justification is retired as of 2026-08-09: those fields
    /// have no writer anywhere in the repo, so they were always empty, and they
    /// are no longer on the wire at all
    /// (`handlers::workspace::detail_of`). The gate is kept on the reason that
    /// survives inspection — a workspace is a **global** resource with no
    /// owner column, so every caller who reaches these methods can rename or
    /// archive a workspace any other user created. That is an argument about
    /// the write half, and since the reads have no consumer to protect there
    /// is nothing to trade against gating the family whole.
    #[test]
    fn the_workspace_family_has_no_member_carve_out() {
        for m in [
            "workspace.create",
            "workspace.list",
            "workspace.get",
            "workspace.update",
            "workspace.archive",
            "workspace.unarchive",
        ] {
            assert!(
                method_requires_admin(m),
                "{m} must be admin-gated — the family is gated whole"
            );
        }
        assert!(
            !MEMBER_CARVE_OUTS
                .iter()
                .any(|m| m.starts_with("workspace.")),
            "no workspace.* carve-out may be added without a member consumer to justify it"
        );
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
