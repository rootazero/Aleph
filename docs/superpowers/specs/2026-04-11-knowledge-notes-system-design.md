# Knowledge Notes System Design

> Date: 2026-04-11
> Status: Draft
> Scope: Memory system restructuring + Canvas visualization overhaul

## Summary

Replace the current facts + GraphNode/GraphEdge architecture with a unified **Knowledge Notes** system inspired by Obsidian. Markdown files become the source of truth for all knowledge, SQLite serves as a rebuildable index, and `[[wikilinks]]` provide the graph structure for Canvas visualization.

## Problem Statement

1. **Facts are too fragmented** — each fact is an atomic sentence, producing hundreds of tiny nodes unsuitable for graph visualization
2. **Facts lack titles** — no display name field, causing cluttered Canvas node labels
3. **No connections between facts** — Canvas shows disconnected nodes without meaningful edges
4. **Canvas lacks polish** — missing click-to-center animation, hover highlighting, continuous drift, and focus effects
5. **Redundant data model** — facts, graph_nodes, graph_edges, and memory_entities form a complex 4-table system with brittle bridging

## Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Data source of truth | Markdown files | Human-readable, LLM-editable, portable, versionable |
| SQLite role | Rebuildable index only | No sync problem — index is derived from files |
| GraphNode/GraphEdge |废弃 (remove) | Fully replaced by notes + wikilinks |
| Note granularity | LLM decides (mixed) | R8 LLM Sovereignty — system doesn't enforce topic vs entity |
| Canvas interaction | Obsidian style | Click=select, double-click=local view, hover=highlight neighbors |
| Node visual style | Obsidian native | Small dot + glow, size by link count, single color (purple), title as primary label |
| Primary key | Filename (sans .md) | Simple, human-readable, no UUID indirection |
| Rename strategy | File rename + wikilink cascade + re-index | Same as Obsidian behavior |

## Architecture

### Directory Structure

```
~/.aleph/notes/
├── 编辑器偏好.md
├── Rust学习历程.md
├── Tokyo旅行计划.md
├── Aleph项目.md
└── ...
```

### Markdown File Format

```markdown
---
category: preference
tags: [editor, vim, neovim]
created: 2026-04-01
updated: 2026-04-10
---

- 用户偏好用 Vim 写代码
- 用户使用 Neovim 配置，基于 LazyVim
- 用户偏好 Lua 插件而非 VimScript

相关：[[Rust学习历程]] [[开发环境]]
```

**Frontmatter fields:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| category | string | yes | Note category (preference, learning, project, plan, tool, personal, etc.) |
| tags | string[] | no | Searchable tags |
| created | date | yes | Creation date |
| updated | date | yes | Last modification date |

**Body:** Bullet-pointed facts (third-person statements) + freeform prose. `[[wikilinks]]` anywhere in the body create graph edges.

### SQLite Index Schema

All tables are derived from the markdown files and can be fully rebuilt at any time.

```sql
-- Note metadata index
CREATE TABLE notes_index (
    filename    TEXT PRIMARY KEY,   -- "编辑器偏好" (no .md)
    category    TEXT NOT NULL,
    tags_json   TEXT NOT NULL DEFAULT '[]',
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,
    last_accessed_at INTEGER,
    content_hash TEXT NOT NULL       -- SHA-256 of file content, for change detection
);

-- Vector embeddings for semantic search
CREATE VIRTUAL TABLE notes_vec_{dim} USING vec0(
    filename TEXT PRIMARY KEY,
    embedding float[{dim}]
);

-- Full-text search
CREATE VIRTUAL TABLE notes_fts USING fts5(
    filename,
    content,
    tokenize='unicode61'
);

-- Wikilink relationships (Canvas edges)
CREATE TABLE notes_links (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    from_note   TEXT NOT NULL,       -- source filename
    to_note     TEXT NOT NULL,       -- target filename
    context     TEXT,                -- surrounding text for context
    UNIQUE(from_note, to_note)
);
CREATE INDEX idx_links_from ON notes_links(from_note);
CREATE INDEX idx_links_to ON notes_links(to_note);
```

### Tables to Remove

- `facts` + `facts_fts` + `facts_vec_*`
- `graph_nodes`
- `graph_edges`
- `memory_entities`
- `compression_metadata` (review — may still be needed for other purposes)

### Indexing Strategy

**Full rebuild** (startup or on-demand):
1. Scan `~/.aleph/notes/*.md`
2. Parse frontmatter + extract wikilinks
3. Compute content_hash; skip files unchanged since last index
4. Generate embeddings for changed files
5. Populate all SQLite index tables

**Incremental update** (runtime):
- File created/modified → update that note's index + re-extract wikilinks + re-embed
- File deleted → remove index entries
- File renamed → delete old + insert new + update wikilinks in all referencing files + re-index affected files

## LLM Extraction Flow

### Current Flow (to be replaced)

```
Conversation → LLM → {facts, entities, relationships} → 3 separate tables
```

### New Flow

```
Conversation → LLM → {notes_updates} → create/append markdown files → incremental re-index
```

**LLM output structure:**

```json
{
  "updates": [
    {
      "note_title": "编辑器偏好",
      "action": "append",
      "new_facts": ["用户最近开始用 Helix 编辑器"],
      "links": ["Rust学习历程"]
    },
    {
      "note_title": "Helix编辑器",
      "action": "create",
      "new_facts": ["Helix 是基于 Kakoune 的现代终端编辑器"],
      "category": "tool",
      "tags": ["editor", "terminal"],
      "links": ["编辑器偏好"]
    }
  ]
}
```

