# P2 — Prompt Assembly YAGNI Retraction & Dead-Module Deletion

**Status**: Design approved 2026-04-25
**Phase**: P2 of harness-dissolution roadmap
**Branch**: `harness-dissolution` (worktree at `/Volumes/TBU4/Workspace/Aleph.harness-dissolution`)
**Parent roadmap**: `docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md`
**Predecessors**: P0 (slim harness, merged), P1 (context consolidation), P3 (guardrails retraction), P4 (verification minimization)

---

## 1. Goal

Retract the roadmap's original P2 plan ("3-way merge `src/thinker/` + `src/prompt/` + `src/harness/sections/` into `src/prompt_assembly/`, introduce `PromptAssembler` / `Section` / `PromptLayer` trait contracts") and replace it with concrete deletions:

1. **Delete `src/prompt/`** (5 files, 747 lines) — dead code built on the removed `UnifiedIntentClassifier`.
2. **Delete `src/payload/`** (13 files, ~1,700 lines) — `PromptAssembler` only called from own tests; closed loop with capability.
3. **Delete `src/capability/`** (10 files, ~2,500 lines) — `CapabilitySystem::execute()` only called from own tests; type-name collision with live `extension::capability::CapabilityDeclaration` is harmless.
4. **Delete `src/prompt_assembly/`** (10 files, 289 lines) — orphan scaffolding created in P0 awaiting wiring that P2 now retracts.
5. **Document the retraction** in the parent roadmap (footnote ⁴ alongside ¹/²/³).

This makes P2 the fourth retraction-class phase, following P1 (`compressor`), P3 (`permission`), P4 (`VerifyStopHook`).

### Anti-goals

- **No** `src/thinker/` → `src/prompt_assembly/` rename. 30 files + 200+ import sites of pure naming churn; the historical name stays.
- **No** `PromptAssembler` / `Section` / `PromptLayer` trait introduction. `src/thinker/prompt_layer.rs:PromptLayer` + `prompt_pipeline.rs:PromptPipeline` already provide the de facto abstraction.
- **No** wiring of `prompt_assembly/sections/*.md` content into the live `thinker/prompt_builder/sections.rs`. Hardcoded string concatenation is the canonical R10 ("Intelligence in Prompt") carrier; introducing `include_str!` indirection is YAGNI.
- **No** changes to `src/thinker/` (18,464 lines, 40+ files — the real prompt assembly workhorse) or `src/intent/` (~150 lines, type-only, gateway uses `IntentResult` + `DirectToolSource` for slash command metadata).

---

## 2. Code Census

Brainstorm-time audit of the five candidate modules.

### 2.1 `src/prompt/` — DEAD

| Aspect | Finding |
|--------|---------|
| Size | 5 files / 747 lines (`builder.rs` 220, `conversational.rs` 125, `executor.rs` 212, `mod.rs` 35, `templates.rs` 155) |
| Responsibility (claimed) | "Unified prompt management module" — `PromptBuilder::executor_prompt()` / `conversational_prompt()` / `direct_tool_prompt()` |
| External consumers | Only `src/payload/assembler/{mod.rs:52, mod.rs:54, intent.rs:9}`, both of which are themselves dead. |
| Design dependency | Module doc declares it builds on `UnifiedIntentClassifier`, which was already removed (`src/intent/mod.rs` comment: *"The detection/classification pipeline has been removed in favor of LLM-native tool selection via the minimal agent loop. Only shared type definitions remain."*) |
| Name-collision check | `crate::prompt::PromptBuilder` ≠ live `crate::thinker::prompt_builder::PromptBuilder`; both names coexist as independent types. Deletion of the former does not touch the latter. |

**Verdict**: Delete entire directory.

### 2.2 `src/payload/` — DEAD

| Aspect | Finding |
|--------|---------|
| Size | 13 files / ~1,700 lines (`builder.rs` 281, `mod.rs` 244, `capability.rs` 9, `context_format.rs` 69, `intent.rs` 146, `assembler/{capability.rs 139, context.rs 97, core.rs 187, formatters.rs 255, intent.rs 71, mod.rs 540, tools.rs 45}`) |
| Responsibility (claimed) | "Structured context protocol for Agent" — `AgentPayload`, `PayloadBuilder`, `PromptAssembler` (distinct from any other `PromptAssembler`) |
| Production reachability | `PromptAssembler::build_prompt_with_intent_result` is only called from `src/payload/assembler/mod.rs:504, 522, 535` — all three are unit tests. `PromptAssembler::new(ContextFormat::*)` has 9 call sites, all inside `src/payload/assembler/mod.rs` test functions. `PayloadBuilder::new()` has zero production call sites (all tests). |
| Closed loop | `crate::payload::*` external consumers are limited to `src/capability/{system,strategy,mod}.rs` and `src/capability/strategies/*.rs` (themselves dead) plus `src/config/types/routing.rs:220, 244` (inside test functions). |

