# MCP Store Unification — Design Spec

**Date:** 2026-06-21
**Status:** Approved (approach), pending spec review → implementation plan
**Supersedes the MCP portion of:** the "dual-store" follow-up note in `2026-06-21-hub-settings-sync-and-nav-design.md` (KEY FINDING)

## Goal

Make the Settings → MCP page operate on the **same live store** the Aleph Hub and the
runtime already use, so MCP servers sync bidirectionally between the two surfaces and
servers added via Settings actually run.

## Problem (verified)

MCP currently has two stores that do **not** sync — and one of them is dead:

| | Settings → MCP page | Hub + runtime |
|---|---|---|
| Store | `config.unified_tools.mcp` (in `~/.aleph/config.toml`) | `~/.aleph/mcp_config.json` (the `McpManagerActor`) |
| Written by | `mcp_config.create/update/delete` (`src/gateway/handlers/mcp_config.rs`) | Hub install (`src/hub/install.rs` → `mcp.add_server`), `mcp.*` handlers |
| Read by | only the Settings page (`mcp_config.list/get`) | `McpManagerActor::new(None)` at boot → `auto_start_servers()` → `tool_bridge.rs` → live tool registry |
| **Executed at runtime?** | **Never.** No startup path loads it into the actor. | **Yes** — sole source of truth for spawnable/callable MCP servers. |

Verification (grep + read, this session):
- `config.unified_tools.mcp` consumers are: `get_effective_tools_config()` (`src/config/methods.rs:23`, called **only by tests**), `enabled_mcp_servers()` (`src/config/types/tools.rs:403`, **zero callers**), and the `mcp_config.*` Settings handlers. None feed the actor.
- `src/bin/aleph-server/commands/start/mod.rs:229` constructs the actor via `McpManagerActor::new(None)` → defaults to `~/.aleph/mcp_config.json` (`src/mcp/manager/config.rs:74-79`). There is **no** code path that seeds the actor from `config.unified_tools.mcp`.
- The `unified_tools()` calls in `src/executor/builtin_registry/*` are `BuiltinToolRegistry::unified_tools()` (the runtime tool-metadata map) — unrelated to `config.unified_tools`.

**Consequence:** a server added on the Settings MCP page is listed but never starts and its
tools never reach the agent; a Hub-installed server runs but is invisible in Settings. (Skills
and Plugins are unaffected — they already share one store, which is why only MCP was broken.)

## Decision

Confirmed with the user:

1. **Approach A — Unify on the actor store.** Repoint the Settings `mcp_config.*` handlers at the live `McpManagerActor` (`~/.aleph/mcp_config.json`); stop reading/writing `config.unified_tools.mcp`. (Rejected: B — bidirectional mirror, two-sources-of-truth drift; C — config.toml as source, largest rewrite + plaintext-secrets-in-toml.)
2. **Secrets via the vault.** Settings-entered secret env vars are stored in the encrypted vault as `{{secret:NAME}}` references (the existing `src/secrets/` + `src/hub/secrets.rs` pipeline), never plaintext on disk.
3. **Migrate once.** Existing `config.unified_tools.mcp` entries are imported into the actor store on first boot after upgrade (idempotent), then cleared from `config.toml`.

### Sub-approach A1 (repoint, do not replace)

A full actor-backed `mcp.*` surface already exists (`src/gateway/handlers/mcp.rs`:
`handle_list/add/update/delete/status/start/stop/restart`, DTO = `McpManagerConfig`). Two
options to make Settings use the actor store:

- **A1 (chosen):** rewrite the **bodies** of `mcp_config.*` to delegate to `McpManagerHandle` (+ vault), keeping the panel's existing wire contract (`mcp_config.list/get/create/update/delete`) and its secret-redaction UX. Leave `mcp.*` untouched (it has other consumers).
- A2 (rejected): repoint the panel at `mcp.*` and delete `mcp_config.*`. Bigger panel rewrite; `mcp.*` lacks the Settings page's secret redaction/"blank keeps it" UX; `mcp.list` returns only lightweight `McpServerInfo` (no command/args/env), so editing would require `mcp.status` round-trips.

A1 is the smaller, surgical change and keeps the handlers as pure I/O (R4). Accepted
trade-off: `mcp.*` and `mcp_config.*` become two thin CRUD surfaces over the same store;
optional future consolidation is a non-goal here.

