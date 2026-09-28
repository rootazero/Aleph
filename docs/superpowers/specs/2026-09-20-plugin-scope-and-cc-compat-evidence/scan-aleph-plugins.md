# Aleph plugin subsystem scan — 2026-09-20 (read-only scout)

Repo: `/Volumes/TBU4/Workspace/Aleph` @ `3ddc1f2e7` (main, clean). All `file:line` cites are against that commit.
Method note (判据 §18): LOC = `wc -l` on `*.rs`; hit counts = `rg -i -c` and are **per-line**, not per-token. Grep results only vouch for the shapes I enumerated (`openclaw|clawhub|claw`, RPC method string literals, `HookEvent::` call sites).

## 1. Inventory of the plugin subsystem today

Naming reality check: there is **no `src/plugins/`, `src/extensions/`, or `src/skills/`**. The host lives in `src/extension/` (singular), skills in `src/skill/` (singular), the catalogue in `src/hub/`. One `ExtensionManager` (`src/extension/mod.rs:147`) owns everything; `crate::skill::SkillSystem` is embedded inside it as a field (`src/extension/mod.rs:169`).

| Path | LOC | Responsibility (one line) | Public entry points | Producers → Consumers |
|---|---|---|---|---|
| `src/extension/mod.rs` | 1,884 | `ExtensionManager`: discovery → adapter parse → `PluginRegistry` → `HookExecutor` sync → projections | `new/with_defaults` :300/:361, `load_all` :526, `ensure_loaded` :782, `reload` :802, `reload_plugin` :1303, `set_mcp_handle` :410, `sync_mcp_plugin_servers` :462, `start_watcher` :874, `active_plugin_tools_snapshot` :1261, `resolve_active_plugin_tool` :1272 | Produced once at boot by `src/bin/aleph-server/commands/start/` and stored process-global via `manager_global.rs` (`init_extension_manager`/`try_extension_manager`). Consumers: tool dispatch, gateway handlers, builtin tools, hooks CLI |
| `src/extension/manifest/` | 7,470 (adapter.rs 410, cc_plugin_json.rs 728, cc_plugin_toml.rs 857, parsers.rs 1,516, types.rs 663, toml_types.rs 612, adapters/{aleph_toml 275, codex 223, cursor 350, auto_discover 218}, declared_sections 261, component_source 197, config_validation 175, manifest_cache 271, mod.rs 502) | Six-adapter `AdapterRegistry` (`adapter.rs:81`): CC TOML(100) → CC JSON(90) → Aleph TOML(85) → Codex(80) → Cursor(70) → auto-discover(-100); first `detect()` wins; `${CLAUDE_PLUGIN_ROOT}` expansion centralised at `adapter.rs:108-139` | `AdapterRegistry::parse_dir`, `parse_manifest_from_dir_cached_global` | Called only from `ExtensionManager::load_all`/`reload_plugin` |
| `src/extension/registry/` | 1,227 (plugin_registry/mod.rs 498, types.rs 702) | In-memory `PluginRegistry`: plugins + tools/hooks/commands/agents/skills registrations, diagnostics | `register_plugin`, `list_*`, `clear`, `get_plugin` | Written by `CapabilityApi`; read by projection, tool index, gateway list handlers |
| `src/extension/registrar/` | 980 (api.rs 375, mcp_registrar.rs 597) | `CapabilityApi` — the one write-path from adapter output into the registry, permission-gated; `mcp_registrar` registers plugin `.mcp.json` servers as **transient** MCP servers | `CapabilityApi::new/register_capability/reload` | `load_all`, `reload_plugin`, `sync_mcp_plugin_servers` |
| `src/extension/hooks/` | 5,010 (mod.rs 1,278, executor.rs 1,538, consent.rs 823, user_settings.rs 614, output_budget.rs 402, json_output.rs 349) | `HookExecutor`: 23 `HookEvent`s, Observer/Interceptor kinds, command/http/prompt/agent/plugin actions, CC stdin-JSON payload + stdout JSON/line-prefix decisions, shell-hook consent gate, user-level `hooks.json` loader | `execute_hooks`, `execute_interceptors` (executor.rs:892), `load_user_hooks` | Fired from **outside** `src/extension/` at 23/23 events — see §3 table |
| `src/extension/runtime/wasm/` | 2,676 | The **only** plugin runtime: wasmtime host functions, capability kernel, allowlist, credential injector, secret resolver | `PluginLoader` (`loader.rs` 731) | Kind `Wasm` only; `PluginKind` has 3 variants `Wasm/Mcp/Static` (`types/plugins.rs:125`) — **no Node.js runtime** (CLI comment `plugin_cmd.rs:19-27`) |
| `src/extension/marketplace/` | 2,706 (mod.rs 1,072, installer.rs 517, manifest.rs 254, source_spec.rs 249, github_source 152, local_source 102, types 295, names 65) | Claude-Code-style `marketplace.json` registrations: add/update/remove/browse/install from git/GitHub/local; `~/.aleph/plugins/known_marketplaces.json` | `MarketplaceManager` | `plugin.marketplace.*` RPC, `plugin_manage` tool, CLI |
| `src/extension/{plugin_state,plugin_trust,plugin_vars,plugin_secrets,plugin_ops,service_manager,service_ops,skill_ops,skill_tool,projection,scope,watcher,validation,template,capability,mcp_config}.rs` | 5,660 | Durable enable/disable (`plugins.toml`), owner trust allowlist, `${VAR}` expansion, per-plugin secrets, RPC-facing ops, long-running plugin services, skill projection, project scoping, hot-reload watcher, manifest validation, capability tiers | see file names | gateway handlers / builtin tools |
| `src/skill/` | 7,993 (mod.rs 1,222, manifest.rs 1,463, prompt.rs 743, installer.rs 618, frontmatter.rs 594, guard.rs 423, …) | Skill System v2: SKILL.md parsing (frontmatter incl. CC `allowed-tools`), eligibility, prompt XML injection, usage stats, install-guard scanning | `SkillSystem`, `shared_skill_system()`, `build_skills_prompt_xml` | Embedded in `ExtensionManager`; consumed by thinker prompt layers and `skill_*` tools |
| `src/domain/skill.rs` | 1,052 | Domain model `SkillManifest`/`SkillSource` (target of the 2026-05-20 unification) | — | `src/skill/`, `src/tools/markdown_skill/` (via `From`) |
| `src/tools/markdown_skill/` | 2,448 (executor 862, loader 426, spec 368, tool_adapter 334, watcher 271, parser 162) | **Deprecated** CLI-skill path: `AlephSkillSpec` (`#[deprecated(since="26.5.20")]` spec.rs:16) — the ONLY code that spells `OpenClawMetadata` | `skills.install` RPC → `markdown_skills.rs` | See §5 |
| `src/hub/` | 4,708 (install.rs 812, catalog_client 499, reconcile 474, origin 429, cache 403, trust 328, official_mcp 323, hub_catalog 312, verify 245, …) | Aleph Hub: one published catalogue over plugin/MCP/skill backends; install/verify/trust/origin ledger | `hub_*` tools, `extensions.*` RPC | `src/builtin_tools/hub/` (6 tools, 1,237 LOC), Panel Extensions view |
| `src/mcp/` | 19,063 | MCP client stack: transports (stdio/http/sse), auth/OAuth, manager actor, tool bridge, sampling, presets | `McpManagerHandle` | Plugin `.mcp.json` servers enter via `registrar/mcp_registrar.rs`; config via `mcp_config.*` RPC |
| `src/bundled/` | 1,656 | `include_dir!` of `skills/` + `plugins/` submodules; extract to `~/.aleph/` on version change; offline fallback for git clone | `extract_bundled_content`, `sync_official_now` | boot |
| `src/discovery/` | 1,154 | Directory scanning for skill/command/agent/plugin dirs by scope | `DiscoveryManager::discover_*` | `collect_plugin_dirs` (mod.rs:1016) |
| `src/memory/extensions/` | 2,429 | `MemoryExtensionRegistry`: plugins declaring `[memory]` become `McpMemoryExtension` | `set_memory_registry` (mod.rs:370) | memory retrieval |
| `src/gateway/handlers/plugins/` | 1,966 | RPC: `plugins.*` (legacy) + `plugin.*` (canonical) + `plugin.marketplace.*` + `plugin.config.*` | registered at `src/gateway/handlers/mod.rs:365-408` | Panel, TUI, CLI (see §7 for which have zero clients) |
| `src/gateway/handlers/{hooks_admin,skills,markdown_skills,mcp,services}.rs` | 2,469 | `hooks.*` (6), `skills.*` (5), `mcp.*approval` (3), `services.*` (4) | `handlers/mod.rs:356-361, 410-413, 524-535` | CLI/Panel |
| `src/bin/aleph-server/commands/start/builder/handlers/{extensions,mcp}.rs` | 209 | `extensions.{catalog,installed,toggle,uninstall,disclosure,install}` (Hub face) + `mcp_config.{list,get,create,update,delete}` | — | Panel Extensions view |
| `src/bin/aleph-server/commands/hooks.rs` | 312 | `aleph-server hooks test` — pipes CC payload to a hook's stdin | — | operator CLI |
| `src/builtin_tools/{plugin_manage,hooks_manage,skill_manage,skill_install,skill_status,mcp_login,mcp_prompt,mcp_resource}.rs` + `hub/` | 5,132 | R8 tool faces: `plugin_manage` (list/show/enable/disable/reload/config_get/config_set/trust*/marketplace_*), `hooks_manage`, `skill_*`, `mcp_*`, `hub_{catalog_search,catalog_sync,fetch_docs,install_run,install_verify,resolve_spec}` | `AlephTool` impls | model |
| `interfaces/webchat/src/platform/wide/views/settings/{plugins,skills}.rs` + `views/extensions/{browse,installed,model}.rs` + `components/extensions/{detail_drawer,install_flow}.rs` + `api/extensions.rs` | 3,930 | Panel: Settings→Plugins (plugins.*/plugin.marketplace.*), Settings→Skills (skills.*), Extensions Hub view (extensions.*), MCP config (mcp_config.*) | — | operator |
| `interfaces/cli/src/commands/{plugin_cmd,plugins_cmd,hooks_cmd,mcp_cmd,skills_cmd}.rs` | 2,162 | `aleph plugin {init,validate,pack,doctor}` (local, 1,171 LOC), `aleph plugins {list,install,uninstall,enable,disable,call,…}`, `aleph hooks *`, `aleph mcp *`, `aleph skills *` | — | **NB** `plugin_manage.rs:36-38`: "`interfaces/cli` — a binary the release workflow does not build" |
| `interfaces/tui/src/tui/commands.rs` | (1 file) | slash-command list only; no plugin panel | — | — |
| `packages/plugin-sdk/` | 906 LOC TS | `@aleph/plugin-sdk` v0.1.0 — TypeScript types for a "plugin IPC protocol" | npm package | **zero Rust consumers**; there is no Node runtime to speak the protocol (see §7) |
| `plugins/` (submodule) | 7 dirs: diagnostics, diff-viewer, llm-task, media-office, memory-analytics, phone-control, voice-call | Official plugins embedded via `BUNDLED_PLUGINS` | — | extracted to `~/.aleph/plugins/cache/aleph-official/` |
| `skills/` (submodule) | 37 dirs | Official skills embedded via `BUNDLED_SKILLS` | — | extracted to `~/.aleph/skills/` |
| `examples/plugins/media-video/` | 1 | Example with `onPreToolUse()` in JS (`src/index.js:61`) — targets a JS runtime that does not exist | — | nobody |
| `tests/{extension_watcher_integration,plugins_install_symlink_refused,memory_extensions_integration,mcp_scope_isolation,skill_status_test}.rs` | — | Integration tests | — | CI |

