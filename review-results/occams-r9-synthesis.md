# occams-r9 Synthesis & Fix Plan (2026-10-04)

## Modules reviewed

| Module | Critical | Warning | Suggested Test | Round-8 Status |
|--------|---------:|--------:|---------------:|----------------|
| `src/tools` | 0 | 10 | 0 | 3 round-8 Criticals all FIXED |
| `src/thinker` | **1 (NEW)** | 11 | 5 | Both round-8 Criticals FIXED |
| `src/tool_metadata` | **2 (PARTIAL FIX)** | 6 + 2 NEW | 0 | Both round-8 Criticals PARTIAL FIX |
| `src/tool_output` | 0 | 9 | 4 | 15 of 25 round-8 Warnings FIXED |
| `src/utils` | 0 | 0 | 0 | Unusually strong; M1+L2 STILL DEFERRED |

## Critical findings

| ID | Module | File:Line | Pattern | Fix |
|----|--------|-----------|---------|-----|
| **A** | thinker | `src/thinker/prompt_size_registry.rs:155-175` | Race window claim is *technically not reachable* under current `Mutex<Inner>`; **test gap** is the real risk. | Add concurrent stress test + explicit atomicity doc comment |
| **B** | tool_metadata | `src/tools/handlers/registration.rs:140,146,155` | 4 of 5 registration paths build `UnifiedTool::new(...)` bare → defaults `safety_level=ReadOnly, requires_confirmation=false` → `infer_visible_channels` cannot exclude dangerous tools from iMessage | Extract `populate_safety_profile(&mut builder, source, meta)` helper; wire all 5 paths |
| **C** | tool_metadata | `src/tool_metadata/registry/mod.rs:260`, `state.rs:90` | `ToolCatalog::set_active` and `ToolState::set_active` exist with polished doc but **zero production callers**; only tests mutate | Trim doc to acknowledge test-only status; add TODO + design note |

## High-value Warnings (apply in same slice as their module's Critical where possible)

| ID | Module | Fix shape | Cost |
|----|--------|-----------|------|
| tool_output W-NEW-1 | tool_output | Drop redundant `base64_marker_count() >= 2` in inner disjunct | 15 min |
| tool_output W-CONF-3 | tool_output | `cap_line` on screenshot metadata branch | 30 min |
| tool_output W-NEW-4 | tool_output | Extract `INLINE_ERROR_SALIENT_LINES` constant | 15 min |
| tool_metadata W-CONF-2 | tool_metadata | Delete 9 dead constants if confirmed unused | 30 min |

## Deferred (this round)

- thinker W-NEW-2..8: design discussions (e.g. extractor shape), require planning
- tool_metadata W-NEW-1, W-NEW-2: design discussions (e.g. `infer_visible_channels` field uniformization)
- tool_output W-NEW-2, W-NEW-3, W-NEW-5, W-NEW-6: API contract nits, mostly doc
- utils M1 (scratch.rs SIGKILL on recycled PID): security-adjacent but reviewer recommended deferral — separate round with `libc::kill(pid,0) + start_time` patch
- utils L2 (atomic_write silent set_permissions): defensive, separate round
- tool_output W-CONF-1 (DCS/PM/APC strip), W-CONF-2 (total_lines blank), W-CONF-4 (size_hint undercount): tail-end correctness, defer

## Fix order (M3 reasoning, Jev failed on schema)

1. **Slice 1 — thinker (Fix A)**: narrowest blast radius; validates per-slice cargo check protocol
2. **Slice 2 — tool_metadata safety (Fix B)**: structural, widest blast radius, highest impact (security-adjacent)
3. **Slice 3 — tool_metadata set_active (Fix C)**: additive, low risk
4. **Slice 4 — tool_output improvements**: no Critical, but 3 trivial Warnings fit cleanly here
5. **Slice 5 — utils cleanup**: defensive M1 + L2 from `utils-2026-08-29.md` DEFERRED state

Each slice: rustfmt → `cargo check -p alephcore` → commit. Final: unified `cargo check -p alephcore` on main after merge.

## Tool-integration gap: Jev attempts blocked by schema

`/rust-occams-razor` calls for Jev on 分类/筛选/路由/简单判断. Attempted 4 `jev_evaluate` calls:

1. **First**: score criteria formatted as map (object with string keys 1-5) → schema rejected with "Score criteria must be a list of descriptions indexed by score from zero, not a map"
2. **Second**: nested `<item>` elements → schema rejected with "questions: must have required properties questions"
3. **Third**: nested `<item>` (different attempt) → same "list of descriptions, not a map" rejection
4. **Fourth**: minimal single-choice call → converter returned only the wrapper key as the option; Jev saw one option and picked it with confidence 1

**Root cause**: the XML-to-JSON converter wraps every `<option>`/`<item>` block as a JSON object key rather than an array element, so `<criteria><option>...</option></criteria>` becomes `{"option": "..."}` — never a list. There is no XML element name that the converter treats as an array element.

