---
title: Skill Data Model Unification
date: 2026-05-20
status: draft
spec_owner: Aleph
brainstorming_source: superpowers:brainstorming session 2026-05-20
related:
  - .claude/memory/project_skill_system_wiring_shipped.md (the deferred-item note)
  - docs/superpowers/specs/2026-05-19-skill-system-wiring-design.md (the v2 wiring this builds on)
  - src/domain/skill.rs (aggregate root)
  - src/tools/markdown_skill/spec.rs (parser layer)
  - src/bundled/manifest.rs (install registry, name-collides)
follow_up:
  - Phase 2 implementation cycle (deferred until 2026-06-03+ — see §6 timing rule)
  - Spec B — Evolution AutoLoader dissolution (separate cycle, same session)
  - Spec C — Host sandbox netns isolation (separate cycle, same session)
---

# Skill Data Model Unification

## 1. Background

The 2026-05-20 skill-system wiring merge (`6443d771b`) shipped `crate::skill::shared_skill_system()` as the single process-wide skill registry and wired its snapshot into `AgentHarnessRunner::build_system_prompt`. That wiring stabilized the **runtime** view of skills but explicitly deferred the **data model** consolidation:

> 三套 skill 数据模型(v2 `SkillManifest` / `AlephSkillSpec` / `NoteType::Skill` 记忆事实)就地保留 —— 统一是破坏性重构

This spec resolves that deferral. A recon pass on 2026-05-20 revealed the situation is slightly worse than the deferred-item note implied — there are actually **four** types with overlapping names and responsibilities, plus one display-only DTO.

### 1.1 Current model inventory

| # | Type | Path | Role | Consumers |
|---|------|------|------|-----------|
| 1 | `domain::skill::SkillManifest` | `src/domain/skill.rs:391` (388-line aggregate) | **DDD aggregate root** — the v2 truth model for "an installable skill artifact". Drives `SkillSystem` + agent loop prompt injection. | 14 files |
| 2 | `bundled::manifest::SkillManifest` | `src/bundled/manifest.rs:11` (~80-line struct) | **Install registry** — `BTreeMap<dir_name, SkillEntry>` persisted to `~/.aleph/skills/manifest.json`. Tracks which bundled version is installed and where each skill came from. **Name-collides with #1.** | 2 files |
| 3 | `tools::markdown_skill::AlephSkillSpec` | `src/tools/markdown_skill/spec.rs:11` (308-line struct + sub-structs) | **SKILL.md parser model** — frontmatter schema for Markdown-CLI-tool-backed skills (OpenClaw compatibility path). Used by `MarkdownCliTool` to register as a `tool` (not a `skill` in v2 terminology). | 7 files in `markdown_skill/` |
| 4 | `memory::NoteType::Skill` | `src/memory/context/enums.rs` | **Memory-layer tag** — marks a `MemoryFact` as representing a skill. Maps to `MemoryCategory::Patterns`. | 5+ files |
| (DTO) | `skill::compat::SkillInfo` | `src/skill/compat.rs:19` | **Display projection** of #1 for external surfaces (RPC payloads, panel). `impl From<SkillManifest> for SkillInfo`. | Not in scope — already a clean projection |

### 1.2 Recorded history of collision pain

`src/bundled/manifest.rs:20-21` carries a comment that confirms the project has already absorbed one round of skill-type collisions and resolved it by renaming:

```rust
/// Named `SkillOrigin` to avoid collision with `domain::skill::SkillSource`
/// and `skills::registry::SkillSource`.
```