**Verdict**: Delete entire directory.

### 2.3 `src/capability/` — DEAD

| Aspect | Finding |
|--------|---------|
| Size | 10 files / ~2,500 lines (`mod.rs`, `declaration.rs`, `request.rs`, `response_parser.rs`, `strategy.rs`, `system.rs`, `strategies/{mod, mcp, memory, skills}.rs`) |
| Responsibility (claimed) | "AI-first intent detection and capability execution" — `CapabilitySystem`, `CapabilityExecutor`, `CapabilityStrategy`, `CapabilityDeclaration` |
| Production reachability | `CapabilitySystem::new()` has 3 call sites, all in `src/capability/system.rs` self-tests. `CapabilityExecutor` is only constructed in `src/capability/mod.rs` self-tests. `CapabilityStrategy::execute()` impl call sites are in `src/capability/strategies/*.rs` self-tests. |
| Name-collision check | `crate::capability::CapabilityDeclaration` (struct, AI-intent flavor) ≠ `crate::extension::capability::CapabilityDeclaration` (enum, plugin-registry flavor). The enum is live (used by `extension/registrar/mcp_registrar.rs`, `extension/types/plugins.rs`). Deletion of the struct does not touch the enum. |
| Stray reference | `src/search/provider.rs:11` has one doc comment line: *"to provide a consistent API to the CapabilityExecutor."* This is a dangling reference to the deleted module, requires a one-line edit. |

**Verdict**: Delete entire directory + edit one comment line.

### 2.4 `src/prompt_assembly/` — ORPHAN SCAFFOLDING

| Aspect | Finding |
|--------|---------|
| Size | 10 files / 289 lines (`mod.rs` 7, `sections/mod.rs` 172, plus 5 static `.md` files: `task_philosophy`, `risk_actions`, `tool_grammar`, `output_style`, `persistence`, plus 3 guidance `.md` files: `browser`, `code_exec`, `subagent`) |
| Origin | P0 created this as relocation scaffolding. `mod.rs` literally says: *"Full trait design and consolidation with src/thinker/ and src/prompt/ is deferred to P2 (P2-prompt-assembly). This directory currently hosts only the section content moved out of src/harness/."* |
| External consumers | **Zero**. `grep -rn 'crate::prompt_assembly' src/` excluding the directory itself returns no matches. |
| Future consumer | P2 brainstorm decided not to wire these `.md` files into `src/thinker/prompt_builder/sections.rs` (R10: hardcoded string concat is the canonical prompt-template carrier). The scaffolding has no future. |

**Verdict**: Delete entire directory (retracts the original "3-way merge → src/prompt_assembly/" plan).

### 2.5 Modules NOT touched

| Module | Reason |
|--------|--------|
| `src/thinker/` (18,464 lines, 40+ files) | Real prompt assembly workhorse: 32 `layers/*.rs` section files + `PromptPipeline` + `PromptBuilder` + `PromptLayer` trait + soul/identity/context/memory_context_provider. External consumers across the codebase. The historical name `thinker` is misleading under R8 ("thinking belongs to the LLM, not the assembler"), but the rename is a 30-file + 200+-import refactor with zero behavioral payoff — explicitly retracted. |
| `src/intent/` (~150 lines, type-only) | After deleting `prompt/payload/capability`, the only surviving external consumer is `src/gateway/inbound_router/command_handler.rs:8` (`IntentResult` + `DirectToolSource` for serializing slash command metadata into `RunRequest`). Module is now clean and minimal. |
| `src/extension/capability::CapabilityDeclaration` | Live enum (plugins, mcp_registrar). Only shares the name with the dead `src/capability/CapabilityDeclaration` struct; no structural relationship. |

---

## 3. Key Decisions