Total first-party Rust in the subsystem (extension + skill + hub + bundled + discovery + handlers + tools + markdown_skill): **≈ 58k LOC**, plus 19k in `src/mcp/`.

## 2. Plugin lifecycle as implemented

### 2.1 Discovery (`src/discovery/scanner.rs`, `src/extension/mod.rs:1016 collect_plugin_dirs`)

| Scope | Directory | Source tag / priority | Note |
|---|---|---|---|
| Claude global | `~/.claude/{skills,commands,agents}` | `ClaudeGlobal` / 0 | read-only compat (`scanner.rs:88-100`). **`~/.claude/plugins/` is NOT scanned** — `discover_plugins_with_extra` only walks `~/.aleph/plugins` + extras (`scanner.rs:214-224`) |
| Aleph global | `~/.aleph/{skills,commands,agents}` + `~/.aleph/plugins/**` (one-level monorepo) | `AlephGlobal` / 10 | |
| Project `.claude/` | upward walk to git root | `Project` / 20+ | |
| Project `.aleph/` | upward walk + `<project>/.aleph/plugins{,.local}` for every registered project (`mod.rs:1027-1041`) | `Project` / 40+ | daemon = union-of-all projects; **no per-project runtime isolation** ("a separate, deferred concern", `mod.rs:1024`) |
| Bundled | `~/.aleph/plugins/cache/aleph-official/` after `bundled::extract_bundled_content` | `PluginOrigin::Bundled` | via `include_dir!` |
| Marketplace cache | `~/.aleph/plugins/cache/<marketplace>/` → installed copy under `~/.aleph/plugins/installed/` (`installer.rs:22`) | | |

Canonical-path dedup, first-wins; a second copy of the same id is recorded as `shadowed` diagnostic on the winner (`mod.rs:556-580`) — `PluginStatus::Overridden` exists (`types/plugins.rs:181`) but the loser gets no row.

### 2.2 Manifest parsing — formats actually recognised

`AdapterRegistry::with_defaults` (`manifest/adapter.rs:81-92`), first `detect()` wins:

| Prio | Adapter | Detects | Files consumed |
|---|---|---|---|
| 100 | `ClaudeCodeTomlAdapter` (`cc_plugin_toml.rs`) | `plugin.toml` | Aleph-flavoured CC layout in TOML |
| 90 | `ClaudeCodeJsonAdapter` (`cc_plugin_json.rs`) | `.claude-plugin/plugin.json` (+ legacy root `plugin.json`) | `commands/*.md`, `agents/*.md`, `skills/*/SKILL.md`, `hooks/hooks.json`, `.mcp.json` (`declared_sections.rs`, `component_source.rs`, `parsers.rs`) |
| 85 | `AlephTomlAdapter` | `aleph.toml` | native |
| 80 | `CodexAdapter` | Codex CLI layout | |
| 70 | `CursorAdapter` | Cursor rules | |
| -100 | `AutoDiscoverAdapter` | bare `skills/`/`commands/`/`agents/` dirs | |

