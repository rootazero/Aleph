# Module: src/tool_metadata (occams-r9 review, 2026-09-29)

## Summary

- **Files reviewed**: 23 .rs (191,066 bytes). Round-8 listed "23 .rs, ~248 KB"; the byte count is a text-encoding artifact (190KB on disk), the file count matches.
- **Total findings**: 2 critical / 6 warning / 2 suggested test
- **Status of round-8 Criticals**: C1 PARTIAL FIX (MCP path wired, 4 other paths still dead); C2 PARTIAL FIX (mutation method + lock + `pub(crate)` added, zero production callers)
- **Status of round-8 Warnings**:
  - W2 (loom tests synthetic): **STILL PRESENT** (file untouched since round-8)
  - W3 (constants.rs dead): **STILL PRESENT, REFINED** (9 of 13 dead; 2 added, both live)
  - W4 (id collision LWW): **PARTIAL FIX** (now logs prev_id/prev_name/prev_source; still LWW semantics)
  - W5 (rename display format): **STILL PRESENT** (RenameExisting `(renamed)` vs RenameNew `(MCP)` unchanged)
  - W8 (pub fields bypass AsyncRwLock): **FIXED** (now `pub(crate)` per a350419d5 H3)
  - W9 (register_plugin_tools dead): **FIXED** (wired in `bin/aleph-server/.../tool_catalog_init.rs:218` and `command/parser.rs:247`)
  - W10 (ToolCatalog::search dead): **STILL PRESENT** (only `tests.rs:286` calls it; zero production callers)
  - W11 (SeqCst vs Acquire/Release in loom): **STILL PRESENT** (file untouched)
  - W12 (HealthSnapshot::unhealthy_iter Instant leak): **FIXED** (commit a9c70a2ab + health.rs:322 captures `now` outside closure)
  - W13 (visibility ordering vs name-conflict): **FIXED** (single pass under write lock per bca0e8a6f)
- **NEW findings (not in round-8)**: 5 new warnings, 1 new suggested test

Round-8 C1/C2 had a single explicit follow-up commit `8d1eeb497` ("tool_metadata: wire safety/confirmation to catalog + add active-flag setter (round-8 §3 critical)") that fixed the MCP-side wiring but did not propagate to the other four registration paths. C2's mutation method exists and is lock-protected but has zero production callers in `src/` (the only `set_active`/`set_inactive` matches in production are unrelated `agent_env::set_active_agent` and `scratchpad_registry::set_active` — different methods on different types).

---

## Critical