| ID | Decision | Rationale |
|----|----------|-----------|
| D1 | Delete `src/prompt/` (5 files, 747 lines) + `src/lib.rs:73` | Sole consumers are dead; built on removed `UnifiedIntentClassifier` |
| D2 | Delete `src/payload/` (13 files, ~1,700 lines) + `src/lib.rs:71` | Test-only `PromptAssembler` and `PayloadBuilder`; closed loop with capability |
| D3 | Delete `src/capability/` (10 files, ~2,500 lines) + `src/lib.rs:43` | Test-only `CapabilitySystem`; harmless name collision with live `extension::capability::CapabilityDeclaration` |
| D4 | Delete `src/prompt_assembly/` (10 files, 289 lines) + `src/lib.rs:74` | Zero external consumers; P0 scaffolding awaiting P2 wiring that's now retracted |
| D5 | Edit `src/search/provider.rs:11` — remove `CapabilityExecutor` reference from doc comment | Dangling reference after Commit 1 |
| D6 | Keep `src/thinker/` unchanged (no rename) | Pure naming refactor; 30 files + 200+ import sites; violates P3 "deletion > relocation" lesson |
| D7 | Keep `src/intent/` unchanged | Gateway uses `IntentResult` + `DirectToolSource` for slash command metadata serialization |
| D8 | Retract `PromptAssembler` / `Section` / `PromptLayer` trait introduction | `thinker::prompt_layer::PromptLayer` + `prompt_pipeline::PromptPipeline` are already the de facto abstraction; adding a parallel layer violates R3 + R8 + R10 |
| D9 | Single atomic commit for all deletions (rather than 4 separate commits) | The 4 directories form a closed dead-code graph — they break together and stay green together. Splitting into 4 commits adds no verification value (no broken intermediate states). |
| D10 | Separate Commit 2 for roadmap close-out | Same pattern as P3 (Commit 1: source deletion, Commit 2: docs); keeps commit topics clear |

---

## 4. Commit Plan

Two atomic commits on `harness-dissolution`. Same verification bar as P0/P1/P3/P4: `cargo check` + `cargo clippy -- -D warnings` after each commit, inheriting pre-existing P0 clippy exemptions. Single-line commit messages required (block-no-verify@1.1.2 hook false-positives on multi-line bodies).

### Commit 1 — `cleanup: delete dead prompt/payload/capability/prompt_assembly modules (~5,200 LOC)`

**Deletions**:
- `src/prompt/` (entire directory)
- `src/payload/` (entire directory)
- `src/capability/` (entire directory)
- `src/prompt_assembly/` (entire directory)

**Modifications**:
- `src/lib.rs` — remove 4 lines:
  - line 43 `pub mod capability;`
  - line 71 `pub mod payload;`
  - line 73 `pub mod prompt;`
  - line 74 `pub mod prompt_assembly;`
- `src/search/provider.rs:11` — delete the doc comment line referencing `CapabilityExecutor` (the `SearchProvider` trait survives unchanged; only the dangling reference is removed)

**Verification**:
- `cargo check -p alephcore` → green (4 dead modules form closed loop with zero live consumers)
- `cargo clippy -- -D warnings` → green (P0 exemptions inherited; deletion may reduce warnings)
- `grep -rn 'crate::prompt::\|crate::payload::\|crate::capability::\|crate::prompt_assembly::' src/` → returns only unrelated matches via `extension::capability::*` (untouched)
- `git diff --stat` → expect ~38 files / ~5,200 lines deleted

### Commit 2 — `docs(spec): mark P2 complete; record YAGNI retraction in roadmap`

**Modifications**:
- `docs/superpowers/specs/2026-04-24-harness-dissolution-roadmap.md`:
  - **§3.3 row 5** (Prompt Assembly) — append `⁴` to "Final Directory" cell; rewrite "Action" cell to: `Dead code in prompt/payload/capability deleted; src/prompt_assembly/ scaffold removed; src/thinker/ kept (rename retracted)`
  - **§4.2 P2 row** — Risk 🟡 Medium → 🟢 Low⁴, Estimate `2 weeks` → `1 day⁴`, Exit Artifact → `prompt/payload/capability/prompt_assembly deleted (~5,200 LOC); facade trait plan retracted⁴`
  - **§6 open question 2** — mark ✅ Resolved (2026-04-25); record that thinker rename was also retracted
  - **§7 status table P2 row** — `📋 Planned` → `✅ Complete | 2026-04-25 | 2026-04-25 | [design](../specs/2026-04-25-p2-prompt-assembly-design.md) | (no plan needed — see commit log)`
  - Insert footnote ⁴ after footnote ³ (draft below)

