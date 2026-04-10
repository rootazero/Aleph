# Self-Growth Module Redesign

**Date**: 2026-04-10
**Status**: Approved (Rev.2 — Skill-as-Fact integration)
**Inspired by**: hermes-agent skill extraction mechanism
**Principle**: Prompt-first (R8 LLM Sovereignty + R9 Everything is a Tool + R10 Intelligence Lives in the Prompt)

---

## Overview

Redesign Aleph's self-growth capability to follow the prompt-first principle. Replace algorithmic memory processing (DBSCAN clustering, 8-dimensional scoring, hardcoded signal detection) with LLM-driven skill extraction during conversation reflection. Skills are normalized prompts — reusable knowledge extracted by the LLM and stored as facts within the existing MemoryStore.

### Core Concept

```
Conversation ends
    → Reflection (existing, extended)
    → LLM outputs structured skill definitions
    → skill_manage tool inserts FactType::Skill into MemoryStore
    → Knowledge graph connects skill to related concepts
    → Next conversation: hybrid_retrieval with fact_type=Skill filter
    → LLM uses skill, evaluates freshness, patches if outdated
    → Dreaming pipeline naturally manages lifecycle (decay, drift, promotion)
```

### Key Insight (Rev.2)

Skills are stored as `FactType::Skill` facts in the existing MemoryStore rather than in a separate skill_index table. This means skills automatically inherit:
- Vector + BM25 hybrid retrieval (RAG)
- Knowledge graph connections
- Tier model (ShortTerm → LongTerm → Core)
- Decay (unused skills naturally fade)
- Drift detection (contradictions caught)
- Scope isolation (Global / Persona via existing MemoryScope)

Zero new storage infrastructure required.

---

## Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Trigger mode | Real-time (reflection) | Skill extraction is semantic judgment; needs full conversation context |
| Storage | FactType::Skill in MemoryStore | Reuses all existing infra (RAG, graph, decay, drift); natural lifecycle |
| File export | Optional SKILL.md export | Human-readable but not source of truth; MemoryStore is authoritative |
| Pipeline cleanup | Layered: keep infra, remove algorithmic judgment | R8: decay is math (keep), clustering is semantic judgment (remove) |
| Skill evolution | Real-time via reflection + dreaming drift detection | LLM judges staleness at use time; drift stage catches contradictions |
| Skill recall | On-demand via hybrid_retrieval (no pinning) | Scales naturally; decay handles cleanup; no separate index |
| Skill scope | Two-level (Global + Persona) via existing MemoryScope | Skills bind to roles; workspace-specific knowledge belongs in regular facts |

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
MemoryStore: insert FactType::Skill fact
    ↓ embedding auto-generated
    ↓ knowledge graph edges created (skill → related concepts)
    ↓ VFS path: aleph://skills/{category}/{name}
    ↓
Next conversation prompt assembly
    ↓
hybrid_retrieval(fact_type_filter=Skill)
    ↓ vector + BM25 + RRF
Inject top-K relevant skill contents into system prompt
    ↓
LLM uses skill → reflection evaluates → patch if outdated
    ↓
Dreaming pipeline (background)
    ↓ decay: unused skills fade
    ↓ drift: contradictions flagged
    ↓ consolidate: validated skills promoted ShortTerm → LongTerm → Core
```

### Skill Lifecycle via Memory Tiers

| Tier | Behavior | Decay |
|------|----------|-------|
| **ShortTerm** | New skills start here. Retrieved via RAG when relevant. | 1 day half-life (fast fade if unused) |
| **LongTerm** | Skills validated through repeated use. Higher retrieval priority. | 30 days half-life |
| **Core** | Fundamental skills. Always retrieved (strength never drops below threshold). | 365 days half-life |

Promotion happens via the dreaming consolidate stage (LLM-driven after refactor). A skill used repeatedly across sessions naturally accumulates signal_count, qualifying it for promotion.

### Constraints

- Reflection is the sole entry point for skill extraction (no mid-conversation interruption)
- LLM operates through `skill_manage` tool for all CRUD (R9: Everything is a Tool)
- Learned skills are prompt-only text — they don't execute commands
- MemoryStore is source of truth; SKILL.md export is optional convenience

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
1. Create `FactType::Skill` fact via skill_manage tool logic
2. VFS path: `aleph://skills/{category}/{name}`
3. Fact content = skill markdown body (description, when-to-use, steps, pitfalls)
4. Metadata: name, category, description stored in fact metadata fields
5. Backward compatible: if LLM outputs old one-line format, fall back to existing behavior

