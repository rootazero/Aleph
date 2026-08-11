# seam.md — src/command/

**Lens:** wiring / producer–consumer (severed-wire seam view)
**Scope:** `src/command/mod.rs` (15 LOC) + `src/command/parser.rs` (221 LOC)
**Base:** commit `e80d17c9`, branch `audit/command-components`
**Method:** read-before-write. Every "no consumer" claim below is backed by a
crate-wide `Grep` over `src/`, `interfaces/`, `shared/` for the symbol name.

> ⚠️ **Tooling note for whoever re-runs this audit:** in this checkout
> `grep -n "pat" <single-file>` silently returns nothing (CRLF checkout /
> shell interaction), while `grep -rn` and the `Grep` tool work. Two of my
> early passes came back empty and looked like "no consumers" when the
> symbol was in fact present. Every negative finding here was re-confirmed
> with the `Grep` tool, not with `grep -n`. A `grep -n` that returns empty
> on this tree is **not** evidence of absence.

---

## Surface area

Everything `src/command/` exports, and every live consumer found.

| Symbol | Defined | Consumers (file:line) | Verdict |
|---|---|---|---|
| `mod parser` (private) | `mod.rs:13` | re-export only | ok |
| `pub use CommandContext` | `mod.rs:15` | `gateway/handlers/commands.rs:13` (import), `:389–409` (`From` impl); `gateway/inbound_router/command_handler.rs:123,133,141,155,168,178` | ok (but see SEAM-1) |
| `pub use CommandParser` | `mod.rs:15` | `gateway/handlers/commands.rs:13,462,483`; `gateway/inbound_router/command_handler.rs:220`; `gateway/inbound_router/mod.rs:894,924`; `bin/aleph-server/.../agent_init/tool_catalog_init.rs:30`; `bin/aleph-server/server_init.rs:453,455` | ok |
| `pub use ParsedCommand` | `mod.rs:15` | `command_handler.rs:122` (`serialize_parsed_command` param); `handlers/commands.rs:483` (binding) | ok |
| `ParsedCommand::source_type` | `parser.rs:11` | `handlers/commands.rs:498` → `source_type_to_string` | **1 consumer** |
| `ParsedCommand::command_name` | `parser.rs:13` | `handlers/commands.rs:488` (`split_namespace_action`); `server_init.rs:461` (log only) | ok |
| `ParsedCommand::tool_id` | `parser.rs:25` | `handlers/commands.rs:497` (`internal_id`); `command_handler.rs:161` (Custom mode JSON — **but never read**, SEAM-2) | partially severed |
| `ParsedCommand::arguments` | `parser.rs:27` | `command_handler.rs:139`; `handlers/commands.rs:493` | ok |
| `ParsedCommand::context` | `parser.rs:29` | `command_handler.rs:133,140` (only real consumer); `handlers/commands.rs:390` (`From` impl — **dead**, SEAM-1) | ok via one path |
| `CommandContext::Builtin{tool_name}` | `parser.rs:35` | `command_handler.rs:133,178` → `direct_tool` mode | ok |
| `CommandContext::Mcp{server_name,tool_name}` | `parser.rs:40` | `command_handler.rs:168` → `mcp` mode → `slash_command.rs:217` reads `server_name` for a log/reason string only | weak |
| `CommandContext::Skill{…}` | `parser.rs:47` | `command_handler.rs:141` → `skill` mode → `execute.rs:417–441` (`instructions`, `allowed_tools`), `slash_command.rs:180,189` (`display_name`, `skill_id`) | ok — fully wired |
| `CommandContext::Custom{system_prompt,provider,pattern}` | `parser.rs:57` | `command_handler.rs:155` → `custom` mode → `slash_command.rs:223` **reads no field** | **SEVERED** (SEAM-2) |
| `CommandParser::new` | `parser.rs:76` | `tool_catalog_init.rs:30` (production); `parser.rs` tests | ok |
| `CommandParser::parse_async` | `parser.rs:85` | `handlers/commands.rs:483`; `inbound_router/mod.rs:894` (via caller); `server_init.rs:455` | ok |
| `CommandParser::tool_registry` | `parser.rs:104` | `command_handler.rs:229,483`; `inbound_router/mod.rs:924` | **used** (see SEAM-6 for the encapsulation smell) |
| `tool_to_command_context` (private) | `parser.rs:113` | `parser.rs:92` | ok |

