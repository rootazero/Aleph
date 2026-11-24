# Classification & Routing Matrix: MCP_MGR (`src/mcp/manager`)

## Executive Summary

The MCP Manager (`src/mcp/manager`) orchestrates multi-server MCP connections via an actor pattern (`McpManagerActor`, `McpManagerHandle`, `McpCommand`, `McpManagerEvent`), configuration persistence (`McpPersistentConfig`), secret resolution (`SecretResolver`), and robust status/health models. 

Across 3,537 lines of Rust code, the implementation is generally structured and robust, but exhibits several Occam's Razor anti-patterns: redundant clones/allocations, overly nested control flow in event matching and config parsing, unused imports, and minor dead code paths.

This audit classifies findings into precise routing categories (`FIX-NOW`, `FIX-CAREFULLY`, `DEFER`, `REJECT`) while strictly preserving:
1. Actor loop message dispatch, handle cloning, and shutdown cascade.
2. Config serialization / deserialization contracts.
3. Secret resolution and redaction integrity.

---

## Classification & Routing Matrix

| Location | Taxonomy | Smell Description | Route | Action |
| :--- | :--- | :--- | :--- | :--- |
| [`src/mcp/manager/actor.rs`](file:///home/zou/data/workspace/Aleph/src/mcp/manager/actor.rs) | Redundant Allocation / Clone | Unnecessary `.clone()` calls on primitives/strings during command dispatch and status queries. | `FIX-NOW` | Remove redundant clones to streamline allocations. |
| [`src/mcp/manager/config.rs`](file:///home/zou/data/workspace/Aleph/src/mcp/manager/config.rs) | Over-nested Control Flow | Deeply nested `match` / `if let` chains when parsing env/secret overrides and persistent configs. | `FIX-NOW` | Flatten control flow using early returns/combinators (`map`, `and_then`). |
| [`src/mcp/manager/handle.rs`](file:///home/zou/data/workspace/Aleph/src/mcp/manager/handle.rs) | Dead Code / Unused Imports | Unused imports and helper wrappers that duplicate channel error mappings. | `FIX-NOW` | Clean up unused imports and streamline error mapping helpers. |
| [`src/mcp/manager/secret_resolver.rs`](file:///home/zou/data/workspace/Aleph/src/mcp/manager/secret_resolver.rs) | Indirection / Redundancy | Redundant string matching passes during secret placeholder expansion. | `FIX-NOW` | Simplify regex or string replacement logic. |
| [`src/mcp/manager/types.rs`](file:///home/zou/data/workspace/Aleph/src/mcp/manager/types.rs) | Redundant Serialization Helpers | Duplicate derivation boilerplate or manual formatting functions for display states. | `DEFER` | Retain current serialization contracts to avoid breaking wire/config formats. |

---

## Invariant Safety Pre-Flight

1. **Actor Loop Message Dispatch & Shutdown Cascade**: Preserved by keeping all `McpCommand` variants, channel semantics, and oneshot response paths structurally identical.
2. **Config Serialization / Deserialization Contracts**: Preserved; no JSON fields, struct field names, or default serde attributes are altered.
3. **Secret Resolution & Redaction Integrity**: Preserved; environment variable expansion and secret masking algorithms remain behaviorally identical.
