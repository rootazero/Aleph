# Agent-Aware Memory System

**Date**: 2026-03-08
**Status**: Approved

## Problem

Memory (raw memories and compressed facts) is stored globally with `workspace = "default"`. No agent isolation exists — all agents share the same memory pool with no way to distinguish which agent produced or should see which memories.

## Decision

Repurpose the existing `workspace` field as the agent isolation key. Set `workspace = agent_id` on write, filter by workspace on read. Zero schema migration — the LanceDB column already exists.

## Changes

### Write Path

1. **`write_conversation_memory()`** (engine.rs) — extract `agent_id` from `session_key`, pass to function, set `entry.workspace = agent_id`.
2. **`CompressionService`** — verify `compress_in_workspace()` receives agent_id (not "default") as workspace_id.
3. **`MemoryEntry::new()` / `with_embedding()`** — change default workspace from `"default"` to `"main"`.

### Read Path

4. **`SearchParams`** — add optional `agent_id` field, map to `MemoryFilter.workspace = WorkspaceFilter::Single(agent_id)`.
5. **`ListFactsParams`** — add optional `agent_id` field. Extend `get_all_facts()` trait method to accept optional workspace filter.
6. **Response structs** — `MemoryEntry` and `FactEntry` (handler-level) gain `agent_id` field, populated from `workspace`.

### Data Migration

7. On server startup, after LanceDB init, run idempotent migration:
   - `UPDATE memories SET workspace = 'main' WHERE workspace = 'default'`
   - `UPDATE facts SET workspace = 'main' WHERE workspace = 'default'`

### Panel UI

8. **API types** — `RawMemory` and `CompressedFact` gain `agent_id: String`.
9. **Memory view** — both tabs display "Agent" column with Badge component.

## Files

| File | Change |
|------|--------|
| `core/src/gateway/execution_engine/engine.rs` | Pass agent_id to write_conversation_memory |
| `core/src/memory/compression/service.rs` | Verify workspace_id = agent_id |
| `core/src/gateway/handlers/memory.rs` | Add agent_id to params and response structs |
| `core/src/memory/store/lance/facts.rs` | get_all_facts supports workspace filter |
| `core/src/memory/store/mod.rs` | Trait signature update |
| `core/src/memory/context/mod.rs` | Default workspace "default" → "main" |
| `core/src/bin/aleph/commands/start/mod.rs` | Startup migration |
| `apps/panel/src/api.rs` | Add agent_id to types |
| `apps/panel/src/views/memory.rs` | Add Agent column |
