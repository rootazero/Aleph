# Wiki Knowledge System Design

> Inspired by [Karpathy's LLM Wiki](https://github.com/karpathy/llm-wiki) — incremental, LLM-maintained knowledge bases that compound over time.

## Problem Statement

Current memory retrieval relies on atomic facts and vector search (RAG). When answering complex questions that require synthesizing multiple documents, the LLM must rediscover and piece together knowledge fragments every time. There is no persistent, structured knowledge layer that accumulates and cross-references information.

Karpathy's insight: instead of retrieving from raw documents at query time, the LLM **incrementally builds and maintains a persistent wiki** — a structured, interlinked collection of markdown files. The knowledge is compiled once and kept current, not re-derived on every query.

## Design Overview

Integrate a Wiki subsystem into Aleph's existing memory architecture. Wiki pages are:

- **First-class facts** (`FactType::Wiki`), on the same level as `FactType::Skill`
- **Generated from raw data** (user-provided articles, PDFs, web pages), not from secondary fact processing
- **Stored as Markdown files** with git version tracking
- **Interlinked via `[[wikilink]]` syntax**, with links automatically reflected in the knowledge graph
- **Agent-isolated**, each agent has its own wiki namespace

### Relationship to Existing Memory Layers

```
Raw Data (conversations, documents, web clips)
  │
  ├─ Facts Layer (atomic knowledge points, vector-indexed)
  │    ├─ Preference, Plan, Learning, Project, Personal, ...
  │    ├─ Skill  ← procedural knowledge from task execution chains
  │    └─ Wiki   ← structured knowledge pages from ingested documents (NEW)
  │
  ├─ Knowledge Graph (entities + relationships)
  │    └─ Wiki pages are graph nodes, wikilinks are graph edges
  │
  └─ Synthesis (cross-cluster pattern extraction, dreaming pipeline)
```

Wiki and Skill are peers — both generated from raw data, both are long-term knowledge, both participate in the knowledge graph. Wiki focuses on **declarative knowledge** (what things are, how they relate), Skill focuses on **procedural knowledge** (how to do things).

## Data Model

### FactType Extension

New variant in `FactType` enum:

```rust
pub enum FactType {
    // ... existing variants ...
    Skill,
    Wiki,  // Structured knowledge page from ingested documents
}
```

Default mappings:
- `FactType::Wiki` → `path: "aleph://wiki/"`
- `FactType::Wiki` → `MemoryCategory::Patterns`
- `FactType::Wiki` → `MemoryTier::LongTerm`

### Fact as Anchor

Each wiki page has a corresponding `MemoryFact` entry that serves as its anchor in the facts system:

- `content`: 1-2 sentence summary of the page (used for embedding and search)
- `path`: `aleph://wiki/{agent_id}/{page_slug}.md` (maps to physical file)
- `parent_path`: `aleph://wiki/{agent_id}/` (enables listing all pages for an agent)
- `embedding`: Vector of the summary (for similarity search)
- `tags`: From Markdown frontmatter
- Standard fact fields: `confidence` (default 0.9, wiki is curated knowledge), `strength` (default 1.0), `tier` (LongTerm), `scope` (Global), etc.

No new fields are added to `MemoryFact`. The `path` field already provides the mapping to the physical Markdown file.

### Wiki Markdown Page Format

```markdown
---
title: Rust Ownership Model
aliases: [ownership, borrow checker]
tags: [rust, memory-safety, programming-language]
sources: [fact_id_1, fact_id_2]
created: 2026-04-10
updated: 2026-04-10
---

# Rust Ownership Model

Overview content...

## Core Rules

Detailed content...

## Comparison with C++

[[cpp-memory-management]] uses manual management...

## Related Concepts

- [[rust-lifetimes]]
- [[rust-smart-pointers]]
```

YAML frontmatter is maintained by the LLM. `sources` traces back to original fact IDs. `aliases` enables multi-name matching.

## Storage & Git Integration

### Physical Directory Structure

```
~/.aleph/data/wiki/
├── .git/                          # Git repo (auto-initialized by Aleph)
├── {agent_id}/
│   ├── index.md                   # Auto-generated page index
│   ├── rust-ownership-model.md
│   ├── llm-prompt-engineering.md
│   └── assets/                    # Images and attachments
│       └── ownership-diagram.png
└── {another_agent_id}/
    ├── index.md
    └── ...
```

- Entire `~/.aleph/data/wiki/` is a single git repo, agents isolated by directory
- Each agent directory has an auto-maintained `index.md`
- `assets/` stores binary files referenced by wiki pages

### Git Strategy

**Auto-commit triggers:**
- WikiManageTool creates/updates/deletes a page
- WikiIngestStage completes batch ingestion
- WikiLintStage auto-fixes issues

**Commit message format:**
```
wiki({agent_id}): {action} {page_slug}
```
Actions: `create`, `update`, `delete`, `lint-fix`, `dream-ingest`

**Implementation:**
- `WikiGitManager` struct wrapping git CLI commands
- `commit_changes(agent_id, action, page_slug)` method
- No auto-push — local repo only, user optionally configures remote

### index.md Auto-Generation

Each agent's `index.md` is generated by Aleph (not LLM) from wiki facts and frontmatter:

```markdown
# Wiki Index

> Auto-generated. Do not edit manually.
> Last updated: 2026-04-10 14:30

## Pages (12)

| Page | Summary | Tags | Updated |
|------|---------|------|---------|
| [Rust Ownership Model](rust-ownership-model.md) | Core memory safety mechanism in Rust | rust, memory-safety | 2026-04-10 |
| [LLM Prompt Engineering](llm-prompt-engineering.md) | Best practices for prompt design | llm, prompt | 2026-04-09 |
```

LLM reads index.md first to locate relevant pages before drilling into content — avoids vector search for most queries.

## WikiManageTool (Active Ingestion)

### Tool Schema

```rust
pub struct WikiManageTool;

// Exposed to LLM
{
    "name": "wiki_manage",
    "description": "Create, update, query, or delete wiki knowledge pages",
    "parameters": {
        "action": "create | update | query | delete | list",
        "page_slug": "string (optional, required for create/update/delete)",
        "content": "string (optional, raw source text to ingest)",
        "query": "string (optional, for query action)"
    }
}
```

### Action Flows

**create:**
1. User provides raw text (web page, PDF, article content)
2. LLM generates: page slug, title, summary, tags, structured Markdown
3. LLM identifies concepts linkable to existing wiki pages, inserts `[[wikilink]]`
4. Write Markdown file to `~/.aleph/data/wiki/{agent_id}/{slug}.md`
5. Create `FactType::Wiki` fact (content = summary, path = `aleph://wiki/{agent_id}/{slug}.md`)
6. Parse `[[wikilink]]` → generate graph edges
7. Update `index.md`
8. Git commit

**update:**
1. Read existing Markdown file + user-provided new information
2. LLM integrates updates: revise content, update frontmatter `updated` and `sources`, check for new/removed `[[wikilink]]`
3. Update corresponding fact's content (summary) and embedding
4. Re-parse wikilinks → update graph edges
5. Update `index.md`
6. Git commit

**query:**
1. Read agent's `index.md`
2. Match relevant pages by title/summary (FTS5 + fact embedding as fallback)
3. Read matched wiki page content
4. Return to LLM for answering user questions

**delete:**
1. Delete Markdown file
2. Mark corresponding fact as `is_valid = false`
3. Clean up related graph edges
4. Update `index.md`
5. Git commit

**list:**
1. Read and return `index.md` content

### R9 Alignment (Everything is a Tool)

All wiki operations are accessible through natural language conversation:
- "Organize this article into the wiki" → `create`
- "Update the Rust wiki page with this new info" → `update`
- "What does my wiki say about LLMs?" → `query`
- "List all wiki pages" → `list`

## Dreaming Pipeline Extensions (Passive Path)

### WikiIngestStage

**Position:** After ConsolidateStage (facts are stable), before WikiLintStage.

**Trigger condition:**
```rust
async fn should_run(&self, ctx: &DreamContext) -> bool {
    has_unprocessed_documents(ctx) && cooldown_elapsed(ctx)
}
```

**Flow:**
1. Query `FactSource::Document` facts not associated with any wiki page
2. Cluster by topic (embedding similarity)
3. For each cluster, determine: create new page vs. update existing page (similarity threshold)
4. LLM generates/updates Markdown content (requires `ctx.provider`)
5. Write files, create/update facts, update graph, update index.md
6. Batch git commit: `wiki({agent_id}): dream-ingest {n} pages`

**Configuration:**
```rust
pub struct WikiIngestConfig {
    pub enabled: bool,
    pub max_pages_per_run: usize,    // default 10
    pub similarity_threshold: f32,    // default 0.75
    pub cooldown_days: u32,           // default 1
}
```

### WikiLintStage

**Position:** After WikiIngestStage, before DecayStage.

**Checks:**

| Check | Method | Action |
|-------|--------|--------|
| Broken links | Parse all `[[wikilink]]`, verify target exists | Mark as broken, report |
| Orphan pages | No inbound wikilinks and no graph edges | Report (no auto-delete) |
| Stale content | Page's `sources` reference invalidated facts | Mark page for refresh |
| Missing pages | High-weight graph nodes without wiki pages | Suggest creation |
| Frontmatter gaps | Missing required fields (title, tags, updated) | Auto-fix |

**Design principle:** WikiLintStage is primarily **diagnostic + reporting**. Only lightweight auto-fixes (e.g., frontmatter completion). Heavy rewrites deferred to WikiIngestStage or user-triggered updates.

**Output:**
```rust
pub struct WikiLintReport {
    pub broken_links: Vec<(String, String)>,    // (page, broken_target)
    pub orphan_pages: Vec<String>,
    pub stale_pages: Vec<String>,
    pub suggested_pages: Vec<String>,
    pub auto_fixed: usize,
}
```

### Updated Pipeline Order

```
Daily Pipeline:
1. SummarizeStage
2. DriftDetectStage
3. ConsolidateStage
4. WikiIngestStage        ← NEW
5. WikiLintStage          ← NEW
6. TunnelDiscoveryStage
7. DecayStage

Weekly Pipeline:
Daily + DeepSynthesisStage
```

## Knowledge Graph Integration

### Wikilink → Graph Edge

When a wiki page is written, parse `[[wikilink]]` from Markdown content:

```rust
fn extract_wikilinks(markdown: &str) -> Vec<String> {
    // Regex: [[page-slug]] or [[page-slug|display text]]
    // Returns target page_slug list
}
```

For each wikilink:
- Find target page_slug's corresponding wiki fact
- Create `GraphEdge`: `from = current page fact_id, to = target page fact_id, relation = "wiki_references"`
- If target page doesn't exist, record as broken link

Additionally, `update_from_fact()` extracts entities from the wiki fact's content (summary), generating `co_occurs` edges. Each wiki page thus has two types of graph relationships:
- **Explicit**: `wiki_references` (from wikilinks)
- **Implicit**: `co_occurs` (from entity extraction)

### Wiki Pages as Graph Nodes

Each wiki fact corresponds to a `GraphNode`:

```rust
GraphNode {
    id: "gn_{fact_id}",
    name: page_title,
    kind: "wiki_page",
    aliases: frontmatter.aliases,
    metadata: { "fact_type": "Wiki", "tags": [...], "slug": "..." }
}
```

Wiki pages naturally interconnect with other graph entities (people, projects, concepts). Query "which wiki pages relate to Rust" → graph traversal from `Rust` node to all connected `wiki_page` nodes.

### Asymmetric Bidirectional Sync

- **Markdown → Graph (automatic):** LLM writes `[[wikilink]]` in pages, Aleph parses and generates/updates graph edges
- **Graph → Markdown (not automatic):** Graph edges from other sources (conversations, fact co-occurrence) are NOT injected back into Markdown. At query time, LLM can see "this wiki page is also related to X, Y in the graph" and may choose to add `[[wikilink]]` in the next update

This keeps Markdown clean and human-readable while the graph provides richer discovery.

### Query Routing

```
User question
  │
  ├─ WikiManageTool.query()
  │    ├─ 1. Read index.md, match relevant page titles/summaries
  │    ├─ 2. If insufficient, FTS5 full-text search wiki facts
  │    ├─ 3. If still insufficient, embedding vector search wiki facts
  │    └─ 4. Read matched Markdown files, return full content
  │
  └─ Existing MemoryRecall (facts retrieval)
       └─ Works as before; wiki facts are naturally included in results
```

No dedicated router needed — wiki facts live in the facts table and are naturally hit by existing vector search and FTS5. WikiManageTool.query provides a more efficient wiki-specific entry point (read index first, then drill into pages).

## Implementation Phases

### Phase 1: Core Usability

| Component | File | Description |
|-----------|------|-------------|
| `FactType::Wiki` | `src/memory/context/enums.rs` | New enum variant + default mappings |
| `WikiManageTool` | `src/wiki/tools/manage.rs` | create/update/query/delete/list |
| Wikilink parser | `src/wiki/wikilink.rs` | `[[link]]` extraction + graph edge generation |
| Wiki Git manager | `src/wiki/git.rs` | Init repo, auto-commit |
| index.md generator | `src/wiki/index.rs` | Generate index from wiki facts |
| Wiki module entry | `src/wiki/mod.rs` | Module organization |

After Phase 1, users can:
- Ingest articles into the wiki via conversation
- Query wiki knowledge
- Navigate between pages via `[[wikilink]]`
- Track all changes via git history

### Phase 2: Dreaming Integration

| Component | File | Description |
|-----------|------|-------------|
| `WikiIngestStage` | `src/memory/dreaming/stages/wiki_ingest.rs` | Passive ingestion of unprocessed documents |
| `WikiLintStage` | `src/memory/dreaming/stages/wiki_lint.rs` | Health checks and diagnostics |
| WikiLintReport | `src/memory/dreaming/report.rs` | Extend DreamReport |
| Pipeline registration | `src/memory/dreaming/mod.rs` | Insert new stages |

### Phase 3: Deep Enhancement (On-Demand)

- Richer graph edge types (`wiki_defines`, `wiki_contradicts`)
- LLM-driven deep entity extraction from page content
- Graph topology recommendations ("suggest creating a page about X")
- Wiki page staleness decay (low access + long unupdated → mark stale)

### Explicit Non-Goals

- **No Obsidian integration** — wiki is internal to Aleph, not exposed to external editors
- **No wiki UI rendering** — wiki interaction is through conversation (R9: everything is a tool)
- **No Raw data layer rebuild** — Karpathy's Raw sources layer already exists in Aleph (conversation history, Document facts)
- **No auto-push** — git repo is local-only, user optionally configures remote
