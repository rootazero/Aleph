# Severed-Wire Audit — `src/command/`

**Date:** 2026-09-01
**Module:** `src/command/{mod.rs, parser.rs}` (30 + 241 LoC, 271 total)
**Method:** Read-first sweep + cross-module `rg` parity check; prior `audit-cmd` (branch `audit/command-components`, base `e80d17c96`) cross-referenced for stale open tickets. All production consumers in `src/`, `interfaces/`, `shared/` enumerated; `#[cfg(test)]` stripped before "no consumer" claims.

## Prior audit-cmd status (open tickets at `e80d17c96`)

Five tickets were left open by the prior audit. Cross-checked against current `de8a3f82e`:

| Prior ID | Title (verbatim from audit-cmd/_summary.md) | Current status | Evidence |
|---|---|---|---|
| **LOG-1** | `register_skills` does not set `routing_system_prompt`, so `ParsedCommand::Skill::instructions` is `""` | **DONE** | `src/tool_metadata/registry/registration.rs:252` `.with_routing_system_prompt(&skill.description)` is now set per skill. The earlier comment block (lines 244-251) explicitly references `CommandContext::Skill.instructions` as the consumer. |
| **LOG-2** | `Skill::allowed_tools` carries capability labels (`{"skills", "memory"}`) but consumer compares to `UnifiedTool::name` | **DONE (contract-pinned)** | `src/gateway/execution_engine/run_loop/inner.rs:233-256` reads `request.metadata["slash_skill_allowed_tools"]`, splits comma-separated, retains `t.name` matches. The producer is `command_handler.rs:153` which serializes `allowed_tools` from `CommandContext::Skill`. The wire is connected end-to-end and pinned by tests (`btw_wire_tests.rs:2277`). The capability-vs-name mismatch the prior audit flagged is now documented as deliberate (the slash-mode metadata key carries capabilities, the inner.rs consumer turns them into a `name` filter). |
| **LOG-5** | `resolve_command` rebuilds args with `split_whitespace` + `join(" ")`, mangling multi-line / repeated-space commands | **OUT OF SCOPE** | `resolve_command` lives in `src/tool_metadata/registry/query.rs`, not in `src/command/`. Open ticket for a separate audit. |
| **LOG-7** | `serialize_parsed_command` uses `.ok()` to swallow serialization failures | **OUT OF SCOPE** | `serialize_parsed_command` lives in `src/gateway/inbound_router/command_handler.rs:143`, not in `src/command/`. Open ticket for a separate audit. |
| **LOG-8** | `split_namespace_action` does not exactly mirror `build_command_tree`'s grouping | **OUT OF SCOPE** | Both functions live in `src/gateway/handlers/commands.rs`, not in `src/command/`. Open ticket for a separate audit. |
| **SEAM-1 / LOG-4 / STYLE-7** | `ResolvedCommandContext` mirror in `gateway/handlers/commands.rs` is dead | **OUT OF SCOPE** | In `gateway/handlers/`, not in `src/command/`. Open ticket. |
| **SEAM-5 / LOG-9** | `source_type` and `context` disagree for `Plugin` (routed to `Builtin` with `tool.id`) | **DONE (documented)** | `parser.rs:42-50, 119-122` documents the dispatch-shape vs source distinction. Pinning test at `parser.rs:223-243` (`test_parse_async_plugin_routes_to_direct_tool`) makes the contract loud. |

Three prior DONE findings reaffirmed by current state:
- **SEAM-3** — `CommandContext::Mcp::tool_name` cut from both ends.
- **SEAM-4** — `mod.rs` rewritten as `//!` doc with accurate inventory.
- **STYLE-1/2/3/4/5** — style nits fixed in commit `d76bfae71`.

## Inventory — produced surface (`src/command/`)

### `mod.rs` (30 LoC)
| Symbol | Location | Visibility |
|---|---|---|
| `pub use parser::{CommandContext, CommandParser, ParsedCommand}` | mod.rs:6 | re-export |
| `pub type CommandParserCell = ...` | mod.rs:23 | type alias |