## Architecture

**Before:** `Settings → mcp_config.* → config.unified_tools.mcp (dead)` while
`Hub/runtime → McpManagerActor → mcp_config.json (live)`.

**After:** a single store.

```
Settings page  ─┐
Hub install    ─┼─► McpManagerHandle ─► McpManagerActor (~/.aleph/mcp_config.json) ─► tool_bridge ─► runtime
manual delete  ─┘            (+ vault for secret refs)
```

Every behavior the user asked for then falls out for free:
- Hub-installed MCP server appears in Settings (Settings list reads the actor store).
- Manual advanced config in Settings is persisted and **actually runs** (actor is runtime truth).
- Deleting in Settings removes it from the actor store → Hub's `extensions.catalog` reconcile shows it un-installed (`collect_installed` already reads `mcp.list_servers`).
- A non-Hub server typed into Settings installs and runs like any other.

## Components to change

| File | Change |
|---|---|
| `src/gateway/handlers/mcp_config.rs` | Rewrite handler bodies: `handle_list/get` read `McpManagerHandle::list_servers` + `get_status` (config detail); `handle_create/update/delete` delegate to `add_server`/`add_server`(upsert)/`remove_server`. Map panel DTO ↔ `McpManagerConfig`. Route secret env vars through the vault. Drop all `config.unified_tools` access. |
| `src/bin/aleph-server/commands/start/builder/handlers/settings.rs` (~385-407) | Pass `McpManagerHandle` + `Arc<SharedTokenManager>` (vault) into the `mcp_config.*` registration (currently gets only `config` + `event_bus`). Both already exist in the builder (the extensions install handler receives the same vault + handle). |
| `interfaces/webchat/src/api/mcp.rs` | Add `id: String` to `McpServerInfo`; send `id` in `get/update/delete` params; on create derive nothing (server assigns id). DTO stays stdio+remote-shaped. |
| `interfaces/webchat/src/views/settings/mcp.rs` | Use `id` as the row identity (was `name`); keep showing `name` as the label. No behavioral redesign. |
| `src/bin/aleph-server/commands/start/mod.rs` (after actor construction, ~line 230+) | One-time migration: import `config.unified_tools.mcp` → actor store, lift plaintext secrets into the vault, then clear the source section and persist. Warn-only on failure (must never abort boot, matching the existing actor-init guard). |

No new crates. No second async runtime. No platform-API access. (Tech-stack guardrails hold.)

## Wire / DTO mapping

Panel `McpServerInfo`/`McpServerConfig` (stdio-centric: `command`, `args`, `env`, `enabled`,
`requires_runtime`, `cwd`) ↔ actor `McpManagerConfig` (`id`, `name`, `transport`, `command`,
`args`, `url`, `env`, `requires_runtime`, `auto_start`, `timeout_seconds`, `tool_filter`):

- `id` ← panel `id` (Hub servers: `aleph-hub_github`; Settings-created: sanitized from `name` at create via the same char rule as `src/hub/secrets.rs::sanitize`).
- `name` ← panel `name` (display).
- `command`/`args`/`env`/`requires_runtime` ← direct.
- `transport` = `stdio` for command-based entries; preserve `http`/`sse` + `url` if present (do not drop remote servers when round-tripping a Hub-installed remote MCP).
- `enabled` ↔ `auto_start` (see below).
- `cwd`: panel-only field with no actor equivalent — out of scope; drop it from the panel DTO rather than silently lose it on save. (It is not currently honored at runtime either.)
- `timeout_seconds`/`tool_filter`: preserved on round-trip, not surfaced in the Settings UI (no new UI).

## Secret model

Mirror the Hub install path exactly (`src/hub/install.rs::mcp_config_from_spec`):

- **Write (create/update):** for each env var whose key matches the existing secret heuristic (`is_secret_env_key` in `mcp_config.rs`: KEY/SECRET/TOKEN/PASSWORD/PASS/CREDENTIAL) **and** has a non-blank submitted value → `SharedTokenManager::store_secret` under `field_key(ExtensionKind::Mcp, id, env_name)`; write `secret_ref(name)` (`{{secret:NAME}}`) into the actor config's `env`. Non-secret vars: plaintext as today. Blank secret on update = keep the existing `{{secret:NAME}}` ref already in the actor config (the new form of today's `merge_secret_env`).
- **Read (list/get):** the actor config `env` holds `{{secret:NAME}}` refs for secrets; the handler blanks/redacts them for display (panel already shows "saved — blank keeps it"). Non-secret values shown plaintext.
- Resolution at spawn is unchanged (`src/mcp/manager/secret_resolver.rs` resolves `{{secret:}}` into the child env only). **No parallel secret scheme is introduced.**

