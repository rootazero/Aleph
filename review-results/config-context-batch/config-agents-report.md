# Severed-Wire Audit — `src/config/agent_manager/` + `src/config/agent_resolver/`

**Scope:** `agent_manager/{mod,crud,agent_files,toml_ops}.rs`, `agent_resolver/{mod,templates}.rs` (tests.rs read for context only, not counted as a production consumer).
**Method:** static read-only review + repo-wide grep for every producer/consumer pair. No edits made.

## Summary

This subsystem is in noticeably good health. Every CRUD method, every RPC handler, every
identity-file write surface, and every `AgentPatch` field has a live, correctly-ordered
consumer. I found **zero CRITICAL/HIGH severed wires**. One genuine MEDIUM-severity issue
(duplicate boot-time resolution, not a wiring break) and one LOW documentation nit. Several
things that *look* like severed wires at first grep turned out to be false positives once
traced to their actual registration site — documented below so the next auditor doesn't
re-walk the same dead ends.

---

## Phase 1 — Seam scan results

### 1. Registration parity (`register_agents_handlers` and friends)

`register_agents_handlers` (`src/bin/aleph-server/commands/start/builder/handlers/agents.rs:3-117`)
registers: `agents.list`, `agents.get`, `agents.create`, `agents.update`, `agents.delete`,
`agents.set_default`, `agents.files.{list,get,set,delete}`, `agents.tools_schema`,
`runtimes.install`. Called once, at `src/bin/aleph-server/commands/start/mod.rs:1777`.

**Verified: the placeholder-overwrite pattern the previous audit flagged is still correct.**

- `GatewayServer::with_config(...)` — which constructs the `HandlerRegistry` and its
  `"agents.*"` / `"runtimes.install"` placeholder handlers (`gateway/handlers/mod.rs:852-926`,
  each returning `INTERNAL_ERROR "... — wire in Gateway startup"`) — runs at
  `start/mod.rs:185`, well before `register_agents_handlers` at line 1777.
- `HandlerRegistry::register` is a plain `HashMap::insert` (`gateway/handlers/mod.rs:971-980`),
  so the later call is a real overwrite, not a race.
- `agents.bindings` (placeholder at `gateway/handlers/mod.rs:508`) is correctly overwritten by
  `register_workspace_handlers` → `register_handler!(server, "agents.bindings", ...)`
  (`start/builder/handlers/settings.rs:76-81`), also called after server construction.
- `agents.teams` (placeholder at `gateway/handlers/mod.rs:749`) is correctly overwritten by
  `register_teams_handlers` (`start/builder/handlers/agents.rs:300-306`).

No stale placeholder is reachable at runtime for any method this file registers.

### 2. CRUD parity (`AgentManager`)

| Producer | Consumer | Status |
|---|---|---|
| `AgentManager::list()` — `crud.rs:182` | `handle_list` — `gateway/handlers/agents.rs:138` | ✅ wired |
| `AgentManager::get()` — `crud.rs:188` | `handle_get` — `gateway/handlers/agents.rs:168` | ✅ wired |
| `AgentManager::create()` — `crud.rs:201` | `handle_create` — `gateway/handlers/agents.rs:211` | ✅ wired, plus hot-registers into `AgentRegistry` when `AgentsRuntimeCtx` is present |
| `AgentManager::update()` — `crud.rs:261` | `handle_update` — `gateway/handlers/agents.rs:289` | ✅ wired, plus live `allowed_users` sync via `AgentRegistry::set_allowed_users` |
| `AgentManager::delete()` — `crud.rs:383` | `handle_delete` — `gateway/handlers/agents.rs:350` | ✅ wired, plus runtime eviction + binding cleanup |
| `AgentManager::set_default()` — `crud.rs:450` | `handle_set_default` — `gateway/handlers/agents.rs:400` | ✅ wired |
| `AgentManager::{list_files,read_file,write_file,delete_file}()` — `agent_files.rs:21,64,81,102` | `handle_files_{list,get,set,delete}` — `gateway/handlers/agents.rs:433,458,486,511` | ✅ all four wired |

Every `AgentPatch` field (`name`, `identity`, `skills`, `skills_blacklist`, `subagents`,
`allowed_links`, `allowed_users`, `model`) has a corresponding branch in
`AgentManager::update()` (`crud.rs:280-372`) — full field-level parity, no silent drops.

