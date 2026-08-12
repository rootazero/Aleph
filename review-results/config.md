# Module: src/config

## Summary
- Path: `src/config/` (~76 files, ~26,299 lines)
- Total Rust public items: 144 (across `src/config/types/*.rs`)
- Issues found: 4 high-confidence, 3 medium, 4 low — see "Findings"

## Reviewers
- Wiring severed-wire audit (PRODUCED − CONSUMED)
- Static analysis (unwrap/expect/panic in non-test code)
- Configuration R1-R10 cross-check
- graphify-coupled entry/exit survey

## Severed-Wire Audit (Phase 1–3)

### Items verified as WIRED (no-op)
- `methods::add_rule_at_top` / `remove_rule` / `move_rule` — all called from `gateway/handlers/routing_rules.rs` (lines 157, 318, 384). Also referenced via webchat `routing_rules.rs` API.
- `ReloadImpact::classify` + `live_apply::apply_live_sections` — both consumed by `gateway/handlers/config.rs:597` and `patcher.rs:381, 631`. Guarded by `every_live_section_has_an_apply_arm` test.
- `ConfigPatcher` — wired into `gateway/handlers/config.rs:514`, `gateway/handlers/moa.rs`, `gateway/handlers/security_config.rs`, `self_config.rs` builtin tool.
- `AcpConfig` / `AcpAdapterEntry` — used heavily by `gateway/handlers/acp_config.rs` (preset merging, list, get, deserialize_adapters_with_presets).
- `presets_override::PresetsOverride` / `OwnedProviderPreset` / `OwnedGenerationPreset` — wired into `handlers/generation_providers`, `handlers/providers/helpers`, `providers/presets/mod.rs`, `types/generation/presets/mod.rs`.
- `defaults_override::get_defaults_override().{provider,memory,generation}_timeout_seconds` — consumed by `types/provider.rs:247`, `types/memory/defaults.rs:10`, `types/generation/provider.rs:107`.
- `ToolPermissionsConfig` (29 inbound consumers) — `channel_policy.rs`, `inbound_router/executor.rs`, `execution_engine/{tool_service_builder,turn_permissions}.rs`, `agent_instance.rs` all consume it.
- `defaults_override::init_defaults_override` + `load_defaults_override` — wired in `load.rs:116/117`, `load.rs:243/244` (both `if exists` and `if not exists` code paths).
- `deploy_guides` — `bin/aleph-server/commands/start/mod.rs:92`.
- `build_ui_hints` / `generate_config_schema_json` — `gateway/handlers/config.rs:365/366`, `tests/schema_integration.rs`.
- `AgentManager::create/update/delete/list/get` — wired via `bin/.../register_agents_handlers` (which overrides the `handlers/mod.rs` placeholder RPCs that error with "wire in Gateway startup"). `register_agents_handlers` is invoked at `start/mod.rs:1777`.
- `register_agents_handlers` also overwrites the `runtimes.install` placeholder.
- `register_cron_handlers` / `register_heartbeat_handlers` / `register_teams_handlers` / `register_graph_handlers` — all wired into `start/mod.rs`. Placeholder "service unavailable" RPCs in `handlers/mod.rs` are covered when the optional handle is registered.
- `ContextCompactor` builders (`with_monitor_scope`, `with_cache_carryover`, `with_summary_reuse`, `with_cheap_provider`) — all consumed by `agents/subagent_spawner.rs:1157-1172`.
- `manual::install_manual_compaction` / `manual_summarizer` / `manual_keep_tokens` — wired in `bin/.../orchestrator_init.rs:300` and `builtin_tools/sessions/compact_tool.rs:121`.
- `CompactorConfig` — set in `subagent_spawner.rs:1159`, `compact_tool.rs:124`, `tests/gateway_chat_common`, `tests/common`.
- `rescue::try_reactive_compact_and_retry` / `drain_context_overflow` / `RescueCx` / `RescueHost` / `MAX_REACTIVE_COMPACT_ATTEMPTS` — wired in `harness/agent/think.rs:590, 606, 649`, `tests/reactive_compaction.rs`, `tests/budget.rs`.
- `context::compact::directive::DirectiveOutcome::SplitTo` — used in `harness/agent/think.rs:475`.
- `RescueHost::note_rescue_attempt` / `account_discarded_tokens` / `mark_rescue_exhausted` / `reserve_rescue_slot` — implementations live in `harness/agent/think.rs` (search hits for `RescueCx` confirm).
- `context::compact::session_split::perform_session_split` — wired in `context/compact/directive.rs:149`, `context/compact/compactor.rs:787`, `tests/...extras.rs`.
- `retrieval::ContentIndex::open` — wired in `tools/result_store.rs:314`.
- `LoopDirective` / `ContextPressure` — wired in `orchestrator/harness_bridge/runner_impl.rs`, `harness/tests/budget.rs`, `tests/preflight_cheap_passes_e2e.rs`.