No `[UNUSED]` `pub` symbol in `src/command/` itself. The severed wires are all
one level out: fields that are produced here, serialized downstream, and then
dropped on the floor by the final consumer.

---

## Per-variant reachability

### `ToolSource` → `CommandContext` (`tool_to_command_context`, parser.rs:113–143)

All six `ToolSource` variants are covered — the `match` is exhaustive with no
wildcard arm, so a new variant is a compile error. Good.

| `ToolSource` | Maps to | Verdict |
|---|---|---|
| `Native` | `Builtin{tool_name: tool.name}` | producer present, downstream reads it (`direct_tool` fast path) |
| `Builtin` | `Builtin{tool_name: tool.name}` | producer present, downstream reads it |
| `Mcp{server}` | `Mcp{server_name, tool_name: Some}` | producer present; downstream reads `server_name` **only to build a human reason string**; `tool_name` is serialized and never read |
| `Skill{id}` | `Skill{…}` | producer present, downstream reads **all four** fields |
| `Custom{rule_index}` | `Custom{system_prompt, provider: None, pattern}` | **producer exists, no consumer** — `rule_index` is discarded, `provider` is hardcoded `None`, and the `custom` fast-path arm reads nothing |
| `Plugin{plugin_id}` | `Builtin{tool_name: tool.id}` | producer present, downstream reads it correctly; **shape/tag disagreement** — see SEAM-5 |

### `CommandContext` variants — downstream reachability

| Variant | `serialize_parsed_command` | Fast-path consumer | `ResolvedCommandContext` | Verdict |
|---|---|---|---|---|
| `Builtin` | ✅ `direct_tool` | ✅ `slash_command.rs:132–177` executes `tool_id` | ⚠️ dead | **wired** |
| `Skill` | ✅ `skill` | ✅ `execute.rs:417–441` + `slash_command.rs:179–200` | ⚠️ dead | **wired** |
| `Mcp` | ✅ `mcp` | ⚠️ `slash_command.rs:202–221` — `server_name` used for a Fallthrough reason string; `tool_name` unread | ⚠️ dead | **half-wired** |
| `Custom` | ✅ `custom` | ❌ `slash_command.rs:223–228` — **zero field reads**, immediate `Fallthrough` | ⚠️ dead | **severed** |

---

## Findings

### SEAM-1
- **ID:** SEAM-1
- **Severity:** medium
- **Title:** `ResolvedCommandContext` + its `From<CommandContext>` impl have zero production consumers — the `command.execute` response never carries a `context` field
- **File(s):** `src/gateway/handlers/commands.rs:366–411` (producer), `src/gateway/handlers/commands.rs:488–505` (the handler that would consume it), `src/command/parser.rs:29` (upstream field)
- **Evidence:**

  Producer side — a fully-built, `Serialize`-derived, externally-tagged enum with a total conversion from `CommandContext`:
  ```rust
  // commands.rs:366
  #[derive(Debug, Clone, Serialize)]
  #[serde(tag = "type")]
  pub enum ResolvedCommandContext { Builtin{..}, Mcp{..}, Skill{..}, Custom{..} }

  // commands.rs:389
  impl From<CommandContext> for ResolvedCommandContext { … }
  ```
  Consumer side — `handle_execute` is the *only* RPC that calls `parse_async`,
  and the struct it returns has no `context` field at all:
  ```rust
  // commands.rs:488
  let info = ResolvedCommandInfo {
      namespace, action,
      args: parsed.arguments,
      internal_id: parsed.tool_id,
      source_type: source_type_to_string(parsed.source_type),
  };   // <- parsed.context is never touched
  ```
  The handler's own doc example confirms the wire shape omits it
  (`commands.rs:448`): `{"resolved":true,"command":{"namespace":…,"action":…,"args":…,"internal_id":…,"source_type":…}}`.

  Crate-wide `Grep` for `ResolvedCommandContext` over `src/`:
  `commands.rs:367` (def), `:389` (From impl), `:994` and `:1001` (**tests only**).
  No third file. No `interfaces/` hit.