- **C-NEW-1** [correctness, partial-fix gap] `src/tool_metadata/registry/registration.rs:55-200, 250-352, 532-575, 578-660` — `with_safety_level` + `with_requires_confirmation` only wired in the MCP path
  - **Round-8 status**: CONFIRMED-Cr (C1 re-verified; partial fix leaves the bug alive for 4 of 5 paths)
  - **Description**: Commit `8d1eeb497` wired `.with_requires_confirmation(true)` and `.with_safety_level(...)` into `src/tools/handlers/registration.rs:140, 146, 155` (the MCP path). The four other registration paths still build `UnifiedTool::new(...)` with no safety/confirmation chain:
    - `register_builtin_tools` (registration.rs:45-237): 9 entries (skill_read, skill_list, groupchat, session_new, cron_manage, voice, goal, help, plus aliases), all bare `UnifiedTool::new`. Default `safety_level=ReadOnly` and `requires_confirmation=false` propagate from `UnifiedTool::new()` (types/unified/mod.rs:243).
    - `register_skills` (registration.rs:301-352): the per-skill `UnifiedTool::new` chain carries `.with_display_name/.with_icon/.with_usage/.with_param_hint (optional)/.with_routing_regex/.with_routing_intent_type/.with_routing_capabilities/.with_routing_strip_prefix` but never `.with_safety_level` or `.with_requires_confirmation`. A skill row's `requires_confirmation` field on `SkillInfo` is parsed by `skill::frontmatter` but never read here.
    - `register_plugin_tools` (registration.rs:532-575): single `UnifiedTool::new` chain — no safety/confirmation.
    - `register_custom_commands` (registration.rs:622-660): single `UnifiedTool::new` chain with optional `.with_routing_system_prompt` — no safety/confirmation.
  - **Evidence**:
    - `src/tools/handlers/registration.rs:140, 146, 155` — the only production callers of `with_requires_confirmation`/`with_safety_level` outside tests.
    - `src/tool_metadata/types/unified/tests.rs:49, 95, 103` — same fluent setters, but only in tests.
    - Boot path (`src/bin/aleph-server/commands/start/builder/agent_init/tool_catalog_init.rs:40, 151, 180, 218`) calls all four functions, propagating the gap to the live catalog.
  - **Downstream consequence**: `infer_visible_channels` (`src/tool_metadata/registry/conflict.rs:21-34`) gates on `safety_level` and `requires_confirmation` to restrict iMessage / Telegram / Discord. Because those fields are uniformly default for 4 of 5 sources, the routing intent round-8 described ("risky op stays on Panel/CLI, confirmation-required excluded from iMessage") is silently downgraded to "always visible" for every non-MCP source — meaning the only safety/restriction that ever fires is on MCP-registered tools, which contradicts the documented intent.
  - **Suggested fix**: Move the safety/confirmation derivation out of `register_mcp_tools` and into a single `populate_safety_profile(&mut self, source: ToolSource, src_meta: ...)` helper called by all five registration paths. For skills, read the parsed `SkillInfo.requires_confirmation` (or a new `skill::frontmatter::safety_level()`); for builtin and custom rules, encode the policy in the config (`RoutingRuleConfig.confirmation_required: bool` is the obvious shape); for plugin tools, read the `commands/*.md` `allowed-tools:`/model frontmatter already parsed by `extension::manifest::parsers.rs`. Until then, the four non-MCP paths silently mis-classify.

