# Severed-Wire Audit — `src/config/types/`

Static, read-only review. Scope: 28 files, ~7,700 lines — every top-level file plus the
`agent/`, `dispatcher/`, `generation/`, `memory/`, `policies/` subdirectories.

Method: 5 parallel deep-dive passes (one per file group), each doing repo-wide
`grep -rn` consumer verification for every `pub` field/fn/const in scope, followed by
independent spot-re-verification of the highest-stakes claims by the synthesizing pass.

## Summary

| Severity | Count |
|----------|-------|
| CRITICAL | 9 |
| HIGH     | 11 |
| MEDIUM   | 6  |
| LOW      | 9  |

**Headline result:** `src/config/types/tools.rs` — the largest file in scope and the one
carrying the `[tool_service]`/`[unified_tools]`/legacy `[tools]`/`[mcp]` sections — is the
worst-wired file in the batch. Its "Phase 2 facade chain" (the harness's own tool-service
decorator pipeline) was deleted in 2026-05-20 per an explicit code comment
(`bin/aleph-server/commands/start/mod.rs:199-203`), but nobody went back and removed the
config surface that used to feed it. Six of the nine CRITICAL findings are in this one
file's orbit: legacy `[tools]` enable/allowlist fields, all of `[unified_tools.native.*]`
except `clipboard`, `[tool_service].{default_timeout_seconds,per_tool_seconds}`, legacy
`[mcp].external_servers`, and the entirety of `[agent].code_exec` / `[agent].file_ops`.
These are exactly the fields a security-conscious operator would reach for
(`blocked_commands`, `deny paths`, `shell_enabled = false`, `require_confirmation_for_delete`)
— all parsed, all validated, all silently inert.

A second cluster sits in `[policies.web_fetch]` and `[generation]`: real config surfaces
with UI round-trips and RPC handlers, but the runtime construction sites never read them
(`WebFetchTool::new()` vs. the unused `WebFetchTool::with_policy()`; generation's
`default_image_provider` etc. vs. `first_for_type()` alphabetical fallback).

---

## Phase 0 — Re-verification of prior audit's "NOT A BUG" / partial-fix claims

The mission required re-verifying four claims from the prior `src/config` audit
(commit `dcd2c678c`, `review-results/config.md`) and its partial follow-up fix
(commit `fb4f942a5`). Findings below; full grep trails are in the per-group agent
transcripts.

### H1 — `is_default_session` snapshot risk (structs.rs, not in this batch's scope)
Out of scope for `src/config/types/` (the function lives in `src/config/structs.rs`,
which is outside the 28-file list for this audit). Not re-verified here; the prior
"not a bug today, flagged for future env-derived defaults" characterization stands
untouched.

### H3 — `parallel_tool_concurrency_opt()` always returns `Some(_)` — **CONFIRMED NOT A BUG**
Traced every consumer of `ToolServiceConfig::parallel_tool_concurrency_opt()`
end-to-end: `orchestrator_init.rs:381` → `harness_bridge/mod.rs` →
`runner_impl.rs` → `dispatch.rs` → `harness/deps.rs` → `agents/runtime.rs` /
`subagent_spawner/mod.rs` / `subagent_tool/{mod,spawn}.rs` →
`gateway/execution_engine/run_loop/inner.rs:1115-1122`. All of these are pure
`Option<usize>` plumbing. The only place that actually branches on the value is
`src/harness/agent/act.rs`, at three independent sites, and **all three** re-derive
the "0/1 disables, ≥2 enables" contract themselves rather than trusting `Some(_)`:
- `act.rs:134` — `matches!(self.deps.parallel_tool_concurrency, Some(n) if n >= 2)`
- `act.rs:686-689` — explicit `par_n < 2` early-return
- `act.rs:875` — `.unwrap_or(0).max(2)`, reached only after both gates above

The doc comment on `parallel_tool_concurrency_opt` already states this contract
explicitly ("`Some(0..=1)` is treated as disabled by the harness"). This is a
deliberate layering choice (config carries the raw value; the harness owns the
`>=2` semantics), not doc/code drift. **Verdict: not a bug — downgrade/close.**

### H4 — `AcpConfig::default_adapters()` static-const deference — **REFINED: a real bug hides behind the framing the prior audit used**
The prior audit's narrow question ("could `all_presets()` return different values
across calls within one process?") is correctly answered "no" — it's a static
const list, verified by checking every one of its ~9 call sites. But that framing
missed the actual defect one line away: `AcpAdapterEntry.preset`'s doc comment
(`acp.rs:167-168`) promises *"if set, missing fields are filled from the preset
defaults."* This is false. Read line-by-line:
- `deserialize_adapters_with_presets` (`acp.rs:41-62`) only backfills the
  `preset` marker field itself: `if e.get().preset.is_none() { e.get_mut().preset
  = entry.preset }` — never `executable`/`args`/`output_format`/`trust_level`/
  `display_name`.
- `handlers/acp_config.rs:160-168`'s `handle_list` merge does the same thing.

So a user who hand-writes **any** partial `[acp.adapters.claude-code]` table
(even just `enabled = false`) gets every other field silently reset to
`AcpAdapterEntry`'s own bare `#[serde(default)]`s, not the preset's values. This
reaches production: `GenericAcpAdapter::from_entry` (`acp/adapters/generic.rs:35-53`)
does `executable.unwrap_or_else(|| id.clone())` — the spawned binary name becomes
the literal string `"claude-code"` instead of `"claude"`, args lose
`--print --output-format json -p`, `output_format` reverts to `PlainText`
(loses JSON extraction), `trust_level` reverts to `Disabled` (silently kills LLM
delegation for that adapter). See Finding #3 below. **Verdict: real bug, HIGH,
not "no real risk."**

