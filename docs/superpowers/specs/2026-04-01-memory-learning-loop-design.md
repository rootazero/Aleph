# Memory Learning Loop Enhancement

> Inspired by hermes-agent's session memory management, adapted to Aleph's Rust architecture and design principles.

## Context

Hermes-agent implements a "learning loop" — the agent generates skills from experience, optimizes during use, and builds deep user profiles across sessions. Aleph's memory infrastructure is far more sophisticated (knowledge graph, VFS, event sourcing, decay curves, multi-layer compression, hybrid retrieval), but lacks several practical "last mile" capabilities that Hermes delivers effectively.

This design closes four specific gaps with minimal changes, leveraging Aleph's existing infrastructure.

## Scope

| Item | Type | Effort |
|------|------|--------|
| A. Cross-session search tool | New tool + FTS5 index | Medium |
| B. Memory guidance in prompt | Prompt builder change | Small |
| C. Memory content scanner | New module | Small |
| D. Reflection skill extraction | Reflection service extension | Small |

Total: 2 new files, 5-6 modified files.

---

## A. Cross-Session Search Tool (SessionSearchTool)

### Problem
Agent cannot search past conversation transcripts. When a user references prior discussions, the agent has no way to recall them, leading to repetitive questions.

### Design

**1. FTS5 Index on Session Messages**

Add a FTS5 virtual table to the session_manager SQLite database:

```sql
CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts
USING fts5(content, content=messages, content_rowid=id);
```

Sync FTS5 index via explicit INSERT into `messages_fts` alongside the main `messages` INSERT (same transaction, no triggers — explicit is more debuggable).

**2. New `SessionSearchTool`**

File: `builtin_tools/session_search.rs`

- **Input**: `query: String`, `max_results: usize` (default 5)
- **Flow**: FTS5 match → group by session_key → return top sessions with context
- **Output per match**: session_key, timestamp, matching snippet, 2 messages before/after as context window
- **Registration**: `pub mod session_search` in `builtin_tools/mod.rs`

### Decisions

- **No vector search** — session messages lack embeddings; adding them is disproportionate cost
- **No LLM summarization** — violates R3 (Core Minimalism); agent judges relevance itself (R8 LLM Sovereignty)
- **No changes to SessionStore trait** — FTS5 is an implementation detail of the SQLite backend

---

## B. Memory Guidance in System Prompt

### Problem
Agent lacks concrete guidance on when to save memories, when to search sessions, and when to extract skills from experience.

### Design

Append a `## Memory Protocol` section to the `BASE_BEHAVIOR` constant in `agent_loop/prompt_builder.rs`:

```markdown
## Memory Protocol

### When to Save Memory
- User corrections and preferences → highest priority, prevents repeating mistakes
- Environment facts (OS, tools, project conventions) → reduces future context gathering
- Do NOT save: task progress, session outcomes, completed-work logs, temporary TODO state

### When to Search Sessions
- User references something from a past conversation
- You suspect relevant cross-session context exists
- Before asking user to repeat information they may have already told you
- Use session_search tool — sessions have verbatim transcripts

### When to Extract Skills
- After completing a complex task (5+ tool calls)
- After fixing a tricky error with a non-obvious solution
- After discovering a reusable workflow or pattern
- Save via memory as a Lesson-type fact with clear steps
```

### Decisions

- **Inside `BASE_BEHAVIOR`, not a new section** — keeps prompt structure flat, no additional separator
- **English only** — LLM processes English prompts more effectively; response language controlled by soul
- **Skills point to Lesson facts** — Aleph's MemoryFact (FactType::Lesson + VFS path) is strictly superior to file-based SKILL.md

---

## C. Memory Content Scanner

### Problem
Stored memory content could contain prompt injection patterns. A malicious or compromised input that gets persisted would be re-injected into every future session's system prompt.

### Design

**1. New module: `memory/content_scanner.rs`**

Stateless, pure-function scanner:

```rust
pub enum ScanVerdict {
    Clean,
    Rejected { reason: String, pattern: &'static str },
}

pub fn scan_content(content: &str) -> ScanVerdict
```