**Fallback**: M3 (this agent) used the same reasoning Jev would have applied — security-adjacent partial-fix-gap > correctness-narrow > additive-dead-code — to score priorities and pick fix order. The "M3 + Jev hybrid" split documented in the user policy is therefore M3-only for synthesis this round. **Recommend opening a ticket to fix the jev_evaluate schema wrapper** so the next round (occams-r10) can route the simple judgments through Jev as intended.

---

## occams-r9 Status (post-application, 2026-10-04)

| Slice | Module | Commit | Critical/Warning status |
|-------|--------|--------|--------------------------|
| 1 | thinker | f0a94d3f0 | C-NEW-1: race-window technical claim closed (Mutex serializes); test gap closed (`concurrent_writers_around_eviction_do_not_race`); atomicity doc added. |
| 2 | tool_metadata | 37e7e7dfc | C-NEW-1 PARTIAL: `populate_safety_profile` helper extracted; MCP path rewired. **4 other paths still unwired — see Slice 2b below.** |
| 3 | tool_metadata | da1db7eef | C-NEW-2 closed: doc on `ToolCatalog::set_active` no longer falsely claims an operator workflow; explicit "no production caller" + TODO(occams-r10+) added. |
| 4 | tool_output | 581091450 | W-NEW-1, W-NEW-4, W-CONF-3 all closed (3 cheap code-quality fixes). |
| 5 | utils | e2cfd9803 | M1 PARTIAL: `kill(pid, 0)` probe before SIGKILL closes the gone-pid case; recycled-pid-with-different-content edge case still tracked as follow-up (requires API change to carry start_time). L2 closed: warn!() instead of silent swallow. |

### Slice 2b — DEFERRED with rationale

The full fix for tool_metadata C-NEW-1 requires populating safety metadata for 4 non-MCP registration paths:

1. **Builtin tools** (~20+ `UnifiedTool::new` calls in `src/executor/builtin_registry/builder/constructor/{agent_acp_tools,collab_session_tools,coord_team_tools,core_tools,optional_tools}.rs`)
2. **Skill tools** (`src/extension/skill*.rs` — per-skill chain)
3. **Plugin tools** (`src/extension/lifecycle.rs` — manifest-driven)
4. **Custom commands** (`src/command/parser.rs:325` — bare)

**Investigation finding** (M3 reasoning): `ToolDefinitionMetadata` in `src/tools/service.rs:180` has no `risk_level` or `requires_confirmation` fields. The current shape carries `idempotent`, `max_duration_ms`, `concurrent_safe` — *operational* metadata, not *safety* metadata. To wire builtin tools, each tool's `definition()` would need to declare its own risk level (file_write → IrreversibleHighRisk, file_read → ReadOnly, etc.) AND each builtin constructor would need to call `populate_safety_profile(td.metadata.risk_level, td.metadata.requires_confirmation)`.

**Why deferred (not just "out of scope")**:
- Adding `risk_level` to `ToolDefinitionMetadata` touches every tool's `definition()` method
- Each tool's risk is currently *implicit* (file_write mutates, file_read doesn't); making it explicit is a per-tool semantic decision that needs a tool-by-tool review, not a mechanical sweep
- Skill / plugin / custom-command paths have heterogeneous metadata sources (skill annotation / manifest field / command declaration) that each need their own design

**Recommended next step (occams-r10+)**:
1. Add `risk_level: ToolSafetyLevel` + `requires_confirmation: bool` to `ToolDefinitionMetadata` (default `ReadOnly`/`false` for backward compat)
2. Walk every builtin tool's `definition()` and set the appropriate risk (file_write → IrreversibleHighRisk, bash → IrreversibleHighRisk, etc.)
3. Update builtin constructors to call `populate_safety_profile` after each `UnifiedTool::new`
4. Mirror the same for skill / plugin / custom-command paths

**What occams-r9 ships that enables this**:
- `populate_safety_profile` helper at `src/tool_metadata/types/unified/builders.rs` (commit 37e7e7dfc)
- Unit tests for the policy table (4 cases: read_only, mutating_no_confirm, mutating_with_confirm, edge_case)
- MCP path already wired and verified (round-8 → occams-r9 net change)

**Tracking**: TODO(occams-r10+) comment in `src/tools/handlers/registration.rs` next to the MCP path's populate_safety_profile call.

### Per-slice cargo check results

| Slice | Time | Result |
|-------|------|--------|
| 1 | 1m 08s | GREEN |
| 2 | 1m 18s | GREEN |
| 3 | 1m 01s | GREEN |
| 4 | 1m 06s | GREEN |
| 5 | 1m 05s | GREEN |

Final unified cargo check runs after all slices applied (see `occams-r9-final-cargo-check` log section below).

