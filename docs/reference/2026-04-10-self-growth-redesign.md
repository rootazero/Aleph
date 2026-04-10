# Self-Growth Module Redesign

**Date**: 2026-04-10
**Status**: Approved
**Inspired by**: hermes-agent skill extraction mechanism
**Principle**: Prompt-first (R8 LLM Sovereignty + R9 Everything is a Tool + R10 Intelligence Lives in the Prompt)

---

## Overview

Redesign Aleph's self-growth capability to follow the prompt-first principle. Replace algorithmic memory processing (DBSCAN clustering, 8-dimensional scoring, hardcoded signal detection) with LLM-driven skill extraction during conversation reflection. Skills are normalized prompts — reusable knowledge extracted by the LLM and stored as Markdown files.

### Core Concept

```
Conversation ends
    → Reflection (existing, extended)
    → LLM outputs structured skill definitions
    → skill_manage tool writes SKILL.md to filesystem
    → MemoryStore indexes skill embedding + metadata
    → Next conversation: SkillRecaller injects relevant skills into system prompt
    → LLM uses skill, evaluates freshness, patches if outdated
```

---

## Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Trigger mode | Real-time (reflection) | Skill extraction is semantic judgment; needs full conversation context |
| Storage | Dual-write (filesystem + DB index) | Human-readable SKILL.md + machine-searchable embedding |
| Pipeline cleanup | Layered: keep infra, remove algorithmic judgment | R8: decay is math (keep), clustering is semantic judgment (remove) |
| Skill evolution | Real-time via reflection | YAGNI — LLM judges staleness at use time; git provides version history |
| Skill recall | Hybrid (frequent skills pinned + semantic retrieval) | Deterministic recall for high-freq + coverage for long tail |
| Skill scope | Two-level (Global + Persona) | Skills bind to roles; workspace-specific knowledge belongs in facts |

---

## Section 1: Architecture

### Data Flow

```
Conversation ends
    ↓
Reflection (existing gate: min_turns=5, min_user_chars=200, cooldown=30min)
    ↓ Extended Skills section prompt
LLM outputs structured skill YAML
    ↓
skill_manage tool (new AlephTool)
    ↓ create / patch / delete
Filesystem: ~/.aleph/skills/learned/{scope}/{category}/{name}/SKILL.md
    ↓ sync
MemoryStore: skill_index table (embedding + metadata)
    ↓
Next conversation prompt assembly
    ↓
SkillRecaller (new)
    ↓ frequent pinned + semantic retrieval
Inject relevant skills into system prompt
```

### Separation of Concerns

- **Learned skills** (`~/.aleph/skills/learned/`): LLM-generated growth skills
- **Installed skills** (`~/.aleph/skills/installed/`): User-installed external skills (unchanged)
- **Generated skills** (`~/.aleph/skills/generated/`): Evolution auto-generated skills (unchanged)

All three share the `markdown_skill` loading/parsing infrastructure.

### Constraints

- Reflection is the sole entry point for skill extraction (no mid-conversation interruption)
- LLM operates through `skill_manage` tool for all CRUD (R9: Everything is a Tool)
- Learned skills are prompt-only (sandbox: host, confirmation: never, network: none) — they inject text, not execute commands

---

## Section 2: Skill Extraction (Reflection Extension)

### Current State

Reflection prompt's Skills section outputs one-line strings (`skill name: concise reusable steps`). Mapper stores them as `FactType::Lesson`.

### Extended Reflection Prompt (Skills Section)

```
## Skills
For any non-trivial, reusable knowledge discovered this session (5+ steps,
likely to recur, or hard-won insight), output a complete skill definition.

Format per skill:
```yaml
- name: kebab-case-name
  category: coding | debugging | workflow | knowledge | communication
  description: One-line description (max 100 chars)
  content: |
    # Skill Title

    ## When to Use
    Trigger conditions...

    ## Steps
    1. ...
    2. ...

    ## Pitfalls
    - ...
```

Rules:
- Only extract if the knowledge is REUSABLE across sessions
- If an existing skill was used and found outdated, output it with updated content
- If a skill was used and confirmed correct, do NOT re-output it
- Maximum 3 skills per reflection
```

### New Data Structure

