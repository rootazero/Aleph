# Task 4 — projection and notification boundaries

## Implemented

Added documentation markers at the existing seams:

- `src/acp/manager/persistence.rs`: ACP session JSON is a projection-only protocol/session representation, not authorization, registry, or recovery source of truth.
- `src/resilience/database/state_database/mod.rs`: `StateDatabase` is an operational query/metrics projection, not capability authorization or session recovery truth.
- `src/mcp/types.rs`: MCP wire/catalog/result types are transport/discovery projections and never grant authorization.
- `src/tools/mcp_scope_view.rs`: the scoped MCP service is layered under the parent authorization gate and does not create a second registry.
- `src/tools/server/ops.rs`: `list_tools_arc_impl` is a live tool-map view, not a registry or authorization source.
- `src/event/global_bus.rs`: `GlobalBus` is notification-only; callbacks are not persistence, authorization, or recovery state.
- `src/tools/service.rs`: `to_metadata_form` remains the real pure legacy compatibility wrapper with its existing signature and direct field projection; it does not provide canonical identity or registry state.

These are documentation contracts. They do not add runtime registries, alter authorization, or change source-of-truth behavior. Existing metadata wrapper tests already provide meaningful purity and parity coverage.

## Verification

- `cargo test -p alephcore --lib capability::facade --no-fail-fast`: 5 passed.
- `cargo test -p alephcore --lib tools::service::metadata_form_tests --no-fail-fast`: 6 passed.
- `cargo check -p alephcore`: passed.
- `git diff --check`: passed.
- No changes under `src/mcp/tool_bridge.rs` or `src/harness/`.
