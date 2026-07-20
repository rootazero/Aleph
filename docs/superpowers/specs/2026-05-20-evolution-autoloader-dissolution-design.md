---
title: Evolution AutoLoader Dissolution (YAGNI Withdrawal)
date: 2026-05-20
status: draft
spec_owner: Aleph
brainstorming_source: superpowers:brainstorming session 2026-05-20
related:
  - docs/reference/HARNESS_PHILOSOPHY.md (R10 YAGNI Withdrawal Pattern)
  - docs/plans/2026-03-11-cognitive-evolution-beta-design.md (the shelved upstream)
  - .claude/memory/project_skill_system_wiring_shipped.md (the deferred-item note)
follow_up:
  - Spec A — Skill data model unification (separate cycle)
  - Spec C — Host sandbox netns isolation (separate cycle)
---

# Evolution AutoLoader Dissolution (YAGNI Withdrawal)

## 1. Background

`docs/.../memory/project_skill_system_wiring_shipped.md` documented three deferred items after the 2026-05-20 skill-system wiring merge. One of them — **"EvolutionAutoLoader 实时触发点延后"** — was framed as *waiting for an Evolution upstream that does not yet exist*. This spec resolves that deferral.

### 1.1 Real situation (recon 2026-05-20)

The Evolution scaffolding in `main` is **doubly orphaned**:

| Link in the chain | State |
|-------------------|-------|
| `experience_replays` DB table + `evolution_status` column | Migration creates the table — **zero writers, zero readers** in `src/` |
| `EvolutionConfig` (`src/config/types/evolution.rs`) — thresholds, ToolGenerationConfig | Loaded into `Config.evolution` — **no code reads the thresholds** |
| Pattern detector / `experience_replays` writer | **Does not exist** |
| `SolidificationSuggestion` constructor (other than test fixtures) | **Does not exist** |
| `MarkdownSkillGenerator::generate()` | Implementation complete (`src/tools/markdown_skill/generator.rs`) — **zero callers** |
| `EvolutionAutoLoader::load_from_suggestion()` | Implementation complete + 4 unit tests (`src/tools/markdown_skill/auto_loader.rs`, 326 lines) — **zero callers** |
| `src/poe/` subsystem (referenced by 2026-03-11 design docs) | **Does not exist on disk** |
| Gateway `poe.sign.*` RBAC scope + `poe.run`/`poe.prepare` rate-limit lane | 3 declaration-level stubs — no RPC handlers behind them |

The `docs/plans/2026-03-11-cognitive-evolution-{beta,gamma,beta-completion}-{design,impl}.md` and `docs/plans/2026-03-10-poe-*.md` documents (6 files) describe an unimplemented design. The AutoLoader / Generator / SolidificationSuggestion code is what was salvaged forward when the actual Evolution Pipeline work was shelved.

### 1.2 Why dissolve instead of preserve

Per R10 (薄 Harness 哲学，笨循环编排核心) and the YAGNI Withdrawal Pattern explicitly called out in CLAUDE.md:

> Any abstraction with "zero current consumers" gets deleted/withdrawn immediately, never "saved for the future." During dissolution, ~5,200 lines of dead code were removed.

The AutoLoader + Generator pair is exactly this case — zero current consumers on both sides. Keeping them creates the **illusion of capability** (LLM-readable types like `EvolutionAutoLoader` suggest the feature exists) and forces every future change in `MarkdownCliTool` / `AlephToolServer` / `markdown_skill::loader` to consider impossible callers.

When Evolution Pipeline work is actually scheduled, the correct path is to **redesign the producer-consumer contract together with the real upstream requirements** — not to inherit a 2026-03 sketch that pre-dates the current `SkillSystem` shape.

## 2. Goal

Remove the doubly-orphaned `EvolutionAutoLoader` + `MarkdownSkillGenerator` + `SolidificationSuggestion` types and their tests. Leave the rest of the Evolution scaffolding in place (it has user-facing or persistence-level surface).