```rust
pub struct SkillExtraction {
    pub name: String,           // kebab-case
    pub category: String,       // coding/debugging/workflow/knowledge/communication
    pub description: String,    // one-liner
    pub content: String,        // full markdown body
    pub is_update: bool,        // true if patching existing skill
}
```

### Mapper Change

Skills section results no longer map to `FactType::Lesson`. Instead:
1. Call `skill_manage` tool logic to create/patch SKILL.md
2. Sync update skill_index in MemoryStore
3. Backward compatible: if LLM outputs old one-line format, fall back to existing behavior

---

## Section 3: Skill Storage & Tools

### Filesystem Structure

```
~/.aleph/skills/
├── learned/                          # LLM-generated (source of truth)
│   ├── global/                       # Global scope
│   │   └── {category}/
│   │       └── {name}/
│   │           └── SKILL.md
│   └── {persona-id}/                 # Persona scope
│       └── {category}/
│           └── {name}/
│               └── SKILL.md
├── installed/                        # User-installed (unchanged)
│   └── ...
└── generated/                        # Evolution auto-generated (unchanged)
    └── ...
```

### SKILL.md Format (Learned Skill)

Reuses existing `AlephSkillSpec` frontmatter with `evolution.source: learned`:

```yaml
---
name: rust-lifetime-debugging
description: Debug Rust lifetime errors systematically
metadata:
  aleph:
    evolution:
      source: learned
      learned_at: "2026-04-10T14:30:00Z"
      learned_from_session: "session-abc123"
    security:
      sandbox: host
      confirmation: never
      network: none
---

# Rust Lifetime Debugging

## When to Use
When encountering Rust lifetime errors that involve...

## Steps
1. ...

## Pitfalls
- ...
```

### skill_index Table (MemoryStore)

```sql
CREATE TABLE skill_index (
    name TEXT PRIMARY KEY,
    scope TEXT NOT NULL,          -- 'global' or persona_id
    category TEXT NOT NULL,
    description TEXT NOT NULL,
    file_path TEXT NOT NULL,      -- absolute path
    embedding BLOB,              -- vector for semantic retrieval
    use_count INTEGER DEFAULT 0, -- determines frequent pinning
    last_used_at INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
```

Filesystem is source of truth. skill_index is a search acceleration layer. Rebuilt from filesystem scan on startup (reuses existing loader logic).

### skill_manage Tool

Static `AlephTool` implementation:

```rust
pub struct SkillManageTool { /* ... */ }

pub struct SkillManageArgs {
    pub action: SkillAction,     // create | patch | delete | list
    pub name: Option<String>,
    pub category: Option<String>,
    pub scope: Option<String>,   // global | persona (default: persona)
    pub content: Option<String>, // full SKILL.md content (for create)
    pub old_text: Option<String>,// patch old text
    pub new_text: Option<String>,// patch new text
}
```

Operations:
- **create**: validate name → write SKILL.md → generate embedding → insert skill_index
- **patch**: find file → find-and-replace → rewrite → update embedding + updated_at
- **delete**: remove file + directory → delete skill_index row
- **list**: query skill_index → return name + description list

### skill_search Tool

```rust
pub struct SkillSearchTool { /* ... */ }

pub struct SkillSearchArgs {
    pub query: String,           // natural language query
    pub scope: Option<String>,   // filter scope
    pub limit: Option<usize>,    // default 5
}
```

Vector search on skill_index embeddings. Returns matching skill names + descriptions + relevance scores.

---

## Section 4: Skill Recall (Prompt Assembly)

### SkillRecaller Component

Called during prompt assembly, before conversation starts.

### Recall Flow

```
Prompt assembly begins
    ↓
1. Frequent layer: query skill_index where use_count >= 3
    → inject name + description (index level, no full text)
    ↓
2. Semantic layer: embed current user message
    → vector search skill_index, top-K (excluding frequent layer)
    → inject name + description
    ↓
3. Assemble as system prompt fragment
```

### Injection Format (Progressive Disclosure)

```
## Available Skills
Before replying, scan these skills. If one matches your task,
load it with skill_search or skill_view and follow its instructions.

### Frequently Used
- rust-lifetime-debugging: Debug Rust lifetime errors systematically
- git-rebase-workflow: Interactive rebase with conflict resolution

### Possibly Relevant
- async-tokio-patterns: Common async patterns in Tokio (relevance: 0.82)
- sqlx-migration-tips: SQLx migration best practices (relevance: 0.71)
```

