# style.md — src/command/

- Path: `src/command/` (`mod.rs` 15 LOC + `parser.rs` 221 LOC = 236 LOC)
- Lens: style / Rust idioms / dead code / clippy (silent lint kinds)
- Method: static read only. No `cargo` invocation — findings below are marked
  **verified-by-reading** vs **needs-compile-check** where that distinction matters.

## Toolchain / gate baseline (matters for severity)

| Thing | Value | Source |
|---|---|---|
| Pinned toolchain | `1.96.0` (MSRV 1.95) | `rust-toolchain.toml` |
| `rustfmt.toml` | **absent** → rustfmt defaults (4-space, `max_width = 100`) | repo root; matches AGENTS.md "4-space indent, 100 char width" |
| `clippy.toml` | only `allow-*-in-tests = true` (7 keys) | `clippy.toml` |
| `[lints]` table | **absent** from root `Cargo.toml` | grep |
| `#![warn(clippy::pedantic)]` etc. | **absent** from `src/lib.rs` header | grep |
| CI gate | `cargo clippy -p alephcore -- -D warnings` | AGENTS.md |

**Consequence:** the enforced gate is the *default* clippy lint set. `pedantic`
and `nursery` are clearly run by hand (the codebase is full of `doc_markdown`
auto-fix artifacts like `"{`source_type}:{name`}"` in `tool_metadata`, plus
`#[must_use]` / `const fn` on every getter — both `pedantic`/`nursery`
signatures), but they are **not** gated. So no finding below can be `high`
under the current gate, and I do not inflate any to look like one.

---

## rustfmt + clippy nits

Read with default rustfmt config (4-space, width 100).

| # | File:line | Kind | Note |
|---|---|---|---|
| 1 | `src/command/parser.rs` (whole file) | rustfmt | **Clean.** No line exceeds 100 cols (longest ≈ 79: `parser.rs:124`). Indentation is uniform 4-space. `grep -n " $"` over `src/command/` returns nothing — no trailing whitespace. Match-arm struct-literal bodies (`115-140`) are in rustfmt's canonical vertical form. |
| 2 | `src/command/mod.rs` (whole file) | rustfmt | Clean. |
| 3 | `src/command/parser.rs:218` | clippy `pedantic::uninlined_format_args` | `panic!("expected Builtin (direct-tool) context, got {:?}", other)` → `panic!("expected Builtin (direct-tool) context, got {other:?}")`. Only actual pedantic hit in the module. Not gate-blocking. |
| 4 | `src/command/parser.rs:98-99` | clippy `nursery::redundant_clone` (likely) | `resolved.tool.name.clone()` + `resolved.tool.id.clone()` are both avoidable — `resolved` is owned, the only borrow (`tool_to_command_context(&resolved.tool)` at :94) has ended by :96. See STYLE-4. |
| 5 | `src/command/parser.rs:131` | *not* a nit | `tool.routing_regex.as_ref().unwrap_or(&tool.name).clone()` is the checklist's `.unwrap_or(&x)`-on-owned pattern, but it is **correct here**: `as_ref()` yields `Option<&String>`, the fallback is a borrow of a live field, and `.clone()` runs once on whichever won. No allocation is wasted. Rewriting it to `map_or_else` would be strictly worse. **Not reported as a finding.** |
| 6 | `src/command/parser.rs:76, 106` | *not* a nit | `#[must_use]` on `new` and on the `tool_registry()` getter is exactly what `clippy::must_use_candidate` asks for. The checklist's "useless `#[must_use]` on `&str -> Option<…>`" pattern does **not** occur — `parse_async` (`:85`) carries no `#[must_use]`, and correctly so (the returned `Future` is already `#[must_use]` by the trait). |
| 7 | `src/command/parser.rs:77` | *not* a compile error | See STYLE-8 — checklist item 9 is a false alarm. |

No `format!`-interpolation nits in production code (the module contains zero
`format!` calls). No redundant `.to_string()` on `&str` in production code.

---

## Doc accuracy

Every doc comment in the module, adjudicated.