**Verification**:
- `cargo check` not affected (docs-only)
- Visual diff review of roadmap

### Footnote ⁴ draft

> ⁴ **P2 YAGNI retraction (2026-04-25)**: P2 brainstorm audited the three locations the roadmap proposed to merge (`src/thinker/`, `src/prompt/`, `src/harness/sections/`) plus two adjacent modules surfaced during the audit (`src/payload/`, `src/capability/`). Findings: (a) `src/prompt/` (747 lines) was dead — its sole consumer chain `payload::assembler::intent` was itself dead, and it was built on the already-removed `UnifiedIntentClassifier`; (b) `src/payload/` (~1,700 lines) and `src/capability/` (~2,500 lines) form a closed loop of test-only code — `PromptAssembler::build_prompt_with_intent_result` and `CapabilitySystem::execute()` are only called from their own unit tests; (c) `src/prompt_assembly/` (289 lines) was orphan scaffolding created in P0 awaiting P2 wiring, with zero external consumers; (d) `src/harness/sections/` was already moved during P0 (see §3.2). All four were deleted per the P1 (`compressor`)/P3 (`permission`)/P4 (`VerifyStopHook`) precedent (~5,200 LOC removed). The `PromptAssembler` / `Section` / `PromptLayer` trait commitment was retracted: `src/thinker/prompt_layer.rs:PromptLayer` + `prompt_pipeline.rs:PromptPipeline` already provide the de facto abstraction, and adding a parallel trait layer violates R3 (Core Minimalism) + R8 (LLM Sovereignty) + R10 (Intelligence in Prompt). The thinker→prompt_assembly directory rename (open question 2) was also retracted: a pure naming refactor touching 30 files + 200+ imports is not worth the diff cost. `src/thinker/` stays as the canonical prompt assembly home under its historical name. `src/intent/` (~150 lines, type-only) stays for gateway slash-command metadata serialization. Risk downgraded 🟡 Medium → 🟢 Low; estimate shortened 2 weeks → 1 day. See P2 design §2–§3 for details.

---

## 5. Verification

Same bar as P0/P1/P3/P4:
- `cargo check -p alephcore` green after each commit
- `cargo clippy -- -D warnings` green after each commit (P0 exemptions inherited; no new warnings)
- `grep -rn 'crate::prompt::\|crate::payload::\|crate::capability::\|crate::prompt_assembly::' src/` returns no matches except unrelated `extension::capability::*` paths after Commit 1
- No `git push`. No `merge` to main. Commits stay on `harness-dissolution`.

---

## 6. Risks & Rollback

**Risk level**: 🟢 Low (matches P1, P3, P4 retraction-class phases).

**Identified risks**:

1. **Hidden runtime consumer via `dyn` dispatch** — None of the four deleted modules expose a trait that's stored as a trait object across module boundaries. All consumers are statically-resolved. `cargo check` green is therefore a sufficient soundness proof.

2. **Test code dependency** — All internal tests are deleted with their respective modules. External tests across `src/` were verified to have zero references to the four module roots (`grep -rn 'crate::{prompt,payload,capability,prompt_assembly}::'` returns only the test code being deleted).

3. **Downstream FFI / public API** — Aleph compiles to a binary (`aleph-server`). No `lib.rs` re-exports leak these types to external crates. No FFI surface.

4. **Name-collision false positive** — `crate::capability::CapabilityDeclaration` and `crate::extension::capability::CapabilityDeclaration` share a name but are distinct types in distinct paths. Deleting the former leaves the latter (used by plugins/mcp_registrar) intact.

**Rollback**: `git revert <commit-1-sha>` restores all four deleted directories exactly as they were. Each directory's history is preserved at the commit-1 parent.

---

## 7. Sequence

1. ✅ Brainstorm + design (this document)
2. → Plan written to `docs/superpowers/plans/2026-04-25-p2-prompt-assembly.md` (next step — covers the 2 commits)
3. → Implementation: 2 commits via subagent-driven-development on `harness-dissolution`
4. → User decides merge timing for `harness-dissolution` after P2 lands
