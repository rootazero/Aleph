# Change: Refactor core.rs into Rust 2018+ Module Structure

## Why

The `core.rs` file has grown to **4492 lines**, containing multiple distinct functional areas tightly coupled in a single file. This violates Rust's module organization best practices and makes the codebase difficult to navigate, test, and maintain. Adopting Rust 2018+ module structure (`core/` directory with submodules) improves code organization and enables better separation of concerns.

## What Changes

### File Structure Changes

**Current Structure:**
```
src/
├── core.rs           (4492 lines - everything in one file)
├── config/
├── mcp/
├── ...
```

**New Structure:**
```
src/
├── core.rs           (only re-exports: pub mod x;)
├── core/
│   ├── mod.rs        (AlephCore struct + new() + core logic)
│   ├── types.rs      (MediaAttachment, CapturedContext, CompressionStats, etc.)
│   ├── memory.rs     (Memory-related methods: store, retrieve, cleanup, compression)
│   ├── config_ops.rs (Config management: load, update, reload_router)
│   ├── mcp_ops.rs    (MCP capability methods: list_mcp_servers, add_mcp_server, etc.)
│   ├── search_ops.rs (Search capability methods: test_search_provider, update_search_config)
│   ├── tools.rs      (Dispatcher/Tool registry methods: list_tools, refresh_tools)
│   ├── conversation.rs (Conversation management: start, continue, end)
│   ├── processing.rs (AI processing pipeline: process_input, build_enriched_payload)
│   └── tests.rs      (Unit tests moved from core.rs)
```

### Code Movement Summary

| Module | Lines (approx) | Functions Moved |
|--------|----------------|-----------------|
| `types.rs` | ~100 | `MediaAttachment`, `CapturedContext`, `CompressionStats`, `RequestContext`, `StorageHelper` |
| `memory.rs` | ~400 | `store_interaction_memory`, `retrieve_and_augment_*`, `cleanup_old_memories`, `trigger_compression`, `get_memory_stats`, `search_memories`, etc. |
| `config_ops.rs` | ~350 | `load_config`, `update_provider`, `delete_provider`, `update_routing_rules`, `reload_router`, `update_shortcuts`, `update_behavior`, etc. |
| `mcp_ops.rs` | ~500 | `get_mcp_config`, `update_mcp_config`, `list_mcp_servers`, `add_mcp_server`, `update_mcp_server`, `delete_mcp_server`, etc. |
| `search_ops.rs` | ~200 | `update_search_config`, `test_search_provider`, `create_search_registry_from_config` |
| `tools.rs` | ~200 | `refresh_tool_registry`, `list_tools`, `search_tools`, `list_unified_tools`, confirmation methods |
| `conversation.rs` | ~250 | `start_conversation`, `continue_conversation`, `end_conversation`, `has_active_conversation` |
| `processing.rs` | ~700 | `process_input`, `process_with_ai_first`, `build_enriched_payload`, `execute_capability_and_continue`, routing context builders |
| `tests.rs` | ~200 | All `#[cfg(test)]` test functions |
| `mod.rs` | ~1500 | `AlephCore` struct, `new()`, helper methods, remaining logic |

### API Stability

- **NO breaking API changes** - All public functions retain exact signatures
- **NO UniFFI changes** - `aleph.udl` remains unchanged
- **Internal reorganization only** - Module paths change but public interface stable

## Impact

- **Affected specs**: `core-library`
- **Affected code**: `Aleph/core/src/core.rs` → split into `Aleph/core/src/core/` directory
- **Risk level**: Low (pure refactor, no logic changes)
- **Testing**: All existing tests must pass; module paths may need updates in test imports