| Location | Doc text (abridged) | Verdict |
|---|---|---|
| `mod.rs:1-11` | "Command Completion System … aggregates commands from Builtin / MCP / User prompts / Skills … exposed as a hierarchical JSON tree over `commands.list`" | **True but invisible** — written with `//`, not `//!`. See STYLE-1. Content itself checks out: `gateway::handlers::commands` does exist and serves `commands.list`. |
| `parser.rs:1-3` | "Unified Slash Command Parser / Delegates all command resolution to `ToolCatalog`" | **True.** `parse_async` does exactly one thing beyond the `/` check: `self.tool_registry.resolve_command(trimmed).await?`. |
| `parser.rs:13` | `command_name` — "Command name (without leading /)" | **Misleading.** The value is `resolved.tool.name` (`:98`) — the *canonical* tool name, not the word the user typed. `ToolCatalog` resolves aliases (`UnifiedTool::aliases`, `tool_metadata/types/unified/mod.rs:55-62`), so `/new` yields `command_name == "session_new"`. Also `resolve_command` strips a Telegram `@botname` suffix (`registry/query.rs:148`). The doc invites a reader to treat this as echo-back-what-the-user-typed. See STYLE-3. |
| `parser.rs:15-23` | `tool_id` — "verbatim from [`UnifiedTool::id`]", examples `builtin:session_new`, `mcp:fs:read_file`, `plugin:diag:ping`, `custom:3:translate` | **True, verified.** `:99` is a literal `resolved.tool.id.clone()` — no transformation. All four example shapes verified against the id constructors: `types/conflict.rs:245` (`builtin:{name}`), `:250` (`skill:{id}`), `:252` (`custom:{rule_index}:{name}`), `registration.rs:278` (`plugin:{plugin_id}:{tool_name}`), and `unified/mod.rs:48` documents `mcp:github:git_status`. The rationale half ("downstream no longer reconstruct it lossily") also holds — `command_handler.rs:167` consumes `parsed.tool_id` for the Custom arm. |
| `parser.rs:24, 26` | `arguments`, `context` | True, trivially. |
| `parser.rs:36` | `Builtin { tool_name }` — "Tool name for agent mode" | **Wrong for one of the three producers.** For `Builtin`/`Native` (`:115-117`) it holds `tool.name`; for `Plugin` (`:133-139`) it holds `tool.id`. The doc names only the first. See STYLE-2. |
| `parser.rs:41,43,48,50,52,54,59,61,63` | Mcp / Skill / Custom field docs | True. `provider: None` at `:130` matches its inline `// Provider is resolved at routing time` and the field doc "Provider override". |
| `parser.rs:75` | "Create a new command parser backed by `ToolCatalog`" | True. |
| `parser.rs:81-84` | `parse_async` — "Returns `Some(ParsedCommand)` if the input matches a registered command. Only processes inputs starting with '/'." | **True**, and slightly better than it claims: `input.trim()` at `:86` means leading whitespace is tolerated before the `/`. The checklist asks whether anything *else* is wrong here — no. The one omission is that the returned `command_name` is canonical, not literal (covered by STYLE-3, which belongs on the field, not here). |
| `parser.rs:105` | "Get a reference to the underlying `ToolCatalog`" | True. |
| `parser.rs:112` | "Derive `CommandContext` from `UnifiedTool` fields" | True. |
| `parser.rs:134-138` | Plugin defensive comment — "Plugin tools live in the tool registry under their namespaced id (`plugin:<plugin_id>:<name>`) … Routing them as `Mcp` mangled the id into `mcp__plugin:<id>_<name>` … so every plugin slash command failed" | **Still truthful, and still load-bearing.** Verified in three places: (a) the id format is literally `format!("plugin:{plugin_id}:{tool_name}")` at `registration.rs:278`; (b) the "direct-tool fast path" it names is real — `command_handler.rs:180-185` maps `CommandContext::Builtin` to `{"type":"direct_tool","tool_id":tool_name}`, and the `Mcp` arm (`:173-179`) emits `server_name`/`tool_name` separately, which is where the mangling would re-enter; (c) the invariant has a regression test (`parser.rs:184-220`) that asserts `plugin:diagnostics:ping` on **both** `tool_id` and the context payload. This is a comment earning its keep — do not prune it. |
| `parser.rs:189-192` | Test doc-comment on `test_parse_async_plugin_routes_to_direct_tool` | True; duplicates `:134-138` almost verbatim. Acceptable (test-as-documentation), noted only for completeness. |

---

## Naming smells

1. **`CommandContext::Builtin { tool_name }` carries an *id* for Plugin.**
   Three producers, two different value kinds in one field:
   - `:116` → `tool.name` (bare name, e.g. `session_new`)
   - `:139` → `tool.id` (namespaced id, e.g. `plugin:diagnostics:ping`)

   This is exactly the CLAUDE.md shape *"一个装了两种东西的字段"* — and the
   downstream consumer proves it: `command_handler.rs:184` serializes the field
   under the JSON key **`tool_id`**, not `tool_name`. The wire name already
   disagrees with the Rust name. See STYLE-2.

