# Skill Model Taxonomy

> Where do skill-related types live, what does each one mean, and which ones should you reach for when writing new code? This document is the canonical reference for the four-layer skill model in Aleph.

**Date:** 2026-05-20 (Phase 1 of `docs/superpowers/specs/2026-05-20-skill-data-model-unification-design.md`)
**Audience:** Contributors writing or maintaining skill-related code.

## Quick reference

| If you want to… | Use this type | In this module |
|-----------------|---------------|----------------|
| Represent a skill the LLM can decide to follow (identity + content + invocation policy) | `SkillManifest` | `crate::domain::skill` |
| Track which skills are installed on disk and where each one came from | `InstallRegistry` | `crate::bundled::manifest` |
| Parse an OpenClaw-style Markdown CLI tool from a SKILL.md frontmatter | `AlephSkillSpec` *(deprecated — see below)* | `crate::tools::markdown_skill::spec` |
| Tag a memory fact as "this is about a skill" | `NoteType::Skill` | `crate::memory::context::enums` |
| Send a flat skill view over RPC / to the panel | `SkillInfo` | `crate::skill::compat` |

## The four-layer model

```text
                ┌────────────────────────────────────┐
                │  Layer 0: in-memory tag             │
                │  NoteType::Skill                    │
                │  (marks a MemoryFact as a skill)    │
                └────────────────────────────────────┘
                ┌────────────────────────────────────┐
                │  Layer 1: install registry          │
                │  bundled::manifest::InstallRegistry │
                │  (on-disk manifest.json, who has    │
                │   what skill installed, from where) │
                └────────────────────────────────────┘
                ┌────────────────────────────────────┐
                │  Layer 2: aggregate root            │
                │  domain::skill::SkillManifest       │
                │  (the v2 truth — identity,          │
                │   content, eligibility, invocation) │
                └────────────────────────────────────┘
                              ▲
                              │  From<&AlephSkillSpec>
                              │  (Phase 1 bridge; Phase 2 absorbs)
                ┌─────────────┴──────────────────────┐
                │  Layer 3: parser surface           │
                │  markdown_skill::AlephSkillSpec    │
                │  (DEPRECATED — see "Phase 2")      │
                │  (Markdown CLI tool frontmatter)   │
                └────────────────────────────────────┘
```

## Layer-by-layer

### Layer 2 — `domain::skill::SkillManifest` (the truth)

The DDD aggregate root for "a skill". 388 lines, 14 consumers. Owns identity (`SkillId`), human-readable name, prompt content (`SkillContent`), provenance (`SkillSource`), eligibility rules (`EligibilitySpec`), install instructions (`InstallSpec`), and invocation policy (`InvocationPolicy`).

This is what `crate::skill::SkillSystem` holds. This is what the agent loop sees rendered into the system prompt. **All new skill-shaped code should reach for this type.**

### Layer 1 — `bundled::manifest::InstallRegistry` (the inventory)

Renamed from `SkillManifest` in Phase 1 to eliminate the name collision with Layer 2. This struct only describes *which* skills are installed under `~/.aleph/skills/` and *where* each one came from (bundled / GitHub / local). It does NOT describe what any skill does — that's Layer 2's job.

`InstallRegistry` is purely a persistence concern: it serializes to `~/.aleph/skills/manifest.json` and is read at startup to decide whether bundled-content re-extraction is needed.

### Layer 3 — `markdown_skill::AlephSkillSpec` (deprecated)

A second SKILL.md frontmatter parser, originally written for OpenClaw-style Markdown CLI tools. It overlaps with Layer 2's parser at the identity + content level and diverges on metadata (it carries `RequiresSpec`, `AlephExtensions { security, input_hints, docker }`, `OpenClawMetadata`).

**Status:** Deprecated as of 2026-05-20. New code MUST NOT use `AlephSkillSpec`. The conversion `impl From<&AlephSkillSpec> for SkillManifest` exists in `markdown_skill::spec` as the migration seam; lossy by design until Phase 2 absorbs the CLI-tool-specific fields onto `SkillManifest`.

The `#![allow(deprecated)]` at `markdown_skill/mod.rs` scope-limits the resulting warnings to the only legitimate consumer module.

### Layer 0 — `NoteType::Skill` (the tag)