### M2 — `UnifiedToolsConfig`/`ToolServiceConfig` accessors tightened in `fb4f942a5` — **the fix commit's own message and doc comments are wrong; the code was never actually narrowed**
`git show fb4f942a5 -- src/config/types/tools.rs` shows the diff for all 8
accessors (`fs_allowed_roots`, `git_allowed_repos`, `is_screen_capture_enabled`,
`screen_capture_config`, `is_search_tool_enabled`, `search_tool_config`,
`enabled_mcp_servers`, `per_tool_durations`) **only adds doc comments that assert
`pub(crate)`** — every `pub fn` signature line is an unchanged context line in the
diff, not a `-`/`+` line. Re-confirmed directly against the current file:
```
$ grep -n "pub fn fs_allowed_roots\|pub fn per_tool_durations\|pub fn enabled_mcp_servers" src/config/types/tools.rs
85:    pub fn per_tool_durations(&self) -> HashMap<String, Duration> {
460:    pub fn fs_allowed_roots(&self) -> Vec<String> {
532:    pub fn enabled_mcp_servers(&self) -> Vec<(&String, &McpServerConfig)> {
```
All 8 are still literally `pub fn`, still zero callers anywhere in the repo
(not just non-test — zero, period). Contrast with the same commit's
`config/patcher.rs::record_mtime`, where the `pub async fn` → `pub(crate) async
fn` edit really did land. **This is itself a severed-wire-shaped bug**: the
commit message and 8 doc-comments assert a visibility the code doesn't have —
"same fact, two statements, only one was changed" (see finding M2-redux below).
Per this project's own judgment criteria this is exactly the class of defect
its own audit process is designed to catch. Additionally, the synthesizing pass
found the same never-narrowed pattern on accessors the M2 fix didn't even touch:
`ToolServiceConfig::default_timeout()`, `UnifiedToolsConfig::shell_config()`,
`is_fs_enabled`/`is_git_enabled`/`is_shell_enabled`/`is_system_info_enabled` — all
still fully `pub`, all zero production callers (see Findings #10-#11).

---

## Findings

### CRITICAL

#### 1. `SubagentPolicy.allow` is parsed, threaded, and completely unenforced
- Producer: `src/config/types/agents_def.rs:339-343` — `SubagentPolicy { allow:
  Vec<String> }`, doc: "Controls which sub-agents an agent is allowed to spawn...
  Use `[\"*\"]` for unrestricted, or list specific agent IDs."
- Threading: `src/config/agent_resolver/mod.rs:380,397` reads
  `agent.subagents.clone()` into `ResolvedAgent.subagent_policy: Option<SubagentPolicy>`.
- Consumer: **none.** Whole-repo grep for `subagent_policy` returns exactly 5 hits:
  the field declaration, the one write in the resolver, and two
  `subagent_policy: None` test fixtures (`gateway/agent_instance.rs:1314,1351`).
  The actual spawn-time gate — `AgentRegistry::spawnable_agent_ids()` /
  `resolve_spawnable()` (`src/agents/registry.rs:132-170`) — filters purely by
  `AgentMode::SubAgent` vs `Primary` and `allowed_tools`; it never consults
  `SubagentPolicy.allow`.
- Severity: **CRITICAL**
- Triage: **DECIDE**
- Reason: an operator who writes `[agents.list.coder.subagents] allow =
  ["reviewer"]` believes delegation from "coder" is restricted to "reviewer";
  enforcement is zero — any spawnable agent id is reachable regardless.
- Proposed fix: wire `resolve_spawnable`/the subagent-tool target-id check to
  consult the spawning agent's `subagent_policy.allow` list (CONNECT), or delete
  `SubagentPolicy`/`AgentDefinition.subagents` and its doc entirely if superseded
  (CUT).

#### 2. `[orchestrator]` config section (`OrchestratorConfig`/`OrchestratorGuards`) is entirely dead
- Producer: `src/config/types/orchestrator.rs:8-86` — `max_rounds`,
  `max_tool_calls`, `max_tokens`, `timeout_seconds`, `no_progress_threshold`, plus
  `is_rounds_exceeded`/`is_tool_calls_exceeded`/`is_tokens_exceeded`/`timeout()`.
  Stored as `Config.orchestrator: OrchestratorConfig`
  (`src/config/structs.rs:111-113`, doc: "Three-Layer Control architecture").
- Consumer: **none anywhere in the repo.** Grepped `.guards`, `max_tool_calls`,
  `is_tool_calls_exceeded`, `is_tokens_exceeded`, `is_rounds_exceeded`,
  `no_progress_threshold`, `OrchestratorGuards {` — zero hits outside
  `orchestrator.rs` itself. The real "Three-Layer Orchestrator" runtime
  (`src/orchestrator/{flow_registry,dispatch,deps_builder,presets}`) derives its
  own limits from `deps_builder/*` / `presets/default_flows.toml` and never
  touches `config.orchestrator`. (The many `.orchestrator` hits elsewhere in the
  repo refer to an unrelated field — `GatewayServer.orchestrator:
  Option<AgentHarnessRunner>` — not `Config.orchestrator: OrchestratorConfig`.)
- Severity: **CRITICAL**
- Triage: **DECIDE** (likely CUT)
- Reason: `[orchestrator] guards.max_rounds = 5` (or any sibling field) parses,
  round-trips, and does nothing — the struct's own doc comments describe exactly
  the bounding behavior that never happens.
- Proposed fix: either wire `OrchestratorGuards` into `src/orchestrator/dispatch.rs`
  / `deps_builder` as real round/tool-call/token/timeout ceilings, or delete
  `OrchestratorConfig`/`OrchestratorGuards` and the `[orchestrator]` TOML section
  with a migration note (this project already has a precedent for that pattern —
  see `profile.rs`'s existing `cache_strategy`/`system_prompt` removal notes).

#### 3. `ToolServiceConfig::{default_timeout_seconds, per_tool_seconds}` — dead; the pipeline they fed was deleted
- Producer: `src/config/types/tools.rs:30-46` (struct), `:68-77`
  (`default_timeout()`), `:85-90` (`per_tool_durations()`).
- Consumer: **none.** `bin/aleph-server/commands/start/mod.rs:199-203` states
  outright that "the Phase 2 facade chain (`build_tool_service`,
  `PermissionLayer`, `TimeoutLayer`, etc.) was deleted in 2026-05-20 — gateway
  always overrides the harness's `tool_service` slot with a per-request
  `ScopedToolService`." Real per-tool timeout enforcement now lives in
  `src/tools/scoped/dispatch.rs::execute_inner` via
  `tools::budget::resolve_tool_budget_ms`, which reads a separate hardcoded
  table (`builtin_tool_budget_ms` + `DEFAULT_TOOL_BUDGET_MS`) and never touches
  `ToolServiceConfig`.
- Severity: **CRITICAL**
- Triage: **CUT** (or CONNECT into `resolve_tool_budget_ms`'s `declared`
  parameter if an operator-configurable override is still wanted — product
  decision).
- Reason: struct doc literally references the `TimeoutLayer`, a component
  removed months ago. `[tool_service] default_timeout_seconds = 5` /
  `per_tool_seconds = {"bash" = 300}` are silently ignored.
- Proposed fix: delete `default_timeout_seconds`, `per_tool_seconds`,
  `default_timeout()`, `per_tool_durations()`, `default_tool_service_timeout_seconds()`
  — keep only `parallel_tool_concurrency` (confirmed genuinely wired, see H3 above).

#### 4. `UnifiedToolsConfig.native.{fs,git,shell,system_info,screen_capture,search}` — entirely dead except `clipboard`
- Producer: `src/config/types/tools.rs:546-574` (`NativeToolsConfig`) +
  `FsToolConfig` (:582), `GitToolConfig` (:604), `ShellToolConfig` (:626),
  `SystemInfoToolConfig` (:661), `ScreenCaptureToolConfig` (:689),
  `SearchToolConfig` (:723).
- Consumer: **none in production.**
  `UnifiedToolsConfig::get_effective_tools_config()` (`src/config/methods.rs:23`)
  has exactly one production call site
  (`executor/builtin_registry/builder/constructor/mod.rs:398-405`), and that
  site only calls `.is_clipboard_enabled()`. Re-confirmed independently: grep
  for `.native.fs`, `.native.git`, `.native.shell`, `.native.system_info`, and
  the type name `NativeToolsConfig` across the whole repo returns zero hits
  outside `src/config/types/tools.rs` and `src/config/tests/`.
- Severity: **CRITICAL**
- Triage: **CUT** the 6 unused sub-configs (or **DECIDE** if
  `screen_capture`/`search` enablement is meant to gate real tools — it
  currently does not).
- Reason: `[unified_tools.native.shell] allowed_commands = [...]` and
  `[unified_tools.native.screen_capture] max_dimension = ...` are documented in
  the struct's own doc-example and round-trip in tests, but drive nothing — real
  gating happens through unrelated systems (`[projects] allowed_roots` for fs,
  `[sandbox.command_policy]` for shell, ExecTier for the rest).
- Proposed fix: delete the 6 dead sub-configs and their fields; keep `clipboard`
  (confirmed wired at `executor/builtin_registry/builder/constructor/mod.rs:404`).

#### 5. Legacy `ToolsConfig` enable/allowlist fields — entirely dead, security-shaped
- Producer: `src/config/types/tools.rs:101-156` — `fs_enabled`,
  `allowed_roots`, `git_enabled`, `allowed_repos`, `shell_enabled`,
  `allowed_commands`, `shell_timeout_seconds`, `system_info_enabled`.
- Consumer: only `ToolsConfig::from_legacy()` (tools.rs:387-433), which feeds
  straight into Finding #4's dead `NativeToolsConfig` chain.
  `config/ui_hints/definitions.rs:218-230` references the field paths, but only
  for settings-page label/help text — cosmetic, not gating.
- Severity: **CRITICAL**
- Triage: **CUT** (same dead chain as #4).
- Reason: `shell_enabled = false` reads as "the bash/shell tool is disabled" and
  is not — the real kill switch is `[sandbox.command_policy]`. A false sense of
  a working off-switch is a security-relevant finding, not cosmetic dead code.
- Proposed fix: delete alongside Finding #4; document that shell/fs/git gating
  lives in `[sandbox.command_policy]` / `[projects] allowed_roots`.

#### 6. Legacy `McpConfig`/`[mcp].external_servers` — entirely dead, no migration path exists
- Producer: `src/config/types/tools.rs:266-292` (`McpConfig.enabled`,
  `.external_servers`), `:299-326` (`McpExternalServerConfig`).
- Consumer: only reachable via `from_legacy()` → same dead chain as #4/#5.
  `external_servers` as a field name has zero other hits repo-wide (a same-named
  but unrelated `HashMap` field exists on `McpClient` in `src/mcp/client.rs` —
  confirmed to be a naming coincidence, not a consumer). The live MCP server
  list is migrated exclusively from `unified_tools.mcp` (a **different**, wired
  `HashMap<String, McpServerConfig>`) by
  `gateway/handlers/mcp_config.rs::migrate_unified_to_actor` (L459-524), which
  reads `cfg.unified_tools.mcp` directly and never `cfg.mcp`.
- Severity: **CRITICAL**
- Triage: **DECIDE** — either CUT `McpConfig`/`McpExternalServerConfig`
  entirely, or CONNECT by adding a `[mcp] external_servers` → actor-store
  migration mirroring `migrate_unified_to_actor`.
- Reason: if any deployment is still on the legacy `[mcp]` TOML format, it
  starts **zero** MCP servers, silently, with no warning anywhere. This is more
  severe than the sibling legacy findings because there's no substitute config
  path a stuck operator would stumble onto (unlike shell/fs which map onto
  `[sandbox]`/`[projects]`).

#### 7. `CodeExecConfigToml` (`[agent].code_exec`) — every field dead, validated only
- Producer: `src/config/types/agent/code_exec.rs:36-73` — `enabled`,
  `default_runtime`, `timeout_seconds`, `sandbox_enabled`, `allowed_runtimes`,
  `allow_network`, `working_directory`, `pass_env`, `blocked_commands`.
- Consumer: **none** except its own `.validate()` (boot-time syntax check only,
  via `CoworkConfigToml::validate()` → `Config::validate()` →
  `config/load.rs:184`). Grepped every field's dotted access and the type name
  `CodeExecConfigToml` across the whole repo: zero hits outside
  `src/config/types/agent/`. `builtin_tools/code_exec.rs` (the real tool) reads
  no `Config` field at all — its sandboxing/timeout comes from `[sandbox]`
  (`src/sandbox/config.rs`, a distinct, confusingly similarly-named struct).
- Severity: **CRITICAL**
- Triage: **CUT** (module doc's own TOML example even shows a stale
  `[cowork.code_exec]` table path, itself a staleness signal).
- Reason: `blocked_commands = ["rm -rf /", "sudo"]`, `allow_network = false`,
  `sandbox_enabled = true` are exactly the settings a security-conscious
  operator sets expecting real protection — none of it is enforced.
- Proposed fix: delete `CodeExecConfigToml` + its `.validate()` call, or wire it
  into `src/sandbox/`/`src/tools/scoped/dispatch.rs` alongside the real
  budget/sandbox mechanisms if the feature should exist.

#### 8. `FileOpsConfigToml` (`[agent].file_ops`) — every field dead, validated only
- Producer: `src/config/types/agent/file_ops.rs:33-65` — `enabled`,
  `allowed_paths`, `denied_paths`, `max_file_size`,
  `require_confirmation_for_write`, `require_confirmation_for_delete`.
- Consumer: **none** except its own `.validate()` (glob-syntax check only, same
  boot path as #7). Zero hits repo-wide for any field's dotted access or the
  type name outside `src/config/types/agent/`. The real confirmation-gate
  mechanism is `src/tools/scoped/` (ExecTier), per this project's own security
  model — `require_confirmation_for_delete` here has zero readers.
- Severity: **CRITICAL**
- Triage: **CUT**.
- Reason: `denied_paths = ["~/.ssh", "~/.gnupg"]` and
  `require_confirmation_for_delete = true` read as real safety controls but do
  nothing — identical anti-pattern to #7, and the same disease the module's own
  doc-comment already documents as retired for `[agent.subagents]` fields (this
  pair slipped through that earlier cleanup).
- Proposed fix: delete `FileOpsConfigToml` + `.validate()` call; if per-agent
  path allow/deny lists are still wanted, route them into `src/tools/scoped/`
  (ExecTier) / `src/sandbox/config.rs`, the actual enforcement point today.

#### 9. `WebFetchPolicy` (`[policies.web_fetch]`) is entirely disconnected from the tool it names
- Producer: `src/config/types/policies/web_fetch.rs:13-44` — `max_content_length`,
  `min_content_length`, `user_agent`, `timeout_seconds`, `enable_readability`,
  `crawl4ai`. Constructor exists: `WebFetchTool::with_policy()`
  (`src/builtin_tools/web_fetch/mod.rs:87-97`).
- Consumer: **none in production**, re-confirmed independently:
  ```
  $ grep -rn "with_policy(" src | grep -v tests
  src/builtin_tools/web_fetch/mod.rs:87:    pub fn with_policy(policy: &WebFetchPolicy) -> Self {
  $ grep -rn "WebFetchTool::new(\|WebFetchTool::with_policy(" src
  ... every production construction site uses WebFetchTool::new() ...
  ```
  Both production construction sites —
  `executor/builtin_registry/builder/constructor/mod.rs:53-77` and
  `executor/builtin_registry/definitions.rs:989` — build via `WebFetchTool::new()`
  (hardcoded `DEFAULT_MAX_CONTENT_LENGTH=10000`,
  `DEFAULT_MIN_CONTENT_LENGTH=100`, `DEFAULT_USER_AGENT="Aleph/1.0"`,
  `DEFAULT_TIMEOUT_SECS=30`, `enable_readability=true`), never
  `with_policy(&cfg.policies.web_fetch)`. `with_policy` itself has zero
  non-test callers anywhere in the repo.
- Severity: **CRITICAL**
- Triage: **CONNECT**
- Reason: `[policies.web_fetch] max_content_length = 50000` parses, validates,
  round-trips through `self_config`/`PoliciesConfig` — but every `web_fetch`
  call at runtime uses the hardcoded 10000/100/30s/"Aleph/1.0" defaults
  regardless.
- Proposed fix: in `executor/builtin_registry/builder/constructor/mod.rs`, build
  `WebFetchTool::with_policy(&cfg_guard.policies.web_fetch)` instead of
  `::new()`, then chain the existing `.with_ssrf_policy(...)` /
  `.with_fetch_providers(...)` calls on top.

---

### HIGH

#### 10. `AgentDefaults.{bootstrap_max_chars, bootstrap_total_max_chars}` — dead, actively documented in the user guide
- Producer: `src/config/types/agents_def.rs:112-117` (`Option<usize>` fields).
- Consumer: **none.** Grepped both identifiers across `*.rs`/`*.toml`/`*.md`:
  every hit is the field definition, its own doc comment, its own tests, or
  `docs/guides/agents.md:21`, which **actively instructs users to set
  `bootstrap_max_chars = 20000`**. The real bootstrap-file load path
  (`gateway/identity_loader.rs::load`/`load_agents_md`, via
  `agent_resolver/mod.rs:374-375`) has no char-limit or truncation logic of any
  kind, hardcoded or configurable.
- Severity: **HIGH**
- Triage: **DECIDE**
- Proposed fix: wire the limits into `IdentityFileLoader::load`/`load_agents_md`
  (truncate SOUL.md/AGENTS.md content to `bootstrap_max_chars`, cap the combined
  total to `bootstrap_total_max_chars`) — CONNECT; or remove the fields and the
  doc-guide instruction — CUT.

#### 11. `AcpAdapterEntry.preset` field-level backfill is a no-op (H4 refined — see Phase 0)
- Producer/doc claim: `src/config/types/acp.rs:167-168`.
- Actual behavior: `acp.rs:41-62` (`deserialize_adapters_with_presets`) and
  `gateway/handlers/acp_config.rs:160-168` only backfill the `preset` marker
  itself, never `executable`/`args`/`output_format`/`trust_level`/`display_name`.
- Consequence reaches production via `acp/adapters/generic.rs:35-53`
  (`GenericAcpAdapter::from_entry`).
- Severity: **HIGH**
- Triage: **CONNECT**
- Proposed fix: in `deserialize_adapters_with_presets`'s `Occupied` arm, when
  `id` matches a preset, backfill each field individually only if it's still at
  its type-level default — or add `AcpAdapterEntry::hydrate_from_preset(&mut
  self, spec: &PresetSpec)` and call it whenever `preset` resolves to a known
  id, so a partial `[acp.adapters.claude-code]` override behaves as documented.

#### 12. `ProfileConfig.{tools, temperature, max_tokens, history_limit, description}` and `is_tool_allowed()`/`effective_model()`/`effective_temperature()` are unreachable
- Producer: `src/config/types/profile.rs:40-73` (fields), `:169-243` (methods).
- Consumer: `ResolvedAgent.profile: ProfileConfig`
  (`agent_resolver/mod.rs:97-98`) carries the whole struct through, but only
  `profile.model` (`agent_resolver/mod.rs:354`) and `profile.smart_recall`
  (separate path, `gateway/agent_env/mod.rs`) are ever read. Grepped
  `.profile.tools`, `.profile.temperature`, `.profile.max_tokens`,
  `.profile.history_limit`, `.profile.description`, `profile.is_tool_allowed`,
  `profile.effective_model`, `profile.effective_temperature` across the whole
  repo — every hit is inside `profile.rs`'s or `agent_env/mod.rs`'s own test
  modules. The tool-permission enforcement path actually used at runtime is a
  different mechanism entirely: `AgentDef::is_tool_allowed`
  (`src/agents/types.rs:422`), consumed by `AllowlistToolService`.
- Severity: **HIGH**
- Triage: **DECIDE** (likely CUT with a migration note — this file already
  documents two prior removals of the same bug class: `cache_strategy` and
  `system_prompt`)
- Reason: `[profiles.readonly] tools = ["fs_read"]` reads as a security
  whitelist and is cosmetic — nothing calls `is_tool_allowed` on this struct
  anywhere. Given the project's own framing of "worse than dead code — a knob a
  user can SET", an inert tool-restriction knob is a security-relevant finding.
- Proposed fix: wire `profile.is_tool_allowed()` into the real tool-gating path
  (`AllowlistToolService`/`AgentDef` resolution) and thread
  `temperature`/`max_tokens`/`history_limit` into generation-parameter
  resolution; or remove the fields with the same removal-note style already
  used for `cache_strategy`/`system_prompt` in this file.

#### 13. `RoutingRuleConfig` — command dispatch drops `provider`/`preferred_model`/`intent_type`/`strip_prefix`; keyword rules never match anything
- Producer: `src/config/types/routing.rs:46-99` (struct + module doc promising
  command/keyword routing with provider selection, prompt injection, prefix
  stripping).
- Consumer: `tool_metadata/registry/registration.rs:316-377`
  (`register_custom_commands`, the only production reader of `config.rules`)
  reads only `rule.is_builtin`, `rule.regex`, `rule.system_prompt` (as static
  tool-description text). `rule.provider`, `.preferred_model`, `.intent_type`,
  `.strip_prefix`, `.icon` never reach `UnifiedTool`/`ToolSource::Custom`.
  Confirmed further: `gateway/execution_engine/slash_command.rs:223-228`'s
  `"custom"` mode arm unconditionally returns `ExecutionError::Fallthrough`;
  `execute.rs:468-497`'s fallthrough path runs the normal agent loop with
  `request.input` **unchanged**, never re-reading `mode_json["system_prompt"]`.
  Keyword rules (`is_keyword_rule()`) are exercised only in `config/validate.rs`
  and this file's own tests — zero runtime matching against inbound user text
  anywhere in the repo.
- Severity: **HIGH**
- Triage: **DECIDE**
- Reason: the CRUD RPC (`routing_rules.create/update/list`) plus validation
  give operators complete confidence that setting `provider`/`system_prompt`/
  `preferred_model` on a rule changes AI behavior on match; none of it reaches
  the LLM call. The entire "Keyword Rules" half of the documented feature has
  no runtime implementation.
- Proposed fix: either wire `rule.provider`/`preferred_model`/`system_prompt`
  into the `"custom"` fallthrough path (stamp into `request.metadata` before
  falling through, read during provider selection) and implement keyword-rule
  matching; or cut `provider`/`preferred_model`/`intent_type`/`strip_prefix` and
  rewrite the module doc to describe the actual behavior (regex → tool name +
  static description only).

#### 14. `CoworkConfigToml::planner_provider` (`[agent].planner_provider`) — validated at boot, never used to select a provider
- Producer: `src/config/types/agent/mod.rs:53-57`, doc: "If not specified, uses
  the default provider from `[general]`."
- Consumer: only `config/validate.rs:538-546`, which checks the named provider
  *exists* — nothing more. Real planner-provider construction
  (`build_strategy_planner_provider`,
  `orchestrator/deps_builder/summary.rs:216-`) reads `config.strategy.
  planner_model`/`.enabled` — a completely different `[strategy]` section —
  never `config.agent.planner_provider`.
- Severity: **HIGH**
- Triage: **DECIDE** (CONNECT preferred)
- Reason: `[agent] planner_provider = "claude"` gets a boot-time existence
  check and nothing else. This is the exact "validated at boot, read by
  nobody" anti-pattern the file's own module doc already calls out for the
  *retired* subagent fields — this one wasn't caught by that cleanup.
- Proposed fix: thread `config.agent.planner_provider` (when set) as the
  `primary_provider_key` override into `build_strategy_planner_provider`, or
  delete the field + its validate.rs branch.

#### 15. `WebFetchPolicy.crawl4ai` — read once for vault injection, then discarded; a parallel `[fetch]` config is the one actually used
- Producer: `src/config/types/policies/web_fetch.rs:88-120`
  (`Crawl4aiConfig`), populated with a vault secret at
  `bin/aleph-server/commands/start/mod.rs:783-791`.
- Consumer: none for the populated struct. `Crawl4aiBackend::from_config`
  (`builtin_tools/crawl4ai.rs:87`) is called only from
  `Crawl4aiFetchProvider::from_backend` (`fetch/providers/crawl4ai.rs:17-26`),
  which builds its **own** `Crawl4aiConfig` from a different section entirely,
  `[fetch].backends.crawl4ai` (`FetchBackendConfig`), wired through
  `FetchRegistry::from_config` → `WebFetchTool::with_fetch_providers`.
  `policies.web_fetch.crawl4ai` — including its vault-injected token — is never
  read again after the injection line.
- Severity: **HIGH**
- Triage: **CUT** (of `policies.web_fetch.crawl4ai`, in favor of the
  already-wired `[fetch].backends.crawl4ai`) or **DECIDE** if two config
  surfaces for the same backend is intentional.
- Reason: two independent config sections claim to configure the same backend;
  only one is live, and the dead one has its own dedicated vault-secret
  injection code that silently does nothing useful.
- Proposed fix: remove `Crawl4aiConfig`/`crawl4ai` from `WebFetchPolicy` and the
  injection block, or wire `Crawl4aiBackend::from_config(&cfg.policies.
  web_fetch.crawl4ai)` as a fallback when `[fetch].backends.crawl4ai` is absent.

#### 16. Generation `default_{image,video,audio,speech}_provider` never influence auto-selection
- Producer: `src/config/types/generation/config.rs:41-59` (fields),
  `GenerationConfig::get_default_provider()` (:149-157).
- Consumer: UI/RPC bookkeeping only
  (`gateway/handlers/generation_config.rs`,
  `handlers/generation_providers/handlers.rs` — badge display). Every
  generation tool that must auto-pick a provider when the model omits one
  falls back to `GenerationProviderRegistry::first_for_type()` —
  alphabetically-first enabled provider — not the configured default:
  `builtin_tools/generation/{image_generate.rs:143-148, video_generate.rs:104,
  audio_generate.rs:91, speech_generate.rs:193}`.
  `get_default_provider()` itself has zero non-test callers repo-wide.
- Severity: **HIGH**
- Triage: **CONNECT**
- Reason: an operator sees a "default" badge next to their chosen provider in
  Settings, but every implicit-provider generation call silently uses whatever
  sorts first alphabetically — a materially different, undocumented rule.
- Proposed fix: in each of the four generation tools' `call_impl`, when
  `args.provider` is `None`, try `registry.get(config.get_default_provider(
  GenerationType::X)?)` before falling back to `first_for_type`.

#### 17. Generation `output_dir` / `resolve_output_dir()` never reaches a save-to-disk path
- Producer: `src/config/types/generation/config.rs:369-385`
  (`resolve_output_dir`, `~` expansion + workspace fallback), field at :61-65.
- Consumer: only the RPC round-trip and Panel settings form.
  `resolve_output_dir()` has zero callers outside its own unit test. No file in
  `src/generation/` or `src/builtin_tools/generation/*.rs` references
  `output_dir`/`resolve_output_dir`/`GenerationConfig` at all.
- Severity: **HIGH**
- Triage: **DECIDE**
- Reason: need to confirm design intent — does generated media get persisted to
  disk today at all (vs. returned as URL/provider-hosted path/base64)? If a
  save step exists under a different name it should call this; if it doesn't
  exist yet, this is documented-but-unimplemented.
- Proposed fix: locate or build the save-to-disk step for
  `GenerationData::LocalPath`/binary outputs and route it through
  `config.generation.resolve_output_dir(&workspace_fallback)`.

#### 18. Generation `auto_paste_threshold_mb` — inert
- Producer: `src/config/types/generation/config.rs:67-70`
  (`default_auto_paste_threshold_mb`, 5 MB).
- Consumer: only the RPC/UI round-trip. No file-size comparison against this
  threshold exists anywhere in `src/generation/` or
  `src/builtin_tools/generation/`.
- Severity: **HIGH**
- Triage: **CONNECT** (has a hardcoded default that's simply never consulted —
  the described "files larger than this saved to disk instead of clipboard"
  feature does not exist in code).
- Proposed fix: implement the auto-paste-vs-save decision at the point
  generated media is returned to the channel/UI, gated on this threshold; until
  then, DECIDE whether to CUT with an "aspirational config" note instead.

#### 19. Generation `background_task_threshold_seconds` — inert
- Producer: `src/config/types/generation/config.rs:72-75`.
- Consumer: only the RPC/UI round-trip. Nothing compares an
  estimated/actual generation duration against this threshold to decide
  sync-vs-background execution.
- Severity: **HIGH**
- Triage: **DECIDE** (same shape as #18 — CONNECT if the sync/background split
  is still wanted, CUT otherwise).

#### 20. `CompoundIngestConfig.{replan_on_hash_conflict, failure_cooldown_seconds, tx_residue_gc_seconds}` — orphaned, no related mechanism exists anywhere
- Producer: `src/config/types/memory/ingest.rs:15-20` (fields), defaults in
  `memory/defaults.rs:256-266`.
- Consumer: **none anywhere in the repo.** `RelatedBudget`
  (`bin/aleph-server/commands/start/builder/agent_init/mod.rs:898-905`) — the
  struct that threads `CompoundIngestConfig` into the real ingest pipeline —
  only picks up `max_related_pages`, `preview_char_cap`, `total_byte_cap`,
  `dedup_enabled`, `dedup_similarity_threshold`, `dedup_noop_threshold`. No
  "hash conflict replanning," "failure cooldown," or "tx residue GC" concept
  exists anywhere in `src/memory/notes/ingest/`.
- Severity: **HIGH**
- Triage: **CUT** (describes a transactional-ingest resilience mechanism that
  was apparently never built, not one that regressed).
- Proposed fix: remove the three fields and their default fns, or mark them
  doc-noted as "not yet wired" if the feature is still planned.

---

### MEDIUM

#### 21. `AgentDefaults.dm_scope` — orphaned duplicate of the real `[session] dm_scope`
- Producer: `src/config/types/agents_def.rs:108-109` (`Option<String>`).
- Consumer: none — the wired sibling is `[session] dm_scope: DmScope` (enum,
  `config/structs.rs:179`) →
  `bin/aleph-server/commands/start/builder/subsystems.rs:566-575`, whose own
  comment says "Single source of truth for dm_scope: the user's `[session]`
  block." `AgentDefaults.dm_scope` is a different type (raw `String`) at a
  different path and is read nowhere outside its own test module.
- Severity: **MEDIUM**
- Triage: **CUT**
- Proposed fix: remove `AgentDefaults.dm_scope` and its doc example.

#### 22. `AcpAdapterEntry.cwd` — silently ignored for every preset-based harness (16 of them)
- Producer: `src/config/types/acp.rs:155-157` — no scoping caveat in the doc.
- Consumer: only `CustomAcpAdapter` (`acp/adapters/custom.rs:65`) reads
  `self.config.cwd`. `GenericAcpAdapter` — used for every entry where
  `preset.is_some()` (all 16 built-in presets) — never captures `entry.cwd` at
  all (`generic.rs:35-53`).
- Severity: **MEDIUM**
- Triage: **DECIDE**
- Proposed fix: thread `entry.cwd` into `GenericAcpAdapter` as a fallback
  default (mirroring `CustomAcpAdapter`), or document the restriction.

#### 23. `[skills]` config section (`SkillsConfig`) is entirely dead
- Producer: `src/config/types/skills.rs:15-60` — `enabled`, `skills_dir`,
  `get_skills_dir_path()`; registered as `Config.skills`
  (`config/structs.rs:78,365`).
- Consumer: none. Actual runtime skills directory comes from unrelated
  `utils::paths::get_skills_dir()` (ALEPH_HOME-based); actual per-skill
  enable/disable comes from unrelated `skill::config::SkillEntryConfig.enabled`
  (per-skill-id, not global).
- Severity: **MEDIUM**
- Triage: **CUT**
- Proposed fix: remove `SkillsConfig`/`Config.skills` with a migration note, or
  route `utils::paths::get_skills_dir()` through it if a configurable skills
  root is genuinely wanted.

#### 24. `SecretProviderConfig` (`[secret_providers.*]`) not wired into production secret resolution
- Producer: `src/config/types/secrets.rs:33-45` — doc promises 1Password/
  Bitwarden backends.
- Consumer: `gateway/handlers/secrets.rs` (list RPC, display-only) and
  `bin/aleph-server/commands/secret.rs:181` (CLI, constructs a throwaway
  `OnePasswordProvider` for a manual health-check only). Production
  `{{secret:name}}` resolution is `secrets/vault_resolver.rs::VaultSecretResolver`,
  whose own doc comment self-describes as "the only production resolver" —
  local vault only.
- Severity: **MEDIUM**
- Triage: **DECIDE** — appears to be a deliberately staged/partial feature
  (self-documenting comment on `VaultSecretResolver`), not a silent regression,
  but the limitation isn't disclosed in `secrets.rs`'s own module doc or the
  Panel UI.
- Proposed fix: implement multi-backend routing in `VaultSecretResolver`, or add
  an explicit doc caveat that non-local providers are list/health-check-only.

#### 25. `GenerationConfig::{get_provider, get_enabled_providers, get_providers_for_type}` — dead accessors, superseded API
- Producer: `src/config/types/generation/config.rs:161-169, 173-201, 207-237`.
- Consumer: none outside this file's own tests. Only `merged_providers()` (used
  by `generation_providers/handlers.rs` and `agent_init/generation_init.rs`)
  and the internal `validate_provider_reference()` are live.
- Severity: **MEDIUM**
- Triage: **CUT** — looks like an earlier typed-map lookup API superseded by
  `merged_providers()`, with nothing ever migrated to call it.

#### 26. Generation `smart_routing_enabled` — inert, no "smart routing" mechanism exists
- Producer: `src/config/types/generation/config.rs:77-79`, default `true`.
- Consumer: only the RPC/UI round-trip. Zero hits for `smart_routing` outside
  that triangle; no routing/selection logic in `src/generation/` branches on it.
- Severity: **MEDIUM**
- Triage: **CUT** (or DECIDE if "smart routing" is a planned feature — the flag
  currently gates nothing).

---

### LOW

#### 27. `AgentModelRef::model_str()` — dead accessor
- Producer: `agents_def.rs:163-168`. Consumer: zero non-test callers; the real
  resolution path (`agent_resolver::resolve_model_ref`) independently
  pattern-matches `AgentModelRef` rather than calling this method.
- Triage: **CUT** (or DECIDE if intended for a future display surface).

#### 28. `AcpAdapterEntry::{preset_claude_code, preset_codex, preset_gemini}` — dead pub fns
- Producer: `acp.rs:433-456`. Consumer: zero non-test callers; production code
  uses `preset_by_id(id)` instead.
- Triage: **CUT**.

#### 29. `PresetSpec.native_acp_args` — populated but never read
- Producer: `acp.rs:221`, set to `&["--acp"]` for all 16 presets.
- Consumer: `impl From<&PresetSpec> for AcpAdapterEntry` never copies it;
  `GenericAcpAdapter::from_entry` independently hardcodes the same literal.
  Currently harmless (values coincide) but editing this field would have zero
  effect.
- Triage: **DECIDE** (CUT the misleading field, or CONNECT it through
  `From<&PresetSpec>` if per-preset native-ACP args are ever needed).

#### 30. `RoutingRuleConfig::{get_provider, should_strip_prefix, get_intent_type, get_preferred_model}` — dead accessors
- Producer: `routing.rs:193,203,213,219`. Consumer: none outside tests —
  orphaned API surface for Finding #13's never-wired dispatch pass.
- Triage: **CUT** (or CONNECT together with #13 if that feature is revived).

#### 31. `McpServerConfig.triggers` — dead field
- Producer: `tools.rs:792-794`, "Trigger keywords for natural language command
  detection." Consumer: none —
  `unified_entry_to_manager_config()` (`gateway/handlers/mcp_config.rs:442-455`)
  never reads `sc.triggers`.
- Triage: **CUT** (also flagged by the sub-agent as R7-adjacent: keeping it
  invites a future contributor to wire keyword-based intent detection, which
  this project's own CLAUDE.md prohibits).

#### 32. `WebFetchPolicy::{timeout_duration, is_content_acceptable}` — dead accessors
- Producer: `policies/web_fetch.rs:123-133`. Consumer: none outside their own
  tests — direct consequence of Finding #9.
- Triage: **CUT** (re-evaluate once #9 is fixed — `is_content_acceptable` looks
  like it should gate `extract_content`'s acceptance logic in
  `web_fetch/extract.rs`, which currently only checks `min_content_length`
  inline).

#### 33. `CompressionPolicy::background_interval_duration()` — dead accessor (underlying field IS wired)
- Producer: `policies/memory.rs:54-59`. Consumer: none outside the module's own
  test — the field `background_interval_seconds` itself is read directly and
  correctly by `src/memory/compression/service.rs:62,633`, so there's no
  functional gap, just an unused convenience wrapper.
- Triage: **CUT**.

#### 34. M2-redux: still-`pub` dead accessors the original M2 fix didn't touch
- Producer: `tools.rs:68` (`ToolServiceConfig::default_timeout()`), `:478`
  (`UnifiedToolsConfig::shell_config()`), `:436-453`
  (`is_fs_enabled`/`is_git_enabled`/`is_shell_enabled`/`is_system_info_enabled`).
- Consumer: zero production callers (test-only or none at all).
- Triage: **CUT** — these fold naturally into Findings #3/#4 (the structs they
  read from are themselves dead), so deleting the parent structs removes these
  too; listed separately only because they weren't part of the original M2
  scope and would otherwise survive a narrow fix of #3/#4.

---

## Guard recommendations — `[policies.*]` (Phase 5)

| Field | Hardcoded fallback in use elsewhere? | Recommendation |
|---|---|---|
| `policies.web_fetch.{max_content_length,min_content_length,user_agent,timeout_seconds,enable_readability}` | Yes — `WebFetchTool::new()`'s `DEFAULT_*` consts | **CONNECT** (Finding #9) |
| `policies.web_fetch.crawl4ai.*` | No — a parallel `[fetch].backends.crawl4ai` is the real path | **CUT** or DECIDE (Finding #15) |
| `policies.exec_tier.*` | N/A — fully wired at `tools/scoped/builder.rs:235`, `execution_engine/slash_command.rs:395` | no action |
| `policies.mode` (session mode) | N/A — fully wired via `SessionMode::effective_core_tools()`/`defers_tool()` | no action |
| `policies.tool_permissions.*` | N/A — fully wired via `turn_permissions.rs:158-161`, `gate_chain.rs:246` | no action |
| `policies.memory.*` (`CompressionPolicy`) | N/A — field wired directly; only the convenience accessor (#33) is dead | no action beyond #33 |
| `policies.metrics`, `policies.guardian_review` | N/A — wired (`start/mod.rs:2864-2885` for guardian_review) | no action |

---

## Verified WIRED (no-op — do not re-flag; listed for completeness)

- **acp.rs**: `AcpConfig.{enabled,adapters}`, all other `AcpAdapterEntry` fields
  besides `preset`-backfill (#11) and `cwd` (#22), `HARNESS_PRESETS`,
  `all_presets()`, `preset_by_id()`, `preset_ids()`, `is_preset_id()`.
- **agents_def.rs**: `AgentsConfig.{defaults,list}`, `ensure_default()`,
  `AgentDefaults.{model,workspace_root,agents_root,skills,skills_blacklist}`,
  `AgentIdentity.*`, `AgentDefinition.{id,default,name,profile,model,skills,
  skills_blacklist,archetype,identity,tool_permissions,allowed_links,
  allowed_users}` (including `allowed_users`/`agent_admits_user` wired through
  `caller_identity.rs`). `AgentDefinition.workspace` is intentionally deprecated
  per its own doc comment — not a finding.
- **desktop.rs**: `DesktopDaemonConfig.allow_global_pointer` fully wired.
- **execution.rs**: every field (`default_timeout_secs`, `max_iterations`,
  `prompt_mode`, `progress_push`, `mid_turn_steering`, `max_runs_global`,
  `max_runs_per_agent`, `max_concurrent_subagents`,
  `busy_queue_max_per_session`, `busy_queue_max_wait_secs`,
  `busy_queue_wake_fallback_secs`, `max_pending_steering`) is consumed.
- **fetch.rs**: all fields, including `fallback_providers` and `enabled`.
- **general.rs**: `typing_speed`/`output_mode` wired into the Panel typewriter
  effect (a code comment there notes this was previously dead and has since
  been fixed — consistent with the pattern this batch found elsewhere).
- **group_chat.rs**: `GroupChatConfig.*`, `PersonaConfig.*`, both `.validate()`
  methods. (Minor: `GroupChatConfig::new()` has zero callers — only `::default()`
  is used; too trivial for a separate finding.)
- **moa.rs**: entirely wired (`MoaToml`, `MoaPreset`, `MoaSlot`, `MoaFanout`,
  `resolve_preset()`, `validation_errors()`).
- **phase6_wiring.rs**: entirely wired — best-wired file in the batch despite
  being specifically flagged for scrutiny (`GuardrailsToml`, `StabilityToml`,
  `FallbackProviderToml`, `ContextBudgetToml`, `ModelThresholdToml`,
  `StrategyToml` all reach `orchestrator/deps_builder/*`, `harness/agent/think.rs`).
- **privacy.rs**: every field consumed by `src/pii/engine.rs`.
- **profile.rs**: `SmartRecallConfig.*`, `ProfileConfig.model` (see #12 for what
  is NOT wired).
- **projects.rs**: `ProjectsConfig.allowed_roots` — the documented fs-access
  safety boundary, wired via `gateway/handlers/fs.rs`.
- **prompt.rs**: `PromptExtraFilesConfig.*` wired via
  `orchestrator/harness_bridge/prompt_build.rs` → `thinker/layers/extra_files.rs`.
- **provider.rs**: every `ProviderConfig` field, including niche ones
  (`system_prompt_mode`, `media_resolution`, `repeat_penalty`) reachable via the
  custom-protocol loader (`providers/protocols/{template,configurable,loader}.rs`).
- **resume.rs**: all fields consumed by `gateway/resume_coordinator.rs`.
- **route.rs**: `ModelRouteConfig` and friends fully consumed by
  `providers/route_policy.rs`, `route_handle.rs`, `route_observe.rs`,
  `failover/provider.rs` — confirmed orthogonal to, not conflicting with,
  routing.rs (route.rs = local/cloud tier + load-balance; routing.rs = regex
  command rules, and per #13 much of *that* is unwired, but not because of
  overlap with route.rs).
- **search.rs**: `SearchConfigInternal`/`SearchBackendConfig` fully wired
  through `search/{registry,factory,providers}`.
- **secrets.rs**: `SecretsConfig.{virtual_keys,custom_leak_patterns}` wired via
  `orchestrator_init.rs` → `security/runtime_guard.rs` /
  `secrets/virtual_key_resolver.rs`. (`SecretProviderConfig` is the one gap —
  Finding #24.)
- **security.rs**: `ShellSecurityConfig` and `CustomRiskPattern`/
  `CustomMaskPattern` fully wired end-to-end through `sandbox/factory.rs` →
  `SecurityKernel::from_config` and `start/helpers.rs::install_mask_patterns` →
  `exec/masker.rs`. Highest-stakes file in the batch and it checks out clean.
- **serde_helpers.rs**: `deserialize_models`/`deserialize_optional_models` have
  multiple real `#[serde(deserialize_with = ...)]` callers; no default/writer
  mismatch found.
- **stop_hooks.rs**: `StopHookConfig` fully wired via
  `verification/stop_hooks.rs::build_from_config`.
- **tools.rs**: `parallel_tool_concurrency` (H3), `core`/
  `truncate_tool_descriptions`/`defer_mcp_tools` (wired via `agent_init/mod.rs:
  809-811`), `unified_tools.mcp` HashMap (wired via direct field access in
  `migrate_unified_to_actor`, bypassing the dead `enabled_mcp_servers()`
  accessor — see #34), `is_clipboard_enabled()`/`.native.clipboard`,
  `UnifiedToolsConfig::from_legacy()` (called, though its output is mostly
  discarded downstream per #4-#6).
- **voice_local.rs**: entirely wired (`VoiceLocalConfig`, `StreamingConfig`,
  `FormatConfig`, `VoiceSection.*`, `vocabulary_hint()`, `normalize_voice_local()`).
- **agent/mod.rs**: the already-documented retirement of `[agent.subagents]`/
  `require_confirmation`/`max_parallelism` — verified accurate (parse-and-ignore
  by design, guarded by its own test).
- **dispatcher/core.rs**: `TeamDispatcherConfigToml`, `TeamBroadcastConfigToml`,
  `TeamMessagesConfigToml` — every field wired at
  `bin/aleph-server/commands/start/builder/agent_init/mod.rs` into
  `teams::dispatcher`/`teams::broadcast`/`teams::messages`.
- **generation/**: `image_providers`/`video_providers`/`speech_providers`/
  `audio_providers`/`transcription_providers`, `default_transcription_provider`,
  `merged_providers()`, `validate()` all wired. `generation/provider.rs` fields
  all wired. `generation/defaults.rs` flows through `to_params()`.
  `generation/presets/registry.rs`: 44-row `PROFILES` table fully wired through
  `presets/mod.rs` and `providers/catalog.rs` — no dead registrations, no
  bypassing parallel lookup.
- **memory/**: `assembler.rs`, `dreaming.rs`, `embed.rs`, `orientation.rs`,
  `profile.rs`, `reflection.rs`, `retrieval.rs` all traced to live consumers in
  `src/memory/`; these files carry extensive `// CUT:` doc comments from prior
  severed-wire remediation rounds (already-removed fields:
  `log_rotate_lines`, `inject_on_agent_switch`,
  `profile_inject_interval_turns`, `max_body_bytes`) — no new findings beyond
  #20. `memory/ingest.rs`'s `CuratedSection` has its own regression test
  (`every_curated_key_reaches_the_config_the_provider_reads`) guarding exactly
  this defect class.
- **policies/**: `exec_tier.rs`, `session_mode.rs`, `tool_permissions.rs`,
  `mod.rs`, `metrics.rs` fully wired to their documented enforcement chokepoints
  (see Guard recommendations table above). `policies/memory.rs` field wired
  (only its convenience accessor is dead, #33).

No `TODO`/`FIXME`/`unimplemented!`/`todo!`/suspicious empty match arms were
found in any of the 28 in-scope files.

---

## Conclusion

- **Phase 0 re-verification**: H3 is confirmed not a bug (deliberate layering,
  not doc drift). H4 was under-scoped by the prior audit — the real defect
  (preset backfill doesn't fill substantive fields) sits one comment away from
  what was checked. M2's fix commit (`fb4f942a5`) **did not actually change any
  visibility in `tools.rs`** — only added doc comments asserting a
  `pub(crate)` that isn't there; this is itself a documentation/code drift
  finding (M2-redux, #34).
- **Biggest cluster**: `src/config/types/tools.rs` and its `agent/{code_exec,
  file_ops}.rs` siblings carry 6 of the 9 CRITICAL findings — an entire
  generation of config surface (legacy `[tools]`, `[unified_tools.native.*]`
  minus clipboard, `[tool_service]` timeouts, legacy `[mcp]`, `[agent].
  code_exec`, `[agent].file_ops`) survived the deletion of the pipeline it used
  to feed (2026-05-20, per an explicit code comment) and is now pure decoration
  — several fields are exactly the security controls (`blocked_commands`,
  `denied_paths`, `shell_enabled`) an operator would reach for first.
- **Second cluster**: `[policies.web_fetch]` and `[generation]` have working
  UI/RPC round-trips whose runtime construction sites never consult them
  (`WebFetchTool::new()` ignoring `with_policy()`; `first_for_type()`
  alphabetical fallback ignoring `default_*_provider`).
- **Recommended prioritization**: fix #9 (WebFetchPolicy CONNECT — one-line
  constructor swap) and #11 (ACP preset backfill CONNECT) first, since both are
  small, well-scoped, and genuinely security/correctness relevant. Findings
  #3–#8 (the tools.rs/agent/ dead-config cluster) should go through a single
  DECIDE pass with a human, since cutting six structs at once is a larger
  change than this audit's read-only scope should resolve unilaterally — but
  they should not be left as-is, since several read as working security
  controls today.