## Migration (one-time)

On first boot after upgrade, for each entry in `config.unified_tools.mcp`:
1. If an actor-store server with the same `id` already exists → skip (actor store wins; idempotent).
2. Build an `McpManagerConfig` from the entry; for any plaintext secret-keyed env value, store it in the vault and replace with a `{{secret:NAME}}` ref (so migration does not copy plaintext into `mcp_config.json`).
3. `add_server` it (respecting `enabled` → `auto_start`).
4. On success, remove the entry from `config.unified_tools.mcp`.

After processing, persist `config.toml` once (entries removed). Clearing the source is the
migration marker: re-running is a no-op, and a user later deleting a migrated server cannot
have it resurrected on the next boot. Partial failures leave their source entries in place and
log a warning; boot continues regardless.

## Enabled semantics

- Settings `enabled` ↔ actor `auto_start` (persisted "starts on boot" intent). `enabled=false` → `add_server` with `auto_start=false` (stored, not started).
- Runtime health (Healthy/Stopped/Degraded…) and the Hub toggle's `start_server`/`stop_server` (runtime-only, non-persisted) are a separate axis. Fully reconciling "enabled" across the Hub badge (derived from health) and Settings (auto_start) is an explicit **non-goal** of this spec.

## Non-goals

- Consolidating `mcp.*` and `mcp_config.*` into one surface.
- Unifying the "enabled" computation across Hub and Settings.
- Adding an enable/disable toggle button to the Settings MCP page (none today).
- Any change to Skills/Plugins stores (already correct).
- Remote (http/sse) MCP secret-header injection (already a separate Hub follow-up).

## Testing strategy

- **Unit (handlers, `cargo test -p alephcore --lib`):** panel↔actor DTO mapping (stdio + remote round-trip, id derivation, enabled↔auto_start); secret write routes to vault ref + blank-keeps-existing; list/get redacts secret refs.
- **Unit (migration):** idempotent skip when id exists; plaintext secret → vault ref; source cleared after import; partial-failure leaves source entry + does not abort.
- **Runtime e2e (isolated `ALEPH_HOME` + spare port, as in the prior follow-up):** create via `mcp_config.create` → appears in `extensions.installed` and `mcp.list_servers`; delete via `mcp_config.delete` → drops from both and Hub catalog reconcile flips `installed:false`; Hub install → appears in `mcp_config.list`. (Note `~/.aleph/mcp_config.json` is `dirs::home_dir()`-based and NOT isolated by `ALEPH_HOME` — back up/restore it.)
- **No** full `cargo test` (pre-existing broken `tests/cancellation_chain.rs`); scope to `--lib`. At most one `cargo check -p alephcore --bin aleph-server` before merge (build is memory-heavy).

## Redline / guardrail compliance

- **R4 (interface = pure I/O):** handlers only map DTOs and call the actor handle + vault; no business logic. Secret routing mirrors the existing Hub install boundary handling.
- **R7/P8 (LLM sovereignty):** pure config plumbing; no inference, no regex intent parsing.
- **Secrets:** reuses `src/secrets/` + `src/hub/secrets.rs`; no parallel scheme.
- **Gateway auth (`src/gateway/CLAUDE.md`):** `mcp_config.*` is config-class and must remain operator-gated (remote Chat-tier Panel cannot call it). No auth/authz/origin logic changes; verify the method gating during implementation, update tests if touched.
- **Tech stack:** no new crate, no second async runtime, no platform-API crate.

## Open questions (for spec review)

1. Migration: clear the migrated entries from `config.toml` (chosen, prevents resurrection) vs leave them inert? 
2. `cwd`: drop from the panel DTO (chosen — no runtime support) vs keep as display-only?
3. Keep `mcp.*` as-is (chosen) vs fold Settings onto it and retire `mcp_config.*` later?
