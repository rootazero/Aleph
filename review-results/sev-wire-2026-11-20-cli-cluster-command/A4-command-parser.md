# A4 — `src/command` Severed-Wire Audit

Scope: full reads of `src/command/mod.rs` and `src/command/parser.rs`; followed their production consumers in `src/gateway/inbound_router/command_handler.rs`, `src/gateway/execution_engine/slash_command.rs`, and `src/extension/visibility.rs`, plus `src/config/types/routing.rs` and custom-command registration. No source changes, commits, or cargo commands.

## Part A — Prior findings

### #5 — custom slash `system_prompt` parsed then dropped
- Verdict: FIXED (for the reported mode-JSON drop).
- Form: 1
- Producer: `src/config/types/routing.rs:124` — `RoutingRuleConfig.system_prompt`; copied into the registered tool at `src/tool_metadata/registry/registration.rs:620-628` as its description and `routing_system_prompt`.
- Consumer: `src/tool_metadata/registry/registration.rs:615-626` — the description is the active model-facing path; `src/gateway/inbound_router/command_handler.rs:193-198` serializes Custom with only `tool_id` and args, not a prompt copy.
- Live caller search: `rg -n "register_custom_commands|routing_regex|routing_system_prompt|system_prompt" src/tool_metadata/registry/registration.rs src/tool_metadata` finds production registration and prompt assignment; `rg -n "ParsedCommand|CommandContext|parse_async|serialize_parsed_command|tool_registry_arc|tool_registry\\(" src/gateway/inbound_router/command_handler.rs src/gateway/execution_engine src/command` finds the live serializer and engine path.
- Symptom: None for the reported redundant mode-JSON copy; the configured prompt is used as the tool description. `routing_system_prompt` itself has no consumer in `alephcore`, as documented in `src/config/types/routing.rs:68-74`.
- Hypothesis: CUT — the stale copy into the mode envelope has been removed; the separately retained routing metadata is explicitly documented as unused, while the description carries the prompt to the model.
- Confidence: high

### #27 — public `tool_registry_arc()` has zero callers
- Verdict: FIXED.
- Form: 1
- Producer: `src/command/parser.rs:166-179` — only the borrowed `tool_registry()` accessor exists now; there is no `tool_registry_arc()` definition.
- Consumer: `src/gateway/inbound_router/mod.rs:1076`, `src/gateway/inbound_router/command_handler.rs:258,540` — live callers use `tool_registry()`; the in-module test at `src/command/parser.rs:431` also checks identity.
- Live caller search: `rg -n "tool_registry_arc" src` returns no matches; `rg -n "\\.tool_registry\\(\\)|\\.tool_registry_arc\\(\\)" src --glob '*.rs'` finds the three production call sites and one test.
- Symptom: None; callers have a borrowed accessor and do not create a second catalog owner.
- Hypothesis: CUT — the orphaned Arc-cloning API described by the finding is absent; no further action indicated.
- Confidence: high

### #28 — unknown `ToolSource` variant degrades silently to plain chat
- Verdict: FIXED.
- Form: 2
- Producer: `src/command/parser.rs:187-205` — non-exhaustive unknown-source arm returns `None` from `tool_to_command_context`; `parse_async` propagates it at `src/command/parser.rs:154`.
- Consumer: `src/command/parser.rs:198-204` — the unknown-source arm emits `tracing::warn!` with tool id/source and explicitly reports degradation to a plain chat turn.
- Live caller search: `rg -n "tool_to_command_context|parse_async" src/command/parser.rs src/gateway/execution_engine src/gateway/inbound_router/command_handler.rs` finds the parser-to-engine paths; `rg -n "unknown tool source" src/command/parser.rs` finds the production warning.
- Symptom: The unknown source still fails closed and the request may proceed as ordinary chat, but it is no longer silent; a warning records why.
- Hypothesis: CUT — the silent observability wire is repaired by the warning; no handler can safely infer a context for an unknown source.
- Confidence: high