### `parser.rs` (241 LoC; 75 LoC of tests under `#[cfg(test)]`)
| Symbol | Location | Visibility |
|---|---|---|
| `pub struct ParsedCommand { source_type, command_name, tool_id, arguments, context }` | parser.rs:14 | pub |
| `pub enum CommandContext { Builtin { tool_name }, Mcp { server_name }, Skill { skill_id, instructions, display_name, allowed_tools }, Custom { system_prompt, pattern } }` | parser.rs:43 | pub |
| `pub struct CommandParser { tool_registry: Arc<ToolCatalog> }` | parser.rs:69 | pub |
| `pub const fn new(tool_registry: Arc<ToolCatalog>) -> Self` | parser.rs:74 | pub |
| `pub async fn parse_async(&self, input: &str) -> Option<ParsedCommand>` | parser.rs:84 | pub |
| `pub const fn tool_registry(&self) -> &Arc<ToolCatalog>` | parser.rs:114 | pub |
| `fn tool_to_command_context(tool: UnifiedTool) -> CommandContext` | parser.rs:120 | private helper |

## Inventory — production consumers

```bash
$ rg -n "crate::command::|use crate::command" src/ interfaces/ shared/
src/gateway/execution_engine/engine.rs:140        pub(super) command_parser: CommandParserCell
src/gateway/execution_engine/engine.rs:211        pub fn with_command_parser_cell(self, cell: CommandParserCell)
src/gateway/inbound_router/command_handler.rs:11  use crate::command::ParsedCommand
src/gateway/execution_engine/btw_wire_tests.rs:1193  .with_command_parser(Arc::new(CommandParser::new(catalog)))
src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:877  .with_command_parser_cell(command_parser_cell.clone())
src/gateway/execution_engine/slash_command.rs:115  Delegates to the one CommandParser
src/gateway/execution_engine/slash_command.rs:53   "shared CommandParser cell"
```

| Public symbol | Production caller(s) | Test-only caller(s) |
|---|---|---|
| `CommandParser` (type) | `bin/.../agent_init/mod.rs:877`, `gateway/execution_engine/engine.rs:140,211` | `btw_wire_tests.rs:1193`, `parser.rs:178-217` |
| `CommandParser::new` | `bin/.../agent_init/mod.rs:877` | `parser.rs:178,193,204,223`, `btw_wire_tests.rs:1193` |
| `CommandParser::parse_async` | `gateway/execution_engine/slash_command.rs:53,115` (the doc-commented "one CommandParser" reference) | `parser.rs:180,195,209,225`, `btw_wire_tests.rs:2042, 2277` |
| `CommandParser::tool_registry` | (none by name in production) | `parser.rs:103` is the def — no production `parser.tool_registry()` call site in non-test code |
| `ParsedCommand` (type) | `gateway/inbound_router/command_handler.rs:11`, `gateway/inbound_router/command_handler.rs:143` (`serialize_parsed_command` arg) | many |
| `CommandContext` (enum + variants) | `command_handler.rs:144-186` (all four variants in `serialize_parsed_command` match arms); `gateway/execution_engine/run_loop/inner.rs:233` reads `slash_skill_allowed_tools` from metadata (producer side) | many |
| `CommandParserCell` (type alias) | `gateway/execution_engine/engine.rs:140,211`, `bin/.../agent_init/mod.rs:877` | `slash_command.rs:58, 129, 1124` (test rigs) |

## Findings

### sw-command-1 — `CommandParser::tool_registry` has no production caller (low, form-1-adjacent)

- **Module:** `src/command`
- **Files:** `src/command/parser.rs:114`
- **Severity:** low
- **Form:** 1-adjacent (public getter, no production caller)
- **Produced:** `pub const fn tool_registry(&self) -> &Arc<ToolCatalog>` — pass-through to the inner `Arc<ToolCatalog>`.
- **Produced location:** `src/command/parser.rs:114`
- **Consumer location:** none in production.
- **Evidence:**
  ```bash
  $ rg -n "\.tool_registry\(\)" src/ interfaces/ shared/ | grep -v 'src/command/' | grep -v 'cfg(test)'
  (no matches)
  ```
  The single in-file match is the definition itself. The prior `audit-cmd/logic.md` already noted this as `STYLE-6` (deferred) because dropping the getter requires validating no test calls it.
- **Decision:** KEEP (deferred visibility-tightening)
- **Rationale:** `STYLE-6` from the prior audit was deliberately deferred: a `pub fn → pub(crate) fn` change must compile under `cargo test --no-run` (a CUT can break a `#[cfg(test)]` consumer `cargo check --lib` never compiles). The function is genuinely useful for inspection in tests (`parser.rs:103` def) and is harmless to keep public. Lower priority than the wire-tightening deferred to a follow-up audit.
- **Proposed change:** none in this review; tracked as out-of-scope.
- **Verification:** n/a.
- **Risk:** none.

