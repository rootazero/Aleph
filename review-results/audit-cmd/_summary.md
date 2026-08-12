# audit-cmd — src/command/ — summary

**Branch:** `audit/command-components`
**Base:** commit `e80d17c96`
**Date:** 2026-08-11
**Modules in scope:** `src/command/` (236 LOC) + downstream consumers in `src/gateway/`.
**Not in scope (see "Open tickets" below):** `src/components/` — was
**cut 11 days ago** by commit `682e49adc` ("components: cut entire module —
dead scaffolding from removed EventHandler chain"). The historical audit
artefact lives at `review-results/components.md`.

## Lenses run (3 parallel subagents)

| Lens | File | Lines |
|---|---|---|
| Wiring / producer–consumer (seam) | `seam.md` | 399 |
| Logic / correctness / API contract | `logic.md` | 222 |
| Style / Rust idioms / clippy nits | `style.md` | 346 |

## Findings — verdict vs action

Cross-lens dedup (the same defect showed up under multiple lenses):

| ID | Severity | Title | Action |
|---|---|---|---|
| **SEAM-1 / LOG-4 / STYLE-7** | medium | `ResolvedCommandContext` mirror in `gateway/handlers/commands.rs` is dead — enum defined, `From` impl exists, two tests cover it, but the producer `command.execute` RPC never returns a `context` field. | **Open ticket** — out of `src/command/` scope; logged for a separate `handlers/commands.rs` audit. |
| **SEAM-2 / LOG-3** | high | Custom slash-command path is severed: `provider` hardcoded `None`, `system_prompt` and `pattern` produced and never read by the fast-path arm. `RoutingRuleConfig.provider` is dropped at `register_custom_commands` (no `routing_provider` on `UnifiedTool`). | **Partial**: `provider` field cut from `CommandContext::Custom` + JSON wire (commits 2+3). `system_prompt` and `pattern` left in place because the agent-loop routing layer (out of audit scope) may consume them; flagged for a `tool_metadata/` audit. |
| **SEAM-3** | medium | `CommandContext::Mcp::tool_name` produced, forwarded to JSON, never read by `slash_command.rs`. | **Done** (commits 2+3): field cut from both ends. |
| **SEAM-4** | low | `mod.rs` module header is stale — claims to "aggregate commands from multiple sources" and omits both `Native` and `Plugin`. | **Done** (commit 1): rewrote as `//!` doc, accurate inventory. |
| **SEAM-5 / LOG-9** | low (contract smell) | `source_type` and `context` disagree for `Plugin` (routed to `Builtin` with `tool.id` stored as `tool_name`). | **Partial** (commit 2): documented the dispatch-shape / source distinction on the variant. Splitting into a `Plugin` variant would be the structurally right fix but is a wire change; DECIDE for the owner. |
| **SEAM-6** | low (P5 only) | `CommandParser::tool_registry()` is a pass-through to `Arc<ToolCatalog>` — not a severed wire (3 live callers) but a Law-of-Demeter smell. | **Open ticket** — not a wiring defect; logged. |
| **STYLE-1** | low | `mod.rs` header uses `//` instead of `//!`. | **Done** (commit 1). |
| **STYLE-2** | low | `Builtin { tool_name }` doc is wrong for Plugin. | **Done** (commit 2). |
| **STYLE-3** | low | `command_name` doc implies literal typed word. | **Done** (commit 2). |
| **STYLE-4** | low (clippy `nursery::redundant_clone`) | `parse_async` redundantly clones `name` and `id` from a value about to be dropped. | **Done** (commit 2). |
| **STYLE-5** | low (clippy `pedantic::uninlined_format_args`) | Test panic uses `{:?}` instead of `{other:?}`. | **Done** (commit 2). |
| **STYLE-6** | low | Whole `src/command/` module could be `pub(crate)` (4 internal-only consumers). | **Deferred** — visibility change needs `cargo test --no-run` validation (a CUT can break `#[cfg(test)]` paths that `cargo check` never compiles). |
| **STYLE-8** | none | Checklist item 9 `Arc::new` const concern was a false alarm. | **No action.** |
| **LOG-1** | high | `register_skills` does not set `routing_system_prompt`, so `ParsedCommand::Skill::instructions` is `""` for every skill. Note: downstream `execute.rs:417` would inject that empty string into the prompt overlay. | **Open ticket** — fix is in `src/tool_metadata/registry/registration.rs`, requires deciding where the skill body actually lives (`SkillInfo::instructions`?). Logged. |
| **LOG-2** | high | `Skill::allowed_tools` carries *capability* labels (`{"skills", "memory"}`) but the consumer (`run_loop/inner.rs:249`) compares them to `UnifiedTool::name`. The whitelist probably silently removes every tool from a skill-triggered run. | **Open ticket** — needs census of `tool.name` vs `routing_capabilities`. Logged. |
| **LOG-5** | medium | `resolve_command` rebuilds args with `split_whitespace` + `join(" ")`, mangling multi-line and repeated-space commands. | **Open ticket** — fix is in `src/tool_metadata/registry/query.rs`, outside audit scope. |
| **LOG-6** | low-medium | `tool_id` JSON key means two different things (bare name vs. namespaced id) for `Builtin` vs `Plugin`. | **Deferred** — rename vs consolidate is a DECIDE; left as-is, doc-only mitigation. |
| **LOG-7** | low | `serialize_parsed_command` uses `.ok()` to swallow serialization failures, conflating them with the deliberate "skip the fast path" `None`. | **Open ticket** — fix is in `command_handler.rs`, needs coordination with whatever consumes the JSON. |
| **LOG-8** | low | `split_namespace_action` does not exactly mirror `build_command_tree`'s grouping (filter for non-empty action in split, none in tree). | **Open ticket** — single-source-of-truth fix in `handlers/commands.rs`. |

## Commits (this branch)

```
51047d0ab  command: drop dead 'provider'/'tool_name' from slash-command-mode JSON
d76bfae71  command: tighten parser docs, drop 2 redundant clones, cut dead fields
9efc95624  command: make mod.rs a //! module-level doc and update inventory
```

## Open tickets (not in this batch)

1. **`handlers/commands.rs`** (LOG-4 / SEAM-1): delete `ResolvedCommandContext`,
   its `From` impl, and the two serialization tests. Mirror is dead.
2. **`tool_metadata/registry/registration.rs`** (LOG-1): populate
   `routing_system_prompt` in `register_skills`; pick the right source
   (`SkillInfo::instructions`? `SkillInfo::body`? the loaded SKILL.md?).
3. **`tool_metadata/registry/registration.rs` + `tool_metadata/types/unified/`** (LOG-2):
   replace `routing_capabilities` with a real tool allowlist on `ToolSource::Skill`.
4. **`tool_metadata/registry/query.rs`** (LOG-5): keep `args` lossless for multi-line input.
5. **`tool_metadata/registry/registration.rs`** (LOG-3, deeper): plumb
   `RoutingRuleConfig.provider` (and `preferred_model`) onto `UnifiedTool`
   so `CommandContext::Custom::system_prompt` has a future home beyond
   the dead JSON field.
6. **`gateway/handlers/commands.rs`** (LOG-8): single-source `TOOL_NAMESPACES`
   grouping + `split_namespace_action` with a `namespace_of(name)` helper that
   also asserts no entry is a prefix of another.
7. **`src/command/`** (STYLE-6): if a `cargo test --no-run` pass confirms
   nothing outside `src/gateway/` references `alephcore::command`, narrow
   the module visibility to `pub(crate)`.

## What this audit did NOT do (per AGENTS.md §6)

- **Did not run `cargo check`** (per the user's instruction set for this batch).
  TODO before merge: `cargo check -p alephcore` once on this branch, and
  `cargo test -p alephcore --lib --no-run` if the `pub` → `pub(crate)`
  narrowing (STYLE-6) is ever attempted.
- **Did not audit `src/components/`** — that module no longer exists (commit
  `682e49adc`, 2026-08-01). Historical review lives at `review-results/components.md`.
- **Did not exhaustively read `src/routing/` or `src/providers/route_policy.rs`** —
  this is the gap behind SEAM-2's "system_prompt/pattern are dead" claim.
  A second matcher in the agent loop could change the verdict from
  CUT-DONE to CONNECT-PENDING.
- **Did not audit downstream Panel/CLI/webchat clients** of the
  `command.execute` RPC — relevant if SEAM-1's "delete the mirror"
  recommendation is taken, since a TS/WASM client might already be
  reading a `context` key by string and would not show up in a Rust
  grep.
