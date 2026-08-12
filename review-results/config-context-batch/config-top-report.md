# src/config top-level — severed-wire audit

## Files in scope

| File | Lines |
|---|---|
| `src/config/mod.rs` | 55 |
| `src/config/structs.rs` | 534 |
| `src/config/load.rs` | 448 |
| `src/config/save.rs` | 505 |
| `src/config/methods.rs` | 245 |
| `src/config/patcher.rs` | 1514 |
| `src/config/validate.rs` | 705 |
| `src/config/schema.rs` | 41 |
| `src/config/migration.rs` | 104 |
| `src/config/defaults_override.rs` | 229 |
| `src/config/presets_override.rs` | 561 |
| `src/config/backup.rs` | 382 |
| `src/config/live_apply.rs` | 251 |
| `src/config/reload_impact.rs` | 230 |
| `src/config/guides.rs` | 62 |
| `src/config/ui_hints/mod.rs` | 88 |
| `src/config/ui_hints/macros.rs` | 236 |
| `src/config/ui_hints/definitions.rs` | 362 |
| **Total** | **6552** |

## Method

Full read of all 18 files (patcher.rs and validate.rs read in one pass each —
both are single, uninterrupted `impl` blocks with tests appended at a clean
`#[cfg(test)] mod tests` boundary, confirmed via `grep -n "^pub fn\|^fn \|^impl "`
before trusting the split). Every `pub`/`pub(crate)` item in scope was grepped
across the whole repo (`src/`, `interfaces/`, `tests/`, `src/bin/`) with test
modules excluded from the consumer count. Where a previous audit
(`review-results/config.md`, commit `dcd2c678c`, partially fixed in
`fb4f942a5`) had already covered a symbol, its "NOT A BUG" verdict was
independently re-derived from the current source, not trusted.

## Phase 1 — Seam scan results

- **Registration parity**: no `register_*`/const-table pattern exists at this
  level (that pattern lives in `gateway/handlers/mod.rs`, out of scope). The
  one const-table this batch does own is `LIVE_SECTIONS` in `reload_impact.rs`
  ↔ the `match` arms in `live_apply.rs` — and that seam is **self-guarded** by
  an in-file test (`every_live_section_has_an_apply_arm`) that fails compile-time
  review if the two drift. No action needed; noted as the model other tables
  in this codebase should copy (see Phase 5).
- **Call-vs-handler parity**: `mod.rs` re-exports (`ConfigPatcher`,
  `classify_verified`, `ReloadImpact`, `generate_config_schema_json`,
  `build_ui_hints`, `ConfigUiHints`, `types::*`) — all traced to real callers
  outside `#[cfg(test)]`, **except** `build_ui_hints`/`ConfigUiHints`, whose
  chain terminates at a discarded RPC field (see H1).
- **Config-reader parity**: every field on `Config` added/touched by these
  files (`presets_override`, `defaults_override`, `ssrf`, `resolved_channels()`
  output) has a live, non-test reader. No inert `Config` fields found in this
  batch.
- **Stub sweep**: zero `TODO`/`FIXME`/`unimplemented!`/`todo!`/empty match arms
  in non-test code across all 18 files (`live_apply.rs`'s `_ => false` arm is
  a real branch guarded by its own drift test, not a stub).
- **Path/route parity**: no filename/RPC-method spelling drift found. `H1`
  below is a *content* severance, not a *name* one — the RPC method resolves,
  the field serializes, nothing renders it.

## Phase 2 — Candidate list