- **Classification:** **CUT** (or DECIDE, if the Panel wants the data)
- **Justification:** Read-before-write. Both ends are complete and the type is
  even test-covered (`:994`, `:1001`) — which is exactly why dead-code lints
  can't see it: the tests keep it alive. The `From` impl is *lossy by
  construction* (`Custom{pattern, ..}` drops `system_prompt`+`provider`;
  `Skill{…, ..}` drops `instructions`+`allowed_tools`), which reads like it
  was designed for a client-facing summary that was never plumbed into
  `ResolvedCommandInfo`. Two honest options: (a) CUT the enum, its `From`
  impl, and the two tests — the RPC's contract as documented at `:448` does
  not include it; or (b) CONNECT by adding `context: ResolvedCommandContext`
  to `ResolvedCommandInfo`, which is a **wire-format addition** and per
  CLAUDE.md §0 ("解析只能证明超集") needs a key-set-equality reconciliation
  test on the client side before shipping. Default to CUT: no client was
  found asking for it. Note the interaction with SEAM-5 — if you CONNECT
  instead, you immediately ship the Plugin mislabeling bug to clients.

### SEAM-2
- **ID:** SEAM-2
- **Severity:** high
- **Title:** The whole `Custom` slash-command path is severed — `system_prompt`, `provider`, `pattern`, and `tool_id` are serialized into the mode JSON and never read by anyone; the parser's "Provider is resolved at routing time" comment names a resolver that does not exist
- **File(s):** `src/command/parser.rs:128–132` (producer + the promise), `src/gateway/inbound_router/command_handler.rs:155–167` (serializer), `src/gateway/execution_engine/slash_command.rs:223–228` (consumer), `src/tool_metadata/registry/registration.rs:364–367` (where `rule.provider` is dropped)
- **Evidence:**

  Producer, with the promise in a comment:
  ```rust
  // parser.rs:128
  ToolSource::Custom { .. } => CommandContext::Custom {
      system_prompt: tool.routing_system_prompt.clone(),
      provider: None, // Provider is resolved at routing time
      pattern: tool.routing_regex.as_ref().unwrap_or(&tool.name).clone(),
  },
  ```
  Serializer faithfully emits all four keys:
  ```rust
  // command_handler.rs:159
  CommandContext::Custom { system_prompt, provider, pattern } => serde_json::json!({
      "type": "custom", "tool_id": parsed.tool_id,
      "system_prompt": system_prompt, "provider": provider,   // <- always null
      "pattern": pattern, "args": args, "source": "custom",
  }),
  ```
  Consumer reads **nothing**:
  ```rust
  // slash_command.rs:223
  "custom" => {
      // Custom commands need LLM with a custom system prompt — fall through
      Err(ExecutionError::Fallthrough { reason: "custom command".to_string() })
  }
  ```
  Contrast with the `skill` arm, which *is* wired: `execute.rs:417–441`
  pre-extracts `instructions` → `slash_skill_instructions` and
  `allowed_tools` → `slash_skill_allowed_tools` into request metadata before
  falling through. **There is no `slash_custom_system_prompt` equivalent.**
  `Grep` for `slash_custom|custom_system_prompt` over `src/` → zero hits.

  And the promise is severed one layer further up, at registration:
  ```rust
  // registration.rs:364 — registers a [[rules]] entry as a Custom tool
  .with_routing_regex(rule.regex.clone());
  if let Some(prompt) = … { tool = tool.with_routing_system_prompt(prompt.clone()); }
  // rule.provider is never copied onto the UnifiedTool
  ```
  `UnifiedTool` (`tool_metadata/types/unified/mod.rs:151,155`) has
  `routing_regex` and `routing_system_prompt` but **no provider field**.
  Crate-wide `Grep` for `RoutingRuleConfig` consumers: `register_custom_commands`
  (drops provider), the `routing_rules.*` CRUD RPCs
  (`handlers/routing_rules.rs:49,84,146,230` — read/write config only),
  `config/validate.rs:198` (validation only), and `query.rs:77` (rebuilds a
  rule *from* a tool, `provider` absent). **No runtime matcher re-applies a
  rule's provider or system_prompt during agent execution.**
