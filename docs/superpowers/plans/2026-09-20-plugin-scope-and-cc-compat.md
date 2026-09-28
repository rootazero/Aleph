# Plugin Scope + Claude Code Compat + MCP Face — Implementation Plan (master)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> This master file carries the goal, the global constraints, the phase order, the cross-phase wires and the lead's reconciliation rulings. **The tasks themselves live in five phase files next to this one** (`2026-09-20-plugin-scope-and-cc-compat/P1-lifecycle.md`, `P2P3-visibility-activation.md`, `P4-cc-compat.md`, `P5P8-cut-and-docs.md`, `P6P7-mcp-face.md` — 76 task headings, 80 units counting P4.7 a–e, plus the P0 spike). An implementer receives exactly one task from one phase file plus this master; a reviewer reads the task, this master and the spec section the task's coverage map names. `RECONCILIATION.md` is the full text of the rulings summarised below; `CONTRACT.md` is the interface contract the phase files were written against — **where a phase file's "Contract deltas" section disagrees with it, the phase file wins** (the deltas are the code facts).

**Goal:** Make every plugin registration on the running process reversible (effects in an `EffectScope`, views by derivation), give every capability a project-level visibility predicate, close the twelve execution-end Claude Code compatibility gaps, cut OpenClaw and the zero-client RPC surface, and expose Aleph as an MCP server so pi / dsh / Claude Code can mount it — without touching `src/harness/`.

**Architecture:** Approach C from the spec — "has an inverse → effect (`Disposer` pushed into the plugin's `EffectScope`, disposed in reverse order); recomputable from the registry → view (one derivation, recomputed only in `lifecycle.rs::after_transition`)". Six effect kinds in one fixed mount order (`registry_row`, `wasm_module`, `mcp_server`, `service`, `memory_extension`, `slash_command`); four lifecycle primitives (`mount` / `unmount` / `reload_plugin` / `reload`); `ScopeKey { Global, Project(root) }` on every registry row and one `visible_to` predicate on the five model-facing faces; `PluginStatus::Pending { waiting_on }` derived from declared dependencies and never timed out; an MCP face (`src/gateway/mcp_face/`, Streamable HTTP on the existing gateway listener) that reuses the gateway's connect rules, scoped dispatch and approval gate.