**Non-goal**: Designing the future Evolution Pipeline. That happens when (and only when) a concrete user signal triggers restart.

## 3. Scope (tier B1)

This spec implements the minimal tier of three considered tiers (B1 / B2 / B3 — see "Alternatives considered" §7). B1 was chosen because larger tiers introduce user-facing or persistence-layer compatibility risk for negligible additional benefit.

### 3.1 In scope — delete

| Artifact | Path | Reason |
|----------|------|--------|
| `EvolutionAutoLoader` struct + impl + tests | `src/tools/markdown_skill/auto_loader.rs` (full file, 326 LOC) | Zero callers |
| `BatchLoadResult` struct + impl | `src/tools/markdown_skill/auto_loader.rs` | Only used by `EvolutionAutoLoader::load_batch` |
| `MarkdownSkillGenerator` + `MarkdownSkillGeneratorConfig` + `generate_*` helpers + `SkillMetrics` + `SolidificationSuggestion` | `src/tools/markdown_skill/generator.rs` (full file) | Zero callers outside `auto_loader.rs` |
| `pub use auto_loader::{BatchLoadResult, EvolutionAutoLoader};` | `src/tools/markdown_skill/mod.rs:15` | Re-export of removed types |
| `pub mod auto_loader;` + `pub mod generator;` declarations | `src/tools/markdown_skill/mod.rs` | Modules deleted |

### 3.2 Kept (intentionally) — out of scope for this spec

| Artifact | Reason kept |
|----------|-------------|
| `EvolutionMeta` struct + `pub use spec::*` re-export at `mod.rs:19` | Part of `AlephSkillSpec` YAML frontmatter schema. Human-authored SKILL.md files can legitimately declare `aleph: evolution: { source, confidence_score, ... }` to document provenance. Removing the field would break the schema contract. |
| `From<&SolidificationSuggestion>` / `Into<…>` impls on `EvolutionMeta` (if any survive in `spec.rs`) | **Delete** — they reference removed types. Decision rule: if removing `generator.rs` causes a compile error in `spec.rs`, delete the offending impl block in the same commit. The struct itself stays. |
| `AlephSkillSpec::evolution: Option<EvolutionMeta>` field | Same reason — frontmatter schema member |
| `EvolutionConfig` + `SolidificationThresholds` + `ToolGenerationConfig` in `src/config/types/evolution.rs` | User config files in production may include an `[evolution]` section. Removing the type would force users to delete the section to upgrade. The thresholds are currently dead data, but the cost of keeping them is one file × ~310 lines with no runtime impact. |
| `experience_replays` table migration in `src/resilience/database/migration.rs` | Migration ran in production DBs; reversing requires a write-time backward migration that adds test matrix surface for zero current benefit. The empty table is a benign scar. |
| `poe.sign.*` RBAC scope in `src/gateway/event_scope.rs` | 3 lines. Cost of removal exceeds cost of keeping. |
| `poe.run` / `poe.prepare` rate-limit lane in `src/gateway/rate_limiter.rs` + `src/gateway/lane.rs` | Same — declaration-level stubs |
| `docs/plans/2026-03-1{0,1}-{poe,cognitive-evolution}-*.md` (6 files) | Historical design record. They describe shelved work, not current state. |

### 3.3 Verification — must pass before merge

1. `cargo check -p alephcore` clean (no new warnings beyond pre-existing baseline drift — see `feedback_fmt_clippy_baseline_drift`)
2. `cargo test -p alephcore --lib` — no new failures vs `project_baseline_test_failures` baseline (19 pre-existing failures known)
3. `grep -rn "EvolutionAutoLoader\|MarkdownSkillGenerator\|SolidificationSuggestion\|BatchLoadResult\|SkillMetrics" src/` returns zero hits
4. `cargo build --release` succeeds
5. Boot smoke: `cargo run --bin aleph-server -- --help` does not panic (no init path references removed types)

## 4. Out-of-scope (explicit)

