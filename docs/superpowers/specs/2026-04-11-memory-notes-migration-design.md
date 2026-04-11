# Memory Data Layer: Facts → Notes Migration (Medium Refactoring)

**Date:** 2026-04-11
**Status:** Approved
**Scope:** Medium — switch compression default to notes, panel fully on notes, facts retained as session store

## Problem

The memory system has two parallel storage layers:
- **facts table** — SQLite-first, stores both raw session summaries and extracted knowledge
- **notes system** — Markdown-first (`~/.aleph/memory/note/{agent_id}/{category}/*.md`) with SQLite index

After renaming facts→notes at the concept level, the data layer still routes through the facts table. The panel displays stale data, stats are wrong, and the terminology is inconsistent.

## Architecture (After)

```
User conversation
    ↓
SessionCompactor → facts table (session store, unchanged)
    ↓
CompressionService.compress_to_notes()  ← DEFAULT path
    ↓
NoteIndexer → markdown files + notes_index + notes_fts + notes_vec
    ↓
Panel stats/list/graph ← notes_index
MemoryRetrieval ← NoteRetrieval (primary) → FactRetrieval (fallback)
```

### Two-Layer Model

| Layer | Name | Storage | Index | Purpose |
|-------|------|---------|-------|---------|
| L1 | Raw Memory | `~/.aleph/memory/raw/{agent_id}/` + facts table (session data) | facts table (session_compressed rows) | Conversation records, session summaries, uploaded files |
| L2 | Note Memory | `~/.aleph/memory/note/{agent_id}/{category}/*.md` | notes_index + notes_links + notes_fts + notes_vec | Compiled knowledge, wikilinked markdown |

## Changes

### 1. CompressionService — Default to Notes Path

**File:** `src/memory/compression/service.rs`

- `compress_in_workspace()` delegates to `compress_to_notes()` as default
- Old `insert_fact()` path retained behind config flag for rollback
- Input source unchanged: reads `session_compressed` facts from facts table
- Output: NoteIndexer writes markdown + indexes

### 2. Gateway Handlers — Notes-Only Stats and Lists

**File:** `src/gateway/handlers/memory.rs`

| Endpoint | Before | After |
|----------|--------|-------|
| `memory.stats` → `totalFacts` | `count_compressed_facts()` (facts table) | `count_all_notes()` (notes_index) |
| `memory.stats` → `totalMemories` | `count_raw_memories()` (facts table) | `count_raw_memories()` (unchanged, session store) |
| `memory.stats` → `totalGraphNodes` | Hardcoded 0 | `get_graph_data()` from notes_index |
| `memory.listFacts` | `get_compressed_facts()` (facts table) | `list_notes()` (notes_index) |

**Status:** Already implemented in this session.

### 3. Graph Handlers — Agent ID Alignment

**File:** `src/gateway/handlers/graph.rs`

- All hardcoded `"default"` → `crate::routing::DEFAULT_AGENT_ID` ("main")

**Status:** Already implemented in this session.

### 4. Startup Note Index Rebuild

**File:** `src/bin/aleph-server/commands/start/mod.rs`

- `tokio::spawn` runs `NoteIndexer::full_rebuild()` at startup
- Scans `~/.aleph/memory/note/{agent_id}/{category}/*.md`
- Indexes into notes_index, notes_links, notes_fts

**Status:** Already implemented in this session.

### 5. NoteStore — Count All Notes

**Files:** `src/memory/notes/store.rs`, `src/memory/store/sqlite/notes.rs`

- New method: `count_all_notes()` — `SELECT COUNT(*) FROM notes_index` (cross-agent)

**Status:** Already implemented in this session.

## Not Changed (Deferred)

| Module | Reason | Future Task |
|--------|--------|-------------|
| Dream daemon (6 stages) | Complex, high risk | Separate spec: dream→notes migration |
| Session compactor | Still writes facts as session store | Migrate when raw storage redesigned |
| Hybrid retrieval | MemoryRetrieval already has notes-first fallback | Migrate tools to notes search |
| Memory tools (search/browse) | Depend on HybridRetrieval → facts | After retrieval migration |
| Event sourcing | Orthogonal to storage layer | Adapt events to notes mutations |
| facts table DDL | Retained as session store | Archive after full migration |

## Data Flow

```
                    ┌──────────────────────┐
                    │   User Conversation  │
                    └──────────┬───────────┘
                               ↓
                    ┌──────────────────────┐
                    │  Session Compactor   │
                    │  (writes to facts    │
                    │   as session store)  │
                    └──────────┬───────────┘
                               ↓
                    ┌──────────────────────┐
                    │ Compression Service  │
                    │ compress_to_notes()  │←── DEFAULT
                    └──────────┬───────────┘
                               ↓
              ┌────────────────┼────────────────┐
              ↓                ↓                ↓
      ┌──────────────┐ ┌─────────────┐ ┌──────────────┐
      │  Markdown    │ │ notes_index │ │  notes_fts   │
      │  files       │ │ notes_links │ │  notes_vec   │
      └──────┬───────┘ └──────┬──────┘ └──────┬───────┘
             │                │                │
             └────────────────┼────────────────┘
                              ↓
              ┌───────────────────────────────┐
              │  Panel (stats/list/graph)     │
              │  MemoryRetrieval (primary)    │
              └───────────────────────────────┘
                              ↓ (fallback)
              ┌───────────────────────────────┐
              │  FactRetrieval (legacy)       │
              │  Dream daemon (unchanged)     │
              └───────────────────────────────┘
```

## Remaining Work

What's already done in this session:
- [x] `handle_stats` queries notes_index
- [x] `handle_list_facts` returns notes_index data
- [x] `graph.rs` agent_id alignment
- [x] Startup note index rebuild
- [x] `count_all_notes()` method
- [x] Cleared old residual facts/graph data
- [x] Created seed note memories (2 files)

What still needs implementation:
- [ ] Switch `compress_in_workspace()` to default to `compress_to_notes()`
- [ ] Verify compression pipeline produces correct notes output
- [ ] Test end-to-end: conversation → session compactor → compression → notes → panel display