The active `SkillManifest` ↔ `SkillManifest` collision (between #1 and #2) is the next one. Every contributor reading either file has to mentally disambiguate them by module path.

### 1.3 The real semantic question

Looking past names, the four types resolve to two genuinely-distinct concepts plus a tag:

| Concept | Type(s) that represent it | Genuinely separate? |
|---------|---------------------------|---------------------|
| **"What is this skill?"** (identity, content, eligibility, invocation policy, source provenance, install requirements) | #1 (`domain::SkillManifest`) **and** #3 (`AlephSkillSpec`) both try to answer this, from two different SKILL.md parsers, with overlap on `name`/`description`/`markdown_content` and divergence on metadata sub-structures | **No** — these are two views of the same concept, frozen at different points in the codebase's evolution |
| **"Which skills are installed and where did they come from?"** | #2 (`bundled::SkillManifest`) — pure install-state persistence | **Yes** — registry, not identity |
| **"This memory fact is about a skill"** | #4 (`NoteType::Skill`) — variant of a generic enum | **Yes** — tag, not model |

The unification opportunity is in #1+#3. #2 and #4 don't need to be merged — they need to be **renamed and decoupled** so the surrounding code does not have to wrestle with name collisions.

## 2. Goal

Establish a target end-state where:

1. **One** type represents "a skill artifact": `domain::skill::SkillManifest` (#1) absorbs `AlephSkillSpec` (#3).
2. **One** SKILL.md parser produces #1 directly; the Markdown-CLI-tool path becomes a `From<SkillManifest> for MarkdownCliTool` adapter instead of having its own parallel parse → spec → tool pipeline.
3. **No name collisions** between modules: `bundled::SkillManifest` (#2) is renamed to `bundled::InstallRegistry`.
4. `NoteType::Skill` (#4) stays as-is (it's an orthogonal concern — a memory tag, not a skill model).
5. The transition happens in **two cycles**: a low-risk Phase 1 (this spec, ships now) and a destructive Phase 2 (deferred ≥2 weeks after Phase 1 to let the recently-merged wiring stabilize).

**Non-goal**: Refactoring the memory layer, the bundled-install machinery, or the SkillSystem runtime/wiring. Those just merged and should not be re-touched.

## 3. Target end-state (post-Phase 2)

```text
┌────────────────────────────────────────────────────────────────────┐
│  src/domain/skill.rs                                               │
│  ─────────────────                                                 │
│  pub struct SkillManifest { … }     ← single source of truth      │
│  pub struct SkillContent(String);                                  │
│  pub enum  SkillSource { Bundled, Github, Local, Markdown, … }    │
│  pub struct InvocationPolicy { … }                                 │
│  pub struct EligibilitySpec { … }                                  │
│  pub struct InstallSpec { … }                                      │
│  pub struct MarkdownCliExtras { … }    ← absorbs AlephExtensions   │
│  pub struct OpenClawCompat { … }       ← absorbs OpenClawMetadata  │
└────────────────────────────────────────────────────────────────────┘
                              ▲                  ▲
                              │                  │
              ┌───────────────┘                  └──────────────┐
              │                                                  │
┌─────────────┴─────────────┐                  ┌─────────────────┴────────┐
│  src/skill/manifest.rs    │                  │ src/tools/markdown_skill │
│  ─────────────────────    │                  │ ──────────────────────── │
│  pub fn parse_skill_md()  │                  │ pub fn into_cli_tool(    │
│      -> SkillManifest     │                  │   m: &SkillManifest)     │
│  (unified parser)         │                  │   -> MarkdownCliTool     │
└───────────────────────────┘                  └──────────────────────────┘

┌──────────────────────────────────┐    ┌──────────────────────────────┐
│ src/bundled/install_registry.rs  │    │ src/memory/context/enums.rs  │
│ ────────────────────────────────  │    │ ──────────────────────────── │
│ pub struct InstallRegistry { … } │    │ pub enum NoteType {          │
│ (renamed from SkillManifest)     │    │     …                         │
│ Pure install-state persistence    │    │     Skill,  ← unchanged       │
└──────────────────────────────────┘    │ }                             │
                                         └──────────────────────────────┘
```

## 4. Phases

### 4.1 Phase 1 — Safe, ships now (this spec covers)

**Goal**: Remove name collisions; establish the conversion seam; document the taxonomy. Zero behavior change.

| Step | Change | Files | Risk |
|------|--------|-------|------|
| 1.1 | Rename `bundled::manifest::SkillManifest` → `InstallRegistry`. Move file to `src/bundled/install_registry.rs` (or keep in `manifest.rs` with rename). | `src/bundled/manifest.rs`, `src/bundled/mod.rs`, `src/bundled/extractor.rs`, all callers (2 files) | Very low — pure rename, 2 callers |
| 1.2 | Add `impl From<&AlephSkillSpec> for SkillManifest` in `src/tools/markdown_skill/spec.rs`. Initially **not used** by any caller; just defines the contract. | `src/tools/markdown_skill/spec.rs` (+ `src/domain/skill.rs` if helper constructors needed) | Low — additive |
| 1.3 | Write `docs/reference/SKILL_MODEL_TAXONOMY.md` explaining the 4 types, why #1 absorbs #3 in Phase 2, and the deprecation timeline for `AlephSkillSpec`. Link from `docs/reference/ARCHITECTURE.md`. | New doc | None |
| 1.4 | Add a `#[deprecated(note = "use domain::skill::SkillManifest via From impl; will be removed in Phase 2")]` on `AlephSkillSpec` itself. **Allow** the deprecation warning for `markdown_skill/` files in `#![allow(deprecated)]` at module root — they remain the only legitimate users until Phase 2. | `src/tools/markdown_skill/spec.rs`, `src/tools/markdown_skill/mod.rs` | Low — controlled |

**Phase 1 verification**:
- `cargo check -p alephcore` clean
- `cargo test -p alephcore --lib` — no new failures vs `project_baseline_test_failures` baseline
- `grep -rn "bundled::manifest::SkillManifest\|bundled::SkillManifest" src/` returns zero hits
- New doc file exists and is linked from ARCHITECTURE.md

Phase 1 implementation plan: ~50–100 LOC diff, single commit, single worktree, mergeable same-day per `feedback_worktree_for_implementation`.

### 4.2 Phase 2 — Destructive, deferred to its own cycle (this spec defines but does NOT execute)

**Timing rule**: Phase 2 must not start until **at least 2 weeks after Phase 1 ships** AND `project_skill_system_wiring_shipped` has not had a regression report in those 2 weeks. Earliest practical start: **2026-06-03**.

| Step | Change | Estimated blast radius |
|------|--------|------------------------|
| 2.1 | Move `AlephExtensions` + `OpenClawMetadata` fields into `domain::SkillManifest` as `markdown_cli_extras: Option<MarkdownCliExtras>` and `openclaw_compat: Option<OpenClawCompat>`. | 7 files in `markdown_skill/` |
| 2.2 | Replace `tools::markdown_skill::parser::parse_skill_md(&str) -> AlephSkillSpec` with `parse_skill_md(&str) -> SkillManifest` directly. The unified parser lives in `src/skill/manifest.rs` (where the v2 parser already is). | `src/tools/markdown_skill/parser.rs` (delete or shrink to facade), `src/tools/markdown_skill/loader.rs`, `src/skill/manifest.rs` (extend with `markdown_cli_extras` support) |
| 2.3 | Convert `MarkdownCliTool` construction from `From<AlephSkillSpec>` to `From<&SkillManifest>`. | `src/tools/markdown_skill/tool_adapter.rs` |
| 2.4 | Delete `AlephSkillSpec`, `SkillMetadata`, `AlephExtensions` (now lives on `SkillManifest`), `OpenClawMetadata` (same), `RequiresSpec` (if subsumed). | `src/tools/markdown_skill/spec.rs` (file shrinks dramatically or deletes) |
| 2.5 | Update tests in `src/tools/markdown_skill/{parser,loader,executor,tool_adapter}.rs` to use unified type. | ~7 test modules |
| 2.6 | Update `src/tools/markdown_skill/mod.rs` re-exports. | 1 file |

**Phase 2 estimated diff**: −600 to −800 LOC (mostly deletion of duplicate parser + types), +200 LOC (additions to `domain::SkillManifest` for the absorbed fields), single commit per step or one bundled commit.

**Phase 2 verification adds to Phase 1's**:
- All `MarkdownCliTool` integration tests pass
- A clawhub-installed skill (e.g., the test fixture in `tests/markdown_skill/`) loads end-to-end without behavior change
- `grep -rn "AlephSkillSpec" src/` returns zero hits

Phase 2 will get its own `superpowers:writing-plans` document. It is **not** in scope for this spec.

## 5. Migration strategy for existing on-disk artifacts

**Phase 1 has no on-disk impact.**

**Phase 2 on-disk impact**:
- SKILL.md files written by users do not change. The YAML frontmatter schema (`aleph:` and `openclaw:` namespaces) is preserved 1:1 in the absorbed `MarkdownCliExtras` / `OpenClawCompat` structs.
- `~/.aleph/skills/manifest.json` schema is untouched (it's a #2 concern, not #3).
- `experience_replays` table is untouched.

The only schema change is in-memory Rust types; no migration script needed.

## 6. Out of scope (explicit)

- **Memory layer changes** — `NoteType::Skill` and `MemoryCategory::Patterns` mapping stay as-is.
- **`bundled::InstallRegistry` internal restructure** — only the rename happens. Its `BTreeMap<dir_name, SkillEntry>` shape is fine.
- **`SkillSystem` runtime / wiring** — just merged, do not re-touch.
- **`MarkdownSkillRefreshSource` (the agent-loop bridge added in the recent wiring work)** — stays unchanged; its dependency is `SkillSystem`, not the underlying types being unified.
- **Cross-language skill formats** (Python skills, executable skills) — out of skill data-model scope.

## 7. Alternatives considered

| Option | Description | Decision | Reason |
|--------|-------------|----------|--------|
| **A1 full** | Single-cycle full destructive unification: rename + absorb + delete in one sprint. | ❌ | Touches code that just merged today. Risk of regressing the new wiring is high. Two-week cooling-off period via Phase 1 / Phase 2 split is safer. |
| **A1 variant + A2 hybrid** (chosen) | Phase 1 = safe (rename + bridge + doc); Phase 2 deferred = destructive (absorb + delete). | ✅ | Captures immediate wins (kill collision, document taxonomy) without destabilizing fresh wiring. Defines the destination so Phase 2 plan is mechanical. |
| **A2 only** | Naming cleanup + conversion bridge, never absorb. | ❌ | Leaves the duplicate-parser fundamental issue unresolved indefinitely. Every future SKILL.md frontmatter field has to be added to two parsers. |
| **A3 only** | Write taxonomy doc, do not touch code. | ❌ | Doc would describe a problem the team has been working around for months. Better to fix the active collision (rename in Phase 1) at minimum. |
| **Absorb #3 into #1 immediately, but keep both available behind a Cargo feature flag** | Feature-flag gated transition. | ❌ | Per `feedback_no_user_capability_override` and the broader "no feature flags for production paths" preference, this is anti-pattern. |
| **Make `domain::SkillManifest` itself generic over a metadata sub-type, parameterizing on parser source** | `SkillManifest<M: MetadataExt>` style. | ❌ | Premature abstraction. Two metadata flavors do not justify a generic parameter that propagates to 14 consumers. |

## 8. Risks

| Risk | Likelihood | Mitigation |
|------|------------|------------|
| Phase 2 collides with another in-flight worktree branch that modifies `markdown_skill/*` | Medium — there are active worktrees in `.worktrees/` | Defer Phase 2 start until `git worktree list` shows no overlapping branches; rebase rather than merge if conflict appears |
| `From<AlephSkillSpec> for SkillManifest` (Phase 1 step 1.2) is incorrectly written and silently loses fields when used by Phase 2 | Medium | Phase 1 includes a unit test asserting field-by-field round-trip; Phase 2's first task is to verify against existing fixtures in `tests/fixtures/markdown_skills/` and the integration test `tests/markdown_skill_wiring_test.rs` |
| The deprecation warning in Phase 1 step 1.4 leaks into downstream lint failures | Low → realized at impl time | Scoped `#![allow(deprecated)]` at `markdown_skill/mod.rs` AND `gateway/handlers/markdown_skills.rs` (the RPC handler reads `tool.spec.*` for payload projection). Any future site that triggers the warning is a known Phase 2 migration target |
| `bundled::SkillManifest::load()`/`save()` are called by code paths I missed in §1.1's "2 files" count | Low | `grep -rn "bundled::manifest::SkillManifest\|bundled::SkillManifest\b" src/ tests/` before rename; full `cargo check` after |
| Phase 2 is forgotten and `AlephSkillSpec` stays deprecated forever | Medium | Save a follow-up memory note (`project_skill_model_phase2_pending`) with the 2026-06-03 earliest-start date; the deprecation `#[deprecated(...)]` will surface a warning every `cargo build` reminding the team |
| Renaming `bundled::SkillManifest` breaks an external integration test or downstream tool | Very low | The type is internal (`pub` but only used by `bundled` + 2 callers); no MCP/RPC/SDK surface exposes it |

## 9. Open questions

None. Phase 1 scope and Phase 2 target end-state are both fully specified above.

## 10. Acceptance criteria (Phase 1 only — this spec)

- [ ] `bundled::SkillManifest` renamed to `InstallRegistry` everywhere in `src/`
- [ ] `impl From<&AlephSkillSpec> for SkillManifest` added with a unit test asserting field-by-field round-trip for a canonical fixture
- [ ] `#[deprecated]` applied to `AlephSkillSpec` with the message specifying Phase 2 removal
- [ ] `docs/reference/SKILL_MODEL_TAXONOMY.md` exists and is linked from `docs/reference/ARCHITECTURE.md`
- [ ] A follow-up memory note `project_skill_model_phase2_pending.md` exists, names the earliest-start date (2026-06-03), and is added to `MEMORY.md` index
- [ ] `cargo check -p alephcore` clean
- [ ] `cargo test -p alephcore --lib` — no new failures vs baseline
- [ ] One commit (or one tightly-scoped sequence) per `feedback_changelog_english` (English commit message)