- **Classification:** **DECIDE** (leaning CONNECT for `system_prompt`, CUT for `provider`)
- **Justification:** This is two severed wires with different right answers, so
  it must not be resolved as one edit.
  - `provider`: the field is hardcoded `None` at the producer, and the named
    resolver ("routing time") does not exist anywhere I could find — `rule.provider`
    dies at `registration.rs:364`. This matches CLAUDE.md §0's *"a comment that
    hides the bug is its only search hit"*. Either CUT the field from
    `CommandContext::Custom` and the mode JSON (it can only ever serialize as
    `null`), or CONNECT it properly: carry `provider` onto `UnifiedTool`, then
    honor it at `providers/route_policy.rs`. **Do not just delete the comment** —
    a user who writes `provider = "openai"` on a `^/foo` rule today gets silence.
  - `system_prompt`: this one has a real user-visible consequence. A
    `[[rules]]` entry with `^/translate` + `system_prompt` resolves, produces
    the mode JSON, falls through to the agent loop with the raw
    `/translate …` text, and the configured system prompt is never applied.
    The `skill` arm shows exactly the shape of the fix (`execute.rs:417`).
  - Flag for the human: I did **not** run the code, and I did not exhaustively
    read `src/routing/` or `src/providers/route_policy.rs` — this lens is
    static. The claim I stand behind is narrow and grep-backed: *no consumer
    reads the `system_prompt`/`provider` keys out of the `SLASH_COMMAND_MODE_KEY`
    JSON, and `rule.provider` is not copied onto the registered tool.* If a
    second, independent regex matcher re-applies rules inside the agent loop,
    that would be a **second source of truth** for the same fact and is itself
    worth a finding.

### SEAM-3
- **ID:** SEAM-3
- **Severity:** medium
- **Title:** `CommandContext::Mcp::tool_name` is produced, serialized, and never read — the MCP fast-path arm uses only `server_name`, and only to build a log string
- **File(s):** `src/command/parser.rs:118–121`, `src/gateway/inbound_router/command_handler.rs:168–177`, `src/gateway/execution_engine/slash_command.rs:202–221`
- **Evidence:**
  ```rust
  // parser.rs:118 — producer always fills it
  ToolSource::Mcp { server } => CommandContext::Mcp {
      server_name: server.clone(), tool_name: Some(tool.name.clone()),
  },
  // command_handler.rs:171 — serialized
  "type": "mcp", "server_name": server_name, "tool_name": tool_name, …
  // slash_command.rs:217 — the only consumer
  let server = mode["server_name"].as_str().unwrap_or("mcp");
  Err(ExecutionError::Fallthrough { reason: format!("mcp command '{server}'") })
  ```
  `mode["tool_name"]` appears nowhere in `slash_command.rs`.
  Note also that `tool_name` is `Option<String>` at the type level but the
  single producer can only ever emit `Some(_)` — the `None` case has no
  producer, so every consumer's `None` handling is unreachable.
- **Classification:** **DECIDE**
- **Justification:** Unlike SEAM-2 this may be intentional: an MCP slash
  command genuinely needs the LLM, and `ParsedCommand::tool_id` already
  carries the canonical `mcp:server:tool` id that a direct-execution path
  would need. Two coherent end states: (a) CUT `tool_name` from the variant
  and the JSON (the `server_name` reason string is all the fallthrough needs),
  or (b) CONNECT — execute MCP slash commands directly via `tool_id` the same
  way `direct_tool` does, which is what the field's presence implies was
  intended. Ask the owner which. If (a), also collapse `Option<String>` →
  nothing rather than leaving a nullable field with one producer.

### SEAM-4
- **ID:** SEAM-4
- **Severity:** low
- **Title:** `src/command/mod.rs` header describes a module that no longer exists — it claims to be a "unified command registry" that "aggregates commands from multiple sources", and its source list omits Plugin
- **File(s):** `src/command/mod.rs:1–11`
- **Evidence:**
  ```rust
  // mod.rs:3
  // This module provides a unified command registry for Aleph's command mode.
  // It aggregates commands from multiple sources:
  // - Builtin commands … - MCP tools … - User prompts … - Skills …
  ```
  versus `parser.rs:1–3`: *"Unified Slash Command Parser — **Delegates all
  command resolution to `ToolCatalog`**."* The module aggregates nothing; it
  holds one parser that asks `ToolCatalog::resolve_command`. The registry is
  `src/tool_metadata/`. The source list is also stale — `ToolSource` has six
  variants (`Native` and `Plugin` are both missing from the comment), and
  `Plugin` is precisely the variant with the subtle routing behavior
  (SEAM-5), so its absence from the module header is the least helpful place
  for the list to be wrong.