### Findings (high-confidence)

#### H1. `is_default_session` is a free function, not a method, but used as a serde `skip_serializing_if`
- Location: `src/config/structs.rs:43-46`
- Code:
  ```rust
  fn is_default_session(s: &crate::routing::config::SessionConfig) -> bool {
      s == &crate::routing::config::SessionConfig::default()
  }
  ```
- Usage: `src/config/structs.rs:183` — `#[serde(default, skip_serializing_if = "is_default_session")]`
- Risk: This is a free function bound to a serde attribute. It compiles fine. But: it captures a snapshot of `SessionConfig::default()` at field-emission time. If `SessionConfig::default()` ever picks up env-derived defaults (it currently does not), this becomes a derived-time-versus-serialize-time drift. **Not a bug today**; flagged for review when `SessionConfig` gains environment-derived defaults.

#### H2. `Schema::generate_config_schema_json` deserialization fall-through to `serde_json::json!({"not": {}})`
- Location: `src/config/patcher.rs:32-38`
- Code:
  ```rust
  SCHEMA.get_or_init(|| {
      let schema = generate_config_schema();
      serde_json::to_value(&schema).unwrap_or_else(|e| {
          tracing::error!("Config schema serialization failed: {}", e);
          serde_json::json!({"not": {}})
      })
  })
  ```
- Risk: **`{"not": {}}` is a Schema that *accepts everything* (a no-schema accept-all)**. If `generate_config_schema()` ever fails to serialize, `cached_config_schema()` will silently pass all config edits through `jsonschema` validation. This is a **silent type-system bypass** — exactly the "false Live" failure mode the patcher guards against elsewhere.
- Severity: **HIGH** — should return `Err`-side or panic instead. Replace with a panic + clear message, since the schema is generated from the same `Config` struct as the validator, and a serialization failure here means the type itself is unsound.

#### H3. `ToolServiceConfig::parallel_tool_concurrency` docstring misleading
- Location: `src/config/types/tools.rs:36-46`
- Doc claims: "`0` or `1` disables the parallel fast path (every batch runs serially); `>= 2` enables it."
- Code in `tools.rs:73`: `pub const fn parallel_tool_concurrency_opt(&self) -> Option<usize> { Some(self.parallel_tool_concurrency) }` — always Some.
- The harness logic that interprets `Some(0..=1)` as disabled is downstream (`subagent_spawner/mod.rs:765` uses `unwrap_or_else(...)`).
- Severity: **MEDIUM** — doc/code diverges from the actual consumer's behavior. Should either return `None` for `0..=1` or document the truncation at the source.

#### H4. `AcpConfig::default_adapters()` depends on `AcpAdapterEntry::all_presets()` — runtime-time inconsistency
- Location: `src/config/types/acp.rs:35-39`, `handlers/acp_config.rs:156-159`
- `default_adapters()` builds the default map by iterating `AcpAdapterEntry::all_presets()` at every `Config::default()` invocation. `all_presets()` is a static list. If the preset list is mutated at runtime (not currently), `Config::default()` would not pick up the change without a process restart.
- Severity: **LOW** — currently a static const, so no real risk. Flagged for awareness.

### Findings (medium)

#### M1. `already_serialized` (placeholder) RPCs in `gateway/handlers/mod.rs:851-925` for `agents.*` and `runtimes.install`
- These are **explicit placeholders** that error out with "wire in Gateway startup" until the real handlers are registered during `register_agents_handlers`. The pattern is documented (`// placeholders — actual handlers wired with AgentManager`) and the real handlers **do** overwrite these at `commands/start/builder/handlers/agents.rs:18-32` and `:75-103`.
- Risk: A `commands/start/` consumer that builds the server without going through `register_agents_handlers` will see INTERNAL_ERROR for `agents.*` RPCs. This is not a wiring bug — it is a documented surface.
- **Action**: None today. The pattern is by design.

#### M2. `validate.rs:705` — file size 705 lines
- Combined `validate` + `migrate_fetch` logic. Consider splitting JSON Schema validation into its own module. Severity: **LOW** (stylistic).

#### M3. `patcher.rs:1498` — file size 1498 lines
- The largest single file in `src/config`. Holds: `ConfigPatcher`, `PatchRequest`, `PatchResult`, `FieldDiff`, `HealthCheckResult`, `RollbackResult`, `SchemaCache`, `apply`/`rollback`/`dry_run` pipeline. Splitting would help readability but risks interface churn. **No action**.