---

## Section 3: Skill Storage & Tools

### Storage Model: FactType::Skill

Skills are stored as regular facts in MemoryStore with `fact_type = Skill`:

```rust
// New variant added to existing FactType enum
pub enum FactType {
    // ... existing variants ...
    Skill,  // Reusable procedural knowledge extracted by LLM
}
```

**Fact fields mapping:**

| Fact Field | Skill Usage |
|------------|-------------|
| `content` | Full skill markdown (When to Use, Steps, Pitfalls) |
| `fact_type` | `FactType::Skill` |
| `tier` | ShortTerm (new) → LongTerm (validated) → Core (fundamental) |
| `scope` | Global or Persona (via existing MemoryScope) |
| `vfs_path` | `aleph://skills/{category}/{name}` |
| `embedding` | Auto-generated from content for semantic retrieval |
| `strength` | Managed by decay stage |
| `signal_count` | Incremented on each use (retrieval + loading) |
| `confidence` | Set by LLM during extraction (default 0.8) |
| `metadata` | JSON: `{"skill_name": "...", "skill_category": "...", "skill_description": "..."}` |

### Optional SKILL.md Export

For human readability, a `skill_export` tool can dump skills to filesystem:

```
~/.aleph/skills/learned/{category}/{name}/SKILL.md
```

This is a one-way export (MemoryStore → filesystem), not a sync mechanism. Users who edit the file can re-import via `skill_manage(action=import)`.

### skill_manage Tool

Static `AlephTool` implementation:

```rust
pub struct SkillManageTool { /* ... */ }

pub struct SkillManageArgs {
    pub action: SkillAction,     // create | patch | delete | list | export
    pub name: Option<String>,
    pub category: Option<String>,
    pub scope: Option<String>,   // global | persona (default: persona)
    pub content: Option<String>, // full skill markdown (for create)
    pub description: Option<String>, // one-line description
    pub old_text: Option<String>,// patch old text
    pub new_text: Option<String>,// patch new text
}
```

Operations:
- **create**: validate name → create FactType::Skill fact → generate embedding → insert via MemoryStore::insert_fact()
- **patch**: find fact by VFS path → update content → regenerate embedding → update via MemoryStore::update_fact()
- **delete**: find fact by VFS path → invalidate via MemoryStore::invalidate_fact()
- **list**: query MemoryStore with fact_type=Skill filter → return name + description list
- **export**: dump skill fact content to SKILL.md file on disk

### skill_search Tool

```rust
pub struct SkillSearchTool { /* ... */ }

pub struct SkillSearchArgs {
    pub query: String,           // natural language query
    pub scope: Option<String>,   // filter scope
    pub limit: Option<usize>,    // default 5
}
```

Thin wrapper around existing `hybrid_retrieval` with `fact_type_filter = Skill`. Returns matching skill names + descriptions + relevance scores.

---

## Section 4: Skill Recall (Prompt Assembly)

### Recall Strategy: Pure RAG (No Pinning)

Unlike the previous design that pinned frequent skills, all skill recall now goes through existing hybrid_retrieval. This scales naturally:

- **Few skills (0-20)**: All relevant skills retrieved efficiently
- **Many skills (100+)**: Vector search + BM25 surfaces best matches; decay has already faded irrelevant ones
- **Thousands of skills**: Same mechanism; no index bloat in system prompt

### Recall Flow