2. **`Native` is silently folded into `Builtin`.** `:115` matches
   `ToolSource::Builtin | ToolSource::Native` into `CommandContext::Builtin`.
   Behaviourally fine (both are direct-tool dispatch), but the variant name
   loses the distinction that `ToolSource` deliberately keeps, and there is no
   comment saying the fold is intentional — unlike the Plugin arm, which is
   heavily commented. Asymmetric commenting invites a future reader to "fix" it.

3. **`CommandParser::tool_registry` / `tool_registry()`** — the type is
   `ToolCatalog`, not a "registry". The whole module says `tool_registry`
   (`:71, 91, 107-108`) while the doc comments say `ToolCatalog` (`:68, 75, 105, 112`).
   Cosmetic; the alias is repo-wide (`gateway/execution_engine/*` also says
   `tool_registry`), so **do not rename** — noted only so it isn't re-flagged.

4. **`command_name` vs. what the user typed** — see STYLE-3.

---

## Public surface

`src/lib.rs:68` declares `pub mod command;`, so everything below is reachable
from outside the `alephcore` crate.

| Item | Line | Real consumers (grep over `src/`, `interfaces/`, `shared/`) | Could tighten? |
|---|---|---|---|
| `pub struct ParsedCommand` + 5 pub fields | `parser.rs:10-29` | `gateway/inbound_router/command_handler.rs:122` only | `pub(crate)` suffices |
| `pub enum CommandContext` (+ variants/fields) | `parser.rs:33-66` | `command_handler.rs:123`, `gateway/handlers/commands.rs:13` | `pub(crate)` suffices |
| `pub struct CommandParser` | `parser.rs:69-72` | `gateway/inbound_router/mod.rs:39`, `gateway/handlers/commands.rs:13` | `pub(crate)` suffices |
| `pub const fn CommandParser::new` | `parser.rs:77` | as above | `pub(crate)` suffices |
| `pub async fn parse_async` | `parser.rs:85` | `command_handler.rs`, `handlers/commands.rs` | `pub(crate)` suffices |
| `pub const fn tool_registry()` | `parser.rs:107` | **live, 3 call sites**: `command_handler.rs:229`, `command_handler.rs:483`, `inbound_router/mod.rs:924` | keep; `pub(crate)` suffices |
| `fn tool_to_command_context` | `parser.rs:113` | private already ✓ | correct as-is |
| `mod parser;` (private) + `pub use` | `mod.rs:13-15` | — | correct as-is |

**Zero external-crate consumers.** `interfaces/` and `shared/` contain no
`alephcore::command` reference; every user is inside `src/gateway/`. Per
AGENTS.md ("Default private, `pub(crate)` for internal sharing"), the whole
module could be `pub(crate) mod command;` at `lib.rs:68` — one line, no
churn inside `parser.rs`. See STYLE-6.

**No dead code found.** Every `pub` item has ≥1 real caller; every
`CommandContext` variant is constructed *and* matched (`command_handler.rs:143-186`
matches all four exhaustively, `handlers/commands.rs:389-411` matches all four
again). No `#[allow(dead_code)]` anywhere in the module.

---

## Findings

### STYLE-1 — `mod.rs` module docs are `//` comments, not `//!` doc comments
- **ID:** STYLE-1
- **Severity:** low (style) — but the highest-value item here
- **File:** `src/command/mod.rs:1-11`
- **Evidence:** all eleven header lines start with `//`. The sibling façade
  `src/sandbox/mod.rs:1-8` uses `//!`, as does `src/command/parser.rs:1-3` in
  the *same module*. Effect: `crate::command` renders as an undocumented
  module in `cargo doc`, and the "aggregates from Builtin / MCP / User prompts
  / Skills" inventory — the only place that inventory is written down — is
  invisible to `rustdoc` and to `missing_docs`-class lints.
- **Fix:** `//` → `//!` for lines 1-11. Zero behaviour change; `mod.rs` is
  otherwise clean.