- **Classification:** **CUT** (the stale prose)
- **Justification:** Per CLAUDE.md §0, *"同一事实的两份表述，只改一份就是静默说谎"* —
  and the comment is the lying half here. The last two lines of the header
  (the `commands.list` / `gateway::handlers::commands` pointer) are accurate
  and worth keeping. Replace the first nine lines with one sentence pointing
  at `ToolCatalog`. Zero behavior risk.

### SEAM-5
- **ID:** SEAM-5
- **Severity:** medium
- **Title:** `Plugin` is routed into `CommandContext::Builtin` carrying a namespaced registry id, so `source_type` and `context` disagree — every consumer that dispatches on *shape* is correct, and every consumer that would dispatch on *tag* is wrong
- **File(s):** `src/command/parser.rs:133–142`, `src/gateway/handlers/commands.rs:392`, `src/gateway/inbound_router/command_handler.rs:133–137,178–183`
- **Evidence:**
  ```rust
  // parser.rs:133
  ToolSource::Plugin { .. } => CommandContext::Builtin {
      // … Routing them as `Mcp` mangled the id into `mcp__plugin:<id>_<name>` …
      tool_name: tool.id.clone(),   // = "plugin:diagnostics:ping", NOT a bare name
  },
  ```
  So a `ParsedCommand` for a plugin has `source_type == ToolSourceType::Plugin`
  but `context == Builtin{tool_name: "plugin:diagnostics:ping"}`. The
  parser's own test (`parser.rs:213–219`) asserts exactly this.

  Consumers, audited one by one:
  1. `command_handler.rs:178` → `{"type":"direct_tool","tool_id": tool_name}` →
     `slash_command.rs:249,289` → `execute_tool("plugin:diagnostics:ping")`.
     **Correct** — this is the documented fix and the reason the shape was chosen.
  2. `command_handler.rs:133` → `is_continuation_driven_slash(tool_name)`.
     Receives the namespaced id; the predicate compares against bare names
     (`slash_command.rs:38`), so a plugin can never match. **Correct by
     accident, but load-bearing** — a plugin exposing a `loop` tool must not
     be treated as the builtin `/loop`. Worth a comment; it currently reads
     like an oversight.
  3. `slash_command.rs:434` `build_tool_arguments(tool_id, …)` matches bare
     names, plugin ids fall to the default arm. **Correct.**
  4. `handlers/commands.rs:392` `CommandContext::Builtin{tool_name} =>
     Self::Builtin{tool_name}` → would serialize to the client as
     `{"type":"builtin","tool_name":"plugin:diagnostics:ping"}` **while the
     sibling `source_type` field says `"plugin"`**. This is the mis-routing
     the checklist anticipated — it is currently **latent only because
     SEAM-1 makes that code unreachable**.
- **Classification:** **DECIDE**
- **Justification:** I am explicitly *not* recommending "give Plugin its own
  variant" as an obvious fix. Read-before-write: the current shape exists
  because of a fixed production bug (the comment at `parser.rs:135–139`
  documents `mcp__plugin:<id>_<name>` never matching a registered tool), and
  it is pinned by a test at `parser.rs:200–220`. Splitting the variant means
  touching the one path that demonstrably works. The real defect is narrower:
  **`CommandContext::Builtin::tool_name` has two different meanings depending
  on `source_type`** (bare name vs. namespaced registry id), and nothing in
  the type says so. Minimum viable fix: rename the field to `tool_id` (it is
  a registry id in the `Plugin` case and a resolvable id in the others) and
  document the invariant on the variant. Then, whoever resolves SEAM-1 must
  make `ResolvedCommandContext` dispatch on `source_type`, not on shape.
  Flagging as DECIDE because "rename vs. split variant" is an owner call.

### SEAM-6
- **ID:** SEAM-6
- **Severity:** low
- **Title:** `CommandParser::tool_registry()` is wired (3 live callers) but hands out the whole `Arc<ToolCatalog>`, making the parser a pass-through handle for unrelated catalog work
- **File(s):** `src/command/parser.rs:102–106`; callers `src/gateway/inbound_router/command_handler.rs:229,483`, `src/gateway/inbound_router/mod.rs:924`
- **Evidence:**
  ```rust
  // parser.rs:104
  pub const fn tool_registry(&self) -> &Arc<ToolCatalog> { &self.tool_registry }
  ```
  Callers use it for `suggest_commands(unknown_cmd, 3)` (`:230`),
  `render_command_help(parser.tool_registry(), None)` (`:483`), and a registry
  read at `inbound_router/mod.rs:924`. None of these are *parsing*.