- **C-NEW-2** [correctness, dead wiring] `src/tool_metadata/registry/mod.rs:260` + `src/tool_metadata/registry/state.rs:90` — `ToolCatalog::set_active` / `ToolState::set_active` has no production caller
  - **Round-8 status**: CONFIRMED-Cr (C2 re-verified; the "field never mutated" symptom became "mutation method exists but nothing calls it")
  - **Description**: Round-8 found `is_active` was a `pub` field that was read on every query path but mutated only in tests (`tools/adapters/registry_adapter.rs:1209`). Commit `8d1eeb497` and `a350419d5` fixed three things:
    1. Added `ToolState::set_active` under the write lock (`state.rs:90`, case-insensitive per `7660b5af5`).
    2. Added the `ToolCatalog::set_active` wrapper (`registry/mod.rs:260`), which invalidates the health cache on change.
    3. Tightened `is_active` from `pub` to `pub(crate)` (`types/unified/mod.rs:83`) so only `state.rs:90` can mutate it.
  - **But**: `rg 'set_active\(' src/` returns only:
    - `src/tool_metadata/registry/state.rs:90` — the definition
    - `src/tool_metadata/registry/mod.rs:260` — the wrapper
    - `src/tool_metadata/types/unified/mod.rs:82` — doc comment pointing at `ToolCatalog::set_active`
    - `src/tool_metadata/registry/mod.rs:258-259` — doc comment mirroring the case-insensitive rationale
    - The unrelated `agent_env::set_active_agent` and `scratchpad_registry::set_active` are different methods on different types (no relation to `ToolCatalog::is_active`).
  - **Evidence**: A grep across the worktree for `set_active`/`set_inactive` returns 30 hits, but every one outside `src/tool_metadata/registry/` is `set_active_agent`, `embedding_providers::handle_set_active`, or `scratchpad_registry::set_active`. The `ToolCatalog::set_active` method is unwired above the registry module — no slash command, no UI handler, no IPC method, no test (the field is mutated directly in tests, not via the catalog wrapper).
  - **Downstream consequence**: Operators cannot pause a tool from any surface (no slash command, no `/pause <tool>` exists in `register_builtin_tools`). The health cache invalidation logic added at `registry/mod.rs:263` never runs. The doc comment at `registry/mod.rs:247-259` describes a "hot-pause a tool — including its descendants' routing — without un-registering it" workflow that the surface does not expose.
  - **Suggested fix**: Wire `set_active` into at least one surface — the natural shape is a new `builtin:tool_pause` + `builtin:tool_resume` slash command in `register_builtin_tools` (mirroring `cron_manage`), or an admin-only gateway handler under `src/gateway/handlers/admin.rs`. Until something calls it, the method is dead weight and the field is effectively read-only again (a different kind of broken than round-8's, but broken in the same way the user-visible impact).

---

## Warning

- **W-CONF-1** [testing, synthetic concurrency] `src/tool_metadata/loom_concurrency.rs:50-90` — `loom_engine_pause_resume_cancel` ends in a meaningless assertion; 4 loom tests cover primitives the production code does not use
  - **Round-8 status**: W2 STILL PRESENT (file untouched — `git log --since='2026-08-29' -- src/tool_metadata/loom_concurrency.rs` returns zero commits)
  - **Description**: Re-read the file in full. Four `#[test]` functions, each using `loom::model`. None of the four primitives appears in production `tool_metadata/`:
    - `loom_registry_concurrent_read_write` uses `loom::sync::RwLock<HashMap<String,u64>>` — production uses `tokio::sync::RwLock<HashMap<String,UnifiedTool>>`.
    - `loom_engine_pause_resume_cancel` uses `loom::sync::Arc<AtomicBool>` — `rg 'AtomicBool' src/tool_metadata/` returns zero hits.
    - `loom_atomic_counter_monotonic` uses `AtomicU64::fetch_add(Ordering::SeqCst)` — production uses `Acquire`/`Release` (health.rs:144, 160, 193, 267).
    - `loom_progress_snapshot` uses `loom::sync::RwLock<(u32,u32)>` — no production equivalent.
  - **The always-true assertion** at line 67 (`assert!(cancelled.load(Ordering::SeqCst));`) is preserved: under one writer and one reader both running deterministically under loom, `cancel_flag.store(true, Ordering::SeqCst)` is the only thing the cancel thread does — the assertion always holds regardless of whether the model interleaves pause around it. This was flagged in round-8 and not fixed.
  - **Evidence**: Full file `src/tool_metadata/loom_concurrency.rs`, lines 1-119; `git log --since='2026-08-29' -- src/tool_metadata/loom_concurrency.rs` empty.
  - **Suggested fix**: Either rewrite against `tokio::sync::RwLock` semantics (using `loom::future::block_on`) so the tests cover the actual lock the production code uses, or move the file out of `tool_metadata/` into a generic concurrency-test crate that does not claim to validate this module's invariants. At minimum, replace the always-true assertion with one that holds only when the model has interleaved the pause between cancel_flag and the wakeup.

- **W-CONF-2** [dead code, constants] `src/tool_metadata/constants.rs:16-34` — 9 of 13 constants are dead (refined from round-8's "9 of 11")
  - **Round-8 status**: W3 SHARPENED — round-8 said "9 of 11"; current file has 13 constants, 2 of the 4 additions are live, leaving the same 9 dead and 4 alive.
  - **Live (2 of 4 new + 0 of 11 original)**: `DEFAULT_CODE_EXEC_TIMEOUT` (constants.rs:22) — used at `src/builtin_tools/code_exec.rs:36, 367` (and the doc at `:63, :2181`, `bash_exec.rs:37`). `DEFAULT_MAX_TOKENS` (constants.rs:38) — used at `src/providers/protocols/anthropic/adapter.rs:24, 302` and `src/providers/protocols/gemini/adapter.rs:9, 72`.
  - **Dead (all 11 round-8-listed, plus nothing new is dead this round)**: `DEFAULT_MAX_FILE_SIZE` (constants.rs:16), `DEFAULT_SANDBOX_ENABLED` (constants.rs:18), `DEFAULT_ALLOW_NETWORK` (constants.rs:20), `DEFAULT_REQUIRE_CONFIRMATION_FOR_WRITE` (constants.rs:24), `DEFAULT_REQUIRE_CONFIRMATION_FOR_DELETE` (constants.rs:26), `DEFAULT_FILE_OPS_ENABLED` (constants.rs:28), `DEFAULT_CODE_EXEC_ENABLED` (constants.rs:30), `DEFAULT_CODE_EXEC_RUNTIME` (constants.rs:32), `DEFAULT_PASS_ENV` (constants.rs:34) — zero references outside their definitions.
  - **Comment block at constants.rs:6-13** (preserved from round-8) explains that `REQUIRE_CONFIRMATION` / `MAX_PARALLELISM` were retired because the keys "self-declared 'legacy, ignored'" — but the comment is silent on the 9 still-listed security-defaults that nothing reads. Either the constants should be deleted (`#[allow(dead_code)]` is not even suppressing them; they are `pub` so the lint passes, the symbol ships), or the corresponding config keys should be re-introduced as deprecated-but-allowed.
  - **Evidence**: `rg -n 'DEFAULT_(MAX_FILE_SIZE|SANDBOX_ENABLED|ALLOW_NETWORK|REQUIRE_CONFIRMATION_FOR_WRITE|REQUIRE_CONFIRMATION_FOR_DELETE|FILE_OPS_ENABLED|CODE_EXEC_ENABLED|CODE_EXEC_RUNTIME|PASS_ENV|MAX_TOKENS|CODE_EXEC_TIMEOUT)' --type rust -g '!target/**'` returns only the definition site for each of the 9 dead constants and live call-sites for the 2 new ones.
  - **Suggested fix**: Delete the 9 dead `pub const`s. They pollute the module's public surface, encourage cargo-culted reads (`grep DEFAULT_FILE_OPS_ENABLED`, "oh, it's there, use it"), and provide false reassurance that a global default exists for a key nothing reads.

- **W-CONF-3** [correctness, collision handling] `src/tool_metadata/registry/conflict.rs:244-258` — duplicate id still LWW; improved audit log only
  - **Round-8 status**: W4 PARTIAL FIX (a350419d5 H1 added `prev_id`/`prev_name`/`prev_source` to the warn! call, but LWW semantics are unchanged)
  - **Description**: `register_with_conflict_resolution` still uses `HashMap::insert` at the id-collision arm. The improvement is in the log line — operators can now see which id lost and to which row — but the resolution is the same: the new row silently overwrites the old, no event surfaces anywhere the boot sequence can act on.
  - **Evidence**: `src/tool_metadata/registry/conflict.rs:244-258` — the duplicate-id arm logs `prev_id`, `prev_name`, `prev_source` (per a350419d5 H1 commit message) and inserts. Comment at lines 247-249 reads "we choose to keep boot forward progress rather than erroring" — a deliberate design decision, but one round-8 still flagged because a downstream component that wants to detect "I lost a tool" has no signal. The deterministic id derivation comment at lines 236-244 (`skill:{id}`, `plugin:{plugin_id}:{tool_name}`, `format_tool_id`) acknowledges the prevention strategy but does not eliminate the case where two sources hash to the same id.
  - **Suggested fix**: Either accept LWW and downgrade the warn to debug (the audit log is for forensics, not operation), or surface a `DuplicateId { id, prev_source, new_source }` event through the existing `EventBus` so the gateway can route it into the panel/health surface.

- **W-CONF-4** [UX, rename inconsistency] `src/tool_metadata/registry/conflict.rs:198` vs `:227` — `RenameExisting` shows `(renamed)` while `RenameNew` shows `(MCP)`/`(Skill)`/etc.
  - **Round-8 status**: W5 STILL PRESENT, NOT REFACTORED
  - **Description**: `RenameExisting` arm at conflict.rs:192-216 writes `existing.display_name = format!("{new_name} (renamed)")` at line 198. `RenameNew` arm at lines 217-235 writes `tool.display_name = format!("{} ({})", new_name, tool.source.label())` at line 227, where `ToolSource::label()` returns `"MCP"`, `"Skill"`, `"Plugin"`, `"Builtin"`, `"Native"`, `"Custom"`. So a `/foo` slot surfaces as either `foo (renamed)` or `foo (MCP)` — two different parenthetical conventions for the same outcome.
  - **Evidence**: Lines 198 and 227 of `src/tool_metadata/registry/conflict.rs`; no commit since round-8 touched these arms (`git log -- src/tool_metadata/registry/conflict.rs | grep rename` returns only round-8).
  - **Suggested fix**: Pick one — `"foo (renamed)"` reads better but loses the source tag, `"foo (MCP)"` preserves provenance. The latter is the existing convention for `RenameNew`; switching `RenameExisting` to `"foo (renamed → MCP)"` would unify on the parenthetical-source tag without changing semantics.

- **W-CONF-5** [dead code, query API] `src/tool_metadata/registry/mod.rs:325` — `ToolCatalog::search` has zero production callers
  - **Round-8 status**: W10 STILL PRESENT
  - **Description**: `ToolCatalog::search` (registry/mod.rs:325) delegates to `ToolQuery::search` (query.rs:382-405), which fuzzy-matches name and description. Grep across the worktree returns only:
    - `src/tool_metadata/registry/mod.rs:325` — the wrapper definition.
    - `src/tool_metadata/registry/tests.rs:286` — `let results = registry.search("search").await;`.
  - The single occurrence in `src/search/registry.rs:726` is `.map(|q| async move { self.search(q, options).await })` — but that is `SearchRegistry::search`, an unrelated method on a different type.
  - **Evidence**: `rg 'ToolCatalog::search|catalog\.search\b|tools\.search\b|self\.search\(' --type rust -g '!target/**' src/tool_metadata/` returns only the two `tool_metadata` lines; a broader grep `rg '\.search\(' src/tool_metadata/` confirms `ToolQuery::search` is exclusively called by `ToolCatalog::search` and tests.
  - **Suggested fix**: Either wire it into a discoverable surface (the iMessage quick-picker, the panel search bar), or delete it. A test-only public API on `ToolCatalog` is a maintenance hazard (signature changes are silent because nothing else observes them).

- **W-NEW-1** [correctness, partial-fix consequence] `src/tool_metadata/registry/conflict.rs:21-34` — `infer_visible_channels` gates on fields that are uniformly default for 4 of 5 sources
  - **Round-8 status**: NEW (round-8 surfaced the consequence in the Critical text but did not file it as a separate finding because the cause was the same C1; this is the descendant of C-NEW-1's partial fix)
  - **Description**: `infer_visible_channels` returns `vec![Panel, Cli]` for `IrreversibleHighRisk`, `vec![Panel, Telegram, Discord, Cli]` for `requires_confirmation`, and `Vec::new()` otherwise. Because C-NEW-1 is only fixed on the MCP path, every non-MCP row falls into the `_ => Vec::new()` arm and is visible to every channel — including iMessage, which has no confirmation UI. The doc comment at conflict.rs:7-13 explicitly states the intent ("Applied uniformly to every tool registered through `ConflictResolver::register_with_conflict_resolution`"), but the data feeding the function is not uniform.
  - **Evidence**: `src/tool_metadata/registry/conflict.rs:21-34`; combined with C-NEW-1's registration.rs path audit.
  - **Suggested fix**: Resolved by C-NEW-1's fix. Listing this separately so the surface area of "partial C1" is explicit — fixing only the builder calls without fixing the consumer would leave the intent wired to nothing.

- **W-NEW-2** [correctness, surface gap] `src/tool_metadata/registry/mod.rs:247-267` — `set_active` workflow described in doc has no caller
  - **Round-8 status**: NEW (the doc describing the operator workflow is new; round-8 did not see it because the method did not exist)
  - **Description**: The doc comment at registry/mod.rs:247-259 reads "Inactive tools are excluded from every list / `find_best_match` query path ... so an operator can hot-pause a tool — including its descendants' routing — without un-registering it." This describes a workflow that does not exist anywhere in `src/`. No slash command, no admin endpoint, no IPC method, no test exercises the workflow. The doc itself is the only place the workflow lives.
  - **Evidence**: Same as C-NEW-2. Listed separately because the doc/code drift is itself a hazard — a future contributor reading the comment will trust that "this is how it works" and build a higher layer against an invariant that the rest of the system never enforces.
  - **Suggested fix**: Either trim the doc to "method exists; not wired to any UI", or wire the workflow (see C-NEW-2).

---

## Suggested Test

- **ST-CONF-1** [test gap, C1] — Cross-path coverage of safety/confirmation propagation
  - **Round-8 status**: W2/W3 test gap carried forward
  - **Description**: Existing test in `types/unified/tests.rs:49, 95, 103` covers `with_safety_level`/`with_requires_confirmation` on a single tool. No test exercises the end-to-end flow "register a builtin tool with the right safety/confirmation defaults, then read it back via `list_for_channel(iMessage)` and confirm the iMessage-only branch is empty." Adding one would catch C-NEW-1 the moment any future refactor breaks the wiring.
  - **Suggested test location**: `src/tool_metadata/registry/tests.rs`, alongside `test_register_builtin_tools` (line 18).
  - **Concrete shape**: build a `ToolCatalog`, call `register_builtin_tools`, assert each of the 9 entries has `safety_level` and `requires_confirmation` set per a documented table, then call `list_for_channel(ChannelType::IMessage)` and assert the expected subset (e.g. nothing for the current default). The table is what C-NEW-1 needs anyway.

- **ST-NEW-1** [test gap, C2 / hot-pause workflow] — End-to-end `set_active` round-trip
  - **Round-8 status**: NEW
  - **Description**: No test exercises `ToolCatalog::set_active(name, false)` → `list_all()` excludes the tool → `set_active(name, true)` → `list_all()` includes it again. The only place the field is mutated in tests is `tools/adapters/registry_adapter.rs:1209` (which writes the field directly, bypassing the catalog wrapper, so it would not catch a regression in the lock-protected path or the case-insensitive lookup that `7660b5af5` added).
  - **Suggested test location**: `src/tool_metadata/registry/tests.rs`, near the existing `unregister_skills` test (line 1096).
  - **Concrete shape**: register `skill_read`, assert `list_all().iter().any(|t| t.name == "skill_read")` is `true`; call `catalog.set_active("SKILL_READ", false)` (uppercase, to exercise case-insensitivity), assert `list_all()` no longer contains it; call `catalog.set_active("skill_read", true)`, assert it returns. Also assert the health cache is invalidated (a probe that registered before the pause now reports stale and re-fetches).

---

## Per-perspective (lower confidence)

- **Security**: `infer_visible_channels` (conflict.rs:21-34) is a defense-in-depth control — it gates dangerous tools to Panel/CLI and excludes confirmation-required tools from iMessage. C-NEW-1 + W-NEW-1 effectively bypass that control for 4 of 5 registration paths. Severity stays "Warning" rather than escalating to a second Critical because the MCP path is the one most likely to surface destructive operations (`delete_repo`, etc.) — the 4 other paths are mostly read or routing helpers — but a future refactor that adds a destructive skill/plugin/custom command will silently expose it to iMessage.
- **Logic**: The single-pass refactor (bca0e8a6f) preserved the three-tier precedence (`canonical > alias > normalized`) and the comparator (`source.priority`, then `id` desc, then `max_by` keeps the last equal). Re-tracing `find_best_match` (query.rs:184-261) confirms tier 0 → tier 1 → tier 2 ordering is honored via `best[0].or(best[1]).or(best[2])`. No logic drift.
- **Architecture**: The five registration paths (builtin / skill / plugin / custom / mcp) live in two different crates (`tool_metadata::registry::registration` for four, `tools::handlers::registration` for MCP). Splitting safety/confirmation across crates makes the C-NEW-1 partial fix structurally likely — fixing it requires pulling safety policy up into a shared helper, which is what C-NEW-1's suggested fix proposes.
- **Quality**: `UnifiedTool::new` (types/unified/mod.rs:243) sets every default explicitly. The fluent setters each call `Self` and return it. The new `types/unified/conversions.rs` (added since round-8) was not deeply reviewed — it is a serialization helper, not part of the mutation or query surface, and the tests around it (registry/tests.rs, types/unified/tests.rs) pass. No findings.
- **Quality (2)**: The new files `types/safety.rs`, `types/conflict.rs`, and `registry/state.rs` (each added since round-8) are clean, well-documented, and contain no `unsafe`. `state.rs:90` correctly acquires the write lock and uses the case-insensitive match per the doc comment.
- **Quality (3)**: The dead-constant count (9 of 13) is unchanged in direction (still majority dead) even though 2 new constants were added live. This suggests the file is being curated as "constants for future use" rather than "constants in active use" — the doc comment at constants.rs:1-13 (`//! Security-enforced constants that are not user-configurable.`) supports that interpretation but does not excuse the absence of `#[allow(dead_code)]` warnings that would otherwise have surfaced the gap.

---

## Conclusion

- **Net delta from round-8**: 4 of 11 warnings fixed (W8, W9, W12, W13), 4 still present unchanged (W2, W5, W10, W11), 1 partially fixed (W4), 1 refined (W3). Both Criticals partially fixed: C1 wired in 1 of 5 registration paths, C2 mechanism added but uncalled. The round-8 follow-up commit `8d1eeb497` addressed the literal round-8 text but left the structural gap alive.
- **What holds**: The query surface (query.rs), conflict-resolver single-pass (conflict.rs), InflightGuard RAII in health.rs (commit a9c70a2ab), the `pub(crate)` tightening on `is_active`/`requires_confirmation`/`safety_level` (commit a350419d5), the case-insensitive `set_active` (commit 7660b5af5), and the `unregister_skills` disposer path (commit 60cde6871) are all real improvements that survive round-9 verification. The new `types/safety.rs`, `types/conflict.rs`, `registry/state.rs`, `types/unified/conversions.rs` are clean.
- **What drifts**: Two structural gaps that the round-8 follow-up commit opened rather than closed:
  1. Safety/confirmation is now only derived on the MCP path; the other four paths silently default to "ReadOnly, no confirmation, visible to all channels".
  2. The `set_active` mechanism exists, is lock-protected, and has a polished doc comment, but no caller exercises the workflow the doc describes.
- **Fix order proposal** (cheapest + highest-impact, metadata-side only):
  1. **Delete the 9 dead constants in `constants.rs:16-34`** (W-CONF-2): zero-risk, ~10 LOC, removes a misleading public surface. ~30 min.
  2. **Add a `populate_safety_profile` helper called by all five registration paths** (C-NEW-1): centralizes the policy, fixes `infer_visible_channels` for non-MCP sources, enables ST-CONF-1. ~2-4 hr depending on how much existing metadata is already parsed.
  3. **Wire `ToolCatalog::set_active` into a slash command + admin handler** (C-NEW-2 / W-NEW-2): turn the documented workflow into a real one. ~1-2 hr for the slash command path; admin IPC if needed is incremental.
- **Do not fix in isolation**: W-CONF-1 (loom tests) and W-CONF-5 (`ToolCatalog::search` dead) are real findings but addressing them changes tests, not behavior. They are correctly deferred behind the behavioral fixes.
