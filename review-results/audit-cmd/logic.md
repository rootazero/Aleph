# logic.md — src/command/

**Lens:** logic / correctness / API contract.
**Commit:** e80d17c9 · **Scope read:** `src/command/{mod,parser}.rs`, `gateway/inbound_router/command_handler.rs`,
`gateway/handlers/commands.rs`, `bin/aleph-server/server_init.rs:440-475`,
`tool_metadata/registry/{mod,query,registration}.rs`, `tool_metadata/types/unified/mod.rs`,
`gateway/execution_engine/run_loop/inner.rs:190-262`.
**Read-only.** No files modified.

---

## Per-checklist verdict

| # | Item | Verdict |
|---|------|---------|
| 1 | `/` alone | **OK.** `parse_async` trims then requires `starts_with('/')`; `resolve_command` (`query.rs:137-140`) takes `&trimmed[1..]` and returns `None` on empty. `"/"`, `"/ "`, `"/\t"` all reject. Byte-slice `[1..]` is safe (index 0 is the 1-byte `/`). The RPC face also survives: `handle_execute("/")` → `words` empty → `first_word = ""` → `is_namespace("")` false → `suggest_commands("")` short-circuits empty (`query.rs:253`) → clean "Unknown command" with `suggestions: []`. |
| 2 | Argument handling | **WARN → LOG-5.** Leading slash is correctly excluded (stripped before word split; args are `all_words[n..]`, never the command word). `/search` with no space → `arguments: None` → serializer substitutes `""`. But args are rebuilt with `join(" ")` from `split_whitespace()`, so **newlines, tabs and repeated spaces are destroyed**. For `Skill`/`Custom` commands the args are free-form prose fed to a model. |
| 3 | `Builtin`/`Native` collapse | **WARN → LOG-9, not a live bug.** Only two consumers of `CommandContext::Builtin` exist (`command_handler.rs:133,178` and the dead `From` impl at `commands.rs:392`); neither branches on Builtin-vs-Native, and `source_type` still carries the distinction to the RPC. The real contract smell is the *other* collapse: `Plugin` also lands in `Builtin`, so `source_type == Plugin` while `context == Builtin` — the two fields of the same struct disagree about the source. |
| 4 | `Skill.instructions` | **BUG at `src/command/parser.rs:124` (+ `registry/registration.rs:229-244`) → LOG-1.** Not merely a misnomer: `routing_system_prompt` has exactly **one** writer in the whole tree (`registration.rs:367`, inside `register_custom_commands`). `register_skills` never sets it ⇒ `instructions` is `""` for **every** skill, always. The doc comment on `serialize_parsed_command` says it exists because "skill instructions were silently dropped" — they still are, one layer up. |
| 5 | `Custom` provider | **BUG (upstream), premise partly false → LOG-3.** `UnifiedTool` has **no `routing_provider` field** (the `routing_*` set is `regex` / `system_prompt` / `capabilities` / `intent_type` / `strip_prefix` / `context_format`, `types/unified/mod.rs:147-176`), so the parser cannot read one. But the data *does* exist one level up: `RoutingRuleConfig.provider` is populated, and `register_custom_commands` (`registration.rs:357-368`) copies only `regex` + `system_prompt`, dropping `provider` **and** `preferred_model`. Net effect is exactly the hypothesised defect: a rule with `provider = "openai"` yields a fast-path payload with `"provider": null`. `tool.routing_regex.unwrap_or(&tool.name)` is fine — custom tools always have the regex set at `registration.rs:364`. |
| 6 | `Plugin` → `Builtin { tool_name: tool.id }` | **OK, intentional; one WARN.** Correct for the live consumer: `serialize_parsed_command` forwards it as `tool_id` and the direct-tool fast path resolves by canonical registry id. `is_continuation_driven_slash` receives a namespaced id for plugins and a bare name for builtins — harmless (a plugin can never be `/loop`/`/goal`). The WARN is that the field name lies for 2 of 5 sources, and the one consumer that would surface it *by name* to a client (`ResolvedCommandContext::Builtin`) is dead code (LOG-4). |
| 7 | `to_string(&value).ok()` | **WARN → LOG-7.** Cannot fail today (every leaf is `String` / `Vec<String>` / `Option<String>` — no non-string map keys, no `f64`). The defect is semantic: `None` is *also* the deliberate "continuation-driven builtin → skip the fast path" signal (`command_handler.rs:133-137`). A future payload that fails to serialise would silently take the same escape hatch instead of erroring. |
| 8 | `Builtin` arm passes `tool_name` as `tool_id` | **WARN → LOG-6.** `parsed.tool_id` is used by the `Custom` arm only. The `Builtin` arm emits `tool_id: tool_name`, which is a **bare name** (`session_new`) for Builtin/Native but a **canonical id** (`plugin:diag:ping`) for Plugin. One JSON key, two namespaces; it only works because the downstream resolver accepts both. `Mcp` and `Skill` drop `tool_id` entirely. |
| 9 | `first_word` edge cases | **OK.** `/@MyBot` → `split_once('@')` = `("", "bot")` → `first_word = ""` → `is_namespace("")` false → `suggest_commands("")` returns empty by its own guard → honest "Unknown command: /@MyBot", `suggestions: []`. `/session@` → `("session","")` → `"session"` → correct namespace listing. `/` → same empty path. No panic, no misclassification. Lowercasing is safe (all namespaces and canonical names are lowercase; `find_best_match` lowercases both sides too). |
| 10 | `split_namespace_action` | **OK on the cases asked; WARN on two others → LOG-8.** `"session"` → `strip_prefix("session")` = `Some("")` → `strip_prefix('_')` = `None` → `(None, None)` ✓. `"session__new"` → `Some("__new")` → `Some("_new")` → returns `("session", "_new")` — it **keeps** the extra underscore rather than losing it (the checklist's reading was off by one); no real tool has a double underscore, cosmetic at most. The two real gaps: (a) the doc claims it mirrors `build_command_tree` "exactly", but the tree's grouping (`commands.rs:141-143`) has no non-empty-action filter, so a tool named exactly `session_` groups under the namespace in the tree and returns `(None, None)` here; (b) both are **iteration-order dependent** — safe today only because no entry of `TOOL_NAMESPACES` is a prefix of another, and nothing pins that invariant. |
| 11 | Tests | **WARN → gaps below.** Assertions are tight where they exist, but the suite systematically avoids the fields that are broken. See "Test gaps" per finding and the summary table at the end. |

---

## Findings

### LOG-1 — Skill slash commands carry an always-empty `instructions`
- **Severity:** high
- **File(s):** `src/command/parser.rs:124`; producer side `src/tool_metadata/registry/registration.rs:229-244`; sink `src/gateway/inbound_router/command_handler.rs:150`
- **Evidence:**
  ```rust
  // parser.rs:124
  instructions: tool.routing_system_prompt.clone().unwrap_or_default(),
  ```
  `routing_system_prompt` is written at exactly one site in the tree — `registration.rs:367`, guarded by `if let Some(ref prompt) = rule.system_prompt` inside `register_custom_commands`. `register_skills` sets `display_name`/`icon`/`usage`/`routing_regex`/`routing_intent_type`/`routing_capabilities`/`routing_strip_prefix` and **not** `routing_system_prompt`. So the field is `None` for every `ToolSource::Skill`, and `unwrap_or_default()` turns the miss into `""` rather than a detectable absence.
- **Consequence:** the `"instructions"` key of the skill mode JSON is `""` on every skill slash command. `run_loop/inner.rs:234-239` already documents the *second* half of the same break ("`slash_skill_instructions` … the legacy gateway path here does not yet thread that string into the system prompt overlay"), so both ends of this wire are dead — and the empty string means even fixing the prompt overlay would inject nothing.
- **Fix sketch:**
  ```rust
  // registration.rs::register_skills — carry the skill body as the routing prompt
  .with_routing_system_prompt(skill.instructions.clone())   // or skill.body / loaded SKILL.md
  // parser.rs:124 — stop laundering "absent" into ""
  instructions: tool.routing_system_prompt.clone()?,        // or keep Option<String> on the variant
  ```
  Rename the variant field to `system_prompt` (matching `Custom`) or keep `instructions` and document that it *is* the routing system prompt — but pick one.
- **Test gap:** no test constructs a `ToolSource::Skill` at all. New: register a skill via `register_skills`, `parse_async("/myskill do a thing")`, assert `instructions` is non-empty and equals the registered text; plus a serializer test asserting the emitted JSON's `instructions` is non-empty.

### LOG-2 — Skill `allowed_tools` carries *capability* names, and the consumer treats them as *tool* names
- **Severity:** high (verified at both ends; the middle hop read by grep only — see caveat)
- **File(s):** `src/command/parser.rs:126`; producer `src/tool_metadata/registry/registration.rs:243`; consumer `src/gateway/execution_engine/run_loop/inner.rs:242-255`
- **Evidence:**
  ```rust
  // parser.rs:126
  allowed_tools: tool.routing_capabilities.clone(),
  // registration.rs:243 — every skill, identical literal
  .with_routing_capabilities(vec!["skills".to_string(), "memory".to_string()])
  // inner.rs:249-252
  if !skill_whitelist.is_empty() {
      allowed_tools.retain(|t| skill_whitelist.contains(t.name.as_str()));
  }
  ```
  The whitelist is `{"skills", "memory"}` — capability labels, not tool names. `retain` compares them against `UnifiedTool::name` (`session_new`, `memory_search`, `web_fetch`, …). The non-empty guard is satisfied, so the retain **does** run, and it plausibly keeps zero tools: a skill-triggered run would execute with an empty toolset while logging a confident `"Applied slash-skill allowed_tools restriction"`.
- **Caveat (do not skip before fixing):** I did not enumerate the registry to prove no tool is literally named `memory` or `skills`, and I read `execute.rs:412-436` (the hop that writes `slash_skill_allowed_tools` from the mode JSON) via grep, not in full. Confirm with: `parse_async` a real skill → serialize → run the execute hop → assert `allowed_tools.len()` after the retain. Either outcome is a bug — "0 tools" is a hard break, "2 tools" is a nonsense restriction.
- **Fix sketch:**
  ```rust
  // Give skills a real tool allowlist field; do NOT reuse routing_capabilities.
  // registration.rs: .with_routing_capabilities(skill.allowed_tools.clone())  // if SkillInfo has it
  // else parser.rs:126: allowed_tools: Vec::new(),   // honest empty => inner.rs skips the retain
  ```
  The cheap, safe first move is `Vec::new()` (restores "no restriction") plus a guard test; the correct move is plumbing `SkillInfo`'s declared tools through.
- **Test gap:** none exists. New: assert that every name in a skill's `allowed_tools` resolves to a registered tool name (a source-level or registry-level census — a whitelist that names nothing is indistinguishable from "deny all" at the point of use).

### LOG-3 — A custom command's configured `provider` is dropped, then hardcoded to `None`
- **Severity:** medium-high
- **File(s):** `src/command/parser.rs:130`; root cause `src/tool_metadata/registry/registration.rs:357-368`; sink `src/gateway/inbound_router/command_handler.rs:163`
- **Evidence:**
  ```rust
  // parser.rs:128-132
  ToolSource::Custom { .. } => CommandContext::Custom {
      system_prompt: tool.routing_system_prompt.clone(),
      provider: None, // Provider is resolved at routing time
      ...
  ```
  `UnifiedTool` has no `routing_provider` field, so the parser genuinely has nothing to read — but `register_custom_commands` had it and threw it away: it copies `rule.regex` and `rule.system_prompt` and silently discards `rule.provider` and `rule.preferred_model`. The comment "Provider is resolved at routing time" is unfalsifiable at this layer: the fast-path payload (`"provider": provider` → `null`) is the only carrier the slash path produces, and the fast path returns before ordinary routing.
- **Fix sketch:**
  ```rust
  // types/unified/mod.rs
  #[serde(skip_serializing_if = "Option::is_none")] pub routing_provider: Option<String>,
  // registration.rs (custom arm)
  if let Some(p) = &rule.provider { tool = tool.with_routing_provider(p.clone()); }
  // parser.rs:130
  provider: tool.routing_provider.clone(),
  ```
  If the comment is in fact true and some later stage re-resolves the provider, delete the field from `CommandContext::Custom` instead — an always-`None` field on a wire payload is worse than no field.
- **Test gap:** `parser.rs:153-171` builds a rule with `provider: Some("openai")` and asserts only `command_name` / `arguments` / `source_type`. The one test that touches the data never looks at it. New: assert `CommandContext::Custom.provider == Some("openai")`, and a serializer assertion that the emitted JSON's `provider` is not null.

### LOG-4 — `ResolvedCommandContext` and its `From<CommandContext>` impl are unreachable; the documented `context` is never returned
- **Severity:** medium
- **File(s):** `src/gateway/handlers/commands.rs:365-411` (type + impl), `commands.rs:490-507` (the response that omits it)
- **Evidence:** `handle_execute` builds only `ResolvedCommandInfo` and emits `{"resolved": true, "command": info}`. Grep for `ResolvedCommandContext` yields the definition, the `From` impl, and two `#[test]` uses (`commands.rs:994`, `:1001`) — **zero production call sites**. Every skill/mcp/custom distinction the enum encodes (and the `From` impl's careful `..` destructuring) is computed nowhere and shipped nowhere.
- **Consequence:** classic severed wire, and the tests keep it green: `test_resolved_command_context_serialization` asserts a struct nobody constructs. Panel/CLI clients get `internal_id` + `source_type` and must re-derive the context they were promised.
- **Fix sketch:** decide, don't defer. Either
  ```rust
  "command": info, "context": ResolvedCommandContext::from(parsed.context),
  ```
  (and document it in the `handle_execute` example JSON), or delete the enum, the `From` impl and `test_resolved_command_context_serialization`. Per R10, zero-consumer abstractions get CUT unless a shipped surface can be named.
- **Test gap:** the existing test actively conceals the break. New (if connecting): assert `result["command"]`… plus `result["context"]["type"] == "skill"` for a skill input.

### LOG-5 — Argument reconstruction normalises whitespace, mangling multi-line command bodies
- **Severity:** medium
- **File(s):** `src/tool_metadata/registry/query.rs:143,169-174` (produces `arguments`), surfaced at `src/command/parser.rs:100`
- **Evidence:**
  ```rust
  let all_words: Vec<&str> = without_slash.split_whitespace().collect();
  ...
  let arguments = if remaining.is_empty() { None } else { Some(remaining.join(" ")) };
  ```
  `/myskill fix this:\n  - a\n  - b` arrives as `fix this: - a - b`. Same for `/translate` bodies and any custom command whose argument is prose or code.
- **Fix sketch:** keep the word split for command-path matching only, and slice the original input for the tail:
  ```rust
  // after choosing depth n, find the byte offset just past the n-th word in `without_slash`
  let tail = without_slash[offset_after_word(n)..].trim_start();
  let arguments = (!tail.is_empty()).then(|| tail.to_string());
  ```
- **Test gap:** no test passes multi-word-with-structure args. New: `parse_async("/search line1\n  line2")` asserts the newline and indent survive.

### LOG-6 — The `tool_id` key of the fast-path JSON means two different things
- **Severity:** low-medium
- **File(s):** `src/gateway/inbound_router/command_handler.rs:178-183`; feeder `src/command/parser.rs:115-116,139`
- **Evidence:** `CommandContext::Builtin { tool_name }` is `tool.name` for Builtin/Native and `tool.id` for Plugin; the serializer emits both under `"tool_id"`. Meanwhile `parsed.tool_id` — the field whose doc comment (`parser.rs:15-23`) says it exists precisely so downstream stops reconstructing ids — is used by the `Custom` arm only, and ignored by `Builtin`, `Mcp` and `Skill`.
- **Fix sketch:** carry both explicitly and let the resolver pick:
  ```rust
  CommandContext::Builtin { tool_name } => json!({
      "type": "direct_tool", "tool_id": parsed.tool_id, "tool_name": tool_name, ...
  }),
  ```
  then drop `tool_name: tool.id` from the Plugin arm of `tool_to_command_context` (it becomes redundant once `tool_id` is authoritative).
- **Test gap:** `test_parse_async_plugin_routes_to_direct_tool` pins the plugin shape but there is **no serializer test at all** for `serialize_parsed_command`. New: one test per variant asserting the exact emitted JSON keys.

### LOG-7 — `.ok()` conflates a serialisation failure with the deliberate "skip the fast path" signal
- **Severity:** low
- **File(s):** `src/gateway/inbound_router/command_handler.rs:185` vs `:133-137`
- **Evidence:** `serde_json::to_string(&value).ok()` returns the same `None` that the continuation-driven-builtin early return uses to mean "route this through the full agent loop on purpose".
- **Fix sketch:**
  ```rust
  match serde_json::to_string(&value) {
      Ok(s) => Some(s),
      Err(e) => { tracing::error!(?e, "slash mode JSON serialize failed"); None }
  }
  ```
  (Or return `Option<Option<String>>` / a two-variant enum if the distinction ever needs to reach the caller.)
- **Test gap:** none needed beyond the log; the value shapes make failure unreachable today. The log is the guard.

### LOG-8 — `split_namespace_action` does not actually mirror `build_command_tree`, and both depend on unpinned `TOOL_NAMESPACES` ordering
- **Severity:** low
- **File(s):** `src/gateway/handlers/commands.rs:99-110` vs `:140-149`
- **Evidence:** the split filters `!action.is_empty()`; the tree grouping (`tool.name.get(ns.len()..ns.len()+1) == Some("_")`) does not. A tool named `session_` is a namespace child in `commands.list` and a standalone command in `command.execute`. Separately, both scan `TOOL_NAMESPACES` in declaration order and take the first prefix hit — correct today only because no entry is a prefix of another, an invariant nothing enforces.
- **Fix sketch:**
  ```rust
  // one shared helper, used by both, longest-match:
  fn namespace_of(name: &str) -> Option<(&'static str, &str)> {
      TOOL_NAMESPACES.iter().filter_map(|ns| name.strip_prefix(ns)
          .and_then(|r| r.strip_prefix('_')).filter(|a| !a.is_empty()).map(|a| (*ns, a)))
          .max_by_key(|(ns, _)| ns.len())
  }
  ```
- **Test gap:** `test_split_namespace_action` covers the good cases only. New: (a) a test asserting no entry of `TOOL_NAMESPACES` is a prefix of another; (b) a test asserting the split and the tree agree on the same name set.

### LOG-9 — `source_type` and `context` disagree for `Plugin`
- **Severity:** low (contract smell, no live consumer confused)
- **File(s):** `src/command/parser.rs:93-94,133-140`
- **Evidence:** `ToolSourceType::from(&source)` yields `Plugin`, while `tool_to_command_context` yields `CommandContext::Builtin`. A reader — or a future `match` on `parsed.context` that assumes it agrees with `parsed.source_type` — will get it wrong; the RPC face already reports `source_type: "plugin"` alongside a Builtin-shaped context.
- **Fix sketch:** either add `CommandContext::Plugin { tool_id }` (and have the serializer's direct-tool arm cover Builtin | Native | Plugin), or document the collapse on the enum:
  ```rust
  /// NOTE: `Builtin` is the *dispatch shape* (direct tool call), not the source.
  /// Builtin, Native and Plugin all land here; read `source_type` for the source.
  ```
- **Test gap:** `test_parse_async_plugin_routes_to_direct_tool` asserts the collapse but reads as if it were validating intent. Add the doc so the test's meaning is legible.

---

## Test gaps (summary)

| Missing coverage | Would catch |
|---|---|
| Any test constructing `ToolSource::Skill` end-to-end | LOG-1, LOG-2 |
| Any test of `serialize_parsed_command` (all four variants) | LOG-1, LOG-3, LOG-6 |
| Assertion that `Custom.provider` survives registration | LOG-3 |
| `parse_async` with `/`, `/ `, `/@bot`, `/cmd` (no args), multi-line args | items 1/2/9, LOG-5 |
| A `command.execute` test asserting the response's `context` key | LOG-4 |
| `TOOL_NAMESPACES` prefix-freedom + split/tree agreement | LOG-8 |
| Plugin **upstream**: does the direct-tool fast path actually resolve `plugin:<id>:<name>`? | the premise `test_parse_async_plugin_routes_to_direct_tool` asserts but never exercises |

Note on the two existing parser tests that *look* like coverage: `test_parse_async_found` sets `provider: Some("openai")` and `system_prompt: Some(...)` and asserts neither survived; `test_resolved_command_context_serialization` exercises a type with no production constructor. Both are green today and would stay green through LOG-3 and LOG-4 respectively.

---

## Recommended fixes priority

**Commit 1 — `command: stop laundering absent skill/custom data into empty values` (cheapest, no behaviour risk beyond the intended one)**
1. LOG-7 — log the serialisation failure instead of `.ok()`. Pure diagnostics.
2. LOG-9 — doc comment on `CommandContext::Builtin` naming the collapse. Zero code.
3. LOG-2 (mitigation only) — `allowed_tools: Vec::new()` in the Skill arm, restoring "no restriction", with the census test. One line; converts a possible total-toolset wipe into today's documented-legacy behaviour.

**Commit 2 — `command: cover the parser's untested edges` (tests only, no production change)**
4. Add the `serialize_parsed_command` variant tests, the `parse_async` edge tests, and the `TOOL_NAMESPACES` prefix-freedom test. Land these **before** commit 3 so the data-carrying fixes have something that goes red first.

**Commit 3 — `tool_metadata: carry provider + skill prompt through registration` (touches the registry contract)**
5. LOG-3 — add `UnifiedTool::routing_provider`, populate from `RoutingRuleConfig.provider`, read it in the parser. New public field on a `#[non_exhaustive] Serialize/Deserialize` struct — check the Panel-side DTO for a matching `#[serde(default)]` before landing (§0: a wire contract with a shape on each side cancels itself out).
6. LOG-1 — populate `routing_system_prompt` in `register_skills`; decide `instructions` vs `system_prompt` naming.
7. LOG-2 (real fix) — plumb the skill's declared tool list, replacing the mitigation from commit 1.

**Commit 4 — `command: make the argument tail lossless` (behaviour change on a hot path)**
8. LOG-5 — byte-offset tail slicing in `resolve_command`. Affects every slash command's args; land alone so a regression is bisectable.

**Commit 5 — `gateway: connect or cut ResolvedCommandContext` (needs a human decision)**
9. LOG-4 — connect (emit `context` and document it) or cut (delete enum + `From` + test). Requires knowing whether any Panel/CLI client is waiting on the documented shape.
10. LOG-6 / LOG-8 — the `tool_id` disambiguation and the shared `namespace_of` helper; both are consolidations that are cheap *after* the LOG-4 decision fixes what the RPC actually returns.