**Tech Stack:** Rust (tokio, serde, axum 0.8 already in tree, `futures::future::BoxFuture`, extism for WASM), existing `src/mcp/{jsonrpc,protocol,types}.rs` wire types, bash + python3 QA fixtures under `qa/`, no new crate dependencies (the one candidate, `wat` as a dev-dependency, is a fallback recorded in P1 open question 4 and needs the lead's approval).

**Spec:** `docs/superpowers/specs/2026-09-20-plugin-scope-and-cc-compat-design.md` (approved 2026-09-20; user rulings U1–U8 in its §0). Evidence: `docs/superpowers/specs/2026-09-20-plugin-scope-and-cc-compat-evidence/`.

## Global Constraints

Every task's requirements implicitly include this section.

- **Base commit** `3ddc1f2e7` (worktree HEAD `35e5f8bca` adds only the spec). Phase files cite `file:line` at `3ddc1f2e7`; a later phase that anchors on code an earlier phase moved cites the earlier phase's task instead (see "Phase order").
- **Worktree only:** `/Volumes/TBU4/Workspace/Aleph/.claude/worktrees/plugin-scope-round`, branch `worktree-plugin-scope-round`. Never touch `main`; merge (P9) only after the user approves. Submodules are initialised (`include_dir!` fails the build if `skills/` or `plugins/` is missing). Builds share ONE target dir with the main checkout (`/Volumes/TBU4/Workspace/Aleph/.cargo/config.toml`, discovered by parent walk, pins `target-dir`); QA stages build from this worktree immediately before running and nobody else builds meanwhile.
- **One agent owns the tree at a time.** Parallelism is for read-only reviewers only.
- **`src/harness/` = 0-line diff in every task** (R10). `git diff --stat 3ddc1f2e7 -- src/harness` must print nothing at every commit.
- **Redlines R1–R10 and principles P1–P8** of `CLAUDE.md` apply; in particular R7/P8 (no regex on natural language), P7 (`unwrap_or_else(|e| e.into_inner())` on std locks; `char_indices()` / `.get(..n)` on strings), P6 (delete, don't comment out).
- **Guards need a recorded red:** every census / round-trip / single-caller guard (G1–G6 in spec §3.5) ships with the mutation step in its task; the commit message records the red test name. A guard whose mutation did not turn it red is not done.
- **Same-commit doc rule (判据 §1):** a code change that falsifies a code comment or a `docs/reference/*` line edits that line in the same commit. P8 holds the sections that describe the *new* shape; P1–P6 tasks list the specific doc lines they carry.
- **Minimal trusted verification set** (from `CLAUDE.md`; run the ones the task names, and all six before a phase is declared done):
  ```
  cargo test -p alephcore --lib --no-run
  cargo test -p alephcore --bins
  cargo test -p alephcore --features test-helpers --test '*' --no-run
  cargo test -p aleph-panel --lib --no-run
  cargo check -p aleph-desktop-{macos,windows,linux}
  cargo clippy --workspace --all-targets        # after `just _stage-shell-placeholders`
  ```
  Read the `test result:` line, never a piped exit code (`cmd | tail` swallows it). One pre-existing red is known and not ours: `capability::census::tests::every_installed_global_is_a_capability_slot` (`src/capability/census.rs:823`, "1 raw + 49 slots = 50, not 49" — KNOWN RED BY ONE on main).
- **Commits:** English, `<scope>: <description>`, trailer `Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>`. Do **not** bypass git hooks on code commits (the `post-commit`/`post-checkout` hooks are graphify rebuilds).
- **Real-machine QA** (`qa/<name>/run.sh <stage>`): the server needs a fake API key or `tools.invoke` answers "boot phase 2" (`Mode: Simulated`); grep the log's `Mode:` line before concluding "not wired". QA runs on this worktree's binary.
- **`PluginId` is a bare `String`** everywhere; **`PluginStatus` keeps today's names** (`Loaded / Disabled / Blocked(String) / Error(String)` + new `Pending { waiting_on: Vec<String> }`, `Overridden` removed) — see "Deviations from the spec".
- **`cargo fmt -p alephcore` reformats ~100 unrelated files (including `src/harness/`)** — format only the files you touched (`rustfmt <file>`, and note it recurses into child modules).

## Phase order and what each later phase anchors on

`P0 → P1 → P2 → P3 → P4 → P5 → P6 → P7 → P8 → P9`. Within a phase, tasks run in numeric order.

| Phase | File | Tasks | Anchors on |
|---|---|---|---|
| P0 | `P4-cc-compat.md` §P0 | hook stdin live capture spike (throwaway; must run before P4.3 is committed) | `3ddc1f2e7` |
| P1 | `P1-lifecycle.md` | P1.1–P1.17: `effects/`, six producers, boot order, `lifecycle.rs`, caller rewiring, dead-body deletions, G1/G2/G3, integration test, `qa/plugins scope`, `projection.rs` comment | `3ddc1f2e7` |
| P2 | `P2P3-visibility-activation.md` §P2 | P2.1–P2.11: `visibility.rs`, `ScopeKey` on rows, five faces, integration test, `qa/plugins visibility` | P1 (`lifecycle.rs::load_all` record site, `slash_effect::plugin_command_skill_info`) |
| P3 | same file §P3 | P3.1–P3.5: `Pending`/`Overridden`, `readiness`, `refresh_readiness`, activation gate + doctor, G6 | P1 (`watch_server_starts`, `activation_settled`) |
| P4 | `P4-cc-compat.md` | P4.1–P4.15 in file order (P4.7 = a–e; **P4.15 runs before P4.13** because the `cc-cache` stage asserts it): hooks (exit 2, `updatedInput`, payload, events + G4, aliases, timeout), commands body, agents body, marketplace, ClaudeCache, `allowed-tools`, skills fields, CC `mcpServers ⇒ Mcp`, QA stages, acceptance table | P1 (`lifecycle.rs::admit` / `migrate_legacy_disabled_marker` / `build_record`, `slash_effect::plugin_command_skill_info`), P2 (`GlobalRoot` / `DiscoveryScope` / `ScopeKey::from_discovery` arms) |
| P5 | `P5P8-cut-and-docs.md` §P5 | P5.1–P5.8: OpenClaw, zero-client RPC groups, `mcp.*` eleven + cascade, phantom Node runtime, `AlephSkillSpec` deadline | P1 (`plugins.load/unload` already gone), P4 (`executeCommand` cascade already gone) |
| P6 | `P6P7-mcp-face.md` | P6.1–P6.9: config + derived default exposure minus a reasoned `DEFAULT_EXPOSE_EXCLUDES` (G5), session table, auth, `McpFace`, protocol, `/mcp` routes + remote limiter, server mount + boot, `packages/pi-aleph/`, live-apply `expose` | P1 (`lifecycle.rs::after_transition`, `projection.rs::tests::G3_PINNED`), P5 (`packages/plugin-sdk/` gone) |
| P7 | same file | P7.1: `qa/mcp_face/run.sh` five stages (+ the six-command verification set over the whole tree) | P1–P6 |
| P8 | `P5P8-cut-and-docs.md` §P8 | P8.1–P8.10: ARCHITECTURE, ALEPH_HUB, PLUGIN_SYSTEM, EXTENSION_SYSTEM, GATEWAY, HARNESS_PHILOSOPHY 第五课, FEATURE_LOCATOR (§3.10, §5.27, D.0.196–199, E triggers), CLAUDE.md, qa/README, archive | P1–P7 final text |
| P9 | — | merge to `main` after user approval: `git merge --no-ff worktree-plugin-scope-round`, then the six-command set on `main` | — |

## Cross-phase wires (the lead's reconciliation rulings — full text in `2026-09-20-plugin-scope-and-cc-compat/RECONCILIATION.md`)

These are the places where two phases touch one function. Each is owned by exactly one task; the other phase only consumes.

| Wire | Owner | Consumer | Ruling |
|---|---|---|---|
| `register_transient_servers` returns `(Disposer, Vec<(server_id, ServerStartReceiver)>)`; `ExtensionManager::watch_server_starts` (sync in P1, spawns one task per plugin, pushes its `JoinHandle` into `activation_watchers`) + `activation_settled()` in `lifecycle.rs` | P1.4 / P1.9 | P3.3 (turns the watcher into the `Pending` writer and makes it `async fn` — one call site in `mount_parsed` gains `.await`), P3.4 (awaits `activation_settled()` at boot, no timer) | R1.1, R3.2, R3.3 |
| `slash_effect::plugin_command_skill_info(&SkillRegistration) -> SkillInfo` — the one builder for plugin-command slash rows (replaces `tool_catalog_init.rs:214-247`) | P1.7 | P2.8 (adds `plugin_id`), P4.7b (projects `argument_hint` / `allowed_tools` / `model`) | R1.3, R2.2, R4.4 |
| `mount` admit gate (`plugins.toml` check + legacy `.disabled` migration) | P1.9 | P4.10 (`is_enabled_for(id, origin)`, migration skipped for `ClaudeCache`) | R1.4, R4.3 |
| `PluginRecord` construction in `lifecycle.rs::load_all` + `write_failed_row` | P1.9 | P2.3 (stamps `scope_key`) | R1.4, R2.1 |
| `after_transition()` — the only view recompute site | P1.9 | P6.4 adds `if let Some(face) = try_mcp_face() { face.notify_tools_list_changed(); }` and extends the G3 `PINNED` const | R1.5, R6.1 |
| `DiscoveryScope { Global(GlobalRoot), Project { root } }` on every `DiscoveredPath` (`(Project, no root)` is unrepresentable) and the wildcard-free `ScopeKey::from_discovery` | P2.3 | P4.10 adds `GlobalRoot::ClaudeCache` + the `DiscoveryScope::source()` / `DiscoverySource::ClaudeCache` / `from_discovery` arms — the compiler refuses to build until all exist (that red is P4.10's recorded mutation) | R2.4, R2.5, R4.3 |
| `plugins.load` / `plugins.unload` CUT (+ `PLUGIN_SYSTEM.md:481-483` paragraph) | P1.10 | P5.3 census only | R1.2, R5.1 |
| `plugins.executeCommand` + cascade (`execute_plugin_command`, `PluginLoader::execute_command`, `DirectCommandResult`, `ExecuteCommandParams`) CUT (+ EXTENSION_SYSTEM "Direct Commands" section) | P4.7d | P5.3 census only | R4.4, R5.1 |
| `load_runtime_plugin` / `unload_runtime_plugin` CUT | P1.11 | P5.3 census only | R5.2 |
| `ToolCatalog::unregister_skills(&[String])` — ids owned by the disposer (no plugin-id key on the catalog) | P1.7 | — (P2.8's `plugin_id` field is for visibility only) | R1.8 |
| `PLUGIN_SYSTEM.md:149-171` status table | P3.1 minimal truth fix | P8.3(a) final text, written against P3.1's | R3.4, R5.3 |
| `hook_event_name` = the spelling the hook was registered under | P4.4 (field on the registration) + P4.3 (payload) | — | U-b, R4.2 |
| `ALEPH_ACTIVATION_GATE=fatal` exported by every `qa/plugins` stage | P3.4 defines | P7 / P4.13 / P1.16 / P2.11 stages export it | P2P3 delta |

## User rulings taken during planning (2026-09-20, after the spec)

| # | Question | Ruling |
|---|---|---|
| U-a | The nine `mcp.*` RPCs with zero clients and no tool face | **All CUT** (with the two that had a tool-face twin ⇒ all eleven go; `start/stop/restart` return as a tool face when a consumer exists, R8) |
| U-b | `hook_event_name` spelling in the hook stdin payload | **Echo the spelling the hook was registered under** (`PreToolUse` ↔ `before_tool_call`); one field, one derivation; the alias table is for dispatch only |
| U-c | Expanded `/cmd` body | **Transient delivery**; the raw `/cmd args` stays the persisted user turn (keeps the session-title invariant at `inner.rs:390-395`) |
| U-d | MCP-face attended approval card vs. client timeouts | **Accept and document** the dangling-card shape (same as an aborted Panel run); "retire the card on handler drop" is a recorded follow-up |

## Deviations from the spec (decided in planning; the spec is not edited)

| Spec says | Plan does | Why |
|---|---|---|
| §3.3 `PluginStatus::{Active, Failed { step, reason }}` | Keeps today's `Loaded` / `Error(String)` (+ `Pending`, − `Overridden`); mount failure writes `Error("<step>: <reason>")` at one site | A rename with no behaviour that ripples into `shared/protocol` and the Panel; the step label is preserved in the string |
| §3.1 `Disposer` returns `()` | `Result<(), String>` | Three of the six inverses report failure; `EffectScope::dispose` logs `step + error` once instead of every closure knowing its own label |
| §3.7 "at least `2025-03-26` and the newest version in `src/mcp/protocol.rs`" | Speaks `["2025-11-25", "2025-06-18", "2025-03-26"]`; `2026-07-28` (`src/mcp/modern/`) is a handshake-less dialect and is **not** spoken (`server/discover` → `-32601`, so pi-mcp-adapter's `auto` probe falls back to `initialize`) | A second dispatch path; deferred |
| §3.7 principal `McpClient { client_name }` | No principal enum exists to extend; `McpClient` is the session record, authority is the connect-resolved `(user, role)` | Code fact |
| §3.7 "no operator UI ⇒ `isError`" via `OperatorApprovalRequester`'s zero-subscriber deny | Implemented via an operator-presence probe over the connection table feeding the existing unattended arm | The zero-subscriber deny never fires on a real server (17 internal bus subscribers) — a finding recorded in FL D.0 |
| §3.3 `waiting_on` from declared deps incl. `runtime:<name>` | Only `mcp:<server_id>` (+ `mcp:manager` when no handle) | `.mcp.json` never sets `requires_runtime` — a consumer arm with no producer (判据 §7) |
| §3.4 "a session with no project sees Global only" | `VisibilityCtx::for_session()` = the run's workspace override (task-local), else the daemon's canonicalised CWD — the hooks' existing derivation, reused as the contract required | One derivation, not two; a daemon started inside a project root therefore treats project-less sessions as that project's. `qa/plugins visibility` starts the daemon outside the planted project so the stage proves the fail-closed half |
| §3.7 `expose` change → `list_changed` | Delivered by P6.9 (`mcp_server.expose` joins `LIVE_SUBSECTIONS`; `[mcp_server].enabled` stays `Restart`) | As promised; only `enabled` needs a restart |
| §3.7 default exposure "一组无副作用能力" | Derived predicate (read-only ∩ builtin − operator-tier − `desktop_*`) **minus** a reasoned `DEFAULT_EXPOSE_EXCLUDES` (`tool_usage`, `config_audit`, `node_list`, `user_profile`) = 36 names pinned name-by-name | Three of the four pass the predicate but disclose server config / fleet / the operator's profile to a foreign agent; conservative default, opt-in by name |
| §3.6 #3 `hook_event_name` (open in the spec) | Echoes the spelling the hook was registered under (U-b) | One field on the registration, one derivation; CC scripts and Aleph-native scripts both keep working |
| §3.9 `command.execute` is "a permanent error stub" | It is wired at startup (`tool_catalog_init.rs:475-483`); the CUT stands on the zero-client ground only | Evidence correction |
| §3.9 OpenClaw grep 286 lines / 156 files | 245 / 128 for `openclaw\|clawhub` (the larger count included bare `claw`); 16 code lines, 4 survive | Evidence correction |
| §7 "update the `src/extension/` routing row in CLAUDE.md" | There is no such row; P8.8 creates it | Code fact |
| Contract: `PluginRegistry::unregister_plugin` is new | It exists (`plugin_registry/mod.rs:432`) — reused | Code fact |

## Definition of done (spec §9, made checkable)

- Every task's tests green; the six-command set green at the end of each phase (except the one known census red-by-one, which P6.7's rostered slot may incidentally clear — if it does, say so in that commit).
- G1–G6 each have a recorded mutation red (commit message names the red test).
- Real-machine stages PASS on this worktree's binary: `qa/plugins/run.sh {scope, visibility, command, exit2, cc-cache}`, `qa/mcp_face/run.sh {handshake, tools, auth, list_changed, deny}`.
- P8 docs landed; `FEATURE_LOCATOR.md` §3.10 round entry + §5.27 + D.0.196–199 + E triggers; CLAUDE.md two routing rows + disallow-list line; `qa/README.md`.
- `git diff --stat 3ddc1f2e7 -- src/harness` is empty.
- The 70-item acceptance table (P4.14) has every row classified IMPLEMENTED / CONNECT (task id) / DEVIATION (reason).