### #30 — `pattern` falls back to command name when catalog regex is absent
- Verdict: FIXED.
- Form: 1
- Producer: `src/command/parser.rs:12-37` — `ParsedCommand` has no `pattern` field; `src/command/parser.rs:51-107` — `CommandContext` has no pattern field.
- Consumer: `src/tool_metadata/registry/registration.rs:620-628` — custom command regex remains on the catalog tool as `routing_regex`; the command parser does not manufacture a pattern from the name.
- Live caller search: `rg -n "pattern" src/command/parser.rs src/gateway/inbound_router/command_handler.rs src/gateway/execution_engine` finds no parser-produced pattern path; `rg -n "routing_regex|with_routing_regex" src/tool_metadata src/gateway` confirms regex is retained on catalog metadata.
- Symptom: None for the reported fallback; no synthetic non-regex pattern is serialized or dispatched.
- Hypothesis: CUT — the `pattern` output has been removed from this parser contract; regex remains catalog metadata.
- Confidence: high

## Part B — New candidates

### A4-01 — Retired routing-rule knobs parse but have no production effect
- Form: 3
- Producer: `src/config/types/routing.rs:112-119` — `provider`; `src/config/types/routing.rs:129-143` — `strip_prefix`, `intent_type`, `preferred_model`, `icon`. These deserialize from operator TOML. `get_intent_type` and `get_preferred_model` are only test-exercised (`src/config/types/routing.rs:231-245`).
- Consumer: no production consumer found for the values as routing behavior (grepped: `rg -n "\\.strip_prefix|\\.preferred_model|\\.intent_type|\\.icon" src --glob '*.rs'`; `rg -n "rule\\.provider|\\.provider" src/config src/tool_metadata src/gateway --glob '*.rs'`; `rg -n "get_intent_type|get_preferred_model" src --glob '*.rs'`). Note: `provider` is read by config validation, but no routing/execution path consumes it; its required validation does not route a command. `RoutingRuleConfig` is passed to registration (`src/tool_metadata/registry/registration.rs:578-628`), which reads `is_builtin`, `regex`, and `system_prompt`, not these knobs.
- Live caller search: `rg -n "RoutingRuleConfig" src --glob '*.rs'` finds production config/registration uses and test fixtures; the value-level searches above find no production routing readers. Getter searches find only their declarations and `#[cfg(test)]` tests.
- Symptom: TOML values appear accepted (and `provider` is even required by validation), but do not select a provider/model, strip input, supply intent metadata, or set command display icon. Test-only getter assertions do not create a production path.
- Hypothesis: DECIDE — these are explicitly retained for compatibility and documented as retired in `src/config/types/routing.rs:50-76`; whether to keep accepting them or reject/remove them is a config compatibility decision, not a missing command-parser connection to guess at.
- Confidence: high

Parsed command result fields do have downstream readers: `source_type` and `command_name` are used for engine logging (`src/gateway/execution_engine/slash_command.rs:174`); `arguments`, `context`, `tool_id`, and `owning_plugin` are serialized (`src/gateway/inbound_router/command_handler.rs:173-213`); context payloads `tool_name`, `skill_id`, `display_name`, `allowed_tools`, and `server_name` are serialized there and consumed by execution-engine mode branches (`src/gateway/execution_engine/slash_command.rs:200+`). `owning_plugin` is derived by `catalog_owner` (`src/extension/visibility.rs:195-207`), serialized at `src/gateway/inbound_router/command_handler.rs:212-213`, and gated at `src/gateway/execution_engine/slash_command.rs:190-198`.

## Seams not covered

- Event emit/subscribe parity was not applicable to these parser/config structs; no event bus contract is produced by this slice.
- HTTP route parity and rate-limiter/network-policy classifier parity were not applicable to this parser boundary.
- The broader `ToolCatalog` registration/dispatch universe was sampled only for custom-command regex/prompt provenance and the parser's named fields; this was not a workspace-wide tool parity audit.