**Actions:**
- `create` — write new markdown file with frontmatter + facts + links
- `append` — read existing file, append new facts, update wikilinks, bump `updated` date
- `update` — replace specific facts (for corrections/invalidation)

**LLM decides:**
- Whether to create a new note or append to existing
- Note title and granularity
- Which wikilinks to add
- Category and tags

## LLM Memory Injection Flow

```
User message → embedding → notes_vec semantic search → top-K note filenames
→ read markdown file contents → inject into system prompt
```

Unchanged in principle from current flow. Only the storage backend changes (SQLite rows → file reads).

**Additional retrieval path via wikilinks:**
After finding top-K notes, optionally follow 1-hop wikilinks to pull related notes for richer context. This is a new capability enabled by the graph structure.

## Canvas Visualization

### Data Source

```
graph.query  → SELECT filename, category, link_count FROM notes_index
               + SELECT from_note, to_note FROM notes_links
graph.neighbors → follow notes_links from a center node up to depth N
graph.node_detail → read markdown file content + linked notes
graph.search → notes_fts full-text search
```

### Node Rendering (Obsidian Native Style)

- **Shape:** Small filled circle with glow (box-shadow)
- **Color:** Single purple tone (`#a78bfa`), glow `rgba(167,139,250,0.5)`
- **Size:** Scaled by wikilink count — `base_radius + ln(link_count + 1) * scale_factor`
- **Label:** Note title (filename), displayed below the dot, white text
- **No emoji icons, no category coloring**

### Edge Rendering

- Thin lines connecting linked notes
- Default alpha 0.15, highlighted alpha 0.6
- Relation label at midpoint (optional, from wikilink context)

### Interaction (Obsidian Style)

| Action | Behavior |
|--------|----------|
| Click node | Select node, right panel shows note content (read-only initially) |
| Double-click node | Enter Local View — center on node, show 2-hop neighbors |
| Hover node | Highlight node + all directly connected edges and neighbors |
| Drag node | Move node position (temporarily pinned) |
| Scroll wheel | Zoom in/out at cursor position |
| Click background | Deselect current node |
| Esc / breadcrumb "All" | Return to Global View |

### New Canvas Features Required

1. **Click-to-center animation** — smooth pan/zoom to center selected node (~300ms ease-out)
2. **Hover neighbor highlighting** — connected nodes brighten, unconnected nodes dim (alpha 0.3)
3. **Continuous drift** — layout never fully converges; add small random perturbation to prevent static graph
4. **Focus dimming** — when a node is selected, unrelated nodes fade to alpha 0.2

### Layout Changes

- Keep existing `ForceLayout` with repulsion/attraction
- Add minimum energy threshold to prevent full convergence (continuous drift)
- Reduce repulsion_strength for smaller node counts (notes vs raw facts)

## Migration Strategy

```
Phase 1: Notes infrastructure
  - Create ~/.aleph/notes/ directory
  - Implement NoteStore trait (file read/write/rename/delete)
  - Implement SQLite index tables (notes_index, notes_vec, notes_fts, notes_links)
  - Implement indexer (full rebuild + incremental)

Phase 2: Data migration
  - Group existing facts by path/category into Knowledge Notes
  - Generate markdown files from grouped facts
  - Extract wikilinks from fact-entity relationships (memory_entities)
  - Verify: all facts accounted for, all entity relationships preserved as wikilinks

Phase 3: LLM extraction refactor
  - Modify FactExtractor to output notes_updates instead of {facts, entities, relationships}
  - Update extraction prompt template
  - Wire file write + incremental re-index on extraction completion

Phase 4: Canvas overhaul
  - Rewrite graph.rs handlers to query notes_index + notes_links
  - Update adapter.rs DTOs for note-based data
  - Implement Obsidian-style node rendering (replace current colored circles)
  - Add click-to-center, hover highlighting, continuous drift, focus dimming

Phase 5: Cleanup
  - Remove facts, graph_nodes, graph_edges, memory_entities tables
  - Remove src/memory/store/sqlite/graph.rs
  - Remove src/memory/context/fact.rs entity-related code
  - Remove FactExtractor's entity/relationship extraction
  - Update all references in gateway handlers, memory retrieval, etc.
```

## Risks and Mitigations

| Risk | Mitigation |
|------|------------|
| File I/O slower than SQLite for retrieval | SQLite index handles search; file reads only for injection (small files, fast) |
| Concurrent file writes during extraction | Single-writer model — extraction runs sequentially per session |
| Wikilink target doesn't exist yet | Create placeholder note on first reference; LLM will fill later |
| Migration loses data | Phase 2 includes verification step; keep old tables until confirmed |
| Too many notes overwhelm Canvas | Same as Obsidian — toolbar filter by category/tag, search to jump |
| Filename collisions | Sanitize titles, append numeric suffix on collision |

## Non-Goals

- **Note editing in Canvas** — right panel is read-only initially; editing is a future enhancement
- **Real-time collaboration** — single-user system
- **File sync (iCloud/Dropbox)** — out of scope; notes are local to ~/.aleph/
- **Obsidian plugin compatibility** — inspired by, not compatible with Obsidian vault format