### STYLE-2 — `CommandContext::Builtin { tool_name }` holds an id for Plugin; doc names only the name
- **ID:** STYLE-2
- **Severity:** low (naming/doc) — behaviour is *correct*, this is about the label
- **File:** `src/command/parser.rs:36-37` (doc + field), `:116` (name), `:139` (id)
- **Evidence:** field doc reads "Tool name for agent mode". `:116` stores
  `tool.name`; `:139` stores `tool.id`. The consumer at
  `command_handler.rs:184` emits it as JSON key `"tool_id"`, and the test at
  `parser.rs:215-216` asserts the field equals `"plugin:diagnostics:ping"` —
  an id. So both the wire format and the test already treat it as an id; only
  the Rust field name and its doc still say "name".
- **Fix:** cheapest correct move is **doc, not rename** — a rename ripples into
  `command_handler.rs:143,180-185` and `handlers/commands.rs:370,391`. Amend
  the field doc to: *"Direct-tool dispatch target. `Builtin`/`Native` store the
  bare `UnifiedTool::name`; `Plugin` stores the namespaced `UnifiedTool::id`
  (both are valid keys for the direct-tool fast path)."* If a rename is
  preferred later, `dispatch_id` is the honest name.
- **Note (out of lens, flagged per AGENTS.md §3):** the deeper shape is the
  CLAUDE.md criterion *"一张列举法名单的上游，常常是一个装了两种东西的字段"*.
  A structurally cleaner fix is a distinct `CommandContext::DirectTool { tool_id }`
  variant. That is a behaviour-touching refactor and belongs in the logic
  audit, not here.

### STYLE-3 — `command_name` doc implies the literal typed word; it is the canonical name
- **ID:** STYLE-3
- **Severity:** low (doc accuracy)
- **File:** `src/command/parser.rs:13`, value assigned at `:98`
- **Evidence:** doc says "Command name (without leading /)". `:98` assigns
  `resolved.tool.name`. `ToolCatalog` matches aliases with lower precedence
  than the canonical name (`tool_metadata/types/unified/mod.rs:55-62`) and
  strips `@botname` (`registry/query.rs:148`), so `/new@mybot topic` yields
  `command_name == "session_new"`. A reader wiring an echo-back or a
  "did you mean" reply from this field gets the wrong string.
- **Fix:** *"Canonical `UnifiedTool::name` of the resolved tool — **not** the
  literal word the user typed (aliases and Telegram `@botname` suffixes are
  resolved away by `ToolCatalog`). Use `raw_input`-derived text if you need to
  echo the user."*

### STYLE-4 — two avoidable `String` clones per slash command
- **ID:** STYLE-4
- **Severity:** low (clippy `nursery::redundant_clone`, not gated)
- **File:** `src/command/parser.rs:98-99`
- **Evidence:** `resolved: ResolvedCommand` is owned by value
  (`registry/types.rs:15-22`). The only borrow of it, `tool_to_command_context(&resolved.tool)`,
  ends at `:94`; `:100` already *moves* `resolved.arguments`. So `:98`/`:99`
  clone two `String`s out of a value that is about to be dropped.
- **Fix:**
  ```rust
  let context = tool_to_command_context(&resolved.tool);
  let source_type = ToolSourceType::from(&resolved.tool.source);
  Some(ParsedCommand {
      source_type,
      command_name: resolved.tool.name,
      tool_id: resolved.tool.id,
      arguments: resolved.arguments,
      context,
  })
  ```
  (partial moves out of `resolved.tool` are fine once the borrow is dead —
  **needs-compile-check**, but there is no `Drop` impl on `ResolvedCommand`
  or `UnifiedTool` to block it.)
- **This is also the answer to checklist item 11.** Slash-command parsing runs
  once per inbound user message — human-rate, not hot-path. `Arc<str>` /
  `Cow<'static, str>` on `ParsedCommand` would buy nothing measurable and
  would infect four consumer match sites with `&*` noise. **Do not convert.**
  Deleting these two clones is the whole win available, and it is small.

### STYLE-5 — `uninlined_format_args` in test panic
- **ID:** STYLE-5
- **Severity:** low (clippy pedantic, not gated)
- **File:** `src/command/parser.rs:218`
- **Evidence:** `panic!("expected Builtin (direct-tool) context, got {:?}", other)`
- **Fix:** `panic!("expected Builtin (direct-tool) context, got {other:?}")`

### STYLE-6 — whole module could be `pub(crate)`
- **ID:** STYLE-6
- **Severity:** low (public-surface hygiene)
- **File:** `src/lib.rs:68` (`pub mod command;`)
- **Evidence:** grep over `src/`, `interfaces/`, `shared/` finds exactly four
  `crate::command::` references, all under `src/gateway/`
  (`command_handler.rs:122,123`; `inbound_router/mod.rs:39`;
  `handlers/commands.rs:13`). No workspace member reaches `alephcore::command`.
  AGENTS.md: "Default private, `pub(crate)` for internal sharing."