`marketplace.json` is parsed separately by `src/extension/marketplace/manifest.rs` (CC `marketplace.json` shape: `name`, `owner`, `plugins[]{name,source,description,…}`). **There is no OpenClaw/ClawHub adapter** in this registry and no `pi`-shaped adapter (§4, §5).

CC `commands/*.md` are stored as `SkillRegistration{skill_type: Command}` — "no parallel `CommandRegistration` type" (`capability.rs:50-53`).

### 2.3 Install

| Path | Entry | Mechanism |
|---|---|---|
| git URL | `plugins.install` → `handlers/install.rs:13 handle_install` | git2 clone into `~/.aleph/plugins/installed/<repo>`; symlink-leaf refusal (`ensure_plugin_destination_is_safe` mod.rs:1398, test `tests/plugins_install_symlink_refused.rs`) |
| zip (base64 over RPC) | `plugins.installFromZip` → `install.rs:217` | extract + `reload()` |
| marketplace | `plugin.marketplace.install` → `marketplace/installer.rs:22 install_plugin_from_cache` | copy from marketplace cache; `directory_digest` integrity |
| Hub | `hub_install_run` tool → `src/hub/install.rs:389 run_install` | consent-gated; origin ledger (`hub/origin.rs`) |
| npm | — | **ABSENT** (no `npm` in install paths; `plugin_cmd.rs:19-27` documents that Node plugins run as MCP stdio servers) |