The `agent_update` **tool** (LLM-facing face of the same verb,
`src/builtin_tools/agent_manage/update.rs`) intentionally covers a *subset* —
`name`/`description`/`model`/`allowed_users`. Its module doc states this explicitly
("Scope: model / name / description / allowed_users. Skills, allowed_links and
tool_permissions are still config-file-only.") and `patch_has_changes()` /
`collect_changed_fields()` are kept in lock-step with that stated scope. **This is a
documented, intentional gap, not a severed wire** — flagging only because a naive grep for
"does the tool support every `AgentPatch` field" would otherwise look like one.

### 3. Template parity

**Note on premise:** the mission brief asks to audit "every `AgentTemplate` and the resolver
that builds it." No `AgentTemplate` type exists anywhere in this codebase — that name belongs
to a different subsystem (`src/teams/templates/types.rs::TeamTemplate`, unrelated to agents).
What `agent_resolver/templates.rs` actually contains is a set of **static markdown template
functions/constants** (`default_soul`, `default_agents`, `default_identity`, `DEFAULT_MEMORY`,
`DEFAULT_TOOLS`, `DEFAULT_HEARTBEAT`) consumed by `initialize_agent_identity()`. I audited
parity for those instead:

| Template | Consumer |
|---|---|
| `templates::default_soul()` — `templates.rs:10` | `initialize_agent_identity()` — `mod.rs:436` | ✅ |
| `templates::default_agents()` — `templates.rs:17` | `mod.rs:438` | ✅ |
| `templates::default_identity()` — `templates.rs:120` | `mod.rs:442` | ✅ |
| `templates::DEFAULT_MEMORY` — `templates.rs:157` | `mod.rs:444` | ✅ |
| `templates::DEFAULT_TOOLS` — `templates.rs:160` | `mod.rs:445` | ✅ |
| `templates::DEFAULT_HEARTBEAT` — `templates.rs:207` | `mod.rs:446` | ✅ |

All six are called from the single `initialize_agent_identity()` function, which itself has
two live production callers: `AgentManager::create()` (`crud.rs:230`) and
`AgentDefinitionResolver::resolve_one()` (`mod.rs:274`, boot-time resolution of every agent).
No orphaned template.

The `SoulArchetype` enum (`expert | companion | assistant | maker`) flows end-to-end:
`CreateAgentParams.archetype` (RPC) / `AgentDefinition.archetype` (TOML) →
`AgentManager::create()` / `AgentDefinitionResolver::resolve_one()` →
`templates::default_soul`/`default_identity` → `compose_soul()`. Confirmed connected; no
finding.

### 4. RPC handler parity

Covered under §1/§2 above — every `agents.*` method registered by `register_agents_handlers`
has a live handler function, and every handler function is registered.

**False-positive I chased down and ruled out:** `runtimes.list` and `runtimes.refresh` are
registered directly inside `HandlerRegistry::new()` (`gateway/handlers/mod.rs:786-799`),
*not* as error placeholders — they build an ad-hoc `CapabilityLedger` per call and delegate
to `runtimes::handle_list`/`handle_refresh` (`gateway/handlers/runtimes.rs:57,69`), both of
which are fully implemented and unit-tested (`runtimes.rs:200-225`). Only `runtimes.install`
(needs `event_bus`) is a genuine placeholder, and it is correctly overwritten in
`register_agents_handlers` (`agents.rs:95-116`). Initial grep made this look like two
dead/unregistered handlers; tracing the actual registration disproved that.

### 5. Stub sweep

`grep -n "todo!\|unimplemented!\|TODO\|FIXME"` across all 6 in-scope non-test files: zero
hits (the one match, `templates.rs:74`, is the word "TODOs" inside prose describing what
*not* to save in curated memory — not a code stub). No empty match arms found in the CRUD or
resolver control flow.

---

## Phase 2/3/4 — Findings

### F1 — `AgentDefinitionResolver::resolve_all()` runs twice at every boot (MEDIUM, efficiency/drift-risk, not a severed wire)

- **Producer:** `AgentDefinitionResolver::resolve_all()` — `src/config/agent_resolver/mod.rs:156-195`
- **Consumer 1:** `src/bin/aleph-server/commands/start/mod.rs:795-799` — result used *only* to
  compute `default_agent_id` (line 803) and for a startup `println!` listing
  (lines 809-816); the `resolved_agents` Vec is then dropped.
- **Consumer 2:** `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:1269-1273`
  — result is what actually feeds `agent_registry.register_config(...)` (the live runtime
  registry the inbound router resolves against).
- **Severity:** MEDIUM
- **Triage:** DECIDE
- **Reason:** Both call sites resolve against what is, in practice, the same config
  (`loaded_app_config` at the first site is the same object later exposed as `app_config` to
  `agent_init`). `resolve_one()` is not a pure function — for **every** agent it does real
  filesystem I/O on every boot: `initialize_agent_dir` (`fs::create_dir_all`),
  `fs::create_dir_all(workspace_path)`, `initialize_agent_identity` (6× `write_if_missing`
  stat+maybe-write), an `IdentityFileLoader::load` for SOUL.md/AGENTS.md, and the lazy
  session-migration check (`old_sessions.is_symlink()` / `is_dir()` / `.migrated` marker
  probe). None of this is destructive on the second pass (idempotent via `write_if_missing`
  and the `.migrated` marker), so this is **not** a correctness bug today — but it is:
  (a) wasted I/O on every server start, scaling with agent count, and (b) a latent
  divergence risk: if a future change threads a *different* config snapshot into one call
  site than the other (e.g. a live-reload edit lands between the two calls), the
  `default_agent_id` computed at line 803 and the actual registered agent set from
  `agent_init` could silently disagree — which would misroute the default channel binding
  without either code path erroring.
- **Proposed fix:** Resolve once, in `start/mod.rs`, and thread the already-computed
  `resolved_agents: Vec<ResolvedAgent>` into `agent_init` (or the `AgentInitContext`/builder
  struct it already takes) instead of having `agent_init` re-derive it from `app_config`.
  This also removes the `if app_config.agents.list.is_empty() { /* legacy full_config path */ }
  else { /* re-resolve */ }` branch's dependency on a second resolver instance. Low-risk,
  mechanical change; not urgent enough to block anything, hence DECIDE rather than CONNECT.

### F2 — Doc-comment naming (`AgentTemplate`) doesn't match code (LOW, documentation)

- **Location:** the audit mission's own framing (Phase 1.3), not a code defect — noting here
  so it isn't rediscovered as a "missing type." No `AgentTemplate` struct/trait exists in
  `src/config/agent_resolver/` or anywhere in `src/config/`. If a future refactor intends to
  introduce a first-class `AgentTemplate` type (e.g. to unify the six string templates into a
  struct, mirroring `teams::templates::types::TeamTemplate`), that would be new work, not a
  reconnection of an existing severed wire.
- **Severity:** LOW
- **Triage:** N/A (nothing to fix; recorded for traceability)

---

## Phase 5 — Guard recommendation

No new guard is needed for the `register_*_handlers`-overwrites-placeholder pattern in this
subsystem — it is already correct and stable (verified against the previous audit's M1
concern). If a guard is wanted anyway for regression-proofing, the shape that would catch a
future break is a boot-order assertion test: construct `GatewayServer` (which seeds
placeholders), call `register_agents_handlers`, then assert
`server.handlers().get("agents.create")` is *not* the placeholder closure — e.g. by invoking
it with a params shape the placeholder would reject differently than the real handler (the
placeholder ignores params entirely and always returns the literal string
`"... — wire in Gateway startup"`; the real handler returns that exact substring nowhere).
This is a "does the seam still exist" test, not a business-logic test, and belongs next to
the existing `HandlerRegistry` tests in `gateway/handlers/mod.rs` (which already assert
`registry.has_method("agents.bindings")` at line 1232 for the presence check — extend that
family rather than adding a parallel one).

For F1, a cheap regression guard would be a boot-integration test asserting
`AgentDefinitionResolver::resolve_all` (or `resolve_one`) is invoked at most once per agent
per process — e.g. by counting `IdentityFileLoader::load` calls or filesystem `stat` calls
against `SOUL.md` during a single boot in a temp `ALEPH_HOME`. Not proposed as urgent; the
fix in F1 (thread the result through) makes the double-call structurally impossible rather
than needing a guard at all.

---

## What I did *not* find (explicitly ruled out, so it isn't re-chased)

- `is_curated_owned` / `curated_owned_reason` (the MEMORY.md write-guard) — confirmed shared
  by all three claimed write surfaces: `agent_manager/agent_files.rs:84,105`,
  `thinker/identity_files.rs:314-315`, `builtin_tools/self_config.rs:272-274`. No duplicate
  re-implementation, no missing surface.
- `tool_permissions` absent from `AgentPatch` / `agent_update` tool — confirmed intentional
  and documented in `agent_manage/update.rs`'s module doc; not a severed wire.
- `runtimes.list` / `runtimes.refresh` "looking unregistered" — false positive, see §4 above.
- No empty match arms, `todo!`, or `unimplemented!` anywhere in the 6 in-scope files.
