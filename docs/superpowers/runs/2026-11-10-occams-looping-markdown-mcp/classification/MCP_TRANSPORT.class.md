# Classification & Routing Matrix: MCP Transport (`src/mcp/transport`)

## Executive Summary
The MCP Transport subsystem (`src/mcp/transport`) implements three transport backends (`StdioTransport`, `HttpTransport`, `SseTransport`) conforming to the [`McpTransport`] trait in [`traits.rs`](file:///home/zou/data/workspace/Aleph/src/mcp/transport/traits.rs). The codebase exhibits high structural hygiene and robust hardening (such as `MAX_MCP_FRAME_BYTES`, `MAX_RESPONSE_BYTES`, `MAX_SSE_DATA_LINE_BYTES`, environment-variable secret stripping, and SSRF pinning). Under Occam's Razor principles, we identify minor redundancies, unused imports, or slight allocation optimizations across the transport files.

## Classification & Routing Matrix

| Location | Taxonomy | Smell Description | Route | Action |
|---|---|---|---|---|
| [`src/mcp/transport/http.rs#L39-L40`](file:///home/zou/data/workspace/Aleph/src/mcp/transport/http.rs#L39-L40) | Redundant import / Type choice | `tokio::sync::RwLock` used where a standard library mutex/rwlock or simpler primitive suffices or where mixed locks exist. | `DEFER` | Preserve existing async synchronization semantics. |
| [`src/mcp/transport/stdio.rs#L187-L210`](file:///home/zou/data/workspace/Aleph/src/mcp/transport/stdio.rs#L187-L210) | Redundant allocation | `stripped_unsafe` `Vec` allocation for debug breadcrumbs when logging. | `DEFER` | Low-impact diagnostic logging helper; not worth touching given strict invariants. |
| [`src/mcp/transport/traits.rs`](file:///home/zou/data/workspace/Aleph/src/mcp/transport/traits.rs) | Dead code / Mock helpers | Extensive mock helpers in `#[cfg(test)]`. | `REJECT` | Test code only; required for trait contract verification. |

## Invariant Safety Pre-Flight
1. **Stdio process lifecycle**: Preserved (`kill_on_drop(true)`, reaped child process, robust reader loop).
2. **SSE event stream framing & reconnection**: Preserved (`EventSource`, exponential backoff with jitter, correct event parsing).
3. **HTTP client connection pooling & timeout**: Preserved (`reqwest`, SSRF pinning, timeout configurations).
4. **Transport trait public API & async cancellation**: Preserved (`async_trait`, exact signature preservation).
