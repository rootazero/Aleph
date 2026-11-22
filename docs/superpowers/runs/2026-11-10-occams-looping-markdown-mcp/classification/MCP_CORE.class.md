# Classification & Routing Matrix: MCP_CORE

## Executive Summary
This document provides the classification and routing matrix for `MCP_CORE` (`src/mcp/{client.rs, error_class.rs, jsonrpc.rs, preflight.rs, presets/mod.rs, prompts.rs, protocol.rs, redact.rs, resources.rs, types.rs}`). Under Occam's Razor principles, our mission is to eliminate unnecessary complexity, dead code, redundant allocations, over-nested control flow, and semantic bugs, while strictly preserving:
1. MCP JSON-RPC 2.0 wire formats, request IDs, error codes.
2. Secret redaction patterns (passwords, tokens).
3. Preflight check constraints and security boundaries.

## Classification & Routing Matrix

| Location | Taxonomy | Smell Description | Route | Action |
| --- | --- | --- | --- | --- |
| [`src/mcp/client.rs`](file:///home/zou/data/workspace/Aleph/src/mcp/client.rs) | Redundant allocation / Clone | Repeated clones in tool listing and index rebuilding | DEFER | Low severity, clean up in broader refactor if needed |
| [`src/mcp/error_class.rs`](file:///home/zou/data/workspace/Aleph/src/mcp/error_class.rs) | Dead code / Unused match arms | Unused error category mappings | FIX-CAREFULLY | Prune dead variants without changing public error API |
| [`src/mcp/jsonrpc.rs`](file:///home/zou/data/workspace/Aleph/src/mcp/jsonrpc.rs) | Over-nested control flow | Deep JSON-RPC parsing match chains | DEFER | Preserves wire format strictly |
| [`src/mcp/preflight.rs`](file:///home/zou/data/workspace/Aleph/src/mcp/preflight.rs) | Redundant checks | Duplicate or overly complex environment checks | FIX-NOW | Simplify condition checks without relaxing security boundaries |
| [`src/mcp/presets/mod.rs`](file:///home/zou/data/workspace/Aleph/src/mcp/presets/mod.rs) | Dead code | Unused preset definitions | DEFER | Keep for backwards compatibility of preset loading |
| [`src/mcp/prompts.rs`](file:///home/zou/data/workspace/Aleph/src/mcp/prompts.rs) | Redundant allocations | String formatting overhead in prompt building | FIX-CAREFULLY | Use string slices / cow where appropriate |
| [`src/mcp/protocol.rs`](file:///home/zou/data/workspace/Aleph/src/mcp/protocol.rs) | Complexity / Indirection | Large state machine match blocks | DEFER | Core protocol stability required |
| [`src/mcp/redact.rs`](file:///home/zou/data/workspace/Aleph/src/mcp/redact.rs) | Redundant regex/alloc | Repeated regex compilation or inefficient string scanning | FIX-NOW | Optimize sanitization loops while maintaining secret redaction patterns |
| [`src/mcp/resources.rs`](file:///home/zou/data/workspace/Aleph/src/mcp/resources.rs) | Stub/Dead code | Minimal resource stub methods | DEFER | Required trait compliance |
| [`src/mcp/types.rs`](file:///home/zou/data/workspace/Aleph/src/mcp/types.rs) | Redundant derives/clones | Extra clone/debug traits or verbose builders | DEFER | Part of public API surface |

## Invariant Safety Pre-Flight
- **MCP JSON-RPC 2.0**: All JSON-RPC structures maintain wire compatibility, preserving `jsonrpc: "2.0"`, ID types, and standard error codes (`-32700` through `-32600`).
- **Secret Redaction**: Redaction patterns in `redact.rs` continue to match credential keys, bearer tokens, API keys, and password fields completely.
- **Preflight Security**: Preflight checks retain all sandbox boundaries, command injection protections, and runtime existence verifications.