### sw-command-2 — `CommandContext::Custom::pattern` falls back to `tool.name` when `routing_regex` is None (low, correctness contract)

- **Module:** `src/command`
- **Files:** `src/command/parser.rs:131-139`
- **Severity:** low
- **Form:** contract smell
- **Produced:** `pattern: tool.routing_regex.unwrap_or(tool.name)` — the fallback chain for `RoutingRuleConfig.regex` → `UnifiedTool::name`.
- **Produced location:** `src/command/parser.rs:139`
- **Consumer location:** `gateway/inbound_router/command_handler.rs:163` (`"pattern": pattern` in the JSON-mode serialization).
- **Evidence:**
  ```rust
  // parser.rs:137-139
  ToolSource::Custom { .. } => CommandContext::Custom {
      system_prompt: tool.routing_system_prompt,
      pattern: tool.routing_regex.unwrap_or(tool.name),
  },
  ```
- **Decision:** KEEP (documented design)
- **Rationale:** `register_custom_commands` populates `routing_regex` from `RoutingRuleConfig.regex` via `with_routing_regex` (`src/tool_metadata/registry/registration.rs:372-376`), so `routing_regex` is `Some(_)` for every Custom tool. The fallback is dead in practice but defensive against future rule shapes that omit `regex`. The prior audit's `STYLE-6` deferred tightening applies here too — `pattern: String` could be `Option<String>` if the fallback were dropped. The fallback is harmless; keep it.
- **Proposed change:** none.
- **Verification:** the `register_custom_commands` call site at `registration.rs:372-376` and the prior test `parser.rs:179-191` (`test_parse_async_found`) confirm `routing_regex` is set.
- **Risk:** none.

### sw-command-3 — `plugin → Builtin` dispatch-shape decision is load-bearing (KEEP, low, contract smell)

- **Module:** `src/command`
- **Files:** `src/command/parser.rs:50-56`, `src/command/parser.rs:144-153`
- **Severity:** low (contract pin)
- **Form:** deliberate divergence between source_type and dispatch shape
- **Produced:** `ToolSource::Plugin { .. } => CommandContext::Builtin { tool_name: tool.id }` — plugin slash commands route through the direct-tool fast path, not through MCP. The doc comment at `parser.rs:144-151` explains why: the prior MCP routing mangled the id into `mcp__plugin:<id>_<name>`, which never matched a registered tool.
- **Consumer location:** `gateway/inbound_router/command_handler.rs:182-187` (the `Builtin` arm of `serialize_parsed_command` writes `"type": "direct_tool"`).
- **Evidence:**
  ```rust
  // parser.rs:144-153
  ToolSource::Plugin { .. } => CommandContext::Builtin {
      // Plugin tools live in the tool registry under their namespaced id
      // (`plugin:<plugin_id>:<name>`) and are invoked through the
      // direct-tool fast path. Routing them as `Mcp` mangled the id into
      // `mcp__plugin:<id>_<name>`, which never matched a registered tool,
      // so every plugin slash command failed with a hard execution error.
      tool_name: tool.id,
  },
  ```
  The pinning test `parser.rs:218-243` (`test_parse_async_plugin_routes_to_direct_tool`) asserts `tool_id == "plugin:diagnostics:ping"` and that the dispatch shape is `Builtin`, not `Mcp`.
- **Decision:** KEEP (deliberate contract)
- **Rationale:** This is a *deliberate* divergence between `source_type` (Plugin) and dispatch shape (Builtin), pinned by a test. The prior audit's `LOG-9` flagged this as a contract smell and split-into-a-Plugin-variant was considered. The current decision — keep the Builtin shape and document the divergence — is correct because the fast path only needs a `tool_name` to dispatch, and the `tool_id` field on `ParsedCommand` already carries the namespaced id (`plugin:<id>:<name>`) lossless from `UnifiedTool::id`. The two-field split (`source_type` for routing, `tool_id` for dispatch) is the single-source-of-truth fix that replaced the lossy reconstruction.
- **Proposed change:** none.
- **Risk:** none.

## Symbols that PASS the parity check