### Token Budget

- Index-level injection: ~15-20 tokens per skill (name + one-line description)
- Frequent layer cap: 10 skills (~200 tokens)
- Semantic layer cap: 5 skills (~100 tokens)
- Total: ~300 tokens

### use_count Update

Incremented when LLM loads skill full text via `skill_view`. Also updates `last_used_at`.

---

## Section 5: Dreaming Pipeline Cleanup & Refactoring

### Files to Delete

| File | Reason |
|------|--------|
| `stages/collect.rs` | Already no-op, SessionStore removed |
| `stages/cluster.rs` | DBSCAN clustering — algorithmic semantic judgment |
| `consolidation/promotion_scorer.rs` | 8-dimensional scoring — algorithmic semantic judgment |
| `value_estimator/signals.rs` | Hardcoded signal detection — algorithmic semantic judgment |

### Files to Keep (Unchanged)

| File | Reason |
|------|--------|
| `stages/decay.rs` | Pure math (Ebbinghaus decay) — infrastructure |
| `store/*` | MemoryStore/GraphStore traits — infrastructure |
| `vfs/*` | VFS path system — infrastructure |
| `dreaming/gate.rs` | Three-level gate logic — infrastructure |
| `dreaming/mod.rs` | DreamDaemon + DreamPipeline framework (adjust stage registration) |

### Files to Refactor

**1. `stages/drift.rs`** — Wire LLM arbitration

Current: Has LLM prompt builder (lines 66-99) but not wired; all candidates default to `DriftAction::Coexist`.

Change:
- Keep vector search for candidate pair discovery (infrastructure)
- Send candidate pairs to LLM using existing prompt template
- LLM returns `Supersede | Merge | Coexist | Ignore`
- Execute corresponding fact operations

**2. `stages/synthesis.rs`** — Wire LLM synthesis

Current: Has LLM synthesis path but coupled with hardcoded DBSCAN.

Change:
- Delete DBSCAN clustering logic
- Group by `fact_type` → take top-N per group (by strength) → send to LLM for cross-fact insight extraction
- LLM output stored as Core tier fact (preserves existing behavior)

**3. `stages/tunnel.rs`** — Add LLM judgment layer

Current: Auto-creates tunnel edge when embedding similarity >= 0.6.

Change:
- Keep embedding similarity computation as candidate filter (infrastructure)
- Send candidate pairs above threshold to LLM
- LLM judges "is this cross-domain connection meaningful?"
- Only create tunnel edge for LLM-confirmed pairs

**4. `stages/summarize.rs`** — Replace with LLM summarization

Current: String concatenation, truncated to 80 chars.

Change:
- Collect day's new facts
- Send to LLM for structured daily summary generation
- Store as `DailyInsight` (preserves existing data structure)

**5. `stages/consolidate.rs`** — Simplify promotion logic

Current: Depends on 8-dimensional promotion_scorer.

Change:
- Delete promotion_scorer dependency
- Simple rule filter (signal_count >= 3, age >= 24h, unique_queries >= 2) for candidates
- Send candidates to LLM: "Is this fact worth promoting from ShortTerm to LongTerm?"
- Preserve pruning logic (strength < 0.1 non-Core facts invalidated)

### Pipeline Stage Registration

```rust
// Before (7 stages)
vec![Collect, Cluster, Summarize, Drift, Consolidate, Tunnel, Decay]

// After (5 stages)
vec![Summarize, Drift, Consolidate, Tunnel, Decay]
```

---

## Section 6: Module Organization

### New Code

```
src/skill/                              # New module
├── mod.rs                              # Module exports
├── recaller.rs                         # SkillRecaller - recall during prompt assembly
├── index.rs                            # SkillIndex - skill_index table CRUD
└── tools/
    ├── mod.rs
    ├── manage.rs                       # SkillManageTool (create/patch/delete/list)
    └── search.rs                       # SkillSearchTool (vector search)
```

### Modified Code