```
Prompt assembly begins
    ↓
1. Take current conversation context (user message + recent history)
    ↓
2. hybrid_retrieval(query, fact_type_filter=Skill, scope_filter=current, limit=5)
    → vector search + BM25 + RRF fusion
    → returns top-K skill facts ranked by relevance
    ↓
3. For each retrieved skill:
    → increment signal_count (tracks usage for tier promotion)
    → inject full content into system prompt
    ↓
4. Assemble as system prompt fragment
```

### Injection Format

```
## Learned Skills (auto-retrieved)
The following skills were learned from past sessions and may be relevant.
Follow them if applicable. If a skill is outdated, update it via skill_manage.

### rust-lifetime-debugging
When encountering Rust lifetime errors that involve...
1. Check the borrow scope...
2. ...

### async-tokio-patterns
Common async patterns in Tokio...
1. ...
```

### Token Budget

- Each skill: ~100-300 tokens (full content, not just index)
- Retrieved limit: 5 skills per conversation
- Total: ~500-1500 tokens (acceptable; skills are high-value context)
- Skills with low strength (decayed) won't surface — natural token control

### Tier-Based Retrieval Boost

Core tier skills get a retrieval boost (higher base strength → higher RRF score). This means fundamental skills surface more easily without explicit pinning. The existing strength field acts as an implicit frequency signal.

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
- **Skill-aware**: drift detection now also catches skill facts that contradict newer knowledge

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
- **Skill-aware**: tunnels can now connect skills to related concept clusters

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
- **Skill-aware**: FactType::Skill facts participate in the same promotion pipeline — frequently used skills naturally get promoted to LongTerm/Core

### LLM Unavailability Fallback

When LLM is unavailable (offline, API failure), LLM-driven stages (Summarize, Drift, Consolidate, Tunnel) skip their core logic gracefully. Only Decay (pure math) always executes. Specifically:
- **Consolidate**: skips promotion, still executes pruning (strength < 0.1 invalidation)
- **Drift/Tunnel/Summarize**: skip entirely, log warning, return empty results

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
src/skill/                              # New module (thin layer over MemoryStore)
├── mod.rs                              # Module exports, SkillExtraction struct
├── recaller.rs                         # Skill recall via hybrid_retrieval wrapper
└── tools/
    ├── mod.rs
    ├── manage.rs                       # SkillManageTool (create/patch/delete/list/export)
    └── search.rs                       # SkillSearchTool (hybrid_retrieval wrapper)
```

### Modified Code

```
src/memory/
├── context/types.rs                    # Add FactType::Skill variant
├── reflection/
│   ├── prompt.rs                       # Extend Skills section prompt
│   ├── parser.rs                       # Add SkillExtraction parsing
│   └── mapper.rs                       # Skills mapping → create FactType::Skill facts
└── dreaming/
    ├── mod.rs                          # Remove Collect/Cluster stage registration
    ├── stages/
    │   ├── drift.rs                    # Wire LLM arbitration
    │   ├── synthesis.rs                # Remove DBSCAN, use LLM
    │   ├── tunnel.rs                   # Add LLM judgment layer
    │   ├── summarize.rs               # Replace with LLM summary
    │   └── consolidate.rs             # Remove promotion_scorer, use LLM
```

### Deleted Code

```
src/memory/dreaming/stages/collect.rs           # Entire file
src/memory/dreaming/stages/cluster.rs           # Entire file
src/memory/consolidation/promotion_scorer.rs    # Entire file
src/memory/value_estimator/signals.rs           # Entire file (+ parent if empty)
```

### Reused Code (No Modification)

- `src/tools/markdown_skill/generator.rs` — Generate SKILL.md for export
- `src/tools/markdown_skill/parser.rs` — Parse SKILL.md for import
- `src/tools/traits.rs` — AlephTool trait for tool registration
- `src/memory/hybrid_retrieval/` — RAG retrieval (used by skill recall)
- `src/memory/store/` — MemoryStore trait (used by skill CRUD)

### Dependency Graph

```
skill::tools::manage  →  memory::store (MemoryStore fact CRUD)
                      →  markdown_skill::generator (optional export)