- **Fix:** `pub(crate) mod command;`. **needs-compile-check** — if it builds,
  the `pub` items inside `parser.rs` become effectively crate-private with no
  edit to `parser.rs` at all. If some doc-test or example depends on it, leave
  it and do nothing.

### STYLE-7 — `ResolvedCommandContext` mirror is justified but undocumented
- **ID:** STYLE-7
- **Severity:** low (missing doc on a deliberate decision)
- **File:** `src/gateway/handlers/commands.rs:365-411`
- **Evidence (answers checklist item 10):** the duplication is **not**
  gratuitous — it is a deliberate *narrowing*. The `From` impl at `:389-411`
  drops four fields on the floor:
  - `Skill { instructions, allowed_tools }` — dropped at `:404-407`
  - `Custom { system_prompt, provider }` — dropped at `:409`

  So deriving `Serialize` on `CommandContext` and sending it directly would
  ship **skill instruction bodies and custom-rule system prompts** to every
  Panel client that calls `command.execute`. That is precisely the CLAUDE.md
  over-send hazard (*"解析只能证明超集，永远证不出相等"*). The mirror is the
  right call. **Recommendation: keep it.**

  The defect is that nothing says so. `ResolvedCommandContext`'s doc
  (`:364`) reads only "Command context details for the client" — a future
  reader doing exactly the checklist-item-10 reasoning ("this is a duplicate,
  let me merge it") would delete the narrowing and leak prompts, with no test
  going red.
- **Fix:** add to `:364`:
  *"Wire-facing **narrowing** of `command::CommandContext`. Deliberately drops
  `Skill.instructions` / `Skill.allowed_tools` / `Custom.system_prompt` /
  `Custom.provider` — those are server-side execution inputs and must not
  reach clients. Do **not** replace this with `#[derive(Serialize)]` on
  `CommandContext`; that would over-send all four."*
  Consider pairing with a key-set-equality test derived from the type itself
  (per CLAUDE.md §0), rather than a literal key list.

### STYLE-8 — checklist item 9 is a false alarm (recorded so it isn't re-raised)
- **ID:** STYLE-8
- **Severity:** none — **not a defect**
- **File:** `src/command/parser.rs:77`
- **Evidence:** `pub const fn new(tool_registry: Arc<ToolCatalog>) -> Self`
  never calls `Arc::new`. It moves an already-constructed `Arc` into
  `Self { tool_registry }`. `const fn` may take non-`Copy` parameters and move
  them into a struct literal; the const-stability of `Arc::new` is irrelevant
  because it does not appear. Same for `const fn tool_registry(&self)` at
  `:107`, which only returns a reference. Both compile on 1.96.0.
  (`Arc` here is `crate::sync_primitives::Arc` — under the `loom` test feature
  this aliases `loom::sync::Arc`, which is still only *moved*, never
  constructed, in this file.)
- **Fix:** none. The `const` on both is `clippy::nursery::missing_const_for_fn`
  compliance.

---

## Tests (checklist item 5 + item 7)

- **Naming:** all four follow `test_<unit>_<scenario>` —
  `test_parse_async_found` (`:152`), `test_parse_async_not_found` (`:170`),
  `test_parse_async_not_slash` (`:177`), `test_parse_async_plugin_routes_to_direct_tool` (`:193`). ✓
- **Async attribute:** `#[tokio::test]` on all four, no `#[test]` + manual
  runtime, no `#[tokio::test(flavor = …)]` divergence. ✓
- **Placement:** bottom-of-file `#[cfg(test)] mod tests` with `use super::*`
  (`:144-146`). That **is** the Rust idiom — "co-located" in Rust means
  same file, not interleaved with each function. ✓
- **`#[cfg(test)]` discipline (item 7):** tests touch **no private item** of
  `parser.rs`. `use super::*` pulls in the private `tool_to_command_context`,
  but nothing calls it; every assertion goes through the public
  `CommandParser::new` / `parse_async` and through `ToolCatalog`'s own public
  API (`register_custom_commands`, `register_plugin_tools`, `ToolCatalog::new`).
  A refactor of `parser.rs` internals would not break these tests. ✓
  The one coupling is `use crate::config::RoutingRuleConfig` (`:147`) —
  cross-module but public, and unavoidable for building a custom-rule fixture.
- **Gap (noted, out of lens):** `tool_to_command_context` has four match arms;
  only the `Custom` and `Plugin` paths are exercised. The `Skill` arm
  (`:122-127`, which does an `unwrap_or_default()` on `routing_system_prompt`)
  and the `Mcp` arm have no test. Also untested: bare `/`, and the
  leading-whitespace trim at `:86`. Belongs to a coverage audit, not this one.

---

## Project-idiom alignment

| Convention (AGENTS.md / CLAUDE.md) | Status |
|---|---|
| rustfmt 4-space / 100-col | ✓ clean, both files |
| `snake_case` / `PascalCase` / `SCREAMING_SNAKE_CASE` | ✓ no violations |
| "Default private, `pub(crate)` for internal sharing" | ⚠ **diverges** — module is `pub` with zero external consumers → STYLE-6 |
| "Never `unwrap()` in production" | ✓ zero `unwrap`/`expect`/`panic` in production code. `unwrap`/`expect`/`panic` appear only under `#[cfg(test)]`, which `clippy.toml` explicitly permits (`allow-unwrap-in-tests`, `allow-expect-in-tests`, `allow-panic-in-tests`). |
| Immutability by default | ✓ no `let mut` in the module |
| English code comments | ✓ throughout |
| P2 "single file > 500 lines → split" | ✓ 221 LOC, far under |
| Module façade idiom | ⚠ **diverges from `src/sandbox/mod.rs`** in *two* directions: (a) `//` instead of `//!` → STYLE-1 (this one is a defect); (b) `mod parser;` private + `pub use` rather than `pub mod parser;` + `pub use`. **(b) is fine and arguably better** — it makes `crate::command::parser` unreachable, so the re-export at `mod.rs:15` is the only door. Facade-with-private-submodule is used elsewhere in the repo. No change recommended for (b). |
| CLAUDE.md "同一事实的两份表述，只改一份就是静默说谎" | ⚠ two live instances: STYLE-2 (field name vs. wire key `tool_id` vs. field doc) and STYLE-3 (doc vs. alias-resolved value). Both are doc-side fixes. |
| CLAUDE.md §10 "`cargo check` 不编译 `#[cfg(test)]`" | n/a to this read-only audit, but relevant to STYLE-4/STYLE-6: **both are `needs-compile-check`, and STYLE-6 in particular must be validated with `cargo test -p alephcore --lib --no-run`, not `cargo check`** — a visibility narrowing can break test-only paths that `cargo check` never expands. |

---

## What I did NOT do (per AGENTS.md §6)

- **Ran no compiler.** Read-only mandate. STYLE-4 (partial move out of
  `resolved.tool`) and STYLE-6 (`pub` → `pub(crate)`) are reasoned from the
  type definitions and grep, and are marked **needs-compile-check** above. I did
  not run `cargo clippy -- -W clippy::pedantic -W clippy::nursery`, so the
  pedantic/nursery list (STYLE-4, STYLE-5) is what static reading surfaces —
  it may be incomplete.
- **Did not audit logic/correctness.** The `Native`→`Builtin` fold (`:115`),
  the `provider: None` hardcode (`:130`), and whether
  `is_continuation_driven_slash` (`command_handler.rs:136`) covers every
  continuation-driven builtin are behaviour questions outside this lens.
- **Did not verify `ToolSourceType::from(&ToolSource)`** (`:93`) is total /
  correct — only that it is called. `src/tool_metadata/types/unified/source.rs`
  was not readable at the path I probed.
- **Did not read all of `gateway/handlers/commands.rs`** (~1000 LOC). I read
  the `ResolvedCommandContext` definition + `From` impl (`:355-411`), the
  `handle_execute` doc/signature (`:437-470`), and the two test call sites
  (`:994`, `:1001`). Conclusions about that file are scoped to those regions.
- **Did not check whether any external workspace member outside
  `src/`, `interfaces/`, `shared/`** (e.g. `desktop/*`, `mobile/*`, a build
  script, or a doc-test) references `alephcore::command`. STYLE-6 is
  conditional on that grep being complete.
- **Proposed no rename for `Builtin { tool_name }`** — deliberately chose the
  doc fix, since a rename touches four consumer sites in two other modules and
  the structurally right fix (a separate `DirectTool` variant) is a behaviour
  change that belongs to the logic audit.
