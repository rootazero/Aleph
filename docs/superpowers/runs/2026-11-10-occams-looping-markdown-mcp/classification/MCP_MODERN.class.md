# Classification & Routing Matrix: Batch MCP_MODERN (`src/mcp/modern`)

## Executive Summary

Batch `MCP_MODERN` (`src/mcp/modern/`) implements the stateless `2026-07-28` Model Context Protocol (MCP) wire shape, supporting:
- Dual-era detection and version negotiation (`mod.rs`, `discover.rs`)
- Per-request metadata attachment (`mod.rs`)
- Cache directive extraction and freshness rules (`cache.rs`)
- Streamable HTTP metadata headers and tool parameter mirroring with robust schema validation and depth capping (`headers.rs`)
- Multi Round-Trip Requests (MRTR) for server-initiated input/sampling (`mrtr.rs`)

### Invariant Safety Pre-Flight
1. **Endpoint Discovery & Capability Negotiation Contracts**: Strictly preserved. `server/discover` parsing and `select_version` correctly distinguish modern, dual-era legacy fallback, and unsupported future versions.
2. **MRTR Header Parsing & Transport Protocol Compliance**: Strictly preserved. `headers.rs` correctly mirrors method, name, and parameter headers with base64 sentinel escaping (`=?base64?…?=`) and strict RFC 9110 token validation. `mrtr.rs` implements MRTR request parsing, independent request IDs on retry, and opaque `requestState` echo.
3. **Cache Eviction & Freshness Rules**: Strictly preserved. `cache.rs` correctly extracts `ttlMs`, clamps excessive values to `MAX_TTL` (24h), and handles missing/zero/malformed TTLs as absent.

---

## Classification & Routing Matrix

| Location | Taxonomy | Smell Description | Route | Action |
|---|---|---|---|---|
| [`mod.rs`](file:///home/zou/data/workspace/Aleph/src/mcp/modern/mod.rs) | Idiomatic Rust / Const Evaluation | `is_modern_error` correctly uses explicit bounds check because `RangeInclusive::contains` is not yet const in MSRV. | `DEFER` | None (Optimal as-is). |
| [`cache.rs`](file:///home/zou/data/workspace/Aleph/src/mcp/modern/cache.rs) | Defensive Bounding | `MAX_TTL` correctly clamps malicious/erroneous multi-year TTLs. | `DEFER` | None (Optimal as-is). |
| [`headers.rs`](file:///home/zou/data/workspace/Aleph/src/mcp/modern/headers.rs#L178-L309) | Recursion Safety / Stack Protection | `walk_schema` enforces `MAX_SCHEMA_DEPTH = 32` to prevent stack overflow on hostile nested schemas. | `DEFER` | None (Optimal as-is). |
| [`mrtr.rs`](file:///home/zou/data/workspace/Aleph/src/mcp/modern/mrtr.rs#L180-L203) | Error Semantics & Fast Failure | `fulfill` validates request methods against declared capabilities and fails fast with descriptive error naming the method and server. | `DEFER` | None (Optimal as-is). |

---

## Conclusion
No refactoring or architectural modifications are required for Batch `MCP_MODERN`. The code adheres strictly to Occam's Razor, avoiding redundant allocations, unneeded indirection, and dead code while fully safeguarding all required protocol invariants.
