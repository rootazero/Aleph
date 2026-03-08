# Memory Storage Cleanup Design

> Remove redundant storage layers, simplify to LanceDB + MEMORY.md dual-channel architecture.

**Date**: 2026-03-08
**Status**: Implemented

---

## Problem

The memory system had 4 overlapping storage mechanisms:

| Layer | Location | Purpose | Action |
|-------|----------|---------|--------|
| LanceDB facts + raw memories | `~/.aleph/data/memory.lance` | Vector search, compression, retrieval | **Keep** |
| SQLite sessions | `~/.aleph/data/sessions.db` | Session state, conversation tracking | **Keep** |
| JSONL session files | `agents/{id}/sessions/*.jsonl` | Duplicate of SQLite sessions | **Removed** |
| Daily memory notes | `workspaces/{id}/memory/*.md` | Daily conversation summaries | **Removed** |

## Changes Implemented

### 1. Removed JSONL Session Storage

Deleted `session_storage.rs` entirely (408 lines). Removed `SessionStorage` from `AgentInstance`. SQLite `sessions.db` is the sole session store.

### 2. Removed Daily Memory Notes (`memory/*.md`)

- Removed `append_daily_memory()` and `load_recent_memory()` from `workspace_loader.rs`
- Removed `DailyMemory` type from `memory_context.rs`
- Removed daily notes injection from `execution_engine.rs` (both extra_instructions and memory_context paths)
- Removed `memory/` directory creation from `initialize_workspace()`

### 3. MEMORY.md — Manual-Edit Only (No LanceDB Sync)

MEMORY.md is a free-format, user-editable memory file. It is NOT synced to LanceDB.

- **Automated memory** (LanceDB facts) = Aleph's compression service handles this automatically
- **Manual memory** (MEMORY.md) = User writes whatever they want, injected as-is into LLM context

No format restrictions. No sync service. No file watcher needed.

### Post-Cleanup Architecture

```
Write paths:
  Conversation → compression service → LanceDB (facts + raw memories)
  User edits  → MEMORY.md (free format, no sync)

Read paths (parallel, non-conflicting):
  LLM call → MemoryAugmentationLayer (LanceDB vector search) → system prompt
  LLM call → WorkspaceFilesLayer (MEMORY.md raw text)         → system prompt
```