A variant on the memory layer's `NoteType` enum. It says "this `MemoryFact` is about a skill" — nothing more. Maps to `MemoryCategory::Patterns` and gets a `aleph://skills/` URI prefix. **Orthogonal** to Layers 1-3 — a memory fact about a skill does not need to *be* a `SkillManifest`.

### The DTO — `skill::compat::SkillInfo`

A flat projection of `SkillManifest` for RPC payloads and the panel. `impl From<SkillManifest> for SkillInfo` lives in `src/skill/compat.rs`. Not in scope for unification — it is already a clean one-way projection, not a competing model.

## Why four layers?

Each layer answers a genuinely different question:

| Layer | Answers | Lives in |
|-------|---------|----------|
| 0 | "Is this memory entry about a skill?" | RAM (and the memory store) |
| 1 | "What's installed where?" | `~/.aleph/skills/manifest.json` |
| 2 | "What is this skill, and when does the LLM apply it?" | `SkillSystem` (process-wide singleton) |
| 3 | "How do I parse a Markdown CLI tool's frontmatter?" | Loader-time only |

Layers 0, 1, 2 are permanent. Layer 3 is a deprecated alias for Layer 2's parsing concern; Phase 2 dissolves it.

## Phase 1 bridge (active 2026-05-20 → Phase 2)

```rust
impl From<&AlephSkillSpec> for crate::domain::skill::SkillManifest {
    fn from(spec: &AlephSkillSpec) -> Self {
        // Currently lossy: only identity + content + description are mapped.
        // SkillSource defaults to Global (matches typical clawhub install path).
        // CLI-tool metadata (requires.bins, security, docker, input_hints,
        // openclaw.*) is dropped until Phase 2 absorbs those onto SkillManifest.
        …
    }
}
```

This bridge is the Phase 1 contract. It exists so that Phase 2 can incrementally migrate consumers without needing to introduce the conversion as part of the destructive change.

## Phase 2 timing rule (≥ 2026-06-03)

Phase 2 — the destructive absorption that deletes `AlephSkillSpec` — must not begin until:

1. At least **two weeks** have passed since Phase 1 ships, AND
2. No regression has been reported against `project_skill_system_wiring_shipped` in those two weeks.

The earliest practical Phase 2 start date is **2026-06-03**. See `docs/superpowers/specs/2026-05-20-skill-data-model-unification-design.md` §4.2 for the full Phase 2 task list.

## Common confusions and what to do about them

| Symptom | Likely cause | Fix |
|---------|--------------|-----|
| You see `SkillManifest` in `use` lines but it doesn't have the field you expect | You imported the Layer 1 install registry. Look at the import path — if it starts with `bundled::manifest::`, that's `InstallRegistry`. After Phase 1, the type name itself disambiguates. | Use `crate::domain::skill::SkillManifest` for skill identity / content. Use `crate::bundled::manifest::InstallRegistry` for install state. |
| You're writing new code that parses a SKILL.md | Don't introduce a third parser. | Use `crate::skill::manifest::parse_skill_md` (Layer 2's parser); add fields to `SkillManifest` if needed. |
| You need to represent "this CLI tool's network mode / docker config / required binaries" for a skill | These will live on `MarkdownCliExtras` after Phase 2 absorbs them. Until then, hold them in `AlephSkillSpec` (with `#[allow(deprecated)]`) at the parser boundary and discard at the SkillManifest layer. | Don't add new fields to `AlephSkillSpec`. Add them to `SkillManifest` directly and update the bridge. |
| You see a `#[deprecated]` warning on `AlephSkillSpec` outside `markdown_skill/` | A consumer outside the controlled allow-list now references the deprecated type. | This is a signal to migrate the consumer to `SkillManifest` via the `From` bridge. |

## See also

- `docs/superpowers/specs/2026-05-20-skill-data-model-unification-design.md` — the full design spec
- `docs/superpowers/specs/2026-05-19-skill-system-wiring-design.md` — the v2 wiring this builds on
- `docs/reference/AGENT_SYSTEM.md` — how `SkillManifest` flows into the agent loop
- `.claude/memory/project_skill_system_wiring_shipped.md` — what shipped in the 2026-05-20 wiring merge
- `.claude/memory/project_skill_model_phase2_pending.md` *(written by Phase 1)* — Phase 2 trigger date and checklist