- **Designing the future Evolution Pipeline.** Deferred until a concrete user signal restarts the work. Section 6 documents the restart criteria.
- **Removing `EvolutionConfig` / `experience_replays` migration** (B2 tier). Rejected — see §7.
- **Annotating `docs/plans/2026-03-11-cognitive-evolution-*.md` with "SHELVED" headers** (B3 add-on). Rejected — historical docs are by definition descriptions of past intent; readers can tell from the date prefix.
- **Restoring the `markdown_skill::loader` or `MarkdownCliTool` skill-instantiation path.** Those are *consumers* of human-authored SKILL.md files, not Evolution-pipeline artifacts. They stay.

## 5. Implementation plan (preview — full plan in separate doc)

A separate implementation plan will be written by `superpowers:writing-plans` after spec approval. Skeleton:

1. **Task 1 (deletion)**: Delete `auto_loader.rs` and `generator.rs`, edit `mod.rs` re-exports.
2. **Task 2 (compile-fix)**: Run `cargo check`; any compile error indicates an in-scope edge case (e.g., another file imports `EvolutionMeta` from the wrong path). Fix in place — should be near-zero.
3. **Task 3 (verification)**: Run the §3.3 checklist.
4. **Task 4 (commit)**: One commit, conventional format: `refactor: dissolve EvolutionAutoLoader + MarkdownSkillGenerator (YAGNI withdrawal)`.

Expected diff: roughly −650 LOC, +0 LOC. No new files, no schema changes, no migrations.

## 6. Restart criteria (when to revisit)

This dissolution is reversible. The Evolution Pipeline work should restart when **any one** of these signals appears:

1. User self-reports installing the same skill manually 3+ times for the same workflow (indicates a real solidification opportunity that a daemon could detect).
2. The `experience_replays` table starts being written by an unrelated subsystem (the column was placed there in anticipation of Evolution work — its first real use would be the trigger).
3. A Hermes / OpenClaw / evolver upstream ships a working pattern-synthesis backend that can be ported.
4. The `superpowers` plugin ships its own evolution backend that needs an Aleph-side consumer.

At restart, the producer-consumer contract should be redesigned from current `SkillSystem` shape — not resurrected from this commit.

## 7. Alternatives considered

| Tier | Description | Decision | Reason |
|------|-------------|----------|--------|
| **B1** (chosen) | Delete only the doubly-orphan code (AutoLoader + Generator + SolidificationSuggestion). Keep config, DB migration, frontmatter `EvolutionMeta`, gateway stubs. | ✅ | Maximum YAGNI gain with zero user-facing / persistence-layer risk. |
| **B2** | B1 + delete `EvolutionConfig` + `experience_replays` migration + `AlephSkillSpec::evolution` field + 3 `poe.*` gateway stubs. | ❌ | Breaks existing user config files (serde fail on unknown `[evolution]` section unless `#[serde(default, deny_unknown_fields)]` is loosened); requires write-time reverse migration; breaks SKILL.md frontmatter schema. Cost > benefit. |
| **B3** | Don't delete code. Add `> ⚠ SHELVED 2026-05-20` header to 6 design docs only. | ❌ | Defeats the purpose of YAGNI withdrawal. Code keeps misleading future readers/LLMs. |

## 8. Risks

- **Risk**: A future Evolution Pipeline implementer rebuilds AutoLoader from scratch when they could have reused the deleted version.
  - **Mitigation**: The Git history preserves the deleted code (`git log --diff-filter=D --name-only` finds it). Section 6 names the deleted artifacts.
- **Risk**: Removing the `EvolutionMeta` re-export breaks an external crate or test fixture.
  - **Mitigation**: §3.3 verification step 3 (grep sweep) catches this before merge.
- **Risk**: The dissolution commit lands in a worktree branch and bit-rots vs `main`.
  - **Mitigation**: Per `feedback_worktree_for_implementation`, implementation happens in a dedicated worktree; merge same-day or rebase on conflict.