```
src/memory/reflection/
├── prompt.rs                           # Extend Skills section prompt
├── parser.rs                           # Add SkillExtraction parsing
└── mapper.rs                           # Skills mapping → call SkillIndex

src/memory/dreaming/
├── mod.rs                              # Remove Collect/Cluster stage registration
├── stages/
│   ├── drift.rs                        # Wire LLM arbitration
│   ├── synthesis.rs                    # Remove DBSCAN, use LLM
│   ├── tunnel.rs                       # Add LLM judgment layer
│   ├── summarize.rs                    # Replace with LLM summary
│   └── consolidate.rs                  # Remove promotion_scorer, use LLM
```

### Deleted Code

```
src/memory/dreaming/stages/collect.rs           # Entire file
src/memory/dreaming/stages/cluster.rs           # Entire file
src/memory/consolidation/promotion_scorer.rs    # Entire file
src/memory/value_estimator/signals.rs           # Entire file (+ parent if empty)
```

### Reused Code (No Modification)

- `src/tools/markdown_skill/parser.rs` — Parse SKILL.md frontmatter + content
- `src/tools/markdown_skill/generator.rs` — Generate SKILL.md files
- `src/tools/markdown_skill/loader.rs` — Startup filesystem scan
- `src/tools/markdown_skill/watcher.rs` — File change hot-reload
- `src/tools/traits.rs` — AlephTool trait for tool registration

### Dependency Graph

```
skill::tools::manage  →  skill::index (write index)
                      →  markdown_skill::generator (generate files)
                      →  markdown_skill::parser (parse files)

skill::recaller       →  skill::index (query index)

reflection::mapper    →  skill::tools::manage (create/update skills)

dreaming::stages::*   →  does NOT depend on skill module (independent refactor)
```

### Constraints

- `src/skill/` does NOT depend on `src/memory/dreaming/` (direction: reflection → skill, dreaming is independent)
- `src/skill/index.rs` may depend on `src/memory/store/` embedding infrastructure (reuse vector computation)
- `markdown_skill` module stays unchanged; new module reuses via calling, not modifying

---

## Section 7: Advantages Over Hermes-Agent

### 1. Rust Type-Safe Skill Validation

Hermes validates skills at runtime with Python string checks. Aleph uses Rust's type system:
- `SkillExtraction` struct guarantees field completeness at compile time
- SKILL.md parsing reuses `AlephSkillSpec` (serde_yaml deserialization) — format errors caught before write
- No separate security_scan needed — learned skills don't execute external commands (prompt-only text injection)

### 2. Semantic Retrieval vs Text Index

Hermes skill recall relies on keyword matching (skill name + description string search). Aleph has full vector search infrastructure:
- skill_index stores embeddings for semantic-level matching
- "handle borrow checker errors" matches `rust-lifetime-debugging` skill even without keyword overlap

### 3. Scope Isolation

Hermes skills are globally shared with no role isolation. Aleph's Global + Persona two-level scope:
- Coding assistant persona's skills don't pollute writing assistant persona's context
- Cross-persona knowledge (e.g., user preferences) stored as Global scope skills

### 4. Dreaming Pipeline Synergy

Hermes has no offline memory processing. Aleph's dreaming pipeline (refactored) provides additional knowledge evolution:
- **Drift detection**: LLM discovers skill content contradicts new facts → marks skill for update
- **Synthesis**: Accumulated LongTerm facts across days may produce new insights → written to DailyInsight, available for next reflection

### 5. Atomic Writes + File Watching

Existing markdown_skill module already provides:
- `watcher.rs`: notify + debouncer file monitoring, skill changes take effect immediately
- Users can directly edit `~/.aleph/skills/learned/` SKILL.md files with any editor, no LLM needed

---

## Summary

This redesign transforms Aleph's self-growth from an algorithm-heavy batch processing system into a prompt-first, LLM-driven skill extraction system. The key changes are:

1. **Skill extraction via reflection** — LLM extracts reusable knowledge at conversation end
2. **Skills as normalized prompts** — stored as human-readable SKILL.md files
3. **Dual-write storage** — filesystem (source of truth) + DB index (search acceleration)
4. **Hybrid recall** — frequent skills pinned + semantic retrieval for long tail
5. **Pipeline cleanup** — delete algorithmic judgment code, wire LLM into remaining stages
6. **Tool-first interface** — all skill CRUD through `skill_manage` tool (R9)

Total impact: ~6 files deleted, ~8 files modified, ~6 files created.
