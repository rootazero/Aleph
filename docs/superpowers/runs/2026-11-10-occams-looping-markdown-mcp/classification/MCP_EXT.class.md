# Classification & Routing Matrix: MCP_EXT

## Executive Summary
This audit covers `src/mcp/external/` (`connection.rs`, `mod.rs`, `runtime.rs`) under Occam's Razor principles, focusing on eliminating redundant allocations, unneeded indirection, dead code, and over-nested control flow, while strictly preserving process lifecycle handling (stdio piping, child process kill-on-drop), connection reconnect state transitions, and async cancellation safety.

## Classification & Routing Matrix

| Location | Taxonomy | Smell Description | Route | Action |
| :--- | :--- | :--- | :--- | :--- |
| [`connection.rs`](file:///home/zou/data/workspace/Aleph/src/mcp/external/connection.rs#L30-L32) | Redundant Allocation | `strip_server_prefix` allocates a temporary `String` via `format!` every time it runs, even though matching can be done via string slicing or `strip_prefix`. | `FIX-NOW` | Replace `format!` with `format_args!` or slice comparison to avoid heap allocation on the hot path. |
| [`connection.rs`](file:///home/zou/data/workspace/Aleph/src/mcp/external/connection.rs#L102-L104) | Redundant Allocation | `fingerprint` serializes items to a `String` unconditionally. | `DEFER` | Used for change detection; performance impact is low and type erasure via string is intentional. |
| [`runtime.rs`](file:///home/zou/data/workspace/Aleph/src/mcp/external/runtime.rs#L160-L179) | Indirection / Control Flow | `get_runtime_path` spawns a separate `which`/`where` process separately from the main version check. | `DEFER` | While spawning `which` adds overhead, it cleanly decouples version checking from executable path resolution and respects cross-platform differences (`where` vs `which`). |

## Invariant Safety Pre-Flight
1. **Child process spawning, stdio piping, process termination and kill on drop**: Preserved. We do not modify transport creation or process drop guards.
2. **Connection reconnect and keepalive state transitions**: Preserved. Handshake, era probing, and list refresh flows remain intact.
3. **Lock order and async cancellation safety**: Preserved. No changes to lock acquisition order or critical sections.