skill::tools::search  →  memory::hybrid_retrieval (RAG search)

skill::recaller       →  memory::hybrid_retrieval (RAG search)

reflection::mapper    →  skill::tools::manage (create/update skill facts)

dreaming::stages::*   →  does NOT depend on skill module (independent refactor)
                      →  FactType::Skill facts processed naturally alongside other facts
```

### Constraints

- `src/skill/` is a thin layer — most logic lives in existing MemoryStore and hybrid_retrieval
- `src/skill/` does NOT depend on `src/memory/dreaming/` (direction: reflection → skill, dreaming is independent)
- Dreaming stages process FactType::Skill facts alongside all other facts — no special-casing needed
- `markdown_skill` module stays unchanged; only used for optional SKILL.md export/import

---

## Section 7: Advantages Over Hermes-Agent

### 1. Unified Memory Architecture

Hermes maintains separate systems for skills (filesystem) and memory (MEMORY.md). Aleph stores skills as facts in the same MemoryStore, gaining:
- Single retrieval pipeline (RAG) for all knowledge types
- Knowledge graph connects skills to related facts and concepts
- No separate index to maintain or rebuild

### 2. Natural Lifecycle Management

Hermes skills persist forever unless manually deleted. Aleph skills have natural lifecycle:
- **Decay**: Unused skills fade via Ebbinghaus curve (ShortTerm: 1-day half-life)
- **Promotion**: Frequently used skills promoted to LongTerm/Core (longer half-life)
- **Drift detection**: Skills contradicting newer knowledge get flagged for update
- **No explosion**: Decay naturally prevents skill accumulation beyond useful capacity

### 3. Semantic Retrieval vs Text Index

Hermes skill recall relies on keyword matching (skill name + description string search). Aleph uses:
- Vector search + BM25 + RRF fusion for semantic-level matching
- "handle borrow checker errors" matches `rust-lifetime-debugging` skill even without keyword overlap

### 4. Scope Isolation

Hermes skills are globally shared with no role isolation. Aleph's Global + Persona scope via existing MemoryScope:
- Coding assistant persona's skills don't pollute writing assistant persona's context
- Cross-persona knowledge stored as Global scope skills

### 5. Knowledge Graph Integration

Skills participate in the knowledge graph:
- Tunnel stage discovers cross-domain connections between skills and other facts
- Graph edges link skills to related concepts, enabling association-based retrieval
- Example: a `rust-lifetime-debugging` skill gets graph-linked to facts about specific Rust projects

### 6. Rust Type Safety

- `SkillExtraction` struct guarantees field completeness at compile time
- `FactType::Skill` variant enforced by Rust's enum exhaustiveness checks
- No runtime type confusion between skills and other fact types

---

## Summary

This redesign transforms Aleph's self-growth from an algorithm-heavy batch processing system into a prompt-first, LLM-driven skill extraction system. The Rev.2 change integrates skills directly into the MemoryStore as `FactType::Skill` facts, eliminating the need for separate storage infrastructure.

Key changes:

1. **Skill extraction via reflection** — LLM extracts reusable knowledge at conversation end
2. **Skills as facts** — `FactType::Skill` in MemoryStore with full RAG/graph/decay/drift support
3. **On-demand recall** — hybrid_retrieval with type filter, no index pinning needed
4. **Natural lifecycle** — decay fades unused skills, promotion elevates validated ones, drift catches contradictions
5. **Pipeline cleanup** — delete algorithmic judgment code, wire LLM into remaining stages
6. **Tool-first interface** — all skill CRUD through `skill_manage` tool (R9)
7. **Optional export** — SKILL.md files for human readability, not source of truth

Total impact: ~4 files deleted, ~8 files modified, ~4 files created. Significantly less new code than Rev.1 due to MemoryStore reuse.