| Producer | Consumer | Note |
|---|---|---|
| `ui_hints/definitions.rs:15` `build_ui_hints()` | `gateway/handlers/config.rs:366` (in scope of RPC handler, out of file scope) | RPC field populated; **no client reads `.ui_hints`** — see H1 |
| `ui_hints/macros.rs` `define_groups!`/`define_hints!` | `ui_hints/definitions.rs` only | Zero use outside the one definitions file that authors them |
| `structs.rs:43` `is_default_session` | `structs.rs:183` (`#[serde(skip_serializing_if)]`) | Wired; re-verified NOT A BUG (H-prev-1) |
| `presets_override.rs` merge/partial fns | `providers/presets/mod.rs`, `types/generation/presets/mod.rs` | Wired |
| `backup.rs` `ConfigBackup::{new,create_snapshot,resolve,list,cleanup,default_dir}` | `patcher.rs`, `bin/aleph-server/commands/start/mod.rs`, 5 builtin-tool test suites | Wired |
| `live_apply.rs` `apply_live_sections`/`classify_verified` | `patcher.rs:397,647`, `gateway/handlers/config.rs:100,597`, `self_config.rs:492` | Wired, self-guarded |
| `reload_impact.rs` `ReloadImpact::classify`/`agent_hint`/`user_hint_zh` | `self_config.rs`, `agent_manage/update.rs`, `gateway/handlers/config.rs:603` | Wired |
| `guides.rs` `GUIDE_FILES`/`deploy_guides` | `lib.rs:144`, `bin/aleph-server/commands/start/mod.rs:92`, `builtin_tools/config_guide.rs:214` | Wired |
| `load.rs` `apply_security_ssrf_overrides` | called at `load.rs:143`, tested by 6 in-file tests | Wired, no external consumer needed (internal bridge) |
| `migration.rs` `migrate_mcp_builtin_in_toml`/`migrate_vector_db_in_toml` | `load.rs:124-125` | Wired |
| `methods.rs` `add_rule_at_top`/`remove_rule`/`move_rule`/`get_rule`/`rule_count` | `gateway/handlers/routing_rules.rs` (confirmed by previous audit, not re-walked this pass — out of file scope) | Wired |
| `structs.rs` `resolved_channels()` | `bin/aleph-server/commands/start/builder/subsystems.rs` (3 call sites) | Wired |

## Phase 3 — Triage (read-first)

