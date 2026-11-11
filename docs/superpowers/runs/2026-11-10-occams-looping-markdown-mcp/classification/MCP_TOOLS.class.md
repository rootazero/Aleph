# Classification & Routing Matrix: MCP_TOOLS Batch

## Executive Summary

The `MCP_TOOLS` batch comprises core subsystems responsible for connecting Model Context Protocol (MCP) servers, bridging dynamic tool registrations into `ToolHandlerRegistry`, injecting untrusted resource and tool metadata into sampling context, validating and sanitizing tool argument schemas against strict provider constraints, and handling server-initiated LLM sampling requests.

Under Occam's Razor principles and strict adherence to the project constitution, this subsystem has been statically audited. The components are exceptionally well-engineered, featuring robust defense-in-depth mechanisms against prompt injection, recursion depth limits on schema parsing (`MAX_SCHEMA_DEPTH`), payload truncation caps (`MAX_DESCRIPTION_BYTES`), and generation-guarded scoping (`ToolRegistrationScope`).

No critical anti-patterns or logic bugs were identified. Minor cleanups, such as leveraging `Arc::clone` ergonomics or avoiding redundant string allocations, are evaluated as **DEFER** or **REJECT** because the current implementations prioritize explicit readability and future-proofing without violating performance budgets or core invariants.

---

## Classification & Routing Matrix

| Location | Taxonomy | Smell Description | Route | Action |
|---|---|---|---|---|
| [`src/mcp/context_injector.rs`](file:///home/zou/data/workspace/Aleph/src/mcp/context_injector.rs#L133-L208) | Redundant Allocation | `parts.iter().map(String::len).sum::<usize>()` during formatting. | **DEFER** | Retain explicit sizing logic; guarantees pre-allocated capacity without premature optimization. |
| [`src/mcp/sampling.rs`](file:///home/zou/data/workspace/Aleph/src/mcp/sampling.rs#L98-L118) | Concurrency / Lock Holding | Read lock clone released before await. | **REJECT** | Behavior is already optimal; correctly prevents deadlock/contention. |
| [`src/mcp/sampling_bridge.rs`](file:///home/zou/data/workspace/Aleph/src/mcp/sampling_bridge.rs#L101-L139) | Error Semantics | Fallback handling for missing provider. | **REJECT** | Fails closed correctly with descriptive error. |
| [`src/mcp/tool_bridge.rs`](file:///home/zou/data/workspace/Aleph/src/mcp/tool_bridge.rs#L110-L176) | Event Loop | Broadcast lag handling with full resync. | **REJECT** | Essential reliability mechanism; correct recovery pattern. |
| [`src/mcp/tool_sanitize.rs`](file:///home/zou/data/workspace/Aleph/src/mcp/tool_sanitize.rs#L88-L104) | Security / DoS | Schema depth traversal capped at `MAX_SCHEMA_DEPTH = 32`. | **REJECT** | Correct defense-in-depth against stack overflow / DoS. |

---

## Invariant Safety Pre-Flight

1. **Tool bridge execution, metadata translation, and error wrapping**: Preserved. `ToolRegistrationScope` and `ToolHandlerRegistry` integrations remain exact.
2. **Tool argument sanitization and payload security**: Preserved. `normalize_tool_schema` and `scan_description_for_injection` maintain robust defense against strict provider validation failures and prompt injection heuristics.
3. **Sampling protocol request / response schema**: Preserved. `SamplingRequest` / `SamplingResponse` parsing, context injection, and system prompt boundary wrapping (`<server-injected>`) remain fully intact.