Only `plugin_manage` (the model's face) deliberately cannot install/uninstall (`plugin_manage.rs:20-25`).

### 2.4 Enable / disable (`plugin_ops.rs:586 set_plugin_enabled`)

Durable answer = `<data_dir>/plugins.toml` (`plugin_state.rs`; legacy `.disabled` marker migrated at `mod.rs:632-661`). Disabled plugins stay **registered** with `PluginStatus::Disabled` so re-enable needs no reload (`mod.rs:664-680`). Owner-trust allowlist (`plugin_trust.rs`) gates `Workspace`/`Global` origins → `PluginStatus::Blocked` (`mod.rs:590-620`).

### 2.5 Load / mount — how each capability reaches the runtime

| Capability | Registered where | Reaches the model via | Dynamic on toggle/reload? |
|---|---|---|---|
| **Tools** (WASM `tools_v2`) | `CapabilityApi::register_capability` → `PluginRegistry`; index `active_plugin_tools` (`mod.rs:1210-1249`) | request-build time `active_plugin_tools_for_agent` (`gateway/execution_engine/tool_refresh.rs:27`); dispatch `executor/builtin_registry/registry/free_fns.rs:35 resolve_active_plugin_tool` → `call_plugin_tool` (WASM) | **yes** (snapshot re-derived by `republish_plugin_projections`) |
| **Hooks** | `sync_hooks_from_registry` (`mod.rs:1128`) + `sync_user_hooks` (`mod.rs:1075`) → `HookExecutor` | per-run `hook_executor_snapshot()` (`run_loop/mod.rs:607`) and `dispatch.rs` (§3) | **yes** — executor rebuilt from scratch on every toggle (`plugin_ops.rs:620-623`) |
| **Commands** (`commands/*.md`) | `SkillRegistration{Command}` | `tool_catalog.register_skills` **at boot only** (`bin/…/agent_init/tool_catalog_init.rs:216-247`) | **NO** — no re-registration path; a plugin enabled/reloaded after boot has stale/absent slash commands. (Second face `plugins.executeCommand` passes the markdown body as a WASM export name, `handlers/runtime.rs:82-116` — nonsense for a `.md` command, and has zero clients) |
| **Agents** (`agents/*.md`) | `plugin_agent_to_def` (`mod.rs:254`) → `agents::PLUGIN_SUBAGENTS` via `projection.rs:118 republish_plugin_projections` | `AgentRegistry::resolve` + `<available_agents>` | **yes** |
| **Skills** (`skills/*/SKILL.md`) | projection → `utils::paths::PLUGIN_SKILL_DIRS` + `SkillSystem` rescan | `<available_skills>` index, `skill_read` | **yes** |
| **MCP servers** (`.mcp.json`) | `registrar/mcp_registrar.rs` → `PluginLoader.mcp_configs` → `McpManagerHandle::add_transient_server` (`mod.rs:462 sync_mcp_plugin_servers`) | MCP tool bridge | add: boot (`start/mod.rs:1446`) + `reload()` (`mod.rs:828`); remove: `unload_runtime_plugin` (`plugin_ops.rs:270-288`) — **`set_plugin_enabled(true)` does not re-add** (only `reload()` does) |
| **Services** | `service_manager.rs` | background tasks | start: boot + `reload()`; stop: `unload_runtime_plugin` (`plugin_ops.rs:237-266`), `stop_orphaned_services` on reload |
| **Memory extensions** (`[memory]`) | `loader.rs:253 register_memory_extension_if_declared` → `MemoryExtensionRegistry::register_mcp` | memory retrieve/capture dispatch | **NO** — `MemoryExtensionRegistry` has `register`/`register_mcp` but **no unregister** (`src/memory/extensions/registry.rs:95-114`); disable leaves the extension bound to a transient MCP server that has been removed |

### 2.6 Unload / reload

- `reload()` (`mod.rs:802`): rebuilds everything from disk (registry `clear()`, fresh `HookExecutor`), then `sync_mcp_plugin_servers`, `stop_orphaned_services`, `sync_plugin_services`. Triggered by the file watcher (`watcher.rs`, debounce, self-write suppression) and `plugin.reload`/`hooks.reload`.
- `reload_plugin(id)` (`mod.rs:1303`): per-plugin `CapabilityApi::reload` (unregister+register atomically) + tool index refresh — but does **not** touch hooks executor, MCP, services, projections (only the tool index). Twin of `reload()` with a narrower effect set (判据 §16).
- `unload_runtime_plugin` (`plugin_ops.rs:228`): stops services, unloads WASM, removes transient MCP servers.

### 2.7 Is there a scoping / disposal / effect-tracking model?

**No fiber / scope / dispose model.** Registration is "global, re-derived on every change":
- `projection.rs:14-24` names Cordis explicitly and declines it: *"Cordis … solves the same problem by making every registration an effect on the plugin's fiber, so one `dispose()` unwinds all of them. Aleph deliberately does **not** adopt a fiber runtime (R10 — see `HARNESS_PHILOSOPHY.md` §2.3); the equivalent guarantee here is cheaper and more Aleph-shaped: **one function derives the whole set from the registry, and every path that can change plugin activation calls it.**"* — guarded by test `publishing_plugin_projections_has_exactly_one_author` (`projection.rs:162`).
- The derivation covers exactly three surfaces (skill dirs, sub-agents, tool index). Everything **outside** the derivation is where state leaks after disable: (a) memory extensions (no unregister), (b) ToolCatalog slash commands (boot-only), (c) MCP transient servers on re-enable (only `reload()` re-adds), (d) `src/extension/scope.rs` (135 LOC) is a *project-scoping* helper for hooks (`project_scope_allows`, executor.rs:917), not a disposal scope.
- `src/extension/runtime/wasm/capability_kernel.rs` (597) is a **permission** kernel (which host functions a WASM plugin may call), not an effect tracker.

## 3. Claude Code compatibility — real vs. paper

Docs read: `PLUGIN_SYSTEM.md` (828 lines, claims "任何 Claude Code 插件…无需修改即可在 Aleph 中安装和运行", :9-10), `EXTENSION_SYSTEM.md` (955), `ALEPH_HUB.md` (280), specs `2026-03-20-plugin-system-claude-code-compat-design.md` (645), `2026-03-25-capability-driven-plugin-architecture-design.md` (690), FL §3.10 §3.11 §5.10 §5.20 §5.21 §5.24.

| CC component | Verdict | Evidence (code + test) | What's missing |
|---|---|---|---|
| `.claude-plugin/plugin.json` manifest (`name/version/description/author/homepage/repository/license/keywords/commands/agents/skills/hooks/mcpServers`) | **IMPLEMENTED** | `manifest/cc_plugin_json.rs` (728), lenient `component_source.rs` (string / array / inline-object forms, fixed 2026-08-19 ③), tests `cc_plugin_json.rs` + `component_source.rs` | `dependencies[]` + host-API version gate: **deliberately deferred** (user ruling 2026-08-19, FL §3.10 ①; ALEPH_HUB.md §7 ❌) |
| `${CLAUDE_PLUGIN_ROOT}` | **IMPLEMENTED** | expanded once for every adapter at `manifest/adapter.rs:108-139`; `plugin_vars.rs`; tests `plugin_variables_are_expanded_in_skill_prose` (adapter.rs:333), `…not_expanded_in_identifiers` (:382) | `${CLAUDE_PLUGIN_DATA}` is an Aleph addition, not CC |
| `skills/*/SKILL.md` (frontmatter `name/description/allowed-tools`) | **IMPLEMENTED** | `skill/manifest.rs` RawFrontmatter (:107-147; comma-scalar `allowed-tools` survives), plugin skill dirs projected `projection.rs:82`, reachable via `skill_read` + `<available_skills>`; QA `qa/plugins/run.sh` | CC `disable-model-invocation`/`user-invocable` parsed (`types/skills.rs:128`) |
| `agents/*.md` (frontmatter `name/description/tools/model` + **body = system prompt**) | **PARTIAL** | `plugin_agent_to_def` `mod.rs:254-294` maps description/tools/model/steps → `AgentDef`; test `plugin_agent_to_def_maps_subagent_and_drops_body` (mod.rs:1469) | **Body (system prompt) is dropped by design** — "刻意仍不做：plugin agent 的 `content`…跨 agents 的既有限制" (FL §3.10). A CC agent whose whole value is its prompt runs as a generic sub-agent. CC `color`/`permissionMode` not mapped |
| `commands/*.md` (`$ARGUMENTS`, `argument-hint`, `allowed-tools`, `!bash`, `@file`) | **PAPER at the execution end** | parsed → `SkillRegistration{Command}` (`parsers.rs:384-396`); listed as slash commands **once at boot** (`tool_catalog_init.rs:216-247`); `/cmd` → `slash_command.rs:249-275` **falls through without injecting the body** ("nothing on this path injects skill text into the prompt"); `skill_read` only finds `<dir>/<id>/SKILL.md` (`skill_reader/read.rs:148+`), never `commands/foo.md`; `SkillTemplate` (`template.rs`, 331 LOC, does `$ARGUMENTS`) has **zero consumers** | The command **prompt never reaches the model**. FL's phrase "plugin commands 仍结构性 human-only" understates it — human can trigger, model never sees the text. No `$ARGUMENTS`, no `!bash`, no `@file`. Second face `plugins.executeCommand` treats the markdown as a WASM export name (`handlers/runtime.rs:82-116`) and has zero clients |
| `hooks/hooks.json` — config shape (`{"hooks":{"PreToolUse":[{"matcher":"…","hooks":[{"type":"command","command":"…","timeout":…}]}]}}`) | **IMPLEMENTED** | plugin: `manifest/parsers.rs parse_hooks_file` (fixed 2026-07-26 ❸); user-level `~/.aleph/hooks.json`, `<project>/.aleph/hooks{,.local}.json` (`hooks/user_settings.rs:5-8`, tests :374-579); consent gate for shell/http (`hooks/consent.rs`) | User-level file lives at `~/.aleph/hooks.json`, **not `~/.claude/settings.json`** — a CC user's existing hooks are not picked up |
| Hook **event names** | **IMPLEMENTED (aliases)** | `types/hooks.rs:39-121`: `PreToolUse`→BeforeToolCall, `PostToolUse`→AfterToolCall, `PostToolUseFailure`, `Notification`, `UserPromptSubmit`, `Stop`, `SubagentStart`, `SubagentStop`, `PreCompact`→BeforeCompaction, `SessionStart`, `SessionEnd`, `PermissionRequest`; all 23 events have a live producer outside `src/extension/` (grep table below) | No alias for any CC event not in that list (check against current CC docs before claiming parity) |
| Hook **execution on tool calls** | **IMPLEMENTED** | `src/tools/scoped/dispatch.rs:1233 run_before_tool_hooks` → `execute_interceptors(BeforeToolCall)` (:1246); `AfterToolCall` :1397/:1404, `AfterToolCallFailure` :1426/:1435, `ToolResultPersist` :1571, `PermissionRequest` :928, `Notification` :938. Stop hooks via `verification/extension_stop_gate.rs` (VerifierChain, **outside `src/harness/`**) | — |
| Hook **stdin JSON payload** (`hook_event_name, session_id, tool_name, tool_input, tool_output, cwd`) | **IMPLEMENTED** | `hooks/executor.rs:100-150 build_event_payload`, piped to stdin :617-648 (concurrent with stdout read to avoid deadlock); env `CLAUDE_PLUGIN_ROOT`, `TOOL_NAME`, `TOOL_INPUT` :575-606; `aleph-server hooks test` uses same serializer | `transcript_path`, `permission_mode` not in payload |
| Hook **stdout JSON decision** (`{"decision","reason"}`, `{"continue":false,"stopReason"}`, `{"systemMessage"}`, `{"hookSpecificOutput":{"permissionDecision","permissionDecisionReason","additionalContext"}}`) | **IMPLEMENTED** | `hooks/json_output.rs` (349), tests :239-345; line-prefix protocol (`deny:`/`block:`/`ask:`/`update_input:`) as fallback | `hookSpecificOutput.updatedInput` **not parsed** (only Aleph's `update_input:` prefix, `hooks/mod.rs:399-401`) |
| Hook **exit-code semantics** (CC: exit 2 = block, stderr fed to model; other non-zero = non-blocking warn) | **ABSENT** | `executor.rs:697-713`: non-zero exit → `warn!` + `ActionResult{success:false, exit_code}`; `execute_interceptors` :941-952 only reads `ar.output`; `exit_code` is never consulted for a decision (`rg exit_code src/extension/hooks` — only populated) | A CC hook written as `echo reason >&2; exit 2` **silently passes**. This is the most common CC hook idiom |
| `.mcp.json` (`mcpServers{cmd,args,env,url,type}`) | **IMPLEMENTED** | `registrar/mcp_registrar.rs` (597) → transient MCP servers; `mcp_config.rs` `${VAR}` substitution; bundled `plugins/{diagnostics,llm-task,media-office,phone-control,voice-call}` are `runtime = "mcp"` | Re-enable after disable needs `reload()` (§2.5) |
| `marketplace.json` + `/plugin marketplace add/…` | **IMPLEMENTED** | `marketplace/manifest.rs`, `mod.rs` (1,072), sources git/GitHub/local; RPC `plugin.marketplace.{list,browse,add,update,remove,install}`; Panel + CLI + `plugin_manage` tool; QA `qa/plugins/run.sh marketplaces` 18/18 | CC's versioned plugin cache + orphan tombstones: **REFERENCE-ONLY 不移植** (FL §3.10) |
| CC `settings.json` (`enabledPlugins`, `hooks`, `permissions`) | **ABSENT** | no reader of `~/.claude/settings.json` in `src/` (`rg 'settings\.json' src/extension src/discovery` → only hooks doc comment "mirrors Claude Code's `settings.json` `hooks` block") | Aleph's answer is `plugins.toml` + `hooks.json` |
| `~/.claude/plugins/` (CC's own installed-plugin cache) | **ABSENT** | discovery scans `~/.claude/{skills,commands,agents}` but not `~/.claude/plugins` (`scanner.rs:214-224`) | A CC user's already-installed plugins are invisible |
| Node.js plugin runtime / `@aleph/plugin-sdk` | **PAPER-ONLY** | `EXTENSION_SYSTEM.md:143-218` documents `src/extension/runtime/nodejs/` + `createServer()` — **directory does not exist** (`ls src/extension/runtime/` = `mod.rs wasm`); `packages/plugin-sdk/src/types.ts:6` spells `PluginKind = "wasm" \| "nodejs" \| "static"` vs Rust `wasm/mcp/static`; SDK also declares `ManifestChannelSection`/`ProviderSection`/`HttpRouteSection` that EXTENSION_SYSTEM.md:915 itself says "一行也没有" | Doc section was corrected for channel/provider/http on 2026-08-19 but the Node.js section was left standing |

**Hook event producer census** (from `rg 'HookEvent::' src` excluding `src/extension/`): BeforeAgentStart `run_loop/mod.rs:619` · AgentEnd :794 · BeforeToolCall `dispatch.rs:1246` · AfterToolCall :1397 · AfterToolCallFailure :1426 · ToolResultPersist :1571 · MessageReceived `inbound_router/mod.rs:627` · MessageSending `channel_registry.rs:635` · MessageSent :660 · SessionStart `run_loop/inner.rs:346` · SessionEnd `session/db_handlers/modify.rs:78` · BeforeCompaction/AfterCompaction `session_compactor/prepare_history.rs:258/188` · PreApiRequest/PostApiRequest `http_provider.rs:468/616` · GatewayStart/Stop `start/mod.rs:3700/3843` · Notification `dispatch.rs:938` · PermissionRequest :928 · UserPromptSubmit `inner.rs:408` · SubagentStart/Stop `agents/runtime.rs:480/569` · Stop `extension_stop_gate.rs:218`. **23/23 wired.**

## 4. pi compatibility today — ABSENT

- `rg -i 'ExtensionAPI|pi\.extensions|\.pi/|pi-mono|pi_extension|PiExtension|pi-coding-agent' src interfaces shared crates` → the only hits are comments citing pi-mono as a design reference (`providers/message.rs:11`, `skill/prompt.rs:11` "Mirrors openclaw/pi/hermes-agent").
- No adapter in `AdapterRegistry` detects a pi `package.json` `"pi"` field or an `extensions/*.ts` layout (`manifest/adapter.rs:81-92` lists six adapters, none pi).
- No JS/TS host exists to run a pi extension (the only runtime is WASM, §1). A pi extension is TypeScript against an `ExtensionAPI` (`pi.on(...)`, `pi.registerTool`, `pi.registerCommand`, …); Aleph has neither the process host nor the API surface.
- What *does* exist: pi as a **terminal-runtime agent** to be hosted (`crates/agent-detect/src/engine.rs:63 Agent::Pi`, manifest `manifests/pi.toml`) — that is "run pi in a PTY", not "load pi extensions". FL §3.1 lists pi mechanisms (steer checkpoint, schema coercion) as harness parity items, not as an extension format.
- Docs: zero mentions of pi extensions in `PLUGIN_SYSTEM.md` / `EXTENSION_SYSTEM.md` / `ALEPH_HUB.md`.

## 5. OpenClaw / ClawHub footprint — the deletion candidate list

### 5.1 The number first (判据 §6 / §11): `rg -i -c 'openclaw|clawhub|claw'`, per-line hits

| Dir | hits | files | Of which are **OpenClaw-format support code** |
|---|---|---|---|
| `src/` | 286 | 156 | **≈50 LOC in one file** (see 5.2). The other ~270 are comments of the form "openclaw parity" / "mirrors openclaw" in gateway (50), providers (33), tools (30), builtin_tools (18), teams (15 — these are `ClawTeam`), security (15), tasks (14), extension (12), cluster (10), … |
| `interfaces/` | 15 | 8 | 0 — all parity comments + one **guard test that the ClawHub tab is already gone** (`webchat/src/components/settings_sidebar.rs:287-291 clawhub_tab_is_removed`) |
| `shared/` | 0 | 0 | — |
| `docs/reference/` | 188 | 18 | FL 93 (§3.10: 7, §5.21: 1, rest are browser/provider/gateway/voice parity), CLUSTER 20, ALEPH_HUB 14, MODEL_CATALOG 13, PLUGIN_SYSTEM 6, SKILL_MODEL_TAXONOMY 4 |
| `qa/` | 0 | 0 | — |
| `crates/` `packages/` `plugins/` `skills/` | 0 | 0 | — |
| `desktop/` | 3 | 3 | 0 (parity comments) |
| `tests/` | 2 | 1 | fixture prose `tests/fixtures/markdown_skills/echo-basic/SKILL.md:3,11` |
| `Cargo.toml` `justfile` | 0 | — | — |

**Three different "claw"s are conflated by the grep**: (a) OpenClaw the agent product (parity comparisons; plus **hosting it as an ACP agent** — `src/config/types/acp.rs:420-429` preset, `builtin_tools/team/member_add.rs:102` — that is the R3 "runtime for other agents" positioning, **keep**); (b) `ClawTeam` (`src/teams/*`, a different reference project); (c) `claw-code` / `clawshell` (`CLAW_CODE_GAP_ANALYSIS.md`, `sandbox/*`). None of those is "OpenClaw plugin support". **There is no OpenClaw plugin manifest adapter, no ClawHub catalogue client, no `src/clawhub/`** (though `ARCHITECTURE.md:261` still lists `src/clawhub/` — a phantom directory).

### 5.2 Code that exists ONLY for OpenClaw/ClawHub format

| Path:line | What | Delete breaks anything non-OpenClaw? |
|---|---|---|
| `src/tools/markdown_skill/spec.rs:47-50` `SkillMetadata.openclaw: Option<OpenClawMetadata>` | serde pass-through of `metadata.openclaw.*` from a ClawHub SKILL.md | **No** — field has **zero readers** (`rg '\.openclaw\b' src` → only the definition and `executor.rs:690` test constructing `None`). Serde ignores unknown keys, so ClawHub files still parse after removal |
| `src/tools/markdown_skill/spec.rs:86-104` `struct OpenClawMetadata {emoji, primaryEnv, homepage, os, always, install}` | DTO | No (unread) |
| `src/tools/markdown_skill/spec.rs:106-124` `struct OpenClawInstallSpec` | DTO | No (unread) |
| `src/tools/markdown_skill/spec.rs:1-4, 36, 44` + `mod.rs:4` + `loader.rs:166` + `tool_adapter.rs:143` | doc/comment claims "Compatible with OpenClaw SKILL.md format" / "OpenClaw skills default to no confirmation" | No — but the **default values** those comments justify (`ConfirmationMode` default = none, `SandboxMode::Host`) stay; only the attribution goes |
| `src/tools/markdown_skill/executor.rs:690` | test fixture `openclaw: None` | No |
| `src/hub/catalog_client.rs:326,345` | test fixture `"via":"clawhub"` on the free-form provenance label `ExtensionEntry.via` (`hub/types.rs:205-208`) | No — `via` is a generic string; change the fixture value |
| `src/skill/guard.rs:236` | comment "e.g. .git, .clawhub.json" on the hidden-file skip | No |
| `tests/fixtures/markdown_skills/echo-basic/SKILL.md:3,11` | fixture prose | No |
| `src/security/content_sanitizer.rs:862`, `gateway/server/flood_guard.rs:80`, `channel_health_monitor.rs:431` | test **names** `*_matches_openclaw*` | No — rename only; the constants they pin are Aleph's |

**Total genuinely OpenClaw-only code: ~50 lines of unread serde DTOs + ~10 comment/fixture lines.** The whole `src/tools/markdown_skill/` module (2,448 LOC, "Markdown CLI tool" = SKILL.md with `aleph.input_hints` → shell tool) is **not** OpenClaw-specific — it is live (`run_loop/inner.rs:846 join_markdown_skills`, boot `start/mod.rs:2621`), but it *is* the `#[deprecated(since="26.5.20")]` Layer 3 whose Phase 2 dissolution was due "≥ 2026-06-03" (`SKILL_MODEL_TAXONOMY.md:100-110`) and is now **3.5 months overdue** — a separate CUT/absorb decision.

Related but **not** OpenClaw compat (OpenClaw *heritage*, now Aleph dialect — keep): Skill v2 frontmatter top-level `primary_env` / `homepage` / `emoji` / `install: [{id,kind,package,bins,os,url}]` (`skill/manifest.rs:107-196`), consumed by `skill/status.rs:111-146`, `skill/installer.rs`, Panel `settings/skills.rs:776`. OpenClaw nests these under `metadata.openclaw.*` in camelCase; Aleph reads them flat in snake_case, so an actual ClawHub SKILL.md's metadata is **already ignored** by the v2 parser.

### 5.3 Docs / FL entries to delete or relabel

| Location | Content | Action |
|---|---|---|
| `docs/reference/ARCHITECTURE.md:261` | `\| **clawhub** \| src/clawhub/ \| ClawHub integration \|` | **CUT** — directory does not exist (判据 §1: doc describes a module that was never there) |
| `docs/reference/ALEPH_HUB.md:4, 54, 230-257` (§7 "openclaw clawhub 逐项对照") | comparison table + "刻意不移植 install-policy.ts" | Relabel as history or move to `docs/archive/`; the "刻意不做" rulings inside it (telemetry, promotions, install-policy engine, dual-file provenance) still bind — keep those rulings, drop the OpenClaw framing |
| `docs/reference/SKILL_MODEL_TAXONOMY.md:14, 65, 98, 100` | "OpenClaw-style Markdown CLI tool", "clawhub install path", "openclaw.* dropped until Phase 2" | Update when Phase 2 lands |
| `docs/reference/PLUGIN_SYSTEM.md:672, 688-699, 708, 719` | "(openclaw parity)" labels on manifest cache + owner trust; "要复活请从 openclaw 的 activation-planner.ts 起步" | Relabel (features are Aleph's; the attribution is the only OpenClaw content) |
| `docs/superpowers/specs/2026-03-18-clawhub-integration-design.md`, `docs/superpowers/plans/2026-03-18-clawhub-integration.md` | original ClawHub integration spec ("Component 4: Skill Format Compatibility") | Archive (Tier 3) |
| `docs/ideation/2026-04-04-gateway-openclaw-comparison-ideation.md`, `docs/plans/2026-04-05-001-feat-openclaw-comparative-analysis-plan.md` | gateway comparison ideation | Not plugin-related; leave |
| FL §3.10 (7 hits), §5.21 (1) | "openclaw parity" annotations on owner-trust / manifest-cache / Hub | Leave as provenance of a decision, or relabel; no code depends on them |

### 5.4 Things the grep would tempt you to delete but must stay

- `src/config/types/acp.rs:420-429` OpenClaw ACP preset and the harness name list in `member_add.rs:102` — that is hosting OpenClaw as an *agent*, the R3 exception ("跑别人 agent 的运行时"), not plugin compat.
- All `ClawTeam`-attributed code in `src/teams/` and `clawshell`-attributed code in `src/sandbox/`, `src/secrets/leak_detector.rs` — different projects.
- `interfaces/webchat/src/components/settings_sidebar.rs:287-291` — the guard that keeps the ClawHub tab deleted.
- The Hub `via` field — generic provenance.

## 6. Prior-round conclusions to honor

### 6.1 Cordis / DeepSeek Harness — what was decided NOT to port (verbatim)

1. **HARNESS_PHILOSOPHY.md §8 第五课 (line 350)**: "对一个「everything is a plugin」的 harness 做完 10 维对照，落点依旧全在循环之外（2026-08-15，deepseek-harness/Cordis）… **`src/harness/` 增删 0 行，第五次**。… 插件架构本身（及其 capability-seam 三角、invariant registry）被记为 DEFER——**对照的产出从来不是「把参考项目的形状搬来」，而是「它的失效知识在我们的形状里住在哪」**".
2. **FL §3.1 Round 8 (line ~895)**: "**架构本身不移植**——插件树 / capability-seam 三角 / 包自有 invariant registry（`ctx.invariants`）全是框架形状，与 Aleph 已裁决的立场（HARNESS_PHILOSOPHY §2.3 拒绝框架式 harness；不变量守卫的 Aleph 形态是源码级 census 测试 + `src/diagnostics/checks/`）正面冲突；可采的只有机制与不变量。"
3. **FL §3.10 round-2 (2026-08-16)**: "**架构本身仍不移植**——2026-08-15 dsh 轮已裁定 fiber 插件树与 harness 立场冲突（§3.1 Round 8），本轮裁的是**另一个子系统**（Aleph 自己的第三方插件宿主），可采的仍只有机制。Cordis 五理念里只有第 3、5 条落地：「注册是可逆 effect」→ 单一投影咽喉；「`inject` 未满足要说出还差什么」→ registry 从「成功名单」变成「每个被发现插件及其结局」。" … "**刻意不引入 fiber**：等价保证由「一个函数派生全部 + 每条改激活态的路径都调它」给出，`src/harness/` 增删 0 行。"
4. **FL §3.10 (2026-08-19 compat round, 刻意不做)**: "版本化插件缓存 + orphan 墓碑（CC）判为 **REFERENCE-ONLY 不移植**：单 daemon 宿主下多版本共存零消费者，违 R3/R10。**Cordis 的 DI 容器同前两轮，仍不移植**。" and "插件**依赖声明**与 **host API 版本闸**（CC 有 `dependencies[]` + DFS 闭包，openclaw 有 `compat.pluginApi` semver 下限）同样未做——ALEPH_HUB.md 早已标 ❌（**这一条仍然成立**）" — reaffirmed by user ruling 2026-08-19 third round: "①（依赖声明 + `pluginApi` 版本闸）继续 defer".
5. **`src/extension/projection.rs:14-24`** (the code-side statement): "Cordis … solves the same problem by making every registration an effect on the plugin's fiber, so one `dispose()` unwinds all of them. Aleph deliberately does **not** adopt a fiber runtime (R10 — see `HARNESS_PHILOSOPHY.md` §2.3)". Guard: `publishing_plugin_projections_has_exactly_one_author` (`projection.rs:162`).
6. **PLUGIN_SYSTEM.md:683-706** Lazy Activation Planner **CUT 2026-08-07**: "`PluginManifest.activation` 从来没有非 `None` 过… 要复活请从 openclaw 的 `activation-planner.ts` … 起步，不要从被删的 Rust 起步". Any "temporal composability" (load-on-trigger) design must know this path was built once, never fired, and was deleted.
7. **ALEPH_HUB.md §7**: "**刻意不移植**：openclaw 的 `install-policy.ts` …把第二套策略引擎装进安装路径会造出第二个强制点（违 SECURITY.md 的单点原则）"; telemetry and promotions feed "**有意不做**".
8. **FL §3.10 (2026-08-19 ④)**: skills dir gets **no** trust gate — "插件闸拦的是**能力注册**…技能注册不了任何东西" — provenance surfaces only on `skill_read`, "刻意只上 `skill_read` 不上 `<available_skills>` 索引".
9. **FL §5.10 hooks**: `HookKind::Resolver` chain deleted (零消费者); `MAX_HOOK_TIMEOUT_SECS = 300` clamp lives only in `HookExecutor::effective_timeout`; hook CRUD only in `append_user_hook`/`remove_user_hooks`.
10. **Plugin agent body**: "刻意仍不做：plugin agent 的 `content`（自定义 system prompt）——`AgentDef` 是 section-key 制、磁盘 agents 同样丢 body… 接它即 scope creep" (FL §3.10).

### 6.2 R10 constraints that bind any hook/plugin-in-the-loop design

- `src/harness/` is **12 files**; the ceiling is `src/harness/tests/budget.rs::CEILING` (measured ratchet, only decreases; CLAUDE.md deliberately does not copy the number). Five comparison rounds (incl. dsh) landed **0 lines** there; the 2026-08-23 self-audit added +99 while lowering CEILING −919 (HARNESS_PHILOSOPHY.md:352).
- The existing hook seams are all **outside** the harness: tool hooks in `src/tools/scoped/dispatch.rs`, Stop hooks via `src/verification/extension_stop_gate.rs` → `VerifierChain` (assembled in `bin/aleph-server/commands/start/orchestrator_init.rs`), prompt/session hooks in `src/gateway/execution_engine/run_loop/{mod,inner}.rs`, API hooks in `src/providers/http_provider.rs`. Any new "plugin intercepts the loop" capability must land on one of these seams; the 5 "不" (no intent filtering, no recovery-strategy selection, …) and the 3 questions apply.
- R10's stated exception: "渐进式工具披露" (static partition + `tool_search`) is allowed because it is not content-based filtering — same test for any Cordis-style "spatial composability" of tool sets.

## 7. Known broken wires / dead code in this subsystem

Method: FL keyword sweep (§3.10/§3.11/§5.10/§5.20/§5.21/§5.24 for 零消费者/未接/尚未/CUT) + my own reading + an RPC census (`scratchpad/rpc_census.sh`: server `register(`/`reg!(` literals in `gateway/handlers/mod.rs` + `bin/…/builder/handlers/` vs. string literals in Panel/TUI/CLI/qa/shared; single-line literals only — multi-line `register(\n"x"` calls were checked by hand).

| # | Symptom | Where | CONNECT / CUT | Evidence |
|---|---|---|---|---|
| 1 | CC `commands/*.md` prompt body never reaches the model | `slash_command.rs:249-275` fallthrough; `skill_reader/read.rs` only finds `<id>/SKILL.md`; `template.rs` unused | **CONNECT** (inject body on `/cmd`, with `$ARGUMENTS`) or **CUT** the Command registration + slash entry so the list stops advertising a no-op (判据 §11 "报成功的 no-op") | §3 row "commands" |
| 2 | `SkillTemplate` (331 LOC, does `$ARGUMENTS`) has zero consumers | `src/extension/template.rs`, re-exported `mod.rs:64` | **CUT** unless #1 CONNECTs through it | `rg SkillTemplate src` → only its own file + re-export |
| 3 | `plugins.executeCommand` passes a markdown body as a WASM export name | `gateway/handlers/plugins/handlers/runtime.rs:82-116` → `plugin_ops.rs:138 execute_plugin_command` | **CUT** (zero clients; semantically wrong since the `CommandRegistration` → `SkillRegistration{Command}` fold) | RPC census: zero clients |
| 4 | 20 RPC methods with **zero clients** | `plugins.{load,unload,executeCommand}` (`handlers/mod.rs:371-374`); `plugin.{list,installFromZip,enable,disable,config.get,config.set}` (:377-389); `mcp.{add,delete,logs,prompts,resources,restart,start,status,stop,tools,update}` (`bin/…/handlers/mcp.rs:32-45`) | **DECIDE** per method: the `plugin.*` singular set duplicates `plugins.*` which is what every client actually calls — the comment "plural — legacy, kept for backward compat" (:364) describes the **opposite** of reality (判据 §1); `plugin.config.*` has a tool face (`plugin_manage` config_get/set call the manager directly) so the RPC face is the orphan (判据 §9) | `rpc_census.sh` output |
| 5 | Plugin slash commands registered **once at boot**; enable/disable/reload/watcher never re-run it | `bin/…/agent_init/tool_catalog_init.rs:216-247` is the only `register_skills` caller for plugin commands; no unregister path | **CONNECT** into `republish_plugin_projections` (projection.rs already owns the "every activation change" contract) — or moot if #1 CUTs | `rg 'register_skills\(' src` |
| 6 | `[memory]` plugin extension survives disable/uninstall | `loader.rs:253 register_memory_extension_if_declared`; `memory/extensions/registry.rs:95-114` has `register`/`register_mcp`, **no unregister**; `unload_runtime_plugin` never touches it | **CONNECT** (add unregister keyed by plugin id, call from `unload_runtime_plugin`) | grep `unregister` in `src/memory/extensions/` → 0 |
| 7 | Re-enabling an MCP-kind plugin does not restart its servers | `set_plugin_enabled` (`plugin_ops.rs:586-641`) calls `unload_runtime_plugin` on disable but nothing on enable; only `reload()` (`mod.rs:828`) calls `sync_mcp_plugin_servers` | **CONNECT** (call `sync_mcp_plugin_servers` + `sync_plugin_services` on enable) — twin asymmetry (判据 §14 闸的两个方向) | code |
| 8 | `reload_plugin(id)` is a narrower twin of `reload()` | `mod.rs:1303-1341` refreshes tool index only; skips hooks executor, projections, MCP, services | **CONNECT** to `republish_plugin_projections` + hook sync, or **CUT** and route `plugin.reload` to `reload()` | 判据 §16 twins |
| 9 | CC hook exit-code 2 is not a block | `hooks/executor.rs:697-713`; `execute_interceptors` :941-952 reads stdout only | **CONNECT** (exit 2 → `blocked` with stderr as reason; other non-zero → observer warn) | §3 |
| 10 | `hookSpecificOutput.updatedInput` ignored | `hooks/json_output.rs` `JsonHookOutput` has no `updated_input`; only `update_input:` prefix (`hooks/mod.rs:399`) | **CONNECT** (one field) | §3 |
| 11 | Hook-decision protocol + `HookAction::Plugin` (WASM) exists, but the official MCP plugins ship JS hook handlers that nothing can call | `plugins/media-office/src/index.js:264 onPostToolUse`, `examples/plugins/media-video/src/index.js:61 onPreToolUse` — MCP stdio has no hook channel; no `hooks.json` in any bundled plugin | **CUT** the JS handlers (submodule / examples) or document that MCP-runtime plugins cannot hook | `find plugins -name hooks.json` → 0 |
| 12 | `examples/plugins/media-video/aleph.plugin.toml` declares `kind = "nodejs"` | `PluginKind` = `{wasm,mcp,static}` → `unknown variant` → never loads (same defect FL ⑦ fixed in the CLI scaffold, not in the example) | **CUT** the example or convert to `runtime = "mcp"` | `types/plugins.rs:125` |
| 13 | `packages/plugin-sdk/` (906 LOC TS) describes a protocol with no host | `types.ts:6 PluginKind = "wasm"\|"nodejs"\|"static"`; declares `ManifestChannelSection`/`ProviderSection`/`HttpRouteSection`/`ChannelDefinition`/`HttpRouteRegistration` — capabilities `EXTENSION_SYSTEM.md:915` says never existed | **CUT** (or rewrite as an MCP-server helper if a Node story is wanted) | zero references outside docs/plans |
| 14 | `EXTENSION_SYSTEM.md:143-218` "Node.js Runtime — Location: `src/extension/runtime/nodejs/`" + `createServer()` SDK | directory does not exist; the 2026-08-19 doc correction fixed channel/provider/http but left this section | **CUT** doc section (判据 §1: doc as the lying copy) | `ls src/extension/runtime/` |
| 15 | `ARCHITECTURE.md:261` lists `src/clawhub/` | phantom directory | **CUT** row | `ls src/` |
| 16 | `PluginStatus::Overridden` still has zero producers | `mod.rs:556-580` records a diagnostic on the **winner** and `continue`s; `PLUGIN_SYSTEM.md:149-160` claims "现在三者都有 registry 行" for overridden/error/blocked | **DECIDE**: either register the loser with `Overridden` (needs a non-id registry key) or fix the doc/enum (判据 §2 恒真谓词) | `rg 'PluginStatus::Overridden' src` → only match arms |
| 17 | `AlephSkillSpec` Layer-3 dissolution 3.5 months overdue | `#[deprecated(since="26.5.20")]` `markdown_skill/spec.rs:16`; `SKILL_MODEL_TAXONOMY.md` "Phase 2 (≥ 2026-06-03)" | **DECIDE** — absorb `input_hints/security/docker/requires.bins` onto `SkillManifest` and delete, or retire the deadline; the OpenClaw DTOs (§5.2) fall out either way | — |
| 18 | `OpenClawMetadata` / `OpenClawInstallSpec` parsed, never read | `markdown_skill/spec.rs:47-124` | **CUT** (§5.2) | zero readers |
| 19 | `command.execute` is a permanent error stub | `gateway/handlers/mod.rs:346-352` registers `command.execute` → `INTERNAL_ERROR "requires ToolRegistry — wire in Gateway startup"`; `commands.list` IS overridden at `tool_catalog_init.rs:303`, **`command.execute` never is** (`handlers/commands.rs::handle_execute` is reached only by its own tests :724/:763); zero clients | **CUT** the stub (and `handle_execute` if nothing else wants it) | `rg '"command\.execute"' src interfaces qa` |
| 20 | Discovery is union-of-all-projects with no per-project runtime isolation | `mod.rs:1020-1026` "isolating which project sees which plugin at runtime is a separate, deferred concern"; hooks alone are project-gated (`executor.rs:917 project_scope_allows`) | **DECIDE** — this is the "spatial composability" gap a Cordis-style scope would address; today only hooks have a scope predicate, tools/skills/agents/MCP do not | code |
| 21 | Hooks read `~/.aleph/hooks.json`, not `~/.claude/settings.json`; plugins from `~/.claude/plugins/` not discovered | `hooks/user_settings.rs:5-8`; `scanner.rs:214-224` | **DECIDE** (CC "first-class" scope question) | §3 |

Dead-code lints cannot see #1, #3, #5, #6, #7, #11 — every one has a producer with tests and a consumer with tests; only the wire is missing (判据 §7).

## 8. Numbers at a glance (for the design round)

- Plugin host + skills + hub + tools + handlers ≈ **58k LOC** Rust; `src/mcp/` another 19k.
- Manifest formats actually parsed: **6 adapters** (CC TOML/JSON, Aleph TOML, Codex, Cursor, auto) + `marketplace.json`. **0** OpenClaw, **0** pi.
- Plugin runtimes: **1** (WASM). Node.js runtime: doc-only. MCP-kind plugins are MCP servers, not a runtime.
- Hook events: **23**, all fired; CC JSON in/out contract real; exit-code-2 and `updatedInput` missing.
- OpenClaw-only code: **≈50 LOC** unread DTOs in `src/tools/markdown_skill/spec.rs` + ~10 comment/fixture lines; everything else the grep finds is parity attribution or a different "claw".
- RPC methods in the plugin/mcp/hook/skill families: **62** registered, **20** with zero clients.
- Prior rulings still binding: no fiber/DI container (3 rounds), no dependency/`pluginApi` gate (user 2026-08-19), no versioned cache/tombstones, no lazy activation (CUT 2026-08-07), plugin agent body dropped, `src/harness/` untouched by plugin work.