### Findings (low)

#### L1. `defaults_override::get_defaults_override()` is a `&'static OnceLock`-backed singleton
- The `OnceLock` is initialized once per process. The `Config::load()` flow writes it **before** `Config::default()`. This is correct (validated by `tests/config_effective_path.rs:62`). The singleton survives across `Config::load()` calls until process exit. Multiple `Config::load()` calls will re-read the file **and re-initialize** the singleton (line 117). The clone at line 172 is necessary because the static is moved into `Config.defaults_override` for serialization.
- **Action**: Comments could be clearer; currently the precedence is implicit.

#### L2. `ChannelInstanceConfig::resolved_channels` consumes unknown channel keys with a warn-and-skip
- `src/config/structs.rs:332-334`: `tracing::warn!("Channel '{}' has no 'type' field and is not a known platform name, skipping")`. This is a soft-fail. The risk is that a user adds a channel and silently nothing happens.
- **Action**: Consider collecting the warnings and exposing them via `Config::validation_warnings()`. **LOW**.

#### L3. `structs.rs` mixes `Config` struct + `PluginMarketplaceEntry` + `ChannelInstanceConfig` + `is_default_session` helper
- The mixed contents could be split. Severity: **LOW** (stylistic).

#### L4. `Config::migrate_fetch` is callable but not invoked from `load.rs`
- `src/config/structs.rs:438-462` defines `migrate_fetch`. It is **not called by `load.rs`**. Search for `migrate_fetch` is restricted to the struct definition itself (`grep -rn "migrate_fetch" src/config/` returns only the implicit definition occurrence).
- Severity: **MEDIUM** — this is **dead wiring**. Either the migration is expected to be invoked by `Config::load()` (in which case it should be called there) or by an upgrade path (in which case it should be documented).
- **Action**: Wire `config.migrate_fetch();` into `Config::load_from_file` (`src/config/load.rs:79`) after `toml::from_str(...)` succeeds, or document the migration path explicitly.

## Architecture (R1-R10) check

- **R1 (Core no platform APIs)**: ✅ No platform API calls in `src/config/`. The `sandbox::SandboxConfig` is a pure data struct.
- **R2 (Complex UI in Leptos only)**: N/A (this is core).
- **R3 (Core minimalism)**: ✅ No heavy deps; only `serde`, `toml`, `schemars`, `tokio`, `tracing`.
- **R4 (Pure I/O shell)**: ✅ No business logic. `live_apply.rs` is the only place that touches runtime state, and it is a narrow, table-driven hot-apply that the module-level docstring explicitly defends.
- **R5-R10**: N/A / ✅.

## Production-grade patterns observed

- `live_apply.rs` enforces "the table that says live and the code that makes it live are the same table" via `every_live_section_has_an_apply_arm` test. Excellent guard against drift.
- `ReloadImpact::classify` is conservative: anything not known to be `Live` or `Inert` is `Restart`. This is the right default for an LLM-self-edit system.
- `ConfigBackup` uses atomic temp+rename writes (verified by `tests/backup_atomic`).
- `defaults_override` is read **before** `Config::default()` so serde defaults can pull from `OnceLock` — a subtle ordering bug avoided with a deliberate comment.
- `migrate_fetch` migration is conservative (no-op when both new and old are present).

## Conclusion

- **H1–H4**: All flagged but **H2 is the only actionable HIGH-severity bug** (`{"not": {}}` is a jsonschema accept-all sentinel; if the schema ever fails to serialize, every config edit bypasses validation silently).
- **L4 / M3-class issue**: `Config::migrate_fetch` is not wired into `Config::load()`. Either wire it or document the alternate path.
- The module is **otherwise cleanly wired**: every `pub fn`/`pub struct` I sampled has a consumer (RPC handler, builtin tool, or harness entry point). The `placeholder` RPCs in `gateway/handlers/mod.rs` are an explicit, documented boot-phase-2 pattern, not a severed wire — they are **overwritten** by `register_agents_handlers` before they are reachable in production.

### Recommended fixes

1. **H2**: Replace `serde_json::json!({"not": {}})` with a hard panic in `cached_config_schema()`. The schema is generated from the same `Config` definition; a serialization failure means the type itself is broken — proceeding with an accept-all sentinel would silently disable validation.
2. **L4 / M3**: Wire `config.migrate_fetch();` into `Config::load_from_file` after `toml::from_str` succeeds, or document the alternate invocation path.
3. **H3**: Clarify `parallel_tool_concurrency` docstring or move the `Option` clamp into the source.
4. **H1**: Add a test asserting `SessionConfig::default()` doesn't depend on env. (Defensive — already true today.)