- **Classification:** **CONNECT-as-is** (i.e. no seam — record only)
- **Justification:** Explicitly **not** a severed wire — checklist item 8
  asked whether it is used, and it is, at three live sites. Recording it only
  because it answers the second half of that question: it does expose the
  full catalog, so `CommandParser` is doubling as a `ToolCatalog` locator for
  callers that already had one available. That is a P5 (Law of Demeter)
  observation for the style lens, not a wiring defect. No action recommended
  from this lens.

---

## Cross-refs

- **SEAM-1 ↔ dead-code lens.** `ResolvedCommandContext` is the textbook
  case the `severed-wire-audit` skill describes: dead-code lints cannot flag
  it because `commands.rs:994,1001` construct it in tests. Whichever lens
  reports it, the *tests must be deleted with it* — leaving them turns the
  next audit's "it has test coverage" into evidence of life.
- **SEAM-2 ↔ logic lens.** The `provider: None` hardcode is a wiring defect
  from this lens, but "does a `[[rules]]` provider override ever apply?" is a
  routing-logic question spanning `src/routing/` and
  `src/providers/route_policy.rs`, which I deliberately did not enter. If the
  logic lens finds a *second* rule matcher in the agent loop, that changes
  SEAM-2's classification from CUT to "two sources of truth."
- **SEAM-2 ↔ config/UX.** `routing_rules.create` (`handlers/routing_rules.rs:146`)
  accepts and persists a `provider` field that has no runtime effect on the
  slash path. That is a *"looks settable, never has an effect"* config surface
  — CLAUDE.md §0's `workspace.get` over-send finding, same shape.
- **SEAM-5 ↔ SEAM-1.** These two must be resolved in a known order. If
  SEAM-1 is resolved as CONNECT before SEAM-5 is decided, the Plugin
  `{"type":"builtin","tool_name":"plugin:…"}` mislabeling ships to clients on
  the same commit. **Decide SEAM-5 first.**
- **SEAM-3 ↔ SEAM-2.** Both are "the fast-path arm reads nothing it was
  handed." Worth one decision about what the `SLASH_COMMAND_MODE_KEY`
  envelope is *for*: an execution directive (then Custom/Mcp should carry
  only what's read) or a resolution record (then the consumers should read
  it). Today it is half of each, and only the `Skill` arm honors the second
  reading.
- **Guard suggestion (for whoever does the CONNECT work).** The
  producer/consumer pair here is a JSON envelope across a module boundary,
  which is exactly the shape CLAUDE.md §0 warns produces green-but-blind
  tests. A regression guard should assert **key-set equality** between what
  `serialize_parsed_command` emits per variant and what the fast path reads,
  with the expected set *derived from the consumer*, not written as a literal.

---

## What this audit did NOT cover

Stated explicitly, per the "state the negative" rule:

- **Not run, not compiled.** Static reading only; no `cargo`, no tests, no
  branch, no source edits, per the task constraints.
- **Did not enter `src/routing/` or `src/providers/route_policy.rs`** — this
  is the one gap that could change a verdict (SEAM-2). My claim there is
  grep-scoped and stated as such.
- **Did not audit `ToolCatalog::resolve_command`** (`src/tool_metadata/registry/`)
  — the parser delegates all resolution to it, so alias handling, namespace
  collisions, and the `suggest_commands` scorer are upstream of this lens.
- **Did not check Panel/`interfaces/webchat` consumers of the `commands.list`
  tree** — relevant to SEAM-1's CUT-vs-CONNECT call. I grepped `interfaces/`
  for the Rust symbol names only; a TS/WASM client reading a `context` key by
  string would not have shown up. **Confirm before CUTting SEAM-1.**
- **Did not run the graph query.** `graphify query "src/command module"` was
  in the brief; the module is 236 LOC across 2 files and I read both in full,
  so the graph would have added edges I already had directly. Noting it as
  skipped rather than silently dropping it.
- **No severity calibration against prior `review-results/` batches** beyond
  matching their tone; if this repo's "high" is reserved for user-visible
  breakage, SEAM-2 qualifies (`[[rules]]` system prompts silently ignored)
  and nothing else here does.
