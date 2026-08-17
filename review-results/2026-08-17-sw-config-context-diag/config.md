# src/config — Severed Wire Audit (2026-08-17 round)

**Scanned:** 92 files under `src/config/` (incl. `agent_manager/`, `agent_resolver/`, `types/`, `tests/`, `ui_hints/`)

**Summary:** 7 candidates (5 CUT, 2 DECIDE, 0 CONNECT)

---

## config-001 · CUT · high confidence
- **Form:** inert_config · **Seam:** config_reader
- **Producer:** `src/config/types/orchestrator.rs:8` — `OrchestratorConfig.guards (OrchestratorGuards)`
- **Consumer:** none (no non-test reader anywhere in `src/`)
- **Rationale:** `OrchestratorConfig` + `OrchestratorGuards` fully defined and Default-constructed in `Config`, but no production code reads `is_rounds_exceeded` / `is_tool_calls_exceeded` / `is_tokens_exceeded` / `timeout`. Harness enforces limits via hardcoded constants in `execution_engine/engine.rs`. No observed pain.
- **Fix:** Delete `OrchestratorConfig` / `OrchestratorGuards` types and the `orchestrator` field from `Config`.

## config-002 · CUT · high confidence
- **Form:** inert_config · **Seam:** config_reader
- **Producer:** `src/config/types/agent/mod.rs:53` — `CoworkConfigToml` (incl. `planner_provider`, `file_ops.{allowed_paths, denied_paths, max_file_size, require_confirmation_for_write, require_confirmation_for_delete, enabled}`, `code_exec.{allowed_runtimes, blocked_commands, timeout_seconds, enabled}`)
- **Consumer:** only `Config::validate()` (recursive + key check on `agent.planner_provider`); zero non-test production reads
- **Rationale:** `file_ops` / `code_exec` configs duplicate policy enforcement already wired through `tool_permissions` / sandbox.
- **Fix:** Delete `CoworkConfigToml` / `FileOpsConfigToml` / `CodeExecConfigToml`, drop `agent` from `Config`.

## config-003 · CUT · high confidence
- **Form:** inert_config · **Seam:** config_reader
- **Producer:** `src/config/types/general.rs:62` — `BehaviorConfig.typing_speed`
- **Consumer:** none (only `behavior_config::handle_update`, `handle_get`, and 50-400 cps range validator read it)
- **Rationale:** Typewriter emission path (`gateway/inbound_router/executor.rs`, `gateway/event_emitter/`) keys only on `behavior.output_mode`; nothing reads `typing_speed` to throttle per-second emission.
- **Fix:** Delete `typing_speed` field + validators + getter returns.

## config-004 · DECIDE · medium confidence
- **Form:** inert_config · **Seam:** config_reader
- **Producer:** `src/config/types/profile.rs:33` — `ProfileConfig.{description, model, temperature, max_tokens, history_limit, smart_recall}`
- **Consumer:** `src/gateway/agent_env/mod.rs:54` references `ProfileConfig` as opaque struct
- **Rationale:** 3 sub-fields (`cache_strategy`, `system_prompt`, `tools`) already removed as dead. Remaining top-level fields unclear whether they reach runtime or silently persist. Need to trace through `agent_env/agent_resolver` to enumerate which fields drive behavior vs. inert.
- **Decision:** TBD — read `agent_env/mod.rs` to determine which fields are live.

## config-005 · DECIDE · low confidence
- **Form:** inert_config · **Seam:** config_reader
- **Producer:** `src/context/budget/...` — actually `src/config/types/memory/mod.rs:110` — `MemoryConfig.injection_mode`
- **Consumer:** only `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:497` (`injection_mode: app_config.memory.injection_mode`); not provable from grep whether downstream branches
- **Rationale:** One production read plumbs value to a downstream builder; need to trace whether downstream has `match injection_mode { ... }` or just serializes it back out as a no-op.
- **Decision:** TBD — read agent_init and downstream memory provider factory.

## config-006 · CUT · high confidence (informational)
- **Form:** stub_far_end · **Seam:** stub
- **Producer:** `src/config/ui_hints/mod.rs:1` — `ConfigUiHints` (empty DTO)
- **Consumer:** none
- **Rationale:** The producer half (`build_ui_hints()` + 686 lines of field-path literals) was already CUT in the 2026-08-12 audit. Empty DTO remains by design for forward-compat on `config.schema.ui_hints`. Re-exported via `pub use ui_hints::ConfigUiHints`.
- **Fix:** Leave as-is. Informational.

## config-007 · CUT · high confidence (cleanup fallout)
- **Form:** name_drift · **Seam:** path
- **Producer:** `src/config/structs.rs:96` — `Config.agent` serde alias accepting both `[agent]` and `[cowork]`
- **Consumer:** none (config-002's cleanup makes both spellings dead)
- **Rationale:** Falls out of config-002. Removing the alias simplifies `dead_keys.rs` (drop duplicate `cowork.*` entries).
- **Fix:** Remove `#[serde(alias = "cowork")]` from `Config.agent`, then prune `dead_keys.rs`.

---

## Verified LIVE (excluded)
The reviewer verified these config sections have non-test readers:
`[privacy]`, `[security.enable_custom_patterns/custom_blocked/custom_danger/mask_patterns]`, `[channels]`, `[acp.adapters]`, `[session.dm_scope]`, `[cron]`, `[heartbeat]`, `[tasks_reaper]`, `[resume]`, `[stop_hooks]`, `[search]`, `[fetch]`, `[projects]`, `[desktop.allow_global_pointer]`, `[tools.{core,truncate_tool_descriptions,defer_mcp_tools}]`, `[sandbox]`, `[policies.{exec_tier,tool_permissions,mode,guardian_review,memory.compression,memory.session_compactor,web_fetch.crawl4ai,metrics}]`, `[moa]`, `[fallback_provider]`, `[guardrails]`, `[stability]`, `[context_budget]`, `[strategy]`, `[team_dispatcher]`, `[team_broadcast]`, `[team_messages]`, `[generation.*]`, `[providers.*]`, `[secret_providers]`, `[secrets_config]`, `[plugin_marketplaces]`, `[voice_local.*]`, `[voice.streaming]`, `[voice.format]`, `[voice.llm_provider/model/vocabulary]`, `[voice.local.*]`.