For `build_ui_hints`/`ConfigUiHints`, the read-before-write step surfaced a
prior, *partial* fix of exactly this problem: commit `6e63cabe2` ("config: cut
ConfigUiHints test-only helpers") already deleted `get_hint`, `merge`,
`sorted_groups`, `fields_in_group` — the query-side API that would have let a
consumer actually *use* a hint by path — on the grounds that "no production
caller ever resolved a hint by path." Its justification for keeping the DTO
and builder was: *"the DTO ConfigUiHints { groups, fields } is the live
serialization shape (gateway/handlers/config.rs reads it as JSON)."* That
justification conflates "the RPC handler serializes it" with "a client
consumes it" — the two are not the same fact, and grepping the actual client
side shows the second one is false. This is not a fresh finding so much as
the second half of an already-started cut.

## Phase 4 — Findings

### H1. `ui_hints/` (686 LOC across 3 files) is a fully-built producer with zero real consumers of its content

- **Producer**: `src/config/ui_hints/definitions.rs:15` `build_ui_hints()` —
  builds `ConfigUiHints { groups: 8 GroupMeta, fields: ~50 FieldHint }` via
  `define_groups!`/`define_hints!` (`src/config/ui_hints/macros.rs`, 236 LOC of
  macro machinery used **only** by `definitions.rs`). `mod.rs` (88 LOC) defines
  the DTO (`GroupMeta`, `FieldHint`, `ConfigUiHints`) with 7 sub-fields
  (`label`, `help`, `group`, `order`, `advanced`, `sensitive`, `placeholder`,
  `icon`).
- **Wiring**: `gateway/handlers/config.rs:366` calls `build_ui_hints()` and
  embeds it as `ConfigSchemaResponse.ui_hints`, returned by the `config.schema`
  RPC (registered at `gateway/handlers/mod.rs:259`).
- **Consumer trace**:
  - `interfaces/cli/src/commands/config_cmd.rs:129-131` is the **only**
    production caller of `config.schema` in the entire repo (confirmed by
    `grep -rn "\"config.schema\""` across `src/` and `interfaces/`). It does:
    `let schema = result.get("schema").cloned().unwrap_or(result);` — this
    extracts `.schema` and **discards everything else**, including
    `.ui_hints`, before printing/writing it.
  - `interfaces/webchat/` (the Panel/Leptos frontend, the only plausible
    renderer of `label`/`help`/`group`/`order`/`advanced`/`sensitive`/
    `placeholder`) **never calls `config.schema`** — zero hits for that string
    anywhere under `interfaces/webchat/src/`. Its actual config-rendering
    machinery (`json_schema_form.rs`) builds its own local `FieldSpec` from a
    *different* schema (`config_schema` from the extension-manifest system,
    `src/extension/manifest/types.rs::ConfigUiHint` — an unrelated, differently
    named type) plus `secrets[*].sensitive` from a disclosure payload. It does
    not read `crate::config::ui_hints::{ConfigUiHints, FieldHint, GroupMeta}`
    at all.
  - No other RPC, builtin tool, or CLI command reads `ConfigUiHints`,
    `GroupMeta`, or `FieldHint` outside `#[cfg(test)]`.
- **Severity**: MEDIUM. Nothing is broken today — the RPC still serializes and
  returns valid JSON, and the one real caller ignores the extra field
  harmlessly. The cost is maintenance debt: 686 lines (with two purpose-built
  macros) of field-path literals (`"providers.*.api_key"`,
  `"mcp.servers.*.env"`, `"channels.telegram.token"`, etc.) that nothing
  outside this module's own tests cross-checks against the live `Config`
  schema for continued accuracy (`test_schema_and_hints_consistency` only
  checks path *syntax*, not that the path still resolves in
  `generate_config_schema_json()`). It will silently drift as fields are
  renamed, and the next person to touch `ui_hints/` will reasonably assume
  it's live because it's RPC-wired and has 9 passing tests.
- **Triage**: **DECIDE**. This is not an accidental severed wire (produce +
  forget) — it is a *half-finished* feature cut: `6e63cabe2` already removed
  the "no consumer" query API and left the DTO believing a consumer existed
  downstream of the RPC. Two honest options:
  1. **CUT**: delete `ui_hints/` (all 3 files), the `pub use` in `mod.rs:48`,
     the `ui_hints` field on `ConfigSchemaResponse`, and the `build_ui_hints()`
     call in `gateway/handlers/config.rs:366` (outside this file's scope but
     the necessary other half of the cut). Matches the precedent `6e63cabe2`
     already set for the query-side half.
  2. **CONNECT**: if a Panel settings-form-from-schema view is actually
     planned, wire `interfaces/webchat` to call `config.schema` and consume
     `.ui_hints` for labels/grouping/sensitivity — at which point the 686
     lines earn their keep.
  Given this is a judgment call about product direction (is a generic
  schema-driven settings form still planned?), it is flagged DECIDE rather
  than unilaterally CUT.
- **Proposed fix**: whichever way it's decided, do the *whole* cut or the
  *whole* connect — leaving the DTO wired to an RPC nobody reads is the
  intermediate state that caused this in the first place.

### Re-verified "NOT A BUG" claims from the previous audit (`review-results/config.md`, commit `dcd2c678c`)

All claims below were independently re-derived from the current source, not
assumed from the prior report.

- **H1 (prev) — `is_default_session` free fn as serde `skip_serializing_if`**
  (`structs.rs:43-46`, used at `structs.rs:183`): confirmed unchanged.
  Re-checked `SessionConfig::default()` (`src/routing/config.rs:46-53`) — it
  is a plain static literal (`DmScope::PerPeer` + empty `HashMap`), no env or
  runtime dependency. **Still NOT A BUG.**
- **M3 (prev) — `patcher.rs` file size**: was 1498 lines at the prior audit,
  now 1514 (+16, the H2 panic-based fix plus its doc comment). Confirmed the
  file is still one clean `impl ConfigPatcher` block + free helper fns + a
  `#[cfg(test)] mod tests` boundary at line 869 — no non-test code hides past
  that boundary (`grep -n "^pub fn\|^fn "` confirms only test code follows).
  **Still a stylistic observation, not a bug.**
- **L1 (prev) — `defaults_override` `OnceLock` re-init semantics**
  (`defaults_override.rs:73-81`): confirmed unchanged. `init_defaults_override`
  still warns and silently ignores a second `.set()`; `load.rs` calls it
  exactly once per `Config::load()`/`load_from_file()` path before `Config`
  construction (lines 116-118 and 243-244), matching the documented ordering
  requirement. **Still NOT A BUG** — the warning-on-reinit is the intended
  single-process-lifetime contract, not a race.
- **L2 (prev) — `resolved_channels()` warn-and-skip on unknown channel keys**
  (`structs.rs:322-328`): confirmed unchanged, same `tracing::warn!` soft-fail
  behavior. Now with a concrete consumer trace this pass didn't have before:
  `bin/aleph-server/commands/start/builder/subsystems.rs` calls
  `resolved_channels()` three times to register iMessage/Telegram/etc. gating
  config — an unknown-typed channel key is silently excluded from that
  registration with only a log line, no user-facing surface. Confirmed
  low-severity as before (a config-authoring mistake, not a wiring break) but
  worth noting the blast radius is "channel access policy silently defaults to
  the router's fallback," which is exactly the class of bug
  `src/gateway/CLAUDE.md` calls out elsewhere as high-consequence when it
  happens *silently*. Still **LOW**, not escalating without more evidence
  that this actually happens in practice.
- **L3 (prev) — `structs.rs` mixes `Config` + `PluginMarketplaceEntry` +
  `ChannelInstanceConfig` + `is_default_session` helper**: confirmed
  unchanged, still one file. Purely organizational. **Still NOT A BUG.**
- Also independently re-confirmed **L4 from the prior report is stale**: it
  claimed `Config::migrate_fetch()` was "not called by `load.rs`" (dead
  wiring). Current `load.rs:181` calls `config.migrate_fetch();` — `git blame`
  shows this line was added in commit `a03e6444f0` (2026-06-28), which
  **predates** the prior audit commit (`dcd2c678c`). The L4 finding was
  already incorrect at the time it was written; it is not something that
  regressed. No action needed today.
- **H3/H4/M1 from the prior report** concern `src/config/types/tools.rs`,
  `src/config/types/acp.rs`, and `src/gateway/handlers/mod.rs` — all outside
  this batch's file list. Spot-checked H3 only (its file was already open):
  `ToolServiceConfig::parallel_tool_concurrency_opt()` at
  `src/config/types/tools.rs:71-74` still unconditionally returns `Some(..)`,
  doc comment unchanged. Consistent with the prior "MEDIUM, doc/code
  divergence" verdict. Not re-verified in full since out of scope for this
  batch — flag for whichever batch covers `src/config/types/`.

## Phase 5 — Guard recommendation

1. **`live_apply.rs` ↔ `reload_impact.rs` is the pattern to copy.** The
   `LIVE_SECTIONS` const table and the `apply_live_sections` match are kept in
   sync by an in-module test (`every_live_section_has_an_apply_arm`) that
   fails if either side gets an entry the other doesn't have. This is exactly
   the "single-source-of-truth + drift guard" shape the skill asks for, and
   it already exists in this batch — no new guard needed there.
2. **New guard recommended for `ui_hints/`, regardless of which way H1 is
   resolved.** If CONNECT is chosen, add a test (in `schema_integration.rs` or
   a new file) that walks every non-wildcard key in
   `build_ui_hints().fields` and asserts it resolves inside
   `generate_config_schema_json()`'s `$defs`/`properties` tree — today
   `test_schema_and_hints_consistency` only checks path *syntax*
   (no leading/trailing dot, non-empty segments), not that the path still
   *exists* in the live schema. Without that, the hint list will drift silently
   exactly the way `6e63cabe2`'s own removed query API already proved nobody
   is watching it. If CUT is chosen, no guard is needed because the code is
   gone.
3. **General pattern for this codebase**: when a `pub` DTO's only production
   reference is "an RPC handler serializes it," that is not yet evidence of a
   live consumer — the read-before-write step should always walk one hop
   further, to whoever calls that RPC, and confirm the specific field is
   actually read there. `6e63cabe2` stopped one hop short (it verified the RPC
   handler reads `ConfigUiHints`, but not that the RPC's caller reads
   `.ui_hints` from the response).

## What I did NOT do

- Did **not** edit any code — this is a read-only static audit as instructed.
- Did **not** re-walk `src/config/types/*.rs` (the `types` submodule, ~76
  files per the prior full-module audit) — out of scope for this "top-level
  files only" batch. H3/H4 from the prior report live there and were only
  spot-checked, not fully re-verified.
- Did **not** re-walk `src/config/agent_manager.rs`/`agent_resolver.rs`
  (actually directories, `src/config/agent_manager/`, `src/config/agent_resolver/`)
  — declared in `mod.rs` but not in this batch's file list. A quick sanity
  grep confirms both are heavily consumed by `src/gateway/agent_instance.rs`,
  `src/gateway/admin_api/`, and `src/gateway/handlers/agents.rs`, so they are
  not orphaned, but I did not do a full line-by-line pass.
- Did **not** verify the M1 (prior) claim about `gateway/handlers/mod.rs`
  placeholder RPCs (`agents.*`, `runtimes.install`) — that file is not in this
  batch.
- Did **not** run `cargo check`/`cargo test` — this was a static read-only
  review per the task instructions; wiring conclusions are grep- and
  read-based, not compiler-verified. All consumer traces were confirmed by
  reading the actual call site, not just grep hit counts, but a compile pass
  was not run to catch anything a text search could miss (e.g. macro-generated
  call sites).
- Did **not** investigate whether the Panel/webchat frontend has any *planned*
  (unimplemented, e.g. in a design doc or open branch) consumer for
  `config.schema`'s `ui_hints` field — the DECIDE verdict on H1 depends on
  product intent that isn't visible from static source alone.