**2. Scan Rules**

| Category | Patterns |
|----------|----------|
| Invisible Unicode | U+200B, U+FEFF, U+200E, U+200F, U+2060, U+2062-2064 |
| Prompt injection | `ignore previous`, `you are now`, `system prompt`, `new instructions` (case-insensitive) |
| Data exfiltration | `curl.*api[_-]?key`, `wget.*token`, `cat.*\.env` (regex) |

**3. Integration Point**

In `LanceMemoryBackend`'s `insert_fact` and `update_fact_content` implementations:
- Call `scan_content(&fact.content)` before write
- `Rejected` → return `AlephError`, fact not persisted
- Log interception (pattern name only, not raw content)

### Decisions

- **Not on the trait** — scanner is implementation detail, not contract (P4 Dependency Inversion)
- **No retroactive scan** — existing data not re-scanned on read
- **Deterministic rules only** — regex for known attack patterns; no LLM call (this IS the "safety hard filter" from R8's empowerment layer)
- **No read-path filtering** — performance cost unjustified; write-path is the trust boundary

---

## D. Reflection Skill Extraction

### Problem
Session-end reflection extracts Invariants, Derived facts, and Lessons, but doesn't identify reusable procedural knowledge (skills) that could help in future similar tasks.

### Design

**1. New `## Skills` category in reflection markdown**

```markdown
## Skills
- Cross-session search: FTS5 index + session grouping + context window extraction
```

**2. Parse & Map**

In `reflection/service.rs`:
- `parse_reflection()` — add parsing for `## Skills` section
- `map_to_facts()` — map Skills entries to:
  - `fact_type`: `FactType::Lesson` (reuse, not new variant)
  - `path`: `aleph://knowledge/skills/{slug}` (distinguishes from regular lessons)
  - `tier`: `MemoryTier::LongTerm`
  - `scope`: `MemoryScope::Global`

**3. Reflection Prompt Enhancement**

Add Skills category description to the LLM prompt template used by the reflection caller:

```
## Skills
Reusable approaches, workflows, or patterns discovered during this session.
Only include if the approach is non-trivial and likely to recur.
Each entry: a concise name + the key steps or insight.
```

### Decisions

- **Reuse `FactType::Lesson`** — Lesson and Skill have fuzzy semantic boundary; VFS path prefix `aleph://knowledge/skills/` provides precise retrieval (P6 Occam's Razor)
- **No separate skill management subsystem** — Aleph's VFS + fact system (embedding, decay, confidence, access tracking) is already more powerful than Hermes' file-based SKILL.md
- **No second LLM call** — one reflection call outputs all categories including Skills
- **LongTerm + Global** — skills are durable cross-workspace knowledge by nature

---

## Files Changed

| File | Change Type | Description |
|------|-------------|-------------|
| `builtin_tools/session_search.rs` | **New** | SessionSearchTool implementation |
| `memory/content_scanner.rs` | **New** | Prompt injection scanner |
| `builtin_tools/mod.rs` | Modify | Register SessionSearchTool |
| `agent_loop/prompt_builder.rs` | Modify | Add Memory Protocol to BASE_BEHAVIOR |
| `gateway/session_manager/` | Modify | Add FTS5 index + search method |
| `memory/store/lance/` | Modify | Integrate content scanner on write path |
| `memory/reflection/service.rs` | Modify | Parse Skills category, map to facts |
| Reflection prompt caller (agent_loop or memory caller) | Modify | Add Skills category to the reflection LLM prompt — locate via `grep -r "Invariants" src/` to find the prompt template |

## Non-Goals

- No frozen memory snapshot (Hermes pattern) — Aleph's per-turn context composition is more flexible
- No separate USER.md equivalent — Aleph's consolidation analyzer + VFS `aleph://user/` path already serves this purpose
- No Honcho-style external service integration — Aleph is self-hosted by design
- No progressive skill disclosure — Aleph's skill prefetcher already handles this