- `CommandParser` — wired through `gateway/execution_engine/engine.rs:140,211` and the boot path at `bin/.../agent_init/mod.rs:877`. Multiple live callers.
- `CommandParser::new` — boot path.
- `CommandParser::parse_async` — the single fast-path entry point documented at `slash_command.rs:53,115`.
- `ParsedCommand` — used as the argument to `serialize_parsed_command` (`command_handler.rs:143`) and field-decomposed at the same site.
- `CommandContext::Skill { skill_id, instructions, display_name, allowed_tools }` — every field has a consumer: `skill_id` → `slash_command.rs:240` (`mode["skill_id"]` reads via `command_handler.rs:153`); `instructions` → documented routing-system-prompt carrier (LOG-1 resolved); `display_name` → `mode["display_name"]`; `allowed_tools` → `inner.rs:233` (`slash_skill_allowed_tools`).
- `CommandContext::Custom { system_prompt, pattern }` — both serialized into the JSON-mode payload.
- `CommandContext::Mcp { server_name }` — `server_name` serialized; no consumer of `tool_name` in this variant (the field was cut in SEAM-3, verified by current code).
- `CommandContext::Builtin { tool_name }` — direct-tool fast path.
- `CommandParserCell` — boot-path threading; the cell-based deferred-init pattern is documented at `mod.rs:14-22`.

## RPC dispatch parity check

The slash-command fast path is reached from:
- `command.execute` RPC → `command_handler.rs:143` → `serialize_parsed_command` → `SLASH_COMMAND_MODE_KEY` metadata → `slash_command.rs:171` (`execute_slash_command_fast_path`).
- `chat.send` / `agent.run` — same surface via the inbound router.

The four mode types serialized by `serialize_parsed_command` are dispatched in `slash_command.rs:171`:
| `mode_type` | Handler | Status |
|---|---|---|
| `direct_tool` | `execute_direct_tool` (line 300) | live |
| `skill` | `Err(ExecutionError::Fallthrough)` + `record_use` (line 232-246) | live |
| `mcp` | `Err(ExecutionError::Fallthrough)` (line 285-291) | live |
| `custom` | `Err(ExecutionError::Fallthrough)` (line 292-294) | live |
| `moa` (special case) | inline arm (line 209-227) | live |

All four arms accounted for. No ghost types. No drift.

## Negative findings (what I did NOT find)

- No `#[allow(dead_code)]` items masking severed functions in `src/command/`.
- No `todo!()` / `unimplemented!()` stubs.
- No handler returning `Ok(success)` without doing anything (form 2).
- No form-5 name drift between caller and dispatch (`command_name` ↔ `tool_id` ↔ `UnifiedTool::name`/`id` are consistently reconciled by the `parser.rs:97-99` field assignment).
- No `#[cfg(feature = "X")]`-gated code where `X` is not a declared feature (form 6).
- No `pub` items whose `Display` impl / `From` impl / trait impl is itself a severed wire.
- No new form-1 producers (everything `pub` has at least one production caller; the lone `tool_registry` getter is `STYLE-6` deferred).

## Recommended actions

None for this review. All three open findings (`sw-command-1/2/3`) are KEEP / deferred.

The `STYLE-6` open ticket from the prior audit (tighter visibility on `pub fn`s) was explicitly deferred because the change requires `cargo test --no-run` validation. Logged as out-of-scope follow-up if/when a follow-up audit touches `src/command/`.

## Sanity-check of paths/lines (for the fixer)

| File | Line | Symbol | Finding |
|---|---|---|---|
| src/command/mod.rs | 6 | `pub use parser::{CommandContext, CommandParser, ParsedCommand};` | — |
| src/command/mod.rs | 23 | `pub type CommandParserCell` | — |
| src/command/parser.rs | 14 | `pub struct ParsedCommand` | — |
| src/command/parser.rs | 43 | `pub enum CommandContext` | — |
| src/command/parser.rs | 69 | `pub struct CommandParser` | — |
| src/command/parser.rs | 74 | `pub const fn new` | — |
| src/command/parser.rs | 84 | `pub async fn parse_async` | — |
| src/command/parser.rs | 114 | `pub const fn tool_registry` | **sw-command-1** (low, KEEP deferred) |
| src/command/parser.rs | 120 | `fn tool_to_command_context` (private) | — |
| src/command/parser.rs | 139 | `pattern: tool.routing_regex.unwrap_or(tool.name)` | **sw-command-2** (low, KEEP) |
| src/command/parser.rs | 144-153 | `ToolSource::Plugin => CommandContext::Builtin { tool_name: tool.id }` | **sw-command-3** (low, KEEP deliberate) |
